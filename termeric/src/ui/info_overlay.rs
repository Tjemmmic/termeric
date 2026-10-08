//! Bounded modal notices used for application information, errors, and choices.

use super::animation::OverlayAnimation;
use super::request_scope::RequestScope;
use super::text_card::{OverlayScroll, TextCard, TextViewport};
use super::text_input::{TextInput, TextInputEdit, TextLimit, grapheme_boundary};
use crate::ui::geometry::Bounds;

pub const MAX_INFO_TITLE_CHARS: usize = 256;
pub const MAX_INFO_LINES: usize = 512;
pub const MAX_INFO_LINE_CHARS: usize = 4096;
pub const MAX_INFO_CHOICES: usize = 32;
pub const MAX_INFO_INPUT_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoOverlayChoice {
    pub id: String,
    pub label: String,
    pub source_line: usize,
    pub shortcut: Option<char>,
}

/// One page of a list paged through in the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayPage {
    pub index: usize,
    pub count: usize,
    /// The page's items are `start..end` of the list.
    pub start: usize,
    pub end: usize,
}

impl OverlayPage {
    /// Page `requested`, or the last page if it is past the end, of `len`
    /// items `size` to a page. An empty list has one empty page.
    pub fn of(len: usize, size: usize, requested: usize) -> Self {
        let size = size.max(1);
        let count = len.div_ceil(size).max(1);
        let index = requested.min(count - 1);
        let start = index * size;
        Self {
            index,
            count,
            start,
            end: (start + size).min(len),
        }
    }
}

/// Append a line reading "\[K\] label" and a choice on it picked by `key`,
/// which shows upper-case.
pub fn push_key_choice(
    lines: &mut Vec<String>,
    choices: &mut Vec<InfoOverlayChoice>,
    id: impl Into<String>,
    key: char,
    label: impl Into<String>,
) {
    let label = label.into();
    lines.push(format!("[{}] {label}", key.to_ascii_uppercase()));
    choices.push(InfoOverlayChoice::new(
        id,
        label,
        lines.len() - 1,
        Some(key),
    ));
}

