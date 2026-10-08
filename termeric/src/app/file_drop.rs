//! Files dropped on the window: quoting a local path into the terminal, or
//! confirming and uploading it to a live-domain pane.

use super::*;
use crate::ui::info_overlay::push_key_choice;
use anyhow::Context as _;

#[derive(Debug, Clone)]
pub(super) struct PendingFileUpload {
    pub(super) request_id: u64,
    pub(super) source: PathBuf,
    pub(super) authority: crate::domain::DomainAuthority,
    pub(super) domain_id: String,
    pub(super) pane_id: u32,
    pub(super) name: String,
    pub(super) source_kind: FileUploadSourceKind,
    pub(super) source_identity: (u64, u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FileUploadSourceKind {
    File { bytes: u64 },
    Directory,
}

#[derive(Debug, Clone)]
pub(crate) struct FileUploadOutcome {
    pub(super) path: String,
    pub(super) bytes: u64,
    pub(super) entries: Option<u32>,
    pub(super) insertion_error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum FileUploadCompletion {
    Uploaded(FileUploadOutcome),
    Cancelled { bytes_received: u64 },
}

#[derive(Debug)]
pub(super) struct ActiveFileUpload {
    pub(super) request_id: u64,
    pub(super) name: String,
    pub(super) bytes_sent: u64,
    pub(super) total_bytes: Option<u64>,
    pub(super) cancellation: Arc<AtomicBool>,
    pub(super) cancel_requested: bool,
    pub(super) progress_toast_id: u64,
}

const MAX_DROPPED_PATH_BYTES: usize = 4_096;
const FILE_UPLOAD_PROGRESS_EVENT_BYTES: u64 = 1024 * 1024;
const UPLOAD_CHOICE: &str = "file-upload-confirm";
const CANCEL_UPLOAD_CHOICE: &str = "file-upload-cancel";

enum DroppedPathAction {
    InsertLocal(String),
    ConfirmUpload {
        source: PathBuf,
        authority: crate::domain::DomainAuthority,
        domain_id: String,
        pane_id: u32,
        name: String,
        source_kind: FileUploadSourceKind,
        source_identity: (u64, u64),
    },
}

struct ValidatedUploadSource {
    name: String,
    source_kind: FileUploadSourceKind,
    identity: (u64, u64),
}

fn quote_local_drop_path(path: &std::path::Path) -> std::result::Result<String, String> {
    if !path.is_absolute() {
        return Err("the compositor supplied a non-absolute path".into());
    }
    let text = path
        .to_str()
        .ok_or_else(|| "non-UTF-8 paths require manual shell entry".to_string())?;
    if text.len() > MAX_DROPPED_PATH_BYTES {
        return Err(format!(
            "path exceeds the {MAX_DROPPED_PATH_BYTES}-byte insertion limit"
        ));
    }
    if text.chars().any(unsafe_drop_path_character) {
        return Err(
            "path contains control, line-separator, or bidirectional formatting text".into(),
        );
    }
    quote_posix_path_text(text).map_err(|error| format!("{error:#}"))
}

fn quote_posix_path_text(text: &str) -> anyhow::Result<String> {
    if !std::path::Path::new(text).is_absolute()
        || text.len() > MAX_DROPPED_PATH_BYTES + crate::domain::protocol::MAX_UPLOAD_NAME_BYTES + 1
        || text.chars().any(unsafe_drop_path_character)
    {
        anyhow::bail!("destination path is not safe absolute UTF-8 text");
    }
    Ok(format!("'{}' ", text.replace('\'', "'\\''")))
}

fn validate_upload_source(
    path: &std::path::Path,
) -> std::result::Result<ValidatedUploadSource, String> {
    if !path.is_absolute() {
        return Err("the compositor supplied a non-absolute upload source".into());
    }
    let text = path
        .to_str()
        .ok_or_else(|| "upload sources require a UTF-8 path and file name".to_string())?;
    if text.len() > MAX_DROPPED_PATH_BYTES || text.chars().any(unsafe_drop_path_character) {
        return Err("upload source path is too long or contains unsafe display text".into());
    }
    let name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| "upload source requires a UTF-8 file name".to_string())?
        .to_string();
    crate::domain::protocol::validate_upload_name(&name).map_err(|error| format!("{error:#}"))?;
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect upload source: {error}"))?;
    if metadata.file_type().is_symlink() {
        return Err("symbolic-link upload sources are not supported".into());
    }
    let source_kind = if metadata.is_file() {
        if metadata.len() > crate::domain::protocol::MAX_FILE_UPLOAD_BYTES {
            return Err(format!(
                "file exceeds the {} MiB upload limit",
                crate::domain::protocol::MAX_FILE_UPLOAD_BYTES / (1024 * 1024)
            ));
        }
        FileUploadSourceKind::File {
            bytes: metadata.len(),
        }
    } else if metadata.is_dir() {
        FileUploadSourceKind::Directory
    } else {
        return Err("upload source must be one regular file or real directory".into());
    };
    use std::os::unix::fs::MetadataExt;
    Ok(ValidatedUploadSource {
        name,
        source_kind,
        identity: (metadata.dev(), metadata.ino()),
    })
}

/// What a finished upload sent: a file's size, or a directory's entry count
/// and archive size.
fn upload_summary(bytes: u64, entries: Option<u32>) -> String {
    match entries {
        Some(entries) => format!("directory with {entries} entries ({bytes} archive bytes)"),
        None => format!("{bytes} bytes"),
    }
}

fn unsafe_drop_path_character(character: char) -> bool {
    crate::safe_text::is_unsafe_display_char(character)
}

impl App {
    /// Validate and quote one compositor-provided local path without reading
    /// file contents or crossing a terminal/domain trust boundary.
    #[cfg(test)]
    pub(super) fn dropped_path_payload(
        &self,
        path: &std::path::Path,
    ) -> std::result::Result<String, String> {
        match self.dropped_path_action(path)? {
            DroppedPathAction::InsertLocal(payload) => Ok(payload),
            DroppedPathAction::ConfirmUpload { .. } => {
                Err("A live-domain file drop requires explicit upload confirmation".into())
            }
        }
    }

    pub(super) fn dropped_path_allowed(&self, path: &std::path::Path) -> bool {
        self.dropped_path_action(path).is_ok()
    }

    fn dropped_path_action(
        &self,
        path: &std::path::Path,
    ) -> std::result::Result<DroppedPathAction, String> {
        if self.file_drop_input_blocked() {
            return Err("Close the active panel or input mode before dropping a file".into());
        }
        if !matches!(self.broadcast, super::BroadcastState::Inactive) {
            return Err("File paths are not inserted while pane broadcast is active".into());
        }
        let tab = self
            .tabs
            .as_ref()
            .and_then(crate::tabs::TabManager::active_tab)
            .filter(|tab| !tab.closed && !tab.is_closing())
            .ok_or_else(|| "No live terminal is available for the dropped path".to_string())?;
        if tab.focused_pane_read_only() {
            return Err("The focused pane is read-only".into());
        }
        if let Some((domain_id, pane_id)) = tab.live_domain_focus_route() {
            if self.active_file_upload.is_some() || self.pending_file_upload().is_some() {
                return Err("Another file upload is already pending for this window".into());
            }
            let authority = self
                .live_domain_authority
                .clone()
                .ok_or_else(|| "Live-domain upload authority is unavailable".to_string())?;
            let source = validate_upload_source(path)?;
            return Ok(DroppedPathAction::ConfirmUpload {
                source: path.to_path_buf(),
                authority,
                domain_id: domain_id.to_string(),
                pane_id,
                name: source.name,
                source_kind: source.source_kind,
                source_identity: source.identity,
            });
        }
        if !tab.pane_accepts_dropped_paths(tab.focused_terminal_identity().pane_id) {
            return Err(
                "File paths are not dropped into serial terminals, to keep this computer's paths off the device"
                    .into(),
            );
        }
        quote_local_drop_path(path).map(DroppedPathAction::InsertLocal)
    }

    pub(super) fn handle_dropped_path(&mut self, path: &std::path::Path) -> bool {
        let action = match self.dropped_path_action(path) {
            Ok(action) => action,
            Err(reason) => {
                self.toasts
                    .warning(format!("File was not dropped: {reason}"));
                return false;
            }
        };
        match action {
            DroppedPathAction::InsertLocal(payload) => {
                if self.write_paste_text(&payload) {
                    self.toasts
                        .success("Inserted quoted local path; press Enter to use it");
                    true
                } else {
                    self.toasts
                        .error("Dropped path could not be written to the focused terminal");
                    false
                }
            }
            DroppedPathAction::ConfirmUpload {
                source,
                authority,
                domain_id,
                pane_id,
                name,
                source_kind,
                source_identity,
            } => {
                let (noun, size) = match source_kind {
                    FileUploadSourceKind::File { bytes } => {
                        ("file", format!("Size: {bytes} bytes"))
                    }
                    FileUploadSourceKind::Directory => (
                        "directory",
                        format!(
                            "Size: calculated after confirmation; maximum archive {} MiB",
                            crate::domain::protocol::MAX_FILE_UPLOAD_BYTES / (1024 * 1024)
                        ),
                    ),
                };
                let mut lines = vec![
                    format!("Upload this local {noun} to the focused live-domain pane?"),
                    String::new(),
                    format!("Source: {}", source.display()),
                    format!("Destination: domain {domain_id}, pane {pane_id}, cwd/{name}"),
                    size,
                    String::new(),
                    "The destination must not exist. Termeric will not overwrite it.".into(),
                    "After upload, the quoted remote path is inserted without Enter.".into(),
                    String::new(),
                ];
                let mut choices = Vec::new();
                push_key_choice(&mut lines, &mut choices, UPLOAD_CHOICE, 'u', "Upload");
                push_key_choice(
                    &mut lines,
                    &mut choices,
                    CANCEL_UPLOAD_CHOICE,
                    'c',
                    "Cancel",
                );
                let request_id =
                    self.info_overlay
                        .show_choices("Confirm live-domain upload", lines, choices);
                self.overlay_flow = Some(OverlayFlow::FileUpload(PendingFileUpload {
                    request_id,
                    source,
                    authority,
                    domain_id,
                    pane_id,
                    name,
                    source_kind,
                    source_identity,
                }));
                true
            }
        }
    }

    /// Answer the upload confirmation. Returns whether the choice was its.
    pub(super) fn handle_file_upload_choice(&mut self, request_id: u64, choice_id: &str) -> bool {
        let upload = match self.overlay_flow.take() {
            Some(OverlayFlow::FileUpload(upload))
                if upload.request_id == request_id
                    && matches!(choice_id, UPLOAD_CHOICE | CANCEL_UPLOAD_CHOICE) =>
            {
                upload
            }
            other => {
                self.overlay_flow = other;
                return false;
            }
        };
        self.info_overlay.dismiss(request_id);
        if choice_id == UPLOAD_CHOICE {
            self.start_file_upload(upload);
        }
        true
    }

    fn start_file_upload(&mut self, upload: PendingFileUpload) {
        let request_id = upload.request_id;
        if self.window.is_none() {
            self.toasts
                .error("Live-domain uploads require an attached native window");
            return;
        }
        let window_key = self.window_key();
        let Some(proxy) = super::EVENT_LOOP_PROXY.get().cloned() else {
            self.toasts
                .error("Live-domain upload event channel is unavailable");
            return;
        };
        let cancellation = Arc::new(AtomicBool::new(false));
        let initial_total = match upload.source_kind {
            FileUploadSourceKind::File { bytes } => Some(bytes),
            FileUploadSourceKind::Directory => None,
        };
        let initial_message = match initial_total {
            Some(bytes) => format!(
                "Uploading {}: 0% (0/{bytes} bytes). Commands → Cancel Upload",
                upload.name
            ),
            None => format!(
                "Preparing directory {}. Commands → Cancel Upload",
                upload.name
            ),
        };
        let progress_toast_id = self.toasts.persistent_info(initial_message);
        self.active_file_upload = Some(ActiveFileUpload {
            request_id,
            name: upload.name.clone(),
            bytes_sent: 0,
            total_bytes: initial_total,
            cancellation: Arc::clone(&cancellation),
            cancel_requested: false,
            progress_toast_id,
        });
        let spawn = std::thread::Builder::new()
            .name("termeric-domain-upload".into())
            .spawn(move || {
                let result = (|| -> anyhow::Result<FileUploadCompletion> {
                    let client = upload.authority.client()?;
                    let metadata = std::fs::symlink_metadata(&upload.source)
                        .context("revalidate confirmed upload source")?;
                    if metadata.file_type().is_symlink()
                        || !match upload.source_kind {
                            FileUploadSourceKind::File { bytes } => {
                                metadata.is_file() && metadata.len() == bytes
                            }
                            FileUploadSourceKind::Directory => metadata.is_dir(),
                        }
                    {
                        anyhow::bail!("upload source changed after confirmation");
                    }
                    {
                        use std::os::unix::fs::MetadataExt;
                        if (metadata.dev(), metadata.ino()) != upload.source_identity {
                            anyhow::bail!("upload source identity changed after confirmation");
                        }
                    }
                    let progress_proxy = proxy.clone();
                    let mut last_reported = 0_u64;
                    let mut announced_total = false;
                    let progress = |bytes_sent: u64, total_bytes: u64| {
                        if !announced_total
                            || bytes_sent == total_bytes
                            || bytes_sent.saturating_sub(last_reported)
                                >= FILE_UPLOAD_PROGRESS_EVENT_BYTES
                        {
                            announced_total = true;
                            last_reported = bytes_sent;
                            let _ = progress_proxy.send_event(AppEvent::WindowResult {
                                window: window_key,
                                result: WindowResult::FileUploadProgress {
                                    request_id,
                                    bytes_sent,
                                    total_bytes,
                                },
                            });
                        }
                    };
                    let response = client.upload(
                        upload.domain_id.clone(),
                        upload.pane_id,
                        &upload.source,
                        crate::domain::UploadOptions {
                            name: Some(upload.name),
                            expected_identity: Some(upload.source_identity),
                            cancelled: &cancellation,
                            progress,
                        },
                    )?;
                    let (path, bytes, entries) = match response {
                        crate::domain::protocol::Response::FileUploaded { path, bytes } => {
                            (path, bytes, None)
                        }
                        crate::domain::protocol::Response::DirectoryUploaded {
                            path,
                            bytes,
                            entries,
                        } => (path, bytes, Some(entries)),
                        crate::domain::protocol::Response::UploadCancelled { bytes_received } => {
                            return Ok(FileUploadCompletion::Cancelled { bytes_received });
                        }
                        _ => anyhow::bail!("domain server returned an invalid upload response"),
                    };
                    let insertion_error = (|| -> anyhow::Result<()> {
                        let quoted = quote_posix_path_text(&path)?;
                        client
                            .request(crate::domain::protocol::Request::SendPaste {
                                domain: upload.domain_id,
                                pane: upload.pane_id,
                                text: quoted,
                            })
                            .and_then(|response| match response {
                                crate::domain::protocol::Response::Ok { .. } => Ok(()),
                                _ => anyhow::bail!(
                                    "domain server returned an invalid input response"
                                ),
                            })
                    })()
                    .err()
                    .map(|error| format!("{error:#}"));
                    Ok(FileUploadCompletion::Uploaded(FileUploadOutcome {
                        path,
                        bytes,
                        entries,
                        insertion_error,
                    }))
                })()
                .map_err(|error| format!("{error:#}"));
                let _ = proxy.send_event(AppEvent::WindowResult {
                    window: window_key,
                    result: WindowResult::FileUploadCompleted { request_id, result },
                });
            });
        if let Err(error) = spawn {
            if let Some(active) = self.active_file_upload.take() {
                self.toasts.dismiss(active.progress_toast_id);
            }
            self.toasts
                .error(format!("Could not start live-domain upload: {error}"));
        }
    }

    pub(super) fn handle_file_upload_completed(
        &mut self,
        request_id: u64,
        result: std::result::Result<FileUploadCompletion, String>,
    ) {
        if self
            .active_file_upload
            .as_ref()
            .is_none_or(|active| active.request_id != request_id)
        {
            return;
        }
        let active = self
            .active_file_upload
            .take()
            .expect("checked active upload");
        self.toasts.dismiss(active.progress_toast_id);
        match result {
            Ok(FileUploadCompletion::Uploaded(outcome)) if outcome.insertion_error.is_none() => {
                let summary = upload_summary(outcome.bytes, outcome.entries);
                self.toasts.success(format!(
                    "Uploaded {summary} and inserted {} without Enter",
                    outcome.path
                ));
            }
            Ok(FileUploadCompletion::Uploaded(outcome)) => {
                let summary = upload_summary(outcome.bytes, outcome.entries);
                self.toasts.warning(format!(
                    "Uploaded {summary} to {}, but could not insert the path: {}",
                    outcome.path,
                    outcome.insertion_error.unwrap_or_default()
                ));
            }
            Ok(FileUploadCompletion::Cancelled { bytes_received }) => {
                let progress = active.total_bytes.map_or_else(
                    || format!("after {bytes_received} bytes"),
                    |total| format!("after {bytes_received}/{total} bytes"),
                );
                self.toasts.info(format!(
                    "Cancelled upload of {} {progress}; no destination was published",
                    active.name
                ));
            }
            Err(error) => self
                .toasts
                .error(format!("Live-domain upload failed: {error}")),
        }
    }

    pub(super) fn handle_file_upload_progress(
        &mut self,
        request_id: u64,
        bytes_sent: u64,
        total_bytes: u64,
    ) {
        let Some(active) = self.active_file_upload.as_mut() else {
            return;
        };
        if active.request_id != request_id
            || active
                .total_bytes
                .is_some_and(|expected| expected != total_bytes)
            || bytes_sent < active.bytes_sent
            || bytes_sent > total_bytes
        {
            return;
        }
        active.total_bytes = Some(total_bytes);
        active.bytes_sent = bytes_sent;
        if !active.cancel_requested {
            let percent = bytes_sent
                .saturating_mul(100)
                .checked_div(total_bytes)
                .unwrap_or(100);
            self.toasts.update_persistent(
                active.progress_toast_id,
                format!(
                    "Uploading {}: {percent}% ({bytes_sent}/{total_bytes} bytes). Commands → Cancel Upload",
                    active.name
                ),
            );
        }
        self.request_redraw(RedrawReason::BackgroundTask);
    }

    pub(super) fn cancel_file_upload(&mut self) {
        let Some(active) = self.active_file_upload.as_mut() else {
            self.toasts.warning("No live-domain upload is active");
            return;
        };
        if active.cancel_requested {
            self.toasts
                .info("Live-domain upload cancellation is already pending");
            return;
        }
        active.cancel_requested = true;
        active.cancellation.store(true, Ordering::Release);
        self.toasts.update_persistent(
            active.progress_toast_id,
            match active.total_bytes {
                Some(total) => format!(
                    "Cancelling upload of {} after {}/{} bytes…",
                    active.name, active.bytes_sent, total
                ),
                None => format!("Cancelling preparation of {}…", active.name),
            },
        );
        self.request_redraw(RedrawReason::Action);
    }

    pub(super) fn file_drop_input_blocked(&self) -> bool {
        self.top_modal_layer().is_some()
            || self.search.is_active()
            || self.copy_mode.is_active()
            || self.tab_rename.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_paths_are_posix_quoted_without_adding_enter() {
        assert_eq!(
            quote_local_drop_path(std::path::Path::new("/tmp/a b'c")).unwrap(),
            "'/tmp/a b'\\''c' "
        );
    }

    #[test]
    fn dropped_paths_reject_ambiguous_or_unsafe_text() {
        assert!(quote_local_drop_path(std::path::Path::new("relative/file")).is_err());
        assert!(quote_local_drop_path(std::path::Path::new("/tmp/line\nbreak")).is_err());
        assert!(quote_local_drop_path(std::path::Path::new("/tmp/escape\u{1b}")).is_err());
        assert!(quote_local_drop_path(std::path::Path::new("/tmp/reverse\u{202e}txt")).is_err());
        assert!(
            quote_local_drop_path(std::path::Path::new(&format!(
                "/{}",
                "x".repeat(MAX_DROPPED_PATH_BYTES)
            )))
            .is_err()
        );
    }

    #[test]
    fn dropped_paths_reject_non_utf8_names() {
        use std::os::unix::ffi::OsStringExt;

        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 0xff]));
        assert!(quote_local_drop_path(&path).is_err());
    }
}
