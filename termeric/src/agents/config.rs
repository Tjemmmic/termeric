//! `[agents.<id>]` configuration, and migration from the retired
//! `[plugins.claude_code]` / `[plugins.codex]` sections.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{AgentDefinition, AgentProfile};

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

fn program_file_name(program: &str) -> Option<String> {
    std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
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

// ── Retired plugin sections ─────────────────────────────────────────────

/// `[plugins.claude_code]` as written by earlier releases. Read once and
/// migrated into `[agents.claude-code]`; never written back.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LegacyClaudeCodeConfig {
    pub enabled: Option<bool>,
    pub binary: Option<String>,
    pub model: Option<String>,
    pub inherit_cwd: Option<bool>,
    pub args: Vec<String>,
    pub profiles: BTreeMap<String, LegacyProfile>,
    pub default_profile: Option<String>,
    pub approval_mode: Option<String>,
}

/// `[plugins.codex]` as written by earlier releases.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LegacyCodexConfig {
    pub enabled: Option<bool>,
    pub binary: Option<String>,
    pub model: Option<String>,
    pub inherit_cwd: Option<bool>,
    pub args: Vec<String>,
    pub approval_mode: Option<String>,
    pub sandbox_mode: Option<String>,
    pub profiles: BTreeMap<String, LegacyProfile>,
    pub default_profile: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LegacyProfile {
    pub description: String,
    pub env: BTreeMap<String, String>,
    pub extra_args: Vec<String>,
    pub approval_mode: Option<String>,
    pub sandbox_mode: Option<String>,
}

fn model_args(model: &Option<String>) -> Vec<String> {
    model
        .iter()
        .filter(|model| !model.is_empty())
        .flat_map(|model| ["--model".to_string(), model.clone()])
        .collect()
}

fn claude_approval_args(mode: Option<&str>) -> Vec<String> {
    match mode {
        Some("auto-accept") => vec!["--dangerously-skip-permissions".into()],
        _ => Vec::new(),
    }
}

/// Codex flags for the retired friendly names. Earlier releases passed the
/// defaults ("suggest", "workspace-write") explicitly; they are dropped so a
/// migrated launch follows the user's own Codex configuration.
fn codex_policy_args(approval: Option<&str>, sandbox: Option<&str>) -> Vec<String> {
    let mut args = Vec::new();
    let policy = match approval {
        Some("auto-edit") => Some("on-request"),
        Some("full-auto") => Some("never"),
        _ => None,
    };
    let sandbox = match sandbox {
        Some("read-only") => Some("read-only"),
        Some("full-access") => Some("danger-full-access"),
        Some("workspace-write") if policy.is_some() => Some("workspace-write"),
        _ => None,
    };
    if let Some(policy) = policy {
        args.extend(["--ask-for-approval".into(), policy.into()]);
    }
    if let Some(sandbox) = sandbox {
        args.extend(["--sandbox".into(), sandbox.into()]);
    }
    args
}

fn migrate_profiles(
    profiles: &BTreeMap<String, LegacyProfile>,
    policy: impl Fn(&LegacyProfile) -> Vec<String>,
) -> BTreeMap<String, AgentProfileConfig> {
    profiles
        .iter()
        .map(|(name, profile)| {
            let mut args = policy(profile);
            args.extend(profile.extra_args.iter().cloned());
            (
                name.clone(),
                AgentProfileConfig {
                    description: profile.description.clone(),
                    args,
                    env: profile.env.clone(),
                },
            )
        })
        .collect()
}

fn non_default_program(binary: &Option<String>, default: &str) -> Option<String> {
    binary
        .clone()
        .filter(|binary| !binary.is_empty() && binary != default)
}

