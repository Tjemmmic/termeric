//! Bounded, request-scoped review surface for unified diffs: one detected in
//! a terminal's output, or one file an agent changed, which can also be
//! staged or reverted from here.

use super::animation::OverlayAnimation;
use super::text_card::{TextCard, TextViewport};
use crate::terminal::diff_detector::{DetectedDiff, DiffLine};
use crate::ui::geometry::Bounds;

pub const MAX_DIFF_FILENAME_CHARS: usize = 512;
pub const MAX_DIFF_REVIEW_ROWS: usize = 4096;
pub const MAX_DIFF_REVIEW_LINE_CHARS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffReviewRow {
    Header(String),
    Context(String),
    Added(String),
    Removed(String),
    Blank,
    Omitted(String),
}

impl DiffReviewRow {
    pub fn text(&self) -> &str {
        match self {
            Self::Header(text)
            | Self::Context(text)
            | Self::Added(text)
            | Self::Removed(text)
            | Self::Omitted(text) => text,
            Self::Blank => "",
        }
    }

    pub fn prefix(&self) -> &str {
        match self {
            Self::Context(_) => "  ",
            Self::Added(_) => "+ ",
            Self::Removed(_) => "- ",
            Self::Header(_) | Self::Blank | Self::Omitted(_) => "",
        }
    }

    pub fn display_text(&self) -> String {
        format!("{}{}", self.prefix(), self.text())
    }
}

/// Whose agent made a reviewed change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeOwner {
    /// An agent running in a terminal.
    Terminal(crate::tabs::TerminalIdentity),
    /// A session in the docked agent view.
    AgentView(crate::agent_view::session::SessionId),
}

/// The agent-changed file a review shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeReviewTarget {
    pub owner: ChangeOwner,
    /// Relative to the repository root.
    pub path: std::path::PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffReviewTarget {
    pub tab_id: usize,
    pub pane_id: crate::panes::PaneId,
    pub diff_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffReviewAction {
    Copy {
        request_id: u64,
    },
    /// Stage the reviewed agent change.
    Stage {
        request_id: u64,
    },
    /// Ask to put the reviewed file back as it was when the agent started.
    Revert {
        request_id: u64,
    },
    Dismiss {
        request_id: u64,
    },
    ScrollRows {
        request_id: u64,
        delta: isize,
    },
    ScrollColumns {
        request_id: u64,
        delta: isize,
    },
    ScrollToTop {
        request_id: u64,
    },
    ScrollToBottom {
        request_id: u64,
    },
    ScrollToLeft {
        request_id: u64,
    },
    ScrollToRight {
        request_id: u64,
    },
    None,
}

/// Exact geometry shared by rendering, pointer input, viewport sizing, and
/// accessibility. The card and its controls remain inside small windows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiffReviewLayout {
    pub card: Bounds,
    pub content: Bounds,
    pub copy_button: Bounds,
    pub done_button: Bounds,
    /// Present only when reviewing an agent's change.
    pub stage_button: Option<Bounds>,
    pub revert_button: Option<Bounds>,
    pub padding: f32,
    pub title_height: f32,
    pub line_height: f32,
    pub char_scale: f32,
    pub footer_height: f32,
    pub rows_that_fit: usize,
    pub columns_that_fit: usize,
    pub scrollbar_x: f32,
}

impl DiffReviewLayout {
    pub fn calculate(
        window_width: f32,
        window_height: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
        filename: &str,
        total_rows: usize,
        max_line_chars: usize,
        change_actions: bool,
    ) -> Self {
        let text = TextCard::calculate(
            window_width,
            window_height,
            cell_width,
            cell_height,
            animation,
            max_line_chars.clamp(50, 100),
            "Diff: ".chars().count() + filename.chars().count(),
            500.0,
            total_rows,
        );
        let row_x = text.card.x + text.padding;
        let (stage_button, revert_button, copy_button, done_button) = if change_actions {
            // Stage, Revert, and Copy share the row equally with a wider Done.
            let available = text.button_row_width(3).max(4.0);
            let small = available * 0.22;
            let done = available - small * 3.0;
            let step = small + text.button_gap;
            (
                Some(text.button(row_x, small)),
                Some(text.button(row_x + step, small)),
                text.button(row_x + step * 2.0, small),
                text.button(row_x + step * 3.0, done),
            )
        } else {
            let available = text.button_row_width(1).max(2.0);
            let copy = available * 0.38;
            (
                None,
                None,
                text.button(row_x, copy),
                text.button(row_x + copy + text.button_gap, available - copy),
            )
        };

        Self {
            card: text.card,
            content: text.content,
            copy_button,
            done_button,
            stage_button,
            revert_button,
            padding: text.padding,
            title_height: text.title_height,
            line_height: text.line_height,
            char_scale: text.char_scale,
            footer_height: text.footer_height,
            rows_that_fit: text.rows_that_fit,
            columns_that_fit: text.columns_that_fit,
            scrollbar_x: text.scrollbar_x,
        }
    }

