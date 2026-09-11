//! Installed declarative third-party plugins.
//!
//! No public version executes third-party code in the Termeric process. A
//! validated profile may request an argv-preserving PTY launch; API v3 may also
//! request a bounded lifecycle-worker invocation after explicit grants, and
//! API v4 may subscribe to bounded semantic command completions, API v5 may
//! keep one correlated newline-delimited event handler alive per plugin, and
//! API v6 may request a bounded current-window workspace inventory. API v7
//! adds explicitly granted stable-ID tab selection and pane focus. API v8 adds
//! separately granted non-destructive tab-order and divider-ratio adjustment,
//! API v9 lets handlers contribute bounded, namespaced palette commands, and
//! API v10 adds bounded, typed, explicitly user-configured non-secret settings.
//! API v11 adds an independently granted, bounded presented-terminal query,
//! API v12 adds exact, separately granted ordinary-pane creation, and API v13
//! adds separately granted ordinary-tab creation in the originating window.
//! API v14 adds separately granted exact ordinary-pane closure.
//! API v15 adds separately granted exact ordinary-tab closure scheduling.
//! API v16 adds separately granted exact ordinary-pane movement, API v17 adds
//! bounded exact-pane terminal input, and API v18 adds exact same-tab pane
//! swapping, balancing, and layout presets. API v19 adds exact current-window
//! closure, available only from an explicitly invoked plugin command. API v20
//! adds a separately granted, bounded process-wide workspace inventory, and
//! API v21 adds exact non-activating cross-window workspace navigation.
//! API v22 adds bounded cross-window presented-terminal-text queries.
//! API v23 adds guarded exact cross-window terminal input.
//! API v24 adds exact cross-window ordinary-tab creation.
//! API v25 adds exact cross-window ordinary-pane creation.
//! API v26 adds exact cross-window ordinary-pane movement.
//! API v27 adds exact cross-window ordinary-tab movement.
//! API v28 adds exact cross-window ordinary-pane closure.
//! API v29 adds exact cross-window ordinary-tab closure.
//! API v30 adds exact cross-window ordinary-pane layouts.
//! API v31 adds exact process-owned window closure with final-window protection.

use super::hooks::{Hook, HookResponse, Plugin, PluginAction, PluginCommand, PluginContext};
use super::structured::{
    MAX_PERSISTENT_SOURCE_EVENT_BYTES, StructuredHandlerInvocation, StructuredHandlerMode,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const PLUGIN_API_VERSION: u32 = 31;
pub const MIN_PLUGIN_API_VERSION: u32 = 1;
pub const REGISTRY_SCHEMA_VERSION: u32 = 1;
pub const MANIFEST_NAME: &str = "termeric-plugin.toml";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_REGISTRY_BYTES: u64 = 512 * 1024;
const MAX_SOURCE_PATH_BYTES: usize = 16 * 1024;
const MAX_PROFILES: usize = 32;
const MAX_HANDLER_COMMANDS: usize = 32;
const MAX_SETTINGS: usize = 32;
pub(crate) const MAX_SETTING_TEXT_BYTES: usize = 512;
const MAX_SETTING_CHOICES: usize = 32;
const MAX_SETTING_VALUE_BYTES: usize = 8 * 1024;
const MAX_ARGS: usize = 64;
const MAX_ARG_BYTES: usize = 4096;
const MAX_TOTAL_ARG_BYTES: usize = 32 * 1024;
const MAX_STRUCTURED_CONTEXT_BYTES: usize = super::structured::MAX_STRUCTURED_EVENT_BYTES;
const MAX_STRUCTURED_COMMAND_JSON_BYTES: usize = 2 * 1024;
const MAX_STRUCTURED_OUTPUT_JSON_BYTES: usize = 8 * 1024;
const STRUCTURED_CONTEXT_SCHEMA_VERSION: u32 = 1;
const STRUCTURED_CONTEXT_ENV: &str = "TERMERIC_PLUGIN_CONTEXT_JSON";
const STRUCTURED_SETTINGS_ENV: &str = "TERMERIC_PLUGIN_SETTINGS_JSON";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginPermission {
    ProcessSpawn,
    WorkspaceQuery,
    WorkspaceActions,
    WorkspaceTopology,
    EventSubscribe,
    UiActions,
    FilesystemRead,
    FilesystemWrite,
    Network,
    Clipboard,
    TerminalText,
    TerminalQuery,
    PaneCreate,
    TabCreate,
    PaneClose,
    TabClose,
    PaneMove,
    TerminalInput,
    PaneLayout,
    WindowClose,
    ProcessQuery,
    ProcessActions,
    ProcessTerminalText,
    ProcessTerminalInput,
    ProcessTabCreate,
    ProcessPaneCreate,
    ProcessPaneMove,
    ProcessTabMove,
    ProcessPaneClose,
    ProcessTabClose,
    ProcessPaneLayout,
    ProcessWindowClose,
    Secrets,
}

impl std::fmt::Display for PluginPermission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ProcessSpawn => "process-spawn",
            Self::WorkspaceQuery => "workspace-query",
            Self::WorkspaceActions => "workspace-actions",
            Self::WorkspaceTopology => "workspace-topology",
            Self::EventSubscribe => "event-subscribe",
            Self::UiActions => "ui-actions",
            Self::FilesystemRead => "filesystem-read",
            Self::FilesystemWrite => "filesystem-write",
            Self::Network => "network",
            Self::Clipboard => "clipboard",
            Self::TerminalText => "terminal-text",
            Self::TerminalQuery => "terminal-query",
            Self::PaneCreate => "pane-create",
            Self::TabCreate => "tab-create",
            Self::PaneClose => "pane-close",
            Self::TabClose => "tab-close",
            Self::PaneMove => "pane-move",
            Self::TerminalInput => "terminal-input",
            Self::PaneLayout => "pane-layout",
            Self::WindowClose => "window-close",
            Self::ProcessQuery => "process-query",
            Self::ProcessActions => "process-actions",
            Self::ProcessTerminalText => "process-terminal-text",
            Self::ProcessTerminalInput => "process-terminal-input",
            Self::ProcessTabCreate => "process-tab-create",
            Self::ProcessPaneCreate => "process-pane-create",
            Self::ProcessPaneMove => "process-pane-move",
            Self::ProcessTabMove => "process-tab-move",
            Self::ProcessPaneClose => "process-pane-close",
            Self::ProcessTabClose => "process-tab-close",
            Self::ProcessPaneLayout => "process-pane-layout",
            Self::ProcessWindowClose => "process-window-close",
            Self::Secrets => "secrets",
        })
    }
}

