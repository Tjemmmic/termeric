//! `[agents.<id>]`, `[agent_status]`, and `[agent_view]` configuration.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{AgentDefinition, AgentProfile};

/// `[agent_status]`: how agents that report through hooks are presented.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentStatusConfig {
    /// Send a desktop notification when an agent the person is not looking
    /// at needs them, finishes, or fails.
    pub notify: bool,
}

impl Default for AgentStatusConfig {
    fn default() -> Self {
        Self { notify: true }
    }
}

/// `[agent_view]`: the docked conversation panel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentViewConfig {
    /// Offer each agent's mode that runs every action without asking, such
    /// as Claude Code's Bypass permissions. Off unless the person opts in.
    pub allow_bypass_permissions: bool,
    /// Where **New Agent…** opens an agent the agent view can drive.
    pub open_agents_in: OpenAgentsIn,
}

/// Where **New Agent…** opens an agent the agent view can drive.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpenAgentsIn {
    /// The agent's own interface in a terminal tab.
    #[default]
    Terminal,
    /// A new tab with the agent view beside its terminal.
    Split,
    /// A new tab with the agent view filling it.
    Full,
}

impl OpenAgentsIn {
    pub const ALL: [OpenAgentsIn; 3] = [
        OpenAgentsIn::Terminal,
        OpenAgentsIn::Split,
        OpenAgentsIn::Full,
    ];

    pub fn label(self) -> &'static str {
        match self {
            OpenAgentsIn::Terminal => "Terminal",
            OpenAgentsIn::Split => "Agent View Beside Terminal",
            OpenAgentsIn::Full => "Agent View Filling Tab",
        }
    }
}

/// One `[agents.<id>]` entry. For a built-in agent every field is an
/// override; for any other id the entry defines a custom agent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Executable to launch; also detected by its file name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    /// Extra program names to recognize, e.g. a wrapper script.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub detect: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_conversation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inherit_cwd: Option<bool>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, AgentProfileConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentProfileConfig {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl AgentProfileConfig {
    fn to_profile(&self) -> AgentProfile {
        AgentProfile {
            description: self.description.clone(),
            args: self.args.clone(),
            env: self.env.clone(),
        }
    }
}

impl AgentConfig {
    pub(super) fn apply_to(&self, agent: &mut AgentDefinition) {
        if let Some(enabled) = self.enabled {
            agent.enabled = enabled;
        }
        if let Some(name) = &self.name {
            agent.name = name.clone();
        }
        if let Some(program) = &self.program {
            agent.program = program.clone();
            push_unique(&mut agent.detect, program_file_name(program));
        }
        for name in &self.detect {
            push_unique(&mut agent.detect, Some(name.clone()));
        }
        if let Some(args) = &self.args {
            agent.args = args.clone();
        }
        if let Some(resume_args) = &self.resume_args {
            agent.resume_args = Some(resume_args.clone());
        }
        if let Some(text) = &self.new_conversation {
            agent.new_conversation = (!text.is_empty()).then(|| text.clone());
        }
        if let Some(inherit_cwd) = self.inherit_cwd {
            agent.inherit_cwd = inherit_cwd;
        }
        agent.env.extend(self.env.clone());
        for (name, profile) in &self.profiles {
            agent.profiles.insert(name.clone(), profile.to_profile());
        }
        if self.default_profile.is_some() {
            agent.default_profile = self.default_profile.clone();
        }
    }

    pub(super) fn to_custom(&self, id: &str) -> AgentDefinition {
        let program = self.program.clone().unwrap_or_else(|| id.to_string());
        let mut agent = AgentDefinition {
            id: id.to_string(),
            name: self.name.clone().unwrap_or_else(|| id.to_string()),
            detect: Vec::new(),
            packages: Vec::new(),
            program: program.clone(),
            args: Vec::new(),
            resume_args: None,
            new_conversation: None,
            inherit_cwd: true,
            env: BTreeMap::new(),
            profiles: BTreeMap::new(),
            default_profile: None,
            enabled: true,
        };
        push_unique(&mut agent.detect, program_file_name(&program));
        self.apply_to(&mut agent);
        agent
    }
}

/// Programs other than script interpreters that start other programs:
/// package runners, container tools, `env`, and fish.
const RUNNERS: &[&str] = &[
    "npx", "pnpx", "bunx", "uvx", "pipx", "docker", "podman", "env", "fish",
];

/// The name `program` is recognized by, unless it is a launcher (a script
/// interpreter or a runner). Launched through one of these, an agent is
/// recognized by its `detect` names; the launcher's own name would claim
/// every other program it starts.
fn program_file_name(program: &str) -> Option<String> {
    std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !super::detect::INTERPRETERS.contains(name) && !RUNNERS.contains(name))
        .map(str::to_owned)
}

fn push_unique(list: &mut Vec<String>, value: Option<String>) {
    if let Some(value) = value.filter(|value| !value.is_empty())
        && !list.contains(&value)
    {
        list.push(value);
    }
}

/// Reject agent configuration that could not launch or be recognized.
pub fn validate(agents: &BTreeMap<String, AgentConfig>) -> anyhow::Result<()> {
    for (id, entry) in agents {
        if id.is_empty()
            || id.len() > 64
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            anyhow::bail!("agents: {id:?} must be 1-64 letters, digits, '-' or '_'");
        }
        if entry.program.as_deref().is_some_and(str::is_empty) {
            anyhow::bail!("agents.{id}.program must not be empty");
        }
        if let Some(profile) = &entry.default_profile
            && !entry.profiles.contains_key(profile)
        {
            anyhow::bail!("agents.{id}.default_profile {profile:?} is not a configured profile");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentCatalog;

    #[test]
    fn validation_rejects_unlaunchable_entries() {
        let parse = |text: &str| -> BTreeMap<String, AgentConfig> { toml::from_str(text).unwrap() };
        assert!(validate(&parse("[aider]\nprogram = \"aider\"")).is_ok());
        assert!(validate(&parse("[aider]\nprogram = \"\"")).is_err());
        assert!(validate(&parse("[\"bad id\"]")).is_err());
        assert!(validate(&parse("[codex]\ndefault_profile = \"nope\"")).is_err());
        assert!(toml::from_str::<BTreeMap<String, AgentConfig>>("[x]\nunknown = 1").is_err());
    }

    #[test]
    fn a_launcher_program_is_not_a_name_to_recognize() {
        let parse = |text: &str| -> BTreeMap<String, AgentConfig> { toml::from_str(text).unwrap() };
        let catalog = AgentCatalog::from_config(&parse(
            "[crush]\nprogram = \"/usr/bin/npx\"\nargs = [\"@charmland/crush\"]\ndetect = [\"crush\"]\n\
             [aider]\nprogram = \"/opt/aider/bin/aider\"",
        ));
        assert_eq!(catalog.get("crush").unwrap().detect, ["crush"]);
        assert_eq!(catalog.get("aider").unwrap().detect, ["aider"]);
        let argv = |args: &[&str]| {
            args.iter()
                .map(|arg| (*arg).to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            catalog.identify(&argv(&["node", "/usr/bin/npx", "cowsay"])),
            None
        );
    }
}
