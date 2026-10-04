//! Configuration management

pub mod save;
pub mod theme;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::path::PathBuf;

use crate::input::keybindings::{KeyBinding, ShortcutOverride};

/// Application configuration
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// Renderer adapter selection and recovery policy.
    pub renderer: RendererConfig,
    /// User-selected accessibility presentation preferences.
    pub accessibility: AccessibilityConfig,
    /// Shell configuration
    pub shell: ShellConfig,
    /// Reusable terminal launch profiles, keyed by a stable name.
    pub profiles: BTreeMap<String, LaunchProfileConfig>,
    /// Font configuration
    pub font: FontConfig,
    /// Scrollback configuration
    pub scrollback: ScrollbackConfig,
    /// Split-pane creation and working-directory policy.
    pub panes: PanesConfig,
    /// Terminal keyboard compatibility profile.
    pub keyboard: KeyboardConfig,
    /// Keyboard copy mode and quick-select configuration.
    pub copy_mode: CopyModeConfig,
    /// Theme name or path
    pub theme: ThemeConfig,
    /// Window configuration
    pub window: WindowConfig,
    /// Cursor configuration
    pub cursor: CursorConfig,
    /// Bell configuration
    pub bell: BellConfigFile,
    /// Security policy for terminal-controlled desktop integrations.
    pub security: SecurityConfig,
    /// Persistent local/remote terminal-domain configuration.
    pub domains: DomainsConfig,
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
    /// `[agents.<id>]`: overrides for built-in coding agents and definitions
    /// of custom ones.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, crate::agents::AgentConfig>,
    /// `[agent_status]`: presentation of agent hook reports.
    #[serde(default)]
    pub agent_status: crate::agents::config::AgentStatusConfig,
    /// `[agent_view]`: the docked conversation panel.
    #[serde(default)]
    pub agent_view: crate::agents::config::AgentViewConfig,
    /// `[supervisor]`: the agent that oversees the others, and its authority.
    #[serde(default)]
    pub supervisor: crate::agents::supervisor::SupervisorConfig,
    /// Harvest mode configuration
    #[serde(default)]
    pub harvest: HarvestConfig,
}

/// Working directory selected for newly split panes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneCwdMode {
    /// Use the focused pane's foreground process directory.
    #[default]
    Focused,
    /// Use the primary pane's foreground process directory.
    Tab,
    /// Let the configured shell inherit Termeric's process directory.
    Process,
}

impl PaneCwdMode {
    /// The folder a new pane starts in: the source pane's, the tab's first
    /// pane's, or Termeric's own. `None` when that folder is unknown, and the
    /// shell starts where it would by default.
    pub fn resolve(
        self,
        source_cwd: impl FnOnce() -> Option<String>,
        primary_cwd: impl FnOnce() -> Option<String>,
    ) -> Option<String> {
        match self {
            Self::Focused => source_cwd(),
            Self::Tab => primary_cwd(),
            Self::Process => std::env::current_dir()
                .ok()
                .map(|path| path.to_string_lossy().into_owned()),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Focused => "focused pane",
            Self::Tab => "tab primary pane",
            Self::Process => "Termeric process",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Focused => Self::Tab,
            Self::Tab => Self::Process,
            Self::Process => Self::Focused,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct PanesConfig {
    pub cwd: PaneCwdMode,
}

/// Xterm-compatible keyboard behavior used for new terminals and RIS.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum XtermKeyboardTypeConfig {
    #[default]
    Normal,
    Legacy,
    Vt220,
}

impl XtermKeyboardTypeConfig {
    pub const fn terminal_type(self) -> termeric_core::term::XtermKeyboardType {
        match self {
            Self::Normal => termeric_core::term::XtermKeyboardType::Normal,
            Self::Legacy => termeric_core::term::XtermKeyboardType::Legacy,
            Self::Vt220 => termeric_core::term::XtermKeyboardType::Vt220,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct KeyboardConfig {
    /// `normal`, `legacy`, or `vt220`.
    pub xterm_type: XtermKeyboardTypeConfig,
}

/// Renderer adapter selection policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RendererBackend {
    /// Prefer a hardware adapter and fall back to WGPU's software adapter.
    #[default]
    Auto,
    /// Require a non-fallback GPU adapter.
    Gpu,
    /// Require WGPU's fallback (normally CPU/software) adapter.
    Software,
}

impl std::fmt::Display for RendererBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Auto => "auto",
            Self::Gpu => "gpu",
            Self::Software => "software",
        })
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct RendererConfig {
    /// Adapter policy: auto, gpu, or software.
    pub backend: RendererBackend,
}

/// Whether an accessibility presentation preference follows the desktop or
/// uses an explicit application override.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessibilityPreference {
    #[default]
    System,
    On,
    Off,
}

impl AccessibilityPreference {
    pub const ALL: [Self; 3] = [Self::System, Self::On, Self::Off];

    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::On => "On",
            Self::Off => "Off",
        }
    }

    pub const fn resolve(self, system: Option<bool>) -> bool {
        match self {
            Self::System => matches!(system, Some(true)),
            Self::On => true,
            Self::Off => false,
        }
    }
}

/// Accessibility presentation preferences. Each policy can follow the native
/// desktop signal or use a portable explicit override.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct AccessibilityConfig {
    /// Remove non-essential panel, overlay, toast, hover, and menu transitions.
    pub reduce_motion: AccessibilityPreference,
    /// Increase chrome text, control, border, and focus contrast without
    /// rewriting terminal/application colors.
    pub high_contrast: AccessibilityPreference,
}

