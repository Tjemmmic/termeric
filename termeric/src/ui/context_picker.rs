//! The context picker: a request-scoped modal for choosing what context to
//! send an agent. `agents::context` gathers it.

use std::path::PathBuf;

use super::animation::{OverlayAnimation, card_open_slide};
use super::constants::CONTENT_CHAR_SCALE;
use super::request_scope::RequestScope;
use super::text_input::{TextInput, TextInputEdit, TextLimit};
use super::text_input_viewport::TextInputViewport;
use crate::agents::AgentTarget;
use crate::agents::context::{
    ContextGatherRequest, MAX_CONTEXT_FILES, MAX_CONTEXT_PATH_CHARS, MAX_CONTEXT_SOURCE_CHARS,
    is_git_repo, validate_chosen_files, validate_specific_files,
};
use crate::ui::geometry::Bounds;

pub const MAX_CONTEXT_SOURCES: usize = 6;
/// Every path, with room for the separators between them.
const MAX_FILE_INPUT_CHARS: usize = MAX_CONTEXT_PATH_CHARS * MAX_CONTEXT_FILES;
/// The most one paste into the file field can carry: everything the field
/// holds, each character at its widest UTF-8 length.
pub const MAX_FILE_INPUT_BYTES: usize = MAX_FILE_INPUT_CHARS * 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContextSourceId {
    GitDiffUnstaged,
    GitDiffStaged,
    FileTree,
    TerminalOutput,
    TestOutput,
    SpecificFiles,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSource {
    pub id: ContextSourceId,
    pub label: String,
    pub selected: bool,
    pub available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextPickerPhase {
    Sources,
    FilePaths,
    Gathering,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextPickerAction {
    Cancel {
        request_id: u64,
    },
    Move {
        request_id: u64,
        delta: isize,
    },
    Scroll {
        request_id: u64,
        delta: isize,
    },
    Toggle {
        request_id: u64,
        source: ContextSourceId,
    },
    Primary {
        request_id: u64,
    },
    EditFiles {
        request_id: u64,
    },
    SetFileInput {
        request_id: u64,
        value: String,
    },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextPickerCompletion {
    Ignored,
    Failed,
    Ready { target: AgentTarget, text: String },
}

/// The picker's buttons. Every action is a row or one of them; there are no
/// letter keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContextPickerButton {
    /// Open the editor for the paths the Specific Files source sends.
    ChooseFiles,
    /// Close the picker, or go back from the file editor; Escape answers it.
    Cancel,
    /// Insert the chosen context, or save the typed paths.
    Primary,
}

impl ContextPickerButton {
    /// Left to right as drawn, which is also the focus order after the
    /// phase's list or field.
    const SOURCES_ROW: [Self; 3] = [Self::ChooseFiles, Self::Cancel, Self::Primary];
    const DIALOG_ROW: [Self; 2] = [Self::Cancel, Self::Primary];

    /// The buttons `phase` offers: Choose Files… only beside the sources.
    pub fn row(phase: ContextPickerPhase) -> &'static [Self] {
        if phase == ContextPickerPhase::Sources {
            &Self::SOURCES_ROW
        } else {
            &Self::DIALOG_ROW
        }
    }

    pub fn label(self, phase: ContextPickerPhase) -> &'static str {
        match (self, phase) {
            (Self::ChooseFiles, _) => "Choose Files…",
            (Self::Cancel, ContextPickerPhase::FilePaths) => "Back",
            (Self::Cancel, _) => "Cancel",
            (Self::Primary, ContextPickerPhase::Sources) => "Insert",
            (Self::Primary, ContextPickerPhase::FilePaths) => "Save",
            (Self::Primary, ContextPickerPhase::Gathering) => "Gathering…",
        }
    }

    /// What pressing it does for request `request_id`.
    pub fn action(self, request_id: u64) -> ContextPickerAction {
        match self {
            Self::ChooseFiles => ContextPickerAction::EditFiles { request_id },
            Self::Cancel => ContextPickerAction::Cancel { request_id },
            Self::Primary => ContextPickerAction::Primary { request_id },
        }
    }

    /// Whether it can be pressed in `phase`: Choose Files… needs a working
    /// folder for the paths, and nothing inserts while gathering.
    fn enabled(self, phase: ContextPickerPhase, has_working_folder: bool) -> bool {
        Self::row(phase).contains(&self)
            && match self {
                Self::ChooseFiles => has_working_folder,
                Self::Cancel => true,
                Self::Primary => phase != ContextPickerPhase::Gathering,
            }
    }
}

/// Where the picker's keyboard focus is. Tab and Shift+Tab move it through
/// the phase's list or field and then its enabled buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextPickerFocus {
    /// The source list: the arrows move its selection and Space toggles it.
    Sources,
    /// The file-path field, which takes typing.
    FileInput,
    Button(ContextPickerButton),
}

/// What a point on the picker lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextPickerTarget {
    Button(ContextPickerButton),
    /// The source at this index.
    Source(usize),
    FileInput,
}

#[derive(Debug, Clone)]
pub struct ContextPickerSnapshot {
    pub request_id: u64,
    pub sources: Vec<ContextSource>,
    pub selected_index: usize,
    pub scroll_offset: usize,
    pub phase: ContextPickerPhase,
    pub file_input: String,
    /// The file field's caret, as a byte offset into `file_input`.
    pub file_cursor: usize,
    pub status: Option<String>,
    pub awaiting: bool,
    pub animation: f32,
    pub focus: ContextPickerFocus,
    /// Whether there is a working folder to choose files in.
    pub has_working_folder: bool,
}

impl ContextPickerSnapshot {
    /// The buttons this phase offers, left to right.
    pub fn buttons(&self) -> &'static [ContextPickerButton] {
        ContextPickerButton::row(self.phase)
    }

    pub fn is_enabled(&self, button: ContextPickerButton) -> bool {
        button.enabled(self.phase, self.has_working_folder)
    }

    /// Whether `button` is drawn with the focus ring.
    pub fn is_focused(&self, button: ContextPickerButton) -> bool {
        self.awaiting && self.focus == ContextPickerFocus::Button(button)
    }
}

/// Geometry shared by rendering, pointer hit testing, viewport clamping, and
/// accessibility semantics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextPickerLayout {
    pub card: Bounds,
    pub content: Bounds,
    pub file_input: Bounds,
    /// Present only beside the sources.
    pub choose_files_button: Option<Bounds>,
    pub cancel_button: Bounds,
    pub primary_button: Bounds,
    /// The status line under the buttons, for gathering progress and errors.
    pub status: Bounds,
    pub padding: f32,
    pub title_height: f32,
    pub row_height: f32,
    pub visible_count: usize,
    pub char_scale: f32,
    /// The terminal cell, which the file field's text is placed by, like
    /// every other single-line field.
    pub cell_width: f32,
    pub cell_height: f32,
    /// The gap between the file field's edges and its text.
    pub field_inset: f32,
}

