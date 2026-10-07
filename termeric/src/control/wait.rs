//! Client-only child waiting. No GUI worker, terminal owner, or mutation is held.

use super::{
    ControlOutput, client,
    paths::ControlEndpoint,
    protocol::{self, ClientFrame, LaunchState, Request, Response},
};
use anyhow::Context;
use std::io::{self, Read, Write};
use std::os::unix::{fs::MetadataExt, net::UnixStream};
use std::path::Path;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(250);

pub(super) fn launch_and_wait(
    process_id: u32,
    mut request: Request,
    request_id: Option<String>,
    timeout_seconds: Option<u32>,
    json: bool,
) -> anyhow::Result<ControlOutput> {
    let deadline = overall_deadline(process_id, timeout_seconds)?;
    let id = request_id.map_or_else(client::generate_request_id, Ok)?;
    match &mut request {
        Request::LaunchTab { request_id, .. } | Request::LaunchWindow { request_id, .. } => {
            *request_id = id.clone();
        }
        _ => anyhow::bail!("wait requires a tab or window command launch"),
    }
    request.canonicalize();
    request.validate().map_err(anyhow::Error::msg)?;
    let result = (|| {
        let endpoint = discover(process_id)?;
        let token = endpoint.paths.read_token()?;
        let identity = socket_identity(&endpoint.paths.socket)?;
        launch_at(&endpoint, &token, identity, request, deadline, json)
    })();
    result.with_context(|| format!(
        "launch or observation failed (process {process_id}, request {id}); no automatic relaunch or cancellation was sent; inspect with `control launch-status {process_id} {id}` or resume with `control wait-launch {process_id} {id}`"
    ))
}

fn launch_at(
    endpoint: &ControlEndpoint,
    token: &str,
    identity: (u64, u64),
    request: Request,
    deadline: Option<Instant>,
    json: bool,
) -> anyhow::Result<ControlOutput> {
    let id = match &request {
        Request::LaunchTab { request_id, .. } | Request::LaunchWindow { request_id, .. } => {
            request_id.clone()
        }
        _ => anyhow::bail!("wait requires a tab or window command launch"),
    };
    request.validate().map_err(anyhow::Error::msg)?;
    let exchange_deadline = exchange_deadline(deadline);
    let response = exchange(endpoint, token, identity, &request, exchange_deadline)?;
    client::answer(endpoint.process_id, &request, response)?;
    remaining(exchange_deadline)?;
    poll_launch(endpoint, token, identity, &id, deadline, json)
}

fn overall_deadline(
    process_id: u32,
    timeout_seconds: Option<u32>,
) -> anyhow::Result<Option<Instant>> {
    if process_id == 0
        || process_id > i32::MAX as u32
        || timeout_seconds.is_some_and(|seconds| !(1..=86400).contains(&seconds))
    {
        anyhow::bail!("invalid wait-launch process ID or timeout");
    }
    Ok(timeout_seconds.map(|seconds| Instant::now() + Duration::from_secs(seconds.into())))
}

fn discover(process_id: u32) -> anyhow::Result<ControlEndpoint> {
    ControlEndpoint::discover_all()?
        .into_iter()
        .find(|endpoint| endpoint.process_id == process_id)
        .ok_or_else(|| anyhow::anyhow!("GUI process {process_id} was not found"))
}

fn exchange_deadline(deadline: Option<Instant>) -> Instant {
    let exchange = Instant::now() + client::IO_TIMEOUT;
    deadline.map_or(exchange, |deadline| deadline.min(exchange))
}

pub(super) fn launch(
    process_id: u32,
    launch_request_id: String,
    timeout_seconds: Option<u32>,
    json: bool,
) -> anyhow::Result<ControlOutput> {
    let deadline = overall_deadline(process_id, timeout_seconds)?;
    Request::LaunchStatus {
        launch_request_id: launch_request_id.clone(),
    }
    .validate()
    .map_err(anyhow::Error::msg)?;
    let endpoint = discover(process_id)?;
    let token = endpoint.paths.read_token()?;
    let identity = socket_identity(&endpoint.paths.socket)?;
    poll_launch(
        &endpoint,
        &token,
        identity,
        &launch_request_id,
        deadline,
        json,
    )
}

