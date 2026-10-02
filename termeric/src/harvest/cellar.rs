//! Root Cellar - Visual command history
//!
//! Displays commands as "roots" growing from a timeline,
//! with branching for piped commands and color-coding by exit status.

use super::HarvestSource;
use std::collections::VecDeque;
use termeric_core::CommandRegion;

/// Default maximum number of commands to keep in history. Overridable per-
/// cellar via [`RootCellar::with_capacity`].
const DEFAULT_MAX_COMMANDS: usize = 100;
pub const MAX_HARVEST_COMMAND_CHARS: usize = 512;
pub const MAX_HARVEST_COMMAND_BYTES: usize = 4 * 1024;
/// Per-entry output snapshot ceiling. At the default 100-entry history this
/// contributes at most 6.25 MiB plus string metadata.
pub const MAX_HARVEST_OUTPUT_BYTES: usize = 64 * 1024;
/// Aggregate owned command/output snapshot ceiling. The default 100-entry cap
/// fits below this even when every output reaches its per-entry limit, while a
/// large configured history cannot grow snapshots without bound.
pub const MAX_HARVEST_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
const UNAVAILABLE_COMMAND_TEXT: &str = "(command text unavailable)";

/// A command entry in the root cellar
#[derive(Debug, Clone)]
pub struct CommandEntry {
    /// Stable identity within the owning cellar.
    pub id: u64,
    /// Stable identity of the exact OSC 133 region in the producing terminal.
    pub region_id: u64,
    /// Bounded one-line command text shown in history.
    pub command: String,
    /// Presented command text, kept independently of the normalized caption.
    /// None is only the pre-retention state of a freshly constructed entry.
    pub command_snapshot: Option<String>,
    /// The selected snapshot is incomplete; never execute it as a command.
    pub command_truncated: bool,
    /// Bounded output snapshot retained after terminal scrollback eviction.
    pub output: String,
    /// Whether the retained output ended at the per-entry byte ceiling.
    pub output_truncated: bool,
    /// Terminal that produced this command.
    pub source: HarvestSource,
    /// Row where the command started
    pub row: i32,
    /// Exit code (None if still running)
    pub exit_code: Option<i32>,
    /// Whether this command was successful
    pub success: bool,
}

impl CommandEntry {
    /// Create a new command entry
    #[cfg(test)]
    pub fn new(row: i32, exit_code: Option<i32>) -> Self {
        Self::new_for_source(
            HarvestSource {
                tab_id: 0,
                pane_id: 0,
            },
            row,
            exit_code,
        )
    }

    pub fn new_for_source(source: HarvestSource, row: i32, exit_code: Option<i32>) -> Self {
        let success = exit_code == Some(0);
        Self {
            id: 0,
            region_id: 0,
            command: String::new(),
            command_snapshot: None,
            command_truncated: false,
            output: String::new(),
            output_truncated: false,
            source,
            row,
            exit_code,
            success,
        }
    }

    /// Get the health of this command (for visual styling)
    /// Returns: Healthy (0), Wilted (1), Running (2)
    pub fn health(&self) -> u8 {
        match self.exit_code {
            Some(0) => 0, // Healthy root
            Some(_) => 1, // Wilted root
            None => 2,    // Still growing
        }
    }

    /// Get color based on health
    /// Returns [r, g, b, a]
    pub fn health_color(&self) -> [f32; 4] {
        match self.health() {
            0 => [0.4, 0.7, 0.3, 1.0], // Healthy green
            1 => [0.7, 0.4, 0.3, 1.0], // Wilted brown
            _ => [0.6, 0.5, 0.4, 0.7], // Growing - muted
        }
    }

    pub fn command_available(&self) -> bool {
        !self.command_text().trim().is_empty() && self.command_text() != UNAVAILABLE_COMMAND_TEXT
    }

