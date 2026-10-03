//! Request-scoped context assembly for developer-tool terminal sessions.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::animation::OverlayAnimation;
use super::constants::CONTENT_CHAR_SCALE;
use super::text_input::{TextInput, TextInputEdit, TextLimit};
use crate::tabs::TerminalIdentity;
use crate::ui::geometry::Bounds;

pub const MAX_CONTEXT_SOURCES: usize = 6;
pub const MAX_CONTEXT_FILES: usize = 16;
/// Every path, with room for the separators between them.
const MAX_FILE_INPUT_CHARS: usize = MAX_CONTEXT_PATH_CHARS * MAX_CONTEXT_FILES;
pub const MAX_CONTEXT_PATH_CHARS: usize = 1024;
pub const MAX_CONTEXT_SOURCE_CHARS: usize = 10_000;
pub const MAX_ASSEMBLED_CONTEXT_CHARS: usize = 64_000;
const CONTEXT_COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextTarget {
    pub agent_id: String,
    pub agent_name: String,
    pub terminal: TerminalIdentity,
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
pub struct ContextGatherRequest {
    pub request_id: u64,
    pub cwd: Option<PathBuf>,
    pub selected: Vec<ContextSourceId>,
    pub specific_files: Vec<PathBuf>,
    pub terminal_output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextPickerCompletion {
    Ignored,
    Failed,
    Ready { target: ContextTarget, text: String },
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
    target: Option<ContextTarget>,
    phase: ContextPickerPhase,
    file_input: TextInput,
    specific_files: Vec<PathBuf>,
    status: Option<String>,
    request_id: u64,
    accepting: bool,
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
            request_id: 0,
            accepting: false,
        }
    }

    /// The agent terminal the open picker gathers context for.
    pub fn target(&self) -> Option<&ContextTarget> {
        self.target.as_ref()
    }

    pub fn open(&mut self, cwd: Option<String>, target: ContextTarget) -> u64 {
        self.request_id = self.request_id.wrapping_add(1).max(1);
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
        self.accepting = true;
        self.animation.open();
        self.request_id
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
            request_id: self.request_id,
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
                    self.accepting = false;
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
                self.accepting = false;
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

fn is_git_repo(cwd: Option<&Path>) -> bool {
    cwd.and_then(|path| path.canonicalize().ok())
        .is_some_and(|path| {
            path.ancestors()
                .any(|ancestor| ancestor.join(".git").exists())
        })
}

fn validate_specific_files(cwd: Option<&Path>, value: &str) -> Result<Vec<PathBuf>, String> {
    let root = cwd
        .ok_or_else(|| "Specific files require a working directory".to_string())?
        .canonicalize()
        .map_err(|error| format!("Could not resolve the working directory: {error}"))?;
    let entries: Vec<_> = value
        .split([',', '\n'])
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .collect();
    if entries.len() > MAX_CONTEXT_FILES {
        return Err(format!("Choose at most {MAX_CONTEXT_FILES} files"));
    }
    let mut paths = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.chars().count() > MAX_CONTEXT_PATH_CHARS {
            return Err(format!(
                "A file path exceeds {MAX_CONTEXT_PATH_CHARS} characters"
            ));
        }
        let candidate = Path::new(entry);
        let candidate = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            root.join(candidate)
        };
        let canonical = candidate
            .canonicalize()
            .map_err(|error| format!("Could not open {entry}: {error}"))?;
        if !canonical.starts_with(&root) {
            return Err(format!("{entry} is outside the context working directory"));
        }
        if !canonical.is_file() {
            return Err(format!("{entry} is not a regular file"));
        }
        if !paths.contains(&canonical) {
            paths.push(canonical);
        }
    }
    Ok(paths)
}

