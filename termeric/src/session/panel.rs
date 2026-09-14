//! Workspace and legacy-session management panel
//!
//! A slide-in panel for listing, loading, and deleting saved sessions.

use crate::ui::animation::PanelAnimation;
use crate::ui::text_input::TextInput;

/// Information about a single session for display
#[derive(Debug, Clone)]
pub struct SessionEntry {
    /// Session directory name (used for load/delete operations)
    pub id: String,
    /// Display name shown to the user
    pub display_name: String,
    /// Short metadata string shown below the name (last command, cwd basename, etc.)
    pub subtitle: String,
    /// Last modified time as a human-readable string
    pub last_modified: String,
}

/// Click result from the session panel
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionPanelAction {
    /// Load a session by its id
    Load(String),
    /// Delete a session by its id
    Delete(String),
    /// Start renaming a workspace by its id.
    Rename(String),
    /// Commit the active workspace rename.
    SubmitRename,
    /// Cancel the active workspace rename.
    CancelRename,
    /// Close the panel
    Close,
    /// No action
    None,
}

/// Info passed to the renderer
#[derive(Debug, Clone)]
pub struct SessionPanelInfo {
    /// Animation progress (0.0 = hidden, 1.0 = fully visible)
    pub animation: f32,
    /// List of sessions to display
    pub sessions: Vec<SessionEntry>,
    /// Mouse-hovered session index (None if not hovering any session)
    pub hovered_index: Option<usize>,
    /// Inline rename text, when active.
    pub rename_text: Option<String>,
    /// UTF-8 byte cursor within the rename input.
    pub rename_cursor: usize,
    /// Whether entries are complete workspaces rather than legacy sessions.
    pub workspaces: bool,
    /// Keyboard-selected absolute entry index.
    pub selected_index: usize,
    /// First entry rendered in the bounded viewport.
    pub scroll_offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionPanelRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl SessionPanelRect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionRenameLayout {
    pub input: SessionPanelRect,
    pub save_button: SessionPanelRect,
    pub cancel_button: SessionPanelRect,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionEntryLayout {
    pub row: SessionPanelRect,
    pub load_button: SessionPanelRect,
    pub rename_button: Option<SessionPanelRect>,
    pub delete_button: SessionPanelRect,
}

/// Shared workspace/session geometry for rendering, hit testing, hover, and
/// accessibility bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionPanelLayout {
    pub panel: SessionPanelRect,
    pub padding_x: f32,
    pub padding_y: f32,
    pub title_y: f32,
    pub close_button: SessionPanelRect,
    pub rename: Option<SessionRenameLayout>,
    pub separator_y: f32,
    pub list_start_y: f32,
    pub row_height: f32,
    pub rows_that_fit: usize,
    pub visible_count: usize,
    pub cell_width: f32,
    pub cell_height: f32,
    pub workspaces: bool,
}

impl SessionPanelLayout {
    #[allow(clippy::too_many_arguments)]
    pub fn calculate(
        window_width: f32,
        window_height: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
        renaming: bool,
        workspaces: bool,
        entry_count: usize,
    ) -> Self {
        let panel_width = (cell_width * 35.0).max(320.0).min(window_width * 0.45);
        let panel_x = window_width - panel_width * animation;
        let padding_x = cell_width * 1.5;
        let padding_y = cell_height * 0.5;
        let title_y = cell_height * 1.5;
        let close_button = SessionPanelRect {
            x: panel_x + panel_width - padding_x - cell_width,
            y: title_y,
            width: cell_width * 1.5,
            height: cell_height * 1.5,
        };
        let rename = renaming.then(|| {
            let form_y = title_y + cell_height * 1.15;
            let form_height = cell_height * 1.5;
            let cancel_width = cell_width * 8.0;
            let save_width = cell_width * 6.0;
            let cancel_x = panel_x + panel_width - padding_x - cancel_width;
            let save_x = cancel_x - cell_width * 0.5 - save_width;
            SessionRenameLayout {
                input: SessionPanelRect {
                    x: panel_x + padding_x,
                    y: form_y,
                    width: (save_x - cell_width * 0.5 - (panel_x + padding_x)).max(cell_width),
                    height: form_height,
                },
                save_button: SessionPanelRect {
                    x: save_x,
                    y: form_y,
                    width: save_width,
                    height: form_height,
                },
                cancel_button: SessionPanelRect {
                    x: cancel_x,
                    y: form_y,
                    width: cancel_width,
                    height: form_height,
                },
            }
        });
        let separator_y = rename.map_or(title_y + cell_height * 2.0, |form| {
            form.input.y + form.input.height + padding_y
        });
        let list_start_y = separator_y + padding_y + 4.0;
        let row_height = (cell_height * 3.5).max(52.0);
        let rows_that_fit = ((window_height - list_start_y).max(0.0) / row_height)
            .floor()
            .max(1.0) as usize;
        Self {
            panel: SessionPanelRect {
                x: panel_x,
                y: 0.0,
                width: panel_width,
                height: window_height,
            },
            padding_x,
            padding_y,
            title_y,
            close_button,
            rename,
            separator_y,
            list_start_y,
            row_height,
            rows_that_fit,
            visible_count: entry_count.min(rows_that_fit),
            cell_width,
            cell_height,
            workspaces,
        }
    }