/// A user-defined custom command (e.g. SSH into a box)
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
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
        Self::load_from(&Self::config_path())
    }

    /// Load the configuration file at `config_path`, or defaults when there
    /// is none.
    pub fn load_from(config_path: &std::path::Path) -> Result<Self> {
        if config_path.exists() {
            let content = std::fs::read_to_string(config_path)
                .with_context(|| format!("failed to read {}", config_path.display()))?;
            Self::from_toml(&content, config_path)
        } else {
            Ok(Self::default())
        }
    }

    /// Parse and validate `text`, the contents of the configuration file at
    /// `source`.
    pub fn from_toml(text: &str, source: &std::path::Path) -> Result<Self> {
        let config: Config = toml::from_str(text)
            .with_context(|| format!("failed to parse {}", source.display()))?;
        config.validate()?;
        Ok(config)
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
        validate_child_environment("shell", &self.shell.env)?;
        validate_font_family("font.family", &self.font.family)?;
        if self.font.fallback.len() > MAX_FONT_FALLBACKS {
            bail!("font.fallback must contain at most {MAX_FONT_FALLBACKS} families");
        }
        let mut font_families = std::collections::HashSet::new();
        font_families.insert(self.font.family.to_ascii_lowercase());
        for (index, family) in self.font.fallback.iter().enumerate() {
            validate_font_family(&format!("font.fallback[{index}]"), family)?;
            if !font_families.insert(family.to_ascii_lowercase()) {
                bail!("font.fallback contains duplicate family {family:?}");
            }
        }
        if self.font.features.len() > MAX_FONT_FEATURES {
            bail!("font.features must contain at most {MAX_FONT_FEATURES} overrides");
        }
        for tag in self.font.features.keys() {
            validate_font_feature_tag(tag)?;
        }
        if self.font.range_overrides.len() > MAX_FONT_RANGE_OVERRIDES {
            bail!("font.range_overrides must contain at most {MAX_FONT_RANGE_OVERRIDES} rules");
        }
        let mut ranges = Vec::with_capacity(self.font.range_overrides.len());
        for (index, rule) in self.font.range_overrides.iter().enumerate() {
            validate_font_family(
                &format!("font.range_overrides[{index}].family"),
                &rule.family,
            )?;
            if rule.start > rule.end {
                bail!("font.range_overrides[{index}] start must not exceed end");
            }
            if rule.end > 0x10_FFFF {
                bail!("font.range_overrides[{index}] exceeds U+10FFFF");
            }
            if rule.start <= 0xDFFF && rule.end >= 0xD800 {
                bail!("font.range_overrides[{index}] intersects the surrogate range");
            }
            ranges.push((rule.start, rule.end, index));
        }
        ranges.sort_unstable_by_key(|&(start, end, _)| (start, end));
        for pair in ranges.windows(2) {
            let (_, previous_end, previous_index) = pair[0];
            let (next_start, _, next_index) = pair[1];
            if next_start <= previous_end {
                bail!(
                    "font.range_overrides[{previous_index}] overlaps font.range_overrides[{next_index}]"
                );
            }
        }
        validate_range("font.size", self.font.size, FONT_SIZE_RANGE, Some("points"))?;
        if !(MIN_FONT_WEIGHT..=MAX_FONT_WEIGHT).contains(&self.font.weight) {
            bail!("font.weight must be between {MIN_FONT_WEIGHT} and {MAX_FONT_WEIGHT}");
        }
        if !(MIN_FONT_WEIGHT..=MAX_FONT_WEIGHT).contains(&self.font.bold_weight) {
            bail!("font.bold_weight must be between {MIN_FONT_WEIGHT} and {MAX_FONT_WEIGHT}");
        }
        if self.font.bold_weight < self.font.weight {
            bail!("font.bold_weight must be greater than or equal to font.weight");
        }
        validate_range(
            "font.line_height",
            self.font.line_height,
            LINE_HEIGHT_RANGE,
            None,
        )?;
        validate_range(
            "scrollback.lines",
            self.scrollback.lines,
            SCROLLBACK_LINES_RANGE,
            Some("lines"),
        )?;
        validate_range(
            "scrollback.max_memory_mib",
            self.scrollback.max_memory_mib,
            SCROLLBACK_MEMORY_MIB_RANGE,
            Some("MiB"),
        )?;
        validate_range(
            "scrollback.scroll_speed",
            self.scrollback.scroll_speed,
            SCROLL_SPEED_RANGE,
            None,
        )?;
        validate_launch_profiles(&self.profiles)?;
        if self.copy_mode.patterns.len() > 32 {
            bail!("copy_mode.patterns must contain at most 32 entries");
        }
        for pattern in &self.copy_mode.patterns {
            if pattern.name.trim().is_empty() {
                bail!("copy_mode pattern names must not be empty");
            }
            if pattern.regex.len() > 4096 {
                bail!("copy_mode pattern regexes must not exceed 4096 bytes");
            }
            regex::Regex::new(&pattern.regex).with_context(|| {
                format!("copy_mode pattern {:?} has an invalid regex", pattern.name)
            })?;
        }
        validate_range(
            "window.opacity",
            self.window.opacity,
            WINDOW_OPACITY_RANGE,
            None,
        )?;
        self.window.background_image.validate()?;
        validate_range(
            "window.padding",
            self.window.padding,
            WINDOW_PADDING_RANGE,
            Some("pixels"),
        )?;
        if !(320..=16_384).contains(&self.window.width)
            || !(200..=16_384).contains(&self.window.height)
        {
            bail!("window dimensions must be between 320x200 and 16384x16384");
        }
        validate_range(
            "cursor.blink_rate",
            self.cursor.blink_rate,
            CURSOR_BLINK_RATE_RANGE,
            Some("milliseconds"),
        )?;
        validate_range(
            "bell.visual_duration_ms",
            self.bell.visual_duration_ms,
            BELL_DURATION_RANGE,
            Some("milliseconds"),
        )?;
        if !(1..=termeric_core::ansi::MAX_OSC52_DECODED_BYTES)
            .contains(&self.security.osc52.max_bytes)
        {
            bail!("security.osc52.max_bytes must be between 1 and 1048576");
        }
        validate_range(
            "harvest.max_history",
            self.harvest.max_history,
            HARVEST_HISTORY_RANGE,
            Some("commands"),
        )?;
        self.domains.validate()?;
        crate::agents::config::validate(&self.agents)?;
        self.supervisor.validate()?;
        for (id, agent) in &self.agents {
            validate_child_environment(&format!("agents.{id}"), &agent.env)?;
            for (name, profile) in &agent.profiles {
                validate_child_environment(&format!("agents.{id}.profiles.{name}"), &profile.env)?;
            }
        }
        Ok(())
    }

    /// Resolve one validated terminal launch profile by its stable name.
    pub fn launch_profile(&self, name: &str) -> Result<&LaunchProfileConfig> {
        validate_terminal_profile_name(name)?;
        self.profiles.get(name).ok_or_else(|| {
            anyhow::anyhow!("unknown terminal profile {name:?}; configure [profiles.{name}]")
        })
    }

    /// Get the configuration file path
    pub fn config_path() -> PathBuf {
        crate::paths::config_dir().map_or_else(
            || PathBuf::from("config.toml"),
            |dir| dir.join("config.toml"),
        )
    }

    /// Get the themes directory path
    pub fn themes_dir() -> PathBuf {
        crate::paths::config_dir().map_or_else(|| PathBuf::from("themes"), |dir| dir.join("themes"))
    }
}

/// Bounds of the numeric settings that the Settings dialog also adjusts. The
/// file validator and the Settings controls both use these, so a control can
/// reach every value the file accepts and nothing it refuses.
pub const FONT_SIZE_RANGE: RangeInclusive<f32> = 8.0..=72.0;
pub const LINE_HEIGHT_RANGE: RangeInclusive<f32> = 0.8..=3.0;
pub const WINDOW_OPACITY_RANGE: RangeInclusive<f32> = 0.1..=1.0;
pub const BACKGROUND_IMAGE_OPACITY_RANGE: RangeInclusive<f32> = 0.0..=1.0;
pub const WINDOW_PADDING_RANGE: RangeInclusive<u32> = 0..=256;
/// Milliseconds.
pub const CURSOR_BLINK_RATE_RANGE: RangeInclusive<u32> = 100..=5_000;
/// Milliseconds.
pub const BELL_DURATION_RANGE: RangeInclusive<u64> = 10..=5_000;
pub const HARVEST_HISTORY_RANGE: RangeInclusive<usize> = 1..=10_000;
pub const SCROLLBACK_LINES_RANGE: RangeInclusive<usize> = 100..=1_000_000;
pub const SCROLLBACK_MEMORY_MIB_RANGE: RangeInclusive<usize> = 8..=4_096;
/// Lines per mouse-wheel step.
pub const SCROLL_SPEED_RANGE: RangeInclusive<f64> = 0.1..=100.0;

