//! Terminal-event bridges for clipboard, notifications, links, and PTY input.

const MAX_DROPPED_PATH_BYTES: usize = 4_096;
const FILE_UPLOAD_PROGRESS_EVENT_BYTES: u64 = 1024 * 1024;

use super::*;
use anyhow::Context as _;

enum DroppedPathAction {
    InsertLocal(String),
    ConfirmUpload {
        source: PathBuf,
        authority: crate::domain::DomainAuthority,
        domain_id: String,
        pane_id: u32,
        name: String,
        source_kind: FileUploadSourceKind,
        source_identity: Option<(u64, u64)>,
    },
}

struct ValidatedUploadSource {
    name: String,
    source_kind: FileUploadSourceKind,
    identity: Option<(u64, u64)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NotificationOriginPresentation {
    keyboard_focused: bool,
    visible: bool,
}

impl App {
    fn notification_origin_presentation(
        &self,
        origin: crate::tabs::TerminalEventOrigin,
    ) -> Option<NotificationOriginPresentation> {
        let tabs = self.tabs.as_ref()?;
        tabs.terminal_for_origin(origin)?;
        let tab_id = match origin {
            crate::tabs::TerminalEventOrigin::Tab { tab_id }
            | crate::tabs::TerminalEventOrigin::Pane { tab_id, .. } => tab_id,
        };
        let tab = tabs.tab_by_id(tab_id)?;
        if tab.closed || tab.is_closing() {
            return None;
        }
        let active = tabs.active_tab_id() == Some(tab_id);
        let (terminal_focused, terminal_visible) = match origin {
            crate::tabs::TerminalEventOrigin::Tab { .. } => (true, true),
            crate::tabs::TerminalEventOrigin::Pane { pane_id, .. } => {
                let panes = tab.pane_manager.as_ref()?;
                let pane = panes.get_pane(pane_id)?;
                if pane.closed {
                    return None;
                }
                (
                    panes.focused_id() == pane_id,
                    panes.zoomed_id().is_none_or(|zoomed| zoomed == pane_id),
                )
            }
        };
        let native_interactive = self.window_focused && !self.window_occluded;
        Some(NotificationOriginPresentation {
            keyboard_focused: native_interactive && active && terminal_focused,
            visible: !self.window_occluded && active && terminal_visible,
        })
    }

    pub(super) fn should_present_terminal_notification(
        &self,
        origin: crate::tabs::TerminalEventOrigin,
        occasion: termeric_core::NotificationOccasion,
    ) -> bool {
        let Some(presentation) = self.notification_origin_presentation(origin) else {
            return false;
        };
        match occasion {
            termeric_core::NotificationOccasion::Always => true,
            termeric_core::NotificationOccasion::Unfocused => !presentation.keyboard_focused,
            termeric_core::NotificationOccasion::Invisible => {
                !presentation.keyboard_focused && !presentation.visible
            }
        }
    }

    /// Restore the exact terminal that emitted an activated notification, then
    /// ask the native window system to activate this owning window. The origin
    /// is validated before any tab or pane mutation; stale origins never fall
    /// back to whichever terminal happens to be focused now.
    pub(super) fn activate_notification_origin(
        &mut self,
        identity: crate::tabs::TerminalIdentity,
    ) -> bool {
        let Some(origin) = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.origin_for_pane(identity.tab_id, identity.pane_id))
        else {
            log::debug!("Ignored notification activation for stale owner {identity:?}");
            return false;
        };

        let selected = match origin {
            crate::tabs::TerminalEventOrigin::Tab { tab_id } => self
                .select_control_tab(tab_id as u64)
                .map(|changed| (changed, false)),
            crate::tabs::TerminalEventOrigin::Pane { tab_id, pane_id } => {
                self.focus_control_pane(tab_id as u64, pane_id.0)
            }
        };
        if let Err(error) = selected {
            log::debug!("Ignored notification activation for stale origin {origin:?}: {error}");
            return false;
        }

        // Selection may run app callbacks. Revalidate before native activation.
        if self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.origin_for_pane(identity.tab_id, identity.pane_id))
            .is_none()
        {
            return false;
        }

        let Some(window) = self.window.as_ref() else {
            // Headless tests still exercise exact-origin selection.
            return true;
        };
        if self.launch.quick_access && !self.quick_access_visible {
            self.quick_access_visible = true;
            self.window_occluded = false;
            window.set_visible(true);
        }
        window.set_minimized(false);

        #[cfg(target_os = "linux")]
        {
            use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
            let is_wayland = window
                .window_handle()
                .ok()
                .is_some_and(|handle| matches!(handle.as_raw(), RawWindowHandle::Wayland(_)));
            if is_wayland {
                // Winit owns the Wayland connection and safely performs an
                // xdg-activation request for this existing surface.
                window.request_user_attention(Some(winit::window::UserAttentionType::Critical));
            } else {
                window.focus_window();
            }
        }
        #[cfg(not(target_os = "linux"))]
        window.focus_window();