    pub fn command_text(&self) -> &str {
        self.command_snapshot.as_deref().unwrap_or(&self.command)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CellarRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl CellarRect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellarAction {
    None,
    Close,
    Select(u64),
    Activate(u64),
}

/// Shared Root Cellar geometry for rendering, pointer actions, and AccessKit.
#[derive(Clone, Debug, PartialEq)]
pub struct CellarLayout {
    pub panel: CellarRect,
    pub close: CellarRect,
    pub summary: CellarRect,
    pub list: CellarRect,
    pub rows: Vec<(u64, CellarRect)>,
    pub scrollbar: CellarRect,
    pub row_height: f32,
    pub max_visible: usize,
}

impl CellarLayout {
    pub fn calculate(
        window_width: f32,
        window_height: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
        visible_ids: impl IntoIterator<Item = u64>,
    ) -> Self {
        let panel_width = (cell_width * 35.0).min(350.0).min(window_width.max(0.0));
        let panel_height = (window_height * 0.6)
            .max(cell_height * 8.0)
            .min(window_height);
        let panel_y = ((window_height - panel_height) / 2.0).max(0.0);
        let eased = 1.0 - (1.0 - animation.clamp(0.0, 1.0)).powi(3);
        let panel_x = -panel_width + panel_width * eased;
        let header_y = panel_y + cell_height * 0.5;
        let separator_y = header_y + cell_height * 2.5;
        let list_y = separator_y + cell_height * 0.5;
        let list_height = (panel_height - (list_y - panel_y) - cell_height).max(0.0);
        let max_visible = (list_height / cell_height).floor() as usize;
        let ids = visible_ids
            .into_iter()
            .take(max_visible)
            .collect::<Vec<_>>();
        let rows = ids
            .iter()
            .enumerate()
            .map(|(index, id)| {
                (
                    *id,
                    CellarRect {
                        x: panel_x + cell_width * 0.5,
                        y: list_y + index as f32 * cell_height,
                        width: (panel_width - cell_width * 1.5).max(0.0),
                        height: cell_height,
                    },
                )
            })
            .collect();
        Self {
            panel: CellarRect {
                x: panel_x,
                y: panel_y,
                width: panel_width,
                height: panel_height,
            },
            close: CellarRect {
                x: panel_x + panel_width - cell_width * 2.0,
                y: header_y,
                width: cell_width,
                height: cell_height,
            },
            summary: CellarRect {
                x: panel_x + cell_width,
                y: header_y + cell_height,
                width: (panel_width - cell_width * 2.0).max(0.0),
                height: cell_height,
            },
            list: CellarRect {
                x: panel_x + cell_width * 0.5,
                y: list_y,
                width: (panel_width - cell_width * 1.5).max(0.0),
                height: list_height,
            },
            rows,
            scrollbar: CellarRect {
                x: panel_x + panel_width - cell_width,
                y: list_y,
                width: 4.0,
                height: list_height,
            },
            row_height: cell_height,
            max_visible,
        }
    }

    pub fn hit_test(&self, x: f32, y: f32, activate: bool) -> CellarAction {
        if self.close.contains(x, y) {
            return CellarAction::Close;
        }
        if let Some((id, _)) = self.rows.iter().find(|(_, row)| row.contains(x, y)) {
            return if activate {
                CellarAction::Activate(*id)
            } else {
                CellarAction::Select(*id)
            };
        }
        if self.panel.contains(x, y) {
            CellarAction::None
        } else {
            CellarAction::Close
        }
    }
}

/// Root Cellar - command history with visual timeline
#[derive(Debug)]
pub struct RootCellar {
    /// Command entries
    entries: VecDeque<CommandEntry>,
    /// Max entries retained before the oldest is evicted
    max_commands: usize,
    /// Number of older entries discarded by the retention cap since clear.
    evicted_commands: u64,
    /// Accounted retained `String` capacity for commands and outputs.
    snapshot_bytes: usize,
    max_snapshot_bytes: usize,
    next_entry_id: u64,
    /// Panel open/close animation — `expanded` reflects target state,
    /// `animation` drives the visual progress.
    pub animation: crate::ui::animation::PanelAnimation,
    /// Scroll position in the cellar view
    pub scroll: usize,
    selected_id: Option<u64>,
    viewport_rows: usize,
}

impl Default for RootCellar {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            max_commands: DEFAULT_MAX_COMMANDS,
            evicted_commands: 0,
            snapshot_bytes: 0,
            max_snapshot_bytes: MAX_HARVEST_SNAPSHOT_BYTES,
            next_entry_id: 1,
            animation: crate::ui::animation::PanelAnimation::new(),
            scroll: 0,
            selected_id: None,
            viewport_rows: 1,
        }
    }
}

