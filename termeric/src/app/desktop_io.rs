//! Desktop integration: activating a notification's origin, opening
//! hyperlinks, and the system clipboard.

use super::*;

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
        let tab = tabs.tab_by_id(origin.tab_id)?;
        if tab.closed || tab.is_closing() || tab.pane(origin.pane_id)?.closed {
            return None;
        }
        let active = tabs.active_tab_id() == Some(origin.tab_id);
        let terminal_focused = tab.focused_terminal_identity().pane_id == origin.pane_id;
        let terminal_visible = tab
            .pane_manager
            .as_ref()
            .and_then(crate::panes::PaneManager::zoomed_id)
            .is_none_or(|zoomed| zoomed == origin.pane_id);
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

        if let Err(error) =
            self.focus_pane_for(Requester::Control, origin.tab_id as u64, origin.pane_id.0)
        {
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

        use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
        let is_wayland = window
            .window_handle()
            .ok()
            .is_some_and(|handle| matches!(handle.as_raw(), RawWindowHandle::Wayland(_)));
        if is_wayland {
            // Raising an existing window on Wayland needs the daemon's
            // activation token, which winit can't apply to an existing
            // window. Asking for attention is the most it allows.
            window.request_user_attention(Some(winit::window::UserAttentionType::Critical));
        } else {
            window.focus_window();
        }

        self.request_redraw(RedrawReason::Action);
        true
    }

    /// Validate immediately before a desktop launch: the originating
    /// terminal must still be live, and no other terminal stands in for it.
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

    /// Opens a hyperlink on behalf of its terminal and reports a refusal or
    /// launch failure as an error toast. Returns whether the opener started.
    pub(super) fn activate_hyperlink(
        &mut self,
        origin: Option<crate::tabs::TerminalEventOrigin>,
        url: &str,
        directory: Option<&str>,
    ) -> bool {
        match self.open_hyperlink_for_origin(origin, url, directory) {
            Ok(()) => true,
            Err(error) => {
                log::error!("Failed to open {url:?}: {error}");
                self.toasts
                    .error(format!("Could not open {url:?}: {error}"));
                false
            }
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

pub(super) fn clipboard_text(
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

pub(super) fn set_clipboard_text(
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
