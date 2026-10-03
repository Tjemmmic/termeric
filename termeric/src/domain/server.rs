//! Local daemon that owns live PTYs independently of GUI processes.

use super::model::*;
use super::paths::{RuntimePaths, effective_uid};
use super::protocol::{
    self, ClientFrame, Request, Response, ServerFrame, UploadKind, UploadRetention, UploadSummary,
};
use anyhow::Context;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use termeric_core::pty::{PtyHandle, PtySize, default_pty_size};
use termeric_core::term::{TerminalMode, XtermKeyboardType};
use termeric_core::{Pty, Terminal, TerminalEvent};

#[path = "server_frame_io.rs"]
mod frame_io;
use frame_io::write_server_frame;

const MAX_CLIENTS: usize = 64;
const MAX_CONCURRENT_UPLOADS: usize = 4;
const UPLOAD_FRAME_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RETAINED_PARTIAL_UPLOADS: usize = 4;
const MAX_RETAINED_UPLOAD_BYTES: u64 =
    protocol::MAX_FILE_UPLOAD_BYTES * MAX_RETAINED_PARTIAL_UPLOADS as u64;
const MAX_COMPLETED_UPLOAD_RECORDS: usize = 32;
const RETAINED_UPLOAD_TTL: Duration = Duration::from_secs(60);
const MANUAL_UPLOAD_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const UPLOAD_JANITOR_INTERVAL: Duration = Duration::from_secs(5);
const UPLOAD_RETRY_DELAY_MILLIS: u16 = 100;
const ATTACH_QUEUE_CAPACITY: usize = 64;
// A full topology snapshot can approach the wire-frame limit when commands
// contain large argument vectors. Retaining only one pending revision per
// subscriber keeps the aggregate queue strictly bounded; a lagging consumer
// reconnects and receives a new atomic snapshot.
const TOPOLOGY_QUEUE_CAPACITY: usize = 1;
const ATTACH_SNAPSHOT_BYTES: usize = 512 * 1024;
// Each item is already bounded by MAX_STREAM_CHUNK_BYTES. Four pending writes
// cap pane-local input backlog at 256 KiB while keeping ordinary paste bursts
// responsive. A stalled child can block only its writer worker, never the
// daemon's shared state or PTY output monitors.
const PANE_INPUT_QUEUE_CAPACITY: usize = 4;

type SharedState = Arc<Mutex<ServerState>>;

struct ServerState {
    domains: HashMap<String, DomainRuntime>,
    allow_screen_checksum_reports: bool,
    xterm_keyboard_type: XtermKeyboardType,
    shutdown: Arc<AtomicBool>,
    wake_socket: Option<std::path::PathBuf>,
    next_attachment: AtomicU64,
    next_topology_subscription: AtomicU64,
    active_uploads: usize,
    upload_ids_in_flight: HashMap<String, UploadDescriptor>,
    upload_records: HashMap<String, UploadRecord>,
    upload_janitor_running: bool,
}

#[derive(Debug, Clone)]
struct UploadDescriptor {
    domain: String,
    pane: u32,
    name: String,
    total_bytes: u64,
    kind: UploadKind,
    retention: UploadRetention,
}

impl UploadDescriptor {
    fn reserved_bytes(&self) -> u64 {
        match self.kind {
            UploadKind::File => self.total_bytes,
            // The daemon briefly owns both the received archive and its
            // extracted tree until atomic publication.
            UploadKind::Directory { .. } => self.total_bytes.saturating_mul(2),
        }
    }

    fn matches_transfer(&self, other: &Self) -> bool {
        self.domain == other.domain
            && self.pane == other.pane
            && self.name == other.name
            && self.total_bytes == other.total_bytes
            && self.kind == other.kind
    }

    fn retention_ttl(&self) -> Duration {
        match self.retention {
            UploadRetention::Automatic => RETAINED_UPLOAD_TTL,
            UploadRetention::Manual => MANUAL_UPLOAD_TTL,
        }
    }
}

#[derive(Debug)]
struct UploadRecord {
    descriptor: UploadDescriptor,
    state: UploadRecordState,
    updated: Instant,
}

#[derive(Debug)]
enum UploadRecordState {
    Partial {
        temporary_path: std::path::PathBuf,
        target_path: std::path::PathBuf,
        bytes_received: u64,
        device: u64,
        inode: u64,
        active: bool,
    },
    Complete {
        response: Response,
    },
}

struct DomainRuntime {
    id: String,
    name: String,
    created_at: String,
    workspace_id: String,
    active_tab: u32,
    next_tab: u32,
    next_pane: u32,
    next_split: u32,
    topology_revision: u64,
    topology_subscribers: Vec<TopologySubscriber>,
    tabs: Vec<TabRuntime>,
}

struct TabRuntime {
    id: u32,
    title: String,
    user_title: Option<String>,
    pinned: bool,
    focused_pane: u32,
    layout: LivePaneLayout,
    panes: HashMap<u32, PaneRuntime>,
}

struct PaneRuntime {
    id: u32,
    title: String,
    command: Vec<String>,
    requested_cwd: Option<String>,
    terminal: Terminal,
    pty: PtyHandle,
    input: SyncSender<Vec<u8>>,
    cols: u16,
    rows: u16,
    pixel_width: u16,
    pixel_height: u16,
    closed: bool,
    sequence: u64,
    attachments: Vec<Attachment>,
}

struct Attachment {
    id: u64,
    sender: SyncSender<ServerFrame>,
    // A cloned descriptor lets queue eviction interrupt both socket workers
    // without waiting for their pending reads/writes or draining stale output.
    transport: UnixStream,
}

impl Attachment {
    fn disconnect(&self) {
        let _ = self.transport.shutdown(std::net::Shutdown::Both);
    }

    fn send_or_disconnect(&self, frame: ServerFrame) -> bool {
        match self.sender.try_send(frame) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.disconnect();
                false
            }
        }
    }
}

struct TopologySubscriber {
    id: u64,
    sender: SyncSender<ServerFrame>,
    transport: UnixStream,
}

impl TopologySubscriber {
    fn disconnect(&self) {
        let _ = self.transport.shutdown(std::net::Shutdown::Both);
    }

    fn send_or_disconnect(&self, frame: ServerFrame) -> bool {
        match self.sender.try_send(frame) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.disconnect();
                false
            }
        }
    }
}

struct TopologySubscriptionGuard {
    state: SharedState,
    domain_id: String,
    subscription_id: u64,
}

impl Drop for TopologySubscriptionGuard {
    fn drop(&mut self) {
        remove_topology_subscription(&self.state, &self.domain_id, self.subscription_id);
    }
}

struct ClientGuard(Arc<AtomicUsize>);

struct UploadSessionGuard {
    state: SharedState,
    upload_id: String,
    disposition: UploadDisposition,
}

enum UploadDisposition {
    Delete,
    Retain { bytes_received: u64 },
    Complete { response: Response },
}

struct TemporaryUpload {
    path: std::path::PathBuf,
    file: File,
    remove_on_drop: bool,
}

struct StagedDirectory {
    path: std::path::PathBuf,
    remove_on_drop: bool,
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl UploadSessionGuard {
    fn retain(&mut self, bytes_received: u64) {
        self.disposition = UploadDisposition::Retain { bytes_received };
    }

    fn complete(&mut self, response: Response) {
        self.disposition = UploadDisposition::Complete { response };
    }
}

impl Drop for UploadSessionGuard {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        state.active_uploads = state.active_uploads.saturating_sub(1);
        state.upload_ids_in_flight.remove(&self.upload_id);
        let mut needs_janitor = false;
        let delete_path = match &self.disposition {
            UploadDisposition::Delete => {
                state
                    .upload_records
                    .remove(&self.upload_id)
                    .and_then(|record| match record.state {
                        UploadRecordState::Partial { temporary_path, .. } => Some(temporary_path),
                        UploadRecordState::Complete { .. } => None,
                    })
            }
            UploadDisposition::Retain { bytes_received } => {
                if let Some(UploadRecord {
                    state:
                        UploadRecordState::Partial {
                            bytes_received: retained_bytes,
                            active,
                            ..
                        },
                    updated,
                    ..
                }) = state.upload_records.get_mut(&self.upload_id)
                {
                    *retained_bytes = *bytes_received;
                    *active = false;
                    *updated = Instant::now();
                    needs_janitor = true;
                }
                None
            }
            UploadDisposition::Complete { response } => {
                if let Some(record) = state.upload_records.get_mut(&self.upload_id) {
                    record.state = UploadRecordState::Complete {
                        response: response.clone(),
                    };
                    record.updated = Instant::now();
                }
                prune_completed_upload_records(&mut state);
                needs_janitor = true;
                None
            }
        };
        drop(state);
        if let Some(path) = delete_path {
            let _ = fs::remove_file(path);
        }
        if needs_janitor {
            ensure_upload_janitor(&self.state);
        }
    }
}

impl Drop for TemporaryUpload {
    fn drop(&mut self) {
        if self.remove_on_drop {
            let _ = fs::remove_file(&self.path);
        }
    }
}

impl Drop for StagedDirectory {
    fn drop(&mut self) {
        if self.remove_on_drop {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn prune_completed_upload_records(state: &mut ServerState) {
    while state
        .upload_records
        .values()
        .filter(|record| matches!(record.state, UploadRecordState::Complete { .. }))
        .count()
        > MAX_COMPLETED_UPLOAD_RECORDS
    {
        let Some(oldest) = state
            .upload_records
            .iter()
            .filter(|(_, record)| matches!(record.state, UploadRecordState::Complete { .. }))
            .min_by_key(|(_, record)| record.updated)
            .map(|(upload_id, _)| upload_id.clone())
        else {
            break;
        };
        state.upload_records.remove(&oldest);
    }
}

fn retained_upload_capacity_available(state: &ServerState, descriptor: &UploadDescriptor) -> bool {
    let retained_count = state
        .upload_records
        .values()
        .filter(|record| matches!(record.state, UploadRecordState::Partial { .. }))
        .count()
        + state.upload_ids_in_flight.len();
    let reserved_bytes = state
        .upload_records
        .values()
        .filter(|record| matches!(record.state, UploadRecordState::Partial { .. }))
        .map(|record| record.descriptor.reserved_bytes())
        .chain(
            state
                .upload_ids_in_flight
                .values()
                .map(UploadDescriptor::reserved_bytes),
        )
        .sum::<u64>();
    retained_count < MAX_RETAINED_PARTIAL_UPLOADS
        && reserved_bytes.saturating_add(descriptor.reserved_bytes()) <= MAX_RETAINED_UPLOAD_BYTES
}

fn prune_expired_upload_records(state: &SharedState, remove_all: bool) {
    let paths = {
        let now = Instant::now();
        let mut state = state.lock().unwrap();
        let expired = state
            .upload_records
            .iter()
            .filter_map(|(upload_id, record)| {
                let active = matches!(
                    record.state,
                    UploadRecordState::Partial { active: true, .. }
                );
                let ttl = if matches!(record.state, UploadRecordState::Complete { .. }) {
                    RETAINED_UPLOAD_TTL
                } else {
                    record.descriptor.retention_ttl()
                };
                (remove_all || (now.saturating_duration_since(record.updated) >= ttl && !active))
                    .then_some(upload_id.clone())
            })
            .collect::<Vec<_>>();
        expired
            .into_iter()
            .filter_map(|upload_id| {
                state
                    .upload_records
                    .remove(&upload_id)
                    .and_then(|record| match record.state {
                        UploadRecordState::Partial { temporary_path, .. } => Some(temporary_path),
                        UploadRecordState::Complete { .. } => None,
                    })
            })
            .collect::<Vec<_>>()
    };
    for path in paths {
        let _ = fs::remove_file(path);
    }
}

fn ensure_upload_janitor(state: &SharedState) {
    let weak_state = Arc::downgrade(state);
    let shutdown = {
        let mut state_guard = state.lock().unwrap();
        if state_guard.upload_janitor_running || state_guard.upload_records.is_empty() {
            return;
        }
        state_guard.upload_janitor_running = true;
        Arc::clone(&state_guard.shutdown)
    };
    let spawn = thread::Builder::new()
        .name("termeric-upload-janitor".into())
        .spawn(move || {
            loop {
                thread::sleep(UPLOAD_JANITOR_INTERVAL);
                let Some(state) = weak_state.upgrade() else {
                    return;
                };
                if shutdown.load(Ordering::Acquire) {
                    prune_expired_upload_records(&state, true);
                } else {
                    prune_expired_upload_records(&state, false);
                }
                let mut state = state.lock().unwrap();
                if shutdown.load(Ordering::Acquire) || state.upload_records.is_empty() {
                    state.upload_janitor_running = false;
                    return;
                }
            }
        });
    if let Err(error) = spawn {
        state.lock().unwrap().upload_janitor_running = false;
        log::warn!("Could not start retained-upload janitor: {error}");
    }
}

fn list_retained_uploads(state: &SharedState) -> Response {
    prune_expired_upload_records(state, false);
    let now = Instant::now();
    let state = state.lock().unwrap();
    let mut uploads = state
        .upload_records
        .iter()
        .filter_map(|(upload_id, record)| {
            let (bytes_received, active) = match &record.state {
                UploadRecordState::Partial {
                    bytes_received,
                    active,
                    ..
                } => (*bytes_received, *active),
                UploadRecordState::Complete { .. } => return None,
            };
            let remaining = record
                .descriptor
                .retention_ttl()
                .saturating_sub(now.saturating_duration_since(record.updated));
            Some(UploadSummary {
                upload_id: upload_id.clone(),
                domain: record.descriptor.domain.clone(),
                pane: record.descriptor.pane,
                name: record.descriptor.name.clone(),
                total_bytes: record.descriptor.total_bytes,
                bytes_received,
                kind: record.descriptor.kind,
                retention: record.descriptor.retention,
                active,
                expires_in_seconds: remaining.as_secs(),
            })
        })
        .collect::<Vec<_>>();
    uploads.sort_by(|left, right| left.upload_id.cmp(&right.upload_id));
    Response::Uploads { uploads }
}

fn cancel_retained_upload(state: &SharedState, upload_id: &str) -> anyhow::Result<Response> {
    protocol::validate_upload_id(upload_id)?;
    prune_expired_upload_records(state, false);
    let path = {
        let mut state = state.lock().unwrap();
        let record = state
            .upload_records
            .get_mut(upload_id)
            .context("retained upload was not found or has expired")?;
        match &mut record.state {
            UploadRecordState::Partial { active: true, .. } => {
                anyhow::bail!("upload is active; cancel it from the originating client")
            }
            UploadRecordState::Partial {
                temporary_path,
                active,
                ..
            } => {
                *active = true;
                temporary_path.clone()
            }
            UploadRecordState::Complete { .. } => {
                anyhow::bail!("upload is already complete and cannot be cancelled")
            }
        }
    };
    if let Err(error) = fs::remove_file(&path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        if let Some(UploadRecord {
            state: UploadRecordState::Partial { active, .. },
            ..
        }) = state.lock().unwrap().upload_records.get_mut(upload_id)
        {
            *active = false;
        }
        return Err(error).context("remove retained upload staging file");
    }
    state.lock().unwrap().upload_records.remove(upload_id);
    Ok(Response::Ok {
        message: format!("cancelled retained upload {upload_id}"),
    })
}

impl TemporaryUpload {
    fn preserve(&mut self) {
        self.remove_on_drop = false;
    }
}

pub fn run_foreground(
    paths: RuntimePaths,
    allow_screen_checksum_reports: bool,
    xterm_keyboard_type: XtermKeyboardType,
) -> anyhow::Result<()> {
    log::debug!("Domain runtime directory: {}", paths.directory.display());
    let token = paths.load_or_create_token()?;
    prepare_socket_path(&paths)?;
    let listener = UnixListener::bind(&paths.socket)
        .with_context(|| format!("bind local domain socket {}", paths.socket.display()))?;
    fs::set_permissions(&paths.socket, fs::Permissions::from_mode(0o600))?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let state = Arc::new(Mutex::new(ServerState {
        domains: HashMap::new(),
        allow_screen_checksum_reports,
        xterm_keyboard_type,
        shutdown: Arc::clone(&shutdown),
        wake_socket: Some(paths.socket.clone()),
        next_attachment: AtomicU64::new(1),
        next_topology_subscription: AtomicU64::new(1),
        active_uploads: 0,
        upload_ids_in_flight: HashMap::new(),
        upload_records: HashMap::new(),
        upload_janitor_running: false,
    }));
    let clients = Arc::new(AtomicUsize::new(0));
    log::info!(
        "Local terminal domain server listening at {}",
        paths.socket.display()
    );

    while !shutdown.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                if clients.fetch_add(1, Ordering::AcqRel) >= MAX_CLIENTS {
                    clients.fetch_sub(1, Ordering::AcqRel);
                    let mut stream = stream;
                    let _ = write_server_frame(
                        &mut stream,
                        &ServerFrame::Error {
                            message: "domain client limit reached".into(),
                        },
                    );
                    continue;
                }
                let state = Arc::clone(&state);
                let token = token.clone();
                let guard = ClientGuard(Arc::clone(&clients));
                thread::Builder::new()
                    .name("termeric-domain-client".into())
                    .spawn(move || {
                        let _guard = guard;
                        if let Err(error) = handle_client(stream, &token, state) {
                            log::debug!("Domain client disconnected: {error:#}");
                        }
                    })?;
            }
            Err(error) => return Err(error).context("accept local domain client"),
        }
    }

    drop(listener);
    prune_expired_upload_records(&state, true);
    if let Err(error) = fs::remove_file(&paths.socket)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!("Could not remove local domain socket: {error}");
    }
    Ok(())
}

fn prepare_socket_path(paths: &RuntimePaths) -> anyhow::Result<()> {
    let Ok(metadata) = fs::symlink_metadata(&paths.socket) else {
        return Ok(());
    };
    if !metadata.file_type().is_socket() || metadata.uid() != effective_uid() {
        anyhow::bail!(
            "refusing to replace unsafe domain socket {}",
            paths.socket.display()
        );
    }
    if UnixStream::connect(&paths.socket).is_ok() {
        anyhow::bail!("a local domain server is already running");
    }
    fs::remove_file(&paths.socket).context("remove stale local domain socket")?;
    Ok(())
}

fn handle_client(
    stream: UnixStream,
    expected_token: &str,
    state: SharedState,
) -> anyhow::Result<()> {
    handle_client_with_setup_timeout(stream, expected_token, state, Duration::from_secs(10))
}

fn read_client_frame_with_timeout(
    stream: &mut UnixStream,
    timeout: Duration,
) -> anyhow::Result<ClientFrame> {
    struct FrameReader<'a> {
        stream: &'a mut UnixStream,
        deadline: Instant,
    }
    impl Read for FrameReader<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "domain client frame read deadline exceeded",
                ));
            }
            self.stream.set_read_timeout(Some(remaining))?;
            self.stream.read(bytes)
        }
    }

    // A per-read timeout alone can be renewed forever by partial-frame input.
    // Restore socket policy on both success and failure; an established relay
    // must not inherit the setup deadline as an idle-disconnect timer.
    let previous = stream.read_timeout()?;
    let result = protocol::read_frame(&mut FrameReader {
        stream,
        deadline: Instant::now() + timeout,
    });
    stream.set_read_timeout(previous)?;
    result
}

fn handle_client_with_setup_timeout(
    mut stream: UnixStream,
    expected_token: &str,
    state: SharedState,
    setup_timeout: Duration,
) -> anyhow::Result<()> {
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    validate_peer(&stream)?;
    if let Err(message) = validate_hello(
        read_client_frame_with_timeout(&mut stream, setup_timeout)?,
        expected_token,
    ) {
        write_server_frame(&mut stream, &ServerFrame::Error { message })?;
        return Ok(());
    }
    write_server_frame(
        &mut stream,
        &ServerFrame::Hello {
            version: protocol::PROTOCOL_VERSION,
        },
    )?;
    // The server-frame writer uses this as one complete-frame budget and
    // preserves it between sends. Streaming queues also enforce capacity.
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let ClientFrame::Request { request } =
        read_client_frame_with_timeout(&mut stream, setup_timeout)?
    else {
        anyhow::bail!("expected one domain request after negotiation");
    };
    stream.set_read_timeout(None)?;
    if let Request::Attach { domain, pane } = request {
        return serve_attachment(stream, state, domain, pane);
    }
    if let Request::SubscribeTopology { domain } = request {
        return serve_topology_subscription(stream, state, domain);
    }
    if let Request::UploadFile {
        domain,
        pane,
        name,
        total_bytes,
        upload_id,
        kind,
        retention,
        resume_only,
    } = request
    {
        return serve_file_upload(
            stream,
            state,
            domain,
            pane,
            name,
            total_bytes,
            upload_id,
            kind,
            retention,
            resume_only,
        );
    }
    if matches!(&request, Request::Shutdown) {
        return serve_shutdown(stream, state);
    }
    let response = process_request(&state, request);
    match response {
        Ok(response) => write_server_frame(&mut stream, &ServerFrame::Response { response }),
        Err(error) => write_server_frame(
            &mut stream,
            &ServerFrame::Error {
                message: format!("{error:#}"),
            },
        ),
    }
}