    pub fn hit_test(&self, x: f32, y: f32, request_id: u64) -> DiffReviewAction {
        if self
            .stage_button
            .is_some_and(|button| button.contains(x, y))
        {
            DiffReviewAction::Stage { request_id }
        } else if self
            .revert_button
            .is_some_and(|button| button.contains(x, y))
        {
            DiffReviewAction::Revert { request_id }
        } else if self.copy_button.contains(x, y) {
            DiffReviewAction::Copy { request_id }
        } else if self.done_button.contains(x, y) {
            DiffReviewAction::Dismiss { request_id }
        } else {
            DiffReviewAction::None
        }
    }
}

/// State and action authority for one displayed diff.
pub struct DiffReviewOverlay {
    animation: OverlayAnimation,
    filename: String,
    rows: Vec<DiffReviewRow>,
    target: Option<DiffReviewTarget>,
    /// Set instead of `target` when reviewing an agent's change.
    change: Option<ChangeReviewTarget>,
    /// The complete unified diff of that change, for Copy.
    change_text: String,
    request_id: u64,
    accepting: bool,
    viewport: TextViewport,
}

impl DiffReviewOverlay {
    pub fn new() -> Self {
        Self {
            animation: OverlayAnimation::new(),
            filename: String::new(),
            rows: Vec::new(),
            target: None,
            change: None,
            change_text: String::new(),
            request_id: 0,
            accepting: false,
            viewport: TextViewport::default(),
        }
    }

    pub fn show(
        &mut self,
        diff: &DetectedDiff,
        tab_id: usize,
        pane_id: crate::panes::PaneId,
    ) -> u64 {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.filename = crate::safe_text::display_line(&diff.filename, MAX_DIFF_FILENAME_CHARS, 4);
        self.rows = bounded_rows(diff);
        self.target = Some(DiffReviewTarget {
            tab_id,
            pane_id,
            diff_id: diff.id,
        });
        self.change = None;
        self.change_text.clear();
        self.open_rows()
    }

    /// Review one file an agent changed, given its unified diff.
    pub fn show_change(&mut self, unified: &str, change: ChangeReviewTarget) -> u64 {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.filename = crate::safe_text::display_line(
            &change.path.to_string_lossy(),
            MAX_DIFF_FILENAME_CHARS,
            4,
        );
        self.rows = rows_from_unified(unified);
        self.target = None;
        self.change = Some(change);
        self.change_text = unified.to_owned();
        self.open_rows()
    }

    fn open_rows(&mut self) -> u64 {
        let widest = self
            .rows
            .iter()
            .map(|row| row.prefix().chars().count() + row.text().chars().count())
            .max()
            .unwrap_or(0);
        self.viewport.reset(self.rows.len(), widest);
        self.accepting = true;
        self.animation.open();
        self.request_id
    }

