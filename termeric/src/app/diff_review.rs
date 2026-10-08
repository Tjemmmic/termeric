//! Diff review overlay actions, and opening the diff an agent printed in its
//! terminal.

use super::*;

impl App {
    /// Diff review's geometry, shared by drawing, pointer hits, and
    /// accessibility.
    pub(super) fn diff_review_layout(&self) -> crate::ui::diff_review::DiffReviewLayout {
        let metrics = self.ui_metrics();
        crate::ui::diff_review::DiffReviewLayout::calculate(
            metrics.width,
            metrics.height,
            metrics.cell_width,
            metrics.cell_height,
            self.diff_review.animation(),
            self.diff_review.filename(),
            self.diff_review.rows().len(),
            self.diff_review.viewport().content_columns(),
            self.diff_review.has_change_actions(),
        )
    }

    /// Keys for diff review: Tab and Shift+Tab move between its buttons,
    /// Enter and Space press the focused one, Escape is Done, and the
    /// arrows, Page Up/Down, and Home/End scroll the diff.
    pub(super) fn handle_diff_review_key(&mut self, key: &winit::keyboard::Key, repeat: bool) {
        use super::overlay_scroll::overlay_scroll_for_key;
        use crate::ui::diff_review::DiffReviewAction;
        use winit::keyboard::{Key, NamedKey};
        let request_id = self.diff_review.request_id();
        let action = match key {
            Key::Named(NamedKey::Tab) => {
                self.diff_review
                    .cycle_focus(request_id, self.modifiers.shift_key());
                DiffReviewAction::None
            }
            Key::Named(NamedKey::Escape) => DiffReviewAction::Dismiss { request_id },
            // A held key must not press something it did not mean to reach.
            Key::Named(NamedKey::Enter | NamedKey::Space) if repeat => DiffReviewAction::None,
            Key::Named(NamedKey::Enter | NamedKey::Space) => {
                self.diff_review.activate_focused(request_id)
            }
            _ => overlay_scroll_for_key(key, self.modifiers, self.diff_review.viewport().rows())
                .map_or(DiffReviewAction::None, |by| DiffReviewAction::Scroll {
                    request_id,
                    by,
                }),
        };
        self.apply_diff_review_action(action);
        self.request_redraw(RedrawReason::KeyboardInput);
    }

    /// A primary press on diff review: a button takes focus and is pressed.
    pub(super) fn press_diff_review(&mut self, x: f32, y: f32) {
        use crate::ui::diff_review::{DiffReviewAction, DiffReviewFocus};
        let request_id = self.diff_review.request_id();
        let Some(button) = self.diff_review_layout().button_at(x, y) else {
            return;
        };
        let action = if self
            .diff_review
            .set_focus(request_id, DiffReviewFocus::Button(button))
        {
            button.action(request_id)
        } else {
            DiffReviewAction::None
        };
        self.apply_diff_review_action(action);
    }