fn serve_shutdown(mut stream: UnixStream, state: SharedState) -> anyhow::Result<()> {
    let wake_socket = {
        let state = state.lock().unwrap();
        if !state.domains.is_empty() {
            drop(state);
            write_server_frame(
                &mut stream,
                &ServerFrame::Error {
                    message: "refusing to stop the server while live domains exist".into(),
                },
            )?;
            return Ok(());
        }

        // Keep the empty-domain state boundary until the complete response is
        // in the socket. Publishing shutdown first can let the daemon process
        // exit and terminate this handler midway through the frame.
        write_server_frame(
            &mut stream,
            &ServerFrame::Response {
                response: Response::Ok {
                    message: "domain server stopping".into(),
                },
            },
        )?;
        state.shutdown.store(true, Ordering::Release);
        state.wake_socket.clone()
    };

    // The accept loop sleeps in the kernel while idle. One private connection
    // wakes it so it can observe the already-published shutdown immediately.
    if let Some(socket) = wake_socket {
        let _ = UnixStream::connect(socket);
    }
    Ok(())
}

fn serve_file_upload(
    mut stream: UnixStream,
    state: SharedState,
    domain: String,
    pane: u32,
    name: String,
    total_bytes: u64,
    upload_id: String,
    kind: UploadKind,
    retention: UploadRetention,
    resume_only: bool,
) -> anyhow::Result<()> {
    stream.set_read_timeout(Some(UPLOAD_FRAME_TIMEOUT))?;
    let result = receive_file_upload(
        &mut stream,
        state,
        &domain,
        pane,
        &name,
        total_bytes,
        &upload_id,
        kind,
        retention,
        resume_only,
    );
    match result {
        Ok(response) => write_server_frame(&mut stream, &ServerFrame::Response { response }),
        Err(error) => write_server_frame(
            &mut stream,
            &ServerFrame::Error {
                message: format!("{error:#}"),
            },
        ),
    }
}

fn read_upload_frame(stream: &mut UnixStream) -> anyhow::Result<ClientFrame> {
    let timeout = stream.read_timeout()?.unwrap_or(UPLOAD_FRAME_TIMEOUT);
    read_client_frame_with_timeout(stream, timeout)
}

fn receive_file_upload(
    stream: &mut UnixStream,
    state: SharedState,
    domain_id: &str,
    pane_id: u32,
    name: &str,
    total_bytes: u64,
    upload_id: &str,
    kind: UploadKind,
    retention: UploadRetention,
    resume_only: bool,
) -> anyhow::Result<Response> {
    validate_id("domain id", domain_id)?;
    protocol::validate_upload_name(name)?;
    protocol::validate_upload_id(upload_id)?;
    if total_bytes > protocol::MAX_FILE_UPLOAD_BYTES {
        anyhow::bail!(
            "upload exceeds the {} MiB transfer limit",
            protocol::MAX_FILE_UPLOAD_BYTES / (1024 * 1024)
        );
    }
    if let UploadKind::Directory { entries } = kind
        && entries > super::archive::MAX_DIRECTORY_ENTRIES
    {
        anyhow::bail!(
            "directory upload exceeds the {}-entry limit",
            super::archive::MAX_DIRECTORY_ENTRIES
        );
    }
    let descriptor = UploadDescriptor {
        domain: domain_id.to_string(),
        pane: pane_id,
        name: name.to_string(),
        total_bytes,
        kind,
        retention,
    };
    prune_expired_upload_records(&state, false);

    enum UploadStart {
        Resume {
            temporary_path: std::path::PathBuf,
            target_path: std::path::PathBuf,
            bytes_received: u64,
            device: u64,
            inode: u64,
        },
        New {
            directory: std::path::PathBuf,
            target_path: std::path::PathBuf,
        },
    }

    let start = {
        let mut state_guard = state.lock().unwrap();
        let at_active_capacity = state_guard.active_uploads >= MAX_CONCURRENT_UPLOADS;
        let start = if let Some(record) = state_guard.upload_records.get_mut(upload_id) {
            if !record.descriptor.matches_transfer(&descriptor) {
                anyhow::bail!("upload id belongs to a different source or destination");
            }
            if retention == UploadRetention::Manual {
                record.descriptor.retention = UploadRetention::Manual;
            }
            let start = match &mut record.state {
                UploadRecordState::Complete { response } => {
                    return Ok(response.clone());
                }
                UploadRecordState::Partial { active: true, .. } => {
                    return Ok(Response::UploadBusy {
                        retry_after_ms: UPLOAD_RETRY_DELAY_MILLIS,
                    });
                }
                UploadRecordState::Partial {
                    temporary_path,
                    target_path,
                    bytes_received,
                    device,
                    inode,
                    active,
                } => {
                    if at_active_capacity {
                        return Ok(Response::UploadBusy {
                            retry_after_ms: UPLOAD_RETRY_DELAY_MILLIS,
                        });
                    }
                    *active = true;
                    UploadStart::Resume {
                        temporary_path: temporary_path.clone(),
                        target_path: target_path.clone(),
                        bytes_received: *bytes_received,
                        device: *device,
                        inode: *inode,
                    }
                }
            };
            record.updated = Instant::now();
            start
        } else {
            if resume_only {
                anyhow::bail!("retained upload was not found or has expired");
            }
            if at_active_capacity {
                return Ok(Response::UploadBusy {
                    retry_after_ms: UPLOAD_RETRY_DELAY_MILLIS,
                });
            }
            if state_guard.upload_ids_in_flight.contains_key(upload_id) {
                return Ok(Response::UploadBusy {
                    retry_after_ms: UPLOAD_RETRY_DELAY_MILLIS,
                });
            }
            if !retained_upload_capacity_available(&state_guard, &descriptor) {
                anyhow::bail!("domain retained-upload capacity is currently full");
            }
            let pane = find_pane(&state_guard, domain_id, pane_id)?;
            if pane.closed {
                anyhow::bail!("pane is closed");
            }
            let cwd = crate::tabs::terminal_working_directory(&pane.terminal)
                .or_else(|| pane.pty.child_id().and_then(crate::tabs::get_process_cwd))
                .or_else(|| pane.requested_cwd.clone())
                .context("focused pane has no known working directory")?;
            let directory = std::path::PathBuf::from(cwd);
            if !directory.is_absolute() || !directory.is_dir() {
                anyhow::bail!(
                    "focused pane working directory is not an available absolute directory"
                );
            }
            let target_path = directory.join(name);
            state_guard
                .upload_ids_in_flight
                .insert(upload_id.to_string(), descriptor.clone());
            UploadStart::New {
                directory,
                target_path,
            }
        };
        state_guard.active_uploads += 1;
        start
    };
    let mut session = UploadSessionGuard {
        state: Arc::clone(&state),
        upload_id: upload_id.to_string(),
        disposition: UploadDisposition::Delete,
    };

    let (mut temporary, target, mut offset) = match start {
        UploadStart::Resume {
            temporary_path,
            target_path,
            bytes_received,
            device,
            inode,
        } => {
            if fs::symlink_metadata(&target_path).is_ok() {
                anyhow::bail!("destination appeared before upload resume; uploads never overwrite");
            }
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&temporary_path)
                .context("reopen retained upload staging file")?;
            let metadata = file
                .metadata()
                .context("inspect retained upload staging file")?;
            if !metadata.is_file()
                || metadata.uid() != effective_uid()
                || metadata.dev() != device
                || metadata.ino() != inode
                || metadata.len() != bytes_received
                || metadata.permissions().mode() & 0o777 != 0o600
            {
                anyhow::bail!("retained upload staging identity or size changed");
            }
            file.seek(SeekFrom::Start(bytes_received))
                .context("seek retained upload staging file")?;
            (
                TemporaryUpload {
                    path: temporary_path,
                    file,
                    remove_on_drop: true,
                },
                target_path,
                bytes_received,
            )
        }
        UploadStart::New {
            directory,
            target_path,
        } => {
            if fs::symlink_metadata(&target_path).is_ok() {
                anyhow::bail!("destination already exists; uploads never overwrite destinations");
            }
            let staging_id = crate::session::generate_session_id();
            let mut temporary = None;
            for attempt in 0..16_u8 {
                let path = directory.join(format!(".termeric-upload-{staging_id}-{attempt}"));
                match OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&path)
                {
                    Ok(file) => {
                        file.set_permissions(fs::Permissions::from_mode(0o600))?;
                        temporary = Some(TemporaryUpload {
                            path,
                            file,
                            remove_on_drop: true,
                        });
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => {
                        return Err(error).context("create temporary upload in pane directory");
                    }
                }
            }
            let temporary =
                temporary.context("could not allocate a unique temporary upload file")?;
            let metadata = temporary
                .file
                .metadata()
                .context("inspect new upload staging file")?;
            {
                let mut state_guard = state.lock().unwrap();
                state_guard.upload_ids_in_flight.remove(upload_id);
                state_guard.upload_records.insert(
                    upload_id.to_string(),
                    UploadRecord {
                        descriptor: descriptor.clone(),
                        state: UploadRecordState::Partial {
                            temporary_path: temporary.path.clone(),
                            target_path: target_path.clone(),
                            bytes_received: 0,
                            device: metadata.dev(),
                            inode: metadata.ino(),
                            active: true,
                        },
                        updated: Instant::now(),
                    },
                );
            }
            (temporary, target_path, 0)
        }
    };
    let prefix_sha256 = sha256_prefix(&mut temporary.file, offset)?;
    if let Err(error) = write_server_frame(
        stream,
        &ServerFrame::Response {
            response: Response::UploadReady {
                max_chunk_bytes: protocol::MAX_STREAM_CHUNK_BYTES,
                offset,
                prefix_sha256,
            },
        },
    ) {
        temporary.preserve();
        session.retain(offset);
        return Err(error);
    }

    while offset < total_bytes {
        let frame = match read_upload_frame(stream) {
            Ok(frame) => frame,
            Err(error) => {
                temporary.preserve();
                session.retain(offset);
                return Err(error);
            }
        };
        let (frame_offset, data) = match frame {
            ClientFrame::UploadChunk { offset, data } => (offset, data),
            ClientFrame::UploadCancel => {
                return Ok(Response::UploadCancelled {
                    bytes_received: offset,
                });
            }
            _ => anyhow::bail!("upload stream ended before all bytes arrived"),
        };
        if frame_offset != offset {
            anyhow::bail!("upload chunk offset is not contiguous");
        }
        if data.is_empty() || data.len() > protocol::MAX_STREAM_CHUNK_BYTES {
            anyhow::bail!("upload chunk is empty or exceeds the per-frame limit");
        }
        if offset.saturating_add(data.len() as u64) > total_bytes {
            anyhow::bail!("upload chunk exceeds the declared file size");
        }
        temporary
            .file
            .write_all(&data)
            .context("write temporary upload")?;
        offset += data.len() as u64;
    }
    let final_frame = match read_upload_frame(stream) {
        Ok(frame) => frame,
        Err(error) => {
            temporary.preserve();
            session.retain(offset);
            return Err(error);
        }
    };
    match final_frame {
        ClientFrame::UploadCommit => {}
        ClientFrame::UploadCancel => {
            return Ok(Response::UploadCancelled {
                bytes_received: offset,
            });
        }
        _ => anyhow::bail!("upload requires an explicit commit after the final byte"),
    }
    let path = target
        .to_str()
        .context("uploaded destination is not valid UTF-8")?
        .to_string();
    let response = match kind {
        UploadKind::File => {
            publish_temporary_upload(&mut temporary, &target)?;
            Response::FileUploaded {
                path,
                bytes: total_bytes,
            }
        }
        UploadKind::Directory { entries } => {
            let mut staging = create_staged_directory(&target)?;
            temporary
                .file
                .flush()
                .context("flush temporary directory archive")?;
            temporary
                .file
                .sync_all()
                .context("sync temporary directory archive")?;
            super::archive::extract_directory_archive(&mut temporary.file, &staging.path, entries)?;
            sync_directory(&staging.path)?;
            publish_staged_directory(&mut staging, &target)?;
            Response::DirectoryUploaded {
                path,
                bytes: total_bytes,
                entries,
            }
        }
    };
    session.complete(response.clone());
    Ok(response)
}

fn sha256_prefix(file: &mut File, bytes: u64) -> anyhow::Result<String> {
    file.seek(SeekFrom::Start(0))
        .context("rewind retained upload for prefix verification")?;
    let mut hasher = Sha256::new();
    let mut remaining = bytes;
    let mut buffer = vec![0_u8; protocol::MAX_STREAM_CHUNK_BYTES];
    while remaining > 0 {
        let wanted = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded upload digest chunk fits usize");
        let read = file
            .read(&mut buffer[..wanted])
            .context("read retained upload prefix")?;
        if read == 0 {
            anyhow::bail!("retained upload ended before its recorded offset");
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    file.seek(SeekFrom::Start(bytes))
        .context("restore retained upload offset after verification")?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn publish_temporary_upload(
    temporary: &mut TemporaryUpload,
    target: &std::path::Path,
) -> anyhow::Result<()> {
    temporary.file.flush().context("flush temporary upload")?;
    temporary.file.sync_all().context("sync temporary upload")?;
    fs::hard_link(&temporary.path, target)
        .context("publish upload without overwrite (the destination may already exist)")
}

fn create_staged_directory(target: &std::path::Path) -> anyhow::Result<StagedDirectory> {
    let parent = target
        .parent()
        .context("upload destination has no parent")?;
    let staging_id = crate::session::generate_session_id();
    for attempt in 0..16_u8 {
        let path = parent.join(format!(".termeric-directory-upload-{staging_id}-{attempt}"));
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(&path) {
            Ok(()) => {
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
                return Ok(StagedDirectory {
                    path,
                    remove_on_drop: true,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("create staged upload directory"),
        }
    }
    anyhow::bail!("could not allocate a unique staged upload directory")
}

fn sync_directory(path: &std::path::Path) -> anyhow::Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("sync staged upload directory {}", path.display()))
}

#[cfg(target_os = "linux")]
fn publish_staged_directory(
    staging: &mut StagedDirectory,
    target: &std::path::Path,
) -> anyhow::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source = CString::new(staging.path.as_os_str().as_bytes())
        .context("staged upload path contains NUL")?;
    let destination =
        CString::new(target.as_os_str().as_bytes()).context("upload destination contains NUL")?;
    // SAFETY: both arguments are NUL-terminated C strings. RENAME_NOREPLACE
    // provides the required atomic publish-without-overwrite operation.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error())
            .context("publish directory upload without overwrite");
    }
    staging.remove_on_drop = false;
    if let Some(parent) = target.parent() {
        // Publication has already succeeded and cannot be rolled back safely.
        // A best-effort parent sync improves crash durability without turning
        // a committed upload into a misleading retryable error.
        let _ = sync_directory(parent);
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn publish_staged_directory(
    _staging: &mut StagedDirectory,
    _target: &std::path::Path,
) -> anyhow::Result<()> {
    anyhow::bail!("atomic directory upload publication currently requires Linux renameat2")
}

fn validate_hello(frame: ClientFrame, expected_token: &str) -> Result<(), String> {
    match frame {
        ClientFrame::Hello { version, .. } if version != protocol::PROTOCOL_VERSION => {
            Err(format!(
                "protocol version {version} is incompatible with server version {}",
                protocol::PROTOCOL_VERSION
            ))
        }
        ClientFrame::Hello { token, .. } if token != expected_token => {
            Err("domain authentication failed".into())
        }
        ClientFrame::Hello { .. } => Ok(()),
        _ => Err("domain client did not negotiate before requesting access".into()),
    }
}

#[cfg(target_os = "linux")]
fn validate_peer(stream: &UnixStream) -> anyhow::Result<()> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the socket descriptor is valid for this call and the output
    // buffer and length point to initialized, correctly sized storage.
    let result = unsafe {
        libc::getsockopt(
            std::os::fd::AsRawFd::as_raw_fd(stream),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("authenticate domain peer owner");
    }
    if credentials.uid != effective_uid() {
        anyhow::bail!("domain peer belongs to a different user");
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "linux")))]
fn validate_peer(stream: &UnixStream) -> anyhow::Result<()> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: getpeereid receives a valid connected Unix socket and two valid
    // output pointers. It does not retain either pointer.
    let result =
        unsafe { libc::getpeereid(std::os::fd::AsRawFd::as_raw_fd(stream), &mut uid, &mut gid) };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("authenticate domain peer owner");
    }
    if uid != effective_uid() {
        anyhow::bail!("domain peer belongs to a different user");
    }
    Ok(())
}

fn process_request(state: &SharedState, request: Request) -> anyhow::Result<Response> {
    match request {
        Request::ListDomains => {
            let state = state.lock().unwrap();
            let mut domains: Vec<_> = state.domains.values().map(DomainRuntime::summary).collect();
            domains.sort_by(|left, right| left.id.cmp(&right.id));
            Ok(Response::Domains { domains })
        }
        Request::ListWorkspaces { domain } => {
            validate_id("domain id", &domain)?;
            let state = state.lock().unwrap();
            let domain = state.domains.get(&domain).context("domain not found")?;
            Ok(Response::Workspaces {
                workspaces: vec![domain.workspace_snapshot()],
            })
        }
        Request::ListPanes { domain } => {
            validate_id("domain id", &domain)?;
            let state = state.lock().unwrap();
            let domain = state.domains.get(&domain).context("domain not found")?;
            Ok(Response::Panes {
                panes: domain.pane_snapshots(),
            })
        }
        Request::Describe { domain } => {
            validate_id("domain id", &domain)?;
            let state = state.lock().unwrap();
            Ok(Response::Domain {
                domain: state
                    .domains
                    .get(&domain)
                    .context("domain not found")?
                    .snapshot(),
            })
        }
        Request::CreateDomain { name, spawn } => create_domain(state, name, spawn),
        Request::SpawnTab { domain, spawn } => spawn_tab(state, &domain, spawn),
        Request::SplitPane {
            domain,
            pane,
            direction,
            spawn,
        } => split_pane(state, &domain, pane, direction, spawn),
        Request::ResizeSplit {
            domain,
            split,
            ratio_millis,
        } => resize_domain_split(state, &domain, split, ratio_millis),
        Request::RenameTab { domain, tab, title } => rename_domain_tab(state, &domain, tab, title),
        Request::SetTabPinned {
            domain,
            tab,
            pinned,
        } => set_domain_tab_pinned(state, &domain, tab, pinned),
        Request::MoveTab { domain, tab, index } => {
            move_domain_tab(state, &domain, tab, usize::from(index))
        }
        Request::SwapPaneNext { domain, pane } => swap_domain_pane_next(state, &domain, pane),
        Request::BalanceTab { domain, tab } => balance_domain_tab(state, &domain, tab),
        Request::ApplyPaneLayout {
            domain,
            tab,
            preset,
        } => apply_domain_pane_layout(state, &domain, tab, preset),
        Request::MovePane {
            domain,
            pane,
            tab,
            target,
            direction,
        } => move_domain_pane(state, &domain, pane, tab, target, direction),
        Request::FocusPane { domain, pane } => {
            validate_id("domain id", &domain)?;
            let mut state = state.lock().unwrap();
            let domain = state.domains.get_mut(&domain).context("domain not found")?;
            let tab_id = domain
                .tabs
                .iter()
                .find(|tab| tab.panes.contains_key(&pane))
                .map(|tab| tab.id)
                .context("pane not found")?;
            let changed = domain.active_tab != tab_id
                || domain
                    .tabs
                    .iter()
                    .find(|tab| tab.id == tab_id)
                    .is_some_and(|tab| tab.focused_pane != pane);
            if changed {
                let tab = domain.tabs.iter_mut().find(|tab| tab.id == tab_id).unwrap();
                tab.focused_pane = pane;
                tab.refresh_title();
                domain.active_tab = tab_id;
                domain.publish_topology();
            }
            Ok(Response::Ok {
                message: format!("focused domain {} pane {pane}", domain.id),
            })
        }
        Request::ClosePane { domain, pane } => remove_domain_pane(state, &domain, pane),
        Request::CloseTab { domain, tab } => remove_domain_tab(state, &domain, tab),
        Request::SendText { domain, pane, data } => {
            if data.len() > protocol::MAX_STREAM_CHUNK_BYTES {
                anyhow::bail!("input exceeds the per-request limit");
            }
            let bytes = data.len();
            let state = state.lock().unwrap();
            let pane = find_pane(&state, &domain, pane)?;
            queue_pane_input(&pane.input, data)?;
            Ok(Response::Ok {
                message: format!("sent {bytes} bytes"),
            })
        }
        Request::SendPaste { domain, pane, text } => {
            if text.len() > protocol::MAX_STREAM_CHUNK_BYTES - 16 {
                anyhow::bail!("paste exceeds the per-request limit");
            }
            let state = state.lock().unwrap();
            let pane = find_pane(&state, &domain, pane)?;
            let data = crate::input::encode_paste(pane.terminal.mode(), &text);
            let bytes = data.len();
            queue_pane_input(&pane.input, data)?;
            Ok(Response::Ok {
                message: format!("sent {bytes} paste bytes"),
            })
        }
        Request::GetText {
            domain,
            pane,
            max_bytes,
        } => {
            let state = state.lock().unwrap();
            let pane = find_pane(&state, &domain, pane)?;
            let maximum = max_bytes.clamp(1, MAX_TEXT_RESPONSE_BYTES);
            let complete = pane.terminal.grid().plain_lines().join("\n");
            let (text, truncated) = newest_utf8_bytes(&complete, maximum);
            Ok(Response::Text { text, truncated })
        }
        Request::ListUploads => Ok(list_retained_uploads(state)),
        Request::CancelUpload { upload_id } => cancel_retained_upload(state, &upload_id),
        Request::UploadFile { .. } => {
            unreachable!("streaming uploads are handled before request dispatch")
        }
        Request::KillDomain { domain } => {
            validate_id("domain id", &domain)?;
            let mut state = state.lock().unwrap();
            let mut domain = state.domains.remove(&domain).context("domain not found")?;
            domain.close_topology_subscribers("domain was terminated");
            domain.close_attachments("domain was terminated");
            Ok(Response::Ok {
                message: format!("terminated domain {domain}", domain = domain.id),
            })
        }
        Request::Shutdown => unreachable!("shutdown is acknowledged before generic dispatch"),
        Request::Attach { .. } | Request::SubscribeTopology { .. } => {
            unreachable!("streaming requests are handled before request dispatch")
        }
    }
}

