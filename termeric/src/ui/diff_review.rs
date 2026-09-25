//! Bounded, request-scoped review surface for unified diffs: one detected in
//! a terminal's output, or one file an agent changed, which can also be
//! staged or reverted from here.

use super::animation::OverlayAnimation;
use super::constants::CONTENT_CHAR_SCALE;
use crate::terminal::diff_detector::{DetectedDiff, DiffLine};

pub const MAX_DIFF_FILENAME_CHARS: usize = 512;
pub const MAX_DIFF_REVIEW_ROWS: usize = 4096;
pub const MAX_DIFF_REVIEW_LINE_CHARS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiffReviewRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl DiffReviewRect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }
}

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
    pub card: DiffReviewRect,
    pub content: DiffReviewRect,
    pub copy_button: DiffReviewRect,
    pub done_button: DiffReviewRect,
    /// Present only when reviewing an agent's change.
    pub stage_button: Option<DiffReviewRect>,
    pub revert_button: Option<DiffReviewRect>,
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
    #[allow(clippy::too_many_arguments)]
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
        let char_scale = (cell_width * CONTENT_CHAR_SCALE).max(1.0);
        let padding = (cell_width * 1.5).max(10.0);
        let line_height = (cell_height * 1.4).max(1.0);
        let title_height = cell_height * 2.5;
        let footer_height = cell_height * 4.2;
        let outer_margin = (cell_width * 2.0).max(12.0);

        let title_width = ("Diff: ".chars().count() + filename.chars().count()) as f32 * char_scale;
        let preferred_columns = max_line_chars.clamp(50, 100);
        let preferred_width = (preferred_columns as f32 * char_scale).max(title_width)
            + padding * 2.0
            + cell_width.max(6.0);
        let available_width = (window_width - outer_margin * 2.0).max(1.0);
        let card_width = preferred_width.max(500.0).min(available_width);

        let fixed_height = padding + title_height + footer_height + padding * 0.5;
        let available_height = (window_height - outer_margin * 2.0).max(line_height);
        let content_budget = (available_height - fixed_height).max(line_height);
        let rows_that_fit = ((content_budget / line_height).floor() as usize).max(1);
        let visible_rows = total_rows.max(1).min(rows_that_fit);
        let content_height = visible_rows as f32 * line_height;
        let card_height = fixed_height + content_height;
        let card_x = (window_width - card_width) / 2.0;
        let card_y = (window_height - card_height) / 2.0 + (1.0 - animation) * -20.0;

        let content_x = card_x + padding;
        let content_y = card_y + padding + title_height;
        let scrollbar_gutter = cell_width.max(6.0);
        let content_width = (card_width - padding * 2.0 - scrollbar_gutter).max(char_scale);
        let columns_that_fit = ((content_width / char_scale).floor() as usize).max(1);

        let button_height = cell_height * 1.8;
        let button_gap = cell_width * 1.5;
        let button_y = content_y + content_height + cell_height * 1.15;
        let button = |x: f32, width: f32| DiffReviewRect {
            x,
            y: button_y,
            width: width.max(1.0),
            height: button_height,
        };
        let row_x = card_x + padding;
        let (stage_button, revert_button, copy_button, done_button) = if change_actions {
            // Stage, Revert, and Copy share the row equally with a wider Done.
            let available = (card_width - padding * 2.0 - button_gap * 3.0).max(4.0);
            let small = available * 0.22;
            let done = available - small * 3.0;
            let step = small + button_gap;
            (
                Some(button(row_x, small)),
                Some(button(row_x + step, small)),
                button(row_x + step * 2.0, small),
                button(row_x + step * 3.0, done),
            )
        } else {
            let available = (card_width - padding * 2.0 - button_gap).max(2.0);
            let copy = available * 0.38;
            (
                None,
                None,
                button(row_x, copy),
                button(row_x + copy + button_gap, available - copy),
            )
        };

        Self {
            card: DiffReviewRect {
                x: card_x,
                y: card_y,
                width: card_width,
                height: card_height,
            },
            content: DiffReviewRect {
                x: content_x,
                y: content_y,
                width: content_width,
                height: content_height,
            },
            copy_button,
            done_button,
            stage_button,
            revert_button,
            padding,
            title_height,
            line_height,
            char_scale,
            footer_height,
            rows_that_fit,
            columns_that_fit,
            scrollbar_x: card_x + card_width - padding - 4.0,
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
    scroll_offset: usize,
    horizontal_offset: usize,
    viewport_rows: usize,
    viewport_columns: usize,
    max_line_chars: usize,
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
            scroll_offset: 0,
            horizontal_offset: 0,
            viewport_rows: 1,
            viewport_columns: 1,
            max_line_chars: 0,
        }
    }

    pub fn show(
        &mut self,
        diff: &DetectedDiff,
        tab_id: usize,
        pane_id: crate::panes::PaneId,
    ) -> u64 {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.filename = bounded_text(&diff.filename, MAX_DIFF_FILENAME_CHARS);
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
        self.filename = bounded_text(&change.path.to_string_lossy(), MAX_DIFF_FILENAME_CHARS);
        self.rows = rows_from_unified(unified);
        self.target = None;
        self.change = Some(change);
        self.change_text = unified.to_owned();
        self.open_rows()
    }

    fn open_rows(&mut self) -> u64 {
        self.max_line_chars = self
            .rows
            .iter()
            .map(|row| row.prefix().chars().count() + row.text().chars().count())
            .max()
            .unwrap_or(0);
        self.accepting = true;
        self.scroll_offset = 0;
        self.horizontal_offset = 0;
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
        self.max_line_chars
    }

    pub fn set_viewport(&mut self, rows: usize, columns: usize) {
        self.viewport_rows = rows.max(1);
        self.viewport_columns = columns.max(1);
        self.clamp_scroll();
    }

    pub fn scroll_rows(&mut self, request_id: u64, delta: isize) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        let before = self.scroll_offset;
        self.scroll_offset = self.scroll_offset.saturating_add_signed(delta);
        self.clamp_scroll();
        before != self.scroll_offset
    }

    pub fn scroll_columns(&mut self, request_id: u64, delta: isize) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        let before = self.horizontal_offset;
        self.horizontal_offset = self.horizontal_offset.saturating_add_signed(delta);
        self.clamp_scroll();
        before != self.horizontal_offset
    }

    pub fn scroll_to_top(&mut self, request_id: u64) -> bool {
        self.set_scroll(request_id, Some(0), None)
    }

    pub fn scroll_to_bottom(&mut self, request_id: u64) -> bool {
        self.set_scroll(request_id, Some(self.max_scroll_offset()), None)
    }

    pub fn scroll_to_left(&mut self, request_id: u64) -> bool {
        self.set_scroll(request_id, None, Some(0))
    }

    pub fn scroll_to_right(&mut self, request_id: u64) -> bool {
        self.set_scroll(request_id, None, Some(self.max_horizontal_offset()))
    }

    fn set_scroll(
        &mut self,
        request_id: u64,
        vertical: Option<usize>,
        horizontal: Option<usize>,
    ) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        let before = (self.scroll_offset, self.horizontal_offset);
        if let Some(vertical) = vertical {
            self.scroll_offset = vertical;
        }
        if let Some(horizontal) = horizontal {
            self.horizontal_offset = horizontal;
        }
        self.clamp_scroll();
        before != (self.scroll_offset, self.horizontal_offset)
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn horizontal_offset(&self) -> usize {
        self.horizontal_offset
    }

    pub fn viewport_rows(&self) -> usize {
        self.viewport_rows
    }

    pub fn viewport_columns(&self) -> usize {
        self.viewport_columns
    }

    pub fn max_scroll_offset(&self) -> usize {
        self.rows.len().saturating_sub(self.viewport_rows)
    }

    pub fn max_horizontal_offset(&self) -> usize {
        self.max_line_chars.saturating_sub(self.viewport_columns)
    }

    pub fn visible_rows(&self) -> impl Iterator<Item = (usize, &DiffReviewRow)> {
        self.rows
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
            .take(self.viewport_rows)
    }

    fn clamp_scroll(&mut self) {
        self.scroll_offset = self.scroll_offset.min(self.max_scroll_offset());
        self.horizontal_offset = self.horizontal_offset.min(self.max_horizontal_offset());
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
            self.scroll_offset = 0;
            self.horizontal_offset = 0;
            self.max_line_chars = 0;
        }
        animating
    }
}

