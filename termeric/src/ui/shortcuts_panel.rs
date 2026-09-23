//! Standalone shortcuts panel — centered modal for viewing and editing keyboard shortcuts
//!
//! A dedicated overlay for browsing, searching, and recording keyboard shortcuts,
//! modeled after the command palette (OverlayAnimation, centered panel, search bar).

use super::animation::OverlayAnimation;
use super::text_input::{TextInput, TextInputEdit};
use super::text_input_viewport::TextInputViewport;
use crate::input::keybindings::{BindableAction, KeyChord, ShortcutCategory, ShortcutRegistry};

const MAX_FILTER_CHARACTERS: usize = 128;

fn valid_filter_text(value: &str) -> bool {
    value.len() <= MAX_FILTER_CHARACTERS * 4
        && value.chars().count() <= MAX_FILTER_CHARACTERS
        && !value
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
}

/// Result of a user action in the shortcuts panel
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutsAction {
    /// Begin recording a replacement chord for one stable action.
    StartRecording(BindableAction),
    /// Clear one action's current binding.
    ClearShortcut(BindableAction),
    /// Set a keyboard shortcut binding
    SetShortcut {
        action: BindableAction,
        chord: Option<KeyChord>,
    },
    /// No action
    None,
}

/// Simplified key events for the shortcuts panel
#[derive(Debug, Clone)]
pub enum ShortcutsKey {
    Up,
    Down,
    Enter,
    Escape,
    /// Delete/Backspace to clear a binding
    Delete,
    /// A captured key chord during recording mode
    RecordedChord(KeyChord),
    /// Backspace in search filter (non-recording mode)
    Backspace,
}

/// A single entry in the shortcuts list for display
#[derive(Debug, Clone)]
pub struct ShortcutEntry {
    /// Display label (action name)
    pub label: &'static str,
    /// Current binding display string (e.g. "Ctrl+Shift+C" or "Unset")
    pub binding: String,
    /// The bindable action
    pub action: BindableAction,
    /// Whether this is a category header (non-selectable separator)
    pub is_header: bool,
    /// Whether this action is the keyboard-selected entry.
    pub selected: bool,
}

/// Display snapshot for the renderer
#[derive(Debug, Clone)]
pub struct ShortcutsPanelInfo {
    /// Animation progress (0.0 = closed, 1.0 = open)
    pub animation: f32,
    /// Current search/filter input
    pub filter: String,
    /// Cursor position in filter input
    pub cursor: usize,
    /// Visible entries (sliced for scroll)
    pub entries: Vec<ShortcutEntry>,
    /// Scroll offset
    pub scroll_offset: usize,
    /// Total filtered entry count (for scrollbar)
    pub total_entries: usize,
    /// Total rendered rows, including category headings.
    pub total_rows: usize,
    /// Whether we're in recording mode
    pub recording: bool,
    /// Conflict label after reassignment
    pub conflict_label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShortcutsPanelRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl ShortcutsPanelRect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }
}

/// Shared shortcuts-modal geometry for rendering and outside-click routing.
#[derive(Debug, Clone, Copy)]
pub struct ShortcutsPanelLayout {
    pub panel_x: f32,
    pub panel_y: f32,
    pub panel_width: f32,
    pub panel_height: f32,
    pub padding: f32,
    pub row_height: f32,
    pub input_height: f32,
    pub footer_height: f32,
    pub gap: f32,
    pub separator_height: f32,
    pub list_height: f32,
    pub rows_that_fit: usize,
    pub input: ShortcutsPanelRect,
    pub list: ShortcutsPanelRect,
}

impl ShortcutsPanelLayout {
    /// The editable text area excludes the prompt and keeps one caret column.
    pub fn input_viewport(
        &self,
        text: &str,
        cursor: usize,
        cell_width: f32,
        cell_height: f32,
    ) -> Option<TextInputViewport> {
        TextInputViewport::calculate(
            text,
            cursor,
            self.input.x + cell_width * 2.2,
            self.input.y + (self.input.height - cell_height) / 2.0,
            self.input.x + self.input.width - cell_width * 0.8,
            cell_width,
        )
    }

    /// Resolve only input clicks, using the exact visible whole-grapheme slice.
    pub fn input_cursor_at(
        &self,
        text: &str,
        cursor: usize,
        position: (f32, f32),
        cell_width: f32,
        cell_height: f32,
    ) -> Option<usize> {
        let (x, y) = position;
        if !self.input.contains(x, y) {
            return None;
        }
        self.input_viewport(text, cursor, cell_width, cell_height)?
            .cursor_at(text, x, cell_width)
    }

