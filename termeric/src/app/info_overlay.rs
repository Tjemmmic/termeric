//! Info overlay actions: scrolling, focus, text entry, and choices, routed
//! back to the flow that opened the overlay.

use super::text_field_keys::{FieldKey, types_text};
use super::text_target::TextTarget;
use super::*;

/// Where a pointer press landed on the info overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InfoOverlayPress {
    CopyButton,
    ContinueButton,
    /// Content line `line`, at text column `column` (the horizontal scroll
    /// included).
    Line {
        line: usize,
        column: usize,
    },
    Elsewhere,
}

impl App {
    /// Keys for the info overlay: focus, its text field, scrolling, copy, and choice shortcuts.
    pub(super) fn handle_info_overlay_key(&mut self, key: &winit::keyboard::Key, repeat: bool) {
        use super::overlay_scroll::{OverlayScroll, overlay_scroll_for_key};
        use crate::ui::info_overlay::InfoOverlayAction;
        use winit::keyboard::{Key, NamedKey};
        let request_id = self.info_overlay.request_id();
        let scroll = overlay_scroll_for_key(key, self.modifiers, self.info_overlay.viewport_rows());
        let scroll_action = |scroll| match scroll {
            OverlayScroll::Rows(delta) => InfoOverlayAction::ScrollLines { request_id, delta },
            OverlayScroll::Columns(delta) => InfoOverlayAction::ScrollColumns { request_id, delta },
            OverlayScroll::Top => InfoOverlayAction::ScrollToTop { request_id },
            OverlayScroll::Bottom => InfoOverlayAction::ScrollToBottom { request_id },
            OverlayScroll::LineStart => InfoOverlayAction::ScrollToLeft { request_id },
            OverlayScroll::LineEnd => InfoOverlayAction::ScrollToRight { request_id },
        };
        let action = if *key == Key::Named(NamedKey::Tab) {
            self.ime.clear_preedit();
            self.info_overlay
                .cycle_focus(request_id, self.modifiers.shift_key());
            InfoOverlayAction::None
        } else if *key == Key::Named(NamedKey::Enter) {
            if repeat {
                InfoOverlayAction::None
            } else {
                self.info_overlay.activate_focused(request_id)
            }
        } else if self.info_overlay.input_focused() {
            // Under AltGr a character key types, even where the layout
            // reports Ctrl with it.
            let control = self.modifiers.control_key() && !self.alt_graph_active;
            if let Some(field_key) = self.text_edit_for_key(key) {
                if let FieldKey::Edit(edit) = field_key {
                    self.info_overlay.edit_input(request_id, edit);
                }
                InfoOverlayAction::None
            } else {
                match key {
                    Key::Named(NamedKey::Escape) => InfoOverlayAction::CancelInput { request_id },
                    _ if let Some(scroll) = scroll => scroll_action(scroll),
                    Key::Character(character)
                        if control && character.as_str().eq_ignore_ascii_case("c") =>
                    {
                        InfoOverlayAction::Copy { request_id }
                    }
                    Key::Character(character)
                        if control && character.as_str().eq_ignore_ascii_case("d") =>
                    {
                        self.info_overlay.choice(request_id, "unset").map_or(
                            InfoOverlayAction::None,
                            |_| InfoOverlayAction::Choose {
                                request_id,
                                choice_id: "unset".into(),
                            },
                        )
                    }
                    Key::Named(NamedKey::Space)
                        if types_text(self.modifiers, self.alt_graph_active) =>
                    {
                        self.info_overlay.insert_input_text(request_id, " ");
                        InfoOverlayAction::None
                    }
                    Key::Character(character)
                        if types_text(self.modifiers, self.alt_graph_active) =>
                    {
                        self.info_overlay
                            .insert_input_text(request_id, character.as_str());
                        InfoOverlayAction::None
                    }
                    _ => InfoOverlayAction::None,
                }
            }
        } else {
            match key {
                Key::Named(NamedKey::Escape) => {
                    if self.info_overlay.has_input() {
                        InfoOverlayAction::CancelInput { request_id }
                    } else {
                        InfoOverlayAction::Dismiss { request_id }
                    }
                }
                _ if let Some(scroll) = scroll => scroll_action(scroll),
                Key::Character(character) => {
                    let choice = character.as_str().chars().next().and_then(|shortcut| {
                        self.info_overlay.choice_for_shortcut(request_id, shortcut)
                    });
                    match choice {
                        // A held key must not pick a choice it did not
                        // mean to reach.
                        Some(_) if repeat => InfoOverlayAction::None,
                        Some(choice) => InfoOverlayAction::Choose {
                            request_id,
                            choice_id: choice.id.clone(),
                        },
                        None if character.as_str().eq_ignore_ascii_case("c")
                            && self.info_overlay.copy_key_free() =>
                        {
                            InfoOverlayAction::Copy { request_id }
                        }
                        None => InfoOverlayAction::None,
                    }
                }
                _ => InfoOverlayAction::None,
            }
        };
        self.apply_info_overlay_action(action);
        self.request_redraw(RedrawReason::KeyboardInput);
    }

