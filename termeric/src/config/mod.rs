//! Configuration management

pub mod theme;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::input::keybindings::{KeyBinding, ShortcutOverride};
use crate::plugin::claude_code::ClaudeCodeConfig;
use crate::plugin::codex::CodexConfig;

/// Application configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// Shell configuration
    pub shell: ShellConfig,
    /// Font configuration
    pub font: FontConfig,
    /// Scrollback configuration
    pub scrollback: ScrollbackConfig,
    /// Theme name or path
    pub theme: ThemeConfig,
    /// Window configuration
    pub window: WindowConfig,
    /// Cursor configuration
    pub cursor: CursorConfig,
    /// Bell configuration
    pub bell: BellConfigFile,
    /// Custom keybindings (plugin/custom: SendText, RunCommand, PluginCommand)
    #[serde(default)]
    pub keybindings: Vec<KeyBinding>,
    /// Keyboard shortcut overrides (unified shortcut system)
    #[serde(default)]
    pub shortcuts: Vec<ShortcutOverride>,
    /// Session save/restore configuration
    #[serde(default)]
    pub session: SessionConfig,
    /// Splash screen configuration
    #[serde(default)]
    pub splash: SplashConfig,
    /// User-defined custom commands / shortcuts
    #[serde(default)]
    pub commands: Vec<CustomCommand>,
    /// Plugin configuration
    #[serde(default)]
    pub plugins: PluginsConfig,
    /// Harvest mode configuration
    #[serde(default)]
    pub harvest: HarvestConfig,
}

/// A user-defined custom command (e.g. SSH into a box)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CustomCommand {
    /// Display name (e.g. "SSH Production")
    pub name: String,
    /// Shell command to execute (e.g. "ssh admin@prod.myapp.com")
    pub command: String,
    /// Optional short alias for command palette (e.g. "prod")
    #[serde(default)]
    pub alias: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shell: ShellConfig::default(),
            font: FontConfig::default(),
            scrollback: ScrollbackConfig::default(),
            theme: ThemeConfig::default(),
            window: WindowConfig::default(),
            cursor: CursorConfig::default(),
            bell: BellConfigFile::default(),
            keybindings: Vec::new(),
            shortcuts: Vec::new(),
            session: SessionConfig::default(),
            splash: SplashConfig::default(),
            commands: Vec::new(),
            plugins: PluginsConfig::default(),
            harvest: HarvestConfig::default(),
        }
    }
}

impl Config {
    /// Load configuration from file or return default
    pub fn load_or_default() -> Result<Self> {
        let config_path = Self::config_path();

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let config: Config = toml::from_str(&content)?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    /// Get the configuration file path
    pub fn config_path() -> PathBuf {
        if let Some(dirs) = directories::ProjectDirs::from("", "", "termeric") {
            dirs.config_dir().join("config.toml")
        } else {
            PathBuf::from("config.toml")
        }
    }

    /// Get the themes directory path
    pub fn themes_dir() -> PathBuf {
        if let Some(dirs) = directories::ProjectDirs::from("", "", "termeric") {
            dirs.config_dir().join("themes")
        } else {
            PathBuf::from("themes")
        }
    }
}

/// Harvest mode configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct HarvestConfig {
    /// Enable harvest mode (command visualization)
    pub enabled: bool,
    /// Show growth indicators for running commands
    pub show_indicators: bool,
    /// Maximum number of commands to keep in history
    pub max_history: usize,
}

impl Default for HarvestConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            show_indicators: true,
            max_history: 100,
        }
    }
}

/// Shell configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ShellConfig {
    /// Path to shell program
    pub program: Option<String>,
    /// Arguments to pass to shell
    pub args: Vec<String>,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            program: None, // Use default shell
            args: vec![],
        }
    }
}

/// Font configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct FontConfig {
    /// Font family name
    pub family: String,
    /// Font size in points
    pub size: f32,
    /// Line height multiplier (e.g. 1.2 = 120%)
    pub line_height: f32,
    /// Fallback font families
    pub fallback: Vec<String>,
    /// Enable font ligatures (for fonts like Fira Code, JetBrains Mono)
    pub ligatures: bool,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "monospace".to_string(),
            size: 14.0,
            line_height: 1.2,
            fallback: vec![],
            ligatures: true,
        }
    }
}