/// Refuse `value` outside `range`; NaN and infinities are outside every
/// float range here. The bounds print as the documentation writes them, a
/// float with its decimal point ("0.1 and 1.0"), followed by `unit`.
fn validate_range<T>(
    key: &str,
    value: T,
    range: RangeInclusive<T>,
    unit: Option<&str>,
) -> Result<()>
where
    T: PartialOrd + std::fmt::Debug,
{
    if !range.contains(&value) {
        let unit = unit.map(|unit| format!(" {unit}")).unwrap_or_default();
        bail!(
            "{key} must be between {:?} and {:?}{unit}",
            range.start(),
            range.end()
        );
    }
    Ok(())
}

/// Write an `f32` setting as the shortest decimal that reads back as it, so
/// a saved `1.2` stays `1.2` instead of the `1.2000000476837158` that
/// widening it to the `f64` TOML stores would give.
fn serialize_shortest_f32<S: serde::Serializer>(
    value: &f32,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    let shortest = value
        .to_string()
        .parse()
        .unwrap_or_else(|_| f64::from(*value));
    serializer.serialize_f64(shortest)
}

/// Saved destinations for the experimental OpenSSH domain transport. OpenSSH
/// remains the source of truth for ports, identities, proxies, certificates,
/// and known-host policy; Termeric profiles only provide bounded stable names.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct DomainsConfig {
    pub ssh_hosts: BTreeMap<String, SshHostProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SshHostProfile {
    pub destination: String,
    #[serde(default)]
    pub description: Option<String>,
}

impl DomainsConfig {
    const MAX_SSH_HOSTS: usize = 64;

    pub fn validate(&self) -> Result<()> {
        if self.ssh_hosts.len() > Self::MAX_SSH_HOSTS {
            bail!(
                "domains.ssh_hosts must contain at most {} profiles",
                Self::MAX_SSH_HOSTS
            );
        }
        for (name, profile) in &self.ssh_hosts {
            validate_ssh_profile_name(name)?;
            crate::domain::validate_ssh_destination(&profile.destination)
                .with_context(|| format!("domains.ssh_hosts.{name}.destination is invalid"))?;
            if let Some(description) = &profile.description
                && (description.trim().is_empty()
                    || description.chars().count() > 256
                    || description.contains(['\n', '\r', '\0']))
            {
                bail!(
                    "domains.ssh_hosts.{name}.description must contain 1 to 256 single-line characters"
                );
            }
        }
        Ok(())
    }

    pub fn ssh_host(&self, name: &str) -> Result<&SshHostProfile> {
        validate_ssh_profile_name(name)?;
        self.ssh_hosts.get(name).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown SSH host profile {name:?}; configure [domains.ssh_hosts.{name}]"
            )
        })
    }
}

fn validate_ssh_profile_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || name.starts_with('.')
        || name.contains("..")
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        bail!("invalid SSH host profile name {name:?}");
    }
    Ok(())
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct SecurityConfig {
    /// Permit terminal applications to derive visible cell contents through
    /// DECRQCRA checksum reports. Disabled by default to prevent screen scraping.
    pub allow_screen_checksum_reports: bool,
    /// Permit terminal applications to read persistent local files through
    /// Kitty's `t=f` graphics medium. Disabled by default because the path is
    /// supplied by the application running inside the terminal.
    pub allow_kitty_file_transfers: bool,
    /// Permit authenticated running-GUI control and separately granted
    /// declarative plugins to write bounded UTF-8 text to an exact terminal.
    /// Disabled by default because external actors can otherwise execute
    /// commands in a live session.
    pub allow_gui_control_input: bool,
    pub osc52: Osc52Config,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Osc52Config {
    /// Permit terminal applications to read desktop clipboard contents.
    pub allow_read: bool,
    /// Permit terminal applications to write desktop clipboard contents.
    pub allow_write: bool,
    /// Maximum decoded UTF-8 payload accepted for an OSC 52 write.
    pub max_bytes: usize,
}

impl Default for Osc52Config {
    fn default() -> Self {
        Self {
            allow_read: false,
            allow_write: true,
            max_bytes: 1024 * 1024,
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct ShellConfig {
    /// Path to shell program
    pub program: Option<String>,
    /// Arguments to pass to shell
    pub args: Vec<String>,
    /// Additional child environment variables.
    pub env: BTreeMap<String, String>,
}

/// A reusable shell or command launch exposed through the CLI and palette.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct LaunchProfileConfig {
    pub description: Option<String>,
    /// Native Linux serial transport. When present, process launch fields are
    /// intentionally unavailable so one profile cannot ambiguously describe
    /// two transports.
    pub serial: Option<SerialProfileConfig>,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub title: Option<String>,
    pub env: BTreeMap<String, String>,
}

impl LaunchProfileConfig {
    pub fn shell_config(&self) -> ShellConfig {
        debug_assert!(self.serial.is_none());
        ShellConfig {
            program: Some(self.program.clone()),
            args: self.args.clone(),
            env: self.env.clone(),
        }
    }

    pub fn serial_config(&self) -> Option<&SerialProfileConfig> {
        self.serial.as_ref()
    }
}

/// Validated native serial-device settings for one terminal profile.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SerialProfileConfig {
    pub device: PathBuf,
    pub baud: u32,
    pub data_bits: u8,
    pub parity: SerialParity,
    pub stop_bits: u8,
    pub flow_control: SerialFlowControl,
}

impl Default for SerialProfileConfig {
    fn default() -> Self {
        Self {
            device: PathBuf::new(),
            baud: 115_200,
            data_bits: 8,
            parity: SerialParity::None,
            stop_bits: 1,
            flow_control: SerialFlowControl::None,
        }
    }
}

impl SerialProfileConfig {
    pub(crate) fn validate(&self, name: &str) -> Result<()> {
        if !self.device.is_absolute() || self.device.as_os_str().is_empty() {
            bail!("serial profile {name:?} device must be an absolute path");
        }
        if self.device.to_str().is_none() {
            bail!("serial profile {name:?} device must be valid UTF-8");
        }
        if !SUPPORTED_SERIAL_BAUD_RATES.contains(&self.baud) {
            bail!(
                "serial profile {name:?} has unsupported baud rate {}",
                self.baud
            );
        }
        if !(5..=8).contains(&self.data_bits) {
            bail!("serial profile {name:?} data_bits must be between 5 and 8");
        }
        if !matches!(self.stop_bits, 1 | 2) {
            bail!("serial profile {name:?} stop_bits must be 1 or 2");
        }
        Ok(())
    }
}

pub(crate) const SUPPORTED_SERIAL_BAUD_RATES: &[u32] = &[
    50, 75, 110, 134, 150, 200, 300, 600, 1_200, 1_800, 2_400, 4_800, 9_600, 19_200, 38_400,
    57_600, 115_200, 230_400, 460_800, 500_000, 576_000, 921_600, 1_000_000, 1_152_000, 1_500_000,
    2_000_000, 2_500_000, 3_000_000, 3_500_000, 4_000_000,
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SerialParity {
    #[default]
    None,
    Even,
    Odd,
}

impl SerialParity {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Even => "even",
            Self::Odd => "odd",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SerialFlowControl {
    #[default]
    None,
    Software,
    Hardware,
}

impl SerialFlowControl {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Software => "software",
            Self::Hardware => "hardware",
        }
    }
}

fn validate_launch_profiles(profiles: &BTreeMap<String, LaunchProfileConfig>) -> Result<()> {
    for (name, profile) in profiles {
        validate_terminal_profile_name(name)?;
        if let Some(serial) = &profile.serial {
            serial.validate(name)?;
            if !profile.program.is_empty()
                || !profile.args.is_empty()
                || profile.cwd.is_some()
                || !profile.env.is_empty()
            {
                bail!("serial profile {name:?} cannot also set program, args, cwd, or environment");
            }
        } else {
            if profile.program.trim().is_empty() || !is_single_line(&profile.program) {
                bail!("terminal profile {name:?} has an invalid program");
            }
            if profile.args.iter().any(|arg| arg.contains('\0')) {
                bail!("terminal profile {name:?} has an argument containing NUL");
            }
        }
        if profile
            .description
            .as_deref()
            .is_some_and(|value| !is_display_line(value))
        {
            bail!(
                "terminal profile {name:?} description must be one non-blank line without \
                 control or bidirectional formatting characters"
            );
        }
        if profile
            .title
            .as_deref()
            .is_some_and(|value| !is_display_line(value))
        {
            bail!(
                "terminal profile {name:?} title must be one non-blank line without control \
                 or bidirectional formatting characters"
            );
        }
        if profile.cwd.as_ref().is_some_and(|path| !path.is_absolute()) {
            bail!("terminal profile {name:?} cwd must be an absolute path");
        }
        validate_child_environment(&format!("terminal profile {name:?}"), &profile.env)?;
    }
    Ok(())
}

/// The one rule for a terminal profile's name, wherever one is given: the
/// configuration, the command line, and saved workspaces. Up to 64 ASCII
/// letters, digits, `.`, `_`, and `-`, not starting with `.` or holding `..`.
pub fn validate_terminal_profile_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || name.starts_with('.')
        || name.contains("..")
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        bail!("invalid terminal profile name {name:?}");
    }
    Ok(())
}