        window.request_redraw();
        true
    }

    /// Validate and quote one compositor-provided local path without reading
    /// file contents or crossing a terminal/domain/plugin trust boundary.
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
            if self.active_file_upload.is_some() || self.pending_file_upload.is_some() {
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
        match &tab.tab_type {
            crate::tabs::TabType::Plugin { .. } => {
                return Err(
                    "File drops are disabled for plugin tabs to prevent accidental data disclosure"
                        .into(),
                );
            }
            crate::tabs::TabType::Serial { .. } => {
                return Err("File paths cannot be dropped into a serial terminal".into());
            }
            crate::tabs::TabType::Shell | crate::tabs::TabType::Profile { .. } => {}
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
                        "Size: calculated after confirmation; maximum archive 64 MiB".into(),
                    ),
                };
                let lines = vec![
                    format!("Upload this local {noun} to the focused live-domain pane?"),
                    String::new(),
                    format!("Source: {}", source.display()),
                    format!("Destination: domain {domain_id}, pane {pane_id}, cwd/{name}"),
                    size,
                    String::new(),
                    "The destination must not exist. Termeric will not overwrite it.".into(),
                    "After upload, the quoted remote path is inserted without Enter.".into(),
                    String::new(),
                    "  U  Upload".into(),
                    "  C  Cancel".into(),
                ];
                let request_id = self.info_overlay.show_choices(
                    "Confirm live-domain upload",
                    lines,
                    vec![
                        crate::ui::info_overlay::InfoOverlayChoice::new(
                            "file-upload-confirm",
                            "Upload",
                            9,
                            Some('u'),
                        ),
                        crate::ui::info_overlay::InfoOverlayChoice::new(
                            "file-upload-cancel",
                            "Cancel",
                            10,
                            Some('c'),
                        ),
                    ],
                );
                self.pending_file_upload = Some(PendingFileUpload {
                    request_id,
                    source,
                    authority,
                    domain_id,
                    pane_id,
                    name,
                    source_kind,
                    source_identity,
                });
                true
            }
        }
    }

    pub(super) fn start_file_upload(&mut self, upload: PendingFileUpload) {
        let request_id = upload.request_id;
        let Some(window_id) = self.window.as_ref().map(|window| window.id()) else {
            self.toasts
                .error("Live-domain uploads require an attached native window");
            return;
        };
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
                    #[cfg(unix)]
                    if let Some((device, inode)) = upload.source_identity {
                        use std::os::unix::fs::MetadataExt;
                        if metadata.dev() != device || metadata.ino() != inode {
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
                            let _ = progress_proxy.send_event(AppEvent::FileUploadProgress {
                                window_id,
                                request_id,
                                bytes_sent,
                                total_bytes,
                            });
                        }
                    };
                    let response = match upload.source_kind {
                        FileUploadSourceKind::File { .. } => {
                            #[cfg(unix)]
                            use std::os::unix::fs::OpenOptionsExt;
                            let mut options = std::fs::OpenOptions::new();
                            options.read(true);
                            #[cfg(unix)]
                            options.custom_flags(libc::O_NOFOLLOW);
                            let file = options.open(&upload.source).with_context(|| {
                                format!("open confirmed upload source {}", upload.source.display())
                            })?;
                            client.upload_open_file_with_control(
                                upload.domain_id.clone(),
                                upload.pane_id,
                                file,
                                upload.name,
                                &cancellation,
                                progress,
                            )?
                        }
                        FileUploadSourceKind::Directory => client.upload_directory_with_control(
                            upload.domain_id.clone(),
                            upload.pane_id,
                            &upload.source,
                            Some(upload.name),
                            upload.source_identity,
                            &cancellation,
                            progress,
                        )?,
                    };
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
                let _ = proxy.send_event(AppEvent::FileUploadCompleted {
                    window_id,
                    request_id,
                    result,
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
                let summary = outcome.entries.map_or_else(
                    || format!("{} bytes", outcome.bytes),
                    |entries| {
                        format!(
                            "directory with {entries} entries ({} archive bytes)",
                            outcome.bytes
                        )
                    },
                );
                self.toasts.success(format!(
                    "Uploaded {summary} and inserted {} without Enter",
                    outcome.path
                ));
            }
            Ok(FileUploadCompletion::Uploaded(outcome)) => {
                let summary = outcome.entries.map_or_else(
                    || format!("{} bytes", outcome.bytes),
                    |entries| {
                        format!(
                            "directory with {entries} entries ({} archive bytes)",
                            outcome.bytes
                        )
                    },
                );
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
        if let Some(window) = &self.window {
            window.request_redraw();
        }
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
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    pub(super) fn file_drop_input_blocked(&self) -> bool {
        self.guardrail.is_active()
            || self.info_overlay.is_active()
            || self.diff_review.is_active()
            || self.context_picker.is_active()
            || self.domain_picker.is_active()
            || self.unicode_picker.is_active()
            || self.command_palette.is_active()
            || self.shortcuts_panel.is_active()
            || self.settings_panel.is_active()
            || self.commands_panel.is_open()
            || self.session_panel.is_open()
            || self.workspace_menu.is_some()
            || self.search.is_active()
            || self.copy_mode.is_active()
            || self.tab_rename.is_some()
            || self.harvest.as_ref().is_some_and(|harvest| {
                harvest.sprout_menu.is_some() || harvest.cellar.is_expanded()
            })
    }

    pub(super) fn apply_guardrail_action(
        &mut self,
        action: crate::terminal::guardrails::GuardrailAction,
    ) {
        use crate::terminal::guardrails::GuardrailAction;
        match action {
            GuardrailAction::Confirm { request_id } => {
                let Some(writes) = self.guardrail.confirm(request_id) else {
                    return;
                };
                let focused_origin = self.active_terminal_origin();
                let result = self
                    .tabs
                    .as_mut()
                    .ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "originating terminal no longer exists",
                        )
                    })
                    .and_then(|tabs| {
                        // Validate the entire captured route before sending any
                        // member. A vanished pane must not produce a partial
                        // broadcast or fall back to the current focus.
                        let writes = writes
                            .into_iter()
                            .map(|write| {
                                let origin = write.resolve_origin(tabs).ok_or_else(|| {
                                    std::io::Error::new(
                                        std::io::ErrorKind::NotFound,
                                        "one or more originating terminals no longer exist",
                                    )
                                })?;
                                Ok((origin, write.data))
                            })
                            .collect::<std::io::Result<Vec<_>>>()?;
                        if writes
                            .iter()
                            .any(|(origin, _)| tabs.origin_read_only(*origin) != Some(false))
                        {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::PermissionDenied,
                                "one or more originating panes are read-only",
                            ));
                        }
                        let affects_focused_input = writes
                            .iter()
                            .any(|(origin, _)| Some(*origin) == focused_origin);
                        for (origin, data) in writes {
                            tabs.write_user_to_origin(origin, &data)?;
                        }
                        Ok(affects_focused_input)
                    });
                if let Err(error) = result {
                    log::error!("Guarded command was not sent: {error}");
                    self.toasts
                        .error(format!("Guarded command was not sent: {error}"));
                } else if matches!(result, Ok(true)) {
                    self.clear_terminal_input_tracking();
                    self.reset_cursor_blink();
                }
            }
            GuardrailAction::Cancel { request_id } => {
                self.guardrail.dismiss(request_id);
            }
            GuardrailAction::None => {}
        }
    }

    /// Validate immediately before a desktop launch. Event routes are mode-
    /// specific: obsolete tab/pane routes cannot follow a different survivor.
    pub(super) fn validate_hyperlink_origin(
        &self,
        origin: Option<crate::tabs::TerminalEventOrigin>,
    ) -> std::io::Result<()> {
        let live = origin.and_then(|origin| self.tabs.as_ref()?.identity_for_origin(origin));
        if live.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "originating terminal is no longer available for hyperlink activation",
            ));
        }
        Ok(())
    }

    /// All GUI hyperlink launch paths share this last side-effect gate. A
    /// directory argument may be a captured quick-select context, so do not
    /// replace it with metadata from a newly focused terminal.
    pub(super) fn open_hyperlink_for_origin(
        &self,
        origin: Option<crate::tabs::TerminalEventOrigin>,
        target: &str,
        directory: Option<&str>,
    ) -> std::io::Result<()> {
        self.validate_hyperlink_origin(origin)?;
        crate::terminal::url::UrlDetector::open_url_in_directory(
            target,
            directory.map(std::path::Path::new),
        )
    }

    /// Hyperlink actions use their own displayed owner's local report, never
    /// another focused pane, hidden live metadata, or process-cwd fallback.
    pub(super) fn hyperlink_open_directory(
        &self,
        origin: crate::tabs::TerminalEventOrigin,
    ) -> Option<String> {
        self.tabs
            .as_ref()?
            .terminal_for_origin(origin)
            .and_then(crate::tabs::presentation_working_directory)
    }

    /// Handle a terminal event.
    pub(super) fn handle_terminal_event(&mut self, routed: RoutedTerminalEvent) {
        let RoutedTerminalEvent { origin, event } = routed;
        match event {
            TerminalEvent::Bell => {
                if let Some(error) = self.bell.ring() {
                    self.toasts.warning(error);
                }
            }
            TerminalEvent::Notification(req) => {
                if !self.should_present_terminal_notification(origin, req.occasion) {
                    log::debug!(
                        "Suppressed OSC notification from {origin:?} for {:?} occasion",
                        req.occasion
                    );
                    return;
                }
                let Some(origin) = self
                    .tabs
                    .as_ref()
                    .and_then(|tabs| tabs.identity_for_origin(origin))
                else {
                    return;
                };
                if let Some(error) = self.notifications.notify_terminal(origin, req) {
                    self.toasts.warning(error);
                }
            }
            TerminalEvent::NotificationClose { identifier } => {
                let Some(origin) = self
                    .tabs
                    .as_ref()
                    .and_then(|tabs| tabs.identity_for_origin(origin))
                else {
                    return;
                };
                if let Some(error) = self.notifications.close_terminal(origin, identifier) {
                    self.toasts.warning(error);
                }
            }
            TerminalEvent::NotificationAliveQuery {
                identifier,
                reply_uses_8bit_controls,
            } => {
                let Some(origin) = self
                    .tabs
                    .as_ref()
                    .and_then(|tabs| tabs.identity_for_origin(origin))
                else {
                    return;
                };
                if let Some(error) =
                    self.notifications
                        .query_alive(origin, identifier, reply_uses_8bit_controls)
                {
                    self.toasts.warning(error);
                }
            }
            TerminalEvent::KittyFileTransfer(command) => {
                self.handle_kitty_file_transfer(origin, command);
            }
            TerminalEvent::TitleChanged(title) => {
                let updated = self
                    .tabs
                    .as_mut()
                    .is_some_and(|tabs| tabs.set_title_for_origin(origin, title));
                if updated {
                    // Inactive-tab events leave the native title unchanged
                    // because this always derives from the current active tab.
                    self.sync_native_window_title();
                } else {
                    log::debug!("Ignored OSC title for stale origin {:?}", origin);
                }
            }
            TerminalEvent::ClipboardRequest {
                selection: which,
                reply_uses_8bit_controls,
            } => {
                // OSC 52 read: echo the clipboard contents back to the PTY
                // as a base64 reply. Denial, unavailability, and an oversized
                // clipboard deliberately return an empty payload so clients do
                // not hang and no clipboard content leaks.
                let text = if self.config.security.osc52.allow_read {
                    self.clipboard
                        .as_mut()
                        .and_then(|clipboard| clipboard_text(clipboard, which).ok())
                        .filter(|text| text.len() <= self.config.security.osc52.max_bytes)
                        .unwrap_or_default()
                } else {
                    log::debug!("Denied OSC 52 clipboard read from {:?}", origin);
                    String::new()
                };
                let encoded = base64_encode_standard(text.as_bytes());
                let canonical = format!("\x1b]52;{};{}\x1b\\", which.osc_code(), encoded);
                if let Some(tabs) = &mut self.tabs {
                    let reply = termeric_core::Terminal::frame_pty_reply_for_mode(
                        canonical.as_bytes(),
                        reply_uses_8bit_controls,
                    );
                    if let Err(error) = tabs.write_to_origin(origin, &reply) {
                        log::warn!(
                            "Could not return clipboard reply to {:?}: {}",
                            origin,
                            error
                        );
                    }
                }
            }
            TerminalEvent::ClipboardWrite { selection, text } => {
                if !self.config.security.osc52.allow_write {
                    log::debug!("Denied OSC 52 {:?} write from {:?}", selection, origin);
                } else if text.len() > self.config.security.osc52.max_bytes {
                    log::warn!("Rejected oversized OSC 52 write from {:?}", origin);
                } else if let Some(clipboard) = &mut self.clipboard
                    && let Err(error) = set_clipboard_text(clipboard, selection, text)
                {
                    log::error!("Failed to set clipboard: {}", error);
                }
            }
            TerminalEvent::DynamicColor(operation) => {
                use termeric_core::DynamicColorOperation;
                match operation {
                    DynamicColorOperation::Query {
                        target,
                        reply_uses_8bit_controls,
                    } => {
                        let color = self.dynamic_color_for_origin(origin, target);
                        if let Some(color) = color {
                            let canonical = dynamic_color_reply(target, color);
                            if let Some(tabs) = &mut self.tabs {
                                let reply = termeric_core::Terminal::frame_pty_reply_for_mode(
                                    canonical.as_bytes(),
                                    reply_uses_8bit_controls,
                                );
                                if let Err(error) = tabs.write_to_origin(origin, &reply) {
                                    log::warn!(
                                        "Could not return dynamic-color reply to {:?}: {}",
                                        origin,
                                        error
                                    );
                                }
                            }
                        }
                    }
                    operation => {
                        if let Some(terminal) = self
                            .tabs
                            .as_mut()
                            .and_then(|tabs| tabs.terminal_for_origin_mut(origin))
                        {
                            terminal.apply_dynamic_color(operation);
                        }
                    }
                }
            }
            TerminalEvent::HyperlinkActivated(url) => {
                let directory = self.hyperlink_open_directory(origin);
                if let Err(e) =
                    self.open_hyperlink_for_origin(Some(origin), &url, directory.as_deref())
                {
                    log::error!("Failed to open URL: {}", e);
                    self.toasts.error(format!("Could not open {url:?}: {e}"));
                }
            }
            TerminalEvent::CursorVisibilityChanged(_) => {
                // Cursor visibility is handled in render
            }
            TerminalEvent::PointerShapeChanged => {
                if self.active_terminal_origin() == Some(origin) {
                    self.refresh_terminal_pointer_cursor();
                }
            }
            TerminalEvent::PtyWrite(_) => {
                // Consumed by the originating tab/pane poller. Reaching App
                // would be a routing invariant violation.
                log::error!(
                    "Terminal protocol reply escaped its PTY owner: {:?}",
                    origin
                );
            }
            TerminalEvent::Exit => {
                // Individual process exit is handled by its owning poller.
            }
        }
    }

    fn dynamic_color_for_origin(
        &self,
        origin: crate::tabs::TerminalEventOrigin,
        target: termeric_core::DynamicColorTarget,
    ) -> Option<termeric_core::Color> {
        use termeric_core::DynamicColorTarget;
        let terminal = self.tabs.as_ref()?.terminal_for_origin(origin)?;
        let dynamic = terminal.dynamic_colors();
        let theme_color = |color: crate::config::theme::Color| {
            termeric_core::Color::rgb(color.r, color.g, color.b)
        };
        Some(match target {
            DynamicColorTarget::Palette(index) => dynamic.palette(index).unwrap_or_else(|| {
                if index < 16 {
                    theme_color(self.theme.ansi_color(index))
                } else {
                    termeric_core::ansi::color_256(index)
                }
            }),
            DynamicColorTarget::Foreground => dynamic
                .foreground()
                .unwrap_or_else(|| theme_color(self.theme.ui.foreground)),
            DynamicColorTarget::Background => dynamic
                .background()
                .unwrap_or_else(|| theme_color(self.theme.ui.background)),
            DynamicColorTarget::Cursor => dynamic
                .cursor()
                .unwrap_or_else(|| theme_color(self.theme.ui.cursor)),
        })
    }

    pub(super) fn active_terminal_origin(&self) -> Option<crate::tabs::TerminalEventOrigin> {
        let tab = self.tabs.as_ref().and_then(TabManager::active_tab)?;
        Some(if let Some(panes) = &tab.pane_manager {
            crate::tabs::TerminalEventOrigin::Pane {
                tab_id: tab.id,
                pane_id: panes.focused_id(),
            }
        } else {
            crate::tabs::TerminalEventOrigin::Tab { tab_id: tab.id }
        })
    }

    /// Write interactive input with saved aliases and Git branch suggestions.
    pub(super) fn write_to_pty(&mut self, data: &[u8]) {
        self.write_editable_input(data, true);
    }

    /// Explicitly chosen command text is already resolved. Preserve normal input
    /// policy, but do not reinterpret it through aliases or Git suggestions.
    pub(super) fn write_saved_command(&mut self, command: &str) {
        for chunk in command_keystrokes(command) {
            self.write_editable_input(&chunk, false);
        }
    }

    /// Tab and Enter must use the same literal encoding and focused-owner route.
    /// True means a selected completion was handled (including safe refusal).
    pub(super) fn accept_git_completion(&mut self, submit: bool) -> bool {
        if !self.git_completion_has_room() {
            self.dismiss_git_completion_popup();
            return false;
        }
        let Some(branch) = self
            .git_completion_index
            .filter(|_| self.git_completions_visible)
            .and_then(|index| self.git_completions.get(index))
            .cloned()
        else {
            return false;
        };
        self.git_completions_visible = false;
        self.git_completions.clear();
        self.git_completion_index = None;
        if self.focused_pane_read_only() || !matches!(self.broadcast, BroadcastState::Inactive) {
            self.toasts
                .warning("Completion was not sent while input is locked or broadcasting");
            return true;
        }
        let integration = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(|tab| {
                if let Some(panes) = &tab.pane_manager {
                    panes.focused_pane().map(|pane| &pane.shell_integration)
                } else {
                    Some(&tab.shell_integration)
                }
            });
        use crate::terminal::shell_integration::ShellIntegrationSupport;
        let quoting = match integration {
            Some(ShellIntegrationSupport::Enabled("bash" | "zsh")) => {
                Some(crate::git::BranchQuoting::Posix)
            }
            Some(ShellIntegrationSupport::Enabled("fish")) => Some(crate::git::BranchQuoting::Fish),
            _ => None,
        };
        let edit = match crate::git::branch_edit(&self.input_line_buffer, &branch, quoting, submit)
        {
            Ok(edit) => edit,
            Err(message) => {
                self.toasts.warning(message);
                return true;
            }
        };
        if let Some(tab) = self.tabs.as_mut().and_then(TabManager::active_tab_mut) {
            match tab.write(&edit.wire) {
                Ok(()) => {
                    if submit {
                        self.input_line_buffer.clear();
                        self.git_watcher.invalidate();
                    } else {
                        self.input_line_buffer = edit.line;
                    }
                }
                Err(error) => self
                    .toasts
                    .error(format!("Completion was not sent: {error}")),
            }
        }
        true
    }

    fn write_editable_input(&mut self, data: &[u8], interactive_assistance: bool) {
        if matches!(self.broadcast, BroadcastState::Armed { .. }) {
            self.toasts
                .warning("Input was not sent while broadcast is armed");
            return;
        }
        if self.focused_pane_read_only() {
            self.clear_terminal_input_tracking();
            return;
        }
        // Reset cursor blink so cursor is fully visible when typing
        self.reset_cursor_blink();

        // Broadcast deliberately bypasses alias expansion and completion.
        if self.write_broadcast_bytes(data) {
            self.git_completions_visible = false;
            return;
        }

        // Track the input line buffer for alias expansion + git completion
        if data == b"\r" {
            // If git completions are visible and a selection is active, accept it
            if interactive_assistance && self.accept_git_completion(true) {
                return;
            }
            if self.git_completions_visible {
                // Dismiss completions
                self.git_completions_visible = false;
                self.git_completions.clear();
                self.git_completion_index = None;
            }

            // Enter pressed — check if first word matches a custom command name or alias
            let trimmed = self.input_line_buffer.trim().to_string();
            let (first_word, rest) = match trimmed.find(' ') {
                Some(pos) => (trimmed[..pos].to_string(), trimmed[pos..].to_string()),
                None => (trimmed.clone(), String::new()),
            };
            let matched_command = if interactive_assistance {
                self.config
                    .commands
                    .iter()
                    .find(|cmd| cmd.name == first_word || cmd.alias.as_deref() == Some(&first_word))
                    .map(|cmd| cmd.command.clone())
            } else {
                None
            };
            let expanded = matched_command.map(|command| format!("{command}{rest}"));
            let input = expanded.as_deref().unwrap_or(&trimmed);
            let rewrite = expanded
                .as_ref()
                .map(|command| format!("\x15{command}\r").into_bytes());

            // If a git command was run, invalidate the watcher to refresh branch state
            if input.starts_with("git ") {
                self.git_watcher.invalidate();
            }

            self.input_line_buffer.clear();

            if let Some(rewrite) = rewrite {
                // Clear the typed alias from the terminal line (Ctrl+U)
                // then send the real command + Enter
                if let Some(tabs) = &mut self.tabs {
                    if let Some(tab) = tabs.active_tab_mut() {
                        if let Some(terminal) = tab.focused_terminal_mut() {
                            terminal.grid_mut().set_scroll_offset(0);
                        }
                        let _ = tab.write(&rewrite);
                    }
                }
                return;
            }
        } else if data == b"\x7f" {
            // Backspace — remove last char from buffer
            self.input_line_buffer.pop();
        } else if data == b"\x15" {
            // Ctrl+U — clear buffer
            self.input_line_buffer.clear();
        } else if data == b"\x17" {
            // Ctrl+W — delete last word
            let trimmed = self.input_line_buffer.trim_end();
            if let Some(pos) = trimmed.rfind(' ') {
                self.input_line_buffer.truncate(pos + 1);
            } else {
                self.input_line_buffer.clear();
            }
        } else if data.len() == 1 && data[0] < 0x20 {
            // Other control characters (Ctrl+C, Ctrl+D, etc.) — reset buffer
            self.input_line_buffer.clear();
        } else if data.len() >= 2 && data[0] == 0x1b {
            // Escape sequences (arrow keys, etc.) — can affect cursor position, reset buffer
            self.input_line_buffer.clear();
        } else {
            // Regular characters — append to buffer
            if let Ok(s) = std::str::from_utf8(data) {
                self.input_line_buffer.push_str(s);
            }
        }

        // Explicit Run text is already resolved; it must neither build nor
        // accept interactive suggestions between its edit and submit chunks.
        if interactive_assistance
            && self.git_completion_has_room()
            && let Some(partial) = crate::git::detect_git_completion(&self.input_line_buffer)
        {
            let completions = self.git_watcher.complete_branch(partial);
            if !completions.is_empty() {
                self.git_completions = completions.into();
                self.git_completion_index = Some(0);
                self.git_completions_visible = true;
            } else {
                self.git_completions_visible = false;
                self.git_completions.clear();
                self.git_completion_index = None;
            }
        } else {
            self.git_completions_visible = false;
            self.git_completions.clear();
            self.git_completion_index = None;
        }

        // Tab owns the focused terminal/PTTY route in both single and pane mode.
        if let Some(tabs) = &mut self.tabs {
            if let Some(tab) = tabs.active_tab_mut() {
                if let Some(terminal) = tab.focused_terminal_mut() {
                    terminal.grid_mut().set_scroll_offset(0);
                }
                if let Err(error) = tab.write(data) {
                    log::error!("Failed to write to focused PTY: {}", error);
                }
            }
        }
    }

    /// Clear state derived from a user's editable terminal line. This must run
    /// whenever a route rejects user input before the bytes reach the PTY so a
    /// later unlock cannot revive aliases or completions the shell never
    /// received.
    pub(super) fn clear_terminal_input_tracking(&mut self) {
        self.input_line_buffer.clear();
        self.git_completions_visible = false;
        self.git_completions.clear();
        self.git_completion_index = None;
    }

    /// Write terminal-protocol bytes without treating them as editable shell
    /// input. Mouse/focus/paste framing must not mutate alias or completion state.
    pub(super) fn write_terminal_protocol_input(&mut self, data: &[u8]) -> bool {
        let result = self
            .tabs
            .as_mut()
            .and_then(TabManager::active_tab_mut)
            .map(|tab| tab.write(data));
        match result {
            Some(Ok(())) => true,
            Some(Err(error)) => {
                log::error!("Failed to write terminal protocol input: {}", error);
                false
            }
            None => false,
        }
    }

    /// Copy `text` to the clipboard, emit a success toast with `label`.
    pub(super) fn copy_to_clipboard(&mut self, text: String, label: &str) {
        if let Some(clipboard) = &mut self.clipboard {
            match clipboard.set_text(&text) {
                Ok(()) => {
                    self.copy_flash_start = Some(Instant::now());
                    self.toasts.success(label.to_string());
                }
                Err(e) => {
                    log::error!("Clipboard write failed: {}", e);
                    self.toasts.error(format!("Copy failed: {}", e));
                }
            }
        } else {
            self.toasts.warning("Clipboard not available");
        }
    }
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
    #[cfg(unix)]
    let source_identity = {
        use std::os::unix::fs::MetadataExt;
        Some((metadata.dev(), metadata.ino()))
    };
    #[cfg(not(unix))]
    let source_identity = None;
    Ok(ValidatedUploadSource {
        name,
        source_kind,
        identity: source_identity,
    })
}

