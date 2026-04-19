//! Standalone shortcuts panel — centered modal for viewing and editing keyboard shortcuts
//!
//! A dedicated overlay for browsing, searching, and recording keyboard shortcuts,
//! modeled after the command palette (OverlayAnimation, centered panel, search bar).

use super::animation::OverlayAnimation;
use super::text_input::TextInput;
use crate::input::keybindings::{BindableAction, KeyChord, ShortcutCategory, ShortcutRegistry};

/// Result of a user action in the shortcuts panel
#[derive(Debug, Clone)]
pub enum ShortcutsAction {
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
    /// Character typed for search filter
    Char(char),
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
    /// Category label (for headers)
    pub category: &'static str,
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
    /// Index of selected entry (relative to visible slice)
    pub selected: usize,
    /// Scroll offset
    pub scroll_offset: usize,
    /// Total filtered entry count (for scrollbar)
    pub total_entries: usize,
    /// Whether we're in recording mode
    pub recording: bool,
    /// Conflict label after reassignment
    pub conflict_label: Option<String>,
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

    /// Build the filtered entries list from the registry
    fn build_entries(&self, registry: &ShortcutRegistry) -> Vec<ShortcutEntry> {
        let filter = self.filter.text().to_lowercase();
        let mut entries = Vec::new();
        let mut last_category: Option<ShortcutCategory> = None;

        for &action in BindableAction::all() {
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
                    category: meta.category.label(),
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
                category: meta.category.label(),
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

    /// Get the selectable ordinal for an entries index
    fn selectable_ordinal(&self, entries: &[ShortcutEntry], idx: usize) -> usize {
        entries[..=idx.min(entries.len().saturating_sub(1))]
            .iter()
            .filter(|e| !e.is_header)
            .count()
            .saturating_sub(1)
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
                self.ensure_visible(selectable);
                ShortcutsAction::None
            }
            ShortcutsKey::Down => {
                if selectable > 0 && self.selected + 1 < selectable {
                    self.selected += 1;
                }
                self.ensure_visible(selectable);
                ShortcutsAction::None
            }
            ShortcutsKey::Enter => {
                // Start recording for the selected action
                let idx = self.selectable_index(&entries, self.selected);
                if let Some(entry) = entries.get(idx) {
                    if !entry.is_header {
                        self.recording = true;
                        self.recording_action = Some(entry.action);
                        self.recording_started = Some(std::time::Instant::now());
                        self.conflict_label = None;
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
            ShortcutsKey::Char(c) => {
                self.filter.insert_char(c);
                self.selected = 0;
                self.scroll_offset = 0;
                ShortcutsAction::None
            }
            ShortcutsKey::Backspace => {
                self.filter.backspace();
                self.selected = 0;
                self.scroll_offset = 0;
                ShortcutsAction::None
            }
            ShortcutsKey::RecordedChord(_) => ShortcutsAction::None,
        }
    }

    /// Ensure the selected item is visible within the scroll window
    fn ensure_visible(&mut self, _selectable: usize) {
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + self.max_visible {
            self.scroll_offset = self.selected + 1 - self.max_visible;
        }
    }

    /// Scroll up by n lines (for mouse wheel)
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Scroll down by n lines (for mouse wheel)
    pub fn scroll_down(&mut self, n: usize, registry: &ShortcutRegistry) {
        let entries = self.build_entries(registry);
        let selectable = self.selectable_count(&entries);
        let max = selectable.saturating_sub(self.max_visible);
        self.scroll_offset = (self.scroll_offset + n).min(max);
    }

    /// Build display info for the renderer
    pub fn build_info(&self, registry: &ShortcutRegistry) -> ShortcutsPanelInfo {
        let all_entries = self.build_entries(registry);
        let selectable = self.selectable_count(&all_entries);

        // Build the visible slice: we need to map selectable indices to entry indices,
        // including headers that fall within the visible range
        let mut visible = Vec::new();
        let mut selectable_idx = 0;
        let visible_end = (self.scroll_offset + self.max_visible).min(selectable);

        for entry in &all_entries {
            if entry.is_header {
                // Include header if the next selectable item after it is in visible range
                // We do this by checking if we're in the visible selectable range
                if selectable_idx >= self.scroll_offset && selectable_idx < visible_end {
                    visible.push(entry.clone());
                }
            } else {
                if selectable_idx >= self.scroll_offset && selectable_idx < visible_end {
                    visible.push(entry.clone());
                }
                selectable_idx += 1;
            }
        }

        // Selected index relative to visible selectable items
        let relative_selected = self.selected.saturating_sub(self.scroll_offset);

        ShortcutsPanelInfo {
            animation: self.animation.eased(),
            filter: self.filter.text().to_string(),
            cursor: self.filter.cursor(),
            entries: visible,
            selected: relative_selected,
            scroll_offset: self.scroll_offset,
            total_entries: selectable,
            recording: self.recording,
            conflict_label: self.conflict_label.clone(),
        }
    }

    /// Update max_visible based on window geometry (called during render)
    pub fn set_max_visible(&mut self, max: usize) {
        self.max_visible = max.max(1);
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
    fn test_filter() {
        let mut panel = ShortcutsPanel::new();
        let registry = ShortcutRegistry::new();

        panel.open();
        let all = panel.build_entries(&registry);
        let all_count = all.len();

        // Type a filter
        panel.handle_key(ShortcutsKey::Char('c'), &registry);
        panel.handle_key(ShortcutsKey::Char('o'), &registry);
        panel.handle_key(ShortcutsKey::Char('p'), &registry);
        panel.handle_key(ShortcutsKey::Char('y'), &registry);
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
        panel.handle_key(ShortcutsKey::Char('t'), &registry);
        assert!(!panel.filter.text().is_empty());

        // First escape clears filter
        panel.handle_key(ShortcutsKey::Escape, &registry);
        assert!(panel.filter.text().is_empty());
        assert!(panel.is_active()); // Still open

        // Second escape closes
        panel.handle_key(ShortcutsKey::Escape, &registry);
        // close() was called but animation may still be visible
    }
}
