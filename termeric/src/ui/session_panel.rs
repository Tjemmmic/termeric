//! Workspace and closed-tab snapshot management panel
//!
//! A slide-in panel for listing, loading, and deleting saved sessions.

use crate::session::listing::SessionListing;
use crate::ui::animation::PanelAnimation;
use crate::ui::geometry::Bounds;
use crate::ui::text_input::{TextInput, TextInputEdit, TextLimit};

const MAX_RENAME_CHARACTERS: usize = 128;

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
    /// Place the active rename caret at a whole-grapheme byte boundary.
    PlaceRenameCaret { cursor: usize },
    /// Select an entry without opening it.
    Select(String),
    /// Show tab snapshots instead of workspaces.
    ShowSnapshots,
    /// Return from tab snapshots to workspaces.
    ShowWorkspaces,
    /// Close the panel
    Close,
    /// No action
    None,
}

/// Header and footer controls that show pointer hover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPanelControl {
    Close,
    Back,
    SnapshotsLink,
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
    /// Header or footer control under the pointer.
    pub hovered_control: Option<SessionPanelControl>,
    /// Inline rename text, when active.
    pub rename_text: Option<String>,
    /// UTF-8 byte cursor within the rename input.
    pub rename_cursor: usize,
    /// Whether entries are workspaces rather than closed-tab snapshots.
    pub workspaces: bool,
    /// Keyboard-selected absolute entry index.
    pub selected_index: usize,
    /// First entry rendered in the bounded viewport.
    pub scroll_offset: usize,
    /// Saved tab snapshots, offered from the workspace list when non-zero.
    pub snapshot_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionRenameLayout {
    pub input: Bounds,
    pub save_button: Bounds,
    pub cancel_button: Bounds,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionEntryLayout {
    pub row: Bounds,
    pub load_button: Bounds,
    pub rename_button: Option<Bounds>,
    pub delete_button: Bounds,
}

/// Shared workspace/session geometry for rendering, hit testing, hover, and
/// accessibility bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionPanelLayout {
    window_width: f32,
    pub panel: Bounds,
    pub padding_x: f32,
    pub padding_y: f32,
    pub title_y: f32,
    /// Line of explanatory text under the title.
    pub note_y: f32,
    pub close_button: Bounds,
    /// Returns from tab snapshots to workspaces.
    pub back_button: Option<Bounds>,
    /// Opens tab snapshots from the workspace list. Reserved in workspace
    /// mode and drawn only when snapshots exist.
    pub snapshots_link: Option<Bounds>,
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
    /// Hit the painted rename field, clamping padding to visible text edges.
    /// A collapsed viewport consumes the field click without moving a caret.
    pub fn rename_input_click(
        &self,
        text: &str,
        cursor: usize,
        x: f32,
        y: f32,
    ) -> Option<SessionPanelAction> {
        let rename = self.rename?;
        if !self.panel.contains(x, y)
            || x > self.window_width
            || !rename.input.contains(x, y)
            || rename.save_button.contains(x, y)
            || rename.cancel_button.contains(x, y)
        {
            return None;
        }
        Some(
            self.rename_input_viewport(text, cursor)
                .and_then(|view| view.cursor_at(text, x, self.cell_width))
                .map_or(SessionPanelAction::None, |cursor| {
                    SessionPanelAction::PlaceRenameCaret { cursor }
                }),
        )
    }

    pub fn rename_input_viewport(
        &self,
        text: &str,
        cursor: usize,
    ) -> Option<crate::ui::text_input_viewport::TextInputViewport> {
        let rename = self.rename?;
        let input = rename.input;
        if input.y < 0.0 || input.y + input.height > self.panel.height {
            return None;
        }
        // Narrow layouts can put the minimum-sized input beneath Save.
        // Do not paint query glyphs or native candidates over that control.
        let right = (input.x + input.width)
            .min(rename.save_button.x - self.cell_width * 0.5)
            .min(self.window_width);
        crate::ui::text_input_viewport::TextInputViewport::calculate(
            text,
            cursor,
            input.x.max(0.0) + self.cell_width * 0.5,
            input.y + (input.height - self.cell_height) / 2.0,
            right - self.cell_width * 0.5,
            self.cell_width,
        )
    }

    /// Geometry of the panel, which slides in from the right edge and spans
    /// from `top` (below the toolbar) to the bottom of the window.
    pub fn calculate(
        window_width: f32,
        window_height: f32,
        top: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
        renaming: bool,
        workspaces: bool,
        entry_count: usize,
    ) -> Self {
        // Wide enough that a hovered row's actions leave room for its name.
        let panel_width = (cell_width * 44.0).max(380.0).min(window_width * 0.45);
        let panel_x = window_width - panel_width * animation;
        let padding_x = cell_width * 1.5;
        let padding_y = cell_height * 0.5;
        let title_y = top + cell_height * 0.9;
        let control = cell_height * 1.5;
        let close_button = Bounds {
            x: panel_x + panel_width - padding_x * 0.5 - control,
            y: title_y,
            width: control,
            height: control,
        };
        let back_button = (!workspaces).then_some(Bounds {
            x: panel_x + padding_x * 0.5,
            y: title_y,
            width: control,
            height: control,
        });
        let note_y = title_y + control;
        let rename = renaming.then(|| {
            let form_y = note_y + cell_height * 1.4;
            let form_height = cell_height * 1.5;
            let cancel_width = cell_width * 8.0;
            let save_width = cell_width * 6.0;
            let cancel_x = panel_x + panel_width - padding_x - cancel_width;
            let save_x = cancel_x - cell_width * 0.5 - save_width;
            SessionRenameLayout {
                input: Bounds {
                    x: panel_x + padding_x,
                    y: form_y,
                    width: (save_x - cell_width * 0.5 - (panel_x + padding_x)).max(cell_width),
                    height: form_height,
                },
                save_button: Bounds {
                    x: save_x,
                    y: form_y,
                    width: save_width,
                    height: form_height,
                },
                cancel_button: Bounds {
                    x: cancel_x,
                    y: form_y,
                    width: cancel_width,
                    height: form_height,
                },
            }
        });
        let separator_y = rename.map_or(note_y + cell_height * 1.7, |form| {
            form.input.y + form.input.height + padding_y
        });
        let list_start_y = separator_y + padding_y;
        let row_height = (cell_height * 3.5).max(52.0);
        let link_height = cell_height * 2.2;
        let snapshots_link = workspaces.then_some(Bounds {
            x: panel_x,
            y: window_height - link_height,
            width: panel_width,
            height: link_height,
        });
        let list_bottom = snapshots_link.map_or(window_height, |link| link.y);
        let rows_that_fit = ((list_bottom - list_start_y).max(0.0) / row_height)
            .floor()
            .max(1.0) as usize;
        Self {
            window_width,
            panel: Bounds {
                x: panel_x,
                y: top,
                width: panel_width,
                height: (window_height - top).max(0.0),
            },
            padding_x,
            padding_y,
            title_y,
            note_y,
            close_button,
            back_button,
            snapshots_link,
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
        let row = Bounds {
            x: self.panel.x,
            y: self.list_start_y + visible_index as f32 * self.row_height,
            width: self.panel.width,
            height: self.row_height,
        };
        let button_height = self.cell_height * 1.5;
        let button_y = row.y + self.padding_y + self.cell_height * 0.1;
        let delete_width = self.cell_width * 8.0;
        let delete_x = self.panel.x + self.panel.width - self.padding_x - delete_width;
        let delete_button = Bounds {
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
                Some(Bounds {
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
        let load_button = Bounds {
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

    /// The header or footer control at a point. The snapshots link only
    /// exists while there are snapshots to open.
    pub fn control_at(self, x: f32, y: f32, snapshot_count: usize) -> Option<SessionPanelControl> {
        if self.close_button.contains(x, y) {
            Some(SessionPanelControl::Close)
        } else if self.back_button.is_some_and(|back| back.contains(x, y)) {
            Some(SessionPanelControl::Back)
        } else if snapshot_count > 0 && self.snapshots_link.is_some_and(|link| link.contains(x, y))
        {
            Some(SessionPanelControl::SnapshotsLink)
        } else {
            None
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
    hovered_control: Option<SessionPanelControl>,
    rename_target: Option<String>,
    rename_input: TextInput,
    workspaces: bool,
    scroll_offset: usize,
    max_visible: usize,
    snapshot_count: usize,
}

impl SessionPanel {
    pub fn new() -> Self {
        Self {
            animation: PanelAnimation::new(),
            selected_index: 0,
            sessions: Vec::new(),
            hovered_index: None,
            hovered_control: None,
            rename_target: None,
            rename_input: TextInput::new(TextLimit::Chars(MAX_RENAME_CHARACTERS)),
            workspaces: true,
            scroll_offset: 0,
            max_visible: 8,
            snapshot_count: 0,
        }
    }

    /// Open on `listing`: the workspace manager or the tab snapshot list.
    pub fn open(&mut self, listing: SessionListing) {
        self.animation.open();
        self.selected_index = 0;
        self.scroll_offset = 0;
        // Hover indexes refer to the previous list; the next pointer motion
        // re-establishes them.
        self.hovered_index = None;
        self.hovered_control = None;
        self.cancel_rename();
        self.refresh(listing);
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

    /// Show `listing`, keeping the selection where it still fits.
    pub fn refresh(&mut self, listing: SessionListing) {
        self.workspaces = listing.workspaces;
        self.snapshot_count = listing.snapshot_count;
        self.sessions = listing.entries;
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
            hovered_control: self.hovered_control,
            rename_text: self
                .rename_target
                .as_ref()
                .map(|_| self.rename_input.text().to_string()),
            rename_cursor: self.rename_input.cursor(),
            workspaces: self.workspaces,
            selected_index: self.selected_index,
            scroll_offset: self.scroll_offset,
            snapshot_count: self.snapshot_count,
        }
    }

    /// Borrow just the rename state; positioning IME must not clone inventory.
    pub fn rename_input_viewport(
        &self,
        width: f32,
        height: f32,
        top: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> Option<crate::ui::text_input_viewport::TextInputViewport> {
        self.renaming_layout(width, height, top, cell_width, cell_height)?
            .rename_input_viewport(self.rename_input.text(), self.rename_input.cursor())
    }

    /// Where a press at `(x, y)` lands in the rename field, as a caret
    /// position: `None` off the field, or while nothing is being renamed.
    pub fn rename_caret_at(
        &self,
        (x, y): (f32, f32),
        width: f32,
        height: f32,
        top: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> Option<usize> {
        let layout = self.renaming_layout(width, height, top, cell_width, cell_height)?;
        match layout.rename_input_click(self.rename_input.text(), self.rename_input.cursor(), x, y)
        {
            Some(SessionPanelAction::PlaceRenameCaret { cursor }) => Some(cursor),
            _ => None,
        }
    }

    /// The open panel's geometry while it renames a workspace.
    fn renaming_layout(
        &self,
        width: f32,
        height: f32,
        top: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> Option<SessionPanelLayout> {
        if !self.is_open() || !self.is_renaming() {
            return None;
        }
        Some(SessionPanelLayout::calculate(
            width,
            height,
            top,
            cell_width,
            cell_height,
            self.animation_progress(),
            true,
            self.workspaces,
            self.sessions.len(),
        ))
    }

    /// Update hover state from mouse position
    pub fn set_hover(&mut self, index: Option<usize>, control: Option<SessionPanelControl>) {
        self.hovered_index = index;
        self.hovered_control = control;
    }

    /// Get the sessions list
    #[cfg(test)]
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
        // A stored name the field would refuse starts the draft empty.
        self.rename_input.clear();
        self.rename_input.replace(&entry.display_name);
        if let Some(index) = self.sessions.iter().position(|entry| entry.id == id) {
            self.selected_index = index;
            self.ensure_selection_visible();
        }
    }

    /// Commit a whole single-line edit, never a truncated prefix.
    pub fn insert_rename_text(&mut self, text: &str) -> bool {
        self.is_renaming() && self.rename_input.insert(text)
    }

    pub fn rename_edit(&mut self, edit: TextInputEdit) {
        if self.is_renaming() {
            self.rename_input.edit_grapheme(edit);
        }
    }

    /// Revalidate pointer placement against the currently open rename draft.
    pub fn place_rename_caret(&mut self, cursor: usize) -> bool {
        if !self.is_open() || !self.is_renaming() {
            return false;
        }
        self.rename_input.place_caret(cursor)
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
        self.is_renaming() && self.rename_input.replace(value)
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
            SessionPanelKey::Escape if !self.workspaces => SessionPanelAction::ShowWorkspaces,
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
        self.scroll_offset = crate::ui::scroll::reveal_row(
            self.scroll_offset,
            self.selected_index,
            self.max_visible,
        )
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

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_segmentation::UnicodeSegmentation;

    #[test]
    fn workspace_rename_is_inline_and_never_activates_for_tab_snapshots() {
        let mut panel = SessionPanel::new();
        panel.sessions.push(SessionEntry {
            id: "workspace-1".into(),
            display_name: "Roots".into(),
            subtitle: String::new(),
            last_modified: String::new(),
        });
        panel.begin_rename("workspace-1");
        assert!(panel.is_renaming());
        assert!(panel.insert_rename_text("!"));
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
    fn rename_text_is_atomic_single_line_and_backspace_removes_whole_graphemes() {
        let mut panel = SessionPanel::new();
        panel.set_test_entries(
            vec![SessionEntry {
                id: "workspace-test".into(),
                display_name: "Roots".into(),
                subtitle: String::new(),
                last_modified: String::new(),
            }],
            true,
        );
        panel.begin_rename("workspace-test");
        assert!(panel.accessibility_set_rename(""));
        assert!(crate::workspace::validate_workspace_name("").is_err());
        assert!(panel.insert_rename_text("e\u{301}👩\u{200d}💻🇺🇸"));
        for expected in ["e\u{301}👩\u{200d}💻", "e\u{301}", ""] {
            panel.rename_edit(TextInputEdit::Backspace);
            assert_eq!(panel.build_info().rename_text.as_deref(), Some(expected));
        }
        assert!(panel.accessibility_set_rename(&"🫚".repeat(127)));
        for text in ["xy", "a\nb", "\t", "\x1b", "\u{85}", "\u{2028}", "\u{2029}"] {
            let before = panel.build_info();
            assert!(!panel.insert_rename_text(text));
            assert_eq!(panel.build_info().rename_text, before.rename_text);
            assert_eq!(panel.build_info().rename_cursor, before.rename_cursor);
        }
        assert!(panel.insert_rename_text("界"));
        assert!(!panel.insert_rename_text("x"));
        let before = panel.build_info().rename_text;
        for text in ["\t", "\x1b", "\u{2028}", "\u{2029}", &"x".repeat(129)] {
            assert!(!panel.accessibility_set_rename(text));
            assert_eq!(panel.build_info().rename_text, before);
        }
        panel.cancel_rename();
        assert!(!panel.insert_rename_text("hidden"));
        assert!(!panel.accessibility_set_rename("hidden"));
    }

    #[test]
    fn rename_viewport_keeps_graphemes_and_caret_inside_visible_field() {
        use unicode_segmentation::UnicodeSegmentation;
        let text = "a界e\u{301}👩\u{200d}💻🇺🇸".repeat(8);
        for width in [400.0, 800.0, 1600.0] {
            for cell in [8.0, 12.0, 20.0] {
                for animation in [0.2, 0.8, 1.0] {
                    let layout = SessionPanelLayout::calculate(
                        width,
                        600.0,
                        0.0,
                        cell,
                        cell * 2.0,
                        animation,
                        true,
                        true,
                        0,
                    );
                    let field = layout.rename.unwrap();
                    for cursor in [0, "a界e\u{301}".len(), text.len()] {
                        let Some(view) = layout.rename_input_viewport(&text, cursor) else {
                            continue;
                        };
                        assert!(view.text_range.start <= cursor && cursor <= view.text_range.end);
                        assert!(text.is_char_boundary(view.text_range.start));
                        assert!(
                            view.text_range.end == text.len()
                                || text
                                    .grapheme_indices(true)
                                    .any(|(i, _)| i == view.text_range.end)
                        );
                        let columns: usize = text[view.text_range.start..cursor]
                            .graphemes(true)
                            .map(termeric_core::unicode::grapheme_width)
                            .sum();
                        assert!(
                            (view.cursor_x - (view.text_x + columns as f32 * cell)).abs() < 0.01
                        );
                        assert!(view.cursor_x + cell <= view.right + 0.01);
                        assert!(view.right <= width && view.right < field.save_button.x);
                        assert!(view.text_x >= field.input.x);
                        assert!(view.text_y >= field.input.y);
                        if cursor == text.len() {
                            assert!(view.text_range.start > 0);
                        }
                    }
                }
            }
        }
        for (width, height, animation, renaming) in [
            (40.0, 600.0, 1.0, true),
            (800.0, 20.0, 1.0, true),
            (800.0, 600.0, 0.0, true),
            (800.0, 600.0, 1.0, false),
        ] {
            let layout = SessionPanelLayout::calculate(
                width, height, 0.0, 10.0, 20.0, animation, renaming, true, 0,
            );
            assert!(layout.rename_input_viewport(&text, text.len()).is_none());
        }
    }

    #[test]
    fn rename_clicks_follow_visible_graphemes_across_scroll_scale_and_animation() {
        let text = "a界e\u{301}👩\u{200d}💻🇺🇸".repeat(8);
        let mut visible_cases = 0;
        for width in [400.0, 800.0, 1600.0] {
            for cell in [8.0, 12.0, 20.0] {
                for animation in [0.2, 0.8, 1.0] {
                    let layout = SessionPanelLayout::calculate(
                        width,
                        600.0,
                        0.0,
                        cell,
                        cell * 2.0,
                        animation,
                        true,
                        true,
                        1,
                    );
                    let field = layout.rename.unwrap();
                    for button in [field.save_button, field.cancel_button] {
                        assert_eq!(
                            layout.rename_input_click(
                                &text,
                                text.len(),
                                button.x + button.width / 2.0,
                                button.y + button.height / 2.0
                            ),
                            None
                        );
                    }
                    for cursor in [0, text.len()] {
                        let Some(view) = layout.rename_input_viewport(&text, cursor) else {
                            continue;
                        };
                        visible_cases += 1;
                        let y = view.text_y + cell;
                        let hit = |x| layout.rename_input_click(&text, cursor, x, y);
                        assert_eq!(
                            hit(field.input.x.max(0.0)),
                            Some(SessionPanelAction::PlaceRenameCaret {
                                cursor: view.text_range.start
                            })
                        );
                        assert_eq!(
                            hit(view.right),
                            Some(SessionPanelAction::PlaceRenameCaret {
                                cursor: view.text_range.end
                            })
                        );
                        let mut x = view.text_x;
                        for (offset, cluster) in
                            text[view.text_range.clone()].grapheme_indices(true)
                        {
                            let pixels =
                                termeric_core::unicode::grapheme_width(cluster) as f32 * cell;
                            assert_eq!(
                                hit(x + pixels * 0.25),
                                Some(SessionPanelAction::PlaceRenameCaret {
                                    cursor: view.text_range.start + offset
                                })
                            );
                            assert_eq!(
                                hit(x + pixels * 0.75),
                                Some(SessionPanelAction::PlaceRenameCaret {
                                    cursor: view.text_range.start + offset + cluster.len()
                                })
                            );
                            x += pixels;
                        }
                        assert_eq!(hit(f32::NAN), None);
                        assert_eq!(
                            layout.rename_input_click(
                                &text,
                                cursor,
                                view.text_x,
                                field.input.y - 1.0
                            ),
                            None
                        );
                    }
                }
            }
        }
        assert!(visible_cases > 10);
        // Partially revealed fields can be clickable before a single text
        // cell fits. Such clicks must not move a hidden caret.
        let layout =
            SessionPanelLayout::calculate(800.0, 600.0, 0.0, 8.0, 16.0, 0.06, true, true, 1);
        let field = layout.rename.unwrap();
        assert!(layout.rename_input_viewport(&text, text.len()).is_none());
        assert_eq!(
            layout.rename_input_click(&text, text.len(), field.input.x + 1.0, field.input.y + 1.0),
            Some(SessionPanelAction::None)
        );
    }

    #[test]
    fn rename_layout_and_value_limits_are_explicit() {
        let layout =
            SessionPanelLayout::calculate(1000.0, 800.0, 0.0, 10.0, 20.0, 1.0, true, true, 4);
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

    #[test]
    fn header_and_footer_controls_are_hit_exactly_and_rows_stay_above_the_link() {
        let centre = |rect: Bounds| (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        let workspaces =
            SessionPanelLayout::calculate(1000.0, 800.0, 34.0, 10.0, 20.0, 1.0, false, true, 40);
        assert!(workspaces.panel.y == 34.0 && workspaces.back_button.is_none());
        let (x, y) = centre(workspaces.close_button);
        assert_eq!(
            workspaces.control_at(x, y, 0),
            Some(SessionPanelControl::Close)
        );
        let link = workspaces.snapshots_link.unwrap();
        let (x, y) = centre(link);
        assert_eq!(
            workspaces.control_at(x, y, 0),
            None,
            "no snapshots, no link"
        );
        assert_eq!(
            workspaces.control_at(x, y, 2),
            Some(SessionPanelControl::SnapshotsLink)
        );
        let last_row =
            workspaces.list_start_y + workspaces.row_height * workspaces.visible_count as f32;
        assert!(last_row <= link.y);

        let snapshots =
            SessionPanelLayout::calculate(1000.0, 800.0, 34.0, 10.0, 20.0, 1.0, false, false, 1);
        assert!(snapshots.snapshots_link.is_none());
        let (x, y) = centre(snapshots.back_button.unwrap());
        assert_eq!(
            snapshots.control_at(x, y, 0),
            Some(SessionPanelControl::Back)
        );
    }
}