/// The `[agents.claude-code]` entry equivalent to a retired section, or
/// `None` when it only restated defaults.
pub fn migrate_claude_code(legacy: &LegacyClaudeCodeConfig) -> Option<AgentConfig> {
    let mut args = model_args(&legacy.model);
    args.extend(legacy.args.iter().cloned());
    args.extend(claude_approval_args(legacy.approval_mode.as_deref()));
    let entry = AgentConfig {
        enabled: legacy.enabled.filter(|enabled| !enabled),
        program: non_default_program(&legacy.binary, "claude"),
        args: (!args.is_empty()).then_some(args),
        inherit_cwd: legacy.inherit_cwd.filter(|inherit| !inherit),
        profiles: migrate_profiles(&legacy.profiles, |profile| {
            claude_approval_args(profile.approval_mode.as_deref())
        }),
        default_profile: legacy.default_profile.clone(),
        ..AgentConfig::default()
    };
    (entry != AgentConfig::default()).then_some(entry)
}

/// The `[agents.codex]` entry equivalent to a retired section. Codex was
/// disabled by default, so `enabled = false` restated the default.
pub fn migrate_codex(legacy: &LegacyCodexConfig) -> Option<AgentConfig> {
    let mut args = model_args(&legacy.model);
    args.extend(legacy.args.iter().cloned());
    args.extend(codex_policy_args(
        legacy.approval_mode.as_deref(),
        legacy.sandbox_mode.as_deref(),
    ));
    let entry = AgentConfig {
        program: non_default_program(&legacy.binary, "codex"),
        args: (!args.is_empty()).then_some(args),
        inherit_cwd: legacy.inherit_cwd.filter(|inherit| !inherit),
        profiles: migrate_profiles(&legacy.profiles, |profile| {
            codex_policy_args(
                profile.approval_mode.as_deref(),
                profile.sandbox_mode.as_deref(),
            )
        }),
        default_profile: legacy.default_profile.clone(),
        ..AgentConfig::default()
    };
    (entry != AgentConfig::default()).then_some(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_sections_keep_only_what_differed_from_their_defaults() {
        let claude: LegacyClaudeCodeConfig = toml::from_str(
            r#"
            enabled = true
            binary = "claude"
            inherit_cwd = true
            args = []
            approval_mode = "auto-accept"
            copy_lines = 50
            [profiles]
            "#,
        )
        .unwrap();
        let entry = migrate_claude_code(&claude).unwrap();
        assert_eq!(
            entry.args.as_deref(),
            Some(&["--dangerously-skip-permissions".to_string()][..])
        );
        assert_eq!(entry.enabled, None);
        assert_eq!(entry.program, None);

        let codex: LegacyCodexConfig = toml::from_str(
            r#"
            enabled = true
            binary = "codex"
            approval_mode = "full-auto"
            sandbox_mode = "workspace-write"
            [profiles.safe]
            description = "Read only"
            approval_mode = "suggest"
            sandbox_mode = "read-only"
            "#,
        )
        .unwrap();
        let entry = migrate_codex(&codex).unwrap();
        assert_eq!(
            entry.args.unwrap(),
            [
                "--ask-for-approval",
                "never",
                "--sandbox",
                "workspace-write"
            ]
        );
        assert_eq!(entry.profiles["safe"].args, ["--sandbox", "read-only"]);

        // Sections that only restate old defaults leave no entry behind.
        let defaults: LegacyCodexConfig = toml::from_str(
            r#"
            enabled = false
            approval_mode = "suggest"
            sandbox_mode = "workspace-write"
            "#,
        )
        .unwrap();
        assert_eq!(migrate_codex(&defaults), None);
        assert_eq!(
            migrate_claude_code(&LegacyClaudeCodeConfig::default()),
            None
        );
    }

    #[test]
    fn validation_rejects_unlaunchable_entries() {
        let parse = |text: &str| -> BTreeMap<String, AgentConfig> { toml::from_str(text).unwrap() };
        assert!(validate(&parse("[aider]\nprogram = \"aider\"")).is_ok());
        assert!(validate(&parse("[aider]\nprogram = \"\"")).is_err());
        assert!(validate(&parse("[\"bad id\"]")).is_err());
        assert!(validate(&parse("[codex]\ndefault_profile = \"nope\"")).is_err());
        assert!(toml::from_str::<BTreeMap<String, AgentConfig>>("[x]\nunknown = 1").is_err());
    }
}
