//! Dialogs: messages, confirmations, and prompts in one quiet modal card.
//!
//! A dialog is built from a [`DialogSpec`]: a heading, body blocks, an
//! optional text field, an optional list of rows, and a right-aligned row of
//! buttons with roles. Each showing is its own request
//! ([`RequestScope`]), and every action names the request it was meant for,
//! so an action aimed at an earlier or dismissed dialog does nothing. The
//! geometry every consumer shares is [`DialogLayout`].

mod layout;

pub use layout::{BlockLayout, DialogHit, DialogLayout, DialogMetrics, FieldLayout};

use super::animation::OverlayAnimation;
use super::request_scope::RequestScope;
use super::text_input::{TextInput, TextInputEdit, TextLimit};

pub const MAX_TITLE_CHARS: usize = 256;
pub const MAX_BLOCKS: usize = 64;
pub const MAX_BLOCK_LINES: usize = 256;
pub const MAX_LINE_CHARS: usize = 4096;
pub const MAX_ROWS: usize = 256;
pub const MAX_BUTTONS: usize = 6;
pub const MAX_FIELD_BYTES: usize = 4096;

/// The answer a dialog's Cancel button gives, which a flow that cares about
/// it handles and which otherwise closes the dialog.
pub const CANCEL: &str = "cancel";

/// What a dialog is, for its width and how assistive technology announces
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    /// A statement the person reads and closes.
    Message,
    /// Something went wrong.
    Error,
    /// A question with consequences.
    Confirmation,
    /// A question answered by typing.
    Prompt,
    /// A list of rows to pick from (the interim managers).
    List,
}

impl DialogKind {
    /// Confirmations and errors interrupt; assistive technology announces
    /// them as alerts.
    pub fn is_alert(self) -> bool {
        matches!(self, Self::Error | Self::Confirmation)
    }

    /// Prompts and lists get the wider card.
    pub fn is_wide(self) -> bool {
        matches!(self, Self::Prompt | Self::List)
    }
}

/// One block of a dialog's body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogBlock {
    /// A paragraph in the UI face, wrapped proportionally.
    Text(String),
    /// A literal (path, command, code, version) in the monospace face on an
    /// inset surface.
    Value(String),
    /// An inline error, such as a validation failure, in the error color.
    Error(String),
}

impl DialogBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    pub fn value(value: impl Into<String>) -> Self {
        Self::Value(value.into())
    }

    pub fn error(error: impl Into<String>) -> Self {
        Self::Error(error.into())
    }

    pub fn content(&self) -> &str {
        match self {
            Self::Text(text) | Self::Value(text) | Self::Error(text) => text,
        }
    }
}

/// How a button looks, and which one Escape and the initial focus pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonRole {
    /// The neutral way out; Escape activates it.
    Cancel,
    /// A neutral surface for secondary actions ("Copy details", "Back").
    Neutral,
    /// The accent fill: the action the dialog suggests.
    Suggested,
    /// The error fill: an action that destroys something.
    Destructive,
}

/// What activating a button does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ButtonAction {
    /// Hand this answer to the flow that opened the dialog.
    Answer(String),
    /// Close the dialog, answering nothing.
    Close,
    /// Copy the heading and body to the clipboard; the dialog stays open.
    CopyDetails,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogButton {
    pub label: String,
    pub role: ButtonRole,
    pub action: ButtonAction,
}

impl DialogButton {
    pub fn answer(id: impl Into<String>, label: impl Into<String>, role: ButtonRole) -> Self {
        Self {
            label: label.into(),
            role,
            action: ButtonAction::Answer(id.into()),
        }
    }

    /// "Cancel", answering [`CANCEL`].
    pub fn cancel() -> Self {
        Self::answer(CANCEL, "Cancel", ButtonRole::Cancel)
    }

    /// A message's one button.
    pub fn close() -> Self {
        Self {
            label: "Close".into(),
            role: ButtonRole::Suggested,
            action: ButtonAction::Close,
        }
    }

    pub fn copy_details() -> Self {
        Self {
            label: "Copy details".into(),
            role: ButtonRole::Neutral,
            action: ButtonAction::CopyDetails,
        }
    }

    fn answer_id(&self) -> Option<&str> {
        match &self.action {
            ButtonAction::Answer(id) => Some(id),
            _ => None,
        }
    }
}

/// A row of a dialog's list: activating it answers `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogRow {
    pub id: String,
    pub label: String,
    /// Dim text at the row's right end, such as a value or a state.
    pub detail: Option<String>,
}

impl DialogRow {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// A prompt's single-line text field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSpec {
    pub label: String,
    pub value: String,
    pub max_bytes: usize,
    /// Why the last value was refused, shown under the field.
    pub error: Option<String>,
}

impl FieldSpec {
    pub fn new(label: impl Into<String>, value: impl Into<String>, max_bytes: usize) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            max_bytes,
            error: None,
        }
    }

    pub fn with_error(mut self, error: Option<impl Into<String>>) -> Self {
        self.error = error.map(Into::into);
        self
    }
}