impl ContextPickerLayout {
    /// Lay out the centred card: a title, the phase's rows, the button row,
    /// and the status line under it. `animation` slides it down into place
    /// as it opens, by a distance `scale` (logical to physical pixels) sizes
    /// for the display.
    pub fn calculate(
        window_width: f32,
        window_height: f32,
        scale: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
        phase: ContextPickerPhase,
        source_count: usize,
    ) -> Self {
        let desired_rows = if phase == ContextPickerPhase::FilePaths {
            3
        } else {
            source_count.max(1)
        };
        // Every length is whole, so the card and everything placed from it
        // land on whole pixels; rows tile through `Bounds::row`.
        let row_height = cell_height * 2.2;
        let padding = (cell_width * 2.0).max(12.0).round();
        let title_height = (cell_height * 2.5).round();
        // The button row and the status line under it.
        let footer_height = (cell_height * 4.5).round();
        let bottom_margin = (padding * 0.5).round();
        let outer_margin = (cell_width * 2.0).max(12.0).round();
        let scrollbar_gutter = cell_width.max(6.0).round();

        // A fixed width: the rows are labels and checkboxes, not text that
        // asks for columns.
        let available_width = (window_width - outer_margin * 2.0).floor().max(1.0);
        let card_width = (cell_width * 58.0).round().max(440.0).min(available_width);

        let fixed_height = padding + title_height + footer_height + bottom_margin;
        let available_height = (window_height - outer_margin * 2.0).max(row_height);
        let content_budget = (available_height - fixed_height).max(row_height);
        let rows_that_fit = ((content_budget / row_height).floor() as usize).max(1);
        let visible_count = desired_rows.min(rows_that_fit);
        let content_height = (visible_count as f32 * row_height).round();
        let card_height = fixed_height + content_height;
        let slide = card_open_slide(animation, scale);
        let card = Bounds::new(
            ((window_width - card_width) / 2.0).round(),
            ((window_height - card_height) / 2.0).round() - slide,
            card_width,
            card_height,
        );
        let char_scale = (cell_width * CONTENT_CHAR_SCALE).max(1.0);
        let content = Bounds::new(
            card.x + padding,
            card.y + padding + title_height,
            (card_width - padding * 2.0 - scrollbar_gutter).max(char_scale.ceil()),
            content_height,
        );

        let button_y = content.bottom() + (cell_height * 1.3).round();
        let button_height = (cell_height * 1.8).round();
        let button_gap = (cell_width * 1.5).round();
        let button = |x: f32, width: f32| Bounds::new(x, button_y, width.max(1.0), button_height);
        // The row inside the padding, less the gaps between `buttons` buttons.
        let row_width = |buttons: usize| {
            (card_width - padding * 2.0 - button_gap * buttons.saturating_sub(1) as f32)
                .max(buttons as f32)
        };
        let row_x = card.x + padding;
        let (choose_files_button, cancel_button, primary_button) =
            if phase == ContextPickerPhase::Sources {
                // Choose Files… and Cancel share the row equally with a
                // wider Insert.
                let available = row_width(3);
                let small = (available * 0.3).round();
                let step = small + button_gap;
                (
                    Some(button(row_x, small)),
                    button(row_x + step, small),
                    button(row_x + step * 2.0, available - small * 2.0),
                )
            } else {
                let available = row_width(2);
                let cancel = (available * 0.38).round();
                (
                    None,
                    button(row_x, cancel),
                    button(row_x + cancel + button_gap, available - cancel),
                )
            };

        Self {
            card,
            content,
            file_input: Bounds {
                x: content.x,
                y: content.y + (row_height * 0.75).round(),
                width: content.width,
                height: row_height.round(),
            },
            choose_files_button,
            cancel_button,
            primary_button,
            // Half a cell below the buttons, inside the padding; its text is
            // centred in it.
            status: Bounds::new(
                content.x,
                button_y + button_height + (cell_height * 0.5).round(),
                card_width - padding * 2.0,
                cell_height.round(),
            ),
            padding,
            title_height,
            row_height,
            visible_count,
            char_scale,
            cell_width,
            cell_height,
            field_inset: cell_width.round(),
        }
    }

    /// The file field's visible text and caret for `text` with its caret at
    /// byte `cursor`: what is drawn, where the input method's caret goes,
    /// and where a press places the caret.
    pub fn file_input_viewport(&self, text: &str, cursor: usize) -> Option<TextInputViewport> {
        let field = self.file_input;
        TextInputViewport::calculate(
            text,
            cursor,
            field.x + self.field_inset,
            (field.y + (field.height - self.cell_height) / 2.0).round(),
            field.right() - self.field_inset,
            self.cell_width,
        )
    }

    /// The caret position a press at `x` on the file field puts in `text`.
    pub fn file_caret_at(&self, text: &str, cursor: usize, x: f32) -> Option<usize> {
        self.file_input_viewport(text, cursor)?
            .cursor_at(text, x, self.cell_width)
    }

    /// The `visible_index`th visible source row, where it is drawn.
    pub fn row(&self, visible_index: usize) -> Bounds {
        self.content.row(self.row_height, visible_index)
    }

    /// The source under a point, if any: the one whose drawn row contains it.
    pub fn source_row_at(
        &self,
        x: f32,
        y: f32,
        scroll_offset: usize,
        source_count: usize,
    ) -> Option<usize> {
        let visible_rows = source_count
            .saturating_sub(scroll_offset)
            .min(self.visible_count);
        (0..visible_rows)
            .find(|&visible_index| self.row(visible_index).contains(x, y))
            .map(|visible_index| scroll_offset + visible_index)
    }

    /// Where `button` is drawn, if this phase offers it.
    pub fn button(&self, button: ContextPickerButton) -> Option<Bounds> {
        match button {
            ContextPickerButton::ChooseFiles => self.choose_files_button,
            ContextPickerButton::Cancel => Some(self.cancel_button),
            ContextPickerButton::Primary => Some(self.primary_button),
        }
    }

