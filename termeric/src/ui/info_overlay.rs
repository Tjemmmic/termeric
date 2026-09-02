//! Bounded modal notices used for application information, errors, and choices.

use super::animation::OverlayAnimation;
use super::constants::CONTENT_CHAR_SCALE;

pub const MAX_INFO_TITLE_CHARS: usize = 256;
pub const MAX_INFO_LINES: usize = 512;
pub const MAX_INFO_LINE_CHARS: usize = 4096;
pub const MAX_INFO_CHOICES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InfoOverlayRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl InfoOverlayRect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoOverlayChoice {
    pub id: String,
    pub label: String,
    pub source_line: usize,
    pub shortcut: Option<char>,
}

impl InfoOverlayChoice {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        source_line: usize,
        shortcut: Option<char>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            source_line,
            shortcut,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InfoOverlayAction {
    Copy { request_id: u64 },
    Dismiss { request_id: u64 },
    Choose { request_id: u64, choice_id: String },
    ScrollLines { request_id: u64, delta: isize },
    ScrollColumns { request_id: u64, delta: isize },
    ScrollToTop { request_id: u64 },
    ScrollToBottom { request_id: u64 },
    ScrollToLeft { request_id: u64 },
    ScrollToRight { request_id: u64 },
    None,
}

/// Geometry shared by rendering, pointer hit testing, viewport sizing, and
/// accessibility. Content is always bounded so controls remain on-screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InfoOverlayLayout {
    pub card: InfoOverlayRect,
    pub content: InfoOverlayRect,
    pub copy_button: InfoOverlayRect,
    pub continue_button: InfoOverlayRect,
    pub padding: f32,
    pub title_height: f32,
    pub line_height: f32,
    pub char_scale: f32,
    pub footer_height: f32,
    pub rows_that_fit: usize,
    pub columns_that_fit: usize,
    pub scrollbar_x: f32,
}

