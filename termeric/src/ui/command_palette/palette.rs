//! Command palette — centered modal for command discovery and execution
//!
//! A world-class command palette with fuzzy search, category grouping,
//! keycap-styled shortcuts, and plugin integration.

use sublime_fuzzy::FuzzySearch;

/// Source of a registered command
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSource {
    /// Built-in core command
    Core,
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

/// Display snapshot for the renderer
#[derive(Debug, Clone)]
pub struct PaletteInfo {
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
}

/// Display info for a single palette entry
#[derive(Debug, Clone)]
pub struct PaletteEntryInfo {
    pub title: String,
    pub category: String,
    pub shortcut: Option<String>,
    pub icon: Option<char>,
    pub destructive: bool,
    pub match_positions: Vec<usize>,
    pub source: CommandSource,
}

/// The command palette state
pub struct CommandPalette {
    /// Whether the palette is currently open
    active: bool,
    /// Search input buffer
    input: String,
    /// Cursor position in input
    cursor: usize,
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
    /// Animation progress (0.0 = closed, 1.0 = fully open)
    animation: f32,
    /// Maximum visible entries
    max_visible: usize,
}

impl CommandPalette {
    /// Create a new command palette
    pub fn new() -> Self {
        Self {
            active: false,
            input: String::new(),
            cursor: 0,
            registry: Vec::new(),
            entries: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            recently_used: Vec::new(),
            animation: 0.0,
            max_visible: 9,
        }
    }

    /// Register commands, replacing any existing ones from the same source
    pub fn register_commands(&mut self, commands: Vec<RegisteredCommand>) {
        // Remove commands from this source if re-registering
        if let Some(first) = commands.first() {
            let source = &first.source;
            self.registry.retain(|c| &c.source != source);
        }
        self.registry.extend(commands);
        // Re-sort: Core first, then Plugin, alphabetically within
        self.registry.sort_by(|a, b| {
            a.category.cmp(&b.category)
                .then_with(|| a.title.cmp(&b.title))
        });
    }

    /// Open the palette
    pub fn open(&mut self) {
        self.active = true;
        self.input.clear();
        self.cursor = 0;
        self.selected = 0;
        self.scroll_offset = 0;
        self.update_entries();
    }

    /// Close the palette
    pub fn close(&mut self) {
        self.active = false;
        self.animation = 0.0;
    }

    /// Check if palette is active
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Update animation, returns true if still animating
    pub fn update_animation(&mut self) -> bool {
        if self.active {
            if self.animation < 1.0 {
                self.animation = (self.animation + 0.15).min(1.0);
                return true;
            }
        } else if self.animation > 0.0 {
            self.animation = (self.animation - 0.2).max(0.0);
            return true;
        }
        false
    }

    /// Handle a character input
    pub fn handle_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.selected = 0;
        self.scroll_offset = 0;
        self.update_entries();
    }

    /// Handle backspace
    pub fn handle_backspace(&mut self) {
        if self.cursor > 0 {
            let prev = self.input[..self.cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.input.remove(prev);
            self.cursor = prev;
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
                CommandSource::Core => PaletteResult::RunAction(id),
            }
        } else {
            self.close();
            PaletteResult::Cancelled
        }
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
                    title: cmd.title.clone(),
                    category: cmd.category.clone(),
                    shortcut: cmd.shortcut.clone(),
                    icon: cmd.icon,
                    destructive: cmd.destructive,
                    match_positions: e.match_positions.clone(),
                    source: cmd.source.clone(),
                }
            })
            .collect();

        PaletteInfo {
            input: self.input.clone(),
            cursor: self.cursor,
            entries,
            selected: self.selected.saturating_sub(self.scroll_offset),
            scroll_offset: self.scroll_offset,
            total_count: self.registry.len(),
            total_entries: self.entries.len(),
            animation: self.animation,
        }
    }

    /// Update filtered entries based on current input
    fn update_entries(&mut self) {
        self.entries.clear();

        if self.input.is_empty() {
            // Show all commands, recently used first
            let mut scored: Vec<PaletteEntry> = self.registry
                .iter()
                .enumerate()
                .map(|(i, cmd)| {
                    let recency_bonus = self.recently_used
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
            scored.sort_by(|a, b| b.score.cmp(&a.score));
            self.entries = scored;
        } else {
            let query = &self.input;

            let mut scored: Vec<PaletteEntry> = self.registry
                .iter()
                .enumerate()
                .filter_map(|(i, cmd)| {
                    // Try matching against title
                    let title_match = FuzzySearch::new(query, &cmd.title).best_match();

                    // Try matching against category
                    let category_match = FuzzySearch::new(query, &cmd.category).best_match();

                    // Try matching against shortcut
                    let shortcut_match = cmd.shortcut.as_ref()
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
                        title_match.as_ref().map(|m| (m.score(), m.matched_indices().copied().collect::<Vec<_>>())),
                        category_match.as_ref().map(|m| (m.score(), Vec::new())),
                        shortcut_match.as_ref().map(|m| (m.score(), Vec::new())),
                        combined_match.as_ref().map(|m| (m.score(), m.matched_indices().copied().collect::<Vec<_>>())),
                    ];

                    let (score, positions) = best.iter()
                        .filter_map(|x| x.as_ref())
                        .max_by_key(|(s, _)| *s)?;

                    // Recency bonus (smaller than search score)
                    let recency_bonus = self.recently_used
                        .iter()
                        .position(|r| r == &cmd.id)
                        .map(|pos| 100 - pos as i64)
                        .unwrap_or(0);

                    Some(PaletteEntry {
                        registry_index: i,
                        score: *score as i64 + prefix_bonus + recency_bonus,
                        match_positions: positions.iter().copied().collect(),
                    })
                })
                .collect();

            scored.sort_by(|a, b| b.score.cmp(&a.score));
            self.entries = scored;
        }
    }

    /// Get the total number of displayed entries
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

/// Build the default set of core commands
pub fn core_commands() -> Vec<RegisteredCommand> {
    let mut commands = vec![
        // ── Core ──
        RegisteredCommand {
            id: "core.new-tab".into(),
            title: "New Tab".into(),
            category: "Core".into(),
            shortcut: Some("Ctrl+Shift+T".into()),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.close-tab".into(),
            title: "Close Tab".into(),
            category: "Core".into(),
            shortcut: Some("Ctrl+Shift+W".into()),
            icon: None,
            source: CommandSource::Core,
            destructive: true,
        },
        RegisteredCommand {
            id: "core.next-tab".into(),
            title: "Next Tab".into(),
            category: "Core".into(),
            shortcut: Some("Ctrl+Tab".into()),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.prev-tab".into(),
            title: "Previous Tab".into(),
            category: "Core".into(),
            shortcut: Some("Ctrl+Shift+Tab".into()),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        // ── Search ──
        RegisteredCommand {
            id: "core.find".into(),
            title: "Find".into(),
            category: "Search".into(),
            shortcut: Some("Ctrl+Shift+F".into()),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        // ── Config ──
        RegisteredCommand {
            id: "core.settings".into(),
            title: "Open Settings".into(),
            category: "Config".into(),
            shortcut: Some("Ctrl+,".into()),
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
        // ── Sessions ──
        RegisteredCommand {
            id: "core.session-manager".into(),
            title: "Session Manager".into(),
            category: "Sessions".into(),
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
            shortcut: Some("Ctrl+=".into()),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.font-size-decrease".into(),
            title: "Decrease Font Size".into(),
            category: "Font".into(),
            shortcut: Some("Ctrl+-".into()),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.font-size-reset".into(),
            title: "Reset Font Size".into(),
            category: "Font".into(),
            shortcut: Some("Ctrl+0".into()),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        // ── Copy/Paste ──
        RegisteredCommand {
            id: "core.copy".into(),
            title: "Copy Selection".into(),
            category: "Core".into(),
            shortcut: Some("Ctrl+Shift+C".into()),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
        RegisteredCommand {
            id: "core.paste".into(),
            title: "Paste from Clipboard".into(),
            category: "Core".into(),
            shortcut: Some("Ctrl+Shift+V".into()),
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
    ];

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
mod tests {
    use super::*;

    fn test_palette() -> CommandPalette {
        let mut palette = CommandPalette::new();
        palette.register_commands(core_commands());
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
        let plugin_cmds = vec![
            RegisteredCommand {
                id: "claude-code.open".into(),
                title: "Open Claude Code".into(),
                category: "AI".into(),
                shortcut: Some("Ctrl+Shift+I".into()),
                icon: Some('🧠'),
                source: CommandSource::Plugin("claude-code".into()),
                destructive: false,
            },
        ];
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
    fn test_execute_plugin_returns_plugin_result() {
        let mut palette = CommandPalette::new();
        palette.register_commands(vec![
            RegisteredCommand {
                id: "test-plugin.action".into(),
                title: "Test Action".into(),
                category: "Test".into(),
                shortcut: None,
                icon: None,
                source: CommandSource::Plugin("test".into()),
                destructive: false,
            },
        ]);
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
