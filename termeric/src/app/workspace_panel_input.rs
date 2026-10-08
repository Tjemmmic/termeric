//! Workspace rename keyboard ownership after higher-priority modal surfaces.

use super::text_field_keys::{FieldKey, types_text};
use super::*;
use crate::ui::workspace_panel::WorkspacePanelAction;
use winit::keyboard::{Key, NamedKey};

impl App {
    /// Keys for the workspace manager: renaming, the list, and dismissal.
    pub(super) fn handle_workspace_panel_key(&mut self, key: &Key) {
        use crate::ui::workspace_panel::WorkspacePanelKey;
        if self.handle_workspace_rename_key(key) {
            return;
        }
        let panel_key = match key {
            Key::Named(NamedKey::Escape) => Some(WorkspacePanelKey::Escape),
            Key::Named(NamedKey::ArrowUp) => Some(WorkspacePanelKey::Up),
            Key::Named(NamedKey::ArrowDown) => Some(WorkspacePanelKey::Down),
            Key::Named(NamedKey::Enter) => Some(WorkspacePanelKey::Enter),
            Key::Named(NamedKey::Delete) => Some(WorkspacePanelKey::Delete),
            Key::Named(NamedKey::F2) => Some(WorkspacePanelKey::Rename),
            _ => None,
        };
        if let Some(key) = panel_key {
            let action = self.workspace_panel.handle_key(key);
            self.apply_workspace_panel_action(action);
        }
        self.request_redraw(RedrawReason::KeyboardInput);
    }

    pub(super) fn handle_workspace_rename_key(&mut self, key: &Key) -> bool {
        if !self.workspace_panel.is_open() || !self.workspace_panel.is_renaming() {
            return false;
        }
        if let Some(field_key) = self.text_edit_for_key(key) {
            let FieldKey::Edit(edit) = field_key else {
                return true;
            };
            self.workspace_panel.rename_edit(edit);
        } else {
            let plain = self.modifiers.is_empty() && !self.alt_graph_active;
            let text = types_text(self.modifiers, self.alt_graph_active);
            match key {
                Key::Named(NamedKey::Escape) if plain => {
                    self.apply_workspace_panel_action(WorkspacePanelAction::CancelRename);
                }
                Key::Named(NamedKey::Enter) if plain => {
                    self.apply_workspace_panel_action(WorkspacePanelAction::SubmitRename);
                }
                Key::Named(NamedKey::Space) if text => {
                    if !self.workspace_panel.insert_rename_text(" ") {
                        return true;
                    }
                }
                Key::Character(value) if text => {
                    if !self.workspace_panel.insert_rename_text(value) {
                        return true;
                    }
                }
                _ => return true,
            }
        }
        self.ime.clear_preedit();
        self.request_redraw(RedrawReason::KeyboardInput);

        true
    }

