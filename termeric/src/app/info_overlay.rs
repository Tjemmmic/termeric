//! Info overlay actions: scrolling, focus, text entry, and choices, routed
//! back to the flow that opened the overlay.

use super::*;

impl App {
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