    pub fn dismiss(&mut self, request_id: u64) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        self.accepting = false;
        self.animation.close();
        true
    }

    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    pub fn is_awaiting(&self, request_id: u64) -> bool {
        self.accepting && self.request_id == request_id
    }

    pub fn is_awaiting_current(&self) -> bool {
        self.is_awaiting(self.request_id)
    }

    pub fn target(&self, request_id: u64) -> Option<DiffReviewTarget> {
        self.is_awaiting(request_id)
            .then_some(self.target)
            .flatten()
    }

    /// The agent change under review, while it can still be acted on.
    pub fn change_target(&self, request_id: u64) -> Option<&ChangeReviewTarget> {
        self.is_awaiting(request_id)
            .then_some(self.change.as_ref())
            .flatten()
    }

    /// The reviewed change's complete diff, for Copy.
    pub fn change_text(&self, request_id: u64) -> Option<&str> {
        self.change_target(request_id)
            .map(|_| self.change_text.as_str())
    }

    /// Whether Stage and Revert are offered.
    pub fn has_change_actions(&self) -> bool {
        self.change.is_some()
    }

    pub fn filename(&self) -> &str {
        &self.filename
    }

    pub fn rows(&self) -> &[DiffReviewRow] {
        &self.rows
    }

    pub fn max_line_chars(&self) -> usize {
        self.viewport.content_columns()
    }

    pub fn set_viewport(&mut self, rows: usize, columns: usize) {
        self.viewport.resize(rows, columns);
    }

    pub fn scroll_rows(&mut self, request_id: u64, delta: isize) -> bool {
        self.is_awaiting(request_id) && self.viewport.scroll_by(delta, 0)
    }

    pub fn scroll_columns(&mut self, request_id: u64, delta: isize) -> bool {
        self.is_awaiting(request_id) && self.viewport.scroll_by(0, delta)
    }

    pub fn scroll_to_top(&mut self, request_id: u64) -> bool {
        self.is_awaiting(request_id) && self.viewport.scroll_to(Some(0), None)
    }

    pub fn scroll_to_bottom(&mut self, request_id: u64) -> bool {
        let bottom = self.viewport.max_row();
        self.is_awaiting(request_id) && self.viewport.scroll_to(Some(bottom), None)
    }

    pub fn scroll_to_left(&mut self, request_id: u64) -> bool {
        self.is_awaiting(request_id) && self.viewport.scroll_to(None, Some(0))
    }

    pub fn scroll_to_right(&mut self, request_id: u64) -> bool {
        let right = self.viewport.max_column();
        self.is_awaiting(request_id) && self.viewport.scroll_to(None, Some(right))
    }

    pub fn scroll_offset(&self) -> usize {
        self.viewport.row()
    }

    pub fn horizontal_offset(&self) -> usize {
        self.viewport.column()
    }

    pub fn viewport_rows(&self) -> usize {
        self.viewport.rows()
    }

    pub fn viewport_columns(&self) -> usize {
        self.viewport.columns()
    }

    pub fn max_scroll_offset(&self) -> usize {
        self.viewport.max_row()
    }

    pub fn max_horizontal_offset(&self) -> usize {
        self.viewport.max_column()
    }

    pub fn visible_rows(&self) -> impl Iterator<Item = (usize, &DiffReviewRow)> {
        self.rows
            .iter()
            .enumerate()
            .skip(self.viewport.row())
            .take(self.viewport.rows())
    }

    pub fn is_active(&self) -> bool {
        self.animation.is_active()
    }

    pub fn is_visible(&self) -> bool {
        self.animation.is_visible()
    }

    pub fn animation(&self) -> f32 {
        self.animation.eased()
    }

    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.animation.set_reduced_motion(reduced_motion);
    }

    pub fn update_animation(&mut self) -> bool {
        let animating = self.animation.update();
        if !self.animation.is_active() && !self.animation.is_visible() {
            self.filename.clear();
            self.rows.clear();
            self.target = None;
            self.change = None;
            self.change_text.clear();
            self.viewport.reset(0, 0);
        }
        animating
    }
}

fn bounded_rows(diff: &DetectedDiff) -> Vec<DiffReviewRow> {
    let mut rows = Vec::new();
    'hunks: for (hunk_index, hunk) in diff.hunks.iter().enumerate() {
        if !push_bounded_row(
            &mut rows,
            DiffReviewRow::Header(crate::safe_text::display_line(
                &hunk.header,
                MAX_DIFF_REVIEW_LINE_CHARS,
                4,
            )),
        ) {
            break;
        }
        for line in &hunk.lines {
            let row = match line {
                DiffLine::Context(text) => DiffReviewRow::Context(crate::safe_text::display_line(
                    text,
                    MAX_DIFF_REVIEW_LINE_CHARS.saturating_sub(2),
                    4,
                )),
                DiffLine::Added(text) => DiffReviewRow::Added(crate::safe_text::display_line(
                    text,
                    MAX_DIFF_REVIEW_LINE_CHARS.saturating_sub(2),
                    4,
                )),
                DiffLine::Removed(text) => DiffReviewRow::Removed(crate::safe_text::display_line(
                    text,
                    MAX_DIFF_REVIEW_LINE_CHARS.saturating_sub(2),
                    4,
                )),
            };
            if !push_bounded_row(&mut rows, row) {
                break 'hunks;
            }
        }
        if hunk_index + 1 < diff.hunks.len() && !push_bounded_row(&mut rows, DiffReviewRow::Blank) {
            break;
        }
    }
    if rows.is_empty() {
        rows.push(DiffReviewRow::Context("No diff hunks available".into()));
    }
    rows
}

