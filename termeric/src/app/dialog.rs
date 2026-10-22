//! Dialog input: keys, presses, the wheel, focus, and the field, and the
//! answers routed back to the flow that opened the dialog.

use super::text_field_keys::{FieldKey, types_text};
use super::text_target::TextTarget;
use super::*;
use crate::ui::dialog::{
    DialogAction, DialogFocus, DialogHit, DialogLayout, DialogMetrics, DialogScroll,
};
use crate::ui::text_input_viewport::TextInputViewport;
use winit::keyboard::{Key, NamedKey};

impl App {
    /// The dialog's geometry, shared by drawing, pointer hits, the IME
    /// caret, and accessibility. Measuring it also tells the dialog how far
    /// its body scrolls, so keyboard focus scrolls into view.
    pub(super) fn dialog_layout(&mut self) -> Option<DialogLayout> {
        let metrics = self.ui_metrics();
        let scale = metrics.scale_factor;
        let dialog_metrics = DialogMetrics {
            window_width: metrics.width,
            window_height: metrics.height,
            scale,
            cell_width: metrics.cell_width,
            cell_height: metrics.cell_height,
            animation: self.dialog.animation(),
        };
        let layout = match self.renderer.as_mut() {
            Some(renderer) => {
                DialogLayout::calculate(&self.dialog, dialog_metrics, renderer.text_measure())
            }
            // Headless tests measure with a fixed advance, near the UI face's.
            #[cfg(test)]
            None => DialogLayout::calculate(
                &self.dialog,
                dialog_metrics,
                &mut crate::ui::text_layout::FixedMeasure {
                    advance: metrics.cell_width * 0.75,
                },
            ),
            #[cfg(not(test))]
            None => return None,
        };
        self.dialog
            .sync_body(layout.body_extent(), |focus| layout.span(focus));
        Some(layout)
    }

    /// The field's visible text and caret, for the IME caret.
    pub(super) fn dialog_field_viewport(&mut self) -> Option<TextInputViewport> {
        let layout = self.dialog_layout()?;
        let field = self.dialog.field(self.dialog.request_id())?;
        layout.field_viewport(&field.value, field.cursor_byte, self.dialog.scroll_offset())
    }

    /// About Termeric: what it is, its version, and where it lives.
    pub(super) fn show_about(&mut self) {
        use crate::ui::dialog::{DialogBlock, DialogSpec};
        self.dialog.show(DialogSpec::message(
            "About Termeric",
            vec![
                DialogBlock::text(env!("CARGO_PKG_DESCRIPTION")),
                DialogBlock::text(format!("Version {}", env!("CARGO_PKG_VERSION"))),
                DialogBlock::value(env!("CARGO_PKG_REPOSITORY").trim_start_matches("https://")),
            ],
        ));
    }

    /// A key for the dialog, which owns the keyboard while it is on top.
    pub(super) fn handle_dialog_key(&mut self, key: &Key, repeat: bool) {
        let action = self.dialog_key_action(key, repeat);
        self.apply_dialog_action(action);
        self.end_composition_outside_dialog_field();
        self.request_redraw(RedrawReason::KeyboardInput);
    }

    /// Drop the input method's composition once the dialog's field no longer
    /// holds the keyboard (focus moved to a button, the dialog went back or
    /// closed), so it cannot reappear in the field later.
    pub(super) fn end_composition_outside_dialog_field(&mut self) {
        if !self.dialog.field_focused() {
            self.ime.clear_preedit();
        }
    }

    fn dialog_key_action(&mut self, key: &Key, repeat: bool) -> DialogAction {
        let request_id = self.dialog.request_id();
        let scroll = |by| DialogAction::Scroll { request_id, by };
        match key {
            Key::Named(NamedKey::Tab) => {
                self.dialog
                    .cycle_focus(request_id, self.modifiers.shift_key());
                DialogAction::None
            }
            Key::Named(NamedKey::Escape) => self.dialog.escape(request_id),
            // A held key must not answer something it did not mean to reach.
            Key::Named(NamedKey::Enter) if repeat => DialogAction::None,
            Key::Named(NamedKey::Enter) => self.dialog.activate_focused(request_id),
            _ if self.dialog.field_focused() => self.dialog_field_key(key, request_id),
            // Space presses the focused button, as Enter does.
            Key::Named(NamedKey::Space) if !repeat => self.dialog.activate_focused(request_id),
            Key::Named(direction @ (NamedKey::ArrowLeft | NamedKey::ArrowRight)) => {
                self.dialog
                    .step_button(request_id, *direction == NamedKey::ArrowRight);
                DialogAction::None
            }
            Key::Named(direction @ (NamedKey::ArrowUp | NamedKey::ArrowDown)) => {
                scroll(DialogScroll::Lines(if *direction == NamedKey::ArrowUp {
                    -1
                } else {
                    1
                }))
            }
            Key::Named(direction @ (NamedKey::PageUp | NamedKey::PageDown)) => {
                scroll(DialogScroll::Pages(if *direction == NamedKey::PageUp {
                    -1
                } else {
                    1
                }))
            }
            Key::Named(NamedKey::Home) => scroll(DialogScroll::Top),
            Key::Named(NamedKey::End) => scroll(DialogScroll::Bottom),
            _ => DialogAction::None,
        }
    }

