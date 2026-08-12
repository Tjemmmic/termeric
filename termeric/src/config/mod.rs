//! Configuration management

pub mod theme;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::input::keybindings::{KeyBinding, ShortcutOverride};
use crate::plugin::claude_code::ClaudeCodeConfig;
use crate::plugin::codex::CodexConfig;

/// Application configuration
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
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

impl Config {
    /// Load configuration from file or return defaults when no file exists.
    /// Parse and validation errors are returned so runtime reloads can preserve
    /// the last known-good configuration.
    pub fn load_or_default() -> Result<Self> {
        let config_path = Self::config_path();

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .with_context(|| format!("failed to read {}", config_path.display()))?;
            let config: Config = toml::from_str(&content)
                .with_context(|| format!("failed to parse {}", config_path.display()))?;
            config.validate()?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    /// Load startup configuration without making a malformed user file fatal.
    /// The warning is returned for presentation after the UI is initialized.
    pub fn load_for_startup() -> (Self, Option<String>) {
        match Self::load_or_default() {
            Ok(config) => (config, None),
            Err(error) => (
                Self::default(),
                Some(format!(
                    "Configuration could not be loaded; using defaults: {error:#}"
                )),
            ),
        }
    }

    /// Reject values that would create unsafe allocations or broken runtime
    /// geometry. Validation is shared by startup, reload, settings, and save.
    pub fn validate(&self) -> Result<()> {
        if self.font.family.trim().is_empty() {
            bail!("font.family must not be empty");
        }
        if !(8.0..=72.0).contains(&self.font.size) || !self.font.size.is_finite() {
            bail!("font.size must be between 8 and 72");
        }
        if !(0.8..=3.0).contains(&self.font.line_height) || !self.font.line_height.is_finite() {
            bail!("font.line_height must be between 0.8 and 3.0");
        }
        if !(100..=1_000_000).contains(&self.scrollback.lines) {
            bail!("scrollback.lines must be between 100 and 1000000");
        }
        if !(0.1..=100.0).contains(&self.scrollback.scroll_speed)
            || !self.scrollback.scroll_speed.is_finite()
        {
            bail!("scrollback.scroll_speed must be between 0.1 and 100");
        }
        if !(0.1..=1.0).contains(&self.window.opacity) || !self.window.opacity.is_finite() {
            bail!("window.opacity must be between 0.1 and 1.0");
        }
        if self.window.padding > 256 {
            bail!("window.padding must not exceed 256");
        }
        if !(320..=16_384).contains(&self.window.width)
            || !(200..=16_384).contains(&self.window.height)
        {
            bail!("window dimensions must be between 320x200 and 16384x16384");
        }
        if !(100..=5_000).contains(&self.cursor.blink_rate) {
            bail!("cursor.blink_rate must be between 100 and 5000 milliseconds");
        }
        if !(10..=5_000).contains(&self.bell.visual_duration_ms) {
            bail!("bell.visual_duration_ms must be between 10 and 5000");
        }
        if !(1..=10_000).contains(&self.harvest.max_history) {
            bail!("harvest.max_history must be between 1 and 10000");
        }
        if !matches!(
            self.plugins.codex.approval_mode.as_str(),
            "suggest" | "auto-edit" | "full-auto"
        ) {
            bail!("plugins.codex.approval_mode must be suggest, auto-edit, or full-auto");
        }
        if let Some(sandbox) = self.plugins.codex.sandbox_mode.as_deref() {
            if !matches!(sandbox, "read-only" | "workspace-write" | "full-access") {
                bail!(
                    "plugins.codex.sandbox_mode must be read-only, workspace-write, or full-access"
                );
            }
        }
        Ok(())
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
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ShellConfig {
    /// Path to shell program
    pub program: Option<String>,
    /// Arguments to pass to shell
    pub args: Vec<String>,
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
    /// Window opacity (0.1 - 1.0)
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    #[default]
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
            auto_restore: false,
            auto_save: true,
            save_scrollback: true,
        }
    }
}

/// When to show the splash screen
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SplashMode {
    /// Never show splash screen
    Never,
    /// Only on first launch ever
    #[default]
    FirstBoot,
    /// Each new window/session (but not restored sessions)
    NewWindow,
    /// Every new tab
    NewTab,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn validation_rejects_non_finite_and_unsafe_values() {
        let mut config = Config::default();
        config.window.opacity = f32::NAN;
        assert!(config.validate().is_err());

        config = Config::default();
        config.scrollback.lines = usize::MAX;
        assert!(config.validate().is_err());

        config = Config::default();
        config.harvest.max_history = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn deserialized_config_is_validated_before_application() {
        let config: Config = toml::from_str(
            r#"
                [font]
                size = 1000.0
            "#,
        )
        .unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn codex_policy_values_are_validated_centrally() {
        let mut config = Config::default();
        config.plugins.codex.approval_mode = "legacy".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn theme_name_and_explicit_path_survive_config_round_trip() {
        let mut config = Config::default();
        config.theme.name = "arrowroot".into();
        config.theme.path = Some(PathBuf::from("/tmp/arrowroot.toml"));

        let encoded = toml::to_string(&config).unwrap();
        let decoded: Config = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.theme.name, "arrowroot");
        assert_eq!(
            decoded.theme.path,
            Some(PathBuf::from("/tmp/arrowroot.toml"))
        );
    }
}