    /// What a point lands on in `snapshot`'s phase, if anything.
    pub fn target_at(
        &self,
        x: f32,
        y: f32,
        snapshot: &ContextPickerSnapshot,
    ) -> Option<ContextPickerTarget> {
        if let Some(button) = snapshot.buttons().iter().copied().find(|&button| {
            self.button(button)
                .is_some_and(|bounds| bounds.contains(x, y))
        }) {
            return Some(ContextPickerTarget::Button(button));
        }
        match snapshot.phase {
            ContextPickerPhase::Sources => self
                .source_row_at(x, y, snapshot.scroll_offset, snapshot.sources.len())
                .map(ContextPickerTarget::Source),
            ContextPickerPhase::FilePaths => self
                .file_input
                .contains(x, y)
                .then_some(ContextPickerTarget::FileInput),
            ContextPickerPhase::Gathering => None,
        }
    }
}

pub struct ContextPicker {
    animation: OverlayAnimation,
    sources: Vec<ContextSource>,
    selected_index: usize,
    scroll_offset: usize,
    viewport_rows: usize,
    cwd: Option<PathBuf>,
    target: Option<AgentTarget>,
    phase: ContextPickerPhase,
    file_input: TextInput,
    specific_files: Vec<PathBuf>,
    status: Option<String>,
    request: RequestScope,
    focus: ContextPickerFocus,
}

impl ContextPicker {
    pub fn new() -> Self {
        Self {
            animation: OverlayAnimation::new(),
            sources: Vec::new(),
            selected_index: 0,
            scroll_offset: 0,
            viewport_rows: 1,
            cwd: None,
            target: None,
            phase: ContextPickerPhase::Sources,
            file_input: TextInput::new(TextLimit::Chars(MAX_FILE_INPUT_CHARS)),
            specific_files: Vec::new(),
            status: None,
            request: RequestScope::default(),
            focus: ContextPickerFocus::Sources,
        }
    }

    /// The agent terminal the open picker gathers context for.
    pub fn target(&self) -> Option<&AgentTarget> {
        self.target.as_ref()
    }

    pub fn open(&mut self, cwd: Option<String>, target: AgentTarget) -> u64 {
        let request_id = self.request.begin();
        self.cwd = cwd.map(PathBuf::from);
        let in_git = is_git_repo(self.cwd.as_deref());
        self.sources = vec![
            source(
                ContextSourceId::GitDiffUnstaged,
                "Git Diff (unstaged)",
                in_git,
                in_git,
            ),
            source(
                ContextSourceId::GitDiffStaged,
                "Git Diff (staged)",
                false,
                in_git,
            ),
            source(
                ContextSourceId::FileTree,
                "File Tree",
                !in_git,
                self.cwd.is_some(),
            ),
            source(
                ContextSourceId::TerminalOutput,
                "Recent Terminal Output (50 lines)",
                false,
                true,
            ),
            source(ContextSourceId::TestOutput, "Last Test Output", false, true),
            // Available once Choose Files… has given it paths.
            source(
                ContextSourceId::SpecificFiles,
                "Specific Files",
                false,
                false,
            ),
        ];
        debug_assert_eq!(self.sources.len(), MAX_CONTEXT_SOURCES);
        // The next measure scrolls the selection into view.
        (self.selected_index, self.focus) = initial_selection(&self.sources);
        self.scroll_offset = 0;
        self.target = Some(target);
        self.phase = ContextPickerPhase::Sources;
        self.file_input.clear();
        self.specific_files.clear();
        self.status = None;
        self.animation.open();
        request_id
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

    /// The folder the context comes from, where the file chooser starts.
    pub fn cwd(&self) -> Option<&std::path::Path> {
        self.cwd.as_deref()
    }

    /// Whether Choose Files… in request `request_id` would open now: the
    /// sources show and there is a folder to choose in.
    pub fn can_choose_files(&self, request_id: u64) -> bool {
        self.is_awaiting(request_id)
            && self.phase == ContextPickerPhase::Sources
            && self.cwd.is_some()
    }

    /// Use the files the desktop's chooser returned for request
    /// `request_id`. Ones it refuses (outside the folder, not files, too
    /// many) open the typed list with them in it and the reason, to fix.
    pub fn use_chosen_files(&mut self, request_id: u64, chosen: &[PathBuf]) {
        if !self.can_choose_files(request_id) {
            return;
        }
        match validate_chosen_files(self.cwd.as_deref(), chosen) {
            Ok(paths) => self.set_specific_files(paths),
            Err(error) => {
                self.specific_files = chosen.to_vec();
                self.open_file_editor();
                self.status = Some(error);
            }
        }
    }

    pub fn phase(&self) -> ContextPickerPhase {
        self.phase
    }

    pub fn sources(&self) -> &[ContextSource] {
        &self.sources
    }

    pub fn selected_source(&self) -> Option<ContextSourceId> {
        self.sources
            .get(self.selected_index)
            .map(|source| source.id)
    }

    pub fn select_source(&mut self, request_id: u64, source: ContextSourceId) -> bool {
        if !self.is_awaiting(request_id) || self.phase != ContextPickerPhase::Sources {
            return false;
        }
        let Some(index) = self.sources.iter().position(|entry| entry.id == source) else {
            return false;
        };
        self.selected_index = index;
        self.reveal_selection();
        true
    }

    pub fn viewport_rows(&self) -> usize {
        self.viewport_rows
    }

    pub fn snapshot(&self) -> ContextPickerSnapshot {
        ContextPickerSnapshot {
            request_id: self.request.id(),
            sources: self.sources.clone(),
            selected_index: self.selected_index,
            scroll_offset: self.scroll_offset,
            phase: self.phase,
            file_input: self.file_input.text().to_string(),
            file_cursor: self.file_input.cursor(),
            status: self.status.clone(),
            awaiting: self.is_awaiting_current(),
            animation: self.animation(),
            focus: self.focus,
            has_working_folder: self.cwd.is_some(),
        }
    }

    #[cfg(test)]
    pub fn focus(&self) -> ContextPickerFocus {
        self.focus
    }

    /// Whether the file field holds the keyboard: it is focused and the
    /// picker still answers its request.
    pub fn file_input_focused(&self) -> bool {
        self.is_awaiting_current() && self.focus == ContextPickerFocus::FileInput
    }

    /// Insert typed, pasted, or composed text at the file field's caret,
    /// whole or not at all. Returns whether it was accepted.
    pub fn insert_file_text(&mut self, request_id: u64, text: &str) -> bool {
        let inserted = self.editing_files(request_id) && self.file_input.insert(text);
        if inserted {
            self.status = None;
        }
        inserted
    }

    /// Apply a grapheme edit to the file field. True when the text or the
    /// caret changed.
    pub fn edit_file_input(&mut self, request_id: u64, edit: TextInputEdit) -> bool {
        if !self.editing_files(request_id) {
            return false;
        }
        let caret = self.file_input.cursor();
        if self.file_input.edit_grapheme(edit) {
            self.status = None;
            return true;
        }
        self.file_input.cursor() != caret
    }

    /// Put the file field's caret at byte `cursor`, a grapheme boundary.
    /// True when it moved.
    pub fn place_file_caret(&mut self, request_id: u64, cursor: usize) -> bool {
        self.editing_files(request_id)
            && self.file_input.cursor() != cursor
            && self.file_input.place_caret(cursor)
    }

    /// Whether request `request_id` is awaited and its file editor is open.
    fn editing_files(&self, request_id: u64) -> bool {
        self.is_awaiting(request_id) && self.phase == ContextPickerPhase::FilePaths
    }

    /// The buttons this phase offers, left to right.
    pub fn buttons(&self) -> &'static [ContextPickerButton] {
        ContextPickerButton::row(self.phase)
    }