fn validate_child_environment(owner: &str, env: &BTreeMap<String, String>) -> Result<()> {
    for (key, value) in env {
        validate_child_env_key(owner, key)?;
        if value.contains('\0') {
            bail!("{owner} environment value {key:?} contains NUL");
        }
    }
    Ok(())
}

fn validate_child_env_key(owner: &str, key: &str) -> Result<()> {
    let mut bytes = key.bytes();
    let valid = bytes
        .next()
        .is_some_and(|byte| byte == b'_' || byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric());
    let reserved = key == "PWD" || termeric_core::pty::terminal_identity_variable(key);
    if !valid || reserved {
        bail!("{owner} has invalid or reserved environment key {key:?}");
    }
    Ok(())
}

fn is_single_line(value: &str) -> bool {
    !value.contains(['\0', '\n', '\r'])
}

/// Text shown as one line of chrome (a tab or window title, a palette entry):
/// not blank, and nothing that breaks the line or reorders what surrounds it,
/// the rule terminal-set titles are filtered by.
fn is_display_line(value: &str) -> bool {
    !value.trim().is_empty() && !value.chars().any(crate::safe_text::is_unsafe_display_char)
}

fn validate_font_family(owner: &str, family: &str) -> Result<()> {
    if family.is_empty() || family.trim() != family {
        bail!("{owner} must be a non-empty trimmed family name");
    }
    if family.len() > MAX_FONT_FAMILY_BYTES {
        bail!("{owner} exceeds {MAX_FONT_FAMILY_BYTES} bytes");
    }
    if family.chars().any(char::is_control) {
        bail!("{owner} must not contain control characters");
    }
    Ok(())
}

fn validate_font_feature_tag(tag: &str) -> Result<()> {
    if tag.len() != 4
        || !tag
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        bail!("font.features tag {tag:?} must be exactly four lowercase ASCII letters or digits");
    }
    if tag == "kern" {
        bail!("font.features must not override kern because terminal cell origins are fixed");
    }
    Ok(())
}

/// Font configuration
pub const MAX_FONT_FALLBACKS: usize = 16;
pub const MAX_FONT_FAMILY_BYTES: usize = 256;
pub const MAX_FONT_FEATURES: usize = 32;
pub const MAX_FONT_RANGE_OVERRIDES: usize = 64;
pub const MIN_FONT_WEIGHT: u16 = 1;
pub const MAX_FONT_WEIGHT: u16 = 1_000;
pub const DEFAULT_FONT_WEIGHT: u16 = 400;
pub const DEFAULT_BOLD_FONT_WEIGHT: u16 = 700;

/// Inclusive Unicode-scalar interval routed to one preferred font family.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct FontRangeOverride {
    /// First included Unicode scalar value.
    pub start: u32,
    /// Last included Unicode scalar value.
    pub end: u32,
    /// Preferred installed font family for matching graphemes.
    pub family: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct FontConfig {
    /// Font family name
    pub family: String,
    /// Font size in points
    #[serde(serialize_with = "serialize_shortest_f32")]
    pub size: f32,
    /// OpenType `wght` value used for ordinary terminal text.
    pub weight: u16,
    /// OpenType `wght` value used for SGR bold terminal text.
    pub bold_weight: u16,
    /// Line height multiplier (e.g. 1.2 = 120%)
    #[serde(serialize_with = "serialize_shortest_f32")]
    pub line_height: f32,
    /// Fallback font families
    pub fallback: Vec<String>,
    /// Explicit four-byte OpenType shaping feature values.
    pub features: BTreeMap<String, u32>,
    /// Bounded non-overlapping Unicode ranges with a preferred family.
    pub range_overrides: Vec<FontRangeOverride>,
    /// Enable font ligatures (for fonts like Fira Code, JetBrains Mono)
    pub ligatures: bool,
    /// Treat bold ANSI colors 0-7 as their bright 8-15 counterparts.
    pub bold_is_bright: bool,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "monospace".to_string(),
            size: 12.0,
            weight: DEFAULT_FONT_WEIGHT,
            bold_weight: DEFAULT_BOLD_FONT_WEIGHT,
            line_height: 1.2,
            fallback: vec![],
            features: BTreeMap::new(),
            range_overrides: vec![],
            ligatures: true,
            bold_is_bright: false,
        }
    }
}

/// Scrollback configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ScrollbackConfig {
    /// Maximum number of lines to keep
    pub lines: usize,
    /// Maximum compact history memory per terminal or pane, in MiB
    pub max_memory_mib: usize,
    /// Scroll speed multiplier (lines per scroll event)
    pub scroll_speed: f64,
}

impl Default for ScrollbackConfig {
    fn default() -> Self {
        Self {
            lines: 10_000,
            max_memory_mib: 128,
            scroll_speed: 6.0,
        }
    }
}

impl ScrollbackConfig {
    pub fn max_memory_bytes(&self) -> usize {
        self.max_memory_mib * 1024 * 1024
    }
}

