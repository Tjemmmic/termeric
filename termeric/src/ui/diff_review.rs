//! Bounded, request-scoped review surface for unified diffs: one detected in
//! a terminal's output, or one file an agent changed, which can also be
//! staged or reverted from here.

use super::animation::OverlayAnimation;
use super::constants::CONTENT_CHAR_SCALE;
use super::request_scope::RequestScope;
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

/// The review's buttons. Every action is one of them; there are no letter
/// keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffReviewButton {
    /// Stage the reviewed agent change.
    Stage,
    /// Ask to put the reviewed file back as it was when the agent started.
    Revert,
    Copy,
    /// Mark the diff reviewed and close; Escape answers it too.
    Done,
}

impl DiffReviewButton {
    /// Left to right as drawn, which is also the focus order.
    const CHANGE_ROW: [Self; 4] = [Self::Stage, Self::Revert, Self::Copy, Self::Done];
    const DIFF_ROW: [Self; 2] = [Self::Copy, Self::Done];

    /// The buttons a review offers: Stage and Revert only for an agent's change.
    pub fn row(change_actions: bool) -> &'static [Self] {
        if change_actions {
            &Self::CHANGE_ROW
        } else {
            &Self::DIFF_ROW
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Stage => "Stage",
            Self::Revert => "Revert…",
            Self::Copy => "Copy",
            Self::Done => "Done",
        }
    }

    /// What pressing it does for request `request_id`.
    pub fn action(self, request_id: u64) -> DiffReviewAction {
        match self {
            Self::Stage => DiffReviewAction::Stage { request_id },
            Self::Revert => DiffReviewAction::Revert { request_id },
            Self::Copy => DiffReviewAction::Copy { request_id },
            Self::Done => DiffReviewAction::Dismiss { request_id },
        }
    }
}

/// Where the review's keyboard focus is. Tab moves it between the buttons;
/// the content takes it only from assistive technology, and the arrows
/// scroll the content wherever focus is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffReviewFocus {
    Content,
    Button(DiffReviewButton),
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
    Scroll {
        request_id: u64,
        by: DiffReviewScroll,
    },
    None,
}

/// One scroll of the diff: by rows or columns, or to an edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffReviewScroll {
    Rows(isize),
    Columns(isize),
    Top,
    Bottom,
    LineStart,
    LineEnd,
}

/// The part of the diff in view: `rows` × `columns` cells scrolled over
/// `content_rows` lines at most `content_columns` wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffViewport {
    row: usize,
    column: usize,
    rows: usize,
    columns: usize,
    content_rows: usize,
    content_columns: usize,
}

impl Default for DiffViewport {
    fn default() -> Self {
        Self {
            row: 0,
            column: 0,
            rows: 1,
            columns: 1,
            content_rows: 0,
            content_columns: 0,
        }
    }
}

impl DiffViewport {
    /// Show a new diff from its top-left corner.
    pub fn reset(&mut self, content_rows: usize, content_columns: usize) {
        self.row = 0;
        self.column = 0;
        self.content_rows = content_rows;
        self.content_columns = content_columns;
        self.clamp();
    }

    /// The card now shows `rows` × `columns` cells.
    pub fn resize(&mut self, rows: usize, columns: usize) {
        self.rows = rows.max(1);
        self.columns = columns.max(1);
        self.clamp();
    }

    /// Apply one scroll; whether anything moved.
    pub fn apply(&mut self, scroll: DiffReviewScroll) -> bool {
        let before = (self.row, self.column);
        match scroll {
            DiffReviewScroll::Rows(delta) => self.row = self.row.saturating_add_signed(delta),
            DiffReviewScroll::Columns(delta) => {
                self.column = self.column.saturating_add_signed(delta);
            }
            DiffReviewScroll::Top => self.row = 0,
            DiffReviewScroll::Bottom => self.row = self.max_row(),
            DiffReviewScroll::LineStart => self.column = 0,
            DiffReviewScroll::LineEnd => self.column = self.max_column(),
        }
        self.clamp();
        before != (self.row, self.column)
    }

    /// The first row in view.
    pub fn row(&self) -> usize {
        self.row
    }

