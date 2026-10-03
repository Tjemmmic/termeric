//! Authenticated local-domain client and transparent GUI relay.

use super::paths::RuntimePaths;
use super::protocol::{
    self, ClientFrame, Request, Response, ServerFrame, UploadKind, UploadRetention,
};
use super::{DomainTopologyEvent, validate_ssh_destination};
use anyhow::Context;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use termeric_core::pty::PtySize;

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_UPLOAD_ATTEMPTS: usize = 4;
const UPLOAD_RETRY_DELAY: Duration = Duration::from_millis(100);
/// What `stop-server` reports, locally or through the SSH bridge, when no
/// daemon is running.
pub(super) const NO_SERVER_RUNNING: &str = "no domain server is running";

#[path = "client_attachment_snapshot.rs"]
mod attachment_snapshot;
#[path = "client_bridge.rs"]
mod bridge;
#[path = "client_relay_frame_writer.rs"]
mod relay_frame_writer;
#[path = "client_relay_output.rs"]
mod relay_output;
#[path = "client_relay_resize.rs"]
mod relay_resize;
#[path = "client_relay_workers.rs"]
mod relay_workers;
#[path = "client_relay_write.rs"]
mod relay_write;

enum UploadAttemptError {
    Retry(anyhow::Error),
    Busy(Duration),
    Fatal(anyhow::Error),
}

pub struct DomainClient {
    endpoint: DomainEndpoint,
}

enum DomainEndpoint {
    Local(RuntimePaths),
    Ssh {
        destination: String,
        start_server: bool,
    },
}

enum DomainStream {
    Local(UnixStream),
    Ssh(SshStream),
}

struct SshStream {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
}

enum DomainReader {
    Local(UnixStream),
    Ssh(ChildStdout),
}

enum DomainWriter {
    Local(UnixStream),
    Ssh(ChildStdin),
}

enum DomainCloser {
    Local(UnixStream),
    Ssh(SshChild),
}

struct SshChild(Mutex<Option<Child>>);

struct DeadlineReader<'a, R> {
    stream: &'a mut R,
    deadline: Instant,
    alive: Option<&'a AtomicBool>,
}

struct FocusQueue {
    pending: Mutex<std::collections::HashMap<(super::DomainAuthority, String), u32>>,
    ready: std::sync::Condvar,
}

const MAX_PENDING_FOCUS_AUTHORITIES: usize = 64;

static FOCUS_QUEUE: std::sync::OnceLock<Arc<FocusQueue>> = std::sync::OnceLock::new();

impl Read for DomainStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Local(stream) => stream.read(buffer),
            Self::Ssh(stream) => stream
                .stdout
                .as_mut()
                .expect("live SSH stream retains stdout")
                .read(buffer),
        }
    }
}

impl<R: Read + AsRawFd> Read for DeadlineReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self
                .alive
                .is_some_and(|alive| !alive.load(Ordering::Acquire))
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionAborted,
                    "domain subscription cancelled",
                ));
            }
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "domain response deadline elapsed",
                ));
            }
            let mut timeout_millis =
                remaining.as_millis().max(1).min(libc::c_int::MAX as u128) as libc::c_int;
            if self.alive.is_some() {
                timeout_millis = timeout_millis.min(500);
            }
            if stdin_ready(self.stream.as_raw_fd(), timeout_millis)? {
                return self.stream.read(buffer);
            }
        }
    }
}

impl Write for DomainStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Local(stream) => stream.write(buffer),
            Self::Ssh(stream) => stream
                .stdin
                .as_mut()
                .expect("live SSH stream retains stdin")
                .write(buffer),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Local(stream) => stream.flush(),
            Self::Ssh(stream) => stream
                .stdin
                .as_mut()
                .expect("live SSH stream retains stdin")
                .flush(),
        }
    }
}

impl AsRawFd for DomainStream {
    fn as_raw_fd(&self) -> RawFd {
        match self {
            Self::Local(stream) => stream.as_raw_fd(),
            Self::Ssh(stream) => stream
                .stdout
                .as_ref()
                .expect("live SSH stream retains stdout")
                .as_raw_fd(),
        }
    }
}

impl DomainStream {
    fn split(mut self) -> anyhow::Result<(DomainReader, DomainWriter, DomainCloser)> {
        match &mut self {
            Self::Local(stream) => Ok((
                DomainReader::Local(stream.try_clone()?),
                DomainWriter::Local(stream.try_clone()?),
                DomainCloser::Local(stream.try_clone()?),
            )),
            Self::Ssh(stream) => Ok((
                DomainReader::Ssh(
                    stream
                        .stdout
                        .take()
                        .context("SSH transport stdout is unavailable")?,
                ),
                DomainWriter::Ssh(
                    stream
                        .stdin
                        .take()
                        .context("SSH transport stdin is unavailable")?,
                ),
                DomainCloser::Ssh(SshChild(Mutex::new(stream.child.take()))),
            )),
        }
    }
}

impl Read for DomainReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Local(stream) => stream.read(buffer),
            Self::Ssh(stream) => stream.read(buffer),
        }
    }
}

impl AsRawFd for DomainReader {
    fn as_raw_fd(&self) -> RawFd {
        match self {
            Self::Local(stream) => stream.as_raw_fd(),
            Self::Ssh(stream) => stream.as_raw_fd(),
        }
    }
}

impl Write for DomainWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Local(stream) => stream.write(buffer),
            Self::Ssh(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Local(stream) => stream.flush(),
            Self::Ssh(stream) => stream.flush(),
        }
    }
}

impl DomainCloser {
    fn close(&self) {
        match self {
            Self::Local(stream) => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
            Self::Ssh(child) => child.close(),
        }
    }
}

