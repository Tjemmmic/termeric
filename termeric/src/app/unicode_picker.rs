//! Exact-destination Unicode picker application routing.

use super::*;
use crate::ui::unicode_picker::UnicodePickerSelection;

impl App {
    pub(super) fn open_unicode_picker(&mut self) {
        if !matches!(self.broadcast, BroadcastState::Inactive) {
            self.toasts
                .warning("Unicode picker is unavailable while broadcast input is active");
            return;
        }
        let Some(origin) = self.active_unicode_picker_origin() else {
            self.toasts.warning("No writable terminal is active");
            return;
        };
        if self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.origin_read_only(origin))
            != Some(false)
        {
            self.toasts.warning("The active pane is read-only");
            return;
        }
        self.domain_picker.close();
        self.command_palette.close();
        self.pending_unicode_picker = Some(PendingUnicodePicker { origin });
        self.unicode_picker.open();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    pub(super) fn close_unicode_picker(&mut self) {
        self.unicode_picker.close();
        self.pending_unicode_picker = None;
    }

    pub(super) fn apply_unicode_picker_selection(
        &mut self,
        selection: UnicodePickerSelection,
    ) -> bool {
        let pending = self.pending_unicode_picker.take();
        self.unicode_picker.close();
        let Some(pending) = pending else {
            return false;
        };
        if !matches!(self.broadcast, BroadcastState::Inactive) {
            self.toasts
                .warning("Unicode character was not inserted because broadcast input changed");
            return false;
        }
        if self.active_unicode_picker_origin() != Some(pending.origin) {
            self.toasts
                .warning("The original Unicode picker destination is no longer focused");
            return false;
        }
        if self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.origin_read_only(pending.origin))
            != Some(false)
        {
            self.clear_terminal_input_tracking();
            self.toasts
                .warning("The original Unicode picker destination is no longer writable");
            return false;
        }
        if selection.text.is_empty() || selection.text.chars().any(char::is_control) {
            self.toasts
                .error("Unicode picker rejected an unsafe character selection");
            return false;
        }
        log::debug!(
            "Unicode picker inserted {} into {:?}",
            selection.id,
            pending.origin
        );
        self.write_to_pty(selection.text.as_bytes());
        true
    }

    pub(super) fn active_unicode_picker_origin(&self) -> Option<crate::tabs::TerminalEventOrigin> {
        let tab = self.tabs.as_ref().and_then(TabManager::active_tab)?;
        if tab.closed || tab.is_closing() || tab.focused_terminal().is_none() {
            return None;
        }
        Some(if let Some(panes) = &tab.pane_manager {
            crate::tabs::TerminalEventOrigin::Pane {
                tab_id: tab.id,
                pane_id: panes.focused_id(),
            }
        } else {
            crate::tabs::TerminalEventOrigin::Tab { tab_id: tab.id }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::time::{Duration, Instant};

    fn selection(text: &str) -> UnicodePickerSelection {
        UnicodePickerSelection {
            id: format!("test.{}", text.escape_unicode()),
            text: text.into(),
        }
    }

    #[cfg(unix)]
    fn shell_app(script: &str) -> App {
        let mut config = Config::default();
        config.shell.program = Some("/bin/sh".into());
        config.shell.args = vec!["-c".into(), script.into()];
        let mut app = App::new_headless(config).unwrap();
        app.allow_auto_restore = false;
        app.init_tabs().unwrap();
        app
    }

    #[cfg(unix)]
    fn wait_for_text(app: &mut App, needle: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            app.process_pty_events();
            if app.active_terminal().is_some_and(|terminal| {
                terminal
                    .grid()
                    .export_scrollback_text()
                    .join("\n")
                    .contains(needle)
            }) {
                return true;
            }
            std::thread::yield_now();
        }
        false
    }

    #[cfg(unix)]
    #[test]
    fn selection_writes_the_exact_utf8_bytes_to_the_captured_terminal() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let script = "stty raw -echo; printf '\rREADY\r\n'; \
                      bytes=$(dd bs=1 count=3 2>/dev/null | od -An -tx1); \
                      printf '\runicode=%s\r\n' \"$bytes\"; sleep 1";
        let mut app = shell_app(script);
        assert!(wait_for_text(&mut app, "READY"));

        app.open_unicode_picker();
        assert!(app.unicode_picker.is_active());
        assert!(app.apply_unicode_picker_selection(selection("→")));
        assert!(wait_for_text(&mut app, "unicode= e2 86 92"));
        assert_eq!(app.input_line_buffer, "→");
    }

    #[cfg(unix)]
    #[test]
    fn changed_destination_refuses_insertion_instead_of_rerouting_it() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let mut app = shell_app("sleep 2");
        app.open_unicode_picker();
        let captured = app.pending_unicode_picker.unwrap().origin;
        app.tabs.as_mut().unwrap().new_tab().unwrap();
        assert_ne!(app.active_unicode_picker_origin(), Some(captured));

        assert!(!app.apply_unicode_picker_selection(selection("λ")));
        assert!(app.input_line_buffer.is_empty());
        assert!(
            app.toasts.iter().last().is_some_and(|toast| {
                toast.message.contains("destination is no longer focused")
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn read_only_and_broadcast_modes_refuse_to_open_or_complete() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let mut app = shell_app("sleep 2");
        app.toggle_focused_pane_read_only();
        app.open_unicode_picker();
        assert!(!app.unicode_picker.is_active());

        app.toggle_focused_pane_read_only();
        app.open_unicode_picker();
        assert!(app.unicode_picker.is_active());
        let tab_id = app.tabs.as_ref().unwrap().active_tab_id().unwrap();
        app.broadcast = BroadcastState::Armed {
            tab_id,
            expires: Instant::now() + Duration::from_secs(1),
            targets: Vec::new(),
        };
        assert!(!app.apply_unicode_picker_selection(selection("€")));
        assert!(app.input_line_buffer.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_selection_payloads_are_rejected_before_terminal_input() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let mut app = shell_app("sleep 2");
        app.open_unicode_picker();
        assert!(!app.apply_unicode_picker_selection(selection("\n")));
        assert!(app.input_line_buffer.is_empty());
        assert!(
            app.toasts
                .iter()
                .last()
                .is_some_and(|toast| { toast.message.contains("unsafe character selection") })
        );
    }

    #[cfg(unix)]
    #[test]
    fn ime_commit_edits_the_picker_filter_without_writing_to_the_terminal() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let mut app = shell_app("sleep 2");
        app.open_unicode_picker();
        app.handle_ime_event(winit::event::Ime::Commit("U+03BB".into()));

        let info = app.unicode_picker.build_info();
        assert_eq!(info.input, "U+03BB");
        assert_eq!(info.total_entries, 1);
        assert_eq!(info.entries[0].id, "unicode-picker.codepoint.3bb");
        assert!(app.input_line_buffer.is_empty());
    }
}