/// Everything a dialog shows. Build one with a constructor for its shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogSpec {
    pub kind: DialogKind,
    pub title: String,
    pub body: Vec<DialogBlock>,
    pub field: Option<FieldSpec>,
    pub rows: Vec<DialogRow>,
    /// Left to right as drawn.
    pub buttons: Vec<DialogButton>,
}

impl DialogSpec {
    fn new(kind: DialogKind, title: impl Into<String>, body: Vec<DialogBlock>) -> Self {
        Self {
            kind,
            title: title.into(),
            body,
            field: None,
            rows: Vec::new(),
            buttons: Vec::new(),
        }
    }

    /// A statement and one Suggested "Close", which Escape also activates.
    pub fn message(title: impl Into<String>, body: Vec<DialogBlock>) -> Self {
        Self::new(DialogKind::Message, title, body).button(DialogButton::close())
    }

    /// A failure: "Copy details", then "Close".
    pub fn error(title: impl Into<String>, body: Vec<DialogBlock>) -> Self {
        Self::new(DialogKind::Error, title, body)
            .button(DialogButton::copy_details())
            .button(DialogButton::close())
    }

    /// A question: Cancel, then the action answering `id`.
    pub fn confirm(
        title: impl Into<String>,
        body: Vec<DialogBlock>,
        id: impl Into<String>,
        action_label: impl Into<String>,
        role: ButtonRole,
    ) -> Self {
        Self::new(DialogKind::Confirmation, title, body)
            .button(DialogButton::cancel())
            .button(DialogButton::answer(id, action_label, role))
    }

    /// A field, Cancel, and a Suggested action answering `id`.
    pub fn prompt(
        title: impl Into<String>,
        body: Vec<DialogBlock>,
        field: FieldSpec,
        id: impl Into<String>,
        action_label: impl Into<String>,
    ) -> Self {
        let mut spec = Self::new(DialogKind::Prompt, title, body)
            .button(DialogButton::cancel())
            .button(DialogButton::answer(
                id,
                action_label,
                ButtonRole::Suggested,
            ));
        spec.field = Some(field);
        spec
    }

    /// Rows to pick from, with the buttons added after.
    pub fn list(title: impl Into<String>, body: Vec<DialogBlock>, rows: Vec<DialogRow>) -> Self {
        let mut spec = Self::new(DialogKind::List, title, body);
        spec.rows = rows;
        spec
    }

    /// Add a button at the right end of the row.
    pub fn button(mut self, button: DialogButton) -> Self {
        self.buttons.push(button);
        self
    }

    /// Relabel the Cancel button, such as "Back" in a manager.
    pub fn cancel_label(mut self, label: impl Into<String>) -> Self {
        if let Some(cancel) = self
            .buttons
            .iter_mut()
            .find(|button| button.role == ButtonRole::Cancel)
        {
            cancel.label = label.into();
        }
        self
    }
}

/// Where keyboard focus rests in a dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogFocus {
    Field,
    Row(usize),
    Button(usize),
}

/// One scroll of the body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogScroll {
    Lines(isize),
    Pages(isize),
    Top,
    Bottom,
}

/// Something a person did to a dialog, for the request it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogAction {
    /// A button or row answered `id`.
    Answer {
        request_id: u64,
        id: String,
    },
    /// Close without an answer.
    Close {
        request_id: u64,
    },
    /// Copy the heading and body.
    Copy {
        request_id: u64,
    },
    Scroll {
        request_id: u64,
        by: DialogScroll,
    },
    None,
}

/// The field as the dialog holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogFieldView {
    pub label: String,
    pub value: String,
    pub cursor_byte: usize,
    pub limit: TextLimit,
    pub error: Option<String>,
}

struct DialogField {
    label: String,
    input: TextInput,
    error: Option<String>,
}

/// The measured body the scroll offset moves over, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BodyExtent {
    /// The height of the body's viewport.
    pub viewport: f32,
    /// The height of everything in the body.
    pub content: f32,
    /// One line, the step of a wheel notch or arrow key.
    pub line: f32,
}

impl BodyExtent {
    fn max_offset(self) -> f32 {
        (self.content - self.viewport).max(0.0).round()
    }
}

/// Request-scoped dialog state.
pub struct Dialog {
    animation: OverlayAnimation,
    request: RequestScope,
    kind: DialogKind,
    title: String,
    body: Vec<DialogBlock>,
    field: Option<DialogField>,
    rows: Vec<DialogRow>,
    buttons: Vec<DialogButton>,
    focus: Option<DialogFocus>,
    /// The row Tab returns to in the list.
    list_row: usize,
    extent: BodyExtent,
    scroll: f32,
    /// Focus moved and the next measure should scroll it into view.
    reveal_focus: bool,
    /// The person scrolled and a focused row now out of view loses focus.
    scrolled: bool,
}

impl Default for Dialog {
    fn default() -> Self {
        Self::new()
    }
}

impl Dialog {
    pub fn new() -> Self {
        Self {
            animation: OverlayAnimation::new(),
            request: RequestScope::default(),
            kind: DialogKind::Message,
            title: String::new(),
            body: Vec::new(),
            field: None,
            rows: Vec::new(),
            buttons: Vec::new(),
            focus: None,
            list_row: 0,
            extent: BodyExtent::default(),
            scroll: 0.0,
            reveal_focus: false,
            scrolled: false,
        }
    }

