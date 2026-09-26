//! `[agents.<id>]` configuration, and migration from the retired
//! `[plugins.claude_code]` / `[plugins.codex]` sections.

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

/// Programs that start other programs. Launched through one of these, an
/// agent is recognized by its `detect` names; the launcher's own name would
/// claim every other program it starts.
const LAUNCHERS: &[&str] = &[
    "npx", "pnpx", "bunx", "uvx", "pipx", "docker", "podman", "node", "nodejs", "bun", "deno",
    "python", "python3", "env", "sh", "bash", "dash", "zsh", "fish",
];

/// The name `program` is recognized by, unless it is a launcher.
fn program_file_name(program: &str) -> Option<String> {
    std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !LAUNCHERS.contains(name))
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

/// One retired Codex approval or sandbox setting, and whether a profile set
/// it. A profile's own setting replaced the section's for that launch.
#[derive(Clone, Copy)]
struct CodexSetting<'a> {
    value: Option<&'a str>,
    from_profile: bool,
}

impl<'a> CodexSetting<'a> {
    fn section(value: Option<&'a str>) -> Self {
        Self {
            value,
            from_profile: false,
        }
    }

    /// The profile's own setting, else the section's.
    fn in_profile(profile: Option<&'a str>, section: Option<&'a str>) -> Self {
        match profile {
            Some(value) => Self {
                value: Some(value),
                from_profile: true,
            },
            None => Self::section(section),
        }
    }
}

/// Codex flags for the retired friendly names. Earlier releases passed the
/// defaults ("suggest", "workspace-write") explicitly even when nobody chose
/// them; from the section they are dropped, so a converted launch follows the
/// person's own Codex configuration. A profile's own setting is always
/// passed, because it replaced a section setting that may have been less
/// careful. Names the retired plugins did not know are refused, as they
/// refused to launch with them.
fn codex_policy_args(approval: CodexSetting, sandbox: CodexSetting) -> anyhow::Result<Vec<String>> {
    let policy = approval
        .value
        .map(|mode| match mode {
            "suggest" => Ok("untrusted"),
            "auto-edit" => Ok("on-request"),
            "full-auto" => Ok("never"),
            _ => Err(anyhow::anyhow!(
                "Codex approval_mode {mode:?} is not suggest, auto-edit, or full-auto"
            )),
        })
        .transpose()?
        .filter(|policy| approval.from_profile || *policy != "untrusted");
    let sandbox_mode = sandbox
        .value
        .map(|mode| match mode {
            "read-only" => Ok("read-only"),
            "workspace-write" => Ok("workspace-write"),
            "full-access" => Ok("danger-full-access"),
            _ => Err(anyhow::anyhow!(
                "Codex sandbox_mode {mode:?} is not read-only, workspace-write, or full-access"
            )),
        })
        .transpose()?
        .filter(|mode| sandbox.from_profile || *mode != "workspace-write" || policy.is_some());
    let mut args = Vec::new();
    if let Some(policy) = policy {
        args.extend(["--ask-for-approval".into(), policy.into()]);
    }
    if let Some(mode) = sandbox_mode {
        args.extend(["--sandbox".into(), mode.into()]);
    }
    Ok(args)
}

/// A retired section's arguments, profiles, and default profile.
struct MigratedLaunch {
    args: Vec<String>,
    profiles: BTreeMap<String, AgentProfileConfig>,
    default_profile: Option<String>,
}

