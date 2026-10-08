//! Renaming a tab inline in the tab bar: its text field, keys, IME, and the
//! live preview.

use super::redraw_scheduler::RedrawReason;
use super::*;

impl App {
    pub(super) fn begin_tab_rename(&mut self) {
        let Some(tab) = self.tabs.as_ref().and_then(TabManager::active_tab) else {
            self.toasts.warning("No tab is available to rename");
            return;
        };
        if tab
            .live_domain_tab_route()
            .is_some_and(|(domain, _)| self.live_domain_busy(domain))
        {
            self.toasts
                .info("Wait for the pending live-domain change before renaming");
            return;
        }
        let max_chars = if tab.live_domain.is_some() {
            crate::domain::model::MAX_DOMAIN_NAME_CHARS
        } else {
            crate::workspace::MAX_WORKSPACE_TITLE_CHARS
        };
        self.tab_rename = Some(TabRenameState::new(
            tab.id,
            tab.user_title().map(str::to_owned),
            tab.user_title().unwrap_or(&tab.title),
            max_chars,
        ));
        self.ime.clear_preedit();
        self.toasts
            .info("Rename tab: type a title, Enter saves, Escape cancels");
    }

    pub(super) fn handle_tab_rename_input(&mut self, event: &winit::event::KeyEvent) -> bool {
        if self.tab_rename.is_none() {
            return false;
        }
        if event.state != winit::event::ElementState::Pressed {
            return true;
        }
        self.handle_tab_rename_key(&event.logical_key)
    }

    fn update_tab_rename_preview(&mut self) {
        let Some(rename) = &self.tab_rename else {
            return;
        };
        if let Some(tab) = self.tabs.as_mut().and_then(|tabs| {
            tabs.all_tabs_mut()
                .iter_mut()
                .find(|tab| tab.id == rename.tab_id)
        }) {
            tab.set_user_title(Some(rename.input.text().to_owned()));
        }
    }

    pub(super) fn insert_tab_rename_text(&mut self, text: &str) -> bool {
        let Some(rename) = &mut self.tab_rename else {
            return false;
        };
        if !rename.input.insert(text) {
            return false;
        }
        self.update_tab_rename_preview();
        true
    }