/// Scrollback configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ScrollbackConfig {
    /// Maximum number of lines to keep
    pub lines: usize,
    /// Scroll speed multiplier (lines per scroll event)
    pub scroll_speed: f64,
}

impl Default for ScrollbackConfig {
    fn default() -> Self {
        Self {
            lines: 10_000,
            scroll_speed: 6.0,
        }
    }
}

/// Theme configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ThemeConfig {
    /// Theme name (looks in themes directory)
    pub name: String,
    /// Or explicit path to theme file
    pub path: Option<PathBuf>,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            name: "turmeric".to_string(),
            path: None,
        }
    }
}

/// Window configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct WindowConfig {
    /// Initial width
    pub width: u32,
    /// Initial height
    pub height: u32,
    /// Window opacity (0.0 - 1.0)
    pub opacity: f32,
    /// Padding around terminal content
    pub padding: u32,
    /// Show window decorations
    pub decorations: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            width: 800,
            height: 600,
            opacity: 1.0,
            padding: 8,
            decorations: true,
        }
    }
}

/// Cursor style
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    Block,
    Underline,
    Beam,
}

impl CursorStyle {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Block => "Block",
            Self::Underline => "Underline",
            Self::Beam => "Beam",
        }
    }

    pub const ALL: &[CursorStyle] = &[Self::Block, Self::Underline, Self::Beam];
}

impl Default for CursorStyle {
    fn default() -> Self {
        Self::Block
    }
}

/// Cursor configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct CursorConfig {
    /// Cursor style
    pub style: CursorStyle,
    /// Enable cursor blinking
    pub blink: bool,
    /// Blink rate in milliseconds
    pub blink_rate: u32,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            style: CursorStyle::Block,
            blink: true,
            blink_rate: 530,
        }
    }
}

/// Bell configuration (serializable version)
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct BellConfigFile {
    /// Enable visual bell (screen flash)
    pub visual: bool,
    /// Enable audio bell
    pub audio: bool,
    /// Duration of visual bell flash in milliseconds
    pub visual_duration_ms: u64,
}

impl Default for BellConfigFile {
    fn default() -> Self {
        Self {
            visual: true,
            audio: false,
            visual_duration_ms: 100,
        }
    }
}

/// Session save/restore configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SessionConfig {
    /// Enable session save/restore
    pub enabled: bool,
    /// Automatically restore session on startup
    pub auto_restore: bool,
    /// Automatically save session on exit
    pub auto_save: bool,
    /// Save scrollback content with sessions
    pub save_scrollback: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_restore: true,
            auto_save: true,
            save_scrollback: true,
        }
    }
}

/// When to show the splash screen
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SplashMode {
    /// Never show splash screen
    Never,
    /// Only on first launch ever
    FirstBoot,
    /// Each new window/session (but not restored sessions)
    NewWindow,
    /// Every new tab
    NewTab,
}

impl Default for SplashMode {
    fn default() -> Self {
        Self::FirstBoot
    }
}

impl SplashMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Never => "Never",
            Self::FirstBoot => "First Boot",
            Self::NewWindow => "New Window",
            Self::NewTab => "New Tab",
        }
    }

    pub const ALL: &[SplashMode] = &[Self::Never, Self::FirstBoot, Self::NewWindow, Self::NewTab];
}

/// Splash screen configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SplashConfig {
    /// When to show the splash screen
    pub mode: SplashMode,
}

impl Default for SplashConfig {
    fn default() -> Self {
        Self {
            mode: SplashMode::FirstBoot,
        }
    }
}

/// Plugin configuration section
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct PluginsConfig {
    /// Claude Code plugin settings
    #[serde(default)]
    pub claude_code: ClaudeCodeConfig,
    /// Codex plugin settings
    #[serde(default)]
    pub codex: CodexConfig,
}
