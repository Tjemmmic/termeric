//! Clipboard ownership for bounded single-line query fields.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PasteTarget {
    Command,
    Unicode,
    Domain,
    Search,
    Blocked,
    Terminal,
}

impl App {
    fn clipboard_paste_target(&self) -> PasteTarget {
        if self.guardrail.is_active()
            || self.info_overlay.is_active()
            || self.diff_review.is_active()
            || self.context_picker.is_active()
            || matches!(self.broadcast, BroadcastState::Armed { .. })
            || self.harvest.as_ref().is_some_and(|harvest| {
                harvest.sprout_menu.is_some() || harvest.cellar.is_expanded()
            })
        {
            return PasteTarget::Blocked;
        }
        // Match native keyboard dispatch, including retained background fields.
        if self.domain_picker.is_active() {
            return PasteTarget::Domain;
        }
        if self.unicode_picker.is_active() {
            return PasteTarget::Unicode;
        }
        if self.command_palette.is_active() {
            return PasteTarget::Command;
        }
        if self.shortcuts_panel.is_active()
            || self.settings_panel.is_active()
            || self.session_panel.is_open()
            || self.commands_panel.is_open()
            || self.tab_rename.is_some()
            || self.session_dropdown_open
        {
            return PasteTarget::Blocked;
        }
        if self.search.is_active() {
            return PasteTarget::Search;
        }
        if self.file_drop_input_blocked() {
            PasteTarget::Blocked
        } else {
            PasteTarget::Terminal
        }
    }

    pub(super) fn clipboard_paste_blocked(&self) -> bool {
        self.clipboard_paste_target() == PasteTarget::Blocked
    }

    pub(super) fn handle_query_paste_key(&mut self, key: &winit::keyboard::Key) -> bool {
        if self.shortcut_registry.check(key, self.modifiers)
            != Some(crate::input::keybindings::BindableAction::Paste)
            || matches!(
                self.clipboard_paste_target(),
                PasteTarget::Blocked | PasteTarget::Terminal
            )
        {
            return false;
        }
        self.paste_clipboard();
        true
    }

    /// Clipboard-only dispatch. Protocol pastes and file drops keep their
    /// explicitly terminal-owned path; a rejected query never falls through.
    pub(super) fn paste_clipboard_payload(&mut self, text: &str) -> bool {
        let target = self.clipboard_paste_target();
        match target {
            PasteTarget::Blocked => return false,
            PasteTarget::Terminal => return self.write_paste_text(text),
            _ => {}
        }
        if text.is_empty() {
            return false;
        }
        if text.len() > crate::terminal::search::MAX_SEARCH_QUERY_BYTES
            || text
                .chars()
                .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
        {
            self.toasts.warning(
                "Query paste requires bounded single-line text without control characters",
            );
            return false;
        }
        let accepted = match target {
            PasteTarget::Command => self.command_palette.insert_query_text(text),
            PasteTarget::Unicode => self.unicode_picker.insert_query_text(text),
            PasteTarget::Domain => self.domain_picker.insert_query_text(text),
            PasteTarget::Search => {
                let cursor = self.search_cursor_pos;
                let accepted = self
                    .with_search_and_active_grid(|search, grid| {
                        search.insert_text(cursor, text, grid)
                    })
                    .unwrap_or(false);
                if accepted {
                    self.search_cursor_pos += text.len();
                }
                accepted
            }
            PasteTarget::Blocked | PasteTarget::Terminal => unreachable!(),
        };
        if accepted {
            self.ime.clear_preedit();
        } else {
            self.toasts.warning("Query paste was not inserted: the field is read-only or its size limit was exceeded");
        }
        self.redraw_scheduler
            .request(redraw_scheduler::RedrawReason::KeyboardInput);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::keybindings::{BindableAction, KeyChord};
    use crate::ui::text_input::TextInputEdit;
    use winit::keyboard::{Key, NamedKey};

    #[test]
    fn query_paste_honors_rebound_and_disabled_shortcuts_without_reading_for_blocked_owners() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.command_palette.open();
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        let default = Key::Character("v".into());
        // No platform clipboard exists in headless mode, but ownership still
        // consumes the chord instead of inserting its V or reaching a PTY.
        assert!(app.handle_query_paste_key(&default));
        let rebound = Key::Named(NamedKey::F8);
        app.shortcut_registry.set_binding(
            BindableAction::Paste,
            KeyChord::from_key_lookup(&rebound, ModifiersState::empty()),
        );
        assert!(!app.handle_query_paste_key(&default));
        app.modifiers = ModifiersState::empty();
        assert!(app.handle_query_paste_key(&rebound));
        app.info_overlay.show("Notice", vec![]);
        assert!(!app.handle_query_paste_key(&rebound));
        assert!(app.clipboard_paste_blocked());
        assert!(!app.paste_clipboard_payload("hidden"));
        assert_eq!(app.command_palette.build_info().input, "");
        app.shortcut_registry.clear_binding(BindableAction::Paste);
        assert!(!app.handle_query_paste_key(&rebound));
    }