    /// Apply one workspace panel action through the same path for
    /// pointer, keyboard, and accessibility input.
    pub(super) fn apply_workspace_panel_action(&mut self, action: WorkspacePanelAction) {
        let workspace_mode = self.workspace_panel.is_workspace_mode();
        match action {
            WorkspacePanelAction::Load(id) => {
                if workspace_mode {
                    self.request_open_workspace(&id);
                } else {
                    self.restore_closed_tab(&id);
                }
                self.workspace_panel.close();
            }
            WorkspacePanelAction::Delete(id) => self.confirm_workspace_delete(&id, !workspace_mode),
            WorkspacePanelAction::Rename(id) => {
                self.ime.clear_preedit();
                self.workspace_panel.begin_rename(&id);
            }
            WorkspacePanelAction::SubmitRename => {
                self.ime.clear_preedit();
                if let Some((id, name)) = self.workspace_panel.take_rename() {
                    match crate::workspace::rename_workspace(&id, &name) {
                        Ok(()) => {
                            self.workspace_panel.refresh(
                                crate::session::listing::WorkspaceListing::load(
                                    self.workspace_panel.is_workspace_mode(),
                                ),
                            );
                            self.toasts.success("Workspace renamed");
                        }
                        Err(error) => self.toasts.error(format!("Rename failed: {error:#}")),
                    }
                }
            }
            WorkspacePanelAction::CancelRename => {
                self.ime.clear_preedit();
                self.workspace_panel.cancel_rename();
            }
            WorkspacePanelAction::PlaceRenameCaret { cursor } => {
                if self.workspace_panel.place_rename_caret(cursor) {
                    self.ime.clear_preedit();
                    self.request_redraw(RedrawReason::PointerInput);
                }
            }
            WorkspacePanelAction::Close => {
                self.ime.clear_preedit();
                self.workspace_panel.close();
            }
            WorkspacePanelAction::Select(id) => {
                self.workspace_panel.select_id(&id);
            }
            WorkspacePanelAction::ShowSnapshots => {
                self.ime.clear_preedit();
                self.workspace_panel
                    .open(crate::session::listing::WorkspaceListing::closed_tabs());
            }
            WorkspacePanelAction::ShowWorkspaces => {
                self.ime.clear_preedit();
                self.workspace_panel
                    .open(crate::session::listing::WorkspaceListing::workspaces());
            }
            WorkspacePanelAction::None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::workspace_panel::WorkspaceEntry;
    use winit::event::Ime;

    fn rename_app() -> App {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.workspace_panel.set_test_entries(
            vec![WorkspaceEntry {
                id: "workspace-keyboard-fixture".into(),
                display_name: "Roots".into(),
                subtitle: String::new(),
                last_modified: String::new(),
            }],
            true,
        );
        app.apply_workspace_panel_action(WorkspacePanelAction::Rename(
            "workspace-keyboard-fixture".into(),
        ));
        app
    }

    fn assert_draft(app: &App, text: &str, cursor: usize) {
        let info = app.workspace_panel.build_info();
        assert_eq!(info.rename_text.as_deref(), Some(text));
        assert_eq!(info.rename_cursor, cursor);
        assert_eq!(app.workspace_panel.entries()[0].display_name, "Roots");
    }

    #[test]
    fn rename_pointer_actions_revalidate_boundaries_and_clear_only_accepted_composition() {
        let mut app = rename_app();
        let text = "界e\u{301}👩\u{200d}💻🇺🇸z";
        assert!(app.workspace_panel.accessibility_set_rename(text));
        app.handle_ime_event(Ime::Preedit("keep".into(), None));
        for cursor in [1, 2, 4, 5, 10, 13, 21, text.len() + 1, usize::MAX] {
            app.apply_workspace_panel_action(WorkspacePanelAction::PlaceRenameCaret { cursor });
            assert_draft(&app, text, text.len());
            assert!(app.ime.preedit().is_some());
        }
        app.redraw_scheduler.begin_frame();
        app.apply_workspace_panel_action(WorkspacePanelAction::PlaceRenameCaret { cursor: 3 });
        assert_draft(&app, text, 3);
        assert!(app.ime.preedit().is_none());
        assert_eq!(
            app.redraw_scheduler.begin_frame().to_string(),
            "pointer-input"
        );
        app.handle_ime_event(Ime::Commit("Q".into()));
        assert_draft(&app, "界Qe\u{301}👩\u{200d}💻🇺🇸z", 4);
        app.workspace_panel.close();
        assert!(!app.workspace_panel.place_rename_caret(0));
        app.apply_workspace_panel_action(WorkspacePanelAction::CancelRename);
        assert!(!app.workspace_panel.place_rename_caret(0));
        assert_eq!(app.workspace_panel.entries()[0].display_name, "Roots");
    }

    #[test]
    fn rename_navigation_and_deletion_use_whole_graphemes_at_the_caret() {
        let mut app = rename_app();
        let text = "a界e\u{301}👩\u{200d}💻🇺🇸z";
        assert!(app.workspace_panel.accessibility_set_rename(text));
        for (key, expected, cursor) in [
            (NamedKey::Home, text, 0),
            (NamedKey::ArrowRight, text, 1),
            (NamedKey::Delete, "ae\u{301}👩\u{200d}💻🇺🇸z", 1),
            (NamedKey::ArrowRight, "ae\u{301}👩\u{200d}💻🇺🇸z", 4),
            (NamedKey::Delete, "ae\u{301}🇺🇸z", 4),
            (NamedKey::End, "ae\u{301}🇺🇸z", 13),
            (NamedKey::ArrowLeft, "ae\u{301}🇺🇸z", 12),
            (NamedKey::Backspace, "ae\u{301}z", 4),
            (NamedKey::Backspace, "az", 1),
        ] {
            app.handle_ime_event(Ime::Preedit("draft".into(), None));
            app.redraw_scheduler.begin_frame();
            assert!(app.handle_workspace_rename_key(&Key::Named(key)));
            assert_draft(&app, expected, cursor);
            assert!(app.ime.preedit().is_none());
            assert_eq!(
                app.redraw_scheduler.begin_frame().to_string(),
                "keyboard-input"
            );
        }
        assert!(app.handle_workspace_rename_key(&Key::Character("界e\u{301}".into())));
        assert_draft(&app, "a界e\u{301}z", 7);
        // A new IME commit inserts at the edited caret, not the draft's end.
        app.handle_ime_event(Ime::Commit("Q".into()));
        assert_draft(&app, "a界e\u{301}Qz", 8);
        assert!(app.handle_workspace_rename_key(&Key::Named(NamedKey::Escape)));
        assert!(!app.workspace_panel.is_renaming());
        assert!(app.workspace_panel.is_open());
        assert!(!app.handle_workspace_rename_key(&Key::Named(NamedKey::Delete)));
        assert_eq!(app.workspace_panel.entries()[0].display_name, "Roots");
    }

    #[test]
    fn rename_control_word_edits_preserve_graphemes_and_empty_drafts() {
        let mut app = rename_app();
        assert!(
            app.workspace_panel
                .accessibility_set_rename("one e\u{301} 👩\u{200d}💻 tail")
        );
        app.modifiers = ModifiersState::CONTROL;
        for (key, expected, cursor) in [
            (NamedKey::ArrowLeft, "one e\u{301} 👩\u{200d}💻 tail", 20),
            (NamedKey::Backspace, "one e\u{301} tail", 8),
            (NamedKey::Delete, "one e\u{301} ", 8),
            (NamedKey::ArrowLeft, "one e\u{301} ", 4),
            (NamedKey::ArrowRight, "one e\u{301} ", 7),
            (NamedKey::Backspace, "one  ", 4),
            (NamedKey::Backspace, " ", 0),
            (NamedKey::Delete, "", 0),
        ] {
            assert!(app.handle_workspace_rename_key(&Key::Named(key)));
            assert_draft(&app, expected, cursor);
        }
        // Boundary edits are harmless and still owned by the rename form.
        for key in [
            NamedKey::ArrowLeft,
            NamedKey::ArrowRight,
            NamedKey::Delete,
            NamedKey::Backspace,
        ] {
            assert!(app.handle_workspace_rename_key(&Key::Named(key)));
            assert_draft(&app, "", 0);
        }
    }

    #[test]
    fn rename_rejected_modifiers_and_text_preserve_draft_caret_and_composition() {
        let mut app = rename_app();
        app.handle_ime_event(Ime::Preedit("draft".into(), None));
        for modifiers in [
            ModifiersState::ALT,
            ModifiersState::SUPER,
            ModifiersState::SHIFT,
            ModifiersState::CONTROL | ModifiersState::SHIFT,
        ] {
            app.modifiers = modifiers;
            for key in [
                NamedKey::Home,
                NamedKey::End,
                NamedKey::ArrowLeft,
                NamedKey::ArrowRight,
                NamedKey::Delete,
                NamedKey::Backspace,
                NamedKey::Enter,
                NamedKey::Escape,
            ] {
                assert!(app.handle_workspace_rename_key(&Key::Named(key)));
                assert_draft(&app, "Roots", 5);
                assert!(app.ime.preedit().is_some());
            }
        }
        app.modifiers = ModifiersState::CONTROL;
        for key in [
            NamedKey::Home,
            NamedKey::End,
            NamedKey::Enter,
            NamedKey::Escape,
        ] {
            assert!(app.handle_workspace_rename_key(&Key::Named(key)));
            assert_draft(&app, "Roots", 5);
        }
        for modifiers in [
            ModifiersState::CONTROL,
            ModifiersState::ALT,
            ModifiersState::SUPER,
        ] {
            app.modifiers = modifiers;
            for key in [Key::Character("x".into()), Key::Named(NamedKey::Space)] {
                assert!(app.handle_workspace_rename_key(&key));
                assert_draft(&app, "Roots", 5);
                assert!(app.ime.preedit().is_some());
            }
        }
        app.modifiers = ModifiersState::empty();
        for value in ["", "bad\ntext", "\u{2028}", &"x".repeat(124)] {
            assert!(app.handle_workspace_rename_key(&Key::Character(value.into())));
            assert_draft(&app, "Roots", 5);
            assert!(app.ime.preedit().is_some());
        }
        app.modifiers = ModifiersState::CONTROL | ModifiersState::ALT;
        app.alt_graph_active = true;
        for key in [NamedKey::Backspace, NamedKey::Enter, NamedKey::Escape] {
            assert!(app.handle_workspace_rename_key(&Key::Named(key)));
            assert_draft(&app, "Roots", 5);
            assert!(app.ime.preedit().is_some());
        }
        assert!(app.handle_workspace_rename_key(&Key::Character("@".into())));
        assert_draft(&app, "Roots@", 6);
        assert!(app.ime.preedit().is_none());
        app.alt_graph_active = false;
        app.modifiers = ModifiersState::SHIFT;
        assert!(app.handle_workspace_rename_key(&Key::Character("A".into())));
        assert!(app.handle_workspace_rename_key(&Key::Named(NamedKey::Space)));
        assert_draft(&app, "Roots@A ", 8);
        app.workspace_panel.close();
        assert!(!app.handle_workspace_rename_key(&Key::Named(NamedKey::Backspace)));
    }
}
