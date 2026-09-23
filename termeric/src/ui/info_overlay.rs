//! Bounded modal notices used for application information, errors, and choices.

use super::animation::OverlayAnimation;
use super::constants::CONTENT_CHAR_SCALE;
use super::text_input::TextInput;

pub const MAX_INFO_TITLE_CHARS: usize = 256;
pub const MAX_INFO_LINES: usize = 512;
pub const MAX_INFO_LINE_CHARS: usize = 4096;
pub const MAX_INFO_CHOICES: usize = 32;
pub const MAX_INFO_INPUT_BYTES: usize = 4096;

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
pub enum InfoOverlayFocus {
    Input,
    Choice(String),
    Copy,
    Continue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InfoOverlayAction {
    Copy { request_id: u64 },
    Dismiss { request_id: u64 },
    Choose { request_id: u64, choice_id: String },
    SubmitInput { request_id: u64 },
    CancelInput { request_id: u64 },
    ScrollLines { request_id: u64, delta: isize },
    ScrollColumns { request_id: u64, delta: isize },
    ScrollToTop { request_id: u64 },
    ScrollToBottom { request_id: u64 },
    ScrollToLeft { request_id: u64 },
    ScrollToRight { request_id: u64 },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoOverlayInputInfo {
    pub label: String,
    pub value: String,
    pub cursor_byte: usize,
    pub source_line: usize,
    pub max_bytes: usize,
}

struct InfoOverlayInput {
    label: String,
    value: TextInput,
    source_line: usize,
    max_bytes: usize,
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

    pub fn source_line_rect(
        &self,
        source_line: usize,
        scroll_offset: usize,
        total_lines: usize,
    ) -> Option<InfoOverlayRect> {
        if source_line >= total_lines
            || source_line < scroll_offset
            || source_line >= scroll_offset.saturating_add(self.rows_that_fit)
        {
            return None;
        }
        Some(InfoOverlayRect {
            x: self.content.x,
            y: self.content.y + source_line.saturating_sub(scroll_offset) as f32 * self.line_height,
            width: self.content.width,
            height: self.line_height,
        })
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
    input: Option<InfoOverlayInput>,
    focus: Option<InfoOverlayFocus>,
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
            input: None,
            focus: None,
        }
    }

    pub fn show_about(&mut self) -> u64 {
        self.show(
            "About Termeric",
            vec![
                format!("Version {}", env!("CARGO_PKG_VERSION")),
                String::new(),
                "A GPU-accelerated terminal emulator".into(),
                String::new(),
                env!("CARGO_PKG_REPOSITORY")
                    .trim_start_matches("https://")
                    .into(),
            ],
        )
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
        self.input = None;
        // Continue is the safe default, never the first (possibly consenting)
        // choice. A new request cannot inherit a previous choice's focus.
        self.focus = Some(InfoOverlayFocus::Continue);
        self.accepting = true;
        self.scroll_offset = 0;
        self.horizontal_offset = 0;
        self.animation.open();
        self.request_id
    }

    pub fn show_input(
        &mut self,
        title: impl Into<String>,
        lines: Vec<String>,
        source_line: usize,
        label: impl Into<String>,
        value: impl Into<String>,
        max_bytes: usize,
        choices: Vec<InfoOverlayChoice>,
    ) -> u64 {
        let request_id = self.show_choices(title, lines, choices);
        if source_line >= self.lines.len() {
            return request_id;
        }
        let label = bounded_text(&label.into(), 128);
        let max_bytes = max_bytes
            .clamp(1, MAX_INFO_INPUT_BYTES)
            .min(MAX_INFO_LINE_CHARS.saturating_sub(label.len()).max(1));
        let value = sanitize_input(&value.into(), max_bytes);
        let mut input = TextInput::new();
        input.replace(value);
        self.input = Some(InfoOverlayInput {
            label,
            value: input,
            source_line,
            max_bytes,
        });
        self.refresh_input_line();
        self.focus = Some(InfoOverlayFocus::Input);
        request_id
    }

    pub fn dismiss(&mut self, request_id: u64) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        self.accepting = false;
        self.focus = None;
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

    pub fn focus(&self) -> Option<&InfoOverlayFocus> {
        self.focus.as_ref().filter(|_| self.is_awaiting_current())
    }

    pub fn input_focused(&self) -> bool {
        self.focus() == Some(&InfoOverlayFocus::Input)
    }

    /// Focus is request-scoped and never activates a choice by itself.
    pub fn set_focus(&mut self, request_id: u64, focus: Option<InfoOverlayFocus>) -> bool {
        if !self.is_awaiting(request_id)
            || match &focus {
                Some(InfoOverlayFocus::Input) => self.input.is_none(),
                Some(InfoOverlayFocus::Choice(id)) => self.choice(request_id, id).is_none(),
                _ => false,
            }
        {
            return false;
        }
        self.focus = focus;
        if matches!(self.focus, Some(InfoOverlayFocus::Choice(_))) {
            self.horizontal_offset = 0;
        }
        self.ensure_focus_visible();
        true
    }

    pub fn cycle_focus(&mut self, request_id: u64, backwards: bool) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        let mut targets = Vec::with_capacity(self.choices.len() + 3);
        if self.input.is_some() {
            targets.push(InfoOverlayFocus::Input);
        }
        targets.extend(
            self.choices
                .iter()
                .map(|choice| InfoOverlayFocus::Choice(choice.id.clone())),
        );
        targets.extend([InfoOverlayFocus::Copy, InfoOverlayFocus::Continue]);
        let current = targets
            .iter()
            .position(|target| Some(target) == self.focus());
        let next = match (current, backwards) {
            (Some(index), true) => (index + targets.len() - 1) % targets.len(),
            (Some(index), false) => (index + 1) % targets.len(),
            (None, true) => targets.len() - 1,
            (None, false) => 0,
        };
        self.set_focus(request_id, Some(targets.swap_remove(next)))
    }

