//! Consent-gated Kitty OSC 5113 downloads onto the terminal host.

use super::{App, AppEvent, EVENT_LOOP_PROXY};
use crate::tabs::{TabType, TerminalEventOrigin, TerminalIdentity};
use crate::ui::info_overlay::InfoOverlayChoice;
use std::collections::HashMap;
use std::path::PathBuf;
#[cfg(unix)]
use std::sync::mpsc::Receiver;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{SyncSender, TrySendError, sync_channel},
};
use std::time::{Duration, Instant};
use termeric_core::{
    KittyFileTransferAction, KittyFileTransferCommand, KittyFileTransferReply,
    KittyFileTransferStatus, KittyFileTransferStatusCode, KittyFileTransferType,
    KittyFileTransmissionType, Terminal,
};

pub(super) const MAX_ACTIVE_KITTY_DOWNLOADS: usize = 4;
pub(super) const MAX_KITTY_DOWNLOAD_FILES: usize = 1_024;
pub(super) const MAX_KITTY_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const MAX_KITTY_UPLOAD_PATHS: usize = 16;
pub(super) const MAX_KITTY_UPLOAD_BYTES: u64 = 64 * 1024 * 1024;
#[cfg(unix)]
const WORKER_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
#[cfg(unix)]
const WORKER_QUEUE_CAPACITY: usize = 1_024;
#[cfg(target_os = "linux")]
const UPLOAD_BATCH_CHUNKS: usize = 8;
const PENDING_UPLOAD_TIMEOUT: Duration = Duration::from_secs(60);

#[cfg(all(test, target_os = "linux"))]
mod routing_tests;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct TransferKey {
    pub(super) origin: TerminalIdentity,
    pub(super) id: String,
}

#[derive(Debug)]
pub(super) struct PendingConsent {
    pub(super) request_id: u64,
    token: u64,
    pub(super) key: TransferKey,
    pub(super) reply_uses_8bit_controls: bool,
}

#[derive(Debug)]
pub(super) struct PendingUpload {
    request_id: Option<u64>,
    token: u64,
    key: TransferKey,
    expected_paths: usize,
    paths: Vec<UploadPath>,
    reply_uses_8bit_controls: bool,
    created_at: Instant,
}

#[derive(Clone)]
struct UploadPath {
    file_id: String,
    requested_name: String,
    path: PathBuf,
}

impl std::fmt::Debug for UploadPath {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UploadPath")
            .field("file_id", &self.file_id)
            .field("requested_name_bytes", &self.requested_name.len())
            .field(
                "resolved_path_bytes",
                &self.path.as_os_str().as_encoded_bytes().len(),
            )
            .finish()
    }
}

#[derive(Debug)]
pub(super) struct ActiveDownload {
    key: TransferKey,
    sender: SyncSender<WorkerCommand>,
    cancellation: Arc<AtomicBool>,
    progress_toast_id: u64,
}

#[derive(Debug)]
pub(super) struct ActiveUpload {
    key: TransferKey,
    sender: SyncSender<UploadWorkerCommand>,
    cancellation: Arc<AtomicBool>,
    progress_toast_id: u64,
    files_sent: usize,
    total_files: usize,
    bytes_sent: u64,
}

#[derive(Debug, Default)]
pub(super) struct KittyFileTransferManager {
    next_token: u64,
    pub(super) pending: Option<PendingConsent>,
    pub(super) active: HashMap<u64, ActiveDownload>,
    tokens: HashMap<TransferKey, u64>,
    pub(super) pending_upload: Option<PendingUpload>,
    pub(super) active_uploads: HashMap<u64, ActiveUpload>,
    upload_tokens: HashMap<TransferKey, u64>,
}

#[derive(Debug)]
struct WorkerCommand {
    command: KittyFileTransferCommand,
}

#[derive(Debug)]
struct UploadWorkerCommand {
    command: KittyFileTransferCommand,
}

pub(crate) struct UploadWorkerCompletion {
    frames: Vec<Vec<u8>>,
    terminal: bool,
    files: usize,
    total_files: usize,
    bytes: u64,
    error: Option<String>,
    reply_uses_8bit_controls: bool,
    acknowledge: Option<SyncSender<bool>>,
}

impl std::fmt::Debug for UploadWorkerCompletion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UploadWorkerCompletion")
            .field("frame_count", &self.frames.len())
            .field(
                "encoded_frame_bytes",
                &self.frames.iter().map(Vec::len).sum::<usize>(),
            )
            .field("terminal", &self.terminal)
            .field("files", &self.files)
            .field("total_files", &self.total_files)
            .field("bytes", &self.bytes)
            .field("error", &self.error)
            .field("reply_uses_8bit_controls", &self.reply_uses_8bit_controls)
            .field("acknowledge_present", &self.acknowledge.is_some())
            .finish()
    }
}

#[derive(Debug)]
pub(crate) struct WorkerCompletion {
    reply_uses_8bit_controls: bool,
    disposition: WorkerDisposition,
}

#[derive(Debug)]
enum WorkerDisposition {
    Status {
        status: KittyFileTransferStatus,
        terminal: bool,
        files: usize,
        bytes: u64,
    },
    Finished {
        files: usize,
        bytes: u64,
    },
}

impl App {
    pub(super) fn handle_kitty_file_transfer(
        &mut self,
        origin: TerminalEventOrigin,
        command: KittyFileTransferCommand,
    ) {
        // Event routes are mode-specific. Consent, session lookup, and worker
        // callbacks must retain the owner, never the current tab/pane mode.
        let Some(origin) = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.identity_for_origin(origin))
        else {
            return;
        };
        if command.action == KittyFileTransferAction::Receive {
            self.request_kitty_upload(origin, command);
            return;
        }
        if command.action == KittyFileTransferAction::Send {
            self.request_kitty_download(origin, command);
            return;
        }

        let key = TransferKey {
            origin,
            id: command.id.clone(),
        };
        if command.action == KittyFileTransferAction::Cancel {
            if self.cancel_kitty_upload(&key, command.reply_uses_8bit_controls, true) {
                return;
            }
            self.cancel_kitty_download(&key, command.reply_uses_8bit_controls, true);
            return;
        }

        if self
            .kitty_file_transfers
            .pending_upload
            .as_ref()
            .is_some_and(|pending| pending.key == key)
        {
            self.collect_kitty_upload_path(command);
            return;
        }

        if let Some(token) = self.kitty_file_transfers.upload_tokens.get(&key).copied() {
            self.route_kitty_upload_command(token, command);
            return;
        }

        if self
            .kitty_file_transfers
            .pending
            .as_ref()
            .is_some_and(|pending| pending.key == key)
        {
            self.deny_pending_kitty_download(
                Some(KittyFileTransferStatusCode::Invalid),
                "data arrived before transfer approval",
            );
            return;
        }

        let Some(token) = self.kitty_file_transfers.tokens.get(&key).copied() else {
            self.write_kitty_status(
                origin,
                command.reply_uses_8bit_controls,
                transfer_status(
                    &command.id,
                    &command.file_id,
                    KittyFileTransferStatusCode::Invalid,
                    "unknown or inactive transfer",
                ),
            );
            return;
        };

        if command.quiet != 0
            || command.bypass_present
            || command.transmission_type != KittyFileTransmissionType::Simple
            || !matches!(
                command.action,
                KittyFileTransferAction::File
                    | KittyFileTransferAction::Data
                    | KittyFileTransferAction::EndData
                    | KittyFileTransferAction::Finish
            )
        {
            self.fail_active_kitty_download(
                token,
                command.reply_uses_8bit_controls,
                KittyFileTransferStatusCode::Invalid,
                &command.file_id,
                "unsupported transfer command, quiet level, bypass, or mode",
            );
            return;
        }

