//! Where typed, pasted, and composed (IME) text goes. Pastes, input-method
//! previews and commits, and the input method's caret all ask
//! [`App::text_target`], in the same order keyboard dispatch offers keys, so
//! none of them can send text to a field the keys would not reach.

use super::modal_layer::ModalLayer;
use super::palette_input::PaletteModal;
use super::*;

/// The surface that receives text, or `Blocked` when the surface that owns
/// input has no field to take it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TextTarget {
    Palette(PaletteModal),
    ShortcutsFilter,
    SettingsFilter,
    WorkspaceRename,
    CommandsForm,
    /// The focused field of a dialog, such as a font editor's.
    DialogField,
    /// The context picker's file-path field, while it is focused.
    ContextFiles,
    TabRename,
    AgentView,
    Search,
    Terminal,
    Blocked,
}

impl App {
    /// The top modal layer's own field, or nothing beneath it; then tab
    /// rename, the agent view while it holds the keyboard, the find box, and
    /// finally the focused terminal when it can take input.
    pub(super) fn text_target(&self) -> TextTarget {
        self.keyboard_owner_above_terminal()
            .unwrap_or_else(|| self.terminal_text_target())
    }

    /// Whether the keyboard is the terminals' own, with nothing above them
    /// (a modal layer, tab rename, the agent view, or the find box) holding
    /// it. Their cursors rest dim and steady otherwise, so it is clear where
    /// typing goes.
    pub(super) fn terminal_has_keyboard(&self) -> bool {
        self.keyboard_owner_above_terminal().is_none()
    }

    /// Where text goes when something above the terminals holds the
    /// keyboard, in `text_target`'s order; `None` when nothing does.
    fn keyboard_owner_above_terminal(&self) -> Option<TextTarget> {
        if let Some(layer) = self.top_modal_layer() {
            return Some(match layer {
                ModalLayer::DomainPicker
                | ModalLayer::UnicodePicker
                | ModalLayer::CommandPalette => self
                    .open_palette_modal()
                    .filter(|modal| self.palette_owner(*modal).query_editable())
                    .map_or(TextTarget::Blocked, TextTarget::Palette),
                ModalLayer::ShortcutsPanel if !self.shortcuts_panel.is_recording() => {
                    TextTarget::ShortcutsFilter
                }
                ModalLayer::Settings if !self.settings_panel.is_dropdown_open() => {
                    TextTarget::SettingsFilter
                }
                ModalLayer::WorkspacePanel if self.workspace_panel.is_renaming() => {
                    TextTarget::WorkspaceRename
                }
                ModalLayer::CommandsPanel if self.commands_panel.is_editing() => {
                    TextTarget::CommandsForm
                }
                ModalLayer::Dialog if self.dialog.field_focused() => TextTarget::DialogField,
                ModalLayer::ContextPicker if self.context_picker.file_input_focused() => {
                    TextTarget::ContextFiles
                }
                _ => TextTarget::Blocked,
            });
        }
        if self.tab_rename.is_some() {
            return Some(TextTarget::TabRename);
        }
        if self.agent_view_has_keyboard() {
            return Some(if self.agent_view_accepts_text() {
                TextTarget::AgentView
            } else {
                TextTarget::Blocked
            });
        }
        self.search.is_active().then_some(TextTarget::Search)
    }

    /// Where text goes when the terminals hold the keyboard: the focused
    /// one, when it can take input.
    fn terminal_text_target(&self) -> TextTarget {
        // Arming takes every key, so nothing may reach a terminal either.
        if self.copy_mode.is_active()
            || matches!(self.broadcast, BroadcastState::Armed { .. })
            || self.focused_pane_read_only()
            || self.active_terminal().is_none()
        {
            TextTarget::Blocked
        } else {
            TextTarget::Terminal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::event::Ime;

    /// Broadcast arming and a read-only pane stop text at the terminal only:
    /// a field above it takes pastes and composed text, as it takes keys.
    #[test]
    fn arming_and_read_only_panes_block_the_terminal_but_not_the_fields_above_it() {
        let mut app = App::new_headless_running("sleep 2", "text-target");
        assert_eq!(app.text_target(), TextTarget::Terminal);
        let tab_id = app.tabs.as_ref().unwrap().active_tab_id().unwrap();
        app.broadcast = BroadcastState::Armed {
            tab_id,
            expires: Instant::now() + Duration::from_secs(10),
            targets: Vec::new(),
        };
        assert_eq!(app.text_target(), TextTarget::Blocked);

        app.command_palette.open();
        assert!(app.paste_clipboard_payload("root"));
        app.handle_ime_event(Ime::Preedit("draft".into(), None));
        assert!(app.ime.preedit().is_some());
        app.handle_ime_event(Ime::Commit("s".into()));
        assert_eq!(app.command_palette.build_info().input, "roots");
        app.command_palette.close();

        app.dialog.set_reduced_motion(true);
        let request = app.dialog.show(crate::ui::dialog::DialogSpec::prompt(
            "Add rule",
            vec![],
            crate::ui::dialog::FieldSpec::new("Rule", "", 64),
            "save",
            "Add",
        ));
        assert!(app.paste_clipboard_payload("tuber"));
        app.handle_ime_event(Ime::Commit("s".into()));
        assert_eq!(app.dialog.field_value(request), Some("tubers"));
        assert!(app.dialog.dismiss(request));
        app.dialog.update_animation();

        app.broadcast = BroadcastState::Inactive;
        assert_eq!(app.text_target(), TextTarget::Terminal);
        app.toggle_focused_pane_read_only();
        assert_eq!(app.text_target(), TextTarget::Blocked);
        app.handle_ime_event(Ime::Preedit("draft".into(), None));
        assert!(app.ime.preedit().is_none());
    }

    /// The terminals keep the keyboard, and their cursors keep blinking,
    /// only while nothing above them takes typing; a blocked terminal, such
    /// as a read-only pane, still holds it.
    #[test]
    fn the_terminal_holds_the_keyboard_until_a_field_above_it_takes_typing() {
        let mut app = App::new_headless_running("sleep 2", "keyboard-owner");
        assert!(app.terminal_has_keyboard());
        app.toggle_focused_pane_read_only();
        assert_eq!(app.text_target(), TextTarget::Blocked);
        assert!(app.terminal_has_keyboard());
        app.toggle_focused_pane_read_only();

        app.search.start();
        assert!(!app.terminal_has_keyboard());
        app.search.cancel();
        assert!(app.terminal_has_keyboard());

        app.command_palette.open();
        assert!(!app.terminal_has_keyboard());
        assert!(!app.cursor_blink_required());
        app.command_palette.close();
        assert!(app.terminal_has_keyboard());
    }
}