    /// Show `spec` as a new request, replacing whatever was shown, and
    /// return the request's id.
    pub fn show(&mut self, spec: DialogSpec) -> u64 {
        let request_id = self.request.begin();
        self.kind = spec.kind;
        self.title = crate::safe_text::display_line(&spec.title, MAX_TITLE_CHARS, 1);
        if self.title.is_empty() {
            self.title = "Notice".into();
        }
        self.body = bounded_blocks(spec.body);
        let (rows, omitted) = bounded_rows(spec.rows);
        if omitted > 0 {
            self.body
                .push(DialogBlock::Text(format!("{omitted} more aren't shown.")));
        }
        self.rows = rows;
        self.buttons = bounded_buttons(spec.buttons);
        self.field = spec.field.map(bounded_field);
        self.list_row = 0;
        self.scroll = 0.0;
        self.extent = BodyExtent::default();
        self.scrolled = false;
        self.focus = Some(self.initial_focus());
        self.reveal_focus = true;
        self.animation.open();
        request_id
    }

    /// The field if there is one; otherwise Cancel beside a Destructive
    /// button, so a stray Enter cannot destroy anything; otherwise the
    /// Suggested button; otherwise the first.
    fn initial_focus(&self) -> DialogFocus {
        if self.field.is_some() {
            return DialogFocus::Field;
        }
        let find = |role| self.buttons.iter().position(|button| button.role == role);
        let destructive = find(ButtonRole::Destructive).is_some();
        let button = destructive
            .then(|| find(ButtonRole::Cancel))
            .flatten()
            .or_else(|| find(ButtonRole::Suggested))
            .unwrap_or(0);
        DialogFocus::Button(button)
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

    pub fn kind(&self) -> DialogKind {
        self.kind
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn body(&self) -> &[DialogBlock] {
        &self.body
    }

    pub fn rows(&self) -> &[DialogRow] {
        &self.rows
    }

    pub fn buttons(&self) -> &[DialogButton] {
        &self.buttons
    }

    /// The heading and every body block, a blank line apart: what "Copy
    /// details" copies, and what assistive technology reads as the
    /// dialog's description (without the heading).
    pub fn copy_text(&self, request_id: u64) -> Option<String> {
        self.is_awaiting(request_id).then(|| {
            std::iter::once(self.title.as_str())
                .chain(self.body.iter().map(DialogBlock::content))
                .collect::<Vec<_>>()
                .join("\n\n")
        })
    }

    /// Whether the current request has a button or row answering `id`.
    pub fn answers(&self, request_id: u64, id: &str) -> bool {
        self.is_awaiting(request_id)
            && (self
                .buttons
                .iter()
                .any(|button| button.answer_id() == Some(id))
                || self.rows.iter().any(|row| row.id == id))
    }

    pub fn focus(&self) -> Option<DialogFocus> {
        self.focus.filter(|_| self.is_awaiting_current())
    }

    pub fn field_focused(&self) -> bool {
        self.focus() == Some(DialogFocus::Field)
    }

    pub fn has_field(&self) -> bool {
        self.is_awaiting_current() && self.field.is_some()
    }

    /// Focus is request-scoped and never activates anything by itself.
    pub fn set_focus(&mut self, request_id: u64, focus: Option<DialogFocus>) -> bool {
        if !self.is_awaiting(request_id) || focus.is_some_and(|focus| !self.focusable(focus)) {
            return false;
        }
        if let Some(DialogFocus::Row(row)) = focus {
            self.list_row = row;
        }
        self.focus = focus;
        self.reveal_focus = true;
        true
    }

    fn focusable(&self, focus: DialogFocus) -> bool {
        match focus {
            DialogFocus::Field => self.field.is_some(),
            DialogFocus::Row(row) => row < self.rows.len(),
            DialogFocus::Button(button) => button < self.buttons.len(),
        }
    }

    /// Tab and Shift+Tab: the field, the list (one stop, at the row it was
    /// left on), then each button, wrapping.
    pub fn cycle_focus(&mut self, request_id: u64, backwards: bool) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        let mut stops = Vec::with_capacity(self.buttons.len() + 2);
        if self.field.is_some() {
            stops.push(DialogFocus::Field);
        }
        if !self.rows.is_empty() {
            stops.push(DialogFocus::Row(self.list_row.min(self.rows.len() - 1)));
        }
        stops.extend((0..self.buttons.len()).map(DialogFocus::Button));
        let current = self.focus().and_then(|focus| {
            stops.iter().position(|stop| match (stop, focus) {
                (DialogFocus::Row(_), DialogFocus::Row(_)) => true,
                (stop, focus) => *stop == focus,
            })
        });
        let next = match (current, backwards) {
            (Some(index), true) => (index + stops.len() - 1) % stops.len(),
            (Some(index), false) => (index + 1) % stops.len(),
            (None, true) => stops.len() - 1,
            (None, false) => 0,
        };
        self.set_focus(request_id, Some(stops[next]))
    }

    /// Left and Right between buttons, stopping at either end. Only a
    /// focused button moves.
    pub fn step_button(&mut self, request_id: u64, forward: bool) -> bool {
        let Some(DialogFocus::Button(button)) = self.focus() else {
            return false;
        };
        let next = if forward {
            (button + 1).min(self.buttons.len() - 1)
        } else {
            button.saturating_sub(1)
        };
        next != button && self.set_focus(request_id, Some(DialogFocus::Button(next)))
    }

    /// Whether focus is in the list.
    pub fn list_focused(&self) -> bool {
        matches!(self.focus(), Some(DialogFocus::Row(_)))
    }

    /// Up and Down (and Page Up/Down by `delta` rows) within the list,
    /// stopping at either end.
    pub fn move_row(&mut self, request_id: u64, delta: isize) -> bool {
        let Some(DialogFocus::Row(row)) = self.focus() else {
            return false;
        };
        let last = self.rows.len().saturating_sub(1);
        let next = row.saturating_add_signed(delta).min(last);
        next != row && self.set_focus(request_id, Some(DialogFocus::Row(next)))
    }

    /// Home and End in the list.
    pub fn row_edge(&mut self, request_id: u64, last: bool) -> bool {
        if !self.list_focused() {
            return false;
        }
        let row = if last {
            self.rows.len().saturating_sub(1)
        } else {
            0
        };
        self.set_focus(request_id, Some(DialogFocus::Row(row)))
    }

    /// Enter: the focused button or row; in the field, the Suggested
    /// button. With nothing focused, nothing.
    pub fn activate_focused(&self, request_id: u64) -> DialogAction {
        if !self.is_awaiting(request_id) {
            return DialogAction::None;
        }
        match self.focus() {
            Some(DialogFocus::Field) => self
                .buttons
                .iter()
                .position(|button| button.role == ButtonRole::Suggested)
                .map_or(DialogAction::None, |button| {
                    self.activate_button(request_id, button)
                }),
            Some(DialogFocus::Row(row)) => self.activate_row(request_id, row),
            Some(DialogFocus::Button(button)) => self.activate_button(request_id, button),
            None => DialogAction::None,
        }
    }

    /// Escape: the Cancel button, or close.
    pub fn escape(&self, request_id: u64) -> DialogAction {
        if !self.is_awaiting(request_id) {
            return DialogAction::None;
        }
        self.buttons
            .iter()
            .position(|button| button.role == ButtonRole::Cancel)
            .map_or(DialogAction::Close { request_id }, |button| {
                self.activate_button(request_id, button)
            })
    }

    pub fn activate_button(&self, request_id: u64, button: usize) -> DialogAction {
        let Some(button) = self
            .buttons
            .get(button)
            .filter(|_| self.is_awaiting(request_id))
        else {
            return DialogAction::None;
        };
        match &button.action {
            ButtonAction::Answer(id) => DialogAction::Answer {
                request_id,
                id: id.clone(),
            },
            ButtonAction::Close => DialogAction::Close { request_id },
            ButtonAction::CopyDetails => DialogAction::Copy { request_id },
        }
    }

    pub fn activate_row(&self, request_id: u64, row: usize) -> DialogAction {
        self.rows
            .get(row)
            .filter(|_| self.is_awaiting(request_id))
            .map_or(DialogAction::None, |row| DialogAction::Answer {
                request_id,
                id: row.id.clone(),
            })
    }

    pub fn field(&self, request_id: u64) -> Option<DialogFieldView> {
        let field = (self.request.id() == request_id).then_some(self.field.as_ref())??;
        Some(DialogFieldView {
            label: field.label.clone(),
            value: field.input.text().to_owned(),
            cursor_byte: field.input.cursor(),
            limit: field.input.limit(),
            error: field.error.clone(),
        })
    }

    pub fn field_value(&self, request_id: u64) -> Option<&str> {
        let field = self
            .is_awaiting(request_id)
            .then_some(self.field.as_ref())??;
        Some(field.input.text())
    }

    fn field_mut(&mut self, request_id: u64) -> Option<&mut TextInput> {
        if !self.is_awaiting(request_id) {
            return None;
        }
        let input = &mut self.field.as_mut()?.input;
        // Typing into a field scrolled out of view brings it back.
        self.reveal_focus = true;
        Some(input)
    }

    /// Insert typed, committed, or pasted text at the caret, whole or not at
    /// all.
    pub fn insert_field_text(&mut self, request_id: u64, text: &str) -> bool {
        self.field_mut(request_id)
            .is_some_and(|input| input.insert(text))
    }

    pub fn replace_field_text(&mut self, request_id: u64, text: &str) -> bool {
        self.field_mut(request_id)
            .is_some_and(|input| input.text() != text && input.replace(text))
    }

    /// Apply a grapheme edit. True when the text or the caret changed.
    pub fn edit_field(&mut self, request_id: u64, edit: TextInputEdit) -> bool {
        self.field_mut(request_id).is_some_and(|input| {
            let before = (input.text().len(), input.cursor());
            input.edit_grapheme(edit) || (input.text().len(), input.cursor()) != before
        })
    }

    /// Put the caret at byte `cursor`, a grapheme boundary.
    pub fn place_field_caret(&mut self, request_id: u64, cursor: usize) -> bool {
        self.field_mut(request_id).is_some_and(|input| {
            let before = input.cursor();
            input.place_caret(cursor) && before != cursor
        })
    }

    /// Scroll the body for the current request; whether anything moved. A
    /// row scrolled out of view loses focus at the next measure, so Enter
    /// cannot act on something no longer shown.
    pub fn scroll(&mut self, request_id: u64, by: DialogScroll) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        let extent = self.extent;
        let page = (extent.viewport - extent.line).max(extent.line);
        let target = match by {
            DialogScroll::Lines(lines) => self.scroll + lines as f32 * extent.line,
            DialogScroll::Pages(pages) => self.scroll + pages as f32 * page,
            DialogScroll::Top => 0.0,
            DialogScroll::Bottom => extent.max_offset(),
        };
        let target = target.clamp(0.0, extent.max_offset()).round();
        if target == self.scroll {
            return false;
        }
        self.scroll = target;
        self.scrolled = true;
        true
    }