    pub(super) fn apply_diff_review_action(
        &mut self,
        action: crate::ui::diff_review::DiffReviewAction,
    ) {
        use crate::ui::diff_review::DiffReviewAction;

        match action {
            DiffReviewAction::Stage { request_id } => self.stage_reviewed_change(request_id),
            DiffReviewAction::Revert { request_id } => {
                self.confirm_revert_reviewed_change(request_id)
            }
            DiffReviewAction::Copy { request_id } => {
                // An agent's change carries its own diff; a diff seen in
                // the terminal is read back from the pane that showed it.
                if let Some(text) = self.diff_review.change_text(request_id) {
                    let text = text.to_owned();
                    self.copy_to_clipboard(text, "Copied diff to clipboard");
                    return;
                }
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
            DiffReviewAction::Scroll { request_id, by } => {
                self.diff_review.scroll(request_id, by);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::diff_detector::{DetectedDiff, DiffHunk, DiffLine};
    use crate::ui::diff_review::DiffReviewButton;
    use winit::keyboard::{Key, ModifiersState, NamedKey};

    fn show_wide_diff(app: &mut App) -> u64 {
        app.diff_review.set_reduced_motion(true);
        let request = app.diff_review.show(
            &DetectedDiff {
                id: 1,
                filename: "src/fixture.rs".into(),
                hunks: vec![DiffHunk {
                    header: "@@ -1,60 +1,60 @@".into(),
                    lines: (0..60)
                        .map(|index| DiffLine::Added(format!("row {index} {}", "x".repeat(300))))
                        .collect(),
                }],
                raw: "--- a/src/fixture.rs\n+++ b/src/fixture.rs".into(),
                reviewed: false,
            },
            usize::MAX,
            crate::panes::PaneId(0),
        );
        let layout = app.diff_review_layout();
        app.diff_review
            .set_viewport(layout.rows_that_fit, layout.columns_that_fit);
        request
    }

    fn at(app: &App) -> (usize, usize) {
        let viewport = app.diff_review.viewport();
        (viewport.row(), viewport.column())
    }

    #[test]
    fn letter_keys_do_nothing_and_the_arrows_still_scroll() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let request = show_wide_diff(&mut app);
        for letter in ["s", "r", "c", "j", "k", "S", "C"] {
            app.handle_diff_review_key(&Key::Character(letter.into()), false);
        }
        assert!(app.diff_review.is_awaiting(request));
        assert_eq!(at(&app), (0, 0), "no letter scrolls");
        assert_eq!(
            app.diff_review.focused_button(),
            Some(DiffReviewButton::Done)
        );

        app.handle_diff_review_key(&Key::Named(NamedKey::ArrowDown), false);
        app.handle_diff_review_key(&Key::Named(NamedKey::ArrowRight), false);
        assert_eq!(at(&app), (1, 4));
        app.handle_diff_review_key(&Key::Named(NamedKey::PageDown), false);
        assert!(at(&app).0 > 1);
        app.handle_diff_review_key(&Key::Named(NamedKey::End), false);
        assert_eq!(at(&app).1, app.diff_review.viewport().max_column());
        app.modifiers = ModifiersState::CONTROL;
        app.handle_diff_review_key(&Key::Named(NamedKey::Home), false);
        app.modifiers = ModifiersState::empty();
        assert_eq!(at(&app).0, 0);
        assert!(
            app.diff_review.is_awaiting(request),
            "scrolling never answers"
        );
    }

    #[test]
    fn tab_moves_between_buttons_and_enter_or_space_presses_the_focused_one() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let request = show_wide_diff(&mut app);
        app.handle_diff_review_key(&Key::Named(NamedKey::Tab), false);
        assert_eq!(
            app.diff_review.focused_button(),
            Some(DiffReviewButton::Copy)
        );
        // Copy of a diff whose pane is gone warns and keeps the review open.
        app.handle_diff_review_key(&Key::Named(NamedKey::Enter), false);
        app.handle_diff_review_key(&Key::Named(NamedKey::Space), false);
        assert!(app.diff_review.is_awaiting(request));

        app.modifiers = ModifiersState::SHIFT;
        app.handle_diff_review_key(&Key::Named(NamedKey::Tab), false);
        app.modifiers = ModifiersState::empty();
        assert_eq!(
            app.diff_review.focused_button(),
            Some(DiffReviewButton::Done)
        );
        app.handle_diff_review_key(&Key::Named(NamedKey::Space), true);
        app.handle_diff_review_key(&Key::Named(NamedKey::Enter), true);
        assert!(
            app.diff_review.is_awaiting(request),
            "a held key presses nothing"
        );
        app.handle_diff_review_key(&Key::Named(NamedKey::Space), false);
        assert!(!app.diff_review.is_awaiting(request));
    }

    #[test]
    fn escape_is_done_wherever_focus_is_and_a_press_focuses_its_button() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let request = show_wide_diff(&mut app);
        let copy = app.diff_review_layout().copy_button;
        app.press_diff_review(copy.x + 1.0, copy.y + 1.0);
        assert_eq!(
            app.diff_review.focused_button(),
            Some(DiffReviewButton::Copy)
        );
        assert!(app.diff_review.is_awaiting(request));
        app.handle_diff_review_key(&Key::Named(NamedKey::Escape), false);
        assert!(!app.diff_review.is_awaiting(request));
    }
}