    pub fn is_enabled(&self, button: ContextPickerButton) -> bool {
        button.enabled(self.phase, self.cwd.is_some())
    }

    /// What pressing `button` does now: nothing when it is disabled, not
    /// offered, or the request is no longer awaited.
    pub fn button_action(
        &self,
        request_id: u64,
        button: ContextPickerButton,
    ) -> ContextPickerAction {
        if self.is_awaiting(request_id) && self.is_enabled(button) {
            button.action(request_id)
        } else {
            ContextPickerAction::None
        }
    }

    /// Focus is request-scoped, goes only to what this phase offers and can
    /// answer, and never activates anything by itself.
    pub fn set_focus(&mut self, request_id: u64, focus: ContextPickerFocus) -> bool {
        let offered = match focus {
            ContextPickerFocus::Sources => self.phase == ContextPickerPhase::Sources,
            ContextPickerFocus::FileInput => self.phase == ContextPickerPhase::FilePaths,
            ContextPickerFocus::Button(button) => self.is_enabled(button),
        };
        if !self.is_awaiting(request_id) || !offered {
            return false;
        }
        self.focus = focus;
        true
    }

    /// The phase's focus stops in order: its list or field, then its
    /// enabled buttons left to right.
    fn focus_order(&self) -> Vec<ContextPickerFocus> {
        let main = match self.phase {
            ContextPickerPhase::Sources => Some(ContextPickerFocus::Sources),
            ContextPickerPhase::FilePaths => Some(ContextPickerFocus::FileInput),
            ContextPickerPhase::Gathering => None,
        };
        main.into_iter()
            .chain(
                self.buttons()
                    .iter()
                    .copied()
                    .filter(|&button| self.is_enabled(button))
                    .map(ContextPickerFocus::Button),
            )
            .collect()
    }

    /// Move focus to the next stop, or the previous one, wrapping around.
    pub fn cycle_focus(&mut self, request_id: u64, backwards: bool) -> bool {
        let order = self.focus_order();
        if order.is_empty() {
            return false;
        }
        let next = match (order.iter().position(|&stop| stop == self.focus), backwards) {
            (Some(index), false) => (index + 1) % order.len(),
            (Some(index), true) => (index + order.len() - 1) % order.len(),
            (None, false) => 0,
            (None, true) => order.len() - 1,
        };
        self.set_focus(request_id, order[next])
    }

    /// What Enter does: press the focused button, or the primary one from
    /// the list or the field.
    pub fn enter_action(&self, request_id: u64) -> ContextPickerAction {
        match self.focus {
            ContextPickerFocus::Button(button) => self.button_action(request_id, button),
            ContextPickerFocus::Sources | ContextPickerFocus::FileInput => {
                self.button_action(request_id, ContextPickerButton::Primary)
            }
        }
    }

    /// What Space does: press the focused button, or toggle the selected
    /// source from the list.
    pub fn space_action(&self, request_id: u64) -> ContextPickerAction {
        match self.focus {
            ContextPickerFocus::Button(button) => self.button_action(request_id, button),
            ContextPickerFocus::Sources => self
                .selected_source()
                .filter(|_| self.is_awaiting(request_id))
                .map_or(ContextPickerAction::None, |source| {
                    ContextPickerAction::Toggle { request_id, source }
                }),
            ContextPickerFocus::FileInput => ContextPickerAction::None,
        }
    }

    /// A primary press on `target`: it takes focus, and a button or source
    /// row is pressed.
    pub fn press(&mut self, request_id: u64, target: ContextPickerTarget) -> ContextPickerAction {
        match target {
            ContextPickerTarget::Button(button) => {
                if self.set_focus(request_id, ContextPickerFocus::Button(button)) {
                    self.button_action(request_id, button)
                } else {
                    ContextPickerAction::None
                }
            }
            ContextPickerTarget::Source(index) => {
                match self.sources.get(index).map(|source| source.id) {
                    Some(source) if self.set_focus(request_id, ContextPickerFocus::Sources) => {
                        ContextPickerAction::Toggle { request_id, source }
                    }
                    _ => ContextPickerAction::None,
                }
            }
            ContextPickerTarget::FileInput => {
                self.set_focus(request_id, ContextPickerFocus::FileInput);
                ContextPickerAction::None
            }
        }
    }

    pub fn set_viewport(&mut self, rows: usize) {
        self.viewport_rows = rows.max(1);
        self.reveal_selection();
    }

    pub fn apply(&mut self, action: ContextPickerAction) -> Option<ContextGatherRequest> {
        let request_id = match &action {
            ContextPickerAction::Cancel { request_id }
            | ContextPickerAction::Move { request_id, .. }
            | ContextPickerAction::Scroll { request_id, .. }
            | ContextPickerAction::Toggle { request_id, .. }
            | ContextPickerAction::Primary { request_id }
            | ContextPickerAction::EditFiles { request_id }
            | ContextPickerAction::SetFileInput { request_id, .. } => *request_id,
            ContextPickerAction::None => return None,
        };
        if !self.is_awaiting(request_id) {
            return None;
        }
        match action {
            ContextPickerAction::Cancel { .. } => {
                if self.phase == ContextPickerPhase::FilePaths {
                    self.phase = ContextPickerPhase::Sources;
                    self.status = None;
                    self.focus = ContextPickerFocus::Button(ContextPickerButton::ChooseFiles);
                } else {
                    self.request.revoke();
                    self.status = None;
                    self.target = None;
                    self.animation.close();
                }
            }
            ContextPickerAction::Move { delta, .. }
                if self.phase == ContextPickerPhase::Sources =>
            {
                self.selected_index = moved_selection(&self.sources, self.selected_index, delta);
                self.reveal_selection();
                // The arrows move the list's selection, so the list takes
                // focus with them.
                self.focus = ContextPickerFocus::Sources;
            }
            ContextPickerAction::Scroll { delta, .. }
                if self.phase == ContextPickerPhase::Sources =>
            {
                self.scroll_offset = self.scroll_offset.saturating_add_signed(delta);
                self.clamp_scroll();
            }
            ContextPickerAction::Toggle { source, .. }
                if self.phase == ContextPickerPhase::Sources =>
            {
                if let Some(index) = self.sources.iter().position(|entry| entry.id == source) {
                    self.selected_index = index;
                    self.reveal_selection();
                    if self.sources[index].available {
                        self.sources[index].selected = !self.sources[index].selected;
                        self.status = None;
                    }
                }
            }
            ContextPickerAction::EditFiles { .. } if self.phase == ContextPickerPhase::Sources => {
                self.open_file_editor();
            }
            ContextPickerAction::SetFileInput { value, .. }
                if self.phase == ContextPickerPhase::FilePaths =>
            {
                if self.file_input.replace(&value) {
                    self.status = None;
                }
            }
            ContextPickerAction::Primary { .. } => match self.phase {
                ContextPickerPhase::Sources => {}
                ContextPickerPhase::FilePaths => self.commit_file_editor(),
                ContextPickerPhase::Gathering => {}
            },
            _ => {}
        }
        None
    }