impl SshChild {
    fn close(&self) {
        let Some(mut child) = self.0.lock().unwrap().take() else {
            return;
        };
        for _ in 0..10 {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(_) => break,
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for SshChild {
    fn drop(&mut self) {
        self.close();
    }
}

impl Drop for SshStream {
    fn drop(&mut self) {
        self.stdin.take();
        self.stdout.take();
        if let Some(child) = self.child.take() {
            SshChild(Mutex::new(Some(child))).close();
        }
    }
}

/// Persist GUI pane focus without blocking the window event handler on socket
/// setup or daemon scheduling. Repeated focus changes are deliberately bounded.
pub fn request_focus_background(authority: super::DomainAuthority, domain: String, pane: u32) {
    let queue = FOCUS_QUEUE.get_or_init(|| {
        let queue = Arc::new(FocusQueue {
            pending: Mutex::new(std::collections::HashMap::new()),
            ready: std::sync::Condvar::new(),
        });
        let worker_queue = Arc::clone(&queue);
        if let Err(error) = thread::Builder::new()
            .name("termeric-domain-focus".into())
            .spawn(move || {
                loop {
                    let (authority, domain, pane) = {
                        let mut pending = worker_queue.pending.lock().unwrap();
                        while pending.is_empty() {
                            pending = worker_queue.ready.wait(pending).unwrap();
                        }
                        let ((authority, domain), pane) = pending
                            .iter()
                            .next()
                            .map(|(route, pane)| (route.clone(), *pane))
                            .expect("nonempty focus queue has one route");
                        pending.remove(&(authority.clone(), domain.clone()));
                        (authority, domain, pane)
                    };
                    let result = authority.client().and_then(|client| {
                        client.request(Request::FocusPane { domain, pane })?;
                        Ok(())
                    });
                    if let Err(error) = result {
                        log::warn!("Could not persist live-domain focus: {error:#}");
                    }
                }
            })
        {
            log::warn!("Could not start live-domain focus worker: {error}");
        }
        queue
    });
    let mut pending = queue.pending.lock().unwrap();
    let route = (authority, domain);
    if pending.len() >= MAX_PENDING_FOCUS_AUTHORITIES && !pending.contains_key(&route) {
        log::warn!(
            "Discarding focus update because {MAX_PENDING_FOCUS_AUTHORITIES} domain authorities are pending"
        );
        return;
    }
    pending.insert(route, pane);
    drop(pending);
    queue.ready.notify_one();
}

impl DomainClient {
    pub fn new_upload_id() -> anyhow::Result<String> {
        generate_upload_id()
    }

    pub fn connect_or_start() -> anyhow::Result<Self> {
        let paths = RuntimePaths::discover()?;
        // Create authentication material before spawning so concurrent clients
        // and the daemon converge on the same private token.
        paths.load_or_create_token()?;
        let client = Self {
            endpoint: DomainEndpoint::Local(paths),
        };
        if client.connect_if_running()? {
            return Ok(client);
        }
        client.spawn_server()?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if client.connect().is_ok() {
                return Ok(client);
            }
            if Instant::now() >= deadline {
                anyhow::bail!("local domain server did not become ready within 3 seconds");
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    /// The local daemon if one is running; never starts one.
    pub fn connect_running() -> anyhow::Result<Option<Self>> {
        let client = Self {
            endpoint: DomainEndpoint::Local(RuntimePaths::discover()?),
        };
        Ok(client.connect_if_running()?.then_some(client))
    }

    /// Whether a daemon answered; `false` when nothing is listening, an error
    /// when something answered but refused us.
    fn connect_if_running(&self) -> anyhow::Result<bool> {
        match self.connect() {
            Ok(_) => Ok(true),
            Err(error) => {
                let nothing_listening = error.downcast_ref::<std::io::Error>().is_some_and(|io| {
                    matches!(
                        io.kind(),
                        std::io::ErrorKind::NotFound
                            | std::io::ErrorKind::ConnectionRefused
                            | std::io::ErrorKind::ConnectionReset
                    )
                });
                if nothing_listening {
                    Ok(false)
                } else {
                    Err(error).context("existing local domain endpoint rejected negotiation")
                }
            }
        }
    }

    #[cfg(test)]
    pub fn for_paths(paths: RuntimePaths) -> Self {
        Self {
            endpoint: DomainEndpoint::Local(paths),
        }
    }

    pub fn for_ssh(destination: String) -> anyhow::Result<Self> {
        validate_ssh_destination(&destination)?;
        Ok(Self {
            endpoint: DomainEndpoint::Ssh {
                destination,
                start_server: true,
            },
        })
    }

    /// Like [`Self::for_ssh`], but the remote bridge refuses rather than
    /// starting a daemon when none is running there.
    pub fn for_running_ssh(destination: String) -> anyhow::Result<Self> {
        validate_ssh_destination(&destination)?;
        Ok(Self {
            endpoint: DomainEndpoint::Ssh {
                destination,
                start_server: false,
            },
        })
    }

    pub fn request(&self, request: Request) -> anyhow::Result<Response> {
        let mut stream = self.connect()?;
        protocol::write_frame(&mut stream, &ClientFrame::Request { request })?;
        match read_frame_with_timeout(&mut stream, RESPONSE_TIMEOUT)? {
            ServerFrame::Response { response } => Ok(response),
            ServerFrame::Error { message } => anyhow::bail!("{message}"),
            frame => anyhow::bail!("unexpected domain response {frame:?}"),
        }
    }

    /// Stream one regular local file into the exact daemon-owned pane cwd.
    /// The server publishes it only after every declared byte arrives and
    /// never replaces an existing destination.
    pub fn upload_file(
        &self,
        domain: String,
        pane: u32,
        source: &std::path::Path,
        name: Option<String>,
    ) -> anyhow::Result<Response> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(source)
            .with_context(|| format!("open upload source {}", source.display()))?;
        let name = name
            .or_else(|| {
                source
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .map(String::from)
            })
            .context("upload source requires a UTF-8 file name")?;
        self.upload_open_file(domain, pane, file, name)
    }

    /// Upload one real file or directory. Directory trees are converted to a
    /// deterministic bounded archive locally and extracted by the daemon only
    /// after the full archive has arrived.
    pub fn upload_path(
        &self,
        domain: String,
        pane: u32,
        source: &std::path::Path,
        name: Option<String>,
    ) -> anyhow::Result<Response> {
        let metadata = std::fs::symlink_metadata(source)
            .with_context(|| format!("inspect upload source {}", source.display()))?;
        if metadata.file_type().is_symlink() {
            anyhow::bail!("upload source must not be a symbolic link");
        }
        if metadata.is_file() {
            return self.upload_file(domain, pane, source, name);
        }
        if metadata.is_dir() {
            return self.upload_directory(domain, pane, source, name);
        }
        anyhow::bail!("upload source must be one regular file or real directory")
    }

    pub fn upload_path_with_manual_resume(
        &self,
        domain: String,
        pane: u32,
        source: &std::path::Path,
        name: Option<String>,
        upload_id: String,
        resume_only: bool,
    ) -> anyhow::Result<Response> {
        let metadata = std::fs::symlink_metadata(source)
            .with_context(|| format!("inspect upload source {}", source.display()))?;
        if metadata.file_type().is_symlink() {
            anyhow::bail!("upload source must not be a symbolic link");
        }
        let name = name
            .or_else(|| {
                source
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .map(String::from)
            })
            .context("upload source requires a UTF-8 name")?;
        let cancelled = AtomicBool::new(false);
        if metadata.is_file() {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(source)
                .with_context(|| format!("open upload source {}", source.display()))?;
            return self.upload_open_file_with_kind_and_id(
                domain,
                pane,
                file,
                name,
                UploadKind::File,
                upload_id,
                UploadRetention::Manual,
                resume_only,
                &cancelled,
                |_, _| {},
            );
        }
        if metadata.is_dir() {
            let archive =
                super::archive::build_directory_archive_with_control(source, &cancelled, None)?;
            let file = archive.file.try_clone()?;
            return self.upload_open_file_with_kind_and_id(
                domain,
                pane,
                file,
                name,
                UploadKind::Directory {
                    entries: archive.entries,
                },
                upload_id,
                UploadRetention::Manual,
                resume_only,
                &cancelled,
                |_, _| {},
            );
        }
        anyhow::bail!("upload source must be one regular file or real directory")
    }

    pub fn upload_directory(
        &self,
        domain: String,
        pane: u32,
        source: &std::path::Path,
        name: Option<String>,
    ) -> anyhow::Result<Response> {
        let cancelled = AtomicBool::new(false);
        self.upload_directory_with_control(domain, pane, source, name, None, &cancelled, |_, _| {})
    }

    pub fn upload_directory_with_control(
        &self,
        domain: String,
        pane: u32,
        source: &std::path::Path,
        name: Option<String>,
        expected_identity: Option<(u64, u64)>,
        cancelled: &AtomicBool,
        progress: impl FnMut(u64, u64),
    ) -> anyhow::Result<Response> {
        let archive = match super::archive::build_directory_archive_with_control(
            source,
            cancelled,
            expected_identity,
        ) {
            Ok(archive) => archive,
            Err(_) if cancelled.load(Ordering::Acquire) => {
                return Ok(Response::UploadCancelled { bytes_received: 0 });
            }
            Err(error) => return Err(error),
        };
        let name = name
            .or_else(|| {
                source
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .map(String::from)
            })
            .context("directory upload source requires a UTF-8 name")?;
        let file = archive
            .file
            .try_clone()
            .context("clone directory upload archive handle")?;
        self.upload_open_file_with_kind_and_id(
            domain,
            pane,
            file,
            name,
            UploadKind::Directory {
                entries: archive.entries,
            },
            generate_upload_id()?,
            UploadRetention::Automatic,
            false,
            cancelled,
            progress,
        )
    }

    #[cfg(test)]
    pub(crate) fn upload_directory_with_test_id(
        &self,
        domain: String,
        pane: u32,
        source: &std::path::Path,
        name: String,
        upload_id: String,
    ) -> anyhow::Result<Response> {
        let cancelled = AtomicBool::new(false);
        let archive =
            super::archive::build_directory_archive_with_control(source, &cancelled, None)?;
        let file = archive.file.try_clone()?;
        self.upload_open_file_with_kind_and_id(
            domain,
            pane,
            file,
            name,
            UploadKind::Directory {
                entries: archive.entries,
            },
            upload_id,
            UploadRetention::Automatic,
            false,
            &cancelled,
            |_, _| {},
        )
    }

    pub fn upload_open_file(
        &self,
        domain: String,
        pane: u32,
        file: std::fs::File,
        name: String,
    ) -> anyhow::Result<Response> {
        let cancelled = AtomicBool::new(false);
        self.upload_open_file_with_control(domain, pane, file, name, &cancelled, |_, _| {})
    }

    /// Stream an already-open file while reporting monotonic sent-byte
    /// progress and honoring cancellation before the publication commit.
    pub fn upload_open_file_with_control(
        &self,
        domain: String,
        pane: u32,
        file: std::fs::File,
        name: String,
        cancelled: &AtomicBool,
        progress: impl FnMut(u64, u64),
    ) -> anyhow::Result<Response> {
        let upload_id = generate_upload_id()?;
        self.upload_open_file_with_kind_and_id(
            domain,
            pane,
            file,
            name,
            UploadKind::File,
            upload_id,
            UploadRetention::Automatic,
            false,
            cancelled,
            progress,
        )
    }

    #[cfg(test)]
    pub(crate) fn upload_open_file_with_test_id(
        &self,
        domain: String,
        pane: u32,
        file: std::fs::File,
        name: String,
        upload_id: String,
        cancelled: &AtomicBool,
        progress: impl FnMut(u64, u64),
    ) -> anyhow::Result<Response> {
        self.upload_open_file_with_kind_and_id(
            domain,
            pane,
            file,
            name,
            UploadKind::File,
            upload_id,
            UploadRetention::Automatic,
            false,
            cancelled,
            progress,
        )
    }

    fn upload_open_file_with_kind_and_id(
        &self,
        domain: String,
        pane: u32,
        mut file: std::fs::File,
        name: String,
        kind: UploadKind,
        upload_id: String,
        retention: UploadRetention,
        resume_only: bool,
        cancelled: &AtomicBool,
        mut progress: impl FnMut(u64, u64),
    ) -> anyhow::Result<Response> {
        let metadata = file.metadata().context("inspect opened upload source")?;
        if !metadata.is_file() {
            anyhow::bail!("upload source must be one regular file");
        }
        let total_bytes = metadata.len();
        if total_bytes > protocol::MAX_FILE_UPLOAD_BYTES {
            anyhow::bail!(
                "file exceeds the {} MiB upload limit",
                protocol::MAX_FILE_UPLOAD_BYTES / (1024 * 1024)
            );
        }
        protocol::validate_upload_name(&name)?;
        protocol::validate_upload_id(&upload_id)?;
        progress(0, total_bytes);
        if cancelled.load(Ordering::Acquire) {
            return Ok(Response::UploadCancelled { bytes_received: 0 });
        }
        let mut reported_progress = 0_u64;

        for attempt in 1..=MAX_UPLOAD_ATTEMPTS {
            match self.upload_attempt(
                &domain,
                pane,
                &mut file,
                &name,
                total_bytes,
                &upload_id,
                kind,
                retention,
                resume_only,
                &metadata,
                cancelled,
                &mut progress,
                &mut reported_progress,
            ) {
                Ok(response) => return Ok(response),
                Err(UploadAttemptError::Fatal(error)) => return Err(error),
                Err(UploadAttemptError::Retry(error)) if attempt == MAX_UPLOAD_ATTEMPTS => {
                    return Err(error).context(format!(
                        "upload transport failed after {MAX_UPLOAD_ATTEMPTS} attempts"
                    ));
                }
                Err(UploadAttemptError::Retry(_)) => thread::sleep(UPLOAD_RETRY_DELAY),
                Err(UploadAttemptError::Busy(_)) if attempt == MAX_UPLOAD_ATTEMPTS => {
                    anyhow::bail!(
                        "upload transport remained busy after {MAX_UPLOAD_ATTEMPTS} attempts"
                    );
                }
                Err(UploadAttemptError::Busy(delay)) => thread::sleep(delay),
            }
        }
        unreachable!("bounded upload retry loop always returns")
    }

    fn upload_attempt(
        &self,
        domain: &str,
        pane: u32,
        file: &mut std::fs::File,
        name: &str,
        total_bytes: u64,
        upload_id: &str,
        kind: UploadKind,
        retention: UploadRetention,
        resume_only: bool,
        expected_metadata: &std::fs::Metadata,
        cancelled: &AtomicBool,
        progress: &mut impl FnMut(u64, u64),
        reported_progress: &mut u64,
    ) -> Result<Response, UploadAttemptError> {
        let mut stream = self.connect().map_err(UploadAttemptError::Retry)?;
        protocol::write_frame(
            &mut stream,
            &ClientFrame::Request {
                request: Request::UploadFile {
                    domain: domain.to_string(),
                    pane,
                    name: name.to_string(),
                    total_bytes,
                    upload_id: upload_id.to_string(),
                    kind,
                    retention,
                    resume_only,
                },
            },
        )
        .map_err(UploadAttemptError::Retry)?;
        let (max_chunk_bytes, mut offset, prefix_sha256) =
            match read_frame_with_timeout(&mut stream, RESPONSE_TIMEOUT)
                .map_err(UploadAttemptError::Retry)?
            {
                ServerFrame::Response {
                    response:
                        Response::UploadReady {
                            max_chunk_bytes,
                            offset,
                            prefix_sha256,
                        },
                } if (1..=protocol::MAX_STREAM_CHUNK_BYTES).contains(&max_chunk_bytes)
                    && offset <= total_bytes =>
                {
                    (max_chunk_bytes, offset, prefix_sha256)
                }
                ServerFrame::Response { response }
                    if upload_response_matches_kind(&response, kind) =>
                {
                    return Ok(response);
                }
                ServerFrame::Response {
                    response: Response::UploadBusy { retry_after_ms },
                } => {
                    return Err(UploadAttemptError::Busy(Duration::from_millis(u64::from(
                        retry_after_ms.clamp(1, 1_000),
                    ))));
                }
                ServerFrame::Error { message } => {
                    return Err(UploadAttemptError::Fatal(anyhow::anyhow!(message)));
                }
                frame => {
                    return Err(UploadAttemptError::Fatal(anyhow::anyhow!(
                        "unexpected upload readiness response {frame:?}"
                    )));
                }
            };
        protocol::validate_sha256(&prefix_sha256).map_err(UploadAttemptError::Fatal)?;
        let local_prefix = sha256_prefix(file, offset).map_err(UploadAttemptError::Fatal)?;
        if local_prefix != prefix_sha256 {
            return Err(UploadAttemptError::Fatal(anyhow::anyhow!(
                "local upload source does not match the daemon's retained prefix"
            )));
        }
        report_upload_progress(progress, reported_progress, offset, total_bytes);
        if cancelled.load(Ordering::Acquire) {
            return cancel_upload_attempt(&mut stream);
        }
        let mut buffer = vec![0_u8; max_chunk_bytes];
        while offset < total_bytes {
            if cancelled.load(Ordering::Acquire) {
                return cancel_upload_attempt(&mut stream);
            }
            let wanted = usize::try_from((total_bytes - offset).min(max_chunk_bytes as u64))
                .expect("bounded upload chunk fits usize");
            let read = loop {
                match file.read(&mut buffer[..wanted]) {
                    Ok(read) => break read,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        return Err(UploadAttemptError::Fatal(
                            anyhow::Error::from(error).context("read upload source"),
                        ));
                    }
                }
            };
            if read == 0 {
                return Err(UploadAttemptError::Fatal(anyhow::anyhow!(
                    "upload source was truncated while it was being read"
                )));
            }
            protocol::write_frame(
                &mut stream,
                &ClientFrame::UploadChunk {
                    offset,
                    data: buffer[..read].to_vec(),
                },
            )
            .map_err(UploadAttemptError::Retry)?;
            offset += read as u64;
            report_upload_progress(progress, reported_progress, offset, total_bytes);
        }
        let mut extra = [0_u8; 1];
        let extra_read = file
            .read(&mut extra)
            .context("recheck upload source size")
            .map_err(UploadAttemptError::Fatal)?;
        if extra_read != 0 {
            return Err(UploadAttemptError::Fatal(anyhow::anyhow!(
                "upload source grew while it was being read"
            )));
        }
        let current_metadata = file
            .metadata()
            .context("revalidate upload source before commit")
            .map_err(UploadAttemptError::Fatal)?;
        if !upload_metadata_unchanged(expected_metadata, &current_metadata) {
            let _ = cancel_upload_attempt(&mut stream);
            return Err(UploadAttemptError::Fatal(anyhow::anyhow!(
                "upload source changed while it was being read"
            )));
        }
        if cancelled.load(Ordering::Acquire) {
            return cancel_upload_attempt(&mut stream);
        }
        protocol::write_frame(&mut stream, &ClientFrame::UploadCommit)
            .map_err(UploadAttemptError::Retry)?;
        match read_frame_with_timeout(&mut stream, RESPONSE_TIMEOUT)
            .map_err(UploadAttemptError::Retry)?
        {
            ServerFrame::Response { response } if upload_response_matches_kind(&response, kind) => {
                Ok(response)
            }
            ServerFrame::Error { message } => {
                Err(UploadAttemptError::Fatal(anyhow::anyhow!(message)))
            }
            frame => Err(UploadAttemptError::Fatal(anyhow::anyhow!(
                "unexpected upload completion response {frame:?}"
            ))),
        }
    }

    /// Block on the authenticated domain topology stream until the daemon
    /// closes it or the receiver returns false. The initial snapshot and every
    /// later revision use the same callback path so consumers cannot miss the
    /// mutation between a separate describe and subscribe request.
    pub fn subscribe_topology(
        &self,
        domain: String,
        alive: &AtomicBool,
        receive: impl FnMut(DomainTopologyEvent) -> bool,
    ) -> anyhow::Result<()> {
        subscribe_topology_stream(self.connect()?, domain, alive, RESPONSE_TIMEOUT, receive)
    }

    pub fn relay(&self, domain: String, pane: u32) -> anyhow::Result<()> {
        let mut stream = self.connect()?;
        protocol::write_frame(
            &mut stream,
            &ClientFrame::Request {
                request: Request::Attach { domain, pane },
            },
        )?;
        let (snapshot, _sequence) = match read_frame_with_timeout(&mut stream, RESPONSE_TIMEOUT)? {
            ServerFrame::Response {
                response: Response::Attached { snapshot, sequence },
            } => (snapshot, sequence),
            ServerFrame::Error { message } => anyhow::bail!("{message}"),
            frame => anyhow::bail!("unexpected attach response {frame:?}"),
        };

        let _raw_terminal = RawTerminal::enter(libc::STDIN_FILENO)?;
        let mut stdout = std::io::stdout().lock();
        attachment_snapshot::replay(&mut stream, &mut stdout, &snapshot)?;
        drop(stdout);

        let (mut reader, writer, closer) = stream.split()?;
        let writer = Arc::new(Mutex::new(relay_frame_writer::Writer::new(writer)?));
        let closer = Arc::new(closer);
        let alive = Arc::new(AtomicBool::new(true));
        let mut workers = relay_workers::Workers::new(Arc::clone(&alive), Arc::clone(&closer));
        let output_alive = Arc::clone(&alive);
        let output_closer = Arc::clone(&closer);
        workers.output = Some(
            thread::Builder::new()
                .name("termeric-domain-relay-output".into())
                .spawn(move || -> anyhow::Result<()> {
                    let mut stdout = std::io::stdout().lock();
                    relay_output::run(&mut reader, &mut stdout, &output_alive, &output_closer)
                })?,
        );

        let resize_alive = Arc::clone(&alive);
        let resize_writer = Arc::clone(&writer);
        let resize_closer = Arc::clone(&closer);
        workers.resize = Some(
            thread::Builder::new()
                .name("termeric-domain-relay-resize".into())
                .spawn(move || {
                    relay_resize::run(
                        &resize_writer,
                        &resize_alive,
                        &resize_closer,
                        || terminal_size(libc::STDIN_FILENO),
                        || thread::sleep(Duration::from_millis(250)),
                    )
                })?,
        );

        let input_result = (|| -> anyhow::Result<()> {
            let mut stdin = std::io::stdin().lock();
            let mut buffer = vec![0u8; 16 * 1024];
            while alive.load(Ordering::Acquire) {
                if !stdin_ready(libc::STDIN_FILENO, 250)? {
                    continue;
                }
                match stdin.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        if !relay_write::send(
                            &writer,
                            &alive,
                            &closer,
                            &ClientFrame::Input {
                                data: buffer[..read].to_vec(),
                            },
                        )
                        .context("write attached terminal input")?
                        {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error).context("read attached terminal input"),
                }
            }
            Ok(())
        })();
        // Preserve a server/output error over a secondary broken input pipe,
        // but never turn an input failure into success after cleanup.
        workers.finish()?;
        input_result
    }

    fn connect(&self) -> anyhow::Result<DomainStream> {
        match &self.endpoint {
            DomainEndpoint::Local(paths) => connect_local(paths),
            DomainEndpoint::Ssh {
                destination,
                start_server,
            } => connect_ssh(destination, *start_server),
        }
    }

    fn spawn_server(&self) -> anyhow::Result<()> {
        let DomainEndpoint::Local(_) = &self.endpoint else {
            anyhow::bail!("remote domain servers are started by the SSH stdio bridge");
        };
        let executable = std::env::current_exe().context("locate Termeric executable")?;
        let mut command = Command::new(executable);
        command
            .args(["domain", "server", "--foreground"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and does not access Rust-owned
        // memory. It only detaches the child session immediately before exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command
            .spawn()
            .context("start local terminal domain server")?;
        Ok(())
    }
}

fn cancel_upload_attempt(stream: &mut DomainStream) -> Result<Response, UploadAttemptError> {
    protocol::write_frame(stream, &ClientFrame::UploadCancel).map_err(UploadAttemptError::Retry)?;
    match read_frame_with_timeout(stream, RESPONSE_TIMEOUT).map_err(UploadAttemptError::Retry)? {
        ServerFrame::Response {
            response: response @ Response::UploadCancelled { .. },
        } => Ok(response),
        ServerFrame::Error { message } => Err(UploadAttemptError::Fatal(anyhow::anyhow!(
            "upload cancellation failed: {message}"
        ))),
        frame => Err(UploadAttemptError::Fatal(anyhow::anyhow!(
            "unexpected upload cancellation response {frame:?}"
        ))),
    }
}

fn upload_response_matches_kind(response: &Response, kind: UploadKind) -> bool {
    matches!(
        (response, kind),
        (Response::FileUploaded { .. }, UploadKind::File)
            | (
                Response::DirectoryUploaded { .. },
                UploadKind::Directory { .. }
            )
    )
}

fn upload_metadata_unchanged(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    before.is_file()
        && after.is_file()
        && before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.len() == after.len()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
}

fn sha256_prefix(file: &mut std::fs::File, bytes: u64) -> anyhow::Result<String> {
    file.seek(SeekFrom::Start(0))
        .context("rewind local upload source for prefix verification")?;
    let mut hasher = Sha256::new();
    let mut remaining = bytes;
    let mut buffer = vec![0_u8; protocol::MAX_STREAM_CHUNK_BYTES];
    while remaining > 0 {
        let wanted = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded upload digest chunk fits usize");
        let read = file
            .read(&mut buffer[..wanted])
            .context("read local upload prefix")?;
        if read == 0 {
            anyhow::bail!("local upload source ended before the daemon resume offset");
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn report_upload_progress(
    progress: &mut impl FnMut(u64, u64),
    reported: &mut u64,
    bytes_sent: u64,
    total_bytes: u64,
) {
    if bytes_sent > *reported {
        *reported = bytes_sent;
        progress(bytes_sent, total_bytes);
    }
}

fn generate_upload_id() -> anyhow::Result<String> {
    termeric_core::hex::random(16).context("generate upload retry identity")
}

fn connect_local(paths: &RuntimePaths) -> anyhow::Result<DomainStream> {
    let token = paths.read_token()?;
    let mut stream = UnixStream::connect(&paths.socket)
        .with_context(|| format!("connect to local domain server {}", paths.socket.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    protocol::write_frame(
        &mut stream,
        &ClientFrame::Hello {
            version: protocol::PROTOCOL_VERSION,
            token,
        },
    )?;
    match protocol::read_frame::<ServerFrame>(&mut stream)? {
        ServerFrame::Hello { version } if version == protocol::PROTOCOL_VERSION => {}
        ServerFrame::Error { message } => anyhow::bail!("{message}"),
        frame => anyhow::bail!("invalid domain negotiation response {frame:?}"),
    }
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(None)?;
    Ok(DomainStream::Local(stream))
}

fn connect_ssh(destination: &str, start_server: bool) -> anyhow::Result<DomainStream> {
    validate_ssh_destination(destination)?;
    let mut command = Command::new("ssh");
    command.args([
        "-T",
        "-oBatchMode=yes",
        "-oStrictHostKeyChecking=yes",
        "-oConnectTimeout=10",
        "-oServerAliveInterval=15",
        "-oServerAliveCountMax=2",
        "-oClearAllForwardings=yes",
        "-oPermitLocalCommand=no",
        "-oRequestTTY=no",
        "-oEscapeChar=none",
        "--",
        destination,
        "termeric",
        "domain",
        "proxy-stdio",
    ]);
    if !start_server {
        command.arg("--no-start");
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("start OpenSSH transport to {destination}"))?;
    let stdin = child
        .stdin
        .take()
        .context("OpenSSH did not provide stdin")?;
    let stdout = child
        .stdout
        .take()
        .context("OpenSSH did not provide stdout")?;
    let mut stream = DomainStream::Ssh(SshStream {
        child: Some(child),
        stdin: Some(stdin),
        stdout: Some(stdout),
    });
    protocol::write_frame(
        &mut stream,
        &ClientFrame::Hello {
            version: protocol::PROTOCOL_VERSION,
            token: String::new(),
        },
    )?;
    match read_frame_with_timeout(&mut stream, RESPONSE_TIMEOUT)? {
        ServerFrame::Hello { version } if version == protocol::PROTOCOL_VERSION => Ok(stream),
        ServerFrame::Error { message } => {
            anyhow::bail!("SSH domain bridge on {destination}: {message}")
        }
        frame => anyhow::bail!("invalid SSH domain negotiation response {frame:?}"),
    }
}

fn subscribe_topology_stream(
    mut stream: DomainStream,
    domain: String,
    alive: &AtomicBool,
    frame_timeout: Duration,
    mut receive: impl FnMut(DomainTopologyEvent) -> bool,
) -> anyhow::Result<()> {
    if !alive.load(Ordering::Acquire) {
        return Ok(());
    }
    protocol::write_frame(
        &mut stream,
        &ClientFrame::Request {
            request: Request::SubscribeTopology {
                domain: domain.clone(),
            },
        },
    )?;
    let Some(initial) = read_frame_while_alive(&mut stream, frame_timeout, alive)? else {
        return Ok(());
    };
    let initial = match initial {
        ServerFrame::Response {
            response: Response::TopologySubscribed { revision, domain },
        } => DomainTopologyEvent::Snapshot { revision, domain },
        ServerFrame::Error { message } => anyhow::bail!("{message}"),
        frame => anyhow::bail!("unexpected topology subscription response {frame:?}"),
    };
    if !receive(initial) {
        let _ = protocol::write_frame(&mut stream, &ClientFrame::Detach);
        return Ok(());
    }

    while alive.load(Ordering::Acquire) {
        if !stdin_ready(stream.as_raw_fd(), 500)? {
            continue;
        }
        // Idle subscriptions have no expiry. Once bytes arrive, one deadline
        // covers the complete header/body and cannot be renewed by trickling.
        let Some(frame) = read_frame_while_alive(&mut stream, frame_timeout, alive)? else {
            return Ok(());
        };
        let event = match frame {
            ServerFrame::TopologyChanged { revision, domain } => {
                DomainTopologyEvent::Snapshot { revision, domain }
            }
            ServerFrame::DomainClosed { domain, message } => {
                receive(DomainTopologyEvent::Closed { domain, message });
                return Ok(());
            }
            ServerFrame::Error { message } => anyhow::bail!("{message}"),
            frame => anyhow::bail!("unexpected frame in topology subscription {frame:?}"),
        };
        if !receive(event) {
            let _ = protocol::write_frame(&mut stream, &ClientFrame::Detach);
            return Ok(());
        }
    }
    let _ = protocol::write_frame(&mut stream, &ClientFrame::Detach);
    Ok(())
}

#[cfg(test)]
#[path = "client_topology_tests.rs"]
mod topology_tests;

fn read_frame_while_alive<T: serde::de::DeserializeOwned>(
    stream: &mut DomainStream,
    timeout: Duration,
    alive: &AtomicBool,
) -> anyhow::Result<Option<T>> {
    let result = protocol::read_frame(&mut DeadlineReader {
        stream,
        deadline: Instant::now() + timeout,
        alive: Some(alive),
    });
    if !alive.load(Ordering::Acquire) {
        // Discard partial or just-completed input on cancellation. The caller
        // drops this stream; it must not reuse the partially consumed frame.
        Ok(None)
    } else {
        result.map(Some)
    }
}

fn read_frame_with_timeout<T: serde::de::DeserializeOwned>(
    stream: &mut (impl Read + AsRawFd),
    timeout: Duration,
) -> anyhow::Result<T> {
    protocol::read_frame(&mut DeadlineReader {
        stream,
        deadline: Instant::now() + timeout,
        alive: None,
    })
}

/// Bridge one authenticated SSH stdio channel to this user's private local
/// daemon socket. The outer handshake deliberately requires an empty token:
/// only this helper reads the remote token, and it never writes that secret to
/// stdout or across the SSH connection. Without `start_server` it refuses
/// instead of starting a daemon (`stop-server`).
pub fn run_ssh_stdio_bridge(start_server: bool) -> anyhow::Result<()> {
    let mut stdin = bridge::clone_input(libc::STDIN_FILENO)?;
    let mut stdout = std::io::stdout().lock();
    let hello = read_frame_with_timeout::<ClientFrame>(&mut stdin, RESPONSE_TIMEOUT)
        .context("read SSH domain bridge negotiation")?;
    match hello {
        ClientFrame::Hello { version, .. } if version != protocol::PROTOCOL_VERSION => {
            protocol::write_frame(
                &mut stdout,
                &ServerFrame::Error {
                    message: format!(
                        "protocol version {version} is incompatible with bridge version {}",
                        protocol::PROTOCOL_VERSION
                    ),
                },
            )?;
            return Ok(());
        }
        ClientFrame::Hello { token, .. } if !token.is_empty() => {
            protocol::write_frame(
                &mut stdout,
                &ServerFrame::Error {
                    message: "SSH domain bridge does not accept daemon tokens".into(),
                },
            )?;
            return Ok(());
        }
        ClientFrame::Hello { .. } => {}
        _ => {
            protocol::write_frame(
                &mut stdout,
                &ServerFrame::Error {
                    message: "SSH domain bridge requires negotiation before requests".into(),
                },
            )?;
            return Ok(());
        }
    }
    let client = if start_server {
        DomainClient::connect_or_start().map(Some)
    } else {
        DomainClient::connect_running()
    };
    let client = match client {
        Ok(Some(client)) => client,
        Ok(None) => {
            protocol::write_frame(
                &mut stdout,
                &ServerFrame::Error {
                    message: NO_SERVER_RUNNING.into(),
                },
            )?;
            return Ok(());
        }
        Err(error) => {
            protocol::write_frame(
                &mut stdout,
                &ServerFrame::Error {
                    message: format!("remote local-domain endpoint is unavailable: {error:#}"),
                },
            )?;
            return Ok(());
        }
    };
    let stream = match client.connect() {
        Ok(stream) => stream,
        Err(error) => {
            protocol::write_frame(
                &mut stdout,
                &ServerFrame::Error {
                    message: format!("remote local-domain negotiation failed: {error:#}"),
                },
            )?;
            return Ok(());
        }
    };
    protocol::write_frame(
        &mut stdout,
        &ServerFrame::Hello {
            version: protocol::PROTOCOL_VERSION,
        },
    )?;
    let DomainStream::Local(stream) = stream else {
        anyhow::bail!("SSH forwarding bridge requires a local daemon socket");
    };
    bridge::run(stream, &mut stdout, move || stdin)
}

/// Forward a framed byte stream without retaining a partial frame in a
/// line-buffered stdio writer. Multi-stage requests such as uploads require
/// the peer to observe readiness before the source side can produce more data.
fn copy_with_flush(reader: &mut impl Read, writer: &mut impl Write) -> std::io::Result<u64> {
    let mut buffer = [0_u8; 16 * 1024];
    let mut copied = 0_u64;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => return Ok(copied),
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        writer.write_all(&buffer[..read])?;
        writer.flush()?;
        copied = copied.saturating_add(read as u64);
    }
}

struct RawTerminal {
    descriptor: libc::c_int,
    original: Option<libc::termios>,
}

impl RawTerminal {
    fn enter(descriptor: libc::c_int) -> anyhow::Result<Self> {
        // SAFETY: termios calls receive a valid descriptor and initialized
        // storage. ENOTTY is accepted for pipe-based automation.
        unsafe {
            let mut original: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(descriptor, &mut original) != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ENOTTY) {
                    return Ok(Self {
                        descriptor,
                        original: None,
                    });
                }
                return Err(error).context("read relay terminal attributes");
            }
            let mut raw = original;
            libc::cfmakeraw(&mut raw);
            if libc::tcsetattr(descriptor, libc::TCSANOW, &raw) != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("enable raw relay terminal mode");
            }
            Ok(Self {
                descriptor,
                original: Some(original),
            })
        }
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        if let Some(original) = &self.original {
            // SAFETY: the stored termios came from this descriptor and remains
            // valid until this process exits.
            unsafe {
                libc::tcsetattr(self.descriptor, libc::TCSANOW, original);
            }
        }
    }
}

fn terminal_size(descriptor: libc::c_int) -> Option<PtySize> {
    // SAFETY: ioctl writes one winsize value into correctly sized storage.
    unsafe {
        let mut size: libc::winsize = std::mem::zeroed();
        if libc::ioctl(descriptor, libc::TIOCGWINSZ, &mut size) == 0
            && size.ws_col > 0
            && size.ws_row > 0
        {
            Some(PtySize {
                cols: size.ws_col,
                rows: size.ws_row,
                pixel_width: size.ws_xpixel,
                pixel_height: size.ws_ypixel,
            })
        } else {
            None
        }
    }
}

fn stdin_ready(descriptor: libc::c_int, timeout_millis: libc::c_int) -> std::io::Result<bool> {
    let mut descriptor = libc::pollfd {
        fd: descriptor,
        events: libc::POLLIN | libc::POLLHUP,
        revents: 0,
    };
    // SAFETY: poll receives one initialized descriptor for the duration of the
    // call and does not retain its pointer.
    let result = unsafe { libc::poll(&mut descriptor, 1, timeout_millis) };
    if result < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(error);
    }
    Ok(result > 0)
}

#[cfg(test)]
mod tests {
    use super::{
        ClientFrame, DomainClient, DomainStream, Request, Response, ServerFrame, SshStream,
        copy_with_flush, generate_upload_id, protocol, read_frame_with_timeout, sha256_prefix,
        upload_metadata_unchanged, validate_ssh_destination,
    };
    use crate::domain::RuntimePaths;
    use sha2::{Digest, Sha256};
    use std::collections::HashSet;
    use std::fs;
    use std::io::{Read, Seek, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::process::{Command, Stdio};
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    #[test]
    fn a_missing_or_stale_daemon_socket_is_not_a_running_daemon() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::from_directory(temporary.path().join("runtime")).unwrap();
        let client = DomainClient::for_paths(paths.clone());
        assert!(!client.connect_if_running().unwrap());
        // A stale socket with nobody listening is not a running daemon either.
        paths.load_or_create_token().unwrap();
        drop(UnixListener::bind(&paths.socket).unwrap());
        assert!(!client.connect_if_running().unwrap());
    }

    #[test]
    fn ssh_destinations_are_bounded_and_cannot_be_options_or_uris() {
        for valid in [
            "build-host",
            "developer@example.test",
            "[2001:db8::1]:2222",
            "ssh_config_alias",
        ] {
            validate_ssh_destination(valid).unwrap();
        }
        for invalid in [
            "",
            "-oProxyCommand=bad",
            "ssh://example.test",
            "two hosts",
            "host\ncommand",
        ] {
            assert!(validate_ssh_destination(invalid).is_err(), "{invalid:?}");
        }
        assert!(validate_ssh_destination(&"x".repeat(513)).is_err());
    }

    #[test]
    fn generated_upload_ids_are_valid_and_unique_in_one_process() {
        let ids = (0..256)
            .map(|_| generate_upload_id().unwrap())
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), 256);
        assert!(
            ids.iter()
                .all(|upload_id| protocol::validate_upload_id(upload_id).is_ok())
        );
    }

    #[test]
    fn upload_metadata_check_detects_same_size_source_changes() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source.bin");
        fs::write(&source, b"same size").unwrap();
        let before = fs::metadata(&source).unwrap();
        assert!(upload_metadata_unchanged(&before, &before));
        fs::set_permissions(&source, fs::Permissions::from_mode(0o400)).unwrap();
        let after = fs::metadata(&source).unwrap();
        assert_eq!(before.len(), after.len());
        assert!(!upload_metadata_unchanged(&before, &after));
    }

    #[test]
    fn upload_prefix_digest_is_standard_and_leaves_the_resume_cursor() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source.bin");
        fs::write(&source, b"abcdef").unwrap();
        let mut file = fs::File::open(source).unwrap();
        assert_eq!(
            sha256_prefix(&mut file, 3).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(file.stream_position().unwrap(), 3);
    }

    #[test]
    fn partial_transport_frames_cannot_hold_a_client_past_its_deadline() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "printf '\\000\\000'; sleep 1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut stream = DomainStream::Ssh(SshStream {
            child: Some(child),
            stdin: Some(stdin),
            stdout: Some(stdout),
        });
        let started = Instant::now();
        let error = read_frame_with_timeout::<ServerFrame>(&mut stream, Duration::from_millis(40))
            .unwrap_err();
        assert!(error.to_string().contains("frame length"));
        assert!(
            error
                .chain()
                .any(|cause| cause.to_string().contains("deadline elapsed"))
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn interactive_bridge_flushes_each_available_chunk_before_eof() {
        struct ChunkReader {
            remaining: &'static [u8],
        }
        impl Read for ChunkReader {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                let read = self.remaining.len().min(2).min(output.len());
                output[..read].copy_from_slice(&self.remaining[..read]);
                self.remaining = &self.remaining[read..];
                Ok(read)
            }
        }
        #[derive(Default)]
        struct FlushWriter {
            bytes: Vec<u8>,
            flushes: usize,
        }
        impl Write for FlushWriter {
            fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
                self.bytes.extend_from_slice(input);
                Ok(input.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.flushes += 1;
                Ok(())
            }
        }

        let mut reader = ChunkReader {
            remaining: b"ready!",
        };
        let mut writer = FlushWriter::default();
        assert_eq!(copy_with_flush(&mut reader, &mut writer).unwrap(), 6);
        assert_eq!(writer.bytes, b"ready!");
        assert_eq!(writer.flushes, 3);
    }

    #[test]
    fn upload_reconnects_with_one_identity_and_daemon_offset() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::from_directory(temporary.path().join("runtime")).unwrap();
        let token = paths.load_or_create_token().unwrap();
        let listener = match UnixListener::bind(&paths.socket) {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skip upload reconnect fixture: sandbox denies AF_UNIX");
                return;
            }
            Err(error) => panic!("bind upload reconnect fixture: {error}"),
        };
        let expected = (0..100_000)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let expected_for_server = expected.clone();
        let upload_id = "abcdef0123456789abcdef0123456789".to_string();
        let upload_id_for_server = upload_id.clone();
        let server = std::thread::spawn(move || {
            let negotiate = |listener: &UnixListener| {
                let (mut stream, _) = listener.accept().unwrap();
                assert!(matches!(
                    protocol::read_frame::<ClientFrame>(&mut stream).unwrap(),
                    ClientFrame::Hello { version, token: ref received }
                        if version == protocol::PROTOCOL_VERSION && received == &token
                ));
                protocol::write_frame(
                    &mut stream,
                    &ServerFrame::Hello {
                        version: protocol::PROTOCOL_VERSION,
                    },
                )
                .unwrap();
                let ClientFrame::Request { request } =
                    protocol::read_frame::<ClientFrame>(&mut stream).unwrap()
                else {
                    panic!("expected upload request");
                };
                (stream, request)
            };

            let (mut first, first_request) = negotiate(&listener);
            assert!(matches!(
                first_request,
                Request::UploadFile { ref upload_id, total_bytes: 100_000, .. }
                    if upload_id == &upload_id_for_server
            ));
            protocol::write_frame(
                &mut first,
                &ServerFrame::Response {
                    response: Response::UploadReady {
                        max_chunk_bytes: protocol::MAX_STREAM_CHUNK_BYTES,
                        offset: 0,
                        prefix_sha256: format!("{:x}", Sha256::digest([])),
                    },
                },
            )
            .unwrap();
            let ClientFrame::UploadChunk { offset: 0, data } =
                protocol::read_frame::<ClientFrame>(&mut first).unwrap()
            else {
                panic!("expected first upload chunk");
            };
            drop(first);

            let (mut busy, busy_request) = negotiate(&listener);
            assert_eq!(busy_request, first_request);
            protocol::write_frame(
                &mut busy,
                &ServerFrame::Response {
                    response: Response::UploadBusy {
                        retry_after_ms: 100,
                    },
                },
            )
            .unwrap();
            drop(busy);

            let (mut resumed, resumed_request) = negotiate(&listener);
            assert_eq!(resumed_request, first_request);
            protocol::write_frame(
                &mut resumed,
                &ServerFrame::Response {
                    response: Response::UploadReady {
                        max_chunk_bytes: protocol::MAX_STREAM_CHUNK_BYTES,
                        offset: data.len() as u64,
                        prefix_sha256: format!("{:x}", Sha256::digest(&data)),
                    },
                },
            )
            .unwrap();
            let mut received = data;
            while received.len() < expected_for_server.len() {
                let ClientFrame::UploadChunk { offset, data } =
                    protocol::read_frame::<ClientFrame>(&mut resumed).unwrap()
                else {
                    panic!("expected resumed upload chunk");
                };
                assert_eq!(offset, received.len() as u64);
                received.extend(data);
            }
            assert_eq!(
                protocol::read_frame::<ClientFrame>(&mut resumed).unwrap(),
                ClientFrame::UploadCommit
            );
            protocol::write_frame(
                &mut resumed,
                &ServerFrame::Response {
                    response: Response::FileUploaded {
                        path: "/remote/retried.bin".into(),
                        bytes: received.len() as u64,
                    },
                },
            )
            .unwrap();
            assert_eq!(received, expected_for_server);
        });

        let source = temporary.path().join("source.bin");
        fs::write(&source, &expected).unwrap();
        let client = DomainClient::for_paths(paths);
        let cancelled = AtomicBool::new(false);
        let mut progress = Vec::new();
        let response = client
            .upload_open_file_with_test_id(
                "daily".into(),
                7,
                fs::File::open(source).unwrap(),
                "retried.bin".into(),
                upload_id,
                &cancelled,
                |sent, total| progress.push((sent, total)),
            )
            .unwrap();
        assert_eq!(
            response,
            Response::FileUploaded {
                path: "/remote/retried.bin".into(),
                bytes: expected.len() as u64,
            }
        );
        assert!(progress.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        server.join().unwrap();
    }
}
