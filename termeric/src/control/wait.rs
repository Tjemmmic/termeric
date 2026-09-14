//! Client-only child waiting. No GUI worker, terminal owner, or mutation is held.

use super::{
    ControlOutput, client,
    paths::ControlPaths,
    protocol::{self, ClientFrame, LaunchState, Request, ServerFrame},
};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt, net::UnixStream};
use std::path::Path;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(250);

pub(super) fn launch(
    process_id: u32,
    launch_request_id: String,
    timeout_seconds: Option<u32>,
    json: bool,
) -> anyhow::Result<ControlOutput> {
    let started = Instant::now();
    Request::LaunchStatus {
        launch_request_id: launch_request_id.clone(),
    }
    .validate()
    .map_err(anyhow::Error::msg)?;
    if process_id == 0
        || process_id > i32::MAX as u32
        || timeout_seconds.is_some_and(|seconds| !(1..=86400).contains(&seconds))
    {
        anyhow::bail!("invalid wait-launch process ID or timeout");
    }
    let deadline = timeout_seconds.map(|seconds| started + Duration::from_secs(seconds.into()));
    let endpoint = ControlPaths::discover_all()?
        .into_iter()
        .find(|endpoint| endpoint.process_id == process_id)
        .ok_or_else(|| anyhow::anyhow!("GUI process {process_id} was not found"))?;
    let token = endpoint.read_token()?;
    let identity = socket_identity(&endpoint.socket)?;
    loop {
        let exchange_deadline = Instant::now() + client::IO_TIMEOUT;
        let exchange_deadline = deadline.map_or(exchange_deadline, |deadline| {
            deadline.min(exchange_deadline)
        });
        remaining(exchange_deadline)?;
        let frame = query(
            &endpoint,
            &token,
            identity,
            &launch_request_id,
            exchange_deadline,
        )?;
        // Validate every running response too: waiting must not conceal wrong
        // identities, unsupported versions, errors, or inconsistent exit state.
        let exit_code = match &frame {
            ServerFrame::LaunchStatus {
                state: LaunchState::Exited { exit_code, .. },
                ..
            } => Some(*exit_code),
            _ => None,
        };
        let text = client::format_launch_status(frame, process_id, &launch_request_id, json)?;
        remaining(exchange_deadline)?;
        if let Some(exit_code) = exit_code {
            return Ok(ControlOutput {
                text,
                exit_code: portable_cli_code(exit_code),
            });
        }
        let delay = match deadline {
            Some(deadline) => POLL_INTERVAL.min(remaining(deadline)?),
            None => POLL_INTERVAL,
        };
        // This sleeps only the standalone CLI, after its socket has closed.
        std::thread::sleep(delay);
    }
}

fn portable_cli_code(code: u32) -> u8 {
    // Never truncate (e.g. 256 -> 0) and accidentally turn failure into success.
    u8::try_from(code).unwrap_or(1)
}

fn socket_identity(path: &Path) -> anyhow::Result<(u64, u64)> {
    use std::os::unix::fs::FileTypeExt;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != crate::domain::paths::effective_uid()
        || metadata.mode() & 0o077 != 0
    {
        anyhow::bail!("unsafe GUI wait endpoint");
    }
    Ok((metadata.dev(), metadata.ino()))
}

fn query(
    endpoint: &ControlPaths,
    token: &str,
    identity: (u64, u64),
    id: &str,
    deadline: Instant,
) -> anyhow::Result<ServerFrame> {
    let check_endpoint = || -> anyhow::Result<()> {
        if socket_identity(&endpoint.socket)? != identity || endpoint.read_token()? != token {
            anyhow::bail!("GUI wait endpoint changed; start a new query explicitly");
        }
        Ok(())
    };
    check_endpoint()?;
    let mut stream = DeadlineStream::connect(&endpoint.socket, deadline)?;
    client::validate_peer(&stream.stream)?;
    check_endpoint()?;
    protocol::write_frame(
        &mut stream,
        &ClientFrame {
            version: protocol::PROTOCOL_VERSION,
            token: token.to_owned(),
            request: Request::LaunchStatus {
                launch_request_id: id.to_owned(),
            },
        },
    )?;
    protocol::read_frame(&mut stream)
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "GUI launch wait timed out; child was not canceled",
            )
        })
}

/// Nonblocking connect/read/write all share one absolute deadline. A peer
/// trickling individual frame bytes cannot reset the timeout indefinitely.
struct DeadlineStream {
    stream: UnixStream,
    deadline: Instant,
}