    pub fn activate_focused(&self, request_id: u64) -> InfoOverlayAction {
        if !self.is_awaiting(request_id) {
            return InfoOverlayAction::None;
        }
        match self.focus() {
            Some(InfoOverlayFocus::Choice(id)) => InfoOverlayAction::Choose {
                request_id,
                choice_id: id.clone(),
            },
            Some(InfoOverlayFocus::Copy) => InfoOverlayAction::Copy { request_id },
            _ if self.input.is_some() => InfoOverlayAction::SubmitInput { request_id },
            _ => InfoOverlayAction::Dismiss { request_id },
        }
    }

    fn ensure_focus_visible(&mut self) {
        match self.focus() {
            Some(InfoOverlayFocus::Input) => self.ensure_input_visible(),
            Some(InfoOverlayFocus::Choice(id)) => {
                let line = self.choice(self.request_id, id).unwrap().source_line;
                if line < self.scroll_offset {
                    self.scroll_offset = line;
                } else if line >= self.scroll_offset.saturating_add(self.viewport_rows) {
                    self.scroll_offset = line.saturating_add(1).saturating_sub(self.viewport_rows);
                }
                self.clamp_scroll();
            }
            _ => {}
        }
    }

    pub fn input_info(&self, request_id: u64) -> Option<InfoOverlayInputInfo> {
        let input = (self.request_id == request_id).then_some(self.input.as_ref())??;
        Some(InfoOverlayInputInfo {
            label: input.label.clone(),
            value: input.value.text().to_string(),
            cursor_byte: input.value.cursor(),
            source_line: input.source_line,
            max_bytes: input.max_bytes,
        })
    }

    pub fn has_input(&self) -> bool {
        self.is_awaiting_current() && self.input.is_some()
    }

    pub fn input_value(&self, request_id: u64) -> Option<&str> {
        let input = self
            .is_awaiting(request_id)
            .then_some(self.input.as_ref())??;
        Some(input.value.text())
    }