/// Carry a retired section's launch settings over. There, a profile's
/// approval and sandbox settings replaced the section's own for that launch;
/// here, a profile's arguments are added to the agent's. So once profiles
/// exist, the policy arguments move into each profile, which gets the
/// section's policy for whatever it did not set itself, and the section's own
/// policy becomes a `default` profile unless another was the default.
fn migrate_launch(
    base_args: Vec<String>,
    section_policy: Vec<String>,
    profiles: &BTreeMap<String, LegacyProfile>,
    profile_policy: impl Fn(&LegacyProfile) -> anyhow::Result<Vec<String>>,
    default_profile: &Option<String>,
) -> anyhow::Result<MigratedLaunch> {
    // A default naming a missing profile fell back to no profile.
    let mut default_profile = default_profile
        .clone()
        .filter(|name| profiles.contains_key(name));
    if profiles.is_empty() {
        let mut args = base_args;
        args.extend(section_policy);
        return Ok(MigratedLaunch {
            args,
            profiles: BTreeMap::new(),
            default_profile,
        });
    }
    let mut migrated = BTreeMap::new();
    for (name, profile) in profiles {
        let mut args = profile_policy(profile)
            .map_err(|error| anyhow::anyhow!("profile {name:?}: {error}"))?;
        args.extend(profile.extra_args.iter().cloned());
        migrated.insert(
            name.clone(),
            AgentProfileConfig {
                description: profile.description.clone(),
                args,
                env: profile.env.clone(),
            },
        );
    }
    if default_profile.is_none() && !section_policy.is_empty() {
        let name = ["default".to_string(), "migrated-default".to_string()]
            .into_iter()
            .chain((2..).map(|number| format!("migrated-default-{number}")))
            .find(|name| !migrated.contains_key(name))
            .expect("some name is free");
        migrated.insert(
            name.clone(),
            AgentProfileConfig {
                description: "Approval settings from before profiles".into(),
                args: section_policy,
                env: BTreeMap::new(),
            },
        );
        default_profile = Some(name);
    }
    Ok(MigratedLaunch {
        args: base_args,
        profiles: migrated,
        default_profile,
    })
}

fn non_default_program(binary: &Option<String>, default: &str) -> Option<String> {
    binary
        .clone()
        .filter(|binary| !binary.is_empty() && binary != default)
}

fn base_args(model: &Option<String>, args: &[String]) -> Vec<String> {
    let mut base = model_args(model);
    base.extend(args.iter().cloned());
    base
}

/// The `[agents.claude-code]` entry equivalent to a retired section, or
/// `None` when it only restated defaults.
pub fn migrate_claude_code(legacy: &LegacyClaudeCodeConfig) -> Option<AgentConfig> {
    let section = legacy.approval_mode.as_deref();
    let launch = migrate_launch(
        base_args(&legacy.model, &legacy.args),
        claude_approval_args(section),
        &legacy.profiles,
        |profile| {
            Ok(claude_approval_args(
                profile.approval_mode.as_deref().or(section),
            ))
        },
        &legacy.default_profile,
    )
    .expect("Claude Code approval modes cannot be refused");
    let entry = AgentConfig {
        enabled: legacy.enabled.filter(|enabled| !enabled),
        program: non_default_program(&legacy.binary, "claude"),
        args: (!launch.args.is_empty()).then_some(launch.args),
        inherit_cwd: legacy.inherit_cwd.filter(|inherit| !inherit),
        profiles: launch.profiles,
        default_profile: launch.default_profile,
        ..AgentConfig::default()
    };
    (entry != AgentConfig::default()).then_some(entry)
}

