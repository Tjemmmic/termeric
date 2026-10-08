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
        let Some(terminal) = self.writable_unicode_picker_target() else {
            self.toasts.warning("No writable terminal is active");
            return;
        };
        self.close_domain_picker();
        self.command_palette.close();
        self.pending_unicode_picker = Some(PendingUnicodePicker { terminal });
        self.unicode_picker.open();
        self.request_redraw(RedrawReason::Action);
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
        if self.writable_unicode_picker_target() != Some(pending.terminal) {
            // As every write the focused read-only pane refuses does.
            if self.focused_pane_read_only() {
                self.clear_terminal_input_tracking();
            }
            self.toasts.warning(
                "The original Unicode picker destination is no longer focused or writable",
            );
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
            pending.terminal
        );
        self.write_to_pty(selection.text.as_bytes());
        true
    }

    /// The focused terminal, while it can take a picked character: its tab
    /// is open, its pane still routes, and it is not read-only.
    pub(super) fn writable_unicode_picker_target(&self) -> Option<crate::tabs::TerminalIdentity> {
        let tabs = self.tabs.as_ref()?;
        let tab = tabs.active_tab()?;
        if tab.closed || tab.is_closing() || tab.focused_terminal().is_none() {
            return None;
        }
        let target = tab.focused_terminal_identity();
        let origin = tabs.origin_for_pane(target.tab_id, target.pane_id)?;
        (tabs.origin_read_only(origin) == Some(false)).then_some(target)
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

    fn shell_app(script: &str) -> App {
        let mut config = Config::default();
        config.shell.program = Some("/bin/sh".into());
        config.shell.args = vec!["-c".into(), script.into()];
        App::new_headless_with_tabs(config)
    }

    #[test]
    fn selection_writes_the_exact_utf8_bytes_to_the_captured_terminal() {
        let script = "stty raw -echo; printf '\rREADY\r\n'; \
                      bytes=$(dd bs=1 count=3 2>/dev/null | od -An -tx1); \
                      printf '\runicode=%s\r\n' \"$bytes\"; sleep 1";
        let mut app = shell_app(script);
        app.wait_for_screen_text("READY");

        app.open_unicode_picker();
        assert!(app.unicode_picker.is_active());
        assert!(app.apply_unicode_picker_selection(selection("→")));
        app.wait_for_screen_text("unicode= e2 86 92");
        assert_eq!(app.input_line_buffer, "→");
    }

    #[test]
    fn cldr_sequence_selection_writes_every_qualified_byte_once() {
        let expected = "👩🏾‍⚖️";
        let expected_hex = expected
            .as_bytes()
            .iter()
            .map(|byte| format!(" {byte:02x}"))
            .collect::<String>();
        let script = format!(
            "stty raw -echo; printf '\rREADY\r\n'; \
             bytes=$(dd bs=1 count={} 2>/dev/null | od -An -v -tx1 | tr -d '\n'); \
             printf '\runicode=%s\r\n' \"$bytes\"; sleep 1",
            expected.len()
        );
        let mut app = shell_app(&script);
        app.wait_for_screen_text("READY");

        app.open_unicode_picker();
        assert!(
            app.unicode_picker
                .set_query("woman judge medium dark skin tone")
        );
        app.unicode_picker.finish_search_for_test();
        let info = app.unicode_picker.build_info();
        let entry = info
            .entries
            .iter()
            .find(|entry| entry.title.starts_with(expected))
            .unwrap();
        let selection = app.unicode_picker.execute_command(&entry.id).unwrap();
        assert_eq!(selection.text.as_bytes(), expected.as_bytes());
        assert!(app.apply_unicode_picker_selection(selection));
        app.wait_for_screen_text(&format!("unicode={expected_hex}"));
        assert_eq!(app.input_line_buffer, expected);
    }

    #[test]
    fn changed_destination_refuses_insertion_instead_of_rerouting_it() {
        let mut app = shell_app("sleep 2");
        app.open_unicode_picker();
        let captured = app.pending_unicode_picker.unwrap().terminal;
        app.tabs.as_mut().unwrap().new_tab().unwrap();
        assert_ne!(app.writable_unicode_picker_target(), Some(captured));

        assert!(!app.apply_unicode_picker_selection(selection("λ")));
        assert!(app.input_line_buffer.is_empty());
        assert!(
            app.toasts.iter().last().is_some_and(|toast| {
                toast.message.contains("destination is no longer focused")
            })
        );
    }

    #[test]
    fn read_only_and_broadcast_modes_refuse_to_open_or_complete() {
        let mut app = shell_app("sleep 2");
        app.toggle_focused_pane_read_only();
        app.open_unicode_picker();
        assert!(!app.unicode_picker.is_active());

        app.toggle_focused_pane_read_only();
        app.open_unicode_picker();
        assert!(app.unicode_picker.is_active());
        // A pane locked while the picker is open refuses the character.
        app.toggle_focused_pane_read_only();
        assert!(!app.apply_unicode_picker_selection(selection("λ")));
        assert!(app.input_line_buffer.is_empty());
        app.toggle_focused_pane_read_only();

        app.open_unicode_picker();
        let tab_id = app.tabs.as_ref().unwrap().active_tab_id().unwrap();
        app.broadcast = BroadcastState::Armed {
            tab_id,
            expires: Instant::now() + Duration::from_secs(1),
            targets: Vec::new(),
        };
        assert!(!app.apply_unicode_picker_selection(selection("€")));
        assert!(app.input_line_buffer.is_empty());
    }

    #[test]
    fn unsafe_selection_payloads_are_rejected_before_terminal_input() {
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

    #[test]
    fn ime_commit_edits_the_picker_filter_without_writing_to_the_terminal() {
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
