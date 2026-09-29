//! Configuration management

pub mod theme;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::collections::BTreeMap;
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
    /// Retired `[plugins.*]` agent sections, read only to migrate them into
    /// `agents` and never written back.
    #[serde(default, skip_serializing)]
    pub plugins: PluginsConfig,
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
/// uses an explicit application override. Boolean values from older config
/// files remain accepted as explicit on/off choices.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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

impl Serialize for AccessibilityPreference {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match self {
            Self::System => "system",
            Self::On => "on",
            Self::Off => "off",
        })
    }
}

impl<'de> Deserialize<'de> for AccessibilityPreference {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum WireValue {
            LegacyBool(bool),
            Name(String),
        }

        match WireValue::deserialize(deserializer)? {
            WireValue::LegacyBool(true) => Ok(Self::On),
            WireValue::LegacyBool(false) => Ok(Self::Off),
            WireValue::Name(name) => match name.as_str() {
                "system" => Ok(Self::System),
                "on" => Ok(Self::On),
                "off" => Ok(Self::Off),
                _ => Err(de::Error::unknown_variant(&name, &["system", "on", "off"])),
            },
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
            let mut config: Config = toml::from_str(&content)
                .with_context(|| format!("failed to parse {}", config_path.display()))?;
            config.migrate_retired_agent_sections();
            config.validate()?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    /// Refuse to replace the configuration file at `path` when it does not
    /// load. Termeric is then running on its defaults or on the last settings
    /// that did load, and writing either over the file would destroy the
    /// person's settings, which may be one typo away from working.
    pub fn ensure_replaceable(path: &std::path::Path) -> Result<()> {
        match Self::load_from(path) {
            Ok(_) => Ok(()),
            Err(error) => bail!(
                "{} has an error, so it was left as it is ({error:#}); fix it, or use Reset Configuration to replace it",
                path.display()
            ),
        }
    }

    /// Move settings from the retired `[plugins.claude_code]` and
    /// `[plugins.codex]` sections into `[agents.*]`, unless an agent entry
    /// already exists. The next save writes only the new form.
    pub fn migrate_retired_agent_sections(&mut self) {
        let plugins = std::mem::take(&mut self.plugins);
        let migrated = [
            (
                "claude-code",
                plugins
                    .claude_code
                    .as_ref()
                    .and_then(crate::agents::config::migrate_claude_code),
            ),
            (
                "codex",
                plugins
                    .codex
                    .as_ref()
                    .filter(|_| !self.agents.contains_key("codex"))
                    .and_then(|legacy| {
                        crate::agents::config::migrate_codex(legacy).unwrap_or_else(|error| {
                            // The retired plugin refused to launch with
                            // this; guessing a policy could be less careful.
                            log::warn!(
                                "{error:#}; Codex stays turned off until [agents.codex] says how to start it"
                            );
                            Some(crate::agents::AgentConfig {
                                enabled: Some(false),
                                ..Default::default()
                            })
                        })
                    }),
            ),
        ];
        for (id, entry) in migrated {
            if let Some(entry) = entry {
                self.agents.entry(id.to_string()).or_insert(entry);
            }
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
        if !(8.0..=72.0).contains(&self.font.size) || !self.font.size.is_finite() {
            bail!("font.size must be between 8 and 72");
        }
        if !(MIN_FONT_WEIGHT..=MAX_FONT_WEIGHT).contains(&self.font.weight) {
            bail!("font.weight must be between {MIN_FONT_WEIGHT} and {MAX_FONT_WEIGHT}");
        }
        if !(MIN_FONT_WEIGHT..=MAX_FONT_WEIGHT).contains(&self.font.bold_weight) {
            bail!("font.bold_weight must be between {MIN_FONT_WEIGHT} and {MAX_FONT_WEIGHT}");
        }
        if self.font.bold_weight < self.font.weight {
            bail!("font.bold_weight must be greater than or equal to font.weight");
        }
        if !(0.8..=3.0).contains(&self.font.line_height) || !self.font.line_height.is_finite() {
            bail!("font.line_height must be between 0.8 and 3.0");
        }
        if !(100..=1_000_000).contains(&self.scrollback.lines) {
            bail!("scrollback.lines must be between 100 and 1000000");
        }
        if !(8..=4_096).contains(&self.scrollback.max_memory_mib) {
            bail!("scrollback.max_memory_mib must be between 8 and 4096");
        }
        if !(0.1..=100.0).contains(&self.scrollback.scroll_speed)
            || !self.scrollback.scroll_speed.is_finite()
        {
            bail!("scrollback.scroll_speed must be between 0.1 and 100");
        }
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
        if !(0.1..=1.0).contains(&self.window.opacity) || !self.window.opacity.is_finite() {
            bail!("window.opacity must be between 0.1 and 1.0");
        }
        self.window.background_image.validate()?;
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
        if !(1..=termeric_core::ansi::MAX_OSC52_DECODED_BYTES)
            .contains(&self.security.osc52.max_bytes)
        {
            bail!("security.osc52.max_bytes must be between 1 and 1048576");
        }
        if !(1..=10_000).contains(&self.harvest.max_history) {
            bail!("harvest.max_history must be between 1 and 10000");
        }
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
        validate_launch_profile_name(name)?;
        self.profiles.get(name).ok_or_else(|| {
            anyhow::anyhow!("unknown terminal profile {name:?}; configure [profiles.{name}]")
        })
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

const MAX_LAUNCH_PROFILES: usize = 64;
const MAX_PROFILE_ARGS: usize = 64;
const MAX_PROFILE_ENV: usize = 64;
const MAX_PROFILE_TOTAL_BYTES: usize = 64 * 1024;

fn validate_launch_profiles(profiles: &BTreeMap<String, LaunchProfileConfig>) -> Result<()> {
    if profiles.len() > MAX_LAUNCH_PROFILES {
        bail!("profiles must contain at most {MAX_LAUNCH_PROFILES} entries");
    }
    for (name, profile) in profiles {
        validate_launch_profile_name(name)?;
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
            if profile.program.trim().is_empty()
                || profile.program.len() > 4096
                || !is_single_line(&profile.program)
            {
                bail!("terminal profile {name:?} has an invalid program");
            }
            if profile.args.len() > MAX_PROFILE_ARGS
                || profile
                    .args
                    .iter()
                    .any(|arg| arg.len() > 4096 || arg.contains('\0'))
            {
                bail!(
                    "terminal profile {name:?} must contain at most {MAX_PROFILE_ARGS} arguments of at most 4096 bytes"
                );
            }
        }
        if profile.description.as_ref().is_some_and(|value| {
            value.trim().is_empty() || value.len() > 512 || !is_single_line(value)
        }) {
            bail!("terminal profile {name:?} has an invalid description");
        }
        if profile.title.as_ref().is_some_and(|value| {
            value.trim().is_empty() || value.chars().count() > 256 || !is_single_line(value)
        }) {
            bail!("terminal profile {name:?} has an invalid title");
        }
        if profile.cwd.as_ref().is_some_and(|path| !path.is_absolute()) {
            bail!("terminal profile {name:?} cwd must be an absolute path");
        }
        validate_child_environment(&format!("terminal profile {name:?}"), &profile.env)?;
        let mut total_bytes = profile.program.len()
            + profile.args.iter().map(String::len).sum::<usize>()
            + profile
                .cwd
                .as_ref()
                .map_or(0, |path| path.as_os_str().len())
            + profile.title.as_ref().map_or(0, String::len)
            + profile.description.as_ref().map_or(0, String::len);
        total_bytes = total_bytes.saturating_add(
            profile
                .serial
                .as_ref()
                .map_or(0, |serial| serial.device.as_os_str().len()),
        );
        for (key, value) in &profile.env {
            total_bytes = total_bytes.saturating_add(key.len() + value.len());
        }
        if total_bytes > MAX_PROFILE_TOTAL_BYTES {
            bail!(
                "terminal profile {name:?} exceeds the {MAX_PROFILE_TOTAL_BYTES}-byte configuration budget"
            );
        }
    }
    Ok(())
}

fn validate_launch_profile_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        bail!("invalid terminal profile name {name:?}");
    }
    Ok(())
}

fn validate_child_environment(owner: &str, env: &BTreeMap<String, String>) -> Result<()> {
    if env.len() > MAX_PROFILE_ENV {
        bail!("{owner} must contain at most {MAX_PROFILE_ENV} environment variables");
    }
    let mut total_bytes = 0usize;
    for (key, value) in env {
        validate_child_env_key(owner, key)?;
        if value.len() > 8192 || value.contains('\0') {
            bail!("{owner} environment value {key:?} is invalid");
        }
        total_bytes = total_bytes.saturating_add(key.len() + value.len());
    }
    if total_bytes > MAX_PROFILE_TOTAL_BYTES {
        bail!("{owner} environment exceeds {MAX_PROFILE_TOTAL_BYTES} bytes");
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
    if !valid || key.len() > 128 || reserved {
        bail!("{owner} has invalid or reserved environment key {key:?}");
    }
    Ok(())
}

fn is_single_line(value: &str) -> bool {
    !value.contains(['\0', '\n', '\r'])
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
    pub size: f32,
    /// OpenType `wght` value used for ordinary terminal text.
    pub weight: u16,
    /// OpenType `wght` value used for SGR bold terminal text.
    pub bold_weight: u16,
    /// Line height multiplier (e.g. 1.2 = 120%)
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
        if !(0.0..=1.0).contains(&self.opacity) || !self.opacity.is_finite() {
            bail!("window.background_image.opacity must be between 0.0 and 1.0");
        }
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

/// Session save/restore configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SessionConfig {
    /// Enable workspace save/restore (legacy key name retained for compatibility)
    pub enabled: bool,
    /// Automatically restore the last referenced workspace on startup
    pub auto_restore: bool,
    /// Automatically save the complete workspace on exit
    pub auto_save: bool,
    /// Save bounded scrollback content with workspaces and legacy sessions
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

/// Retired `[plugins]` section: agent settings from earlier releases, read
/// only so they can migrate into `[agents.*]`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PluginsConfig {
    pub claude_code: Option<crate::agents::config::LegacyClaudeCodeConfig>,
    pub codex: Option<crate::agents::config::LegacyCodexConfig>,
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
    fn accessibility_preferences_accept_legacy_booleans_as_explicit_overrides() {
        let config: Config =
            toml::from_str("[accessibility]\nreduce_motion = true\nhigh_contrast = false\n")
                .unwrap();
        assert_eq!(
            config.accessibility.reduce_motion,
            AccessibilityPreference::On
        );
        assert_eq!(
            config.accessibility.high_contrast,
            AccessibilityPreference::Off
        );
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
    fn window_levels_are_typed_and_round_trip() {
        let config: Config = toml::from_str(
            r#"
                [window]
                level = "always_on_bottom"
            "#,
        )
        .unwrap();
        assert_eq!(config.window.level, WindowLevelConfig::AlwaysOnBottom);
        assert_eq!(
            config.window.level.native(),
            winit::window::WindowLevel::AlwaysOnBottom
        );
        assert_eq!(
            config.window.level.native_for_window(true),
            winit::window::WindowLevel::AlwaysOnTop
        );

        let encoded = toml::to_string(&config).unwrap();
        let decoded: Config = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.window.level, WindowLevelConfig::AlwaysOnBottom);
    }

    #[test]
    fn tab_bar_positions_are_typed_and_round_trip() {
        let config: Config = toml::from_str(
            r#"
                [window]
                tab_bar_position = "left"
            "#,
        )
        .unwrap();
        assert_eq!(config.window.tab_bar_position, TabBarPosition::Left);

        let encoded = toml::to_string(&config).unwrap();
        let decoded: Config = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.window.tab_bar_position, TabBarPosition::Left);
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
    fn renderer_backend_defaults_to_auto_and_round_trips_software() {
        assert_eq!(Config::default().renderer.backend, RendererBackend::Auto);
        let config: Config = toml::from_str(
            r#"
                [renderer]
                backend = "software"
            "#,
        )
        .unwrap();
        assert_eq!(config.renderer.backend, RendererBackend::Software);
        let encoded = toml::to_string(&config).unwrap();
        assert_eq!(
            toml::from_str::<Config>(&encoded).unwrap().renderer.backend,
            RendererBackend::Software
        );
    }

    #[test]
    fn xterm_keyboard_type_defaults_to_normal_and_round_trips_historical_types() {
        assert_eq!(
            Config::default().keyboard.xterm_type,
            XtermKeyboardTypeConfig::Normal
        );
        let config: Config = toml::from_str(
            r#"
                [keyboard]
                xterm_type = "vt220"
            "#,
        )
        .unwrap();
        assert_eq!(config.keyboard.xterm_type, XtermKeyboardTypeConfig::Vt220);
        assert_eq!(
            config.keyboard.xterm_type.terminal_type(),
            termeric_core::term::XtermKeyboardType::Vt220
        );
        let encoded = toml::to_string(&config).unwrap();
        assert_eq!(
            toml::from_str::<Config>(&encoded)
                .unwrap()
                .keyboard
                .xterm_type,
            XtermKeyboardTypeConfig::Vt220
        );
        assert!(toml::from_str::<Config>("[keyboard]\nxterm_type = \"sun\"").is_err());
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
    fn terminal_profiles_are_bounded_validated_and_round_trip() {
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
    fn terminal_profiles_reject_ambiguous_or_unbounded_process_configuration() {
        for source in [
            "[profiles.'bad name']\nprogram = '/bin/sh'",
            "[profiles.dev]\nprogram = ''",
            "[profiles.dev]\nprogram = '/bin/sh'\ncwd = 'relative'",
            "[profiles.dev]\nprogram = '/bin/sh'\nenv = { TERM = 'spoofed' }",
            "[profiles.dev]\nprogram = '/bin/sh'\nenv = { 'BAD-KEY' = 'value' }",
        ] {
            let config: Config = toml::from_str(source).unwrap();
            assert!(config.validate().is_err(), "accepted {source:?}");
        }

        let mut multiline = Config::default();
        multiline.profiles.insert(
            "dev".into(),
            LaunchProfileConfig {
                program: "/bin/sh".into(),
                title: Some("two\nlines".into()),
                ..LaunchProfileConfig::default()
            },
        );
        assert!(multiline.validate().is_err());

        for reserved in ["TERM", "PWD", "TERMERIC_TERMINAL_ID"] {
            let mut reserved_shell_env = Config::default();
            reserved_shell_env
                .shell
                .env
                .insert(reserved.into(), "spoofed".into());
            assert!(reserved_shell_env.validate().is_err(), "{reserved}");
        }

        let mut config = Config::default();
        for index in 0..=MAX_LAUNCH_PROFILES {
            config.profiles.insert(
                format!("profile-{index}"),
                LaunchProfileConfig {
                    program: "/bin/sh".into(),
                    ..LaunchProfileConfig::default()
                },
            );
        }
        assert!(config.validate().is_err());
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
    fn retired_agent_sections_migrate_once_and_are_not_written_back() {
        let mut config: Config = toml::from_str(
            r#"
            [plugins.claude_code]
            approval_mode = "auto-accept"
            [plugins.codex]
            enabled = true
            approval_mode = "full-auto"
            sandbox_mode = "workspace-write"
            [agents.codex]
            args = ["--kept"]
            "#,
        )
        .unwrap();
        config.migrate_retired_agent_sections();
        config.validate().unwrap();
        assert_eq!(
            config.agents["claude-code"].args.as_deref(),
            Some(&["--dangerously-skip-permissions".to_string()][..])
        );
        // An existing agent entry always wins over a retired section.
        assert_eq!(
            config.agents["codex"].args.as_deref(),
            Some(&["--kept".to_string()][..])
        );
        let encoded = toml::to_string(&config).unwrap();
        assert!(!encoded.contains("[plugins"));
        assert!(encoded.contains("[agents.claude-code]"));

        // A name the retired plugin refused never costs the rest of the
        // configuration: Codex is turned off instead, and an existing entry
        // makes the retired section irrelevant.
        let mut config: Config =
            toml::from_str("[window]\nwidth = 1234\n[plugins.codex]\napproval_mode = \"yolo\"\n")
                .unwrap();
        config.migrate_retired_agent_sections();
        config.validate().unwrap();
        assert_eq!(config.window.width, 1234);
        assert_eq!(config.agents["codex"].enabled, Some(false));
        let mut config: Config = toml::from_str(
            "[plugins.codex]\napproval_mode = \"yolo\"\n[agents.codex]\nargs = []\n",
        )
        .unwrap();
        config.migrate_retired_agent_sections();
        assert_eq!(config.agents["codex"].enabled, None);
    }

    #[test]
    fn a_configuration_that_does_not_load_is_never_saved_over() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        Config::ensure_replaceable(&path).expect("no file yet");
        std::fs::write(&path, "[window]\nwidth = 1234\n").unwrap();
        Config::ensure_replaceable(&path).expect("a file that loads");
        // One typo away from working: the person's settings must survive.
        std::fs::write(&path, "[window]\nwidth = 1234\n[font\nsize = 13\n").unwrap();
        let error = Config::ensure_replaceable(&path).unwrap_err().to_string();
        assert!(error.contains("Reset Configuration"), "{error}");
        assert!(error.contains("config.toml"), "{error}");
        // So is one that parses but is refused by validation.
        std::fs::write(&path, "[window]\nwidth = 0\n").unwrap();
        assert!(Config::ensure_replaceable(&path).is_err());
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