    pub fn insert_input_text(&mut self, request_id: u64, text: &str) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        let Some(input) = self.input.as_mut() else {
            return false;
        };
        let mut changed = false;
        for character in text.chars().filter(|character| !character.is_control()) {
            if input
                .value
                .text()
                .len()
                .saturating_add(character.len_utf8())
                > input.max_bytes
            {
                break;
            }
            input.value.insert_char(character);
            changed = true;
        }
        if changed {
            self.refresh_input_line();
        }
        changed
    }

    pub fn replace_input_text(&mut self, request_id: u64, text: &str) -> bool {
        if !self.is_awaiting(request_id)
            || text.len() > self.input.as_ref().map_or(0, |input| input.max_bytes)
            || text.chars().any(char::is_control)
        {
            return false;
        }
        let Some(input) = self.input.as_mut() else {
            return false;
        };
        if input.value.text() == text {
            return false;
        }
        input.value.replace(text);
        self.refresh_input_line();
        true
    }

    pub fn backspace_input(&mut self, request_id: u64) -> bool {
        self.mutate_input(request_id, TextInput::backspace)
    }

    pub fn delete_input(&mut self, request_id: u64) -> bool {
        self.mutate_input(request_id, TextInput::delete)
    }

    pub fn move_input_left(&mut self, request_id: u64) -> bool {
        self.mutate_input(request_id, TextInput::cursor_left)
    }

    pub fn move_input_right(&mut self, request_id: u64) -> bool {
        self.mutate_input(request_id, TextInput::cursor_right)
    }

    pub fn move_input_home(&mut self, request_id: u64) -> bool {
        self.set_input_cursor(request_id, 0)
    }

    pub fn move_input_end(&mut self, request_id: u64) -> bool {
        let end = self
            .input
            .as_ref()
            .map_or(0, |input| input.value.text().len());
        self.set_input_cursor(request_id, end)
    }

    pub fn set_input_cursor_from_column(&mut self, request_id: u64, column: usize) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        let Some(input) = self.input.as_ref() else {
            return false;
        };
        let value_column = column.saturating_sub(input.label.chars().count());
        let byte = input
            .value
            .text()
            .char_indices()
            .nth(value_column)
            .map_or(input.value.text().len(), |(byte, _)| byte);
        self.set_input_cursor(request_id, byte)
    }

    pub fn input_cursor_source_column(&self, request_id: u64) -> Option<usize> {
        let input = self
            .is_awaiting(request_id)
            .then_some(self.input.as_ref())??;
        Some(
            input.label.chars().count()
                + input.value.text()[..input.value.cursor()].chars().count(),
        )
    }

    fn set_input_cursor(&mut self, request_id: u64, byte: usize) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        let Some(input) = self.input.as_mut() else {
            return false;
        };
        let before = input.value.cursor();
        if !input.value.set_cursor_byte(byte) || before == byte {
            return false;
        }
        self.ensure_input_cursor_visible();
        true
    }

    fn mutate_input(
        &mut self,
        request_id: u64,
        mutation: impl FnOnce(&mut TextInput) -> bool,
    ) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        let Some(input) = self.input.as_mut() else {
            return false;
        };
        let changed = mutation(&mut input.value);
        if changed {
            self.refresh_input_line();
        }
        changed
    }

    fn refresh_input_line(&mut self) {
        let Some(input) = self.input.as_ref() else {
            return;
        };
        self.lines[input.source_line] = format!("{}{}", input.label, input.value.text());
        self.max_line_chars = self
            .lines
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0);
        self.ensure_input_visible();
    }

    fn ensure_input_cursor_visible(&mut self) {
        let Some(column) = self.input_cursor_source_column(self.request_id) else {
            return;
        };
        if column < self.horizontal_offset {
            self.horizontal_offset = column;
        } else if column >= self.horizontal_offset.saturating_add(self.viewport_columns) {
            self.horizontal_offset = column
                .saturating_add(1)
                .saturating_sub(self.viewport_columns);
        }
        self.clamp_scroll();
    }

    fn ensure_input_visible(&mut self) {
        let Some(source_line) = self.input.as_ref().map(|input| input.source_line) else {
            return;
        };
        if source_line < self.scroll_offset {
            self.scroll_offset = source_line;
        } else if source_line >= self.scroll_offset.saturating_add(self.viewport_rows) {
            self.scroll_offset = source_line
                .saturating_add(1)
                .saturating_sub(self.viewport_rows);
        }
        self.ensure_input_cursor_visible();
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
        self.ensure_focus_visible();
    }

    pub fn scroll_lines(&mut self, request_id: u64, delta: isize) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        let before = self.scroll_offset;
        self.scroll_offset = self.scroll_offset.saturating_add_signed(delta);
        self.clamp_scroll();
        if before != self.scroll_offset && matches!(self.focus(), Some(InfoOverlayFocus::Choice(_)))
        {
            self.focus = None;
        }
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
        if before.0 != self.scroll_offset
            && matches!(self.focus(), Some(InfoOverlayFocus::Choice(_)))
        {
            self.focus = None;
        }
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
            self.input = None;
            self.scroll_offset = 0;
            self.horizontal_offset = 0;
            self.max_line_chars = 0;
        }
        animating
    }
}

