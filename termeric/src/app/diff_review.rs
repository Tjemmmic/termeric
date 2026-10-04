//! Diff review overlay actions, and opening the diff an agent printed in its
//! terminal.

use super::*;

impl App {
    /// Keys for diff review: scrolling, copy, and staging or reverting an agent change.
    pub(super) fn handle_diff_review_key(&mut self, key: &winit::keyboard::Key) {
        use super::overlay_scroll::{OverlayScroll, overlay_scroll_for_key};
        use crate::ui::diff_review::DiffReviewAction;
        use winit::keyboard::{Key, NamedKey};
        let request_id = self.diff_review.request_id();
        let scroll = overlay_scroll_for_key(key, self.modifiers, self.diff_review.viewport_rows());
        let action = match (scroll, key) {
            (Some(OverlayScroll::Rows(delta)), _) => {
                DiffReviewAction::ScrollRows { request_id, delta }
            }
            (Some(OverlayScroll::Columns(delta)), _) => {
                DiffReviewAction::ScrollColumns { request_id, delta }
            }
            (Some(OverlayScroll::Top), _) => DiffReviewAction::ScrollToTop { request_id },
            (Some(OverlayScroll::Bottom), _) => DiffReviewAction::ScrollToBottom { request_id },
            (Some(OverlayScroll::LineStart), _) => DiffReviewAction::ScrollToLeft { request_id },
            (Some(OverlayScroll::LineEnd), _) => DiffReviewAction::ScrollToRight { request_id },
            (None, Key::Named(NamedKey::Enter | NamedKey::Escape)) => {
                DiffReviewAction::Dismiss { request_id }
            }
            (None, Key::Character(key)) if key.as_str().eq_ignore_ascii_case("k") => {
                DiffReviewAction::ScrollRows {
                    request_id,
                    delta: -3,
                }
            }
            (None, Key::Character(key)) if key.as_str().eq_ignore_ascii_case("j") => {
                DiffReviewAction::ScrollRows {
                    request_id,
                    delta: 3,
                }
            }
            (None, Key::Character(key)) if key.as_str().eq_ignore_ascii_case("c") => {
                DiffReviewAction::Copy { request_id }
            }
            (None, Key::Character(key))
                if key.as_str().eq_ignore_ascii_case("s")
                    && self.diff_review.has_change_actions() =>
            {
                DiffReviewAction::Stage { request_id }
            }
            (None, Key::Character(key))
                if key.as_str().eq_ignore_ascii_case("r")
                    && self.diff_review.has_change_actions() =>
            {
                DiffReviewAction::Revert { request_id }
            }
            _ => DiffReviewAction::None,
        };
        self.apply_diff_review_action(action);
        self.request_redraw(RedrawReason::KeyboardInput);
    }

    pub(super) fn apply_diff_review_action(
        &mut self,
        action: crate::ui::diff_review::DiffReviewAction,
    ) {
        use crate::ui::diff_review::DiffReviewAction;

        match action {
            DiffReviewAction::Copy { request_id }
                if self.diff_review.change_text(request_id).is_some() =>
            {
                let text = self.diff_review.change_text(request_id).unwrap_or_default();
                self.copy_to_clipboard(text.to_owned(), "Copied diff to clipboard");
            }
            DiffReviewAction::Stage { request_id } => self.stage_reviewed_change(request_id),
            DiffReviewAction::Revert { request_id } => {
                self.confirm_revert_reviewed_change(request_id)
            }
            DiffReviewAction::Copy { request_id } => {
                let raw = self.diff_review.target(request_id).and_then(|target| {
                    let tabs = self.tabs.as_ref()?;
                    tabs.origin_for_pane(target.tab_id, target.pane_id)?;
                    tabs.tab_by_id(target.tab_id)?
                        .pane_integration(target.pane_id)?
                        .diff_detector
                        .detected_by_id(target.diff_id)
                        .map(|diff| diff.raw.clone())
                });
                if let Some(raw) = raw {
                    self.copy_to_clipboard(raw, "Copied diff to clipboard");
                } else if self.diff_review.is_awaiting(request_id) {
                    self.toasts
                        .warning("The reviewed diff is no longer available");
                }
            }
            DiffReviewAction::Dismiss { request_id } => {
                let target = self.diff_review.target(request_id);
                if self.diff_review.dismiss(request_id)
                    && let (Some(target), Some(tabs)) = (target, &mut self.tabs)
                    && tabs
                        .origin_for_pane(target.tab_id, target.pane_id)
                        .is_some()
                    && let Some(tab) = tabs.tab_mut_by_id(target.tab_id)
                    && let Some(state) = tab.pane_integration_mut(target.pane_id)
                {
                    state.diff_detector.mark_reviewed(target.diff_id);
                }
            }
            DiffReviewAction::ScrollRows { request_id, delta } => {
                self.diff_review.scroll_rows(request_id, delta);
            }
            DiffReviewAction::ScrollColumns { request_id, delta } => {
                self.diff_review.scroll_columns(request_id, delta);
            }
            DiffReviewAction::ScrollToTop { request_id } => {
                self.diff_review.scroll_to_top(request_id);
            }
            DiffReviewAction::ScrollToBottom { request_id } => {
                self.diff_review.scroll_to_bottom(request_id);
            }
            DiffReviewAction::ScrollToLeft { request_id } => {
                self.diff_review.scroll_to_left(request_id);
            }
            DiffReviewAction::ScrollToRight { request_id } => {
                self.diff_review.scroll_to_right(request_id);
            }
            DiffReviewAction::None => {}
        }
    }

    pub(super) fn selected_diff(
        &self,
        identity: crate::tabs::TerminalIdentity,
    ) -> Option<crate::terminal::diff_detector::DetectedDiff> {
        let tabs = self.tabs.as_ref()?;
        tabs.origin_for_pane(identity.tab_id, identity.pane_id)?;
        Some(
            tabs.tab_by_id(identity.tab_id)?
                .pane_integration(identity.pane_id)?
                .diff_detector
                .latest_unreviewed()?
                .clone(),
        )
    }

    pub(super) fn open_agent_diff_review(&mut self, identity: crate::tabs::TerminalIdentity) {
        if let Some(diff) = self.selected_diff(identity) {
            self.diff_review
                .show(&diff, identity.tab_id, identity.pane_id);
        } else {
            self.toasts.info("No unreviewed diffs");
        }
    }

    pub(super) fn copy_agent_diff(&mut self, identity: crate::tabs::TerminalIdentity) {
        if let Some(diff) = self.selected_diff(identity) {
            self.copy_to_clipboard(diff.raw, "Copied diff");
        } else {
            self.toasts.info("No unreviewed diffs to copy");
        }
    }
}
