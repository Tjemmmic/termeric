//! The context picker: a request-scoped modal for choosing what context to
//! send an agent. `agents::context` gathers it.

use std::path::PathBuf;

use super::animation::OverlayAnimation;
use super::constants::CONTENT_CHAR_SCALE;
use super::request_scope::RequestScope;
use super::text_input::{TextInput, TextInputEdit, TextLimit};
use crate::agents::AgentTarget;
use crate::agents::context::{
    ContextGatherRequest, MAX_CONTEXT_FILES, MAX_CONTEXT_PATH_CHARS, MAX_CONTEXT_SOURCE_CHARS,
    is_git_repo, validate_specific_files,
};
use crate::ui::geometry::Bounds;

pub const MAX_CONTEXT_SOURCES: usize = 6;
/// Every path, with room for the separators between them.
const MAX_FILE_INPUT_CHARS: usize = MAX_CONTEXT_PATH_CHARS * MAX_CONTEXT_FILES;

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
    AppendFileInput {
        request_id: u64,
        character: char,
    },
    BackspaceFileInput {
        request_id: u64,
    },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextPickerCompletion {
    Ignored,
    Failed,
    Ready { target: AgentTarget, text: String },
}

#[derive(Debug, Clone)]
pub struct ContextPickerSnapshot {
    pub request_id: u64,
    pub sources: Vec<ContextSource>,
    pub selected_index: usize,
    pub scroll_offset: usize,
    pub phase: ContextPickerPhase,
    pub file_input: String,
    pub status: Option<String>,
    pub awaiting: bool,
    pub animation: f32,
}

impl ContextPickerSnapshot {
    pub fn primary_label(&self) -> &'static str {
        match self.phase {
            ContextPickerPhase::Sources => "Gather & Insert",
            ContextPickerPhase::FilePaths => "Save Files",
            ContextPickerPhase::Gathering => "Gathering…",
        }
    }

    pub fn cancel_label(&self) -> &'static str {
        if self.phase == ContextPickerPhase::FilePaths {
            "Back"
        } else {
            "Cancel"
        }
    }
}

/// Geometry shared by rendering, pointer hit testing, viewport clamping, and
/// accessibility semantics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextPickerLayout {
    pub card: Bounds,
    pub content: Bounds,
    pub file_input: Bounds,
    pub cancel_button: Bounds,
    pub primary_button: Bounds,
    pub padding: f32,
    pub title_height: f32,
    pub row_height: f32,
    pub footer_height: f32,
    pub visible_count: usize,
    pub char_scale: f32,
    pub scrollbar_x: f32,
}

impl ContextPickerLayout {
    pub fn calculate(
        window_width: f32,
        window_height: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
        phase: ContextPickerPhase,
        source_count: usize,
    ) -> Self {
        let char_scale = (cell_width * CONTENT_CHAR_SCALE).max(1.0);
        let padding = (cell_width * 2.0).max(12.0);
        let title_height = cell_height * 2.5;
        let row_height = cell_height * 2.2;
        let footer_height = cell_height * 5.2;
        let outer_margin = (cell_width * 2.0).max(12.0);
        let available_width = (window_width - outer_margin * 2.0).max(1.0);
        let card_width = (cell_width * 58.0).max(440.0).min(available_width);

        let fixed_height = padding + title_height + footer_height + padding * 0.5;
        let available_height = (window_height - outer_margin * 2.0).max(row_height);
        let content_budget = (available_height - fixed_height).max(row_height);
        let rows_that_fit = ((content_budget / row_height).floor() as usize).max(1);
        let desired_rows = if phase == ContextPickerPhase::FilePaths {
            3
        } else {
            source_count.max(1)
        };
        let visible_count = desired_rows.min(rows_that_fit);
        let content_height = visible_count as f32 * row_height;
        let card_height = fixed_height + content_height;
        let card_x = (window_width - card_width) / 2.0;
        let card_y = (window_height - card_height) / 2.0 + (1.0 - animation) * -20.0;
        let content_x = card_x + padding;
        let content_y = card_y + padding + title_height;
        let scrollbar_gutter = cell_width.max(6.0);
        let content_width = (card_width - padding * 2.0 - scrollbar_gutter).max(char_scale);

        let button_height = cell_height * 1.8;
        let button_gap = cell_width * 1.5;
        let button_y = content_y + content_height + cell_height * 1.15;
        let available_button_width = (card_width - padding * 2.0 - button_gap).max(2.0);
        let cancel_width = (available_button_width * 0.38).max(1.0);
        let primary_width = (available_button_width - cancel_width).max(1.0);
        let cancel_x = card_x + padding;
        let primary_x = cancel_x + cancel_width + button_gap;

        Self {
            card: Bounds {
                x: card_x,
                y: card_y,
                width: card_width,
                height: card_height,
            },
            content: Bounds {
                x: content_x,
                y: content_y,
                width: content_width,
                height: content_height,
            },
            file_input: Bounds {
                x: content_x,
                y: content_y + row_height * 0.75,
                width: content_width,
                height: row_height,
            },
            cancel_button: Bounds {
                x: cancel_x,
                y: button_y,
                width: cancel_width,
                height: button_height,
            },
            primary_button: Bounds {
                x: primary_x,
                y: button_y,
                width: primary_width,
                height: button_height,
            },
            padding,
            title_height,
            row_height,
            footer_height,
            visible_count,
            char_scale,
            scrollbar_x: card_x + card_width - padding - 4.0,
        }
    }