fn create_domain(state: &SharedState, name: String, spawn: SpawnSpec) -> anyhow::Result<Response> {
    validate_name("domain name", &name)?;
    spawn.validate()?;
    let id = format!("domain-{}", crate::session::generate_session_id());
    let workspace_id = format!("live-{id}");
    let title = spawn.title.clone().unwrap_or_else(|| name.clone());
    let created_at = crate::session::format_unix_timestamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        None,
    );
    let reader = {
        let mut state_guard = state.lock().unwrap();
        if state_guard.domains.len() >= MAX_DOMAINS {
            anyhow::bail!("domain limit of {MAX_DOMAINS} reached");
        }
        if state_guard.pane_count() >= MAX_TOTAL_PANES {
            anyhow::bail!("server pane limit of {MAX_TOTAL_PANES} reached");
        }
        let (pane, reader) = PaneRuntime::spawn(
            0,
            &spawn,
            state_guard.allow_screen_checksum_reports,
            state_guard.xterm_keyboard_type,
        )?;
        let mut panes = HashMap::new();
        panes.insert(0, pane);
        state_guard.domains.insert(
            id.clone(),
            DomainRuntime {
                id: id.clone(),
                name,
                created_at,
                workspace_id,
                active_tab: 0,
                next_tab: 1,
                next_pane: 1,
                next_split: 0,
                topology_revision: 1,
                topology_subscribers: Vec::new(),
                tabs: vec![TabRuntime {
                    id: 0,
                    title,
                    user_title: spawn.title.clone(),
                    pinned: false,
                    focused_pane: 0,
                    layout: LivePaneLayout::Pane { pane: 0 },
                    panes,
                }],
            },
        );
        reader
    };
    if let Err(error) = spawn_reader_monitor(Arc::clone(state), id.clone(), 0, reader) {
        state.lock().unwrap().domains.remove(&id);
        return Err(error).context("start domain PTY reader");
    }
    let snapshot = state.lock().unwrap().domains[&id].snapshot();
    Ok(Response::Domain { domain: snapshot })
}

fn spawn_tab(state: &SharedState, domain_id: &str, spawn: SpawnSpec) -> anyhow::Result<Response> {
    validate_id("domain id", domain_id)?;
    spawn.validate()?;
    let (pane_id, reader) = {
        let mut state_guard = state.lock().unwrap();
        if state_guard.pane_count() >= MAX_TOTAL_PANES {
            anyhow::bail!("server pane limit of {MAX_TOTAL_PANES} reached");
        }
        let allow_screen_checksum_reports = state_guard.allow_screen_checksum_reports;
        let xterm_keyboard_type = state_guard.xterm_keyboard_type;
        let domain = state_guard
            .domains
            .get_mut(domain_id)
            .context("domain not found")?;
        if domain.tabs.len() >= MAX_TABS_PER_DOMAIN {
            anyhow::bail!("tab limit of {MAX_TABS_PER_DOMAIN} reached");
        }
        let pane_id = domain.next_pane;
        let tab_id = domain.next_tab;
        let (pane, reader) = PaneRuntime::spawn(
            pane_id,
            &spawn,
            allow_screen_checksum_reports,
            xterm_keyboard_type,
        )?;
        let mut panes = HashMap::new();
        panes.insert(pane_id, pane);
        domain.tabs.push(TabRuntime {
            id: tab_id,
            title: spawn
                .title
                .clone()
                .unwrap_or_else(|| format!("Tab {}", tab_id + 1)),
            user_title: spawn.title.clone(),
            pinned: false,
            focused_pane: pane_id,
            layout: LivePaneLayout::Pane { pane: pane_id },
            panes,
        });
        domain.active_tab = tab_id;
        domain.next_tab += 1;
        domain.next_pane += 1;
        (pane_id, reader)
    };
    if let Err(error) =
        spawn_reader_monitor(Arc::clone(state), domain_id.to_string(), pane_id, reader)
    {
        let mut state_guard = state.lock().unwrap();
        if let Some(domain) = state_guard.domains.get_mut(domain_id) {
            domain.tabs.retain(|tab| tab.id + 1 != domain.next_tab);
            domain.next_tab = domain.next_tab.saturating_sub(1);
            domain.next_pane = domain.next_pane.saturating_sub(1);
            domain.active_tab = domain.tabs.last().map_or(0, |tab| tab.id);
        }
        return Err(error).context("start domain tab PTY reader");
    }
    let snapshot = {
        let mut state = state.lock().unwrap();
        let domain = state
            .domains
            .get_mut(domain_id)
            .context("domain disappeared after spawning a tab")?;
        domain.publish_topology();
        domain.snapshot()
    };
    Ok(Response::Domain { domain: snapshot })
}

fn split_pane(
    state: &SharedState,
    domain_id: &str,
    pane_id: u32,
    direction: SplitDirection,
    mut spawn: SpawnSpec,
) -> anyhow::Result<Response> {
    validate_id("domain id", domain_id)?;
    let (new_id, reader, tab_id, old_layout, old_focus, old_active) = {
        let mut state_guard = state.lock().unwrap();
        if state_guard.pane_count() >= MAX_TOTAL_PANES {
            anyhow::bail!("server pane limit of {MAX_TOTAL_PANES} reached");
        }
        let allow_screen_checksum_reports = state_guard.allow_screen_checksum_reports;
        let xterm_keyboard_type = state_guard.xterm_keyboard_type;
        let domain = state_guard
            .domains
            .get_mut(domain_id)
            .context("domain not found")?;
        if domain.pane_count() >= MAX_PANES_PER_DOMAIN {
            anyhow::bail!("pane limit of {MAX_PANES_PER_DOMAIN} reached");
        }
        let pane = domain
            .tabs
            .iter()
            .find_map(|tab| tab.panes.get(&pane_id))
            .context("pane not found")?;
        if spawn.working_directory.is_none() {
            spawn.working_directory = crate::tabs::terminal_working_directory(&pane.terminal)
                .or_else(|| pane.pty.child_id().and_then(crate::tabs::get_process_cwd))
                .or_else(|| pane.requested_cwd.clone());
        }
        spawn.validate()?;
        let new_id = domain.next_pane;
        let split_id = domain.next_split;
        let (new_pane, reader) = PaneRuntime::spawn(
            new_id,
            &spawn,
            allow_screen_checksum_reports,
            xterm_keyboard_type,
        )?;
        let old_active = domain.active_tab;
        let tab = domain
            .tabs
            .iter_mut()
            .find(|tab| tab.panes.contains_key(&pane_id))
            .context("pane disappeared while splitting")?;
        let tab_id = tab.id;
        let old_layout = tab.layout.clone();
        let old_focus = tab.focused_pane;
        if !split_layout(&mut tab.layout, pane_id, new_id, split_id, direction) {
            anyhow::bail!("pane layout no longer contains pane {pane_id}");
        }
        tab.panes.insert(new_id, new_pane);
        tab.focused_pane = new_id;
        tab.refresh_title();
        domain.active_tab = tab.id;
        domain.next_pane += 1;
        domain.next_split = domain.next_split.wrapping_add(1);
        (new_id, reader, tab_id, old_layout, old_focus, old_active)
    };
    if let Err(error) =
        spawn_reader_monitor(Arc::clone(state), domain_id.to_string(), new_id, reader)
    {
        let mut state_guard = state.lock().unwrap();
        if let Some(domain) = state_guard.domains.get_mut(domain_id) {
            if let Some(tab) = domain.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.layout = old_layout;
                tab.focused_pane = old_focus;
                tab.panes.remove(&new_id);
            }
            domain.active_tab = old_active;
            domain.next_pane = domain.next_pane.saturating_sub(1);
        }
        return Err(error).context("start split domain PTY reader");
    }
    let snapshot = {
        let mut state = state.lock().unwrap();
        let domain = state
            .domains
            .get_mut(domain_id)
            .context("domain disappeared after splitting a pane")?;
        domain.publish_topology();
        domain.snapshot()
    };
    Ok(Response::Domain { domain: snapshot })
}

fn split_layout(
    layout: &mut LivePaneLayout,
    target: u32,
    new_pane: u32,
    split_id: u32,
    direction: SplitDirection,
) -> bool {
    match layout {
        LivePaneLayout::Pane { pane } if *pane == target => {
            *layout = LivePaneLayout::Split {
                split_id,
                direction,
                ratio_millis: 500,
                first: Box::new(LivePaneLayout::Pane { pane: target }),
                second: Box::new(LivePaneLayout::Pane { pane: new_pane }),
            };
            true
        }
        LivePaneLayout::Pane { .. } => false,
        LivePaneLayout::Split { first, second, .. } => {
            split_layout(first, target, new_pane, split_id, direction)
                || split_layout(second, target, new_pane, split_id, direction)
        }
    }
}

fn resize_domain_split(
    state: &SharedState,
    domain_id: &str,
    split_id: u32,
    ratio_millis: u16,
) -> anyhow::Result<Response> {
    validate_id("domain id", domain_id)?;
    if !(100..=900).contains(&ratio_millis) {
        anyhow::bail!("split ratio must be between 100 and 900 thousandths");
    }
    let mut state = state.lock().unwrap();
    let domain = state
        .domains
        .get_mut(domain_id)
        .context("domain not found")?;
    let changed = domain
        .tabs
        .iter_mut()
        .find_map(|tab| resize_layout_split(&mut tab.layout, split_id, ratio_millis))
        .context("split not found")?;
    if changed {
        domain.publish_topology();
    }
    Ok(Response::Domain {
        domain: domain.snapshot(),
    })
}

fn resize_layout_split(
    layout: &mut LivePaneLayout,
    target: u32,
    ratio_millis: u16,
) -> Option<bool> {
    match layout {
        LivePaneLayout::Pane { .. } => None,
        LivePaneLayout::Split {
            split_id,
            ratio_millis: ratio,
            first,
            second,
            ..
        } => {
            if *split_id == target {
                let changed = *ratio != ratio_millis;
                *ratio = ratio_millis;
                Some(changed)
            } else {
                resize_layout_split(first, target, ratio_millis)
                    .or_else(|| resize_layout_split(second, target, ratio_millis))
            }
        }
    }
}

fn rename_domain_tab(
    state: &SharedState,
    domain_id: &str,
    tab_id: u32,
    title: Option<String>,
) -> anyhow::Result<Response> {
    validate_id("domain id", domain_id)?;
    let title = title.filter(|title| !title.trim().is_empty());
    if let Some(title) = &title {
        validate_name("tab title", title)?;
    }
    let mut state = state.lock().unwrap();
    let domain = state
        .domains
        .get_mut(domain_id)
        .context("domain not found")?;
    let tab = domain
        .tabs
        .iter_mut()
        .find(|tab| tab.id == tab_id)
        .context("tab not found")?;
    let changed = tab.user_title != title;
    if changed {
        tab.user_title = title;
        tab.refresh_title();
        domain.publish_topology();
    }
    Ok(Response::Domain {
        domain: domain.snapshot(),
    })
}

fn set_domain_tab_pinned(
    state: &SharedState,
    domain_id: &str,
    tab_id: u32,
    pinned: bool,
) -> anyhow::Result<Response> {
    validate_id("domain id", domain_id)?;
    let mut state = state.lock().unwrap();
    let domain = state
        .domains
        .get_mut(domain_id)
        .context("domain not found")?;
    let index = domain
        .tabs
        .iter()
        .position(|tab| tab.id == tab_id)
        .context("tab not found")?;
    if domain.tabs[index].pinned != pinned {
        let mut tab = domain.tabs.remove(index);
        tab.pinned = pinned;
        let boundary = domain.tabs.iter().take_while(|tab| tab.pinned).count();
        domain.tabs.insert(boundary, tab);
        domain.publish_topology();
    }
    Ok(Response::Domain {
        domain: domain.snapshot(),
    })
}

fn move_domain_tab(
    state: &SharedState,
    domain_id: &str,
    tab_id: u32,
    target: usize,
) -> anyhow::Result<Response> {
    validate_id("domain id", domain_id)?;
    let mut state = state.lock().unwrap();
    let domain = state
        .domains
        .get_mut(domain_id)
        .context("domain not found")?;
    if target >= domain.tabs.len() {
        anyhow::bail!("tab target index {target} is out of range");
    }
    let source = domain
        .tabs
        .iter()
        .position(|tab| tab.id == tab_id)
        .context("tab not found")?;
    if domain.tabs[source].pinned != domain.tabs[target].pinned {
        anyhow::bail!("tabs cannot move across the pinned group boundary");
    }
    if source != target {
        let tab = domain.tabs.remove(source);
        domain.tabs.insert(target, tab);
        domain.publish_topology();
    }
    Ok(Response::Domain {
        domain: domain.snapshot(),
    })
}

fn swap_domain_pane_next(
    state: &SharedState,
    domain_id: &str,
    pane_id: u32,
) -> anyhow::Result<Response> {
    validate_id("domain id", domain_id)?;
    let mut state = state.lock().unwrap();
    let domain = state
        .domains
        .get_mut(domain_id)
        .context("domain not found")?;
    let tab = domain
        .tabs
        .iter_mut()
        .find(|tab| tab.panes.contains_key(&pane_id))
        .context("pane not found")?;
    let mut panes = Vec::new();
    collect_layout_panes(&tab.layout, &mut panes);
    if panes.len() <= 1 {
        anyhow::bail!("swap requires at least two panes");
    }
    let index = panes
        .iter()
        .position(|pane| *pane == pane_id)
        .context("pane layout does not contain the requested pane")?;
    let other = panes[(index + 1) % panes.len()];
    swap_layout_panes(&mut tab.layout, pane_id, other);
    domain.publish_topology();
    Ok(Response::Domain {
        domain: domain.snapshot(),
    })
}

fn balance_domain_tab(
    state: &SharedState,
    domain_id: &str,
    tab_id: u32,
) -> anyhow::Result<Response> {
    validate_id("domain id", domain_id)?;
    let mut state = state.lock().unwrap();
    let domain = state
        .domains
        .get_mut(domain_id)
        .context("domain not found")?;
    let tab = domain
        .tabs
        .iter_mut()
        .find(|tab| tab.id == tab_id)
        .context("tab not found")?;
    if tab.panes.len() <= 1 {
        anyhow::bail!("balance requires at least two panes");
    }
    if balance_layout(&mut tab.layout) {
        domain.publish_topology();
    }
    Ok(Response::Domain {
        domain: domain.snapshot(),
    })
}

fn apply_domain_pane_layout(
    state: &SharedState,
    domain_id: &str,
    tab_id: u32,
    preset: LivePaneLayoutPreset,
) -> anyhow::Result<Response> {
    validate_id("domain id", domain_id)?;
    let mut state = state.lock().unwrap();
    let domain = state
        .domains
        .get_mut(domain_id)
        .context("domain not found")?;
    let tab_index = domain
        .tabs
        .iter()
        .position(|tab| tab.id == tab_id)
        .context("tab not found")?;
    let mut panes = Vec::new();
    collect_layout_panes(&domain.tabs[tab_index].layout, &mut panes);
    if panes.len() <= 1 {
        anyhow::bail!("layout preset requires at least two panes");
    }
    let focused_pane = domain.tabs[tab_index].focused_pane;
    let mut next_split = domain.next_split;
    let layout = match preset {
        LivePaneLayoutPreset::Columns => {
            preset_layout(&panes, SplitDirection::Vertical, false, &mut next_split)?
        }
        LivePaneLayoutPreset::Rows => {
            preset_layout(&panes, SplitDirection::Horizontal, false, &mut next_split)?
        }
        LivePaneLayoutPreset::Grid => {
            preset_layout(&panes, SplitDirection::Vertical, true, &mut next_split)?
        }
        LivePaneLayoutPreset::Tall => master_detail_layout(
            &panes,
            focused_pane,
            SplitDirection::Vertical,
            &mut next_split,
        )?,
        LivePaneLayoutPreset::Fat => master_detail_layout(
            &panes,
            focused_pane,
            SplitDirection::Horizontal,
            &mut next_split,
        )?,
    };
    domain.tabs[tab_index].layout = layout;
    domain.next_split = next_split;
    domain.publish_topology();
    Ok(Response::Domain {
        domain: domain.snapshot(),
    })
}

fn move_domain_pane(
    state: &SharedState,
    domain_id: &str,
    pane_id: u32,
    destination_tab_id: u32,
    target_pane_id: Option<u32>,
    direction: SplitDirection,
) -> anyhow::Result<Response> {
    validate_id("domain id", domain_id)?;
    let mut state = state.lock().unwrap();
    let domain = state
        .domains
        .get_mut(domain_id)
        .context("domain not found")?;
    let source_index = domain
        .tabs
        .iter()
        .position(|tab| tab.panes.contains_key(&pane_id))
        .context("pane not found")?;
    let destination_index = domain
        .tabs
        .iter()
        .position(|tab| tab.id == destination_tab_id)
        .context("destination tab not found")?;
    if source_index == destination_index {
        anyhow::bail!("source and destination tabs must be different");
    }
    if domain.tabs[source_index].panes.len() <= 1 {
        anyhow::bail!("the last pane in a tab cannot be moved");
    }

    let source_layout = remove_layout_pane(domain.tabs[source_index].layout.clone(), pane_id)
        .context("source pane layout does not contain the requested pane")?;
    let source_focus = if domain.tabs[source_index].focused_pane == pane_id {
        sibling_layout_pane(&domain.tabs[source_index].layout, pane_id)
            .context("source pane has no surviving sibling")?
    } else {
        domain.tabs[source_index].focused_pane
    };
    let destination_focus = target_pane_id.unwrap_or(domain.tabs[destination_index].focused_pane);
    if !domain.tabs[destination_index]
        .panes
        .contains_key(&destination_focus)
    {
        anyhow::bail!("destination anchor pane not found in destination tab");
    }
    let split_id = domain.next_split;
    let next_split = split_id
        .checked_add(1)
        .context("domain split identity space exhausted")?;
    let mut destination_layout = domain.tabs[destination_index].layout.clone();
    if !split_layout(
        &mut destination_layout,
        destination_focus,
        pane_id,
        split_id,
        direction,
    ) {
        anyhow::bail!("destination layout does not contain its focused pane");
    }

    let pane = domain.tabs[source_index]
        .panes
        .remove(&pane_id)
        .context("pane disappeared while moving")?;
    domain.tabs[source_index].layout = source_layout;
    domain.tabs[source_index].focused_pane = source_focus;
    domain.tabs[source_index].refresh_title();
    domain.tabs[destination_index].panes.insert(pane_id, pane);
    domain.tabs[destination_index].layout = destination_layout;
    domain.tabs[destination_index].focused_pane = pane_id;
    domain.tabs[destination_index].refresh_title();
    domain.active_tab = destination_tab_id;
    domain.next_split = next_split;
    domain.publish_topology();
    Ok(Response::Domain {
        domain: domain.snapshot(),
    })
}

fn collect_layout_panes(layout: &LivePaneLayout, panes: &mut Vec<u32>) {
    match layout {
        LivePaneLayout::Pane { pane } => panes.push(*pane),
        LivePaneLayout::Split { first, second, .. } => {
            collect_layout_panes(first, panes);
            collect_layout_panes(second, panes);
        }
    }
}

fn swap_layout_panes(layout: &mut LivePaneLayout, first_id: u32, second_id: u32) {
    match layout {
        LivePaneLayout::Pane { pane } if *pane == first_id => *pane = second_id,
        LivePaneLayout::Pane { pane } if *pane == second_id => *pane = first_id,
        LivePaneLayout::Pane { .. } => {}
        LivePaneLayout::Split { first, second, .. } => {
            swap_layout_panes(first, first_id, second_id);
            swap_layout_panes(second, first_id, second_id);
        }
    }
}

fn balance_layout(layout: &mut LivePaneLayout) -> bool {
    match layout {
        LivePaneLayout::Pane { .. } => false,
        LivePaneLayout::Split {
            ratio_millis,
            first,
            second,
            ..
        } => {
            let changed = *ratio_millis != 500;
            *ratio_millis = 500;
            balance_layout(first) | balance_layout(second) | changed
        }
    }
}