    pub fn entry(self, visible_index: usize) -> SessionEntryLayout {
        let row = SessionPanelRect {
            x: self.panel.x,
            y: self.list_start_y + visible_index as f32 * self.row_height,
            width: self.panel.width,
            height: self.row_height,
        };
        let button_height = self.cell_height * 1.5;
        let button_y = row.y + self.padding_y + self.cell_height * 0.1;
        let delete_width = self.cell_width * 8.0;
        let delete_x = self.panel.x + self.panel.width - self.padding_x - delete_width;
        let delete_button = SessionPanelRect {
            x: delete_x,
            y: button_y,
            width: delete_width,
            height: button_height,
        };
        let (load_anchor_x, rename_button) = if self.workspaces {
            let width = self.cell_width * 8.0;
            let x = delete_x - width - self.cell_width * 0.5;
            (
                x,
                Some(SessionPanelRect {
                    x,
                    y: button_y,
                    width,
                    height: button_height,
                }),
            )
        } else {
            (delete_x, None)
        };
        let load_width = self.cell_width * 6.0;
        let load_button = SessionPanelRect {
            x: load_anchor_x - load_width - self.cell_width * 0.5,
            y: button_y,
            width: load_width,
            height: button_height,
        };
        SessionEntryLayout {
            row,
            load_button,
            rename_button,
            delete_button,
        }
    }

    pub fn visible_row_at(self, y: f32) -> Option<usize> {
        if y < self.list_start_y {
            return None;
        }
        let row = ((y - self.list_start_y) / self.row_height) as usize;
        (row < self.visible_count).then_some(row)
    }
}

/// Session management panel state
pub struct SessionPanel {
    /// Animation state
    animation: PanelAnimation,
    /// Currently highlighted session index
    selected_index: usize,
    /// Cached session entries
    sessions: Vec<SessionEntry>,
    /// Mouse-hovered session index
    hovered_index: Option<usize>,
    rename_target: Option<String>,
    rename_input: TextInput,
    workspaces: bool,
    scroll_offset: usize,
    max_visible: usize,
}

impl SessionPanel {
    pub fn new() -> Self {
        Self {
            animation: PanelAnimation::new(),
            selected_index: 0,
            sessions: Vec::new(),
            hovered_index: None,
            rename_target: None,
            rename_input: TextInput::new(),
            workspaces: true,
            scroll_offset: 0,
            max_visible: 8,
        }
    }

    /// Open the complete workspace manager.
    pub fn open_workspaces(&mut self) {
        self.workspaces = true;
        self.open_current_mode();
    }

    /// Open the legacy focused-terminal session manager.
    pub fn open_sessions(&mut self) {
        self.workspaces = false;
        self.open_current_mode();
    }

    fn open_current_mode(&mut self) {
        self.animation.open();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.cancel_rename();
        self.refresh_sessions();
    }

    /// Close the panel
    pub fn close(&mut self) {
        self.cancel_rename();
        self.animation.close();
    }

    /// Whether the panel is logically open (may still be animating closed)
    pub fn is_open(&self) -> bool {
        self.animation.is_open()
    }