fn sanitize_input(value: &str, max_bytes: usize) -> String {
    let mut output = String::new();
    for character in value.chars().filter(|character| !character.is_control()) {
        if output.len().saturating_add(character.len_utf8()) > max_bytes {
            break;
        }
        output.push(character);
    }
    output
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
    #[test]
    fn keyboard_focus_reaches_all_choices_and_buttons_with_safe_defaults() {
        let mut overlay = InfoOverlay::new();
        let request = overlay.show_choices(
            "Choices",
            (0..64).map(|i| format!("row {i}")).collect(),
            (0..MAX_INFO_CHOICES)
                .map(|i| {
                    InfoOverlayChoice::new(format!("id-{i}"), format!("Choice {i}"), i * 2, None)
                })
                .collect(),
        );
        overlay.set_viewport(4, 30);
        assert_eq!(
            overlay.activate_focused(request),
            InfoOverlayAction::Dismiss {
                request_id: request
            }
        );
        for i in 0..MAX_INFO_CHOICES {
            assert!(overlay.cycle_focus(request, false));
            assert_eq!(
                overlay.focus(),
                Some(&InfoOverlayFocus::Choice(format!("id-{i}")))
            );
            assert_eq!(
                overlay.activate_focused(request),
                InfoOverlayAction::Choose {
                    request_id: request,
                    choice_id: format!("id-{i}")
                }
            );
            assert!(
                (overlay.scroll_offset()..overlay.scroll_offset() + overlay.viewport_rows())
                    .contains(&(i * 2))
            );
        }
        overlay.cycle_focus(request, false);
        assert_eq!(
            overlay.activate_focused(request),
            InfoOverlayAction::Copy {
                request_id: request
            }
        );
        overlay.cycle_focus(request, false);
        assert_eq!(overlay.focus(), Some(&InfoOverlayFocus::Continue));
        overlay.cycle_focus(request, false);
        assert_eq!(
            overlay.focus(),
            Some(&InfoOverlayFocus::Choice("id-0".into()))
        );
        overlay.cycle_focus(request, true);
        assert_eq!(overlay.focus(), Some(&InfoOverlayFocus::Continue));
        overlay.cycle_focus(request, true);
        assert_eq!(overlay.focus(), Some(&InfoOverlayFocus::Copy));
        overlay.cycle_focus(request, true);
        overlay.set_viewport(1, 20);
        assert_eq!(overlay.scroll_offset(), 62);
        overlay.scroll_to_top(request);
        assert!(overlay.focus().is_none());
        assert_eq!(
            overlay.activate_focused(request),
            InfoOverlayAction::Dismiss {
                request_id: request
            }
        );
    }

    #[test]
    fn focused_choices_cannot_survive_dismissal_or_replacement() {
        let mut overlay = InfoOverlay::new();
        let first = overlay.show_choices(
            "Consent",
            vec!["Allow".into()],
            vec![InfoOverlayChoice::new("allow", "Allow", 0, None)],
        );
        overlay.cycle_focus(first, false);
        assert!(overlay.dismiss(first));
        assert!(!overlay.cycle_focus(first, false));
        assert_eq!(overlay.activate_focused(first), InfoOverlayAction::None);
        let second = overlay.show_choices(
            "Different consent",
            vec!["Allow".into()],
            vec![InfoOverlayChoice::new("allow", "Allow", 0, None)],
        );
        assert!(!overlay.set_focus(first, Some(InfoOverlayFocus::Choice("allow".into()))));
        assert_eq!(
            overlay.activate_focused(second),
            InfoOverlayAction::Dismiss { request_id: second }
        );
        assert!(!overlay.set_focus(second, Some(InfoOverlayFocus::Choice("missing".into()))));
    }

    #[test]
    fn input_focus_cycles_through_choices_and_footer_without_editing() {
        let mut overlay = InfoOverlay::new();
        let request = overlay.show_input(
            "Edit",
            vec!["Value".into(), "Reset".into()],
            0,
            "Value: ",
            "warm",
            30,
            vec![InfoOverlayChoice::new("reset", "Reset", 1, None)],
        );
        assert!(overlay.input_focused());
        overlay.set_viewport(1, 2);
        assert!(overlay.horizontal_offset() > 0);
        overlay.cycle_focus(request, false);
        assert!(!overlay.input_focused());
        assert_eq!(overlay.horizontal_offset(), 0);
        assert_eq!(
            overlay.activate_focused(request),
            InfoOverlayAction::Choose {
                request_id: request,
                choice_id: "reset".into()
            }
        );
        overlay.cycle_focus(request, false);
        assert_eq!(
            overlay.activate_focused(request),
            InfoOverlayAction::Copy {
                request_id: request
            }
        );
        overlay.cycle_focus(request, false);
        assert_eq!(
            overlay.activate_focused(request),
            InfoOverlayAction::SubmitInput {
                request_id: request
            }
        );
        overlay.cycle_focus(request, false);
        assert!(overlay.input_focused());
        assert_eq!(overlay.input_value(request), Some("warm"));
    }
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

    #[test]
    fn explicit_animation_phases_keep_content_controls_and_hit_rows_aligned() {
        let calculate =
            |phase| InfoOverlayLayout::calculate(900.0, 500.0, 10.0, 20.0, phase, "Notice", 40, 80);
        let settled = calculate(1.0);
        // The same eased phase has the same geometry on opening and closing.
        // These are explicit inputs, independent of scheduler/Instant timing.
        for phase in [0.0, 0.125, 0.5, 0.875, 1.0, 0.875, 0.5, 0.125, 0.0] {
            let layout = calculate(phase);
            let translation = (1.0 - phase) * -30.0;
            assert_eq!(layout.rows_that_fit, settled.rows_that_fit);
            assert_eq!(layout.columns_that_fit, settled.columns_that_fit);
            for (actual, expected) in [
                (layout.card, settled.card),
                (layout.content, settled.content),
                (layout.copy_button, settled.copy_button),
                (layout.continue_button, settled.continue_button),
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
            for visible in 0..layout.rows_that_fit {
                let row = layout.source_line_rect(5 + visible, 5, 40).unwrap();
                assert_eq!(row.x, layout.content.x);
                assert_eq!(
                    row.y,
                    layout.content.y + visible as f32 * layout.line_height
                );
                assert_eq!(
                    layout.content_row_at(row.x + 1.0, row.y + row.height / 2.0, 5, 40),
                    Some(5 + visible)
                );
            }
            assert!(layout.source_line_rect(4, 5, 40).is_none());
            assert!(
                layout
                    .source_line_rect(5 + layout.rows_that_fit, 5, 40)
                    .is_none()
            );
            assert_eq!(
                layout.content_row_at(layout.content.x - 1.0, layout.content.y, 5, 40),
                None
            );
        }
        assert!((calculate(0.0).card.y - settled.card.y + 30.0).abs() < 0.001);
    }

    #[test]
    fn input_is_request_scoped_bounded_and_utf8_cursor_safe() {
        let mut overlay = InfoOverlay::new();
        let request = overlay.show_input(
            "Edit",
            vec!["Setting".into(), String::new()],
            1,
            "Value: ",
            "a🫚",
            8,
            Vec::new(),
        );
        let input = overlay.input_info(request).unwrap();
        assert_eq!(input.value, "a🫚");
        assert_eq!(input.cursor_byte, "a🫚".len());
        assert_eq!(overlay.lines()[1], "Value: a🫚");

        assert!(overlay.move_input_left(request));
        assert!(overlay.insert_input_text(request, "é"));
        assert_eq!(overlay.input_value(request), Some("aé🫚"));
        overlay.insert_input_text(request, "root");
        assert!(overlay.input_value(request).unwrap().len() <= 8);
        assert!(!overlay.replace_input_text(request, "bad\nvalue"));
        assert!(overlay.set_input_cursor_from_column(request, "Value: ".chars().count()));
        assert!(overlay.delete_input(request));
        assert!(!overlay.input_value(request).unwrap().starts_with('a'));

        let replacement = overlay.show("Replacement", vec!["body".into()]);
        assert_ne!(request, replacement);
        assert!(overlay.input_info(request).is_none());
        assert!(!overlay.insert_input_text(request, "stale"));
    }

    #[test]
    fn input_line_geometry_tracks_the_shared_scrolled_layout() {
        let layout = InfoOverlayLayout::calculate(900.0, 500.0, 10.0, 20.0, 1.0, "Edit", 20, 20);
        let rect = layout.source_line_rect(7, 5, 20).unwrap();
        assert_eq!(rect.x, layout.content.x);
        assert_eq!(rect.y, layout.content.y + layout.line_height * 2.0);
        assert!(layout.source_line_rect(4, 5, 20).is_none());
        assert!(
            layout
                .source_line_rect(5 + layout.rows_that_fit, 5, 20)
                .is_none()
        );
    }
}