pub fn gather_context(request: ContextGatherRequest) -> Result<String, String> {
    let mut parts = Vec::new();
    for source in request.selected {
        let part = match source {
            ContextSourceId::GitDiffUnstaged => bounded_git_diff(request.cwd.as_deref(), false)?
                .filter(|text| !text.trim().is_empty())
                .map(|text| format!("=== Git Diff (Unstaged) ===\n{text}")),
            ContextSourceId::GitDiffStaged => bounded_git_diff(request.cwd.as_deref(), true)?
                .filter(|text| !text.trim().is_empty())
                .map(|text| format!("=== Git Diff (Staged) ===\n{text}")),
            ContextSourceId::FileTree => bounded_file_tree(request.cwd.as_deref())?
                .filter(|text| !text.trim().is_empty())
                .map(|text| format!("=== File Tree ===\n{text}")),
            ContextSourceId::TerminalOutput => request
                .terminal_output
                .as_deref()
                .filter(|text| !text.trim().is_empty())
                .map(|text| {
                    format!(
                        "=== Recent Terminal Output ===\n{}",
                        crate::safe_text::truncate_with_ellipsis(text, MAX_CONTEXT_SOURCE_CHARS)
                    )
                }),
            ContextSourceId::TestOutput => request
                .terminal_output
                .as_deref()
                .and_then(extract_test_output)
                .map(|text| {
                    format!(
                        "=== Test Output ===\n{}",
                        crate::safe_text::truncate_with_ellipsis(&text, MAX_CONTEXT_SOURCE_CHARS)
                    )
                }),
            ContextSourceId::SpecificFiles => {
                let files = gather_specific_files(request.cwd.as_deref(), &request.specific_files)?;
                (!files.is_empty()).then(|| files.join("\n\n"))
            }
        };
        if let Some(part) = part {
            parts.push(part);
        }
    }
    if parts.is_empty() {
        return Err("Selected sources produced no context".into());
    }
    Ok(crate::safe_text::truncate_with_ellipsis(
        &parts.join("\n\n"),
        MAX_ASSEMBLED_CONTEXT_CHARS,
    ))
}

fn gather_specific_files(root: Option<&Path>, paths: &[PathBuf]) -> Result<Vec<String>, String> {
    let root = root
        .ok_or_else(|| "Specific files require a working directory".to_string())?
        .canonicalize()
        .map_err(|error| format!("Could not resolve the working directory: {error}"))?;
    let mut files = Vec::with_capacity(paths.len().min(MAX_CONTEXT_FILES));
    for path in paths.iter().take(MAX_CONTEXT_FILES) {
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("Could not resolve {}: {error}", path.display()))?;
        if !canonical.starts_with(&root) || !canonical.is_file() {
            return Err(format!(
                "{} is no longer a regular file inside the context working directory",
                path.display()
            ));
        }
        let content = fs::read_to_string(&canonical)
            .map_err(|error| format!("Could not read {}: {error}", canonical.display()))?;
        files.push(format!(
            "=== File: {} ===\n{}",
            canonical.display(),
            crate::safe_text::truncate_with_ellipsis(&content, MAX_CONTEXT_SOURCE_CHARS)
        ));
    }
    Ok(files)
}

fn bounded_git_diff(cwd: Option<&Path>, staged: bool) -> Result<Option<String>, String> {
    let mut args = vec!["diff"];
    if staged {
        args.push("--cached");
    }
    run_bounded_command("git", &args, cwd, MAX_CONTEXT_SOURCE_CHARS * 4)
}

fn bounded_file_tree(cwd: Option<&Path>) -> Result<Option<String>, String> {
    if let Some(output) =
        run_bounded_command("git", &["ls-files"], cwd, MAX_CONTEXT_SOURCE_CHARS * 4)?
    {
        if !output.trim().is_empty() {
            return Ok(Some(crate::safe_text::truncate_with_ellipsis(
                &output,
                MAX_CONTEXT_SOURCE_CHARS,
            )));
        }
    }
    let Some(root) = cwd else {
        return Ok(None);
    };
    let mut files = Vec::new();
    collect_files(root, root, 0, &mut files)?;
    Ok((!files.is_empty())
        .then(|| crate::safe_text::truncate_with_ellipsis(&files.join("\n"), 5_000)))
}