impl std::str::FromStr for PluginPermission {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "process-spawn" => Ok(Self::ProcessSpawn),
            "workspace-query" => Ok(Self::WorkspaceQuery),
            "workspace-actions" => Ok(Self::WorkspaceActions),
            "workspace-topology" => Ok(Self::WorkspaceTopology),
            "event-subscribe" => Ok(Self::EventSubscribe),
            "ui-actions" => Ok(Self::UiActions),
            "filesystem-read" => Ok(Self::FilesystemRead),
            "filesystem-write" => Ok(Self::FilesystemWrite),
            "network" => Ok(Self::Network),
            "clipboard" => Ok(Self::Clipboard),
            "terminal-text" => Ok(Self::TerminalText),
            "terminal-query" => Ok(Self::TerminalQuery),
            "pane-create" => Ok(Self::PaneCreate),
            "tab-create" => Ok(Self::TabCreate),
            "pane-close" => Ok(Self::PaneClose),
            "tab-close" => Ok(Self::TabClose),
            "pane-move" => Ok(Self::PaneMove),
            "terminal-input" => Ok(Self::TerminalInput),
            "pane-layout" => Ok(Self::PaneLayout),
            "window-close" => Ok(Self::WindowClose),
            "process-query" => Ok(Self::ProcessQuery),
            "process-actions" => Ok(Self::ProcessActions),
            "process-terminal-text" => Ok(Self::ProcessTerminalText),
            "process-terminal-input" => Ok(Self::ProcessTerminalInput),
            "process-tab-create" => Ok(Self::ProcessTabCreate),
            "process-pane-create" => Ok(Self::ProcessPaneCreate),
            "process-pane-move" => Ok(Self::ProcessPaneMove),
            "process-tab-move" => Ok(Self::ProcessTabMove),
            "process-pane-close" => Ok(Self::ProcessPaneClose),
            "process-tab-close" => Ok(Self::ProcessTabClose),
            "process-pane-layout" => Ok(Self::ProcessPaneLayout),
            "process-window-close" => Ok(Self::ProcessWindowClose),
            "secrets" => Ok(Self::Secrets),
            _ => bail!("unknown plugin permission {value:?}"),
        }
    }
}

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
    pub permissions: BTreeSet<PluginPermission>,
    #[serde(default)]
    pub profiles: Vec<DeclarativeProfile>,
    #[serde(default)]
    pub commands: Vec<DeclarativeHandlerCommand>,
    #[serde(default)]
    pub settings: Vec<DeclarativeSetting>,
    #[serde(default)]
    pub event_handler: Option<DeclarativeEventHandler>,
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
    #[serde(default)]
    pub mode: DeclarativeHandlerMode,
    pub events: BTreeSet<StructuredEventKind>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeclarativeHandlerMode {
    #[default]
    ProcessPerEvent,
    Persistent,
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
        let metadata =
            fs::symlink_metadata(&path).with_context(|| format!("missing {}", path.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            bail!("plugin manifest must be a regular, non-symlink file");
        }
        if metadata.len() > MAX_MANIFEST_BYTES {
            bail!("plugin manifest exceeds {MAX_MANIFEST_BYTES} bytes");
        }
        let bytes = read_bounded(&path, MAX_MANIFEST_BYTES, "plugin manifest")?;
        let text = std::str::from_utf8(&bytes).context("plugin manifest must be UTF-8")?;
        let manifest: Self = toml::from_str(text).context("invalid plugin manifest TOML")?;
        manifest.validate()?;
        Ok((manifest, directory, bytes))
    }

    pub fn validate(&self) -> Result<()> {
        if !(MIN_PLUGIN_API_VERSION..=PLUGIN_API_VERSION).contains(&self.api_version) {
            bail!(
                "plugin API version {} is unsupported; Termeric supports {} through {}",
                self.api_version,
                MIN_PLUGIN_API_VERSION,
                PLUGIN_API_VERSION
            );
        }
        validate_identifier("plugin id", &self.id)?;
        if matches!(self.id.as_str(), "claude-code" | "codex") {
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
        if !self.permissions.contains(&PluginPermission::ProcessSpawn) {
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
        let mut setting_ids = BTreeSet::new();
        for setting in &self.settings {
            if self.api_version < 10 {
                bail!("plugin settings require declarative plugin API v10");
            }
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
        if !self.commands.is_empty() {
            if self.api_version < 9 {
                bail!("handler commands require declarative plugin API v9");
            }
            if !self.permissions.contains(&PluginPermission::EventSubscribe)
                || self.event_handler.as_ref().is_none_or(|handler| {
                    !handler
                        .events
                        .contains(&StructuredEventKind::CommandInvoked)
                })
            {
                bail!("handler commands require an event handler subscribed to command-invoked");
            }
        }
        if let Some(handler) = &self.event_handler {
            if self.api_version < 3 {
                bail!("event handlers require declarative plugin API v3");
            }
            if !self.permissions.contains(&PluginPermission::EventSubscribe) {
                bail!("an event handler must request event-subscribe");
            }
            if handler.events.is_empty() {
                bail!("an event handler must subscribe to at least one event");
            }
            validate_argv("event handler", &handler.program, &handler.args)?;
            if handler.mode == DeclarativeHandlerMode::Persistent && self.api_version < 5 {
                bail!("persistent event handlers require declarative plugin API v5");
            }
            if handler
                .events
                .contains(&StructuredEventKind::CommandCompleted)
            {
                if self.api_version < 4 {
                    bail!("command-completed events require declarative plugin API v4");
                }
                if !self.permissions.contains(&PluginPermission::TerminalText) {
                    bail!("command-completed events must request terminal-text");
                }
            }
            if handler
                .events
                .contains(&StructuredEventKind::CommandInvoked)
            {
                if self.api_version < 9 {
                    bail!("command-invoked events require declarative plugin API v9");
                }
                if self.commands.is_empty() {
                    bail!("command-invoked events require at least one handler command");
                }
            }
        }
        if self.permissions.contains(&PluginPermission::EventSubscribe)
            && self.event_handler.is_none()
        {
            bail!("event-subscribe requires an event handler");
        }
        if self.permissions.contains(&PluginPermission::UiActions)
            && (!self.permissions.contains(&PluginPermission::EventSubscribe)
                || self.event_handler.is_none())
        {
            bail!("ui-actions requires an event handler and event-subscribe");
        }
        if self
            .permissions
            .contains(&PluginPermission::WorkspaceActions)
            && (self.api_version < 7
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("workspace-actions requires an API v7 persistent event handler");
        }
        if self
            .permissions
            .contains(&PluginPermission::WorkspaceTopology)
            && (self.api_version < 8
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("workspace-topology requires an API v8 persistent event handler");
        }
        if self.permissions.contains(&PluginPermission::TerminalQuery)
            && (self.api_version < 11
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("terminal-query requires an API v11 persistent event handler");
        }
        if self.permissions.contains(&PluginPermission::PaneCreate)
            && (self.api_version < 12
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("pane-create requires an API v12 persistent event handler");
        }
        if self.permissions.contains(&PluginPermission::TabCreate)
            && (self.api_version < 13
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("tab-create requires an API v13 persistent event handler");
        }
        if self.permissions.contains(&PluginPermission::PaneClose)
            && (self.api_version < 14
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("pane-close requires an API v14 persistent event handler");
        }
        if self.permissions.contains(&PluginPermission::TabClose)
            && (self.api_version < 15
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("tab-close requires an API v15 persistent event handler");
        }
        if self.permissions.contains(&PluginPermission::PaneMove)
            && (self.api_version < 16
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("pane-move requires an API v16 persistent event handler");
        }
        if self.permissions.contains(&PluginPermission::TerminalInput)
            && (self.api_version < 17
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("terminal-input requires an API v17 persistent event handler");
        }
        if self.permissions.contains(&PluginPermission::PaneLayout)
            && (self.api_version < 18
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("pane-layout requires an API v18 persistent event handler");
        }
        if self.permissions.contains(&PluginPermission::WindowClose)
            && (self.api_version < 19
                || self.commands.is_empty()
                || self.event_handler.as_ref().is_none_or(|handler| {
                    handler.mode != DeclarativeHandlerMode::Persistent
                        || !handler
                            .events
                            .contains(&StructuredEventKind::CommandInvoked)
                }))
        {
            bail!("window-close requires an API v19 persistent command-invoked event handler");
        }
        if self.permissions.contains(&PluginPermission::ProcessQuery)
            && (self.api_version < 20
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("process-query requires an API v20 persistent event handler");
        }
        if self.permissions.contains(&PluginPermission::ProcessActions)
            && (self.api_version < 21
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("process-actions requires an API v21 persistent event handler");
        }
        if self
            .permissions
            .contains(&PluginPermission::ProcessTerminalText)
            && (self.api_version < 22
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("process-terminal-text requires an API v22 persistent event handler");
        }
        if self
            .permissions
            .contains(&PluginPermission::ProcessTerminalInput)
            && (self.api_version < 23
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("process-terminal-input requires an API v23 persistent event handler");
        }
        if self
            .permissions
            .contains(&PluginPermission::ProcessTabCreate)
            && (self.api_version < 24
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("process-tab-create requires an API v24 persistent event handler");
        }
        if self
            .permissions
            .contains(&PluginPermission::ProcessPaneCreate)
            && (self.api_version < 25
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("process-pane-create requires an API v25 persistent event handler");
        }
        if self
            .permissions
            .contains(&PluginPermission::ProcessPaneMove)
            && (self.api_version < 26
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("process-pane-move requires an API v26 persistent event handler");
        }
        if self.permissions.contains(&PluginPermission::ProcessTabMove)
            && (self.api_version < 27
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("process-tab-move requires an API v27 persistent event handler");
        }
        if self
            .permissions
            .contains(&PluginPermission::ProcessPaneClose)
            && (self.api_version < 28
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("process-pane-close requires an API v28 persistent event handler");
        }
        if self
            .permissions
            .contains(&PluginPermission::ProcessTabClose)
            && (self.api_version < 29
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("process-tab-close requires an API v29 persistent event handler");
        }
        if self
            .permissions
            .contains(&PluginPermission::ProcessPaneLayout)
            && (self.api_version < 30
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("process-pane-layout requires an API v30 persistent event handler");
        }
        if self
            .permissions
            .contains(&PluginPermission::ProcessWindowClose)
            && (self.api_version < 31
                || self
                    .event_handler
                    .as_ref()
                    .is_none_or(|handler| handler.mode != DeclarativeHandlerMode::Persistent))
        {
            bail!("process-window-close requires an API v31 persistent event handler");
        }
        if self.permissions.contains(&PluginPermission::TerminalText)
            && self.api_version >= 4
            && self.event_handler.as_ref().is_none_or(|handler| {
                !handler
                    .events
                    .contains(&StructuredEventKind::CommandCompleted)
            })
        {
            bail!("terminal-text requires an API v4 command-completed event handler");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledPlugin {
    pub id: String,
    pub enabled: bool,
    pub source: PathBuf,
    pub source_fingerprint: String,
    #[serde(default)]
    pub grants: BTreeSet<PluginPermission>,
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
    pub fn load() -> Result<Self> {
        Self::load_from(&registry_path())
    }

    fn load_from(path: &Path) -> Result<Self> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                bail!("plugin registry must be a regular, non-symlink file")
            }
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error.into()),
        };
        if metadata.len() > MAX_REGISTRY_BYTES {
            bail!("plugin registry exceeds {MAX_REGISTRY_BYTES} bytes");
        }
        let bytes = read_bounded(path, MAX_REGISTRY_BYTES, "plugin registry")?;
        let text = std::str::from_utf8(&bytes).context("plugin registry must be UTF-8")?;
        let registry: Self = toml::from_str(text).context("invalid plugin registry")?;
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
            if matches!(plugin.id.as_str(), "claude-code" | "codex") {
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
            if plugin.grants.iter().any(|permission| {
                !matches!(
                    permission,
                    PluginPermission::ProcessSpawn
                        | PluginPermission::WorkspaceQuery
                        | PluginPermission::WorkspaceActions
                        | PluginPermission::WorkspaceTopology
                        | PluginPermission::EventSubscribe
                        | PluginPermission::UiActions
                        | PluginPermission::TerminalText
                        | PluginPermission::TerminalQuery
                        | PluginPermission::PaneCreate
                        | PluginPermission::TabCreate
                        | PluginPermission::PaneClose
                        | PluginPermission::TabClose
                        | PluginPermission::PaneMove
                        | PluginPermission::TerminalInput
                        | PluginPermission::PaneLayout
                        | PluginPermission::WindowClose
                        | PluginPermission::ProcessQuery
                        | PluginPermission::ProcessActions
                        | PluginPermission::ProcessTerminalText
                        | PluginPermission::ProcessTerminalInput
                        | PluginPermission::ProcessTabCreate
                        | PluginPermission::ProcessPaneCreate
                        | PluginPermission::ProcessPaneMove
                        | PluginPermission::ProcessTabMove
                        | PluginPermission::ProcessPaneClose
                        | PluginPermission::ProcessTabClose
                        | PluginPermission::ProcessPaneLayout
                        | PluginPermission::ProcessWindowClose
                )
            }) {
                bail!(
                    "installed plugin {:?} contains an unsupported grant",
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
        let temporary = path.with_extension("toml.tmp");
        write_new_temporary(&temporary, toml::to_string_pretty(self)?.as_bytes())?;
        fs::rename(&temporary, path)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RuntimePluginStatus {
    pub id: String,
    pub name: String,
    pub version: String,
    pub enabled: bool,
    pub source: PathBuf,
    pub requested: BTreeSet<PluginPermission>,
    pub granted: BTreeSet<PluginPermission>,
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

pub fn discover_runtime_plugins() -> Result<(Vec<DeclarativePlugin>, Vec<RuntimePluginStatus>)> {
    let registry = PluginRegistry::load().context("third-party plugin registry rejected")?;
    Ok(discover_from_registry(&registry, &plugins_dir()))
}

#[cfg(test)]
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
                        .map_or_else(BTreeSet::new, |manifest| manifest.permissions),
                    granted: installed.grants.clone(),
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
    grants: BTreeSet<PluginPermission>,
    settings: BTreeMap<String, PluginSettingValue>,
}

impl DeclarativePlugin {
    #[cfg(test)]
    fn new(
        manifest: PluginManifest,
        default_profile: Option<String>,
        grants: BTreeSet<PluginPermission>,
    ) -> Self {
        Self::new_with_settings(manifest, default_profile, grants, BTreeMap::new())
    }

    fn new_with_settings(
        manifest: PluginManifest,
        default_profile: Option<String>,
        grants: BTreeSet<PluginPermission>,
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
        if self.manifest.api_version >= 10 && !self.manifest.settings.is_empty() {
            let value = encode_settings_document(&self.manifest, &self.settings)?;
            environment.insert(STRUCTURED_SETTINGS_ENV.into(), value);
        }
        if self.manifest.api_version < 2 || !self.grants.contains(&PluginPermission::WorkspaceQuery)
        {
            return Ok(environment);
        }
        #[derive(Serialize)]
        struct TerminalSize {
            columns: usize,
            rows: usize,
        }
        #[derive(Serialize)]
        struct LaunchContext<'a> {
            schema_version: u32,
            plugin_id: &'a str,
            profile_id: &'a str,
            active_working_directory: Option<&'a str>,
            tab_count: usize,
            active_tab_index: usize,
            terminal: TerminalSize,
        }
        let value = serde_json::to_string(&LaunchContext {
            schema_version: STRUCTURED_CONTEXT_SCHEMA_VERSION,
            plugin_id: &self.manifest.id,
            profile_id,
            active_working_directory: context.cwd.as_deref(),
            tab_count: context.tab_count,
            active_tab_index: context.active_tab,
            terminal: TerminalSize {
                columns: context.terminal_size.0,
                rows: context.terminal_size.1,
            },
        })?;
        if value.len() > MAX_STRUCTURED_CONTEXT_BYTES {
            bail!("structured plugin launch context exceeds {MAX_STRUCTURED_CONTEXT_BYTES} bytes");
        }
        environment.insert(STRUCTURED_CONTEXT_ENV.into(), value);
        Ok(environment)
    }

    fn action_for(&self, profile_id: &str, context: &PluginContext) -> HookResponse {
        let Some(profile) = self
            .manifest
            .profiles
            .iter()
            .find(|profile| profile.id == profile_id)
        else {
            return HookResponse::Action(PluginAction::ShowNotice {
                title: format!("{} profile unavailable", self.manifest.name),
                lines: vec![format!("No profile named {profile_id:?} is installed.")],
            });
        };
        let env = match self.structured_context_environment(profile_id, context) {
            Ok(env) => env,
            Err(error) => {
                return HookResponse::Action(PluginAction::ShowNotice {
                    title: format!("{} context unavailable", self.manifest.name),
                    lines: vec![format!("{error:#}")],
                });
            }
        };
        HookResponse::Action(PluginAction::OpenTab {
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
        })
    }

    fn event_action(&self, hook: &Hook, context: &PluginContext) -> Option<HookResponse> {
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
        if !handler.events.contains(&kind)
            || !self.grants.contains(&PluginPermission::EventSubscribe)
        {
            return None;
        }
        if kind == StructuredEventKind::CommandCompleted
            && !self.grants.contains(&PluginPermission::TerminalText)
        {
            return None;
        }
        let context_value = self
            .grants
            .contains(&PluginPermission::WorkspaceQuery)
            .then(|| {
                serde_json::json!({
                    "active_working_directory": context.cwd,
                    "tab_count": context.tab_count,
                    "active_tab_index": context.active_tab,
                    "terminal": {
                        "columns": context.terminal_size.0,
                        "rows": context.terminal_size.1,
                    },
                })
            });
        let mut envelope = serde_json::json!({
            "schema_version": 1,
            "plugin_id": self.manifest.id,
            "event": event,
        });
        if self.manifest.api_version >= 10 && !self.manifest.settings.is_empty() {
            envelope["settings"] = serde_json::to_value(&self.settings)
                .expect("validated plugin settings serialize to JSON");
        }
        if let Some(value) = context_value {
            envelope["context"] = value;
        }
        let max_event_bytes = if handler.mode == DeclarativeHandlerMode::Persistent {
            MAX_PERSISTENT_SOURCE_EVENT_BYTES
        } else {
            MAX_STRUCTURED_CONTEXT_BYTES
        };
        let event_json = match if kind == StructuredEventKind::CommandCompleted {
            encode_command_event(envelope, max_event_bytes)
        } else {
            serde_json::to_string(&envelope).map_err(anyhow::Error::from)
        } {
            Ok(value) if value.len() <= max_event_bytes => value,
            Ok(_) => {
                return Some(HookResponse::Action(PluginAction::ShowNotice {
                    title: format!("{} event unavailable", self.manifest.name),
                    lines: vec![format!(
                        "Structured plugin event exceeds {MAX_STRUCTURED_CONTEXT_BYTES} bytes."
                    )],
                }));
            }
            Err(error) => {
                return Some(HookResponse::Action(PluginAction::ShowNotice {
                    title: format!("{} event unavailable", self.manifest.name),
                    lines: vec![format!("Could not encode structured plugin event: {error}")],
                }));
            }
        };
        Some(HookResponse::Action(
            PluginAction::InvokeStructuredHandler {
                invocation: StructuredHandlerInvocation {
                    plugin_id: self.manifest.id.clone(),
                    program: handler.program.clone(),
                    args: handler.args.clone(),
                    event_json,
                    allow_ui_actions: self.grants.contains(&PluginPermission::UiActions),
                    allow_workspace_query: self.manifest.api_version >= 6
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::WorkspaceQuery),
                    allow_workspace_actions: self.manifest.api_version >= 7
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::WorkspaceActions),
                    allow_workspace_topology: self.manifest.api_version >= 8
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::WorkspaceTopology),
                    allow_terminal_query: self.manifest.api_version >= 11
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::TerminalQuery),
                    allow_pane_create: self.manifest.api_version >= 12
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::PaneCreate),
                    allow_tab_create: self.manifest.api_version >= 13
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::TabCreate),
                    allow_pane_close: self.manifest.api_version >= 14
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::PaneClose),
                    allow_tab_close: self.manifest.api_version >= 15
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::TabClose),
                    allow_pane_move: self.manifest.api_version >= 16
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::PaneMove),
                    allow_terminal_input: self.manifest.api_version >= 17
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::TerminalInput),
                    allow_pane_layout: self.manifest.api_version >= 18
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::PaneLayout),
                    allow_window_close: self.manifest.api_version >= 19
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && kind == StructuredEventKind::CommandInvoked
                        && self.grants.contains(&PluginPermission::WindowClose),
                    allow_process_query: self.manifest.api_version >= 20
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::ProcessQuery),
                    allow_process_actions: self.manifest.api_version >= 21
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::ProcessActions),
                    allow_process_terminal_text: self.manifest.api_version >= 22
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::ProcessTerminalText),
                    allow_process_terminal_input: self.manifest.api_version >= 23
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self
                            .grants
                            .contains(&PluginPermission::ProcessTerminalInput),
                    allow_process_tab_create: self.manifest.api_version >= 24
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::ProcessTabCreate),
                    allow_process_pane_create: self.manifest.api_version >= 25
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::ProcessPaneCreate),
                    allow_process_pane_move: self.manifest.api_version >= 26
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::ProcessPaneMove),
                    allow_process_tab_move: self.manifest.api_version >= 27
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::ProcessTabMove),
                    allow_process_pane_close: self.manifest.api_version >= 28
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::ProcessPaneClose),
                    allow_process_tab_close: self.manifest.api_version >= 29
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::ProcessTabClose),
                    allow_process_pane_layout: self.manifest.api_version >= 30
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::ProcessPaneLayout),
                    allow_process_window_close: self.manifest.api_version >= 31
                        && handler.mode == DeclarativeHandlerMode::Persistent
                        && self.grants.contains(&PluginPermission::ProcessWindowClose),
                    mode: match handler.mode {
                        DeclarativeHandlerMode::ProcessPerEvent => {
                            StructuredHandlerMode::ProcessPerEvent
                        }
                        DeclarativeHandlerMode::Persistent => StructuredHandlerMode::Persistent,
                    },
                },
            },
        ))
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

    fn on_hook(&mut self, hook: &Hook, context: &PluginContext) -> HookResponse {
        let response = match hook {
            Hook::CommandInvoked { command_id } => self
                .manifest
                .profiles
                .iter()
                .find(|profile| command_id == &format!("{}.open-{}", self.manifest.id, profile.id))
                .map_or(HookResponse::Ignored, |profile| {
                    self.action_for(&profile.id, context)
                }),
            Hook::WorkspaceRestore {
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
                    return HookResponse::Action(PluginAction::ShowNotice {
                        title: format!("{} profile unavailable", self.manifest.name),
                        lines: vec!["This event-only plugin has no launch profiles.".into()],
                    });
                };
                let mut restore_context = context.clone();
                restore_context.cwd.clone_from(cwd);
                self.action_for(selected, &restore_context)
            }
            _ => HookResponse::Ignored,
        };
        if matches!(response, HookResponse::Ignored) {
            self.event_action(hook, context)
                .unwrap_or(HookResponse::Ignored)
        } else {
            response
        }
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
        grants: BTreeSet<PluginPermission>,
        enable: bool,
        replace: bool,
    },
    Update {
        id: String,
    },
    Enable {
        id: String,
    },
    Disable {
        id: String,
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
    Set { key: String, value: String },
    Unset { key: String },
    ResetSettings,
}

pub fn execute_admin(command: PluginAdminCommand) -> Result<String> {
    execute_admin_at(command, &registry_path(), &plugins_dir())
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
                "valid plugin {} v{} api={} source={} fingerprint={} permissions={}",
                manifest.id,
                manifest.version,
                manifest.api_version,
                source.display(),
                fingerprint(&bytes),
                permission_list(manifest.permissions.iter().copied())
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
            let added_permissions: Vec<_> = manifest
                .permissions
                .difference(&previous_manifest.permissions)
                .copied()
                .collect();
            if !added_permissions.is_empty() {
                bail!(
                    "update requests new permissions ({}); review and reinstall with explicit grants",
                    permission_list(added_permissions)
                );
            }
            install_manifest(
                &mut registry,
                registry_path,
                root,
                manifest,
                source,
                bytes,
                installed.grants,
                installed.enabled,
                true,
            )
        }
        PluginAdminCommand::Enable { id } => {
            set_enabled(&mut registry, registry_path, root, &id, true)
        }
        PluginAdminCommand::Disable { id } => {
            set_enabled(&mut registry, registry_path, root, &id, false)
        }
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

