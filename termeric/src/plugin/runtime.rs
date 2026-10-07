//! Installed declarative third-party plugins.
//!
//! No public version executes third-party code in the Termeric process. A
//! validated manifest may declare argv-preserving launch profiles, typed
//! non-secret settings, palette commands, and one bounded out-of-process event
//! handler. Every capability is requested in the manifest, reviewed, and
//! explicitly granted before it takes effect; window-acting capabilities are
//! limited to the plugin's own window unless granted with `:all-windows`.

use super::hooks::{Hook, Plugin, PluginAction, PluginCommand, PluginContext};
use super::permission::{Capability, PermissionScope, PluginPermission, PluginPermissions};
use super::structured::{
    HandlerAuthority, MAX_PERSISTENT_SOURCE_EVENT_BYTES, StructuredHandlerInvocation,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

/// The one declarative plugin API version. Manifests must declare it exactly;
/// capabilities, not versions, decide what a plugin may do.
pub const PLUGIN_API_VERSION: u32 = 1;
pub const REGISTRY_SCHEMA_VERSION: u32 = 1;
pub const MANIFEST_NAME: &str = "termeric-plugin.toml";
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_REGISTRY_BYTES: usize = 512 * 1024;
const MAX_SOURCE_PATH_BYTES: usize = 16 * 1024;
const MAX_PROFILES: usize = 32;
const MAX_HANDLER_COMMANDS: usize = 32;
const MAX_SETTINGS: usize = 32;
pub(crate) const MAX_SETTING_TEXT_BYTES: usize = 512;
const MAX_SETTING_CHOICES: usize = 32;
const MAX_SETTING_VALUE_BYTES: usize = super::structured::MAX_REQUEST_SETTINGS_BYTES;
const MAX_ARGS: usize = 64;
const MAX_ARG_BYTES: usize = 4096;
const MAX_TOTAL_ARG_BYTES: usize = 32 * 1024;
const MAX_STRUCTURED_CONTEXT_BYTES: usize = MAX_PERSISTENT_SOURCE_EVENT_BYTES;
const MAX_STRUCTURED_COMMAND_JSON_BYTES: usize = 2 * 1024;
const MAX_STRUCTURED_OUTPUT_JSON_BYTES: usize = 8 * 1024;
/// Version of the launch-context and settings documents handed to profiles
/// through the environment; they are versioned apart from the handler protocol.
const ENVIRONMENT_DOCUMENT_SCHEMA_VERSION: u32 = 1;
const STRUCTURED_CONTEXT_ENV: &str = "TERMERIC_PLUGIN_CONTEXT_JSON";
const STRUCTURED_SETTINGS_ENV: &str = "TERMERIC_PLUGIN_SETTINGS_JSON";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub api_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub permissions: PluginPermissions,
    #[serde(default)]
    pub profiles: Vec<DeclarativeProfile>,
    #[serde(default)]
    pub commands: Vec<DeclarativeHandlerCommand>,
    #[serde(default)]
    pub settings: Vec<DeclarativeSetting>,
    #[serde(default)]
    pub event_handler: Option<DeclarativeEventHandler>,
}

/// Immutable metadata shown at the local-plugin trust boundary before install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginInstallPreview {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub source: PathBuf,
    pub source_fingerprint: String,
    pub permissions: PluginPermissions,
    pub profile_count: usize,
    pub command_count: usize,
}