    /// The body's scroll offset in physical pixels.
    pub fn scroll_offset(&self) -> f32 {
        self.scroll
    }

    pub fn extent(&self) -> BodyExtent {
        self.extent
    }

    /// Take the body's measure from the layout just drawn: clamp the scroll
    /// to it, scroll focus moved by keyboard into view, and drop focus from
    /// a row the person scrolled away from. `span` gives where a focus sits
    /// in the body, top and bottom, when it sits there at all.
    pub fn sync_body(
        &mut self,
        extent: BodyExtent,
        span: impl Fn(DialogFocus) -> Option<(f32, f32)>,
    ) {
        self.extent = extent;
        let max = extent.max_offset();
        let focused = self.focus().and_then(|focus| Some((focus, span(focus)?)));
        if self.reveal_focus
            && let Some((_, (top, bottom))) = focused
        {
            if top < self.scroll {
                self.scroll = top;
            } else if bottom > self.scroll + extent.viewport {
                self.scroll = bottom - extent.viewport;
            }
        } else if self.scrolled
            && let Some((DialogFocus::Row(_), (top, bottom))) = focused
            && (top < self.scroll || bottom > self.scroll + extent.viewport)
        {
            self.focus = None;
        }
        self.scroll = self.scroll.clamp(0.0, max).round();
        self.reveal_focus = false;
        self.scrolled = false;
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
            self.body.clear();
            self.rows.clear();
            self.buttons.clear();
            self.field = None;
            self.scroll = 0.0;
        }
        animating
    }
}