    /// A key while the field is focused: editing, typing, and paging the
    /// body. Under AltGr a character key types, even where the layout
    /// reports Ctrl with it.
    fn dialog_field_key(&mut self, key: &Key, request_id: u64) -> DialogAction {
        if let Some(field_key) = self.text_edit_for_key(key) {
            if let FieldKey::Edit(edit) = field_key {
                self.dialog.edit_field(request_id, edit);
            }
            return DialogAction::None;
        }
        let types = types_text(self.modifiers, self.alt_graph_active);
        match key {
            Key::Named(NamedKey::Space) if types => {
                self.dialog.insert_field_text(request_id, " ");
            }
            Key::Character(character) if types => {
                self.dialog
                    .insert_field_text(request_id, character.as_str());
            }
            Key::Named(NamedKey::PageUp) => {
                return DialogAction::Scroll {
                    request_id,
                    by: DialogScroll::Pages(-1),
                };
            }
            Key::Named(NamedKey::PageDown) => {
                return DialogAction::Scroll {
                    request_id,
                    by: DialogScroll::Pages(1),
                };
            }
            _ => {}
        }
        DialogAction::None
    }

    /// A primary or middle press on the dialog. The primary button works
    /// its buttons and field. The middle button on the field focuses
    /// it at the press and pastes the primary selection there, as on every
    /// other field; anywhere else it does nothing.
    pub(super) fn press_dialog(&mut self, button: winit::event::MouseButton, x: f32, y: f32) {
        use winit::event::MouseButton;
        let Some(layout) = self.dialog_layout() else {
            return;
        };
        let request_id = self.dialog.request_id();
        let offset = self.dialog.scroll_offset();
        let primary = button == MouseButton::Left;
        let action = match layout.hit(x, y, offset) {
            Some(DialogHit::Button(index)) if primary => {
                self.dialog
                    .set_focus(request_id, Some(DialogFocus::Button(index)));
                self.dialog.activate_button(request_id, index)
            }
            Some(DialogHit::Field) if matches!(button, MouseButton::Left | MouseButton::Middle) => {
                if self.dialog.set_focus(request_id, Some(DialogFocus::Field))
                    && let Some(field) = self.dialog.field(request_id)
                    && let Some(cursor) =
                        layout.field_caret_at(&field.value, field.cursor_byte, x, offset)
                {
                    self.dialog.place_field_caret(request_id, cursor);
                }
                if button == MouseButton::Middle {
                    self.paste_primary_into_field(TextTarget::DialogField);
                }
                DialogAction::None
            }
            _ => DialogAction::None,
        };
        self.apply_dialog_action(action);
        self.end_composition_outside_dialog_field();
    }

    pub(super) fn apply_dialog_action(&mut self, action: DialogAction) {
        match action {
            DialogAction::Copy { request_id } => {
                if let Some(text) = self.dialog.copy_text(request_id) {
                    self.copy_to_clipboard(text, "Copied details to clipboard");
                }
            }
            DialogAction::Close { request_id } => {
                if self.dialog.dismiss(request_id) {
                    self.forget_dialog_flows(request_id);
                }
            }
            DialogAction::Answer { request_id, id } => {
                if !self.dialog.answers(request_id, &id) {
                    return;
                }
                let claimed = self.handle_kitty_download_choice(request_id, &id)
                    || self.handle_kitty_upload_choice(request_id, &id)
                    || self.handle_font_feature_choice(request_id, &id)
                    || self.handle_font_range_choice(request_id, &id)
                    || self.handle_font_coverage_choice(request_id, &id)
                    || self.handle_plugin_choice(request_id, &id)
                    || self.handle_agent_hook_choice(request_id, &id)
                    || self.handle_revert_choice(request_id, &id)
                    || self.handle_config_reset_choice(request_id, &id)
                    || self.handle_discard_settings_choice(request_id, &id)
                    || self.handle_shortcut_reassign_choice(request_id, &id)
                    || self.handle_workspace_choice(request_id, &id)
                    || self.handle_file_upload_choice(request_id, &id);
                // An answer no flow takes, such as Cancel on a plain
                // question, closes the dialog.
                if !claimed && self.dialog.dismiss(request_id) {
                    self.forget_dialog_flows(request_id);
                }
            }
            DialogAction::Scroll { request_id, by } => {
                self.dialog.scroll(request_id, by);
            }
            DialogAction::None => {}
        }
    }