pub(crate) fn preview_install(directory: &Path) -> Result<PluginInstallPreview> {
    let (manifest, source, bytes) = PluginManifest::from_directory(directory)?;
    Ok(PluginInstallPreview {
        id: manifest.id,
        name: manifest.name,
        version: manifest.version,
        description: manifest.description,
        source,
        source_fingerprint: fingerprint(&bytes),
        permissions: manifest.permissions,
        profile_count: manifest.profiles.len(),
        command_count: manifest.commands.len(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DeclarativeSetting {
    Boolean {
        id: String,
        name: String,
        #[serde(default)]
        description: String,
        default: bool,
    },
    Integer {
        id: String,
        name: String,
        #[serde(default)]
        description: String,
        default: i64,
        min: i64,
        max: i64,
    },
    Text {
        id: String,
        name: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        default: String,
    },
    Choice {
        id: String,
        name: String,
        #[serde(default)]
        description: String,
        default: String,
        choices: Vec<String>,
    },
}

impl DeclarativeSetting {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Boolean { id, .. }
            | Self::Integer { id, .. }
            | Self::Text { id, .. }
            | Self::Choice { id, .. } => id,
        }
    }

    pub(crate) fn name(&self) -> &str {
        match self {
            Self::Boolean { name, .. }
            | Self::Integer { name, .. }
            | Self::Text { name, .. }
            | Self::Choice { name, .. } => name,
        }
    }

    pub(crate) fn description(&self) -> &str {
        match self {
            Self::Boolean { description, .. }
            | Self::Integer { description, .. }
            | Self::Text { description, .. }
            | Self::Choice { description, .. } => description,
        }
    }

    pub(crate) fn default_value(&self) -> PluginSettingValue {
        match self {
            Self::Boolean { default, .. } => PluginSettingValue::Boolean(*default),
            Self::Integer { default, .. } => PluginSettingValue::Integer(*default),
            Self::Text { default, .. } | Self::Choice { default, .. } => {
                PluginSettingValue::Text(default.clone())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum PluginSettingValue {
    Boolean(bool),
    Integer(i64),
    Text(String),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeclarativeHandlerCommand {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeclarativeEventHandler {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub events: BTreeSet<StructuredEventKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StructuredEventKind {
    AppReady,
    TabCreated,
    TabClosed,
    TabSwitched,
    CommandCompleted,
    CommandInvoked,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeclarativeProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub working_directory: WorkingDirectory,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkingDirectory {
    #[default]
    Active,
    Inherit,
}

impl PluginManifest {
    pub fn from_directory(directory: &Path) -> Result<(Self, PathBuf, Vec<u8>)> {
        let directory = directory
            .canonicalize()
            .with_context(|| format!("plugin source {} is unavailable", directory.display()))?;
        if !directory.is_dir() {
            bail!("plugin source {} is not a directory", directory.display());
        }
        let path = directory.join(MANIFEST_NAME);
        let bytes = crate::safe_fs::read_bytes(&path, MAX_MANIFEST_BYTES)
            .with_context(|| format!("failed to read plugin manifest {}", path.display()))?;
        let text = std::str::from_utf8(&bytes).context("plugin manifest must be UTF-8")?;
        let manifest: Self = toml::from_str(text).context("invalid plugin manifest TOML")?;
        manifest.validate()?;
        Ok((manifest, directory, bytes))
    }

    pub fn validate(&self) -> Result<()> {
        if self.api_version != PLUGIN_API_VERSION {
            bail!(
                "plugin API version {} is unsupported; Termeric supports API version {PLUGIN_API_VERSION}",
                self.api_version
            );
        }
        validate_identifier("plugin id", &self.id)?;
        if is_reserved_plugin_id(&self.id) {
            bail!(
                "plugin id {:?} is reserved for a built-in integration",
                self.id
            );
        }
        validate_text("plugin name", &self.name, 80)?;
        validate_text("plugin version", &self.version, 64)?;
        validate_optional_text("plugin description", &self.description, 1024)?;
        if self.profiles.len() > MAX_PROFILES {
            bail!("plugins cannot define more than {MAX_PROFILES} profiles");
        }
        if self.commands.len() > MAX_HANDLER_COMMANDS {
            bail!("plugins cannot define more than {MAX_HANDLER_COMMANDS} handler commands");
        }
        if self.settings.len() > MAX_SETTINGS {
            bail!("plugins cannot define more than {MAX_SETTINGS} settings");
        }
        if self.profiles.is_empty() && self.commands.is_empty() && self.event_handler.is_none() {
            bail!("plugins must define at least one profile or an event handler");
        }
        if !self.permissions.contains(Capability::ProcessSpawn) {
            bail!("a declarative launch plugin must request process-spawn");
        }
        let mut ids = BTreeSet::new();
        for profile in &self.profiles {
            validate_identifier("profile id", &profile.id)?;
            if !ids.insert(profile.id.as_str()) {
                bail!("duplicate profile id {:?}", profile.id);
            }
            validate_text("profile name", &profile.name, 80)?;
            validate_optional_text("profile description", &profile.description, 512)?;
            validate_argv(
                &format!("profile {:?}", profile.id),
                &profile.program,
                &profile.args,
            )?;
        }
        let mut command_ids = BTreeSet::new();
        for command in &self.commands {
            validate_identifier("handler command id", &command.id)?;
            if !command_ids.insert(command.id.as_str()) {
                bail!("duplicate handler command id {:?}", command.id);
            }
            if self
                .profiles
                .iter()
                .any(|profile| command.id == format!("open-{}", profile.id))
            {
                bail!(
                    "handler command {:?} conflicts with a generated profile command",
                    command.id
                );
            }
            validate_text("handler command name", &command.name, 80)?;
            validate_optional_text("handler command description", &command.description, 512)?;
        }
        self.validate_settings()?;
        self.validate_event_handler()?;
        self.validate_capability_requirements()
    }

    fn validate_settings(&self) -> Result<()> {
        let mut setting_ids = BTreeSet::new();
        for setting in &self.settings {
            validate_identifier("setting id", setting.id())?;
            if !setting_ids.insert(setting.id()) {
                bail!("duplicate setting id {:?}", setting.id());
            }
            validate_text("setting name", setting.name(), 80)?;
            validate_optional_text("setting description", setting.description(), 512)?;
            match setting {
                DeclarativeSetting::Boolean { .. } => {}
                DeclarativeSetting::Integer {
                    id,
                    default,
                    min,
                    max,
                    ..
                } => {
                    if min > max || !(min..=max).contains(&default) {
                        bail!("integer setting {id:?} requires min <= default <= max");
                    }
                }
                DeclarativeSetting::Text { id, default, .. } => {
                    validate_setting_text(&format!("text setting {id:?} default"), default)?;
                }
                DeclarativeSetting::Choice {
                    id,
                    default,
                    choices,
                    ..
                } => {
                    if choices.is_empty() || choices.len() > MAX_SETTING_CHOICES {
                        bail!("choice setting {id:?} must define 1-{MAX_SETTING_CHOICES} choices");
                    }
                    let mut unique = BTreeSet::new();
                    for choice in choices {
                        validate_text("setting choice", choice, 80)?;
                        if !unique.insert(choice.as_str()) {
                            bail!("choice setting {id:?} contains duplicate value {choice:?}");
                        }
                    }
                    if !unique.contains(default.as_str()) {
                        bail!("choice setting {id:?} default is not one of its choices");
                    }
                }
            }
        }
        if !self.settings.is_empty() {
            effective_settings(self, &BTreeMap::new())?;
        }
        Ok(())
    }

    fn validate_event_handler(&self) -> Result<()> {
        let subscribes = |event| {
            self.event_handler
                .as_ref()
                .is_some_and(|handler| handler.events.contains(&event))
        };
        if !self.commands.is_empty() && !subscribes(StructuredEventKind::CommandInvoked) {
            bail!("handler commands require an event handler subscribed to command-invoked");
        }
        let Some(handler) = &self.event_handler else {
            return Ok(());
        };
        if handler.events.is_empty() {
            bail!("an event handler must subscribe to at least one event");
        }
        validate_argv("event handler", &handler.program, &handler.args)?;
        if subscribes(StructuredEventKind::CommandInvoked) && self.commands.is_empty() {
            bail!("command-invoked events require at least one handler command");
        }
        Ok(())
    }

    fn validate_capability_requirements(&self) -> Result<()> {
        let handler = self.event_handler.as_ref();
        let has_handler = handler.is_some();
        let subscribes_to_command_completed = handler.is_some_and(|handler| {
            handler
                .events
                .contains(&StructuredEventKind::CommandCompleted)
        });
        let requests = |capability| self.permissions.contains(capability);

        if has_handler != requests(Capability::EventSubscribe) {
            bail!("an event handler and the event-subscribe permission require each other");
        }
        if subscribes_to_command_completed != requests(Capability::CommandOutput) {
            bail!("command-completed events and the command-output permission require each other");
        }
        if let Some(capability) = Capability::ALL
            .into_iter()
            .find(|capability| capability.requires_event_handler() && requests(*capability))
            && !has_handler
        {
            bail!("{capability} requires an event handler");
        }
        if self.permissions.scope(Capability::WindowClose) == Some(PermissionScope::Window)
            && self.commands.is_empty()
        {
            bail!(
                "window-close for the plugin's own window is command-scoped and requires at least one handler command"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledPlugin {
    pub id: String,
    pub enabled: bool,
    pub source: PathBuf,
    pub source_fingerprint: String,
    #[serde(default)]
    pub grants: PluginPermissions,
    pub default_profile: Option<String>,
    #[serde(default)]
    pub settings: BTreeMap<String, PluginSettingValue>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginRegistry {
    pub schema_version: u32,
    #[serde(default)]
    pub plugins: Vec<InstalledPlugin>,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: Vec::new(),
        }
    }
}

impl PluginRegistry {
    fn load_from(path: &Path) -> Result<Self> {
        let bytes = match crate::safe_fs::read_bytes(path, MAX_REGISTRY_BYTES) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to read plugin registry {}", path.display()));
            }
        };
        let text = std::str::from_utf8(&bytes).context("plugin registry must be UTF-8")?;
        let registry: Self = toml::from_str(text).with_context(|| {
            format!(
                "invalid plugin registry {}; delete the affected [[plugins]] entry (or the whole file), then reinstall the plugin with `termeric plugin install <directory> --replace`, since its installed copy remains",
                path.display()
            )
        })?;
        registry.validate()?;
        Ok(registry)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != REGISTRY_SCHEMA_VERSION {
            bail!(
                "plugin registry schema {} is unsupported; expected {}",
                self.schema_version,
                REGISTRY_SCHEMA_VERSION
            );
        }
        if self.plugins.len() > 64 {
            bail!("plugin registry cannot contain more than 64 plugins");
        }
        let mut ids = BTreeSet::new();
        for plugin in &self.plugins {
            validate_identifier("installed plugin id", &plugin.id)?;
            if is_reserved_plugin_id(&plugin.id) {
                bail!("installed plugin id {:?} is reserved", plugin.id);
            }
            if !ids.insert(plugin.id.as_str()) {
                bail!("duplicate installed plugin id {:?}", plugin.id);
            }
            if !plugin.source.is_absolute() {
                bail!("installed plugin {:?} source must be absolute", plugin.id);
            }
            let source = plugin.source.to_string_lossy();
            if source.len() > MAX_SOURCE_PATH_BYTES || source.contains('\0') {
                bail!(
                    "installed plugin {:?} source exceeds its path boundary",
                    plugin.id
                );
            }
            let fingerprint = plugin.source_fingerprint.as_bytes();
            if fingerprint.len() != 24
                || !plugin.source_fingerprint.starts_with("fnv1a64:")
                || !fingerprint[8..].iter().all(u8::is_ascii_hexdigit)
            {
                bail!(
                    "installed plugin {:?} has an invalid source fingerprint",
                    plugin.id
                );
            }
            if let Some(profile) = plugin.default_profile.as_deref() {
                validate_identifier("default profile id", profile)?;
            }
            validate_stored_settings_shape(&plugin.settings)?;
        }
        Ok(())
    }

    fn save_to(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("plugin registry path has no parent"))?;
        ensure_real_directory(parent, "plugin registry directory")?;
        let text = toml::to_string_pretty(self)?;
        crate::safe_fs::atomic_replace(path, |writer| writer.write_all(text.as_bytes()))
            .with_context(|| format!("failed to write plugin registry {}", path.display()))
    }
}

#[derive(Debug, Clone)]
pub struct RuntimePluginProfileSummary {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginUpdatePreview {
    pub id: String,
    pub name: String,
    pub source: PathBuf,
    pub current_version: String,
    pub candidate_version: String,
    pub current_source_fingerprint: String,
    pub candidate_source_fingerprint: String,
    pub current_permissions: PluginPermissions,
    pub candidate_permissions: PluginPermissions,
    pub current_grants: PluginPermissions,
    pub enabled: bool,
    pub current_profile_count: usize,
    pub candidate_profile_count: usize,
    pub current_command_count: usize,
    pub candidate_command_count: usize,
    pub configured_default_profile: Option<String>,
    pub default_profile_will_be_cleared: bool,
    pub(crate) expected_installed: InstalledPlugin,
}

impl PluginUpdatePreview {
    pub(crate) fn command(&self, grants: PluginPermissions, enable: bool) -> PluginAdminCommand {
        PluginAdminCommand::UpdateReviewed {
            id: self.id.clone(),
            expected_candidate_fingerprint: self.candidate_source_fingerprint.clone(),
            expected_installed: self.expected_installed.clone(),
            grants,
            enable,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimePluginStatus {
    pub id: String,
    pub name: String,
    pub version: String,
    pub enabled: bool,
    pub source: PathBuf,
    pub requested: PluginPermissions,
    pub granted: PluginPermissions,
    pub profiles: Vec<RuntimePluginProfileSummary>,
    pub configured_default_profile: Option<String>,
    pub settings: BTreeMap<String, PluginSettingValue>,
    /// Validated manifest schema for healthy installations.
    pub setting_definitions: Vec<DeclarativeSetting>,
    /// Explicit user overrides, kept separate from effective defaults.
    pub configured_settings: BTreeMap<String, PluginSettingValue>,
    /// Number of explicit user overrides stored in the registry. Effective
    /// settings above may also contain manifest defaults.
    pub configured_setting_count: usize,
    pub diagnostic: Option<String>,
}

/// Where the plugin registry and installed plugins live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPaths {
    pub registry: PathBuf,
    pub root: PathBuf,
}

impl PluginPaths {
    /// Beside the configuration file; `None` without a configuration
    /// directory, where no plugin can be installed.
    pub fn configured() -> Option<Self> {
        let dir = crate::paths::config_dir()?;
        Some(Self {
            registry: dir.join("plugins.toml"),
            root: dir.join("plugins"),
        })
    }

    /// When the registry was last written, which the running app polls to
    /// pick up changes made outside it; `None` while there is no registry.
    pub fn registry_mtime(&self) -> Option<std::time::SystemTime> {
        fs::metadata(&self.registry)
            .and_then(|metadata| metadata.modified())
            .ok()
    }
}

pub(crate) fn preview_update_at(
    id: &str,
    registry_path: &Path,
    root: &Path,
) -> Result<PluginUpdatePreview> {
    validate_identifier("plugin id", id)?;
    let registry = PluginRegistry::load_from(registry_path)?;
    let installed = registry
        .plugins
        .iter()
        .find(|plugin| plugin.id == id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("plugin {id:?} is not installed"))?;
    let current = load_installed(&installed, root)?;
    let (candidate, source, bytes) = PluginManifest::from_directory(&installed.source)?;
    if candidate.id != id {
        bail!(
            "updated source changed plugin id from {id:?} to {:?}",
            candidate.id
        );
    }
    effective_settings(&candidate, &installed.settings).context(
        "updated plugin settings are incompatible; unset or reset the stored overrides first",
    )?;
    let default_profile_will_be_cleared = installed
        .default_profile
        .as_ref()
        .is_some_and(|id| !candidate.profiles.iter().any(|profile| &profile.id == id));
    Ok(PluginUpdatePreview {
        id: candidate.id.clone(),
        name: candidate.name.clone(),
        source,
        current_version: current.version,
        candidate_version: candidate.version,
        current_source_fingerprint: installed.source_fingerprint.clone(),
        candidate_source_fingerprint: fingerprint(&bytes),
        current_permissions: current.permissions,
        candidate_permissions: candidate.permissions,
        current_grants: installed.grants.clone(),
        enabled: installed.enabled,
        current_profile_count: current.profiles.len(),
        candidate_profile_count: candidate.profiles.len(),
        current_command_count: current.commands.len(),
        candidate_command_count: candidate.commands.len(),
        configured_default_profile: installed.default_profile.clone(),
        default_profile_will_be_cleared,
        expected_installed: installed,
    })
}

/// What a review of an installed plugin's permissions shows: its installed
/// manifest's requests and its current grants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginGrantsPreview {
    pub id: String,
    pub name: String,
    pub version: String,
    pub requested: PluginPermissions,
    pub current_grants: PluginPermissions,
    pub enabled: bool,
    pub(crate) expected_installed: InstalledPlugin,
}

impl PluginGrantsPreview {
    pub(crate) fn command(&self, grants: PluginPermissions, enable: bool) -> PluginAdminCommand {
        PluginAdminCommand::SetGrants {
            id: self.id.clone(),
            grants,
            enable,
            expected_installed: self.expected_installed.clone(),
        }
    }
}

/// Review an installed plugin's permissions from its installed manifest;
/// its source directory isn't read.
pub(crate) fn preview_grants_at(
    id: &str,
    registry_path: &Path,
    root: &Path,
) -> Result<PluginGrantsPreview> {
    validate_identifier("plugin id", id)?;
    let registry = PluginRegistry::load_from(registry_path)?;
    let installed = registry
        .plugins
        .iter()
        .find(|plugin| plugin.id == id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("plugin {id:?} is not installed"))?;
    let manifest = load_installed(&installed, root)?;
    Ok(PluginGrantsPreview {
        id: manifest.id,
        name: manifest.name,
        version: manifest.version,
        requested: manifest.permissions,
        current_grants: installed.grants.clone(),
        enabled: installed.enabled,
        expected_installed: installed,
    })
}

pub(crate) fn discover_runtime_plugins_at(
    registry_path: &Path,
    root: &Path,
) -> Result<(Vec<DeclarativePlugin>, Vec<RuntimePluginStatus>)> {
    let registry =
        PluginRegistry::load_from(registry_path).context("third-party plugin registry rejected")?;
    Ok(discover_from_registry(&registry, root))
}

fn discover_from_registry(
    registry: &PluginRegistry,
    root: &Path,
) -> (Vec<DeclarativePlugin>, Vec<RuntimePluginStatus>) {
    let mut loaded = Vec::new();
    let mut statuses = Vec::new();
    for installed in &registry.plugins {
        let result = load_installed(installed, root);
        match result {
            Ok(manifest) => {
                let status = RuntimePluginStatus {
                    id: manifest.id.clone(),
                    name: manifest.name.clone(),
                    version: manifest.version.clone(),
                    enabled: installed.enabled,
                    source: installed.source.clone(),
                    requested: manifest.permissions.clone(),
                    granted: installed.grants.clone(),
                    profiles: manifest
                        .profiles
                        .iter()
                        .map(|profile| RuntimePluginProfileSummary {
                            id: profile.id.clone(),
                            name: profile.name.clone(),
                            description: profile.description.clone(),
                        })
                        .collect(),
                    configured_default_profile: installed.default_profile.clone(),
                    settings: effective_settings(&manifest, &installed.settings)
                        .expect("installed settings were validated during load"),
                    setting_definitions: manifest.settings.clone(),
                    configured_settings: installed.settings.clone(),
                    configured_setting_count: installed.settings.len(),
                    diagnostic: None,
                };
                if installed.enabled {
                    loaded.push(DeclarativePlugin::new_with_settings(
                        manifest,
                        installed.default_profile.clone(),
                        installed.grants.clone(),
                        installed.settings.clone(),
                    ));
                }
                statuses.push(status);
            }
            Err(error) => {
                let inspected = PluginManifest::from_directory(&root.join(&installed.id))
                    .ok()
                    .map(|(manifest, _, _)| manifest);
                statuses.push(RuntimePluginStatus {
                    id: installed.id.clone(),
                    name: inspected
                        .as_ref()
                        .map_or_else(|| installed.id.clone(), |manifest| manifest.name.clone()),
                    version: inspected
                        .as_ref()
                        .map_or_else(|| "unknown".into(), |manifest| manifest.version.clone()),
                    enabled: installed.enabled,
                    source: installed.source.clone(),
                    requested: inspected
                        .map_or_else(PluginPermissions::default, |manifest| manifest.permissions),
                    granted: installed.grants.clone(),
                    profiles: Vec::new(),
                    configured_default_profile: installed.default_profile.clone(),
                    settings: BTreeMap::new(),
                    setting_definitions: Vec::new(),
                    configured_settings: BTreeMap::new(),
                    configured_setting_count: installed.settings.len(),
                    diagnostic: Some(format!("{error:#}")),
                });
            }
        }
    }
    (loaded, statuses)
}

fn load_installed(installed: &InstalledPlugin, root: &Path) -> Result<PluginManifest> {
    require_real_directory(root, "plugin installation root")?;
    let directory = root.join(&installed.id);
    let metadata = fs::symlink_metadata(&directory).with_context(|| {
        format!(
            "installed plugin directory {} is unavailable",
            directory.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("installed plugin directory must be a real, non-symlink directory");
    }
    let (manifest, _, bytes) = PluginManifest::from_directory(&directory)?;
    if manifest.id != installed.id {
        bail!("installed manifest id changed from {:?}", installed.id);
    }
    if fingerprint(&bytes) != installed.source_fingerprint {
        bail!("installed manifest differs from its recorded source fingerprint; reinstall it");
    }
    if installed.enabled {
        validate_grants(&manifest, &installed.grants)?;
    }
    if let Some(profile) = installed.default_profile.as_deref()
        && !manifest
            .profiles
            .iter()
            .any(|candidate| candidate.id == profile)
    {
        bail!("configured default profile {profile:?} does not exist");
    }
    effective_settings(&manifest, &installed.settings)?;
    Ok(manifest)
}

#[derive(Debug)]
pub struct DeclarativePlugin {
    manifest: PluginManifest,
    default_profile: Option<String>,
    grants: PluginPermissions,
    settings: BTreeMap<String, PluginSettingValue>,
}

impl DeclarativePlugin {
    #[cfg(test)]
    fn new(
        manifest: PluginManifest,
        default_profile: Option<String>,
        grants: PluginPermissions,
    ) -> Self {
        Self::new_with_settings(manifest, default_profile, grants, BTreeMap::new())
    }

    fn new_with_settings(
        manifest: PluginManifest,
        default_profile: Option<String>,
        grants: PluginPermissions,
        configured_settings: BTreeMap<String, PluginSettingValue>,
    ) -> Self {
        let settings = effective_settings(&manifest, &configured_settings)
            .expect("declarative plugin settings must be validated before construction");
        Self {
            manifest,
            default_profile,
            grants,
            settings,
        }
    }

    fn structured_context_environment(
        &self,
        profile_id: &str,
        context: &PluginContext,
    ) -> Result<HashMap<String, String>> {
        let mut environment = HashMap::new();
        if !self.manifest.settings.is_empty() {
            let value = encode_settings_document(&self.manifest, &self.settings)?;
            environment.insert(STRUCTURED_SETTINGS_ENV.into(), value);
        }
        if !self.grants.contains(Capability::WorkspaceQuery) {
            return Ok(environment);
        }
        #[derive(Serialize)]
        struct LaunchContext<'a> {
            schema_version: u32,
            plugin_id: &'a str,
            profile_id: &'a str,
            #[serde(flatten)]
            workspace: WorkspaceContext<'a>,
        }
        let value = serde_json::to_string(&LaunchContext {
            schema_version: ENVIRONMENT_DOCUMENT_SCHEMA_VERSION,
            plugin_id: &self.manifest.id,
            profile_id,
            workspace: WorkspaceContext::of(context),
        })?;
        if value.len() > MAX_STRUCTURED_CONTEXT_BYTES {
            bail!("structured plugin launch context exceeds {MAX_STRUCTURED_CONTEXT_BYTES} bytes");
        }
        environment.insert(STRUCTURED_CONTEXT_ENV.into(), value);
        Ok(environment)
    }

    fn action_for(&self, profile_id: &str, context: &PluginContext) -> PluginAction {
        let Some(profile) = self
            .manifest
            .profiles
            .iter()
            .find(|profile| profile.id == profile_id)
        else {
            return PluginAction::ShowNotice {
                title: format!("{} profile unavailable", self.manifest.name),
                lines: vec![format!("No profile named {profile_id:?} is installed.")],
            };
        };
        let env = match self.structured_context_environment(profile_id, context) {
            Ok(env) => env,
            Err(error) => {
                return PluginAction::ShowNotice {
                    title: format!("{} context unavailable", self.manifest.name),
                    lines: vec![format!("{error:#}")],
                };
            }
        };
        PluginAction::OpenTab {
            program: profile.program.clone(),
            args: profile.args.clone(),
            cwd: match profile.working_directory {
                WorkingDirectory::Active => context.cwd.clone(),
                WorkingDirectory::Inherit => None,
            },
            title: Some(profile.name.clone()),
            plugin_id: Some(self.manifest.id.clone()),
            profile: Some(profile.id.clone()),
            env,
        }
    }

    fn event_action(&self, hook: &Hook, context: &PluginContext) -> Option<PluginAction> {
        let (kind, event) = match hook {
            Hook::AppReady => (
                StructuredEventKind::AppReady,
                serde_json::json!({ "type": "app-ready" }),
            ),
            Hook::TabCreated { tab_id } => (
                StructuredEventKind::TabCreated,
                serde_json::json!({ "type": "tab-created", "tab_id": tab_id }),
            ),
            Hook::TabClosed { tab_id } => (
                StructuredEventKind::TabClosed,
                serde_json::json!({ "type": "tab-closed", "tab_id": tab_id }),
            ),
            Hook::TabSwitched { from, to } => (
                StructuredEventKind::TabSwitched,
                serde_json::json!({ "type": "tab-switched", "from": from, "to": to }),
            ),
            Hook::CommandExecuted {
                region_id,
                tab_id,
                pane_id,
                command,
                command_truncated,
                output,
                output_truncated,
                content_available,
                exit_code,
            } => (
                StructuredEventKind::CommandCompleted,
                serde_json::json!({
                    "type": "command-completed",
                    "region_id": region_id,
                    "tab_id": tab_id,
                    "pane_id": pane_id,
                    "exit_code": exit_code,
                    "content_available": content_available,
                    "command": {
                        "text": command,
                        "truncated": command_truncated,
                        "sanitized": false,
                    },
                    "output": {
                        "text": output,
                        "truncated": output_truncated,
                        "sanitized": false,
                    },
                }),
            ),
            Hook::CommandInvoked { command_id }
                if self.manifest.commands.iter().any(|command| {
                    command_id == &format!("{}.{}", self.manifest.id, command.id)
                }) =>
            {
                (
                    StructuredEventKind::CommandInvoked,
                    serde_json::json!({
                        "type": "command-invoked",
                        "command_id": command_id,
                    }),
                )
            }
            _ => return None,
        };
        let handler = self.manifest.event_handler.as_ref()?;
        if !handler.events.contains(&kind) || !self.grants.contains(Capability::EventSubscribe) {
            return None;
        }
        if kind == StructuredEventKind::CommandCompleted
            && !self.grants.contains(Capability::CommandOutput)
        {
            return None;
        }
        let mut envelope = serde_json::json!({
            "schema_version": super::structured::SCHEMA_VERSION,
            "plugin_id": self.manifest.id,
            "origin_window_id": context.window_id,
            "event": event,
        });
        let settings = (!self.manifest.settings.is_empty()).then(|| {
            serde_json::to_value(&self.settings)
                .expect("validated plugin settings serialize to JSON")
        });
        if self.grants.contains(Capability::WorkspaceQuery) {
            envelope["context"] = serde_json::to_value(WorkspaceContext::of(context))
                .expect("a workspace context serializes to JSON");
        }
        let max_event_bytes = MAX_PERSISTENT_SOURCE_EVENT_BYTES;
        let event_json = match if kind == StructuredEventKind::CommandCompleted {
            encode_command_event(envelope, max_event_bytes)
        } else {
            serde_json::to_string(&envelope).map_err(anyhow::Error::from)
        } {
            Ok(value) if value.len() <= max_event_bytes => value,
            Ok(_) => {
                return Some(PluginAction::ShowNotice {
                    title: format!("{} event unavailable", self.manifest.name),
                    lines: vec![format!(
                        "Structured plugin event exceeds {max_event_bytes} bytes."
                    )],
                });
            }
            Err(error) => {
                return Some(PluginAction::ShowNotice {
                    title: format!("{} event unavailable", self.manifest.name),
                    lines: vec![format!("Could not encode structured plugin event: {error}")],
                });
            }
        };
        Some(PluginAction::InvokeStructuredHandler {
            invocation: StructuredHandlerInvocation {
                plugin_id: self.manifest.id.clone(),
                program: handler.program.clone(),
                args: handler.args.clone(),
                event_json,
                settings,
                authority: HandlerAuthority::for_event(
                    self.grants.clone(),
                    kind == StructuredEventKind::CommandInvoked,
                ),
            },
        })
    }
}

impl Plugin for DeclarativePlugin {
    fn id(&self) -> &str {
        &self.manifest.id
    }

    fn name(&self) -> &str {
        &self.manifest.name
    }

    fn version(&self) -> &str {
        &self.manifest.version
    }

    fn on_hook(&mut self, hook: &Hook, context: &PluginContext) -> Option<PluginAction> {
        let response = match hook {
            Hook::CommandInvoked { command_id } => self
                .manifest
                .profiles
                .iter()
                .find(|profile| command_id == &format!("{}.open-{}", self.manifest.id, profile.id))
                .map(|profile| self.action_for(&profile.id, context)),
            Hook::PaneLaunch {
                plugin_id,
                profile,
                cwd,
            }
            | Hook::WorkspaceRestore {
                plugin_id,
                profile,
                cwd,
            } if plugin_id == &self.manifest.id => {
                let selected = profile
                    .as_deref()
                    .or(self.default_profile.as_deref())
                    .or_else(|| {
                        self.manifest
                            .profiles
                            .first()
                            .map(|profile| profile.id.as_str())
                    });
                let Some(selected) = selected else {
                    return Some(PluginAction::ShowNotice {
                        title: format!("{} profile unavailable", self.manifest.name),
                        lines: vec!["This event-only plugin has no launch profiles.".into()],
                    });
                };
                let mut restore_context = context.clone();
                restore_context.cwd.clone_from(cwd);
                let mut response = Some(self.action_for(selected, &restore_context));
                // Explicit pane launches use the host's pane cwd policy, not
                // a tab profile's active/inherit placement default.
                if matches!(hook, Hook::PaneLaunch { .. })
                    && let Some(PluginAction::OpenTab { cwd: actual, .. }) = &mut response
                {
                    actual.clone_from(cwd);
                }
                response
            }
            _ => None,
        };
        response.or_else(|| self.event_action(hook, context))
    }

    fn pane_profiles(&self) -> Option<Vec<String>> {
        (!self.manifest.profiles.is_empty()).then(|| {
            self.manifest
                .profiles
                .iter()
                .map(|profile| profile.id.clone())
                .collect()
        })
    }

    fn commands(&self) -> Vec<PluginCommand> {
        let profiles = self.manifest.profiles.iter().map(|profile| PluginCommand {
            id: format!("{}.open-{}", self.manifest.id, profile.id),
            name: format!("Open {}", profile.name),
            description: profile.description.clone(),
            category: Some(self.manifest.name.clone()),
        });
        let handlers = self.manifest.commands.iter().map(|command| PluginCommand {
            id: format!("{}.{}", self.manifest.id, command.id),
            name: command.name.clone(),
            description: command.description.clone(),
            category: Some(self.manifest.name.clone()),
        });
        profiles.chain(handlers).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginAdminCommand {
    List {
        json: bool,
    },
    Show {
        id: String,
        json: bool,
    },
    Validate {
        directory: PathBuf,
    },
    Install {
        directory: PathBuf,
        grants: PluginPermissions,
        enable: bool,
        replace: bool,
    },
    InstallReviewed {
        directory: PathBuf,
        grants: PluginPermissions,
        enable: bool,
        expected_source_fingerprint: String,
    },
    Update {
        id: String,
    },
    UpdateReviewed {
        id: String,
        expected_candidate_fingerprint: String,
        expected_installed: InstalledPlugin,
        grants: PluginPermissions,
        enable: bool,
    },
    /// Add `grants` to an installed plugin's grants, then enable it.
    Enable {
        id: String,
        grants: PluginPermissions,
    },
    Disable {
        id: String,
    },
    /// Add `grants` to an installed plugin's grants, replacing the scope of a
    /// capability already granted.
    Grant {
        id: String,
        grants: PluginPermissions,
    },
    /// Withdraw the grant of each capability in `permissions`. An enabled
    /// plugin left without every requested permission is disabled.
    Revoke {
        id: String,
        permissions: PluginPermissions,
    },
    /// Replace an installed plugin's grants after a review of its installed
    /// manifest, refused if its registry entry changed since the review.
    SetGrants {
        id: String,
        grants: PluginPermissions,
        enable: bool,
        expected_installed: InstalledPlugin,
    },
    Configure {
        id: String,
        change: PluginConfigurationChange,
    },
    Remove {
        id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginConfigurationChange {
    DefaultProfile(String),
    ClearDefaultProfile,
    Set { key: String, value: String },
    Unset { key: String },
    ResetSettings,
}

pub fn execute_admin(command: PluginAdminCommand) -> Result<String> {
    let paths = PluginPaths::configured().context(crate::paths::NO_HOME_DIRECTORY)?;
    execute_admin_at(command, &paths.registry, &paths.root)
}

pub(crate) fn execute_admin_at(
    command: PluginAdminCommand,
    registry_path: &Path,
    root: &Path,
) -> Result<String> {
    let mut registry = PluginRegistry::load_from(registry_path)?;
    match command {
        PluginAdminCommand::Validate { directory } => {
            let (manifest, source, bytes) = PluginManifest::from_directory(&directory)?;
            Ok(format!(
                "valid plugin {} v{} source={} fingerprint={} permissions={}",
                manifest.id,
                manifest.version,
                source.display(),
                fingerprint(&bytes),
                manifest.permissions
            ))
        }
        PluginAdminCommand::Install {
            directory,
            grants,
            enable,
            replace,
        } => {
            let (manifest, source, bytes) = PluginManifest::from_directory(&directory)?;
            install_manifest(
                &mut registry,
                registry_path,
                root,
                manifest,
                source,
                bytes,
                grants,
                enable,
                replace,
            )
        }
        PluginAdminCommand::InstallReviewed {
            directory,
            grants,
            enable,
            expected_source_fingerprint,
        } => {
            let (manifest, source, bytes) = PluginManifest::from_directory(&directory)?;
            let source_fingerprint = fingerprint(&bytes);
            if source_fingerprint != expected_source_fingerprint {
                bail!(
                    "plugin source changed after review (expected {expected_source_fingerprint}, found {source_fingerprint}); review it again"
                );
            }
            install_manifest(
                &mut registry,
                registry_path,
                root,
                manifest,
                source,
                bytes,
                grants,
                enable,
                false,
            )
        }
        PluginAdminCommand::Update { id } => {
            validate_identifier("plugin id", &id)?;
            let installed = registry
                .plugins
                .iter()
                .find(|plugin| plugin.id == id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("plugin {id:?} is not installed"))?;
            let (manifest, source, bytes) = PluginManifest::from_directory(&installed.source)?;
            if manifest.id != id {
                bail!(
                    "updated source changed plugin id from {id:?} to {:?}",
                    manifest.id
                );
            }
            let previous_manifest = load_installed(&installed, root)?;
            let added_permissions = manifest
                .permissions
                .uncovered_by(&previous_manifest.permissions);
            if !added_permissions.is_empty() {
                bail!(
                    "update requests new permissions ({}); review and reinstall with explicit grants",
                    permission_list(added_permissions)
                );
            }
            // Like the in-app update review, keep only what the update still
            // requests, each at the narrower of the granted and requested
            // scopes.
            let grants = installed.grants.intersection(&manifest.permissions);
            install_manifest(
                &mut registry,
                registry_path,
                root,
                manifest,
                source,
                bytes,
                grants,
                installed.enabled,
                true,
            )
        }
        PluginAdminCommand::UpdateReviewed {
            id,
            expected_candidate_fingerprint,
            expected_installed,
            grants,
            enable,
        } => {
            validate_identifier("plugin id", &id)?;
            let installed = registry
                .plugins
                .iter()
                .find(|plugin| plugin.id == id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("plugin {id:?} is not installed"))?;
            if installed != expected_installed {
                bail!("plugin {id:?} changed after review; review the update again");
            }
            let (manifest, source, bytes) = PluginManifest::from_directory(&installed.source)?;
            if manifest.id != id {
                bail!(
                    "updated source changed plugin id from {id:?} to {:?}",
                    manifest.id
                );
            }
            let candidate_fingerprint = fingerprint(&bytes);
            if candidate_fingerprint != expected_candidate_fingerprint {
                bail!(
                    "plugin update source changed after review (expected {expected_candidate_fingerprint}, found {candidate_fingerprint}); review it again"
                );
            }
            install_manifest(
                &mut registry,
                registry_path,
                root,
                manifest,
                source,
                bytes,
                grants,
                enable,
                true,
            )
        }
        PluginAdminCommand::Enable { id, grants } => set_grants(
            &mut registry,
            registry_path,
            root,
            &id,
            None,
            |current| with_grants(current, &grants),
            Some(true),
        ),
        PluginAdminCommand::Disable { id } => disable(&mut registry, registry_path, &id),
        PluginAdminCommand::Grant { id, grants } => set_grants(
            &mut registry,
            registry_path,
            root,
            &id,
            None,
            |current| with_grants(current, &grants),
            None,
        ),
        PluginAdminCommand::Revoke { id, permissions } => set_grants(
            &mut registry,
            registry_path,
            root,
            &id,
            None,
            |current| {
                let mut remaining = current.clone();
                for permission in permissions.iter() {
                    remaining.remove(permission.capability);
                }
                remaining
            },
            None,
        ),
        PluginAdminCommand::SetGrants {
            id,
            grants,
            enable,
            expected_installed,
        } => set_grants(
            &mut registry,
            registry_path,
            root,
            &id,
            Some(&expected_installed),
            |_| grants,
            Some(enable),
        ),
        PluginAdminCommand::Configure { id, change } => {
            validate_identifier("plugin id", &id)?;
            let installed = registry
                .plugins
                .iter_mut()
                .find(|plugin| plugin.id == id)
                .ok_or_else(|| anyhow::anyhow!("plugin {id:?} is not installed"))?;
            let manifest = load_installed(installed, root)?;
            let message = match change {
                PluginConfigurationChange::DefaultProfile(default_profile) => {
                    validate_identifier("profile id", &default_profile)?;
                    if !manifest
                        .profiles
                        .iter()
                        .any(|profile| profile.id == default_profile)
                    {
                        bail!("plugin {id:?} has no profile {default_profile:?}");
                    }
                    installed.default_profile = Some(default_profile.clone());
                    format!("configured {id}: default-profile={default_profile}")
                }
                PluginConfigurationChange::ClearDefaultProfile => {
                    installed.default_profile = None;
                    format!("configured {id}: default-profile=manifest-order")
                }
                PluginConfigurationChange::Set { key, value } => {
                    validate_identifier("setting id", &key)?;
                    let setting = manifest
                        .settings
                        .iter()
                        .find(|setting| setting.id() == key)
                        .ok_or_else(|| anyhow::anyhow!("plugin {id:?} has no setting {key:?}"))?;
                    let value = parse_setting_value(setting, &value)?;
                    installed.settings.insert(key.clone(), value);
                    effective_settings(&manifest, &installed.settings)?;
                    format!("configured {id}: setting {key} updated")
                }
                PluginConfigurationChange::Unset { key } => {
                    validate_identifier("setting id", &key)?;
                    if !manifest.settings.iter().any(|setting| setting.id() == key) {
                        bail!("plugin {id:?} has no setting {key:?}");
                    }
                    installed.settings.remove(&key);
                    format!("configured {id}: setting {key} restored to its default")
                }
                PluginConfigurationChange::ResetSettings => {
                    installed.settings.clear();
                    format!("configured {id}: all settings restored to defaults")
                }
            };
            registry.save_to(registry_path)?;
            Ok(message)
        }
        PluginAdminCommand::Remove { id } => {
            validate_identifier("plugin id", &id)?;
            let directory = root.join(&id);
            let directory_exists = match fs::symlink_metadata(&directory) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                    bail!("refusing to remove a non-directory or symlinked plugin installation")
                }
                Ok(_) => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(error.into()),
            };
            let before = registry.plugins.len();
            registry.plugins.retain(|plugin| plugin.id != id);
            if registry.plugins.len() == before {
                bail!("plugin {id:?} is not installed");
            }
            let quarantine = root.join(format!(".{id}.remove.tmp"));
            if directory_exists {
                if fs::symlink_metadata(&quarantine).is_ok() {
                    bail!(
                        "refusing removal while stale quarantine {} exists",
                        quarantine.display()
                    );
                }
                fs::rename(&directory, &quarantine).with_context(|| {
                    format!(
                        "could not quarantine plugin directory {}",
                        directory.display()
                    )
                })?;
            }
            if let Err(error) = registry.save_to(registry_path) {
                if directory_exists {
                    let _ = fs::rename(&quarantine, &directory);
                }
                return Err(error);
            }
            if directory_exists {
                fs::remove_dir_all(&quarantine).with_context(|| {
                    format!(
                        "plugin was removed from the registry but quarantine cleanup failed at {}",
                        quarantine.display()
                    )
                })?;
            }
            Ok(format!("removed {id}"))
        }
        PluginAdminCommand::List { json } => render_registry(&registry, root, None, json),
        PluginAdminCommand::Show { id, json } => {
            validate_identifier("plugin id", &id)?;
            render_registry(&registry, root, Some(&id), json)
        }
    }
}

fn install_manifest(
    registry: &mut PluginRegistry,
    registry_path: &Path,
    root: &Path,
    manifest: PluginManifest,
    source: PathBuf,
    bytes: Vec<u8>,
    grants: PluginPermissions,
    enable: bool,
    replace: bool,
) -> Result<String> {
    let position = registry
        .plugins
        .iter()
        .position(|plugin| plugin.id == manifest.id);
    let previous = position.map(|index| registry.plugins[index].clone());
    let settings = previous
        .as_ref()
        .map_or_else(BTreeMap::new, |plugin| plugin.settings.clone());
    effective_settings(&manifest, &settings).context(
        "updated plugin settings are incompatible; unset or reset the stored overrides first",
    )?;
    if position.is_some() && !replace {
        bail!(
            "plugin {:?} is already installed; use update or --replace",
            manifest.id
        );
    }
    let unrequested = grants.uncovered_by(&manifest.permissions);
    if !unrequested.is_empty() {
        bail!(
            "cannot grant unrequested permissions: {}",
            permission_list(unrequested)
        );
    }
    let denied = manifest.permissions.uncovered_by(&grants);
    if enable && !denied.is_empty() {
        bail!(
            "cannot enable until permissions are granted: {}",
            permission_list(denied)
        );
    }
    ensure_real_directory(root, "plugin installation root")?;
    let destination = root.join(&manifest.id);
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            bail!("plugin installation directory must be a real, non-symlink directory")
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&destination)?;
        }
        Err(error) => return Err(error.into()),
    }
    let destination_manifest = destination.join(MANIFEST_NAME);
    let destination_metadata = match fs::symlink_metadata(&destination_manifest) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if destination_metadata.is_some() && position.is_none() && !replace {
        bail!(
            "an unregistered installed copy already exists at {}; use --replace only after review",
            destination_manifest.display()
        );
    }
    let backup = destination.join(format!("{MANIFEST_NAME}.previous"));
    let had_previous = destination_metadata.is_some();
    if had_previous {
        let metadata = destination_metadata.expect("previous manifest metadata was inspected");
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            bail!("refusing to replace a non-regular installed manifest");
        }
        match fs::symlink_metadata(&backup) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                bail!("refusing to replace a non-regular plugin rollback file")
            }
            Ok(_) => fs::remove_file(&backup)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        fs::rename(&destination_manifest, &backup)?;
    }
    if let Err(error) =
        crate::safe_fs::atomic_replace(&destination_manifest, |writer| writer.write_all(&bytes))
    {
        if had_previous {
            let _ = fs::rename(&backup, &destination_manifest);
        }
        return Err(error).with_context(|| {
            format!(
                "failed to write installed plugin manifest {}",
                destination_manifest.display()
            )
        });
    }
    let default_profile = previous
        .as_ref()
        .and_then(|plugin| plugin.default_profile.clone())
        .filter(|profile| {
            manifest
                .profiles
                .iter()
                .any(|candidate| &candidate.id == profile)
        });
    let installed = InstalledPlugin {
        id: manifest.id.clone(),
        enabled: enable,
        source: source.clone(),
        source_fingerprint: fingerprint(&bytes),
        grants,
        default_profile,
        settings,
    };
    if let Some(index) = position {
        registry.plugins[index] = installed;
    } else {
        registry.plugins.push(installed);
        registry
            .plugins
            .sort_by(|left, right| left.id.cmp(&right.id));
    }
    if let Err(error) = registry.save_to(registry_path) {
        let _ = fs::remove_file(&destination_manifest);
        if had_previous {
            let _ = fs::rename(&backup, &destination_manifest);
        }
        return Err(error);
    }
    if had_previous && let Err(error) = fs::remove_file(&backup) {
        log::warn!(
            "Installed plugin {:?}, but could not remove rollback copy {}: {error}",
            manifest.id,
            backup.display()
        );
    }
    let granted = registry
        .plugins
        .iter()
        .find(|plugin| plugin.id == manifest.id)
        .map(|plugin| plugin.grants.to_string())
        .unwrap_or_default();
    Ok(format!(
        "installed {} v{} enabled={} source={} permissions={} grants={granted}",
        manifest.id,
        manifest.version,
        enable,
        source.display(),
        manifest.permissions,
    ))
}

fn ensure_real_directory(path: &Path, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => return require_real_directory(path, label),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("{label} must be a real, non-symlink directory");
    }
    Ok(())
}

fn require_real_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("{label} {} is unavailable", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("{label} must be a real, non-symlink directory");
    }
    Ok(())
}

fn disable(registry: &mut PluginRegistry, registry_path: &Path, id: &str) -> Result<String> {
    validate_identifier("plugin id", id)?;
    let installed = registry
        .plugins
        .iter_mut()
        .find(|plugin| plugin.id == id)
        .ok_or_else(|| anyhow::anyhow!("plugin {id:?} is not installed"))?;
    installed.enabled = false;
    registry.save_to(registry_path)?;
    Ok(format!("disabled {id}"))
}

/// `current` with each of `added` granted, at the scope `added` names.
fn with_grants(current: &PluginPermissions, added: &PluginPermissions) -> PluginPermissions {
    let mut grants = current.clone();
    for permission in added.iter() {
        grants.set(permission);
    }
    grants
}

/// Change an installed plugin's grants. They are checked against its
/// installed manifest, so its source directory isn't needed. `enable` turns
/// it on, which needs every requested permission, or off; without it, an
/// enabled plugin stays enabled only while its grants cover every request.
fn set_grants(
    registry: &mut PluginRegistry,
    registry_path: &Path,
    root: &Path,
    id: &str,
    expected_installed: Option<&InstalledPlugin>,
    grants: impl FnOnce(&PluginPermissions) -> PluginPermissions,
    enable: Option<bool>,
) -> Result<String> {
    validate_identifier("plugin id", id)?;
    let installed = registry
        .plugins
        .iter_mut()
        .find(|plugin| plugin.id == id)
        .ok_or_else(|| anyhow::anyhow!("plugin {id:?} is not installed"))?;
    if expected_installed.is_some_and(|expected| expected != installed) {
        bail!("plugin {id:?} changed after review; review its permissions again");
    }
    let manifest = load_installed(installed, root)?;
    let grants = grants(&installed.grants);
    let unrequested = grants.uncovered_by(&manifest.permissions);
    if !unrequested.is_empty() {
        bail!(
            "cannot grant unrequested permissions: {}",
            permission_list(unrequested)
        );
    }
    let denied = manifest.permissions.uncovered_by(&grants);
    let enabled = match enable {
        Some(true) if !denied.is_empty() => bail!(
            "cannot enable until permissions are granted: {}",
            permission_list(denied)
        ),
        Some(enabled) => enabled,
        None => installed.enabled && denied.is_empty(),
    };
    let disabled_by_change = installed.enabled && !enabled && enable.is_none();
    installed.grants = grants;
    installed.enabled = enabled;
    let granted = if installed.grants.is_empty() {
        "none".to_string()
    } else {
        installed.grants.to_string()
    };
    registry.save_to(registry_path)?;
    Ok(if disabled_by_change {
        format!("granted {id}: {granted}; disabled until every requested permission is granted")
    } else {
        format!(
            "granted {id}: {granted}; {}",
            if enabled { "enabled" } else { "disabled" }
        )
    })
}

fn validate_grants(manifest: &PluginManifest, grants: &PluginPermissions) -> Result<()> {
    let denied = manifest.permissions.uncovered_by(grants);
    if !denied.is_empty() {
        bail!("permissions denied: {}", permission_list(denied));
    }
    let unrequested = grants.uncovered_by(&manifest.permissions);
    if !unrequested.is_empty() {
        bail!(
            "grants exceed the manifest's requests: {}",
            permission_list(unrequested)
        );
    }
    Ok(())
}

/// The origin window as a plugin granted `workspace-query` sees it: in its
/// launch context and in each event's `context`, as
/// `examples/plugins/README.md` documents both.
#[derive(Serialize)]
struct WorkspaceContext<'a> {
    active_working_directory: Option<&'a str>,
    tab_count: usize,
    active_tab_index: usize,
    active_tab_id: Option<usize>,
    terminal: TerminalSize,
}

#[derive(Serialize)]
struct TerminalSize {
    columns: usize,
    rows: usize,
}

impl<'a> WorkspaceContext<'a> {
    fn of(context: &'a PluginContext) -> Self {
        Self {
            active_working_directory: context.cwd.as_deref(),
            tab_count: context.tab_count,
            active_tab_index: context.active_tab,
            active_tab_id: context.active_tab_id,
            terminal: TerminalSize {
                columns: context.terminal_size.0,
                rows: context.terminal_size.1,
            },
        }
    }
}

fn encode_command_event(mut envelope: serde_json::Value, max_event_bytes: usize) -> Result<String> {
    let content_available = envelope["event"]["content_available"]
        .as_bool()
        .unwrap_or(false);
    let command_source = std::mem::take(&mut envelope["event"]["command"]["text"])
        .as_str()
        .unwrap_or("")
        .to_owned();
    let output_source = std::mem::take(&mut envelope["event"]["output"]["text"])
        .as_str()
        .unwrap_or("")
        .to_owned();
    let command_was_truncated = envelope["event"]["command"]["truncated"]
        .as_bool()
        .unwrap_or(false);
    let output_was_truncated = envelope["event"]["output"]["truncated"]
        .as_bool()
        .unwrap_or(false);
    let base = serde_json::to_string(&envelope)?;
    if base.len() > max_event_bytes {
        bail!("structured command event metadata exceeds {max_event_bytes} bytes");
    }
    let mut remaining = max_event_bytes - base.len();

    let (command, command_truncated, command_sanitized, command_json_bytes) =
        bounded_json_terminal_text(
            if content_available {
                &command_source
            } else {
                ""
            },
            remaining.min(MAX_STRUCTURED_COMMAND_JSON_BYTES),
            KeepText::Start,
        );
    remaining = remaining.saturating_sub(command_json_bytes);
    let (output, output_truncated, output_sanitized, _) = bounded_json_terminal_text(
        if content_available {
            &output_source
        } else {
            ""
        },
        remaining.min(MAX_STRUCTURED_OUTPUT_JSON_BYTES),
        KeepText::End,
    );
    envelope["event"]["command"] = serde_json::json!({
        "text": command,
        "truncated": command_was_truncated || command_truncated,
        "sanitized": command_sanitized,
    });
    envelope["event"]["output"] = serde_json::json!({
        "text": output,
        "truncated": output_was_truncated || output_truncated,
        "sanitized": output_sanitized,
    });
    let encoded = serde_json::to_string(&envelope)?;
    if encoded.len() > max_event_bytes {
        bail!("structured command event exceeds {max_event_bytes} bytes");
    }
    Ok(encoded)
}

/// Which end of a text survives when it is cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeepText {
    /// A command line, read from its start.
    Start,
    /// Command output, whose end matters most.
    End,
}

/// Bound terminal text by its JSON string-body representation so quotes,
/// backslashes, and newlines cannot make the final event exceed its wire cap.
/// C0/C1 controls other than tab/newline are replaced before disclosure.
fn bounded_json_terminal_text(
    value: &str,
    max_json_bytes: usize,
    keep: KeepText,
) -> (String, bool, bool, usize) {
    match keep {
        KeepText::Start => bounded_json_characters(value.chars(), max_json_bytes),
        KeepText::End => {
            let (reversed, truncated, sanitized, used) =
                bounded_json_characters(value.chars().rev(), max_json_bytes);
            (reversed.chars().rev().collect(), truncated, sanitized, used)
        }
    }
}

/// The characters `characters` yields until the next one would pass
/// `max_json_bytes`, sanitized, in the order they were yielded.
fn bounded_json_characters(
    characters: impl Iterator<Item = char>,
    max_json_bytes: usize,
) -> (String, bool, bool, usize) {
    let mut output = String::new();
    let mut used = 0_usize;
    let mut truncated = false;
    let mut sanitized = false;
    for character in characters {
        let (character, json_bytes) = if character == '\n' || character == '\t' {
            (character, 2)
        } else if character.is_control() {
            sanitized = true;
            ('\u{fffd}', '\u{fffd}'.len_utf8())
        } else if matches!(character, '"' | '\\') {
            (character, 2)
        } else {
            (character, character.len_utf8())
        };
        if used.saturating_add(json_bytes) > max_json_bytes {
            truncated = true;
            break;
        }
        output.push(character);
        used += json_bytes;
    }
    (output, truncated, sanitized, used)
}

fn render_registry(
    registry: &PluginRegistry,
    root: &Path,
    only: Option<&str>,
    json: bool,
) -> Result<String> {
    let (_, statuses) = discover_from_registry(registry, root);
    let statuses: Vec<_> = statuses
        .into_iter()
        .filter(|status| only.is_none_or(|id| status.id == id))
        .collect();
    if only.is_some() && statuses.is_empty() {
        bail!("plugin {:?} is not installed", only.unwrap_or_default());
    }
    if json {
        let values: Vec<_> = statuses
            .iter()
            .map(|status| {
                serde_json::json!({
                    "id": status.id,
                    "name": status.name,
                    "version": status.version,
                    "enabled": status.enabled,
                    "source": status.source,
                    "requested_permissions": status.requested,
                    "granted_permissions": status.granted,
                    "settings": status.settings,
                    "diagnostic": status.diagnostic,
                })
            })
            .collect();
        return Ok(serde_json::to_string_pretty(&values)?);
    }
    if statuses.is_empty() {
        return Ok("No third-party plugins installed".into());
    }
    Ok(statuses
        .iter()
        .map(|status| {
            format!(
                "{} {} v{} enabled={} source={} requested=[{}] granted=[{}] settings={}{}",
                status.id,
                status.name,
                status.version,
                status.enabled,
                status.source.display(),
                status.requested,
                status.granted,
                serde_json::to_string(&status.settings)
                    .expect("validated plugin settings serialize to JSON"),
                status
                    .diagnostic
                    .as_ref()
                    .map_or(String::new(), |error| format!(" error={error:?}"))
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Built-in agents register as plugins under their own IDs, so a
/// third-party plugin can never use one.
fn is_reserved_plugin_id(id: &str) -> bool {
    crate::agents::builtin_agent_names().any(|(builtin, _)| builtin == id)
}

fn validate_identifier(label: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || !value.as_bytes()[0].is_ascii_lowercase()
    {
        bail!(
            "{label} must be 1-64 lowercase ASCII letters, digits, or hyphens and start with a letter"
        );
    }
    Ok(())
}

fn validate_argv(label: &str, program: &str, args: &[String]) -> Result<()> {
    validate_text(&format!("{label} program"), program, MAX_ARG_BYTES)?;
    if args.len() > MAX_ARGS {
        bail!("{label} has more than {MAX_ARGS} arguments");
    }
    let mut total = program.len();
    for argument in args {
        if argument.len() > MAX_ARG_BYTES || argument.contains('\0') {
            bail!("{label} arguments must be NUL-free and at most {MAX_ARG_BYTES} bytes");
        }
        total = total.saturating_add(argument.len());
    }
    if total > MAX_TOTAL_ARG_BYTES {
        bail!("{label} argv exceeds {MAX_TOTAL_ARG_BYTES} bytes");
    }
    Ok(())
}

fn validate_text(label: &str, value: &str, max: usize) -> Result<()> {
    if value.trim().is_empty() || value.len() > max || value.contains(['\0', '\n', '\r']) {
        bail!("{label} must be a non-empty, single-line UTF-8 value of at most {max} bytes");
    }
    Ok(())
}

fn validate_optional_text(label: &str, value: &str, max: usize) -> Result<()> {
    if value.len() > max || value.contains('\0') {
        bail!("{label} must be NUL-free and at most {max} bytes");
    }
    Ok(())
}

fn validate_setting_text(label: &str, value: &str) -> Result<()> {
    if value.len() > MAX_SETTING_TEXT_BYTES || value.contains(['\0', '\n', '\r']) {
        bail!(
            "{label} must be a single-line, NUL-free UTF-8 value of at most {MAX_SETTING_TEXT_BYTES} bytes"
        );
    }
    Ok(())
}

fn validate_stored_settings_shape(settings: &BTreeMap<String, PluginSettingValue>) -> Result<()> {
    if settings.len() > MAX_SETTINGS {
        bail!("installed plugins cannot store more than {MAX_SETTINGS} setting overrides");
    }
    for (id, value) in settings {
        validate_identifier("stored setting id", id)?;
        if let PluginSettingValue::Text(text) = value {
            validate_setting_text(&format!("stored setting {id:?}"), text)?;
        }
    }
    let encoded = serde_json::to_vec(settings)?;
    if encoded.len() > MAX_SETTING_VALUE_BYTES {
        bail!("stored plugin settings exceed {MAX_SETTING_VALUE_BYTES} bytes");
    }
    Ok(())
}

fn validate_setting_value(setting: &DeclarativeSetting, value: &PluginSettingValue) -> Result<()> {
    match (setting, value) {
        (DeclarativeSetting::Boolean { .. }, PluginSettingValue::Boolean(_)) => Ok(()),
        (DeclarativeSetting::Integer { min, max, .. }, PluginSettingValue::Integer(value))
            if (min..=max).contains(&value) =>
        {
            Ok(())
        }
        (DeclarativeSetting::Integer { id, min, max, .. }, PluginSettingValue::Integer(value)) => {
            bail!("setting {id:?} value {value} is outside {min}..={max}")
        }
        (DeclarativeSetting::Text { id, .. }, PluginSettingValue::Text(value)) => {
            validate_setting_text(&format!("setting {id:?}"), value)
        }
        (DeclarativeSetting::Choice { id, choices, .. }, PluginSettingValue::Text(value))
            if choices.contains(value) =>
        {
            Ok(())
        }
        (DeclarativeSetting::Choice { id, .. }, PluginSettingValue::Text(value)) => {
            bail!("setting {id:?} does not permit choice {value:?}")
        }
        _ => bail!("setting {:?} has the wrong value type", setting.id()),
    }
}

fn effective_settings(
    manifest: &PluginManifest,
    configured: &BTreeMap<String, PluginSettingValue>,
) -> Result<BTreeMap<String, PluginSettingValue>> {
    validate_stored_settings_shape(configured)?;
    let mut values = manifest
        .settings
        .iter()
        .map(|setting| (setting.id().to_string(), setting.default_value()))
        .collect::<BTreeMap<_, _>>();
    for (id, value) in configured {
        let setting = manifest
            .settings
            .iter()
            .find(|setting| setting.id() == id)
            .ok_or_else(|| anyhow::anyhow!("configured setting {id:?} does not exist"))?;
        validate_setting_value(setting, value)?;
        values.insert(id.clone(), value.clone());
    }
    encode_settings_document(manifest, &values)?;
    Ok(values)
}

fn encode_settings_document(
    manifest: &PluginManifest,
    values: &BTreeMap<String, PluginSettingValue>,
) -> Result<String> {
    let encoded = serde_json::to_string(&serde_json::json!({
        "schema_version": ENVIRONMENT_DOCUMENT_SCHEMA_VERSION,
        "plugin_id": manifest.id,
        "values": values,
    }))?;
    if encoded.len() > MAX_SETTING_VALUE_BYTES {
        bail!("effective plugin settings exceed {MAX_SETTING_VALUE_BYTES} bytes");
    }
    Ok(encoded)
}

fn parse_setting_value(setting: &DeclarativeSetting, input: &str) -> Result<PluginSettingValue> {
    let value = match setting {
        DeclarativeSetting::Boolean { id, .. } => match input {
            "true" => PluginSettingValue::Boolean(true),
            "false" => PluginSettingValue::Boolean(false),
            _ => bail!("boolean setting {id:?} expects true or false"),
        },
        DeclarativeSetting::Integer { id, .. } => PluginSettingValue::Integer(
            input
                .parse::<i64>()
                .with_context(|| format!("integer setting {id:?} expects a base-10 integer"))?,
        ),
        DeclarativeSetting::Text { .. } | DeclarativeSetting::Choice { .. } => {
            PluginSettingValue::Text(input.to_string())
        }
    };
    validate_setting_value(setting, &value)?;
    Ok(value)
}

fn permission_list(permissions: impl IntoIterator<Item = PluginPermission>) -> String {
    permissions
        .into_iter()
        .map(|permission| permission.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn fingerprint(bytes: &[u8]) -> String {
    // Provenance/change detection, not a signature. FNV-1a is deterministic
    // across platforms; trust comes from the displayed canonical local source.
    format!("fnv1a64:{:016x}", termeric_core::checksum::fnv1a64(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::structured::StructuredResponseAction;
    use std::time::{Duration, Instant};

    fn permissions(values: &[&str]) -> PluginPermissions {
        PluginPermissions::from_permissions(values.iter().map(|value| value.parse().unwrap()))
            .unwrap()
    }

    fn context() -> PluginContext {
        PluginContext {
            window_id: 3,
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            active_tab_id: Some(1),
            terminal_size: (80, 24),
        }
    }

    fn manifest(version: &str, extra_permission: &str, program: &str) -> String {
        format!(
            r#"api_version = 1
id = "sample-tools"
name = "Sample Tools"
version = "{version}"
description = "A local declarative fixture"
permissions = ["process-spawn"{extra_permission}]

[[profiles]]
id = "default"
name = "Sample Shell"
description = "Open the sample"
program = "{program}"
args = ["--fixture"]
working_directory = "active"

[[profiles]]
id = "quiet"
name = "Quiet Sample"
program = "{program}"
args = ["--quiet"]
working_directory = "inherit"
"#
        )
    }

    #[test]
    fn every_built_in_agent_id_is_reserved() {
        for (id, _) in crate::agents::builtin_agent_names() {
            let source = manifest("1.0.0", "", "/bin/true")
                .replace(r#"id = "sample-tools""#, &format!("id = {id:?}"));
            let parsed: PluginManifest = toml::from_str(&source).unwrap();
            let error = parsed.validate().unwrap_err().to_string();
            assert!(error.contains("reserved"), "{id}: {error}");
        }
    }

    /// A handler plugin requesting `requested` (beyond process-spawn and
    /// event-subscribe). A `command-invoked` subscription declares one
    /// command, and a `command-completed` one requests command-output.
    fn handler_manifest(requested: &[&str], events: &[&str]) -> PluginManifest {
        let mut requested_permissions = vec!["process-spawn", "event-subscribe"];
        requested_permissions.extend_from_slice(requested);
        if events.contains(&"command-completed") {
            requested_permissions.push("command-output");
        }
        let commands = if events.contains(&"command-invoked") {
            "[[commands]]\nid = \"run\"\nname = \"Run\"\n\n"
        } else {
            ""
        };
        toml::from_str(&format!(
            r#"api_version = 1
id = "handler-fixture"
name = "Handler Fixture"
version = "1.0.0"
permissions = {requested_permissions:?}

{commands}[event_handler]
program = "fixture-handler"
args = ["--stdio"]
events = {events:?}
"#
        ))
        .unwrap()
    }

    fn invocation_for(
        manifest: PluginManifest,
        grants: PluginPermissions,
        hook: &Hook,
        context: &PluginContext,
    ) -> StructuredHandlerInvocation {
        let mut plugin = DeclarativePlugin::new(manifest, None, grants);
        let Some(PluginAction::InvokeStructuredHandler { invocation }) =
            plugin.on_hook(hook, context)
        else {
            panic!("subscribed hook did not create a handler invocation")
        };
        invocation
    }

    fn configurable_manifest(version: &str) -> String {
        format!(
            r#"api_version = 1
id = "configurable-tool"
name = "Configurable Tool"
version = "{version}"
permissions = ["process-spawn", "event-subscribe", "ui-actions"]

[[profiles]]
id = "run"
name = "Run Tool"
program = "configurable-handler"

[[commands]]
id = "show-settings"
name = "Show Settings"

[[settings]]
type = "boolean"
id = "enabled-output"
name = "Enabled output"
default = true

[[settings]]
type = "integer"
id = "line-count"
name = "Line count"
default = 10
min = 1
max = 100

[[settings]]
type = "text"
id = "label"
name = "Label"
default = "local"

[[settings]]
type = "choice"
id = "detail"
name = "Detail"
default = "summary"
choices = ["summary", "full"]

[event_handler]
program = "configurable-handler"
events = ["command-invoked"]
"#
        )
    }

    fn configurable_plugin_manifest() -> PluginManifest {
        toml::from_str(&configurable_manifest("1.0.0")).unwrap()
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry = temp.path().join("plugins.toml");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest("1.0.0", "", "sample-cli"),
        )
        .unwrap();
        (temp, source, root, registry)
    }

    fn install(
        source: &Path,
        grants: PluginPermissions,
        enable: bool,
        registry_path: &Path,
        root: &Path,
    ) -> Result<String> {
        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.to_path_buf(),
                grants,
                enable,
                replace: true,
            },
            registry_path,
            root,
        )
    }

    #[test]
    fn explicit_pane_hook_preserves_profile_but_uses_host_cwd_policy() {
        let manifest: PluginManifest =
            toml::from_str(&manifest("1.0.0", "", "synthetic-not-executed")).unwrap();
        let mut plugin = DeclarativePlugin::new(manifest, None, permissions(&["process-spawn"]));
        assert_eq!(plugin.pane_profiles().unwrap(), vec!["default", "quiet"]);
        let context = PluginContext {
            cwd: Some("/chosen/pane".into()),
            terminal_size: (39, 12),
            ..context()
        };
        for name in ["default", "quiet"] {
            let response = plugin.on_hook(
                &Hook::PaneLaunch {
                    plugin_id: "sample-tools".into(),
                    profile: Some(name.into()),
                    cwd: context.cwd.clone(),
                },
                &context,
            );
            assert!(
                matches!(response, Some(PluginAction::OpenTab { cwd: Some(cwd), profile: Some(profile), .. }) if cwd == "/chosen/pane" && profile == name)
            );
        }
        assert!(matches!(
            plugin.on_hook(
                &Hook::CommandInvoked {
                    command_id: "sample-tools.open-quiet".into()
                },
                &context
            ),
            Some(PluginAction::OpenTab { cwd: None, .. })
        ));
    }

    #[test]
    fn local_plugin_full_lifecycle_is_rebuild_free_and_profile_configurable() {
        let (_temp, source, root, registry_path) = fixture();
        let validated = execute_admin_at(
            PluginAdminCommand::Validate {
                directory: source.clone(),
            },
            &registry_path,
            &root,
        )
        .unwrap();
        assert!(validated.contains("permissions=process-spawn"));
        assert!(validated.contains("source="));
        assert!(validated.contains("fingerprint=fnv1a64:"));

        let installed = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: permissions(&["process-spawn"]),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        assert!(installed.contains("enabled=true"));
        assert!(installed.contains("grants=process-spawn"));

        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let (mut plugins, statuses) = discover_from_registry(&registry, &root);
        assert_eq!(plugins.len(), 1);
        assert_eq!(statuses.len(), 1);
        assert_eq!(plugins[0].commands().len(), 2);
        let context = PluginContext {
            cwd: Some("/tmp/project".into()),
            ..context()
        };
        assert!(matches!(
            plugins[0].on_hook(
                &Hook::CommandInvoked {
                    command_id: "sample-tools.open-default".into()
                },
                &context
            ),
            Some(PluginAction::OpenTab {
                program,
                cwd: Some(cwd),
                profile: Some(profile),
                ..
            }) if program == "sample-cli" && cwd == "/tmp/project" && profile == "default"
        ));
        assert!(matches!(
            plugins[0].on_hook(
                &Hook::WorkspaceRestore {
                    plugin_id: "sample-tools".into(),
                    profile: Some("default".into()),
                    cwd: Some("/saved/workspace".into())
                },
                &context
            ),
            Some(PluginAction::OpenTab {
                cwd: Some(cwd), profile: Some(profile), ..
            }) if cwd == "/saved/workspace" && profile == "default"
        ));

        execute_admin_at(
            PluginAdminCommand::Configure {
                id: "sample-tools".into(),
                change: PluginConfigurationChange::DefaultProfile("quiet".into()),
            },
            &registry_path,
            &root,
        )
        .unwrap();
        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let (mut plugins, _) = discover_from_registry(&registry, &root);
        assert!(matches!(
            plugins[0].on_hook(
                &Hook::WorkspaceRestore {
                    plugin_id: "sample-tools".into(),
                    profile: None,
                    cwd: None
                },
                &context
            ),
            Some(PluginAction::OpenTab {
                profile: Some(profile), cwd: None, ..
            }) if profile == "quiet"
        ));

        execute_admin_at(
            PluginAdminCommand::Configure {
                id: "sample-tools".into(),
                change: PluginConfigurationChange::ClearDefaultProfile,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        assert!(registry.plugins[0].default_profile.is_none());
        let (mut plugins, statuses) = discover_from_registry(&registry, &root);
        assert_eq!(statuses[0].profiles.len(), 2);
        assert!(statuses[0].configured_default_profile.is_none());
        assert!(matches!(
            plugins[0].on_hook(
                &Hook::WorkspaceRestore {
                    plugin_id: "sample-tools".into(),
                    profile: None,
                    cwd: None
                },
                &context
            ),
            Some(PluginAction::OpenTab {
                profile: Some(profile), cwd: None, ..
            }) if profile == "default"
        ));

        execute_admin_at(
            PluginAdminCommand::Disable {
                id: "sample-tools".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();
        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let (loaded, statuses) = discover_from_registry(&registry, &root);
        assert!(loaded.is_empty());
        assert!(statuses[0].requested.contains(Capability::ProcessSpawn));
        assert!(statuses[0].granted.contains(Capability::ProcessSpawn));
        execute_admin_at(
            PluginAdminCommand::Enable {
                id: "sample-tools".into(),
                grants: PluginPermissions::default(),
            },
            &registry_path,
            &root,
        )
        .unwrap();

        let json = execute_admin_at(
            PluginAdminCommand::Show {
                id: "sample-tools".into(),
                json: true,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        let shown: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            shown[0]["requested_permissions"],
            serde_json::json!(["process-spawn"])
        );
        assert!(json.contains("\"source\""));

        execute_admin_at(
            PluginAdminCommand::Remove {
                id: "sample-tools".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();
        assert!(!root.join("sample-tools").exists());
        assert!(
            PluginRegistry::load_from(&registry_path)
                .unwrap()
                .plugins
                .is_empty()
        );
    }

    #[test]
    fn reviewed_install_rejects_source_drift_before_mutating_state() {
        let (_temp, source, root, registry_path) = fixture();
        let preview = preview_install(&source).unwrap();
        assert_eq!(preview.id, "sample-tools");
        assert_eq!(preview.version, "1.0.0");
        assert_eq!(preview.profile_count, 2);
        assert_eq!(preview.command_count, 0);
        assert_eq!(preview.permissions, permissions(&["process-spawn"]));
        assert_eq!(preview.source, source.canonicalize().unwrap());
        assert!(preview.source_fingerprint.starts_with("fnv1a64:"));

        fs::write(
            source.join(MANIFEST_NAME),
            manifest("2.0.0", "", "sample-cli"),
        )
        .unwrap();
        let error = execute_admin_at(
            PluginAdminCommand::InstallReviewed {
                directory: source.clone(),
                grants: permissions(&["process-spawn"]),
                enable: true,
                expected_source_fingerprint: preview.source_fingerprint,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("source changed after review"));
        assert!(!registry_path.exists());
        assert!(!root.exists());

        let current = preview_install(&source).unwrap();
        execute_admin_at(
            PluginAdminCommand::InstallReviewed {
                directory: current.source,
                grants: PluginPermissions::default(),
                enable: false,
                expected_source_fingerprint: current.source_fingerprint,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let (_, statuses) = discover_from_registry(&registry, &root);
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].version, "2.0.0");
        assert!(!statuses[0].enabled);
        assert!(statuses[0].granted.is_empty());
    }

    #[test]
    fn api_version_must_be_exactly_one() {
        let (_temp, source, _root, _registry_path) = fixture();
        for version in [0, 2, 32, 33] {
            fs::write(
                source.join(MANIFEST_NAME),
                manifest("1.0.0", "", "sample")
                    .replace("api_version = 1", &format!("api_version = {version}")),
            )
            .unwrap();
            let error = PluginManifest::from_directory(&source).unwrap_err();
            assert!(
                format!("{error:#}").contains("supports API version 1"),
                "{version}: {error:#}"
            );
        }
        let missing = manifest("1.0.0", "", "sample").replace("api_version = 1\n", "");
        assert!(toml::from_str::<PluginManifest>(&missing).is_err());
    }

    #[test]
    fn unknown_removed_duplicate_and_misscoped_permissions_are_rejected() {
        for (extra, expected) in [
            (", \"network\"", "unknown plugin permission"),
            (", \"secrets\"", "unknown plugin permission"),
            (", \"terminal-text\"", "unknown plugin permission"),
            (", \"workspace-topology\"", "unknown plugin permission"),
            (", \"process-query\"", "unknown plugin permission"),
            (", \"process-pane-create\"", "unknown plugin permission"),
            (
                ", \"ui-actions:all-windows\"",
                "does not take a window scope",
            ),
            (
                ", \"window-focus:all-windows\"",
                "does not take a window scope",
            ),
            (", \"workspace-query:window\"", "unknown scope"),
            (", \"process-spawn\"", "more than once"),
            (
                ", \"workspace-query\", \"workspace-query:all-windows\"",
                "more than once",
            ),
        ] {
            let error = toml::from_str::<PluginManifest>(&manifest("1.0.0", extra, "sample"))
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{extra}: {error}");
        }
    }

    #[test]
    fn grants_may_not_exceed_requests_and_enabling_needs_every_request() {
        let (_temp, source, root, registry_path) = fixture();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest("1.0.0", ", \"workspace-query\"", "sample"),
        )
        .unwrap();

        install(
            &source,
            permissions(&["process-spawn"]),
            false,
            &registry_path,
            &root,
        )
        .unwrap();
        let error = execute_admin_at(
            PluginAdminCommand::Enable {
                id: "sample-tools".into(),
                grants: PluginPermissions::default(),
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(
            format!("{error:#}")
                .contains("cannot enable until permissions are granted: workspace-query")
        );
        let error = install(
            &source,
            permissions(&["process-spawn"]),
            true,
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("cannot enable until permissions are granted"));

        for (grants, unrequested) in [
            (&["process-spawn", "navigate"][..], "navigate"),
            (
                &["process-spawn", "workspace-query:all-windows"][..],
                "workspace-query:all-windows",
            ),
        ] {
            let error =
                install(&source, permissions(grants), false, &registry_path, &root).unwrap_err();
            assert!(
                format!("{error:#}").contains(&format!(
                    "cannot grant unrequested permissions: {unrequested}"
                )),
                "{error:#}"
            );
        }

        // A tampered registry grant beyond the request cannot enable either.
        let mut registry = PluginRegistry::load_from(&registry_path).unwrap();
        registry.plugins[0].grants = permissions(&["process-spawn", "workspace-query:all-windows"]);
        registry.save_to(&registry_path).unwrap();
        let error = execute_admin_at(
            PluginAdminCommand::Enable {
                id: "sample-tools".into(),
                grants: PluginPermissions::default(),
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(
            format!("{error:#}")
                .contains("cannot grant unrequested permissions: workspace-query:all-windows")
        );

        install(
            &source,
            permissions(&["process-spawn", "workspace-query"]),
            true,
            &registry_path,
            &root,
        )
        .unwrap();
        let (loaded, statuses) = discover_runtime_plugins_at(&registry_path, &root).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            statuses[0].granted,
            permissions(&["process-spawn", "workspace-query"])
        );
    }

    #[test]
    fn installed_plugin_grants_change_against_the_installed_manifest() {
        let (_temp, source, root, registry_path) = fixture();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest("1.0.0", ", \"workspace-query\"", "sample"),
        )
        .unwrap();
        install(
            &source,
            PluginPermissions::default(),
            false,
            &registry_path,
            &root,
        )
        .unwrap();
        // Grants come from the installed manifest; the source may be gone.
        fs::remove_dir_all(&source).unwrap();
        let admin = |command| execute_admin_at(command, &registry_path, &root);
        let stored = || {
            let registry = PluginRegistry::load_from(&registry_path).unwrap();
            (
                registry.plugins[0].grants.clone(),
                registry.plugins[0].enabled,
            )
        };

        // A partial grant leaves enabling refused, with what is missing.
        admin(PluginAdminCommand::Grant {
            id: "sample-tools".into(),
            grants: permissions(&["process-spawn"]),
        })
        .unwrap();
        assert_eq!(stored(), (permissions(&["process-spawn"]), false));
        let error = admin(PluginAdminCommand::Enable {
            id: "sample-tools".into(),
            grants: PluginPermissions::default(),
        })
        .unwrap_err();
        assert!(
            format!("{error:#}")
                .contains("cannot enable until permissions are granted: workspace-query"),
            "{error:#}"
        );

        // Nothing beyond the manifest's requests, or wider, can be granted.
        for grants in [&["navigate"][..], &["workspace-query:all-windows"][..]] {
            let error = admin(PluginAdminCommand::Grant {
                id: "sample-tools".into(),
                grants: permissions(grants),
            })
            .unwrap_err();
            assert!(
                format!("{error:#}").contains("cannot grant unrequested permissions"),
                "{error:#}"
            );
        }
        assert_eq!(stored(), (permissions(&["process-spawn"]), false));

        // Enable can grant the rest itself.
        let message = admin(PluginAdminCommand::Enable {
            id: "sample-tools".into(),
            grants: permissions(&["workspace-query"]),
        })
        .unwrap();
        assert!(message.ends_with("enabled"), "{message}");
        assert_eq!(
            stored(),
            (permissions(&["process-spawn", "workspace-query"]), true)
        );

        // Revoking a requested permission disables the plugin.
        let message = admin(PluginAdminCommand::Revoke {
            id: "sample-tools".into(),
            permissions: permissions(&["workspace-query"]),
        })
        .unwrap();
        assert!(message.contains("disabled until every requested permission"));
        assert_eq!(stored(), (permissions(&["process-spawn"]), false));

        // A reviewed change applies only to the entry that was reviewed.
        let preview = preview_grants_at("sample-tools", &registry_path, &root).unwrap();
        assert_eq!(
            preview.requested,
            permissions(&["process-spawn", "workspace-query"])
        );
        assert_eq!(preview.current_grants, permissions(&["process-spawn"]));
        assert!(!preview.enabled);
        admin(PluginAdminCommand::Configure {
            id: "sample-tools".into(),
            change: PluginConfigurationChange::DefaultProfile("quiet".into()),
        })
        .unwrap();
        let error = admin(preview.command(preview.requested.clone(), true)).unwrap_err();
        assert!(
            format!("{error:#}").contains("changed after review"),
            "{error:#}"
        );
        assert!(!stored().1);
        let preview = preview_grants_at("sample-tools", &registry_path, &root).unwrap();
        let error = admin(preview.command(permissions(&["process-spawn"]), true)).unwrap_err();
        assert!(format!("{error:#}").contains("cannot enable until permissions are granted"));
        admin(preview.command(preview.requested.clone(), true)).unwrap();
        assert_eq!(stored(), (preview.requested.clone(), true));
        let (loaded, _) = discover_runtime_plugins_at(&registry_path, &root).unwrap();
        assert_eq!(loaded.len(), 1);
    }

    #[test]
    fn schema_tamper_and_resource_limits_fail_closed() {
        let (_temp, source, root, registry_path) = fixture();
        let mut oversized = PluginManifest {
            api_version: 1,
            id: "large".into(),
            name: "Large".into(),
            version: "1".into(),
            description: String::new(),
            permissions: permissions(&["process-spawn"]),
            profiles: vec![DeclarativeProfile {
                id: "default".into(),
                name: "Default".into(),
                description: String::new(),
                program: "sample".into(),
                args: vec!["x".into(); MAX_ARGS + 1],
                working_directory: WorkingDirectory::Active,
            }],
            commands: Vec::new(),
            settings: Vec::new(),
            event_handler: None,
        };
        assert!(oversized.validate().is_err());
        oversized.profiles[0].args.clear();
        oversized.profiles.push(oversized.profiles[0].clone());
        assert!(oversized.validate().is_err());

        install(
            &source,
            permissions(&["process-spawn"]),
            true,
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            root.join("sample-tools").join(MANIFEST_NAME),
            manifest("tampered", "", "sample"),
        )
        .unwrap();
        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let (loaded, statuses) = discover_from_registry(&registry, &root);
        assert!(loaded.is_empty());
        assert!(
            statuses[0]
                .diagnostic
                .as_deref()
                .is_some_and(|error| error.contains("fingerprint"))
        );

        fs::write(&registry_path, vec![b'x'; MAX_REGISTRY_BYTES + 1]).unwrap();
        let error = PluginRegistry::load_from(&registry_path).unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(std::io::ErrorKind::InvalidData)
        );

        fs::write(&registry_path, "schema_version = 99\nplugins = []\n").unwrap();
        assert!(PluginRegistry::load_from(&registry_path).is_err());

        let mut malformed = PluginRegistry::default();
        malformed.plugins.push(InstalledPlugin {
            id: "sample-tools".into(),
            enabled: false,
            source: PathBuf::from("relative/source"),
            source_fingerprint: "not-a-fingerprint".into(),
            grants: PluginPermissions::default(),
            default_profile: None,
            settings: BTreeMap::new(),
        });
        assert!(malformed.validate().is_err());
    }

    #[test]
    fn launch_context_is_gated_by_workspace_query_and_bounded() {
        let parsed: PluginManifest = toml::from_str(&manifest(
            "2.0.0",
            ", \"workspace-query\"",
            "context-fixture",
        ))
        .unwrap();
        parsed.validate().unwrap();
        let grants = permissions(&["process-spawn", "workspace-query"]);
        validate_grants(&parsed, &grants).unwrap();
        let mut plugin = DeclarativePlugin::new(parsed.clone(), None, grants.clone());
        let context = PluginContext {
            window_id: 3,
            cwd: Some("/workspace/fixture".into()),
            tab_count: 4,
            active_tab: 2,
            active_tab_id: Some(12),
            terminal_size: (132, 43),
        };
        let response = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "sample-tools.open-default".into(),
            },
            &context,
        );
        let Some(PluginAction::OpenTab { env, .. }) = response else {
            panic!("structured profile did not produce an open-tab action")
        };
        assert_eq!(env.len(), 1);
        let context_json = env.get(STRUCTURED_CONTEXT_ENV).unwrap();
        let value: serde_json::Value = serde_json::from_str(context_json).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["plugin_id"], "sample-tools");
        assert_eq!(value["profile_id"], "default");
        assert_eq!(value["active_working_directory"], "/workspace/fixture");
        assert_eq!(value["tab_count"], 4);
        assert_eq!(value["active_tab_index"], 2);
        assert_eq!(value["active_tab_id"], 12);
        assert_eq!(value["terminal"]["columns"], 132);
        assert_eq!(value["terminal"]["rows"], 43);
        // The documented plugin API, field for field and in its order.
        assert_eq!(
            context_json,
            r#"{"schema_version":1,"plugin_id":"sample-tools","profile_id":"default","active_working_directory":"/workspace/fixture","tab_count":4,"active_tab_index":2,"active_tab_id":12,"terminal":{"columns":132,"rows":43}}"#
        );
        assert!(context_json.len() <= MAX_STRUCTURED_CONTEXT_BYTES);

        let response = plugin.on_hook(
            &Hook::WorkspaceRestore {
                plugin_id: "sample-tools".into(),
                profile: Some("default".into()),
                cwd: Some("/archived/workspace".into()),
            },
            &context,
        );
        let Some(PluginAction::OpenTab { cwd, env, .. }) = response else {
            panic!("structured workspace restore did not produce an open-tab action")
        };
        assert_eq!(cwd.as_deref(), Some("/archived/workspace"));
        let restored: serde_json::Value =
            serde_json::from_str(env.get(STRUCTURED_CONTEXT_ENV).unwrap()).unwrap();
        assert_eq!(restored["active_working_directory"], "/archived/workspace");

        // Without the grant, no context reaches the profile.
        let mut ungranted = DeclarativePlugin::new(
            toml::from_str(&manifest("1.0.0", "", "context-fixture")).unwrap(),
            None,
            permissions(&["process-spawn"]),
        );
        let Some(PluginAction::OpenTab { env, .. }) = ungranted.on_hook(
            &Hook::CommandInvoked {
                command_id: "sample-tools.open-default".into(),
            },
            &context,
        ) else {
            panic!("profile did not produce an open-tab action")
        };
        assert!(env.is_empty());

        // Oversized context fails visibly instead of truncating silently.
        let response = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "sample-tools.open-default".into(),
            },
            &PluginContext {
                cwd: Some("x".repeat(MAX_STRUCTURED_CONTEXT_BYTES + 1)),
                ..context
            },
        );
        assert!(matches!(
            response,
            Some(PluginAction::ShowNotice { title, lines })
                if title.contains("context unavailable")
                    && lines[0].contains("structured plugin launch context exceeds")
        ));
    }

    #[test]
    fn scoped_grants_round_trip_install_and_discovery() {
        let (_temp, source, root, registry_path) = fixture();
        let scoped = handler_manifest(
            &[
                "workspace-query:all-windows",
                "navigate:all-windows",
                "pane-create",
            ],
            &["app-ready"],
        );
        fs::write(
            source.join(MANIFEST_NAME),
            toml::to_string(&scoped).unwrap(),
        )
        .unwrap();
        install(
            &source,
            scoped.permissions.clone(),
            true,
            &registry_path,
            &root,
        )
        .unwrap();
        let stored = fs::read_to_string(&registry_path).unwrap();
        assert!(
            stored.contains("\"workspace-query:all-windows\""),
            "{stored}"
        );
        assert!(stored.contains("\"pane-create\""), "{stored}");
        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        assert_eq!(registry.plugins[0].grants, scoped.permissions);
        let (loaded, statuses) = discover_from_registry(&registry, &root);
        assert_eq!(loaded.len(), 1);
        assert_eq!(statuses[0].granted, scoped.permissions);
        assert_eq!(
            statuses[0].granted.scope(Capability::Navigate),
            Some(PermissionScope::AllWindows)
        );
        assert_eq!(
            statuses[0].granted.scope(Capability::PaneCreate),
            Some(PermissionScope::Window)
        );
    }

    #[test]
    fn lifecycle_events_carry_the_origin_window_and_grant_scoped_context() {
        let events = ["app-ready", "tab-created", "tab-closed", "tab-switched"];
        let manifest = handler_manifest(&["ui-actions", "workspace-query"], &events);
        manifest.validate().unwrap();
        let grants = manifest.permissions.clone();
        let invocation = invocation_for(
            manifest,
            grants.clone(),
            &Hook::TabSwitched {
                from: Some(11),
                to: 19,
            },
            &PluginContext {
                window_id: 7,
                cwd: Some("/workspace/project".into()),
                tab_count: 5,
                active_tab: 4,
                active_tab_id: Some(19),
                terminal_size: (120, 36),
            },
        );
        assert_eq!(invocation.plugin_id, "handler-fixture");
        assert_eq!(invocation.authority.grants(), &grants);
        // A plugin without settings sends none, with the event or its results.
        assert!(invocation.settings.is_none());
        let value: serde_json::Value = serde_json::from_str(&invocation.event_json).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["origin_window_id"], 7);
        assert_eq!(value["event"]["type"], "tab-switched");
        assert_eq!(value["event"]["from"], 11);
        assert_eq!(value["event"]["to"], 19);
        assert_eq!(value["context"]["active_tab_index"], 4);
        assert_eq!(value["context"]["active_tab_id"], 19);
        assert_eq!(
            value["context"]["active_working_directory"],
            "/workspace/project"
        );
        assert_eq!(value["context"]["terminal"]["columns"], 120);
        // The documented plugin API, field for field.
        assert!(
            invocation.event_json.contains(
                r#""context":{"active_tab_id":19,"active_tab_index":4,"active_working_directory":"/workspace/project","tab_count":5,"terminal":{"columns":120,"rows":36}}"#
            ),
            "{}",
            invocation.event_json
        );
        assert!(invocation.event_json.len() <= MAX_PERSISTENT_SOURCE_EVENT_BYTES);
        assert!(!invocation.event_json.contains("command"));

        // A window's first active tab follows no earlier one.
        let manifest = handler_manifest(&["ui-actions"], &events);
        let grants = manifest.permissions.clone();
        let invocation = invocation_for(
            manifest,
            grants,
            &Hook::TabSwitched { from: None, to: 1 },
            &context(),
        );
        let value: serde_json::Value = serde_json::from_str(&invocation.event_json).unwrap();
        assert!(value["event"]["from"].is_null());
        assert_eq!(value["event"]["to"], 1);

        let manifest = handler_manifest(&["ui-actions"], &events);
        let grants = manifest.permissions.clone();
        let invocation = invocation_for(
            manifest,
            grants,
            &Hook::AppReady,
            &PluginContext {
                cwd: Some("/must-not-leak".into()),
                ..context()
            },
        );
        let value: serde_json::Value = serde_json::from_str(&invocation.event_json).unwrap();
        assert!(value.get("context").is_none());
        assert_eq!(value["origin_window_id"], 3);
        assert!(!invocation.event_json.contains("must-not-leak"));

        let unsubscribed = handler_manifest(&["ui-actions"], &["app-ready"]);
        let grants = unsubscribed.permissions.clone();
        let mut plugin = DeclarativePlugin::new(unsubscribed, None, grants);
        assert!(
            plugin
                .on_hook(&Hook::TabCreated { tab_id: 1 }, &context())
                .is_none()
        );
    }

    #[test]
    fn event_handler_manifest_rules_fail_closed() {
        let expect_error = |manifest: PluginManifest, expected: &str| {
            let error = manifest.validate().unwrap_err().to_string();
            assert!(error.contains(expected), "{expected}: {error}");
        };
        let base = handler_manifest(&["ui-actions"], &["app-ready"]);
        base.validate().unwrap();

        let mut missing_subscribe = base.clone();
        missing_subscribe
            .permissions
            .remove(Capability::EventSubscribe);
        expect_error(missing_subscribe, "event-subscribe");

        let mut no_events = base.clone();
        no_events.event_handler.as_mut().unwrap().events.clear();
        expect_error(no_events, "at least one event");

        let mut profile_only: PluginManifest =
            toml::from_str(&manifest("1.0.0", ", \"event-subscribe\"", "profile")).unwrap();
        expect_error(
            profile_only.clone(),
            "event-subscribe permission require each other",
        );
        profile_only.permissions = permissions(&["process-spawn", "ui-actions"]);
        expect_error(profile_only, "ui-actions requires an event handler");

        let mut no_spawn = base.clone();
        no_spawn.permissions.remove(Capability::ProcessSpawn);
        expect_error(no_spawn, "must request process-spawn");

        let base_grants = permissions(&["process-spawn", "event-subscribe"]);
        assert!(
            validate_grants(&base, &base_grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: ui-actions")
        );
    }

    #[test]
    fn events_reserve_protocol_space() {
        let manifest = handler_manifest(&["ui-actions"], &["tab-created"]);
        manifest.validate().unwrap();
        let grants = manifest.permissions.clone();
        let invocation = invocation_for(
            manifest,
            grants,
            &Hook::TabCreated { tab_id: usize::MAX },
            &PluginContext {
                window_id: u64::MAX,
                cwd: None,
                tab_count: usize::MAX,
                active_tab: usize::MAX,
                active_tab_id: Some(usize::MAX),
                terminal_size: (usize::MAX, usize::MAX),
            },
        );
        assert!(invocation.event_json.len() <= MAX_PERSISTENT_SOURCE_EVENT_BYTES);
    }

    #[test]
    fn capabilities_used_through_a_handler_require_one() {
        for capability in Capability::ALL {
            let name = capability.name();
            if matches!(
                capability,
                Capability::ProcessSpawn | Capability::EventSubscribe | Capability::CommandOutput
            ) {
                continue;
            }
            let events: &[&str] = if capability == Capability::WindowClose {
                &["command-invoked"]
            } else {
                &["app-ready"]
            };
            handler_manifest(&[name], events)
                .validate()
                .unwrap_or_else(|error| panic!("{name} with a handler: {error}"));
            let profile_only: PluginManifest =
                toml::from_str(&manifest("1.0.0", &format!(", {name:?}"), "profile")).unwrap();
            let without_handler = profile_only.validate();
            if capability.requires_event_handler() {
                assert!(
                    without_handler
                        .unwrap_err()
                        .to_string()
                        .contains(&format!("{name} requires an event handler")),
                    "{name}"
                );
            } else {
                without_handler.unwrap_or_else(|error| panic!("{name} without a handler: {error}"));
            }
        }
    }

    #[test]
    fn handler_authority_carries_grants_scopes_and_command_origin() {
        let close = StructuredResponseAction::CloseWindow {
            action_id: "close".into(),
            window_id: None,
        };
        let manifest = handler_manifest(
            &["workspace-query:all-windows", "window-close"],
            &["app-ready", "command-invoked"],
        );
        manifest.validate().unwrap();
        let grants = manifest.permissions.clone();

        let command = invocation_for(
            manifest.clone(),
            grants.clone(),
            &Hook::CommandInvoked {
                command_id: "handler-fixture.run".into(),
            },
            &context(),
        );
        assert_eq!(
            command.authority.grants().scope(Capability::WorkspaceQuery),
            Some(PermissionScope::AllWindows)
        );
        command
            .authority
            .check(std::slice::from_ref(&close))
            .unwrap();

        let lifecycle = invocation_for(manifest.clone(), grants, &Hook::AppReady, &context());
        assert!(
            lifecycle
                .authority
                .check(std::slice::from_ref(&close))
                .unwrap_err()
                .contains("outside an invoked plugin command")
        );

        // Ungranted capabilities never reach the handler's authority.
        let partial = invocation_for(
            manifest,
            permissions(&["process-spawn", "event-subscribe", "workspace-query"]),
            &Hook::CommandInvoked {
                command_id: "handler-fixture.run".into(),
            },
            &context(),
        );
        assert!(
            partial
                .authority
                .check(std::slice::from_ref(&close))
                .unwrap_err()
                .contains("without the window-close permission")
        );
        assert_eq!(
            partial.authority.grants().scope(Capability::WorkspaceQuery),
            Some(PermissionScope::Window)
        );
    }

    #[test]
    fn window_close_scope_selects_its_command_requirement() {
        let window_scoped = handler_manifest(&["window-close"], &["app-ready"]);
        assert!(
            window_scoped
                .validate()
                .unwrap_err()
                .to_string()
                .contains("command-scoped")
        );
        handler_manifest(&["window-close"], &["command-invoked"])
            .validate()
            .unwrap();
        handler_manifest(&["window-close:all-windows"], &["app-ready"])
            .validate()
            .unwrap();
    }

    #[test]
    fn handler_commands_are_bounded_namespaced_and_exact() {
        let manifest = handler_manifest(&["ui-actions"], &["command-invoked"]);
        manifest.validate().unwrap();
        let grants = manifest.permissions.clone();
        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, grants.clone());
        let commands = plugin.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].id, "handler-fixture.run");
        assert_eq!(commands[0].name, "Run");
        assert_eq!(commands[0].category.as_deref(), Some("Handler Fixture"));

        let context = PluginContext {
            cwd: Some("/workspace/private".into()),
            ..context()
        };
        let Some(PluginAction::InvokeStructuredHandler { invocation }) = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "handler-fixture.run".into(),
            },
            &context,
        ) else {
            panic!("declared command did not create a handler invocation")
        };
        let value: serde_json::Value = serde_json::from_str(&invocation.event_json).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["plugin_id"], "handler-fixture");
        assert_eq!(value["event"]["type"], "command-invoked");
        assert_eq!(value["event"]["command_id"], "handler-fixture.run");
        assert!(value.get("context").is_none());
        assert!(invocation.event_json.len() <= MAX_PERSISTENT_SOURCE_EVENT_BYTES);

        assert!(
            plugin
                .on_hook(
                    &Hook::CommandInvoked {
                        command_id: "handler-fixture.undeclared".into(),
                    },
                    &context,
                )
                .is_none()
        );
    }

    #[test]
    fn handler_command_contract_fails_closed() {
        let manifest = handler_manifest(&["ui-actions"], &["command-invoked"]);

        let mut missing_subscription = manifest.clone();
        missing_subscription.event_handler.as_mut().unwrap().events =
            BTreeSet::from([StructuredEventKind::AppReady]);
        assert!(
            missing_subscription
                .validate()
                .unwrap_err()
                .to_string()
                .contains("subscribed to command-invoked")
        );

        let mut no_commands = manifest.clone();
        no_commands.commands.clear();
        assert!(
            no_commands
                .validate()
                .unwrap_err()
                .to_string()
                .contains("at least one handler command")
        );

        let mut duplicate = manifest.clone();
        duplicate.commands.push(duplicate.commands[0].clone());
        assert!(
            duplicate
                .validate()
                .unwrap_err()
                .to_string()
                .contains("duplicate handler command")
        );

        let mut generated_conflict = manifest.clone();
        generated_conflict.profiles.push(DeclarativeProfile {
            id: "workspace".into(),
            name: "Workspace".into(),
            description: String::new(),
            program: "fixture".into(),
            args: Vec::new(),
            working_directory: WorkingDirectory::Active,
        });
        generated_conflict.commands[0].id = "open-workspace".into();
        assert!(
            generated_conflict
                .validate()
                .unwrap_err()
                .to_string()
                .contains("generated profile command")
        );

        let mut oversized = manifest;
        oversized.commands = vec![oversized.commands[0].clone(); MAX_HANDLER_COMMANDS + 1];
        assert!(
            oversized
                .validate()
                .unwrap_err()
                .to_string()
                .contains("more than 32 handler commands")
        );
    }

    #[test]
    fn setting_schema_is_typed_and_bounded() {
        let manifest = configurable_plugin_manifest();
        manifest.validate().unwrap();
        let defaults = effective_settings(&manifest, &BTreeMap::new()).unwrap();
        assert_eq!(
            defaults["enabled-output"],
            PluginSettingValue::Boolean(true)
        );
        assert_eq!(defaults["line-count"], PluginSettingValue::Integer(10));
        assert_eq!(
            defaults["detail"],
            PluginSettingValue::Text("summary".into())
        );

        let mut duplicate = manifest.clone();
        duplicate.settings.push(duplicate.settings[0].clone());
        assert!(
            duplicate
                .validate()
                .unwrap_err()
                .to_string()
                .contains("duplicate setting")
        );

        let mut invalid_integer = manifest.clone();
        let DeclarativeSetting::Integer { default, .. } = &mut invalid_integer.settings[1] else {
            panic!("fixture setting changed type")
        };
        *default = 101;
        assert!(
            invalid_integer
                .validate()
                .unwrap_err()
                .to_string()
                .contains("min <= default <= max")
        );

        let mut invalid_choice = manifest.clone();
        let DeclarativeSetting::Choice { choices, .. } = &mut invalid_choice.settings[3] else {
            panic!("fixture setting changed type")
        };
        choices.push("summary".into());
        assert!(
            invalid_choice
                .validate()
                .unwrap_err()
                .to_string()
                .contains("duplicate value")
        );

        let mut oversized = manifest;
        oversized.settings = vec![oversized.settings[0].clone(); MAX_SETTINGS + 1];
        assert!(
            oversized
                .validate()
                .unwrap_err()
                .to_string()
                .contains("more than 32 settings")
        );
    }

    #[test]
    fn settings_persist_reload_and_reach_profiles_and_handlers() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join(MANIFEST_NAME), configurable_manifest("1.0.0")).unwrap();
        install(
            &source,
            permissions(&["process-spawn", "event-subscribe", "ui-actions"]),
            true,
            &registry_path,
            &root,
        )
        .unwrap();
        for (key, value) in [
            ("enabled-output", "false"),
            ("line-count", "42"),
            ("label", "configured"),
            ("detail", "full"),
        ] {
            execute_admin_at(
                PluginAdminCommand::Configure {
                    id: "configurable-tool".into(),
                    change: PluginConfigurationChange::Set {
                        key: key.into(),
                        value: value.into(),
                    },
                },
                &registry_path,
                &root,
            )
            .unwrap();
        }

        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        assert_eq!(registry.plugins[0].settings.len(), 4);
        let (mut plugins, statuses) = discover_from_registry(&registry, &root);
        assert_eq!(plugins.len(), 1);
        assert_eq!(
            statuses[0].settings["line-count"],
            PluginSettingValue::Integer(42)
        );
        let context = PluginContext {
            cwd: Some("/workspace".into()),
            ..context()
        };
        let Some(PluginAction::OpenTab { env, .. }) = plugins[0].on_hook(
            &Hook::CommandInvoked {
                command_id: "configurable-tool.open-run".into(),
            },
            &context,
        ) else {
            panic!("configured profile did not produce an open-tab action")
        };
        assert!(!env.contains_key(STRUCTURED_CONTEXT_ENV));
        let profile_settings: serde_json::Value =
            serde_json::from_str(env.get(STRUCTURED_SETTINGS_ENV).unwrap()).unwrap();
        assert_eq!(profile_settings["schema_version"], 1);
        assert_eq!(profile_settings["values"]["line-count"], 42);
        assert_eq!(profile_settings["values"]["enabled-output"], false);

        let Some(PluginAction::InvokeStructuredHandler { invocation }) = plugins[0].on_hook(
            &Hook::CommandInvoked {
                command_id: "configurable-tool.show-settings".into(),
            },
            &context,
        ) else {
            panic!("configured handler command did not produce an invocation")
        };
        // Settings travel with the invocation, so the event's results carry
        // them too; the event envelope itself holds none.
        let settings = invocation.settings.as_ref().unwrap();
        assert_eq!(settings["detail"], "full");
        assert_eq!(settings["label"], "configured");
        let handler_event: serde_json::Value =
            serde_json::from_str(&invocation.event_json).unwrap();
        assert!(handler_event.get("settings").is_none());

        let shown: serde_json::Value = serde_json::from_str(
            &execute_admin_at(
                PluginAdminCommand::Show {
                    id: "configurable-tool".into(),
                    json: true,
                },
                &registry_path,
                &root,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(shown[0]["settings"]["line-count"], 42);

        execute_admin_at(
            PluginAdminCommand::Configure {
                id: "configurable-tool".into(),
                change: PluginConfigurationChange::Unset {
                    key: "line-count".into(),
                },
            },
            &registry_path,
            &root,
        )
        .unwrap();
        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let (_, statuses) = discover_from_registry(&registry, &root);
        assert_eq!(
            statuses[0].settings["line-count"],
            PluginSettingValue::Integer(10)
        );

        execute_admin_at(
            PluginAdminCommand::Configure {
                id: "configurable-tool".into(),
                change: PluginConfigurationChange::ResetSettings,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        assert!(
            PluginRegistry::load_from(&registry_path).unwrap().plugins[0]
                .settings
                .is_empty()
        );
    }

    #[test]
    fn invalid_setting_overrides_fail_without_persistence() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join(MANIFEST_NAME), configurable_manifest("1.0.0")).unwrap();
        install(
            &source,
            permissions(&["process-spawn", "event-subscribe", "ui-actions"]),
            true,
            &registry_path,
            &root,
        )
        .unwrap();

        for (key, value, expected) in [
            ("enabled-output", "yes", "expects true or false"),
            ("line-count", "101", "outside 1..=100"),
            ("detail", "verbose", "does not permit choice"),
            ("label", "line\nbreak", "single-line"),
            ("missing", "value", "has no setting"),
        ] {
            let error = execute_admin_at(
                PluginAdminCommand::Configure {
                    id: "configurable-tool".into(),
                    change: PluginConfigurationChange::Set {
                        key: key.into(),
                        value: value.into(),
                    },
                },
                &registry_path,
                &root,
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains(expected));
            assert!(
                PluginRegistry::load_from(&registry_path).unwrap().plugins[0]
                    .settings
                    .is_empty()
            );
        }

        let mut registry = PluginRegistry::load_from(&registry_path).unwrap();
        registry.plugins[0]
            .settings
            .insert("line-count".into(), PluginSettingValue::Boolean(false));
        registry.save_to(&registry_path).unwrap();
        let (loaded, statuses) = discover_from_registry(&registry, &root);
        assert!(loaded.is_empty());
        assert!(
            statuses[0]
                .diagnostic
                .as_deref()
                .is_some_and(|error| error.contains("wrong value type"))
        );
    }

    fn command_completed(command: String, output: String, content_available: bool) -> Hook {
        Hook::CommandExecuted {
            region_id: 44,
            tab_id: 7,
            pane_id: Some(3),
            command,
            command_truncated: false,
            output,
            output_truncated: true,
            content_available,
            exit_code: Some(9),
        }
    }

    #[test]
    fn command_events_are_bounded_typed_and_command_output_gated() {
        let manifest = handler_manifest(&[], &["command-completed"]);
        manifest.validate().unwrap();
        let grants = manifest.permissions.clone();
        assert!(
            validate_grants(
                &manifest,
                &permissions(&["process-spawn", "event-subscribe"])
            )
            .unwrap_err()
            .to_string()
            .contains("permissions denied: command-output")
        );
        let context = PluginContext {
            cwd: Some("/workspace/project".into()),
            ..context()
        };
        let invocation = invocation_for(
            manifest.clone(),
            grants,
            &command_completed(
                format!("\u{0007}printf '\"{}\"'", "x".repeat(4_000)),
                format!("\u{0001}{}\u{0002}", "\\\n".repeat(12_000)),
                true,
            ),
            &context,
        );
        assert!(invocation.event_json.len() <= MAX_PERSISTENT_SOURCE_EVENT_BYTES);
        let value: serde_json::Value = serde_json::from_str(&invocation.event_json).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["event"]["type"], "command-completed");
        assert_eq!(value["event"]["region_id"], 44);
        assert_eq!(value["event"]["tab_id"], 7);
        assert_eq!(value["event"]["pane_id"], 3);
        assert_eq!(value["event"]["exit_code"], 9);
        assert!(value["event"]["command"]["truncated"].as_bool().unwrap());
        assert!(value["event"]["command"]["sanitized"].as_bool().unwrap());
        assert!(value["event"]["output"]["truncated"].as_bool().unwrap());
        assert!(value["event"]["output"]["sanitized"].as_bool().unwrap());
        // The command keeps its start and the output its end, each with its
        // control character replaced.
        let command = value["event"]["command"]["text"].as_str().unwrap();
        assert!(command.len() <= MAX_STRUCTURED_COMMAND_JSON_BYTES);
        assert!(command.starts_with("\u{fffd}printf '\""));
        let output = value["event"]["output"]["text"].as_str().unwrap();
        assert!(output.ends_with("\\\n\u{fffd}"));
        assert_eq!(output.matches('\u{fffd}').count(), 1);
        assert!(value.get("context").is_none());

        let invocation = invocation_for(
            manifest.clone(),
            manifest.permissions.clone(),
            &command_completed(
                "cargo test".into(),
                format!("{}error: 3 tests failed", "running\n".repeat(4_000)),
                true,
            ),
            &context,
        );
        let value: serde_json::Value = serde_json::from_str(&invocation.event_json).unwrap();
        let output = &value["event"]["output"];
        assert!(output["truncated"].as_bool().unwrap());
        assert!(!output["sanitized"].as_bool().unwrap());
        let text = output["text"].as_str().unwrap();
        assert!(text.ends_with("running\nerror: 3 tests failed"), "{text}");
        assert!(text.len() < 4_000 * "running\n".len());
        assert!(!invocation.event_json.contains("/workspace/project"));

        let mut ungranted = DeclarativePlugin::new(
            manifest.clone(),
            None,
            permissions(&["process-spawn", "event-subscribe"]),
        );
        assert!(
            ungranted
                .on_hook(
                    &command_completed("must-not-leak".into(), "must-not-leak".into(), true),
                    &context,
                )
                .is_none()
        );

        let mut no_request = manifest.clone();
        no_request.permissions.remove(Capability::CommandOutput);
        assert!(
            no_request
                .validate()
                .unwrap_err()
                .to_string()
                .contains("command-output")
        );
        let mut unsubscribed = manifest;
        unsubscribed.event_handler.as_mut().unwrap().events =
            BTreeSet::from([StructuredEventKind::AppReady]);
        assert!(
            unsubscribed
                .validate()
                .unwrap_err()
                .to_string()
                .contains("command-output permission require each other")
        );
    }

    #[test]
    fn command_events_never_disclose_evicted_region_text() {
        let manifest = handler_manifest(&[], &["command-completed"]);
        let grants = manifest.permissions.clone();
        let invocation = invocation_for(
            manifest,
            grants,
            &command_completed(
                "stale-secret-command".into(),
                "stale-secret-output".into(),
                false,
            ),
            &context(),
        );
        assert!(!invocation.event_json.contains("stale-secret"));
        let value: serde_json::Value = serde_json::from_str(&invocation.event_json).unwrap();
        assert!(!value["event"]["content_available"].as_bool().unwrap());
        assert_eq!(value["event"]["command"]["text"], "");
        assert_eq!(value["event"]["output"]["text"], "");
    }

    #[test]
    fn symlinked_installed_directory_never_becomes_a_runtime_plugin() {
        use std::os::unix::fs::symlink;

        let (_temp, source, root, registry_path) = fixture();
        install(
            &source,
            permissions(&["process-spawn"]),
            true,
            &registry_path,
            &root,
        )
        .unwrap();

        let installed = root.join("sample-tools");
        fs::remove_dir_all(&installed).unwrap();
        symlink(&source, &installed).unwrap();
        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let (loaded, statuses) = discover_from_registry(&registry, &root);
        assert!(loaded.is_empty());
        assert_eq!(statuses.len(), 1);
        assert!(
            statuses[0]
                .diagnostic
                .as_deref()
                .is_some_and(|error| error.contains("non-symlink directory"))
        );

        let error = execute_admin_at(
            PluginAdminCommand::Remove {
                id: "sample-tools".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("symlinked plugin installation"));
        assert_eq!(
            PluginRegistry::load_from(&registry_path)
                .unwrap()
                .plugins
                .len(),
            1,
            "a refused removal must not silently delete the registry record"
        );
    }

    #[test]
    fn administrative_files_are_staged_privately_and_never_written_through_links() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let (temp, source, root, registry_path) = fixture();
        install(
            &source,
            permissions(&["process-spawn"]),
            true,
            &registry_path,
            &root,
        )
        .unwrap();
        let installed = root.join("sample-tools");
        for file in [installed.join(MANIFEST_NAME), registry_path.clone()] {
            let metadata = fs::symlink_metadata(&file).unwrap();
            assert!(metadata.is_file(), "{}", file.display());
            assert_eq!(
                metadata.permissions().mode() & 0o077,
                0,
                "{}",
                file.display()
            );
        }
        // Nothing staged is left beside either file.
        assert_eq!(fs::read_dir(&installed).unwrap().count(), 1);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 3);

        // A link planted at the registry is refused when read, and replaced
        // rather than written through when saved.
        let sentinel = temp.path().join("sentinel");
        fs::write(&sentinel, "do-not-replace").unwrap();
        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        fs::remove_file(&registry_path).unwrap();
        symlink(&sentinel, &registry_path).unwrap();
        let error = PluginRegistry::load_from(&registry_path).unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::raw_os_error),
            Some(libc::ELOOP)
        );
        registry.save_to(&registry_path).unwrap();
        assert_eq!(fs::read_to_string(&sentinel).unwrap(), "do-not-replace");
        assert!(fs::symlink_metadata(&registry_path).unwrap().is_file());
    }

    #[test]
    fn symlinked_plugin_root_is_rejected_before_installation() {
        use std::os::unix::fs::symlink;

        let (temp, source, root, registry_path) = fixture();
        let real_root = temp.path().join("real-installed");
        let linked_root = temp.path().join("linked-installed");
        fs::create_dir(&real_root).unwrap();
        symlink(&real_root, &linked_root).unwrap();

        let error = install(
            &source,
            permissions(&["process-spawn"]),
            true,
            &registry_path,
            &linked_root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("installation root"));
        assert!(fs::read_dir(real_root).unwrap().next().is_none());
        assert!(!registry_path.exists());

        install(
            &source,
            permissions(&["process-spawn"]),
            true,
            &registry_path,
            &root,
        )
        .unwrap();
        let moved_root = temp.path().join("moved-installed");
        fs::rename(&root, &moved_root).unwrap();
        symlink(&moved_root, &root).unwrap();
        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let (loaded, statuses) = discover_from_registry(&registry, &root);
        assert!(loaded.is_empty());
        assert!(
            statuses[0]
                .diagnostic
                .as_deref()
                .is_some_and(|error| error.contains("installation root"))
        );
    }

    #[test]
    fn an_unloadable_registry_explains_the_replacing_reinstall() {
        let (_temp, source, root, registry_path) = fixture();
        install(
            &source,
            permissions(&["process-spawn"]),
            false,
            &registry_path,
            &root,
        )
        .unwrap();
        let registry = fs::read_to_string(&registry_path).unwrap();
        assert!(registry.contains("\"process-spawn\""), "{registry}");
        fs::write(
            &registry_path,
            registry.replace("\"process-spawn\"", "\"retired-permission\""),
        )
        .unwrap();
        let error = format!(
            "{:#}",
            PluginRegistry::load_from(&registry_path).unwrap_err()
        );
        assert!(
            error.contains(&registry_path.display().to_string())
                && error.contains("delete the affected [[plugins]] entry")
                && error.contains("termeric plugin install <directory> --replace"),
            "{error}"
        );

        // With the entry gone the installed copy remains, so only a replacing
        // install succeeds, exactly as the error says.
        fs::remove_file(&registry_path).unwrap();
        let install_without_replace = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: permissions(&["process-spawn"]),
                enable: false,
                replace: false,
            },
            &registry_path,
            &root,
        );
        assert!(
            format!("{:#}", install_without_replace.unwrap_err())
                .contains("unregistered installed copy already exists")
        );
        install(
            &source,
            permissions(&["process-spawn"]),
            false,
            &registry_path,
            &root,
        )
        .unwrap();
    }

    #[test]
    fn update_narrows_grants_to_dropped_or_narrowed_requests() {
        let (_temp, source, root, registry_path) = fixture();
        let write = |version: &str, extra: &str| {
            fs::write(
                source.join(MANIFEST_NAME),
                manifest(version, extra, "sample"),
            )
            .unwrap();
        };
        let update = || {
            execute_admin_at(
                PluginAdminCommand::Update {
                    id: "sample-tools".into(),
                },
                &registry_path,
                &root,
            )
        };
        let installed = || {
            let (_, statuses) = discover_runtime_plugins_at(&registry_path, &root).unwrap();
            let status = statuses.into_iter().next().unwrap();
            (status.version, status.granted, status.enabled)
        };
        write("1.0.0", ", \"workspace-query:all-windows\"");
        install(
            &source,
            permissions(&["process-spawn", "workspace-query:all-windows"]),
            true,
            &registry_path,
            &root,
        )
        .unwrap();

        // A narrowed scope narrows its grant; the plugin stays enabled.
        write("2.0.0", ", \"workspace-query\"");
        update().unwrap();
        assert_eq!(
            installed(),
            (
                "2.0.0".into(),
                permissions(&["process-spawn", "workspace-query"]),
                true
            )
        );

        // A narrowed grant is never widened back by a later update: widening a
        // scope is a new permission, exactly like a new capability.
        write("3.0.0", ", \"workspace-query:all-windows\"");
        assert!(
            format!("{:#}", update().unwrap_err())
                .contains("new permissions (workspace-query:all-windows)")
        );
        assert_eq!(installed().0, "2.0.0");

        // A dropped request drops its grant.
        write("4.0.0", "");
        update().unwrap();
        assert_eq!(
            installed(),
            ("4.0.0".into(), permissions(&["process-spawn"]), true)
        );
    }

    #[test]
    fn reviewed_update_intersects_grants_with_scope_aware_requests() {
        let (_temp, source, root, registry_path) = fixture();
        let write = |version: &str, extra: &str| {
            fs::write(
                source.join(MANIFEST_NAME),
                manifest(version, extra, "sample"),
            )
            .unwrap();
        };
        install(
            &source,
            permissions(&["process-spawn"]),
            true,
            &registry_path,
            &root,
        )
        .unwrap();

        // Added capability: the old grants do not cover it.
        write("2.0.0", ", \"workspace-query:all-windows\"");
        let preview = preview_update_at("sample-tools", &registry_path, &root).unwrap();
        assert_eq!(preview.current_permissions, permissions(&["process-spawn"]));
        assert_eq!(
            preview.candidate_permissions,
            permissions(&["process-spawn", "workspace-query:all-windows"])
        );
        let retained = preview
            .current_grants
            .intersection(&preview.candidate_permissions);
        assert_eq!(retained, permissions(&["process-spawn"]));
        let error = execute_admin_at(
            preview.command(retained.clone(), true),
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("workspace-query:all-windows"));
        execute_admin_at(preview.command(retained, false), &registry_path, &root).unwrap();
        let preview = preview_update_at("sample-tools", &registry_path, &root).unwrap();
        execute_admin_at(
            preview.command(preview.candidate_permissions.clone(), true),
            &registry_path,
            &root,
        )
        .unwrap();
        let (_, statuses) = discover_runtime_plugins_at(&registry_path, &root).unwrap();
        assert_eq!(statuses[0].version, "2.0.0");
        assert!(statuses[0].enabled);

        // Narrowed scope: an all-windows grant shrinks to the new request.
        write("3.0.0", ", \"workspace-query\"");
        let preview = preview_update_at("sample-tools", &registry_path, &root).unwrap();
        let retained = preview
            .current_grants
            .intersection(&preview.candidate_permissions);
        assert_eq!(retained, permissions(&["process-spawn", "workspace-query"]));
        execute_admin_at(preview.command(retained, true), &registry_path, &root).unwrap();

        // Widened scope: a window-scoped grant stays window-scoped, which does
        // not cover the wider request.
        write("4.0.0", ", \"workspace-query:all-windows\"");
        let preview = preview_update_at("sample-tools", &registry_path, &root).unwrap();
        let retained = preview
            .current_grants
            .intersection(&preview.candidate_permissions);
        assert_eq!(retained, permissions(&["process-spawn", "workspace-query"]));
        assert_eq!(
            preview.candidate_permissions.uncovered_by(&retained),
            vec!["workspace-query:all-windows".parse().unwrap()]
        );
        assert!(execute_admin_at(preview.command(retained, true), &registry_path, &root).is_err());

        // Removed capability: its grant is dropped.
        write("5.0.0", "");
        let preview = preview_update_at("sample-tools", &registry_path, &root).unwrap();
        let retained = preview
            .current_grants
            .intersection(&preview.candidate_permissions);
        assert_eq!(retained, permissions(&["process-spawn"]));
        execute_admin_at(preview.command(retained, true), &registry_path, &root).unwrap();
        let (_, statuses) = discover_runtime_plugins_at(&registry_path, &root).unwrap();
        assert_eq!(statuses[0].version, "5.0.0");
        assert_eq!(statuses[0].requested, permissions(&["process-spawn"]));
        assert_eq!(statuses[0].granted, permissions(&["process-spawn"]));
        assert!(statuses[0].enabled);
    }

    #[test]
    fn reviewed_update_rejects_source_and_registry_drift_without_partial_commit() {
        let (_temp, source, root, registry_path) = fixture();
        install(
            &source,
            permissions(&["process-spawn"]),
            true,
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest("2.0.0", "", "sample-v2"),
        )
        .unwrap();
        let preview = preview_update_at("sample-tools", &registry_path, &root).unwrap();
        let installed_manifest = root.join("sample-tools").join(MANIFEST_NAME);
        let installed_before = fs::read(&installed_manifest).unwrap();
        let registry_before = fs::read(&registry_path).unwrap();

        fs::write(
            source.join(MANIFEST_NAME),
            manifest("2.0.1", "", "drifted-source"),
        )
        .unwrap();
        let error = execute_admin_at(
            preview.command(permissions(&["process-spawn"]), true),
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("source changed after review"));
        assert_eq!(fs::read(&installed_manifest).unwrap(), installed_before);
        assert_eq!(fs::read(&registry_path).unwrap(), registry_before);

        let preview = preview_update_at("sample-tools", &registry_path, &root).unwrap();
        execute_admin_at(
            PluginAdminCommand::Disable {
                id: "sample-tools".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();
        let registry_after_disable = fs::read(&registry_path).unwrap();
        let error = execute_admin_at(
            preview.command(permissions(&["process-spawn"]), true),
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("changed after review"));
        assert_eq!(fs::read(&installed_manifest).unwrap(), installed_before);
        assert_eq!(fs::read(&registry_path).unwrap(), registry_after_disable);
    }

    #[test]
    fn reviewed_update_preserves_or_clears_the_configured_profile_truthfully() {
        let (_temp, source, root, registry_path) = fixture();
        install(
            &source,
            permissions(&["process-spawn"]),
            true,
            &registry_path,
            &root,
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Configure {
                id: "sample-tools".into(),
                change: PluginConfigurationChange::DefaultProfile("quiet".into()),
            },
            &registry_path,
            &root,
        )
        .unwrap();

        fs::write(
            source.join(MANIFEST_NAME),
            manifest("2.0.0", "", "sample-v2"),
        )
        .unwrap();
        let preview = preview_update_at("sample-tools", &registry_path, &root).unwrap();
        assert_eq!(preview.configured_default_profile.as_deref(), Some("quiet"));
        assert!(!preview.default_profile_will_be_cleared);
        execute_admin_at(
            preview.command(permissions(&["process-spawn"]), true),
            &registry_path,
            &root,
        )
        .unwrap();
        let (_, statuses) = discover_runtime_plugins_at(&registry_path, &root).unwrap();
        assert_eq!(
            statuses[0].configured_default_profile.as_deref(),
            Some("quiet")
        );

        let candidate = manifest("3.0.0", "", "sample-v3").replace(
            r#"
[[profiles]]
id = "quiet"
name = "Quiet Sample"
program = "sample-v3"
args = ["--quiet"]
working_directory = "inherit"
"#,
            "",
        );
        fs::write(source.join(MANIFEST_NAME), candidate).unwrap();
        let preview = preview_update_at("sample-tools", &registry_path, &root).unwrap();
        assert!(preview.default_profile_will_be_cleared);
        execute_admin_at(
            preview.command(permissions(&["process-spawn"]), true),
            &registry_path,
            &root,
        )
        .unwrap();
        let (_, statuses) = discover_runtime_plugins_at(&registry_path, &root).unwrap();
        assert_eq!(statuses[0].version, "3.0.0");
        assert!(statuses[0].configured_default_profile.is_none());
    }

    #[test]
    fn update_preserves_compatible_settings_and_rejects_schema_loss() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join(MANIFEST_NAME), configurable_manifest("1.0.0")).unwrap();
        install(
            &source,
            permissions(&["process-spawn", "event-subscribe", "ui-actions"]),
            true,
            &registry_path,
            &root,
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Configure {
                id: "configurable-tool".into(),
                change: PluginConfigurationChange::Set {
                    key: "label".into(),
                    value: "preserved".into(),
                },
            },
            &registry_path,
            &root,
        )
        .unwrap();

        let removed_label = configurable_manifest("2.0.0").replace(
            r#"[[settings]]
type = "text"
id = "label"
name = "Label"
default = "local"

"#,
            "",
        );
        fs::write(source.join(MANIFEST_NAME), removed_label).unwrap();
        let preview_error =
            preview_update_at("configurable-tool", &registry_path, &root).unwrap_err();
        assert!(format!("{preview_error:#}").contains("incompatible"));
        let error = execute_admin_at(
            PluginAdminCommand::Update {
                id: "configurable-tool".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("incompatible"));
        let shown = execute_admin_at(
            PluginAdminCommand::Show {
                id: "configurable-tool".into(),
                json: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        assert!(shown.contains("v1.0.0"));

        fs::write(source.join(MANIFEST_NAME), configurable_manifest("1.1.0")).unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "configurable-tool".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();
        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        assert_eq!(
            registry.plugins[0].settings["label"],
            PluginSettingValue::Text("preserved".into())
        );
    }

    #[test]
    fn declarative_timeout_and_crash_profiles_are_only_bounded_launch_requests() {
        let parsed: PluginManifest =
            toml::from_str(&manifest("1.0.0", "", "child-that-may-crash-or-hang")).unwrap();
        parsed.validate().unwrap();
        let mut plugin = DeclarativePlugin::new(parsed, None, permissions(&["process-spawn"]));
        let context = context();
        let started = Instant::now();
        for _ in 0..10_000 {
            let response = plugin.on_hook(
                &Hook::CommandInvoked {
                    command_id: "sample-tools.open-default".into(),
                },
                &context,
            );
            assert!(matches!(response, Some(PluginAction::OpenTab { .. })));
        }
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn every_checked_in_example_validates_installs_and_enables() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins");
        // Each example is a directory; the plugin reference sits beside them.
        let mut names = fs::read_dir(&examples)
            .unwrap()
            .map(Result::unwrap)
            .filter(|entry| entry.file_type().unwrap().is_dir())
            .map(|entry| entry.file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            [
                "command-notifier",
                "pane-layouts",
                "rerun-in-sibling",
                "task-runner",
                "window-switcher",
            ]
        );
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        for name in &names {
            let directory = examples.join(name);
            let (manifest, _, _) = PluginManifest::from_directory(&directory)
                .unwrap_or_else(|error| panic!("example {name} is invalid: {error:#}"));
            assert_eq!(&manifest.id, name);
            install(
                &directory,
                manifest.permissions.clone(),
                true,
                &registry_path,
                &root,
            )
            .unwrap_or_else(|error| panic!("example {name} did not install: {error:#}"));
        }
        let (loaded, statuses) = discover_runtime_plugins_at(&registry_path, &root).unwrap();
        assert_eq!(loaded.len(), names.len());
        assert!(statuses.iter().all(|status| status.diagnostic.is_none()));

        let task_runner = loaded
            .into_iter()
            .find(|plugin| plugin.id() == "task-runner")
            .unwrap();
        let mut task_runner = task_runner;
        let Some(PluginAction::OpenTab { env, .. }) = task_runner.on_hook(
            &Hook::CommandInvoked {
                command_id: "task-runner.open-tests".into(),
            },
            &context(),
        ) else {
            panic!("task-runner tests profile did not open a tab")
        };
        let settings: serde_json::Value =
            serde_json::from_str(&env[STRUCTURED_SETTINGS_ENV]).unwrap();
        assert_eq!(settings["values"]["test-command"], "cargo test");
        assert!(env.contains_key(STRUCTURED_CONTEXT_ENV));
    }
}