/// Built-in modal bindings used after entering copy mode. The global entry
/// shortcuts remain independently configurable in the shortcut registry.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CopyModeBindings {
    #[default]
    Vi,
    Emacs,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct QuickSelectPattern {
    pub name: String,
    pub regex: String,
}

/// Copy mode deliberately keeps semantic patterns bounded and compiles them
/// during validated configuration application, never in the render loop.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct CopyModeConfig {
    pub bindings: CopyModeBindings,
    pub patterns: Vec<QuickSelectPattern>,
}

impl Default for CopyModeConfig {
    fn default() -> Self {
        Self {
            bindings: CopyModeBindings::Vi,
            patterns: vec![
                QuickSelectPattern {
                    name: "URL".into(),
                    regex: r"(?:https?|ftp|ftps|file|ssh|git)://[^\s<>]+|mailto:[^\s<>]+".into(),
                },
                QuickSelectPattern {
                    name: "path".into(),
                    regex: r"(?:\.{0,2}/|/)[[:alnum:]_.~+@%=-]+(?:/[[:alnum:]_.~+@%=-]+)*".into(),
                },
                QuickSelectPattern {
                    name: "hash".into(),
                    regex: r"\b[0-9a-fA-F]{7,40}\b".into(),
                },
            ],
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

/// Desktop stacking policy requested for ordinary windows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowLevelConfig {
    /// Let the desktop environment manage the window normally.
    #[default]
    Normal,
    /// Request that the window remain above ordinary windows.
    AlwaysOnTop,
    /// Request that the window remain below ordinary windows.
    AlwaysOnBottom,
}

impl WindowLevelConfig {
    pub const ALL: &[Self] = &[Self::Normal, Self::AlwaysOnTop, Self::AlwaysOnBottom];

    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::AlwaysOnTop => "Always on top",
            Self::AlwaysOnBottom => "Always on bottom",
        }
    }

    pub fn native(self) -> winit::window::WindowLevel {
        match self {
            Self::Normal => winit::window::WindowLevel::Normal,
            Self::AlwaysOnTop => winit::window::WindowLevel::AlwaysOnTop,
            Self::AlwaysOnBottom => winit::window::WindowLevel::AlwaysOnBottom,
        }
    }

    /// Quick access is a dedicated transient policy and cannot be demoted by
    /// the ordinary-window preference.
    pub fn native_for_window(self, quick_access: bool) -> winit::window::WindowLevel {
        if quick_access {
            winit::window::WindowLevel::AlwaysOnTop
        } else {
            self.native()
        }
    }
}

/// Placement of the tab strip. Global window controls remain in the top
/// toolbar so session/settings affordances keep stable desktop geometry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TabBarPosition {
    /// Lay tabs out horizontally in the top toolbar.
    #[default]
    Top,
    /// Lay tabs out vertically in a bounded rail on the left.
    Left,
}

impl TabBarPosition {
    pub const ALL: &[Self] = &[Self::Top, Self::Left];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Top => "Top",
            Self::Left => "Left",
        }
    }
}

/// Maximum serialized path length accepted for a user-selected background.
pub const MAX_BACKGROUND_IMAGE_PATH_BYTES: usize = 4096;

/// How a static background image is mapped into the terminal content viewport.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundImageFit {
    /// Fill the viewport while preserving aspect ratio and cropping centrally.
    #[default]
    Cover,
    /// Show the complete image while preserving aspect ratio and letterboxing.
    Contain,
    /// Fill the viewport without preserving aspect ratio.
    Stretch,
    /// Keep the image at its decoded pixel size, centered and clipped.
    Center,
}

impl BackgroundImageFit {
    pub const ALL: &[Self] = &[Self::Cover, Self::Contain, Self::Stretch, Self::Center];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Cover => "Cover",
            Self::Contain => "Contain",
            Self::Stretch => "Stretch",
            Self::Center => "Center",
        }
    }
}

/// Optional, local-only terminal background image policy.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct BackgroundImageConfig {
    /// PNG or JPEG path. Relative paths are resolved beside `config.toml`.
    pub path: Option<PathBuf>,
    /// Image alpha over the theme background.
    #[serde(serialize_with = "serialize_shortest_f32")]
    pub opacity: f32,
    /// Viewport mapping policy.
    pub fit: BackgroundImageFit,
}

impl Default for BackgroundImageConfig {
    fn default() -> Self {
        Self {
            path: None,
            opacity: 0.2,
            fit: BackgroundImageFit::Cover,
        }
    }
}

impl BackgroundImageConfig {
    fn validate(&self) -> Result<()> {
        validate_range(
            "window.background_image.opacity",
            self.opacity,
            BACKGROUND_IMAGE_OPACITY_RANGE,
            None,
        )?;
        if let Some(path) = self.path.as_ref() {
            let path = path.as_os_str().to_string_lossy();
            if path.is_empty() {
                bail!("window.background_image.path must not be empty");
            }
            if path.len() > MAX_BACKGROUND_IMAGE_PATH_BYTES {
                bail!(
                    "window.background_image.path must not exceed {MAX_BACKGROUND_IMAGE_PATH_BYTES} bytes"
                );
            }
            if path.contains('\0') {
                bail!("window.background_image.path must not contain NUL");
            }
        }
        Ok(())
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
    #[serde(serialize_with = "serialize_shortest_f32")]
    pub opacity: f32,
    /// Padding around terminal content
    pub padding: u32,
    /// Show window decorations
    pub decorations: bool,
    /// Requested desktop stacking level for ordinary windows.
    pub level: WindowLevelConfig,
    /// Horizontal top tabs or a vertical left tab rail.
    pub tab_bar_position: TabBarPosition,
    /// Open a new window in the running Termeric on this display, so every
    /// window shares one process and tabs can move between them.
    pub share_process: bool,
    /// Optional bounded static background for the terminal content viewport.
    pub background_image: BackgroundImageConfig,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            width: 800,
            height: 600,
            opacity: 1.0,
            padding: 8,
            decorations: true,
            level: WindowLevelConfig::Normal,
            tab_bar_position: TabBarPosition::Top,
            share_process: true,
            background_image: BackgroundImageConfig::default(),
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

/// `[session]`: automatic saving and restoring of workspaces and closed-tab
/// snapshots, the two things that carry a session across restarts.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SessionConfig {
    /// Allow automatic saving and restoring at all; manual workspace saves
    /// work either way
    pub enabled: bool,
    /// Automatically restore the last referenced workspace on startup
    pub auto_restore: bool,
    /// Automatically save the complete workspace on exit
    pub auto_save: bool,
    /// Save bounded scrollback content with workspaces and closed-tab snapshots
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn shipped_example_config_loads() {
        let example =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/config.example.toml");
        Config::load_from(&example).unwrap();
    }

    #[test]
    fn accessibility_preferences_default_to_system_and_round_trip() {
        let defaults: Config = toml::from_str("").unwrap();
        assert_eq!(defaults.accessibility, AccessibilityConfig::default());
        assert_eq!(
            defaults.accessibility.reduce_motion,
            AccessibilityPreference::System
        );

        let mut config = Config::default();
        config.accessibility.reduce_motion = AccessibilityPreference::On;
        config.accessibility.high_contrast = AccessibilityPreference::Off;
        let encoded = toml::to_string(&config).unwrap();
        assert!(encoded.contains("reduce_motion = \"on\""));
        assert!(encoded.contains("high_contrast = \"off\""));
        let decoded: Config = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.accessibility, config.accessibility);
    }