impl RootCellar {
    /// Create a new root cellar with the default capacity.
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a cellar with a specific max history size.
    pub fn with_capacity(max_commands: usize) -> Self {
        Self {
            max_commands: max_commands.max(1),
            ..Self::default()
        }
    }

    /// Update the cap. Evicts oldest entries if the new cap is lower.
    pub fn set_max_commands(&mut self, max: usize) {
        self.max_commands = max.max(1);
        while self.entries.len() > self.max_commands {
            let removed = self.evict_oldest();
            if removed == self.selected_id {
                self.selected_id = self.entries.front().map(|entry| entry.id);
            }
        }
        self.clamp_viewport();
    }

    /// Add a command entry from a shell integration region
    pub fn add_from_region(
        &mut self,
        source: HarvestSource,
        region: &CommandRegion,
        command: impl Into<String>,
        command_truncated: bool,
        output: impl Into<String>,
    ) {
        let mut entry = CommandEntry::new_for_source(source, region.prompt_row, region.exit_code);
        entry.region_id = region.id;
        entry.command = command.into();
        entry.command_truncated = command_truncated;
        (entry.output, entry.output_truncated) = bounded_output(output.into());
        self.add(entry);
    }

    /// Add a command entry
    pub fn add(&mut self, entry: CommandEntry) {
        let mut entry = entry;
        let mut snapshot = entry
            .command_snapshot
            .take()
            .unwrap_or_else(|| std::mem::take(&mut entry.command));
        if snapshot.len() > MAX_HARVEST_COMMAND_BYTES {
            let mut end = MAX_HARVEST_COMMAND_BYTES;
            while !snapshot.is_char_boundary(end) {
                end -= 1;
            }
            snapshot.truncate(end);
            entry.command_truncated = true;
        }
        snapshot.shrink_to_fit();
        entry.command = bounded_command(snapshot.clone());
        entry.command_snapshot = Some(snapshot);
        let (output, truncated) = bounded_output(std::mem::take(&mut entry.output));
        entry.output = output;
        entry.output_truncated |= truncated;
        if entry.id == 0 {
            entry.id = self.next_entry_id;
            self.next_entry_id = self.next_entry_id.saturating_add(1);
        }
        // Remove oldest if at capacity
        if self.entries.len() >= self.max_commands {
            let removed = self.evict_oldest();
            if removed == self.selected_id {
                self.selected_id = None;
            }
        }
        let id = entry.id;
        self.snapshot_bytes = self
            .snapshot_bytes
            .saturating_add(entry_snapshot_bytes(&entry));
        self.entries.push_back(entry);
        while self.snapshot_bytes > self.max_snapshot_bytes && self.entries.len() > 1 {
            let removed = self.evict_oldest();
            if removed == self.selected_id {
                self.selected_id = None;
            }
        }
        if self.is_expanded() {
            self.selected_id = Some(id);
            self.reveal_selected();
        }
    }

    /// Get all entries
    pub fn entries(&self) -> impl Iterator<Item = &CommandEntry> {
        self.entries.iter()
    }

    /// Get the most recent entry
    #[cfg(test)]
    pub fn latest(&self) -> Option<&CommandEntry> {
        self.entries.back()
    }