/// Rows for a unified diff as Git prints it. The file headers are dropped:
/// the title already names the file.
fn rows_from_unified(text: &str) -> Vec<DiffReviewRow> {
    let body = |line: &str| {
        crate::safe_text::display_line(&line[1..], MAX_DIFF_REVIEW_LINE_CHARS.saturating_sub(2), 4)
    };
    let mut rows = Vec::new();
    let mut in_hunk = false;
    for line in text.lines() {
        let row = if line.starts_with("@@") {
            if in_hunk && !push_bounded_row(&mut rows, DiffReviewRow::Blank) {
                break;
            }
            in_hunk = true;
            DiffReviewRow::Header(crate::safe_text::display_line(
                line,
                MAX_DIFF_REVIEW_LINE_CHARS,
                4,
            ))
        } else if !in_hunk {
            if line.starts_with("Binary files") {
                DiffReviewRow::Omitted("Binary file; its content is not shown".into())
            } else {
                continue;
            }
        } else if line.starts_with('+') {
            DiffReviewRow::Added(body(line))
        } else if line.starts_with('-') {
            DiffReviewRow::Removed(body(line))
        } else if line.starts_with(' ') {
            DiffReviewRow::Context(body(line))
        } else {
            // "\ No newline at end of file" and similar notes.
            DiffReviewRow::Omitted(crate::safe_text::display_line(
                line,
                MAX_DIFF_REVIEW_LINE_CHARS,
                4,
            ))
        };
        if !push_bounded_row(&mut rows, row) {
            break;
        }
    }
    if rows.is_empty() {
        rows.push(DiffReviewRow::Context(
            "No differences remain in this file".into(),
        ));
    }
    rows
}