    #[test]
    fn accessibility_preferences_resolve_system_and_explicit_policies() {
        assert!(AccessibilityPreference::System.resolve(Some(true)));
        assert!(!AccessibilityPreference::System.resolve(Some(false)));
        assert!(!AccessibilityPreference::System.resolve(None));
        assert!(AccessibilityPreference::On.resolve(None));
        assert!(!AccessibilityPreference::Off.resolve(Some(true)));
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
        config.scrollback.max_memory_mib = 0;
        assert!(config.validate().is_err());

        config = Config::default();
        config.harvest.max_history = 0;
        assert!(config.validate().is_err());

        config = Config::default();
        config.copy_mode.patterns[0].regex = "(".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn shared_numeric_bounds_accept_their_ends_and_refuse_beyond() {
        let mut config = Config::default();
        config.window.padding = *WINDOW_PADDING_RANGE.end();
        config.cursor.blink_rate = *CURSOR_BLINK_RATE_RANGE.end();
        config.bell.visual_duration_ms = *BELL_DURATION_RANGE.start();
        config.font.line_height = *LINE_HEIGHT_RANGE.start();
        config.validate().unwrap();

        config.window.padding += 1;
        let error = config.validate().unwrap_err().to_string();
        assert_eq!(error, "window.padding must be between 0 and 256 pixels");
    }

    #[test]
    fn range_errors_read_like_the_documentation() {
        let error = |edit: fn(&mut Config)| {
            let mut config = Config::default();
            edit(&mut config);
            config.validate().unwrap_err().to_string()
        };
        assert_eq!(
            error(|config| config.window.opacity = 0.05),
            "window.opacity must be between 0.1 and 1.0"
        );
        assert_eq!(
            error(|config| config.font.size = 100.0),
            "font.size must be between 8.0 and 72.0 points"
        );
        assert_eq!(
            error(|config| config.cursor.blink_rate = 10),
            "cursor.blink_rate must be between 100 and 5000 milliseconds"
        );
        assert_eq!(
            error(|config| config.scrollback.scroll_speed = f64::NAN),
            "scrollback.scroll_speed must be between 0.1 and 100.0"
        );
    }

    #[test]
    fn copy_mode_defaults_are_bounded_and_user_patterns_deserialize() {
        let defaults = Config::default();
        assert_eq!(defaults.copy_mode.bindings, CopyModeBindings::Vi);
        assert_eq!(defaults.copy_mode.patterns.len(), 3);
        defaults.validate().unwrap();

        let config: Config = toml::from_str(
            r#"
                [copy_mode]
                bindings = "emacs"

                [[copy_mode.patterns]]
                name = "ticket"
                regex = "PROJ-[0-9]+"
            "#,
        )
        .unwrap();
        assert_eq!(config.copy_mode.bindings, CopyModeBindings::Emacs);
        assert_eq!(config.copy_mode.patterns[0].name, "ticket");
        config.validate().unwrap();
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
    fn font_fallback_chain_is_bounded_unique_and_control_free() {
        let mut config = Config::default();
        config.font.family = "Fira Mono".into();
        config.font.fallback = vec!["Noto Sans Mono".into(), "Noto Color Emoji".into()];
        config.validate().unwrap();

        config.font.fallback.push("fira mono".into());
        assert!(config.validate().is_err());

        config.font.fallback = vec!["Noto\nSans".into()];
        assert!(config.validate().is_err());

        config.font.fallback = (0..=MAX_FONT_FALLBACKS)
            .map(|index| format!("Fallback {index}"))
            .collect();
        assert!(config.validate().is_err());

        config.font.fallback = vec!["x".repeat(MAX_FONT_FAMILY_BYTES + 1)];
        assert!(config.validate().is_err());
    }

    #[test]
    fn font_features_are_typed_bounded_safe_and_deterministic() {
        let config: Config = toml::from_str(
            r#"
                [font.features]
                calt = 0
                cv01 = 2
                ss01 = 1
            "#,
        )
        .unwrap();
        config.validate().unwrap();
        assert_eq!(config.font.features["calt"], 0);
        assert_eq!(config.font.features["cv01"], 2);
        assert_eq!(config.font.features["ss01"], 1);

        let encoded = toml::to_string(&config).unwrap();
        let decoded: Config = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.font.features, config.font.features);
        assert!(encoded.find("calt = 0").unwrap() < encoded.find("ss01 = 1").unwrap());

        for tag in ["abc", "abcde", "SS01", "ab-c", "kern", "éééé"] {
            let mut invalid = Config::default();
            invalid.font.features.insert(tag.into(), 1);
            assert!(invalid.validate().is_err(), "accepted feature tag {tag:?}");
        }

        let mut too_many = Config::default();
        too_many.font.features = (0..=MAX_FONT_FEATURES)
            .map(|index| (format!("x{index:03}"), index as u32))
            .collect();
        assert!(too_many.validate().is_err());

        for invalid_toml in [
            "[font.features]\nss01 = -1\n",
            "[font.features]\nss01 = 4294967296\n",
            "[font.features]\nss01 = 1\nss01 = 2\n",
        ] {
            assert!(toml::from_str::<Config>(invalid_toml).is_err());
        }
    }

    #[test]
    fn font_weights_are_typed_bounded_ordered_and_default_compatible() {
        let defaults: Config = toml::from_str("[font]\n").unwrap();
        assert_eq!(defaults.font.weight, DEFAULT_FONT_WEIGHT);
        assert_eq!(defaults.font.bold_weight, DEFAULT_BOLD_FONT_WEIGHT);
        defaults.validate().unwrap();

        let configured: Config = toml::from_str(
            r#"
                [font]
                weight = 325
                bold_weight = 825
            "#,
        )
        .unwrap();
        configured.validate().unwrap();
        assert_eq!(configured.font.weight, 325);
        assert_eq!(configured.font.bold_weight, 825);

        for (weight, bold_weight) in [(0, 700), (400, 0), (1_001, 1_001), (700, 699)] {
            let mut invalid = Config::default();
            invalid.font.weight = weight;
            invalid.font.bold_weight = bold_weight;
            assert!(
                invalid.validate().is_err(),
                "accepted weight pair {weight}/{bold_weight}"
            );
        }

        let mut equal = Config::default();
        equal.font.weight = 500;
        equal.font.bold_weight = 500;
        equal.validate().unwrap();
    }

    #[test]
    fn font_range_overrides_are_typed_bounded_non_overlapping_scalars() {
        let config: Config = toml::from_str(
            r#"
                [font]
                family = "Fira Mono"

                [[font.range_overrides]]
                start = 0xE000
                end = 0xF8FF
                family = "Symbols Nerd Font Mono"

                [[font.range_overrides]]
                start = 0x1F300
                end = 0x1FAFF
                family = "Noto Color Emoji"
            "#,
        )
        .unwrap();
        config.validate().unwrap();
        assert_eq!(config.font.range_overrides.len(), 2);
        assert_eq!(config.font.range_overrides[0].start, 0xE000);
        assert_eq!(config.font.range_overrides[1].end, 0x1FAFF);

        let encoded = toml::to_string(&config).unwrap();
        let decoded: Config = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.font.range_overrides, config.font.range_overrides);
        decoded.validate().unwrap();
    }