impl InfoOverlayLayout {
    pub fn calculate(
        window_width: f32,
        window_height: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
        title: &str,
        total_lines: usize,
        max_line_chars: usize,
    ) -> Self {
        let char_scale = (cell_width * CONTENT_CHAR_SCALE).max(1.0);
        let padding = (cell_width * 2.5).max(12.0);
        let line_height = (cell_height * 1.6).max(1.0);
        let title_height = cell_height * 2.8;
        let footer_height = cell_height * 4.5;
        let outer_margin = (cell_width * 2.0).max(12.0);

        let hint_chars = 41_usize; // "Esc / Enter dismiss · C copy · arrows scroll"
        let content_width = max_line_chars.max(hint_chars) as f32 * char_scale;
        let title_width = title.chars().count() as f32 * cell_width;
        let preferred_width = content_width.max(title_width) + padding * 2.0 + cell_width * 2.0;
        let available_width = (window_width - outer_margin * 2.0).max(1.0);
        let card_width = preferred_width.max(380.0).min(available_width);

        let fixed_height = padding + title_height + footer_height + padding * 0.5;
        let available_height = (window_height - outer_margin * 2.0).max(line_height);
        let content_budget = (available_height - fixed_height).max(line_height);
        let rows_that_fit = ((content_budget / line_height).floor() as usize).max(1);
        let visible_rows = total_lines.max(1).min(rows_that_fit);
        let content_height = visible_rows as f32 * line_height;
        let card_height = fixed_height + content_height;
        let card_x = (window_width - card_width) / 2.0;
        let card_y = (window_height - card_height) / 2.0 + (1.0 - animation) * -30.0;

        let content_x = card_x + padding;
        let content_y = card_y + padding + title_height;
        let scrollbar_gutter = cell_width.max(6.0);
        let content_rect_width = (card_width - padding * 2.0 - scrollbar_gutter).max(char_scale);
        let columns_that_fit = ((content_rect_width / char_scale).floor() as usize).max(1);

        let button_height = cell_height * 1.8;
        let button_gap = cell_width * 1.5;
        let button_y = content_y + content_height + cell_height * 1.3;
        let available_button_width = (card_width - padding * 2.0 - button_gap).max(2.0);
        let copy_width = (available_button_width * 0.38).max(1.0);
        let continue_width = (available_button_width - copy_width).max(1.0);
        let copy_x = card_x + padding;
        let continue_x = copy_x + copy_width + button_gap;

        Self {
            card: InfoOverlayRect {
                x: card_x,
                y: card_y,
                width: card_width,
                height: card_height,
            },
            content: InfoOverlayRect {
                x: content_x,
                y: content_y,
                width: content_rect_width,
                height: content_height,
            },
            copy_button: InfoOverlayRect {
                x: copy_x,
                y: button_y,
                width: copy_width,
                height: button_height,
            },
            continue_button: InfoOverlayRect {
                x: continue_x,
                y: button_y,
                width: continue_width,
                height: button_height,
            },
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

    pub fn content_row_at(
        &self,
        x: f32,
        y: f32,
        scroll_offset: usize,
        total_lines: usize,
    ) -> Option<usize> {
        if !self.content.contains(x, y) {
            return None;
        }
        let visible_row = ((y - self.content.y) / self.line_height).floor() as usize;
        let source_row = scroll_offset.saturating_add(visible_row);
        (source_row < total_lines).then_some(source_row)
    }
}

/// Request-scoped information overlay state.
pub struct InfoOverlay {
    animation: OverlayAnimation,
    title: String,
    lines: Vec<String>,
    choices: Vec<InfoOverlayChoice>,
    request_id: u64,
    accepting: bool,
    scroll_offset: usize,
    horizontal_offset: usize,
    viewport_rows: usize,
    viewport_columns: usize,
    max_line_chars: usize,
}

impl InfoOverlay {
    pub fn new() -> Self {
        Self {
            animation: OverlayAnimation::new(),
            title: String::new(),
            lines: Vec::new(),
            choices: Vec::new(),
            request_id: 0,
            accepting: false,
            scroll_offset: 0,
            horizontal_offset: 0,
            viewport_rows: 1,
            viewport_columns: 1,
            max_line_chars: 0,
        }
    }

    pub fn show_about(&mut self) -> u64 {
        self.show(
            "About Termeric",
            vec![
                format!("Version {}", env!("CARGO_PKG_VERSION")),
                String::new(),
                "A GPU-accelerated terminal emulator".into(),
                "Built with wgpu + winit".into(),
                String::new(),
                "github.com/termeric-terminal/termeric".into(),
            ],
        )
    }

    pub fn show_version(&mut self) -> u64 {
        self.show("Termeric", vec![format!("v{}", env!("CARGO_PKG_VERSION"))])
    }

    pub fn show(&mut self, title: impl Into<String>, lines: Vec<String>) -> u64 {
        self.show_choices(title, lines, Vec::new())
    }

    pub fn show_choices(
        &mut self,
        title: impl Into<String>,
        lines: Vec<String>,
        choices: Vec<InfoOverlayChoice>,
    ) -> u64 {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.title = bounded_text(&title.into(), MAX_INFO_TITLE_CHARS);
        if self.title.is_empty() {
            self.title = "Notice".into();
        }
        self.lines = bounded_lines(lines);
        self.max_line_chars = self
            .lines
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0);
        self.choices = bounded_choices(choices, self.lines.len());
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

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn choices(&self) -> &[InfoOverlayChoice] {
        &self.choices
    }

    pub fn copy_text(&self, request_id: u64) -> Option<String> {
        self.is_awaiting(request_id).then(|| self.lines.join("\n"))
    }

    pub fn choice(&self, request_id: u64, choice_id: &str) -> Option<&InfoOverlayChoice> {
        self.is_awaiting(request_id)
            .then(|| self.choices.iter().find(|choice| choice.id == choice_id))?
    }

    pub fn choice_at_line(&self, request_id: u64, line: usize) -> Option<&InfoOverlayChoice> {
        self.is_awaiting(request_id).then(|| {
            self.choices
                .iter()
                .find(|choice| choice.source_line == line)
        })?
    }

    pub fn choice_for_shortcut(
        &self,
        request_id: u64,
        shortcut: char,
    ) -> Option<&InfoOverlayChoice> {
        self.is_awaiting(request_id).then(|| {
            self.choices
                .iter()
                .find(|choice| choice.shortcut == Some(shortcut))
        })?
    }

    pub fn set_viewport(&mut self, rows: usize, columns: usize) {
        self.viewport_rows = rows.max(1);
        self.viewport_columns = columns.max(1);
        self.clamp_scroll();
    }

    pub fn scroll_lines(&mut self, request_id: u64, delta: isize) -> bool {
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
        self.lines.len().saturating_sub(self.viewport_rows)
    }

    pub fn max_horizontal_offset(&self) -> usize {
        self.max_line_chars.saturating_sub(self.viewport_columns)
    }

    pub fn max_line_chars(&self) -> usize {
        self.max_line_chars
    }

    pub fn visible_lines(&self) -> impl Iterator<Item = (usize, &str)> {
        self.lines
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
            .take(self.viewport_rows)
            .map(|(index, line)| (index, line.as_str()))
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
            self.title.clear();
            self.lines.clear();
            self.choices.clear();
            self.scroll_offset = 0;
            self.horizontal_offset = 0;
            self.max_line_chars = 0;
        }
        animating
    }
}

fn bounded_text(text: &str, limit: usize) -> String {
    let mut output = String::new();
    let mut truncated = false;
    for (count, character) in text.chars().enumerate() {
        if count >= limit {
            truncated = true;
            break;
        }
        match character {
            '\t' => output.push(' '),
            character if character.is_control() => output.push('\u{fffd}'),
            character => output.push(character),
        }
    }
    if truncated && limit > 0 {
        output.pop();
        output.push('…');
    }
    output
}

fn bounded_lines(lines: Vec<String>) -> Vec<String> {
    let mut output = Vec::new();
    let mut omitted = false;
    'sources: for source in lines {
        for line in source.split('\n') {
            if output.len() >= MAX_INFO_LINES.saturating_sub(1) {
                omitted = true;
                break 'sources;
            }
            output.push(bounded_text(
                line.trim_end_matches('\r'),
                MAX_INFO_LINE_CHARS,
            ));
        }
    }
    if omitted {
        output.push("[Additional notice lines omitted]".into());
    }
    if output.is_empty() {
        output.push(String::new());
    }
    output
}

