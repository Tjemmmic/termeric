//! Command palette — centered modal for command discovery and execution
//!
//! A world-class command palette with fuzzy search, category grouping,
//! keycap-styled shortcuts, and plugin integration.

use sublime_fuzzy::FuzzySearch;

use super::super::animation::OverlayAnimation;
use super::super::text_input::TextInput;
use crate::input::keybindings::{BindableAction, ShortcutRegistry};

/// Source of a registered command
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSource {
    /// Built-in core command
    Core,
    /// User-configured reusable terminal profiles.
    LaunchProfiles,
    /// From a plugin
    Plugin(String),
}

/// A command registered in the palette
#[derive(Debug, Clone)]
pub struct RegisteredCommand {
    /// Unique identifier (e.g., "core.new-tab", "claude-code.open")
    pub id: String,
    /// Display title (e.g., "New Tab")
    pub title: String,
    /// Category for grouping (e.g., "Core", "AI", "Config")
    pub category: String,
    /// Human-readable shortcut (e.g., "Ctrl+T")
    pub shortcut: Option<String>,
    /// Unicode icon character
    pub icon: Option<char>,
    /// Where this command came from
    pub source: CommandSource,
    /// Whether this command is destructive (amber highlight)
    pub destructive: bool,
}

/// A filtered/scored entry for display
#[derive(Debug, Clone)]
pub struct PaletteEntry {
    /// Index into the registry
    pub registry_index: usize,
    /// Fuzzy search score (higher = better match)
    pub score: i64,
    /// Character positions in the title that matched the query
    pub match_positions: Vec<usize>,
}

/// Result of executing a palette command
#[derive(Debug, Clone)]
pub enum PaletteResult {
    /// Run a core action by ID
    RunAction(String),
    /// Run a plugin command by ID
    RunPluginCommand(String),
    /// No-op (cancelled)
    Cancelled,
}