    /// Whether the panel is fully closed and invisible
    pub fn is_closed(&self) -> bool {
        self.animation.is_closed()
    }

    /// Whether the panel needs redrawing (animation in progress)
    pub fn needs_redraw(&self) -> bool {
        self.animation.needs_redraw()
    }

    /// Current animation progress (0.0 to 1.0)
    pub fn animation_progress(&self) -> f32 {
        self.animation.progress()
    }

    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.animation.set_reduced_motion(reduced_motion);
    }

    /// Mark the panel as fully closed once animation completes
    pub fn update(&mut self) {
        self.animation.update();
    }

    /// Refresh the session list from disk
    pub fn refresh_sessions(&mut self) {
        self.sessions = if self.workspaces {
            crate::workspace::list_workspaces()
                .into_iter()
                .map(|workspace| {
                    let subtitle = workspace.warning.unwrap_or_else(|| {
                        format!(
                            "{} tab{} · {} pane{}",
                            workspace.tab_count,
                            if workspace.tab_count == 1 { "" } else { "s" },
                            workspace.pane_count,
                            if workspace.pane_count == 1 { "" } else { "s" },
                        )
                    });
                    SessionEntry {
                        id: workspace.id,
                        display_name: workspace.name,
                        subtitle,
                        last_modified: workspace.updated_at,
                    }
                })
                .collect()
        } else {
            legacy_session_entries()
        };
        self.selected_index = self
            .selected_index
            .min(self.sessions.len().saturating_sub(1));
        self.ensure_selection_visible();
    }

    /// Build info for the renderer
    pub fn build_info(&self) -> SessionPanelInfo {
        SessionPanelInfo {
            animation: self.animation_progress(),
            sessions: self.sessions.clone(),
            hovered_index: self.hovered_index,
            rename_text: self
                .rename_target
                .as_ref()
                .map(|_| self.rename_input.text().to_string()),
            rename_cursor: self.rename_input.cursor(),
            workspaces: self.workspaces,
            selected_index: self.selected_index,
            scroll_offset: self.scroll_offset,
        }
    }

    /// Update hover state from mouse position
    pub fn set_hover(&mut self, index: Option<usize>) {
        self.hovered_index = index;
    }

    /// Get the sessions list
    pub fn sessions(&self) -> &[SessionEntry] {
        &self.sessions
    }

    pub fn is_renaming(&self) -> bool {
        self.rename_target.is_some()
    }

    pub fn is_workspace_mode(&self) -> bool {
        self.workspaces
    }

    pub fn begin_rename(&mut self, id: &str) {
        if !self.workspaces {
            return;
        }
        let Some(entry) = self.sessions.iter().find(|entry| entry.id == id) else {
            return;
        };
        self.rename_target = Some(id.to_string());
        self.rename_input.clear();
        for character in entry.display_name.chars() {
            self.rename_input.insert_char(character);
        }
        if let Some(index) = self.sessions.iter().position(|entry| entry.id == id) {
            self.selected_index = index;
            self.ensure_selection_visible();
        }
    }

    pub fn rename_char(&mut self, character: char) {
        if self.is_renaming()
            && character != '\n'
            && character != '\r'
            && character != '\0'
            && self.rename_input.text().chars().count() < 128
        {
            self.rename_input.insert_char(character);
        }
    }

    pub fn rename_backspace(&mut self) {
        if self.is_renaming() {
            self.rename_input.backspace();
        }
    }

    pub fn cancel_rename(&mut self) {
        self.rename_target = None;
        self.rename_input.clear();
    }

    pub fn take_rename(&mut self) -> Option<(String, String)> {
        let id = self.rename_target.take()?;
        let name = self.rename_input.text().trim().to_string();
        self.rename_input.clear();
        Some((id, name))
    }

    pub fn accessibility_set_rename(&mut self, value: &str) -> bool {
        if !self.is_renaming()
            || value.trim().is_empty()
            || value.chars().count() > 128
            || value.contains(['\n', '\r', '\0'])
        {
            return false;
        }
        self.rename_input.replace(value);
        true
    }