    pub fn begin_gather(
        &mut self,
        request_id: u64,
        terminal_output: Option<String>,
    ) -> Option<ContextGatherRequest> {
        if !self.is_awaiting(request_id) || self.phase != ContextPickerPhase::Sources {
            return None;
        }
        let selected: Vec<_> = self
            .sources
            .iter()
            .filter(|source| source.selected && source.available)
            .map(|source| source.id)
            .collect();
        if selected.is_empty() {
            self.status = Some("Select at least one available context source".into());
            return None;
        }
        self.phase = ContextPickerPhase::Gathering;
        self.status = Some("Gathering context…".into());
        // Cancel is the one button that answers while gathering.
        self.focus = ContextPickerFocus::Button(ContextPickerButton::Cancel);
        Some(ContextGatherRequest {
            request_id,
            cwd: self.cwd.clone(),
            selected,
            specific_files: self.specific_files.clone(),
            terminal_output: terminal_output.map(|text| {
                crate::safe_text::truncate_with_ellipsis(&text, MAX_CONTEXT_SOURCE_CHARS)
            }),
        })
    }

    pub fn complete(
        &mut self,
        request_id: u64,
        result: Result<String, String>,
    ) -> ContextPickerCompletion {
        if !self.is_awaiting(request_id) || self.phase != ContextPickerPhase::Gathering {
            return ContextPickerCompletion::Ignored;
        }
        match result {
            Ok(text) => {
                let Some(target) = self.target.take() else {
                    self.phase = ContextPickerPhase::Sources;
                    self.focus = ContextPickerFocus::Sources;
                    self.status = Some("The context destination is no longer available".into());
                    return ContextPickerCompletion::Failed;
                };
                self.request.revoke();
                self.status = None;
                self.animation.close();
                ContextPickerCompletion::Ready { target, text }
            }
            Err(error) => {
                self.phase = ContextPickerPhase::Sources;
                self.focus = ContextPickerFocus::Sources;
                self.status = Some(crate::safe_text::truncate_with_ellipsis(&error, 512));
                ContextPickerCompletion::Failed
            }
        }
    }

    fn open_file_editor(&mut self) {
        if self.cwd.is_none() {
            self.status = Some("Specific files require a working directory".into());
            return;
        }
        let paths = self
            .specific_files
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        // Paths the field would refuse leave it empty to be typed again.
        self.file_input.clear();
        self.file_input.replace(&paths);
        self.phase = ContextPickerPhase::FilePaths;
        self.status = None;
        self.focus = ContextPickerFocus::FileInput;
    }

    fn commit_file_editor(&mut self) {
        match validate_specific_files(self.cwd.as_deref(), self.file_input.text()) {
            Ok(paths) => self.set_specific_files(paths),
            Err(error) => self.status = Some(error),
        }
    }

    /// Send `paths` as Specific Files, back on the sources.
    fn set_specific_files(&mut self, paths: Vec<PathBuf>) {
        self.specific_files = paths;
        if let Some(source) = self
            .sources
            .iter_mut()
            .find(|source| source.id == ContextSourceId::SpecificFiles)
        {
            // Saving paths includes them; saving none leaves the
            // source nothing to send.
            let chosen = !self.specific_files.is_empty();
            source.available = chosen;
            source.selected = chosen;
            source.label = if chosen {
                format!("Specific Files ({})", self.specific_files.len())
            } else {
                "Specific Files".into()
            };
        }
        self.phase = ContextPickerPhase::Sources;
        self.status = None;
        self.focus = ContextPickerFocus::Button(ContextPickerButton::ChooseFiles);
    }

    fn reveal_selection(&mut self) {
        self.scroll_offset = crate::ui::scroll::reveal_row(
            self.scroll_offset,
            self.selected_index,
            self.viewport_rows,
        );
        self.clamp_scroll();
    }

    fn clamp_scroll(&mut self) {
        self.scroll_offset = self
            .scroll_offset
            .min(self.sources.len().saturating_sub(self.viewport_rows));
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
            self.sources.clear();
            self.cwd = None;
            self.target = None;
            self.file_input.clear();
            self.specific_files.clear();
            self.status = None;
            self.scroll_offset = 0;
        }
        animating
    }
}

/// Where a fresh picker's selection and focus start: on the first source
/// that can be chosen, or on Cancel when none can, so focus never starts on
/// a row that does nothing.
fn initial_selection(sources: &[ContextSource]) -> (usize, ContextPickerFocus) {
    match sources.iter().position(|source| source.available) {
        Some(index) => (index, ContextPickerFocus::Sources),
        None => (0, ContextPickerFocus::Button(ContextPickerButton::Cancel)),
    }
}

/// Where moving the selection `delta` rows from `current` lands: the row
/// that far, or the nearest source that can be chosen past it, else the
/// nearest one short of it; the selection stays put when there is none that
/// way. A source that cannot be chosen is never selected by the arrows.
fn moved_selection(sources: &[ContextSource], current: usize, delta: isize) -> usize {
    let last = sources.len().saturating_sub(1);
    let target = current.saturating_add_signed(delta).min(last);
    let available = |index: &usize| sources[*index].available;
    if delta >= 0 {
        (target..=last)
            .find(available)
            .or_else(|| (current + 1..target).rev().find(available))
    } else {
        (0..=target)
            .rev()
            .find(available)
            .or_else(|| (target + 1..current).find(available))
    }
    .unwrap_or(current)
}