    /// The first column in view.
    pub fn column(&self) -> usize {
        self.column
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn columns(&self) -> usize {
        self.columns
    }

    pub fn max_row(&self) -> usize {
        self.content_rows.saturating_sub(self.rows)
    }

    pub fn max_column(&self) -> usize {
        self.content_columns.saturating_sub(self.columns)
    }

    /// The content columns the widest line spans.
    pub fn content_columns(&self) -> usize {
        self.content_columns
    }

    fn clamp(&mut self) {
        self.row = self.row.min(self.max_row());
        self.column = self.column.min(self.max_column());
    }
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
    /// Horizontal advance of one diff column.
    pub char_scale: f32,
    pub rows_that_fit: usize,
    pub columns_that_fit: usize,
}

impl DiffReviewLayout {
    /// Lay out a centred card showing `total_rows` diff rows under a title
    /// naming `filename` and above the button row. It is wide enough for
    /// the title and for 50 to 100 columns of diff where the window allows,
    /// and never narrower than 500 pixels; `animation` slides it down into
    /// place as it opens.
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
        // Every length is whole, so the card and everything placed from it
        // land on whole pixels; rows tile through `Bounds::row`.
        let line_height = (cell_height * 1.5).max(1.0);
        let char_scale = (cell_width * CONTENT_CHAR_SCALE).max(1.0);
        let padding = (cell_width * 2.0).max(12.0).round();
        let title_height = (cell_height * 2.5).round();
        // The button row and the space below it.
        let footer_height = (cell_height * 4.5).round();
        let bottom_margin = (padding * 0.5).round();
        let outer_margin = (cell_width * 2.0).max(12.0).round();
        let scrollbar_gutter = cell_width.max(6.0).round();

        let columns = max_line_chars
            .clamp(50, 100)
            .max("Changes in ".chars().count() + filename.chars().count());
        let preferred_width =
            (columns as f32 * char_scale + padding * 2.0 + scrollbar_gutter).round();
        let available_width = (window_width - outer_margin * 2.0).floor().max(1.0);
        let card_width = preferred_width.max(500.0).min(available_width);

        let fixed_height = padding + title_height + footer_height + bottom_margin;
        let available_height = (window_height - outer_margin * 2.0).max(line_height);
        let content_budget = (available_height - fixed_height).max(line_height);
        let rows_that_fit = ((content_budget / line_height).floor() as usize).max(1);
        let content_height = (total_rows.max(1).min(rows_that_fit) as f32 * line_height).round();
        let card_height = fixed_height + content_height;
        let slide = ((1.0 - animation) * -20.0).round();
        let card = Bounds::new(
            ((window_width - card_width) / 2.0).round(),
            ((window_height - card_height) / 2.0).round() + slide,
            card_width,
            card_height,
        );

        let content_width = (card_width - padding * 2.0 - scrollbar_gutter).max(char_scale.ceil());
        let content = Bounds::new(
            card.x + padding,
            card.y + padding + title_height,
            content_width,
            content_height,
        );

        let button_y = content.bottom() + (cell_height * 1.3).round();
        let button_height = (cell_height * 1.8).round();
        let button_gap = (cell_width * 1.5).round();
        let button = |x: f32, width: f32| Bounds::new(x, button_y, width.max(1.0), button_height);
        // The row inside the padding, less the gaps between `buttons` buttons.
        let row_width = |buttons: usize| {
            card_width - padding * 2.0 - button_gap * buttons.saturating_sub(1) as f32
        };
        let row_x = card.x + padding;
        let (stage_button, revert_button, copy_button, done_button) = if change_actions {
            // Stage, Revert, and Copy share the row equally with a wider Done.
            let available = row_width(4).max(4.0);
            let small = (available * 0.22).round();
            let done = available - small * 3.0;
            let step = small + button_gap;
            (
                Some(button(row_x, small)),
                Some(button(row_x + step, small)),
                button(row_x + step * 2.0, small),
                button(row_x + step * 3.0, done),
            )
        } else {
            let available = row_width(2).max(2.0);
            let copy = (available * 0.38).round();
            (
                None,
                None,
                button(row_x, copy),
                button(row_x + copy + button_gap, available - copy),
            )
        };

        Self {
            card,
            content,
            copy_button,
            done_button,
            stage_button,
            revert_button,
            padding,
            title_height,
            line_height,
            char_scale,
            rows_that_fit,
            columns_that_fit: ((content_width / char_scale).floor() as usize).max(1),
        }
    }