        let reply_uses_8bit_controls = command.reply_uses_8bit_controls;
        let file_id = command.file_id.clone();
        let send_result = match self
            .kitty_file_transfers
            .active
            .get(&token)
            .expect("token map and active transfer stay synchronized")
            .sender
            .try_send(WorkerCommand { command })
        {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err("transfer worker queue is full"),
            Err(TrySendError::Disconnected(_)) => Err("transfer worker stopped"),
        };
        if let Err(error) = send_result {
            self.fail_active_kitty_download(
                token,
                reply_uses_8bit_controls,
                KittyFileTransferStatusCode::Busy,
                &file_id,
                error,
            );
        }
    }

    fn request_kitty_upload(
        &mut self,
        origin: TerminalIdentity,
        command: KittyFileTransferCommand,
    ) {
        let key = TransferKey {
            origin,
            id: command.id.clone(),
        };
        let expected_paths = command
            .size
            .and_then(|size| usize::try_from(size).ok())
            .filter(|size| (1..=MAX_KITTY_UPLOAD_PATHS).contains(size));
        let unsupported = if command.quiet != 0 {
            Some("quiet transfer modes are not supported")
        } else if command.bypass_present {
            Some("confirmation bypass credentials are not supported")
        } else if command.transmission_type != KittyFileTransmissionType::Simple {
            Some("rsync transfer mode is not supported")
        } else if expected_paths.is_none() {
            Some("receive must declare between 1 and 16 paths")
        } else if command.compression != termeric_core::KittyFileTransferCompression::None
            || command.file_type != KittyFileTransferType::Regular
            || command.mtime_ns.is_some()
            || command.permissions.is_some()
            || command.status.is_some()
            || command.parent.is_some()
            || command.name.is_some()
            || !command.file_id.is_empty()
            || !command.data.is_empty()
        {
            Some("the initial receive command contains unexpected file fields")
        } else {
            None
        };
        if let Some(reason) = unsupported {
            self.write_kitty_status(
                origin,
                command.reply_uses_8bit_controls,
                transfer_status(
                    &command.id,
                    "",
                    KittyFileTransferStatusCode::Invalid,
                    reason,
                ),
            );
            return;
        }
        if self.kitty_file_transfers.tokens.contains_key(&key)
            || self.kitty_file_transfers.upload_tokens.contains_key(&key)
            || self
                .kitty_file_transfers
                .pending
                .as_ref()
                .is_some_and(|pending| pending.key == key)
            || self
                .kitty_file_transfers
                .pending_upload
                .as_ref()
                .is_some_and(|pending| pending.key == key)
        {
            self.write_kitty_status(
                origin,
                command.reply_uses_8bit_controls,
                transfer_status(
                    &command.id,
                    "",
                    KittyFileTransferStatusCode::Busy,
                    "transfer id is already active",
                ),
            );
            return;
        }
        if self.kitty_file_transfers.pending.is_some()
            || self.kitty_file_transfers.pending_upload.is_some()
            || self.kitty_file_transfers.active.len()
                + self.kitty_file_transfers.active_uploads.len()
                >= MAX_ACTIVE_KITTY_DOWNLOADS
            || self.info_overlay.is_active()
        {
            self.write_kitty_status(
                origin,
                command.reply_uses_8bit_controls,
                transfer_status(
                    &command.id,
                    "",
                    KittyFileTransferStatusCode::Busy,
                    "another confirmation or too many transfers are active",
                ),
            );
            return;
        }
        if !self.kitty_download_origin_allowed(origin) {
            self.write_kitty_status(
                origin,
                command.reply_uses_8bit_controls,
                transfer_status(
                    &command.id,
                    "",
                    KittyFileTransferStatusCode::PermissionDenied,
                    "this terminal type cannot read host files",
                ),
            );
            return;
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.write_kitty_status(
                origin,
                command.reply_uses_8bit_controls,
                transfer_status(
                    &command.id,
                    "",
                    KittyFileTransferStatusCode::PermissionDenied,
                    "host file reads are currently supported only on Linux",
                ),
            );
            return;
        }

        self.kitty_file_transfers.next_token = self
            .kitty_file_transfers
            .next_token
            .checked_add(1)
            .unwrap_or(1);
        self.kitty_file_transfers.pending_upload = Some(PendingUpload {
            request_id: None,
            token: self.kitty_file_transfers.next_token,
            key,
            expected_paths: expected_paths.expect("validated path count"),
            paths: Vec::with_capacity(expected_paths.expect("validated path count")),
            reply_uses_8bit_controls: command.reply_uses_8bit_controls,
            created_at: Instant::now(),
        });
    }

    fn collect_kitty_upload_path(&mut self, command: KittyFileTransferCommand) {
        let Some(pending) = self.kitty_file_transfers.pending_upload.as_mut() else {
            return;
        };
        let valid = pending.request_id.is_none()
            && command.action == KittyFileTransferAction::File
            && command.quiet == 0
            && !command.bypass_present
            && command.transmission_type == KittyFileTransmissionType::Simple
            && command.compression == termeric_core::KittyFileTransferCompression::None
            && command.file_type == KittyFileTransferType::Regular
            && !command.file_id.is_empty()
            && command.name.is_some()
            && command.mtime_ns.is_none()
            && command.permissions.is_none()
            && command.size.is_none()
            && command.status.is_none()
            && command.parent.is_none()
            && command.data.is_empty();
        if !valid {
            self.deny_pending_kitty_upload(
                Some(KittyFileTransferStatusCode::Invalid),
                "invalid path specification or data arrived before approval",
            );
            return;
        }
        if pending
            .paths
            .iter()
            .any(|path| path.file_id == command.file_id)
        {
            self.deny_pending_kitty_upload(
                Some(KittyFileTransferStatusCode::Invalid),
                "duplicate file id",
            );
            return;
        }
        let requested_name = command.name.expect("validated name");
        let path = match resolve_kitty_upload_path(&requested_name) {
            Ok(path) => path,
            Err(error) => {
                self.deny_pending_kitty_upload(Some(KittyFileTransferStatusCode::Invalid), &error);
                return;
            }
        };
        if pending.paths.iter().any(|existing| existing.path == path) {
            self.deny_pending_kitty_upload(
                Some(KittyFileTransferStatusCode::Invalid),
                "duplicate requested path",
            );
            return;
        }
        pending.paths.push(UploadPath {
            file_id: command.file_id,
            requested_name,
            path,
        });
        if pending.paths.len() == pending.expected_paths {
            self.show_kitty_upload_consent();
        }
    }

    fn show_kitty_upload_consent(&mut self) {
        if self.info_overlay.is_active() {
            self.deny_pending_kitty_upload(
                Some(KittyFileTransferStatusCode::Busy),
                "another information dialog appeared before path collection completed",
            );
            return;
        }
        let Some(pending) = self.kitty_file_transfers.pending_upload.as_ref() else {
            return;
        };
        let mut lines = vec![
            "The exact originating terminal wants to read files from this computer.".into(),
            String::new(),
            format!("Session: {}", pending.key.id),
            format!("Origin: {}", describe_origin(pending.key.origin)),
            format!("Limits: at most {MAX_KITTY_UPLOAD_PATHS} regular files and 64 MiB"),
            String::new(),
            "Requested paths:".into(),
        ];
        lines.extend(
            pending
                .paths
                .iter()
                .map(|path| format!("  {}", path.path.display())),
        );
        lines.extend([
            String::new(),
            "Directories, links, and non-regular files will be refused.".into(),
            "Only continue if you trust the command that requested these exact files.".into(),
            String::new(),
            "  A  Allow these reads once".into(),
            "  D  Deny".into(),
        ]);
        let allow_line = lines.len() - 2;
        let deny_line = lines.len() - 1;
        let request_id = self.info_overlay.show_choices(
            "Allow terminal to read host files?",
            lines,
            vec![
                InfoOverlayChoice::new(
                    "kitty-upload-allow",
                    "Allow these reads once",
                    allow_line,
                    Some('a'),
                ),
                InfoOverlayChoice::new("kitty-upload-deny", "Deny", deny_line, Some('d')),
            ],
        );
        if let Some(pending) = self.kitty_file_transfers.pending_upload.as_mut() {
            pending.request_id = Some(request_id);
        }
    }

    fn request_kitty_download(
        &mut self,
        origin: TerminalIdentity,
        command: KittyFileTransferCommand,
    ) {
        let key = TransferKey {
            origin,
            id: command.id.clone(),
        };
        let unsupported = if command.quiet != 0 {
            Some("quiet transfer modes are not supported")
        } else if command.bypass_present {
            Some("confirmation bypass credentials are not supported")
        } else if command.transmission_type != KittyFileTransmissionType::Simple {
            Some("rsync transfer mode is not supported")
        } else if command.compression != termeric_core::KittyFileTransferCompression::None
            || command.file_type != KittyFileTransferType::Regular
            || command.mtime_ns.is_some()
            || command.permissions.is_some()
            || command.status.is_some()
            || command.parent.is_some()
            || command.size.is_some()
            || command.name.is_some()
            || !command.file_id.is_empty()
            || !command.data.is_empty()
        {
            Some("the initial send command contains unexpected file fields")
        } else {
            None
        };
        if let Some(reason) = unsupported {
            self.write_kitty_status(
                origin,
                command.reply_uses_8bit_controls,
                transfer_status(
                    &command.id,
                    "",
                    KittyFileTransferStatusCode::Invalid,
                    reason,
                ),
            );
            return;
        }
        if self.kitty_file_transfers.tokens.contains_key(&key)
            || self.kitty_file_transfers.upload_tokens.contains_key(&key)
            || self
                .kitty_file_transfers
                .pending
                .as_ref()
                .is_some_and(|pending| pending.key == key)
            || self
                .kitty_file_transfers
                .pending_upload
                .as_ref()
                .is_some_and(|pending| pending.key == key)
        {
            self.write_kitty_status(
                origin,
                command.reply_uses_8bit_controls,
                transfer_status(
                    &command.id,
                    "",
                    KittyFileTransferStatusCode::Busy,
                    "transfer id is already active",
                ),
            );
            return;
        }
        if self.kitty_file_transfers.pending.is_some()
            || self.kitty_file_transfers.pending_upload.is_some()
            || self.kitty_file_transfers.active.len()
                + self.kitty_file_transfers.active_uploads.len()
                >= MAX_ACTIVE_KITTY_DOWNLOADS
            || self.info_overlay.is_active()
        {
            self.write_kitty_status(
                origin,
                command.reply_uses_8bit_controls,
                transfer_status(
                    &command.id,
                    "",
                    KittyFileTransferStatusCode::Busy,
                    "another confirmation or too many transfers are active",
                ),
            );
            return;
        }
        if !self.kitty_download_origin_allowed(origin) {
            self.write_kitty_status(
                origin,
                command.reply_uses_8bit_controls,
                transfer_status(
                    &command.id,
                    "",
                    KittyFileTransferStatusCode::PermissionDenied,
                    "this terminal type cannot write host files",
                ),
            );
            return;
        }

        #[cfg(not(unix))]
        {
            self.write_kitty_status(
                origin,
                command.reply_uses_8bit_controls,
                transfer_status(
                    &command.id,
                    "",
                    KittyFileTransferStatusCode::PermissionDenied,
                    "host file transfer is currently supported only on Unix",
                ),
            );
            return;
        }

        self.kitty_file_transfers.next_token = self
            .kitty_file_transfers
            .next_token
            .checked_add(1)
            .unwrap_or(1);
        let token = self.kitty_file_transfers.next_token;
        let lines = vec![
            "The exact originating terminal wants to create files on this computer.".into(),
            String::new(),
            format!("Session: {}", command.id),
            format!("Origin: {}", describe_origin(origin)),
            format!(
                "Limits: at most {MAX_KITTY_DOWNLOAD_FILES} entries and {} MiB",
                MAX_KITTY_DOWNLOAD_BYTES / (1024 * 1024)
            ),
            String::new(),
            "The remote side chooses destination paths and symbolic-link targets.".into(),
            "Existing entries will not be replaced.".into(),
            "Only continue if you trust the remote computer and the command you ran.".into(),
            String::new(),
            "  A  Allow this transfer".into(),
            "  D  Deny".into(),
        ];
        let request_id = self.info_overlay.show_choices(
            "Allow remote file transfer?",
            lines,
            vec![
                InfoOverlayChoice::new("kitty-download-allow", "Allow transfer", 10, Some('a')),
                InfoOverlayChoice::new("kitty-download-deny", "Deny transfer", 11, Some('d')),
            ],
        );
        self.kitty_file_transfers.pending = Some(PendingConsent {
            request_id,
            token,
            key,
            reply_uses_8bit_controls: command.reply_uses_8bit_controls,
        });
    }

    fn kitty_transfer_origin(&self, identity: TerminalIdentity) -> Option<TerminalEventOrigin> {
        self.tabs
            .as_ref()?
            .origin_for_pane(identity.tab_id, identity.pane_id)
    }

    fn kitty_download_origin_allowed(&self, origin: TerminalIdentity) -> bool {
        if self.kitty_transfer_origin(origin).is_none() {
            return false;
        }
        self.tabs
            .as_ref()
            .and_then(|tabs| tabs.tab_by_id(origin.tab_id))
            .is_some_and(|tab| {
                tab.live_domain.is_some()
                    || matches!(tab.tab_type, TabType::Shell | TabType::Profile { .. })
            })
    }

    pub(super) fn handle_kitty_download_choice(
        &mut self,
        request_id: u64,
        choice_id: &str,
    ) -> bool {
        let Some(pending) = self.kitty_file_transfers.pending.as_ref() else {
            return false;
        };
        if pending.request_id != request_id
            || !matches!(choice_id, "kitty-download-allow" | "kitty-download-deny")
        {
            return false;
        }
        self.info_overlay.dismiss(request_id);
        if choice_id == "kitty-download-deny" {
            self.deny_pending_kitty_download(
                Some(KittyFileTransferStatusCode::PermissionDenied),
                "user refused the transfer",
            );
            return true;
        }
        let pending = self
            .kitty_file_transfers
            .pending
            .take()
            .expect("checked pending transfer");
        if !self.kitty_download_origin_allowed(pending.key.origin) {
            self.write_kitty_status(
                pending.key.origin,
                pending.reply_uses_8bit_controls,
                transfer_status(
                    &pending.key.id,
                    "",
                    KittyFileTransferStatusCode::PermissionDenied,
                    "this terminal type can no longer write host files",
                ),
            );
            return true;
        }
        if self
            .tabs
            .as_ref()
            .and_then(|tabs| {
                tabs.origin_for_pane(pending.key.origin.tab_id, pending.key.origin.pane_id)
            })
            .is_none()
        {
            return true;
        }
        let Some(window_id) = self.window.as_ref().map(|window| window.id()) else {
            self.write_kitty_status(
                pending.key.origin,
                pending.reply_uses_8bit_controls,
                transfer_status(
                    &pending.key.id,
                    "",
                    KittyFileTransferStatusCode::PermissionDenied,
                    "a native window is required for consented transfers",
                ),
            );
            return true;
        };
        let Some(proxy) = EVENT_LOOP_PROXY.get().cloned() else {
            self.write_kitty_status(
                pending.key.origin,
                pending.reply_uses_8bit_controls,
                transfer_status(
                    &pending.key.id,
                    "",
                    KittyFileTransferStatusCode::Io,
                    "transfer event channel is unavailable",
                ),
            );
            return true;
        };
        let cancellation = Arc::new(AtomicBool::new(false));
        let worker = start_worker(
            window_id,
            pending.token,
            pending.key.id.clone(),
            pending.reply_uses_8bit_controls,
            Arc::clone(&cancellation),
            proxy,
        );
        let sender = match worker {
            Ok(sender) => sender,
            Err(error) => {
                self.write_kitty_status(
                    pending.key.origin,
                    pending.reply_uses_8bit_controls,
                    transfer_status(&pending.key.id, "", KittyFileTransferStatusCode::Io, &error),
                );
                return true;
            }
        };
        let toast = self.toasts.persistent_info(format!(
            "Receiving remote files for session {}",
            pending.key.id
        ));
        self.kitty_file_transfers
            .tokens
            .insert(pending.key.clone(), pending.token);
        self.kitty_file_transfers.active.insert(
            pending.token,
            ActiveDownload {
                key: pending.key.clone(),
                sender,
                cancellation,
                progress_toast_id: toast,
            },
        );
        if !self.write_kitty_status(
            pending.key.origin,
            pending.reply_uses_8bit_controls,
            transfer_status(&pending.key.id, "", KittyFileTransferStatusCode::Ok, ""),
        ) {
            self.remove_active_kitty_download(pending.token);
        }
        true
    }

    pub(super) fn handle_kitty_upload_choice(&mut self, request_id: u64, choice_id: &str) -> bool {
        let Some(pending) = self.kitty_file_transfers.pending_upload.as_ref() else {
            return false;
        };
        if pending.request_id != Some(request_id)
            || !matches!(choice_id, "kitty-upload-allow" | "kitty-upload-deny")
        {
            return false;
        }
        self.info_overlay.dismiss(request_id);
        if choice_id == "kitty-upload-deny" {
            self.deny_pending_kitty_upload(
                Some(KittyFileTransferStatusCode::PermissionDenied),
                "user refused the host file read",
            );
            return true;
        }
        let pending = self
            .kitty_file_transfers
            .pending_upload
            .take()
            .expect("checked pending upload");
        if !self.kitty_download_origin_allowed(pending.key.origin) {
            self.write_kitty_status(
                pending.key.origin,
                pending.reply_uses_8bit_controls,
                transfer_status(
                    &pending.key.id,
                    "",
                    KittyFileTransferStatusCode::PermissionDenied,
                    "this terminal type can no longer read host files",
                ),
            );
            return true;
        }
        if self
            .tabs
            .as_ref()
            .and_then(|tabs| {
                tabs.origin_for_pane(pending.key.origin.tab_id, pending.key.origin.pane_id)
            })
            .is_none()
        {
            return true;
        }
        let Some(window_id) = self.window.as_ref().map(|window| window.id()) else {
            self.write_kitty_status(
                pending.key.origin,
                pending.reply_uses_8bit_controls,
                transfer_status(
                    &pending.key.id,
                    "",
                    KittyFileTransferStatusCode::PermissionDenied,
                    "a native window is required for consented transfers",
                ),
            );
            return true;
        };
        let Some(proxy) = EVENT_LOOP_PROXY.get().cloned() else {
            self.write_kitty_status(
                pending.key.origin,
                pending.reply_uses_8bit_controls,
                transfer_status(
                    &pending.key.id,
                    "",
                    KittyFileTransferStatusCode::Io,
                    "transfer event channel is unavailable",
                ),
            );
            return true;
        };
        let cancellation = Arc::new(AtomicBool::new(false));
        let sender = match start_upload_worker(
            window_id,
            pending.token,
            pending.key.id.clone(),
            pending.paths,
            pending.reply_uses_8bit_controls,
            Arc::clone(&cancellation),
            proxy,
        ) {
            Ok(sender) => sender,
            Err(error) => {
                self.write_kitty_status(
                    pending.key.origin,
                    pending.reply_uses_8bit_controls,
                    transfer_status(&pending.key.id, "", KittyFileTransferStatusCode::Io, &error),
                );
                return true;
            }
        };
        let toast = self.toasts.persistent_info(format!(
            "Sending approved host files for session {}",
            pending.key.id
        ));
        self.kitty_file_transfers
            .upload_tokens
            .insert(pending.key.clone(), pending.token);
        self.kitty_file_transfers.active_uploads.insert(
            pending.token,
            ActiveUpload {
                key: pending.key.clone(),
                sender,
                cancellation,
                progress_toast_id: toast,
                files_sent: 0,
                total_files: pending.expected_paths,
                bytes_sent: 0,
            },
        );
        if !self.write_kitty_status(
            pending.key.origin,
            pending.reply_uses_8bit_controls,
            transfer_status(&pending.key.id, "", KittyFileTransferStatusCode::Ok, ""),
        ) {
            self.remove_active_kitty_upload(pending.token);
        }
        true
    }

    fn route_kitty_upload_command(&mut self, token: u64, command: KittyFileTransferCommand) {
        if command.action == KittyFileTransferAction::Finish {
            if let Some(active) = self.remove_active_kitty_upload(token) {
                let message = format!(
                    "Sent {}/{} approved host files ({} bytes)",
                    active.files_sent, active.total_files, active.bytes_sent
                );
                if active.files_sent == active.total_files {
                    self.toasts.success(message);
                } else {
                    self.toasts.warning(message);
                }
            }
            return;
        }
        if command.action == KittyFileTransferAction::Status {
            self.remove_active_kitty_upload(token);
            return;
        }
        let valid = command.action == KittyFileTransferAction::File
            && command.quiet == 0
            && !command.bypass_present
            && command.transmission_type == KittyFileTransmissionType::Simple
            && command.compression == termeric_core::KittyFileTransferCompression::None
            && command.file_type == KittyFileTransferType::Regular
            && !command.file_id.is_empty()
            && command.name.is_some()
            && command.mtime_ns.is_none()
            && command.permissions.is_none()
            && command.size.is_none()
            && command.status.is_none()
            && command.parent.is_none()
            && command.data.is_empty();
        if !valid {
            self.fail_active_kitty_upload(
                token,
                command.reply_uses_8bit_controls,
                &command.file_id,
                KittyFileTransferStatusCode::Invalid,
                "unsupported or invalid host file request",
            );
            return;
        }
        let reply_mode = command.reply_uses_8bit_controls;
        let file_id = command.file_id.clone();
        let send_result = self
            .kitty_file_transfers
            .active_uploads
            .get(&token)
            .expect("upload token and active transfer stay synchronized")
            .sender
            .try_send(UploadWorkerCommand { command });
        if let Err(error) = send_result {
            let message = match error {
                TrySendError::Full(_) => "host-read worker queue is full",
                TrySendError::Disconnected(_) => "host-read worker stopped",
            };
            self.fail_active_kitty_upload(
                token,
                reply_mode,
                &file_id,
                KittyFileTransferStatusCode::Busy,
                message,
            );
        }
    }

    fn cancel_kitty_upload(
        &mut self,
        key: &TransferKey,
        reply_uses_8bit_controls: bool,
        acknowledge: bool,
    ) -> bool {
        if self
            .kitty_file_transfers
            .pending_upload
            .as_ref()
            .is_some_and(|pending| &pending.key == key)
        {
            self.deny_pending_kitty_upload(
                acknowledge.then_some(KittyFileTransferStatusCode::Canceled),
                "",
            );
            return true;
        }
        let Some(token) = self.kitty_file_transfers.upload_tokens.get(key).copied() else {
            return false;
        };
        self.remove_active_kitty_upload(token);
        if acknowledge {
            self.write_kitty_status(
                key.origin,
                reply_uses_8bit_controls,
                transfer_status(&key.id, "", KittyFileTransferStatusCode::Canceled, ""),
            );
        }
        true
    }

    fn deny_pending_kitty_upload(
        &mut self,
        code: Option<KittyFileTransferStatusCode>,
        message: &str,
    ) {
        let Some(pending) = self.kitty_file_transfers.pending_upload.take() else {
            return;
        };
        if let Some(request_id) = pending.request_id {
            self.info_overlay.dismiss(request_id);
        }
        if let Some(code) = code {
            self.write_kitty_status(
                pending.key.origin,
                pending.reply_uses_8bit_controls,
                transfer_status(&pending.key.id, "", code, message),
            );
        }
    }

    fn fail_active_kitty_upload(
        &mut self,
        token: u64,
        reply_uses_8bit_controls: bool,
        file_id: &str,
        code: KittyFileTransferStatusCode,
        message: &str,
    ) {
        let Some(active) = self.remove_active_kitty_upload(token) else {
            return;
        };
        self.write_kitty_status(
            active.key.origin,
            reply_uses_8bit_controls,
            transfer_status(&active.key.id, file_id, code, message),
        );
        self.toasts
            .error(format!("Host file transfer failed: {message}"));
    }

    fn remove_active_kitty_upload(&mut self, token: u64) -> Option<ActiveUpload> {
        let active = self.kitty_file_transfers.active_uploads.remove(&token)?;
        active.cancellation.store(true, Ordering::Release);
        self.kitty_file_transfers.upload_tokens.remove(&active.key);
        self.toasts.dismiss(active.progress_toast_id);
        Some(active)
    }

    pub(super) fn dismiss_pending_kitty_download(&mut self, request_id: u64) -> bool {
        if self
            .kitty_file_transfers
            .pending
            .as_ref()
            .is_some_and(|pending| pending.request_id == request_id)
        {
            self.deny_pending_kitty_download(
                Some(KittyFileTransferStatusCode::PermissionDenied),
                "user dismissed the transfer confirmation",
            );
            return true;
        }
        if self
            .kitty_file_transfers
            .pending_upload
            .as_ref()
            .is_some_and(|pending| pending.request_id == Some(request_id))
        {
            self.deny_pending_kitty_upload(
                Some(KittyFileTransferStatusCode::PermissionDenied),
                "user dismissed the host file read confirmation",
            );
            return true;
        }
        false
    }

    fn deny_pending_kitty_download(
        &mut self,
        code: Option<KittyFileTransferStatusCode>,
        message: &str,
    ) {
        let Some(pending) = self.kitty_file_transfers.pending.take() else {
            return;
        };
        self.info_overlay.dismiss(pending.request_id);
        if let Some(code) = code {
            self.write_kitty_status(
                pending.key.origin,
                pending.reply_uses_8bit_controls,
                transfer_status(&pending.key.id, "", code, message),
            );
        }
    }

    fn cancel_kitty_download(
        &mut self,
        key: &TransferKey,
        reply_uses_8bit_controls: bool,
        acknowledge: bool,
    ) {
        if self
            .kitty_file_transfers
            .pending
            .as_ref()
            .is_some_and(|pending| &pending.key == key)
        {
            self.deny_pending_kitty_download(
                acknowledge.then_some(KittyFileTransferStatusCode::Canceled),
                "",
            );
            return;
        }
        let Some(token) = self.kitty_file_transfers.tokens.get(key).copied() else {
            if acknowledge {
                self.write_kitty_status(
                    key.origin,
                    reply_uses_8bit_controls,
                    transfer_status(&key.id, "", KittyFileTransferStatusCode::Canceled, ""),
                );
            }
            return;
        };
        self.remove_active_kitty_download(token);
        if acknowledge {
            self.write_kitty_status(
                key.origin,
                reply_uses_8bit_controls,
                transfer_status(&key.id, "", KittyFileTransferStatusCode::Canceled, ""),
            );
        }
    }

    fn fail_active_kitty_download(
        &mut self,
        token: u64,
        reply_uses_8bit_controls: bool,
        code: KittyFileTransferStatusCode,
        file_id: &str,
        message: &str,
    ) {
        let Some(active) = self.remove_active_kitty_download(token) else {
            return;
        };
        self.write_kitty_status(
            active.key.origin,
            reply_uses_8bit_controls,
            transfer_status(&active.key.id, file_id, code, message),
        );
        self.toasts
            .error(format!("Remote file transfer failed: {message}"));
    }

    fn remove_active_kitty_download(&mut self, token: u64) -> Option<ActiveDownload> {
        let active = self.kitty_file_transfers.active.remove(&token)?;
        active.cancellation.store(true, Ordering::Release);
        self.kitty_file_transfers.tokens.remove(&active.key);
        self.toasts.dismiss(active.progress_toast_id);
        Some(active)
    }

    pub(super) fn handle_kitty_file_transfer_completed(
        &mut self,
        token: u64,
        completion: WorkerCompletion,
    ) {
        if self
            .kitty_file_transfers
            .active
            .get(&token)
            .is_some_and(|active| {
                self.tabs
                    .as_ref()
                    .and_then(|tabs| {
                        tabs.origin_for_pane(active.key.origin.tab_id, active.key.origin.pane_id)
                    })
                    .is_none()
            })
        {
            self.remove_active_kitty_download(token);
            return;
        }
        let Some(active) = self.kitty_file_transfers.active.get(&token) else {
            return;
        };
        let origin = active.key.origin;
        let progress_toast_id = active.progress_toast_id;
        match completion.disposition {
            WorkerDisposition::Status {
                status,
                terminal,
                files,
                bytes,
            } => {
                if !terminal {
                    self.toasts.update_persistent(
                        progress_toast_id,
                        format!(
                            "Receiving remote files: {files} entries, {}",
                            format_transfer_bytes(bytes)
                        ),
                    );
                }
                let reply_written =
                    self.write_kitty_status(origin, completion.reply_uses_8bit_controls, status);
                if terminal || !reply_written {
                    self.remove_active_kitty_download(token);
                }
            }
            WorkerDisposition::Finished { files, bytes } => {
                self.remove_active_kitty_download(token);
                self.toasts.success(format!(
                    "Received {files} remote file-system entries ({bytes} bytes)"
                ));
            }
        }
    }

    pub(super) fn handle_kitty_file_upload_completed(
        &mut self,
        token: u64,
        mut completion: UploadWorkerCompletion,
    ) {
        let Some(active) = self.kitty_file_transfers.active_uploads.get(&token) else {
            if let Some(acknowledge) = completion.acknowledge.take() {
                let _ = acknowledge.try_send(false);
            }
            return;
        };
        let origin = active.key.origin;
        let progress_toast_id = active.progress_toast_id;
        let resolved_origin = self.kitty_transfer_origin(origin);
        let mut written = resolved_origin.is_some();
        for canonical in &completion.frames {
            if !written {
                break;
            }
            let reply =
                Terminal::frame_pty_reply_for_mode(canonical, completion.reply_uses_8bit_controls);
            written = self.tabs.as_mut().is_some_and(|tabs| {
                resolved_origin.is_some_and(|route| tabs.write_to_origin(route, &reply).is_ok())
            });
        }
        if let Some(acknowledge) = completion.acknowledge.take() {
            let _ = acknowledge.try_send(written);
        }
        if !written || completion.terminal {
            self.remove_active_kitty_upload(token);
            if let Some(error) = completion.error {
                self.toasts
                    .error(format!("Host file transfer failed: {error}"));
            }
            return;
        }
        if let Some(active) = self.kitty_file_transfers.active_uploads.get_mut(&token) {
            active.files_sent = completion.files;
            active.total_files = completion.total_files;
            active.bytes_sent = completion.bytes;
        }
        self.toasts.update_persistent(
            progress_toast_id,
            format!(
                "Sending approved host files: {}/{} files, {}",
                completion.files,
                completion.total_files,
                format_transfer_bytes_sent(completion.bytes)
            ),
        );
    }

    fn write_kitty_status(
        &mut self,
        identity: TerminalIdentity,
        reply_uses_8bit_controls: bool,
        status: KittyFileTransferStatus,
    ) -> bool {
        let Some(origin) = self.kitty_transfer_origin(identity) else {
            return false;
        };
        let canonical = status.canonical_reply();
        let reply = Terminal::frame_pty_reply_for_mode(&canonical, reply_uses_8bit_controls);
        if let Some(tabs) = &mut self.tabs {
            if let Err(error) = tabs.write_to_origin(origin, &reply) {
                log::debug!("Could not return Kitty file-transfer status to {origin:?}: {error}");
                return false;
            }
            return true;
        }
        false
    }

    pub(super) fn prune_stale_kitty_file_transfers(&mut self) -> bool {
        let stale_pending = self
            .kitty_file_transfers
            .pending
            .as_ref()
            .is_some_and(|pending| {
                !self.info_overlay.is_awaiting(pending.request_id)
                    || self
                        .tabs
                        .as_ref()
                        .and_then(|tabs| {
                            tabs.origin_for_pane(
                                pending.key.origin.tab_id,
                                pending.key.origin.pane_id,
                            )
                        })
                        .is_none()
            });
        if stale_pending {
            self.deny_pending_kitty_download(
                Some(KittyFileTransferStatusCode::PermissionDenied),
                "transfer confirmation is no longer available",
            );
        }

        let stale_pending_upload = self
            .kitty_file_transfers
            .pending_upload
            .as_ref()
            .is_some_and(|pending| {
                pending.created_at.elapsed() >= PENDING_UPLOAD_TIMEOUT
                    || pending
                        .request_id
                        .is_some_and(|request_id| !self.info_overlay.is_awaiting(request_id))
                    || self
                        .tabs
                        .as_ref()
                        .and_then(|tabs| {
                            tabs.origin_for_pane(
                                pending.key.origin.tab_id,
                                pending.key.origin.pane_id,
                            )
                        })
                        .is_none()
            });
        if stale_pending_upload {
            self.deny_pending_kitty_upload(
                Some(KittyFileTransferStatusCode::PermissionDenied),
                "host file read request expired or its confirmation is no longer available",
            );
        }

        let stale_tokens: Vec<_> = self
            .kitty_file_transfers
            .active
            .iter()
            .filter_map(|(token, active)| {
                self.tabs
                    .as_ref()
                    .and_then(|tabs| {
                        tabs.origin_for_pane(active.key.origin.tab_id, active.key.origin.pane_id)
                    })
                    .is_none()
                    .then_some(*token)
            })
            .collect();
        for token in &stale_tokens {
            self.remove_active_kitty_download(*token);
        }
        let stale_upload_tokens: Vec<_> = self
            .kitty_file_transfers
            .active_uploads
            .iter()
            .filter_map(|(token, active)| {
                self.tabs
                    .as_ref()
                    .and_then(|tabs| {
                        tabs.origin_for_pane(active.key.origin.tab_id, active.key.origin.pane_id)
                    })
                    .is_none()
                    .then_some(*token)
            })
            .collect();
        for token in &stale_upload_tokens {
            self.remove_active_kitty_upload(*token);
        }
        stale_pending
            || stale_pending_upload
            || !stale_tokens.is_empty()
            || !stale_upload_tokens.is_empty()
    }

    pub(super) fn shutdown_kitty_file_transfers(&mut self) {
        self.kitty_file_transfers.pending = None;
        self.kitty_file_transfers.pending_upload = None;
        let tokens: Vec<_> = self.kitty_file_transfers.active.keys().copied().collect();
        for token in tokens {
            self.remove_active_kitty_download(token);
        }
        let upload_tokens: Vec<_> = self
            .kitty_file_transfers
            .active_uploads
            .keys()
            .copied()
            .collect();
        for token in upload_tokens {
            self.remove_active_kitty_upload(token);
        }
    }
}

fn format_transfer_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB received", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB received", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} bytes received")
    }
}

fn format_transfer_bytes_sent(bytes: u64) -> String {
    format_transfer_bytes(bytes).replace("received", "sent")
}

fn describe_origin(origin: TerminalIdentity) -> String {
    format!("tab {}, pane {}", origin.tab_id, origin.pane_id.0)
}

fn transfer_status(
    id: &str,
    file_id: &str,
    code: KittyFileTransferStatusCode,
    message: &str,
) -> KittyFileTransferStatus {
    KittyFileTransferStatus {
        id: id.into(),
        file_id: file_id.into(),
        code,
        message: (!message.is_empty()).then(|| message.into()),
        name: None,
        size: None,
        transmission_type: KittyFileTransmissionType::Simple,
    }
}

fn resolve_kitty_upload_path(name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name.len() > termeric_core::MAX_KITTY_TRANSFER_PATH_BYTES {
        return Err("empty or oversized requested path".into());
    }
    if name.chars().any(|character| {
        character.is_control()
            || matches!(
                character,
                '\u{200e}'
                    | '\u{200f}'
                    | '\u{202a}'..='\u{202e}'
                    | '\u{2066}'..='\u{2069}'
            )
    }) {
        return Err("requested path contains unsafe display controls".into());
    }
    let components = name
        .strip_prefix("~/")
        .or_else(|| name.strip_prefix('/'))
        .unwrap_or(name);
    if components.is_empty()
        || components.split('/').any(|component| {
            component.is_empty()
                || matches!(component, "." | "..")
                || component.as_bytes().contains(&0)
                || component.len() > 255
        })
    {
        return Err("requested path contains empty, traversal, or oversized components".into());
    }
    let home = directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .ok_or_else(|| "home directory is unavailable".to_string())?;
    let path = if name == "~" {
        home
    } else if let Some(relative) = name.strip_prefix("~/") {
        home.join(relative)
    } else if std::path::Path::new(name).is_absolute() {
        PathBuf::from(name)
    } else {
        home.join(name)
    };
    if path.as_os_str().as_encoded_bytes().len() > termeric_core::MAX_KITTY_TRANSFER_PATH_BYTES {
        return Err("resolved requested path is too large".into());
    }
    Ok(path)
}