/// Append an indented line reading `label` and a choice on it with no key,
/// for a confirmation whose consequence a single key press must not reach.
pub fn push_keyless_choice(
    lines: &mut Vec<String>,
    choices: &mut Vec<InfoOverlayChoice>,
    id: impl Into<String>,
    label: impl Into<String>,
) {
    let label = label.into();
    lines.push(format!("  {label}"));
    choices.push(InfoOverlayChoice::new(id, label, lines.len() - 1, None));
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
    Scroll { request_id: u64, by: OverlayScroll },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoOverlayInputInfo {
    pub label: String,
    pub value: String,
    pub cursor_byte: usize,
    pub source_line: usize,
    pub limit: TextLimit,
}

struct InfoOverlayInput {
    label: String,
    value: TextInput,
    source_line: usize,
}

/// Geometry shared by rendering, pointer hit testing, viewport sizing, and
/// accessibility. Content is always bounded so controls remain on-screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InfoOverlayLayout {
    pub card: Bounds,
    pub content: Bounds,
    pub copy_button: Bounds,
    pub continue_button: Bounds,
    /// The keyboard hint under the buttons.
    pub hint: Bounds,
    pub padding: f32,
    pub title_height: f32,
    pub line_height: f32,
    pub char_scale: f32,
    pub footer_height: f32,
    pub rows_that_fit: usize,
    pub columns_that_fit: usize,
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
        let text = TextCard::calculate(
            window_width,
            window_height,
            cell_width,
            cell_height,
            animation,
            TextCard::text_line_height(cell_height),
            max_line_chars,
            title.chars().count(),
            380.0,
            total_lines,
        );
        let available_button_width = text.button_row_width(1).max(2.0);
        let copy_width = (available_button_width * 0.38).max(1.0);
        let continue_width = (available_button_width - copy_width).max(1.0);
        let copy_x = text.card.x + text.padding;
        let continue_x = copy_x + copy_width + text.button_gap;

        Self {
            card: text.card,
            content: text.content,
            copy_button: text.button(copy_x, copy_width),
            continue_button: text.button(continue_x, continue_width),
            hint: text.hint,
            padding: text.padding,
            title_height: text.title_height,
            line_height: text.line_height,
            char_scale: text.char_scale,
            footer_height: text.footer_height,
            rows_that_fit: text.rows_that_fit,
            columns_that_fit: text.columns_that_fit,
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
    ) -> Option<Bounds> {
        if source_line >= total_lines
            || source_line < scroll_offset
            || source_line >= scroll_offset.saturating_add(self.rows_that_fit)
        {
            return None;
        }
        Some(Bounds {
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
    request: RequestScope,
    viewport: TextViewport,
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
            request: RequestScope::default(),
            viewport: TextViewport::default(),
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
                env!("CARGO_PKG_DESCRIPTION").into(),
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
        let request_id = self.request.begin();
        self.title = crate::safe_text::display_line(&title.into(), MAX_INFO_TITLE_CHARS, 1);
        if self.title.is_empty() {
            self.title = "Notice".into();
        }
        self.lines = bounded_lines(lines);
        self.viewport
            .reset(self.lines.len(), widest_line_chars(&self.lines));
        self.choices = bounded_choices(choices, self.lines.len());
        self.input = None;
        // Continue is the safe default, never the first (possibly consenting)
        // choice. A new request cannot inherit a previous choice's focus.
        self.focus = Some(InfoOverlayFocus::Continue);
        self.animation.open();
        request_id
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
        let label = crate::safe_text::display_line(&label.into(), 128, 1);
        let max_bytes = max_bytes
            .clamp(1, MAX_INFO_INPUT_BYTES)
            .min(MAX_INFO_LINE_CHARS.saturating_sub(label.len()).max(1));
        let value = crate::safe_text::single_line_within(&value.into(), max_bytes);
        let mut input = TextInput::new(TextLimit::Bytes(max_bytes));
        input.replace(&value);
        self.input = Some(InfoOverlayInput {
            label,
            value: input,
            source_line,
        });
        self.refresh_input_line();
        self.focus = Some(InfoOverlayFocus::Input);
        request_id
    }

    pub fn dismiss(&mut self, request_id: u64) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        self.request.revoke();
        self.focus = None;
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
            self.viewport.scroll_to(None, Some(0));
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
                let line = self.choice(self.request.id(), id).unwrap().source_line;
                self.viewport.reveal_row(line);
            }
            _ => {}
        }
    }

    pub fn input_info(&self, request_id: u64) -> Option<InfoOverlayInputInfo> {
        let input = (self.request.id() == request_id).then_some(self.input.as_ref())??;
        Some(InfoOverlayInputInfo {
            label: input.label.clone(),
            value: input.value.text().to_string(),
            cursor_byte: input.value.cursor(),
            source_line: input.source_line,
            limit: input.value.limit(),
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

    /// Insert typed or committed text at the caret, whole or not at all.
    pub fn insert_input_text(&mut self, request_id: u64, text: &str) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        let Some(input) = self.input.as_mut() else {
            return false;
        };
        if !input.value.insert(text) {
            return false;
        }
        self.refresh_input_line();
        true
    }

    pub fn replace_input_text(&mut self, request_id: u64, text: &str) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        let Some(input) = self.input.as_mut() else {
            return false;
        };
        if input.value.text() == text || !input.value.replace(text) {
            return false;
        }
        self.refresh_input_line();
        true
    }

    /// Apply a grapheme edit. True when the text or the caret changed; a
    /// caret-only move still scrolls the caret into view.
    pub fn edit_input(&mut self, request_id: u64, edit: TextInputEdit) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        let Some(input) = self.input.as_mut() else {
            return false;
        };
        let caret = input.value.cursor();
        if input.value.edit_grapheme(edit) {
            self.refresh_input_line();
            return true;
        }
        if input.value.cursor() == caret {
            return false;
        }
        self.ensure_input_cursor_visible();
        true
    }

    pub fn set_input_cursor_from_column(&mut self, request_id: u64, column: usize) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        let Some(input) = self.input.as_ref() else {
            return false;
        };
        // Columns are chars, but the caret only rests between graphemes, so a
        // click inside a cluster lands at its start.
        let value = input.value.text();
        let value_column = column.saturating_sub(input.label.chars().count());
        let byte = value
            .char_indices()
            .nth(value_column)
            .map_or(value.len(), |(byte, _)| byte);
        self.set_input_cursor(request_id, grapheme_boundary(value, byte, true))
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
        if !input.value.place_caret(byte) || before == byte {
            return false;
        }
        self.ensure_input_cursor_visible();
        true
    }

    fn refresh_input_line(&mut self) {
        let Some(input) = self.input.as_ref() else {
            return;
        };
        self.lines[input.source_line] = format!("{}{}", input.label, input.value.text());
        self.viewport
            .set_content(self.lines.len(), widest_line_chars(&self.lines));
        self.ensure_input_visible();
    }

    fn ensure_input_cursor_visible(&mut self) {
        let Some(column) = self.input_cursor_source_column(self.request.id()) else {
            return;
        };
        self.viewport.reveal_column(column);
    }

    fn ensure_input_visible(&mut self) {
        let Some(source_line) = self.input.as_ref().map(|input| input.source_line) else {
            return;
        };
        self.viewport.reveal_row(source_line);
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

    /// Whether C copies the notice: a choice bound to C takes the key
    /// instead. Choice shortcuts are lower-case, as `push_key_choice` draws
    /// them upper-case.
    pub fn copy_key_free(&self) -> bool {
        !self
            .choices
            .iter()
            .any(|choice| choice.shortcut == Some('c'))
    }

    pub fn set_viewport(&mut self, rows: usize, columns: usize) {
        self.viewport.resize(rows, columns);
        self.ensure_focus_visible();
    }

    /// Scroll for the current request; whether anything moved. A choice
    /// scrolled out of view loses keyboard focus, so Enter cannot act on
    /// something no longer shown.
    pub fn scroll(&mut self, request_id: u64, by: OverlayScroll) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        let row = self.viewport.row();
        let moved = self.viewport.apply(by);
        if self.viewport.row() != row && matches!(self.focus(), Some(InfoOverlayFocus::Choice(_))) {
            self.focus = None;
        }
        moved
    }

    /// The part of the text in view.
    pub fn viewport(&self) -> &TextViewport {
        &self.viewport
    }

    pub fn visible_lines(&self) -> impl Iterator<Item = (usize, &str)> {
        self.lines
            .iter()
            .enumerate()
            .skip(self.viewport.row())
            .take(self.viewport.rows())
            .map(|(index, line)| (index, line.as_str()))
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
            self.viewport.reset(0, 0);
        }
        animating
    }
}