fn bounded_choices(choices: Vec<InfoOverlayChoice>, line_count: usize) -> Vec<InfoOverlayChoice> {
    let mut output = Vec::new();
    for mut choice in choices {
        if output.len() >= MAX_INFO_CHOICES
            || choice.source_line >= line_count
            || output
                .iter()
                .any(|existing: &InfoOverlayChoice| existing.id == choice.id)
        {
            continue;
        }
        choice.id = bounded_text(&choice.id, 256);
        choice.label = bounded_text(&choice.label, 512);
        if choice.id.is_empty() || choice.label.is_empty() {
            continue;
        }
        output.push(choice);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_and_choices_are_bounded_and_controls_stay_in_the_card() {
        let layout =
            InfoOverlayLayout::calculate(900.0, 500.0, 10.0, 20.0, 1.0, "Notice", 1_000, 10_000);
        assert!(layout.rows_that_fit > 0);
        assert!(layout.card.height < 500.0);
        assert!(layout.card.contains(
            layout.continue_button.x + layout.continue_button.width / 2.0,
            layout.continue_button.y + layout.continue_button.height / 2.0,
        ));

        let mut overlay = InfoOverlay::new();
        overlay.show(
            "x".repeat(1_000),
            vec!["x".repeat(10_000); MAX_INFO_LINES + 50],
        );
        assert!(overlay.title().chars().count() <= MAX_INFO_TITLE_CHARS);
        assert_eq!(overlay.lines().len(), MAX_INFO_LINES);
        assert_eq!(overlay.max_line_chars(), MAX_INFO_LINE_CHARS);
        assert!(
            overlay
                .lines()
                .iter()
                .all(|line| line.chars().count() <= MAX_INFO_LINE_CHARS)
        );
    }

    #[test]
    fn dismiss_revokes_actions_while_the_exit_animation_still_blocks_input() {
        let mut overlay = InfoOverlay::new();
        let request = overlay.show("Fixture", vec!["body".into()]);
        assert!(overlay.dismiss(request));
        assert!(overlay.is_active());
        assert!(!overlay.is_awaiting(request));
        assert_eq!(overlay.copy_text(request), None);
        assert!(!overlay.scroll_lines(request, 1));
    }

    #[test]
    fn replacement_notice_rejects_stale_actions_and_choices() {
        let mut overlay = InfoOverlay::new();
        let stale = overlay.show_choices(
            "Choose",
            vec!["one".into()],
            vec![InfoOverlayChoice::new("one", "One", 0, Some('1'))],
        );
        let current = overlay.show("Replacement", vec!["current".into()]);
        assert_ne!(stale, current);
        assert!(overlay.choice(stale, "one").is_none());
        assert!(!overlay.dismiss(stale));
        assert!(overlay.is_awaiting(current));
    }

    #[test]
    fn vertical_and_horizontal_scrolling_are_clamped_to_the_viewport() {
        let mut overlay = InfoOverlay::new();
        let request = overlay.show(
            "Scrollable",
            (0..10)
                .map(|index| format!("line-{index}-{}", "x".repeat(40)))
                .collect(),
        );
        overlay.set_viewport(3, 8);
        assert!(overlay.scroll_lines(request, 100));
        assert_eq!(overlay.scroll_offset(), 7);
        assert!(overlay.scroll_columns(request, 100));
        assert_eq!(overlay.horizontal_offset(), overlay.max_horizontal_offset());
        assert!(overlay.scroll_to_top(request));
        assert!(overlay.scroll_to_left(request));
        assert_eq!(
            (overlay.scroll_offset(), overlay.horizontal_offset()),
            (0, 0)
        );
    }

    #[test]
    fn shared_content_geometry_resolves_only_visible_source_rows() {
        let lines: Vec<_> = (0..20).map(|index| format!("line {index}")).collect();
        let layout =
            InfoOverlayLayout::calculate(900.0, 500.0, 10.0, 20.0, 1.0, "Notice", lines.len(), 7);
        assert_eq!(
            layout.content_row_at(
                layout.content.x + 1.0,
                layout.content.y + layout.line_height / 2.0,
                5,
                lines.len(),
            ),
            Some(5)
        );
        assert_eq!(
            layout.content_row_at(layout.content.x - 1.0, layout.content.y, 5, lines.len()),
            None
        );
    }
}