    /// A primary or middle press on the info overlay. The primary button
    /// works its buttons, choices, and text field. The middle button on the
    /// text field focuses it at the press and pastes the primary selection
    /// there, as on the find box's query; anywhere else it does nothing.
    pub(super) fn press_info_overlay(
        &mut self,
        button: winit::event::MouseButton,
        press: InfoOverlayPress,
    ) {
        use crate::ui::info_overlay::{InfoOverlayAction, InfoOverlayFocus};
        use winit::event::MouseButton;
        let request_id = self.info_overlay.request_id();
        let primary = button == MouseButton::Left;
        let action = match press {
            InfoOverlayPress::CopyButton if primary => {
                self.info_overlay
                    .set_focus(request_id, Some(InfoOverlayFocus::Copy));
                InfoOverlayAction::Copy { request_id }
            }
            InfoOverlayPress::ContinueButton if primary => {
                if self.info_overlay.has_input() {
                    InfoOverlayAction::SubmitInput { request_id }
                } else {
                    InfoOverlayAction::Dismiss { request_id }
                }
            }
            InfoOverlayPress::Line { line, column }
                if self
                    .info_overlay
                    .input_info(request_id)
                    .is_some_and(|input| input.source_line == line) =>
            {
                self.info_overlay
                    .set_focus(request_id, Some(InfoOverlayFocus::Input));
                self.info_overlay
                    .set_input_cursor_from_column(request_id, column);
                if button == MouseButton::Middle {
                    self.paste_primary_into_field(TextTarget::InfoOverlayInput);
                }
                InfoOverlayAction::None
            }
            InfoOverlayPress::Line { line, .. } if primary => {
                self.info_overlay.choice_at_line(request_id, line).map_or(
                    InfoOverlayAction::None,
                    |choice| InfoOverlayAction::Choose {
                        request_id,
                        choice_id: choice.id.clone(),
                    },
                )
            }
            _ => InfoOverlayAction::None,
        };
        self.apply_info_overlay_action(action);
    }