    #[test]
    fn font_range_override_validation_rejects_ambiguous_or_invalid_rules() {
        let rule = |start, end, family: &str| FontRangeOverride {
            start,
            end,
            family: family.into(),
        };
        let rejects = |rules: Vec<FontRangeOverride>| {
            let mut config = Config::default();
            config.font.range_overrides = rules;
            config.validate().is_err()
        };

        assert!(rejects(vec![rule(0x2000, 0x1000, "Symbols")]));
        assert!(rejects(vec![rule(0x10_FFFF, 0x11_0000, "Symbols")]));
        assert!(rejects(vec![rule(0xD7FF, 0xE000, "Symbols")]));
        assert!(rejects(vec![rule(0xD800, 0xD800, "Symbols")]));
        assert!(rejects(vec![rule(0xE000, 0xE100, "Bad\nFamily")]));
        assert!(rejects(vec![
            rule(0xE000, 0xE100, "Symbols A"),
            rule(0xE100, 0xE200, "Symbols B"),
        ]));
        assert!(rejects(
            (0..=MAX_FONT_RANGE_OVERRIDES)
                .map(|index| rule(index as u32, index as u32, "Symbols"))
                .collect()
        ));

        let mut adjacent = Config::default();
        adjacent.font.range_overrides = vec![
            rule(0xE000, 0xE0FF, "Symbols A"),
            rule(0xE100, 0xE1FF, "Symbols B"),
            rule(0x1F300, 0x1FAFF, "Emoji"),
        ];
        adjacent.validate().unwrap();
    }

    #[test]
    fn enum_settings_parse_and_round_trip() {
        type Holds = fn(&Config) -> bool;
        let cases: [(&str, Holds); 4] = [
            ("[window]\nlevel = \"always_on_bottom\"", |config| {
                config.window.level == WindowLevelConfig::AlwaysOnBottom
            }),
            ("[window]\ntab_bar_position = \"left\"", |config| {
                config.window.tab_bar_position == TabBarPosition::Left
            }),
            ("[renderer]\nbackend = \"software\"", |config| {
                config.renderer.backend == RendererBackend::Software
            }),
            ("[keyboard]\nxterm_type = \"vt220\"", |config| {
                config.keyboard.xterm_type == XtermKeyboardTypeConfig::Vt220
            }),
        ];
        for (source, holds) in cases {
            let config: Config = toml::from_str(source).unwrap();
            assert!(holds(&config), "{source}");
            let decoded: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
            assert!(holds(&decoded), "{source} after a round trip");
        }
        assert!(toml::from_str::<Config>("[keyboard]\nxterm_type = \"sun\"").is_err());
    }

    #[test]
    fn enum_settings_default_and_map_to_their_native_types() {
        let defaults = Config::default();
        assert_eq!(defaults.renderer.backend, RendererBackend::Auto);
        assert_eq!(
            defaults.keyboard.xterm_type,
            XtermKeyboardTypeConfig::Normal
        );

        assert_eq!(
            WindowLevelConfig::AlwaysOnBottom.native(),
            winit::window::WindowLevel::AlwaysOnBottom
        );
        assert_eq!(
            WindowLevelConfig::AlwaysOnBottom.native_for_window(true),
            winit::window::WindowLevel::AlwaysOnTop
        );
        assert_eq!(
            XtermKeyboardTypeConfig::Vt220.terminal_type(),
            termeric_core::term::XtermKeyboardType::Vt220
        );
    }