fn unsafe_drop_path_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

fn dynamic_color_reply(
    target: termeric_core::DynamicColorTarget,
    color: termeric_core::Color,
) -> String {
    use termeric_core::DynamicColorTarget;
    let specification = format!(
        "rgb:{:04x}/{:04x}/{:04x}",
        u16::from(color.r) * 257,
        u16::from(color.g) * 257,
        u16::from(color.b) * 257
    );
    match target {
        DynamicColorTarget::Palette(index) => format!("\x1b]4;{index};{specification}\x1b\\"),
        DynamicColorTarget::Foreground => format!("\x1b]10;{specification}\x1b\\"),
        DynamicColorTarget::Background => format!("\x1b]11;{specification}\x1b\\"),
        DynamicColorTarget::Cursor => format!("\x1b]12;{specification}\x1b\\"),
    }
}

#[cfg(target_os = "linux")]
fn clipboard_text(
    clipboard: &mut arboard::Clipboard,
    selection: termeric_core::ClipboardRequest,
) -> Result<String, arboard::Error> {
    use arboard::{GetExtLinux, LinuxClipboardKind};
    let kind = match selection {
        termeric_core::ClipboardRequest::Primary => LinuxClipboardKind::Primary,
        termeric_core::ClipboardRequest::Clipboard => LinuxClipboardKind::Clipboard,
    };
    clipboard.get().clipboard(kind).text()
}