#[cfg(target_os = "linux")]
fn start_upload_worker(
    window_id: winit::window::WindowId,
    token: u64,
    session_id: String,
    paths: Vec<UploadPath>,
    reply_uses_8bit_controls: bool,
    cancellation: Arc<AtomicBool>,
    proxy: winit::event_loop::EventLoopProxy<AppEvent>,
) -> Result<SyncSender<UploadWorkerCommand>, String> {
    let (sender, receiver) = sync_channel::<UploadWorkerCommand>(MAX_KITTY_UPLOAD_PATHS + 2);
    std::thread::Builder::new()
        .name("termeric-kitty-upload".into())
        .spawn(move || {
            run_upload_worker(
                receiver,
                session_id,
                paths,
                reply_uses_8bit_controls,
                cancellation,
                |result| {
                    proxy
                        .send_event(AppEvent::KittyFileUploadCompleted {
                            window_id,
                            token,
                            result,
                        })
                        .is_ok()
                },
            );
        })
        .map_err(|error| format!("could not start host-read worker: {error}"))?;
    Ok(sender)
}

#[cfg(not(target_os = "linux"))]
fn start_upload_worker(
    _window_id: winit::window::WindowId,
    _token: u64,
    _session_id: String,
    _paths: Vec<UploadPath>,
    _reply_uses_8bit_controls: bool,
    _cancellation: Arc<AtomicBool>,
    _proxy: winit::event_loop::EventLoopProxy<AppEvent>,
) -> Result<SyncSender<UploadWorkerCommand>, String> {
    Err("host file reads are currently supported only on Linux".into())
}

#[cfg(target_os = "linux")]
fn run_upload_worker(
    receiver: Receiver<UploadWorkerCommand>,
    session_id: String,
    paths: Vec<UploadPath>,
    reply_uses_8bit_controls: bool,
    cancellation: Arc<AtomicBool>,
    mut emit: impl FnMut(UploadWorkerCompletion) -> bool,
) {
    use std::ffi::CString;
    use std::fs::File;
    use std::io::Read;
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;

    struct Source {
        requested_name: String,
        path_name: String,
        file: File,
        size: u64,
        sent: bool,
    }

    fn open_without_symlinks(path: &std::path::Path) -> std::io::Result<File> {
        #[repr(C)]
        struct OpenHow {
            flags: u64,
            mode: u64,
            resolve: u64,
        }

        let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "approved path contains NUL",
            )
        })?;
        let how = OpenHow {
            flags: (libc::O_RDONLY | libc::O_CLOEXEC) as u64,
            mode: 0,
            resolve: libc::RESOLVE_NO_SYMLINKS,
        };
        // SAFETY: `path` is a live NUL-terminated CString, `how` points to a
        // fully initialized `open_how` for the supplied size, and a successful
        // syscall transfers ownership of one new descriptor to this function.
        let descriptor = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                libc::AT_FDCWD,
                path.as_ptr(),
                &how,
                std::mem::size_of::<OpenHow>(),
            )
        };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `openat2` returned a new owned descriptor and it has not been
        // wrapped or closed elsewhere.
        Ok(unsafe { File::from_raw_fd(descriptor as libc::c_int) })
    }

    fn error_frame(
        session_id: &str,
        file_id: &str,
        code: KittyFileTransferStatusCode,
        message: &str,
    ) -> Vec<u8> {
        transfer_status(session_id, file_id, code, message).canonical_reply()
    }

    fn emit_with_ack(
        emit: &mut impl FnMut(UploadWorkerCompletion) -> bool,
        cancellation: &AtomicBool,
        frames: Vec<Vec<u8>>,
        terminal: bool,
        files: usize,
        total_files: usize,
        bytes: u64,
        error: Option<String>,
        reply_uses_8bit_controls: bool,
    ) -> bool {
        let (sender, receiver) = sync_channel(1);
        if !emit(UploadWorkerCompletion {
            frames,
            terminal,
            files,
            total_files,
            bytes,
            error,
            reply_uses_8bit_controls,
            acknowledge: Some(sender),
        }) {
            return false;
        }
        loop {
            if cancellation.load(Ordering::Acquire) {
                return false;
            }
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(written) => return written,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return false,
            }
        }
    }

    let total_files = paths.len();
    let mut sources = HashMap::with_capacity(total_files);
    let mut metadata_frames = Vec::with_capacity(total_files + 1);
    let mut total_bytes = 0u64;
    for path in paths {
        if cancellation.load(Ordering::Acquire) {
            return;
        }
        let file = match open_without_symlinks(&path.path) {
            Ok(file) => file,
            Err(error) => {
                let message = format!("could not open approved regular file: {error}");
                let code = if error.kind() == std::io::ErrorKind::PermissionDenied {
                    KittyFileTransferStatusCode::PermissionDenied
                } else {
                    KittyFileTransferStatusCode::Io
                };
                let _ = emit_with_ack(
                    &mut emit,
                    &cancellation,
                    vec![error_frame(&session_id, &path.file_id, code, &message)],
                    true,
                    0,
                    total_files,
                    0,
                    Some(message),
                    reply_uses_8bit_controls,
                );
                return;
            }
        };
        let metadata = match file.metadata() {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => {
                let message = "approved path is not a regular file".to_string();
                let _ = emit_with_ack(
                    &mut emit,
                    &cancellation,
                    vec![error_frame(
                        &session_id,
                        &path.file_id,
                        KittyFileTransferStatusCode::Invalid,
                        &message,
                    )],
                    true,
                    0,
                    total_files,
                    0,
                    Some(message),
                    reply_uses_8bit_controls,
                );
                return;
            }
            Err(error) => {
                let message = format!("could not inspect approved regular file: {error}");
                let _ = emit_with_ack(
                    &mut emit,
                    &cancellation,
                    vec![error_frame(
                        &session_id,
                        &path.file_id,
                        KittyFileTransferStatusCode::Io,
                        &message,
                    )],
                    true,
                    0,
                    total_files,
                    0,
                    Some(message),
                    reply_uses_8bit_controls,
                );
                return;
            }
        };
        match std::fs::symlink_metadata(&path.path) {
            Ok(path_metadata)
                if path_metadata.is_file()
                    && path_metadata.dev() == metadata.dev()
                    && path_metadata.ino() == metadata.ino() => {}
            Ok(_) => {
                let message =
                    "approved path changed identity while its file was being opened".to_string();
                let _ = emit_with_ack(
                    &mut emit,
                    &cancellation,
                    vec![error_frame(
                        &session_id,
                        &path.file_id,
                        KittyFileTransferStatusCode::PermissionDenied,
                        &message,
                    )],
                    true,
                    0,
                    total_files,
                    0,
                    Some(message),
                    reply_uses_8bit_controls,
                );
                return;
            }
            Err(error) => {
                let message = format!("could not revalidate approved file identity: {error}");
                let _ = emit_with_ack(
                    &mut emit,
                    &cancellation,
                    vec![error_frame(
                        &session_id,
                        &path.file_id,
                        KittyFileTransferStatusCode::Io,
                        &message,
                    )],
                    true,
                    0,
                    total_files,
                    0,
                    Some(message),
                    reply_uses_8bit_controls,
                );
                return;
            }
        }
        total_bytes = match total_bytes.checked_add(metadata.len()) {
            Some(total) if total <= MAX_KITTY_UPLOAD_BYTES => total,
            _ => {
                let message = "approved files exceed the 64 MiB transfer limit".to_string();
                let _ = emit_with_ack(
                    &mut emit,
                    &cancellation,
                    vec![error_frame(
                        &session_id,
                        &path.file_id,
                        KittyFileTransferStatusCode::TooLarge,
                        &message,
                    )],
                    true,
                    0,
                    total_files,
                    0,
                    Some(message),
                    reply_uses_8bit_controls,
                );
                return;
            }
        };
        let Some(path_name) = path.path.to_str().map(str::to_owned) else {
            let message = "approved path is not valid UTF-8".to_string();
            let _ = emit_with_ack(
                &mut emit,
                &cancellation,
                vec![error_frame(
                    &session_id,
                    &path.file_id,
                    KittyFileTransferStatusCode::Invalid,
                    &message,
                )],
                true,
                0,
                total_files,
                0,
                Some(message),
                reply_uses_8bit_controls,
            );
            return;
        };
        let mtime_ns = metadata
            .mtime()
            .saturating_mul(1_000_000_000)
            .saturating_add(metadata.mtime_nsec());
        metadata_frames.push(
            KittyFileTransferReply::File {
                id: session_id.clone(),
                file_id: path.file_id.clone(),
                actual_file_id: path.file_id.clone(),
                name: path_name.clone(),
                mtime_ns,
                permissions: metadata.mode() & 0o777,
                size: metadata.len(),
            }
            .canonical_reply(),
        );
        sources.insert(
            path.file_id,
            Source {
                requested_name: path.requested_name,
                path_name,
                file,
                size: metadata.len(),
                sent: false,
            },
        );
    }
    let home = directories::BaseDirs::new()
        .and_then(|dirs| dirs.home_dir().to_str().map(str::to_owned))
        .unwrap_or_else(|| "/".into());
    let mut ready = transfer_status(&session_id, "", KittyFileTransferStatusCode::Ok, "");
    ready.name = Some(home);
    metadata_frames.push(ready.canonical_reply());
    if !emit_with_ack(
        &mut emit,
        &cancellation,
        metadata_frames,
        false,
        0,
        total_files,
        0,
        None,
        reply_uses_8bit_controls,
    ) {
        return;
    }

    let mut files_sent = 0usize;
    let mut bytes_sent = 0u64;
    loop {
        let command = match receiver.recv_timeout(WORKER_IDLE_TIMEOUT) {
            Ok(command) => command.command,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let message = "host file read expired after ten minutes of inactivity".to_string();
                let _ = emit_with_ack(
                    &mut emit,
                    &cancellation,
                    vec![error_frame(
                        &session_id,
                        "",
                        KittyFileTransferStatusCode::TimedOut,
                        &message,
                    )],
                    true,
                    files_sent,
                    total_files,
                    bytes_sent,
                    Some(message),
                    reply_uses_8bit_controls,
                );
                return;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        };
        if cancellation.load(Ordering::Acquire) {
            return;
        }
        let Some(source) = sources.get_mut(&command.file_id) else {
            let message = "requested an unknown approved file id".to_string();
            let _ = emit_with_ack(
                &mut emit,
                &cancellation,
                vec![error_frame(
                    &session_id,
                    &command.file_id,
                    KittyFileTransferStatusCode::Invalid,
                    &message,
                )],
                true,
                files_sent,
                total_files,
                bytes_sent,
                Some(message),
                reply_uses_8bit_controls,
            );
            return;
        };
        if source.sent
            || command
                .name
                .as_deref()
                .is_none_or(|name| name != source.path_name && name != source.requested_name)
        {
            let message = "file was already sent or its path did not match approval".to_string();
            let _ = emit_with_ack(
                &mut emit,
                &cancellation,
                vec![error_frame(
                    &session_id,
                    &command.file_id,
                    KittyFileTransferStatusCode::Invalid,
                    &message,
                )],
                true,
                files_sent,
                total_files,
                bytes_sent,
                Some(message),
                reply_uses_8bit_controls,
            );
            return;
        }
        let mut remaining = source.size;
        let mut batch = Vec::with_capacity(UPLOAD_BATCH_CHUNKS);
        if remaining == 0 {
            batch.push(
                KittyFileTransferReply::Data {
                    id: session_id.clone(),
                    file_id: command.file_id.clone(),
                    data: Box::default(),
                    last: true,
                }
                .canonical_reply(),
            );
        }
        while remaining > 0 {
            if cancellation.load(Ordering::Acquire) {
                return;
            }
            let mut chunk = vec![0u8; termeric_core::MAX_KITTY_TRANSFER_DATA_BYTES];
            let wanted = usize::try_from(remaining.min(chunk.len() as u64)).unwrap_or(chunk.len());
            let count = match source.file.read(&mut chunk[..wanted]) {
                Ok(0) => {
                    let message = "approved file changed or ended while being read".to_string();
                    let _ = emit_with_ack(
                        &mut emit,
                        &cancellation,
                        vec![error_frame(
                            &session_id,
                            &command.file_id,
                            KittyFileTransferStatusCode::Io,
                            &message,
                        )],
                        true,
                        files_sent,
                        total_files,
                        bytes_sent,
                        Some(message),
                        reply_uses_8bit_controls,
                    );
                    return;
                }
                Ok(count) => count,
                Err(error) => {
                    let message = format!("could not read approved regular file: {error}");
                    let _ = emit_with_ack(
                        &mut emit,
                        &cancellation,
                        vec![error_frame(
                            &session_id,
                            &command.file_id,
                            KittyFileTransferStatusCode::Io,
                            &message,
                        )],
                        true,
                        files_sent,
                        total_files,
                        bytes_sent,
                        Some(message),
                        reply_uses_8bit_controls,
                    );
                    return;
                }
            };
            chunk.truncate(count);
            remaining -= count as u64;
            batch.push(
                KittyFileTransferReply::Data {
                    id: session_id.clone(),
                    file_id: command.file_id.clone(),
                    data: chunk.into_boxed_slice(),
                    last: remaining == 0,
                }
                .canonical_reply(),
            );
            if batch.len() == UPLOAD_BATCH_CHUNKS || remaining == 0 {
                if !emit_with_ack(
                    &mut emit,
                    &cancellation,
                    std::mem::take(&mut batch),
                    false,
                    files_sent + usize::from(remaining == 0),
                    total_files,
                    bytes_sent + (source.size - remaining),
                    None,
                    reply_uses_8bit_controls,
                ) {
                    return;
                }
            }
        }
        bytes_sent = bytes_sent.saturating_add(source.size);
        source.sent = true;
        files_sent += 1;
    }
}

#[cfg(unix)]
fn start_worker(
    window_id: winit::window::WindowId,
    token: u64,
    session_id: String,
    reply_uses_8bit_controls: bool,
    cancellation: Arc<AtomicBool>,
    proxy: winit::event_loop::EventLoopProxy<AppEvent>,
) -> Result<SyncSender<WorkerCommand>, String> {
    let (sender, receiver) = sync_channel::<WorkerCommand>(WORKER_QUEUE_CAPACITY);
    std::thread::Builder::new()
        .name("termeric-kitty-download".into())
        .spawn(move || {
            run_worker(
                receiver,
                session_id,
                reply_uses_8bit_controls,
                cancellation,
                WORKER_IDLE_TIMEOUT,
                |result| {
                    proxy
                        .send_event(AppEvent::KittyFileTransferCompleted {
                            window_id,
                            token,
                            result,
                        })
                        .is_ok()
                },
            );
        })
        .map_err(|error| format!("could not start transfer worker: {error}"))?;
    Ok(sender)
}

