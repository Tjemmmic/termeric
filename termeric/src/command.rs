//! Command mode for built-in terminal commands
//!
//! Provides a Ctrl+Shift+P command palette for configuring the terminal.

use std::fmt;
use sublime_fuzzy::FuzzySearch;

/// Result of executing a command
#[derive(Debug, Clone)]
pub enum CommandResult {
    /// Command executed successfully with optional message
    Success(Option<String>),
    /// Command failed with error message
    Error(String),
    /// Command wants to change theme
    SetTheme(String),
    /// Command wants to change font size
    SetFontSize(f32),
    /// Command wants to change font family
    SetFontFamily(String),
    /// Command wants to change window opacity
    SetOpacity(f32),
    /// Command wants to change window padding
    SetPadding(u32),
    /// Command wants to change scrollback lines
    SetScrollback(usize),
    /// Command wants to show current configuration
    ShowConfig,
    /// Command wants to reset config to defaults
    ResetConfig,
    /// Command wants to open config UI
    OpenConfigUI,
    /// Command wants to save config
    SaveConfig,
    /// Command wants to reload config
    ReloadConfig,
    /// Show help text
    ShowHelp(String),
    /// List all available themes
    ListThemes,
    /// List all available commands
    ListCommands,
    /// List all keyboard shortcuts
    ListKeybindings,
    /// Show version information
    ShowVersion,
    /// Show about/credits information
    ShowAbout,
    /// Exit command mode without action
    Cancel,
}

/// Available built-in commands
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Change theme
    Theme(String),
    /// Change font size
    FontSize(f32),
    /// Change font family
    FontFamily(String),
    /// Change window opacity
    Opacity(f32),
    /// Change window padding
    Padding(u32),
    /// Change scrollback lines
    Scrollback(usize),
    /// Show current configuration
    ShowConfig,
    /// Reset configuration to defaults
    ResetConfig,
    /// Open configuration UI
    Config,
    /// Save current configuration
    Save,
    /// Reload configuration from file
    Reload,
    /// Show help
    Help(Option<String>),
    /// List all available themes
    ListThemes,
    /// List all available commands
    ListCommands,
    /// List all keyboard shortcuts
    ListKeybindings,
    /// Show version information
    Version,
    /// Show about/credits information
    About,
    /// Unknown command
    Unknown(String),
}

impl Command {
    /// Parse a command string into a Command
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();

        // Empty input
        if input.is_empty() {
            return None;
        }

        let mut parts = input.split_whitespace();
        let cmd = parts.next()?;
        let args: Vec<&str> = parts.collect();