    /// Forget what was waiting on dialog `request_id`, just closed without
    /// an answer.
    fn forget_dialog_flows(&mut self, request_id: u64) {
        self.dismiss_pending_kitty_download(request_id);
        // A replacement dialog may have superseded a flow's own; any older
        // hidden flow is obsolete too.
        self.dialog_flow = None;
    }
}

#[cfg(test)]
impl App {
    /// Answer the open dialog with `id`, as its button would.
    pub(super) fn answer_dialog(&mut self, id: &str) {
        let request_id = self.dialog.request_id();
        assert!(
            self.dialog.answers(request_id, id),
            "{:?} has no {id:?}",
            self.dialog.title()
        );
        self.apply_dialog_action(DialogAction::Answer {
            request_id,
            id: id.into(),
        });
    }

    /// Type `value` into the open dialog's field and press Enter there.
    pub(super) fn submit_dialog_field(&mut self, value: &str) {
        let request_id = self.dialog.request_id();
        // Typing the text it already holds changes nothing.
        self.dialog.replace_field_text(request_id, value);
        assert_eq!(self.dialog.field_value(request_id), Some(value));
        self.dialog.set_focus(request_id, Some(DialogFocus::Field));
        let action = self.dialog.activate_focused(request_id);
        self.apply_dialog_action(action);
    }

    /// Everything the open dialog says: its heading, body, and field error.
    pub(super) fn dialog_text(&self) -> String {
        let request_id = self.dialog.request_id();
        let mut text = self.dialog.copy_text(request_id).unwrap_or_default();
        if let Some(error) = self.dialog.field(request_id).and_then(|field| field.error) {
            text.push_str("\n\n");
            text.push_str(&error);
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::dialog::{DialogBlock, DialogSpec, FieldSpec};
    use winit::event::MouseButton;

    fn show_field(app: &mut App, value: &str) -> u64 {
        app.dialog.show(DialogSpec::prompt(
            "Add feature",
            vec![DialogBlock::text("tag = value")],
            FieldSpec::new("Feature", value, 64),
            "save",
            "Add",
        ))
    }

    /// A composition belongs to the field: it lasts while the field holds
    /// the keyboard and ends when focus or the dialog leaves it.
    #[test]
    fn a_composition_ends_when_the_field_loses_the_keyboard() {
        use winit::event::Ime;
        let mut app = App::new_headless(Config::default()).unwrap();
        show_field(&mut app, "");
        app.handle_ime_event(Ime::Preedit("ka".into(), None));
        app.handle_dialog_key(&Key::Named(NamedKey::ArrowLeft), false);
        assert!(app.ime.preedit().is_some());
        app.handle_dialog_key(&Key::Named(NamedKey::Tab), false);
        assert!(!app.dialog.field_focused());
        assert!(app.ime.preedit().is_none());

        show_field(&mut app, "");
        app.handle_ime_event(Ime::Preedit("ka".into(), None));
        app.handle_dialog_key(&Key::Named(NamedKey::Escape), false);
        assert!(app.ime.preedit().is_none());
    }

    #[test]
    fn altgr_types_into_the_field_where_ctrl_would_make_a_chord() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let (clipboard, memory) = super::super::clipboard::Clipboard::in_memory();
        app.clipboard = clipboard;
        let request = show_field(&mut app, "");
        // Some layouts report Ctrl+Alt with AltGr.
        app.modifiers = ModifiersState::CONTROL | ModifiersState::ALT;
        app.alt_graph_active = true;
        for letter in ["d", "c"] {
            app.handle_dialog_key(&Key::Character(letter.into()), false);
        }
        assert_eq!(app.dialog.field_value(request), Some("dc"));

        // Without AltGr, Ctrl+C neither types nor copies the dialog.
        app.modifiers = ModifiersState::CONTROL;
        app.alt_graph_active = false;
        app.handle_dialog_key(&Key::Character("c".into()), false);
        assert_eq!(app.dialog.field_value(request), Some("dc"));
        assert_eq!(memory.lock().unwrap().clipboard, None);
    }

    #[test]
    fn letter_keys_answer_nothing_and_enter_and_escape_answer_the_buttons() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.dialog.set_reduced_motion(true);
        let request = app.dialog.show(DialogSpec::confirm(
            "Delete workspace?",
            vec![],
            "delete",
            "Delete",
            crate::ui::dialog::ButtonRole::Destructive,
        ));
        for letter in ["d", "y", "c", "D"] {
            app.handle_dialog_key(&Key::Character(letter.into()), false);
        }
        assert!(app.dialog.is_awaiting(request));
        assert_eq!(app.dialog.focus(), Some(DialogFocus::Button(0)));
        // Right moves to Delete; a held Enter does nothing; Left goes back,
        // and Enter there cancels.
        app.handle_dialog_key(&Key::Named(NamedKey::ArrowRight), false);
        assert_eq!(app.dialog.focus(), Some(DialogFocus::Button(1)));
        app.handle_dialog_key(&Key::Named(NamedKey::Enter), true);
        assert!(app.dialog.is_awaiting(request));
        app.handle_dialog_key(&Key::Named(NamedKey::ArrowLeft), false);
        app.handle_dialog_key(&Key::Named(NamedKey::Enter), false);
        assert!(!app.dialog.is_awaiting(request));

        let request = app.dialog.show(DialogSpec::message("Saved", vec![]));
        app.handle_dialog_key(&Key::Named(NamedKey::Escape), false);
        assert!(!app.dialog.is_awaiting(request));
    }