    #[test]
    fn palette_query_paste_is_atomic_at_caret_and_retains_limits() {
        for kind in [
            PasteTarget::Command,
            PasteTarget::Unicode,
            PasteTarget::Domain,
        ] {
            let mut app = App::new_headless(Config::default()).unwrap();
            match kind {
                PasteTarget::Command => app.command_palette.open(),
                PasteTarget::Unicode => app.unicode_picker.open(),
                PasteTarget::Domain => app.domain_picker.open(&app.config.domains),
                _ => unreachable!(),
            }
            assert_eq!(app.clipboard_paste_target(), kind);
            assert!(app.paste_clipboard_payload("az"));
            assert!(app.handle_palette_edit_key(&Key::Named(NamedKey::ArrowLeft)));
            assert!(app.paste_clipboard_payload("界e\u{301}👩\u{200d}💻"));
            let info = |app: &App| match kind {
                PasteTarget::Command => app.command_palette.build_info(),
                PasteTarget::Unicode => app.unicode_picker.build_info(),
                PasteTarget::Domain => app.domain_picker.build_info(),
                _ => unreachable!(),
            };
            let before = info(&app);
            assert_eq!(before.input, "a界e\u{301}👩\u{200d}💻z");
            assert_eq!(before.cursor, before.input.len() - 1);
            for rejected in [
                "",
                "a\nb",
                "a\rb",
                "a\tb",
                "\u{1b}[31m",
                "\u{7f}",
                "\u{2028}",
                "\u{2029}",
                &"x".repeat(257),
                &"x".repeat(4097),
            ] {
                assert!(!app.paste_clipboard_payload(rejected));
                assert_eq!(info(&app).input, before.input);
                assert_eq!(info(&app).cursor, before.cursor);
            }
            let remaining = 256 - before.input.chars().count();
            assert!(app.paste_clipboard_payload(&"🫚".repeat(remaining)));
            assert_eq!(info(&app).input.chars().count(), 256);
            assert!(!app.paste_clipboard_payload("x"));
            assert_eq!(info(&app).input.chars().count(), 256);
            app.unicode_picker.close();
            app.unicode_picker.finish_search_for_test();
        }
    }

    #[test]
    fn retained_query_paste_respects_modal_priority_and_busy_domain_refusal() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.search.start();
        for open in [
            (|app: &mut App| app.settings_panel.toggle()) as fn(&mut App),
            |app| app.shortcuts_panel.open(),
            |app| app.commands_panel.toggle(),
            |app| app.session_panel.open_workspaces(),
        ] {
            let mut app = App::new_headless(Config::default()).unwrap();
            app.search.start();
            open(&mut app);
            assert_eq!(app.clipboard_paste_target(), PasteTarget::Blocked);
            assert!(!app.paste_clipboard_payload("hidden"));
            assert!(app.search.query().is_empty());
        }
        app.command_palette.open();
        assert!(app.paste_clipboard_payload("background"));
        app.unicode_picker.open();
        assert_eq!(app.clipboard_paste_target(), PasteTarget::Unicode);
        app.domain_picker.open(&app.config.domains);
        assert_eq!(app.clipboard_paste_target(), PasteTarget::Domain);
        assert!(app.domain_picker.activate().is_some());
        assert!(app.domain_picker.is_busy());
        assert!(!app.paste_clipboard_payload("local"));
        assert_eq!(app.command_palette.build_info().input, "background");
        assert!(app.unicode_picker.build_info().input.is_empty());
        assert!(app.search.query().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn search_clipboard_paste_is_bounded_and_query_pastes_never_reach_raw_pty() {
        let mut tabs = TabManager::new(
            80,
            24,
            crate::config::ShellConfig::default(),
            100,
            1024 * 1024,
        );
        tabs.new_tab_with_command("/bin/sh", &[
            "-c".into(), "stty raw -echo; printf READY; dd bs=1 count=1 2>/dev/null | od -An -tx1; sleep 0.1".into(),
        ], None, Some("query-paste"), None, Default::default()).unwrap();
        let mut app = App::new_headless(Config::default()).unwrap();
        app.tabs = Some(tabs);
        let wait_for = |app: &mut App, expected: &str| {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                app.process_pty_events();
                let output = app.active_terminal().unwrap().rows_text(0, 23);
                if output.contains(expected) {
                    break;
                }
                assert!(Instant::now() < deadline, "missing {expected}: {output}");
                std::thread::yield_now();
            }
        };
        wait_for(&mut app, "READY");
        app.search.start();
        assert!(app.paste_clipboard_payload("az"));
        app.search_cursor_pos = 1;
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        assert!(app.paste_clipboard_payload("界e\u{301}"));
        assert!(app.ime.preedit().is_none());
        assert_eq!(app.search.query(), "a界e\u{301}z");
        assert_eq!(app.search_cursor_pos, "a界e\u{301}".len());
        assert!(!app.paste_clipboard_payload("\nexecute\n"));
        let remaining = crate::terminal::search::MAX_SEARCH_QUERY_BYTES - app.search.query().len();
        assert!(app.paste_clipboard_payload(&"x".repeat(remaining)));
        let full = app.search.query().to_string();
        assert!(!app.paste_clipboard_payload("界"));
        assert_eq!(app.search.query(), full);
        app.command_palette.open();
        assert!(app.paste_clipboard_payload("query only"));
        app.command_palette.edit_input(TextInputEdit::Home);
        assert!(!app.paste_clipboard_payload("\u{1b}[200~"));
        app.command_palette.close();
        app.search.cancel();
        assert!(app.paste_clipboard_payload("Z"));
        wait_for(&mut app, "5a");
    }
}