fn push_bounded_row(rows: &mut Vec<DiffReviewRow>, row: DiffReviewRow) -> bool {
    if rows.len() + 1 < MAX_DIFF_REVIEW_ROWS {
        rows.push(row);
        return true;
    }
    if rows.len() < MAX_DIFF_REVIEW_ROWS {
        rows.push(DiffReviewRow::Omitted(
            "… additional diff rows omitted from the review view; Copy retains the raw diff".into(),
        ));
    }
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn explicit_animation_phases_translate_content_and_controls_together() {
        let calculate = |phase| {
            super::DiffReviewLayout::calculate(
                900.0,
                500.0,
                10.0,
                20.0,
                phase,
                "fixture.rs",
                40,
                80,
                false,
            )
        };
        let settled = calculate(1.0);
        for phase in [0.0, 0.125, 0.5, 0.875, 1.0, 0.875, 0.5, 0.125, 0.0] {
            let layout = calculate(phase);
            let translation = (1.0 - phase) * -20.0;
            assert_eq!(layout.rows_that_fit, settled.rows_that_fit);
            assert_eq!(layout.columns_that_fit, settled.columns_that_fit);
            for (actual, expected) in [
                (layout.card, settled.card),
                (layout.content, settled.content),
                (layout.copy_button, settled.copy_button),
                (layout.done_button, settled.done_button),
            ] {
                assert_eq!(actual.x, expected.x);
                assert_eq!(actual.width, expected.width);
                assert_eq!(actual.height, expected.height);
                assert!((actual.y - expected.y - translation).abs() < 0.001);
                assert!(actual.contains(
                    actual.x + actual.width / 2.0,
                    actual.y + actual.height / 2.0
                ));
            }
        }
    }
    use super::*;
    use crate::terminal::diff_detector::DiffHunk;

    fn fixture(id: u64, line_count: usize, line_width: usize) -> DetectedDiff {
        DetectedDiff {
            id,
            filename: "src/fixture.rs".into(),
            hunks: vec![DiffHunk {
                header: "@@ -1 +1 @@".into(),
                lines: (0..line_count)
                    .map(|_| DiffLine::Added("x".repeat(line_width)))
                    .collect(),
            }],
            raw: "--- a/src/fixture.rs\n+++ b/src/fixture.rs".into(),
            reviewed: false,
        }
    }

    #[test]
    fn layout_is_bounded_and_buttons_share_hit_testing() {
        let layout = DiffReviewLayout::calculate(
            900.0,
            500.0,
            10.0,
            20.0,
            1.0,
            "fixture.rs",
            50_000,
            50_000,
            false,
        );
        assert!(layout.stage_button.is_none() && layout.revert_button.is_none());
        assert!(layout.card.height < 500.0);
        assert!(layout.card.width < 900.0);
        assert_eq!(
            layout.hit_test(layout.done_button.x + 1.0, layout.done_button.y + 1.0, 7),
            DiffReviewAction::Dismiss { request_id: 7 }
        );
    }

    #[test]
    fn rows_and_line_width_are_bounded_without_consuming_source_raw_text() {
        let mut overlay = DiffReviewOverlay::new();
        let diff = fixture(
            4,
            MAX_DIFF_REVIEW_ROWS + 50,
            MAX_DIFF_REVIEW_LINE_CHARS + 50,
        );
        let raw = diff.raw.clone();
        let request = overlay.show(&diff, 12, crate::panes::PaneId(0));
        assert_eq!(overlay.rows().len(), MAX_DIFF_REVIEW_ROWS);
        assert_eq!(overlay.max_line_chars(), MAX_DIFF_REVIEW_LINE_CHARS);
        assert!(matches!(
            overlay.rows().last(),
            Some(DiffReviewRow::Omitted(_))
        ));
        assert_eq!(overlay.target(request).unwrap().diff_id, 4);
        assert_eq!(diff.raw, raw);
    }

    #[test]
    fn two_axis_scroll_and_dismiss_are_request_scoped() {
        let mut overlay = DiffReviewOverlay::new();
        let first_diff = fixture(1, 20, 100);
        let first = overlay.show(&first_diff, 9, crate::panes::PaneId(0));
        overlay.set_viewport(4, 10);
        assert!(overlay.scroll_rows(first, 100));
        assert_eq!(overlay.scroll_offset(), overlay.max_scroll_offset());
        assert!(overlay.scroll_columns(first, 100));
        assert_eq!(overlay.horizontal_offset(), overlay.max_horizontal_offset());

        let current_diff = fixture(2, 1, 1);
        let current = overlay.show(&current_diff, 10, crate::panes::PaneId(0));
        assert!(!overlay.dismiss(first));
        assert!(overlay.dismiss(current));
        assert!(overlay.is_active());
        assert!(!overlay.is_awaiting(current));
    }

    #[test]
    fn change_reviews_add_stage_and_revert_inside_the_card() {
        let layout =
            DiffReviewLayout::calculate(900.0, 500.0, 10.0, 20.0, 1.0, "src/lib.rs", 10, 40, true);
        let stage = layout.stage_button.unwrap();
        let revert = layout.revert_button.unwrap();
        let buttons = [stage, revert, layout.copy_button, layout.done_button];
        for pair in buttons.windows(2) {
            assert!(
                pair[0].x + pair[0].width < pair[1].x,
                "buttons do not overlap"
            );
        }
        let last = layout.done_button;
        assert!(last.x + last.width <= layout.card.x + layout.card.width);
        let centre = |rect: Bounds| (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        let hit = |rect| {
            let (x, y) = centre(rect);
            layout.hit_test(x, y, 3)
        };
        assert_eq!(hit(stage), DiffReviewAction::Stage { request_id: 3 });
        assert_eq!(hit(revert), DiffReviewAction::Revert { request_id: 3 });
        assert_eq!(
            hit(layout.copy_button),
            DiffReviewAction::Copy { request_id: 3 }
        );
    }

    #[test]
    fn change_reviews_parse_git_patches_and_expire_with_the_review() {
        let mut overlay = DiffReviewOverlay::new();
        let target = ChangeReviewTarget {
            owner: ChangeOwner::Terminal(crate::tabs::TerminalIdentity {
                tab_id: 1,
                pane_id: crate::panes::PaneId(0),
            }),
            path: "src/lib.rs".into(),
        };
        let patch = "diff --git a/src/lib.rs b/src/lib.rs\nindex 1..2 100644\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,2 @@\n keep\n-old\n+new\n@@ -9 +9 @@\n-a\n+b\n\\ No newline at end of file\n";
        let request = overlay.show_change(patch, target.clone());
        assert!(overlay.has_change_actions());
        assert_eq!(overlay.filename(), "src/lib.rs");
        assert_eq!(
            overlay.rows(),
            [
                DiffReviewRow::Header("@@ -1,2 +1,2 @@".into()),
                DiffReviewRow::Context("keep".into()),
                DiffReviewRow::Removed("old".into()),
                DiffReviewRow::Added("new".into()),
                DiffReviewRow::Blank,
                DiffReviewRow::Header("@@ -9 +9 @@".into()),
                DiffReviewRow::Removed("a".into()),
                DiffReviewRow::Added("b".into()),
                DiffReviewRow::Omitted("\\ No newline at end of file".into()),
            ]
        );
        assert_eq!(overlay.change_target(request), Some(&target));
        assert_eq!(overlay.change_text(request), Some(patch));
        assert!(
            overlay.target(request).is_none(),
            "no detected-diff authority"
        );
        assert!(overlay.dismiss(request));
        assert_eq!(overlay.change_target(request), None);

        let request = overlay.show_change("Binary files a/x and b/x differ\n", target);
        assert_eq!(
            overlay.rows(),
            [DiffReviewRow::Omitted(
                "Binary file; its content is not shown".into()
            )]
        );
        assert!(overlay.change_text(request).is_some());
    }
}