/// The `[agents.codex]` entry equivalent to a retired section. Codex was
/// disabled by default, so `enabled = false` restated the default.
pub fn migrate_codex(legacy: &LegacyCodexConfig) -> anyhow::Result<Option<AgentConfig>> {
    let (approval, sandbox) = (
        legacy.approval_mode.as_deref(),
        legacy.sandbox_mode.as_deref(),
    );
    let launch = migrate_launch(
        base_args(&legacy.model, &legacy.args),
        codex_policy_args(
            CodexSetting::section(approval),
            CodexSetting::section(sandbox),
        )?,
        &legacy.profiles,
        |profile| {
            codex_policy_args(
                CodexSetting::in_profile(profile.approval_mode.as_deref(), approval),
                CodexSetting::in_profile(profile.sandbox_mode.as_deref(), sandbox),
            )
        },
        &legacy.default_profile,
    )
    .map_err(|error| anyhow::anyhow!("[plugins.codex] {error}"))?;
    let entry = AgentConfig {
        program: non_default_program(&legacy.binary, "codex"),
        args: (!launch.args.is_empty()).then_some(launch.args),
        inherit_cwd: legacy.inherit_cwd.filter(|inherit| !inherit),
        profiles: launch.profiles,
        default_profile: launch.default_profile,
        ..AgentConfig::default()
    };
    Ok((entry != AgentConfig::default()).then_some(entry))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentCatalog;

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
        let entry = migrate_codex(&codex).unwrap().unwrap();
        // With profiles, each launch gets exactly one policy: the profile's,
        // which replaces the section's, or the section's as `default`.
        assert_eq!(entry.args, None);
        assert_eq!(
            entry.profiles["safe"].args,
            ["--ask-for-approval", "untrusted", "--sandbox", "read-only"],
            "the profile's own careful choice is passed, never left to Codex's defaults"
        );
        assert_eq!(
            entry.profiles["default"].args,
            [
                "--ask-for-approval",
                "never",
                "--sandbox",
                "workspace-write"
            ]
        );
        assert_eq!(entry.default_profile.as_deref(), Some("default"));
        let agent = AgentCatalog::from_config(&BTreeMap::from([("codex".into(), entry)]));
        let codex_agent = agent.get("codex").unwrap();
        assert_eq!(
            codex_agent.launch(Some("safe"), false).unwrap().args,
            ["--ask-for-approval", "untrusted", "--sandbox", "read-only"],
            "the safe profile never inherits `never`"
        );
        assert_eq!(
            codex_agent.launch(None, false).unwrap().args,
            [
                "--ask-for-approval",
                "never",
                "--sandbox",
                "workspace-write"
            ]
        );

        // A profile that sets nothing inherits the section's policy, and a
        // default naming a missing profile is dropped.
        let partial: LegacyCodexConfig = toml::from_str(
            r#"
            approval_mode = "auto-edit"
            default_profile = "gone"
            [profiles.fast]
            extra_args = ["--search"]
            [profiles.default]
            description = "Mine"
            "#,
        )
        .unwrap();
        let entry = migrate_codex(&partial).unwrap().unwrap();
        assert_eq!(
            entry.profiles["fast"].args,
            ["--ask-for-approval", "on-request", "--search"]
        );
        assert_eq!(entry.profiles["default"].description, "Mine");
        assert_eq!(
            entry.profiles["migrated-default"].args,
            ["--ask-for-approval", "on-request"]
        );
        assert_eq!(entry.default_profile.as_deref(), Some("migrated-default"));
        validate(&BTreeMap::from([("codex".into(), entry)])).unwrap();

        // An explicit default in a profile still overrides a permissive
        // section, and names the retired plugin refused are refused.
        let careful: LegacyCodexConfig = toml::from_str(
            r#"
            approval_mode = "full-auto"
            sandbox_mode = "full-access"
            [profiles.careful]
            approval_mode = "suggest"
            sandbox_mode = "workspace-write"
            [profiles.default]
            [profiles.migrated-default]
            "#,
        )
        .unwrap();
        let entry = migrate_codex(&careful).unwrap().unwrap();
        assert_eq!(
            entry.profiles["careful"].args,
            [
                "--ask-for-approval",
                "untrusted",
                "--sandbox",
                "workspace-write"
            ]
        );
        assert_eq!(
            entry.default_profile.as_deref(),
            Some("migrated-default-2"),
            "the person's own profiles are never replaced"
        );
        let unknown: LegacyCodexConfig =
            toml::from_str("[profiles.x]\napproval_mode = \"yolo\"").unwrap();
        assert!(
            migrate_codex(&unknown)
                .unwrap_err()
                .to_string()
                .contains("\"yolo\"")
        );

        let claude: LegacyClaudeCodeConfig = toml::from_str(
            r#"
            approval_mode = "auto-accept"
            default_profile = "careful"
            [profiles.careful]
            approval_mode = "default"
            "#,
        )
        .unwrap();
        let entry = migrate_claude_code(&claude).unwrap();
        assert!(entry.profiles["careful"].args.is_empty());
        assert_eq!(entry.default_profile.as_deref(), Some("careful"));
        assert_eq!(entry.args, None);

        // Sections that only restate old defaults leave no entry behind.
        let defaults: LegacyCodexConfig = toml::from_str(
            r#"
            enabled = false
            approval_mode = "suggest"
            sandbox_mode = "workspace-write"
            "#,
        )
        .unwrap();
        assert_eq!(migrate_codex(&defaults).unwrap(), None);
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