/// `text` as a dialog shows it: each line made safe to display, and at most
/// [`MAX_BLOCK_LINES`] of them.
fn bounded_paragraph(text: &str) -> String {
    let mut lines = text.split('\n');
    let mut shown: Vec<String> = lines
        .by_ref()
        .take(MAX_BLOCK_LINES)
        .map(|line| crate::safe_text::display_line(line.trim_end_matches('\r'), MAX_LINE_CHARS, 4))
        .collect();
    if lines.next().is_some() {
        shown.push("…".into());
    }
    shown.join("\n")
}

fn bounded_blocks(blocks: Vec<DialogBlock>) -> Vec<DialogBlock> {
    blocks
        .into_iter()
        .take(MAX_BLOCKS)
        .map(|block| match block {
            DialogBlock::Text(text) => DialogBlock::Text(bounded_paragraph(&text)),
            DialogBlock::Value(text) => DialogBlock::Value(bounded_paragraph(&text)),
            DialogBlock::Error(text) => DialogBlock::Error(bounded_paragraph(&text)),
        })
        .filter(|block| !block.content().is_empty())
        .collect()
}

/// The rows a dialog keeps, unique by id and with something to show, and
/// how many past [`MAX_ROWS`] it leaves out.
fn bounded_rows(rows: Vec<DialogRow>) -> (Vec<DialogRow>, usize) {
    let mut kept: Vec<DialogRow> = Vec::new();
    let mut omitted = 0;
    for row in rows {
        let row = DialogRow {
            id: crate::safe_text::display_line(&row.id, 256, 1),
            label: crate::safe_text::display_line(&row.label, 512, 1),
            detail: row
                .detail
                .map(|detail| crate::safe_text::display_line(&detail, 256, 1))
                .filter(|detail| !detail.is_empty()),
        };
        if row.id.is_empty() || row.label.is_empty() || kept.iter().any(|kept| kept.id == row.id) {
            continue;
        }
        if kept.len() == MAX_ROWS {
            omitted += 1;
            continue;
        }
        kept.push(row);
    }
    (kept, omitted)
}

/// At most [`MAX_BUTTONS`] labelled buttons answering distinct ids, and
/// never none: a dialog always has a way out.
fn bounded_buttons(buttons: Vec<DialogButton>) -> Vec<DialogButton> {
    let mut kept: Vec<DialogButton> = Vec::new();
    for mut button in buttons {
        button.label = crate::safe_text::display_line(&button.label, 128, 1);
        if let ButtonAction::Answer(id) = &mut button.action {
            *id = crate::safe_text::display_line(id, 256, 1);
        }
        let duplicate = kept.iter().any(|kept| kept.action == button.action);
        let unanswerable = button.answer_id().is_some_and(str::is_empty);
        if kept.len() == MAX_BUTTONS || button.label.is_empty() || duplicate || unanswerable {
            continue;
        }
        kept.push(button);
    }
    if kept.is_empty() {
        kept.push(DialogButton::close());
    }
    kept
}