fn source(id: ContextSourceId, label: &str, selected: bool, available: bool) -> ContextSource {
    ContextSource {
        id,
        label: label.into(),
        selected,
        available,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(tab_id: usize) -> AgentTarget {
        AgentTarget {
            identity: crate::tabs::TerminalIdentity {
                tab_id,
                pane_id: crate::panes::PaneId(0),
            },
            agent_id: "fixture-agent".into(),
            agent_name: "Fixture Agent".into(),
        }
    }

    #[test]
    fn modal_actions_are_request_scoped_and_dismiss_revokes_immediately() {
        let mut picker = ContextPicker::new();
        let first = picker.open(None, target(1));
        let second = picker.open(None, target(2));
        picker.apply(ContextPickerAction::Cancel { request_id: first });
        assert!(picker.is_awaiting(second));
        picker.apply(ContextPickerAction::Cancel { request_id: second });
        assert!(picker.is_active());
        assert!(!picker.is_awaiting(second));
        assert!(!picker.is_awaiting(second));
    }

    #[test]
    fn shared_layout_bounds_rows_buttons_and_pointer_actions() {
        let mut picker = ContextPicker::new();
        let request = picker.open(None, target(7));
        let layout = ContextPickerLayout::calculate(
            900.0,
            300.0,
            1.0,
            10.0,
            20.0,
            1.0,
            picker.phase(),
            picker.sources().len(),
        );
        picker.set_viewport(layout.visible_count);
        let snapshot = picker.snapshot();
        assert!(layout.card.height < 300.0);
        assert!(layout.visible_count < picker.sources().len());
        // Without a working folder the selection starts below the rows that
        // fit, and the list scrolls to show it.
        let first = snapshot.scroll_offset;
        assert!(first > 0);
        assert!((first..first + layout.visible_count).contains(&snapshot.selected_index));
        for button in ContextPickerButton::row(ContextPickerPhase::Sources) {
            let [x, y] = layout.button(*button).unwrap().center();
            assert_eq!(
                layout.target_at(x, y, &snapshot),
                Some(ContextPickerTarget::Button(*button))
            );
        }
        assert_eq!(
            layout.target_at(layout.content.x + 1.0, layout.content.y + 1.0, &snapshot),
            Some(ContextPickerTarget::Source(first))
        );
        assert_eq!(
            layout.target_at(layout.card.x + 1.0, layout.card.y + 1.0, &snapshot),
            None
        );
        assert_eq!(
            layout.source_row_at(
                layout.content.x + 1.0,
                layout.content.y + 1.0,
                picker.snapshot().scroll_offset,
                picker.sources().len(),
            ),
            Some(first)
        );
        picker.apply(ContextPickerAction::Scroll {
            request_id: request,
            delta: 100,
        });
        assert_eq!(
            picker.snapshot().scroll_offset,
            picker.sources().len() - layout.visible_count
        );
    }

    #[test]
    fn choose_files_sits_beside_the_sources_only_and_no_button_overlaps() {
        for phase in [
            ContextPickerPhase::Sources,
            ContextPickerPhase::FilePaths,
            ContextPickerPhase::Gathering,
        ] {
            let layout =
                ContextPickerLayout::calculate(900.0, 600.0, 1.0, 10.0, 20.0, 1.0, phase, 6);
            let buttons: Vec<_> = ContextPickerButton::row(phase)
                .iter()
                .map(|button| layout.button(*button).unwrap())
                .collect();
            assert_eq!(
                layout.choose_files_button.is_some(),
                phase == ContextPickerPhase::Sources
            );
            assert_eq!(buttons[0].x, layout.card.x + layout.padding);
            assert_eq!(
                buttons.last().unwrap().right(),
                layout.card.right() - layout.padding
            );
            for pair in buttons.windows(2) {
                assert!(pair[0].right() < pair[1].x, "{phase:?}: {pair:?}");
            }
        }
    }

    #[test]
    fn focus_cycles_the_list_or_field_and_the_enabled_buttons() {
        use ContextPickerButton as B;
        use ContextPickerFocus as F;
        let directory = tempfile::tempdir().unwrap();
        let mut picker = ContextPicker::new();
        let request = picker.open(Some(directory.path().display().to_string()), target(3));
        assert_eq!(picker.focus(), F::Sources);
        let mut order = Vec::new();
        for _ in 0..4 {
            assert!(picker.cycle_focus(request, false));
            order.push(picker.focus());
        }
        assert_eq!(
            order,
            [
                F::Button(B::ChooseFiles),
                F::Button(B::Cancel),
                F::Button(B::Primary),
                F::Sources
            ]
        );
        assert!(picker.cycle_focus(request, true));
        assert_eq!(
            picker.focus(),
            F::Button(B::Primary),
            "Shift+Tab wraps back"
        );
        assert!(
            !picker.set_focus(request, F::FileInput),
            "no field beside the sources"
        );

        // Choose Files… opens the editor with its field focused; Back
        // returns focus to it.
        assert!(picker.set_focus(request, F::Button(B::ChooseFiles)));
        let choose = picker.enter_action(request);
        assert_eq!(
            choose,
            ContextPickerAction::EditFiles {
                request_id: request
            }
        );
        assert_eq!(picker.space_action(request), choose);
        picker.apply(choose);
        assert_eq!(picker.phase(), ContextPickerPhase::FilePaths);
        assert_eq!(picker.focus(), F::FileInput);
        assert_eq!(
            picker.enter_action(request),
            ContextPickerAction::Primary {
                request_id: request
            },
            "Enter in the field saves"
        );
        assert_eq!(picker.space_action(request), ContextPickerAction::None);
        assert!(picker.cycle_focus(request, false));
        assert_eq!(picker.focus(), F::Button(B::Cancel));
        picker.apply(picker.enter_action(request));
        assert_eq!(picker.phase(), ContextPickerPhase::Sources);
        assert_eq!(picker.focus(), F::Button(B::ChooseFiles));

        // From the list, Enter inserts and Space toggles the selection.
        assert!(picker.set_focus(request, F::Sources));
        assert_eq!(
            picker.enter_action(request),
            ContextPickerAction::Primary {
                request_id: request
            }
        );
        assert_eq!(
            picker.space_action(request),
            ContextPickerAction::Toggle {
                request_id: request,
                source: picker.selected_source().unwrap()
            }
        );
        let newer = picker.open(None, target(4));
        assert!(!picker.cycle_focus(request, false), "an earlier request");
        assert_eq!(picker.enter_action(request), ContextPickerAction::None);
        assert_eq!(
            picker.focus(),
            F::Sources,
            "each opening starts on the list"
        );
        assert!(!picker.is_enabled(B::ChooseFiles));
        assert!(!picker.set_focus(newer, F::Button(B::ChooseFiles)));
        assert_eq!(
            picker.press(newer, ContextPickerTarget::Button(B::ChooseFiles)),
            ContextPickerAction::None,
            "a disabled button answers nothing"
        );
    }

    #[test]
    fn the_selection_starts_on_the_first_source_that_can_be_chosen() {
        let directory = tempfile::tempdir().unwrap();
        let mut picker = ContextPicker::new();
        for cwd in [Some(directory.path().display().to_string()), None] {
            picker.open(cwd, target(8));
            let first_available = picker.sources().iter().find(|entry| entry.available);
            assert_eq!(
                picker.selected_source(),
                first_available.map(|entry| entry.id)
            );
            assert_eq!(picker.focus(), ContextPickerFocus::Sources);
        }
        assert_eq!(
            picker.selected_source(),
            Some(ContextSourceId::TerminalOutput),
            "without a working folder, neither Git nor the file tree is available"
        );

        // With nothing to choose, focus starts on Cancel.
        let unavailable: Vec<_> = picker
            .sources()
            .iter()
            .map(|entry| source(entry.id, &entry.label, false, false))
            .collect();
        assert_eq!(
            initial_selection(&unavailable),
            (0, ContextPickerFocus::Button(ContextPickerButton::Cancel))
        );
    }

    #[test]
    fn files_chosen_in_the_desktops_chooser_are_sent_or_offered_to_fix() {
        let directory = tempfile::tempdir().unwrap();
        // A comma in a name is fine: chosen paths are never split as text.
        let first = directory.path().join("roots, notes.txt");
        let second = directory.path().join("ginger.rs");
        std::fs::write(&first, "a\n").unwrap();
        std::fs::write(&second, "b\n").unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let mut picker = ContextPicker::new();
        let request = picker.open(Some(directory.path().display().to_string()), target(5));
        assert!(picker.can_choose_files(request));
        assert!(!picker.can_choose_files(request + 1));

        picker.use_chosen_files(request, &[first.clone(), second.clone()]);
        assert_eq!(picker.phase(), ContextPickerPhase::Sources);
        let files = picker
            .sources()
            .iter()
            .find(|source| source.id == ContextSourceId::SpecificFiles)
            .cloned()
            .unwrap();
        assert_eq!(files.label, "Specific Files (2)");
        assert!(files.selected);

        // A file outside the folder opens the typed list with the reason.
        picker.use_chosen_files(request, &[outside.path().to_path_buf()]);
        assert_eq!(picker.phase(), ContextPickerPhase::FilePaths);
        assert!(
            picker
                .status
                .as_deref()
                .is_some_and(|status| status.contains("outside"))
        );
    }

    #[test]
    fn specific_files_is_a_checkbox_once_files_are_chosen() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("fixture.rs"), "fn fixture() {}\n").unwrap();
        let mut picker = ContextPicker::new();
        let request = picker.open(Some(directory.path().display().to_string()), target(5));
        let files = |picker: &ContextPicker| {
            picker
                .sources()
                .iter()
                .find(|source| source.id == ContextSourceId::SpecificFiles)
                .cloned()
                .unwrap()
        };
        assert!(!files(&picker).available && !files(&picker).selected);
        let toggle = ContextPickerAction::Toggle {
            request_id: request,
            source: ContextSourceId::SpecificFiles,
        };
        picker.apply(toggle.clone());
        assert_eq!(
            picker.phase(),
            ContextPickerPhase::Sources,
            "the row opens nothing"
        );
        assert!(!files(&picker).selected);

        picker.apply(ContextPickerButton::ChooseFiles.action(request));
        picker.apply(ContextPickerAction::SetFileInput {
            request_id: request,
            value: "fixture.rs".into(),
        });
        picker.apply(ContextPickerAction::Primary {
            request_id: request,
        });
        assert_eq!(picker.phase(), ContextPickerPhase::Sources);
        assert!(files(&picker).available && files(&picker).selected);
        assert_eq!(files(&picker).label, "Specific Files (1)");
        picker.apply(toggle);
        assert!(!files(&picker).selected, "it toggles like any other source");

        picker.apply(ContextPickerButton::ChooseFiles.action(request));
        picker.apply(ContextPickerAction::SetFileInput {
            request_id: request,
            value: String::new(),
        });
        picker.apply(ContextPickerAction::Primary {
            request_id: request,
        });
        assert!(!files(&picker).available && !files(&picker).selected);
        assert_eq!(files(&picker).label, "Specific Files");
    }

    #[test]
    fn a_press_focuses_what_it_lands_on() {
        use ContextPickerFocus as F;
        let mut picker = ContextPicker::new();
        let request = picker.open(None, target(6));
        assert_eq!(
            picker.press(
                request,
                ContextPickerTarget::Button(ContextPickerButton::Cancel)
            ),
            ContextPickerAction::Cancel {
                request_id: request
            }
        );
        assert_eq!(picker.focus(), F::Button(ContextPickerButton::Cancel));
        assert_eq!(
            picker.press(request, ContextPickerTarget::Source(3)),
            ContextPickerAction::Toggle {
                request_id: request,
                source: ContextSourceId::TerminalOutput
            }
        );
        assert_eq!(picker.focus(), F::Sources);
        assert_eq!(
            picker.press(request, ContextPickerTarget::FileInput),
            ContextPickerAction::None
        );
        assert_eq!(picker.focus(), F::Sources, "no field beside the sources");
    }

    #[test]
    fn the_card_fits_small_windows_and_its_status_line_sits_half_a_cell_below_the_buttons() {
        for (window_width, window_height) in [(480.0, 300.0), (1_600.0, 1_000.0)] {
            let layout = ContextPickerLayout::calculate(
                window_width,
                window_height,
                1.0,
                10.0,
                20.0,
                1.0,
                ContextPickerPhase::Sources,
                MAX_CONTEXT_SOURCES,
            );
            assert!(layout.card.x >= 0.0 && layout.card.right() <= window_width);
            assert!(layout.card.y >= 0.0 && layout.card.bottom() <= window_height);
            assert!(layout.content.right() < layout.card.right() - layout.padding);
            assert_eq!(layout.status.y, layout.cancel_button.bottom() + 10.0);
            assert_eq!(layout.status.height, 20.0);
            assert_eq!(layout.status.x, layout.card.x + layout.padding);
            assert_eq!(layout.status.right(), layout.card.right() - layout.padding);
            assert!(layout.status.bottom() <= layout.card.bottom());
        }
    }

    #[test]
    fn the_card_and_everything_placed_from_it_land_on_whole_pixels() {
        // Fractional cells, an odd window, and a mid-slide phase would each
        // put a centred card between pixels.
        for (cell_width, cell_height) in [(10.0, 20.0), (7.3, 15.6), (9.6, 19.5)] {
            for animation in [1.0, 0.875, 0.37, 0.0] {
                for phase in [ContextPickerPhase::Sources, ContextPickerPhase::FilePaths] {
                    let layout = ContextPickerLayout::calculate(
                        901.0,
                        701.0,
                        1.0,
                        cell_width,
                        cell_height,
                        animation,
                        phase,
                        MAX_CONTEXT_SOURCES,
                    );
                    let mut placed = vec![
                        layout.card,
                        layout.content,
                        layout.file_input,
                        layout.cancel_button,
                        layout.primary_button,
                        layout.status,
                    ];
                    placed.extend((0..=layout.visible_count).map(|index| layout.row(index)));
                    for bounds in placed {
                        assert_eq!(
                            [bounds.x, bounds.y, bounds.width, bounds.height].map(f32::fract),
                            [0.0; 4],
                            "{cell_width}x{cell_height} at {animation}: {bounds:?}"
                        );
                    }
                    assert_eq!(
                        [layout.padding, layout.title_height].map(f32::fract),
                        [0.0; 2]
                    );
                    assert_eq!(
                        layout.row(layout.visible_count - 1).bottom(),
                        layout.content.bottom()
                    );
                }
            }
        }
    }

    #[test]
    fn the_file_fields_text_and_caret_stay_inside_it_and_a_press_finds_the_caret() {
        for (cell_width, cell_height) in [(10.0, 20.0), (7.3, 15.6)] {
            let layout = ContextPickerLayout::calculate(
                901.0,
                701.0,
                1.0,
                cell_width,
                cell_height,
                1.0,
                ContextPickerPhase::FilePaths,
                MAX_CONTEXT_SOURCES,
            );
            let field = layout.file_input;
            let long = "src/".repeat(80);
            for (text, cursor) in [("", 0), ("a b.rs", 3), (long.as_str(), long.len())] {
                let view = layout.file_input_viewport(text, cursor).unwrap();
                assert!(view.text_x > field.x && view.right < field.right());
                assert!(view.cursor_x >= view.text_x && view.cursor_x <= view.right);
                assert!(view.text_y >= field.y);
                assert!(view.text_y + cell_height <= field.bottom());
                assert_eq!([view.text_x, view.text_y].map(f32::fract), [0.0; 2]);
                assert_eq!(view.text_range.end, text.len(), "the caret's end shows");
            }
            // Just past the middle of the second character: after it.
            let x = layout.file_input_viewport("a b.rs", 0).unwrap().text_x + cell_width * 1.6;
            assert_eq!(layout.file_caret_at("a b.rs", 0, x), Some(2));
        }
    }

    #[test]
    fn the_card_slides_open_by_a_distance_that_scales_with_the_display() {
        for scale in [1.0, 1.5, 2.0] {
            let at = |animation| {
                ContextPickerLayout::calculate(
                    900.0 * scale,
                    700.0 * scale,
                    scale,
                    10.0 * scale,
                    20.0 * scale,
                    animation,
                    ContextPickerPhase::Sources,
                    MAX_CONTEXT_SOURCES,
                )
            };
            let open = at(1.0);
            assert_eq!(at(0.0).card.y, open.card.y - 20.0 * scale);
            let halfway = at(0.5);
            assert_eq!(halfway.card.y, open.card.y - 10.0 * scale);
            assert_eq!(
                halfway.primary_button.y - halfway.card.y,
                open.primary_button.y - open.card.y,
                "everything slides with the card"
            );
        }
    }

    #[test]
    fn source_rows_answer_exactly_where_they_are_drawn() {
        // 19.5-pixel cells give rows 42.9 pixels tall.
        for phase in [1.0, 0.875, 0.5, 0.125, 0.0] {
            let layout = ContextPickerLayout::calculate(
                900.0,
                300.0,
                1.0,
                10.0,
                19.5,
                phase,
                ContextPickerPhase::Sources,
                MAX_CONTEXT_SOURCES,
            );
            let x = layout.content.x + 1.0;
            let sources = MAX_CONTEXT_SOURCES;
            assert_eq!(
                layout.source_row_at(x, layout.row(0).y - 0.01, 1, sources),
                None
            );
            for visible in 0..layout.visible_count {
                let row = layout.row(visible);
                assert_eq!(row.bottom(), layout.row(visible + 1).y);
                assert_eq!(
                    layout.source_row_at(x, row.y, 1, sources),
                    Some(1 + visible)
                );
                assert_eq!(
                    layout.source_row_at(x, row.bottom() - 0.01, 1, sources),
                    Some(1 + visible)
                );
            }
            let last = layout.row(layout.visible_count - 1);
            assert_eq!(layout.source_row_at(x, last.bottom(), 1, sources), None);
            assert_eq!(layout.source_row_at(last.right(), last.y, 1, sources), None);
            // Rows past the last source answer nothing.
            assert_eq!(
                layout.source_row_at(x, layout.row(1).y, sources - 1, sources),
                None
            );
        }
    }

    #[test]
    fn gathering_completion_rejects_replaced_and_cancelled_requests() {
        let mut picker = ContextPicker::new();
        let first = picker.open(None, target(11));
        picker.apply(ContextPickerAction::Toggle {
            request_id: first,
            source: ContextSourceId::TerminalOutput,
        });
        assert!(picker.begin_gather(first, Some("fixture".into())).is_some());
        let second = picker.open(None, target(12));
        assert_eq!(
            picker.complete(first, Ok("stale".into())),
            ContextPickerCompletion::Ignored
        );
        assert!(picker.is_awaiting(second));
    }

    #[test]
    fn the_arrows_skip_sources_that_cannot_be_chosen() {
        let sources: Vec<_> = [true, false, false, true, false]
            .into_iter()
            .enumerate()
            .map(|(index, available)| {
                source(
                    ContextSourceId::TerminalOutput,
                    &index.to_string(),
                    false,
                    available,
                )
            })
            .collect();
        assert_eq!(moved_selection(&sources, 0, 1), 3);
        assert_eq!(moved_selection(&sources, 3, -1), 0);
        assert_eq!(moved_selection(&sources, 3, 1), 3);
        assert_eq!(moved_selection(&sources, 0, -1), 0);
        // A page past the end lands on the last source that can be chosen.
        assert_eq!(moved_selection(&sources, 0, 10), 3);
        assert_eq!(moved_selection(&sources, 3, -10), 0);
    }
}