    pub fn source_row_at(
        &self,
        x: f32,
        y: f32,
        scroll_offset: usize,
        source_count: usize,
    ) -> Option<usize> {
        if !self.content.contains(x, y) {
            return None;
        }
        let visible = ((y - self.content.y) / self.row_height).floor() as usize;
        let source = scroll_offset.saturating_add(visible);
        (source < source_count).then_some(source)
    }

    pub fn hit_test(
        &self,
        x: f32,
        y: f32,
        snapshot: &ContextPickerSnapshot,
    ) -> ContextPickerAction {
        let request_id = snapshot.request_id;
        if self.cancel_button.contains(x, y) {
            return ContextPickerAction::Cancel { request_id };
        }
        if self.primary_button.contains(x, y) {
            return ContextPickerAction::Primary { request_id };
        }
        if snapshot.phase == ContextPickerPhase::Sources
            && let Some(index) =
                self.source_row_at(x, y, snapshot.scroll_offset, snapshot.sources.len())
        {
            return ContextPickerAction::Toggle {
                request_id,
                source: snapshot.sources[index].id,
            };
        }
        ContextPickerAction::None
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
            source(
                ContextSourceId::SpecificFiles,
                "Specific Files…",
                false,
                self.cwd.is_some(),
            ),
        ];
        debug_assert_eq!(self.sources.len(), MAX_CONTEXT_SOURCES);
        self.selected_index = 0;
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
            status: self.status.clone(),
            awaiting: self.is_awaiting_current(),
            animation: self.animation(),
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
            | ContextPickerAction::SetFileInput { request_id, .. }
            | ContextPickerAction::AppendFileInput { request_id, .. }
            | ContextPickerAction::BackspaceFileInput { request_id } => *request_id,
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
                self.selected_index = self
                    .selected_index
                    .saturating_add_signed(delta)
                    .min(self.sources.len().saturating_sub(1));
                self.reveal_selection();
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
                    if source == ContextSourceId::SpecificFiles {
                        self.open_file_editor();
                    } else if self.sources[index].available {
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
            ContextPickerAction::AppendFileInput { character, .. }
                if self.phase == ContextPickerPhase::FilePaths =>
            {
                if self.file_input.insert(character.encode_utf8(&mut [0; 4])) {
                    self.status = None;
                }
            }
            ContextPickerAction::BackspaceFileInput { .. }
                if self.phase == ContextPickerPhase::FilePaths =>
            {
                self.file_input.edit_grapheme(TextInputEdit::Backspace);
                self.status = None;
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
        self.status = Some("Gathering bounded context…".into());
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
                self.status = Some(crate::safe_text::truncate_with_ellipsis(&error, 512));
                ContextPickerCompletion::Failed
            }
        }
    }

    fn open_file_editor(&mut self) {
        let available = self
            .sources
            .iter()
            .find(|source| source.id == ContextSourceId::SpecificFiles)
            .is_some_and(|source| source.available);
        if !available {
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
    }

    fn commit_file_editor(&mut self) {
        match validate_specific_files(self.cwd.as_deref(), self.file_input.text()) {
            Ok(paths) => {
                self.specific_files = paths;
                if let Some(source) = self
                    .sources
                    .iter_mut()
                    .find(|source| source.id == ContextSourceId::SpecificFiles)
                {
                    source.selected = !self.specific_files.is_empty();
                    source.label = format!("Specific Files… ({})", self.specific_files.len());
                }
                self.phase = ContextPickerPhase::Sources;
                self.status = None;
            }
            Err(error) => self.status = Some(error),
        }
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
        assert_eq!(
            layout.hit_test(
                layout.primary_button.x + 1.0,
                layout.primary_button.y + 1.0,
                &snapshot,
            ),
            ContextPickerAction::Primary {
                request_id: request
            }
        );
        assert_eq!(
            layout.source_row_at(
                layout.content.x + 1.0,
                layout.content.y + 1.0,
                picker.snapshot().scroll_offset,
                picker.sources().len(),
            ),
            Some(0)
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
}