    pub fn latest_for_source(&self, source: HarvestSource) -> Option<&CommandEntry> {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.source == source)
    }

    pub fn entry(&self, id: u64) -> Option<&CommandEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    #[cfg(test)]
    pub fn selected(&self) -> Option<&CommandEntry> {
        self.selected_id.and_then(|id| self.entry(id))
    }

    pub fn selected_id(&self) -> Option<u64> {
        self.selected_id
    }

    pub fn select(&mut self, id: u64) -> bool {
        if self.entry(id).is_none() {
            return false;
        }
        self.selected_id = Some(id);
        self.reveal_selected();
        true
    }

    pub fn select_up(&mut self) {
        let current = self
            .selected_id
            .and_then(|id| self.entries.iter().position(|entry| entry.id == id))
            .unwrap_or_else(|| self.entries.len().saturating_sub(1));
        if let Some(entry) = self.entries.get(current.saturating_sub(1)) {
            self.selected_id = Some(entry.id);
            self.reveal_selected();
        }
    }

    pub fn select_down(&mut self) {
        let current = self
            .selected_id
            .and_then(|id| self.entries.iter().position(|entry| entry.id == id))
            .unwrap_or(0);
        if let Some(entry) = self
            .entries
            .get((current + 1).min(self.entries.len().saturating_sub(1)))
        {
            self.selected_id = Some(entry.id);
            self.reveal_selected();
        }
    }

    pub fn select_first(&mut self) {
        self.selected_id = self.entries.front().map(|entry| entry.id);
        self.reveal_selected();
    }

    pub fn select_last(&mut self) {
        self.selected_id = self.entries.back().map(|entry| entry.id);
        self.reveal_selected();
    }

    pub fn select_page(&mut self, direction: isize) {
        if self.entries.is_empty() {
            return;
        }
        let current = self
            .selected_id
            .and_then(|id| self.entries.iter().position(|entry| entry.id == id))
            .unwrap_or_else(|| self.entries.len().saturating_sub(1));
        let distance = self.viewport_rows.max(1);
        let target = current
            .saturating_add_signed(direction.saturating_mul(distance as isize))
            .min(self.entries.len() - 1);
        self.selected_id = self.entries.get(target).map(|entry| entry.id);
        self.reveal_selected();
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let max_scroll = self.entries.len().saturating_sub(self.viewport_rows.max(1));
        self.scroll = self.scroll.saturating_add_signed(delta).min(max_scroll);
        let rows = self.viewport_rows.max(1);
        let selected_index = self
            .selected_id
            .and_then(|id| self.entries.iter().position(|entry| entry.id == id));
        if selected_index
            .is_none_or(|index| index < self.scroll || index >= self.scroll.saturating_add(rows))
        {
            self.selected_id = self.entries.get(self.scroll).map(|entry| entry.id);
        }
    }

    pub fn set_viewport_rows(&mut self, rows: usize) {
        self.viewport_rows = rows.max(1);
        self.clamp_viewport();
    }

    pub fn visible_entries(&self) -> impl Iterator<Item = &CommandEntry> {
        self.entries
            .iter()
            .skip(self.scroll)
            .take(self.viewport_rows)
    }

    /// Get number of entries
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Number of entries discarded by the configured history cap since the
    /// user last cleared the Cellar.
    pub fn evicted_commands(&self) -> u64 {
        self.evicted_commands
    }

    #[cfg(test)]
    pub fn snapshot_bytes(&self) -> usize {
        self.snapshot_bytes
    }

    /// Clear all entries
    pub fn clear(&mut self) {
        self.entries.clear();
        self.evicted_commands = 0;
        self.snapshot_bytes = 0;
        self.selected_id = None;
        self.scroll = 0;
    }

    /// Toggle expanded view. Drives the slide-in/out animation.
    pub fn toggle_expanded(&mut self) {
        self.animation.toggle();
        if self.animation.is_open() {
            self.selected_id = self.entries.back().map(|entry| entry.id);
            self.reveal_selected();
        }
    }

    pub fn close(&mut self) {
        self.animation.close();
    }

    /// Whether the cellar is logically open (ignoring animation state).
    pub fn is_expanded(&self) -> bool {
        self.animation.is_open()
    }

    /// Whether the cellar should be rendered this frame (open or
    /// currently animating out).
    pub fn is_visible(&self) -> bool {
        self.animation.is_visible()
    }

    /// Update animations (call each frame). Returns true if anything is
    /// still animating (caller should keep scheduling redraws).
    pub fn update(&mut self) -> bool {
        self.animation.needs_redraw()
    }

    /// Get success rate (percentage of successful commands)
    pub fn success_rate(&self) -> f32 {
        if self.entries.is_empty() {
            return 1.0;
        }

        let completed: Vec<_> = self
            .entries
            .iter()
            .filter(|e| e.exit_code.is_some())
            .collect();

        if completed.is_empty() {
            return 1.0;
        }

        let successful = completed.iter().filter(|e| e.success).count();
        successful as f32 / completed.len() as f32
    }

    fn reveal_selected(&mut self) {
        let Some(index) = self
            .selected_id
            .and_then(|id| self.entries.iter().position(|entry| entry.id == id))
        else {
            self.clamp_viewport();
            return;
        };
        let rows = self.viewport_rows.max(1);
        if index < self.scroll {
            self.scroll = index;
        } else if index >= self.scroll.saturating_add(rows) {
            self.scroll = index + 1 - rows;
        }
        self.clamp_viewport();
    }

    fn clamp_viewport(&mut self) {
        self.scroll = self
            .scroll
            .min(self.entries.len().saturating_sub(self.viewport_rows.max(1)));
    }

    fn evict_oldest(&mut self) -> Option<u64> {
        let removed = self.entries.pop_front()?;
        self.snapshot_bytes = self
            .snapshot_bytes
            .saturating_sub(entry_snapshot_bytes(&removed));
        self.evicted_commands = self.evicted_commands.saturating_add(1);
        Some(removed.id)
    }

    #[cfg(test)]
    fn set_snapshot_budget(&mut self, bytes: usize) {
        self.max_snapshot_bytes = bytes.max(1);
        while self.snapshot_bytes > self.max_snapshot_bytes && self.entries.len() > 1 {
            self.evict_oldest();
        }
        self.clamp_viewport();
    }
}