#[cfg(not(target_os = "linux"))]
fn clipboard_text(
    clipboard: &mut arboard::Clipboard,
    _selection: termeric_core::ClipboardRequest,
) -> Result<String, arboard::Error> {
    clipboard.get_text()
}

#[cfg(target_os = "linux")]
fn set_clipboard_text(
    clipboard: &mut arboard::Clipboard,
    selection: termeric_core::ClipboardRequest,
    text: String,
) -> Result<(), arboard::Error> {
    use arboard::{LinuxClipboardKind, SetExtLinux};
    let kind = match selection {
        termeric_core::ClipboardRequest::Primary => LinuxClipboardKind::Primary,
        termeric_core::ClipboardRequest::Clipboard => LinuxClipboardKind::Clipboard,
    };
    clipboard.set().clipboard(kind).text(text)
}

#[cfg(not(target_os = "linux"))]
fn set_clipboard_text(
    clipboard: &mut arboard::Clipboard,
    _selection: termeric_core::ClipboardRequest,
    text: String,
) -> Result<(), arboard::Error> {
    clipboard.set_text(text)
}

#[cfg(test)]
mod desktop_io_tests {
    use super::*;

    #[cfg(unix)]
    fn raw_command_app(count: usize) -> App {
        let mut tabs = TabManager::new(
            256,
            24,
            crate::config::ShellConfig::default(),
            100,
            1024 * 1024,
        );
        tabs.new_tab_with_command("/bin/sh", &[
            "-c".into(), format!("stty raw -echo; printf READY; dd bs=1 count={count} 2>/dev/null | od -An -tx1 -v; sleep 0.2"),
        ], None, Some("saved-command-bytes"), None, Default::default()).unwrap();
        let mut app = App::new_headless(Config::default()).unwrap();
        app.tabs = Some(tabs);
        wait_command_output(&mut app, "READY");
        app.config.commands = vec![crate::config::CustomCommand {
            name: "B".into(),
            command: "X".into(),
            alias: Some("alias".into()),
        }];
        app
    }

