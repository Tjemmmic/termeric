//! Consent-gated Kitty OSC 5113 file transfer: downloads written onto the
//! terminal host, and host files read back to the terminal (uploads). This
//! file holds the App side (consent, routing, progress); the workers and the
//! download transaction live in `download` and `upload`.

use super::{App, AppEvent, EVENT_LOOP_PROXY};
use crate::tabs::{TerminalEventOrigin, TerminalIdentity};
use crate::ui::info_overlay::InfoOverlayChoice;
use std::collections::HashMap;
use std::path::PathBuf;
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
const WORKER_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const WORKER_QUEUE_CAPACITY: usize = 1_024;
const UPLOAD_BATCH_CHUNKS: usize = 8;
const PENDING_UPLOAD_TIMEOUT: Duration = Duration::from_secs(60);

mod download;
mod upload;

#[cfg(test)]
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
        // Consent, session lookup, and worker callbacks retain the exact
        // originating terminal, never whichever one is focused later.
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
            self.cancel_kitty_transfer(&key, command.reply_uses_8bit_controls);
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
                &command.file_id,
                KittyFileTransferStatusCode::Invalid,
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
                &file_id,
                KittyFileTransferStatusCode::Busy,
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
        if !self.kitty_origin_accepts_host_paths(origin) {
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
        if !self.kitty_origin_accepts_host_paths(origin) {
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
            "The entry limit includes automatically created parent directories.".into(),
            String::new(),
            "The remote side chooses destination paths and symbolic-link targets.".into(),
            "Existing entries will not be replaced.".into(),
            "Only continue if you trust the remote computer and the command you ran.".into(),
            String::new(),
            "  A  Allow this transfer".into(),
            "  D  Deny".into(),
        ];
        let allow_line = lines.len() - 2;
        let deny_line = lines.len() - 1;
        let request_id = self.info_overlay.show_choices(
            "Allow remote file transfer?",
            lines,
            vec![
                InfoOverlayChoice::new(
                    "kitty-download-allow",
                    "Allow transfer",
                    allow_line,
                    Some('a'),
                ),
                InfoOverlayChoice::new(
                    "kitty-download-deny",
                    "Deny transfer",
                    deny_line,
                    Some('d'),
                ),
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

    fn kitty_origin_accepts_host_paths(&self, origin: TerminalIdentity) -> bool {
        self.kitty_transfer_origin(origin).is_some()
            && self
                .tabs
                .as_ref()
                .and_then(|tabs| tabs.tab_by_id(origin.tab_id))
                .is_some_and(|tab| tab.pane_accepts_host_paths(origin.pane_id))
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
        if !self.kitty_origin_accepts_host_paths(pending.key.origin) {
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
        if !self.kitty_origin_accepts_host_paths(pending.key.origin) {
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

    /// Cancel the transfer `key` names, in either direction. The terminal
    /// gets a canceled acknowledgement even for a pending request or an ID it
    /// no longer knows.
    fn cancel_kitty_transfer(&mut self, key: &TransferKey, reply_uses_8bit_controls: bool) {
        let canceled = Some(KittyFileTransferStatusCode::Canceled);
        if self
            .kitty_file_transfers
            .pending_upload
            .as_ref()
            .is_some_and(|pending| &pending.key == key)
        {
            self.deny_pending_kitty_upload(canceled, "");
            return;
        }
        if let Some(token) = self.kitty_file_transfers.upload_tokens.get(key).copied() {
            self.remove_active_kitty_upload(token);
        } else if self
            .kitty_file_transfers
            .pending
            .as_ref()
            .is_some_and(|pending| &pending.key == key)
        {
            self.deny_pending_kitty_download(canceled, "");
            return;
        } else if let Some(token) = self.kitty_file_transfers.tokens.get(key).copied() {
            self.remove_active_kitty_download(token);
        }
        self.write_kitty_status(
            key.origin,
            reply_uses_8bit_controls,
            transfer_status(&key.id, "", KittyFileTransferStatusCode::Canceled, ""),
        );
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

    fn fail_active_kitty_download(
        &mut self,
        token: u64,
        reply_uses_8bit_controls: bool,
        file_id: &str,
        code: KittyFileTransferStatusCode,
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
                    || self.kitty_transfer_origin(pending.key.origin).is_none()
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
                    || self.kitty_transfer_origin(pending.key.origin).is_none()
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
                self.kitty_transfer_origin(active.key.origin)
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
                self.kitty_transfer_origin(active.key.origin)
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
    // Only uploads show the requested name (in the consent dialog).
    if name.chars().any(crate::safe_text::is_unsafe_display_char) {
        return Err("requested path contains unsafe display controls".into());
    }
    resolve_home_relative_path(name)
}

/// Resolve a path a Kitty transfer names: `~`, `~/…`, and relative paths are
/// under the home directory; absolute paths stay as they are. The result is an
/// absolute, bounded path with no empty, `.`, `..`, NUL, or oversized
/// component, and never a filesystem root.
fn resolve_home_relative_path(name: &str) -> Result<PathBuf, String> {
    use std::path::Component;

    if name.is_empty() || name.len() > termeric_core::MAX_KITTY_TRANSFER_PATH_BYTES {
        return Err("empty or oversized path".into());
    }
    let home = directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .ok_or_else(|| "home directory is unavailable".to_string())?;
    let (base, rest) = if name == "~" {
        (home, "")
    } else if let Some(rest) = name.strip_prefix("~/") {
        (home, rest)
    } else if let Some(rest) = name.strip_prefix('/') {
        (PathBuf::from("/"), rest)
    } else {
        (home, name)
    };
    if name != "~"
        && (rest.is_empty()
            || rest.split('/').any(|component| {
                component.is_empty()
                    || matches!(component, "." | "..")
                    || component.contains('\0')
                    || component.len() > 255
            }))
    {
        return Err("path contains empty, traversal, or oversized components".into());
    }
    let path = if rest.is_empty() {
        base
    } else {
        base.join(rest)
    };
    if !path.is_absolute()
        || path.as_os_str().len() > termeric_core::MAX_KITTY_TRANSFER_PATH_BYTES
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err("path must be an absolute bounded POSIX path".into());
    }
    if path.parent().is_none() {
        return Err("path cannot be a filesystem root".into());
    }
    Ok(path)
}

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
            upload::run_upload_worker(
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
            download::run_worker(
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