fn preset_layout(
    panes: &[u32],
    direction: SplitDirection,
    alternate: bool,
    next_split: &mut u32,
) -> anyhow::Result<LivePaneLayout> {
    if panes.len() == 1 {
        return Ok(LivePaneLayout::Pane { pane: panes[0] });
    }
    let midpoint = panes.len().div_ceil(2);
    let split_id = *next_split;
    *next_split = next_split
        .checked_add(1)
        .context("domain split identity space exhausted")?;
    let next_direction = if alternate {
        match direction {
            SplitDirection::Horizontal => SplitDirection::Vertical,
            SplitDirection::Vertical => SplitDirection::Horizontal,
        }
    } else {
        direction
    };
    let ratio_millis = u16::try_from((midpoint * 1_000 + panes.len() / 2) / panes.len())
        .expect("bounded pane count produces a ratio in u16");
    Ok(LivePaneLayout::Split {
        split_id,
        direction,
        ratio_millis,
        first: Box::new(preset_layout(
            &panes[..midpoint],
            next_direction,
            alternate,
            next_split,
        )?),
        second: Box::new(preset_layout(
            &panes[midpoint..],
            next_direction,
            alternate,
            next_split,
        )?),
    })
}

fn master_detail_layout(
    panes: &[u32],
    master: u32,
    direction: SplitDirection,
    next_split: &mut u32,
) -> anyhow::Result<LivePaneLayout> {
    if panes.len() < 2 || !panes.contains(&master) {
        anyhow::bail!("master-detail layout requires its focused pane in the layout");
    }
    let details = panes
        .iter()
        .copied()
        .filter(|pane| *pane != master)
        .collect::<Vec<_>>();
    let detail_direction = match direction {
        SplitDirection::Horizontal => SplitDirection::Vertical,
        SplitDirection::Vertical => SplitDirection::Horizontal,
    };
    let split_id = *next_split;
    *next_split = next_split
        .checked_add(1)
        .context("domain split identity space exhausted")?;
    Ok(LivePaneLayout::Split {
        split_id,
        direction,
        ratio_millis: 500,
        first: Box::new(LivePaneLayout::Pane { pane: master }),
        second: Box::new(preset_layout(
            &details,
            detail_direction,
            false,
            next_split,
        )?),
    })
}

fn remove_domain_pane(
    state: &SharedState,
    domain_id: &str,
    pane_id: u32,
) -> anyhow::Result<Response> {
    validate_id("domain id", domain_id)?;
    let (mut removed, snapshot) = {
        let mut state = state.lock().unwrap();
        let domain = state
            .domains
            .get_mut(domain_id)
            .context("domain not found")?;
        let tab = domain
            .tabs
            .iter_mut()
            .find(|tab| tab.panes.contains_key(&pane_id))
            .context("pane not found")?;
        if tab.panes.len() == 1 {
            anyhow::bail!(
                "pane {pane_id} is the last pane in tab {}; use close-tab instead",
                tab.id
            );
        }
        let layout = remove_layout_pane(tab.layout.clone(), pane_id)
            .context("pane layout did not contain the requested pane")?;
        let removed = tab
            .panes
            .remove(&pane_id)
            .context("pane disappeared while closing")?;
        tab.layout = layout;
        if tab.focused_pane == pane_id {
            tab.focused_pane = first_layout_pane(&tab.layout);
            tab.refresh_title();
        }
        domain.publish_topology();
        let snapshot = domain.snapshot();
        (removed, snapshot)
    };
    removed.close_attachments("pane was closed by a domain control request");
    // Dropping the PTY handle terminates a still-running child. Keeping the
    // drop outside the state lock lets the reader monitor observe removal
    // without contending with process teardown.
    drop(removed);
    Ok(Response::Domain { domain: snapshot })
}

fn remove_domain_tab(
    state: &SharedState,
    domain_id: &str,
    tab_id: u32,
) -> anyhow::Result<Response> {
    validate_id("domain id", domain_id)?;
    let (mut removed, snapshot) = {
        let mut state = state.lock().unwrap();
        let domain = state
            .domains
            .get_mut(domain_id)
            .context("domain not found")?;
        if domain.tabs.len() == 1 {
            anyhow::bail!("tab {tab_id} is the last tab; use kill to terminate the domain");
        }
        let index = domain
            .tabs
            .iter()
            .position(|tab| tab.id == tab_id)
            .context("tab not found")?;
        let removed = domain.tabs.remove(index);
        if domain.active_tab == tab_id {
            domain.active_tab = domain.tabs[index.min(domain.tabs.len() - 1)].id;
        }
        domain.publish_topology();
        let snapshot = domain.snapshot();
        (removed, snapshot)
    };
    removed.close_attachments("tab was closed by a domain control request");
    drop(removed);
    Ok(Response::Domain { domain: snapshot })
}

fn remove_layout_pane(layout: LivePaneLayout, target: u32) -> Option<LivePaneLayout> {
    match layout {
        LivePaneLayout::Pane { pane } if pane == target => None,
        pane @ LivePaneLayout::Pane { .. } => Some(pane),
        LivePaneLayout::Split {
            split_id,
            direction,
            ratio_millis,
            first,
            second,
        } => match (
            remove_layout_pane(*first, target),
            remove_layout_pane(*second, target),
        ) {
            (Some(first), Some(second)) => Some(LivePaneLayout::Split {
                split_id,
                direction,
                ratio_millis,
                first: Box::new(first),
                second: Box::new(second),
            }),
            (Some(remaining), None) | (None, Some(remaining)) => Some(remaining),
            (None, None) => None,
        },
    }
}

fn first_layout_pane(layout: &LivePaneLayout) -> u32 {
    match layout {
        LivePaneLayout::Pane { pane } => *pane,
        LivePaneLayout::Split { first, .. } => first_layout_pane(first),
    }
}

fn sibling_layout_pane(layout: &LivePaneLayout, target: u32) -> Option<u32> {
    match layout {
        LivePaneLayout::Pane { .. } => None,
        LivePaneLayout::Split { first, second, .. } => {
            if matches!(first.as_ref(), LivePaneLayout::Pane { pane } if *pane == target) {
                return Some(first_layout_pane(second));
            }
            if matches!(second.as_ref(), LivePaneLayout::Pane { pane } if *pane == target) {
                return Some(first_layout_pane(first));
            }
            sibling_layout_pane(first, target).or_else(|| sibling_layout_pane(second, target))
        }
    }
}

fn spawn_reader_monitor(
    state: SharedState,
    domain: String,
    pane: u32,
    mut reader: Box<dyn Read + Send>,
) -> anyhow::Result<()> {
    thread::Builder::new()
        .name(format!("termeric-domain-{pane}"))
        .spawn(move || {
            let mut buffer = vec![0u8; protocol::MAX_STREAM_CHUNK_BYTES.min(16 * 1024)];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        close_pane(&state, &domain, pane, "terminal process exited");
                        break;
                    }
                    Ok(read) => publish_output(&state, &domain, pane, &buffer[..read]),
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        close_pane(&state, &domain, pane, &format!("PTY read failed: {error}"));
                        break;
                    }
                }
            }
        })?;
    Ok(())
}

fn publish_output(state: &SharedState, domain_id: &str, pane_id: u32, data: &[u8]) {
    let mut state = state.lock().unwrap();
    let Ok(pane) = find_pane_mut(&mut state, domain_id, pane_id) else {
        return;
    };
    pane.terminal.process(data);
    let events = pane.terminal.drain_events();
    let mut title_change = None;
    for event in events {
        match event {
            TerminalEvent::PtyWrite(reply) => {
                if let Err(error) = queue_pane_input(&pane.input, reply) {
                    log::warn!("Could not write domain protocol reply: {error}");
                    pane.closed = true;
                }
            }
            TerminalEvent::TitleChanged(title) => {
                let title = sanitize_runtime_title(&title);
                pane.title.clone_from(&title);
                title_change = Some(title);
            }
            TerminalEvent::KittyFileTransfer(command) if pane.attachments.is_empty() => {
                let reply = termeric_core::KittyFileTransferStatus {
                    id: command.id,
                    file_id: command.file_id,
                    code: termeric_core::KittyFileTransferStatusCode::PermissionDenied,
                    message: Some(
                        "file transfer requires an attached consenting Termeric window".into(),
                    ),
                    name: None,
                    size: None,
                    transmission_type: termeric_core::KittyFileTransmissionType::Simple,
                }
                .canonical_reply();
                let reply = termeric_core::Terminal::frame_pty_reply_for_mode(
                    &reply,
                    command.reply_uses_8bit_controls,
                );
                if let Err(error) = queue_pane_input(&pane.input, reply) {
                    log::warn!("Could not deny detached-domain file transfer: {error}");
                    pane.closed = true;
                }
            }
            _ => {}
        }
    }
    pane.sequence = pane.sequence.wrapping_add(1);
    let frame = ServerFrame::Output {
        sequence: pane.sequence,
        data: data.to_vec(),
    };
    pane.attachments
        .retain(|attachment| attachment.send_or_disconnect(frame.clone()));
    if let Some(title) = title_change
        && let Some(domain) = state.domains.get_mut(domain_id)
    {
        let changed = domain
            .tabs
            .iter_mut()
            .find(|tab| tab.panes.contains_key(&pane_id))
            .filter(|tab| {
                tab.focused_pane == pane_id && tab.user_title.is_none() && tab.title != title
            })
            .is_some_and(|tab| {
                tab.title = title;
                true
            });
        if changed {
            domain.publish_topology();
        }
    }
}

fn sanitize_runtime_title(title: &str) -> String {
    let title: String = title
        .chars()
        .filter(|character| !crate::safe_text::is_unsafe_display_char(*character))
        .take(128)
        .collect();
    if title.is_empty() {
        "terminal".into()
    } else {
        title
    }
}

fn close_pane(state: &SharedState, domain_id: &str, pane_id: u32, message: &str) {
    let mut state = state.lock().unwrap();
    let Some(domain) = state.domains.get_mut(domain_id) else {
        return;
    };
    let Some(tab_index) = domain
        .tabs
        .iter()
        .position(|tab| tab.panes.contains_key(&pane_id))
    else {
        return;
    };
    {
        let pane = domain.tabs[tab_index].panes.get_mut(&pane_id).unwrap();
        // Reader EOF normally follows child exit; try_wait through is_running so
        // the completed child is reaped instead of retained as a zombie record.
        let _ = pane.pty.is_running();
        pane.closed = true;
        pane.close_attachments(message);
    }

    if domain.tabs[tab_index].panes.len() > 1 {
        let tab = &mut domain.tabs[tab_index];
        if let Some(layout) = remove_layout_pane(tab.layout.clone(), pane_id) {
            tab.layout = layout;
            tab.panes.remove(&pane_id);
            if tab.focused_pane == pane_id {
                tab.focused_pane = first_layout_pane(&tab.layout);
                tab.refresh_title();
            }
        }
    } else if domain.tabs.len() > 1 {
        let removed_tab = domain.tabs.remove(tab_index);
        if domain.active_tab == removed_tab.id {
            domain.active_tab = domain.tabs[tab_index.min(domain.tabs.len() - 1)].id;
        }
    }
    // The sole pane of the sole tab remains inspectable as a closed terminal;
    // every other exited leaf is removed from the authoritative topology.
    domain.publish_topology();
}

fn serve_attachment(
    mut stream: UnixStream,
    state: SharedState,
    domain_id: String,
    pane_id: u32,
) -> anyhow::Result<()> {
    validate_id("domain id", &domain_id)?;
    let transport = stream.try_clone()?;
    let (attachment_id, receiver, snapshot, graphics_snapshot, sequence) = {
        let mut state_guard = state.lock().unwrap();
        let attachment_id = state_guard.next_attachment.fetch_add(1, Ordering::Relaxed);
        let pane = find_pane_mut(&mut state_guard, &domain_id, pane_id)?;
        if pane.attachments.len() >= MAX_ATTACHMENTS_PER_PANE {
            anyhow::bail!("pane attachment limit of {MAX_ATTACHMENTS_PER_PANE} reached");
        }
        let (sender, receiver) = std::sync::mpsc::sync_channel(ATTACH_QUEUE_CAPACITY);
        let snapshot = terminal_snapshot(&pane.terminal, ATTACH_SNAPSHOT_BYTES);
        let graphics_snapshot = pane.terminal.graphics_relay_snapshot(Instant::now());
        let sequence = pane.sequence;
        if pane.closed {
            let _ = sender.try_send(ServerFrame::Closed {
                exit_message: "terminal process already exited".into(),
            });
        } else {
            pane.attachments.push(Attachment {
                id: attachment_id,
                sender,
                transport,
            });
        }
        (
            attachment_id,
            receiver,
            snapshot,
            graphics_snapshot,
            sequence,
        )
    };
    let initial_write = (|| -> anyhow::Result<()> {
        write_server_frame(
            &mut stream,
            &ServerFrame::Response {
                response: Response::Attached { snapshot, sequence },
            },
        )?;
        let mut snapshot_write_error = None;
        graphics_snapshot
            .for_each_escape_chunk(|data| {
                if snapshot_write_error.is_some() {
                    return false;
                }
                if let Err(error) =
                    write_server_frame(&mut stream, &ServerFrame::SnapshotOutput { data })
                {
                    snapshot_write_error = Some(error);
                    false
                } else {
                    true
                }
            })
            .map_err(anyhow::Error::msg)?;
        if let Some(error) = snapshot_write_error {
            return Err(error);
        }
        write_server_frame(&mut stream, &ServerFrame::SnapshotComplete)
    })();
    if let Err(error) = initial_write {
        remove_attachment(&state, &domain_id, pane_id, attachment_id);
        return Err(error);
    }

    let reader_stream = match stream.try_clone() {
        Ok(stream) => stream,
        Err(error) => {
            remove_attachment(&state, &domain_id, pane_id, attachment_id);
            return Err(error.into());
        }
    };
    let reader_state = Arc::clone(&state);
    let reader_domain = domain_id.clone();
    let reader = match thread::Builder::new()
        .name("termeric-domain-attach-input".into())
        .spawn(move || {
            attachment_input_loop(
                reader_stream,
                &reader_state,
                &reader_domain,
                pane_id,
                attachment_id,
            );
            remove_attachment(&reader_state, &reader_domain, pane_id, attachment_id);
        }) {
        Ok(reader) => reader,
        Err(error) => {
            remove_attachment(&state, &domain_id, pane_id, attachment_id);
            return Err(error.into());
        }
    };

    for frame in receiver {
        let closed = matches!(frame, ServerFrame::Closed { .. });
        if write_server_frame(&mut stream, &frame).is_err() || closed {
            break;
        }
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
    let _ = reader.join();
    remove_attachment(&state, &domain_id, pane_id, attachment_id);
    Ok(())
}

fn serve_topology_subscription(
    mut stream: UnixStream,
    state: SharedState,
    domain_id: String,
) -> anyhow::Result<()> {
    validate_id("domain id", &domain_id)?;
    let transport = stream.try_clone()?;
    let (subscription_id, receiver, revision, snapshot) = {
        let mut state_guard = state.lock().unwrap();
        let subscription_id = state_guard
            .next_topology_subscription
            .fetch_add(1, Ordering::Relaxed);
        let domain = state_guard
            .domains
            .get_mut(&domain_id)
            .context("domain not found")?;
        if domain.topology_subscribers.len() >= MAX_TOPOLOGY_SUBSCRIBERS_PER_DOMAIN {
            anyhow::bail!(
                "topology subscriber limit of {MAX_TOPOLOGY_SUBSCRIBERS_PER_DOMAIN} reached"
            );
        }
        let (sender, receiver) = std::sync::mpsc::sync_channel(TOPOLOGY_QUEUE_CAPACITY);
        let revision = domain.topology_revision;
        let snapshot = domain.topology_snapshot();
        domain.topology_subscribers.push(TopologySubscriber {
            id: subscription_id,
            sender,
            transport,
        });
        (subscription_id, receiver, revision, snapshot)
    };
    // Cover snapshot write, descriptor clone, and reader-spawn failures as
    // well as normal completion. Never retain a dead slot until a later edit.
    let _registration = TopologySubscriptionGuard {
        state: Arc::clone(&state),
        domain_id: domain_id.clone(),
        subscription_id,
    };
    write_server_frame(
        &mut stream,
        &ServerFrame::Response {
            response: Response::TopologySubscribed {
                revision,
                domain: snapshot,
            },
        },
    )?;

    let reader_stream = stream.try_clone()?;
    let reader_state = Arc::clone(&state);
    let reader_domain = domain_id.clone();
    let reader = thread::Builder::new()
        .name("termeric-domain-topology-input".into())
        .spawn(move || {
            topology_subscription_input_loop(reader_stream);
            remove_topology_subscription(&reader_state, &reader_domain, subscription_id);
        })?;

    for frame in receiver {
        let closed = matches!(frame, ServerFrame::DomainClosed { .. });
        if write_server_frame(&mut stream, &frame).is_err() || closed {
            break;
        }
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
    let _ = reader.join();
    Ok(())
}

fn topology_subscription_input_loop(mut stream: UnixStream) {
    while let Ok(frame) = protocol::read_frame::<ClientFrame>(&mut stream) {
        if matches!(frame, ClientFrame::Detach) {
            return;
        }
    }
}

fn remove_topology_subscription(state: &SharedState, domain_id: &str, subscription_id: u64) {
    let mut state = state.lock().unwrap();
    if let Some(domain) = state.domains.get_mut(domain_id) {
        domain.topology_subscribers.retain(|subscriber| {
            if subscriber.id == subscription_id {
                subscriber.disconnect();
                false
            } else {
                true
            }
        });
    }
}

fn attachment_input_loop(
    mut stream: UnixStream,
    state: &SharedState,
    domain_id: &str,
    pane_id: u32,
    attachment_id: u64,
) {
    while let Ok(frame) = protocol::read_frame::<ClientFrame>(&mut stream) {
        let mut state = state.lock().unwrap();
        let Ok(pane) = find_pane_mut(&mut state, domain_id, pane_id) else {
            return;
        };
        // Socket shutdown cannot retract a frame already read by this thread.
        // Validate authority under the same lock used by eviction and mutation.
        if pane.closed || !pane.attachments.iter().any(|item| item.id == attachment_id) {
            return;
        }
        match frame {
            ClientFrame::Input { data } if data.len() <= protocol::MAX_STREAM_CHUNK_BYTES => {
                if queue_pane_input(&pane.input, data).is_err() {
                    return;
                }
            }
            ClientFrame::Resize {
                cols,
                rows,
                pixel_width,
                pixel_height,
            } if cols > 0 && rows > 0 && cols <= 1_000 && rows <= 1_000 => {
                if pane
                    .resize(PtySize {
                        rows,
                        cols,
                        pixel_width,
                        pixel_height,
                    })
                    .is_err()
                {
                    return;
                }
            }
            ClientFrame::Detach => return,
            _ => return,
        }
    }
}

fn remove_attachment(state: &SharedState, domain_id: &str, pane_id: u32, attachment_id: u64) {
    let mut state = state.lock().unwrap();
    if let Ok(pane) = find_pane_mut(&mut state, domain_id, pane_id) {
        pane.attachments.retain(|attachment| {
            if attachment.id == attachment_id {
                attachment.disconnect();
                false
            } else {
                true
            }
        });
    }
}

fn terminal_snapshot(terminal: &Terminal, max_bytes: usize) -> Vec<u8> {
    let multiple_cursors = terminal
        .extra_cursors()
        .replay(max_bytes.saturating_sub(512));
    let user_defined_keys = user_defined_keys_snapshot(terminal);
    let user_defined_keys = if user_defined_keys.len() <= max_bytes.saturating_sub(512) {
        user_defined_keys
    } else {
        Vec::new()
    };
    let lines = terminal.grid().styled_lines();
    let mut selected = Vec::new();
    let mut bytes = 0usize;
    let content_budget = max_bytes
        .saturating_sub(512)
        .saturating_sub(user_defined_keys.len())
        .saturating_sub(multiple_cursors.len());
    for line in lines.into_iter().rev() {
        let needed = line.len().saturating_add(2);
        if bytes.saturating_add(needed) > content_budget {
            break;
        }
        bytes += needed;
        selected.push(line);
    }
    selected.reverse();
    let mut snapshot = b"\x1bc\x1b[2J\x1b[H".to_vec();
    let mode = terminal.mode();
    if mode.contains(TerminalMode::ALT_SCREEN) {
        snapshot.extend_from_slice(b"\x1b[?1049h");
    }
    snapshot.extend_from_slice(selected.join("\r\n").as_bytes());
    push_private_mode(
        &mut snapshot,
        1,
        mode.contains(TerminalMode::CURSOR_KEYS_APPLICATION),
    );
    push_private_mode(&mut snapshot, 5, mode.contains(TerminalMode::REVERSE_VIDEO));
    push_private_mode(&mut snapshot, 6, mode.contains(TerminalMode::ORIGIN));
    push_private_mode(&mut snapshot, 7, mode.contains(TerminalMode::AUTOWRAP));
    if terminal.cursor_style().is_some() {
        snapshot
            .extend_from_slice(format!("\x1b[{} q", terminal.cursor_style_parameter()).as_bytes());
    }
    if let Some(blinking) = terminal.cursor_blink_override() {
        push_private_mode(&mut snapshot, 12, blinking);
    }
    push_private_mode(
        &mut snapshot,
        25,
        mode.contains(TerminalMode::CURSOR_VISIBLE),
    );
    push_private_mode(&mut snapshot, 9, mode.contains(TerminalMode::MOUSE_X10));
    push_private_mode(
        &mut snapshot,
        1000,
        mode.contains(TerminalMode::MOUSE_TRACKING),
    );
    push_private_mode(
        &mut snapshot,
        1002,
        mode.contains(TerminalMode::MOUSE_BUTTON_EVENT),
    );
    push_private_mode(
        &mut snapshot,
        1003,
        mode.contains(TerminalMode::MOUSE_ANY_EVENT),
    );
    push_private_mode(&mut snapshot, 1005, mode.contains(TerminalMode::MOUSE_UTF8));
    push_private_mode(&mut snapshot, 1006, mode.contains(TerminalMode::MOUSE_SGR));
    push_private_mode(
        &mut snapshot,
        1004,
        mode.contains(TerminalMode::FOCUS_EVENTS),
    );
    push_private_mode(
        &mut snapshot,
        2004,
        mode.contains(TerminalMode::BRACKETED_PASTE),
    );
    push_private_mode(
        &mut snapshot,
        1007,
        mode.contains(TerminalMode::ALTERNATE_SCROLL),
    );
    snapshot.extend_from_slice(if mode.contains(TerminalMode::INSERT) {
        b"\x1b[4h"
    } else {
        b"\x1b[4l"
    });
    snapshot.extend_from_slice(if mode.contains(TerminalMode::LINEFEED_NEWLINE) {
        b"\x1b[20h"
    } else {
        b"\x1b[20l"
    });
    snapshot.extend_from_slice(if mode.contains(TerminalMode::KEYPAD_APPLICATION) {
        b"\x1b="
    } else {
        b"\x1b>"
    });
    snapshot
        .extend_from_slice(format!("\x1b[={}u", terminal.kitty_keyboard_flags().bits()).as_bytes());
    match terminal.xterm_keyboard_type() {
        XtermKeyboardType::Normal => push_private_mode(&mut snapshot, 1060, false),
        XtermKeyboardType::Legacy => push_private_mode(&mut snapshot, 1060, true),
        XtermKeyboardType::Vt220 => push_private_mode(&mut snapshot, 1061, true),
    }
    snapshot.extend_from_slice(&user_defined_keys);
    let keyboard_resource = terminal.xterm_keyboard_resource();
    snapshot.extend_from_slice(format!("\x1b[>0;{}m", keyboard_resource.modify()).as_bytes());
    snapshot.extend_from_slice(format!("\x1b[>0;{}f", keyboard_resource.format()).as_bytes());
    let cursor_keys = terminal.xterm_cursor_keys();
    snapshot.extend_from_slice(format!("\x1b[>1;{}m", cursor_keys.mode().parameter()).as_bytes());
    snapshot.extend_from_slice(format!("\x1b[>1;{}f", cursor_keys.format().parameter()).as_bytes());
    let function_keys = terminal.xterm_function_keys();
    snapshot.extend_from_slice(format!("\x1b[>2;{}m", function_keys.mode().parameter()).as_bytes());
    snapshot
        .extend_from_slice(format!("\x1b[>2;{}f", function_keys.format().parameter()).as_bytes());
    let keypad_keys = terminal.xterm_keypad_keys();
    snapshot.extend_from_slice(format!("\x1b[>3;{}m", keypad_keys.mode().parameter()).as_bytes());
    snapshot.extend_from_slice(format!("\x1b[>3;{}f", keypad_keys.format().parameter()).as_bytes());
    let string_keys = terminal.xterm_string_keys();
    snapshot.extend_from_slice(format!("\x1b[>5;{}m", string_keys.mode().parameter()).as_bytes());
    snapshot.extend_from_slice(format!("\x1b[>5;{}f", string_keys.format().parameter()).as_bytes());
    let modifier_keys = terminal.xterm_modifier_keys();
    snapshot.extend_from_slice(format!("\x1b[>6;{}m", modifier_keys.mode().parameter()).as_bytes());
    snapshot
        .extend_from_slice(format!("\x1b[>6;{}f", modifier_keys.format().parameter()).as_bytes());
    let special_keys = terminal.xterm_special_keys();
    snapshot.extend_from_slice(format!("\x1b[>7;{}m", special_keys.mode().parameter()).as_bytes());
    snapshot
        .extend_from_slice(format!("\x1b[>7;{}f", special_keys.format().parameter()).as_bytes());
    let modify_other = terminal.xterm_modify_other_keys();
    snapshot.extend_from_slice(
        format!(
            "\x1b[>4;{}:{}m",
            modify_other.level(),
            modify_other.modifier_mask()
        )
        .as_bytes(),
    );
    snapshot
        .extend_from_slice(format!("\x1b[>4;{}f", modify_other.format().parameter()).as_bytes());
    snapshot.extend_from_slice(
        format!("\x1b[{}\"q", terminal.character_protection_parameter()).as_bytes(),
    );
    // Last, because replaying DECOM above homes the cursor.
    let cursor = terminal.grid().cursor();
    snapshot.extend_from_slice(format!("\x1b[{};{}H", cursor.row + 1, cursor.col + 1).as_bytes());
    snapshot.extend_from_slice(&multiple_cursors);
    debug_assert!(snapshot.len() <= max_bytes || max_bytes < 512);
    snapshot
}

fn user_defined_keys_snapshot(terminal: &Terminal) -> Vec<u8> {
    let mut definitions = terminal.xterm_user_defined_keys().peekable();
    if definitions.peek().is_none() {
        return Vec::new();
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = b"\x1bP0;1|".to_vec();
    for (index, (code, definition)) in definitions.enumerate() {
        if index != 0 {
            output.push(b';');
        }
        output.extend_from_slice(code.to_string().as_bytes());
        output.push(b'/');
        for byte in definition {
            output.push(HEX[(byte >> 4) as usize]);
            output.push(HEX[(byte & 0x0f) as usize]);
        }
    }
    output.extend_from_slice(b"\x1b\\");
    output
}

fn push_private_mode(snapshot: &mut Vec<u8>, mode: u16, enabled: bool) {
    snapshot
        .extend_from_slice(format!("\x1b[?{mode}{}", if enabled { 'h' } else { 'l' }).as_bytes());
}

fn newest_utf8_bytes(text: &str, maximum: usize) -> (String, bool) {
    if text.len() <= maximum {
        return (text.to_string(), false);
    }
    let mut start = text.len() - maximum;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    (text[start..].to_string(), true)
}

fn find_pane<'a>(
    state: &'a ServerState,
    domain_id: &str,
    pane_id: u32,
) -> anyhow::Result<&'a PaneRuntime> {
    validate_id("domain id", domain_id)?;
    state
        .domains
        .get(domain_id)
        .context("domain not found")?
        .tabs
        .iter()
        .find_map(|tab| tab.panes.get(&pane_id))
        .context("pane not found")
}

fn find_pane_mut<'a>(
    state: &'a mut ServerState,
    domain_id: &str,
    pane_id: u32,
) -> anyhow::Result<&'a mut PaneRuntime> {
    validate_id("domain id", domain_id)?;
    state
        .domains
        .get_mut(domain_id)
        .context("domain not found")?
        .tabs
        .iter_mut()
        .find_map(|tab| tab.panes.get_mut(&pane_id))
        .context("pane not found")
}