fn poll_launch(
    endpoint: &ControlEndpoint,
    token: &str,
    identity: (u64, u64),
    launch_request_id: &str,
    deadline: Option<Instant>,
    json: bool,
) -> anyhow::Result<ControlOutput> {
    let query = Request::LaunchStatus {
        launch_request_id: launch_request_id.to_owned(),
    };
    loop {
        let exchange_deadline = exchange_deadline(deadline);
        remaining(exchange_deadline)?;
        // Check every running reply too: waiting must not conceal wrong
        // identities, unsupported versions, errors, or inconsistent exit state.
        let response = client::answer(
            endpoint.process_id,
            &query,
            exchange(endpoint, token, identity, &query, exchange_deadline)?,
        )?;
        let exit_code = match &response {
            Response::LaunchStatus {
                state: LaunchState::Exited { exit_code, .. },
                ..
            } => Some(*exit_code),
            _ => None,
        };
        let text = client::print(&query, &response, json)?;
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
        || metadata.uid() != crate::local_socket::effective_uid()
        || metadata.mode() & 0o077 != 0
    {
        anyhow::bail!("unsafe GUI wait endpoint");
    }
    Ok((metadata.dev(), metadata.ino()))
}

/// One exchange on the endpoint first found, refused if it has since been
/// replaced; the reply may still be a refusal.
fn exchange(
    endpoint: &ControlEndpoint,
    token: &str,
    identity: (u64, u64),
    request: &Request,
    deadline: Instant,
) -> anyhow::Result<Response> {
    let check_endpoint = || -> anyhow::Result<()> {
        if socket_identity(&endpoint.paths.socket)? != identity
            || endpoint.paths.read_token()? != token
        {
            anyhow::bail!("GUI wait endpoint changed; start a new query explicitly");
        }
        Ok(())
    };
    check_endpoint()?;
    let mut stream = DeadlineStream::connect(&endpoint.paths.socket, deadline)?;
    client::validate_peer(&stream.stream)?;
    check_endpoint()?;
    protocol::write_frame(
        &mut stream,
        &ClientFrame {
            version: protocol::PROTOCOL_VERSION,
            token: token.to_owned(),
            request: request.clone(),
        },
    )?;
    client::accept(protocol::read_frame(&mut stream)?)
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

/// Reads and writes share one absolute deadline: each call waits at most for
/// the time left, so a peer trickling single frame bytes cannot stretch the
/// exchange. Connecting blocks only while the listener's backlog is full.
struct DeadlineStream {
    stream: UnixStream,
    deadline: Instant,
}

impl DeadlineStream {
    fn connect(path: &Path, deadline: Instant) -> io::Result<Self> {
        remaining(deadline)?;
        let stream = UnixStream::connect(path)?;
        remaining(deadline)?;
        Ok(Self { stream, deadline })
    }
}

/// A socket timeout surfaces as `WouldBlock`; retrying re-checks the
/// deadline, which reports `TimedOut` once it has passed.
fn retryable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

impl Read for DeadlineStream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            self.stream
                .set_read_timeout(Some(remaining(self.deadline)?))?;
            match self.stream.read(bytes) {
                Err(error) if retryable(&error) => {}
                result => return result,
            }
        }
    }
}

impl Write for DeadlineStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            self.stream
                .set_write_timeout(Some(remaining(self.deadline)?))?;
            match self.stream.write(bytes) {
                Err(error) if retryable(&error) => {}
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
    fn stream_deadline_covers_trickled_frames_and_backpressure() {
        // Each byte arrives well within a socket timeout, but the frame
        // header never completes before the shared deadline.
        let (stream, mut peer) = UnixStream::pair().unwrap();
        let deadline = Instant::now() + Duration::from_millis(80);
        let mut stream = DeadlineStream { stream, deadline };
        let trickle = std::thread::spawn(move || {
            for _ in 0..3 {
                std::thread::sleep(Duration::from_millis(30));
                peer.write_all(&[0]).unwrap();
            }
            peer
        });
        let mut header = [0; 4];
        assert_eq!(
            stream.read_exact(&mut header).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(Instant::now() >= deadline);
        drop(trickle.join().unwrap());

        let (stream, _peer) = UnixStream::pair().unwrap();
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
            ControlEndpoint::from_directory(directory.path().into(), std::process::id()).unwrap();
        let _listener = std::os::unix::net::UnixListener::bind(&endpoint.paths.socket).unwrap();
        std::fs::set_permissions(
            &endpoint.paths.socket,
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let identity = socket_identity(&endpoint.paths.socket).unwrap();
        let token = endpoint.paths.load_or_create_token().unwrap();
        let different = if token.starts_with('a') {
            "b".repeat(token.len())
        } else {
            "a".repeat(token.len())
        };
        let error = exchange(
            &endpoint,
            &different,
            identity,
            &Request::LaunchStatus {
                launch_request_id: "a".repeat(32),
            },
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(error.to_string().contains("endpoint changed"));
        assert!(!error.to_string().contains(&token));
        assert!(!error.to_string().contains(&different));
    }

    #[test]
    fn fifo_token_substitution_does_not_block_before_type_validation() {
        use std::os::unix::ffi::OsStrExt;
        let directory = tempfile::tempdir().unwrap();
        let endpoint =
            ControlEndpoint::from_directory(directory.path().into(), std::process::id()).unwrap();
        let path = std::ffi::CString::new(endpoint.paths.token.as_os_str().as_bytes()).unwrap();
        // SAFETY: mkfifo receives a live NUL-terminated pathname in the fixture.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        // If regular-file validation regresses, cap the test by letting a writer
        // release a blocking open. The nonblocking implementation returns at once.
        let token_path = endpoint.paths.token.clone();
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
        let result = endpoint.paths.read_token();
        let _ = finished.send(());
        unblock.join().unwrap();
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_millis(500));
    }
}