    pub(super) fn handle_tab_rename_key(&mut self, key: &winit::keyboard::Key) -> bool {
        if self.tab_rename.is_none() {
            return false;
        }
        use winit::keyboard::{Key, NamedKey};
        let mut commit = false;
        let mut cancel = false;
        let plain = self.modifiers.is_empty() && !self.alt_graph_active;
        let text_allowed =
            super::text_field_keys::types_text(self.modifiers, self.alt_graph_active);
        match key {
            Key::Named(NamedKey::Enter) if plain => commit = true,
            Key::Named(NamedKey::Escape) if plain => cancel = true,
            Key::Named(NamedKey::Backspace) if plain => {
                let rename = self.tab_rename.as_mut().expect("rename checked above");
                rename
                    .input
                    .edit_grapheme(crate::ui::text_input::TextInputEdit::Backspace);
                self.update_tab_rename_preview();
            }
            Key::Named(NamedKey::Space) if text_allowed => {
                if !self.insert_tab_rename_text(" ") {
                    return true;
                }
            }
            Key::Character(text) if text_allowed => {
                if !self.insert_tab_rename_text(text) {
                    return true;
                }
            }
            _ => return true,
        }
        if commit {
            if let Some(rename) = self.tab_rename.take() {
                let title = (!rename.input.text().trim().is_empty())
                    .then(|| rename.input.text().to_owned());
                let live_route = self.tabs.as_ref().and_then(|tabs| {
                    tabs.iter().find_map(|(_, tab)| {
                        (tab.id == rename.tab_id)
                            .then(|| tab.live_domain_tab_route())
                            .flatten()
                            .map(|(domain, remote_tab)| (domain.to_string(), remote_tab))
                    })
                });
                if let Some(tab) = self.tabs.as_mut().and_then(|tabs| {
                    tabs.all_tabs_mut()
                        .iter_mut()
                        .find(|tab| tab.id == rename.tab_id)
                }) {
                    tab.set_user_title(title.clone());
                    if let Some((domain_id, remote_tab_id)) = live_route {
                        self.queue_live_domain_control(LiveDomainControl::RenameTab {
                            domain_id,
                            remote_tab_id,
                            title,
                        });
                    } else {
                        self.toasts.success("Tab renamed");
                    }
                }
            }
        } else if cancel && let Some(rename) = self.tab_rename.take() {
            if let Some(tab) = self.tabs.as_mut().and_then(|tabs| {
                tabs.all_tabs_mut()
                    .iter_mut()
                    .find(|tab| tab.id == rename.tab_id)
            }) {
                tab.set_user_title(rename.original);
            }
            self.toasts.info("Tab rename cancelled");
        }
        self.ime.clear_preedit();
        self.request_redraw(RedrawReason::KeyboardInput);

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_rename_text_is_single_line_control_free_and_strictly_bounded() {
        let draft = |title: &str, max_chars| {
            TabRenameState::new(0, None, title, max_chars)
                .input
                .text()
                .to_owned()
        };
        assert_eq!(draft("alpha\nbeta", 20), "alphabeta");
        assert_eq!(draft("界🙂abc", 3), "界🙂a");
        assert_eq!(draft("a\u{2028}b\u{2029}c", 20), "abc");

        let mut rename = TabRenameState::new(0, None, "xxx", 5);
        assert!(!rename.input.insert(" \nmore"));
        assert_eq!(rename.input.text(), "xxx");
        assert!(rename.input.insert("界🙂"));
        assert_eq!(rename.input.text(), "xxx界🙂");
        assert!(!rename.input.insert("ignored"));
        assert_eq!(rename.input.text(), "xxx界🙂");
    }

    #[test]
    fn tab_rename_keyboard_and_ime_are_atomic_at_local_and_domain_limits() {
        use winit::{
            event::Ime,
            keyboard::{Key, NamedKey},
        };
        for max_chars in [128, 256] {
            let mut app = App::new_headless(Config::default()).unwrap();
            let original = "界".repeat(max_chars - 1);
            app.tab_rename = Some(TabRenameState::new(0, None, &original, max_chars));
            for value in [
                "",
                "xy",
                "a\nb",
                "\r",
                "\t",
                "\0",
                "\x1b",
                "\u{85}",
                "\u{2028}",
                "\u{2029}",
                &"x".repeat(max_chars + 1),
            ] {
                app.handle_ime_event(Ime::Preedit("keep".into(), None));
                assert!(app.handle_tab_rename_key(&Key::Character(value.into())));
                assert_eq!(app.tab_rename.as_ref().unwrap().input.text(), original);
                assert!(app.ime.preedit().is_some());
                app.handle_ime_event(Ime::Commit(value.into()));
                assert_eq!(app.tab_rename.as_ref().unwrap().input.text(), original);
                assert!(app.ime.preedit().is_none());
            }
            app.handle_ime_event(Ime::Commit("🫚".into()));
            assert_eq!(
                app.tab_rename.as_ref().unwrap().input.text(),
                format!("{original}🫚")
            );
            app.handle_ime_event(Ime::Preedit("keep".into(), None));
            assert!(app.handle_tab_rename_key(&Key::Named(NamedKey::Space)));
            assert_eq!(
                app.tab_rename
                    .as_ref()
                    .unwrap()
                    .input
                    .text()
                    .chars()
                    .count(),
                max_chars
            );
            assert!(app.ime.preedit().is_some());
            assert!(
                app.tab_rename
                    .as_mut()
                    .unwrap()
                    .input
                    .replace("e\u{301}👩\u{200d}💻🇺🇸")
            );
            for expected in ["e\u{301}👩\u{200d}💻", "e\u{301}", "", ""] {
                assert!(app.handle_tab_rename_key(&Key::Named(NamedKey::Backspace)));
                assert_eq!(app.tab_rename.as_ref().unwrap().input.text(), expected);
                assert!(app.ime.preedit().is_none());
            }
            assert!(app.handle_tab_rename_key(&Key::Character("e\u{301}👩\u{200d}💻".into())));
            assert_eq!(
                app.tab_rename.as_ref().unwrap().input.text(),
                "e\u{301}👩\u{200d}💻"
            );
            assert!(app.pending_live_domain_controls.is_empty());
        }
    }

    #[test]
    fn tab_rename_modified_keys_cannot_delete_commit_or_cancel() {
        use winit::{
            event::Ime,
            keyboard::{Key, NamedKey},
        };
        let mut app = App::new_headless(Config::default()).unwrap();
        assert!(!app.handle_tab_rename_key(&Key::Named(NamedKey::Backspace)));
        app.tab_rename = Some(TabRenameState::new(0, None, "Roots", 256));
        app.handle_ime_event(Ime::Preedit("keep".into(), None));
        for modifiers in [
            ModifiersState::CONTROL,
            ModifiersState::ALT,
            ModifiersState::SUPER,
            ModifiersState::SHIFT,
            ModifiersState::CONTROL | ModifiersState::ALT,
        ] {
            app.modifiers = modifiers;
            for key in [
                NamedKey::Backspace,
                NamedKey::Enter,
                NamedKey::Escape,
                NamedKey::ArrowLeft,
                NamedKey::Delete,
            ] {
                assert!(app.handle_tab_rename_key(&Key::Named(key)));
                assert_eq!(app.tab_rename.as_ref().unwrap().input.text(), "Roots");
                assert!(app.ime.preedit().is_some());
            }
            if modifiers != ModifiersState::SHIFT {
                assert!(app.handle_tab_rename_key(&Key::Character("x".into())));
                assert!(app.handle_tab_rename_key(&Key::Named(NamedKey::Space)));
                assert_eq!(app.tab_rename.as_ref().unwrap().input.text(), "Roots");
                assert!(app.ime.preedit().is_some());
            }
        }
        app.alt_graph_active = true;
        assert!(app.handle_tab_rename_key(&Key::Named(NamedKey::Backspace)));
        assert_eq!(app.tab_rename.as_ref().unwrap().input.text(), "Roots");
        assert!(app.handle_tab_rename_key(&Key::Character("@".into())));
        assert_eq!(app.tab_rename.as_ref().unwrap().input.text(), "Roots@");
        assert!(app.ime.preedit().is_none());
        app.alt_graph_active = false;
        app.modifiers = ModifiersState::SHIFT;
        assert!(app.handle_tab_rename_key(&Key::Character("A".into())));
        assert_eq!(app.tab_rename.as_ref().unwrap().input.text(), "Roots@A");
        app.modifiers = ModifiersState::empty();
        assert!(app.handle_tab_rename_key(&Key::Named(NamedKey::Escape)));
        assert!(app.tab_rename.is_none());
    }
}