    pub fn calculate(
        window_width: f32,
        window_height: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
        entry_count: usize,
    ) -> Self {
        let panel_width = (cell_width * 60.0).max(500.0).min(window_width * 0.7);
        let panel_x = (window_width - panel_width) / 2.0;
        let padding = cell_width * 1.5;
        let row_height = cell_height * 2.0;
        let input_height = cell_height * 2.0;
        let footer_height = cell_height * 1.6;
        let gap = 6.0;
        let separator_height = 1.0;
        let header_area =
            padding + cell_height * 1.4 + gap + input_height + gap + separator_height + gap;
        let footer_area = footer_height + padding * 0.5;
        let max_list_height = window_height * 0.75 - header_area - footer_area;
        let rows_that_fit = (max_list_height / row_height).floor().max(1.0) as usize;
        let list_height = entry_count.min(rows_that_fit) as f32 * row_height;
        let panel_height = header_area + list_height + footer_area;
        let panel_y = window_height * 0.10 + (1.0 - animation) * -30.0;

        let title_y = panel_y + padding;
        let input_y = title_y + cell_height * 1.4 + gap;
        let list_y = input_y + input_height + gap + separator_height + gap;
        Self {
            panel_x,
            panel_y,
            panel_width,
            panel_height,
            padding,
            row_height,
            input_height,
            footer_height,
            gap,
            separator_height,
            list_height,
            rows_that_fit,
            input: ShortcutsPanelRect {
                x: panel_x + padding,
                y: input_y,
                width: panel_width - padding * 2.0,
                height: input_height,
            },
            list: ShortcutsPanelRect {
                x: panel_x + padding * 0.5,
                y: list_y,
                width: panel_width - padding,
                height: list_height,
            },
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.panel_x
            && x <= self.panel_x + self.panel_width
            && y >= self.panel_y
            && y <= self.panel_y + self.panel_height
    }

    pub fn row(&self, visible_index: usize) -> ShortcutsPanelRect {
        ShortcutsPanelRect {
            x: self.list.x,
            y: self.list.y + visible_index as f32 * self.row_height,
            width: self.list.width,
            height: self.row_height,
        }
    }

    pub fn row_at(&self, x: f32, y: f32, visible_rows: usize) -> Option<usize> {
        if !self.list.contains(x, y) || self.row_height <= 0.0 {
            return None;
        }
        let row = ((y - self.list.y) / self.row_height).floor() as usize;
        (row < visible_rows).then_some(row)
    }
}

/// The shortcuts panel state
pub struct ShortcutsPanel {
    /// Animation state (fade in/out like command palette)
    animation: OverlayAnimation,
    /// Search/filter input
    filter: TextInput,
    /// Selected item index (within filtered list)
    selected: usize,
    /// Scroll offset for long lists
    scroll_offset: usize,
    /// Max visible rows (calculated dynamically)
    max_visible: usize,
    /// Whether we're in key recording mode
    recording: bool,
    /// The action being recorded
    recording_action: Option<BindableAction>,
    /// When recording mode was entered. Used to auto-cancel after a
    /// timeout so users can't get stuck if they walk away.
    recording_started: Option<std::time::Instant>,
    /// Brief conflict notification text
    conflict_label: Option<String>,
}

impl ShortcutsPanel {
    pub fn new() -> Self {
        Self {
            animation: OverlayAnimation::new(),
            filter: TextInput::new(),
            selected: 0,
            scroll_offset: 0,
            max_visible: 12,
            recording: false,
            recording_action: None,
            recording_started: None,
            conflict_label: None,
        }
    }

    /// Open the shortcuts panel
    pub fn open(&mut self) {
        self.animation.open();
        self.filter.clear();
        self.selected = 0;
        self.scroll_offset = 0;
        self.recording = false;
        self.recording_action = None;
        self.recording_started = None;
        self.conflict_label = None;
    }

    /// Close the shortcuts panel
    pub fn close(&mut self) {
        self.animation.close();
        self.recording = false;
        self.recording_action = None;
        self.recording_started = None;
    }

    /// Close immediately (no animation)
    #[cfg(test)]
    pub fn close_immediately(&mut self) {
        self.animation.close_immediately();
        self.recording = false;
        self.recording_action = None;
        self.recording_started = None;
    }

    /// Cancel recording if it's been idle longer than `timeout`. Returns
    /// true if a cancellation happened this call (caller may want to
    /// redraw).
    pub fn tick_recording_timeout(&mut self, timeout: std::time::Duration) -> bool {
        if !self.recording {
            return false;
        }
        let Some(started) = self.recording_started else {
            return false;
        };
        if started.elapsed() >= timeout {
            self.recording = false;
            self.recording_action = None;
            self.recording_started = None;
            self.conflict_label = Some("Recording timed out".to_string());
            return true;
        }
        false
    }

    pub fn recording_deadline(&self, timeout: std::time::Duration) -> Option<std::time::Instant> {
        self.recording
            .then_some(self.recording_started?)
            .map(|started| started + timeout)
    }

    /// Toggle open/close
    pub fn toggle(&mut self) {
        if self.is_active() {
            self.close();
        } else {
            self.open();
        }
    }

    /// Whether the panel is logically active (blocking input)
    pub fn is_active(&self) -> bool {
        self.animation.is_active()
    }

    /// Whether the panel should be rendered
    pub fn is_visible(&self) -> bool {
        self.animation.is_visible()
    }