#[allow(clippy::too_many_arguments)]
fn install_manifest(
    registry: &mut PluginRegistry,
    registry_path: &Path,
    root: &Path,
    manifest: PluginManifest,
    source: PathBuf,
    bytes: Vec<u8>,
    grants: BTreeSet<PluginPermission>,
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
    let unrequested: Vec<_> = grants.difference(&manifest.permissions).copied().collect();
    if !unrequested.is_empty() {
        bail!(
            "cannot grant unrequested permissions: {}",
            permission_list(unrequested.iter().copied())
        );
    }
    let unsupported: Vec<_> = grants
        .iter()
        .copied()
        .filter(|permission| !permission_supported(manifest.api_version, *permission))
        .collect();
    if !unsupported.is_empty() {
        bail!(
            "declarative API v{} cannot grant: {}",
            manifest.api_version,
            permission_list(unsupported.iter().copied())
        );
    }
    let denied: Vec<_> = manifest.permissions.difference(&grants).copied().collect();
    if enable && !denied.is_empty() {
        bail!(
            "cannot enable until permissions are granted: {}",
            permission_list(denied.iter().copied())
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
    let temporary = destination.join(format!("{MANIFEST_NAME}.tmp"));
    let backup = destination.join(format!("{MANIFEST_NAME}.previous"));
    write_new_temporary(&temporary, &bytes)?;
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
    if let Err(error) = fs::rename(&temporary, &destination_manifest) {
        if had_previous {
            let _ = fs::rename(&backup, &destination_manifest);
        }
        return Err(error.into());
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
    Ok(format!(
        "installed {} v{} enabled={} source={} permissions={} grants={}",
        manifest.id,
        manifest.version,
        enable,
        source.display(),
        permission_list(manifest.permissions.iter().copied()),
        permission_list(
            registry
                .plugins
                .iter()
                .find(|plugin| plugin.id == manifest.id)
                .into_iter()
                .flat_map(|plugin| plugin.grants.iter().copied())
        )
    ))
}

fn write_new_temporary(path: &Path, bytes: &[u8]) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            bail!(
                "refusing to replace non-regular temporary file {}",
                path.display()
            )
        }
        Ok(_) => fs::remove_file(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("failed to create temporary file {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn read_bounded(path: &Path, max_bytes: u64, label: &str) -> Result<Vec<u8>> {
    let file =
        fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut bytes = Vec::with_capacity(
        file.metadata()
            .map(|metadata| metadata.len().min(max_bytes) as usize)
            .unwrap_or(0),
    );
    file.take(max_bytes + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        bail!("{label} exceeds {max_bytes} bytes");
    }
    Ok(bytes)
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

fn set_enabled(
    registry: &mut PluginRegistry,
    registry_path: &Path,
    root: &Path,
    id: &str,
    enabled: bool,
) -> Result<String> {
    validate_identifier("plugin id", id)?;
    let installed = registry
        .plugins
        .iter_mut()
        .find(|plugin| plugin.id == id)
        .ok_or_else(|| anyhow::anyhow!("plugin {id:?} is not installed"))?;
    if enabled {
        let manifest = load_installed(installed, root)?;
        validate_grants(&manifest, &installed.grants)?;
    }
    installed.enabled = enabled;
    registry.save_to(registry_path)?;
    Ok(format!(
        "{} {id}",
        if enabled { "enabled" } else { "disabled" }
    ))
}

fn validate_grants(manifest: &PluginManifest, grants: &BTreeSet<PluginPermission>) -> Result<()> {
    let denied: Vec<_> = manifest.permissions.difference(grants).copied().collect();
    if !denied.is_empty() {
        bail!(
            "permissions denied: {}",
            permission_list(denied.iter().copied())
        );
    }
    let unsupported: Vec<_> = grants
        .iter()
        .copied()
        .filter(|permission| !permission_supported(manifest.api_version, *permission))
        .collect();
    if !unsupported.is_empty() {
        bail!(
            "declarative API v{} cannot grant: {}",
            manifest.api_version,
            permission_list(unsupported.iter().copied())
        );
    }
    Ok(())
}

fn permission_supported(api_version: u32, permission: PluginPermission) -> bool {
    match api_version {
        1 => permission == PluginPermission::ProcessSpawn,
        2 => matches!(
            permission,
            PluginPermission::ProcessSpawn | PluginPermission::WorkspaceQuery
        ),
        3 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
        ),
        4 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
        ),
        5 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
        ),
        6 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
        ),
        7 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
        ),
        8..=10 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
        ),
        11 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
        ),
        12 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
        ),
        13 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
                | PluginPermission::TabCreate
        ),
        14 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
                | PluginPermission::TabCreate
                | PluginPermission::PaneClose
        ),
        15 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
                | PluginPermission::TabCreate
                | PluginPermission::PaneClose
                | PluginPermission::TabClose
        ),
        16 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
                | PluginPermission::TabCreate
                | PluginPermission::PaneClose
                | PluginPermission::TabClose
                | PluginPermission::PaneMove
        ),
        17 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
                | PluginPermission::TabCreate
                | PluginPermission::PaneClose
                | PluginPermission::TabClose
                | PluginPermission::PaneMove
                | PluginPermission::TerminalInput
        ),
        18 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
                | PluginPermission::TabCreate
                | PluginPermission::PaneClose
                | PluginPermission::TabClose
                | PluginPermission::PaneMove
                | PluginPermission::TerminalInput
                | PluginPermission::PaneLayout
        ),
        19 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
                | PluginPermission::TabCreate
                | PluginPermission::PaneClose
                | PluginPermission::TabClose
                | PluginPermission::PaneMove
                | PluginPermission::TerminalInput
                | PluginPermission::PaneLayout
                | PluginPermission::WindowClose
        ),
        20 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
                | PluginPermission::TabCreate
                | PluginPermission::PaneClose
                | PluginPermission::TabClose
                | PluginPermission::PaneMove
                | PluginPermission::TerminalInput
                | PluginPermission::PaneLayout
                | PluginPermission::WindowClose
                | PluginPermission::ProcessQuery
        ),
        21 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
                | PluginPermission::TabCreate
                | PluginPermission::PaneClose
                | PluginPermission::TabClose
                | PluginPermission::PaneMove
                | PluginPermission::TerminalInput
                | PluginPermission::PaneLayout
                | PluginPermission::WindowClose
                | PluginPermission::ProcessQuery
                | PluginPermission::ProcessActions
        ),
        22 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
                | PluginPermission::TabCreate
                | PluginPermission::PaneClose
                | PluginPermission::TabClose
                | PluginPermission::PaneMove
                | PluginPermission::TerminalInput
                | PluginPermission::PaneLayout
                | PluginPermission::WindowClose
                | PluginPermission::ProcessQuery
                | PluginPermission::ProcessActions
                | PluginPermission::ProcessTerminalText
        ),
        23 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
                | PluginPermission::TabCreate
                | PluginPermission::PaneClose
                | PluginPermission::TabClose
                | PluginPermission::PaneMove
                | PluginPermission::TerminalInput
                | PluginPermission::PaneLayout
                | PluginPermission::WindowClose
                | PluginPermission::ProcessQuery
                | PluginPermission::ProcessActions
                | PluginPermission::ProcessTerminalText
                | PluginPermission::ProcessTerminalInput
        ),
        24 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
                | PluginPermission::TabCreate
                | PluginPermission::PaneClose
                | PluginPermission::TabClose
                | PluginPermission::PaneMove
                | PluginPermission::TerminalInput
                | PluginPermission::PaneLayout
                | PluginPermission::WindowClose
                | PluginPermission::ProcessQuery
                | PluginPermission::ProcessActions
                | PluginPermission::ProcessTerminalText
                | PluginPermission::ProcessTerminalInput
                | PluginPermission::ProcessTabCreate
        ),
        25 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
                | PluginPermission::TabCreate
                | PluginPermission::PaneClose
                | PluginPermission::TabClose
                | PluginPermission::PaneMove
                | PluginPermission::TerminalInput
                | PluginPermission::PaneLayout
                | PluginPermission::WindowClose
                | PluginPermission::ProcessQuery
                | PluginPermission::ProcessActions
                | PluginPermission::ProcessTerminalText
                | PluginPermission::ProcessTerminalInput
                | PluginPermission::ProcessTabCreate
                | PluginPermission::ProcessPaneCreate
        ),
        26 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
                | PluginPermission::TabCreate
                | PluginPermission::PaneClose
                | PluginPermission::TabClose
                | PluginPermission::PaneMove
                | PluginPermission::TerminalInput
                | PluginPermission::PaneLayout
                | PluginPermission::WindowClose
                | PluginPermission::ProcessQuery
                | PluginPermission::ProcessActions
                | PluginPermission::ProcessTerminalText
                | PluginPermission::ProcessTerminalInput
                | PluginPermission::ProcessTabCreate
                | PluginPermission::ProcessPaneCreate
                | PluginPermission::ProcessPaneMove
        ),
        27 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
                | PluginPermission::TabCreate
                | PluginPermission::PaneClose
                | PluginPermission::TabClose
                | PluginPermission::PaneMove
                | PluginPermission::TerminalInput
                | PluginPermission::PaneLayout
                | PluginPermission::WindowClose
                | PluginPermission::ProcessQuery
                | PluginPermission::ProcessActions
                | PluginPermission::ProcessTerminalText
                | PluginPermission::ProcessTerminalInput
                | PluginPermission::ProcessTabCreate
                | PluginPermission::ProcessPaneCreate
                | PluginPermission::ProcessPaneMove
                | PluginPermission::ProcessTabMove
        ),
        28 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
                | PluginPermission::TabCreate
                | PluginPermission::PaneClose
                | PluginPermission::TabClose
                | PluginPermission::PaneMove
                | PluginPermission::TerminalInput
                | PluginPermission::PaneLayout
                | PluginPermission::WindowClose
                | PluginPermission::ProcessQuery
                | PluginPermission::ProcessActions
                | PluginPermission::ProcessTerminalText
                | PluginPermission::ProcessTerminalInput
                | PluginPermission::ProcessTabCreate
                | PluginPermission::ProcessPaneCreate
                | PluginPermission::ProcessPaneMove
                | PluginPermission::ProcessTabMove
                | PluginPermission::ProcessPaneClose
        ),
        29 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
                | PluginPermission::TabCreate
                | PluginPermission::PaneClose
                | PluginPermission::TabClose
                | PluginPermission::PaneMove
                | PluginPermission::TerminalInput
                | PluginPermission::PaneLayout
                | PluginPermission::WindowClose
                | PluginPermission::ProcessQuery
                | PluginPermission::ProcessActions
                | PluginPermission::ProcessTerminalText
                | PluginPermission::ProcessTerminalInput
                | PluginPermission::ProcessTabCreate
                | PluginPermission::ProcessPaneCreate
                | PluginPermission::ProcessPaneMove
                | PluginPermission::ProcessTabMove
                | PluginPermission::ProcessPaneClose
                | PluginPermission::ProcessTabClose
        ),
        30 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
                | PluginPermission::TabCreate
                | PluginPermission::PaneClose
                | PluginPermission::TabClose
                | PluginPermission::PaneMove
                | PluginPermission::TerminalInput
                | PluginPermission::PaneLayout
                | PluginPermission::WindowClose
                | PluginPermission::ProcessQuery
                | PluginPermission::ProcessActions
                | PluginPermission::ProcessTerminalText
                | PluginPermission::ProcessTerminalInput
                | PluginPermission::ProcessTabCreate
                | PluginPermission::ProcessPaneCreate
                | PluginPermission::ProcessPaneMove
                | PluginPermission::ProcessTabMove
                | PluginPermission::ProcessPaneClose
                | PluginPermission::ProcessTabClose
                | PluginPermission::ProcessPaneLayout
        ),
        31 => matches!(
            permission,
            PluginPermission::ProcessSpawn
                | PluginPermission::WorkspaceQuery
                | PluginPermission::WorkspaceActions
                | PluginPermission::WorkspaceTopology
                | PluginPermission::EventSubscribe
                | PluginPermission::UiActions
                | PluginPermission::TerminalText
                | PluginPermission::TerminalQuery
                | PluginPermission::PaneCreate
                | PluginPermission::TabCreate
                | PluginPermission::PaneClose
                | PluginPermission::TabClose
                | PluginPermission::PaneMove
                | PluginPermission::TerminalInput
                | PluginPermission::PaneLayout
                | PluginPermission::WindowClose
                | PluginPermission::ProcessQuery
                | PluginPermission::ProcessActions
                | PluginPermission::ProcessTerminalText
                | PluginPermission::ProcessTerminalInput
                | PluginPermission::ProcessTabCreate
                | PluginPermission::ProcessPaneCreate
                | PluginPermission::ProcessPaneMove
                | PluginPermission::ProcessTabMove
                | PluginPermission::ProcessPaneClose
                | PluginPermission::ProcessTabClose
                | PluginPermission::ProcessPaneLayout
                | PluginPermission::ProcessWindowClose
        ),
        _ => false,
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
        );
    remaining = remaining.saturating_sub(command_json_bytes);
    let (output, output_truncated, output_sanitized, _) = bounded_json_terminal_text(
        if content_available {
            &output_source
        } else {
            ""
        },
        remaining.min(MAX_STRUCTURED_OUTPUT_JSON_BYTES),
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

/// Bound terminal text by its JSON string-body representation so quotes,
/// backslashes, and newlines cannot make the final event exceed its wire cap.
/// C0/C1 controls other than tab/newline are replaced before disclosure.
fn bounded_json_terminal_text(value: &str, max_json_bytes: usize) -> (String, bool, bool, usize) {
    let mut output = String::with_capacity(value.len().min(max_json_bytes));
    let mut used = 0_usize;
    let mut truncated = false;
    let mut sanitized = false;
    for character in value.chars() {
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
                    "requested_permissions": status.requested.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    "granted_permissions": status.granted.iter().map(ToString::to_string).collect::<Vec<_>>(),
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
                permission_list(status.requested.iter().copied()),
                permission_list(status.granted.iter().copied()),
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

pub fn registry_path() -> PathBuf {
    crate::config::Config::config_path().parent().map_or_else(
        || PathBuf::from("plugins.toml"),
        |parent| parent.join("plugins.toml"),
    )
}

pub fn plugins_dir() -> PathBuf {
    crate::config::Config::config_path()
        .parent()
        .map_or_else(|| PathBuf::from("plugins"), |parent| parent.join("plugins"))
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
        "schema_version": 1,
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
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fnv1a64:{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

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

    fn process_grant() -> BTreeSet<PluginPermission> {
        BTreeSet::from([PluginPermission::ProcessSpawn])
    }

    fn structured_grants() -> BTreeSet<PluginPermission> {
        BTreeSet::from([
            PluginPermission::ProcessSpawn,
            PluginPermission::WorkspaceQuery,
        ])
    }

    fn event_grants(workspace: bool) -> BTreeSet<PluginPermission> {
        let mut grants = BTreeSet::from([
            PluginPermission::ProcessSpawn,
            PluginPermission::EventSubscribe,
            PluginPermission::UiActions,
        ]);
        if workspace {
            grants.insert(PluginPermission::WorkspaceQuery);
        }
        grants
    }

    fn event_manifest(workspace: bool) -> PluginManifest {
        let workspace_permission = if workspace {
            ", \"workspace-query\""
        } else {
            ""
        };
        toml::from_str(&format!(
            r#"api_version = 3
id = "lifecycle-fixture"
name = "Lifecycle Fixture"
version = "1.0.0"
permissions = ["process-spawn", "event-subscribe", "ui-actions"{workspace_permission}]

[event_handler]
program = "lifecycle-handler"
args = ["--stdio"]
events = ["app-ready", "tab-created", "tab-closed", "tab-switched"]
"#
        ))
        .unwrap()
    }

    fn command_event_grants() -> BTreeSet<PluginPermission> {
        BTreeSet::from([
            PluginPermission::ProcessSpawn,
            PluginPermission::EventSubscribe,
            PluginPermission::TerminalText,
        ])
    }

    fn command_event_manifest() -> PluginManifest {
        toml::from_str(
            r#"api_version = 4
id = "command-observer"
name = "Command Observer"
version = "1.0.0"
permissions = ["process-spawn", "event-subscribe", "terminal-text"]

[event_handler]
program = "command-handler"
args = ["--stdio"]
events = ["command-completed"]
"#,
        )
        .unwrap()
    }

    fn persistent_event_manifest() -> PluginManifest {
        let mut manifest = event_manifest(false);
        manifest.api_version = 5;
        manifest.event_handler.as_mut().unwrap().mode = DeclarativeHandlerMode::Persistent;
        manifest
    }

    fn workspace_query_manifest() -> PluginManifest {
        let mut manifest = persistent_event_manifest();
        manifest.api_version = 6;
        manifest
            .permissions
            .insert(PluginPermission::WorkspaceQuery);
        manifest
    }

    fn terminal_query_grants() -> BTreeSet<PluginPermission> {
        let mut grants = event_grants(true);
        grants.insert(PluginPermission::TerminalQuery);
        grants
    }

    fn terminal_query_manifest() -> PluginManifest {
        let mut manifest = workspace_query_manifest();
        manifest.api_version = 11;
        manifest.id = "terminal-reader".into();
        manifest.permissions.insert(PluginPermission::TerminalQuery);
        manifest.commands.push(DeclarativeHandlerCommand {
            id: "read-active-terminal".into(),
            name: "Read Active Terminal".into(),
            description: String::new(),
        });
        manifest
            .event_handler
            .as_mut()
            .unwrap()
            .events
            .insert(StructuredEventKind::CommandInvoked);
        manifest
    }

    fn pane_create_grants() -> BTreeSet<PluginPermission> {
        let mut grants = event_grants(true);
        grants.insert(PluginPermission::PaneCreate);
        grants
    }

    fn pane_create_manifest() -> PluginManifest {
        let mut manifest = workspace_query_manifest();
        manifest.api_version = 12;
        manifest.id = "pane-builder".into();
        manifest.permissions.insert(PluginPermission::PaneCreate);
        manifest.commands.push(DeclarativeHandlerCommand {
            id: "split-active-pane".into(),
            name: "Split Active Pane".into(),
            description: String::new(),
        });
        manifest
            .event_handler
            .as_mut()
            .unwrap()
            .events
            .insert(StructuredEventKind::CommandInvoked);
        manifest
    }

    fn tab_create_grants() -> BTreeSet<PluginPermission> {
        let mut grants = event_grants(false);
        grants.insert(PluginPermission::TabCreate);
        grants
    }

    fn tab_create_manifest() -> PluginManifest {
        let mut manifest = persistent_event_manifest();
        manifest.api_version = 13;
        manifest.id = "tab-builder".into();
        manifest.permissions.insert(PluginPermission::TabCreate);
        manifest.commands.push(DeclarativeHandlerCommand {
            id: "new-task-tab".into(),
            name: "New Task Tab".into(),
            description: String::new(),
        });
        manifest
            .event_handler
            .as_mut()
            .unwrap()
            .events
            .insert(StructuredEventKind::CommandInvoked);
        manifest
    }

    fn pane_close_grants() -> BTreeSet<PluginPermission> {
        let mut grants = event_grants(true);
        grants.insert(PluginPermission::PaneClose);
        grants
    }

    fn pane_close_manifest() -> PluginManifest {
        let mut manifest = workspace_query_manifest();
        manifest.api_version = 14;
        manifest.id = "pane-pruner".into();
        manifest.permissions.insert(PluginPermission::PaneClose);
        manifest.commands.push(DeclarativeHandlerCommand {
            id: "close-sibling-pane".into(),
            name: "Close Sibling Pane".into(),
            description: String::new(),
        });
        manifest
            .event_handler
            .as_mut()
            .unwrap()
            .events
            .insert(StructuredEventKind::CommandInvoked);
        manifest
    }

    fn tab_close_grants() -> BTreeSet<PluginPermission> {
        let mut grants = event_grants(true);
        grants.insert(PluginPermission::TabClose);
        grants
    }

    fn tab_close_manifest() -> PluginManifest {
        let mut manifest = workspace_query_manifest();
        manifest.api_version = 15;
        manifest.id = "tab-pruner".into();
        manifest.permissions.insert(PluginPermission::TabClose);
        manifest.commands.push(DeclarativeHandlerCommand {
            id: "close-background-tab".into(),
            name: "Close Background Tab".into(),
            description: String::new(),
        });
        manifest
            .event_handler
            .as_mut()
            .unwrap()
            .events
            .insert(StructuredEventKind::CommandInvoked);
        manifest
    }

    fn pane_move_grants() -> BTreeSet<PluginPermission> {
        let mut grants = event_grants(true);
        grants.insert(PluginPermission::PaneMove);
        grants
    }

    fn pane_move_manifest() -> PluginManifest {
        let mut manifest = workspace_query_manifest();
        manifest.api_version = 16;
        manifest.id = "pane-mover".into();
        manifest.permissions.insert(PluginPermission::PaneMove);
        manifest.commands.push(DeclarativeHandlerCommand {
            id: "move-sibling-pane".into(),
            name: "Move Sibling Pane".into(),
            description: String::new(),
        });
        manifest
            .event_handler
            .as_mut()
            .unwrap()
            .events
            .insert(StructuredEventKind::CommandInvoked);
        manifest
    }

    fn terminal_input_grants() -> BTreeSet<PluginPermission> {
        let mut grants = event_grants(true);
        grants.insert(PluginPermission::TerminalInput);
        grants
    }

    fn terminal_input_manifest() -> PluginManifest {
        let mut manifest = workspace_query_manifest();
        manifest.api_version = 17;
        manifest.id = "terminal-writer".into();
        manifest.permissions.insert(PluginPermission::TerminalInput);
        manifest.commands.push(DeclarativeHandlerCommand {
            id: "draft-cargo-test".into(),
            name: "Draft Cargo Test".into(),
            description: String::new(),
        });
        manifest
            .event_handler
            .as_mut()
            .unwrap()
            .events
            .insert(StructuredEventKind::CommandInvoked);
        manifest
    }

    fn pane_layout_grants() -> BTreeSet<PluginPermission> {
        let mut grants = event_grants(true);
        grants.insert(PluginPermission::PaneLayout);
        grants
    }

    fn pane_layout_manifest() -> PluginManifest {
        let mut manifest = workspace_query_manifest();
        manifest.api_version = 18;
        manifest.id = "pane-arranger".into();
        manifest.permissions.insert(PluginPermission::PaneLayout);
        manifest.commands.push(DeclarativeHandlerCommand {
            id: "apply-grid-layout".into(),
            name: "Apply Grid Layout".into(),
            description: String::new(),
        });
        manifest
            .event_handler
            .as_mut()
            .unwrap()
            .events
            .insert(StructuredEventKind::CommandInvoked);
        manifest
    }

    fn window_close_grants() -> BTreeSet<PluginPermission> {
        let mut grants = event_grants(true);
        grants.remove(&PluginPermission::UiActions);
        grants.insert(PluginPermission::WindowClose);
        grants
    }

    fn window_close_manifest() -> PluginManifest {
        let mut manifest = workspace_query_manifest();
        manifest.api_version = 19;
        manifest.id = "window-closer".into();
        manifest.permissions.remove(&PluginPermission::UiActions);
        manifest.permissions.insert(PluginPermission::WindowClose);
        manifest.commands.push(DeclarativeHandlerCommand {
            id: "close-current-window".into(),
            name: "Close Current Window".into(),
            description: String::new(),
        });
        manifest
            .event_handler
            .as_mut()
            .unwrap()
            .events
            .insert(StructuredEventKind::CommandInvoked);
        manifest
    }

    fn process_query_grants() -> BTreeSet<PluginPermission> {
        let mut grants = event_grants(false);
        grants.insert(PluginPermission::ProcessQuery);
        grants
    }

    fn process_query_manifest() -> PluginManifest {
        let mut manifest = persistent_event_manifest();
        manifest.api_version = 20;
        manifest.id = "process-inspector".into();
        manifest.permissions.insert(PluginPermission::ProcessQuery);
        manifest.commands.push(DeclarativeHandlerCommand {
            id: "inspect-process".into(),
            name: "Inspect Process Windows".into(),
            description: String::new(),
        });
        manifest
            .event_handler
            .as_mut()
            .unwrap()
            .events
            .insert(StructuredEventKind::CommandInvoked);
        manifest
    }

    fn process_action_grants() -> BTreeSet<PluginPermission> {
        let mut grants = process_query_grants();
        grants.insert(PluginPermission::ProcessActions);
        grants
    }

    fn process_action_manifest() -> PluginManifest {
        let mut manifest = process_query_manifest();
        manifest.api_version = 21;
        manifest.id = "process-navigator".into();
        manifest
            .permissions
            .insert(PluginPermission::ProcessActions);
        manifest.commands[0].id = "focus-first-pane".into();
        manifest.commands[0].name = "Focus First Pane in Another Window".into();
        manifest
    }

    fn process_terminal_text_grants() -> BTreeSet<PluginPermission> {
        let mut grants = process_query_grants();
        grants.insert(PluginPermission::ProcessTerminalText);
        grants
    }

    fn process_terminal_text_manifest() -> PluginManifest {
        let mut manifest = process_query_manifest();
        manifest.api_version = 22;
        manifest.id = "process-terminal-reader".into();
        manifest
            .permissions
            .insert(PluginPermission::ProcessTerminalText);
        manifest.commands[0].id = "read-sibling-pane".into();
        manifest.commands[0].name = "Read Sibling Pane Text".into();
        manifest
    }

    fn process_terminal_input_grants() -> BTreeSet<PluginPermission> {
        let mut grants = process_query_grants();
        grants.insert(PluginPermission::ProcessTerminalInput);
        grants
    }

    fn process_terminal_input_manifest() -> PluginManifest {
        let mut manifest = process_query_manifest();
        manifest.api_version = 23;
        manifest.id = "process-terminal-writer".into();
        manifest
            .permissions
            .insert(PluginPermission::ProcessTerminalInput);
        manifest.commands[0].id = "draft-in-sibling-pane".into();
        manifest.commands[0].name = "Draft Text in Sibling Pane".into();
        manifest
    }

    fn process_tab_create_grants() -> BTreeSet<PluginPermission> {
        let mut grants = process_query_grants();
        grants.insert(PluginPermission::ProcessTabCreate);
        grants
    }

    fn process_tab_create_manifest() -> PluginManifest {
        let mut manifest = process_query_manifest();
        manifest.api_version = 24;
        manifest.id = "process-tab-builder".into();
        manifest
            .permissions
            .insert(PluginPermission::ProcessTabCreate);
        manifest.commands[0].id = "new-tab-in-sibling-window".into();
        manifest.commands[0].name = "New Tab in Another Window".into();
        manifest
    }

    fn process_pane_create_grants() -> BTreeSet<PluginPermission> {
        let mut grants = process_query_grants();
        grants.insert(PluginPermission::ProcessPaneCreate);
        grants
    }

    fn process_pane_create_manifest() -> PluginManifest {
        let mut manifest = process_query_manifest();
        manifest.api_version = 25;
        manifest.id = "process-pane-builder".into();
        manifest
            .permissions
            .insert(PluginPermission::ProcessPaneCreate);
        manifest.commands[0].id = "split-pane-in-sibling-window".into();
        manifest.commands[0].name = "Split Pane in Another Window".into();
        manifest
    }

    fn process_pane_move_grants() -> BTreeSet<PluginPermission> {
        let mut grants = process_query_grants();
        grants.insert(PluginPermission::ProcessPaneMove);
        grants
    }

    fn process_pane_move_manifest() -> PluginManifest {
        let mut manifest = process_query_manifest();
        manifest.api_version = 26;
        manifest.id = "process-pane-mover".into();
        manifest
            .permissions
            .insert(PluginPermission::ProcessPaneMove);
        manifest.commands[0].id = "move-pane-between-windows".into();
        manifest.commands[0].name = "Move Pane Between Windows".into();
        manifest
    }

    fn process_tab_move_grants() -> BTreeSet<PluginPermission> {
        let mut grants = process_query_grants();
        grants.insert(PluginPermission::ProcessTabMove);
        grants
    }

    fn process_tab_move_manifest() -> PluginManifest {
        let mut manifest = process_query_manifest();
        manifest.api_version = 27;
        manifest.id = "process-tab-mover".into();
        manifest
            .permissions
            .insert(PluginPermission::ProcessTabMove);
        manifest.commands[0].id = "move-tab-between-windows".into();
        manifest.commands[0].name = "Move Tab Between Windows".into();
        manifest
    }

    fn process_pane_close_grants() -> BTreeSet<PluginPermission> {
        let mut grants = process_query_grants();
        grants.insert(PluginPermission::ProcessPaneClose);
        grants
    }

    fn process_pane_close_manifest() -> PluginManifest {
        let mut manifest = process_query_manifest();
        manifest.api_version = 28;
        manifest.id = "process-pane-pruner".into();
        manifest
            .permissions
            .insert(PluginPermission::ProcessPaneClose);
        manifest.commands[0].id = "close-pane-in-sibling-window".into();
        manifest.commands[0].name = "Close Pane in Another Window".into();
        manifest
    }

    fn process_tab_close_grants() -> BTreeSet<PluginPermission> {
        let mut grants = process_query_grants();
        grants.insert(PluginPermission::ProcessTabClose);
        grants
    }

    fn process_tab_close_manifest() -> PluginManifest {
        let mut manifest = process_query_manifest();
        manifest.api_version = 29;
        manifest.id = "process-tab-pruner".into();
        manifest
            .permissions
            .insert(PluginPermission::ProcessTabClose);
        manifest.commands[0].id = "close-tab-in-sibling-window".into();
        manifest.commands[0].name = "Close Tab in Another Window".into();
        manifest
    }

    fn process_pane_layout_grants() -> BTreeSet<PluginPermission> {
        let mut grants = process_query_grants();
        grants.insert(PluginPermission::ProcessPaneLayout);
        grants
    }

    fn process_pane_layout_manifest() -> PluginManifest {
        let mut manifest = process_query_manifest();
        manifest.api_version = 30;
        manifest.id = "process-pane-arranger".into();
        manifest
            .permissions
            .insert(PluginPermission::ProcessPaneLayout);
        manifest.commands[0].id = "apply-grid-in-sibling-window".into();
        manifest.commands[0].name = "Grid Panes in Another Window".into();
        manifest
    }

    fn process_window_close_grants() -> BTreeSet<PluginPermission> {
        let mut grants = process_query_grants();
        grants.insert(PluginPermission::ProcessWindowClose);
        grants
    }

    fn process_window_close_manifest() -> PluginManifest {
        let mut manifest = process_query_manifest();
        manifest.api_version = 31;
        manifest.id = "process-window-closer".into();
        manifest
            .permissions
            .insert(PluginPermission::ProcessWindowClose);
        manifest.commands[0].id = "close-sibling-window".into();
        manifest.commands[0].name = "Close Another Window".into();
        manifest
    }

    fn workspace_action_grants() -> BTreeSet<PluginPermission> {
        let mut grants = event_grants(true);
        grants.insert(PluginPermission::WorkspaceActions);
        grants
    }

    fn workspace_actions_manifest() -> PluginManifest {
        let mut manifest = workspace_query_manifest();
        manifest.api_version = 7;
        manifest
            .permissions
            .insert(PluginPermission::WorkspaceActions);
        manifest
    }

    fn workspace_topology_grants() -> BTreeSet<PluginPermission> {
        let mut grants = event_grants(true);
        grants.insert(PluginPermission::WorkspaceTopology);
        grants
    }

    fn workspace_topology_manifest() -> PluginManifest {
        let mut manifest = workspace_query_manifest();
        manifest.api_version = 8;
        manifest
            .permissions
            .insert(PluginPermission::WorkspaceTopology);
        manifest
    }

    fn handler_command_manifest() -> PluginManifest {
        toml::from_str(
            r#"api_version = 9
id = "command-tool"
name = "Command Tool"
version = "1.0.0"
permissions = ["process-spawn", "event-subscribe", "ui-actions"]

[[commands]]
id = "inspect-workspace"
name = "Inspect Workspace"
description = "Inspect the current workspace"

[event_handler]
program = "command-handler"
args = ["--stdio"]
mode = "persistent"
events = ["command-invoked"]
"#,
        )
        .unwrap()
    }

    fn configurable_manifest(version: &str) -> String {
        format!(
            r#"api_version = 10
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
mode = "persistent"
events = ["command-invoked"]
"#
        )
    }

    fn configurable_plugin_manifest() -> PluginManifest {
        toml::from_str(&configurable_manifest("1.0.0")).unwrap()
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
        assert!(validated.contains("api=1"));
        assert!(validated.contains("source="));
        assert!(validated.contains("fingerprint=fnv1a64:"));

        let installed = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: process_grant(),
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
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        assert!(matches!(
            plugins[0].on_hook(
                &Hook::CommandInvoked {
                    command_id: "sample-tools.open-default".into()
                },
                &context
            ),
            HookResponse::Action(PluginAction::OpenTab {
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
            HookResponse::Action(PluginAction::OpenTab {
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
            HookResponse::Action(PluginAction::OpenTab {
                profile: Some(profile), cwd: None, ..
            }) if profile == "quiet"
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
        assert!(
            statuses[0]
                .requested
                .contains(&PluginPermission::ProcessSpawn)
        );
        assert!(
            statuses[0]
                .granted
                .contains(&PluginPermission::ProcessSpawn)
        );
        execute_admin_at(
            PluginAdminCommand::Enable {
                id: "sample-tools".into(),
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
        assert!(json.contains("\"requested_permissions\""));
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
    fn denied_and_unsupported_capabilities_never_become_runtime_actions() {
        let (_temp, source, root, registry_path) = fixture();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest("1.0.0", ", \"network\"", "untrusted"),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: process_grant(),
                enable: false,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        let error = execute_admin_at(
            PluginAdminCommand::Enable {
                id: "sample-tools".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("permissions denied: network"));

        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source,
                grants: BTreeSet::from([PluginPermission::ProcessSpawn, PluginPermission::Network]),
                enable: true,
                replace: true,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("cannot grant"));
        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let (loaded, statuses) = discover_from_registry(&registry, &root);
        assert!(loaded.is_empty());
        assert!(statuses[0].requested.contains(&PluginPermission::Network));
        assert!(
            statuses[0]
                .granted
                .contains(&PluginPermission::ProcessSpawn)
        );
    }

    #[test]
    fn version_schema_tamper_and_resource_limits_fail_closed() {
        let (_temp, source, root, registry_path) = fixture();
        let invalid_api =
            manifest("1.0.0", "", "sample").replace("api_version = 1", "api_version = 32");
        fs::write(source.join(MANIFEST_NAME), invalid_api).unwrap();
        assert!(PluginManifest::from_directory(&source).is_err());

        let mut oversized = PluginManifest {
            api_version: 1,
            id: "large".into(),
            name: "Large".into(),
            version: "1".into(),
            description: String::new(),
            permissions: process_grant(),
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

        fs::write(source.join(MANIFEST_NAME), manifest("1.0.0", "", "sample")).unwrap();
        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source,
                grants: process_grant(),
                enable: true,
                replace: false,
            },
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

        fs::write(&registry_path, vec![b'x'; MAX_REGISTRY_BYTES as usize + 1]).unwrap();
        let error = PluginRegistry::load_from(&registry_path).unwrap_err();
        assert!(format!("{error:#}").contains("registry exceeds"));

        fs::write(&registry_path, "schema_version = 99\nplugins = []\n").unwrap();
        assert!(PluginRegistry::load_from(&registry_path).is_err());

        let mut malformed = PluginRegistry::default();
        malformed.plugins.push(InstalledPlugin {
            id: "sample-tools".into(),
            enabled: false,
            source: PathBuf::from("relative/source"),
            source_fingerprint: "not-a-fingerprint".into(),
            grants: BTreeSet::new(),
            default_profile: None,
            settings: BTreeMap::new(),
        });
        assert!(malformed.validate().is_err());
    }

    #[test]
    fn api_v2_workspace_query_is_explicit_versioned_and_bounded() {
        let parsed: PluginManifest = toml::from_str(
            &manifest("2.0.0", ", \"workspace-query\"", "context-fixture")
                .replace("api_version = 1", "api_version = 2"),
        )
        .unwrap();
        parsed.validate().unwrap();
        validate_grants(&parsed, &structured_grants()).unwrap();
        let mut plugin = DeclarativePlugin::new(parsed, None, structured_grants());
        let context = PluginContext {
            cwd: Some("/workspace/fixture".into()),
            tab_count: 4,
            active_tab: 2,
            terminal_size: (132, 43),
        };
        let response = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "sample-tools.open-default".into(),
            },
            &context,
        );
        let HookResponse::Action(PluginAction::OpenTab { env, .. }) = response else {
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
        assert_eq!(value["terminal"]["columns"], 132);
        assert_eq!(value["terminal"]["rows"], 43);
        assert!(context_json.len() <= MAX_STRUCTURED_CONTEXT_BYTES);
        assert!(!context_json.contains("terminal_text"));

        let response = plugin.on_hook(
            &Hook::WorkspaceRestore {
                plugin_id: "sample-tools".into(),
                profile: Some("default".into()),
                cwd: Some("/archived/workspace".into()),
            },
            &context,
        );
        let HookResponse::Action(PluginAction::OpenTab { cwd, env, .. }) = response else {
            panic!("structured workspace restore did not produce an open-tab action")
        };
        assert_eq!(cwd.as_deref(), Some("/archived/workspace"));
        let restored: serde_json::Value =
            serde_json::from_str(env.get(STRUCTURED_CONTEXT_ENV).unwrap()).unwrap();
        assert_eq!(restored["active_working_directory"], "/archived/workspace");
    }

    #[test]
    fn api_v2_workspace_query_grant_round_trips_install_and_discovery() {
        let (_temp, source, root, registry_path) = fixture();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest("2.0.0", ", \"workspace-query\"", "context-fixture")
                .replace("api_version = 1", "api_version = 2"),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source,
                grants: structured_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        assert_eq!(registry.plugins[0].grants, structured_grants());
        let (mut loaded, statuses) = discover_from_registry(&registry, &root);
        assert_eq!(loaded.len(), 1);
        assert_eq!(statuses[0].granted, structured_grants());
        let response = loaded[0].on_hook(
            &Hook::CommandInvoked {
                command_id: "sample-tools.open-default".into(),
            },
            &PluginContext {
                cwd: None,
                tab_count: 2,
                active_tab: 1,
                terminal_size: (90, 28),
            },
        );
        assert!(matches!(
            response,
            HookResponse::Action(PluginAction::OpenTab { env, .. })
                if env.contains_key(STRUCTURED_CONTEXT_ENV)
        ));
    }

    #[test]
    fn checked_in_api_v2_context_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/context-probe");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 2);
        assert_eq!(manifest.id, "context-probe");
        assert_eq!(manifest.permissions, structured_grants());
        assert_eq!(source, directory.canonicalize().unwrap());
    }

    #[test]
    fn checked_in_api_v3_lifecycle_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/lifecycle-notifier");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 3);
        assert_eq!(manifest.id, "lifecycle-notifier");
        assert_eq!(manifest.permissions, event_grants(false));
        assert!(manifest.profiles.is_empty());
        assert_eq!(source, directory.canonicalize().unwrap());
    }

    #[test]
    fn checked_in_api_v4_command_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/command-observer");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 4);
        assert_eq!(manifest.id, "command-observer");
        assert_eq!(manifest.permissions, command_event_grants());
        assert!(manifest.profiles.is_empty());
        assert_eq!(source, directory.canonicalize().unwrap());

        let temporary = tempfile::tempdir().unwrap();
        let registry_path = temporary.path().join("plugins.toml");
        let root = temporary.path().join("installed");
        execute_admin_at(
            PluginAdminCommand::Install {
                directory,
                grants: command_event_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let (loaded, statuses) = discover_from_registry(&registry, &root);
        assert_eq!(loaded.len(), 1);
        assert!(statuses[0].enabled);
        assert_eq!(statuses[0].granted, command_event_grants());
    }

    #[test]
    fn checked_in_api_v5_persistent_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/persistent-counter");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 5);
        assert_eq!(manifest.id, "persistent-counter");
        assert_eq!(manifest.permissions, event_grants(false));
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
    }

    #[test]
    fn checked_in_api_v6_workspace_query_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/workspace-inspector");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 6);
        assert_eq!(manifest.id, "workspace-inspector");
        assert_eq!(manifest.permissions, event_grants(true));
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
    }

    #[test]
    fn checked_in_api_v7_workspace_action_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/workspace-navigator");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 7);
        assert_eq!(manifest.id, "workspace-navigator");
        assert_eq!(manifest.permissions, workspace_action_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
    }

    #[test]
    fn checked_in_api_v8_workspace_topology_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/workspace-organizer");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 8);
        assert_eq!(manifest.id, "workspace-organizer");
        assert_eq!(manifest.permissions, workspace_topology_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
    }

    #[test]
    fn checked_in_api_v9_handler_command_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/command-tool");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 9);
        assert_eq!(manifest.id, "command-tool");
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.commands[0].id, "inspect-workspace");
        assert_eq!(manifest.permissions, event_grants(true));
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
    }

    #[test]
    fn checked_in_api_v10_configurable_command_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/configurable-command");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 10);
        assert_eq!(manifest.id, "configurable-command");
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.settings.len(), 3);
        assert_eq!(manifest.permissions, event_grants(false));
        assert_eq!(source, directory.canonicalize().unwrap());
    }

    #[test]
    fn checked_in_api_v11_terminal_reader_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/terminal-reader");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 11);
        assert_eq!(manifest.id, "terminal-reader");
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.commands[0].id, "read-active-terminal");
        assert_eq!(manifest.permissions, terminal_query_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
        let registry = PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "terminal-reader".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: terminal_query_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        };
        registry.validate().unwrap();
    }

    #[test]
    fn checked_in_api_v12_pane_builder_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/pane-builder");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 12);
        assert_eq!(manifest.id, "pane-builder");
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.commands[0].id, "split-active-pane");
        assert_eq!(manifest.permissions, pane_create_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
        let registry = PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "pane-builder".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: pane_create_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        };
        registry.validate().unwrap();
    }

    #[test]
    fn checked_in_api_v16_pane_mover_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/pane-mover");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 16);
        assert_eq!(manifest.id, "pane-mover");
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.commands[0].id, "move-sibling-pane");
        assert_eq!(manifest.permissions, pane_move_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
        let registry = PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "pane-mover".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: pane_move_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        };
        registry.validate().unwrap();
    }

    #[test]
    fn checked_in_api_v17_terminal_writer_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/terminal-writer");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 17);
        assert_eq!(manifest.id, "terminal-writer");
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.commands[0].id, "draft-cargo-test");
        assert_eq!(manifest.permissions, terminal_input_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
        let registry = PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "terminal-writer".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: terminal_input_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        };
        registry.validate().unwrap();
    }

    #[test]
    fn checked_in_api_v18_pane_arranger_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/pane-arranger");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 18);
        assert_eq!(manifest.id, "pane-arranger");
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.commands[0].id, "apply-grid-layout");
        assert_eq!(manifest.permissions, pane_layout_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
        let registry = PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "pane-arranger".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: pane_layout_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        };
        registry.validate().unwrap();
    }

    #[test]
    fn checked_in_api_v19_window_closer_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/window-closer");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 19);
        assert_eq!(manifest.id, "window-closer");
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.commands[0].id, "close-current-window");
        assert_eq!(manifest.permissions, window_close_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
        let registry = PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "window-closer".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: window_close_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        };
        registry.validate().unwrap();
    }

    #[test]
    fn checked_in_api_v20_process_inspector_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/process-inspector");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 20);
        assert_eq!(manifest.id, "process-inspector");
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.commands[0].id, "inspect-process");
        assert_eq!(manifest.permissions, process_query_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
        let registry = PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "process-inspector".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: process_query_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        };
        registry.validate().unwrap();
    }

    #[test]
    fn checked_in_api_v21_process_navigator_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/process-navigator");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 21);
        assert_eq!(manifest.id, "process-navigator");
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.commands[0].id, "focus-first-pane");
        assert_eq!(manifest.permissions, process_action_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
        let registry = PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "process-navigator".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: process_action_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        };
        registry.validate().unwrap();
    }

    #[test]
    fn checked_in_api_v22_process_terminal_reader_example_is_a_valid_manifest() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/process-terminal-reader");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 22);
        assert_eq!(manifest.id, "process-terminal-reader");
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.commands[0].id, "read-sibling-pane");
        assert_eq!(manifest.permissions, process_terminal_text_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
        let registry = PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "process-terminal-reader".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: process_terminal_text_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        };
        registry.validate().unwrap();
    }

    #[test]
    fn checked_in_api_v23_process_terminal_writer_example_is_a_valid_manifest() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/process-terminal-writer");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 23);
        assert_eq!(manifest.id, "process-terminal-writer");
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.commands[0].id, "draft-in-sibling-pane");
        assert_eq!(manifest.permissions, process_terminal_input_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
        let registry = PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "process-terminal-writer".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: process_terminal_input_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        };
        registry.validate().unwrap();
    }

    #[test]
    fn checked_in_api_v24_process_tab_builder_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/process-tab-builder");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 24);
        assert_eq!(manifest.id, "process-tab-builder");
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.commands[0].id, "new-tab-in-sibling-window");
        assert_eq!(manifest.permissions, process_tab_create_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
        let registry = PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "process-tab-builder".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: process_tab_create_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        };
        registry.validate().unwrap();
    }

    #[test]
    fn checked_in_api_v25_process_pane_builder_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/process-pane-builder");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 25);
        assert_eq!(manifest.id, "process-pane-builder");
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.commands[0].id, "split-pane-in-sibling-window");
        assert_eq!(manifest.permissions, process_pane_create_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
        let registry = PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "process-pane-builder".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: process_pane_create_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        };
        registry.validate().unwrap();
    }

    #[test]
    fn checked_in_api_v26_process_pane_mover_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/process-pane-mover");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 26);
        assert_eq!(manifest.id, "process-pane-mover");
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.commands[0].id, "move-pane-between-windows");
        assert_eq!(manifest.permissions, process_pane_move_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
        let registry = PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "process-pane-mover".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: process_pane_move_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        };
        registry.validate().unwrap();
    }

    #[test]
    fn checked_in_api_v27_process_tab_mover_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/process-tab-mover");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 27);
        assert_eq!(manifest.id, "process-tab-mover");
        assert_eq!(manifest.commands[0].id, "move-tab-between-windows");
        assert_eq!(manifest.permissions, process_tab_move_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
        PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "process-tab-mover".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: process_tab_move_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn checked_in_api_v28_process_pane_pruner_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/process-pane-pruner");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 28);
        assert_eq!(manifest.id, "process-pane-pruner");
        assert_eq!(manifest.commands[0].id, "close-pane-in-sibling-window");
        assert_eq!(manifest.permissions, process_pane_close_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
        PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "process-pane-pruner".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: process_pane_close_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn checked_in_api_v29_process_tab_pruner_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/process-tab-pruner");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 29);
        assert_eq!(manifest.id, "process-tab-pruner");
        assert_eq!(manifest.commands[0].id, "close-tab-in-sibling-window");
        assert_eq!(manifest.permissions, process_tab_close_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "process-tab-pruner".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: process_tab_close_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn checked_in_api_v30_process_pane_arranger_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/process-pane-arranger");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 30);
        assert_eq!(manifest.id, "process-pane-arranger");
        assert_eq!(manifest.commands[0].id, "apply-grid-in-sibling-window");
        assert_eq!(manifest.permissions, process_pane_layout_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "process-pane-arranger".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: process_pane_layout_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn checked_in_api_v31_process_window_closer_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/process-window-closer");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 31);
        assert_eq!(manifest.id, "process-window-closer");
        assert_eq!(manifest.commands[0].id, "close-sibling-window");
        assert_eq!(manifest.permissions, process_window_close_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "process-window-closer".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: process_window_close_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn checked_in_api_v15_tab_pruner_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/tab-pruner");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 15);
        assert_eq!(manifest.id, "tab-pruner");
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.commands[0].id, "close-background-tab");
        assert_eq!(manifest.permissions, tab_close_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
        let registry = PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "tab-pruner".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: tab_close_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        };
        registry.validate().unwrap();
    }

    #[test]
    fn checked_in_api_v14_pane_pruner_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/pane-pruner");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 14);
        assert_eq!(manifest.id, "pane-pruner");
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.commands[0].id, "close-sibling-pane");
        assert_eq!(manifest.permissions, pane_close_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
        let registry = PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "pane-pruner".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: pane_close_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        };
        registry.validate().unwrap();
    }

    #[test]
    fn checked_in_api_v13_tab_builder_example_is_a_valid_manifest() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins/tab-builder");
        let (manifest, source, _) = PluginManifest::from_directory(&directory).unwrap();
        assert_eq!(manifest.api_version, 13);
        assert_eq!(manifest.id, "tab-builder");
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.commands[0].id, "new-task-tab");
        assert_eq!(manifest.permissions, tab_create_grants());
        assert_eq!(
            manifest.event_handler.unwrap().mode,
            DeclarativeHandlerMode::Persistent
        );
        assert_eq!(source, directory.canonicalize().unwrap());
        let registry = PluginRegistry {
            schema_version: REGISTRY_SCHEMA_VERSION,
            plugins: vec![InstalledPlugin {
                id: "tab-builder".into(),
                enabled: true,
                source,
                source_fingerprint: "fnv1a64:0123456789abcdef".into(),
                grants: tab_create_grants(),
                default_profile: None,
                settings: BTreeMap::new(),
            }],
        };
        registry.validate().unwrap();
    }

    #[test]
    fn api_v17_terminal_input_installs_and_updates_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/terminal-writer")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();

        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: event_grants(true),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("terminal-input"));

        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: terminal_input_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "terminal-writer".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();

        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "terminal-writer")
            .unwrap();
        assert!(installed.enabled);
        assert_eq!(installed.grants, terminal_input_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("terminal-writer")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v18_pane_layout_installs_and_updates_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/pane-arranger")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();

        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: event_grants(true),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("pane-layout"));

        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: pane_layout_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "pane-arranger".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();

        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "pane-arranger")
            .unwrap();
        assert!(installed.enabled);
        assert_eq!(installed.grants, pane_layout_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("pane-arranger")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v19_window_close_installs_and_updates_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/window-closer")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();

        let mut missing_window_close = window_close_grants();
        missing_window_close.remove(&PluginPermission::WindowClose);
        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: missing_window_close,
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("window-close"));

        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: window_close_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "window-closer".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();

        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "window-closer")
            .unwrap();
        assert!(installed.enabled);
        assert_eq!(installed.grants, window_close_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("window-closer")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v20_process_query_installs_and_updates_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/process-inspector")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();

        let mut missing_process_query = process_query_grants();
        missing_process_query.remove(&PluginPermission::ProcessQuery);
        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: missing_process_query,
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("process-query"));

        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: process_query_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "process-inspector".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();

        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "process-inspector")
            .unwrap();
        assert!(installed.enabled);
        assert_eq!(installed.grants, process_query_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("process-inspector")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v21_process_actions_install_and_update_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/process-navigator")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();

        let mut missing_process_actions = process_action_grants();
        missing_process_actions.remove(&PluginPermission::ProcessActions);
        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: missing_process_actions,
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("process-actions"));

        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: process_action_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "process-navigator".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();

        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "process-navigator")
            .unwrap();
        assert!(installed.enabled);
        assert_eq!(installed.grants, process_action_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("process-navigator")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v22_process_terminal_text_installs_and_updates_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/process-terminal-reader")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();

        let mut missing_terminal_text = process_terminal_text_grants();
        missing_terminal_text.remove(&PluginPermission::ProcessTerminalText);
        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: missing_terminal_text,
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("process-terminal-text"));

        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: process_terminal_text_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "process-terminal-reader".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();

        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "process-terminal-reader")
            .unwrap();
        assert!(installed.enabled);
        assert_eq!(installed.grants, process_terminal_text_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("process-terminal-reader")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v23_process_terminal_input_installs_and_updates_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/process-terminal-writer")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();

        let mut missing_terminal_input = process_terminal_input_grants();
        missing_terminal_input.remove(&PluginPermission::ProcessTerminalInput);
        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: missing_terminal_input,
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("process-terminal-input"));

        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: process_terminal_input_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "process-terminal-writer".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();

        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "process-terminal-writer")
            .unwrap();
        assert!(installed.enabled);
        assert_eq!(installed.grants, process_terminal_input_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("process-terminal-writer")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v24_process_tab_create_installs_and_updates_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/process-tab-builder")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();

        let mut missing_tab_create = process_tab_create_grants();
        missing_tab_create.remove(&PluginPermission::ProcessTabCreate);
        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: missing_tab_create,
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("process-tab-create"));

        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: process_tab_create_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "process-tab-builder".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();

        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "process-tab-builder")
            .unwrap();
        assert!(installed.enabled);
        assert_eq!(installed.grants, process_tab_create_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("process-tab-builder")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v25_process_pane_create_installs_and_updates_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/process-pane-builder")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();

        let mut missing_pane_create = process_pane_create_grants();
        missing_pane_create.remove(&PluginPermission::ProcessPaneCreate);
        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: missing_pane_create,
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("process-pane-create"));

        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: process_pane_create_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "process-pane-builder".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();

        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "process-pane-builder")
            .unwrap();
        assert!(installed.enabled);
        assert_eq!(installed.grants, process_pane_create_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("process-pane-builder")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v26_process_pane_move_installs_and_updates_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/process-pane-mover")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();

        let mut missing_pane_move = process_pane_move_grants();
        missing_pane_move.remove(&PluginPermission::ProcessPaneMove);
        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: missing_pane_move,
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("process-pane-move"));

        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: process_pane_move_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "process-pane-mover".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();

        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "process-pane-mover")
            .unwrap();
        assert!(installed.enabled);
        assert_eq!(installed.grants, process_pane_move_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("process-pane-mover")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v27_process_tab_move_installs_and_updates_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/process-tab-mover")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();

        let mut missing = process_tab_move_grants();
        missing.remove(&PluginPermission::ProcessTabMove);
        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: missing,
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("process-tab-move"));

        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: process_tab_move_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "process-tab-mover".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();
        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "process-tab-mover")
            .unwrap();
        assert_eq!(installed.grants, process_tab_move_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("process-tab-mover")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v28_process_pane_close_installs_and_updates_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/process-pane-pruner")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();

        let mut missing = process_pane_close_grants();
        missing.remove(&PluginPermission::ProcessPaneClose);
        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: missing,
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("process-pane-close"));

        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: process_pane_close_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "process-pane-pruner".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();
        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "process-pane-pruner")
            .unwrap();
        assert_eq!(installed.grants, process_pane_close_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("process-pane-pruner")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v29_process_tab_close_installs_and_updates_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/process-tab-pruner")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();
        let mut missing = process_tab_close_grants();
        missing.remove(&PluginPermission::ProcessTabClose);
        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: missing,
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("process-tab-close"));
        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: process_tab_close_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "process-tab-pruner".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();
        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "process-tab-pruner")
            .unwrap();
        assert_eq!(installed.grants, process_tab_close_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("process-tab-pruner")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v30_process_pane_layout_installs_and_updates_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/process-pane-arranger")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();
        let mut missing = process_pane_layout_grants();
        missing.remove(&PluginPermission::ProcessPaneLayout);
        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: missing,
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("process-pane-layout"));
        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: process_pane_layout_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "process-pane-arranger".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();
        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "process-pane-arranger")
            .unwrap();
        assert_eq!(installed.grants, process_pane_layout_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("process-pane-arranger")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v31_process_window_close_installs_and_updates_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/process-window-closer")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();
        let mut missing = process_window_close_grants();
        missing.remove(&PluginPermission::ProcessWindowClose);
        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: missing,
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("process-window-close"));
        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: process_window_close_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "process-window-closer".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();
        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "process-window-closer")
            .unwrap();
        assert_eq!(installed.grants, process_window_close_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("process-window-closer")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v16_pane_move_installs_and_updates_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/pane-mover")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();

        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: event_grants(true),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("pane-move"));

        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: pane_move_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "pane-mover".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();

        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "pane-mover")
            .unwrap();
        assert!(installed.enabled);
        assert_eq!(installed.grants, pane_move_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("pane-mover")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v15_tab_close_installs_and_updates_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/tab-pruner")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();

        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: event_grants(true),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("tab-close"));

        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: tab_close_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "tab-pruner".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();

        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "tab-pruner")
            .unwrap();
        assert!(installed.enabled);
        assert_eq!(installed.grants, tab_close_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("tab-pruner")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v14_pane_close_installs_and_updates_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/pane-pruner")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();

        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: event_grants(true),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("pane-close"));

        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: pane_close_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "pane-pruner".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();

        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "pane-pruner")
            .unwrap();
        assert!(installed.enabled);
        assert_eq!(installed.grants, pane_close_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("pane-pruner")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v13_tab_create_installs_and_updates_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/tab-builder")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();

        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: event_grants(false),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("tab-create"));

        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: tab_create_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "tab-builder".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();

        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "tab-builder")
            .unwrap();
        assert!(installed.enabled);
        assert_eq!(installed.grants, tab_create_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("tab-builder")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v12_pane_create_installs_and_updates_only_with_the_retained_grant() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir(&source).unwrap();
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/plugins/pane-builder")
            .join(MANIFEST_NAME);
        let manifest = fs::read_to_string(checked_in).unwrap();
        fs::write(source.join(MANIFEST_NAME), &manifest).unwrap();

        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: event_grants(true),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("pane-create"));

        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: pane_create_grants(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest.replace("version = \"1.0.0\"", "version = \"1.1.0\""),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "pane-builder".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();

        let registry = PluginRegistry::load_from(&registry_path).unwrap();
        let installed = registry
            .plugins
            .iter()
            .find(|plugin| plugin.id == "pane-builder")
            .unwrap();
        assert!(installed.enabled);
        assert_eq!(installed.grants, pane_create_grants());
        let (installed_manifest, _, _) =
            PluginManifest::from_directory(&root.join("pane-builder")).unwrap();
        assert_eq!(installed_manifest.version, "1.1.0");
    }

    #[test]
    fn api_v1_cannot_gain_workspace_query_and_oversized_context_fails_visible() {
        let v1: PluginManifest = toml::from_str(&manifest(
            "1.0.0",
            ", \"workspace-query\"",
            "context-fixture",
        ))
        .unwrap();
        v1.validate().unwrap();
        let error = validate_grants(&v1, &structured_grants()).unwrap_err();
        assert!(format!("{error:#}").contains("API v1 cannot grant: workspace-query"));

        let v2: PluginManifest = toml::from_str(
            &manifest("2.0.0", ", \"workspace-query\"", "context-fixture")
                .replace("api_version = 1", "api_version = 2"),
        )
        .unwrap();
        let mut plugin = DeclarativePlugin::new(v2, None, structured_grants());
        let response = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "sample-tools.open-default".into(),
            },
            &PluginContext {
                cwd: Some("x".repeat(MAX_STRUCTURED_CONTEXT_BYTES + 1)),
                tab_count: 1,
                active_tab: 0,
                terminal_size: (80, 24),
            },
        );
        assert!(matches!(
            response,
            HookResponse::Action(PluginAction::ShowNotice { title, lines })
                if title.contains("context unavailable")
                    && lines[0].contains("structured plugin launch context exceeds")
        ));
    }

    #[test]
    fn api_v3_lifecycle_events_are_versioned_coarse_and_permission_scoped() {
        let manifest = event_manifest(true);
        manifest.validate().unwrap();
        validate_grants(&manifest, &event_grants(true)).unwrap();
        let mut plugin = DeclarativePlugin::new(manifest, None, event_grants(true));
        let response = plugin.on_hook(
            &Hook::TabSwitched { from: 2, to: 4 },
            &PluginContext {
                cwd: Some("/workspace/project".into()),
                tab_count: 5,
                active_tab: 4,
                terminal_size: (120, 36),
            },
        );
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = response
        else {
            panic!("subscribed lifecycle event did not create a handler invocation")
        };
        assert_eq!(invocation.plugin_id, "lifecycle-fixture");
        assert!(invocation.allow_ui_actions);
        assert_eq!(invocation.mode, StructuredHandlerMode::ProcessPerEvent);
        let value: serde_json::Value = serde_json::from_str(&invocation.event_json).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["event"]["type"], "tab-switched");
        assert_eq!(value["event"]["from"], 2);
        assert_eq!(value["event"]["to"], 4);
        assert_eq!(
            value["context"]["active_working_directory"],
            "/workspace/project"
        );
        assert_eq!(value["context"]["terminal"]["columns"], 120);
        assert!(invocation.event_json.len() <= MAX_STRUCTURED_CONTEXT_BYTES);
        assert!(!invocation.event_json.contains("terminal_text"));
        assert!(!invocation.event_json.contains("command"));

        let manifest = event_manifest(false);
        let mut plugin = DeclarativePlugin::new(manifest, None, event_grants(false));
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::AppReady,
                &PluginContext {
                    cwd: Some("/must-not-leak".into()),
                    tab_count: 1,
                    active_tab: 0,
                    terminal_size: (80, 24),
                },
            )
        else {
            panic!("app-ready subscription did not create an invocation")
        };
        let value: serde_json::Value = serde_json::from_str(&invocation.event_json).unwrap();
        assert!(value.get("context").is_none());
        assert!(!invocation.event_json.contains("must-not-leak"));
    }

    #[test]
    fn api_v3_event_manifest_rules_fail_closed() {
        let mut manifest = event_manifest(false);
        manifest.api_version = 2;
        assert!(
            manifest
                .validate()
                .unwrap_err()
                .to_string()
                .contains("API v3")
        );

        let mut manifest = event_manifest(false);
        manifest
            .permissions
            .remove(&PluginPermission::EventSubscribe);
        assert!(
            manifest
                .validate()
                .unwrap_err()
                .to_string()
                .contains("event-subscribe")
        );

        let mut manifest = event_manifest(false);
        manifest.event_handler.as_mut().unwrap().events.clear();
        assert!(
            manifest
                .validate()
                .unwrap_err()
                .to_string()
                .contains("at least one event")
        );

        let mut manifest: PluginManifest =
            toml::from_str(&self::manifest("1.0.0", "", "profile-handler")).unwrap();
        manifest.api_version = 3;
        manifest
            .permissions
            .insert(PluginPermission::EventSubscribe);
        assert!(
            manifest
                .validate()
                .unwrap_err()
                .to_string()
                .contains("event-subscribe requires")
        );

        let manifest = event_manifest(false);
        let denied = BTreeSet::from([PluginPermission::ProcessSpawn]);
        assert!(
            validate_grants(&manifest, &denied)
                .unwrap_err()
                .to_string()
                .contains("permissions denied")
        );
    }

    #[test]
    fn api_v5_persistent_mode_is_explicit_and_reserves_protocol_space() {
        let manifest = persistent_event_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &event_grants(false)).unwrap();
        let mut plugin = DeclarativePlugin::new(manifest, None, event_grants(false));
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::TabCreated { tab_id: usize::MAX },
                &PluginContext {
                    cwd: None,
                    tab_count: usize::MAX,
                    active_tab: usize::MAX,
                    terminal_size: (usize::MAX, usize::MAX),
                },
            )
        else {
            panic!("persistent subscription did not create an invocation")
        };
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);
        assert!(!invocation.allow_workspace_query);
        assert!(invocation.event_json.len() <= MAX_PERSISTENT_SOURCE_EVENT_BYTES);

        let mut v4 = persistent_event_manifest();
        v4.api_version = 4;
        assert!(v4.validate().unwrap_err().to_string().contains("API v5"));
    }

    #[test]
    fn api_v6_workspace_queries_require_persistent_mode_and_exact_grant() {
        let manifest = workspace_query_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &event_grants(true)).unwrap();
        let context = PluginContext {
            cwd: Some("/workspace".into()),
            tab_count: 2,
            active_tab: 1,
            terminal_size: (100, 30),
        };
        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, event_grants(true));
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) =
            plugin.on_hook(&Hook::AppReady, &context)
        else {
            panic!("API v6 query handler did not create an invocation")
        };
        assert!(invocation.allow_workspace_query);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v5 = manifest.clone();
        v5.api_version = 5;
        let mut plugin = DeclarativePlugin::new(v5, None, event_grants(true));
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) =
            plugin.on_hook(&Hook::AppReady, &context)
        else {
            panic!("API v5 persistent handler did not create an invocation")
        };
        assert!(!invocation.allow_workspace_query);

        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        let mut plugin = DeclarativePlugin::new(process_per_event, None, event_grants(true));
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) =
            plugin.on_hook(&Hook::AppReady, &context)
        else {
            panic!("API v6 process-per-event handler did not create an invocation")
        };
        assert!(!invocation.allow_workspace_query);

        let mut unrequested = manifest;
        unrequested
            .permissions
            .remove(&PluginPermission::WorkspaceQuery);
        let grants = BTreeSet::from([
            PluginPermission::ProcessSpawn,
            PluginPermission::EventSubscribe,
            PluginPermission::UiActions,
        ]);
        let mut plugin = DeclarativePlugin::new(unrequested, None, grants);
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) =
            plugin.on_hook(&Hook::AppReady, &context)
        else {
            panic!("unrequested query fixture did not create an invocation")
        };
        assert!(!invocation.allow_workspace_query);
    }

    #[test]
    fn api_v11_terminal_queries_require_persistent_mode_and_exact_grant() {
        let manifest = terminal_query_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &terminal_query_grants()).unwrap();
        let context = PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, terminal_query_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "terminal-reader.read-active-terminal".into(),
                },
                &context,
            )
        else {
            panic!("API v11 terminal query handler did not create an invocation")
        };
        assert!(invocation.allow_workspace_query);
        assert!(invocation.allow_terminal_query);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v10 = manifest.clone();
        v10.api_version = 10;
        assert!(v10.validate().unwrap_err().to_string().contains("API v11"));

        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );

        let mut grants = terminal_query_grants();
        grants.remove(&PluginPermission::TerminalQuery);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: terminal-query")
        );
    }

    #[test]
    fn api_v12_pane_creation_requires_persistent_mode_and_exact_grant() {
        let manifest = pane_create_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &pane_create_grants()).unwrap();
        let context = PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, pane_create_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "pane-builder.split-active-pane".into(),
                },
                &context,
            )
        else {
            panic!("API v12 pane creation handler did not create an invocation")
        };
        assert!(invocation.allow_workspace_query);
        assert!(invocation.allow_pane_create);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v11 = manifest.clone();
        v11.api_version = 11;
        assert!(v11.validate().unwrap_err().to_string().contains("API v12"));

        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );

        let mut grants = pane_create_grants();
        grants.remove(&PluginPermission::PaneCreate);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: pane-create")
        );

        let mut plugin = DeclarativePlugin::new(manifest, None, event_grants(true));
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "pane-builder.split-active-pane".into(),
                },
                &context,
            )
        else {
            panic!("ungranted pane creation fixture did not create an invocation")
        };
        assert!(!invocation.allow_pane_create);
    }

    #[test]
    fn api_v17_terminal_input_requires_persistent_mode_and_exact_grant() {
        let manifest = terminal_input_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &terminal_input_grants()).unwrap();
        let context = PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, terminal_input_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "terminal-writer.draft-cargo-test".into(),
                },
                &context,
            )
        else {
            panic!("API v17 terminal input handler did not create an invocation")
        };
        assert!(invocation.allow_workspace_query);
        assert!(invocation.allow_terminal_input);
        assert!(!invocation.allow_pane_move);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v16 = manifest.clone();
        v16.api_version = 16;
        assert!(v16.validate().unwrap_err().to_string().contains("API v17"));

        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );

        let mut grants = terminal_input_grants();
        grants.remove(&PluginPermission::TerminalInput);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: terminal-input")
        );

        let mut plugin = DeclarativePlugin::new(manifest, None, event_grants(true));
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "terminal-writer.draft-cargo-test".into(),
                },
                &context,
            )
        else {
            panic!("ungranted terminal input fixture did not create an invocation")
        };
        assert!(!invocation.allow_terminal_input);
    }

    #[test]
    fn api_v18_pane_layout_requires_persistent_mode_and_exact_grant() {
        let manifest = pane_layout_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &pane_layout_grants()).unwrap();
        let context = PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, pane_layout_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "pane-arranger.apply-grid-layout".into(),
                },
                &context,
            )
        else {
            panic!("API v18 pane layout handler did not create an invocation")
        };
        assert!(invocation.allow_workspace_query);
        assert!(invocation.allow_pane_layout);
        assert!(!invocation.allow_terminal_input);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v17 = manifest.clone();
        v17.api_version = 17;
        assert!(v17.validate().unwrap_err().to_string().contains("API v18"));

        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );

        let mut grants = pane_layout_grants();
        grants.remove(&PluginPermission::PaneLayout);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: pane-layout")
        );

        let mut plugin = DeclarativePlugin::new(manifest, None, event_grants(true));
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "pane-arranger.apply-grid-layout".into(),
                },
                &context,
            )
        else {
            panic!("ungranted pane layout fixture did not create an invocation")
        };
        assert!(!invocation.allow_pane_layout);
    }

    #[test]
    fn api_v19_window_close_requires_exact_grant_and_explicit_command_scope() {
        let manifest = window_close_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &window_close_grants()).unwrap();
        assert_eq!(PluginPermission::WindowClose.to_string(), "window-close");
        assert_eq!(
            "window-close".parse::<PluginPermission>().unwrap(),
            PluginPermission::WindowClose
        );
        let context = PluginContext {
            cwd: None,
            tab_count: 2,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, window_close_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "window-closer.close-current-window".into(),
                },
                &context,
            )
        else {
            panic!("API v19 window close command did not create an invocation")
        };
        assert!(invocation.allow_workspace_query);
        assert!(invocation.allow_window_close);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) =
            plugin.on_hook(&Hook::AppReady, &context)
        else {
            panic!("API v19 lifecycle fixture did not create an invocation")
        };
        assert!(!invocation.allow_window_close);

        let mut v18 = manifest.clone();
        v18.api_version = 18;
        assert!(v18.validate().unwrap_err().to_string().contains("API v19"));

        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );

        let mut no_command = manifest.clone();
        no_command.commands.clear();
        assert!(
            no_command
                .validate()
                .unwrap_err()
                .to_string()
                .contains("command-invoked")
        );

        let mut grants = window_close_grants();
        grants.remove(&PluginPermission::WindowClose);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: window-close")
        );

        let mut plugin = DeclarativePlugin::new(manifest, None, event_grants(true));
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "window-closer.close-current-window".into(),
                },
                &context,
            )
        else {
            panic!("ungranted window close fixture did not create an invocation")
        };
        assert!(!invocation.allow_window_close);
    }

    #[test]
    fn api_v20_process_query_requires_persistent_mode_and_exact_grant() {
        let manifest = process_query_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &process_query_grants()).unwrap();
        assert_eq!(PluginPermission::ProcessQuery.to_string(), "process-query");
        assert_eq!(
            "process-query".parse::<PluginPermission>().unwrap(),
            PluginPermission::ProcessQuery
        );
        let context = PluginContext {
            cwd: None,
            tab_count: 2,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, process_query_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-inspector.inspect-process".into(),
                },
                &context,
            )
        else {
            panic!("API v20 process query command did not create an invocation")
        };
        assert!(invocation.allow_process_query);
        assert!(!invocation.allow_window_close);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v19 = manifest.clone();
        v19.api_version = 19;
        assert!(v19.validate().unwrap_err().to_string().contains("API v20"));

        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );

        let mut grants = process_query_grants();
        grants.remove(&PluginPermission::ProcessQuery);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: process-query")
        );

        let mut plugin = DeclarativePlugin::new(manifest, None, event_grants(false));
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-inspector.inspect-process".into(),
                },
                &context,
            )
        else {
            panic!("ungranted process query fixture did not create an invocation")
        };
        assert!(!invocation.allow_process_query);
    }

    #[test]
    fn api_v21_process_actions_require_persistent_mode_and_exact_grant() {
        let manifest = process_action_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &process_action_grants()).unwrap();
        assert_eq!(
            PluginPermission::ProcessActions.to_string(),
            "process-actions"
        );
        assert_eq!(
            "process-actions".parse::<PluginPermission>().unwrap(),
            PluginPermission::ProcessActions
        );
        let context = PluginContext {
            cwd: None,
            tab_count: 2,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, process_action_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-navigator.focus-first-pane".into(),
                },
                &context,
            )
        else {
            panic!("API v21 process action command did not create an invocation")
        };
        assert!(invocation.allow_process_query);
        assert!(invocation.allow_process_actions);
        assert!(!invocation.allow_process_terminal_text);
        assert!(!invocation.allow_process_terminal_input);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v20 = manifest.clone();
        v20.api_version = 20;
        assert!(v20.validate().unwrap_err().to_string().contains("API v21"));

        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );

        let mut grants = process_action_grants();
        grants.remove(&PluginPermission::ProcessActions);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: process-actions")
        );

        let mut plugin = DeclarativePlugin::new(manifest, None, process_query_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-navigator.focus-first-pane".into(),
                },
                &context,
            )
        else {
            panic!("ungranted process action fixture did not create an invocation")
        };
        assert!(invocation.allow_process_query);
        assert!(!invocation.allow_process_actions);
    }

    #[test]
    fn api_v22_process_terminal_text_requires_persistent_mode_and_exact_grant() {
        let manifest = process_terminal_text_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &process_terminal_text_grants()).unwrap();
        assert_eq!(
            PluginPermission::ProcessTerminalText.to_string(),
            "process-terminal-text"
        );
        assert_eq!(
            "process-terminal-text".parse::<PluginPermission>().unwrap(),
            PluginPermission::ProcessTerminalText
        );
        let context = PluginContext {
            cwd: None,
            tab_count: 2,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin =
            DeclarativePlugin::new(manifest.clone(), None, process_terminal_text_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-terminal-reader.read-sibling-pane".into(),
                },
                &context,
            )
        else {
            panic!("API v22 process terminal query command did not create an invocation")
        };
        assert!(invocation.allow_process_query);
        assert!(!invocation.allow_process_actions);
        assert!(invocation.allow_process_terminal_text);
        assert!(!invocation.allow_process_terminal_input);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v21 = manifest.clone();
        v21.api_version = 21;
        assert!(v21.validate().unwrap_err().to_string().contains("API v22"));

        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );

        let mut grants = process_terminal_text_grants();
        grants.remove(&PluginPermission::ProcessTerminalText);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: process-terminal-text")
        );

        let mut plugin = DeclarativePlugin::new(manifest, None, process_query_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-terminal-reader.read-sibling-pane".into(),
                },
                &context,
            )
        else {
            panic!("ungranted process terminal query fixture did not create an invocation")
        };
        assert!(invocation.allow_process_query);
        assert!(!invocation.allow_process_terminal_text);
    }

    #[test]
    fn api_v23_process_terminal_input_requires_persistent_mode_and_exact_grant() {
        let manifest = process_terminal_input_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &process_terminal_input_grants()).unwrap();
        assert_eq!(
            PluginPermission::ProcessTerminalInput.to_string(),
            "process-terminal-input"
        );
        assert_eq!(
            "process-terminal-input"
                .parse::<PluginPermission>()
                .unwrap(),
            PluginPermission::ProcessTerminalInput
        );
        let context = PluginContext {
            cwd: None,
            tab_count: 2,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin =
            DeclarativePlugin::new(manifest.clone(), None, process_terminal_input_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-terminal-writer.draft-in-sibling-pane".into(),
                },
                &context,
            )
        else {
            panic!("API v23 process terminal input command did not create an invocation")
        };
        assert!(invocation.allow_process_query);
        assert!(!invocation.allow_process_actions);
        assert!(!invocation.allow_process_terminal_text);
        assert!(invocation.allow_process_terminal_input);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v22 = manifest.clone();
        v22.api_version = 22;
        assert!(v22.validate().unwrap_err().to_string().contains("API v23"));

        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );

        let mut grants = process_terminal_input_grants();
        grants.remove(&PluginPermission::ProcessTerminalInput);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: process-terminal-input")
        );

        let mut plugin = DeclarativePlugin::new(manifest, None, process_query_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-terminal-writer.draft-in-sibling-pane".into(),
                },
                &context,
            )
        else {
            panic!("ungranted process terminal input fixture did not create an invocation")
        };
        assert!(invocation.allow_process_query);
        assert!(!invocation.allow_process_terminal_input);
    }

    #[test]
    fn api_v24_process_tab_creation_requires_persistent_mode_and_exact_grant() {
        let manifest = process_tab_create_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &process_tab_create_grants()).unwrap();
        assert_eq!(
            PluginPermission::ProcessTabCreate.to_string(),
            "process-tab-create"
        );
        assert_eq!(
            "process-tab-create".parse::<PluginPermission>().unwrap(),
            PluginPermission::ProcessTabCreate
        );
        let context = PluginContext {
            cwd: None,
            tab_count: 2,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin =
            DeclarativePlugin::new(manifest.clone(), None, process_tab_create_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-tab-builder.new-tab-in-sibling-window".into(),
                },
                &context,
            )
        else {
            panic!("API v24 process tab creation command did not create an invocation")
        };
        assert!(invocation.allow_process_query);
        assert!(!invocation.allow_process_actions);
        assert!(!invocation.allow_process_terminal_text);
        assert!(!invocation.allow_process_terminal_input);
        assert!(invocation.allow_process_tab_create);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v23 = manifest.clone();
        v23.api_version = 23;
        assert!(v23.validate().unwrap_err().to_string().contains("API v24"));

        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );

        let mut grants = process_tab_create_grants();
        grants.remove(&PluginPermission::ProcessTabCreate);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: process-tab-create")
        );

        let mut plugin = DeclarativePlugin::new(manifest, None, process_query_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-tab-builder.new-tab-in-sibling-window".into(),
                },
                &context,
            )
        else {
            panic!("ungranted process tab creation fixture did not create an invocation")
        };
        assert!(invocation.allow_process_query);
        assert!(!invocation.allow_process_tab_create);
    }

    #[test]
    fn api_v25_process_pane_creation_requires_persistent_mode_and_exact_grant() {
        let manifest = process_pane_create_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &process_pane_create_grants()).unwrap();
        assert_eq!(
            PluginPermission::ProcessPaneCreate.to_string(),
            "process-pane-create"
        );
        assert_eq!(
            "process-pane-create".parse::<PluginPermission>().unwrap(),
            PluginPermission::ProcessPaneCreate
        );
        let context = PluginContext {
            cwd: None,
            tab_count: 2,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin =
            DeclarativePlugin::new(manifest.clone(), None, process_pane_create_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-pane-builder.split-pane-in-sibling-window".into(),
                },
                &context,
            )
        else {
            panic!("API v25 process pane creation command did not create an invocation")
        };
        assert!(invocation.allow_process_query);
        assert!(!invocation.allow_process_actions);
        assert!(!invocation.allow_process_terminal_text);
        assert!(!invocation.allow_process_terminal_input);
        assert!(!invocation.allow_process_tab_create);
        assert!(invocation.allow_process_pane_create);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v24 = manifest.clone();
        v24.api_version = 24;
        assert!(v24.validate().unwrap_err().to_string().contains("API v25"));

        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );

        let mut grants = process_pane_create_grants();
        grants.remove(&PluginPermission::ProcessPaneCreate);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: process-pane-create")
        );

        let mut plugin = DeclarativePlugin::new(manifest, None, process_query_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-pane-builder.split-pane-in-sibling-window".into(),
                },
                &context,
            )
        else {
            panic!("ungranted process pane creation fixture did not create an invocation")
        };
        assert!(invocation.allow_process_query);
        assert!(!invocation.allow_process_pane_create);
    }

    #[test]
    fn api_v26_process_pane_movement_requires_persistent_mode_and_exact_grant() {
        let manifest = process_pane_move_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &process_pane_move_grants()).unwrap();
        assert_eq!(
            PluginPermission::ProcessPaneMove.to_string(),
            "process-pane-move"
        );
        assert_eq!(
            "process-pane-move".parse::<PluginPermission>().unwrap(),
            PluginPermission::ProcessPaneMove
        );
        let context = PluginContext {
            cwd: None,
            tab_count: 2,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, process_pane_move_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-pane-mover.move-pane-between-windows".into(),
                },
                &context,
            )
        else {
            panic!("API v26 process pane movement command did not create an invocation")
        };
        assert!(invocation.allow_process_query);
        assert!(!invocation.allow_process_actions);
        assert!(!invocation.allow_process_terminal_text);
        assert!(!invocation.allow_process_terminal_input);
        assert!(!invocation.allow_process_tab_create);
        assert!(!invocation.allow_process_pane_create);
        assert!(invocation.allow_process_pane_move);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v25 = manifest.clone();
        v25.api_version = 25;
        assert!(v25.validate().unwrap_err().to_string().contains("API v26"));

        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );

        let mut grants = process_pane_move_grants();
        grants.remove(&PluginPermission::ProcessPaneMove);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: process-pane-move")
        );

        let mut plugin = DeclarativePlugin::new(manifest, None, process_query_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-pane-mover.move-pane-between-windows".into(),
                },
                &context,
            )
        else {
            panic!("ungranted process pane movement fixture did not create an invocation")
        };
        assert!(invocation.allow_process_query);
        assert!(!invocation.allow_process_pane_move);
    }

    #[test]
    fn api_v27_process_tab_movement_requires_persistent_mode_and_exact_grant() {
        let manifest = process_tab_move_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &process_tab_move_grants()).unwrap();
        assert_eq!(
            PluginPermission::ProcessTabMove.to_string(),
            "process-tab-move"
        );
        assert_eq!(
            "process-tab-move".parse::<PluginPermission>().unwrap(),
            PluginPermission::ProcessTabMove
        );
        let context = PluginContext {
            cwd: None,
            tab_count: 2,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, process_tab_move_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-tab-mover.move-tab-between-windows".into(),
                },
                &context,
            )
        else {
            panic!("API v27 process tab movement command did not create an invocation")
        };
        assert!(invocation.allow_process_query);
        assert!(!invocation.allow_process_pane_move);
        assert!(invocation.allow_process_tab_move);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v26 = manifest.clone();
        v26.api_version = 26;
        assert!(v26.validate().unwrap_err().to_string().contains("API v27"));
        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );
        let mut grants = process_tab_move_grants();
        grants.remove(&PluginPermission::ProcessTabMove);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: process-tab-move")
        );
        let mut plugin = DeclarativePlugin::new(manifest, None, process_query_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-tab-mover.move-tab-between-windows".into(),
                },
                &context,
            )
        else {
            panic!("ungranted process tab movement fixture did not create an invocation")
        };
        assert!(!invocation.allow_process_tab_move);
    }

    #[test]
    fn api_v28_process_pane_closure_requires_persistent_mode_and_exact_grant() {
        let manifest = process_pane_close_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &process_pane_close_grants()).unwrap();
        assert_eq!(
            PluginPermission::ProcessPaneClose.to_string(),
            "process-pane-close"
        );
        assert_eq!(
            "process-pane-close".parse::<PluginPermission>().unwrap(),
            PluginPermission::ProcessPaneClose
        );
        let context = PluginContext {
            cwd: None,
            tab_count: 2,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin =
            DeclarativePlugin::new(manifest.clone(), None, process_pane_close_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-pane-pruner.close-pane-in-sibling-window".into(),
                },
                &context,
            )
        else {
            panic!("API v28 process pane closure command did not create an invocation")
        };
        assert!(invocation.allow_process_query);
        assert!(!invocation.allow_process_tab_move);
        assert!(invocation.allow_process_pane_close);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v27 = manifest.clone();
        v27.api_version = 27;
        assert!(v27.validate().unwrap_err().to_string().contains("API v28"));
        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );
        let mut grants = process_pane_close_grants();
        grants.remove(&PluginPermission::ProcessPaneClose);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: process-pane-close")
        );
        let mut plugin = DeclarativePlugin::new(manifest, None, process_query_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-pane-pruner.close-pane-in-sibling-window".into(),
                },
                &context,
            )
        else {
            panic!("ungranted process pane closure fixture did not create an invocation")
        };
        assert!(!invocation.allow_process_pane_close);
    }

    #[test]
    fn api_v29_process_tab_closure_requires_persistent_mode_and_exact_grant() {
        let manifest = process_tab_close_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &process_tab_close_grants()).unwrap();
        assert_eq!(
            PluginPermission::ProcessTabClose.to_string(),
            "process-tab-close"
        );
        assert_eq!(
            "process-tab-close".parse::<PluginPermission>().unwrap(),
            PluginPermission::ProcessTabClose
        );
        let context = PluginContext {
            cwd: None,
            tab_count: 2,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, process_tab_close_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-tab-pruner.close-tab-in-sibling-window".into(),
                },
                &context,
            )
        else {
            panic!("API v29 process tab closure command did not create an invocation")
        };
        assert!(invocation.allow_process_query);
        assert!(!invocation.allow_process_pane_close);
        assert!(invocation.allow_process_tab_close);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);
        let mut v28 = manifest.clone();
        v28.api_version = 28;
        assert!(v28.validate().unwrap_err().to_string().contains("API v29"));
        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );
        let mut grants = process_tab_close_grants();
        grants.remove(&PluginPermission::ProcessTabClose);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: process-tab-close")
        );
        let mut plugin = DeclarativePlugin::new(manifest, None, process_query_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-tab-pruner.close-tab-in-sibling-window".into(),
                },
                &context,
            )
        else {
            panic!("ungranted process tab closure fixture did not create an invocation")
        };
        assert!(!invocation.allow_process_tab_close);
    }

    #[test]
    fn api_v30_process_pane_layout_requires_persistent_mode_and_exact_grant() {
        let manifest = process_pane_layout_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &process_pane_layout_grants()).unwrap();
        assert_eq!(
            PluginPermission::ProcessPaneLayout.to_string(),
            "process-pane-layout"
        );
        assert_eq!(
            "process-pane-layout".parse::<PluginPermission>().unwrap(),
            PluginPermission::ProcessPaneLayout
        );
        let context = PluginContext {
            cwd: None,
            tab_count: 2,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin =
            DeclarativePlugin::new(manifest.clone(), None, process_pane_layout_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-pane-arranger.apply-grid-in-sibling-window".into(),
                },
                &context,
            )
        else {
            panic!("API v30 process pane layout command did not create an invocation")
        };
        assert!(invocation.allow_process_query);
        assert!(!invocation.allow_process_tab_close);
        assert!(invocation.allow_process_pane_layout);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v29 = manifest.clone();
        v29.api_version = 29;
        assert!(v29.validate().unwrap_err().to_string().contains("API v30"));
        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );
        let mut grants = process_pane_layout_grants();
        grants.remove(&PluginPermission::ProcessPaneLayout);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: process-pane-layout")
        );
        let mut plugin = DeclarativePlugin::new(manifest, None, process_query_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-pane-arranger.apply-grid-in-sibling-window".into(),
                },
                &context,
            )
        else {
            panic!("ungranted process pane layout fixture did not create an invocation")
        };
        assert!(!invocation.allow_process_pane_layout);
    }

    #[test]
    fn api_v31_process_window_closure_requires_persistent_mode_and_exact_grant() {
        let manifest = process_window_close_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &process_window_close_grants()).unwrap();
        assert_eq!(
            PluginPermission::ProcessWindowClose.to_string(),
            "process-window-close"
        );
        assert_eq!(
            "process-window-close".parse::<PluginPermission>().unwrap(),
            PluginPermission::ProcessWindowClose
        );
        let context = PluginContext {
            cwd: None,
            tab_count: 2,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin =
            DeclarativePlugin::new(manifest.clone(), None, process_window_close_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-window-closer.close-sibling-window".into(),
                },
                &context,
            )
        else {
            panic!("API v31 process window closure command did not create an invocation")
        };
        assert!(invocation.allow_process_query);
        assert!(!invocation.allow_process_pane_layout);
        assert!(invocation.allow_process_window_close);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v30 = manifest.clone();
        v30.api_version = 30;
        assert!(v30.validate().unwrap_err().to_string().contains("API v31"));
        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );
        let mut grants = process_window_close_grants();
        grants.remove(&PluginPermission::ProcessWindowClose);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: process-window-close")
        );
        let mut plugin = DeclarativePlugin::new(manifest, None, process_query_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "process-window-closer.close-sibling-window".into(),
                },
                &context,
            )
        else {
            panic!("ungranted process window closure fixture did not create an invocation")
        };
        assert!(!invocation.allow_process_window_close);
    }

    #[test]
    fn api_v16_pane_movement_requires_persistent_mode_and_exact_grant() {
        let manifest = pane_move_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &pane_move_grants()).unwrap();
        let context = PluginContext {
            cwd: None,
            tab_count: 2,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, pane_move_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "pane-mover.move-sibling-pane".into(),
                },
                &context,
            )
        else {
            panic!("API v16 pane movement handler did not create an invocation")
        };
        assert!(invocation.allow_workspace_query);
        assert!(invocation.allow_pane_move);
        assert!(!invocation.allow_tab_close);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v15 = manifest.clone();
        v15.api_version = 15;
        assert!(v15.validate().unwrap_err().to_string().contains("API v16"));

        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );

        let mut grants = pane_move_grants();
        grants.remove(&PluginPermission::PaneMove);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: pane-move")
        );

        let mut plugin = DeclarativePlugin::new(manifest, None, event_grants(true));
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "pane-mover.move-sibling-pane".into(),
                },
                &context,
            )
        else {
            panic!("ungranted pane movement fixture did not create an invocation")
        };
        assert!(!invocation.allow_pane_move);
    }

    #[test]
    fn api_v15_tab_closure_requires_persistent_mode_and_exact_grant() {
        let manifest = tab_close_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &tab_close_grants()).unwrap();
        let context = PluginContext {
            cwd: None,
            tab_count: 2,
            active_tab: 1,
            terminal_size: (80, 24),
        };
        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, tab_close_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "tab-pruner.close-background-tab".into(),
                },
                &context,
            )
        else {
            panic!("API v15 tab closure handler did not create an invocation")
        };
        assert!(invocation.allow_workspace_query);
        assert!(invocation.allow_tab_close);
        assert!(!invocation.allow_pane_close);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v14 = manifest.clone();
        v14.api_version = 14;
        assert!(v14.validate().unwrap_err().to_string().contains("API v15"));

        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );

        let mut grants = tab_close_grants();
        grants.remove(&PluginPermission::TabClose);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: tab-close")
        );

        let mut plugin = DeclarativePlugin::new(manifest, None, event_grants(true));
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "tab-pruner.close-background-tab".into(),
                },
                &context,
            )
        else {
            panic!("ungranted tab closure fixture did not create an invocation")
        };
        assert!(!invocation.allow_tab_close);
    }

    #[test]
    fn api_v14_pane_closure_requires_persistent_mode_and_exact_grant() {
        let manifest = pane_close_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &pane_close_grants()).unwrap();
        let context = PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, pane_close_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "pane-pruner.close-sibling-pane".into(),
                },
                &context,
            )
        else {
            panic!("API v14 pane closure handler did not create an invocation")
        };
        assert!(invocation.allow_workspace_query);
        assert!(invocation.allow_pane_close);
        assert!(!invocation.allow_pane_create);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v13 = manifest.clone();
        v13.api_version = 13;
        assert!(v13.validate().unwrap_err().to_string().contains("API v14"));

        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );

        let mut grants = pane_close_grants();
        grants.remove(&PluginPermission::PaneClose);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: pane-close")
        );

        let mut plugin = DeclarativePlugin::new(manifest, None, event_grants(true));
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "pane-pruner.close-sibling-pane".into(),
                },
                &context,
            )
        else {
            panic!("ungranted pane closure fixture did not create an invocation")
        };
        assert!(!invocation.allow_pane_close);
    }

    #[test]
    fn api_v13_tab_creation_requires_persistent_mode_and_exact_grant() {
        let manifest = tab_create_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &tab_create_grants()).unwrap();
        let context = PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, tab_create_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "tab-builder.new-task-tab".into(),
                },
                &context,
            )
        else {
            panic!("API v13 tab creation handler did not create an invocation")
        };
        assert!(invocation.allow_tab_create);
        assert!(!invocation.allow_pane_create);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut v12 = manifest.clone();
        v12.api_version = 12;
        assert!(v12.validate().unwrap_err().to_string().contains("API v13"));

        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("persistent")
        );

        let mut grants = tab_create_grants();
        grants.remove(&PluginPermission::TabCreate);
        assert!(
            validate_grants(&manifest, &grants)
                .unwrap_err()
                .to_string()
                .contains("permissions denied: tab-create")
        );

        let mut plugin = DeclarativePlugin::new(manifest, None, event_grants(false));
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "tab-builder.new-task-tab".into(),
                },
                &context,
            )
        else {
            panic!("ungranted tab creation fixture did not create an invocation")
        };
        assert!(!invocation.allow_tab_create);
    }

    #[test]
    fn api_v7_workspace_actions_require_persistent_mode_and_exact_grant() {
        let manifest = workspace_actions_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &workspace_action_grants()).unwrap();
        let context = PluginContext {
            cwd: Some("/workspace".into()),
            tab_count: 2,
            active_tab: 1,
            terminal_size: (100, 30),
        };
        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, workspace_action_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) =
            plugin.on_hook(&Hook::AppReady, &context)
        else {
            panic!("API v7 workspace action handler did not create an invocation")
        };
        assert!(invocation.allow_workspace_query);
        assert!(invocation.allow_workspace_actions);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut action_only_manifest = manifest.clone();
        action_only_manifest
            .permissions
            .remove(&PluginPermission::WorkspaceQuery);
        let mut action_only_grants = event_grants(false);
        action_only_grants.insert(PluginPermission::WorkspaceActions);
        action_only_manifest.validate().unwrap();
        validate_grants(&action_only_manifest, &action_only_grants).unwrap();
        let mut plugin = DeclarativePlugin::new(action_only_manifest, None, action_only_grants);
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) =
            plugin.on_hook(&Hook::AppReady, &context)
        else {
            panic!("action-only API v7 fixture did not create an invocation")
        };
        assert!(!invocation.allow_workspace_query);
        assert!(invocation.allow_workspace_actions);

        let mut v6 = manifest.clone();
        v6.api_version = 6;
        assert!(
            v6.validate()
                .unwrap_err()
                .to_string()
                .contains("API v7 persistent")
        );

        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("API v7 persistent")
        );

        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, event_grants(true));
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) =
            plugin.on_hook(&Hook::AppReady, &context)
        else {
            panic!("ungranted workspace action fixture did not create an invocation")
        };
        assert!(!invocation.allow_workspace_actions);
        assert!(
            validate_grants(&manifest, &event_grants(true))
                .unwrap_err()
                .to_string()
                .contains("workspace-actions")
        );

        let mut unrequested = manifest;
        unrequested
            .permissions
            .remove(&PluginPermission::WorkspaceActions);
        let mut plugin = DeclarativePlugin::new(unrequested, None, event_grants(true));
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) =
            plugin.on_hook(&Hook::AppReady, &context)
        else {
            panic!("unrequested workspace action fixture did not create an invocation")
        };
        assert!(!invocation.allow_workspace_actions);
    }

    #[test]
    fn api_v8_workspace_topology_requires_persistent_mode_and_exact_grant() {
        let manifest = workspace_topology_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &workspace_topology_grants()).unwrap();
        let context = PluginContext {
            cwd: Some("/workspace".into()),
            tab_count: 2,
            active_tab: 1,
            terminal_size: (100, 30),
        };
        let mut plugin =
            DeclarativePlugin::new(manifest.clone(), None, workspace_topology_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) =
            plugin.on_hook(&Hook::AppReady, &context)
        else {
            panic!("API v8 workspace topology handler did not create an invocation")
        };
        assert!(invocation.allow_workspace_query);
        assert!(!invocation.allow_workspace_actions);
        assert!(invocation.allow_workspace_topology);
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);

        let mut topology_only_manifest = manifest.clone();
        topology_only_manifest
            .permissions
            .remove(&PluginPermission::WorkspaceQuery);
        let mut topology_only_grants = event_grants(false);
        topology_only_grants.insert(PluginPermission::WorkspaceTopology);
        topology_only_manifest.validate().unwrap();
        validate_grants(&topology_only_manifest, &topology_only_grants).unwrap();
        let mut plugin = DeclarativePlugin::new(topology_only_manifest, None, topology_only_grants);
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) =
            plugin.on_hook(&Hook::AppReady, &context)
        else {
            panic!("topology-only API v8 fixture did not create an invocation")
        };
        assert!(!invocation.allow_workspace_query);
        assert!(invocation.allow_workspace_topology);

        let mut v7 = manifest.clone();
        v7.api_version = 7;
        assert!(
            v7.validate()
                .unwrap_err()
                .to_string()
                .contains("API v8 persistent")
        );

        let mut process_per_event = manifest.clone();
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        assert!(
            process_per_event
                .validate()
                .unwrap_err()
                .to_string()
                .contains("API v8 persistent")
        );

        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, event_grants(true));
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) =
            plugin.on_hook(&Hook::AppReady, &context)
        else {
            panic!("ungranted workspace topology fixture did not create an invocation")
        };
        assert!(!invocation.allow_workspace_topology);
        assert!(
            validate_grants(&manifest, &event_grants(true))
                .unwrap_err()
                .to_string()
                .contains("workspace-topology")
        );

        let mut unrequested = manifest;
        unrequested
            .permissions
            .remove(&PluginPermission::WorkspaceTopology);
        let mut plugin = DeclarativePlugin::new(unrequested, None, event_grants(true));
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) =
            plugin.on_hook(&Hook::AppReady, &context)
        else {
            panic!("unrequested workspace topology fixture did not create an invocation")
        };
        assert!(!invocation.allow_workspace_topology);
    }

    #[test]
    fn api_v9_handler_commands_are_bounded_namespaced_and_exact() {
        let manifest = handler_command_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &event_grants(false)).unwrap();
        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, event_grants(false));
        let commands = plugin.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].id, "command-tool.inspect-workspace");
        assert_eq!(commands[0].name, "Inspect Workspace");
        assert_eq!(commands[0].category.as_deref(), Some("Command Tool"));

        let context = PluginContext {
            cwd: Some("/workspace/private".into()),
            tab_count: 3,
            active_tab: 1,
            terminal_size: (120, 36),
        };
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "command-tool.inspect-workspace".into(),
                },
                &context,
            )
        else {
            panic!("declared API v9 command did not create a handler invocation")
        };
        assert_eq!(invocation.plugin_id, "command-tool");
        assert_eq!(invocation.mode, StructuredHandlerMode::Persistent);
        let value: serde_json::Value = serde_json::from_str(&invocation.event_json).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["plugin_id"], "command-tool");
        assert_eq!(value["event"]["type"], "command-invoked");
        assert_eq!(
            value["event"]["command_id"],
            "command-tool.inspect-workspace"
        );
        assert!(value.get("context").is_none());
        assert!(invocation.event_json.len() <= MAX_PERSISTENT_SOURCE_EVENT_BYTES);

        assert!(matches!(
            plugin.on_hook(
                &Hook::CommandInvoked {
                    command_id: "command-tool.undeclared".into(),
                },
                &context,
            ),
            HookResponse::Ignored
        ));

        let mut process_per_event = manifest;
        process_per_event.event_handler.as_mut().unwrap().mode =
            DeclarativeHandlerMode::ProcessPerEvent;
        process_per_event.validate().unwrap();
        let mut plugin = DeclarativePlugin::new(process_per_event, None, event_grants(false));
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "command-tool.inspect-workspace".into(),
                },
                &context,
            )
        else {
            panic!("process-per-event API v9 command did not create an invocation")
        };
        assert_eq!(invocation.mode, StructuredHandlerMode::ProcessPerEvent);
    }

    #[test]
    fn api_v9_handler_command_contract_fails_closed() {
        let manifest = handler_command_manifest();

        let mut v8 = manifest.clone();
        v8.api_version = 8;
        assert!(v8.validate().unwrap_err().to_string().contains("API v9"));

        let mut missing_subscription = manifest.clone();
        missing_subscription
            .event_handler
            .as_mut()
            .unwrap()
            .events
            .clear();
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
    fn api_v10_setting_schema_is_typed_bounded_and_versioned() {
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

        let mut v9 = manifest.clone();
        v9.api_version = 9;
        assert!(v9.validate().unwrap_err().to_string().contains("API v10"));

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
    fn api_v10_settings_persist_reload_and_reach_profiles_and_handlers() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join(MANIFEST_NAME), configurable_manifest("1.0.0")).unwrap();
        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source,
                grants: event_grants(false),
                enable: true,
                replace: false,
            },
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
            tab_count: 2,
            active_tab: 1,
            terminal_size: (100, 30),
        };
        let HookResponse::Action(PluginAction::OpenTab { env, .. }) = plugins[0].on_hook(
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

        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugins[0]
            .on_hook(
                &Hook::CommandInvoked {
                    command_id: "configurable-tool.show-settings".into(),
                },
                &context,
            )
        else {
            panic!("configured handler command did not produce an invocation")
        };
        let handler_event: serde_json::Value =
            serde_json::from_str(&invocation.event_json).unwrap();
        assert_eq!(handler_event["settings"]["detail"], "full");
        assert_eq!(handler_event["settings"]["label"], "configured");

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
    fn api_v10_invalid_setting_overrides_fail_without_persistence() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join(MANIFEST_NAME), configurable_manifest("1.0.0")).unwrap();
        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source,
                grants: event_grants(false),
                enable: true,
                replace: false,
            },
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

    #[test]
    fn api_v4_command_events_are_bounded_typed_and_terminal_text_gated() {
        let manifest = command_event_manifest();
        manifest.validate().unwrap();
        validate_grants(&manifest, &command_event_grants()).unwrap();
        assert!(
            validate_grants(
                &manifest,
                &BTreeSet::from([
                    PluginPermission::ProcessSpawn,
                    PluginPermission::EventSubscribe,
                ]),
            )
            .unwrap_err()
            .to_string()
            .contains("permissions denied: terminal-text")
        );
        let mut plugin = DeclarativePlugin::new(manifest.clone(), None, command_event_grants());
        let response = plugin.on_hook(
            &Hook::CommandExecuted {
                region_id: 44,
                tab_id: 7,
                pane_id: Some(3),
                command: format!("\u{0007}printf '\"{}\"'", "x".repeat(4_000)),
                command_truncated: false,
                output: format!("\u{0001}{}", "\\\n".repeat(12_000)),
                output_truncated: true,
                content_available: true,
                exit_code: Some(9),
            },
            &PluginContext {
                cwd: Some("/workspace/project".into()),
                tab_count: 8,
                active_tab: 7,
                terminal_size: (120, 40),
            },
        );
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = response
        else {
            panic!("subscribed command event did not create a handler invocation")
        };
        assert!(invocation.event_json.len() <= MAX_STRUCTURED_CONTEXT_BYTES);
        assert_eq!(invocation.mode, StructuredHandlerMode::ProcessPerEvent);
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
        assert!(
            value["event"]["command"]["text"].as_str().unwrap().len()
                <= MAX_STRUCTURED_COMMAND_JSON_BYTES
        );
        assert!(value.get("context").is_none());
        assert!(!invocation.event_json.contains("/workspace/project"));

        let mut ungranted = DeclarativePlugin::new(
            manifest.clone(),
            None,
            BTreeSet::from([
                PluginPermission::ProcessSpawn,
                PluginPermission::EventSubscribe,
            ]),
        );
        assert!(matches!(
            ungranted.on_hook(
                &Hook::CommandExecuted {
                    region_id: 1,
                    tab_id: 1,
                    pane_id: None,
                    command: "must-not-leak".into(),
                    command_truncated: false,
                    output: "must-not-leak".into(),
                    output_truncated: false,
                    content_available: true,
                    exit_code: Some(0),
                },
                &PluginContext {
                    cwd: None,
                    tab_count: 1,
                    active_tab: 0,
                    terminal_size: (80, 24),
                },
            ),
            HookResponse::Ignored
        ));

        let mut v3 = manifest.clone();
        v3.api_version = 3;
        assert!(v3.validate().unwrap_err().to_string().contains("API v4"));
        let mut no_request = manifest;
        no_request
            .permissions
            .remove(&PluginPermission::TerminalText);
        assert!(
            no_request
                .validate()
                .unwrap_err()
                .to_string()
                .contains("terminal-text")
        );
    }

    #[test]
    fn api_v4_never_discloses_evicted_command_region_text() {
        let mut plugin =
            DeclarativePlugin::new(command_event_manifest(), None, command_event_grants());
        let HookResponse::Action(PluginAction::InvokeStructuredHandler { invocation }) = plugin
            .on_hook(
                &Hook::CommandExecuted {
                    region_id: 90,
                    tab_id: 2,
                    pane_id: None,
                    command: "stale-secret-command".into(),
                    command_truncated: false,
                    output: "stale-secret-output".into(),
                    output_truncated: false,
                    content_available: false,
                    exit_code: Some(0),
                },
                &PluginContext {
                    cwd: None,
                    tab_count: 1,
                    active_tab: 0,
                    terminal_size: (80, 24),
                },
            )
        else {
            panic!("subscribed command event did not create an invocation")
        };
        assert!(!invocation.event_json.contains("stale-secret"));
        let value: serde_json::Value = serde_json::from_str(&invocation.event_json).unwrap();
        assert!(!value["event"]["content_available"].as_bool().unwrap());
        assert_eq!(value["event"]["command"]["text"], "");
        assert_eq!(value["event"]["output"]["text"], "");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_installed_directory_never_becomes_a_runtime_plugin() {
        use std::os::unix::fs::symlink;

        let (_temp, source, root, registry_path) = fixture();
        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: process_grant(),
                enable: true,
                replace: false,
            },
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

    #[cfg(unix)]
    #[test]
    fn administrative_temporary_symlinks_are_never_followed() {
        use std::os::unix::fs::symlink;

        let (temp, source, root, registry_path) = fixture();
        let installed = root.join("sample-tools");
        fs::create_dir_all(&installed).unwrap();
        let sentinel = temp.path().join("sentinel");
        fs::write(&sentinel, "do-not-replace").unwrap();
        symlink(&sentinel, installed.join(format!("{MANIFEST_NAME}.tmp"))).unwrap();

        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source,
                grants: process_grant(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("non-regular temporary file"));
        assert_eq!(fs::read_to_string(&sentinel).unwrap(), "do-not-replace");
        assert!(!registry_path.exists());
        assert!(!installed.join(MANIFEST_NAME).exists());

        symlink(&sentinel, &registry_path).unwrap();
        let error = PluginRegistry::load_from(&registry_path).unwrap_err();
        assert!(format!("{error:#}").contains("non-symlink file"));
        assert_eq!(fs::read_to_string(&sentinel).unwrap(), "do-not-replace");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_plugin_root_is_rejected_before_installation() {
        use std::os::unix::fs::symlink;

        let (temp, source, root, registry_path) = fixture();
        let real_root = temp.path().join("real-installed");
        let linked_root = temp.path().join("linked-installed");
        fs::create_dir(&real_root).unwrap();
        symlink(&real_root, &linked_root).unwrap();

        let error = execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: process_grant(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &linked_root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("installation root"));
        assert!(fs::read_dir(real_root).unwrap().next().is_none());
        assert!(!registry_path.exists());

        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source,
                grants: process_grant(),
                enable: true,
                replace: false,
            },
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
    fn update_is_explicit_and_new_permissions_cannot_migrate_silently() {
        let (_temp, source, root, registry_path) = fixture();
        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: process_grant(),
                enable: true,
                replace: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest("1.1.0", "", "sample-v2"),
        )
        .unwrap();
        execute_admin_at(
            PluginAdminCommand::Update {
                id: "sample-tools".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();
        let shown = execute_admin_at(
            PluginAdminCommand::Show {
                id: "sample-tools".into(),
                json: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        assert!(shown.contains("v1.1.0"));

        execute_admin_at(
            PluginAdminCommand::Disable {
                id: "sample-tools".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap();
        fs::write(
            source.join(MANIFEST_NAME),
            manifest("2.0.0", ", \"terminal-text\"", "sample-v3"),
        )
        .unwrap();
        let error = execute_admin_at(
            PluginAdminCommand::Update {
                id: "sample-tools".into(),
            },
            &registry_path,
            &root,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("new permissions"));
        let shown = execute_admin_at(
            PluginAdminCommand::Show {
                id: "sample-tools".into(),
                json: false,
            },
            &registry_path,
            &root,
        )
        .unwrap();
        assert!(shown.contains("v1.1.0"));
    }

    #[test]
    fn api_v10_update_preserves_compatible_settings_and_rejects_schema_loss() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("installed");
        let registry_path = temp.path().join("plugins.toml");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join(MANIFEST_NAME), configurable_manifest("1.0.0")).unwrap();
        execute_admin_at(
            PluginAdminCommand::Install {
                directory: source.clone(),
                grants: event_grants(false),
                enable: true,
                replace: false,
            },
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
        let mut plugin = DeclarativePlugin::new(parsed, None, process_grant());
        let context = PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        let started = Instant::now();
        for _ in 0..10_000 {
            let response = plugin.on_hook(
                &Hook::CommandInvoked {
                    command_id: "sample-tools.open-default".into(),
                },
                &context,
            );
            assert!(matches!(
                response,
                HookResponse::Action(PluginAction::OpenTab { .. })
            ));
        }
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