    pub(super) fn apply_info_overlay_action(
        &mut self,
        action: crate::ui::info_overlay::InfoOverlayAction,
    ) {
        use crate::ui::info_overlay::InfoOverlayAction;
        match action {
            InfoOverlayAction::Copy { request_id } => {
                if let Some(text) = self.info_overlay.copy_text(request_id) {
                    self.copy_to_clipboard(text, "Copied notice to clipboard");
                }
            }
            InfoOverlayAction::Dismiss { request_id } => {
                if self.info_overlay.dismiss(request_id) {
                    self.dismiss_pending_kitty_download(request_id);
                    // A replacement notice may have superseded a profile
                    // picker. Any older hidden action context is obsolete too.
                    self.overlay_flow = None;
                    self.cancel_pending_hook_change(request_id);
                    self.cancel_pending_revert(request_id);
                }
            }
            InfoOverlayAction::Choose {
                request_id,
                choice_id,
            } => {
                let choice_is_current = self.info_overlay.choice(request_id, &choice_id).is_some();
                if !choice_is_current {
                    return;
                }
                if self.handle_kitty_download_choice(request_id, &choice_id) {
                    return;
                }
                if self.handle_kitty_upload_choice(request_id, &choice_id) {
                    return;
                }
                if self.handle_font_feature_manager_choice(request_id, &choice_id) {
                    return;
                }
                if self.handle_font_range_manager_choice(request_id, &choice_id) {
                    return;
                }
                if self.handle_runtime_plugin_manager_choice(request_id, &choice_id) {
                    return;
                }
                if self.handle_agent_hook_choice(request_id, &choice_id) {
                    return;
                }
                if self.handle_revert_choice(request_id, &choice_id) {
                    return;
                }
                if self.handle_config_reset_choice(request_id, &choice_id) {
                    return;
                }
                if self.handle_shortcut_reassign_choice(request_id, &choice_id) {
                    return;
                }
                if self.handle_workspace_choice(request_id, &choice_id) {
                    return;
                }
                if self
                    .pending_file_upload()
                    .is_some_and(|pending| pending.request_id == request_id)
                    && matches!(
                        choice_id.as_str(),
                        "file-upload-confirm" | "file-upload-cancel"
                    )
                {
                    self.info_overlay.dismiss(request_id);
                    let upload = self.take_pending_file_upload();
                    if choice_id == "file-upload-confirm" {
                        if let Some(upload) = upload {
                            self.start_file_upload(upload);
                        }
                    }
                }
            }
            InfoOverlayAction::SubmitInput { request_id } => {
                if !self.submit_font_feature_manager_input(request_id)
                    && !self.submit_font_range_manager_input(request_id)
                    && !self.submit_runtime_plugin_manager_input(request_id)
                    && self.info_overlay.dismiss(request_id)
                {
                    self.dismiss_pending_kitty_download(request_id);
                    self.overlay_flow = None;
                    self.cancel_pending_hook_change(request_id);
                    self.cancel_pending_revert(request_id);
                }
            }
            InfoOverlayAction::CancelInput { request_id } => {
                if !self.cancel_font_feature_manager_input(request_id)
                    && !self.cancel_font_range_manager_input(request_id)
                    && !self.cancel_runtime_plugin_manager_input(request_id)
                    && self.info_overlay.dismiss(request_id)
                {
                    self.dismiss_pending_kitty_download(request_id);
                    self.overlay_flow = None;
                    self.cancel_pending_hook_change(request_id);
                    self.cancel_pending_revert(request_id);
                }
            }
            InfoOverlayAction::ScrollLines { request_id, delta } => {
                self.info_overlay.scroll_lines(request_id, delta);
            }
            InfoOverlayAction::ScrollColumns { request_id, delta } => {
                self.info_overlay.scroll_columns(request_id, delta);
            }
            InfoOverlayAction::ScrollToTop { request_id } => {
                self.info_overlay.scroll_to_top(request_id);
            }
            InfoOverlayAction::ScrollToBottom { request_id } => {
                self.info_overlay.scroll_to_bottom(request_id);
            }
            InfoOverlayAction::ScrollToLeft { request_id } => {
                self.info_overlay.scroll_to_left(request_id);
            }
            InfoOverlayAction::ScrollToRight { request_id } => {
                self.info_overlay.scroll_to_right(request_id);
            }
            InfoOverlayAction::None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::info_overlay::{InfoOverlayChoice, InfoOverlayFocus};
    use winit::event::MouseButton;
    use winit::keyboard::Key;

    const LABEL: &str = "Value: ";

    /// A font-editor-style field holding `value`, with an Unset choice
    /// (Ctrl+D) on the line below it.
    fn show_field(app: &mut App, value: &str) -> u64 {
        app.info_overlay.show_input(
            "Font feature",
            vec![LABEL.into(), "Unset".into()],
            0,
            LABEL,
            value,
            64,
            vec![InfoOverlayChoice {
                id: "unset".into(),
                label: "Unset".into(),
                source_line: 1,
                shortcut: None,
            }],
        )
    }

    #[test]
    fn altgr_types_into_the_field_where_ctrl_c_and_ctrl_d_would_copy_or_unset() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let (clipboard, memory) = super::super::clipboard::Clipboard::in_memory();
        app.clipboard = clipboard;
        let request = show_field(&mut app, "");
        // Some layouts report Ctrl+Alt with AltGr.
        app.modifiers = ModifiersState::CONTROL | ModifiersState::ALT;
        app.alt_graph_active = true;
        for letter in ["d", "c"] {
            app.handle_info_overlay_key(&Key::Character(letter.into()), false);
        }
        assert_eq!(app.info_overlay.input_value(request), Some("dc"));
        assert_eq!(memory.lock().unwrap().clipboard, None);

        // Without AltGr, Ctrl+C is still the field's copy chord.
        app.modifiers = ModifiersState::CONTROL;
        app.alt_graph_active = false;
        app.handle_info_overlay_key(&Key::Character("c".into()), false);
        assert_eq!(app.info_overlay.input_value(request), Some("dc"));
        assert!(memory.lock().unwrap().clipboard.is_some());
    }

    #[test]
    fn a_middle_press_on_the_field_pastes_the_primary_selection_where_it_landed() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let (clipboard, memory) = super::super::clipboard::Clipboard::in_memory();
        memory.lock().unwrap().primary = Some("ginger".into());
        app.clipboard = clipboard;
        let request = show_field(&mut app, "ab");
        app.info_overlay
            .set_focus(request, Some(InfoOverlayFocus::Copy));
        let column = LABEL.chars().count() + 1;

        app.press_info_overlay(
            MouseButton::Middle,
            InfoOverlayPress::Line { line: 0, column },
        );
        assert!(app.info_overlay.input_focused());
        assert_eq!(app.info_overlay.input_value(request), Some("agingerb"));

        // The same limits as a keyboard paste: nothing that breaks the line.
        memory.lock().unwrap().primary = Some("one\ntwo".into());
        app.press_info_overlay(
            MouseButton::Middle,
            InfoOverlayPress::Line { line: 0, column },
        );
        assert_eq!(app.info_overlay.input_value(request), Some("agingerb"));

        // Anywhere but the field, the middle button does nothing at all.
        memory.lock().unwrap().primary = Some("ginger".into());
        for press in [
            InfoOverlayPress::Line { line: 1, column: 0 },
            InfoOverlayPress::CopyButton,
            InfoOverlayPress::ContinueButton,
            InfoOverlayPress::Elsewhere,
        ] {
            app.press_info_overlay(MouseButton::Middle, press);
        }
        assert!(app.info_overlay.is_awaiting(request));
        assert!(app.info_overlay.input_focused());
        assert_eq!(app.info_overlay.input_value(request), Some("agingerb"));
        assert_eq!(memory.lock().unwrap().clipboard, None);
    }
}