    /// Whether we're in key recording mode
    pub fn is_recording(&self) -> bool {
        self.recording
    }

    /// Update animation state. Returns true if still animating.
    pub fn update_animation(&mut self) -> bool {
        self.animation.update()
    }

    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.animation.set_reduced_motion(reduced_motion);
    }

    /// Build the filtered entries list from the registry
    fn build_entries(&self, registry: &ShortcutRegistry) -> Vec<ShortcutEntry> {
        let filter = self.filter.text().to_lowercase();
        let mut entries = Vec::new();
        let mut last_category: Option<ShortcutCategory> = None;

        // Group by category so each header appears once, whatever order the
        // action registry declares its members in.
        let grouped = ShortcutCategory::ORDERED.into_iter().flat_map(|category| {
            BindableAction::all()
                .iter()
                .copied()
                .filter(move |action| action.meta().category == category)
        });
        for action in grouped {
            let meta = action.meta();

            // Apply search filter
            if !filter.is_empty()
                && !meta.label.to_lowercase().contains(&filter)
                && !meta.category.label().to_lowercase().contains(&filter)
                && !registry
                    .binding_for(action)
                    .map(|c| c.display_string().to_lowercase().contains(&filter))
                    .unwrap_or(false)
            {
                continue;
            }

            // Insert category header when category changes
            if last_category != Some(meta.category) {
                entries.push(ShortcutEntry {
                    label: meta.category.label(),
                    binding: String::new(),
                    action,
                    is_header: true,
                    selected: false,
                });
                last_category = Some(meta.category);
            }

            let binding = registry
                .binding_for(action)
                .map(|c| c.display_string())
                .unwrap_or_else(|| "Unset".to_string());

            entries.push(ShortcutEntry {
                label: meta.label,
                binding,
                action,
                is_header: false,
                selected: false,
            });
        }
        entries
    }

    /// Count of selectable (non-header) entries
    fn selectable_count(&self, entries: &[ShortcutEntry]) -> usize {
        entries.iter().filter(|e| !e.is_header).count()
    }

    /// Get the actual index in entries for the nth selectable item
    fn selectable_index(&self, entries: &[ShortcutEntry], nth: usize) -> usize {
        let mut count = 0;
        for (i, e) in entries.iter().enumerate() {
            if !e.is_header {
                if count == nth {
                    return i;
                }
                count += 1;
            }
        }
        entries.len().saturating_sub(1)
    }