fn spawn_pane_input_worker(
    pane_id: u32,
    mut writer: Box<dyn Write + Send>,
) -> anyhow::Result<SyncSender<Vec<u8>>> {
    let (sender, receiver) = std::sync::mpsc::sync_channel::<Vec<u8>>(PANE_INPUT_QUEUE_CAPACITY);
    thread::Builder::new()
        .name(format!("termeric-domain-input-{pane_id}"))
        .spawn(move || {
            for data in receiver {
                if let Err(error) = writer.write_all(&data) {
                    log::warn!("Domain pane {pane_id} input writer stopped: {error}");
                    break;
                }
            }
        })
        .context("start domain PTY input writer")?;
    Ok(sender)
}

fn queue_pane_input(sender: &SyncSender<Vec<u8>>, data: Vec<u8>) -> anyhow::Result<()> {
    match sender.try_send(data) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => {
            anyhow::bail!("domain pane input backpressure limit reached")
        }
        Err(TrySendError::Disconnected(_)) => {
            anyhow::bail!("domain pane input writer is unavailable")
        }
    }
}

impl PaneRuntime {
    fn spawn(
        id: u32,
        spec: &SpawnSpec,
        allow_screen_checksum_reports: bool,
        xterm_keyboard_type: XtermKeyboardType,
    ) -> anyhow::Result<(Self, Box<dyn Read + Send>)> {
        let mut environment = Vec::new();
        if let Some(cwd) = &spec.working_directory {
            environment.push(("PWD".to_string(), cwd.clone()));
        }
        let initial_size = default_pty_size(spec.cols, spec.rows);
        let pty = Pty::new_with_env_and_size(
            initial_size,
            spec.program.as_deref(),
            &spec.args,
            &environment,
        )?;
        let term_name = pty.term_name();
        let (mut handle, reader) = pty.take_reader();
        let writer = handle
            .take_writer()
            .context("domain PTY writer was unavailable")?;
        let input = spawn_pane_input_worker(id, writer)?;
        let title = spec.title.clone().unwrap_or_else(|| {
            spec.program
                .as_deref()
                .and_then(|program| std::path::Path::new(program).file_name())
                .and_then(|name| name.to_str())
                .unwrap_or("shell")
                .to_string()
        });
        let mut command = Vec::new();
        command.push(
            spec.program
                .clone()
                .unwrap_or_else(|| "<default-shell>".into()),
        );
        command.extend(spec.args.clone());
        let mut terminal = Terminal::new(spec.cols as usize, spec.rows as usize);
        terminal.set_pixel_geometry(
            initial_size.pixel_width.into(),
            initial_size.pixel_height.into(),
        );
        terminal.set_term_name(term_name);
        terminal.set_screen_checksum_reporting(allow_screen_checksum_reports);
        terminal.configure_xterm_keyboard_type(xterm_keyboard_type, true);
        terminal.set_scrollback_limits(100_000, MAX_PANE_SCROLLBACK_BYTES);
        Ok((
            Self {
                id,
                title,
                command,
                requested_cwd: spec.working_directory.clone(),
                terminal,
                pty: handle,
                input,
                cols: spec.cols,
                rows: spec.rows,
                pixel_width: initial_size.pixel_width,
                pixel_height: initial_size.pixel_height,
                closed: false,
                sequence: 0,
                attachments: Vec::new(),
            },
            reader,
        ))
    }

    fn snapshot(&self) -> LivePane {
        LivePane {
            id: self.id,
            title: self.title.clone(),
            working_directory: crate::tabs::terminal_working_directory(&self.terminal)
                .or_else(|| self.pty.child_id().and_then(crate::tabs::get_process_cwd))
                .or_else(|| self.requested_cwd.clone()),
            command: self.command.clone(),
            cols: self.cols,
            rows: self.rows,
            closed: self.closed,
            attachments: self.attachments.len(),
        }
    }

    /// Apply renderer-backed relay geometry. Some external terminal parents
    /// report zero pixel fields; in that case preserve the last known physical
    /// cell extent rather than erasing valid kernel geometry.
    fn resize(&mut self, reported: PtySize) -> anyhow::Result<()> {
        let size = PtySize {
            cols: reported.cols,
            rows: reported.rows,
            pixel_width: normalize_relay_pixel_extent(
                reported.pixel_width,
                self.pixel_width,
                self.cols,
                reported.cols,
                termeric_core::pty::DEFAULT_CELL_PIXEL_WIDTH,
            ),
            pixel_height: normalize_relay_pixel_extent(
                reported.pixel_height,
                self.pixel_height,
                self.rows,
                reported.rows,
                termeric_core::pty::DEFAULT_CELL_PIXEL_HEIGHT,
            ),
        };
        if size
            == (PtySize {
                cols: self.cols,
                rows: self.rows,
                pixel_width: self.pixel_width,
                pixel_height: self.pixel_height,
            })
        {
            return Ok(());
        }

        self.pty.resize(size)?;
        self.terminal.resize_with_pixel_geometry(
            size.cols as usize,
            size.rows as usize,
            size.pixel_width.into(),
            size.pixel_height.into(),
        );
        self.cols = size.cols;
        self.rows = size.rows;
        self.pixel_width = size.pixel_width;
        self.pixel_height = size.pixel_height;
        Ok(())
    }

    fn close_attachments(&mut self, message: &str) {
        let frame = ServerFrame::Closed {
            exit_message: message.to_string(),
        };
        for attachment in self.attachments.drain(..) {
            attachment.send_or_disconnect(frame.clone());
        }
    }
}

fn normalize_relay_pixel_extent(
    reported: u16,
    previous_pixels: u16,
    previous_cells: u16,
    cells: u16,
    fallback_cell_pixels: u16,
) -> u16 {
    if reported > 0 {
        return reported;
    }
    if previous_pixels > 0 && previous_cells > 0 {
        let scaled = (u32::from(previous_pixels) * u32::from(cells)
            + u32::from(previous_cells) / 2)
            / u32::from(previous_cells);
        return scaled.clamp(1, u32::from(u16::MAX)) as u16;
    }
    cells.saturating_mul(fallback_cell_pixels).max(1)
}

impl ServerState {
    fn pane_count(&self) -> usize {
        self.domains.values().map(DomainRuntime::pane_count).sum()
    }
}

impl TabRuntime {
    fn refresh_title(&mut self) -> bool {
        let title = self.user_title.clone().unwrap_or_else(|| {
            self.panes
                .get(&self.focused_pane)
                .map(|pane| pane.title.clone())
                .unwrap_or_else(|| format!("Tab {}", self.id + 1))
        });
        if self.title == title {
            false
        } else {
            self.title = title;
            true
        }
    }

    fn snapshot(&self) -> LiveTab {
        let mut panes: Vec<_> = self.panes.values().map(PaneRuntime::snapshot).collect();
        panes.sort_by_key(|pane| pane.id);
        LiveTab {
            id: self.id,
            title: self.title.clone(),
            user_title: self.user_title.clone(),
            pinned: self.pinned,
            focused_pane: self.focused_pane,
            layout: self.layout.clone(),
            panes,
        }
    }

    fn topology_snapshot(&self) -> LiveTopologyTab {
        let mut panes: Vec<_> = self
            .panes
            .values()
            .map(|pane| LiveTopologyPane {
                id: pane.id,
                title: pane.title.clone(),
                closed: pane.closed,
            })
            .collect();
        panes.sort_by_key(|pane| pane.id);
        LiveTopologyTab {
            id: self.id,
            title: self.title.clone(),
            user_title: self.user_title.clone(),
            pinned: self.pinned,
            focused_pane: self.focused_pane,
            layout: self.layout.clone(),
            panes,
        }
    }

    fn close_attachments(&mut self, message: &str) {
        for pane in self.panes.values_mut() {
            pane.close_attachments(message);
        }
    }
}

impl DomainRuntime {
    fn pane_count(&self) -> usize {
        self.tabs.iter().map(|tab| tab.panes.len()).sum()
    }

    fn attachment_count(&self) -> usize {
        self.tabs
            .iter()
            .flat_map(|tab| tab.panes.values())
            .map(|pane| pane.attachments.len())
            .sum()
    }

    fn pane_snapshots(&self) -> Vec<LivePane> {
        let mut panes: Vec<_> = self
            .tabs
            .iter()
            .flat_map(|tab| tab.panes.values())
            .map(PaneRuntime::snapshot)
            .collect();
        panes.sort_by_key(|pane| pane.id);
        panes
    }

    fn workspace_snapshot(&self) -> LiveWorkspace {
        LiveWorkspace {
            id: self.workspace_id.clone(),
            name: self.name.clone(),
            active_tab: self.active_tab,
            tabs: self.tabs.iter().map(TabRuntime::snapshot).collect(),
        }
    }

    fn snapshot(&self) -> DomainSnapshot {
        DomainSnapshot {
            id: self.id.clone(),
            name: self.name.clone(),
            created_at: self.created_at.clone(),
            workspace: self.workspace_snapshot(),
        }
    }

    fn topology_snapshot(&self) -> DomainTopology {
        DomainTopology {
            domain_id: self.id.clone(),
            active_tab: self.active_tab,
            tabs: self
                .tabs
                .iter()
                .map(TabRuntime::topology_snapshot)
                .collect(),
        }
    }

    fn publish_topology(&mut self) {
        self.topology_revision = self.topology_revision.wrapping_add(1).max(1);
        let frame = ServerFrame::TopologyChanged {
            revision: self.topology_revision,
            domain: self.topology_snapshot(),
        };
        self.topology_subscribers
            .retain(|subscriber| subscriber.send_or_disconnect(frame.clone()));
    }

    fn close_topology_subscribers(&mut self, message: &str) {
        let frame = ServerFrame::DomainClosed {
            domain: self.id.clone(),
            message: message.to_string(),
        };
        for subscriber in self.topology_subscribers.drain(..) {
            subscriber.send_or_disconnect(frame.clone());
        }
    }

    fn summary(&self) -> DomainSummary {
        DomainSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            workspace_id: self.workspace_id.clone(),
            tabs: self.tabs.len(),
            panes: self.pane_count(),
            attachments: self.attachment_count(),
            created_at: self.created_at.clone(),
        }
    }

    fn close_attachments(&mut self, message: &str) {
        for tab in &mut self.tabs {
            tab.close_attachments(message);
        }
    }
}

#[cfg(test)]
#[path = "server_test_support.rs"]
mod test_support;

#[cfg(test)]
#[path = "upload_deadline_tests.rs"]
mod upload_deadline_tests;