    #[cfg(unix)]
    fn wait_command_output(app: &mut App, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut last = String::new();
        loop {
            app.process_pty_events();
            let Some(terminal) = app.active_terminal() else {
                panic!("reader exited before {expected:?}; last output: {last:?}");
            };
            last = terminal.rows_text(0, 23);
            // od can wrap bytes onto multiple rows; preserve order but not whitespace.
            if last
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .contains(expected)
            {
                return;
            }
            assert!(Instant::now() < deadline, "missing {expected:?}: {last:?}");
            std::thread::yield_now();
        }
    }

    #[cfg(unix)]
    fn assert_command_bytes(app: &mut App, expected: &[u8]) {
        let expected = expected
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        wait_command_output(app, &expected);
    }

    #[cfg(unix)]
    fn seed_git_completion_cache(app: &mut App) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(directory.path()).unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let signature =
            git2::Signature::new("Fixture", "fixture@example.invalid", &git2::Time::new(1, 0))
                .unwrap();
        repo.commit(
            Some("refs/heads/main"),
            &signature,
            &signature,
            "inert fixture",
            &tree,
            &[],
        )
        .unwrap();
        repo.set_head("refs/heads/main").unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            app.git_watcher.update(directory.path().to_str());
            if app.git_watcher.complete_branch("ma") == ["main"] {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Git fixture scan did not publish: {:?}",
                app.git_watcher.take_error()
            );
            std::thread::yield_now();
        }
        drop(tree);
        drop(repo);
        directory
    }

    #[cfg(unix)]
    #[test]
    fn explicit_run_preserves_git_text_even_with_live_branch_suggestions() {
        let command = "git checkout ma";
        let expected = format!("\x15{command}\rZ");
        for keybinding in [false, true] {
            let mut app = raw_command_app(expected.len());
            let _repository = seed_git_completion_cache(&mut app);
            if keybinding {
                assert!(
                    app.handle_key_action(crate::input::keybindings::KeyAction::RunCommand {
                        command: command.into()
                    })
                );
            } else {
                app.run_custom_command(command);
            }
            assert!(!app.git_completions_visible);
            assert!(app.input_line_buffer.is_empty());
            app.write_to_pty(b"Z");
            assert_command_bytes(&mut app, expected.as_bytes());
        }
    }

    #[cfg(unix)]
    #[test]
    fn explicit_git_run_in_an_agent_tab_sends_the_exact_text_without_a_completion_rewrite() {
        let command = "git checkout rm -rf /synthetic-never-executed ma";
        let expected = format!("\x15{command}\rZ");
        let mut app = raw_command_app(expected.len());
        let _repository = seed_git_completion_cache(&mut app);
        app.tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .tab_type = crate::tabs::TabType::Plugin {
            plugin_id: "inert-explicit-git-guard".into(),
        };
        app.run_custom_command(command);
        assert!(!app.git_completions_visible);
        assert!(app.git_completions.is_empty());
        app.write_to_pty(b"Z");
        assert_command_bytes(&mut app, expected.as_bytes());
    }

    #[cfg(unix)]
    #[test]
    fn ordinary_typed_git_input_still_builds_and_accepts_real_cached_suggestions() {
        let expected = b"git checkout ma\x7f\x7fmain\rZ";
        let mut app = raw_command_app(expected.len());
        let _repository = seed_git_completion_cache(&mut app);
        app.write_to_pty(b"git checkout ma");
        assert!(app.git_completions_visible);
        assert_eq!(&*app.git_completions, ["main"]);
        assert_eq!(app.git_completion_index, Some(0));
        app.write_to_pty(b"\r");
        assert!(!app.git_completions_visible);
        assert!(app.input_line_buffer.is_empty());
        app.write_to_pty(b"Z");
        assert_command_bytes(&mut app, expected);
    }

    #[cfg(unix)]
    #[test]
    fn git_completion_enter_quotes_repository_text_before_pty_delivery() {
        let expected = b"git checkout to\x7f\x7f'topic;printf${IFS}UNEXPECTED'\rZ";
        let mut app = raw_command_app(expected.len());
        app.tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .shell_integration =
            crate::terminal::shell_integration::ShellIntegrationSupport::Enabled("bash");
        app.write_to_pty(b"git checkout to");
        app.git_completions = vec!["topic;printf${IFS}UNEXPECTED".into()].into();
        app.git_completion_index = Some(0);
        app.git_completions_visible = true;
        app.write_to_pty(b"\r");
        app.write_to_pty(b"Z");
        assert_command_bytes(&mut app, expected);
    }

    #[cfg(unix)]
    #[test]
    fn git_completion_tab_keeps_quoted_text_for_later_enter() {
        let expected = b"git checkout a\x7f'a'\\''b'\rZ";
        let mut app = raw_command_app(expected.len());
        app.tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .shell_integration =
            crate::terminal::shell_integration::ShellIntegrationSupport::Enabled("bash");
        app.write_to_pty(b"git checkout a");
        app.git_completions = vec!["a'b".into()].into();
        app.git_completion_index = Some(0);
        app.git_completions_visible = true;
        assert!(app.handle_git_completion_key(&winit::keyboard::Key::Named(
            winit::keyboard::NamedKey::Tab,
        )));
        assert_eq!(app.input_line_buffer, "git checkout 'a'\\''b'");
        assert!(!app.git_completions_visible);
        app.write_to_pty(b"\r");
        assert!(app.input_line_buffer.is_empty());
        app.write_to_pty(b"Z");
        assert_command_bytes(&mut app, expected);
    }

    #[cfg(unix)]
    #[test]
    fn completion_popup_does_not_steal_control_tab_or_write_edits() {
        use winit::keyboard::{Key, NamedKey};
        for read_only in [false, true] {
            let typed = "git checkout ma";
            let mut app = raw_command_app(typed.len() + 1);
            app.write_to_pty(typed.as_bytes());
            app.tabs.as_mut().unwrap().new_tab_with_command("/bin/sh", &[
                "-c".into(), "stty raw -echo; printf READY; dd bs=1 count=1 2>/dev/null | od -An -tx1; sleep 0.2".into(),
            ], None, Some("shortcut-target-reader"), None, Default::default()).unwrap();
            wait_command_output(&mut app, "READY");
            app.tabs.as_mut().unwrap().switch_to(0);
            if read_only {
                app.toggle_focused_pane_read_only();
            }
            app.input_line_buffer = typed.into();
            app.git_completions = vec!["main".into()].into();
            app.git_completion_index = Some(0);
            app.git_completions_visible = true;
            app.modifiers = ModifiersState::CONTROL;
            let key = Key::Named(NamedKey::Tab);
            assert!(!app.handle_git_completion_key(&key));
            let action = app.shortcut_registry.check(&key, app.modifiers).unwrap();
            assert_eq!(action, crate::input::keybindings::BindableAction::NextTab);
            assert!(app.handle_action(action.to_action()));
            assert_eq!(app.tabs.as_ref().unwrap().active_index(), 1);
            app.modifiers = ModifiersState::empty();
            app.write_to_pty(b"Z");
            assert_command_bytes(&mut app, b"Z");
            app.tabs.as_mut().unwrap().switch_to(0);
            if read_only {
                app.toggle_focused_pane_read_only();
            }
            app.write_to_pty(b"Z");
            assert_command_bytes(&mut app, format!("{typed}Z").as_bytes());
        }
    }

    #[cfg(unix)]
    #[test]
    fn modified_enter_cannot_accept_a_completion_through_legacy_carriage_return() {
        use winit::keyboard::{Key, NamedKey};
        let expected = b"git checkout ma\rZ";
        let mut app = raw_command_app(expected.len());
        app.write_to_pty(b"git checkout ma");
        app.git_completions = vec!["main".into()].into();
        app.git_completion_index = Some(0);
        app.git_completions_visible = true;
        app.modifiers = ModifiersState::CONTROL;
        assert!(!app.handle_git_completion_key(&Key::Named(NamedKey::Enter)));
        // Even if the terminal's keyboard mode encodes this key as legacy CR,
        // only the user's original line is submitted, without a branch rewrite.
        app.write_to_pty(b"\r");
        app.modifiers = ModifiersState::empty();
        app.write_to_pty(b"Z");
        assert_command_bytes(&mut app, expected);
    }

    #[cfg(unix)]
    #[test]
    fn git_completion_refusal_emits_neither_edits_nor_enter() {
        for submit in [false, true] {
            for (typed, branch) in [
                ("git checkout a", "a;b"),
                ("git checkout 'a", "main"),
                ("git checkout a", "a\nb"),
                ("git checkout a", "-flag"),
            ] {
                let expected = format!("{typed}Z");
                let mut app = raw_command_app(expected.len());
                app.write_to_pty(typed.as_bytes());
                app.git_completions = vec![branch.into()].into();
                app.git_completion_index = Some(0);
                app.git_completions_visible = true;
                assert!(app.accept_git_completion(submit));
                assert_eq!(app.input_line_buffer, typed);
                assert!(app.git_completions.is_empty());
                app.write_to_pty(b"Z");
                assert_command_bytes(&mut app, expected.as_bytes());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn git_completion_tab_cannot_bypass_read_only_or_broadcast_arming() {
        for read_only in [true, false] {
            let expected = b"git checkout maZ";
            let mut app = raw_command_app(expected.len());
            app.write_to_pty(b"git checkout ma");
            app.git_completions = vec!["main".into()].into();
            app.git_completion_index = Some(0);
            app.git_completions_visible = true;
            if read_only {
                app.toggle_focused_pane_read_only();
                // Exercise the mutation boundary even with a stale visible list.
                app.git_completions = vec!["main".into()].into();
                app.git_completion_index = Some(0);
                app.git_completions_visible = true;
            } else {
                app.broadcast = BroadcastState::Armed {
                    tab_id: 0,
                    targets: vec![],
                    expires: Instant::now() + Duration::from_secs(5),
                };
            }
            assert!(app.accept_git_completion(false));
            if read_only {
                app.toggle_focused_pane_read_only();
            } else {
                app.broadcast = BroadcastState::Inactive;
            }
            app.write_to_pty(b"Z");
            assert_command_bytes(&mut app, expected);
        }
    }

    #[cfg(unix)]
    #[test]
    fn git_completion_uses_focused_pane_dialect_and_destination() {
        use crate::terminal::shell_integration::ShellIntegrationSupport;
        let expected = b"git checkout a\x7f'a\\'b'\rZ";
        let mut app = raw_command_app(1);
        let shell = crate::config::ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec![
                "-c".into(),
                format!(
                    "stty raw -echo; printf READY; dd bs=1 count={} 2>/dev/null | od -An -tx1 -v; sleep 0.2",
                    expected.len()
                ),
            ],
            env: Default::default(),
        };
        let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        tab.shell_integration = ShellIntegrationSupport::Enabled("bash");
        let owner = tab.take_terminal_owner().unwrap();
        let mut panes =
            PaneManager::from_existing(owner, shell, 100, 1024 * 1024, 1600.0, 480.0, 8.0, 16.0);
        let original = panes.focused_id();
        panes.split(SplitDirection::Vertical).unwrap();
        let second = panes
            .pane_ids()
            .into_iter()
            .find(|id| *id != original)
            .unwrap();
        panes.focus(second);
        tab.pane_manager = Some(panes);
        wait_command_output(&mut app, "READY");
        let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        tab.shell_integration = ShellIntegrationSupport::Enabled("bash");
        tab.pane_manager
            .as_mut()
            .unwrap()
            .focused_pane_mut()
            .unwrap()
            .shell_integration = ShellIntegrationSupport::Enabled("fish");
        app.write_to_pty(b"git checkout a");
        app.git_completions = vec!["a'b".into()].into();
        app.git_completion_index = Some(0);
        app.git_completions_visible = true;
        app.write_to_pty(b"\r");
        app.write_to_pty(b"Z");
        assert_command_bytes(&mut app, expected);
        app.tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .pane_manager
            .as_mut()
            .unwrap()
            .focus(original);
        app.write_to_pty(b"Z");
        assert_command_bytes(&mut app, b"Z");
    }

    #[cfg(unix)]
    #[test]
    fn explicit_run_preserves_text_when_saved_names_or_aliases_collide() {
        for (command, expected) in [
            ("B", b"\x15B\r".as_slice()),
            ("B tail", b"\x15B tail\r".as_slice()),
            ("alias tail\n", b"\x15alias tail\r".as_slice()),
        ] {
            let mut app = raw_command_app(expected.len() + 1);
            app.run_custom_command(command);
            assert!(app.input_line_buffer.is_empty());
            app.write_to_pty(b"Z");
            // A following byte fences the operation, rejecting duplicate trailing input.
            assert_command_bytes(&mut app, &[expected, b"Z"].concat());
        }
    }

    #[cfg(unix)]
    #[test]
    fn ordinary_typed_names_and_aliases_still_expand_once() {
        for (name, guarded) in [("B", false), ("alias", false), ("B", true), ("alias", true)] {
            let typed = format!("{name} tail");
            let expected = format!("{typed}\x15X tail\r");
            let mut app = raw_command_app(expected.len() + 1);
            if guarded {
                app.tabs
                    .as_mut()
                    .unwrap()
                    .active_tab_mut()
                    .unwrap()
                    .tab_type = crate::tabs::TabType::Plugin {
                    plugin_id: "inert-safe-alias".into(),
                };
            }
            app.write_to_pty(typed.as_bytes());
            app.write_to_pty(b"\r");
            app.write_to_pty(b"Z");
            assert_command_bytes(&mut app, &[expected.as_bytes(), b"Z"].concat());
        }
    }

    #[cfg(unix)]
    #[test]
    fn typed_and_pasted_input_in_agent_tabs_reaches_the_terminal_without_confirmation() {
        // Only automated input (control protocol, plugins) is confirmed; what
        // a person types, expands, completes, or pastes is sent as-is.
        for kind in ["typed", "alias", "paste"] {
            let typed = if kind == "alias" {
                "alias /synthetic-never-executed"
            } else {
                "rm -rf /synthetic-never-executed"
            };
            let expected = match kind {
                "alias" => format!("{typed}\x15rm -rf /synthetic-never-executed\rZ"),
                "paste" => format!("{typed}\rZ"),
                _ => format!("{typed}\rZ"),
            };
            let mut app = raw_command_app(expected.len());
            app.config.commands[0].command = "rm -rf".into();
            app.tabs
                .as_mut()
                .unwrap()
                .active_tab_mut()
                .unwrap()
                .tab_type = crate::tabs::TabType::Plugin {
                plugin_id: "inert-agent-input".into(),
            };
            if kind == "paste" {
                app.write_paste_text(&format!("{typed}\r"));
            } else {
                app.write_to_pty(typed.as_bytes());
                app.write_to_pty(b"\r");
            }
            assert!(!app.guardrail.is_awaiting_current(), "{kind}");
            app.write_to_pty(b"Z");
            assert_command_bytes(&mut app, expected.as_bytes());
        }
    }

    #[cfg(unix)]
    #[test]
    fn git_completion_safe_enter_and_unselected_fallback_preserve_exact_bytes() {
        for (selected, expected) in [
            (Some(0), b"git checkout ma\x7f\x7fmain\rZ".as_slice()),
            (None, b"git checkout ma\rZ".as_slice()),
            (Some(5), b"git checkout ma\rZ".as_slice()),
        ] {
            let mut app = raw_command_app(expected.len());
            app.tabs
                .as_mut()
                .unwrap()
                .active_tab_mut()
                .unwrap()
                .tab_type = crate::tabs::TabType::Plugin {
                plugin_id: "inert-safe-completion".into(),
            };
            app.write_to_pty(b"git checkout ma");
            app.git_completions = vec!["main".into()].into();
            app.git_completion_index = selected;
            app.git_completions_visible = true;
            app.write_to_pty(b"\r");
            assert!(app.input_line_buffer.is_empty());
            assert!(app.git_completions.is_empty());
            app.write_to_pty(b"Z");
            assert_command_bytes(&mut app, expected);
        }
    }

    #[cfg(unix)]
    #[test]
    fn explicit_run_keeps_read_only_and_armed_broadcast_guards() {
        let mut app = raw_command_app(1);
        app.toggle_focused_pane_read_only();
        app.input_line_buffer = "stale".into();
        app.git_completions = vec!["stale".into()].into();
        app.git_completion_index = Some(0);
        app.git_completions_visible = true;
        app.run_custom_command("B");
        assert!(app.input_line_buffer.is_empty());
        assert!(app.git_completions.is_empty());
        assert!(!app.git_completions_visible);
        app.toggle_focused_pane_read_only();
        let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
        app.broadcast = BroadcastState::Armed {
            tab_id: tab.id,
            targets: vec![tab.single_pane_id()],
            expires: Instant::now() + Duration::from_secs(10),
        };
        app.run_custom_command("B");
        app.broadcast = BroadcastState::Inactive;
        app.write_to_pty(b"Z");
        // Even the line-clear prefix must not reach a refused destination.
        assert_command_bytes(&mut app, b"Z");
    }

    #[cfg(unix)]
    #[test]
    fn explicit_run_still_broadcasts_literal_text_to_selected_panes() {
        let mut app = raw_command_app(4);
        let shell = crate::config::ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "stty raw -echo; printf READY; dd bs=1 count=4 2>/dev/null | od -An -tx1; sleep 0.2".into()],
            env: Default::default(),
        };
        let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        let owner = tab.take_terminal_owner().unwrap();
        let mut panes =
            PaneManager::from_existing(owner, shell, 100, 1024 * 1024, 800.0, 480.0, 8.0, 16.0);
        panes.split(SplitDirection::Vertical).unwrap();
        let targets = panes.pane_ids();
        let tab_id = tab.id;
        tab.pane_manager = Some(panes);
        let wait_all = |app: &mut App, expected: &str| {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                app.process_pty_events();
                let panes = app
                    .tabs
                    .as_ref()
                    .unwrap()
                    .active_tab()
                    .unwrap()
                    .pane_manager
                    .as_ref()
                    .expect("both raw readers must remain live until observed");
                let outputs: Vec<_> = targets
                    .iter()
                    .map(|id| panes.get_pane(*id).unwrap().terminal.rows_text(0, 23))
                    .collect();
                if outputs.iter().all(|output| output.contains(expected)) {
                    return;
                }
                assert!(Instant::now() < deadline, "missing {expected}: {outputs:?}");
                std::thread::yield_now();
            }
        };
        wait_all(&mut app, "READY");
        app.broadcast = BroadcastState::Active {
            tab_id,
            targets: targets.clone(),
        };
        app.run_custom_command("B");
        app.write_to_pty(b"Z");
        wait_all(&mut app, "15 42 0d 5a");
    }

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

    #[cfg(unix)]
    #[test]
    fn dropped_paths_reject_non_utf8_names() {
        use std::os::unix::ffi::OsStringExt;

        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 0xff]));
        assert!(quote_local_drop_path(&path).is_err());
    }

    #[test]
    fn dynamic_color_queries_use_xterm_16_bit_rgb_and_st() {
        let color = termeric_core::Color::rgb(255, 128, 0);
        assert_eq!(
            dynamic_color_reply(termeric_core::DynamicColorTarget::Palette(3), color),
            "\x1b]4;3;rgb:ffff/8080/0000\x1b\\"
        );
        assert_eq!(
            dynamic_color_reply(termeric_core::DynamicColorTarget::Cursor, color),
            "\x1b]12;rgb:ffff/8080/0000\x1b\\"
        );
    }

    #[test]
    fn app_mediated_osc_replies_use_the_origin_terminal_c1_mode() {
        let mut terminal = termeric_core::Terminal::new(8, 4);
        let color = dynamic_color_reply(
            termeric_core::DynamicColorTarget::Background,
            termeric_core::Color::rgb(0x20, 0x18, 0x14),
        );
        assert_eq!(
            terminal.frame_pty_reply(color.as_bytes()),
            b"\x1b]11;rgb:2020/1818/1414\x1b\\"
        );

        terminal.process(b"\x1b G");
        let captured_mode = terminal.uses_8bit_controls();
        terminal.process(b"\x1b F");
        assert!(!terminal.uses_8bit_controls());
        assert_eq!(
            termeric_core::Terminal::frame_pty_reply_for_mode(color.as_bytes(), captured_mode),
            b"\x9d11;rgb:2020/1818/1414\x9c"
        );
        assert_eq!(
            termeric_core::Terminal::frame_pty_reply_for_mode(b"\x1b]52;c;\x1b\\", captured_mode,),
            b"\x9d52;c;\x9c"
        );
    }
}