    /// The `visible_index`th diff row in view, where it is drawn: on whole
    /// pixels, each row's bottom the next row's top.
    pub fn row(&self, visible_index: usize) -> Bounds {
        self.content.row(self.line_height, visible_index)
    }

    /// Where `button` is drawn, if this review offers it.
    pub fn button(&self, button: DiffReviewButton) -> Option<Bounds> {
        match button {
            DiffReviewButton::Stage => self.stage_button,
            DiffReviewButton::Revert => self.revert_button,
            DiffReviewButton::Copy => Some(self.copy_button),
            DiffReviewButton::Done => Some(self.done_button),
        }
    }

    /// The button under a point, if any.
    pub fn button_at(&self, x: f32, y: f32) -> Option<DiffReviewButton> {
        DiffReviewButton::row(self.stage_button.is_some())
            .iter()
            .copied()
            .find(|&button| {
                self.button(button)
                    .is_some_and(|bounds| bounds.contains(x, y))
            })
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
    request: RequestScope,
    viewport: DiffViewport,
    focus: DiffReviewFocus,
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
            request: RequestScope::default(),
            viewport: DiffViewport::default(),
            focus: DiffReviewFocus::Button(DiffReviewButton::Done),
        }
    }

    pub fn show(
        &mut self,
        diff: &DetectedDiff,
        tab_id: usize,
        pane_id: crate::panes::PaneId,
    ) -> u64 {
        self.request.begin();
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
        self.request.begin();
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
        // Nothing here is destructive (Revert asks first), so focus starts
        // on the suggested Done.
        self.focus = DiffReviewFocus::Button(DiffReviewButton::Done);
        self.animation.open();
        self.request.id()
    }

    pub fn dismiss(&mut self, request_id: u64) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        self.request.revoke();
        self.animation.close();
        true
    }

    pub fn request_id(&self) -> u64 {
        self.request.id()
    }

    pub fn is_awaiting(&self, request_id: u64) -> bool {
        self.request.is_awaiting(request_id)
    }

    pub fn is_awaiting_current(&self) -> bool {
        self.request.is_awaiting_current()
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

    /// The buttons this review offers, left to right.
    pub fn buttons(&self) -> &'static [DiffReviewButton] {
        DiffReviewButton::row(self.has_change_actions())
    }

    /// The focused button, if focus is on one.
    pub fn focused_button(&self) -> Option<DiffReviewButton> {
        match self.focus {
            DiffReviewFocus::Button(button) => Some(button),
            DiffReviewFocus::Content => None,
        }
    }

    /// Focus is request-scoped and never activates anything by itself.
    pub fn set_focus(&mut self, request_id: u64, focus: DiffReviewFocus) -> bool {
        let offered = match focus {
            DiffReviewFocus::Content => true,
            DiffReviewFocus::Button(button) => self.buttons().contains(&button),
        };
        if !self.is_awaiting(request_id) || !offered {
            return false;
        }
        self.focus = focus;
        true
    }

    /// Move focus to the next button, or the previous one, wrapping around.
    /// From the content, it goes to the first or last button.
    pub fn cycle_focus(&mut self, request_id: u64, backwards: bool) -> bool {
        let buttons = self.buttons();
        let position = self
            .focused_button()
            .and_then(|button| buttons.iter().position(|&offered| offered == button));
        let next = match (position, backwards) {
            (Some(index), false) => (index + 1) % buttons.len(),
            (Some(index), true) => (index + buttons.len() - 1) % buttons.len(),
            (None, false) => 0,
            (None, true) => buttons.len() - 1,
        };
        self.set_focus(request_id, DiffReviewFocus::Button(buttons[next]))
    }

    /// What Enter or Space does: press the focused button.
    pub fn activate_focused(&self, request_id: u64) -> DiffReviewAction {
        match self.focused_button() {
            Some(button) if self.is_awaiting(request_id) => button.action(request_id),
            _ => DiffReviewAction::None,
        }
    }

    pub fn filename(&self) -> &str {
        &self.filename
    }

    pub fn rows(&self) -> &[DiffReviewRow] {
        &self.rows
    }

    pub fn set_viewport(&mut self, rows: usize, columns: usize) {
        self.viewport.resize(rows, columns);
    }

    /// Scroll for the current request; whether anything moved.
    pub fn scroll(&mut self, request_id: u64, by: DiffReviewScroll) -> bool {
        self.is_awaiting(request_id) && self.viewport.apply(by)
    }

    /// The part of the diff in view.
    pub fn viewport(&self) -> &DiffViewport {
        &self.viewport
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
            let translation = ((1.0 - phase) * -20.0_f32).round();
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
                assert_eq!(actual.y, expected.y + translation);
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
    fn each_scroll_moves_its_axis_within_the_diff_to_the_step_or_edge() {
        let mut viewport = DiffViewport::default();
        viewport.reset(100, 40);
        viewport.resize(10, 30);
        let at = |viewport: &DiffViewport| (viewport.row(), viewport.column());
        assert!(viewport.apply(DiffReviewScroll::Rows(3)));
        assert!(viewport.apply(DiffReviewScroll::Columns(2)));
        assert_eq!(at(&viewport), (3, 2));
        assert!(viewport.apply(DiffReviewScroll::Rows(1_000)));
        assert!(viewport.apply(DiffReviewScroll::Columns(1_000)));
        assert_eq!(at(&viewport), (90, 10));
        assert!(
            !viewport.apply(DiffReviewScroll::Bottom),
            "already at the bottom"
        );
        assert!(!viewport.apply(DiffReviewScroll::LineEnd));
        assert!(viewport.apply(DiffReviewScroll::Top));
        assert!(viewport.apply(DiffReviewScroll::LineStart));
        assert_eq!(at(&viewport), (0, 0));
        assert!(viewport.apply(DiffReviewScroll::Bottom));
        assert!(viewport.apply(DiffReviewScroll::LineEnd));
        assert_eq!(at(&viewport), (90, 10));
        viewport.resize(95, 38);
        assert_eq!(at(&viewport), (5, 2), "a taller card pulls the scroll back");
        viewport.reset(3, 3);
        assert_eq!((viewport.max_row(), viewport.max_column()), (0, 0));
    }

    #[test]
    fn the_card_stays_in_small_windows_and_slides_with_its_animation() {
        let open = DiffReviewLayout::calculate(
            300.0,
            200.0,
            10.0,
            20.0,
            1.0,
            "fixture.rs",
            1_000,
            400,
            true,
        );
        assert!(open.card.x >= 0.0 && open.card.right() <= 300.0);
        assert!(open.card.y >= 0.0 && open.card.bottom() <= 200.0);
        assert!(open.content.right() < open.card.right() - open.padding);
        assert!(open.done_button.right() <= open.card.right() - open.padding + 0.001);
        assert_eq!(open.rows_that_fit, 1);
        let opening = DiffReviewLayout::calculate(
            300.0,
            200.0,
            10.0,
            20.0,
            0.5,
            "fixture.rs",
            1_000,
            400,
            true,
        );
        assert_eq!(opening.card.y, open.card.y - 10.0);
        assert_eq!(opening.done_button.y, open.done_button.y - 10.0);
    }

    #[test]
    fn the_card_and_everything_placed_from_it_land_on_whole_pixels() {
        // Fractional cells, an odd window, and a mid-slide phase would each
        // put a centred card between pixels.
        for (cell_width, cell_height) in [(10.0, 20.0), (7.3, 15.6), (9.6, 19.5)] {
            for phase in [1.0, 0.875, 0.37, 0.0] {
                for change_actions in [false, true] {
                    let layout = DiffReviewLayout::calculate(
                        901.0,
                        501.0,
                        cell_width,
                        cell_height,
                        phase,
                        "fixture.rs",
                        40,
                        80,
                        change_actions,
                    );
                    let mut placed = vec![layout.card, layout.content];
                    placed.extend(
                        DiffReviewButton::row(change_actions)
                            .iter()
                            .filter_map(|button| layout.button(*button)),
                    );
                    placed.extend((0..=layout.rows_that_fit).map(|index| layout.row(index)));
                    for bounds in placed {
                        assert_eq!(
                            [bounds.x, bounds.y, bounds.width, bounds.height].map(f32::fract),
                            [0.0; 4],
                            "{cell_width}x{cell_height} at {phase}: {bounds:?}"
                        );
                    }
                    assert_eq!(
                        [layout.padding, layout.title_height].map(f32::fract),
                        [0.0; 2]
                    );
                }
            }
        }
    }

    #[test]
    fn rows_tile_the_content() {
        // 19.5-pixel cells give lines 29.25 pixels tall.
        let layout =
            DiffReviewLayout::calculate(901.0, 501.0, 9.6, 19.5, 0.37, "fixture.rs", 40, 80, false);
        assert_ne!(layout.line_height.fract(), 0.0);
        assert_eq!(layout.row(0).y, layout.content.y);
        for index in 0..layout.rows_that_fit {
            let row = layout.row(index);
            assert_eq!((row.x, row.width), (layout.content.x, layout.content.width));
            assert_eq!(layout.row(index + 1).y, row.bottom());
            assert!((row.y - (layout.content.y + index as f32 * layout.line_height)).abs() <= 0.5);
        }
        assert_eq!(
            layout.row(layout.rows_that_fit - 1).bottom(),
            layout.content.bottom()
        );
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
            layout.button_at(layout.done_button.x + 1.0, layout.done_button.y + 1.0),
            Some(DiffReviewButton::Done)
        );
        assert_eq!(layout.button_at(layout.content.x, layout.content.y), None);
    }

    #[test]
    fn focus_starts_on_done_cycles_the_offered_buttons_and_is_request_scoped() {
        use DiffReviewButton as B;
        let mut overlay = DiffReviewOverlay::new();
        let request = overlay.show(&fixture(1, 3, 10), 9, crate::panes::PaneId(0));
        assert_eq!(overlay.focused_button(), Some(B::Done));
        assert_eq!(
            overlay.activate_focused(request),
            DiffReviewAction::Dismiss {
                request_id: request
            }
        );
        assert!(overlay.cycle_focus(request, false));
        assert_eq!(
            overlay.focused_button(),
            Some(B::Copy),
            "wraps to the first"
        );
        assert!(overlay.cycle_focus(request, true));
        assert!(overlay.cycle_focus(request, true));
        assert_eq!(overlay.focused_button(), Some(B::Copy));
        assert_eq!(
            overlay.activate_focused(request),
            DiffReviewAction::Copy {
                request_id: request
            }
        );
        assert!(
            !overlay.set_focus(request, DiffReviewFocus::Button(B::Stage)),
            "a detected diff offers no Stage"
        );
        assert!(overlay.set_focus(request, DiffReviewFocus::Content));
        assert_eq!(overlay.activate_focused(request), DiffReviewAction::None);
        assert!(overlay.cycle_focus(request, true));
        assert_eq!(
            overlay.focused_button(),
            Some(B::Done),
            "from the content, last"
        );

        let target = ChangeReviewTarget {
            owner: ChangeOwner::Terminal(crate::tabs::TerminalIdentity {
                tab_id: 1,
                pane_id: crate::panes::PaneId(0),
            }),
            path: "src/lib.rs".into(),
        };
        let change = overlay.show_change("@@ -1 +1 @@\n-a\n+b\n", target);
        assert_eq!(overlay.buttons(), [B::Stage, B::Revert, B::Copy, B::Done]);
        assert_eq!(
            overlay.focused_button(),
            Some(B::Done),
            "each review starts on Done"
        );
        let mut order = Vec::new();
        for _ in 0..4 {
            assert!(overlay.cycle_focus(change, false));
            order.push(overlay.focused_button().unwrap());
        }
        assert_eq!(order, [B::Stage, B::Revert, B::Copy, B::Done]);
        assert!(!overlay.cycle_focus(request, false), "an earlier review");
        assert!(overlay.dismiss(change));
        assert!(!overlay.cycle_focus(change, false));
        assert_eq!(overlay.activate_focused(change), DiffReviewAction::None);
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
        assert_eq!(
            overlay.viewport().content_columns(),
            MAX_DIFF_REVIEW_LINE_CHARS
        );
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
        assert!(overlay.scroll(first, DiffReviewScroll::Rows(100)));
        assert_eq!(overlay.viewport().row(), overlay.viewport().max_row());
        assert!(overlay.scroll(first, DiffReviewScroll::Columns(100)));
        assert_eq!(overlay.viewport().column(), overlay.viewport().max_column());

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
        for button in DiffReviewButton::row(true) {
            let (x, y) = centre(layout.button(*button).unwrap());
            assert_eq!(layout.button_at(x, y), Some(*button));
        }
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