        Some(match cmd.to_lowercase().as_str() {
            "theme" | "t" => {
                if let Some(name) = args.first() {
                    Command::Theme(name.to_string())
                } else {
                    Command::Unknown("theme: missing theme name".to_string())
                }
            }
            "font-size" | "fontsize" | "fs" => {
                if let Some(size_str) = args.first() {
                    match size_str.parse::<f32>() {
                        Ok(size) if size >= 6.0 && size <= 72.0 => Command::FontSize(size),
                        Ok(_) => Command::Unknown("font-size: must be between 6 and 72".to_string()),
                        Err(_) => Command::Unknown("font-size: invalid number".to_string()),
                    }
                } else {
                    Command::Unknown("font-size: missing size".to_string())
                }
            }
            "font-family" | "fontfamily" | "font" | "ff" => {
                if let Some(family) = args.first() {
                    // Join all args for multi-word font names
                    Command::FontFamily(args.join(" "))
                } else {
                    Command::Unknown("font-family: missing font name".to_string())
                }
            }
            "opacity" | "alpha" | "o" => {
                if let Some(opacity_str) = args.first() {
                    match opacity_str.parse::<f32>() {
                        Ok(opacity) if (0.0..=1.0).contains(&opacity) => Command::Opacity(opacity),
                        Ok(_) => Command::Unknown("opacity: must be between 0.0 and 1.0".to_string()),
                        Err(_) => Command::Unknown("opacity: invalid number".to_string()),
                    }
                } else {
                    Command::Unknown("opacity: missing value".to_string())
                }
            }
            "padding" | "pad" | "p" => {
                if let Some(padding_str) = args.first() {
                    match padding_str.parse::<u32>() {
                        Ok(padding) if padding <= 100 => Command::Padding(padding),
                        Ok(_) => Command::Unknown("padding: must be between 0 and 100".to_string()),
                        Err(_) => Command::Unknown("padding: invalid number".to_string()),
                    }
                } else {
                    Command::Unknown("padding: missing value".to_string())
                }
            }
            "scrollback" | "scroll" | "sb" => {
                if let Some(lines_str) = args.first() {
                    match lines_str.parse::<usize>() {
                        Ok(lines) if lines >= 100 && lines <= 1_000_000 => Command::Scrollback(lines),
                        Ok(_) => Command::Unknown("scrollback: must be between 100 and 1,000,000".to_string()),
                        Err(_) => Command::Unknown("scrollback: invalid number".to_string()),
                    }
                } else {
                    Command::Unknown("scrollback: missing value".to_string())
                }
            }
            "show-config" | "showconfig" | "show" => Command::ShowConfig,
            "reset-config" | "resetconfig" | "reset" => Command::ResetConfig,
            "config" | "c" => Command::Config,
            "save" | "s" => Command::Save,
            "reload" | "r" => Command::Reload,
            "help" | "h" | "?" => {
                Command::Help(args.first().map(|s| s.to_string()))
            }
            "list-themes" | "listthemes" | "themes" => Command::ListThemes,
            "list-commands" | "listcommands" | "commands" | "list" => Command::ListCommands,
            "list-keybindings" | "listkeybindings" | "keybindings" | "keys" | "shortcuts" => Command::ListKeybindings,
            "version" | "v" | "ver" => Command::Version,
            "about" | "credits" | "info" => Command::About,
            "" => Command::Unknown("empty command".to_string()),
            other => Command::Unknown(format!("unknown command: {}", other)),
        })
    }

    /// Execute the command and return the result
    pub fn execute(self) -> CommandResult {
        match self {
            Command::Theme(name) => CommandResult::SetTheme(name),
            Command::FontSize(size) => CommandResult::SetFontSize(size),
            Command::FontFamily(family) => CommandResult::SetFontFamily(family),
            Command::Opacity(opacity) => CommandResult::SetOpacity(opacity),
            Command::Padding(padding) => CommandResult::SetPadding(padding),
            Command::Scrollback(lines) => CommandResult::SetScrollback(lines),
            Command::ShowConfig => CommandResult::ShowConfig,
            Command::ResetConfig => CommandResult::ResetConfig,
            Command::Config => CommandResult::OpenConfigUI,
            Command::Save => CommandResult::SaveConfig,
            Command::Reload => CommandResult::ReloadConfig,
            Command::Help(topic) => CommandResult::ShowHelp(Self::help_text(topic.as_deref())),
            Command::ListThemes => CommandResult::ListThemes,
            Command::ListCommands => CommandResult::ListCommands,
            Command::ListKeybindings => CommandResult::ListKeybindings,
            Command::Version => CommandResult::ShowVersion,
            Command::About => CommandResult::ShowAbout,
            Command::Unknown(msg) => CommandResult::Error(msg),
        }
    }

    /// Get help text for commands
    fn help_text(topic: Option<&str>) -> String {
        match topic {
            Some("theme") | Some("t") => {
                "Usage: theme <name>\n\
                 Change the terminal color theme.\n\
                 Available themes: turmeric, solarized-dark, dracula, nord\n\
                 Alias: t".to_string()
            }
            Some("font-size") | Some("fontsize") | Some("fs") => {
                "Usage: font-size <size>\n\
                 Change the font size (6-72 points).\n\
                 Alias: fs".to_string()
            }
            Some("config") | Some("c") => {
                "Usage: config\n\
                 Open configuration UI for easy settings management.\n\
                 Provides a visual interface for all terminal settings.\n\
                 Alias: c".to_string()
            }
            Some("save") | Some("s") => {
                "Usage: save\n\
                 Save current settings to config file.\n\
                 Config location: ~/.config/termeric/config.toml\n\
                 Alias: s".to_string()
            }
            Some("reload") | Some("r") => {
                "Usage: reload\n\
                 Reload configuration from file.\n\
                 Alias: r".to_string()
            }
            _ => {
                "Termeric Commands (Ctrl+Shift+P):\n\n\
                 APPEARANCE:\n\
                 theme <name>       - Change color theme (t)\n\
                 font-size <size>   - Change font size 6-72 (fs)\n\
                 font-family <name> - Change font family (ff)\n\
                 opacity <0.0-1.0>  - Set window opacity (o)\n\
                 padding <0-100>    - Set content padding (p)\n\n\
                 TERMINAL:\n\
                 scrollback <lines> - Set scrollback lines 100-1M (sb)\n\n\
                 CONFIG:\n\
                 show-config        - Show current settings (show)\n\
                 reset-config       - Reset to defaults (reset)\n\
                 config             - Open config UI (c)\n\
                 save               - Save settings (s)\n\
                 reload             - Reload settings (r)\n\n\
                 HELP & DISCOVERY:\n\
                 help [cmd]         - Show help (h, ?)\n\
                 list-themes        - List available themes (themes)\n\
                 list-commands      - List all commands (commands, list)\n\
                 list-keybindings   - Show keyboard shortcuts (keys)\n\
                 version            - Show version info (v)\n\
                 about              - Show about/credits (info)\n\n\
                 Press Escape to cancel.".to_string()
            }
        }
    }
}

/// Command metadata for registry and autocomplete
#[derive(Debug, Clone)]
pub struct CommandDef {
    /// Primary command name
    pub name: &'static str,
    /// Short alias (optional)
    pub alias: Option<&'static str>,
    /// Category for grouping
    pub category: &'static str,
    /// Brief description
    pub description: &'static str,
    /// Usage example
    pub usage: &'static str,
}