    /// Handle a key press. Returns a ShortcutsAction.
    pub fn handle_key(
        &mut self,
        key: ShortcutsKey,
        registry: &ShortcutRegistry,
    ) -> ShortcutsAction {
        // Recording mode: capture the key
        if self.recording {
            return match key {
                ShortcutsKey::Escape => {
                    self.recording = false;
                    self.recording_action = None;
                    self.recording_started = None;
                    ShortcutsAction::None
                }
                ShortcutsKey::Delete => {
                    self.recording = false;
                    self.recording_started = None;
                    let action = self.recording_action.take();
                    match action {
                        Some(a) => ShortcutsAction::SetShortcut {
                            action: a,
                            chord: None,
                        },
                        None => ShortcutsAction::None,
                    }
                }
                ShortcutsKey::RecordedChord(chord) => {
                    self.recording = false;
                    self.recording_started = None;
                    let action = self.recording_action.take();
                    if let Some(a) = action {
                        // Check for conflict
                        if let Some(conflicting) = registry.conflict_for(&chord) {
                            if conflicting != a {
                                self.conflict_label = Some(conflicting.meta().label.to_string());
                            }
                        }
                        ShortcutsAction::SetShortcut {
                            action: a,
                            chord: Some(chord),
                        }
                    } else {
                        ShortcutsAction::None
                    }
                }
                _ => ShortcutsAction::None,
            };
        }

        let entries = self.build_entries(registry);
        let selectable = self.selectable_count(&entries);

        match key {
            ShortcutsKey::Escape => {
                if !self.filter.text().is_empty() {
                    // Clear filter first
                    self.filter.clear();
                    self.selected = 0;
                    self.scroll_offset = 0;
                } else {
                    self.close();
                }
                ShortcutsAction::None
            }
            ShortcutsKey::Up => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                self.ensure_visible(&entries);
                ShortcutsAction::None
            }
            ShortcutsKey::Down => {
                if selectable > 0 && self.selected + 1 < selectable {
                    self.selected += 1;
                }
                self.ensure_visible(&entries);
                ShortcutsAction::None
            }
            ShortcutsKey::Enter => {
                // Start recording for the selected action
                let idx = self.selectable_index(&entries, self.selected);
                if let Some(entry) = entries.get(idx) {
                    if !entry.is_header {
                        self.start_recording(entry.action);
                    }
                }
                ShortcutsAction::None
            }
            ShortcutsKey::Delete => {
                // Clear binding for selected action
                let idx = self.selectable_index(&entries, self.selected);
                if let Some(entry) = entries.get(idx) {
                    if !entry.is_header {
                        return ShortcutsAction::SetShortcut {
                            action: entry.action,
                            chord: None,
                        };
                    }
                }
                ShortcutsAction::None
            }
            ShortcutsKey::Backspace => {
                self.edit_filter(TextInputEdit::Backspace);
                ShortcutsAction::None
            }
            ShortcutsKey::RecordedChord(_) => ShortcutsAction::None,
        }
    }

    /// Ensure the selected item is visible within the scroll window
    fn ensure_visible(&mut self, entries: &[ShortcutEntry]) {
        let selected_row = self.selectable_index(entries, self.selected);
        if selected_row < self.scroll_offset {
            self.scroll_offset = selected_row;
        } else if selected_row >= self.scroll_offset + self.max_visible {
            self.scroll_offset = selected_row + 1 - self.max_visible;
        }
    }

    pub fn set_max_visible(&mut self, max_visible: usize) {
        self.max_visible = max_visible.max(1);
    }

    pub fn accessibility_set_filter(&mut self, value: &str) -> bool {
        if !self.is_active() || self.recording || !valid_filter_text(value) {
            return false;
        }
        self.filter.replace(value);
        self.selected = 0;
        self.scroll_offset = 0;
        true
    }

    /// Insert an entire keyboard/IME event or reject it unchanged. The same
    /// scalar and single-line policy applies to semantic replacement.
    pub fn insert_filter_text(&mut self, value: &str) -> bool {
        if !self.is_active()
            || self.recording
            || value.is_empty()
            || !valid_filter_text(value)
            || self.filter.text().chars().count() + value.chars().count() > MAX_FILTER_CHARACTERS
        {
            return false;
        }
        self.filter.insert_text(value);
        self.selected = 0;
        self.scroll_offset = 0;
        true
    }

    /// Edit the filter without disturbing result navigation on caret-only moves.
    /// Returns whether either the text or caret changed.
    pub fn edit_filter(&mut self, edit: TextInputEdit) -> bool {
        if !self.is_active() || self.recording {
            return false;
        }
        let previous_cursor = self.filter.cursor();
        let changed = self.filter.edit_grapheme(edit);
        if changed {
            self.selected = 0;
            self.scroll_offset = 0;
        }
        changed || self.filter.cursor() != previous_cursor
    }

    /// Place the caret without changing the filter, result selection, or scroll.
    /// Hidden/recording/collapsed fields cannot acquire a pointer caret.
    pub fn place_filter_caret_at(
        &mut self,
        x: f32,
        y: f32,
        layout: &ShortcutsPanelLayout,
        cell_width: f32,
        cell_height: f32,
    ) -> bool {
        if !self.is_active() || self.recording {
            return false;
        }
        let Some(cursor) = layout.input_cursor_at(
            self.filter.text(),
            self.filter.cursor(),
            (x, y),
            cell_width,
            cell_height,
        ) else {
            return false;
        };
        self.filter.set_cursor_byte(cursor)
    }

    /// Active filter geometry without rebuilding or cloning result rows.
    pub fn input_viewport(
        &self,
        width: f32,
        height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> Option<TextInputViewport> {
        if !self.is_active() || self.recording {
            return None;
        }
        // Result count changes panel height, never the filter rectangle.
        ShortcutsPanelLayout::calculate(
            width,
            height,
            cell_width,
            cell_height,
            self.animation.eased(),
            0,
        )
        .input_viewport(
            self.filter.text(),
            self.filter.cursor(),
            cell_width,
            cell_height,
        )
    }

    /// Enter recording mode for a stable action identity.
    pub fn start_recording(&mut self, action: BindableAction) {
        self.recording = true;
        self.recording_action = Some(action);
        self.recording_started = Some(std::time::Instant::now());
        self.conflict_label = None;
    }

    pub fn select_action(&mut self, action: BindableAction, registry: &ShortcutRegistry) -> bool {
        let entries = self.build_entries(registry);
        let mut selectable = 0;
        for entry in &entries {
            if entry.is_header {
                continue;
            }
            if entry.action == action {
                self.selected = selectable;
                self.ensure_visible(&entries);
                return true;
            }
            selectable += 1;
        }
        false
    }

    /// Resolve a pointer click through the same stable action path as keyboard
    /// and accessibility activation.
    pub fn handle_click(
        &mut self,
        x: f32,
        y: f32,
        layout: &ShortcutsPanelLayout,
        registry: &ShortcutRegistry,
    ) -> ShortcutsAction {
        let info = self.build_info(registry);
        let Some(row) = layout.row_at(x, y, info.entries.len()) else {
            return ShortcutsAction::None;
        };
        let Some(entry) = info.entries.get(row) else {
            return ShortcutsAction::None;
        };
        if entry.is_header {
            return ShortcutsAction::None;
        }
        self.select_action(entry.action, registry);
        ShortcutsAction::StartRecording(entry.action)
    }

    /// Whether an action is still part of the current filtered result set.
    pub fn is_visible_action(&self, action: BindableAction, registry: &ShortcutRegistry) -> bool {
        self.build_info(registry)
            .entries
            .iter()
            .any(|entry| !entry.is_header && entry.action == action)
    }

    /// Scroll up by n lines (for mouse wheel)
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Scroll down by n lines (for mouse wheel)
    pub fn scroll_down(&mut self, n: usize, registry: &ShortcutRegistry) {
        let entries = self.build_entries(registry);
        let max = entries.len().saturating_sub(self.max_visible);
        self.scroll_offset = self.scroll_offset.saturating_add(n).min(max);
    }

    /// Build display info for the renderer
    pub fn build_info(&self, registry: &ShortcutRegistry) -> ShortcutsPanelInfo {
        let all_entries = self.build_entries(registry);
        let selectable = self.selectable_count(&all_entries);
        let total_rows = all_entries.len();
        let scroll_offset = self
            .scroll_offset
            .min(total_rows.saturating_sub(self.max_visible));
        let visible_end = (scroll_offset + self.max_visible).min(total_rows);
        let selected_row = self.selectable_index(&all_entries, self.selected);
        let mut visible = all_entries[scroll_offset..visible_end].to_vec();
        for (row, entry) in visible.iter_mut().enumerate() {
            entry.selected = scroll_offset + row == selected_row;
        }
        ShortcutsPanelInfo {
            animation: self.animation.eased(),
            filter: self.filter.text().to_string(),
            cursor: self.filter.cursor(),
            entries: visible,
            scroll_offset,
            total_entries: selectable,
            total_rows,
            recording: self.recording,
            conflict_label: self.conflict_label.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_close() {
        let mut panel = ShortcutsPanel::new();
        assert!(!panel.is_active());
        assert!(!panel.is_visible());

        panel.open();
        assert!(panel.is_active());

        panel.close_immediately();
        assert!(!panel.is_active());
    }

    #[test]
    fn every_category_header_appears_once_with_all_its_shortcuts_beneath_it() {
        let panel = ShortcutsPanel::new();
        let entries = panel.build_entries(&ShortcutRegistry::new());
        let headers: Vec<_> = entries
            .iter()
            .filter(|entry| entry.is_header)
            .map(|entry| entry.label)
            .collect();
        let mut unique = headers.clone();
        unique.dedup();
        assert_eq!(headers, unique, "a category header repeats: {headers:?}");
        assert_eq!(
            headers,
            ShortcutCategory::ORDERED
                .iter()
                .map(|category| category.label())
                .collect::<Vec<_>>()
        );
        let mut current = None;
        for entry in &entries {
            if entry.is_header {
                current = Some(entry.label);
            } else {
                assert_eq!(Some(entry.action.meta().category.label()), current);
            }
        }
        assert_eq!(
            entries.iter().filter(|entry| !entry.is_header).count(),
            BindableAction::all().len()
        );
    }

    #[test]
    fn test_build_entries() {
        let panel = ShortcutsPanel::new();
        let registry = ShortcutRegistry::new();
        let entries = panel.build_entries(&registry);
        // Should have category headers + all actions
        assert!(entries.len() > BindableAction::all().len());
        // First entry should be a header
        assert!(entries[0].is_header);
    }

    #[test]
    fn shared_layout_uses_rendered_height_for_outside_clicks() {
        let layout = ShortcutsPanelLayout::calculate(1200.0, 800.0, 10.0, 20.0, 1.0, 2);
        assert!(layout.contains(
            layout.panel_x + layout.panel_width / 2.0,
            layout.panel_y + layout.panel_height - 1.0,
        ));
        assert!(!layout.contains(
            layout.panel_x + layout.panel_width / 2.0,
            layout.panel_y + layout.panel_height + 1.0,
        ));
        assert!(layout.panel_height < 800.0 * 0.75);
    }

    #[test]
    fn test_filter() {
        let mut panel = ShortcutsPanel::new();
        let registry = ShortcutRegistry::new();

        panel.open();
        let all = panel.build_entries(&registry);
        let all_count = all.len();

        // Type a filter
        for character in ["c", "o", "p", "y"] {
            assert!(panel.insert_filter_text(character));
        }
        let filtered = panel.build_entries(&registry);
        assert!(filtered.len() < all_count);
        // Should find "Copy"
        assert!(filtered.iter().any(|e| e.label == "Copy"));
    }

    #[test]
    fn test_recording_mode() {
        let mut panel = ShortcutsPanel::new();
        let registry = ShortcutRegistry::new();

        panel.open();
        assert!(!panel.is_recording());

        // Press Enter to start recording
        panel.handle_key(ShortcutsKey::Enter, &registry);
        assert!(panel.is_recording());

        // Escape cancels recording
        panel.handle_key(ShortcutsKey::Escape, &registry);
        assert!(!panel.is_recording());
    }

    #[test]
    fn test_navigation() {
        let mut panel = ShortcutsPanel::new();
        let registry = ShortcutRegistry::new();

        panel.open();
        assert_eq!(panel.selected, 0);

        panel.handle_key(ShortcutsKey::Down, &registry);
        assert_eq!(panel.selected, 1);

        panel.handle_key(ShortcutsKey::Up, &registry);
        assert_eq!(panel.selected, 0);

        // Can't go above 0
        panel.handle_key(ShortcutsKey::Up, &registry);
        assert_eq!(panel.selected, 0);
    }

    #[test]
    fn test_escape_clears_filter_first() {
        let mut panel = ShortcutsPanel::new();
        let registry = ShortcutRegistry::new();

        panel.open();
        assert!(panel.insert_filter_text("t"));
        assert!(!panel.filter.text().is_empty());

        // First escape clears filter
        panel.handle_key(ShortcutsKey::Escape, &registry);
        assert!(panel.filter.text().is_empty());
        assert!(panel.is_active()); // Still open

        // Second escape closes
        panel.handle_key(ShortcutsKey::Escape, &registry);
        // close() was called but animation may still be visible
    }

    #[test]
    fn pointer_rows_resolve_stable_actions_through_shared_geometry() {
        let mut panel = ShortcutsPanel::new();
        let registry = ShortcutRegistry::new();
        panel.open();
        panel.set_max_visible(4);
        let info = panel.build_info(&registry);
        let action_row = info
            .entries
            .iter()
            .position(|entry| !entry.is_header)
            .unwrap();
        let expected = info.entries[action_row].action;
        let layout =
            ShortcutsPanelLayout::calculate(1200.0, 800.0, 10.0, 20.0, 1.0, info.entries.len());
        let row = layout.row(action_row);
        let action = panel.handle_click(
            row.x + row.width / 2.0,
            row.y + row.height / 2.0,
            &layout,
            &registry,
        );
        assert_eq!(action, ShortcutsAction::StartRecording(expected));
        assert!(
            panel
                .build_info(&registry)
                .entries
                .iter()
                .any(|entry| { !entry.is_header && entry.action == expected && entry.selected })
        );
    }

    #[test]
    fn scrolling_is_bounded_by_rendered_rows_including_headings() {
        let mut panel = ShortcutsPanel::new();
        let registry = ShortcutRegistry::new();
        panel.open();
        panel.set_max_visible(5);
        panel.scroll_down(usize::MAX, &registry);
        let info = panel.build_info(&registry);
        assert_eq!(info.entries.len(), 5);
        assert_eq!(info.scroll_offset, info.total_rows - 5);
        assert!(info.scroll_offset + info.entries.len() <= info.total_rows);
    }

    #[test]
    fn filter_viewport_keeps_graphemes_and_caret_inside_scaled_animated_input() {
        use unicode_segmentation::UnicodeSegmentation;
        let text = "界e\u{301}👩\u{200d}💻".repeat(20);
        for (width, cell) in [(240.0, 8.0), (800.0, 8.0), (1600.0, 16.0)] {
            for animation in [0.25, 1.0] {
                let layout =
                    ShortcutsPanelLayout::calculate(width, 800.0, cell, cell * 2.0, animation, 5);
                assert_eq!(
                    layout.input,
                    ShortcutsPanelLayout::calculate(width, 800.0, cell, cell * 2.0, animation, 0)
                        .input
                );
                for cursor in text
                    .grapheme_indices(true)
                    .map(|(i, _)| i)
                    .chain([text.len()])
                {
                    let view = layout
                        .input_viewport(&text, cursor, cell, cell * 2.0)
                        .unwrap();
                    assert!(view.text_range.start <= cursor && cursor <= view.text_range.end);
                    assert!(view.text_x > layout.input.x);
                    assert!(
                        view.cursor_x >= view.text_x && view.cursor_x + cell <= view.right + 0.001
                    );
                    assert!(view.right < layout.input.x + layout.input.width);
                    assert_eq!(view.text_y, layout.input.y + cell);
                    for edge in [view.text_range.start, view.text_range.end] {
                        assert!(
                            edge == text.len()
                                || text.grapheme_indices(true).any(|(i, _)| i == edge)
                        );
                    }
                    if cursor == text.len() {
                        assert!(view.text_range.start > 0);
                    }
                }
                assert!(layout.input_viewport(&text, 1, cell, cell * 2.0).is_none());
            }
        }
        let layout = ShortcutsPanelLayout::calculate(40.0, 800.0, 8.0, 16.0, 1.0, 0);
        assert!(
            layout
                .input_viewport(&text, text.len(), 8.0, 16.0)
                .is_none()
        );
    }

    #[test]
    fn filter_input_and_replacement_share_scalar_limits_and_atomic_rejection() {
        let mut panel = ShortcutsPanel::new();
        assert!(!panel.insert_filter_text("hidden"));
        panel.open();
        for _ in 0..MAX_FILTER_CHARACTERS {
            assert!(panel.insert_filter_text("🫚"));
        }
        assert_eq!(panel.filter.text().len(), MAX_FILTER_CHARACTERS * 4);
        assert!(!panel.insert_filter_text("x"));
        assert!(panel.accessibility_set_filter(&"x".repeat(127)));
        panel.selected = 2;
        panel.scroll_offset = 1;
        for invalid in [
            "e\u{301}",
            "界界",
            "\n",
            "x\t",
            "\0",
            "\u{1b}",
            "\u{85}",
            "\u{2028}",
            "\u{2029}",
            &"x".repeat(513),
        ] {
            assert!(!panel.insert_filter_text(invalid));
            assert_eq!(panel.filter.text(), "x".repeat(127));
            assert_eq!(panel.filter.cursor(), 127);
            assert_eq!((panel.selected, panel.scroll_offset), (2, 1));
        }
        assert!(panel.insert_filter_text("界"));
        assert_eq!(panel.filter.text().chars().count(), 128);
        assert_eq!((panel.selected, panel.scroll_offset), (0, 0));
        for invalid in ["\n", "\t", "\u{2028}", "\u{2029}", &"x".repeat(129)] {
            assert!(!panel.accessibility_set_filter(invalid));
            assert_eq!(panel.filter.text().chars().count(), 128);
        }
        panel.start_recording(BindableAction::Copy);
        assert!(!panel.insert_filter_text("new"));
        assert!(!panel.accessibility_set_filter("new"));
        assert!(panel.is_recording());
        assert_eq!(panel.filter.text().chars().count(), 128);
    }

    #[test]
    fn filter_pointer_geometry_matches_visible_graphemes_at_scales_and_animation() {
        use unicode_segmentation::UnicodeSegmentation;
        for (width, height, cw, ch) in [(1000.0, 700.0, 8.0, 16.0), (500.0, 400.0, 15.0, 30.0)] {
            for animation in [0.2, 0.7, 1.0] {
                let layout = ShortcutsPanelLayout::calculate(width, height, cw, ch, animation, 8);
                for text in [
                    "e\u{301}界👩\u{200d}💻🇺🇸".to_string(),
                    "a界e\u{301}👩\u{200d}💻".repeat(16),
                ] {
                    let cursor = text.len();
                    let view = layout.input_viewport(&text, cursor, cw, ch).unwrap();
                    let mut x = view.text_x;
                    for (offset, grapheme) in text[view.text_range.clone()].grapheme_indices(true) {
                        let pixels = termeric_core::unicode::grapheme_width(grapheme) as f32 * cw;
                        for (fraction, expected) in [
                            (0.25, view.text_range.start + offset),
                            (0.75, view.text_range.start + offset + grapheme.len()),
                        ] {
                            assert_eq!(
                                layout.input_cursor_at(
                                    &text,
                                    cursor,
                                    (x + fraction * pixels, view.text_y),
                                    cw,
                                    ch
                                ),
                                Some(expected)
                            );
                        }
                        x += pixels;
                    }
                    assert_eq!(
                        layout.input_cursor_at(
                            &text,
                            cursor,
                            (layout.input.x, view.text_y),
                            cw,
                            ch
                        ),
                        Some(view.text_range.start)
                    );
                    assert_eq!(
                        layout.input_cursor_at(
                            &text,
                            cursor,
                            (layout.input.x + layout.input.width, view.text_y),
                            cw,
                            ch
                        ),
                        Some(view.text_range.end)
                    );
                    for position in [
                        (view.text_x, layout.input.y - 1.0),
                        (f32::NAN, view.text_y),
                        (view.text_x, f32::INFINITY),
                    ] {
                        assert_eq!(
                            layout.input_cursor_at(&text, cursor, position, cw, ch),
                            None
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn filter_pointer_caret_preserves_results_and_refuses_recording_or_collapsed_input() {
        let mut panel = ShortcutsPanel::new();
        let layout = ShortcutsPanelLayout::calculate(1000.0, 700.0, 8.0, 16.0, 1.0, 8);
        let (x, y) = (layout.input.x + 2.2 * 8.0, layout.input.y + 8.0);
        assert!(!panel.place_filter_caret_at(x, y, &layout, 8.0, 16.0));
        panel.open();
        assert!(panel.insert_filter_text("e\u{301}界👩\u{200d}💻"));
        panel.selected = 2;
        panel.scroll_offset = 1;
        assert!(panel.place_filter_caret_at(x + 6.0, y, &layout, 8.0, 16.0));
        assert_eq!(panel.filter.cursor(), "e\u{301}".len());
        assert_eq!((panel.selected, panel.scroll_offset), (2, 1));
        assert_eq!(panel.filter.text(), "e\u{301}界👩\u{200d}💻");
        assert!(panel.insert_filter_text("@"));
        assert_eq!(panel.filter.text(), "e\u{301}@界👩\u{200d}💻");
        let before = panel.filter.cursor();
        panel.start_recording(BindableAction::Copy);
        assert!(!panel.place_filter_caret_at(x, y, &layout, 8.0, 16.0));
        assert_eq!(panel.filter.cursor(), before);
        panel.open();
        assert!(panel.place_filter_caret_at(x, y, &layout, 8.0, 16.0));
        assert_eq!(panel.filter.cursor(), 0);
        let collapsed = ShortcutsPanelLayout::calculate(60.0, 200.0, 8.0, 16.0, 1.0, 0);
        assert!(collapsed.input.width > 0.0);
        assert!(!panel.place_filter_caret_at(
            collapsed.input.x,
            collapsed.input.y,
            &collapsed,
            8.0,
            16.0
        ));
        panel.close_immediately();
        assert!(!panel.place_filter_caret_at(x, y, &layout, 8.0, 16.0));
    }

    #[test]
    fn filter_caret_edits_preserve_result_navigation_until_text_changes() {
        let mut panel = ShortcutsPanel::new();
        panel.open();
        let text = "e\u{301}👩\u{200d}💻🇺🇸";
        assert!(panel.insert_filter_text(text));
        panel.selected = 2;
        panel.scroll_offset = 1;
        for cursor in ["e\u{301}👩\u{200d}💻".len(), "e\u{301}".len(), 0] {
            assert!(panel.edit_filter(TextInputEdit::Left));
            assert_eq!(panel.filter.cursor(), cursor);
            assert_eq!((panel.selected, panel.scroll_offset), (2, 1));
        }
        assert!(!panel.edit_filter(TextInputEdit::Left));
        assert!(!panel.edit_filter(TextInputEdit::Backspace));
        assert_eq!((panel.selected, panel.scroll_offset), (2, 1));
        assert!(panel.edit_filter(TextInputEdit::Right));
        assert_eq!(panel.filter.cursor(), "e\u{301}".len());
        assert!(panel.edit_filter(TextInputEdit::Backspace));
        assert_eq!(panel.filter.text(), "👩\u{200d}💻🇺🇸");
        assert_eq!((panel.selected, panel.scroll_offset), (0, 0));
        assert!(panel.edit_filter(TextInputEdit::End));
        assert_eq!(panel.filter.cursor(), panel.filter.text().len());
        assert!(panel.edit_filter(TextInputEdit::Home));
        assert!(panel.insert_filter_text("界"));
        assert_eq!(panel.filter.text(), "界👩\u{200d}💻🇺🇸");
    }

    #[test]
    fn filter_word_edits_obey_lifecycle_and_plain_delete_keeps_binding_action() {
        let mut panel = ShortcutsPanel::new();
        let registry = ShortcutRegistry::new();
        assert!(!panel.edit_filter(TextInputEdit::Home));
        panel.open();
        assert!(panel.insert_filter_text("copy paste"));
        assert!(panel.edit_filter(TextInputEdit::WordLeft));
        assert_eq!(panel.filter.cursor(), 5);
        assert!(panel.edit_filter(TextInputEdit::WordRight));
        assert_eq!(panel.filter.cursor(), 10);
        assert!(panel.edit_filter(TextInputEdit::BackspaceWord));
        assert_eq!(panel.filter.text(), "copy ");
        panel.start_recording(BindableAction::Copy);
        assert!(!panel.edit_filter(TextInputEdit::Home));
        assert!(!panel.edit_filter(TextInputEdit::BackspaceWord));
        assert_eq!(panel.filter.text(), "copy ");
        assert_eq!(panel.filter.cursor(), 5);
        panel.handle_key(ShortcutsKey::Escape, &registry);
        assert!(panel.accessibility_set_filter("copy"));
        assert!(matches!(
            panel.handle_key(ShortcutsKey::Delete, &registry),
            ShortcutsAction::SetShortcut { chord: None, .. }
        ));
        assert_eq!(panel.filter.text(), "copy");
        panel.close_immediately();
        assert!(!panel.edit_filter(TextInputEdit::Backspace));
        assert_eq!(panel.filter.text(), "copy");
    }

    #[test]
    fn filter_backspace_removes_complete_graphemes() {
        let mut panel = ShortcutsPanel::new();
        let registry = ShortcutRegistry::new();
        panel.open();
        for cluster in ["e\u{301}", "👩\u{200d}💻", "🇺🇸"] {
            assert!(panel.insert_filter_text(cluster));
            assert_eq!(
                panel.handle_key(ShortcutsKey::Backspace, &registry),
                ShortcutsAction::None
            );
            assert!(panel.filter.text().is_empty());
            assert_eq!(panel.filter.cursor(), 0);
        }
    }

    #[test]
    fn semantic_filter_replacement_is_utf8_aware_single_line_and_bounded() {
        let mut panel = ShortcutsPanel::new();
        assert!(!panel.accessibility_set_filter("hidden"));
        panel.open();
        assert!(panel.accessibility_set_filter("copy 🫚"));
        assert_eq!(panel.filter.text(), "copy 🫚");
        assert!(!panel.accessibility_set_filter("copy\npaste"));
        assert_eq!(panel.filter.text(), "copy 🫚");
        assert!(!panel.accessibility_set_filter(&"x".repeat(129)));
        assert_eq!(panel.filter.text(), "copy 🫚");
        panel.close_immediately();
        assert!(!panel.accessibility_set_filter(""));
        assert_eq!(panel.filter.text(), "copy 🫚");
    }
}