/// Geometry for the centered command palette modal — single source of truth
/// shared by the renderer, click hit-test, and scroll logic.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Several fields are exposed for symmetry / future callers.
pub struct PaletteLayout {
    pub panel_x: f32,
    pub panel_y: f32,
    pub panel_width: f32,
    pub panel_height: f32,
    pub padding: f32,
    pub gap: f32,
    pub separator_height: f32,
    pub row_height: f32,
    pub input_height: f32,
    pub footer_height: f32,
    pub header_area: f32,
    pub footer_area: f32,
    pub list_y: f32,
    pub list_height: f32,
    pub rows_that_fit: usize,
    pub visible_count: usize,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl PaletteLayout {
    pub fn calculate(
        window_width: f32,
        window_height: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
        entry_count: usize,
    ) -> Self {
        let panel_width = (cell_width * 55.0).max(450.0).min(window_width * 0.65);
        let panel_x = (window_width - panel_width) / 2.0;
        let padding = cell_width * 1.5;
        let row_height = cell_height * 2.2;
        let input_height = cell_height * 2.0;
        let footer_height = cell_height * 1.6;
        let gap = 6.0;
        let separator_height = 1.0;
        let header_area = padding + input_height + gap + separator_height + gap;
        let footer_area = footer_height + padding * 0.5;
        let max_panel_height = window_height * 0.75;
        let max_list_height = (max_panel_height - header_area - footer_area).max(row_height);
        let rows_that_fit = (max_list_height / row_height).floor().max(1.0) as usize;
        let visible_count = entry_count.min(rows_that_fit);
        let list_height = visible_count as f32 * row_height;
        let panel_height = header_area + list_height + footer_area;
        let panel_y = (window_height * 0.12) + (1.0 - animation) * -30.0;
        let list_y = panel_y + header_area;
        Self {
            panel_x,
            panel_y,
            panel_width,
            panel_height,
            padding,
            gap,
            separator_height,
            row_height,
            input_height,
            footer_height,
            header_area,
            footer_area,
            list_y,
            list_height,
            rows_that_fit,
            visible_count,
            cell_width,
            cell_height,
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.panel_x
            && x <= self.panel_x + self.panel_width
            && y >= self.panel_y
            && y <= self.panel_y + self.panel_height
    }

    /// Map a y-coordinate to the 0-based visible row, if inside the list area.
    pub fn visible_row_at(&self, y: f32) -> Option<usize> {
        if y < self.list_y || y >= self.list_y + self.list_height {
            return None;
        }
        let row = ((y - self.list_y) / self.row_height) as usize;
        if row < self.visible_count {
            Some(row)
        } else {
            None
        }
    }
}

/// Display snapshot for the renderer
#[derive(Debug, Clone)]
pub struct PaletteInfo {
    /// Optional non-actionable progress/error text in place of the result count.
    pub status: Option<&'static str>,
    /// Current search input
    pub input: String,
    /// Cursor position in input
    pub cursor: usize,
    /// Filtered entries to display
    pub entries: Vec<PaletteEntryInfo>,
    /// Index of selected entry
    pub selected: usize,
    /// Scroll offset for long lists
    pub scroll_offset: usize,
    /// Total number of registered commands
    pub total_count: usize,
    /// Total number of filtered entries (for scrollbar)
    pub total_entries: usize,
    /// Animation progress (0.0 = closed, 1.0 = open)
    pub animation: f32,
    /// Empty-input guidance for this palette's purpose.
    pub placeholder: String,
    /// Singular noun used by the result counter.
    pub item_noun: String,
    /// Label shown beside the Escape key hint.
    pub escape_label: String,
}

/// Display info for a single palette entry
#[derive(Debug, Clone)]
pub struct PaletteEntryInfo {
    /// Stable command identifier used for semantic actions and diagnostics.
    pub id: String,
    pub title: String,
    pub category: String,
    pub shortcut: Option<String>,
    pub icon: Option<char>,
    pub destructive: bool,
    pub match_positions: Vec<usize>,
}

/// The command palette state
pub struct CommandPalette {
    /// Animation state
    animation: OverlayAnimation,
    /// Search input buffer with cursor
    input: TextInput,
    /// All registered commands
    registry: Vec<RegisteredCommand>,
    /// Filtered/scored results
    entries: Vec<PaletteEntry>,
    /// Currently selected entry index
    selected: usize,
    /// Scroll offset for long lists
    scroll_offset: usize,
    /// Recently used command IDs (most recent first)
    recently_used: Vec<String>,
    /// Maximum visible entries
    max_visible: usize,
    placeholder: String,
    item_noun: String,
    escape_label: String,
}

impl CommandPalette {
    /// Create a new command palette
    pub fn new() -> Self {
        Self {
            animation: OverlayAnimation::new(),
            input: TextInput::new(),
            registry: Vec::new(),
            entries: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            recently_used: Vec::new(),
            max_visible: 9,
            placeholder: "Search commands...".into(),
            item_noun: "command".into(),
            escape_label: "Close".into(),
        }
    }

    /// Customize the shared palette chrome for another bounded selection
    /// workflow while retaining identical keyboard/mouse geometry.
    pub fn set_presentation(&mut self, placeholder: &str, item_noun: &str, escape_label: &str) {
        self.placeholder = placeholder.to_string();
        self.item_noun = item_noun.to_string();
        self.escape_label = escape_label.to_string();
    }

    /// Register commands, replacing any existing ones from the same source
    pub fn register_commands(&mut self, commands: Vec<RegisteredCommand>) {
        // Remove commands from this source if re-registering
        if let Some(first) = commands.first() {
            let source = &first.source;
            self.registry.retain(|c| &c.source != source);
        }
        self.registry.extend(commands);
        // Re-sort by visible category/title across core, profile, and plugin sources.
        self.registry.sort_by(|a, b| {
            a.category
                .cmp(&b.category)
                .then_with(|| a.title.cmp(&b.title))
        });
    }

    /// Replace the complete registry for a dedicated palette instance.
    pub fn replace_all_commands(&mut self, mut commands: Vec<RegisteredCommand>) {
        commands.sort_by(|a, b| {
            a.category
                .cmp(&b.category)
                .then_with(|| a.title.cmp(&b.title))
        });
        self.registry = commands;
        if self.animation.is_active() {
            self.update_entries();
        }
    }

    /// Replace the registry with an already-ranked, bounded result set.
    ///
    /// Dedicated picker workflows can perform domain-specific searching
    /// without forcing the general command palette to retain their complete
    /// backing catalog. The supplied order is authoritative and every command
    /// is exposed as a current match.
    pub fn replace_ranked_results(&mut self, commands: Vec<RegisteredCommand>) {
        self.registry = commands;
        self.entries = (0..self.registry.len())
            .map(|registry_index| PaletteEntry {
                registry_index,
                score: (self.registry.len() - registry_index) as i64,
                match_positions: Vec::new(),
            })
            .collect();
        self.selected = 0;
        self.scroll_offset = 0;
    }

    /// Replace every plugin-owned command while preserving core/theme entries.
    pub fn replace_plugin_commands(&mut self, commands: Vec<RegisteredCommand>) {
        self.registry
            .retain(|command| !matches!(command.source, CommandSource::Plugin(_)));
        self.registry.extend(commands);
        self.registry.sort_by(|a, b| {
            a.category
                .cmp(&b.category)
                .then_with(|| a.title.cmp(&b.title))
        });
        if self.animation.is_active() {
            self.update_entries();
        }
    }

    /// Replace every user-configured terminal-profile command.
    pub fn replace_launch_profile_commands(&mut self, commands: Vec<RegisteredCommand>) {
        self.registry
            .retain(|command| command.source != CommandSource::LaunchProfiles);
        self.registry.extend(commands);
        self.registry.sort_by(|a, b| {
            a.category
                .cmp(&b.category)
                .then_with(|| a.title.cmp(&b.title))
        });
        if self.animation.is_active() {
            self.update_entries();
        }
    }

    /// Open the palette
    pub fn open(&mut self) {
        self.input.clear();
        self.selected = 0;
        self.scroll_offset = 0;
        self.animation.open();
        self.update_entries();
    }

    /// Close the palette immediately (no close animation).
    pub fn close(&mut self) {
        self.animation.close_immediately();
    }

    /// Check if palette is active
    pub fn is_active(&self) -> bool {
        self.animation.is_active()
    }

    /// Stable identifiers in the current registry. Accessibility uses this
    /// borrowed view to retire removed command nodes without cloning the whole
    /// registry into every renderer snapshot.
    pub fn command_ids(&self) -> impl Iterator<Item = &str> {
        self.registry.iter().map(|command| command.id.as_str())
    }

    /// Keep keyboard selection and wheel scrolling aligned with the rows the
    /// shared layout can actually render.
    pub fn set_max_visible(&mut self, max_visible: usize) {
        self.max_visible = max_visible.max(1);
        let max_offset = self.entries.len().saturating_sub(self.max_visible);
        self.scroll_offset = self.scroll_offset.min(max_offset);
        if self.selected < self.scroll_offset {
            self.selected = self.scroll_offset;
        } else if self.selected >= self.scroll_offset + self.max_visible {
            self.selected = self.scroll_offset + self.max_visible - 1;
        }
    }

    /// Replace search text from a semantic text-input action. The same bound
    /// applies to dedicated palette workflows so accessibility cannot create
    /// an unbounded or multiline filter that keyboard input could not express.
    pub fn accessibility_set_input(&mut self, value: &str) -> bool {
        if value.chars().count() > 256 || value.chars().any(char::is_control) {
            return false;
        }
        self.input.replace(value);
        self.selected = 0;
        self.scroll_offset = 0;
        self.update_entries();
        true
    }

    /// Update animation, returns true if still animating
    pub fn update_animation(&mut self) -> bool {
        self.animation.update()
    }

    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.animation.set_reduced_motion(reduced_motion);
    }

    /// Handle a character input
    pub fn handle_char(&mut self, c: char) {
        self.input.insert_char(c);
        self.selected = 0;
        self.scroll_offset = 0;
        self.update_entries();
    }

    /// Handle backspace
    pub fn handle_backspace(&mut self) {
        if self.input.backspace() {
            self.selected = 0;
            self.scroll_offset = 0;
            self.update_entries();
        }
    }

    /// Move selection up
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            // Scroll up if needed
            if self.selected < self.scroll_offset {
                self.scroll_offset = self.selected;
            }
        }
    }

    /// Move selection down
    pub fn select_next(&mut self) {
        if self.selected + 1 < self.entries.len() {
            self.selected += 1;
            // Scroll down if needed
            if self.selected >= self.scroll_offset + self.max_visible {
                self.scroll_offset = self.selected - self.max_visible + 1;
            }
        }
    }

    /// Scroll up by N entries (for mouse wheel)
    pub fn scroll_up(&mut self, n: usize) {
        if self.scroll_offset > 0 {
            self.scroll_offset = self.scroll_offset.saturating_sub(n);
            // Keep selection visible
            if self.selected >= self.scroll_offset + self.max_visible {
                self.selected = self.scroll_offset + self.max_visible - 1;
            }
        }
    }

    /// Scroll down by N entries (for mouse wheel)
    pub fn scroll_down(&mut self, n: usize) {
        let max_offset = self.entries.len().saturating_sub(self.max_visible);
        if self.scroll_offset < max_offset {
            self.scroll_offset = (self.scroll_offset + n).min(max_offset);
            // Keep selection visible
            if self.selected < self.scroll_offset {
                self.selected = self.scroll_offset;
            }
        }
    }

    /// Resolve a click against the modal. Returns a result when it should
    /// drive a state change in the app:
    /// - click outside the panel → `Cancelled` (caller should close)
    /// - click on a visible entry → selects + executes that entry
    /// - click inside the panel but not on an entry → `None` (absorb)
    pub fn handle_click(
        &mut self,
        x: f32,
        y: f32,
        layout: &PaletteLayout,
    ) -> Option<PaletteResult> {
        if !layout.contains(x, y) {
            self.close();
            return Some(PaletteResult::Cancelled);
        }
        if let Some(row) = layout.visible_row_at(y) {
            let abs_idx = self.scroll_offset + row;
            if abs_idx < self.entries.len() {
                self.selected = abs_idx;
                return Some(self.execute());
            }
        }
        None
    }

    /// Execute the currently selected command
    pub fn execute(&mut self) -> PaletteResult {
        if let Some(entry) = self.entries.get(self.selected) {
            let cmd = &self.registry[entry.registry_index];
            let id = cmd.id.clone();
            let source = cmd.source.clone();

            // Track recently used
            self.recently_used.retain(|r| r != &id);
            self.recently_used.insert(0, id.clone());
            self.recently_used.truncate(20);

            self.close();

            match source {
                CommandSource::Plugin(_) => PaletteResult::RunPluginCommand(id),
                CommandSource::Core | CommandSource::LaunchProfiles => PaletteResult::RunAction(id),
            }
        } else {
            self.close();
            PaletteResult::Cancelled
        }
    }

    /// Select a currently filtered command by its stable identifier.
    ///
    /// This is used by accessibility actions, whose target must remain tied to
    /// the announced command rather than to a row that can change after a
    /// filter or registry update.
    pub fn select_command(&mut self, id: &str) -> bool {
        let Some(index) = self.entries.iter().position(|entry| {
            self.registry
                .get(entry.registry_index)
                .is_some_and(|command| command.id == id)
        }) else {
            return false;
        };
        self.selected = index;
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + self.max_visible {
            self.scroll_offset = self.selected + 1 - self.max_visible;
        }
        true
    }

    /// Execute a currently filtered command by stable identifier.
    pub fn execute_command(&mut self, id: &str) -> Option<PaletteResult> {
        self.select_command(id).then(|| self.execute())
    }

    /// Build display info for the renderer
    pub fn build_info(&self) -> PaletteInfo {
        let visible_start = self.scroll_offset;
        let visible_end = (self.scroll_offset + self.max_visible).min(self.entries.len());

        let entries = self.entries[visible_start..visible_end]
            .iter()
            .map(|e| {
                let cmd = &self.registry[e.registry_index];
                PaletteEntryInfo {
                    id: cmd.id.clone(),
                    title: cmd.title.clone(),
                    category: cmd.category.clone(),
                    shortcut: cmd.shortcut.clone(),
                    icon: cmd.icon,
                    destructive: cmd.destructive,
                    match_positions: e.match_positions.clone(),
                }
            })
            .collect();

        PaletteInfo {
            status: None,
            input: self.input.text().to_string(),
            cursor: self.input.cursor(),
            entries,
            selected: self.selected.saturating_sub(self.scroll_offset),
            scroll_offset: self.scroll_offset,
            total_count: self.registry.len(),
            total_entries: self.entries.len(),
            animation: self.animation.eased(),
            placeholder: self.placeholder.clone(),
            item_noun: self.item_noun.clone(),
            escape_label: self.escape_label.clone(),
        }
    }

    /// Update filtered entries based on current input
    fn update_entries(&mut self) {
        self.entries.clear();

        if self.input.text().is_empty() {
            // Show all commands, recently used first
            let mut scored: Vec<PaletteEntry> = self
                .registry
                .iter()
                .enumerate()
                .map(|(i, cmd)| {
                    let recency_bonus = self
                        .recently_used
                        .iter()
                        .position(|r| r == &cmd.id)
                        .map(|pos| 1_000_000 - (pos as i64 * 10_000))
                        .unwrap_or(0);
                    PaletteEntry {
                        registry_index: i,
                        score: recency_bonus,
                        match_positions: Vec::new(),
                    }
                })
                .collect();
            scored.sort_by_key(|entry| std::cmp::Reverse(entry.score));
            self.entries = scored;
        } else {
            let query = self.input.text();

            let mut scored: Vec<PaletteEntry> = self
                .registry
                .iter()
                .enumerate()
                .filter_map(|(i, cmd)| {
                    // Try matching against title
                    let title_match = FuzzySearch::new(query, &cmd.title).best_match();

                    // Try matching against category
                    let category_match = FuzzySearch::new(query, &cmd.category).best_match();

                    // Try matching against shortcut
                    let shortcut_match = cmd
                        .shortcut
                        .as_ref()
                        .and_then(|s| FuzzySearch::new(query, s).best_match());

                    // Try matching against combined "title category"
                    let combined = format!("{} {}", cmd.title, cmd.category);
                    let combined_match = FuzzySearch::new(query, &combined).best_match();

                    // Exact prefix match on title gets massive bonus
                    let title_lower = cmd.title.to_lowercase();
                    let query_lower = query.to_lowercase();
                    let prefix_bonus = if title_lower.starts_with(&query_lower) {
                        500_000 + query.len() as i64 * 1000
                    } else {
                        0
                    };

                    // Take best score
                    let best = [
                        title_match
                            .as_ref()
                            .map(|m| (m.score(), m.matched_indices().copied().collect::<Vec<_>>())),
                        category_match.as_ref().map(|m| (m.score(), Vec::new())),
                        shortcut_match.as_ref().map(|m| (m.score(), Vec::new())),
                        combined_match
                            .as_ref()
                            .map(|m| (m.score(), m.matched_indices().copied().collect::<Vec<_>>())),
                    ];

                    let (score, positions) = best
                        .iter()
                        .filter_map(|x| x.as_ref())
                        .max_by_key(|(s, _)| *s)?;

                    // Recency bonus (smaller than search score)
                    let recency_bonus = self
                        .recently_used
                        .iter()
                        .position(|r| r == &cmd.id)
                        .map(|pos| 100 - pos as i64)
                        .unwrap_or(0);

                    Some(PaletteEntry {
                        registry_index: i,
                        score: *score as i64 + prefix_bonus + recency_bonus,
                        match_positions: positions.to_vec(),
                    })
                })
                .collect();

            scored.sort_by_key(|entry| std::cmp::Reverse(entry.score));
            self.entries = scored;
        }
    }
}

