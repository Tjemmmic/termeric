//! Info overlay actions: scrolling, focus, text entry, and choices, routed
//! back to the flow that opened the overlay.

use super::text_field_keys::FieldKey;
use super::*;

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
                        if self.modifiers.control_key()
                            && character.as_str().eq_ignore_ascii_case("c") =>
                    {
                        InfoOverlayAction::Copy { request_id }
                    }
                    Key::Character(character)
                        if self.modifiers.control_key()
                            && character.as_str().eq_ignore_ascii_case("d") =>
                    {
                        self.info_overlay.choice(request_id, "unset").map_or(
                            InfoOverlayAction::None,
                            |_| InfoOverlayAction::Choose {
                                request_id,
                                choice_id: "unset".into(),
                            },
                        )
                    }
                    Key::Character(character)
                        if !self.modifiers.control_key()
                            && !self.modifiers.super_key()
                            && !self.modifiers.alt_key() =>
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