    pub fn set_max_visible(&mut self, rows: usize) {
        self.max_visible = rows.max(1);
        self.ensure_selection_visible();
    }

    pub fn scroll_up(&mut self, rows: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(rows);
    }

    pub fn scroll_down(&mut self, rows: usize) {
        let max = self.sessions.len().saturating_sub(self.max_visible);
        self.scroll_offset = (self.scroll_offset + rows).min(max);
    }

    pub fn select_id(&mut self, id: &str) -> bool {
        let Some(index) = self.sessions.iter().position(|entry| entry.id == id) else {
            return false;
        };
        self.selected_index = index;
        self.ensure_selection_visible();
        true
    }

    pub fn is_visible_id(&self, id: &str) -> bool {
        self.sessions
            .iter()
            .position(|entry| entry.id == id)
            .is_some_and(|index| {
                index >= self.scroll_offset && index < self.scroll_offset + self.max_visible
            })
    }

    pub fn handle_key(&mut self, key: SessionPanelKey) -> SessionPanelAction {
        if self.is_renaming() {
            return match key {
                SessionPanelKey::Enter => SessionPanelAction::SubmitRename,
                SessionPanelKey::Escape => SessionPanelAction::CancelRename,
                _ => SessionPanelAction::None,
            };
        }
        match key {
            SessionPanelKey::Up => {
                self.selected_index = self.selected_index.saturating_sub(1);
                self.ensure_selection_visible();
                SessionPanelAction::None
            }
            SessionPanelKey::Down => {
                if self.selected_index + 1 < self.sessions.len() {
                    self.selected_index += 1;
                    self.ensure_selection_visible();
                }
                SessionPanelAction::None
            }
            SessionPanelKey::Enter => self.selected_action(SessionPanelAction::Load),
            SessionPanelKey::Delete => self.selected_action(SessionPanelAction::Delete),
            SessionPanelKey::Rename if self.workspaces => {
                self.selected_action(SessionPanelAction::Rename)
            }
            SessionPanelKey::Escape => SessionPanelAction::Close,
            SessionPanelKey::Rename => SessionPanelAction::None,
        }
    }

    fn selected_action(
        &self,
        action: impl FnOnce(String) -> SessionPanelAction,
    ) -> SessionPanelAction {
        self.sessions
            .get(self.selected_index)
            .map(|entry| action(entry.id.clone()))
            .unwrap_or(SessionPanelAction::None)
    }

    fn ensure_selection_visible(&mut self) {
        if self.sessions.is_empty() {
            self.selected_index = 0;
            self.scroll_offset = 0;
            return;
        }
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + self.max_visible {
            self.scroll_offset = self.selected_index + 1 - self.max_visible;
        }
        self.scroll_offset = self
            .scroll_offset
            .min(self.sessions.len().saturating_sub(self.max_visible));
    }