fn bounded_rows(diff: &DetectedDiff) -> Vec<DiffReviewRow> {
    let mut rows = Vec::new();
    'hunks: for (hunk_index, hunk) in diff.hunks.iter().enumerate() {
        if !push_bounded_row(
            &mut rows,
            DiffReviewRow::Header(bounded_text(&hunk.header, MAX_DIFF_REVIEW_LINE_CHARS)),
        ) {
            break;
        }
        for line in &hunk.lines {
            let row = match line {
                DiffLine::Context(text) => DiffReviewRow::Context(bounded_text(
                    text,
                    MAX_DIFF_REVIEW_LINE_CHARS.saturating_sub(2),
                )),
                DiffLine::Added(text) => DiffReviewRow::Added(bounded_text(
                    text,
                    MAX_DIFF_REVIEW_LINE_CHARS.saturating_sub(2),
                )),
                DiffLine::Removed(text) => DiffReviewRow::Removed(bounded_text(
                    text,
                    MAX_DIFF_REVIEW_LINE_CHARS.saturating_sub(2),
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
    let body = |line: &str| bounded_text(&line[1..], MAX_DIFF_REVIEW_LINE_CHARS.saturating_sub(2));
    let mut rows = Vec::new();
    let mut in_hunk = false;
    for line in text.lines() {
        let row = if line.starts_with("@@") {
            if in_hunk && !push_bounded_row(&mut rows, DiffReviewRow::Blank) {
                break;
            }
            in_hunk = true;
            DiffReviewRow::Header(bounded_text(line, MAX_DIFF_REVIEW_LINE_CHARS))
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
            DiffReviewRow::Omitted(bounded_text(line, MAX_DIFF_REVIEW_LINE_CHARS))
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

fn bounded_text(text: &str, limit: usize) -> String {
    let mut output = String::new();
    let mut count = 0;
    let mut truncated = false;
    for character in text.chars() {
        if count >= limit {
            truncated = true;
            break;
        }
        match character {
            '\t' => {
                for _ in 0..4 {
                    if count >= limit {
                        truncated = true;
                        break;
                    }
                    output.push(' ');
                    count += 1;
                }
            }
            character if character.is_control() => {
                output.push('\u{fffd}');
                count += 1;
            }
            character => {
                output.push(character);
                count += 1;
            }
        }
    }
    if truncated && limit > 0 {
        output.pop();
        output.push('…');
    }
    output
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
        let centre = |rect: DiffReviewRect| (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
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