fn entry_snapshot_bytes(entry: &CommandEntry) -> usize {
    entry
        .command
        .capacity()
        .saturating_add(entry.command_snapshot.as_ref().map_or(0, String::capacity))
        .saturating_add(entry.output.capacity())
}

fn bounded_command(command: String) -> String {
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return UNAVAILABLE_COMMAND_TEXT.to_string();
    }
    if normalized.chars().count() <= MAX_HARVEST_COMMAND_CHARS {
        return normalized;
    }
    let mut bounded = normalized
        .chars()
        .take(MAX_HARVEST_COMMAND_CHARS.saturating_sub(1))
        .collect::<String>();
    bounded.push('…');
    bounded
}

fn bounded_output(mut output: String) -> (String, bool) {
    if output.len() <= MAX_HARVEST_OUTPUT_BYTES {
        output.shrink_to_fit();
        return (output, false);
    }
    let mut end = MAX_HARVEST_OUTPUT_BYTES.saturating_sub('…'.len_utf8());
    while !output.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    let mut bounded = output[..end].to_owned();
    bounded.push('…');
    bounded.shrink_to_fit();
    (bounded, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_entry_health() {
        let success = CommandEntry::new(0, Some(0));
        assert_eq!(success.health(), 0);

        let failure = CommandEntry::new(0, Some(1));
        assert_eq!(failure.health(), 1);

        let running = CommandEntry::new(0, None);
        assert_eq!(running.health(), 2);
    }

    #[test]
    fn test_root_cellar_add() {
        let mut cellar = RootCellar::new();
        cellar.add(CommandEntry::new(0, Some(0)));
        cellar.add(CommandEntry::new(5, Some(1)));

        let exit_codes: Vec<_> = cellar.entries().map(|entry| entry.exit_code).collect();
        assert_eq!(exit_codes, [Some(0), Some(1)]);
    }

    #[test]
    fn test_success_rate() {
        let mut cellar = RootCellar::new();
        cellar.add(CommandEntry::new(0, Some(0)));
        cellar.add(CommandEntry::new(5, Some(0)));
        cellar.add(CommandEntry::new(10, Some(1)));

        // 2 successes out of 3
        assert!((cellar.success_rate() - 0.666).abs() < 0.01);
    }

    #[test]
    fn open_selects_and_reveals_the_latest_bounded_command() {
        let mut cellar = RootCellar::with_capacity(3);
        cellar.set_viewport_rows(2);
        for row in 0..4 {
            let mut entry = CommandEntry::new(row, Some(0));
            entry.command = format!("printf   {row}");
            cellar.add(entry);
        }
        cellar.toggle_expanded();
        assert_eq!(cellar.len(), 3);
        assert_eq!(cellar.evicted_commands(), 1);
        assert_eq!(cellar.selected().unwrap().row, 3);
        assert_eq!(cellar.scroll, 1);
        cellar.select_up();
        assert_eq!(cellar.selected().unwrap().row, 2);
        cellar.scroll_by(-50);
        assert_eq!(cellar.scroll, 0);
        cellar.close();
        assert!(!cellar.is_expanded());
    }

    #[test]
    fn retention_evictions_are_counted_for_capacity_rollover_and_cap_changes() {
        let mut cellar = RootCellar::with_capacity(4);
        for row in 0..4 {
            cellar.add(CommandEntry::new(row, Some(0)));
        }
        assert_eq!(cellar.evicted_commands(), 0);
        cellar.add(CommandEntry::new(4, Some(0)));
        assert_eq!(cellar.evicted_commands(), 1);
        cellar.set_max_commands(2);
        assert_eq!(cellar.len(), 2);
        assert_eq!(cellar.evicted_commands(), 3);
        cellar.clear();
        assert_eq!(cellar.evicted_commands(), 0);
    }

    #[test]
    fn aggregate_snapshot_budget_evicts_oldest_entries_and_tracks_owned_capacity() {
        let mut cellar = RootCellar::with_capacity(10);
        cellar.set_snapshot_budget(64);
        for row in 0..4 {
            let mut entry = CommandEntry::new(row, Some(0));
            entry.command = format!("cmd-{row}");
            entry.output = "12345678901234567890".into();
            cellar.add(entry);
            assert!(cellar.snapshot_bytes() <= 64);
        }
        assert!(cellar.len() < 4);
        assert!(cellar.evicted_commands() > 0);
        assert_eq!(cellar.latest().unwrap().row, 3);
    }

    #[test]
    fn command_text_is_normalized_and_bounded() {
        let mut cellar = RootCellar::new();
        let mut tracker = termeric_core::shell::PromptTracker::new();
        tracker.mark_prompt_start(0);
        tracker.mark_command_start(0);
        tracker.mark_output_start(1);
        tracker.mark_command_finished(1, Some(0));
        let region = tracker.completed_region(0).unwrap();
        cellar.add_from_region(
            HarvestSource {
                tab_id: 1,
                pane_id: 0,
            },
            region,
            format!("  echo\n{}  ", "x".repeat(MAX_HARVEST_COMMAND_CHARS + 100)),
            false,
            format!("{}終", "y".repeat(MAX_HARVEST_OUTPUT_BYTES + 100)),
        );
        let entry = cellar.latest().unwrap();
        let command = &entry.command;
        assert_eq!(command.chars().count(), MAX_HARVEST_COMMAND_CHARS);
        assert!(!command.contains('\n'));
        assert!(command.ends_with('…'));
        assert!(entry.output_truncated);
        assert!(entry.output.len() <= MAX_HARVEST_OUTPUT_BYTES);
        assert!(entry.output.capacity() <= MAX_HARVEST_OUTPUT_BYTES);
        assert!(entry.output.ends_with('…'));
    }

    #[test]
    fn direct_add_cannot_bypass_string_or_owned_capacity_limits() {
        let mut cellar = RootCellar::new();
        let mut entry = CommandEntry::new(0, Some(0));
        entry.command = "x".repeat(MAX_HARVEST_COMMAND_CHARS + 50);
        entry.output = String::with_capacity(MAX_HARVEST_OUTPUT_BYTES * 4);
        entry
            .output
            .push_str(&"y".repeat(MAX_HARVEST_OUTPUT_BYTES + 50));
        cellar.add(entry);

        let entry = cellar.latest().unwrap();
        assert_eq!(entry.command.chars().count(), MAX_HARVEST_COMMAND_CHARS);
        assert!(entry.output_truncated);
        assert!(entry.output.capacity() <= MAX_HARVEST_OUTPUT_BYTES);
        assert_eq!(cellar.snapshot_bytes(), entry_snapshot_bytes(entry));
    }

    #[test]
    fn retained_command_snapshot_is_distinct_bounded_and_capacity_accounted() {
        for text in [
            "printf '%s' 'two  spaces'\nnext".to_string(),
            "x".repeat(MAX_HARVEST_COMMAND_BYTES),
            "界".repeat(MAX_HARVEST_COMMAND_BYTES),
        ] {
            let mut cellar = RootCellar::new();
            let mut entry = CommandEntry::new(0, Some(0));
            entry.command = text.clone();
            cellar.add(entry);
            let entry = cellar.latest().unwrap();
            assert!(text.starts_with(entry.command_text()));
            assert!(entry.command_text().len() <= MAX_HARVEST_COMMAND_BYTES);
            assert!(
                entry.command_snapshot.as_ref().unwrap().capacity() <= MAX_HARVEST_COMMAND_BYTES
            );
            assert_eq!(
                entry.command_truncated,
                text.len() > MAX_HARVEST_COMMAND_BYTES
            );
            if !entry.command_truncated {
                assert_eq!(entry.command_text(), text);
            }
            assert_eq!(
                cellar.snapshot_bytes(),
                entry.command.capacity()
                    + entry.command_snapshot.as_ref().unwrap().capacity()
                    + entry.output.capacity()
            );
        }
    }

    #[test]
    fn upstream_truncation_survives_retention_and_budget_eviction_counts_command_snapshots() {
        let mut cellar = RootCellar::with_capacity(100);
        cellar.set_snapshot_budget(MAX_HARVEST_COMMAND_BYTES * 2);
        for id in 0..4 {
            let mut entry = CommandEntry::new(id, Some(0));
            entry.command = "x".repeat(MAX_HARVEST_COMMAND_BYTES);
            entry.command_truncated = true;
            cellar.add(entry);
        }
        assert_eq!(cellar.len(), 1);
        assert_eq!(cellar.evicted_commands(), 3);
        let mut entry = cellar.latest().unwrap().clone();
        entry.command = "edited caption is not action authority".into();
        cellar.add(entry);
        assert!(cellar.latest().unwrap().command_truncated);
        assert_eq!(
            cellar.latest().unwrap().command_text(),
            "x".repeat(MAX_HARVEST_COMMAND_BYTES)
        );
    }

    #[test]
    fn shared_layout_targets_exact_entries_close_and_background() {
        let layout = CellarLayout::calculate(800.0, 600.0, 8.0, 16.0, 1.0, [41_u64, 73_u64]);
        let first = layout.rows[0].1;
        assert_eq!(
            layout.hit_test(first.x + 1.0, first.y + 1.0, false),
            CellarAction::Select(41)
        );
        assert_eq!(
            layout.hit_test(first.x + 1.0, first.y + 1.0, true),
            CellarAction::Activate(41)
        );
        assert_eq!(
            layout.hit_test(layout.close.x + 1.0, layout.close.y + 1.0, true),
            CellarAction::Close
        );
        assert_eq!(layout.hit_test(799.0, 599.0, true), CellarAction::Close);
    }
}