impl CommandDef {
    /// Get searchable text (for fuzzy matching)
    pub fn search_text(&self) -> String {
        format!("{} {}", self.name, self.description)
    }
}

/// Command registry with all available commands
pub struct CommandRegistry {
    commands: Vec<CommandDef>,
}

impl CommandRegistry {
    /// Create new command registry with all available commands
    pub fn new() -> Self {
        Self {
            commands: vec![
                CommandDef {
                    name: "theme",
                    alias: Some("t"),
                    category: "Theme",
                    description: "Change color theme",
                    usage: "theme <name>",
                },
                CommandDef {
                    name: "font-size",
                    alias: Some("fs"),
                    category: "Font",
                    description: "Change font size (6-72 points)",
                    usage: "font-size <size>",
                },
                CommandDef {
                    name: "font-family",
                    alias: Some("ff"),
                    category: "Font",
                    description: "Change font family",
                    usage: "font-family <name>",
                },
                CommandDef {
                    name: "opacity",
                    alias: Some("o"),
                    category: "Window",
                    description: "Set window opacity (0.0-1.0)",
                    usage: "opacity <value>",
                },
                CommandDef {
                    name: "padding",
                    alias: Some("p"),
                    category: "Window",
                    description: "Set content padding (0-100)",
                    usage: "padding <pixels>",
                },
                CommandDef {
                    name: "scrollback",
                    alias: Some("sb"),
                    category: "Terminal",
                    description: "Set scrollback lines (100-1M)",
                    usage: "scrollback <lines>",
                },
                CommandDef {
                    name: "show-config",
                    alias: Some("show"),
                    category: "Config",
                    description: "Show current configuration",
                    usage: "show-config",
                },
                CommandDef {
                    name: "reset-config",
                    alias: Some("reset"),
                    category: "Config",
                    description: "Reset configuration to defaults",
                    usage: "reset-config",
                },
                CommandDef {
                    name: "config",
                    alias: Some("c"),
                    category: "Config",
                    description: "Open configuration UI",
                    usage: "config",
                },
                CommandDef {
                    name: "save",
                    alias: Some("s"),
                    category: "Config",
                    description: "Save settings to config file",
                    usage: "save",
                },
                CommandDef {
                    name: "reload",
                    alias: Some("r"),
                    category: "Config",
                    description: "Reload settings from config file",
                    usage: "reload",
                },
                CommandDef {
                    name: "help",
                    alias: Some("h"),
                    category: "Help",
                    description: "Show command help",
                    usage: "help [command]",
                },
                CommandDef {
                    name: "list-themes",
                    alias: Some("themes"),
                    category: "Help",
                    description: "List all available themes",
                    usage: "list-themes",
                },
                CommandDef {
                    name: "list-commands",
                    alias: Some("commands"),
                    category: "Help",
                    description: "List all available commands",
                    usage: "list-commands",
                },
                CommandDef {
                    name: "list-keybindings",
                    alias: Some("keys"),
                    category: "Help",
                    description: "Show keyboard shortcuts",
                    usage: "list-keybindings",
                },
                CommandDef {
                    name: "version",
                    alias: Some("v"),
                    category: "Help",
                    description: "Show version information",
                    usage: "version",
                },
                CommandDef {
                    name: "about",
                    alias: Some("info"),
                    category: "Help",
                    description: "Show about/credits",
                    usage: "about",
                },
            ],
        }
    }

    /// Search commands with fuzzy matching
    /// Returns up to `limit` matches, sorted by score
    pub fn search(&self, query: &str, limit: usize) -> Vec<&CommandDef> {
        if query.is_empty() {
            // Return no suggestions when no query (only show when user types)
            return Vec::new();
        }

        let query_lower = query.to_lowercase();

        let mut matches: Vec<(&CommandDef, isize)> = self.commands
            .iter()
            .filter_map(|cmd| {
                let cmd_name_lower = cmd.name.to_lowercase();

                // Exact match gets highest priority
                if cmd_name_lower == query_lower {
                    return Some((cmd, 1_000_000));
                }

                // Exact alias match
                if let Some(alias) = cmd.alias {
                    if alias.to_lowercase() == query_lower {
                        return Some((cmd, 900_000));
                    }
                }

                // Prefix match gets very high priority
                if cmd_name_lower.starts_with(&query_lower) {
                    // Longer match within prefix = higher score
                    return Some((cmd, 800_000 + query.len() as isize * 1000));
                }

                // Prefix match on alias
                if let Some(alias) = cmd.alias {
                    if alias.to_lowercase().starts_with(&query_lower) {
                        return Some((cmd, 700_000 + query.len() as isize * 1000));
                    }
                }

                // Try matching against name first
                let name_match = FuzzySearch::new(query, cmd.name).best_match();

                // Try matching against alias
                let alias_match = cmd.alias
                    .and_then(|alias| FuzzySearch::new(query, alias).best_match());

                // Try matching against full search text (name + description)
                let full_match = FuzzySearch::new(query, &cmd.search_text()).best_match();

                // Take the best score from fuzzy matching
                let best_score = [
                    name_match.map(|m| m.score()),
                    alias_match.map(|m| m.score()),
                    full_match.map(|m| m.score()),
                ]
                .iter()
                .filter_map(|&s| s)
                .max()?;

                Some((cmd, best_score))
            })
            .collect();

        // Sort by score descending (higher score = better match)
        matches.sort_by(|a, b| b.1.cmp(&a.1));

        // Return top matches
        matches.into_iter().take(limit).map(|(cmd, _)| cmd).collect()
    }