    #[cfg(test)]
    pub(crate) fn set_test_entries(&mut self, entries: Vec<SessionEntry>, workspaces: bool) {
        self.sessions = entries;
        self.workspaces = workspaces;
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.animation.open();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPanelKey {
    Up,
    Down,
    Enter,
    Delete,
    Rename,
    Escape,
}

fn legacy_session_entries() -> Vec<SessionEntry> {
    super::list_sessions()
        .into_iter()
        .map(|name| {
            let session = super::session_file_path(&name)
                .and_then(|path| super::snapshot::read_metadata(&path).ok());
            let field = |key: &str| -> Option<String> {
                session
                    .as_ref()
                    .and_then(|session| match key {
                        "title" => Some(session.title.clone()),
                        "last_command" => session.last_command.clone(),
                        "working_directory" => session.working_directory.clone(),
                        _ => None,
                    })
                    .filter(|value| !value.is_empty())
            };
            let display_name = field("title").unwrap_or_else(|| name.clone());
            let detail = field("last_command")
                .or_else(|| {
                    field("working_directory").map(|cwd| {
                        std::path::Path::new(&cwd)
                            .file_name()
                            .map(|part| part.to_string_lossy().to_string())
                            .unwrap_or(cwd)
                    })
                })
                .unwrap_or_default();
            let focused = session.as_ref().is_some_and(|session| {
                session.capture_scope == super::SessionCaptureScope::FocusedPane
            });
            let subtitle = match (focused, detail.is_empty()) {
                (true, true) => "focused pane".into(),
                (true, false) => format!("focused pane · {detail}"),
                (false, _) => detail,
            };
            SessionEntry {
                id: name,
                display_name,
                subtitle,
                last_modified: "legacy snapshot".into(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_metadata_limits_apply_to_restore_title_list_and_panel() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("oversized-fixture");
        std::fs::create_dir(&dir).unwrap();
        let mut file = std::fs::File::create(dir.join("session.toml")).unwrap();
        file.write_all(b"title = \"must not be parsed\"\n").unwrap();
        file.set_len(super::super::file_io::MAX_SESSION_METADATA_BYTES as u64 + 1)
            .unwrap();
        super::super::set_test_sessions_base_dir(Some(root.path().to_owned()));
        let restored = super::super::restore::restore_named_session("oversized-fixture");
        let titles = super::super::list_sessions_with_titles();
        let entries = legacy_session_entries();
        super::super::set_test_sessions_base_dir(None);
        assert!(restored.unwrap_err().to_string().contains("limit"));
        assert_eq!(
            titles,
            [("oversized-fixture".into(), "oversized-fixture".into())]
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].display_name, "oversized-fixture");
        assert!(entries[0].subtitle.is_empty());
    }

    #[test]
    fn workspace_rename_is_inline_and_never_activates_for_legacy_sessions() {
        let mut panel = SessionPanel::new();
        panel.sessions.push(SessionEntry {
            id: "workspace-1".into(),
            display_name: "Roots".into(),
            subtitle: String::new(),
            last_modified: String::new(),
        });
        panel.begin_rename("workspace-1");
        assert!(panel.is_renaming());
        panel.rename_char('!');
        assert_eq!(
            panel.take_rename(),
            Some(("workspace-1".into(), "Roots!".into()))
        );

        panel.workspaces = false;
        panel.begin_rename("workspace-1");
        assert!(!panel.is_renaming());
    }

    #[test]
    fn keyboard_selection_reveals_entries_and_returns_stable_id_actions() {
        let mut panel = SessionPanel::new();
        panel.set_test_entries(
            (0..8)
                .map(|index| SessionEntry {
                    id: format!("workspace-{index}"),
                    display_name: format!("Workspace {index}"),
                    subtitle: String::new(),
                    last_modified: String::new(),
                })
                .collect(),
            true,
        );
        panel.set_max_visible(3);
        for _ in 0..5 {
            assert_eq!(
                panel.handle_key(SessionPanelKey::Down),
                SessionPanelAction::None
            );
        }
        let info = panel.build_info();
        assert_eq!(info.selected_index, 5);
        assert_eq!(info.scroll_offset, 3);
        assert_eq!(
            panel.handle_key(SessionPanelKey::Enter),
            SessionPanelAction::Load("workspace-5".into())
        );
        assert_eq!(
            panel.handle_key(SessionPanelKey::Rename),
            SessionPanelAction::Rename("workspace-5".into())
        );
    }

    #[test]
    fn rename_layout_and_value_limits_are_explicit() {
        let layout = SessionPanelLayout::calculate(1000.0, 800.0, 10.0, 20.0, 1.0, true, true, 4);
        let rename = layout.rename.unwrap();
        assert!(!rename.input.contains(
            rename.save_button.x + rename.save_button.width / 2.0,
            rename.save_button.y + rename.save_button.height / 2.0,
        ));
        assert!(layout.list_start_y > rename.input.y + rename.input.height);

        let mut panel = SessionPanel::new();
        panel.set_test_entries(
            vec![SessionEntry {
                id: "workspace-1".into(),
                display_name: "Roots".into(),
                subtitle: String::new(),
                last_modified: String::new(),
            }],
            true,
        );
        panel.begin_rename("workspace-1");
        assert!(panel.accessibility_set_rename("Roots 🫚"));
        assert_eq!(panel.build_info().rename_text.as_deref(), Some("Roots 🫚"));
        assert!(!panel.accessibility_set_rename("bad\nname"));
        assert!(!panel.accessibility_set_rename(&"x".repeat(129)));
    }
}