#[cfg(test)]
#[path = "topology_lifecycle_tests.rs"]
mod topology_lifecycle_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if condition() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        condition()
    }

    fn read_graphics_attachment_snapshot(stream: &mut UnixStream) -> Vec<u8> {
        let mut snapshot = Vec::new();
        loop {
            match protocol::read_frame::<ServerFrame>(stream).unwrap() {
                ServerFrame::SnapshotOutput { data } => {
                    snapshot.extend_from_slice(data.as_bytes());
                }
                ServerFrame::SnapshotComplete => return snapshot,
                frame => panic!("unexpected attachment snapshot frame {frame:?}"),
            }
        }
    }

    #[test]
    fn temporary_upload_publication_is_private_atomic_and_never_overwrites() {
        let directory = tempfile::tempdir().unwrap();
        let temporary_path = directory.path().join(".termeric-upload-fixture");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary_path)
            .unwrap();
        let mut temporary = TemporaryUpload {
            path: temporary_path.clone(),
            file,
            remove_on_drop: true,
        };
        temporary.file.write_all(b"first").unwrap();
        let target = directory.path().join("artifact.bin");
        publish_temporary_upload(&mut temporary, &target).unwrap();
        drop(temporary);
        assert!(!temporary_path.exists());
        assert_eq!(fs::read(&target).unwrap(), b"first");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let second_path = directory.path().join(".termeric-upload-second");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&second_path)
            .unwrap();
        let mut second = TemporaryUpload {
            path: second_path.clone(),
            file,
            remove_on_drop: true,
        };
        second.file.write_all(b"second").unwrap();
        assert!(publish_temporary_upload(&mut second, &target).is_err());
        drop(second);
        assert!(!second_path.exists());
        assert_eq!(fs::read(target).unwrap(), b"first");
    }

    #[test]
    fn retained_upload_cleanup_expires_idle_records_and_preserves_active_work() {
        let directory = tempfile::tempdir().unwrap();
        let idle_path = directory.path().join("idle.part");
        let active_path = directory.path().join("active.part");
        let manual_path = directory.path().join("manual.part");
        fs::write(&idle_path, b"idle").unwrap();
        fs::write(&active_path, b"active").unwrap();
        fs::write(&manual_path, b"manual").unwrap();
        let old = Instant::now() - RETAINED_UPLOAD_TTL - Duration::from_secs(1);
        let descriptor = |name: &str, total_bytes| UploadDescriptor {
            domain: "daily".into(),
            pane: 0,
            name: name.into(),
            total_bytes,
            kind: UploadKind::File,
            retention: UploadRetention::Automatic,
        };
        let mut manual_descriptor = descriptor("manual.bin", 6);
        manual_descriptor.retention = UploadRetention::Manual;
        let state = Arc::new(Mutex::new(ServerState {
            domains: HashMap::new(),
            allow_screen_checksum_reports: false,
            xterm_keyboard_type: XtermKeyboardType::Normal,
            shutdown: Arc::new(AtomicBool::new(false)),
            wake_socket: None,
            next_attachment: AtomicU64::new(1),
            next_topology_subscription: AtomicU64::new(1),
            active_uploads: 1,
            upload_ids_in_flight: HashMap::new(),
            upload_records: HashMap::from([
                (
                    "11111111111111111111111111111111".into(),
                    UploadRecord {
                        descriptor: descriptor("idle.bin", 4),
                        state: UploadRecordState::Partial {
                            temporary_path: idle_path.clone(),
                            target_path: directory.path().join("idle.bin"),
                            bytes_received: 4,
                            device: 1,
                            inode: 1,
                            active: false,
                        },
                        updated: old,
                    },
                ),
                (
                    "22222222222222222222222222222222".into(),
                    UploadRecord {
                        descriptor: descriptor("active.bin", 6),
                        state: UploadRecordState::Partial {
                            temporary_path: active_path.clone(),
                            target_path: directory.path().join("active.bin"),
                            bytes_received: 6,
                            device: 2,
                            inode: 2,
                            active: true,
                        },
                        updated: old,
                    },
                ),
                (
                    "33333333333333333333333333333333".into(),
                    UploadRecord {
                        descriptor: descriptor("done.bin", 3),
                        state: UploadRecordState::Complete {
                            response: Response::FileUploaded {
                                path: directory.path().join("done.bin").display().to_string(),
                                bytes: 3,
                            },
                        },
                        updated: old,
                    },
                ),
                (
                    "66666666666666666666666666666666".into(),
                    UploadRecord {
                        descriptor: manual_descriptor,
                        state: UploadRecordState::Partial {
                            temporary_path: manual_path.clone(),
                            target_path: directory.path().join("manual.bin"),
                            bytes_received: 6,
                            device: 3,
                            inode: 3,
                            active: false,
                        },
                        updated: old,
                    },
                ),
            ]),
            upload_janitor_running: false,
        }));

        {
            let mut state = state.lock().unwrap();
            assert!(retained_upload_capacity_available(
                &state,
                &descriptor("new.bin", protocol::MAX_FILE_UPLOAD_BYTES)
            ));
            state.upload_ids_in_flight.insert(
                "44444444444444444444444444444444".into(),
                descriptor("queued-a.bin", protocol::MAX_FILE_UPLOAD_BYTES),
            );
            state.upload_ids_in_flight.insert(
                "55555555555555555555555555555555".into(),
                descriptor("queued-b.bin", protocol::MAX_FILE_UPLOAD_BYTES),
            );
            assert!(!retained_upload_capacity_available(
                &state,
                &descriptor("new.bin", 1)
            ));
            state.upload_ids_in_flight.clear();
        }

        prune_expired_upload_records(&state, false);
        assert!(!idle_path.exists());
        assert!(active_path.exists());
        assert!(manual_path.exists());
        {
            let state = state.lock().unwrap();
            assert_eq!(state.upload_records.len(), 2);
            assert!(
                state
                    .upload_records
                    .contains_key("22222222222222222222222222222222")
            );
        }

        let Response::Uploads { uploads } = list_retained_uploads(&state) else {
            panic!("expected retained uploads");
        };
        assert_eq!(uploads.len(), 2);
        assert!(uploads.iter().any(|upload| {
            upload.upload_id == "66666666666666666666666666666666"
                && upload.retention == UploadRetention::Manual
                && !upload.active
                && upload.expires_in_seconds > 60
        }));
        assert!(
            cancel_retained_upload(&state, "22222222222222222222222222222222")
                .unwrap_err()
                .to_string()
                .contains("active")
        );
        assert!(matches!(
            cancel_retained_upload(&state, "66666666666666666666666666666666"),
            Ok(Response::Ok { .. })
        ));
        assert!(!manual_path.exists());

        prune_expired_upload_records(&state, true);
        assert!(!active_path.exists());
        assert!(state.lock().unwrap().upload_records.is_empty());
    }

    #[test]
    fn pane_input_admission_is_bounded_and_never_waits_for_the_writer() {
        let (sender, receiver) =
            std::sync::mpsc::sync_channel::<Vec<u8>>(PANE_INPUT_QUEUE_CAPACITY);
        for value in 0..PANE_INPUT_QUEUE_CAPACITY {
            queue_pane_input(&sender, vec![value as u8]).unwrap();
        }
        let full = queue_pane_input(&sender, vec![0xff]).unwrap_err();
        assert!(format!("{full:#}").contains("backpressure limit"));

        drop(receiver);
        let disconnected = queue_pane_input(&sender, vec![0xfe]).unwrap_err();
        assert!(format!("{disconnected:#}").contains("writer is unavailable"));
    }

    #[test]
    fn split_layout_preserves_existing_branches_and_exact_direction() {
        let mut layout = LivePaneLayout::Pane { pane: 0 };
        assert!(split_layout(
            &mut layout,
            0,
            1,
            10,
            SplitDirection::Vertical
        ));
        assert!(split_layout(
            &mut layout,
            1,
            2,
            11,
            SplitDirection::Horizontal
        ));
        assert!(!split_layout(
            &mut layout,
            99,
            3,
            12,
            SplitDirection::Vertical
        ));
        assert_eq!(resize_layout_split(&mut layout, 11, 350), Some(true));
        assert_eq!(resize_layout_split(&mut layout, 11, 350), Some(false));
        assert_eq!(resize_layout_split(&mut layout, 999, 500), None);
        assert!(matches!(
            &layout,
            LivePaneLayout::Split {
                split_id: 10,
                ratio_millis: 500,
                second,
                ..
            } if matches!(
                second.as_ref(),
                LivePaneLayout::Split {
                    split_id: 11,
                    ratio_millis: 350,
                    ..
                }
            )
        ));
        let encoded = serde_json::to_string(&layout).unwrap();
        assert!(encoded.contains("vertical"));
        assert!(encoded.contains("horizontal"));
    }

    #[test]
    fn closing_a_layout_leaf_collapses_only_its_parent_and_preserves_order() {
        let mut layout = LivePaneLayout::Pane { pane: 0 };
        assert!(split_layout(
            &mut layout,
            0,
            1,
            10,
            SplitDirection::Vertical
        ));
        assert!(split_layout(
            &mut layout,
            1,
            2,
            11,
            SplitDirection::Horizontal
        ));

        let mut layout = remove_layout_pane(layout, 1).unwrap();
        assert_eq!(
            layout,
            LivePaneLayout::Split {
                split_id: 10,
                direction: SplitDirection::Vertical,
                ratio_millis: 500,
                first: Box::new(LivePaneLayout::Pane { pane: 0 }),
                second: Box::new(LivePaneLayout::Pane { pane: 2 }),
            }
        );
        assert_eq!(
            resize_layout_split(&mut layout, 11, 350),
            None,
            "the collapsed child split ID must become stale"
        );
        assert_eq!(resize_layout_split(&mut layout, 10, 650), Some(true));
        assert_eq!(first_layout_pane(&layout), 0);
        assert_eq!(
            remove_layout_pane(layout, 0),
            Some(LivePaneLayout::Pane { pane: 2 })
        );
        assert_eq!(
            remove_layout_pane(LivePaneLayout::Pane { pane: 2 }, 2),
            None
        );
    }

    #[test]
    fn pane_layout_operations_preserve_leaf_identity_and_allocate_stable_splits() {
        let mut next_split = 20;
        let mut layout =
            preset_layout(&[3, 4, 5], SplitDirection::Vertical, true, &mut next_split).unwrap();
        assert_eq!(next_split, 22);
        assert!(matches!(
            &layout,
            LivePaneLayout::Split {
                split_id: 20,
                direction: SplitDirection::Vertical,
                ratio_millis: 667,
                first,
                second,
            } if matches!(
                first.as_ref(),
                LivePaneLayout::Split {
                    split_id: 21,
                    direction: SplitDirection::Horizontal,
                    ratio_millis: 500,
                    ..
                }
            ) && matches!(second.as_ref(), LivePaneLayout::Pane { pane: 5 })
        ));
        let mut panes = Vec::new();
        collect_layout_panes(&layout, &mut panes);
        assert_eq!(panes, vec![3, 4, 5]);

        swap_layout_panes(&mut layout, 4, 5);
        panes.clear();
        collect_layout_panes(&layout, &mut panes);
        assert_eq!(panes, vec![3, 5, 4]);
        assert!(balance_layout(&mut layout));
        assert!(!balance_layout(&mut layout));
        assert!(matches!(
            layout,
            LivePaneLayout::Split {
                ratio_millis: 500,
                ..
            }
        ));

        let mut next_split = 30;
        let tall =
            master_detail_layout(&[3, 4, 5, 6], 5, SplitDirection::Vertical, &mut next_split)
                .unwrap();
        assert_eq!(next_split, 33);
        assert!(matches!(
            &tall,
            LivePaneLayout::Split {
                split_id: 30,
                direction: SplitDirection::Vertical,
                ratio_millis: 500,
                first,
                second,
            } if matches!(first.as_ref(), LivePaneLayout::Pane { pane: 5 })
                && matches!(second.as_ref(), LivePaneLayout::Split {
                    split_id: 31,
                    direction: SplitDirection::Horizontal,
                    ratio_millis: 667,
                    ..
                })
        ));
        let mut panes = Vec::new();
        collect_layout_panes(&tall, &mut panes);
        assert_eq!(panes, vec![5, 3, 4, 6]);

        let mut next_split = 40;
        let fat = master_detail_layout(
            &[3, 4, 5, 6],
            4,
            SplitDirection::Horizontal,
            &mut next_split,
        )
        .unwrap();
        assert!(matches!(
            &fat,
            LivePaneLayout::Split {
                direction: SplitDirection::Horizontal,
                ratio_millis: 500,
                first,
                second,
                ..
            } if matches!(first.as_ref(), LivePaneLayout::Pane { pane: 4 })
                && matches!(second.as_ref(), LivePaneLayout::Split {
                    direction: SplitDirection::Vertical,
                    ..
                })
        ));
        panes.clear();
        collect_layout_panes(&fat, &mut panes);
        assert_eq!(panes, vec![4, 3, 5, 6]);
        assert!(
            master_detail_layout(&[3, 4], 99, SplitDirection::Vertical, &mut next_split)
                .unwrap_err()
                .to_string()
                .contains("focused pane")
        );
    }

    #[test]
    fn negotiation_requires_exact_version_and_authentication_token() {
        assert!(
            validate_hello(
                ClientFrame::Hello {
                    version: protocol::PROTOCOL_VERSION,
                    token: "expected".into(),
                },
                "expected",
            )
            .is_ok()
        );
        assert!(
            validate_hello(
                ClientFrame::Hello {
                    version: protocol::PROTOCOL_VERSION,
                    token: "wrong".into(),
                },
                "expected",
            )
            .unwrap_err()
            .contains("authentication")
        );
        assert!(
            validate_hello(
                ClientFrame::Hello {
                    version: protocol::PROTOCOL_VERSION + 1,
                    token: "expected".into(),
                },
                "expected",
            )
            .unwrap_err()
            .contains("incompatible")
        );
        assert!(validate_hello(ClientFrame::Detach, "expected").is_err());
    }

    #[test]
    fn text_truncation_keeps_valid_utf8_from_the_newest_end() {
        assert_eq!(newest_utf8_bytes("root-λ", 2), ("λ".into(), true));
        assert_eq!(newest_utf8_bytes("short", 10), ("short".into(), false));
    }

    #[test]
    fn terminal_attach_snapshot_is_bounded_and_resets_the_replica() {
        let mut terminal = Terminal::new(10, 2);
        terminal.process(
            b"\x1b[5 q\x1b[?12h\x1b[?25l\x1b[?1061h\x1b[=3u\x1b[>0;15m\x1b[>0;42f\x1b[>1;4m\x1b[>1;1f\x1b[>2;4m\x1b[>2;1f\x1b[>3;4m\x1b[>3;1f\x1b[>4;2:1m\x1b[>4;1f\x1b[>5;4m\x1b[>5;1f\x1b[>6;4m\x1b[>6;1f\x1b[>7;4m\x1b[>7;1f",
        );
        terminal.process(b"\x1bP0;1|17/7265706c79\x1b\\");
        terminal.process("a very long λ line".repeat(100).as_bytes());
        terminal.process(b"\x1b[H\x1b[1\"qX");
        let snapshot = terminal_snapshot(&terminal, 1024);
        assert!(snapshot.starts_with(b"\x1bc\x1b[2J\x1b[H"));
        assert!(snapshot.len() <= 1024);
        assert!(String::from_utf8_lossy(&snapshot).contains("\x1b[?25l"));

        let mut replica = Terminal::new(10, 2);
        replica.process(&snapshot);
        assert_eq!(replica.cursor_blink_override(), Some(true));
        assert!(!replica.grid().cursor().visible);
        assert_eq!(replica.cursor_style_parameter(), 5);
        assert_eq!(replica.character_protection_parameter(), 1);
        assert_eq!(replica.kitty_keyboard_flags().bits(), 3);
        assert_eq!(
            replica.xterm_keyboard_type(),
            termeric_core::term::XtermKeyboardType::Vt220
        );
        assert_eq!(
            replica.xterm_user_defined_key(17),
            Some(b"reply".as_slice())
        );
        assert_eq!(
            replica.xterm_keyboard_resource(),
            termeric_core::term::XtermKeyboardResource::new(15, 42)
        );
        assert_eq!(
            replica.xterm_cursor_keys().mode(),
            termeric_core::term::XtermKeyMode::Extended
        );
        assert_eq!(
            replica.xterm_cursor_keys().format(),
            termeric_core::term::XtermKeyFormat::CsiU
        );
        assert_eq!(
            replica.xterm_function_keys().mode(),
            termeric_core::term::XtermKeyMode::Extended
        );
        assert_eq!(
            replica.xterm_function_keys().format(),
            termeric_core::term::XtermKeyFormat::CsiU
        );
        assert_eq!(
            replica.xterm_keypad_keys().mode(),
            termeric_core::term::XtermKeyMode::Extended
        );
        assert_eq!(
            replica.xterm_keypad_keys().format(),
            termeric_core::term::XtermKeyFormat::CsiU
        );
        assert_eq!(
            replica.xterm_string_keys().mode(),
            termeric_core::term::XtermKeyMode::Extended
        );
        assert_eq!(
            replica.xterm_string_keys().format(),
            termeric_core::term::XtermKeyFormat::CsiU
        );
        assert_eq!(
            replica.xterm_modifier_keys().mode(),
            termeric_core::term::XtermKeyMode::Extended
        );
        assert_eq!(
            replica.xterm_modifier_keys().format(),
            termeric_core::term::XtermKeyFormat::CsiU
        );
        assert_eq!(
            replica.xterm_special_keys().mode(),
            termeric_core::term::XtermKeyMode::Extended
        );
        assert_eq!(
            replica.xterm_special_keys().format(),
            termeric_core::term::XtermKeyFormat::CsiU
        );
        assert_eq!(replica.xterm_modify_other_keys().level(), 2);
        assert_eq!(replica.xterm_modify_other_keys().modifier_mask(), 1);
        assert_eq!(
            replica.xterm_modify_other_keys().format(),
            termeric_core::term::XtermKeyFormat::CsiU
        );
        assert!(
            replica
                .grid()
                .cell(0, 0)
                .unwrap()
                .flags
                .contains(termeric_core::CellFlags::PROTECTED)
        );
    }

    #[test]
    fn terminal_attach_snapshot_preserves_osc66_content_and_geometry_once() {
        let mut terminal = Terminal::new(10, 3);
        terminal.process(b"\x1b]66;s=2:w=2;domain\x07!");

        let snapshot = terminal_snapshot(&terminal, 4096);
        assert_eq!(
            String::from_utf8_lossy(&snapshot).matches("domain").count(),
            1
        );

        let mut replica = Terminal::new(10, 3);
        replica.process(&snapshot);
        let region = replica.grid().multicell_region_at(1, 3).unwrap();
        assert_eq!((region.top, region.left), (0, 0));
        assert_eq!(
            (region.descriptor.rows(), region.descriptor.columns()),
            (2, 4)
        );
        assert_eq!(
            replica.grid().cell(0, 0).unwrap().content.to_text(),
            "domain"
        );
        assert_eq!(replica.grid().cell(0, 4).unwrap().content.to_text(), "!");
    }

    #[test]
    fn terminal_attach_snapshot_replays_multiple_cursors_without_replies() {
        let mut terminal = Terminal::new(10, 3);
        terminal.process(
            b"domain\x1b[>1;2:1:2:1:3 q\x1b[>29;2:3:10 q\x1b[>30;2:1:2:3 q\x1b[>40;5:196 q",
        );

        let snapshot = terminal_snapshot(&terminal, 4096);
        assert!(snapshot.len() <= 4096);
        let mut replica = Terminal::new(10, 3);
        replica.process(&snapshot);
        assert_eq!(
            replica.extra_cursors().cursors().collect::<Vec<_>>(),
            terminal.extra_cursors().cursors().collect::<Vec<_>>()
        );
        assert_eq!(
            replica.extra_cursors().colors(),
            terminal.extra_cursors().colors()
        );
        assert!(replica.drain_events().is_empty());
    }

    #[test]
    fn terminal_attach_snapshot_replays_an_idle_prompt_and_a_cleared_screen_exactly() {
        for output in [
            &b"older\r\noutput\r\nuser@host:~$ "[..],
            &b"line 1\r\nline 2\r\nline 3\r\nline 4\x1b[H\x1b[2J$ "[..],
        ] {
            let mut terminal = Terminal::new(20, 4);
            terminal.process(output);
            let mut replica = Terminal::new(20, 4);
            replica.process(&terminal_snapshot(&terminal, 4096));
            for row in 0..4 {
                assert_eq!(replica.rows_text(row, row), terminal.rows_text(row, row));
            }
            assert_eq!(
                (replica.grid().cursor().row, replica.grid().cursor().col),
                (terminal.grid().cursor().row, terminal.grid().cursor().col)
            );
        }
    }

    #[test]
    fn terminal_attach_snapshot_restores_decoded_graphics_after_the_text_ris() {
        let mut terminal = Terminal::new(10, 2);
        terminal.process(b"prompt\x1b_Ga=t,f=32,s=1,v=1,i=7;/wAA/w==\x1b\\");
        terminal.process(b"\x1b[2;4H\x1b_Ga=p,i=7,p=3,C=1\x1b\\");
        terminal.drain_events();

        let text = terminal_snapshot(&terminal, 1024);
        let graphics = terminal.graphics_relay_snapshot(Instant::now());
        let mut replica = Terminal::new(10, 2);
        replica.set_graphics_relay_snapshot_allowed(true);
        replica.process(&text);
        graphics
            .for_each_escape_chunk(|chunk| {
                replica.process(chunk.as_bytes());
                true
            })
            .unwrap();

        assert_eq!(replica.rows_text(0, 0), "prompt");
        let images = replica.presentation_images();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].placement.placement_id, Some(3));
        assert_eq!(images[0].image.data.as_ref(), [255, 0, 0, 255]);
        assert!(replica.drain_events().is_empty());
    }

    #[test]
    fn terminal_attach_snapshot_forces_normal_keyboard_type_on_configured_replica() {
        let terminal = Terminal::new(10, 2);
        let snapshot = terminal_snapshot(&terminal, 1024);
        assert!(String::from_utf8_lossy(&snapshot).contains("\x1b[?1060l"));

        let mut replica = Terminal::new(10, 2);
        replica.configure_xterm_keyboard_type(XtermKeyboardType::Legacy, true);
        replica.process(&snapshot);
        assert_eq!(replica.xterm_keyboard_type(), XtermKeyboardType::Normal);
    }

    #[test]
    fn domain_panes_inherit_the_server_checksum_policy() {
        if !std::path::Path::new("/bin/true").exists() {
            return;
        }
        let spec = SpawnSpec {
            program: Some("/bin/true".into()),
            ..SpawnSpec::default()
        };
        let (disabled, _reader) =
            PaneRuntime::spawn(0, &spec, false, XtermKeyboardType::Legacy).unwrap();
        assert!(!disabled.terminal.screen_checksum_reporting());
        assert!(!disabled.terminal.kitty_file_transmission());
        assert_eq!(
            disabled.terminal.xterm_keyboard_type(),
            XtermKeyboardType::Legacy
        );
        let (enabled, _reader) =
            PaneRuntime::spawn(1, &spec, true, XtermKeyboardType::Vt220).unwrap();
        assert!(enabled.terminal.screen_checksum_reporting());
        assert!(!enabled.terminal.kitty_file_transmission());
        assert_eq!(
            enabled.terminal.xterm_keyboard_type(),
            XtermKeyboardType::Vt220
        );
    }

    #[cfg(unix)]
    #[test]
    fn domain_panes_spawn_and_resize_with_nonzero_kernel_pixels() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let spec = SpawnSpec {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 1".into()],
            cols: 83,
            rows: 27,
            ..SpawnSpec::default()
        };
        let (mut pane, _reader) =
            PaneRuntime::spawn(0, &spec, false, XtermKeyboardType::Normal).unwrap();
        let bootstrap = PtySize {
            cols: 83,
            rows: 27,
            pixel_width: 664,
            pixel_height: 432,
        };
        assert_eq!(pane.pty.size().unwrap(), bootstrap);
        assert_eq!((pane.pixel_width, pane.pixel_height), (664, 432));

        let renderer_geometry = PtySize {
            cols: 83,
            rows: 27,
            pixel_width: 971,
            pixel_height: 629,
        };
        pane.resize(renderer_geometry).unwrap();
        assert_eq!(pane.pty.size().unwrap(), renderer_geometry);
        // Exact repeats are no-ops at both the pane and core PTY layers.
        pane.resize(renderer_geometry).unwrap();
        assert_eq!(pane.pty.size().unwrap(), renderer_geometry);

        // A parent terminal that reports no pixels must not erase the valid
        // extent. Cell-count changes retain the last known physical cell size.
        pane.resize(PtySize {
            cols: 41,
            rows: 19,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
        let inferred = PtySize {
            cols: 41,
            rows: 19,
            pixel_width: 480,
            pixel_height: 443,
        };
        assert_eq!(pane.pty.size().unwrap(), inferred);
        assert_eq!(
            (pane.terminal.grid().cols(), pane.terminal.grid().rows()),
            (41, 19)
        );
        pane.resize(PtySize {
            cols: 8,
            rows: 5,
            pixel_width: 16,
            pixel_height: 15,
        })
        .unwrap();
        crate::test_support::seed_pixel_geometry_retention_probe(&mut pane.terminal);
        let scaled = PtySize {
            cols: 8,
            rows: 2,
            pixel_width: 8,
            pixel_height: 2,
        };
        pane.resize(scaled).unwrap();
        assert_eq!(pane.pty.size().unwrap(), scaled);
        crate::test_support::assert_pixel_geometry_retention_probe(&pane.terminal);
    }

    #[test]
    fn relay_pixel_normalization_rounds_and_never_returns_zero() {
        assert_eq!(normalize_relay_pixel_extent(777, 971, 105, 59, 8), 777);
        assert_eq!(normalize_relay_pixel_extent(0, 971, 105, 59, 8), 546);
        assert_eq!(normalize_relay_pixel_extent(0, 0, 0, 80, 8), 640);
        assert_eq!(normalize_relay_pixel_extent(0, 0, 0, 0, 8), 1);
    }

    #[test]
    fn local_server_survives_disconnect_and_supports_the_complete_control_surface() {
        let temporary = tempfile::tempdir().unwrap();
        let reported_cwd = temporary.path().join("reported-cwd");
        std::fs::create_dir(&reported_cwd).unwrap();
        let paths = RuntimePaths::from_directory(temporary.path().join("runtime")).unwrap();
        // Some test sandboxes deny AF_UNIX entirely. Keep the integration
        // fixture active everywhere else while retaining protocol/model unit
        // coverage in those restricted environments.
        let probe = paths.directory.join("socket-probe");
        match UnixListener::bind(&probe) {
            Ok(listener) => {
                drop(listener);
                fs::remove_file(&probe).unwrap();
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skip domain socket integration: sandbox denies AF_UNIX");
                return;
            }
            Err(error) => panic!("could not probe Unix-domain socket support: {error}"),
        }
        let server_paths = paths.clone();
        let server = thread::spawn(move || {
            run_foreground(server_paths, false, XtermKeyboardType::Normal).unwrap()
        });
        assert!(wait_for(Duration::from_secs(2), || paths.socket.exists()));
        let client = crate::domain::DomainClient::for_paths(paths.clone());

        // Version negotiation fails closed before a request is accepted.
        let mut incompatible = UnixStream::connect(&paths.socket).unwrap();
        protocol::write_frame(
            &mut incompatible,
            &ClientFrame::Hello {
                version: protocol::PROTOCOL_VERSION + 1,
                token: paths.read_token().unwrap(),
            },
        )
        .unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut incompatible).unwrap(),
            ServerFrame::Error { message } if message.contains("incompatible")
        ));

        let response = client
            .request(Request::CreateDomain {
                name: "integration".into(),
                spawn: SpawnSpec {
                    program: Some("/bin/sh".into()),
                    args: vec![
                        "-c".into(),
                        format!(
                            "printf '\\033]7;file://{}\\007root-ready\\n'; sleep 5",
                            reported_cwd.display()
                        ),
                    ],
                    ..SpawnSpec::default()
                },
            })
            .unwrap();
        let Response::Domain { domain } = response else {
            panic!("expected created domain");
        };
        let domain_id = domain.id;

        // A topology subscription receives an atomic initial snapshot, then
        // strictly increasing whole-domain revisions on structural changes.
        let mut topology = UnixStream::connect(&paths.socket).unwrap();
        protocol::write_frame(
            &mut topology,
            &ClientFrame::Hello {
                version: protocol::PROTOCOL_VERSION,
                token: paths.read_token().unwrap(),
            },
        )
        .unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut topology).unwrap(),
            ServerFrame::Hello { .. }
        ));
        protocol::write_frame(
            &mut topology,
            &ClientFrame::Request {
                request: Request::SubscribeTopology {
                    domain: domain_id.clone(),
                },
            },
        )
        .unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut topology).unwrap(),
            ServerFrame::Response {
                response: Response::TopologySubscribed {
                    revision: 1,
                    domain
                }
            } if domain.tabs.len() == 1
        ));
        assert!(wait_for(Duration::from_secs(2), || {
            matches!(
                client.request(Request::GetText {
                    domain: domain_id.clone(),
                    pane: 0,
                    max_bytes: 4096,
                }),
                Ok(Response::Text { text, .. }) if text.contains("root-ready")
            )
        }));

        let upload_source = temporary.path().join("local-upload-source.bin");
        let upload_bytes = (0..150_000)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        fs::write(&upload_source, &upload_bytes).unwrap();
        let response = client
            .upload_file(
                domain_id.clone(),
                0,
                &upload_source,
                Some("artifact.bin".into()),
            )
            .unwrap();
        assert!(matches!(
            response,
            Response::FileUploaded { ref path, bytes }
                if path == &reported_cwd.join("artifact.bin").display().to_string()
                    && bytes == upload_bytes.len() as u64
        ));
        assert_eq!(
            fs::read(reported_cwd.join("artifact.bin")).unwrap(),
            upload_bytes
        );

        let directory_source = temporary.path().join("local-project");
        fs::create_dir(&directory_source).unwrap();
        fs::create_dir(directory_source.join("empty")).unwrap();
        fs::create_dir(directory_source.join("bin")).unwrap();
        fs::write(directory_source.join("README.md"), b"directory upload\n").unwrap();
        fs::write(directory_source.join("bin/run"), b"#!/bin/sh\n").unwrap();
        fs::set_permissions(
            directory_source.join("bin/run"),
            fs::Permissions::from_mode(0o751),
        )
        .unwrap();
        let directory_upload_id = "66666666666666666666666666666666";
        let directory_response = client
            .upload_directory_with_test_id(
                domain_id.clone(),
                0,
                &directory_source,
                "project".into(),
                directory_upload_id.into(),
            )
            .unwrap();
        assert!(matches!(
            directory_response,
            Response::DirectoryUploaded {
                ref path,
                bytes,
                entries: 4,
            } if path == &reported_cwd.join("project").display().to_string() && bytes > 0
        ));
        assert_eq!(
            fs::read(reported_cwd.join("project/README.md")).unwrap(),
            b"directory upload\n"
        );
        assert!(reported_cwd.join("project/empty").is_dir());
        assert_eq!(
            fs::metadata(reported_cwd.join("project/README.md"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(reported_cwd.join("project/bin/run"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            client
                .upload_directory_with_test_id(
                    domain_id.clone(),
                    0,
                    &directory_source,
                    "project".into(),
                    directory_upload_id.into(),
                )
                .unwrap(),
            directory_response
        );
        let directory_overwrite = client
            .upload_directory(
                domain_id.clone(),
                0,
                &directory_source,
                Some("project".into()),
            )
            .unwrap_err();
        assert!(format!("{directory_overwrite:#}").contains("never overwrite"));
        assert!(fs::read_dir(&reported_cwd).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".termeric-directory-upload-")
        }));

        assert_eq!(
            fs::metadata(reported_cwd.join("artifact.bin"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let overwrite_error = client
            .upload_file(
                domain_id.clone(),
                0,
                &upload_source,
                Some("artifact.bin".into()),
            )
            .unwrap_err();
        assert!(format!("{overwrite_error:#}").contains("never overwrite"));
        assert_eq!(
            fs::read(reported_cwd.join("artifact.bin")).unwrap(),
            upload_bytes
        );

        let cancellation = AtomicBool::new(false);
        let mut progress = Vec::new();
        let cancelled = client
            .upload_open_file_with_control(
                domain_id.clone(),
                0,
                fs::File::open(&upload_source).unwrap(),
                "cancelled.bin".into(),
                &cancellation,
                |bytes_sent, total_bytes| {
                    progress.push((bytes_sent, total_bytes));
                    if bytes_sent >= protocol::MAX_STREAM_CHUNK_BYTES as u64 {
                        cancellation.store(true, Ordering::Release);
                    }
                },
            )
            .unwrap();
        assert!(matches!(
            cancelled,
            Response::UploadCancelled { bytes_received }
                if bytes_received == protocol::MAX_STREAM_CHUNK_BYTES as u64
        ));
        assert!(
            progress
                .windows(2)
                .all(|pair| pair[0].0 <= pair[1].0 && pair[0].1 == pair[1].1)
        );
        assert!(!reported_cwd.join("cancelled.bin").exists());
        assert!(wait_for(Duration::from_secs(2), || {
            fs::read_dir(&reported_cwd).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".termeric-upload-")
            })
        }));

        // A disconnected partial stream never publishes a destination. Its
        // private staging file remains bounded and can be resumed only by the
        // exact authenticated upload identity and immutable descriptor.
        let resumable_id = "11111111111111111111111111111111";
        let mut partial = UnixStream::connect(&paths.socket).unwrap();
        protocol::write_frame(
            &mut partial,
            &ClientFrame::Hello {
                version: protocol::PROTOCOL_VERSION,
                token: paths.read_token().unwrap(),
            },
        )
        .unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut partial).unwrap(),
            ServerFrame::Hello { .. }
        ));
        protocol::write_frame(
            &mut partial,
            &ClientFrame::Request {
                request: Request::UploadFile {
                    domain: domain_id.clone(),
                    pane: 0,
                    name: "partial.bin".into(),
                    total_bytes: 10,
                    upload_id: resumable_id.into(),
                    kind: UploadKind::File,
                    retention: UploadRetention::Automatic,
                    resume_only: false,
                },
            },
        )
        .unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut partial).unwrap(),
            ServerFrame::Response {
                response: Response::UploadReady { .. }
            }
        ));
        protocol::write_frame(
            &mut partial,
            &ClientFrame::UploadChunk {
                offset: 0,
                data: b"abc".to_vec(),
            },
        )
        .unwrap();
        drop(partial);
        assert!(wait_for(Duration::from_secs(2), || {
            !reported_cwd.join("partial.bin").exists()
                && fs::read_dir(&reported_cwd).unwrap().any(|entry| {
                    entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".termeric-upload-")
                })
        }));
        let resume_source = temporary.path().join("resume-source.bin");
        fs::write(&resume_source, b"abcdefghij").unwrap();
        let missing_resume = client
            .upload_path_with_manual_resume(
                domain_id.clone(),
                0,
                &resume_source,
                Some("missing.bin".into()),
                "99999999999999999999999999999999".into(),
                true,
            )
            .unwrap_err();
        assert!(format!("{missing_resume:#}").contains("not found or has expired"));

        let wrong_resume_source = temporary.path().join("wrong-resume-source.bin");
        fs::write(&wrong_resume_source, b"xxxdefghij").unwrap();
        let wrong_prefix = client
            .upload_path_with_manual_resume(
                domain_id.clone(),
                0,
                &wrong_resume_source,
                Some("partial.bin".into()),
                resumable_id.into(),
                true,
            )
            .unwrap_err();
        assert!(format!("{wrong_prefix:#}").contains("retained prefix"));
        let mut observed_uploads = Vec::new();
        assert!(
            wait_for(Duration::from_secs(2), || {
                let Ok(Response::Uploads { uploads }) = client.request(Request::ListUploads) else {
                    return false;
                };
                let retained = uploads.iter().any(|upload| {
                    upload.upload_id == resumable_id
                        && upload.bytes_received == 3
                        && upload.retention == UploadRetention::Manual
                        && !upload.active
                });
                observed_uploads = uploads;
                retained
            }),
            "retained upload did not become inactive: {observed_uploads:?}"
        );

        let cancellation = AtomicBool::new(false);
        let mut resume_progress = Vec::new();
        let resumed = client
            .upload_open_file_with_test_id(
                domain_id.clone(),
                0,
                fs::File::open(&resume_source).unwrap(),
                "partial.bin".into(),
                resumable_id.into(),
                &cancellation,
                |sent, total| resume_progress.push((sent, total)),
            )
            .unwrap();
        assert!(matches!(resumed, Response::FileUploaded { bytes: 10, .. }));
        assert!(resume_progress.contains(&(3, 10)));
        assert_eq!(
            fs::read(reported_cwd.join("partial.bin")).unwrap(),
            b"abcdefghij"
        );
        assert!(fs::read_dir(&reported_cwd).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".termeric-upload-")
        }));

        // A lost completion response is idempotently replayed, while the same
        // random identity cannot be rebound to another destination.
        let replayed = client
            .upload_open_file_with_test_id(
                domain_id.clone(),
                0,
                fs::File::open(&resume_source).unwrap(),
                "partial.bin".into(),
                resumable_id.into(),
                &cancellation,
                |_, _| {},
            )
            .unwrap();
        assert_eq!(replayed, resumed);
        let mismatch = client
            .upload_open_file_with_test_id(
                domain_id.clone(),
                0,
                fs::File::open(&resume_source).unwrap(),
                "different.bin".into(),
                resumable_id.into(),
                &cancellation,
                |_, _| {},
            )
            .unwrap_err();
        assert!(format!("{mismatch:#}").contains("different source or destination"));

        let Response::Domain { domain } = client
            .request(Request::SplitPane {
                domain: domain_id.clone(),
                pane: 0,
                direction: SplitDirection::Vertical,
                spawn: SpawnSpec {
                    program: Some("/bin/sh".into()),
                    args: vec!["-c".into(), "printf 'split-ready\\n'; sleep 5".into()],
                    ..SpawnSpec::default()
                },
            })
            .unwrap()
        else {
            panic!("expected split domain");
        };
        assert_eq!(domain.workspace.tabs[0].panes.len(), 2);
        assert_eq!(
            domain.workspace.tabs[0]
                .panes
                .iter()
                .find(|pane| pane.id == 1)
                .and_then(|pane| pane.working_directory.as_deref()),
            reported_cwd.to_str(),
            "a local-domain split must inherit its source terminal's OSC 7 cwd"
        );
        assert!(matches!(
            &domain.workspace.tabs[0].layout,
            LivePaneLayout::Split { .. }
        ));
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut topology).unwrap(),
            ServerFrame::TopologyChanged {
                revision: 2,
                domain
            } if domain.tabs[0].panes.len() == 2
        ));
        let Response::Domain { domain } = client
            .request(Request::ResizeSplit {
                domain: domain_id.clone(),
                split: 0,
                ratio_millis: 350,
            })
            .unwrap()
        else {
            panic!("expected resized domain");
        };
        assert!(matches!(
            &domain.workspace.tabs[0].layout,
            LivePaneLayout::Split {
                split_id: 0,
                ratio_millis: 350,
                ..
            }
        ));
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut topology).unwrap(),
            ServerFrame::TopologyChanged {
                revision: 3,
                domain
            } if matches!(
                domain.tabs[0].layout,
                LivePaneLayout::Split {
                    split_id: 0,
                    ratio_millis: 350,
                    ..
                }
            )
        ));
        assert!(
            client
                .request(Request::ResizeSplit {
                    domain: domain_id.clone(),
                    split: 999,
                    ratio_millis: 500,
                })
                .unwrap_err()
                .to_string()
                .contains("split not found")
        );
        assert!(
            client
                .request(Request::ResizeSplit {
                    domain: domain_id.clone(),
                    split: 0,
                    ratio_millis: 99,
                })
                .unwrap_err()
                .to_string()
                .contains("between 100 and 900")
        );
        let Response::Domain { domain } = client
            .request(Request::SwapPaneNext {
                domain: domain_id.clone(),
                pane: 1,
            })
            .unwrap()
        else {
            panic!("expected domain after swapping panes");
        };
        assert!(matches!(
            &domain.workspace.tabs[0].layout,
            LivePaneLayout::Split {
                first,
                second,
                ..
            } if matches!(first.as_ref(), LivePaneLayout::Pane { pane: 1 })
                && matches!(second.as_ref(), LivePaneLayout::Pane { pane: 0 })
        ));
        assert_eq!(domain.workspace.tabs[0].focused_pane, 1);
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut topology).unwrap(),
            ServerFrame::TopologyChanged {
                revision: 4,
                domain
            } if matches!(
                &domain.tabs[0].layout,
                LivePaneLayout::Split { first, .. }
                    if matches!(first.as_ref(), LivePaneLayout::Pane { pane: 1 })
            )
        ));
        let Response::Domain { domain } = client
            .request(Request::BalanceTab {
                domain: domain_id.clone(),
                tab: 0,
            })
            .unwrap()
        else {
            panic!("expected domain after balancing panes");
        };
        assert!(matches!(
            domain.workspace.tabs[0].layout,
            LivePaneLayout::Split {
                ratio_millis: 500,
                ..
            }
        ));
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut topology).unwrap(),
            ServerFrame::TopologyChanged {
                revision: 5,
                domain
            } if matches!(
                domain.tabs[0].layout,
                LivePaneLayout::Split {
                    ratio_millis: 500,
                    ..
                }
            )
        ));
        let Response::Domain { domain } = client
            .request(Request::ApplyPaneLayout {
                domain: domain_id.clone(),
                tab: 0,
                preset: LivePaneLayoutPreset::Rows,
            })
            .unwrap()
        else {
            panic!("expected domain after applying row layout");
        };
        assert!(matches!(
            domain.workspace.tabs[0].layout,
            LivePaneLayout::Split {
                split_id: 1,
                direction: SplitDirection::Horizontal,
                ratio_millis: 500,
                ..
            }
        ));
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut topology).unwrap(),
            ServerFrame::TopologyChanged {
                revision: 6,
                domain
            } if matches!(
                domain.tabs[0].layout,
                LivePaneLayout::Split {
                    split_id: 1,
                    direction: SplitDirection::Horizontal,
                    ..
                }
            )
        ));
        client
            .request(Request::FocusPane {
                domain: domain_id.clone(),
                pane: 1,
            })
            .unwrap();
        client
            .request(Request::SendText {
                domain: domain_id.clone(),
                pane: 1,
                data: b"ignored-after-command".to_vec(),
            })
            .unwrap();
        assert!(matches!(
            client
                .request(Request::ListDomains)
                .unwrap(),
            Response::Domains { domains } if domains.len() == 1 && domains[0].panes == 2
        ));
        assert!(matches!(
            client
                .request(Request::ListWorkspaces {
                    domain: domain_id.clone(),
                })
                .unwrap(),
            Response::Workspaces { workspaces } if workspaces[0].active_tab == 0
        ));
        assert!(matches!(
            client
                .request(Request::ListPanes {
                    domain: domain_id.clone(),
                })
                .unwrap(),
            Response::Panes { panes } if panes.len() == 2
        ));

        // A detached streaming client does not own or terminate the PTY.
        let mut attached = UnixStream::connect(&paths.socket).unwrap();
        protocol::write_frame(
            &mut attached,
            &ClientFrame::Hello {
                version: protocol::PROTOCOL_VERSION,
                token: paths.read_token().unwrap(),
            },
        )
        .unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut attached).unwrap(),
            ServerFrame::Hello { .. }
        ));
        protocol::write_frame(
            &mut attached,
            &ClientFrame::Request {
                request: Request::Attach {
                    domain: domain_id.clone(),
                    pane: 0,
                },
            },
        )
        .unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut attached).unwrap(),
            ServerFrame::Response {
                response: Response::Attached { .. }
            }
        ));
        read_graphics_attachment_snapshot(&mut attached);
        drop(attached);
        assert!(matches!(
            client
                .request(Request::Describe {
                    domain: domain_id.clone(),
                })
                .unwrap(),
            Response::Domain { domain } if domain.workspace.tabs[0].panes.len() == 2
        ));

        // Daemon-owned closure repairs layout/focus and tells every attached
        // relay why its stream ended.
        let mut closing_attachment = UnixStream::connect(&paths.socket).unwrap();
        protocol::write_frame(
            &mut closing_attachment,
            &ClientFrame::Hello {
                version: protocol::PROTOCOL_VERSION,
                token: paths.read_token().unwrap(),
            },
        )
        .unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut closing_attachment).unwrap(),
            ServerFrame::Hello { .. }
        ));
        protocol::write_frame(
            &mut closing_attachment,
            &ClientFrame::Request {
                request: Request::Attach {
                    domain: domain_id.clone(),
                    pane: 1,
                },
            },
        )
        .unwrap();
        let ServerFrame::Response {
            response: Response::Attached { sequence, .. },
        } = protocol::read_frame::<ServerFrame>(&mut closing_attachment).unwrap()
        else {
            panic!("expected closing attachment snapshot");
        };
        read_graphics_attachment_snapshot(&mut closing_attachment);
        // Make outstanding PTY output deterministic: SendText acknowledges
        // queueing input, not draining its echo to every attachment.
        const CLOSURE_MARKER: &str = "closure-output-barrier";
        client
            .request(Request::SendText {
                domain: domain_id.clone(),
                pane: 1,
                data: CLOSURE_MARKER.as_bytes().to_vec(),
            })
            .unwrap();
        assert!(wait_for(Duration::from_secs(2), || {
            matches!(
                client.request(Request::GetText {
                    domain: domain_id.clone(),
                    pane: 1,
                    max_bytes: 4096,
                }).unwrap(),
                Response::Text { text, .. } if text.contains(CLOSURE_MARKER)
            )
        }));
        let Response::Domain { domain } = client
            .request(Request::ClosePane {
                domain: domain_id.clone(),
                pane: 1,
            })
            .unwrap()
        else {
            panic!("expected domain after closing pane");
        };
        assert_eq!(domain.workspace.tabs[0].focused_pane, 0);
        assert_eq!(domain.workspace.tabs[0].panes.len(), 1);
        assert_eq!(
            domain.workspace.tabs[0].layout,
            LivePaneLayout::Pane { pane: 0 }
        );
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut topology).unwrap(),
            ServerFrame::TopologyChanged {
                revision: 7,
                domain
            } if domain.tabs[0].panes.len() == 1
        ));
        // Close is ordered after already-published output, not a replacement
        // for it. Check that contract rather than assuming the queue is empty.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut next_sequence = sequence.wrapping_add(1);
        let mut pending_output = Vec::new();
        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .expect("attachment did not close before the deadline");
            closing_attachment
                .set_read_timeout(Some(remaining))
                .unwrap();
            match protocol::read_frame::<ServerFrame>(&mut closing_attachment).unwrap() {
                ServerFrame::Output { sequence, data } => {
                    assert_eq!(sequence, next_sequence, "output must remain ordered");
                    next_sequence = next_sequence.wrapping_add(1);
                    assert!(pending_output.len() + data.len() <= 4096);
                    pending_output.extend(data);
                }
                ServerFrame::Closed { exit_message } => {
                    assert_eq!(exit_message, "pane was closed by a domain control request");
                    break;
                }
                frame => panic!("unexpected closing attachment frame: {frame:?}"),
            }
        }
        assert!(
            pending_output
                .windows(CLOSURE_MARKER.len())
                .any(|bytes| bytes == CLOSURE_MARKER.as_bytes())
        );
        closing_attachment
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        assert_eq!(closing_attachment.read(&mut [0; 1]).unwrap(), 0);
        for request in [
            Request::SwapPaneNext {
                domain: domain_id.clone(),
                pane: 0,
            },
            Request::BalanceTab {
                domain: domain_id.clone(),
                tab: 0,
            },
            Request::ApplyPaneLayout {
                domain: domain_id.clone(),
                tab: 0,
                preset: LivePaneLayoutPreset::Grid,
            },
        ] {
            assert!(
                client
                    .request(request)
                    .unwrap_err()
                    .to_string()
                    .contains("at least two panes")
            );
        }
        assert!(
            client
                .request(Request::ClosePane {
                    domain: domain_id.clone(),
                    pane: 0,
                })
                .unwrap_err()
                .to_string()
                .contains("use close-tab")
        );

        let Response::Domain { domain } = client
            .request(Request::SpawnTab {
                domain: domain_id.clone(),
                spawn: SpawnSpec {
                    program: Some("/bin/sh".into()),
                    args: vec![
                        "-c".into(),
                        "printf '\\033]2;runtime title\\007'; sleep 5".into(),
                    ],
                    title: Some("temporary".into()),
                    ..SpawnSpec::default()
                },
            })
            .unwrap()
        else {
            panic!("expected domain after spawning tab");
        };
        assert_eq!(domain.workspace.active_tab, 1);
        assert_eq!(domain.workspace.tabs.len(), 2);
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut topology).unwrap(),
            ServerFrame::TopologyChanged {
                revision: 8,
                domain
            } if domain.active_tab == 1 && domain.tabs.len() == 2
        ));
        let Response::Domain { domain } = client
            .request(Request::SplitPane {
                domain: domain_id.clone(),
                pane: 0,
                direction: SplitDirection::Vertical,
                spawn: SpawnSpec {
                    program: Some("/bin/sh".into()),
                    args: vec!["-c".into(), "sleep 30".into()],
                    ..SpawnSpec::default()
                },
            })
            .unwrap()
        else {
            panic!("expected source tab after preparing a pane move");
        };
        assert_eq!(domain.workspace.active_tab, 0);
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut topology).unwrap(),
            ServerFrame::TopologyChanged {
                revision: 9,
                domain
            } if domain.active_tab == 0 && domain.tabs[0].panes.len() == 2
        ));
        let mut moving_attachment = UnixStream::connect(&paths.socket).unwrap();
        protocol::write_frame(
            &mut moving_attachment,
            &ClientFrame::Hello {
                version: protocol::PROTOCOL_VERSION,
                token: paths.read_token().unwrap(),
            },
        )
        .unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut moving_attachment).unwrap(),
            ServerFrame::Hello { .. }
        ));
        protocol::write_frame(
            &mut moving_attachment,
            &ClientFrame::Request {
                request: Request::Attach {
                    domain: domain_id.clone(),
                    pane: 3,
                },
            },
        )
        .unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut moving_attachment).unwrap(),
            ServerFrame::Response {
                response: Response::Attached { .. }
            }
        ));
        read_graphics_attachment_snapshot(&mut moving_attachment);
        assert!(
            client
                .request(Request::MovePane {
                    domain: domain_id.clone(),
                    pane: 3,
                    tab: 1,
                    target: Some(999),
                    direction: SplitDirection::Horizontal,
                })
                .unwrap_err()
                .to_string()
                .contains("anchor pane")
        );
        let Response::Domain { domain } = client
            .request(Request::MovePane {
                domain: domain_id.clone(),
                pane: 3,
                tab: 1,
                target: Some(2),
                direction: SplitDirection::Horizontal,
            })
            .unwrap()
        else {
            panic!("expected domain after moving a pane between tabs");
        };
        assert_eq!(domain.workspace.active_tab, 1);
        assert_eq!(domain.workspace.tabs[0].focused_pane, 0);
        assert_eq!(
            domain.workspace.tabs[0].layout,
            LivePaneLayout::Pane { pane: 0 }
        );
        assert_eq!(domain.workspace.tabs[1].focused_pane, 3);
        assert!(matches!(
            &domain.workspace.tabs[1].layout,
            LivePaneLayout::Split {
                split_id: 3,
                direction: SplitDirection::Horizontal,
                first,
                second,
                ..
            } if matches!(first.as_ref(), LivePaneLayout::Pane { pane: 2 })
                && matches!(second.as_ref(), LivePaneLayout::Pane { pane: 3 })
        ));
        assert_eq!(
            domain.workspace.tabs[1]
                .panes
                .iter()
                .find(|pane| pane.id == 3)
                .unwrap()
                .attachments,
            1,
            "the daemon must transfer the existing pane runtime and relays"
        );
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut topology).unwrap(),
            ServerFrame::TopologyChanged {
                revision: 10,
                domain
            } if domain.active_tab == 1
                && domain.tabs[0].panes.len() == 1
                && domain.tabs[1].panes.len() == 2
        ));
        for request in [
            Request::MovePane {
                domain: domain_id.clone(),
                pane: 0,
                tab: 1,
                target: None,
                direction: SplitDirection::Vertical,
            },
            Request::MovePane {
                domain: domain_id.clone(),
                pane: 3,
                tab: 1,
                target: None,
                direction: SplitDirection::Vertical,
            },
            Request::MovePane {
                domain: domain_id.clone(),
                pane: 3,
                tab: 999,
                target: None,
                direction: SplitDirection::Vertical,
            },
        ] {
            assert!(client.request(request).is_err());
        }
        assert!(wait_for(Duration::from_secs(2), || {
            matches!(
                client.request(Request::Describe {
                    domain: domain_id.clone(),
                }),
                Ok(Response::Domain { domain })
                    if domain.workspace.tabs[1].title == "temporary"
                        && domain.workspace.tabs[1].panes[0].title == "runtime title"
            )
        }));
        let Response::Domain { domain } = client
            .request(Request::RenameTab {
                domain: domain_id.clone(),
                tab: 1,
                title: Some("renamed λ".into()),
            })
            .unwrap()
        else {
            panic!("expected domain after renaming tab");
        };
        assert_eq!(domain.workspace.tabs[1].title, "renamed λ");
        assert_eq!(
            domain.workspace.tabs[1].user_title.as_deref(),
            Some("renamed λ")
        );
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut topology).unwrap(),
            ServerFrame::TopologyChanged {
                revision: 11,
                domain
            } if domain.tabs[1].title == "renamed λ"
                && domain.tabs[1].user_title.as_deref() == Some("renamed λ")
        ));
        assert!(
            client
                .request(Request::RenameTab {
                    domain: domain_id.clone(),
                    tab: 1,
                    title: Some("two\nlines".into()),
                })
                .unwrap_err()
                .to_string()
                .contains("1 to 128")
        );
        let Response::Domain { domain } = client
            .request(Request::SetTabPinned {
                domain: domain_id.clone(),
                tab: 1,
                pinned: true,
            })
            .unwrap()
        else {
            panic!("expected domain after pinning tab");
        };
        assert_eq!(
            domain
                .workspace
                .tabs
                .iter()
                .map(|tab| tab.id)
                .collect::<Vec<_>>(),
            vec![1, 0]
        );
        assert!(domain.workspace.tabs[0].pinned);
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut topology).unwrap(),
            ServerFrame::TopologyChanged {
                revision: 12,
                domain
            } if domain.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>() == vec![1, 0]
                && domain.tabs[0].pinned
        ));
        assert!(
            client
                .request(Request::MoveTab {
                    domain: domain_id.clone(),
                    tab: 0,
                    index: 0,
                })
                .unwrap_err()
                .to_string()
                .contains("pinned group boundary")
        );
        client
            .request(Request::SetTabPinned {
                domain: domain_id.clone(),
                tab: 1,
                pinned: false,
            })
            .unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut topology).unwrap(),
            ServerFrame::TopologyChanged {
                revision: 13,
                domain
            } if domain.tabs.iter().all(|tab| !tab.pinned)
        ));
        let Response::Domain { domain } = client
            .request(Request::MoveTab {
                domain: domain_id.clone(),
                tab: 0,
                index: 0,
            })
            .unwrap()
        else {
            panic!("expected domain after moving tab");
        };
        assert_eq!(
            domain
                .workspace
                .tabs
                .iter()
                .map(|tab| tab.id)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut topology).unwrap(),
            ServerFrame::TopologyChanged {
                revision: 14,
                domain
            } if domain.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>() == vec![0, 1]
        ));
        let Response::Domain { domain } = client
            .request(Request::RenameTab {
                domain: domain_id.clone(),
                tab: 1,
                title: None,
            })
            .unwrap()
        else {
            panic!("expected domain after clearing tab title");
        };
        assert_eq!(domain.workspace.tabs[1].title, "sh");
        assert_eq!(domain.workspace.tabs[1].user_title, None);
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut topology).unwrap(),
            ServerFrame::TopologyChanged {
                revision: 15,
                domain
            } if domain.tabs[1].title == "sh" && domain.tabs[1].user_title.is_none()
        ));
        let Response::Domain { domain } = client
            .request(Request::CloseTab {
                domain: domain_id.clone(),
                tab: 1,
            })
            .unwrap()
        else {
            panic!("expected domain after closing tab");
        };
        assert_eq!(domain.workspace.active_tab, 0);
        assert_eq!(domain.workspace.tabs.len(), 1);
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut topology).unwrap(),
            ServerFrame::TopologyChanged {
                revision: 16,
                domain
            } if domain.active_tab == 0 && domain.tabs.len() == 1
        ));
        assert!(
            client
                .request(Request::CloseTab {
                    domain: domain_id.clone(),
                    tab: 0,
                })
                .unwrap_err()
                .to_string()
                .contains("use kill")
        );

        client
            .request(Request::KillDomain {
                domain: domain_id.clone(),
            })
            .unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut topology).unwrap(),
            ServerFrame::DomainClosed { domain, message }
                if domain == domain_id && message.contains("terminated")
        ));
        client.request(Request::Shutdown).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn shutdown_acknowledgement_is_transactional() {
        let (mut probe, _peer) = UnixStream::pair().unwrap();
        if let Err(error) = probe.write_all(&[0]) {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                eprintln!("skip domain shutdown exchange: sandbox denies AF_UNIX I/O");
                return;
            }
            panic!("could not probe Unix-domain socket I/O: {error}");
        }
        let empty_state = || {
            Arc::new(Mutex::new(ServerState {
                domains: HashMap::new(),
                allow_screen_checksum_reports: false,
                xterm_keyboard_type: XtermKeyboardType::Normal,
                shutdown: Arc::new(AtomicBool::new(false)),
                wake_socket: None,
                next_attachment: AtomicU64::new(1),
                next_topology_subscription: AtomicU64::new(1),
                active_uploads: 0,
                upload_ids_in_flight: HashMap::new(),
                upload_records: HashMap::new(),
                upload_janitor_running: false,
            }))
        };

        let state = empty_state();
        let (server, mut client) = UnixStream::pair().unwrap();
        serve_shutdown(server, Arc::clone(&state)).unwrap();
        assert!(matches!(
            protocol::read_frame::<ServerFrame>(&mut client).unwrap(),
            ServerFrame::Response {
                response: Response::Ok { message }
            } if message == "domain server stopping"
        ));
        assert!(state.lock().unwrap().shutdown.load(Ordering::Acquire));

        let state = empty_state();
        let (server, client) = UnixStream::pair().unwrap();
        client.shutdown(std::net::Shutdown::Both).unwrap();
        drop(client);
        assert!(serve_shutdown(server, Arc::clone(&state)).is_err());
        assert!(!state.lock().unwrap().shutdown.load(Ordering::Acquire));
    }

    #[test]
    fn attachment_queue_failure_disconnects_even_when_the_close_frame_cannot_fit() {
        for disconnected in [false, true] {
            let (sender, receiver) = std::sync::mpsc::sync_channel(1);
            let _receiver = (!disconnected).then_some(receiver);
            let (transport, mut client) = UnixStream::pair().unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let attachment = Attachment {
                id: 1,
                sender,
                transport,
            };
            if !disconnected {
                assert!(attachment.send_or_disconnect(ServerFrame::Output {
                    sequence: 1,
                    data: b"queued".to_vec(),
                }));
            }
            assert!(!attachment.send_or_disconnect(ServerFrame::Closed {
                exit_message: "fixture close".into(),
            }));
            assert_eq!(client.read(&mut [0; 1]).unwrap(), 0);
        }
    }

    #[test]
    fn slow_subscriber_is_evicted_while_high_output_stays_memory_bounded() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let shutdown = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(ServerState {
            domains: HashMap::new(),
            allow_screen_checksum_reports: false,
            xterm_keyboard_type: XtermKeyboardType::Normal,
            shutdown,
            wake_socket: None,
            next_attachment: AtomicU64::new(1),
            next_topology_subscription: AtomicU64::new(1),
            active_uploads: 0,
            upload_ids_in_flight: HashMap::new(),
            upload_records: HashMap::new(),
            upload_janitor_running: false,
        }));
        let Response::Domain { domain } = create_domain(
            &state,
            "load".into(),
            SpawnSpec {
                program: Some("/bin/sh".into()),
                args: vec!["-c".into(), "sleep 5".into()],
                ..SpawnSpec::default()
            },
        )
        .unwrap() else {
            panic!("expected created domain");
        };
        let (sender, _receiver) = std::sync::mpsc::sync_channel(1);
        let (transport, mut evicted_client) = UnixStream::pair().unwrap();
        evicted_client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let (topology_sender, topology_receiver) = std::sync::mpsc::sync_channel(1);
        let (topology_transport, _topology_client) = UnixStream::pair().unwrap();
        {
            let mut guard = state.lock().unwrap();
            find_pane_mut(&mut guard, &domain.id, 0)
                .unwrap()
                .attachments
                .push(Attachment {
                    id: 1,
                    sender,
                    transport,
                });
            let runtime = guard.domains.get_mut(&domain.id).unwrap();
            runtime.topology_subscribers.push(TopologySubscriber {
                id: 1,
                sender: topology_sender,
                transport: topology_transport,
            });
            runtime.publish_topology();
            assert_eq!(runtime.topology_revision, 2);
            assert_eq!(runtime.topology_subscribers.len(), 1);
            runtime.publish_topology();
            assert_eq!(runtime.topology_revision, 3);
            assert!(runtime.topology_subscribers.is_empty());
        }
        assert!(matches!(
            topology_receiver.try_recv().unwrap(),
            ServerFrame::TopologyChanged { revision: 2, .. }
        ));
        publish_output(&state, &domain.id, 0, b"first\r\n");
        publish_output(&state, &domain.id, 0, b"second\r\n");
        assert_eq!(
            evicted_client.read(&mut [0; 1]).unwrap(),
            0,
            "eviction must close the transport without draining its output queue"
        );
        assert_eq!(
            find_pane(&state.lock().unwrap(), &domain.id, 0)
                .unwrap()
                .attachments
                .len(),
            0
        );

        // Model input already read by the evicted reader: its pane still
        // exists, but that attachment must no longer have mutation authority.
        let (input_sender, input_receiver) = std::sync::mpsc::sync_channel(4);
        let (original_input, original_size) = {
            let mut guard = state.lock().unwrap();
            let pane = find_pane_mut(&mut guard, &domain.id, 0).unwrap();
            (
                std::mem::replace(&mut pane.input, input_sender),
                (pane.cols, pane.rows),
            )
        };
        let (reader, mut client) = UnixStream::pair().unwrap();
        for frame in [
            ClientFrame::Resize {
                cols: 91,
                rows: 37,
                pixel_width: 910,
                pixel_height: 740,
            },
            ClientFrame::Input {
                data: b"stale-input".to_vec(),
            },
            ClientFrame::Detach,
        ] {
            protocol::write_frame(&mut client, &frame).unwrap();
        }
        attachment_input_loop(reader, &state, &domain.id, 0, 1);
        assert!(
            matches!(
                input_receiver.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ),
            "evicted attachment queued input"
        );
        {
            let mut guard = state.lock().unwrap();
            let pane = find_pane_mut(&mut guard, &domain.id, 0).unwrap();
            assert_eq!(
                (pane.cols, pane.rows),
                original_size,
                "evicted attachment resized pane"
            );
        }

        // A replacement's presence must not revive the old reader. Conversely,
        // a live reader for the exact new ID retains input and resize authority.
        let (sender, _replacement_receiver) = std::sync::mpsc::sync_channel(4);
        let (transport, _replacement_client) = UnixStream::pair().unwrap();
        find_pane_mut(&mut state.lock().unwrap(), &domain.id, 0)
            .unwrap()
            .attachments
            .push(Attachment {
                id: 2,
                sender,
                transport,
            });
        for (reader_id, closed, accepted) in [(1, false, false), (2, true, false), (2, false, true)]
        {
            find_pane_mut(&mut state.lock().unwrap(), &domain.id, 0)
                .unwrap()
                .closed = closed;
            let (reader, mut client) = UnixStream::pair().unwrap();
            for frame in [
                ClientFrame::Resize {
                    cols: 91,
                    rows: 37,
                    pixel_width: 910,
                    pixel_height: 740,
                },
                ClientFrame::Input {
                    data: b"replacement-input".to_vec(),
                },
                ClientFrame::Detach,
            ] {
                protocol::write_frame(&mut client, &frame).unwrap();
            }
            attachment_input_loop(reader, &state, &domain.id, 0, reader_id);
            let guard = state.lock().unwrap();
            let pane = find_pane(&guard, &domain.id, 0).unwrap();
            if accepted {
                assert_eq!(input_receiver.try_recv().unwrap(), b"replacement-input");
                assert_eq!(
                    (pane.cols, pane.rows, pane.pixel_width, pane.pixel_height),
                    (91, 37, 910, 740)
                );
            } else {
                assert!(matches!(
                    input_receiver.try_recv(),
                    Err(std::sync::mpsc::TryRecvError::Empty)
                ));
                assert_eq!((pane.cols, pane.rows), original_size);
            }
        }
        find_pane_mut(&mut state.lock().unwrap(), &domain.id, 0)
            .unwrap()
            .input = original_input;

        let mut chunk = vec![b'x'; 4094];
        chunk.extend_from_slice(b"\r\n");
        for _ in 0..2_100 {
            publish_output(&state, &domain.id, 0, &chunk);
        }
        let guard = state.lock().unwrap();
        let pane = find_pane(&guard, &domain.id, 0).unwrap();
        assert!(
            pane.terminal.memory_usage().scrollback_bytes <= MAX_PANE_SCROLLBACK_BYTES,
            "domain scrollback exceeded its configured byte budget"
        );
    }

    #[test]
    fn exited_nonfinal_tab_is_pruned_and_published_to_topology_subscribers() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let shutdown = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(ServerState {
            domains: HashMap::new(),
            allow_screen_checksum_reports: false,
            xterm_keyboard_type: XtermKeyboardType::Normal,
            shutdown,
            wake_socket: None,
            next_attachment: AtomicU64::new(1),
            next_topology_subscription: AtomicU64::new(1),
            active_uploads: 0,
            upload_ids_in_flight: HashMap::new(),
            upload_records: HashMap::new(),
            upload_janitor_running: false,
        }));
        let Response::Domain { domain } = create_domain(
            &state,
            "exit-cleanup".into(),
            SpawnSpec {
                program: Some("/bin/sh".into()),
                args: vec!["-c".into(), "sleep 5".into()],
                ..SpawnSpec::default()
            },
        )
        .unwrap() else {
            panic!("expected created domain");
        };
        spawn_tab(
            &state,
            &domain.id,
            SpawnSpec {
                program: Some("/bin/sh".into()),
                args: vec!["-c".into(), "sleep 5".into()],
                ..SpawnSpec::default()
            },
        )
        .unwrap();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let (transport, _topology_client) = UnixStream::pair().unwrap();
        {
            let mut state = state.lock().unwrap();
            state
                .domains
                .get_mut(&domain.id)
                .unwrap()
                .topology_subscribers
                .push(TopologySubscriber {
                    id: 1,
                    sender,
                    transport,
                });
        }

        close_pane(&state, &domain.id, 1, "terminal process exited");
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            ServerFrame::TopologyChanged {
                revision: 3,
                domain: snapshot
            } if snapshot.tabs.len() == 1
                && snapshot.tabs[0].id == 0
                && snapshot.tabs[0].panes.len() == 1
        ));
        process_request(&state, Request::KillDomain { domain: domain.id }).unwrap();
    }
}