    /// Get all commands
    pub fn all(&self) -> &[CommandDef] {
        &self.commands
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Phase of command input
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandInputPhase {
    /// User is selecting a command
    SelectingCommand,
    /// User is entering arguments for a selected command
    EnteringArguments,
    /// Browsing command history
    BrowsingHistory,
}

/// State for command input mode
pub struct CommandMode {
    /// Current input buffer
    input: String,
    /// Cursor position in input
    cursor: usize,
    /// Whether command mode is active
    active: bool,
    /// Current input phase
    phase: CommandInputPhase,
    /// Selected command name when in EnteringArguments phase
    selected_command: Option<String>,
    /// Message to display (result of last command)
    message: Option<CommandMessage>,
    /// When the message was set (for auto-hide timeout)
    message_time: Option<std::time::Instant>,
    /// Command registry for autocompletion
    registry: CommandRegistry,
    /// Current autocomplete suggestions
    suggestions: Vec<CommandDef>,
    /// Selected suggestion index
    selected_suggestion: usize,
    /// Command history (most recent first)
    history: Vec<String>,
    /// Current position in history (None = not browsing)
    history_index: Option<usize>,
    /// Saved input when browsing history
    history_saved_input: String,
    /// Scroll offset for message display (in lines)
    message_scroll_offset: usize,
}

/// Message displayed after command execution
#[derive(Debug, Clone)]
pub struct CommandMessage {
    /// The message text
    pub text: String,
    /// Whether this is an error
    pub is_error: bool,
}

impl CommandMode {
    /// Create a new command mode (inactive)
    pub fn new() -> Self {
        let registry = CommandRegistry::new();

        Self {
            input: String::new(),
            cursor: 0,
            active: false,
            phase: CommandInputPhase::SelectingCommand,
            selected_command: None,
            message: None,
            message_time: None,
            registry,
            suggestions: Vec::new(), // Start with no suggestions
            selected_suggestion: 0,
            history: Vec::new(),
            history_index: None,
            history_saved_input: String::new(),
            message_scroll_offset: 0,
        }
    }

    /// Check if command mode is active
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Enter command mode
    pub fn activate(&mut self) {
        self.active = true;
        self.phase = CommandInputPhase::SelectingCommand;
        self.selected_command = None;
        self.input.clear();
        self.cursor = 0;
        self.message = None;
        self.message_time = None;
        self.update_suggestions();
        self.selected_suggestion = 0;
        self.history_index = None;
        self.history_saved_input.clear();
    }

    /// Exit command mode
    pub fn deactivate(&mut self) {
        self.active = false;
        self.phase = CommandInputPhase::SelectingCommand;
        self.selected_command = None;
        self.input.clear();
        self.cursor = 0;
    }

    /// Get current input phase
    pub fn phase(&self) -> CommandInputPhase {
        self.phase
    }

    /// Get the current input
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Get cursor position
    pub fn cursor_position(&self) -> usize {
        self.cursor
    }

    /// Get current message if any
    pub fn message(&self) -> Option<&CommandMessage> {
        self.message.as_ref()
    }

    /// Clear any displayed message
    pub fn clear_message(&mut self) {
        self.message = None;
    }

    /// Set a message to display
    pub fn set_message(&mut self, text: String, is_error: bool) {
        self.message = Some(CommandMessage { text, is_error });
        self.message_time = Some(std::time::Instant::now());
        self.message_scroll_offset = 0; // Reset scroll when showing new message
    }

    /// Check if message should be hidden (after timeout)
    /// Returns true if the message has expired
    pub fn should_hide_message(&self) -> bool {
        const MESSAGE_TIMEOUT_SECS: f32 = 2.5;

        if let (Some(_), Some(time)) = (&self.message, self.message_time) {
            time.elapsed().as_secs_f32() > MESSAGE_TIMEOUT_SECS
        } else {
            false
        }
    }

    /// Clear message if it has timed out
    pub fn update(&mut self) {
        // Don't timeout messages while command mode is active
        // (e.g., help text should stay visible)
        if !self.active && self.should_hide_message() {
            self.message = None;
            self.message_time = None;
        }
    }

    /// Handle a character input
    pub fn handle_char(&mut self, c: char) {
        // Insert character at cursor position
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.update_suggestions();
        self.selected_suggestion = 0;
    }

    /// Handle backspace
    pub fn handle_backspace(&mut self) {
        if self.cursor > 0 {
            let prev_char_start = self.input[..self.cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.input.remove(prev_char_start);
            self.cursor = prev_char_start;

            // If we're in EnteringArguments phase and backspaced away all arguments,
            // return to SelectingCommand phase
            if self.phase == CommandInputPhase::EnteringArguments {
                let trimmed = self.input.trim();
                if !trimmed.contains(' ') {
                    // No more space means no more arguments
                    self.phase = CommandInputPhase::SelectingCommand;
                    self.selected_command = None;
                }
            }

            self.update_suggestions();
            self.selected_suggestion = 0;
        }
    }

    /// Handle delete key
    pub fn handle_delete(&mut self) {
        if self.cursor < self.input.len() {
            self.input.remove(self.cursor);
        }
    }

    /// Move cursor left
    pub fn cursor_left(&mut self) {
        if self.cursor > 1 {
            // Find previous char boundary
            let prev = self.input[..self.cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(1);
            self.cursor = prev.max(1);
        }
    }

    /// Move cursor right
    pub fn cursor_right(&mut self) {
        if self.cursor < self.input.len() {
            // Find next char boundary
            let next = self.input[self.cursor..]
                .chars()
                .next()
                .map(|c| self.cursor + c.len_utf8())
                .unwrap_or(self.input.len());
            self.cursor = next;
        }
    }

    /// Move cursor to start
    pub fn cursor_home(&mut self) {
        self.cursor = 0;
    }

    /// Move cursor to end
    pub fn cursor_end(&mut self) {
        self.cursor = self.input.len();
    }

    /// Execute the current command
    pub fn execute(&mut self) -> Option<CommandResult> {
        if !self.active {
            return None;
        }

        let command = Command::parse(&self.input)?;
        let result = command.execute();

        // Add to history if successful (but not duplicates at front)
        if !matches!(result, CommandResult::Error(_) | CommandResult::Cancel) {
            let cmd_text = self.input.clone();
            if !cmd_text.is_empty() {
                // Remove if already exists to move to front
                self.history.retain(|h| h != &cmd_text);
                self.history.insert(0, cmd_text);
                // Keep last 50 commands
                self.history.truncate(50);
            }
        }

        // Handle the result to set message
        match &result {
            CommandResult::Success(msg) => {
                if let Some(text) = msg {
                    self.set_message(text.clone(), false);
                }
                self.deactivate();
            }
            CommandResult::Error(msg) => {
                self.set_message(msg.clone(), true);
                // Stay in command mode on error
            }
            CommandResult::ShowHelp(text) => {
                self.set_message(text.clone(), false);
                // Keep command mode active to show help, but clear input
                self.input.clear();
                self.cursor = 0;
                self.phase = CommandInputPhase::SelectingCommand;
                self.selected_command = None;
                self.update_suggestions();
            }
            CommandResult::ListThemes |
            CommandResult::ListCommands |
            CommandResult::ListKeybindings |
            CommandResult::ShowVersion |
            CommandResult::ShowAbout => {
                // These are handled by App and will set messages
                // Keep command mode active to show the result, clear input
                self.input.clear();
                self.cursor = 0;
                self.phase = CommandInputPhase::SelectingCommand;
                self.selected_command = None;
                self.update_suggestions();
            }
            CommandResult::Cancel => {
                self.deactivate();
            }
            // For actions that need to be handled by App
            CommandResult::SetTheme(_) |
            CommandResult::SetFontSize(_) |
            CommandResult::SetFontFamily(_) |
            CommandResult::SetOpacity(_) |
            CommandResult::SetPadding(_) |
            CommandResult::SetScrollback(_) |
            CommandResult::ShowConfig |
            CommandResult::ResetConfig |
            CommandResult::OpenConfigUI |
            CommandResult::SaveConfig |
            CommandResult::ReloadConfig => {
                self.deactivate();
            }
        }

        Some(result)
    }

    /// Update autocomplete suggestions based on current input
    fn update_suggestions(&mut self) {
        // Check if we're typing arguments for a command
        // If input contains space, we might be typing arguments
        if let Some(space_idx) = self.input.find(' ') {
            let cmd_part = self.input[..space_idx].trim().to_lowercase();
            let arg_part = &self.input[space_idx + 1..];

            // Provide argument suggestions based on command
            match cmd_part.as_str() {
                "theme" | "t" => {
                    // Define static theme suggestions
                    static THEME_SUGGESTIONS: &[CommandDef] = &[
                        CommandDef {
                            name: "turmeric",
                            alias: None,
                            category: "Theme",
                            description: "Warm turmeric-inspired theme (default)",
                            usage: "theme turmeric",
                        },
                        CommandDef {
                            name: "solarized-dark",
                            alias: None,
                            category: "Theme",
                            description: "Popular Solarized Dark theme",
                            usage: "theme solarized-dark",
                        },
                        CommandDef {
                            name: "dracula",
                            alias: None,
                            category: "Theme",
                            description: "Dracula color scheme",
                            usage: "theme dracula",
                        },
                        CommandDef {
                            name: "nord",
                            alias: None,
                            category: "Theme",
                            description: "Nord color palette",
                            usage: "theme nord",
                        },
                    ];

                    self.suggestions = THEME_SUGGESTIONS
                        .iter()
                        .filter(|theme| theme.name.starts_with(&arg_part.to_lowercase()))
                        .cloned()
                        .take(6)
                        .collect();
                    return;
                }
                "font-size" | "fontsize" | "fs" => {
                    // Define static size suggestions
                    static SIZE_SUGGESTIONS: &[CommandDef] = &[
                        CommandDef {
                            name: "10",
                            alias: None,
                            category: "Font",
                            description: "Small size",
                            usage: "font-size 10",
                        },
                        CommandDef {
                            name: "12",
                            alias: None,
                            category: "Font",
                            description: "Medium size",
                            usage: "font-size 12",
                        },
                        CommandDef {
                            name: "14",
                            alias: None,
                            category: "Font",
                            description: "Default size",
                            usage: "font-size 14",
                        },
                        CommandDef {
                            name: "16",
                            alias: None,
                            category: "Font",
                            description: "Large size",
                            usage: "font-size 16",
                        },
                        CommandDef {
                            name: "18",
                            alias: None,
                            category: "Font",
                            description: "Extra large size",
                            usage: "font-size 18",
                        },
                        CommandDef {
                            name: "20",
                            alias: None,
                            category: "Font",
                            description: "Very large size",
                            usage: "font-size 20",
                        },
                    ];

                    self.suggestions = SIZE_SUGGESTIONS
                        .iter()
                        .filter(|size| size.name.starts_with(arg_part))
                        .cloned()
                        .take(6)
                        .collect();
                    return;
                }
                "font-family" | "fontfamily" | "font" | "ff" => {
                    // Define static font family suggestions
                    static FONT_SUGGESTIONS: &[CommandDef] = &[
                        CommandDef {
                            name: "JetBrains Mono",
                            alias: None,
                            category: "Font",
                            description: "Popular programming font",
                            usage: "font-family JetBrains Mono",
                        },
                        CommandDef {
                            name: "Fira Code",
                            alias: None,
                            category: "Font",
                            description: "Font with programming ligatures",
                            usage: "font-family Fira Code",
                        },
                        CommandDef {
                            name: "Cascadia Code",
                            alias: None,
                            category: "Font",
                            description: "Microsoft's monospace font",
                            usage: "font-family Cascadia Code",
                        },
                        CommandDef {
                            name: "Hack",
                            alias: None,
                            category: "Font",
                            description: "Clean programming font",
                            usage: "font-family Hack",
                        },
                        CommandDef {
                            name: "Source Code Pro",
                            alias: None,
                            category: "Font",
                            description: "Adobe's monospace font",
                            usage: "font-family Source Code Pro",
                        },
                        CommandDef {
                            name: "Inconsolata",
                            alias: None,
                            category: "Font",
                            description: "Humanist monospace font",
                            usage: "font-family Inconsolata",
                        },
                    ];

                    self.suggestions = FONT_SUGGESTIONS
                        .iter()
                        .filter(|font| font.name.to_lowercase().contains(&arg_part.to_lowercase()))
                        .cloned()
                        .take(6)
                        .collect();
                    return;
                }
                "opacity" | "alpha" | "o" => {
                    // Define static opacity suggestions
                    static OPACITY_SUGGESTIONS: &[CommandDef] = &[
                        CommandDef {
                            name: "1.0",
                            alias: None,
                            category: "Window",
                            description: "Fully opaque (default)",
                            usage: "opacity 1.0",
                        },
                        CommandDef {
                            name: "0.95",
                            alias: None,
                            category: "Window",
                            description: "Slightly transparent",
                            usage: "opacity 0.95",
                        },
                        CommandDef {
                            name: "0.9",
                            alias: None,
                            category: "Window",
                            description: "Moderately transparent",
                            usage: "opacity 0.9",
                        },
                        CommandDef {
                            name: "0.85",
                            alias: None,
                            category: "Window",
                            description: "More transparent",
                            usage: "opacity 0.85",
                        },
                        CommandDef {
                            name: "0.8",
                            alias: None,
                            category: "Window",
                            description: "Very transparent",
                            usage: "opacity 0.8",
                        },
                    ];

                    self.suggestions = OPACITY_SUGGESTIONS
                        .iter()
                        .filter(|val| val.name.starts_with(arg_part))
                        .cloned()
                        .take(6)
                        .collect();
                    return;
                }
                "padding" | "pad" | "p" => {
                    // Define static padding suggestions
                    static PADDING_SUGGESTIONS: &[CommandDef] = &[
                        CommandDef {
                            name: "0",
                            alias: None,
                            category: "Window",
                            description: "No padding",
                            usage: "padding 0",
                        },
                        CommandDef {
                            name: "4",
                            alias: None,
                            category: "Window",
                            description: "Minimal padding",
                            usage: "padding 4",
                        },
                        CommandDef {
                            name: "8",
                            alias: None,
                            category: "Window",
                            description: "Default padding",
                            usage: "padding 8",
                        },
                        CommandDef {
                            name: "12",
                            alias: None,
                            category: "Window",
                            description: "Medium padding",
                            usage: "padding 12",
                        },
                        CommandDef {
                            name: "16",
                            alias: None,
                            category: "Window",
                            description: "Large padding",
                            usage: "padding 16",
                        },
                        CommandDef {
                            name: "24",
                            alias: None,
                            category: "Window",
                            description: "Extra large padding",
                            usage: "padding 24",
                        },
                    ];

                    self.suggestions = PADDING_SUGGESTIONS
                        .iter()
                        .filter(|val| val.name.starts_with(arg_part))
                        .cloned()
                        .take(6)
                        .collect();
                    return;
                }
                "scrollback" | "sb" => {
                    // Define static scrollback suggestions
                    static SCROLLBACK_SUGGESTIONS: &[CommandDef] = &[
                        CommandDef {
                            name: "1000",
                            alias: None,
                            category: "Terminal",
                            description: "Small scrollback buffer",
                            usage: "scrollback 1000",
                        },
                        CommandDef {
                            name: "5000",
                            alias: None,
                            category: "Terminal",
                            description: "Medium scrollback buffer",
                            usage: "scrollback 5000",
                        },
                        CommandDef {
                            name: "10000",
                            alias: None,
                            category: "Terminal",
                            description: "Default scrollback buffer",
                            usage: "scrollback 10000",
                        },
                        CommandDef {
                            name: "50000",
                            alias: None,
                            category: "Terminal",
                            description: "Large scrollback buffer",
                            usage: "scrollback 50000",
                        },
                        CommandDef {
                            name: "100000",
                            alias: None,
                            category: "Terminal",
                            description: "Very large scrollback buffer",
                            usage: "scrollback 100000",
                        },
                    ];

                    self.suggestions = SCROLLBACK_SUGGESTIONS
                        .iter()
                        .filter(|val| val.name.starts_with(arg_part))
                        .cloned()
                        .take(6)
                        .collect();
                    return;
                }
                _ => {
                    // Unknown command with args, show no suggestions
                    self.suggestions.clear();
                    return;
                }
            }
        }

        // No space, search for command names
        self.suggestions = self.registry
            .search(&self.input, 6)
            .into_iter()
            .cloned()
            .collect();
    }

    /// Get current autocomplete suggestions
    pub fn suggestions(&self) -> &[CommandDef] {
        &self.suggestions
    }

    /// Get selected suggestion index
    pub fn selected_suggestion(&self) -> usize {
        self.selected_suggestion
    }

    /// Move selection to next suggestion
    pub fn next_suggestion(&mut self) {
        if !self.suggestions.is_empty() {
            self.selected_suggestion = (self.selected_suggestion + 1) % self.suggestions.len();
        }
    }

    /// Move selection to previous suggestion
    pub fn prev_suggestion(&mut self) {
        if !self.suggestions.is_empty() {
            if self.selected_suggestion == 0 {
                self.selected_suggestion = self.suggestions.len() - 1;
            } else {
                self.selected_suggestion -= 1;
            }
        }
    }

    /// Browse history backwards (Ctrl+Up)
    pub fn history_prev(&mut self) {
        if self.phase == CommandInputPhase::BrowsingHistory {
            // Already browsing, go back further
            if let Some(idx) = self.history_index {
                if idx + 1 < self.history.len() {
                    self.history_index = Some(idx + 1);
                    self.input = self.history[idx + 1].clone();
                    self.cursor = self.input.len();
                }
            }
        } else if !self.history.is_empty() {
            // Start browsing history
            self.phase = CommandInputPhase::BrowsingHistory;
            self.history_saved_input = self.input.clone();
            self.history_index = Some(0);
            self.input = self.history[0].clone();
            self.cursor = self.input.len();
            self.update_suggestions();
        }
    }

    /// Browse history forwards (Ctrl+Down)
    pub fn history_next(&mut self) {
        if self.phase == CommandInputPhase::BrowsingHistory {
            if let Some(idx) = self.history_index {
                if idx > 0 {
                    self.history_index = Some(idx - 1);
                    self.input = self.history[idx - 1].clone();
                    self.cursor = self.input.len();
                } else {
                    // Reached current, restore saved input and exit history mode
                    self.phase = CommandInputPhase::SelectingCommand;
                    self.history_index = None;
                    self.input = self.history_saved_input.clone();
                    self.cursor = self.input.len();
                }
                self.update_suggestions();
            }
        }
    }

    /// Check if a command needs arguments
    fn command_needs_arguments(cmd: &str) -> bool {
        matches!(
            cmd.to_lowercase().as_str(),
            "theme" | "t" |
            "font-size" | "fontsize" | "fs" |
            "font-family" | "fontfamily" | "font" | "ff" |
            "opacity" | "alpha" | "o" |
            "padding" | "pad" | "p" |
            "scrollback" | "sb" |
            "help" | "h"
        )
    }

    /// Handle Enter key - returns true if command should be executed
    pub fn handle_enter(&mut self) -> bool {
        match self.phase {
            CommandInputPhase::SelectingCommand => {
                // Apply suggestion if available
                if !self.suggestions.is_empty() {
                    self.apply_suggestion();
                }

                // Check if the input is a command that needs arguments
                let input_trimmed = self.input.trim();
                if let Some(space_idx) = input_trimmed.find(' ') {
                    // Already has arguments, execute
                    return true;
                }

                // Check if command needs arguments
                if Self::command_needs_arguments(input_trimmed) {
                    // Transition to EnteringArguments phase
                    self.phase = CommandInputPhase::EnteringArguments;
                    self.selected_command = Some(input_trimmed.to_string());
                    // Add space if not already there
                    if !self.input.ends_with(' ') {
                        self.input.push(' ');
                        self.cursor += 1;
                    }
                    self.update_suggestions();
                    self.selected_suggestion = 0;
                    false // Don't execute yet
                } else {
                    // Command doesn't need arguments, execute
                    true
                }
            }
            CommandInputPhase::EnteringArguments => {
                // Apply suggestion if available
                if !self.suggestions.is_empty() {
                    self.apply_suggestion();
                }
                // Execute the command
                true
            }
            CommandInputPhase::BrowsingHistory => {
                // Accept history selection and execute
                self.phase = CommandInputPhase::SelectingCommand;
                self.history_index = None;
                true
            }
        }
    }

    /// Apply the currently selected suggestion (Tab completion)
    pub fn apply_suggestion(&mut self) {
        if let Some(suggestion) = self.suggestions.get(self.selected_suggestion) {
            // Check if we're completing an argument or a command
            if let Some(space_idx) = self.input.find(' ') {
                // Completing an argument - replace the argument part
                let cmd_part = &self.input[..=space_idx];
                self.input = format!("{}{}", cmd_part, suggestion.name);
                self.cursor = self.input.len();
            } else {
                // Completing a command name
                self.input = suggestion.name.to_string();
                self.cursor = self.input.len();
                // Add space for commands that need arguments
                if !suggestion.usage.ends_with(suggestion.name) {
                    self.input.push(' ');
                    self.cursor += 1;
                }
            }
            self.update_suggestions();
        }
    }

    /// Scroll message up (show earlier lines)
    pub fn scroll_message_up(&mut self) {
        if self.message_scroll_offset > 0 {
            self.message_scroll_offset -= 1;
        }
    }

    /// Scroll message down (show later lines)
    pub fn scroll_message_down(&mut self) {
        // We'll validate max scroll in the renderer based on visible lines
        self.message_scroll_offset += 1;
    }

    /// Get current message scroll offset
    pub fn message_scroll_offset(&self) -> usize {
        self.message_scroll_offset
    }

    /// Get access to the command registry
    pub fn registry(&self) -> &CommandRegistry {
        &self.registry
    }
}

impl Default for CommandMode {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CommandMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_theme_command() {
        assert_eq!(
            Command::parse("theme dracula"),
            Some(Command::Theme("dracula".to_string()))
        );
        assert_eq!(
            Command::parse("t nord"),
            Some(Command::Theme("nord".to_string()))
        );
    }

    #[test]
    fn test_parse_font_size_command() {
        assert_eq!(
            Command::parse("font-size 16"),
            Some(Command::FontSize(16.0))
        );
        assert_eq!(
            Command::parse("fs 14.5"),
            Some(Command::FontSize(14.5))
        );
    }

    #[test]
    fn test_parse_simple_commands() {
        assert_eq!(Command::parse("save"), Some(Command::Save));
        assert_eq!(Command::parse("reload"), Some(Command::Reload));
        assert_eq!(Command::parse("help"), Some(Command::Help(None)));
        assert_eq!(
            Command::parse("help theme"),
            Some(Command::Help(Some("theme".to_string())))
        );
    }

    #[test]
    fn test_command_mode_input() {
        let mut mode = CommandMode::new();
        mode.activate();

        assert!(mode.is_active());
        assert_eq!(mode.input(), "");

        mode.handle_char('h');
        mode.handle_char('e');
        mode.handle_char('l');
        mode.handle_char('p');

        assert_eq!(mode.input(), "help");
    }

    #[test]
    fn test_command_mode_backspace() {
        let mut mode = CommandMode::new();
        mode.activate();
        mode.handle_char('a');
        mode.handle_char('b');
        mode.handle_backspace();

        assert_eq!(mode.input(), "a");

        // Can delete everything
        mode.handle_backspace();
        assert_eq!(mode.input(), "");

        // Backspace on empty does nothing
        mode.handle_backspace();
        assert_eq!(mode.input(), "");
    }
}