fn bounded_field(field: FieldSpec) -> DialogField {
    let max_bytes = field.max_bytes.clamp(1, MAX_FIELD_BYTES);
    let mut input = TextInput::new(TextLimit::Bytes(max_bytes));
    input.replace(&crate::safe_text::single_line_within(
        &field.value,
        max_bytes,
    ));
    DialogField {
        label: crate::safe_text::display_line(&field.label, 128, 1),
        input,
        error: field
            .error
            .map(|error| bounded_paragraph(&error))
            .filter(|error| !error.is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(count: usize) -> Vec<DialogRow> {
        (0..count)
            .map(|index| DialogRow::new(format!("row:{index}"), format!("Row {index}")))
            .collect()
    }

    /// A body of `rows` 10-pixel lines in a 30-pixel viewport, rows at
    /// their own line.
    fn sync(dialog: &mut Dialog, rows: usize) {
        dialog.sync_body(
            BodyExtent {
                viewport: 30.0,
                content: rows as f32 * 10.0,
                line: 10.0,
            },
            |focus| match focus {
                DialogFocus::Row(row) => Some((row as f32 * 10.0, row as f32 * 10.0 + 10.0)),
                _ => None,
            },
        );
    }

    #[test]
    fn constructors_give_each_shape_its_buttons_and_roles() {
        let message = DialogSpec::message("Saved", vec![DialogBlock::text("Done.")]);
        assert_eq!(message.kind, DialogKind::Message);
        assert_eq!(message.buttons, [DialogButton::close()]);

        let error = DialogSpec::error("Failed", vec![]);
        assert_eq!(
            error.buttons,
            [DialogButton::copy_details(), DialogButton::close()]
        );
        assert!(error.kind.is_alert());

        let confirm = DialogSpec::confirm(
            "Delete?",
            vec![],
            "delete",
            "Delete",
            ButtonRole::Destructive,
        );
        assert_eq!(
            confirm
                .buttons
                .iter()
                .map(|button| (button.label.as_str(), button.role))
                .collect::<Vec<_>>(),
            [
                ("Cancel", ButtonRole::Cancel),
                ("Delete", ButtonRole::Destructive)
            ]
        );
        assert!(confirm.kind.is_alert() && !confirm.kind.is_wide());

        let prompt = DialogSpec::prompt(
            "Name?",
            vec![],
            FieldSpec::new("Name", "", 32),
            "save",
            "Save",
        );
        assert!(prompt.field.is_some() && prompt.kind.is_wide() && !prompt.kind.is_alert());
        assert_eq!(prompt.buttons[1].role, ButtonRole::Suggested);
        assert_eq!(
            DialogSpec::confirm("?", vec![], "go", "Go", ButtonRole::Suggested)
                .cancel_label("Back")
                .buttons[0]
                .label,
            "Back"
        );
    }

    #[test]
    fn initial_focus_is_the_field_then_the_safe_choice_then_the_suggestion() {
        let mut dialog = Dialog::new();
        dialog.show(DialogSpec::prompt(
            "?",
            vec![],
            FieldSpec::new("Name", "", 8),
            "save",
            "Save",
        ));
        assert_eq!(dialog.focus(), Some(DialogFocus::Field));

        dialog.show(DialogSpec::confirm(
            "?",
            vec![],
            "delete",
            "Delete",
            ButtonRole::Destructive,
        ));
        assert_eq!(dialog.focus(), Some(DialogFocus::Button(0)), "Cancel");

        dialog.show(DialogSpec::confirm(
            "?",
            vec![],
            "go",
            "Go",
            ButtonRole::Suggested,
        ));
        assert_eq!(
            dialog.focus(),
            Some(DialogFocus::Button(1)),
            "the suggestion"
        );

        dialog.show(DialogSpec::error("Failed", vec![]));
        assert_eq!(dialog.focus(), Some(DialogFocus::Button(1)), "Close");

        dialog.show(
            DialogSpec::list("Pick", vec![], rows(3)).button(DialogButton::answer(
                "back",
                "Back",
                ButtonRole::Neutral,
            )),
        );
        assert_eq!(dialog.focus(), Some(DialogFocus::Button(0)), "the first");
    }

    #[test]
    fn tab_visits_field_list_and_buttons_and_arrows_stay_in_their_group() {
        let mut dialog = Dialog::new();
        let mut spec = DialogSpec::list("Pick", vec![], rows(5))
            .button(DialogButton::cancel())
            .button(DialogButton::answer("add", "Add", ButtonRole::Suggested));
        spec.field = Some(FieldSpec::new("Filter", "", 16));
        let request = dialog.show(spec);
        assert_eq!(dialog.focus(), Some(DialogFocus::Field));
        let mut visited = Vec::new();
        for _ in 0..4 {
            assert!(dialog.cycle_focus(request, false));
            visited.push(dialog.focus().unwrap());
        }
        assert_eq!(
            visited,
            [
                DialogFocus::Row(0),
                DialogFocus::Button(0),
                DialogFocus::Button(1),
                DialogFocus::Field
            ]
        );

        // Up and Down move within the list; Tab leaves it and comes back to
        // the row it left.
        dialog.cycle_focus(request, false);
        assert!(dialog.move_row(request, 3));
        assert_eq!(dialog.focus(), Some(DialogFocus::Row(3)));
        assert!(
            dialog.move_row(request, 9),
            "a page past the end stops at it"
        );
        assert_eq!(dialog.focus(), Some(DialogFocus::Row(4)));
        assert!(!dialog.move_row(request, 1), "the end of the list");
        assert!(dialog.row_edge(request, false));
        assert_eq!(dialog.focus(), Some(DialogFocus::Row(0)));
        assert!(dialog.move_row(request, 2));
        dialog.cycle_focus(request, false);
        assert_eq!(dialog.focus(), Some(DialogFocus::Button(0)));
        dialog.cycle_focus(request, true);
        assert_eq!(dialog.focus(), Some(DialogFocus::Row(2)));

        // Left and Right move between buttons and stop at the ends.
        assert!(!dialog.step_button(request, true), "not on a button");
        dialog.set_focus(request, Some(DialogFocus::Button(0)));
        assert!(!dialog.step_button(request, false));
        assert!(dialog.step_button(request, true));
        assert_eq!(dialog.focus(), Some(DialogFocus::Button(1)));
        assert!(!dialog.step_button(request, true));
        dialog.cycle_focus(request, false);
        assert_eq!(dialog.focus(), Some(DialogFocus::Field), "Tab wraps");
        dialog.cycle_focus(request, true);
        assert_eq!(
            dialog.focus(),
            Some(DialogFocus::Button(1)),
            "Shift+Tab wraps"
        );
    }

    #[test]
    fn enter_and_escape_answer_through_the_buttons() {
        let mut dialog = Dialog::new();
        let request = dialog.show(DialogSpec::prompt(
            "Name?",
            vec![],
            FieldSpec::new("Name", "ginger", 32),
            "save",
            "Save",
        ));
        let answer = |id: &str| DialogAction::Answer {
            request_id: request,
            id: id.into(),
        };
        assert_eq!(
            dialog.activate_focused(request),
            answer("save"),
            "Enter in the field"
        );
        assert_eq!(dialog.escape(request), answer(CANCEL));
        dialog.set_focus(request, Some(DialogFocus::Button(0)));
        assert_eq!(dialog.activate_focused(request), answer(CANCEL));

        let request = dialog.show(DialogSpec::message("Saved", vec![]));
        assert_eq!(
            dialog.escape(request),
            DialogAction::Close {
                request_id: request
            }
        );
        assert_eq!(
            dialog.activate_focused(request),
            DialogAction::Close {
                request_id: request
            }
        );

        let request = dialog.show(DialogSpec::error(
            "Failed",
            vec![DialogBlock::text("Disk full")],
        ));
        assert_eq!(
            dialog.activate_button(request, 0),
            DialogAction::Copy {
                request_id: request
            }
        );
        assert_eq!(
            dialog.copy_text(request).as_deref(),
            Some("Failed\n\nDisk full")
        );

        let request = dialog.show(DialogSpec::list("Pick", vec![], rows(2)));
        assert_eq!(
            dialog.escape(request),
            DialogAction::Close {
                request_id: request
            }
        );
        dialog.set_focus(request, Some(DialogFocus::Row(1)));
        assert_eq!(
            dialog.activate_focused(request),
            DialogAction::Answer {
                request_id: request,
                id: "row:1".into()
            }
        );
        assert!(dialog.answers(request, "row:0"));
        assert!(!dialog.answers(request, "row:2"));
    }

    #[test]
    fn actions_for_a_dismissed_or_replaced_dialog_do_nothing() {
        let mut dialog = Dialog::new();
        let first = dialog.show(DialogSpec::confirm(
            "Delete?",
            vec![],
            "delete",
            "Delete",
            ButtonRole::Destructive,
        ));
        dialog.set_focus(first, Some(DialogFocus::Button(1)));
        assert!(dialog.dismiss(first));
        assert!(
            dialog.is_active(),
            "the closing animation still blocks input"
        );
        assert_eq!(dialog.activate_focused(first), DialogAction::None);
        assert_eq!(dialog.activate_button(first, 1), DialogAction::None);
        assert_eq!(dialog.escape(first), DialogAction::None);
        assert!(!dialog.answers(first, "delete"));
        assert!(!dialog.cycle_focus(first, false));
        assert!(!dialog.dismiss(first));

        let second = dialog.show(DialogSpec::confirm(
            "Delete?",
            vec![],
            "delete",
            "Delete",
            ButtonRole::Destructive,
        ));
        assert_ne!(first, second);
        assert!(!dialog.set_focus(first, Some(DialogFocus::Button(1))));
        assert_eq!(
            dialog.focus(),
            Some(DialogFocus::Button(0)),
            "a new request starts safe"
        );
        assert!(!dialog.set_focus(second, Some(DialogFocus::Button(7))));
        assert!(!dialog.set_focus(second, Some(DialogFocus::Field)));
        assert_eq!(dialog.copy_text(first), None);
    }

    #[test]
    fn the_field_edits_whole_and_only_for_its_request() {
        let mut dialog = Dialog::new();
        let request = dialog.show(DialogSpec::prompt(
            "Rule",
            vec![],
            FieldSpec::new("Rule", "a\u{1fada}", 8).with_error(Some("bad")),
            "save",
            "Save",
        ));
        let field = dialog.field(request).unwrap();
        assert_eq!(field.value, "a\u{1fada}");
        assert_eq!(field.cursor_byte, field.value.len());
        assert_eq!(field.error.as_deref(), Some("bad"));
        assert!(dialog.edit_field(request, TextInputEdit::Left));
        assert!(dialog.insert_field_text(request, "é"));
        assert_eq!(dialog.field_value(request), Some("aé\u{1fada}"));
        for refused in ["root", "x\u{7}", "two\nlines"] {
            assert!(!dialog.insert_field_text(request, refused));
        }
        assert!(!dialog.replace_field_text(request, "bad\nvalue"));
        assert!(dialog.place_field_caret(request, 0));
        assert!(!dialog.place_field_caret(request, 0), "already there");
        assert!(dialog.replace_field_text(request, "one two"));
        assert!(dialog.edit_field(request, TextInputEdit::BackspaceWord));
        assert_eq!(dialog.field_value(request), Some("one "));

        let replacement = dialog.show(DialogSpec::message("Other", vec![]));
        assert!(dialog.field(request).is_none());
        assert!(!dialog.insert_field_text(request, "stale"));
        assert!(!dialog.insert_field_text(replacement, "none"));
    }

    #[test]
    fn scrolling_clamps_to_the_body_and_reveals_keyboard_focus() {
        let mut dialog = Dialog::new();
        let request =
            dialog.show(DialogSpec::list("Pick", vec![], rows(10)).button(DialogButton::cancel()));
        sync(&mut dialog, 10);
        assert!(dialog.scroll(request, DialogScroll::Lines(100)));
        assert_eq!(dialog.scroll_offset(), 70.0);
        assert!(!dialog.scroll(request, DialogScroll::Bottom));
        assert!(dialog.scroll(request, DialogScroll::Pages(-1)));
        assert_eq!(dialog.scroll_offset(), 50.0);
        assert!(dialog.scroll(request, DialogScroll::Top));

        // Keyboard focus on a row out of view scrolls just enough to show it.
        dialog.set_focus(request, Some(DialogFocus::Row(6)));
        sync(&mut dialog, 10);
        assert_eq!(dialog.scroll_offset(), 40.0);
        dialog.move_row(request, -4);
        sync(&mut dialog, 10);
        assert_eq!(dialog.scroll_offset(), 20.0);

        // Scrolling a focused row out of view drops its focus, so Enter
        // cannot answer something no longer shown.
        dialog.scroll(request, DialogScroll::Lines(3));
        sync(&mut dialog, 10);
        assert_eq!(dialog.focus(), None);
        assert_eq!(dialog.activate_focused(request), DialogAction::None);

        // A body that shrinks keeps the scroll inside it.
        dialog.scroll(request, DialogScroll::Bottom);
        sync(&mut dialog, 4);
        assert_eq!(dialog.scroll_offset(), 10.0);
        dialog.dismiss(request);
        assert!(!dialog.scroll(request, DialogScroll::Top));
    }

    #[test]
    fn content_is_bounded_and_a_dialog_always_has_a_way_out() {
        let mut dialog = Dialog::new();
        let mut spec = DialogSpec::list(
            "x".repeat(1_000),
            vec![DialogBlock::text("line\n".repeat(MAX_BLOCK_LINES + 10)); MAX_BLOCKS + 5],
            rows(MAX_ROWS + 3),
        );
        spec.rows.push(DialogRow::new("row:0", "Duplicate"));
        spec.rows.push(DialogRow::new("", "No id"));
        let request = dialog.show(spec);
        assert!(dialog.title().chars().count() <= MAX_TITLE_CHARS);
        assert_eq!(dialog.rows().len(), MAX_ROWS);
        // The rows left out are said to be, after the bounded blocks.
        assert_eq!(dialog.body().len(), MAX_BLOCKS + 1);
        assert!(dialog.body()[0].content().lines().count() <= MAX_BLOCK_LINES + 1);
        assert_eq!(dialog.buttons(), [DialogButton::close()]);
        assert_eq!(
            dialog.escape(request),
            DialogAction::Close {
                request_id: request
            }
        );

        let mut spec = DialogSpec::message("Many", vec![]);
        spec.buttons = (0..MAX_BUTTONS + 2)
            .map(|index| DialogButton::answer(format!("b{index}"), "B", ButtonRole::Neutral))
            .chain([DialogButton::answer("b0", "Again", ButtonRole::Neutral)])
            .collect();
        dialog.show(spec);
        assert_eq!(dialog.buttons().len(), MAX_BUTTONS);
    }
}