impl DeadlineStream {
    fn connect(path: &Path, deadline: Instant) -> io::Result<Self> {
        remaining(deadline)?;
        let bytes = path.as_os_str().as_bytes();
        // SAFETY: sockaddr_un is a C record whose all-zero representation is valid.
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        if bytes.is_empty() || bytes.contains(&0) || bytes.len() >= address.sun_path.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid GUI socket path",
            ));
        }
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (target, byte) in address.sun_path.iter_mut().zip(bytes) {
            *target = *byte as libc::c_char;
        }
        let length = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1)
            as libc::socklen_t;
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        {
            address.sun_len = length as u8;
        }
        // SAFETY: socket takes no pointers and returns a uniquely owned descriptor.
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: adopt the new valid descriptor exactly once; every failure drops it.
        let stream = unsafe { UnixStream::from_raw_fd(fd) };
        // SAFETY: fcntl acts on our live descriptor and sets close-on-exec.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
        stream.set_nonblocking(true)?;
        let stream = Self { stream, deadline };
        // SAFETY: the address contains the checked pathname plus zero terminator;
        // length stays within the initialized record and connect retains no pointer.
        if unsafe { libc::connect(fd, (&address as *const libc::sockaddr_un).cast(), length) } != 0
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINPROGRESS) {
                return Err(error);
            }
            stream.ready(libc::POLLOUT)?;
            if let Some(error) = stream.stream.take_error()? {
                return Err(error);
            }
        }
        remaining(deadline)?;
        Ok(stream)
    }

    fn ready(&self, events: libc::c_short) -> io::Result<()> {
        loop {
            let timeout = remaining(self.deadline)?
                .as_millis()
                .clamp(1, i32::MAX as u128) as i32;
            let mut descriptor = libc::pollfd {
                fd: self.stream.as_raw_fd(),
                events,
                revents: 0,
            };
            // SAFETY: poll receives one initialized record for our live descriptor.
            let result = unsafe { libc::poll(&mut descriptor, 1, timeout) };
            if result > 0 {
                return Ok(());
            }
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }
}

impl Read for DeadlineStream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            remaining(self.deadline)?;
            match self.stream.read(bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.ready(libc::POLLIN)?
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                result => return result,
            }
        }
    }
}

impl Write for DeadlineStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            remaining(self.deadline)?;
            match self.stream.write(bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.ready(libc::POLLOUT)?
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_exit_mapping_never_turns_failure_into_success() {
        for code in [0, 1, 7, 124, 255] {
            assert_eq!(portable_cli_code(code), code as u8);
        }
        for code in [256, 512, u32::MAX] {
            assert_eq!(portable_cli_code(code), 1);
        }
    }

    #[test]
    fn stream_deadline_covers_partial_frames_and_backpressure() {
        let (stream, mut peer) = UnixStream::pair().unwrap();
        stream.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_millis(40);
        let mut stream = DeadlineStream { stream, deadline };
        peer.write_all(&[0, 0]).unwrap();
        let mut header = [0; 4];
        assert_eq!(
            stream.read_exact(&mut header).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(Instant::now() >= deadline);

        let (stream, _peer) = UnixStream::pair().unwrap();
        stream.set_nonblocking(true).unwrap();
        let mut stream = DeadlineStream {
            stream,
            deadline: Instant::now() + Duration::from_millis(40),
        };
        let chunk = [0; 8192];
        loop {
            match stream.write_all(&chunk) {
                Ok(()) => {}
                Err(error) => {
                    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
                    break;
                }
            }
        }
    }

    #[test]
    fn changed_token_is_rejected_before_connecting_or_disclosing_it() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let endpoint =
            ControlPaths::from_directory(directory.path().into(), std::process::id()).unwrap();
        let _listener = std::os::unix::net::UnixListener::bind(&endpoint.socket).unwrap();
        std::fs::set_permissions(&endpoint.socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let identity = socket_identity(&endpoint.socket).unwrap();
        let token = endpoint.load_or_create_token().unwrap();
        let different = if token.starts_with('a') {
            "b".repeat(token.len())
        } else {
            "a".repeat(token.len())
        };
        let error = query(
            &endpoint,
            &different,
            identity,
            &"a".repeat(32),
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(error.to_string().contains("endpoint changed"));
        assert!(!error.to_string().contains(&token));
        assert!(!error.to_string().contains(&different));
    }

    #[test]
    fn fifo_token_substitution_does_not_block_before_type_validation() {
        let directory = tempfile::tempdir().unwrap();
        let endpoint =
            ControlPaths::from_directory(directory.path().into(), std::process::id()).unwrap();
        let path = std::ffi::CString::new(endpoint.token.as_os_str().as_bytes()).unwrap();
        // SAFETY: mkfifo receives a live NUL-terminated pathname in the fixture.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        // If regular-file validation regresses, cap the test by letting a writer
        // release a blocking open. The nonblocking implementation returns at once.
        let token_path = endpoint.token.clone();
        let (finished, cancel) = std::sync::mpsc::channel();
        let unblock = std::thread::spawn(move || {
            if cancel.recv_timeout(Duration::from_secs(1)).is_err() {
                use std::os::unix::fs::OpenOptionsExt;
                let _ = std::fs::OpenOptions::new()
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(token_path);
            }
        });
        let started = Instant::now();
        let result = endpoint.read_token();
        let _ = finished.send(());
        unblock.join().unwrap();
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_millis(500));
    }
}