#[cfg(unix)]
fn run_worker(
    receiver: Receiver<WorkerCommand>,
    session_id: String,
    reply_uses_8bit_controls: bool,
    cancellation: Arc<AtomicBool>,
    idle_timeout: Duration,
    mut emit: impl FnMut(WorkerCompletion) -> bool,
) {
    let mut transaction = unix::DownloadTransaction::new(session_id);
    loop {
        let command = match receiver.recv_timeout(idle_timeout) {
            Ok(command) => command.command,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let status = transaction.status(
                    "",
                    KittyFileTransferStatusCode::TimedOut,
                    "transfer expired after ten minutes of inactivity",
                );
                let (files, bytes) = transaction.progress();
                emit(WorkerCompletion {
                    reply_uses_8bit_controls,
                    disposition: WorkerDisposition::Status {
                        status,
                        terminal: true,
                        files,
                        bytes,
                    },
                });
                break;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if cancellation.load(Ordering::Acquire) {
            break;
        }
        let reply_mode = command.reply_uses_8bit_controls;
        let action = command.action;
        let result = transaction.handle(command, &cancellation);
        let (progress_files, progress_bytes) = transaction.progress();
        let disposition = match result {
            Ok(unix::TransactionResult::Status(status)) => WorkerDisposition::Status {
                status,
                terminal: false,
                files: progress_files,
                bytes: progress_bytes,
            },
            Ok(unix::TransactionResult::Finished { files, bytes }) => {
                WorkerDisposition::Finished { files, bytes }
            }
            Err(error) => WorkerDisposition::Status {
                status: transaction.status(&error.file_id, error.code, &error.message),
                terminal: true,
                files: progress_files,
                bytes: progress_bytes,
            },
        };
        let terminal = matches!(
            disposition,
            WorkerDisposition::Finished { .. } | WorkerDisposition::Status { terminal: true, .. }
        );
        if !emit(WorkerCompletion {
            reply_uses_8bit_controls: reply_mode,
            disposition,
        }) || terminal
            || action == KittyFileTransferAction::Finish
        {
            break;
        }
    }
}

#[cfg(not(unix))]
fn start_worker(
    _window_id: winit::window::WindowId,
    _token: u64,
    _session_id: String,
    _reply_uses_8bit_controls: bool,
    _cancellation: Arc<AtomicBool>,
    _proxy: winit::event_loop::EventLoopProxy<AppEvent>,
) -> Result<SyncSender<WorkerCommand>, String> {
    Err("host file transfer is currently supported only on Unix".into())
}

#[cfg(all(test, unix))]
mod worker_tests {
    use super::*;

    #[test]
    fn idle_worker_expires_with_original_reply_framing_and_no_files() {
        let (_sender, receiver) = sync_channel(1);
        let mut completions = Vec::new();
        run_worker(
            receiver,
            "idle-session".into(),
            true,
            Arc::new(AtomicBool::new(false)),
            Duration::from_millis(1),
            |completion| {
                completions.push(completion);
                true
            },
        );

        assert_eq!(completions.len(), 1);
        let completion = completions.pop().unwrap();
        assert!(completion.reply_uses_8bit_controls);
        let WorkerDisposition::Status {
            status,
            terminal,
            files,
            bytes,
        } = completion.disposition
        else {
            panic!("expected timeout status");
        };
        assert!(terminal);
        assert_eq!(files, 0);
        assert_eq!(bytes, 0);
        assert_eq!(status.id, "idle-session");
        assert_eq!(status.code, KittyFileTransferStatusCode::TimedOut);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod upload_worker_tests {
    use super::*;

    fn request(session: &str, file_id: &str, name: &str) -> KittyFileTransferCommand {
        KittyFileTransferCommand {
            action: KittyFileTransferAction::File,
            compression: termeric_core::KittyFileTransferCompression::None,
            file_type: KittyFileTransferType::Regular,
            transmission_type: KittyFileTransmissionType::Simple,
            id: session.into(),
            file_id: file_id.into(),
            quiet: 0,
            mtime_ns: None,
            permissions: None,
            size: None,
            name: Some(name.into()),
            status: None,
            parent: None,
            data: Box::default(),
            bypass_present: false,
            reply_uses_8bit_controls: true,
        }
    }

    #[test]
    fn upload_worker_opens_after_consent_and_streams_bounded_canonical_frames() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("approved.txt");
        std::fs::write(&path, b"approved contents").unwrap();
        let path_name = path.to_str().unwrap().to_string();
        let (sender, receiver) = sync_channel(2);
        sender
            .send(UploadWorkerCommand {
                command: request("upload-test", "f1", &path_name),
            })
            .unwrap();
        drop(sender);
        let mut completions = Vec::new();
        run_upload_worker(
            receiver,
            "upload-test".into(),
            vec![UploadPath {
                file_id: "f1".into(),
                requested_name: path_name.clone(),
                path,
            }],
            true,
            Arc::new(AtomicBool::new(false)),
            |mut completion| {
                if let Some(acknowledge) = completion.acknowledge.take() {
                    acknowledge.send(true).unwrap();
                }
                completions.push(completion);
                true
            },
        );

        assert_eq!(completions.len(), 2);
        assert_eq!(completions[0].frames.len(), 2);
        let metadata = String::from_utf8(completions[0].frames[0].clone()).unwrap();
        assert!(metadata.starts_with("\u{1b}]5113;ac=file;id=upload-test;fid=f1;"));
        assert!(metadata.contains(";sz=17;"));
        assert!(!metadata.contains("approved contents"));
        let ready = String::from_utf8(completions[0].frames[1].clone()).unwrap();
        assert!(ready.contains(";ac=status;id=upload-test;"));
        assert_eq!(completions[1].frames.len(), 1);
        let payload = String::from_utf8(completions[1].frames[0].clone()).unwrap();
        assert!(payload.contains(";ac=end_data;id=upload-test;fid=f1;"));
        assert!(payload.contains("d=YXBwcm92ZWQgY29udGVudHM="));
        assert_eq!(completions[1].files, 1);
        assert_eq!(completions[1].bytes, 17);
        assert!(completions[1].reply_uses_8bit_controls);
    }

    #[test]
    fn upload_worker_refuses_symlinks_without_reading_their_targets() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target.txt");
        let link = temp.path().join("link.txt");
        std::fs::write(&target, b"must not leak").unwrap();
        symlink(&target, &link).unwrap();
        let link_name = link.to_str().unwrap().to_string();
        let (_sender, receiver) = sync_channel(1);
        let mut completions = Vec::new();
        run_upload_worker(
            receiver,
            "symlink-test".into(),
            vec![UploadPath {
                file_id: "f1".into(),
                requested_name: link_name.clone(),
                path: link,
            }],
            false,
            Arc::new(AtomicBool::new(false)),
            |mut completion| {
                if let Some(acknowledge) = completion.acknowledge.take() {
                    acknowledge.send(true).unwrap();
                }
                completions.push(completion);
                true
            },
        );

        assert_eq!(completions.len(), 1);
        assert!(completions[0].terminal);
        assert_eq!(completions[0].bytes, 0);
        let error = String::from_utf8(completions[0].frames[0].clone()).unwrap();
        assert!(error.contains(";ac=status;id=symlink-test;fid=f1;"));
        assert!(!error.contains("must not leak"));

        let real_directory = temp.path().join("real-directory");
        let linked_directory = temp.path().join("linked-directory");
        std::fs::create_dir(&real_directory).unwrap();
        std::fs::write(real_directory.join("nested.txt"), b"also must not leak").unwrap();
        symlink(&real_directory, &linked_directory).unwrap();
        let nested = linked_directory.join("nested.txt");
        let nested_name = nested.to_str().unwrap().to_string();
        let (_sender, receiver) = sync_channel(1);
        let mut ancestor_completions = Vec::new();
        run_upload_worker(
            receiver,
            "ancestor-test".into(),
            vec![UploadPath {
                file_id: "f2".into(),
                requested_name: nested_name,
                path: nested,
            }],
            false,
            Arc::new(AtomicBool::new(false)),
            |mut completion| {
                if let Some(acknowledge) = completion.acknowledge.take() {
                    acknowledge.send(true).unwrap();
                }
                ancestor_completions.push(completion);
                true
            },
        );
        assert_eq!(ancestor_completions.len(), 1);
        assert!(ancestor_completions[0].terminal);
        let error = String::from_utf8(ancestor_completions[0].frames[0].clone()).unwrap();
        assert!(error.contains(";ac=status;id=ancestor-test;fid=f2;"));
        assert!(!error.contains("also must not leak"));
    }

    #[test]
    fn upload_worker_rejects_aggregate_size_before_emitting_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("oversized.bin");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_KITTY_UPLOAD_BYTES + 1).unwrap();
        drop(file);
        let path_name = path.to_str().unwrap().to_string();
        let (_sender, receiver) = sync_channel(1);
        let mut completions = Vec::new();
        run_upload_worker(
            receiver,
            "oversized-test".into(),
            vec![UploadPath {
                file_id: "f1".into(),
                requested_name: path_name,
                path,
            }],
            false,
            Arc::new(AtomicBool::new(false)),
            |mut completion| {
                if let Some(acknowledge) = completion.acknowledge.take() {
                    acknowledge.send(true).unwrap();
                }
                completions.push(completion);
                true
            },
        );

        assert_eq!(completions.len(), 1);
        assert!(completions[0].terminal);
        assert_eq!(completions[0].bytes, 0);
        let error = String::from_utf8(completions[0].frames[0].clone()).unwrap();
        assert!(error.contains("st=RUZCSUc6"));
        assert!(!error.contains("ac=file"));
    }

    #[test]
    fn upload_worker_batches_large_files_behind_acknowledgements() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("multi-chunk.bin");
        let size = termeric_core::MAX_KITTY_TRANSFER_DATA_BYTES * 9 + 1;
        std::fs::write(&path, vec![0x5a; size]).unwrap();
        let path_name = path.to_str().unwrap().to_string();
        let (sender, receiver) = sync_channel(1);
        sender
            .send(UploadWorkerCommand {
                command: request("batch-test", "f1", &path_name),
            })
            .unwrap();
        drop(sender);
        let mut completions = Vec::new();
        run_upload_worker(
            receiver,
            "batch-test".into(),
            vec![UploadPath {
                file_id: "f1".into(),
                requested_name: path_name,
                path,
            }],
            false,
            Arc::new(AtomicBool::new(false)),
            |mut completion| {
                assert!(completion.acknowledge.is_some());
                completion.acknowledge.take().unwrap().send(true).unwrap();
                completions.push(completion);
                true
            },
        );

        assert_eq!(completions.len(), 3);
        assert_eq!(completions[1].frames.len(), UPLOAD_BATCH_CHUNKS);
        assert_eq!(completions[1].bytes, (UPLOAD_BATCH_CHUNKS * 4096) as u64);
        assert_eq!(completions[1].files, 0);
        assert_eq!(completions[2].frames.len(), 2);
        assert_eq!(completions[2].bytes, size as u64);
        assert_eq!(completions[2].files, 1);
        assert!(
            String::from_utf8(completions[2].frames[1].clone())
                .unwrap()
                .contains("ac=end_data")
        );
    }

    #[test]
    fn upload_paths_are_lexical_bounded_and_home_relative() {
        assert!(resolve_kitty_upload_path("../secret").is_err());
        assert!(resolve_kitty_upload_path("/tmp//secret").is_err());
        assert!(resolve_kitty_upload_path("/tmp/./secret").is_err());
        assert!(resolve_kitty_upload_path("/tmp/line\nbreak").is_err());
        assert!(resolve_kitty_upload_path("/tmp/a\u{202e}b").is_err());
        assert!(
            resolve_kitty_upload_path("/tmp/secret")
                .unwrap()
                .is_absolute()
        );
        let relative = resolve_kitty_upload_path("project/file").unwrap();
        assert!(relative.is_absolute());
        assert!(relative.ends_with("project/file"));
        let debug = format!(
            "{:?}",
            UploadPath {
                file_id: "f1".into(),
                requested_name: "/tmp/private-name".into(),
                path: PathBuf::from("/tmp/private-name"),
            }
        );
        assert!(!debug.contains("private-name"));
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use flate2::{Decompress, FlushDecompress, Status};
    use std::collections::{HashMap, HashSet};
    use std::ffi::CString;
    use std::fs::{self, File, Permissions};
    use std::io::{self, Write};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::path::{Component, Path, PathBuf};
    use tempfile::{NamedTempFile, TempPath};

    const DECOMPRESS_BUFFER_BYTES: usize = 64 * 1024;

    pub(super) struct DownloadTransaction {
        session_id: String,
        entries: HashMap<String, Entry>,
        paths: HashSet<PathBuf>,
        node_ids: HashMap<String, String>,
        declared_bytes: u64,
        received_bytes: u64,
    }

    struct Entry {
        path: PathBuf,
        kind: EntryKind,
        mode: u32,
        mtime_ns: Option<i64>,
        declared_size: u64,
        bytes_written: u64,
        complete: bool,
    }

    enum EntryKind {
        Directory,
        Regular {
            temporary: Option<StagedFile>,
            decompressor: Option<DownloadDecompressor>,
        },
        Symlink {
            target: Vec<u8>,
        },
        HardLink {
            target: Vec<u8>,
        },
    }

    struct DownloadDecompressor {
        decoder: Decompress,
        finished: bool,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum CreatedKind {
        Directory,
        File,
        Symlink,
    }

    struct CreatedPath {
        path: PathBuf,
        device: u64,
        inode: u64,
        kind: CreatedKind,
        _identity_guard: Option<File>,
    }

    pub(super) enum TransactionResult {
        Status(KittyFileTransferStatus),
        Finished { files: usize, bytes: u64 },
    }

    #[derive(Debug)]
    pub(super) struct TransactionError {
        pub file_id: String,
        pub code: KittyFileTransferStatusCode,
        pub message: String,
    }

    impl TransactionError {
        fn new(
            file_id: impl Into<String>,
            code: KittyFileTransferStatusCode,
            message: impl Into<String>,
        ) -> Self {
            Self {
                file_id: file_id.into(),
                code,
                message: message.into(),
            }
        }

        fn invalid(file_id: &str, message: impl Into<String>) -> Self {
            Self::new(file_id, KittyFileTransferStatusCode::Invalid, message)
        }

        fn io(file_id: &str, context: &str, error: io::Error) -> Self {
            let code = if error.kind() == io::ErrorKind::AlreadyExists {
                KittyFileTransferStatusCode::AlreadyExists
            } else if error.kind() == io::ErrorKind::PermissionDenied {
                KittyFileTransferStatusCode::PermissionDenied
            } else {
                KittyFileTransferStatusCode::Io
            };
            Self::new(file_id, code, format!("{context}: {error}"))
        }
    }

    impl DownloadTransaction {
        pub(super) fn new(session_id: String) -> Self {
            Self {
                session_id,
                entries: HashMap::new(),
                paths: HashSet::new(),
                node_ids: HashMap::new(),
                declared_bytes: 0,
                received_bytes: 0,
            }
        }

        pub(super) fn status(
            &self,
            file_id: &str,
            code: KittyFileTransferStatusCode,
            message: &str,
        ) -> KittyFileTransferStatus {
            transfer_status(&self.session_id, file_id, code, message)
        }

        pub(super) fn progress(&self) -> (usize, u64) {
            (self.entries.len(), self.received_bytes)
        }

        pub(super) fn handle(
            &mut self,
            command: KittyFileTransferCommand,
            cancellation: &AtomicBool,
        ) -> Result<TransactionResult, TransactionError> {
            match command.action {
                KittyFileTransferAction::File => self.add_file(command),
                KittyFileTransferAction::Data | KittyFileTransferAction::EndData => {
                    self.add_data(command, cancellation)
                }
                KittyFileTransferAction::Finish => self.commit(command, cancellation),
                _ => Err(TransactionError::invalid(
                    &command.file_id,
                    "invalid worker action",
                )),
            }
        }

        fn add_file(
            &mut self,
            command: KittyFileTransferCommand,
        ) -> Result<TransactionResult, TransactionError> {
            if !command.data.is_empty() {
                return Err(TransactionError::invalid(
                    &command.file_id,
                    "file metadata unexpectedly contains payload data",
                ));
            }
            if command.file_type != KittyFileTransferType::Regular
                && command.compression != termeric_core::KittyFileTransferCompression::None
            {
                return Err(TransactionError::invalid(
                    &command.file_id,
                    "compression applies only to regular files",
                ));
            }
            if self.entries.len() >= MAX_KITTY_DOWNLOAD_FILES {
                return Err(TransactionError::new(
                    command.file_id,
                    KittyFileTransferStatusCode::TooLarge,
                    "transfer has too many entries",
                ));
            }
            if command.file_id.is_empty() || self.entries.contains_key(&command.file_id) {
                return Err(TransactionError::invalid(
                    &command.file_id,
                    "missing or duplicate file id",
                ));
            }
            let name = command
                .name
                .as_deref()
                .ok_or_else(|| TransactionError::invalid(&command.file_id, "missing path"))?;
            let path = resolve_target_path(name)
                .map_err(|message| TransactionError::invalid(&command.file_id, message))?;
            if self.paths.contains(&path) {
                return Err(TransactionError::invalid(
                    &command.file_id,
                    "duplicate destination path",
                ));
            }
            validate_destination(&path).map_err(|error| {
                TransactionError::io(&command.file_id, "unsafe destination", error)
            })?;

            let status_id = command.status.as_deref().unwrap_or_default();
            if !status_id.is_empty() && !is_safe_metadata_id(status_id) {
                return Err(TransactionError::invalid(
                    &command.file_id,
                    "invalid metadata node id",
                ));
            }
            if !status_id.is_empty() && self.node_ids.contains_key(status_id) {
                return Err(TransactionError::invalid(
                    &command.file_id,
                    "duplicate metadata node id",
                ));
            }
            if let Some(parent_id) = command.parent.as_deref().filter(|value| !value.is_empty()) {
                let parent_file_id = self.node_ids.get(parent_id).ok_or_else(|| {
                    TransactionError::invalid(&command.file_id, "unknown metadata parent")
                })?;
                let parent = self
                    .entries
                    .get(parent_file_id)
                    .expect("node id names an entry");
                if !matches!(parent.kind, EntryKind::Directory)
                    || !path.starts_with(&parent.path)
                    || path == parent.path
                {
                    return Err(TransactionError::invalid(
                        &command.file_id,
                        "metadata parent does not contain this path",
                    ));
                }
            }

            for entry in self.entries.values() {
                if path.starts_with(&entry.path)
                    && path != entry.path
                    && !matches!(entry.kind, EntryKind::Directory)
                {
                    return Err(TransactionError::invalid(
                        &command.file_id,
                        "destination descends through a non-directory entry",
                    ));
                }
                if entry.path.starts_with(&path)
                    && entry.path != path
                    && command.file_type != KittyFileTransferType::Directory
                {
                    return Err(TransactionError::invalid(
                        &command.file_id,
                        "non-directory destination contains an earlier entry",
                    ));
                }
            }

            let declared_size = match command.file_type {
                KittyFileTransferType::Regular => command.size.ok_or_else(|| {
                    TransactionError::invalid(&command.file_id, "regular file is missing size")
                })?,
                _ => command.size.unwrap_or(0),
            };
            if declared_size < 0 {
                return Err(TransactionError::invalid(
                    &command.file_id,
                    "negative file size",
                ));
            }
            let declared_size = declared_size as u64;
            if command.file_type == KittyFileTransferType::Regular {
                self.declared_bytes =
                    self.declared_bytes
                        .checked_add(declared_size)
                        .ok_or_else(|| {
                            TransactionError::new(
                                &command.file_id,
                                KittyFileTransferStatusCode::TooLarge,
                                "declared transfer size overflowed",
                            )
                        })?;
                if self.declared_bytes > MAX_KITTY_DOWNLOAD_BYTES {
                    return Err(TransactionError::new(
                        &command.file_id,
                        KittyFileTransferStatusCode::TooLarge,
                        "transfer exceeds the 64 MiB limit",
                    ));
                }
            }

            let mode = command.permissions.unwrap_or(match command.file_type {
                KittyFileTransferType::Directory => 0o700,
                _ => 0o600,
            }) & 0o777;
            // Kitty modification times are signed nanoseconds since the epoch;
            // negative values are valid dates, not missing-metadata sentinels.
            let mtime_ns = command.mtime_ns;
            let kind = match command.file_type {
                KittyFileTransferType::Directory => EntryKind::Directory,
                KittyFileTransferType::Regular => EntryKind::Regular {
                    temporary: None,
                    decompressor: (command.compression
                        == termeric_core::KittyFileTransferCompression::Zlib)
                        .then(|| DownloadDecompressor {
                            decoder: Decompress::new(true),
                            finished: false,
                        }),
                },
                KittyFileTransferType::Symlink => EntryKind::Symlink { target: Vec::new() },
                KittyFileTransferType::HardLink => EntryKind::HardLink { target: Vec::new() },
            };
            let complete = command.file_type == KittyFileTransferType::Directory;
            let file_id = command.file_id.clone();
            self.paths.insert(path.clone());
            if !status_id.is_empty() {
                self.node_ids.insert(status_id.into(), file_id.clone());
            }
            self.entries.insert(
                file_id.clone(),
                Entry {
                    path: path.clone(),
                    kind,
                    mode,
                    mtime_ns,
                    declared_size,
                    bytes_written: 0,
                    complete,
                },
            );
            let mut status = self.status(
                &file_id,
                if complete {
                    KittyFileTransferStatusCode::Ok
                } else {
                    KittyFileTransferStatusCode::Started
                },
                "",
            );
            status.name = Some(path.to_string_lossy().into_owned());
            if !complete {
                status.size = Some(-1);
            }
            Ok(TransactionResult::Status(status))
        }

        fn add_data(
            &mut self,
            command: KittyFileTransferCommand,
            cancellation: &AtomicBool,
        ) -> Result<TransactionResult, TransactionError> {
            if command.name.is_some()
                || command.status.is_some()
                || command.parent.is_some()
                || command.size.is_some()
                || command.mtime_ns.is_some()
                || command.permissions.is_some()
                || command.compression != termeric_core::KittyFileTransferCompression::None
                || command.file_type != KittyFileTransferType::Regular
            {
                return Err(TransactionError::invalid(
                    &command.file_id,
                    "data command contains unexpected metadata",
                ));
            }
            let is_last = command.action == KittyFileTransferAction::EndData;
            let file_id = command.file_id.clone();
            let entry = self.entries.get_mut(&file_id).ok_or_else(|| {
                TransactionError::invalid(&file_id, "data arrived before file metadata")
            })?;
            if entry.complete {
                return Err(TransactionError::invalid(
                    &file_id,
                    "data arrived for a completed entry",
                ));
            }
            if cancellation.load(Ordering::Acquire) {
                return Err(TransactionError::new(
                    &file_id,
                    KittyFileTransferStatusCode::Canceled,
                    "transfer was canceled",
                ));
            }

            let before = entry.bytes_written;
            match &mut entry.kind {
                EntryKind::Directory => {
                    return Err(TransactionError::invalid(
                        &file_id,
                        "directory entries cannot receive data",
                    ));
                }
                EntryKind::Regular {
                    temporary,
                    decompressor,
                } => {
                    if temporary.is_none() {
                        *temporary = Some(stage_file(&entry.path).map_err(|error| {
                            TransactionError::io(&file_id, "create staging file", error)
                        })?);
                    }
                    let temporary = temporary.as_mut().expect("staging file was created");
                    if let Some(decompressor) = decompressor {
                        write_decompressed(
                            decompressor,
                            &command.data,
                            is_last,
                            temporary.as_file_mut(),
                            &mut entry.bytes_written,
                            entry.declared_size,
                            cancellation,
                        )
                        .map_err(|message| TransactionError::invalid(&file_id, message))?;
                    } else {
                        let data_len = command.data.len() as u64;
                        if entry.bytes_written.saturating_add(data_len) > entry.declared_size {
                            return Err(TransactionError::invalid(
                                &file_id,
                                "received data exceeds declared size",
                            ));
                        }
                        temporary
                            .as_file_mut()
                            .write_all(&command.data)
                            .map_err(|error| {
                                TransactionError::io(&file_id, "write staging file", error)
                            })?;
                        entry.bytes_written += data_len;
                    }
                    if is_last && entry.bytes_written != entry.declared_size {
                        return Err(TransactionError::invalid(
                            &file_id,
                            format!(
                                "received {} bytes but metadata declared {}",
                                entry.bytes_written, entry.declared_size
                            ),
                        ));
                    }
                }
                EntryKind::Symlink { target } | EntryKind::HardLink { target } => {
                    if target.len().saturating_add(command.data.len())
                        > termeric_core::MAX_KITTY_TRANSFER_PATH_BYTES
                    {
                        return Err(TransactionError::new(
                            &file_id,
                            KittyFileTransferStatusCode::TooLarge,
                            "link target is too large",
                        ));
                    }
                    target.extend_from_slice(&command.data);
                    entry.bytes_written = target.len() as u64;
                }
            }
            self.received_bytes = self
                .received_bytes
                .saturating_add(entry.bytes_written.saturating_sub(before));
            if self.received_bytes > MAX_KITTY_DOWNLOAD_BYTES {
                return Err(TransactionError::new(
                    &file_id,
                    KittyFileTransferStatusCode::TooLarge,
                    "transfer exceeds the 64 MiB limit",
                ));
            }
            if is_last {
                entry.complete = true;
            }
            let complete = entry.complete;
            let path = entry.path.to_string_lossy().into_owned();
            let bytes_written = entry.bytes_written;
            let mut status = self.status(
                &file_id,
                if complete {
                    KittyFileTransferStatusCode::Ok
                } else {
                    KittyFileTransferStatusCode::Progress
                },
                "",
            );
            status.name = Some(path);
            status.size = Some(bytes_written.min(i64::MAX as u64) as i64);
            Ok(TransactionResult::Status(status))
        }

        fn commit(
            &mut self,
            command: KittyFileTransferCommand,
            cancellation: &AtomicBool,
        ) -> Result<TransactionResult, TransactionError> {
            if !command.file_id.is_empty()
                || !command.data.is_empty()
                || command.name.is_some()
                || command.status.is_some()
                || command.parent.is_some()
                || command.size.is_some()
                || command.mtime_ns.is_some()
                || command.permissions.is_some()
                || command.compression != termeric_core::KittyFileTransferCompression::None
                || command.file_type != KittyFileTransferType::Regular
            {
                return Err(TransactionError::invalid(
                    &command.file_id,
                    "finish command contains unexpected file fields",
                ));
            }
            if self.entries.is_empty() {
                return Err(TransactionError::invalid(
                    "",
                    "transfer contains no entries",
                ));
            }
            if self.entries.values().any(|entry| !entry.complete) {
                return Err(TransactionError::invalid(
                    "",
                    "transfer finished with incomplete entries",
                ));
            }
            if cancellation.load(Ordering::Acquire) {
                return Err(TransactionError::new(
                    "",
                    KittyFileTransferStatusCode::Canceled,
                    "transfer was canceled",
                ));
            }
            for entry in self.entries.values() {
                validate_destination(&entry.path).map_err(|error| {
                    TransactionError::io("", "destination changed before commit", error)
                })?;
            }

            let mut created = Vec::<CreatedPath>::new();
            let result = self.commit_inner(cancellation, &mut created);
            if let Err(error) = result {
                rollback_paths(&created);
                return Err(error);
            }
            Ok(TransactionResult::Finished {
                files: self.entries.len(),
                bytes: self.received_bytes,
            })
        }

        fn commit_inner(
            &mut self,
            cancellation: &AtomicBool,
            created: &mut Vec<CreatedPath>,
        ) -> Result<(), TransactionError> {
            let mut directories: Vec<_> = self
                .entries
                .iter()
                .filter(|(_, entry)| matches!(entry.kind, EntryKind::Directory))
                .map(|(id, entry)| (id.clone(), entry.path.clone()))
                .collect();
            directories.sort_by_key(|(_, path)| path.components().count());
            for (id, path) in &directories {
                check_cancel(cancellation, id)?;
                create_missing_parents(path, &self.paths, created).map_err(|error| {
                    TransactionError::io(id, "create destination parent", error)
                })?;
                create_directory(path, created)
                    .map_err(|error| TransactionError::io(id, "create directory", error))?;
            }

            let file_ids: Vec<_> = self
                .entries
                .iter()
                .filter(|(_, entry)| matches!(entry.kind, EntryKind::Regular { .. }))
                .map(|(id, _)| id.clone())
                .collect();
            for id in file_ids {
                check_cancel(cancellation, &id)?;
                let entry = self.entries.get_mut(&id).expect("file id exists");
                create_missing_parents(&entry.path, &self.paths, created).map_err(|error| {
                    TransactionError::io(&id, "create destination parent", error)
                })?;
                let temporary = match &mut entry.kind {
                    EntryKind::Regular { temporary, .. } => temporary.take(),
                    _ => None,
                }
                .ok_or_else(|| TransactionError::invalid(&id, "missing staged file"))?;
                prepare_staged_file(temporary.as_file(), entry.mode, entry.mtime_ns)
                    .map_err(|error| TransactionError::io(&id, "finalize staging file", error))?;
                let identity = temporary
                    .as_file()
                    .metadata()
                    .map_err(|error| TransactionError::io(&id, "inspect staging file", error))?;
                let destination = PublicationTarget::open(&entry.path)
                    .map_err(|error| TransactionError::io(&id, "pin destination parent", error))?;
                let published = destination
                    .publish(temporary)
                    .map_err(|error| TransactionError::io(&id, "publish file", error))?;
                created.push(CreatedPath {
                    path: entry.path.clone(),
                    device: identity.dev(),
                    inode: identity.ino(),
                    kind: CreatedKind::File,
                    _identity_guard: Some(published),
                });
            }

            let link_ids: Vec<_> = self
                .entries
                .iter()
                .filter(|(_, entry)| {
                    matches!(
                        entry.kind,
                        EntryKind::Symlink { .. } | EntryKind::HardLink { .. }
                    )
                })
                .map(|(id, _)| id.clone())
                .collect();
            for id in link_ids {
                check_cancel(cancellation, &id)?;
                let (path, is_symlink, target_data, mtime) = {
                    let entry = self.entries.get(&id).expect("link id exists");
                    let (is_symlink, target) = match &entry.kind {
                        EntryKind::Symlink { target } => (true, target.clone()),
                        EntryKind::HardLink { target } => (false, target.clone()),
                        _ => unreachable!(),
                    };
                    (entry.path.clone(), is_symlink, target, entry.mtime_ns)
                };
                create_missing_parents(&path, &self.paths, created).map_err(|error| {
                    TransactionError::io(&id, "create destination parent", error)
                })?;
                let target = self.resolve_link_target(&id, &path, &target_data, is_symlink)?;
                if is_symlink {
                    std::os::unix::fs::symlink(&target, &path).map_err(|error| {
                        TransactionError::io(&id, "create symbolic link", error)
                    })?;
                } else {
                    fs::hard_link(&target, &path)
                        .map_err(|error| TransactionError::io(&id, "create hard link", error))?;
                }
                record_created(
                    &path,
                    if is_symlink {
                        CreatedKind::Symlink
                    } else {
                        CreatedKind::File
                    },
                    created,
                )
                .map_err(|error| TransactionError::io(&id, "record link", error))?;
                apply_link_mtime(created.last().expect("link was recorded"), mtime).map_err(
                    |error| TransactionError::io(&id, "set link modification time", error),
                )?;
            }

            directories.sort_by_key(|(_, path)| std::cmp::Reverse(path.components().count()));
            for (id, path) in directories {
                let entry = self.entries.get(&id).expect("directory id exists");
                finalize_directory(&path, entry.mode, entry.mtime_ns, created)
                    .map_err(|error| TransactionError::io(&id, "set directory metadata", error))?;
            }
            Ok(())
        }

        fn resolve_link_target(
            &self,
            file_id: &str,
            link_path: &Path,
            data: &[u8],
            symlink: bool,
        ) -> Result<PathBuf, TransactionError> {
            let text = std::str::from_utf8(data)
                .map_err(|_| TransactionError::invalid(file_id, "link target is not UTF-8"))?;
            if !symlink {
                return self
                    .entries
                    .get(text)
                    .filter(|entry| matches!(entry.kind, EntryKind::Regular { .. }))
                    .map(|entry| entry.path.clone())
                    .ok_or_else(|| {
                        TransactionError::invalid(
                            file_id,
                            "hard link names an unknown or non-regular target file id",
                        )
                    });
            }

            let absolute = if let Some(target_id) = text
                .strip_prefix("fid:")
                .or_else(|| text.strip_prefix("fid_abs:"))
            {
                // Symbolic links can name any transferred entry, including a
                // directory. Resolve its destination path without following it;
                // only hard links require the regular-file restriction above.
                self.entries
                    .get(target_id)
                    .map(|entry| entry.path.clone())
                    .ok_or_else(|| {
                        TransactionError::invalid(file_id, "link names unknown file id")
                    })?
            } else if let Some(path) = text.strip_prefix("path:") {
                let raw = Path::new(path);
                if path.is_empty()
                    || path.len() > termeric_core::MAX_KITTY_TRANSFER_PATH_BYTES
                    || path.as_bytes().contains(&0)
                {
                    return Err(TransactionError::invalid(
                        file_id,
                        "symbolic link target is empty or invalid",
                    ));
                }
                return Ok(raw.to_path_buf());
            } else {
                return Err(TransactionError::invalid(
                    file_id,
                    "unknown link target encoding",
                ));
            };
            if text.starts_with("fid:") {
                relative_path(
                    link_path.parent().unwrap_or_else(|| Path::new("/")),
                    &absolute,
                )
                .ok_or_else(|| TransactionError::invalid(file_id, "cannot relativize link"))
            } else {
                Ok(absolute)
            }
        }
    }

    fn resolve_target_path(name: &str) -> Result<PathBuf, String> {
        if name.is_empty() || name.len() > termeric_core::MAX_KITTY_TRANSFER_PATH_BYTES {
            return Err("empty or oversized destination path".into());
        }
        let components = name.strip_prefix('/').unwrap_or(name);
        let components = components.strip_prefix("~/").unwrap_or(components);
        if components.is_empty()
            || components
                .split('/')
                .any(|component| component.is_empty() || matches!(component, "." | ".."))
        {
            return Err("destination contains empty or traversal components".into());
        }
        let home = directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().to_path_buf())
            .ok_or_else(|| "home directory is unavailable".to_string())?;
        let path = if name == "~" {
            home
        } else if let Some(relative) = name.strip_prefix("~/") {
            home.join(relative)
        } else if Path::new(name).is_absolute() {
            PathBuf::from(name)
        } else {
            home.join(name)
        };
        validate_lexical_path(&path)?;
        if path.parent().is_none() {
            return Err("destination cannot be a filesystem root".into());
        }
        Ok(path)
    }

    fn is_safe_metadata_id(value: &str) -> bool {
        value.len() <= termeric_core::MAX_KITTY_TRANSFER_FILE_ID_BYTES
            && value.bytes().all(|byte| {
                matches!(
                    byte,
                    b'0'..=b'9'
                        | b'a'..=b'z'
                        | b'A'..=b'Z'
                        | b'_'
                        | b':'
                        | b'.'
                        | b'/'
                        | b'@'
                        | b'-'
                )
            })
    }

    fn validate_lexical_path(path: &Path) -> Result<(), String> {
        if !path.is_absolute()
            || path.as_os_str().as_bytes().len() > termeric_core::MAX_KITTY_TRANSFER_PATH_BYTES
        {
            return Err("destination must be an absolute bounded POSIX path".into());
        }
        for component in path.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(value) => {
                    let bytes = value.as_bytes();
                    if bytes.is_empty() || bytes.len() > 255 || bytes.contains(&0) {
                        return Err("destination has an invalid path component".into());
                    }
                }
                Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                    return Err("destination contains traversal or a platform prefix".into());
                }
            }
        }
        Ok(())
    }

    fn validate_destination(path: &Path) -> io::Result<()> {
        match fs::symlink_metadata(path) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "destination already exists",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let mut ancestors = Vec::new();
        let mut current = path.parent();
        while let Some(candidate) = current {
            ancestors.push(candidate);
            current = candidate.parent();
        }
        for ancestor in ancestors.into_iter().rev() {
            match fs::symlink_metadata(ancestor) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "destination ancestor is a symbolic link",
                    ));
                }
                Ok(metadata) if !metadata.is_dir() => {
                    return Err(io::Error::new(
                        io::ErrorKind::NotADirectory,
                        "destination ancestor is not a directory",
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    struct StagedFile {
        file: File,
        path: TempPath,
    }

    impl From<NamedTempFile> for StagedFile {
        fn from(temporary: NamedTempFile) -> Self {
            let (file, mut path) = temporary.into_parts();
            // Never let TempPath blindly unlink a replacement during failure,
            // cancellation, or successful publication. Our Drop checks identity.
            path.disable_cleanup(true);
            Self { file, path }
        }
    }

    impl StagedFile {
        fn as_file(&self) -> &File {
            &self.file
        }

        fn as_file_mut(&mut self) -> &mut File {
            &mut self.file
        }
    }

    impl Drop for StagedFile {
        fn drop(&mut self) {
            let Ok(owned) = self.file.metadata() else {
                return;
            };
            let Ok(named) = fs::symlink_metadata(&self.path) else {
                return;
            };
            if owned.is_file()
                && named.is_file()
                && (owned.dev(), owned.ino()) == (named.dev(), named.ino())
            {
                // Best effort, not an atomic compare-and-unlink. A moved source
                // is not searched for, and an unknown/replaced name is retained.
                let _ = fs::remove_file(&self.path);
            }
        }
    }

    struct PublicationTarget {
        #[cfg(target_os = "linux")]
        parent: File,
        path: PathBuf,
    }

    impl PublicationTarget {
        fn open(path: &Path) -> io::Result<Self> {
            let name = path.file_name().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "missing destination name")
            })?;
            let bytes = name.as_bytes();
            if !path.is_absolute()
                || bytes.is_empty()
                || bytes.len() > 255
                || bytes.contains(&0)
                || path
                    .components()
                    .any(|part| matches!(part, Component::ParentDir))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid publication destination",
                ));
            }
            #[cfg(target_os = "linux")]
            {
                use std::os::fd::{AsRawFd, FromRawFd};

                #[repr(C)]
                struct OpenHow {
                    flags: u64,
                    mode: u64,
                    resolve: u64,
                }

                let parent = CString::new(path.parent().unwrap().as_os_str().as_bytes())
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in parent"))?;
                let how = OpenHow {
                    flags: (libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC) as u64,
                    mode: 0,
                    resolve: libc::RESOLVE_NO_SYMLINKS,
                };
                // SAFETY: the NUL-terminated path and initialized open_how live
                // throughout the call. Success returns a new owned descriptor.
                let descriptor = unsafe {
                    libc::syscall(
                        libc::SYS_openat2,
                        libc::AT_FDCWD,
                        parent.as_ptr(),
                        &how,
                        std::mem::size_of::<OpenHow>(),
                    )
                };
                if descriptor < 0 {
                    return Err(io::Error::last_os_error());
                }
                // SAFETY: the successful syscall's descriptor has no other owner.
                let parent = unsafe { File::from_raw_fd(descriptor as _) };
                let path =
                    PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(name);
                Ok(Self { parent, path })
            }
            // Actual host transfers are Linux-only. Preserve the internal Unix
            // transaction test implementation without claiming equivalent safety.
            #[cfg(not(target_os = "linux"))]
            Ok(Self { path: path.into() })
        }

        fn publish(self, temporary: impl Into<StagedFile>) -> io::Result<File> {
            let temporary = temporary.into();
            // Acquire the returned handle before publication so a descriptor
            // allocation failure cannot leave an unrecorded destination behind.
            let published = temporary.as_file().try_clone()?;
            #[cfg(target_os = "linux")]
            {
                use std::os::fd::AsRawFd;

                let source =
                    CString::new(format!("/proc/self/fd/{}", temporary.as_file().as_raw_fd()))
                        .expect("descriptor path contains no NUL");
                let destination = CString::new(self.path.as_os_str().as_bytes())
                    .expect("publication path was validated");
                // SAFETY: both CString pointers and both retained descriptors
                // stay live through linkat. Only our trusted procfs source link
                // is followed. linkat atomically refuses an existing destination.
                let result = unsafe {
                    libc::linkat(
                        libc::AT_FDCWD,
                        source.as_ptr(),
                        libc::AT_FDCWD,
                        destination.as_ptr(),
                        libc::AT_SYMLINK_FOLLOW,
                    )
                };
                if result != 0 {
                    return Err(io::Error::last_os_error());
                }
                drop(self.parent);
            }
            // Internal Unix tests only; actual host transfers remain Linux-only.
            #[cfg(not(target_os = "linux"))]
            fs::hard_link(&temporary.path, &self.path)?;
            Ok(published)
        }
    }

    fn stage_file(path: &Path) -> io::Result<StagedFile> {
        let mut ancestor = path.parent();
        while let Some(candidate) = ancestor {
            match fs::symlink_metadata(candidate) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    return tempfile::Builder::new()
                        .prefix(".termeric-transfer-")
                        .tempfile_in(candidate)
                        .map(StagedFile::from);
                }
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "staging ancestor is not a real directory",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    ancestor = candidate.parent();
                }
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no existing destination ancestor",
        ))
    }

    fn write_decompressed(
        stream: &mut DownloadDecompressor,
        mut input: &[u8],
        is_last: bool,
        output: &mut File,
        bytes_written: &mut u64,
        declared_size: u64,
        cancellation: &AtomicBool,
    ) -> Result<(), String> {
        if cancellation.load(Ordering::Acquire) {
            return Err("transfer was canceled".into());
        }
        if stream.finished {
            // Compression EOF does not finish the protocol entry. Empty data
            // and end_data packets are valid; a second stream or any trailing
            // payload is not, even when it arrives in a later packet.
            return if input.is_empty() {
                Ok(())
            } else {
                Err("data follows the end of the zlib stream".into())
            };
        }
        let decompressor = &mut stream.decoder;
        let mut buffer = [0u8; DECOMPRESS_BUFFER_BYTES];
        loop {
            if cancellation.load(Ordering::Acquire) {
                return Err("transfer was canceled".into());
            }
            let before_in = decompressor.total_in();
            let before_out = decompressor.total_out();
            let status = decompressor
                .decompress(
                    input,
                    &mut buffer,
                    if is_last {
                        FlushDecompress::Finish
                    } else {
                        FlushDecompress::None
                    },
                )
                .map_err(|_| "invalid zlib stream".to_string())?;
            let consumed = (decompressor.total_in() - before_in) as usize;
            let produced = (decompressor.total_out() - before_out) as usize;
            if bytes_written.saturating_add(produced as u64) > declared_size
                || bytes_written.saturating_add(produced as u64) > MAX_KITTY_DOWNLOAD_BYTES
            {
                return Err("decompressed data exceeds declared or session size".into());
            }
            output
                .write_all(&buffer[..produced])
                .map_err(|error| format!("write decompressed data: {error}"))?;
            *bytes_written += produced as u64;
            input = &input[consumed..];
            if status == Status::StreamEnd {
                if !input.is_empty() {
                    return Err("data follows the end of the zlib stream".into());
                }
                stream.finished = true;
                return Ok(());
            }
            if input.is_empty() && produced < buffer.len() {
                if is_last {
                    return Err("truncated zlib stream".into());
                }
                return Ok(());
            }
            if consumed == 0 && produced == 0 {
                return Err("zlib decoder made no progress".into());
            }
        }
    }

    fn create_missing_parents(
        path: &Path,
        transfer_paths: &HashSet<PathBuf>,
        created: &mut Vec<CreatedPath>,
    ) -> io::Result<()> {
        let mut missing = Vec::new();
        let mut current = path.parent();
        while let Some(candidate) = current {
            match fs::symlink_metadata(candidate) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => break,
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "destination ancestor is not a real directory",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    missing.push(candidate.to_path_buf());
                    current = candidate.parent();
                }
                Err(error) => return Err(error),
            }
        }
        for directory in missing.into_iter().rev() {
            if transfer_paths.contains(&directory) {
                continue;
            }
            create_directory(&directory, created)?;
        }
        Ok(())
    }

    fn open_directory(path: &Path) -> io::Result<File> {
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
    }

    fn open_directory_identity(path: &Path) -> io::Result<File> {
        let result = open_directory(path);
        #[cfg(target_os = "linux")]
        if result
            .as_ref()
            .is_err_and(|error| error.kind() == io::ErrorKind::PermissionDenied)
        {
            // O_PATH can pin a directory whose initial permissions were
            // removed by umask, without reading it or following a symlink.
            return fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW)
                .open(path);
        }
        result
    }

    fn create_directory(path: &Path, created: &mut Vec<CreatedPath>) -> io::Result<()> {
        fs::DirBuilder::new().mode(0o700).create(path)?;
        record_created(path, CreatedKind::Directory, created)?;
        let entry = created.last_mut().expect("created directory was recorded");
        ensure_directory_access(entry)
    }

    fn ensure_directory_access(entry: &mut CreatedPath) -> io::Result<()> {
        let directory = entry._identity_guard.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "directory handle is missing",
            )
        })?;
        let metadata = directory.metadata()?;
        if entry.kind != CreatedKind::Directory
            || !metadata.is_dir()
            || metadata.dev() != entry.device
            || metadata.ino() != entry.inode
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "directory handle does not match its recorded identity",
            ));
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: directory owns a live descriptor; F_GETFL takes no
            // additional argument and does not mutate descriptor state.
            let flags = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_GETFL) };
            if flags < 0 {
                return Err(io::Error::last_os_error());
            }
            if flags & libc::O_PATH != 0 {
                // This trusted procfs magic link names the retained inode,
                // even if the destination/ancestor pathname was replaced.
                // Keep directory borrowed until reopening is complete. If
                // procfs is unavailable, fail without a destination fallback.
                let proc_path = format!("/proc/self/fd/{}", directory.as_raw_fd());
                fs::set_permissions(&proc_path, Permissions::from_mode(0o700))?;
                let upgraded = fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_DIRECTORY | libc::O_NONBLOCK)
                    .open(&proc_path)?;
                let metadata = upgraded.metadata()?;
                if !metadata.is_dir()
                    || metadata.dev() != entry.device
                    || metadata.ino() != entry.inode
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "reopened directory does not match its recorded identity",
                    ));
                }
                entry._identity_guard = Some(upgraded);
                return Ok(());
            }
        }
        directory.set_permissions(Permissions::from_mode(0o700))
    }

    fn prepare_staged_file(file: &File, mode: u32, mtime_ns: Option<i64>) -> io::Result<()> {
        // Finish metadata on the retained inode before making it visible, then
        // sync both data and metadata. Never re-open its eventual destination.
        apply_file_mtime(file, mtime_ns)?;
        file.set_permissions(Permissions::from_mode(mode))?;
        file.sync_all()
    }

    fn finalize_directory(
        path: &Path,
        mode: u32,
        mtime_ns: Option<i64>,
        created: &[CreatedPath],
    ) -> io::Result<()> {
        // The path is a transaction lookup key, never fresh filesystem
        // authority: another process may have replaced it since creation.
        let entry = created
            .iter()
            .find(|entry| entry.path == path && entry.kind == CreatedKind::Directory)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "directory was not recorded"))?;
        let directory = entry._identity_guard.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "directory handle is missing",
            )
        })?;
        let metadata = directory.metadata()?;
        if !metadata.is_dir() || metadata.dev() != entry.device || metadata.ino() != entry.inode {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "directory handle does not match its recorded identity",
            ));
        }
        apply_file_mtime(directory, mtime_ns)?;
        directory.set_permissions(Permissions::from_mode(mode))
    }

    fn apply_file_mtime(file: &File, mtime_ns: Option<i64>) -> io::Result<()> {
        if let Some(mtime_ns) = mtime_ns {
            let distance = Duration::from_nanos(mtime_ns.unsigned_abs());
            let timestamp = if mtime_ns < 0 {
                std::time::UNIX_EPOCH.checked_sub(distance)
            } else {
                std::time::UNIX_EPOCH.checked_add(distance)
            }
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "timestamp is out of range")
            })?;
            file.set_times(
                fs::FileTimes::new()
                    .set_accessed(timestamp)
                    .set_modified(timestamp),
            )?;
        }
        Ok(())
    }

    fn record_created(
        path: &Path,
        kind: CreatedKind,
        created: &mut Vec<CreatedPath>,
    ) -> io::Result<()> {
        record_created_with(path, kind, created, |path| fs::symlink_metadata(path))
    }

    fn record_created_with(
        path: &Path,
        kind: CreatedKind,
        created: &mut Vec<CreatedPath>,
        inspect: impl FnOnce(&Path) -> io::Result<fs::Metadata>,
    ) -> io::Result<()> {
        match inspect(path) {
            Ok(metadata) => {
                let kind_matches = match kind {
                    CreatedKind::Directory => metadata.is_dir(),
                    CreatedKind::File => metadata.is_file(),
                    CreatedKind::Symlink => metadata.file_type().is_symlink(),
                };
                if !kind_matches {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "created path changed type before its identity was recorded",
                    ));
                }
                // Bookkeeping must never open a symbolic link's target: it
                // could be a FIFO/device, and a followed handle would not pin
                // the link's inode anyway. Linux O_PATH pins even dangling
                // symlinks without opening their contents or requiring readers.
                let identity_guard = if kind == CreatedKind::Directory {
                    // Retain an fchmod-capable handle before final permissions
                    // can remove owner access needed by rollback.
                    open_directory_identity(path)
                } else {
                    #[cfg(target_os = "linux")]
                    let flags = libc::O_PATH | libc::O_NOFOLLOW;
                    #[cfg(not(target_os = "linux"))]
                    let flags = libc::O_NOFOLLOW | libc::O_NONBLOCK;
                    fs::OpenOptions::new()
                        .read(true)
                        .custom_flags(flags)
                        .open(path)
                }
                .ok()
                .filter(|file| {
                    file.metadata().is_ok_and(|opened| {
                        opened.dev() == metadata.dev() && opened.ino() == metadata.ino()
                    })
                });
                created.push(CreatedPath {
                    path: path.to_path_buf(),
                    device: metadata.dev(),
                    inode: metadata.ino(),
                    kind,
                    _identity_guard: identity_guard,
                });
                Ok(())
            }
            // Without an observed identity there is no safe cleanup authority:
            // the pathname might already refer to somebody else's replacement.
            Err(error) => Err(error),
        }
    }

    fn rollback_paths(created: &[CreatedPath]) {
        // Restore access top-down through retained handles before unlinking
        // children. Never chmod the current path, which might be a replacement.
        for entry in created {
            if entry.kind == CreatedKind::Directory
                && let Some(directory) = &entry._identity_guard
                && let Ok(metadata) = directory.metadata()
                && metadata.is_dir()
                && metadata.dev() == entry.device
                && metadata.ino() == entry.inode
            {
                let _ = directory.set_permissions(Permissions::from_mode(
                    (metadata.permissions().mode() & 0o777) | 0o700,
                ));
            }
        }
        for created in created.iter().rev() {
            match fs::symlink_metadata(&created.path) {
                Ok(metadata)
                    if metadata.dev() == created.device
                        && metadata.ino() == created.inode
                        && match created.kind {
                            CreatedKind::Directory => {
                                metadata.is_dir() && !metadata.file_type().is_symlink()
                            }
                            CreatedKind::File => {
                                metadata.is_file() && !metadata.file_type().is_symlink()
                            }
                            CreatedKind::Symlink => metadata.file_type().is_symlink(),
                        } =>
                {
                    let _ = if created.kind == CreatedKind::Directory {
                        fs::remove_dir(&created.path)
                    } else {
                        fs::remove_file(&created.path)
                    };
                }
                Ok(_) | Err(_) => {}
            }
        }
    }

    fn check_cancel(cancellation: &AtomicBool, file_id: &str) -> Result<(), TransactionError> {
        if cancellation.load(Ordering::Acquire) {
            Err(TransactionError::new(
                file_id,
                KittyFileTransferStatusCode::Canceled,
                "transfer was canceled",
            ))
        } else {
            Ok(())
        }
    }

    fn apply_link_mtime(entry: &CreatedPath, mtime_ns: Option<i64>) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;

            let Some(mtime_ns) = mtime_ns else {
                return Ok(());
            };
            let file = entry._identity_guard.as_ref().ok_or_else(|| {
                io::Error::new(io::ErrorKind::PermissionDenied, "link handle is missing")
            })?;
            let metadata = file.metadata()?;
            let kind_matches = match entry.kind {
                CreatedKind::Symlink => metadata.file_type().is_symlink(),
                CreatedKind::File => metadata.is_file(),
                CreatedKind::Directory => false,
            };
            if !kind_matches || metadata.dev() != entry.device || metadata.ino() != entry.inode {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "link handle does not match its recorded identity",
                ));
            }
            let times = timestamp_times(mtime_ns)?;
            // SAFETY: file owns the live descriptor, c"" is NUL-terminated,
            // and times contains exactly two initialized timespec values.
            // AT_EMPTY_PATH operates on the pinned O_PATH inode; NOFOLLOW
            // keeps symlink timestamps on the link, never its target. Do not
            // fall back to a pathname if this requires a newer kernel.
            let result = unsafe {
                libc::utimensat(
                    file.as_raw_fd(),
                    c"".as_ptr(),
                    times.as_ptr(),
                    libc::AT_EMPTY_PATH | libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if result == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            // Non-Linux hosts still reject transfers at the application gate.
            apply_mtime(&entry.path, mtime_ns, entry.kind == CreatedKind::Symlink)
        }
    }

    #[cfg(any(test, not(target_os = "linux")))]
    fn apply_mtime(path: &Path, mtime_ns: Option<i64>, symlink: bool) -> io::Result<()> {
        let Some(mtime_ns) = mtime_ns else {
            return Ok(());
        };
        let times = timestamp_times(mtime_ns)?;
        let path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
        let flags = if symlink {
            libc::AT_SYMLINK_NOFOLLOW
        } else {
            0
        };
        // SAFETY: `path` is a live NUL-terminated byte string and `times`
        // contains exactly two initialized timespec values for this call.
        let result =
            unsafe { libc::utimensat(libc::AT_FDCWD, path.as_ptr(), times.as_ptr(), flags) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn timestamp_times(mtime_ns: i64) -> io::Result<[libc::timespec; 2]> {
        // time_t is narrower on some Unix targets; never wrap a wire timestamp.
        #[allow(clippy::useless_conversion)]
        let seconds = mtime_ns.div_euclid(1_000_000_000).try_into().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "timestamp is out of range")
        })?;
        Ok([libc::timespec {
            tv_sec: seconds,
            tv_nsec: mtime_ns.rem_euclid(1_000_000_000) as libc::c_long,
        }; 2])
    }

    fn relative_path(base: &Path, target: &Path) -> Option<PathBuf> {
        let base_components: Vec<_> = base.components().collect();
        let target_components: Vec<_> = target.components().collect();
        let common = base_components
            .iter()
            .zip(&target_components)
            .take_while(|(left, right)| left == right)
            .count();
        if common == 0 {
            return None;
        }
        let mut output = PathBuf::new();
        for _ in common..base_components.len() {
            output.push("..");
        }
        for component in &target_components[common..] {
            output.push(component.as_os_str());
        }
        Some(output)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use flate2::{Compression, write::ZlibEncoder};
        use tempfile::tempdir;

        fn command(
            action: KittyFileTransferAction,
            file_id: &str,
            path: Option<&Path>,
            file_type: KittyFileTransferType,
            data: &[u8],
        ) -> KittyFileTransferCommand {
            let is_metadata = action == KittyFileTransferAction::File;
            KittyFileTransferCommand {
                action,
                compression: termeric_core::KittyFileTransferCompression::None,
                file_type: if is_metadata {
                    file_type
                } else {
                    KittyFileTransferType::Regular
                },
                transmission_type: KittyFileTransmissionType::Simple,
                id: "session".into(),
                file_id: file_id.into(),
                quiet: 0,
                mtime_ns: is_metadata.then_some(1_700_000_000_123_456_789),
                permissions: is_metadata.then_some(
                    if file_type == KittyFileTransferType::Directory {
                        0o750
                    } else {
                        0o640
                    },
                ),
                size: is_metadata.then(|| path.map_or(0, |_| data.len() as i64)),
                name: path.map(|path| path.to_string_lossy().into_owned()),
                status: is_metadata.then(|| file_id.into()),
                parent: None,
                data: data.into(),
                bypass_present: false,
                reply_uses_8bit_controls: false,
            }
        }

        #[test]
        fn directory_final_permissions_do_not_retain_assembly_access() {
            for requested in [0o000, 0o055, 0o555, 0o2750] {
                let root = tempdir().unwrap();
                let directory = root.path().join("tree");
                let child = directory.join("child");
                let file = child.join("file");
                let cancel = AtomicBool::new(false);
                let mut transaction = DownloadTransaction::new("session".into());
                for (id, path) in [("d", &directory), ("c", &child)] {
                    let mut metadata = command(
                        KittyFileTransferAction::File,
                        id,
                        Some(path),
                        KittyFileTransferType::Directory,
                        b"",
                    );
                    metadata.permissions = Some(requested);
                    transaction.handle(metadata, &cancel).unwrap();
                }
                let mut metadata = command(
                    KittyFileTransferAction::File,
                    "f",
                    Some(&file),
                    KittyFileTransferType::Regular,
                    b"content",
                );
                metadata.data = Box::default();
                transaction.handle(metadata, &cancel).unwrap();
                transaction
                    .handle(
                        command(
                            KittyFileTransferAction::EndData,
                            "f",
                            None,
                            KittyFileTransferType::Regular,
                            b"content",
                        ),
                        &cancel,
                    )
                    .unwrap();
                transaction
                    .handle(
                        command(
                            KittyFileTransferAction::Finish,
                            "",
                            None,
                            KittyFileTransferType::Regular,
                            b"",
                        ),
                        &cancel,
                    )
                    .unwrap();
                // Restore access top-down only after observing the actual final
                // mode, so 000/055 directories are inspectable and removable.
                for path in [&directory, &child] {
                    let actual = fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777;
                    fs::set_permissions(path, Permissions::from_mode(0o700)).unwrap();
                    assert_eq!(actual, requested & 0o777, "{path:?}");
                }
                assert_eq!(fs::read(&file).unwrap(), b"content");
            }
        }

        #[test]
        fn rollback_restores_owned_directory_access_before_removing_children() {
            let root = tempdir().unwrap();
            let parent = root.path().join("parent");
            let child = parent.join("child");
            let file = child.join("file");
            let mut created = Vec::new();
            for path in [&parent, &child] {
                fs::create_dir(path).unwrap();
                record_created(path, CreatedKind::Directory, &mut created).unwrap();
            }
            fs::write(&file, b"payload").unwrap();
            record_created(&file, CreatedKind::File, &mut created).unwrap();
            for path in [&child, &parent] {
                fs::set_permissions(path, Permissions::from_mode(0o000)).unwrap();
            }
            rollback_paths(&created);
            let removed = fs::symlink_metadata(&parent).is_err();
            // A failing implementation must not leave inaccessible test data.
            if !removed {
                fs::set_permissions(&parent, Permissions::from_mode(0o700)).unwrap();
                fs::set_permissions(&child, Permissions::from_mode(0o700)).unwrap();
                rollback_paths(&created);
            }
            assert!(
                removed,
                "rollback left a finalized restrictive directory tree"
            );
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
        }

        #[test]
        fn unverified_directory_handle_never_changes_replacement_permissions() {
            for symbolic in [false, true] {
                let root = tempdir().unwrap();
                let path = root.path().join("owned");
                let moved = root.path().join("moved");
                let target = root.path().join("target");
                let mut created = Vec::new();
                create_directory(&path, &mut created).unwrap();
                fs::rename(&path, &moved).unwrap();
                let replacement = if symbolic { &target } else { &path };
                fs::create_dir(replacement).unwrap();
                fs::set_permissions(replacement, Permissions::from_mode(0o750)).unwrap();
                if symbolic {
                    std::os::unix::fs::symlink(&target, &path).unwrap();
                }
                created[0]._identity_guard = None;
                assert!(ensure_directory_access(&mut created[0]).is_err());
                created[0]._identity_guard = Some(open_directory(replacement).unwrap());
                assert!(ensure_directory_access(&mut created[0]).is_err());
                assert_eq!(
                    fs::metadata(replacement).unwrap().permissions().mode() & 0o777,
                    0o750
                );
                assert_eq!(
                    fs::metadata(&moved).unwrap().permissions().mode() & 0o777,
                    0o700
                );
            }
        }

        #[test]
        #[cfg(target_os = "linux")]
        fn opath_directory_access_recovery_ignores_replaced_paths() {
            for symbolic in [false, true] {
                let root = tempdir().unwrap();
                let path = root.path().join("owned");
                let moved = root.path().join("moved");
                let target = root.path().join("target");
                let mut created = Vec::new();
                create_directory(&path, &mut created).unwrap();
                // Force this path even for a privileged test runner that can
                // open unreadable directories without the O_PATH fallback.
                created[0]._identity_guard = Some(
                    fs::OpenOptions::new()
                        .read(true)
                        .custom_flags(libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW)
                        .open(&path)
                        .unwrap(),
                );
                fs::set_permissions(&path, Permissions::from_mode(0o000)).unwrap();
                fs::rename(&path, &moved).unwrap();
                let replacement = if symbolic { &target } else { &path };
                fs::create_dir(replacement).unwrap();
                fs::set_permissions(replacement, Permissions::from_mode(0o750)).unwrap();
                if symbolic {
                    std::os::unix::fs::symlink(&target, &path).unwrap();
                }
                let before = fs::metadata(replacement).unwrap();
                ensure_directory_access(&mut created[0]).unwrap();
                assert_eq!(
                    fs::metadata(&moved).unwrap().permissions().mode() & 0o777,
                    0o700
                );
                // Finalization requires the upgraded usable handle, not merely
                // a successful chmod through the metadata-only handle.
                finalize_directory(&path, 0o555, Some(-1), &created).unwrap();
                let owned = fs::metadata(&moved).unwrap();
                assert_eq!(owned.permissions().mode() & 0o777, 0o555);
                assert_eq!((owned.mtime(), owned.mtime_nsec()), (-1, 999_999_999));
                rollback_paths(&created);
                let after = fs::metadata(replacement).unwrap();
                assert_eq!(after.permissions().mode(), before.permissions().mode());
                assert_eq!(
                    (after.mtime(), after.mtime_nsec()),
                    (before.mtime(), before.mtime_nsec())
                );
                assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
            }
        }

        #[test]
        #[cfg(target_os = "linux")]
        fn directory_assembly_access_survives_restrictive_umask() {
            const CHILD: &str = "TERMERIC_TEST_TRANSFER_DIRECTORY_UMASK";
            if std::env::var_os(CHILD).is_none() {
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "app::kitty_file_transfer::unix::tests::directory_assembly_access_survives_restrictive_umask", "--nocapture"])
                    .env(CHILD, "1")
                    .output().unwrap();
                assert!(
                    output.status.success(),
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                return;
            }
            let root = tempdir().unwrap();
            // Change the process-wide umask only in this isolated test process.
            // SAFETY: umask has no pointer arguments; no sibling test runs here.
            let previous = unsafe { libc::umask(0o777) };
            let mut created = Vec::new();
            let path = root.path().join("private");
            let result = create_directory(&path, &mut created);
            // SAFETY: restore the original process umask before assertions/cleanup.
            unsafe { libc::umask(previous) };
            result.unwrap();
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert!(created[0]._identity_guard.is_some());
            rollback_paths(&created);
            assert!(!path.exists());
        }

        #[test]
        fn directory_finalization_never_changes_a_replacement_path() {
            for symlink_replacement in [false, true] {
                let root = tempdir().unwrap();
                let path = root.path().join("owned");
                let moved = root.path().join("moved");
                let target = root.path().join("target");
                let mut created = Vec::new();
                create_directory(&path, &mut created).unwrap();
                fs::rename(&path, &moved).unwrap();
                let replacement = if symlink_replacement { &target } else { &path };
                fs::create_dir(replacement).unwrap();
                fs::set_permissions(replacement, Permissions::from_mode(0o750)).unwrap();
                apply_mtime(replacement, Some(1_700_000_000_123_456_789), false).unwrap();
                let before = fs::metadata(replacement).unwrap();
                if symlink_replacement {
                    std::os::unix::fs::symlink(&target, &path).unwrap();
                }
                finalize_directory(&path, 0o555, Some(-1), &created).unwrap();
                let owned = fs::metadata(&moved).unwrap();
                let after = fs::metadata(replacement).unwrap();
                // Restore cleanup access before asserting, including on the
                // vulnerable path-based implementation used by the red test.
                created[0]
                    ._identity_guard
                    .as_ref()
                    .unwrap()
                    .set_permissions(Permissions::from_mode(0o700))
                    .unwrap();
                fs::set_permissions(replacement, Permissions::from_mode(0o700)).unwrap();
                assert_eq!(owned.permissions().mode() & 0o777, 0o555);
                assert_eq!((owned.mtime(), owned.mtime_nsec()), (-1, 999_999_999));
                assert_eq!(after.permissions().mode(), before.permissions().mode());
                assert_eq!(
                    (after.mtime(), after.mtime_nsec()),
                    (before.mtime(), before.mtime_nsec())
                );
                assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
            }
        }

        #[test]
        fn directory_finalization_requires_its_recorded_handle() {
            for invalid in ["unrecorded", "missing", "mismatched", "wrong-kind"] {
                let root = tempdir().unwrap();
                let path = root.path().join("owned");
                let other = root.path().join("other");
                let mut created = Vec::new();
                create_directory(&path, &mut created).unwrap();
                fs::create_dir(&other).unwrap();
                let before = fs::metadata(&path).unwrap();
                let other_before = fs::metadata(&other).unwrap();
                match invalid {
                    "unrecorded" => created.clear(),
                    "missing" => created[0]._identity_guard = None,
                    "mismatched" => {
                        created[0]._identity_guard = Some(open_directory(&other).unwrap())
                    }
                    "wrong-kind" => created[0].kind = CreatedKind::File,
                    _ => unreachable!(),
                }
                assert!(
                    finalize_directory(&path, 0o000, Some(-1), &created).is_err(),
                    "{invalid}"
                );
                for (path, before) in [(&path, before), (&other, other_before)] {
                    let after = fs::metadata(path).unwrap();
                    assert_eq!(
                        after.permissions().mode(),
                        before.permissions().mode(),
                        "{invalid}"
                    );
                    assert_eq!(
                        (after.mtime(), after.mtime_nsec()),
                        (before.mtime(), before.mtime_nsec()),
                        "{invalid}"
                    );
                }
            }
        }

        #[test]
        fn directory_rollback_does_not_change_replacement_permissions() {
            for symlink_replacement in [false, true] {
                let root = tempdir().unwrap();
                let path = root.path().join("owned");
                let moved = root.path().join("moved");
                let target = root.path().join("target");
                let mut created = Vec::new();
                create_directory(&path, &mut created).unwrap();
                fs::set_permissions(&path, Permissions::from_mode(0o000)).unwrap();
                fs::rename(&path, &moved).unwrap();
                let replacement = if symlink_replacement { &target } else { &path };
                fs::create_dir(replacement).unwrap();
                fs::set_permissions(replacement, Permissions::from_mode(0o555)).unwrap();
                if symlink_replacement {
                    std::os::unix::fs::symlink(&target, &path).unwrap();
                }
                rollback_paths(&created);
                assert_eq!(
                    fs::metadata(replacement).unwrap().permissions().mode() & 0o777,
                    0o555
                );
                assert_eq!(
                    fs::metadata(&moved).unwrap().permissions().mode() & 0o777,
                    0o700
                );
                assert!(fs::symlink_metadata(&path).is_ok());
                fs::set_permissions(replacement, Permissions::from_mode(0o700)).unwrap();
            }
        }

        #[test]
        fn directory_symlinks_resolve_forward_relative_and_absolute_targets() {
            let root = tempdir().unwrap();
            let directory = root.path().join("tree");
            let file = directory.join("file");
            let relative = root.path().join("links/relative");
            let absolute = root.path().join("absolute");
            let cancel = AtomicBool::new(false);
            let mut transaction = DownloadTransaction::new("session".into());
            // Announce both links before the directory they name. Resolution
            // belongs to finish, once the complete transfer inventory exists.
            for (id, path, target) in [
                ("r", &relative, b"fid:d".as_slice()),
                ("a", &absolute, b"fid_abs:d".as_slice()),
            ] {
                transaction
                    .handle(
                        command(
                            KittyFileTransferAction::File,
                            id,
                            Some(path),
                            KittyFileTransferType::Symlink,
                            b"",
                        ),
                        &cancel,
                    )
                    .unwrap();
                transaction
                    .handle(
                        command(
                            KittyFileTransferAction::EndData,
                            id,
                            None,
                            KittyFileTransferType::Symlink,
                            target,
                        ),
                        &cancel,
                    )
                    .unwrap();
            }
            let mut dir = command(
                KittyFileTransferAction::File,
                "d",
                Some(&directory),
                KittyFileTransferType::Directory,
                b"",
            );
            dir.permissions = Some(0o750);
            transaction.handle(dir, &cancel).unwrap();
            let mut metadata = command(
                KittyFileTransferAction::File,
                "f",
                Some(&file),
                KittyFileTransferType::Regular,
                b"content",
            );
            metadata.data = Box::default();
            metadata.parent = Some("d".into());
            transaction.handle(metadata, &cancel).unwrap();
            transaction
                .handle(
                    command(
                        KittyFileTransferAction::EndData,
                        "f",
                        None,
                        KittyFileTransferType::Regular,
                        b"content",
                    ),
                    &cancel,
                )
                .unwrap();
            for path in [&directory, &relative, &absolute] {
                assert!(
                    fs::symlink_metadata(path).is_err(),
                    "published early: {path:?}"
                );
            }
            assert!(matches!(
                transaction
                    .handle(
                        command(
                            KittyFileTransferAction::Finish,
                            "",
                            None,
                            KittyFileTransferType::Regular,
                            b"",
                        ),
                        &cancel
                    )
                    .unwrap(),
                TransactionResult::Finished {
                    files: 4,
                    bytes: 21
                }
            ));
            assert_eq!(fs::read_link(&relative).unwrap(), PathBuf::from("../tree"));
            assert_eq!(fs::read_link(&absolute).unwrap(), directory);
            for path in [&relative, &absolute] {
                assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
                assert_eq!(fs::read(path.join("file")).unwrap(), b"content");
            }
            assert_eq!(
                fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
                0o750
            );
        }

        #[test]
        fn directory_hardlinks_and_unknown_symlink_ids_fail_with_rollback() {
            for (kind, target) in [
                (KittyFileTransferType::HardLink, b"d".as_slice()),
                (KittyFileTransferType::Symlink, b"fid:unknown".as_slice()),
                (
                    KittyFileTransferType::Symlink,
                    b"fid_abs:unknown".as_slice(),
                ),
            ] {
                let root = tempdir().unwrap();
                let directory = root.path().join("tree");
                let link = root.path().join("link");
                let cancel = AtomicBool::new(false);
                let mut transaction = DownloadTransaction::new("session".into());
                transaction
                    .handle(
                        command(
                            KittyFileTransferAction::File,
                            "d",
                            Some(&directory),
                            KittyFileTransferType::Directory,
                            b"",
                        ),
                        &cancel,
                    )
                    .unwrap();
                transaction
                    .handle(
                        command(KittyFileTransferAction::File, "l", Some(&link), kind, b""),
                        &cancel,
                    )
                    .unwrap();
                transaction
                    .handle(
                        command(KittyFileTransferAction::EndData, "l", None, kind, target),
                        &cancel,
                    )
                    .unwrap();
                let error = transaction
                    .handle(
                        command(
                            KittyFileTransferAction::Finish,
                            "",
                            None,
                            KittyFileTransferType::Regular,
                            b"",
                        ),
                        &cancel,
                    )
                    .err()
                    .expect("invalid target must fail");
                assert_eq!(error.file_id, "l");
                assert_eq!(error.code, KittyFileTransferStatusCode::Invalid);
                drop(transaction);
                assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
            }
        }

        #[test]
        fn signed_modification_times_survive_transaction_commit() {
            for mtime_ns in [-1_i64, -1_000_000_001, 0, 1_700_000_000_123_456_789] {
                let root = tempdir().unwrap();
                let directory = root.path().join("tree");
                let file = directory.join("file");
                let hardlink = directory.join("hard");
                let symlink = directory.join("symbolic");
                let cancel = AtomicBool::new(false);
                let mut transaction = DownloadTransaction::new("session".into());
                for (id, path, kind, data, timestamp) in [
                    (
                        "d",
                        &directory,
                        KittyFileTransferType::Directory,
                        b"".as_slice(),
                        mtime_ns,
                    ),
                    (
                        "f",
                        &file,
                        KittyFileTransferType::Regular,
                        b"content".as_slice(),
                        mtime_ns,
                    ),
                    (
                        "h",
                        &hardlink,
                        KittyFileTransferType::HardLink,
                        b"f".as_slice(),
                        mtime_ns,
                    ),
                    // A distinct link timestamp verifies AT_SYMLINK_NOFOLLOW:
                    // setting it must not change the regular file's timestamp.
                    (
                        "s",
                        &symlink,
                        KittyFileTransferType::Symlink,
                        b"fid:f".as_slice(),
                        mtime_ns - 1,
                    ),
                ] {
                    let mut metadata =
                        command(KittyFileTransferAction::File, id, Some(path), kind, b"");
                    metadata.mtime_ns = Some(timestamp);
                    metadata.size = Some(if kind == KittyFileTransferType::Regular {
                        data.len() as i64
                    } else {
                        0
                    });
                    if id != "d" {
                        metadata.parent = Some("d".into());
                    }
                    transaction.handle(metadata, &cancel).unwrap();
                    if kind != KittyFileTransferType::Directory {
                        transaction
                            .handle(
                                command(KittyFileTransferAction::EndData, id, None, kind, data),
                                &cancel,
                            )
                            .unwrap();
                    }
                }
                transaction
                    .handle(
                        command(
                            KittyFileTransferAction::Finish,
                            "",
                            None,
                            KittyFileTransferType::Regular,
                            b"",
                        ),
                        &cancel,
                    )
                    .unwrap();
                for (path, expected) in [
                    (&directory, mtime_ns),
                    (&file, mtime_ns),
                    (&hardlink, mtime_ns),
                    (&symlink, mtime_ns - 1),
                ] {
                    let metadata = fs::symlink_metadata(path).unwrap();
                    assert_eq!(
                        metadata.mtime(),
                        expected.div_euclid(1_000_000_000),
                        "{path:?}"
                    );
                    assert_eq!(
                        metadata.mtime_nsec(),
                        expected.rem_euclid(1_000_000_000),
                        "{path:?}"
                    );
                }
                assert_eq!(fs::read(&file).unwrap(), b"content");
                assert_eq!(
                    fs::metadata(&file).unwrap().ino(),
                    fs::metadata(&hardlink).unwrap().ino()
                );
                assert_eq!(fs::read_link(&symlink).unwrap(), PathBuf::from("file"));
            }
        }

        #[test]
        #[cfg(target_os = "linux")]
        fn staging_cleanup_preserves_replacements_before_publication() {
            for replacement_kind in ["file", "link", "directory", "none"] {
                let root = tempdir().unwrap();
                let staged = stage_file(&root.path().join("destination")).unwrap();
                let source = staged.path.to_path_buf();
                if replacement_kind == "none" {
                    drop(staged);
                    assert!(!source.exists());
                    continue;
                }
                let moved = root.path().join("moved");
                fs::rename(&source, &moved).unwrap();
                match replacement_kind {
                    "file" => fs::write(&source, b"keep").unwrap(),
                    "link" => std::os::unix::fs::symlink("missing", &source).unwrap(),
                    "directory" => fs::create_dir(&source).unwrap(),
                    _ => unreachable!(),
                }
                let before = fs::symlink_metadata(&source).unwrap();
                drop(staged);
                let after = fs::symlink_metadata(&source).unwrap();
                assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
                assert!(moved.exists());
            }
        }

        #[test]
        #[cfg(target_os = "linux")]
        fn publication_uses_staging_inode_not_replacement_name() {
            for symbolic in [false, true] {
                for remove_original in [false, true] {
                    let root = tempdir().unwrap();
                    let mut staged = NamedTempFile::new_in(root.path()).unwrap();
                    staged.write_all(b"approved payload").unwrap();
                    prepare_staged_file(staged.as_file(), 0o400, Some(-1)).unwrap();
                    let identity = staged.as_file().metadata().unwrap();
                    let source = staged.path().to_owned();
                    if remove_original {
                        fs::remove_file(&source).unwrap();
                    } else {
                        fs::rename(&source, root.path().join("moved")).unwrap();
                    }
                    let victim = root.path().join("victim");
                    if symbolic {
                        fs::write(&victim, b"keep victim").unwrap();
                        std::os::unix::fs::symlink(&victim, &source).unwrap();
                    } else {
                        fs::write(&source, b"replacement payload").unwrap();
                    }
                    let replacement = fs::symlink_metadata(&source).unwrap();
                    let destination = root.path().join("published");
                    let result = PublicationTarget::open(&destination)
                        .unwrap()
                        .publish(staged);
                    if remove_original {
                        assert!(
                            result.is_err(),
                            "an unlinked named inode cannot be republished"
                        );
                        assert!(!destination.exists());
                    } else {
                        let published = result.unwrap();
                        let metadata = fs::symlink_metadata(&destination).unwrap();
                        assert!(metadata.is_file());
                        assert_eq!(
                            (metadata.dev(), metadata.ino()),
                            (identity.dev(), identity.ino())
                        );
                        assert_eq!(published.metadata().unwrap().ino(), metadata.ino());
                        assert_eq!(metadata.permissions().mode() & 0o777, 0o400);
                        assert_eq!((metadata.mtime(), metadata.mtime_nsec()), (-1, 999_999_999));
                        assert_eq!(fs::read(&destination).unwrap(), b"approved payload");
                    }
                    let after = fs::symlink_metadata(&source).unwrap();
                    assert_eq!(
                        (after.dev(), after.ino()),
                        (replacement.dev(), replacement.ino())
                    );
                    if symbolic {
                        assert_eq!(fs::read_link(&source).unwrap(), victim);
                        assert_eq!(fs::read(&victim).unwrap(), b"keep victim");
                    } else {
                        assert_eq!(fs::read(&source).unwrap(), b"replacement payload");
                    }
                }
            }
        }

        #[test]
        #[cfg(target_os = "linux")]
        fn publication_retains_parent_after_ancestor_replacement() {
            for symbolic in [false, true] {
                for replace_ancestor in [false, true] {
                    let root = tempdir().unwrap();
                    let ancestor = root.path().join("ancestor");
                    let parent = ancestor.join("parent");
                    fs::create_dir_all(&parent).unwrap();
                    let target = PublicationTarget::open(&parent.join("published")).unwrap();
                    let mut staged = NamedTempFile::new_in(root.path()).unwrap();
                    staged.write_all(b"payload").unwrap();
                    prepare_staged_file(staged.as_file(), 0o400, Some(-1)).unwrap();
                    let identity = staged.as_file().metadata().unwrap();
                    let replaced = if replace_ancestor { &ancestor } else { &parent };
                    let moved = root.path().join("moved");
                    fs::rename(replaced, &moved).unwrap();
                    let outside = root.path().join("outside");
                    let replacement = if symbolic { &outside } else { replaced };
                    fs::create_dir(replacement).unwrap();
                    let replacement_parent = if replace_ancestor {
                        let nested = replacement.join("parent");
                        fs::create_dir(&nested).unwrap();
                        nested
                    } else {
                        replacement.clone()
                    };
                    if symbolic {
                        std::os::unix::fs::symlink(&outside, replaced).unwrap();
                    }
                    let before = fs::metadata(&replacement_parent).unwrap();
                    let published = target.publish(staged).unwrap();
                    let metadata = published.metadata().unwrap();
                    assert_eq!(
                        (metadata.dev(), metadata.ino()),
                        (identity.dev(), identity.ino())
                    );
                    assert_eq!(metadata.permissions().mode() & 0o777, 0o400);
                    assert_eq!((metadata.mtime(), metadata.mtime_nsec()), (-1, 999_999_999));
                    let moved_parent = if replace_ancestor {
                        moved.join("parent")
                    } else {
                        moved
                    };
                    assert_eq!(
                        fs::read(moved_parent.join("published")).unwrap(),
                        b"payload"
                    );
                    assert!(!replacement_parent.join("published").exists());
                    let after = fs::metadata(&replacement_parent).unwrap();
                    assert_eq!(
                        (after.mtime(), after.mtime_nsec()),
                        (before.mtime(), before.mtime_nsec())
                    );
                }
            }
        }

        #[test]
        #[cfg(target_os = "linux")]
        fn publication_refuses_symlink_parents_and_invalid_destinations() {
            let root = tempdir().unwrap();
            let directory = root.path().join("directory");
            fs::create_dir_all(directory.join("nested")).unwrap();
            let link = root.path().join("link");
            std::os::unix::fs::symlink(&directory, &link).unwrap();
            for path in [link.join("file"), link.join("nested/file")] {
                assert!(PublicationTarget::open(&path).is_err());
            }
            for path in [
                PathBuf::from("relative/file"),
                PathBuf::from("/"),
                directory.join("../escape"),
                directory.join("bad\0name"),
                directory.join("x".repeat(256)),
            ] {
                assert!(PublicationTarget::open(&path).is_err(), "{path:?}");
            }
        }

        #[test]
        #[cfg(target_os = "linux")]
        fn publication_never_overwrites_existing_symlinks() {
            for dangling in [false, true] {
                let root = tempdir().unwrap();
                let victim = root.path().join("victim");
                if !dangling {
                    fs::write(&victim, b"keep").unwrap();
                }
                let destination = root.path().join("destination");
                std::os::unix::fs::symlink(&victim, &destination).unwrap();
                let before = fs::symlink_metadata(&destination).unwrap();
                let staged = NamedTempFile::new_in(root.path()).unwrap();
                let staging_path = staged.path().to_owned();
                let error = PublicationTarget::open(&destination)
                    .unwrap()
                    .publish(staged)
                    .unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
                assert!(!staging_path.exists());
                let after = fs::symlink_metadata(&destination).unwrap();
                assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
                assert_eq!(fs::read_link(&destination).unwrap(), victim);
                if dangling {
                    assert!(!victim.exists());
                } else {
                    assert_eq!(fs::read(&victim).unwrap(), b"keep");
                }
            }
        }

        #[test]
        fn staged_file_metadata_is_complete_before_publication() {
            for mtime in [None, Some(-1), Some(0), Some(1_700_000_000_123_456_789)] {
                for mode in [0o000, 0o400, 0o640] {
                    let root = tempdir().unwrap();
                    let destination = root.path().join("published");
                    let mut staged = NamedTempFile::new_in(root.path()).unwrap();
                    staged.write_all(b"payload").unwrap();
                    let before = staged.as_file().metadata().unwrap();
                    prepare_staged_file(staged.as_file(), mode, mtime).unwrap();
                    assert!(!destination.exists());
                    let prepared = staged.as_file().metadata().unwrap();
                    let expected = mtime.map_or((before.mtime(), before.mtime_nsec()), |value| {
                        (
                            value.div_euclid(1_000_000_000),
                            value.rem_euclid(1_000_000_000),
                        )
                    });
                    assert_eq!((prepared.mtime(), prepared.mtime_nsec()), expected);
                    assert_eq!(prepared.permissions().mode() & 0o777, mode);
                    let published = PublicationTarget::open(&destination)
                        .unwrap()
                        .publish(staged)
                        .unwrap();
                    let after = published.metadata().unwrap();
                    assert_eq!((after.dev(), after.ino()), (prepared.dev(), prepared.ino()));
                    assert_eq!((after.mtime(), after.mtime_nsec()), expected);
                    assert_eq!(after.permissions().mode() & 0o777, mode);
                }
            }
        }

        #[test]
        fn staged_file_metadata_never_changes_a_replacement_path() {
            for symlink_replacement in [false, true] {
                let root = tempdir().unwrap();
                let path = root.path().join("staged");
                let moved = root.path().join("moved");
                let target = root.path().join("target");
                let mut file = File::create(&path).unwrap();
                file.write_all(b"ours").unwrap();
                fs::rename(&path, &moved).unwrap();
                let replacement = if symlink_replacement { &target } else { &path };
                fs::write(replacement, b"keep").unwrap();
                apply_mtime(replacement, Some(1_700_000_000_123_456_789), false).unwrap();
                let before = fs::metadata(replacement).unwrap();
                if symlink_replacement {
                    std::os::unix::fs::symlink(&target, &path).unwrap();
                }
                prepare_staged_file(&file, 0o400, Some(-1)).unwrap();
                let owned = file.metadata().unwrap();
                let after = fs::metadata(replacement).unwrap();
                assert_eq!(owned.permissions().mode() & 0o777, 0o400);
                assert_eq!((owned.mtime(), owned.mtime_nsec()), (-1, 999_999_999));
                assert_eq!(after.permissions().mode(), before.permissions().mode());
                assert_eq!(
                    (after.mtime(), after.mtime_nsec()),
                    (before.mtime(), before.mtime_nsec())
                );
                assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
                assert_eq!(fs::read(replacement).unwrap(), b"keep");
                assert_eq!(fs::read(&moved).unwrap(), b"ours");
            }
        }

        #[test]
        fn prepared_file_does_not_change_an_existing_destination() {
            let root = tempdir().unwrap();
            let destination = root.path().join("existing");
            fs::write(&destination, b"keep").unwrap();
            apply_mtime(&destination, Some(1_700_000_000_123_456_789), false).unwrap();
            let before = fs::metadata(&destination).unwrap();
            let mut staged = NamedTempFile::new_in(root.path()).unwrap();
            staged.write_all(b"new").unwrap();
            let staged_path = staged.path().to_path_buf();
            prepare_staged_file(staged.as_file(), 0o000, Some(-1)).unwrap();
            let error = PublicationTarget::open(&destination)
                .unwrap()
                .publish(staged)
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            drop(error);
            assert!(!staged_path.exists());
            let after = fs::metadata(&destination).unwrap();
            assert_eq!(after.permissions().mode(), before.permissions().mode());
            assert_eq!(
                (after.mtime(), after.mtime_nsec()),
                (before.mtime(), before.mtime_nsec())
            );
            assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
            assert_eq!(fs::read(&destination).unwrap(), b"keep");
        }

        #[test]
        #[cfg(target_os = "linux")]
        fn link_timestamps_use_the_retained_inode_after_path_replacement() {
            for symbolic in [false, true] {
                for replacement_symbolic in [false, true] {
                    let root = tempdir().unwrap();
                    let source = root.path().join("source");
                    let path = root.path().join("link");
                    let moved = root.path().join("moved");
                    let victim = root.path().join("victim");
                    fs::write(&source, b"source").unwrap();
                    apply_mtime(&source, Some(1_700_000_000_123_456_789), false).unwrap();
                    let source_before = fs::metadata(&source).unwrap();
                    let kind = if symbolic {
                        std::os::unix::fs::symlink(&source, &path).unwrap();
                        CreatedKind::Symlink
                    } else {
                        fs::hard_link(&source, &path).unwrap();
                        CreatedKind::File
                    };
                    let mut created = Vec::new();
                    record_created(&path, kind, &mut created).unwrap();
                    fs::rename(&path, &moved).unwrap();
                    fs::write(&victim, b"keep").unwrap();
                    if replacement_symbolic {
                        std::os::unix::fs::symlink(&victim, &path).unwrap();
                    } else {
                        fs::write(&path, b"replacement").unwrap();
                    }
                    let replacement_before = fs::symlink_metadata(&path).unwrap();
                    let victim_before = fs::metadata(&victim).unwrap();
                    apply_link_mtime(&created[0], Some(-1)).unwrap();
                    let owned = fs::symlink_metadata(&moved).unwrap();
                    assert_eq!((owned.mtime(), owned.mtime_nsec()), (-1, 999_999_999));
                    for (path, before) in [(&path, replacement_before), (&victim, victim_before)] {
                        let after = fs::symlink_metadata(path).unwrap();
                        assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
                        assert_eq!(
                            (after.mtime(), after.mtime_nsec()),
                            (before.mtime(), before.mtime_nsec())
                        );
                    }
                    let source_after = fs::metadata(&source).unwrap();
                    let expected = if symbolic {
                        (source_before.mtime(), source_before.mtime_nsec())
                    } else {
                        (-1, 999_999_999)
                    };
                    assert_eq!((source_after.mtime(), source_after.mtime_nsec()), expected);
                    assert_eq!(fs::read(&victim).unwrap(), b"keep");
                    assert_eq!(fs::read(&source).unwrap(), b"source");
                }
            }
        }

        #[test]
        #[cfg(target_os = "linux")]
        fn link_timestamp_updates_require_the_recorded_identity() {
            for invalid in ["missing", "foreign-link", "wrong-kind", "wrong-inode"] {
                let root = tempdir().unwrap();
                let source = root.path().join("source");
                let first = root.path().join("first");
                let second = root.path().join("second");
                fs::write(&source, b"keep").unwrap();
                std::os::unix::fs::symlink(&source, &first).unwrap();
                std::os::unix::fs::symlink(&source, &second).unwrap();
                let mut created = Vec::new();
                record_created(&first, CreatedKind::Symlink, &mut created).unwrap();
                record_created(&second, CreatedKind::Symlink, &mut created).unwrap();
                let snapshots: Vec<_> = [&source, &first, &second]
                    .into_iter()
                    .map(|path| (path, fs::symlink_metadata(path).unwrap()))
                    .collect();
                match invalid {
                    "missing" => created[0]._identity_guard = None,
                    "foreign-link" => {
                        created[0]._identity_guard = Some(
                            created[1]
                                ._identity_guard
                                .as_ref()
                                .unwrap()
                                .try_clone()
                                .unwrap(),
                        )
                    }
                    "wrong-kind" => created[0].kind = CreatedKind::File,
                    "wrong-inode" => created[0].inode ^= 1,
                    _ => unreachable!(),
                }
                // Missing timestamps perform no syscall, even without a guard.
                apply_link_mtime(&created[0], None).unwrap();
                assert_eq!(
                    apply_link_mtime(&created[0], Some(-1)).unwrap_err().kind(),
                    io::ErrorKind::PermissionDenied,
                    "{invalid}"
                );
                for (path, before) in snapshots {
                    let after = fs::symlink_metadata(path).unwrap();
                    assert_eq!(
                        (after.mtime(), after.mtime_nsec()),
                        (before.mtime(), before.mtime_nsec()),
                        "{invalid}: {path:?}"
                    );
                }
            }
        }

        #[test]
        #[cfg(target_os = "linux")]
        fn dangling_link_timestamps_survive_unlink_without_path_recovery() {
            let root = tempdir().unwrap();
            let path = root.path().join("link");
            std::os::unix::fs::symlink("missing", &path).unwrap();
            let mut created = Vec::new();
            record_created(&path, CreatedKind::Symlink, &mut created).unwrap();
            fs::remove_file(&path).unwrap();
            apply_link_mtime(&created[0], Some(-1_000_000_001)).unwrap();
            let metadata = created[0]
                ._identity_guard
                .as_ref()
                .unwrap()
                .metadata()
                .unwrap();
            assert!(metadata.file_type().is_symlink());
            assert_eq!((metadata.mtime(), metadata.mtime_nsec()), (-2, 999_999_999));
            assert!(fs::symlink_metadata(&path).is_err());
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
        }

        #[test]
        fn regular_file_is_staged_and_published_only_on_finish() {
            let root = tempdir().unwrap();
            let path = root.path().join("nested/file.txt");
            let cancel = AtomicBool::new(false);
            let mut transaction = DownloadTransaction::new("session".into());
            let mut metadata = command(
                KittyFileTransferAction::File,
                "f1",
                Some(&path),
                KittyFileTransferType::Regular,
                b"hello",
            );
            metadata.data = Box::default();
            transaction.handle(metadata, &cancel).unwrap();
            assert!(!path.exists());
            let data = command(
                KittyFileTransferAction::EndData,
                "f1",
                None,
                KittyFileTransferType::Regular,
                b"hello",
            );
            transaction.handle(data, &cancel).unwrap();
            assert!(!path.exists());
            let finish = command(
                KittyFileTransferAction::Finish,
                "",
                None,
                KittyFileTransferType::Regular,
                b"",
            );
            assert!(matches!(
                transaction.handle(finish, &cancel).unwrap(),
                TransactionResult::Finished { files: 1, bytes: 5 }
            ));
            assert_eq!(fs::read(&path).unwrap(), b"hello");
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }

        #[test]
        fn zlib_stream_boundaries_are_independent_of_transfer_packets() {
            for payload in [
                Vec::new(),
                b"compressed content".to_vec(),
                vec![b'x'; 128 * 1024],
            ] {
                let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
                encoder.write_all(&payload).unwrap();
                let compressed = encoder.finish().unwrap();
                assert!(compressed.len() < 4096);
                for split in 0..=compressed.len() {
                    for separate_end in [false, true] {
                        let root = tempdir().unwrap();
                        let path = root.path().join("file");
                        let cancel = AtomicBool::new(false);
                        let mut transaction = compressed_transaction(&path, payload.len());
                        transaction
                            .handle(
                                command(
                                    KittyFileTransferAction::Data,
                                    "f",
                                    None,
                                    KittyFileTransferType::Regular,
                                    &compressed[..split],
                                ),
                                &cancel,
                            )
                            .unwrap();
                        transaction
                            .handle(
                                command(
                                    if separate_end {
                                        KittyFileTransferAction::Data
                                    } else {
                                        KittyFileTransferAction::EndData
                                    },
                                    "f",
                                    None,
                                    KittyFileTransferType::Regular,
                                    &compressed[split..],
                                ),
                                &cancel,
                            )
                            .unwrap();
                        if separate_end {
                            // An empty non-final packet must not complete the
                            // entry or reset a decoder that has reached EOF.
                            transaction
                                .handle(
                                    command(
                                        KittyFileTransferAction::Data,
                                        "f",
                                        None,
                                        KittyFileTransferType::Regular,
                                        b"",
                                    ),
                                    &cancel,
                                )
                                .unwrap();
                            assert!(!transaction.entries["f"].complete);
                            transaction
                                .handle(
                                    command(
                                        KittyFileTransferAction::EndData,
                                        "f",
                                        None,
                                        KittyFileTransferType::Regular,
                                        b"",
                                    ),
                                    &cancel,
                                )
                                .unwrap();
                        }
                        assert!(!path.exists());
                        transaction
                            .handle(
                                command(
                                    KittyFileTransferAction::Finish,
                                    "",
                                    None,
                                    KittyFileTransferType::Regular,
                                    b"",
                                ),
                                &cancel,
                            )
                            .unwrap();
                        assert_eq!(fs::read(&path).unwrap(), payload);
                    }
                }
            }
        }

        fn compressed_transaction(path: &Path, size: usize) -> DownloadTransaction {
            let mut transaction = DownloadTransaction::new("session".into());
            let mut metadata = command(
                KittyFileTransferAction::File,
                "f",
                Some(path),
                KittyFileTransferType::Regular,
                b"",
            );
            metadata.size = Some(size as i64);
            metadata.compression = termeric_core::KittyFileTransferCompression::Zlib;
            transaction
                .handle(metadata, &AtomicBool::new(false))
                .unwrap();
            transaction
        }

        #[test]
        fn invalid_zlib_framing_never_publishes_or_retains_staging() {
            use KittyFileTransferAction::{Data, EndData};

            let payload = b"compressed content";
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(payload).unwrap();
            let compressed = encoder.finish().unwrap();
            let mut corrupt = compressed.clone();
            *corrupt.last_mut().unwrap() ^= 1;
            let mut trailing = compressed.clone();
            trailing.extend_from_slice(b"junk");
            let mut concatenated = compressed.clone();
            concatenated.extend_from_slice(&compressed);
            for (name, size, packets) in [
                (
                    "truncated",
                    payload.len(),
                    vec![(EndData, compressed[..compressed.len() - 1].to_vec())],
                ),
                ("checksum", payload.len(), vec![(EndData, corrupt)]),
                (
                    "same-packet suffix",
                    payload.len(),
                    vec![(EndData, trailing)],
                ),
                (
                    "same-packet second stream",
                    payload.len(),
                    vec![(EndData, concatenated)],
                ),
                (
                    "later suffix",
                    payload.len(),
                    vec![(Data, compressed.clone()), (EndData, b"junk".to_vec())],
                ),
                (
                    "later second stream",
                    payload.len(),
                    vec![(Data, compressed.clone()), (Data, compressed.clone())],
                ),
                (
                    "missing end_data",
                    payload.len(),
                    vec![(Data, compressed.clone())],
                ),
                (
                    "undersized declaration",
                    payload.len() - 1,
                    vec![(EndData, compressed.clone())],
                ),
                (
                    "oversized declaration",
                    payload.len() + 1,
                    vec![(Data, compressed.clone()), (EndData, Vec::new())],
                ),
            ] {
                let root = tempdir().unwrap();
                let path = root.path().join("file");
                let cancel = AtomicBool::new(false);
                let mut transaction = compressed_transaction(&path, size);
                let mut failure = None;
                for (action, data) in packets {
                    if let Err(error) = transaction.handle(
                        command(action, "f", None, KittyFileTransferType::Regular, &data),
                        &cancel,
                    ) {
                        failure = Some(error);
                        break;
                    }
                }
                if failure.is_none() {
                    failure = transaction
                        .handle(
                            command(
                                KittyFileTransferAction::Finish,
                                "",
                                None,
                                KittyFileTransferType::Regular,
                                b"",
                            ),
                            &cancel,
                        )
                        .err();
                }
                assert_eq!(
                    failure.expect(name).code,
                    KittyFileTransferStatusCode::Invalid,
                    "{name}"
                );
                assert!(!path.exists(), "published {name}");
                drop(transaction);
                assert_eq!(
                    fs::read_dir(root.path()).unwrap().count(),
                    0,
                    "retained staging for {name}"
                );
            }
        }

        #[test]
        fn zlib_directory_and_internal_links_commit_together() {
            let root = tempdir().unwrap();
            let directory = root.path().join("tree");
            let file = directory.join("file.txt");
            let symlink = directory.join("alias.txt");
            let hardlink = directory.join("hard.txt");
            let cancel = AtomicBool::new(false);
            let mut transaction = DownloadTransaction::new("session".into());

            let mut dir = command(
                KittyFileTransferAction::File,
                "d",
                Some(&directory),
                KittyFileTransferType::Directory,
                b"",
            );
            dir.size = Some(0);
            transaction.handle(dir, &cancel).unwrap();

            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(b"compressed").unwrap();
            let compressed = encoder.finish().unwrap();
            let mut regular = command(
                KittyFileTransferAction::File,
                "f",
                Some(&file),
                KittyFileTransferType::Regular,
                b"compressed",
            );
            regular.data = Box::default();
            regular.parent = Some("d".into());
            regular.compression = termeric_core::KittyFileTransferCompression::Zlib;
            transaction.handle(regular, &cancel).unwrap();
            let body = command(
                KittyFileTransferAction::EndData,
                "f",
                None,
                KittyFileTransferType::Regular,
                &compressed,
            );
            transaction.handle(body, &cancel).unwrap();

            for (id, path, kind, target) in [
                (
                    "s",
                    &symlink,
                    KittyFileTransferType::Symlink,
                    b"fid:f".as_slice(),
                ),
                (
                    "h",
                    &hardlink,
                    KittyFileTransferType::HardLink,
                    b"f".as_slice(),
                ),
            ] {
                let mut metadata =
                    command(KittyFileTransferAction::File, id, Some(path), kind, b"");
                metadata.size = Some(0);
                metadata.parent = Some("d".into());
                transaction.handle(metadata, &cancel).unwrap();
                let data = command(KittyFileTransferAction::EndData, id, None, kind, target);
                transaction.handle(data, &cancel).unwrap();
            }
            let finish = command(
                KittyFileTransferAction::Finish,
                "",
                None,
                KittyFileTransferType::Regular,
                b"",
            );
            transaction.handle(finish, &cancel).unwrap();
            assert_eq!(fs::read(&file).unwrap(), b"compressed");
            assert_eq!(fs::read_link(&symlink).unwrap(), PathBuf::from("file.txt"));
            assert_eq!(fs::read(&hardlink).unwrap(), b"compressed");
            assert_eq!(
                fs::metadata(&file).unwrap().ino(),
                fs::metadata(&hardlink).unwrap().ino()
            );
        }

        #[test]
        fn traversal_existing_targets_and_size_mismatch_fail_closed() {
            let root = tempdir().unwrap();
            let existing = root.path().join("existing");
            fs::write(&existing, b"keep").unwrap();
            let cancel = AtomicBool::new(false);
            let mut transaction = DownloadTransaction::new("session".into());
            assert!(
                transaction
                    .handle(
                        command(
                            KittyFileTransferAction::File,
                            "e",
                            Some(&existing),
                            KittyFileTransferType::Regular,
                            b"x",
                        ),
                        &cancel,
                    )
                    .is_err()
            );
            assert_eq!(fs::read(&existing).unwrap(), b"keep");
            assert!(resolve_target_path("../escape").is_err());
            assert!(resolve_target_path("nested//empty").is_err());
            assert!(resolve_target_path("nested/./dot").is_err());

            let path = root.path().join("short");
            let mut metadata = command(
                KittyFileTransferAction::File,
                "short",
                Some(&path),
                KittyFileTransferType::Regular,
                b"1234",
            );
            metadata.data = Box::default();
            transaction.handle(metadata, &cancel).unwrap();
            assert!(
                transaction
                    .handle(
                        command(
                            KittyFileTransferAction::EndData,
                            "short",
                            None,
                            KittyFileTransferType::Regular,
                            b"12",
                        ),
                        &cancel,
                    )
                    .is_err()
            );
            assert!(!path.exists());
        }

        #[test]
        fn cancellation_decompression_limits_and_symlink_races_leave_no_payload() {
            let root = tempdir().unwrap();
            let cancel = AtomicBool::new(false);

            let canceled_path = root.path().join("canceled.bin");
            let mut canceled = DownloadTransaction::new("cancel".into());
            let mut metadata = command(
                KittyFileTransferAction::File,
                "c",
                Some(&canceled_path),
                KittyFileTransferType::Regular,
                b"payload",
            );
            metadata.data = Box::default();
            canceled.handle(metadata, &cancel).unwrap();
            canceled
                .handle(
                    command(
                        KittyFileTransferAction::EndData,
                        "c",
                        None,
                        KittyFileTransferType::Regular,
                        b"payload",
                    ),
                    &cancel,
                )
                .unwrap();
            cancel.store(true, Ordering::Release);
            assert!(
                canceled
                    .handle(
                        command(
                            KittyFileTransferAction::Finish,
                            "",
                            None,
                            KittyFileTransferType::Regular,
                            b"",
                        ),
                        &cancel,
                    )
                    .is_err()
            );
            drop(canceled);
            assert!(!canceled_path.exists());
            assert!(fs::read_dir(root.path()).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".termeric-transfer-")
            }));

            let bomb_path = root.path().join("bomb.bin");
            let mut bomb = DownloadTransaction::new("bomb".into());
            let mut bomb_metadata = command(
                KittyFileTransferAction::File,
                "b",
                Some(&bomb_path),
                KittyFileTransferType::Regular,
                b"tiny",
            );
            bomb_metadata.data = Box::default();
            bomb_metadata.size = Some(4);
            bomb_metadata.compression = termeric_core::KittyFileTransferCompression::Zlib;
            bomb.handle(bomb_metadata, &AtomicBool::new(false)).unwrap();
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&vec![b'x'; 8 * 1024]).unwrap();
            let compressed = encoder.finish().unwrap();
            let bomb_data = command(
                KittyFileTransferAction::EndData,
                "b",
                None,
                KittyFileTransferType::Regular,
                &compressed,
            );
            assert!(bomb.handle(bomb_data, &AtomicBool::new(false)).is_err());
            assert!(!bomb_path.exists());

            let outside = tempdir().unwrap();
            let raced_parent = root.path().join("raced");
            let raced_path = raced_parent.join("file");
            let mut raced = DownloadTransaction::new("race".into());
            let mut raced_metadata = command(
                KittyFileTransferAction::File,
                "r",
                Some(&raced_path),
                KittyFileTransferType::Regular,
                b"data",
            );
            raced_metadata.data = Box::default();
            raced
                .handle(raced_metadata, &AtomicBool::new(false))
                .unwrap();
            raced
                .handle(
                    command(
                        KittyFileTransferAction::EndData,
                        "r",
                        None,
                        KittyFileTransferType::Regular,
                        b"data",
                    ),
                    &AtomicBool::new(false),
                )
                .unwrap();
            std::os::unix::fs::symlink(outside.path(), &raced_parent).unwrap();
            assert!(
                raced
                    .handle(
                        command(
                            KittyFileTransferAction::Finish,
                            "",
                            None,
                            KittyFileTransferType::Regular,
                            b"",
                        ),
                        &AtomicBool::new(false),
                    )
                    .is_err()
            );
            assert!(!outside.path().join("file").exists());
        }

        #[test]
        fn duplicate_ids_paths_oversized_declarations_and_external_symlinks_are_bounded() {
            let root = tempdir().unwrap();
            let first = root.path().join("first");
            let second = root.path().join("second");
            let cancel = AtomicBool::new(false);
            let mut transaction = DownloadTransaction::new("bounds".into());

            let mut oversized = command(
                KittyFileTransferAction::File,
                "large",
                Some(&first),
                KittyFileTransferType::Regular,
                b"",
            );
            oversized.size = Some((MAX_KITTY_DOWNLOAD_BYTES + 1) as i64);
            assert!(transaction.handle(oversized, &cancel).is_err());
            let mut transaction = DownloadTransaction::new("duplicates".into());

            let mut file = command(
                KittyFileTransferAction::File,
                "f",
                Some(&first),
                KittyFileTransferType::Regular,
                b"one",
            );
            file.data = Box::default();
            transaction.handle(file.clone(), &cancel).unwrap();
            assert!(transaction.handle(file, &cancel).is_err());

            let mut duplicate_path = command(
                KittyFileTransferAction::File,
                "other",
                Some(&first),
                KittyFileTransferType::Regular,
                b"one",
            );
            duplicate_path.data = Box::default();
            assert!(transaction.handle(duplicate_path, &cancel).is_err());

            transaction
                .handle(
                    command(
                        KittyFileTransferAction::EndData,
                        "f",
                        None,
                        KittyFileTransferType::Regular,
                        b"one",
                    ),
                    &cancel,
                )
                .unwrap();
            let mut link = command(
                KittyFileTransferAction::File,
                "s",
                Some(&second),
                KittyFileTransferType::Symlink,
                b"",
            );
            link.size = Some(0);
            transaction.handle(link, &cancel).unwrap();
            transaction
                .handle(
                    command(
                        KittyFileTransferAction::EndData,
                        "s",
                        None,
                        KittyFileTransferType::Symlink,
                        b"path:../outside-target",
                    ),
                    &cancel,
                )
                .unwrap();
            transaction
                .handle(
                    command(
                        KittyFileTransferAction::Finish,
                        "",
                        None,
                        KittyFileTransferType::Regular,
                        b"",
                    ),
                    &cancel,
                )
                .unwrap();
            assert_eq!(
                fs::read_link(second).unwrap(),
                PathBuf::from("../outside-target")
            );
        }

        #[test]
        #[cfg(target_os = "linux")]
        fn fifo_symlink_commit_does_not_wait_for_a_target_writer() {
            use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};

            let root = tempdir().unwrap();
            let fifo = root.path().join("pipe");
            let link = root.path().join("link");
            let fifo_name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
            // SAFETY: fifo_name is a live NUL-terminated path in a private tempdir.
            assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
            let before = fs::symlink_metadata(&fifo).unwrap();
            let worker_link = link.clone();
            let (sender, receiver) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                let cancel = AtomicBool::new(false);
                let mut transaction = DownloadTransaction::new("session".into());
                transaction
                    .handle(
                        command(
                            KittyFileTransferAction::File,
                            "s",
                            Some(&worker_link),
                            KittyFileTransferType::Symlink,
                            b"",
                        ),
                        &cancel,
                    )
                    .unwrap();
                transaction
                    .handle(
                        command(
                            KittyFileTransferAction::EndData,
                            "s",
                            None,
                            KittyFileTransferType::Symlink,
                            b"path:pipe",
                        ),
                        &cancel,
                    )
                    .unwrap();
                let result = transaction.handle(
                    command(
                        KittyFileTransferAction::Finish,
                        "",
                        None,
                        KittyFileTransferType::Regular,
                        b"",
                    ),
                    &cancel,
                );
                sender.send(result).unwrap();
            });
            let observed = receiver.recv_timeout(Duration::from_secs(2));
            let completed_without_writer = observed.is_ok();
            // Rescue the old blocking implementation before asserting, so a
            // failing regression never strands its worker or FIFO endpoint.
            let result = match observed {
                Ok(result) => result,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    let _rescue = fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .custom_flags(libc::O_NONBLOCK)
                        .open(&fifo)
                        .unwrap();
                    receiver.recv_timeout(Duration::from_secs(2)).unwrap()
                }
                Err(error) => panic!("transfer worker disconnected: {error}"),
            };
            worker.join().unwrap();
            result.unwrap();
            assert!(
                completed_without_writer,
                "symlink bookkeeping waited for a FIFO writer"
            );
            assert_eq!(fs::read_link(&link).unwrap(), PathBuf::from("pipe"));
            let after = fs::symlink_metadata(&fifo).unwrap();
            assert!(after.file_type().is_fifo());
            assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
            assert_eq!(
                (before.mtime(), before.mtime_nsec()),
                (after.mtime(), after.mtime_nsec())
            );
            let no_reader = fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&fifo)
                .unwrap_err();
            assert_eq!(no_reader.raw_os_error(), Some(libc::ENXIO));
        }

        #[test]
        #[cfg(target_os = "linux")]
        fn rollback_identity_guard_pins_the_symlink_not_its_target() {
            let root = tempdir().unwrap();
            let target = root.path().join("target");
            let link = root.path().join("link");
            fs::write(&target, b"keep").unwrap();
            std::os::unix::fs::symlink(&target, &link).unwrap();
            let mut created = Vec::new();
            record_created(&link, CreatedKind::Symlink, &mut created).unwrap();
            let guard = created[0]
                ._identity_guard
                .as_ref()
                .expect("pin the symlink inode");
            let pinned = guard.metadata().unwrap();
            assert!(pinned.file_type().is_symlink());
            assert_eq!(
                (pinned.dev(), pinned.ino()),
                (created[0].device, created[0].inode)
            );
            // Unlink/recreate cannot reuse the still-pinned original inode.
            fs::remove_file(&link).unwrap();
            std::os::unix::fs::symlink("missing", &link).unwrap();
            rollback_paths(&created);
            assert_eq!(fs::read_link(&link).unwrap(), PathBuf::from("missing"));
            assert_eq!(fs::read(&target).unwrap(), b"keep");
            let mut replacement = Vec::new();
            record_created(&link, CreatedKind::Symlink, &mut replacement).unwrap();
            assert!(
                replacement[0]._identity_guard.is_some(),
                "pin dangling links too"
            );
            rollback_paths(&replacement);
            assert!(fs::symlink_metadata(&link).is_err());
            assert_eq!(fs::read(&target).unwrap(), b"keep");
        }

        #[test]
        fn failed_created_path_inspection_never_deletes_a_replacement() {
            for kind in [
                CreatedKind::File,
                CreatedKind::Directory,
                CreatedKind::Symlink,
            ] {
                let root = tempdir().unwrap();
                let path = root.path().join("entry");
                let moved = root.path().join("original");
                let target = root.path().join("target");
                fs::write(&target, b"keep target").unwrap();
                let create = |path: &Path| match kind {
                    CreatedKind::File => fs::write(path, b"keep file"),
                    CreatedKind::Directory => fs::create_dir(path),
                    CreatedKind::Symlink => std::os::unix::fs::symlink(&target, path),
                };
                create(&path).unwrap();
                let mut created = Vec::new();
                let error = record_created_with(&path, kind, &mut created, |observed| {
                    // Model a failed metadata observation while another actor
                    // moves the original and replaces its pathname.
                    fs::rename(observed, &moved).unwrap();
                    create(observed).unwrap();
                    Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "injected inspection failure",
                    ))
                })
                .unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::Interrupted);
                assert!(created.is_empty());
                assert!(
                    fs::symlink_metadata(&path).is_ok(),
                    "replacement was deleted"
                );
                assert!(fs::symlink_metadata(&moved).is_ok(), "original was deleted");
                assert_eq!(fs::read(&target).unwrap(), b"keep target");
                rollback_paths(&created);
                assert!(fs::symlink_metadata(&path).is_ok());
            }
        }

        #[test]
        fn created_path_type_mismatches_never_gain_rollback_authority() {
            let kinds = [
                CreatedKind::File,
                CreatedKind::Directory,
                CreatedKind::Symlink,
            ];
            for actual in kinds {
                let root = tempdir().unwrap();
                let path = root.path().join("entry");
                let target = root.path().join("target");
                fs::write(&target, b"keep target").unwrap();
                match actual {
                    CreatedKind::File => fs::write(&path, b"keep file").unwrap(),
                    CreatedKind::Directory => fs::create_dir(&path).unwrap(),
                    CreatedKind::Symlink => std::os::unix::fs::symlink(&target, &path).unwrap(),
                }
                let before = fs::symlink_metadata(&path).unwrap();
                let target_before = fs::metadata(&target).unwrap();
                for expected in kinds.into_iter().filter(|kind| *kind != actual) {
                    let mut created = Vec::new();
                    let error = record_created(&path, expected, &mut created).unwrap_err();
                    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
                    assert!(created.is_empty());
                    rollback_paths(&created);
                    let after = fs::symlink_metadata(&path).unwrap();
                    assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
                    assert_eq!(after.permissions().mode(), before.permissions().mode());
                    let target_after = fs::metadata(&target).unwrap();
                    assert_eq!(
                        target_after.permissions().mode(),
                        target_before.permissions().mode()
                    );
                    assert_eq!(fs::read(&target).unwrap(), b"keep target");
                }
            }
        }

        #[test]
        fn rollback_does_not_remove_a_replaced_path() {
            let root = tempdir().unwrap();
            let path = root.path().join("published");
            fs::write(&path, b"ours").unwrap();
            let mut created = Vec::new();
            record_created(&path, CreatedKind::File, &mut created).unwrap();
            fs::remove_file(&path).unwrap();
            fs::write(&path, b"replacement").unwrap();

            rollback_paths(&created);

            assert_eq!(fs::read(path).unwrap(), b"replacement");
        }
    }
}