fn collect_files(
    root: &Path,
    directory: &Path,
    depth: usize,
    output: &mut Vec<String>,
) -> Result<(), String> {
    if depth > 32 || output.len() >= 5_000 {
        return Ok(());
    }
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("Could not list {}: {error}", directory.display()))?;
    let mut entries = entries
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Could not read directory entry: {error}"))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if output.len() >= 5_000 {
            break;
        }
        let path = entry.path();
        if path.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            collect_files(root, &path, depth + 1, output)?;
        } else if kind.is_file() {
            output.push(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn run_bounded_command(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    max_bytes: usize,
) -> Result<Option<String>, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not start {program}: {error}")),
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("Could not capture {program} output"))?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(max_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + CONTEXT_COMMAND_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(format!("{program} context command timed out"));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(format!("Could not wait for {program}: {error}"));
            }
        }
    };
    let mut bytes = reader
        .join()
        .map_err(|_| format!("{program} output reader panicked"))?
        .map_err(|error| format!("Could not read {program} output: {error}"))?;
    let truncated = bytes.len() > max_bytes;
    bytes.truncate(max_bytes);
    if !status.success() && !truncated {
        return Ok(None);
    }
    let mut output = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        output.push_str("\n… output truncated");
    }
    Ok(Some(output))
}

pub fn extract_test_output(output: &str) -> Option<String> {
    let patterns = [
        "test result:",
        "PASS",
        "FAIL",
        "panicked at",
        "error[E",
        "PASSED",
        "FAILED",
        "tests passed",
        "tests failed",
    ];
    let lines: Vec<_> = output.lines().collect();
    let mut selected = Vec::new();
    let mut last_match = None;
    for (index, line) in lines.iter().enumerate() {
        if patterns.iter().any(|pattern| line.contains(pattern)) {
            let start = index.saturating_sub(5);
            if last_match.is_none() {
                selected.extend_from_slice(&lines[start..index]);
            }
            selected.push(*line);
            last_match = Some(index);
        } else if last_match.is_some_and(|last| index.saturating_sub(last) < 3) {
            selected.push(*line);
        } else {
            last_match = None;
        }
    }
    (!selected.is_empty()).then(|| selected.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(tab_id: usize) -> ContextTarget {
        ContextTarget {
            agent_id: "fixture-agent".into(),
            agent_name: "Fixture Agent".into(),
            terminal: TerminalIdentity {
                tab_id,
                pane_id: crate::panes::PaneId(0),
            },
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

    #[test]
    fn specific_files_are_workspace_bounded_and_gathered_with_total_caps() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("fixture.rs");
        fs::write(&file, "x".repeat(MAX_CONTEXT_SOURCE_CHARS + 100)).unwrap();
        let paths = validate_specific_files(Some(directory.path()), "fixture.rs").unwrap();
        assert_eq!(paths, [file.canonicalize().unwrap()]);
        assert!(validate_specific_files(Some(directory.path()), "/etc/passwd").is_err());

        let text = gather_context(ContextGatherRequest {
            request_id: 1,
            cwd: Some(directory.path().to_path_buf()),
            selected: vec![ContextSourceId::SpecificFiles],
            specific_files: paths,
            terminal_output: None,
        })
        .unwrap();
        assert!(text.contains("=== File:"));
        assert!(text.chars().count() <= MAX_ASSEMBLED_CONTEXT_CHARS);
    }

    #[cfg(unix)]
    #[test]
    fn specific_file_is_revalidated_after_selection_to_block_symlink_retargeting() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("fixture.rs");
        fs::write(&file, "safe fixture").unwrap();
        let paths = validate_specific_files(Some(directory.path()), "fixture.rs").unwrap();
        fs::remove_file(&file).unwrap();
        symlink("/etc/passwd", &file).unwrap();

        let result = gather_context(ContextGatherRequest {
            request_id: 2,
            cwd: Some(directory.path().to_path_buf()),
            selected: vec![ContextSourceId::SpecificFiles],
            specific_files: paths,
            terminal_output: None,
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_output_extraction_is_bounded_to_relevant_blocks() {
        let output = "some random output\ncompiling...\ntest foo ... ok\ntest bar ... FAIL\ntest result: 1 passed; 1 failed\nmore stuff";
        let extracted = extract_test_output(output).unwrap();
        assert!(extracted.contains("FAIL"));
        assert!(extracted.contains("test result:"));
        assert!(extract_test_output("hello world").is_none());
    }
}