fn widest_line_chars(lines: &[String]) -> usize {
    lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0)
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
            output.push(crate::safe_text::display_line(
                line.trim_end_matches('\r'),
                MAX_INFO_LINE_CHARS,
                1,
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
        choice.id = crate::safe_text::display_line(&choice.id, 256, 1);
        choice.label = crate::safe_text::display_line(&choice.label, 512, 1);
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
    fn pushed_choices_sit_on_the_line_that_shows_them() {
        let mut lines = vec!["Question?".to_owned(), String::new()];
        let mut choices = Vec::new();
        push_key_choice(&mut lines, &mut choices, "go", 'g', "Go");
        push_keyless_choice(&mut lines, &mut choices, "stop", "Stop");
        assert_eq!(lines[2..], ["[G] Go", "  Stop"]);
        assert_eq!(
            choices,
            [
                InfoOverlayChoice::new("go", "Go", 2, Some('g')),
                InfoOverlayChoice::new("stop", "Stop", 3, None),
            ]
        );
    }

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
                (overlay.viewport().row()..overlay.viewport().row() + overlay.viewport().rows())
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
        assert_eq!(overlay.viewport().row(), 62);
        overlay.scroll(request, OverlayScroll::Top);
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
        assert!(overlay.viewport().column() > 0);
        overlay.cycle_focus(request, false);
        assert!(!overlay.input_focused());
        assert_eq!(overlay.viewport().column(), 0);
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
        assert_eq!(overlay.viewport().content_columns(), MAX_INFO_LINE_CHARS);
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
        assert!(!overlay.scroll(request, OverlayScroll::Rows(1)));
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
        assert!(overlay.scroll(request, OverlayScroll::Rows(100)));
        assert_eq!(overlay.viewport().row(), 7);
        assert!(overlay.scroll(request, OverlayScroll::Columns(100)));
        assert_eq!(overlay.viewport().column(), overlay.viewport().max_column());
        assert!(overlay.scroll(request, OverlayScroll::Top));
        assert!(overlay.scroll(request, OverlayScroll::LineStart));
        assert_eq!(
            (overlay.viewport().row(), overlay.viewport().column()),
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
        // The pixel between two rows belongs to the lower one only; hover
        // reads the same answer, so one row lights at a time.
        let boundary = layout.content.y + layout.line_height;
        assert_eq!(
            layout.content_row_at(layout.content.x + 1.0, boundary, 5, lines.len()),
            Some(6)
        );
        assert_eq!(
            layout.content_row_at(layout.content.x + 1.0, boundary - 0.01, 5, lines.len()),
            Some(5)
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
            let translation = (1.0 - phase) * -20.0;
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
        assert!((calculate(0.0).card.y - settled.card.y + 20.0).abs() < 0.001);
    }

    fn input_overlay(value: &str, max_bytes: usize) -> (InfoOverlay, u64) {
        let mut overlay = InfoOverlay::new();
        let request = overlay.show_input(
            "Edit",
            vec!["Setting".into(), String::new()],
            1,
            "Value: ",
            value,
            max_bytes,
            Vec::new(),
        );
        (overlay, request)
    }

    #[test]
    fn input_is_request_scoped_and_rejects_oversized_or_control_text_whole() {
        let (mut overlay, request) = input_overlay("a🫚", 8);
        let input = overlay.input_info(request).unwrap();
        assert_eq!(input.value, "a🫚");
        assert_eq!(input.cursor_byte, "a🫚".len());
        assert_eq!(overlay.lines()[1], "Value: a🫚");

        assert!(overlay.edit_input(request, TextInputEdit::Left));
        assert!(overlay.insert_input_text(request, "é"));
        assert_eq!(overlay.input_value(request), Some("aé🫚"));
        assert_eq!(overlay.lines()[1], "Value: aé🫚");
        for rejected in ["root", "x\u{7}", ""] {
            assert!(!overlay.insert_input_text(request, rejected));
            assert_eq!(overlay.input_value(request), Some("aé🫚"));
        }
        assert!(!overlay.replace_input_text(request, "bad\nvalue"));

        let replacement = overlay.show("Replacement", vec!["body".into()]);
        assert_ne!(request, replacement);
        assert!(overlay.input_info(request).is_none());
        assert!(!overlay.insert_input_text(request, "stale"));
        assert!(!overlay.edit_input(request, TextInputEdit::Home));
    }

    #[test]
    fn input_edits_and_clicks_keep_the_caret_between_graphemes() {
        let cluster = "e\u{301}";
        let (mut overlay, request) = input_overlay(&format!("{cluster}x"), 32);
        let caret = |overlay: &InfoOverlay| overlay.input_info(request).unwrap().cursor_byte;

        assert!(overlay.edit_input(request, TextInputEdit::Left));
        assert_eq!(caret(&overlay), cluster.len());
        assert!(overlay.edit_input(request, TextInputEdit::Left));
        assert_eq!(caret(&overlay), 0);
        assert!(!overlay.edit_input(request, TextInputEdit::Left));
        assert!(overlay.edit_input(request, TextInputEdit::Delete));
        assert_eq!(overlay.input_value(request), Some("x"));

        assert!(overlay.replace_input_text(request, &format!("{cluster}x")));
        let label = "Value: ".chars().count();
        // The combining mark's column is inside the cluster.
        assert!(overlay.set_input_cursor_from_column(request, label + 1));
        assert_eq!(caret(&overlay), 0);
        assert!(overlay.set_input_cursor_from_column(request, label + 40));
        assert_eq!(caret(&overlay), cluster.len() + 1);
        assert!(!overlay.set_input_cursor_from_column(request, label + 40));

        assert!(overlay.replace_input_text(request, "one two"));
        assert!(overlay.edit_input(request, TextInputEdit::BackspaceWord));
        assert_eq!(overlay.input_value(request), Some("one "));
    }

    #[test]
    fn caret_only_moves_scroll_the_caret_into_view() {
        let (mut overlay, request) = input_overlay(&"x".repeat(40), 64);
        overlay.set_viewport(5, 10);
        let label = "Value: ".chars().count();
        assert!(overlay.viewport().column() > label);

        assert!(overlay.edit_input(request, TextInputEdit::Home));
        assert_eq!(overlay.viewport().column(), label);
        assert!(overlay.edit_input(request, TextInputEdit::End));
        assert!(overlay.viewport().column() > label);
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