    #[test]
    fn a_middle_press_on_the_field_pastes_the_primary_selection_where_it_landed() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let (clipboard, memory) = super::super::clipboard::Clipboard::in_memory();
        memory.lock().unwrap().primary = Some("ginger".into());
        app.clipboard = clipboard;
        let request = show_field(&mut app, "ab");
        app.dialog.set_focus(request, Some(DialogFocus::Button(0)));
        let layout = app.dialog_layout().unwrap();
        let view = layout
            .field_viewport("ab", 2, app.dialog.scroll_offset())
            .unwrap();
        // Between "a" and "b".
        let x = view.text_x + layout.cell_width * 1.2;
        let y = layout.field_box(0.0).unwrap().center()[1];

        app.press_dialog(MouseButton::Middle, x, y);
        assert!(app.dialog.field_focused());
        assert_eq!(app.dialog.field_value(request), Some("agingerb"));

        // The same limits as a keyboard paste: nothing that breaks the line.
        memory.lock().unwrap().primary = Some("one\ntwo".into());
        app.press_dialog(MouseButton::Middle, x, y);
        assert_eq!(app.dialog.field_value(request), Some("agingerb"));

        // Anywhere but the field, the middle button does nothing at all.
        memory.lock().unwrap().primary = Some("ginger".into());
        for bounds in [layout.buttons[0], layout.buttons[1], layout.heading_bounds] {
            let [x, y] = bounds.center();
            app.press_dialog(MouseButton::Middle, x, y);
        }
        assert!(app.dialog.is_awaiting(request));
        assert!(app.dialog.field_focused());
        assert_eq!(app.dialog.field_value(request), Some("agingerb"));
        assert_eq!(memory.lock().unwrap().clipboard, None);
    }

    #[test]
    fn copy_details_copies_the_heading_and_body_and_keeps_the_dialog() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let (clipboard, memory) = super::super::clipboard::Clipboard::in_memory();
        app.clipboard = clipboard;
        let request = app.dialog.show(DialogSpec::error(
            "Couldn't start",
            vec![DialogBlock::value("exit status 127")],
        ));
        let layout = app.dialog_layout().unwrap();
        let [x, y] = layout.buttons[0].center();
        app.press_dialog(MouseButton::Left, x, y);
        assert_eq!(
            memory.lock().unwrap().clipboard.as_deref(),
            Some("Couldn't start\n\nexit status 127")
        );
        assert!(app.dialog.is_awaiting(request));
        let [x, y] = layout.buttons[1].center();
        app.press_dialog(MouseButton::Left, x, y);
        assert!(!app.dialog.is_awaiting(request));
    }
}