/// Build the default set of core commands using the active shortcut registry,
/// so command-palette hints stay correct after user overrides.
pub fn core_commands(
    harvest_enabled: bool,
    shortcuts: &ShortcutRegistry,
) -> Vec<RegisteredCommand> {
    let shortcut = |action| {
        shortcuts
            .binding_for(action)
            .map(|chord| chord.display_string())
    };
    let mut commands = vec![
        // ── Core ──
        RegisteredCommand {
            id: "core.new-window".into(),
            title: "New Window".into(),
            category: "Core".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.toggle-fullscreen".into(),
            title: "Toggle Fullscreen".into(),
            category: "Core".into(),
            shortcut: shortcut(BindableAction::ToggleFullscreen),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.attach-domain".into(),
            title: "Attach Live Domain...".into(),
            category: "Core".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.cancel-file-upload".into(),
            title: "Cancel Upload".into(),
            category: "Core".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: true,
        },
        RegisteredCommand {
            id: "core.new-tab".into(),
            title: "New Tab".into(),
            category: "Core".into(),
            shortcut: shortcut(BindableAction::NewTab),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.close-tab".into(),
            title: "Close Tab".into(),
            category: "Core".into(),
            shortcut: shortcut(BindableAction::CloseTab),
            icon: None,
            source: CommandSource::Core,
            destructive: true,
        },
        RegisteredCommand {
            id: "core.next-tab".into(),
            title: "Next Tab".into(),
            category: "Core".into(),
            shortcut: shortcut(BindableAction::NextTab),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.prev-tab".into(),
            title: "Previous Tab".into(),
            category: "Core".into(),
            shortcut: shortcut(BindableAction::PrevTab),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        // ── Search ──
        RegisteredCommand {
            id: "core.find".into(),
            title: "Find".into(),
            category: "Search".into(),
            shortcut: shortcut(BindableAction::ToggleSearch),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.copy-mode".into(),
            title: "Keyboard Copy Mode".into(),
            category: "Search".into(),
            shortcut: shortcut(BindableAction::ToggleCopyMode),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.quick-select".into(),
            title: "Quick Select Links and Matches".into(),
            category: "Search".into(),
            shortcut: shortcut(BindableAction::QuickSelect),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.unicode-picker".into(),
            title: "Insert Unicode Character...".into(),
            category: "Search".into(),
            shortcut: shortcut(BindableAction::OpenUnicodePicker),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        // ── Config ──
        RegisteredCommand {
            id: "core.settings".into(),
            title: "Open Settings".into(),
            category: "Config".into(),
            shortcut: shortcut(BindableAction::ToggleSettings),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.shortcuts".into(),
            title: "Keyboard Shortcuts".into(),
            category: "Config".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.save-config".into(),
            title: "Save Configuration".into(),
            category: "Config".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.reload-config".into(),
            title: "Reload Configuration".into(),
            category: "Config".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.reset-config".into(),
            title: "Reset Configuration to Defaults".into(),
            category: "Config".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: true,
        },
        // ── Workspaces ──
        RegisteredCommand {
            id: "core.save-workspace".into(),
            title: "Save Workspace".into(),
            category: "Workspaces".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.workspace-manager".into(),
            title: "Workspace Manager".into(),
            category: "Workspaces".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        // ── Legacy single-terminal snapshots ──
        RegisteredCommand {
            id: "core.save-session".into(),
            title: "Save Open Tabs as Legacy Snapshots".into(),
            category: "Legacy Snapshots".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.session-manager".into(),
            title: "Legacy Snapshot Manager".into(),
            category: "Legacy Snapshots".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        // ── Plugins ──
        RegisteredCommand {
            id: "core.plugins".into(),
            title: "Configure Plugins".into(),
            category: "Config".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        // ── Help ──
        RegisteredCommand {
            id: "core.version".into(),
            title: "Show Version".into(),
            category: "Help".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.about".into(),
            title: "About Termeric".into(),
            category: "Help".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        // ── Font ──
        RegisteredCommand {
            id: "core.font-size-increase".into(),
            title: "Increase Font Size".into(),
            category: "Font".into(),
            shortcut: shortcut(BindableAction::IncreaseFontSize),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.font-size-decrease".into(),
            title: "Decrease Font Size".into(),
            category: "Font".into(),
            shortcut: shortcut(BindableAction::DecreaseFontSize),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.font-size-reset".into(),
            title: "Reset Font Size".into(),
            category: "Font".into(),
            shortcut: shortcut(BindableAction::ResetFontSize),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        // ── Export ──
        RegisteredCommand {
            id: "core.export-session".into(),
            title: "Export Session".into(),
            category: "Sessions".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        // ── Copy/Paste ──
        RegisteredCommand {
            id: "core.copy".into(),
            title: "Copy Selection".into(),
            category: "Core".into(),
            shortcut: shortcut(BindableAction::Copy),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.paste".into(),
            title: "Paste from Clipboard".into(),
            category: "Core".into(),
            shortcut: shortcut(BindableAction::Paste),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
    ];

    // Harvest is an experimental beta and stays out of the palette unless
    // explicitly enabled in configuration.
    if harvest_enabled {
        commands.push(RegisteredCommand {
            id: "core.harvest-cellar".into(),
            title: "Show Harvest Cellar".into(),
            category: "Harvest".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        });
        commands.push(RegisteredCommand {
            id: "core.show-sprout-menu".into(),
            title: "Show Sprout Menu".into(),
            category: "Harvest".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        });
        commands.push(RegisteredCommand {
            id: "core.hide-sprout-menu".into(),
            title: "Hide Sprout Menu".into(),
            category: "Harvest".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        });
        commands.push(RegisteredCommand {
            id: "core.clear-harvest".into(),
            title: "Clear Harvest History".into(),
            category: "Harvest".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: true,
        });
    }
    // ── Transcript ──
    commands.push(RegisteredCommand {
        id: "core.toggle-transcript".into(),
        title: "Toggle Transcript Recording".into(),
        category: "Sessions".into(),
        shortcut: None,
        icon: None,
        source: CommandSource::Core,
        destructive: false,
    });

    // ── Panes ──
    commands.push(RegisteredCommand {
        id: "core.split-vertical".into(),
        title: "Split Pane Right".into(),
        category: "Panes".into(),
        shortcut: shortcut(BindableAction::SplitVertical),
        icon: None,
        source: CommandSource::Core,
        destructive: false,
    });
    commands.push(RegisteredCommand {
        id: "core.split-horizontal".into(),
        title: "Split Pane Down".into(),
        category: "Panes".into(),
        shortcut: shortcut(BindableAction::SplitHorizontal),
        icon: None,
        source: CommandSource::Core,
        destructive: false,
    });
    commands.push(RegisteredCommand {
        id: "core.close-pane".into(),
        title: "Close Pane".into(),
        category: "Panes".into(),
        shortcut: shortcut(BindableAction::ClosePane),
        icon: None,
        source: CommandSource::Core,
        destructive: false,
    });
    for (id, title, action) in [
        (
            "core.navigate-pane-up",
            "Navigate Pane Up",
            BindableAction::NavigatePaneUp,
        ),
        (
            "core.navigate-pane-down",
            "Navigate Pane Down",
            BindableAction::NavigatePaneDown,
        ),
        (
            "core.navigate-pane-left",
            "Navigate Pane Left",
            BindableAction::NavigatePaneLeft,
        ),
        (
            "core.navigate-pane-right",
            "Navigate Pane Right",
            BindableAction::NavigatePaneRight,
        ),
    ] {
        commands.push(RegisteredCommand {
            id: id.into(),
            title: title.into(),
            category: "Panes".into(),
            shortcut: shortcut(action),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        });
    }
    for (id, title, action, destructive) in [
        (
            "core.rename-tab",
            "Rename Tab",
            BindableAction::RenameTab,
            false,
        ),
        (
            "core.move-tab-left",
            "Move Tab Left",
            BindableAction::MoveTabLeft,
            false,
        ),
        (
            "core.move-tab-right",
            "Move Tab Right",
            BindableAction::MoveTabRight,
            false,
        ),
        (
            "core.move-tab-next-window",
            "Move Tab to Next Window",
            BindableAction::MoveTabToNextWindow,
            false,
        ),
        (
            "core.toggle-pin-tab",
            "Pin or Unpin Tab",
            BindableAction::TogglePinTab,
            false,
        ),
        (
            "core.toggle-pane-zoom",
            "Zoom Pane",
            BindableAction::TogglePaneZoom,
            false,
        ),
        (
            "core.toggle-pane-read-only",
            "Toggle Pane Read-only",
            BindableAction::TogglePaneReadOnly,
            false,
        ),
        (
            "core.swap-pane-next",
            "Swap Pane with Next",
            BindableAction::SwapPaneNext,
            false,
        ),
        (
            "core.balance-panes",
            "Balance Panes",
            BindableAction::BalancePanes,
            false,
        ),
        (
            "core.move-pane-next-tab",
            "Move Pane to Next Tab",
            BindableAction::MovePaneToNextTab,
            false,
        ),
        (
            "core.move-pane-next-window",
            "Move Pane to Next Window",
            BindableAction::MovePaneToNextWindow,
            false,
        ),
        (
            "core.layout-columns",
            "Layout: Columns",
            BindableAction::LayoutColumns,
            false,
        ),
        (
            "core.layout-rows",
            "Layout: Rows",
            BindableAction::LayoutRows,
            false,
        ),
        (
            "core.layout-grid",
            "Layout: Grid",
            BindableAction::LayoutGrid,
            false,
        ),
        (
            "core.layout-tall",
            "Layout: Tall",
            BindableAction::LayoutTall,
            false,
        ),
        (
            "core.layout-fat",
            "Layout: Fat",
            BindableAction::LayoutFat,
            false,
        ),
        (
            "core.cycle-pane-cwd",
            "Cycle New Pane Directory",
            BindableAction::CyclePaneCwdMode,
            false,
        ),
        (
            "core.toggle-broadcast",
            "Toggle Broadcast Input",
            BindableAction::ToggleBroadcast,
            true,
        ),
    ] {
        commands.push(RegisteredCommand {
            id: id.into(),
            title: title.into(),
            category: if id.contains("tab") && !id.contains("pane") {
                "Tabs".into()
            } else {
                "Panes".into()
            },
            shortcut: shortcut(action),
            icon: None,
            source: CommandSource::Core,
            destructive,
        });
    }

    // ── Theme commands — generated dynamically from all available themes ──
    for name in crate::config::theme::Theme::list_available() {
        let display = theme_display_name(name);
        commands.push(RegisteredCommand {
            id: format!("core.theme.{}", name),
            title: format!("Theme: {}", display),
            category: "Theme".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        });
    }

    // ── Custom themes from user's themes directory ──
    for name in crate::config::theme::Theme::list_custom() {
        let display = theme_display_name(&name);
        commands.push(RegisteredCommand {
            id: format!("core.theme.{}", name),
            title: format!("Theme: {} (Custom)", display),
            category: "Theme".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        });
    }

    commands
}

/// Convert a theme slug (e.g., "catppuccin-mocha") to display name ("Catppuccin Mocha")
fn theme_display_name(slug: &str) -> String {
    slug.split('-')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => format!("{}{}", c.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
impl CommandPalette {
    fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_palette() -> CommandPalette {
        let mut palette = CommandPalette::new();
        palette.register_commands(core_commands(true, &ShortcutRegistry::new()));
        palette
    }

    #[test]
    fn test_open_shows_all_commands() {
        let mut palette = test_palette();
        palette.open();
        assert!(palette.is_active());
        assert_eq!(palette.entry_count(), palette.registry.len());
    }

    #[test]
    fn equal_scores_preserve_registration_order_before_and_after_recency_ranking() {
        let mut palette = CommandPalette::new();
        palette.register_commands(
            ["third", "first", "second"]
                .into_iter()
                .map(|id| RegisteredCommand {
                    id: id.into(),
                    title: "Same Command".into(),
                    category: "Test".into(),
                    shortcut: None,
                    icon: None,
                    source: CommandSource::Core,
                    destructive: false,
                })
                .collect(),
        );
        palette.open();
        for query in ["", "Same"] {
            assert!(palette.accessibility_set_input(query));
            let info = palette.build_info();
            assert_eq!(
                info.entries
                    .iter()
                    .map(|entry| entry.id.as_str())
                    .collect::<Vec<_>>(),
                ["third", "first", "second"]
            );
        }
        palette.recently_used.push("second".into());
        for query in ["", "Same"] {
            assert!(palette.accessibility_set_input(query));
            let info = palette.build_info();
            assert_eq!(
                info.entries
                    .iter()
                    .map(|entry| entry.id.as_str())
                    .collect::<Vec<_>>(),
                ["second", "third", "first"]
            );
        }
    }

    #[test]
    fn harvest_commands_are_hidden_until_enabled() {
        let registry = ShortcutRegistry::new();
        let disabled = core_commands(false, &registry);
        assert!(disabled.iter().all(|command| command.category != "Harvest"));
        let enabled = core_commands(true, &registry);
        assert!(enabled.iter().any(|command| command.category == "Harvest"));
    }

    #[test]
    fn manual_session_save_is_discoverable() {
        let commands = core_commands(false, &ShortcutRegistry::new());
        assert!(commands.iter().any(|command| {
            command.id == "core.save-session"
                && command.title == "Save Open Tabs as Legacy Snapshots"
                && command.category == "Legacy Snapshots"
        }));
    }

    #[test]
    fn live_domain_picker_is_discoverable() {
        let commands = core_commands(false, &ShortcutRegistry::new());
        assert!(commands.iter().any(|command| {
            command.id == "core.attach-domain" && command.title == "Attach Live Domain..."
        }));
    }

    #[test]
    fn active_file_upload_cancellation_is_discoverable() {
        let commands = core_commands(false, &ShortcutRegistry::new());
        assert!(commands.iter().any(|command| {
            command.id == "core.cancel-file-upload"
                && command.title == "Cancel Upload"
                && command.destructive
        }));
    }

    #[test]
    fn complete_workspace_save_and_manager_are_discoverable() {
        let commands = core_commands(false, &ShortcutRegistry::new());
        assert!(commands.iter().any(|command| {
            command.id == "core.save-workspace"
                && command.title == "Save Workspace"
                && command.category == "Workspaces"
        }));
        assert!(commands.iter().any(|command| {
            command.id == "core.workspace-manager"
                && command.title == "Workspace Manager"
                && command.category == "Workspaces"
        }));
    }

    #[test]
    fn window_controls_are_discoverable() {
        let commands = core_commands(false, &ShortcutRegistry::new());
        assert!(
            commands
                .iter()
                .any(|command| command.id == "core.new-window" && command.title == "New Window")
        );
        assert!(commands.iter().any(|command| {
            command.id == "core.toggle-fullscreen"
                && command.title == "Toggle Fullscreen"
                && command.shortcut.as_deref() == Some("F11")
        }));
        assert!(commands.iter().any(|command| {
            command.id == "core.move-tab-next-window"
                && command.title == "Move Tab to Next Window"
                && command.category == "Tabs"
        }));
        assert!(commands.iter().any(|command| {
            command.id == "core.move-pane-next-window"
                && command.title == "Move Pane to Next Window"
                && command.category == "Panes"
        }));
    }

    #[test]
    fn pane_commands_show_their_default_shortcuts() {
        let commands = core_commands(false, &ShortcutRegistry::new());
        let pane_commands: Vec<_> = commands
            .iter()
            .filter(|command| command.category == "Panes")
            .collect();

        assert_eq!(pane_commands.len(), 20);
        assert_eq!(
            pane_commands
                .iter()
                .filter(|command| command.shortcut.is_some())
                .count(),
            7
        );
        assert!(pane_commands.iter().any(|command| {
            command.id == "core.split-vertical"
                && command.shortcut.as_deref() == Some("Alt+Shift+Right")
        }));
        for id in [
            "core.toggle-pane-zoom",
            "core.toggle-pane-read-only",
            "core.swap-pane-next",
            "core.balance-panes",
            "core.move-pane-next-tab",
            "core.move-pane-next-window",
            "core.layout-columns",
            "core.layout-rows",
            "core.layout-grid",
            "core.layout-tall",
            "core.layout-fat",
            "core.cycle-pane-cwd",
            "core.toggle-broadcast",
        ] {
            assert!(pane_commands.iter().any(|command| command.id == id));
        }
    }

    #[test]
    fn command_shortcut_hints_follow_user_overrides() {
        let mut registry = ShortcutRegistry::new();
        registry.set_binding(
            BindableAction::SplitVertical,
            Some(crate::input::keybindings::KeyChord::new(
                "z", true, true, false, false,
            )),
        );

        let commands = core_commands(false, &registry);
        let split = commands
            .iter()
            .find(|command| command.id == "core.split-vertical")
            .unwrap();
        assert_eq!(split.shortcut.as_deref(), Some("Ctrl+Alt+Z"));
    }

    #[test]
    fn test_close() {
        let mut palette = test_palette();
        palette.open();
        palette.close();
        assert!(!palette.is_active());
    }

    #[test]
    fn test_fuzzy_search_filters() {
        let mut palette = test_palette();
        palette.open();
        palette.handle_char('t');
        palette.handle_char('a');
        palette.handle_char('b');
        // Should match "New Tab", "Close Tab", etc.
        assert!(palette.entry_count() > 0);
        assert!(palette.entry_count() < palette.registry.len());
    }

    #[test]
    fn test_prefix_match_scores_highest() {
        let mut palette = test_palette();
        palette.open();
        palette.handle_char('N');
        palette.handle_char('e');
        palette.handle_char('w');
        // "New Tab" should be first
        let info = palette.build_info();
        assert!(info.entries[0].title.starts_with("New"));
    }

    #[test]
    fn test_navigation() {
        let mut palette = test_palette();
        palette.open();
        assert_eq!(palette.selected, 0);
        palette.select_next();
        assert_eq!(palette.selected, 1);
        palette.select_prev();
        assert_eq!(palette.selected, 0);
        // Can't go below 0
        palette.select_prev();
        assert_eq!(palette.selected, 0);
    }

    #[test]
    fn test_backspace() {
        let mut palette = test_palette();
        palette.open();
        let initial_count = palette.entry_count();
        palette.handle_char('x');
        palette.handle_char('x');
        let filtered_count = palette.entry_count();
        assert!(filtered_count < initial_count);
        palette.handle_backspace();
        palette.handle_backspace();
        assert_eq!(palette.entry_count(), initial_count);
    }

    #[test]
    fn test_execute_tracks_recently_used() {
        let mut palette = test_palette();
        palette.open();
        let result = palette.execute();
        assert!(matches!(result, PaletteResult::RunAction(_)));
        assert!(!palette.recently_used.is_empty());
    }

    #[test]
    fn test_recently_used_bubbles_to_top() {
        let mut palette = test_palette();

        // Execute "About Termeric" first
        palette.open();
        // Find "About" and select it
        palette.handle_char('A');
        palette.handle_char('b');
        palette.handle_char('o');
        palette.handle_char('u');
        palette.handle_char('t');
        let _ = palette.execute();

        // Reopen — "About Termeric" should be first when no query
        palette.open();
        let info = palette.build_info();
        assert_eq!(info.entries[0].title, "About Termeric");
    }

    #[test]
    fn test_plugin_commands_register() {
        let mut palette = test_palette();
        let plugin_cmds = vec![RegisteredCommand {
            id: "claude-code.open".into(),
            title: "Open Claude Code".into(),
            category: "AI".into(),
            shortcut: Some("Ctrl+Shift+I".into()),
            icon: None,
            source: CommandSource::Plugin("claude-code".into()),
            destructive: false,
        }];
        palette.register_commands(plugin_cmds);
        palette.open();
        palette.handle_char('c');
        palette.handle_char('l');
        palette.handle_char('a');
        // Should find claude command
        let info = palette.build_info();
        assert!(info.entries.iter().any(|e| e.title.contains("Claude")));
    }

    #[test]
    fn replace_plugin_commands_removes_disabled_plugin_sources() {
        let mut palette = CommandPalette::new();
        palette.register_commands(core_commands(true, &ShortcutRegistry::new()));
        palette.replace_plugin_commands(vec![RegisteredCommand {
            id: "codex.open".into(),
            title: "Open Codex".into(),
            category: "Plugins".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Plugin("codex".into()),
            destructive: false,
        }]);
        assert!(
            palette
                .registry
                .iter()
                .any(|command| command.id == "codex.open")
        );

        palette.replace_plugin_commands(Vec::new());
        assert!(
            !palette
                .registry
                .iter()
                .any(|command| matches!(command.source, CommandSource::Plugin(_)))
        );
        assert!(
            palette
                .registry
                .iter()
                .any(|command| command.id == "core.new-tab")
        );
    }

    #[test]
    fn terminal_profile_commands_replace_cleanly_and_execute_as_app_actions() {
        let mut palette = CommandPalette::new();
        palette.register_commands(core_commands(true, &ShortcutRegistry::new()));
        palette.replace_launch_profile_commands(vec![RegisteredCommand {
            id: "profile.open.dev".into(),
            title: "New Tab: dev".into(),
            category: "Terminal Profiles".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::LaunchProfiles,
            destructive: false,
        }]);
        palette.open();
        palette.handle_char('d');
        palette.handle_char('e');
        palette.handle_char('v');
        assert!(matches!(
            palette.execute(),
            PaletteResult::RunAction(id) if id == "profile.open.dev"
        ));

        palette.replace_launch_profile_commands(Vec::new());
        assert!(
            !palette
                .registry
                .iter()
                .any(|command| command.source == CommandSource::LaunchProfiles)
        );
        assert!(
            palette
                .registry
                .iter()
                .any(|command| command.id == "core.new-tab")
        );
    }

    #[test]
    fn test_execute_plugin_returns_plugin_result() {
        let mut palette = CommandPalette::new();
        palette.register_commands(vec![RegisteredCommand {
            id: "test-plugin.action".into(),
            title: "Test Action".into(),
            category: "Test".into(),
            shortcut: None,
            icon: None,
            source: CommandSource::Plugin("test".into()),
            destructive: false,
        }]);
        palette.open();
        let result = palette.execute();
        assert!(matches!(result, PaletteResult::RunPluginCommand(_)));
    }

    #[test]
    fn test_build_info_snapshot() {
        let mut palette = test_palette();
        palette.open();
        let info = palette.build_info();
        assert!(!info.entries.is_empty());
        assert_eq!(info.total_count, palette.registry.len());
        assert_eq!(info.selected, 0);
    }

    #[test]
    fn test_category_search() {
        let mut palette = test_palette();
        palette.open();
        palette.handle_char('C');
        palette.handle_char('o');
        palette.handle_char('n');
        palette.handle_char('f');
        palette.handle_char('i');
        palette.handle_char('g');
        // Should find Config category commands
        let info = palette.build_info();
        assert!(info.entries.iter().any(|e| e.category == "Config"));
    }

    #[test]
    fn test_shortcut_search() {
        let mut palette = test_palette();
        palette.open();
        // Search by shortcut text
        palette.handle_char('C');
        palette.handle_char('t');
        palette.handle_char('r');
        palette.handle_char('l');
        palette.handle_char('+');
        palette.handle_char('T');
        // Should find commands with Ctrl+T in shortcut
        assert!(palette.entry_count() > 0);
    }
}