    #[test]
    fn background_image_policy_is_typed_bounded_and_round_trips() {
        let config: Config = toml::from_str(
            r#"
                [window.background_image]
                path = "wallpaper.png"
                opacity = 0.35
                fit = "contain"
            "#,
        )
        .unwrap();
        assert_eq!(
            config.window.background_image.path.as_deref(),
            Some(std::path::Path::new("wallpaper.png"))
        );
        assert_eq!(
            config.window.background_image.fit,
            BackgroundImageFit::Contain
        );
        config.validate().unwrap();

        let encoded = toml::to_string(&config).unwrap();
        let decoded: Config = toml::from_str(&encoded).unwrap();
        assert_eq!(
            decoded.window.background_image,
            config.window.background_image
        );

        let mut invalid = Config::default();
        invalid.window.background_image.opacity = f32::NAN;
        assert!(invalid.validate().is_err());
        invalid.window.background_image.opacity = 0.5;
        invalid.window.background_image.path = Some(PathBuf::from(
            "x".repeat(MAX_BACKGROUND_IMAGE_PATH_BYTES + 1),
        ));
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn bold_bright_compatibility_is_explicit_and_opt_in() {
        assert!(!Config::default().font.bold_is_bright);

        let config: Config = toml::from_str(
            r#"
                [font]
                bold_is_bright = true
            "#,
        )
        .unwrap();
        assert!(config.font.bold_is_bright);
    }

    #[test]
    fn terminal_data_security_defaults_are_conservative_and_bounded() {
        let security = Config::default().security;
        assert!(!security.allow_screen_checksum_reports);
        assert!(!security.allow_kitty_file_transfers);
        assert!(!security.allow_gui_control_input);
        let policy = security.osc52;
        assert!(!policy.allow_read);
        assert!(policy.allow_write);
        assert_eq!(policy.max_bytes, 1024 * 1024);

        let mut config = Config::default();
        config.security.osc52.max_bytes = 0;
        assert!(config.validate().is_err());
        config.security.osc52.max_bytes = termeric_core::ansi::MAX_OSC52_DECODED_BYTES + 1;
        assert!(config.validate().is_err());

        let enabled: Config = toml::from_str(
            r#"
                [security]
                allow_screen_checksum_reports = true
                allow_kitty_file_transfers = true
                allow_gui_control_input = true
            "#,
        )
        .unwrap();
        assert!(enabled.security.allow_screen_checksum_reports);
        assert!(enabled.security.allow_kitty_file_transfers);
        assert!(enabled.security.allow_gui_control_input);
    }

    #[test]
    fn ssh_host_profiles_are_bounded_validated_and_round_trip() {
        let config: Config = toml::from_str(
            r#"
                [domains.ssh_hosts.build]
                destination = "developer@build.example"
                description = "Build farm"

                [domains.ssh_hosts.production]
                destination = "prod-alias"
            "#,
        )
        .unwrap();
        config.validate().unwrap();
        assert_eq!(
            config.domains.ssh_host("build").unwrap(),
            &SshHostProfile {
                destination: "developer@build.example".into(),
                description: Some("Build farm".into()),
            }
        );
        assert_eq!(
            toml::from_str::<Config>(&toml::to_string(&config).unwrap())
                .unwrap()
                .domains
                .ssh_host("production")
                .unwrap()
                .destination,
            "prod-alias"
        );
        assert!(config.domains.ssh_host("missing").is_err());
    }

    #[test]
    fn ssh_host_profiles_fail_closed_on_unsafe_or_excessive_data() {
        for source in [
            "[domains.ssh_hosts.'bad name']\ndestination = 'host'",
            "[domains.ssh_hosts.build]\ndestination = '-oProxyCommand=bad'",
            "[domains.ssh_hosts.build]\ndestination = 'ssh://host'",
            "[domains.ssh_hosts.build]\ndestination = 'two hosts'",
            "[domains.ssh_hosts.build]\ndestination = 'host'\ndescription = ''",
        ] {
            let config: Config = toml::from_str(source).unwrap();
            assert!(config.validate().is_err(), "accepted {source:?}");
        }

        let mut config = Config::default();
        for index in 0..=DomainsConfig::MAX_SSH_HOSTS {
            config.domains.ssh_hosts.insert(
                format!("host-{index}"),
                SshHostProfile {
                    destination: format!("host-{index}.example"),
                    description: None,
                },
            );
        }
        assert!(config.validate().is_err());
    }

    #[test]
    fn terminal_profiles_are_validated_and_round_trip() {
        let mut config: Config = toml::from_str(
            r#"
                [profiles.dev]
                description = "Development shell"
                program = "/bin/sh"
                args = ["-l"]
                title = "Development"
                env = { RUST_BACKTRACE = "1", PROFILE_MARK = "dev" }
            "#,
        )
        .unwrap();
        let expected_cwd = std::env::temp_dir();
        config.profiles.get_mut("dev").unwrap().cwd = Some(expected_cwd.clone());
        config.validate().unwrap();
        let profile = config.launch_profile("dev").unwrap();
        assert_eq!(profile.program, "/bin/sh");
        assert_eq!(profile.cwd.as_deref(), Some(expected_cwd.as_path()));
        assert_eq!(
            profile.env.get("PROFILE_MARK").map(String::as_str),
            Some("dev")
        );
        assert_eq!(profile.shell_config().program.as_deref(), Some("/bin/sh"));

        let decoded: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
        assert_eq!(decoded.launch_profile("dev").unwrap(), profile);
        assert!(decoded.launch_profile("missing").is_err());
    }

    #[test]
    fn terminal_profile_names_follow_one_rule() {
        let longest = "a".repeat(64);
        for name in ["dev", "dev.rust", "a-b_c.d", "trailing.", longest.as_str()] {
            validate_terminal_profile_name(name).unwrap();
            let config: Config =
                toml::from_str(&format!("[profiles.'{name}']\nprogram = '/bin/sh'")).unwrap();
            config.validate().unwrap();
        }
        let too_long = "a".repeat(65);
        for name in [
            "",
            ".dev",
            "a..b",
            "..",
            "bad name",
            "dév",
            too_long.as_str(),
        ] {
            assert!(validate_terminal_profile_name(name).is_err(), "{name:?}");
            let config: Config =
                toml::from_str(&format!("[profiles.'{name}']\nprogram = '/bin/sh'")).unwrap();
            assert!(config.validate().is_err(), "accepted [profiles.{name:?}]");
        }
    }

    #[test]
    fn terminal_profiles_reject_ambiguous_process_configuration() {
        for source in [
            "[profiles.'bad name']\nprogram = '/bin/sh'",
            "[profiles.dev]\nprogram = ''",
            "[profiles.dev]\nprogram = '/bin/sh'\ncwd = 'relative'",
            "[profiles.dev]\nprogram = '/bin/sh'\nenv = { TERM = 'spoofed' }",
            "[profiles.dev]\nprogram = '/bin/sh'\nenv = { 'BAD-KEY' = 'value' }",
            "[profiles.dev]\nprogram = '/bin/sh'\nargs = [\"a\\u0000b\"]",
            "[profiles.dev]\nprogram = '/bin/sh'\nenv = { MARK = \"a\\u0000b\" }",
        ] {
            let config: Config = toml::from_str(source).unwrap();
            assert!(config.validate().is_err(), "accepted {source:?}");
        }

        for text in [
            "two\nlines",
            "tab\there",
            "a\u{202e}txt.exe",
            "\u{2066}x",
            " ",
        ] {
            for field in ["title", "description"] {
                let mut profile = LaunchProfileConfig {
                    program: "/bin/sh".into(),
                    ..LaunchProfileConfig::default()
                };
                let slot = match field {
                    "title" => &mut profile.title,
                    _ => &mut profile.description,
                };
                *slot = Some(text.into());
                let mut config = Config::default();
                config.profiles.insert("dev".into(), profile);
                let error = config.validate().unwrap_err().to_string();
                assert!(
                    error.contains(&format!("{field} must be one non-blank line")),
                    "{field} {text:?}: {error}"
                );
            }
        }
        let mut displayable = Config::default();
        displayable.profiles.insert(
            "dev".into(),
            LaunchProfileConfig {
                program: "/bin/sh".into(),
                title: Some("Café 界 — build".into()),
                description: Some("עברית mixed with English".into()),
                ..LaunchProfileConfig::default()
            },
        );
        displayable.validate().unwrap();

        for reserved in ["TERM", "PWD", "TERMERIC_TERMINAL_ID"] {
            let mut reserved_shell_env = Config::default();
            reserved_shell_env
                .shell
                .env
                .insert(reserved.into(), "spoofed".into());
            assert!(reserved_shell_env.validate().is_err(), "{reserved}");
        }
    }

    #[test]
    fn serial_terminal_profiles_are_typed_bounded_and_unambiguous() {
        let config: Config = toml::from_str(
            r#"
                [profiles.console]
                description = "Development board"
                title = "Board console"

                [profiles.console.serial]
                device = "/dev/ttyUSB0"
                baud = 115200
                data_bits = 8
                parity = "none"
                stop_bits = 1
                flow_control = "hardware"
            "#,
        )
        .unwrap();
        config.validate().unwrap();
        let profile = config.launch_profile("console").unwrap();
        assert_eq!(
            profile.serial_config().unwrap(),
            &SerialProfileConfig {
                device: PathBuf::from("/dev/ttyUSB0"),
                baud: 115_200,
                data_bits: 8,
                parity: SerialParity::None,
                stop_bits: 1,
                flow_control: SerialFlowControl::Hardware,
            }
        );
        let decoded: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
        assert_eq!(decoded.launch_profile("console").unwrap(), profile);

        for source in [
            "[profiles.bad.serial]\ndevice = 'relative'",
            "[profiles.bad.serial]\ndevice = '/dev/ttyS0'\nbaud = 12345",
            "[profiles.bad.serial]\ndevice = '/dev/ttyS0'\ndata_bits = 9",
            "[profiles.bad.serial]\ndevice = '/dev/ttyS0'\nstop_bits = 3",
            "[profiles.bad]\nprogram = '/bin/sh'\n[profiles.bad.serial]\ndevice = '/dev/ttyS0'",
            "[profiles.bad]\ncwd = '/tmp'\n[profiles.bad.serial]\ndevice = '/dev/ttyS0'",
        ] {
            let invalid: Config = toml::from_str(source).unwrap();
            assert!(invalid.validate().is_err(), "accepted {source:?}");
        }
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
