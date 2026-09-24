//! Coding agents — AI command-line tools such as Claude Code or Codex that run
//! in a terminal pane.
//!
//! Every agent is described by data: how to recognize its process, how to
//! launch or resume it, and which in-conversation command starts over. Termeric
//! ships definitions for the agents it knows and merges `[agents.<id>]`
//! entries from the configuration on top, so supporting another agent is a
//! configuration entry rather than new code.

pub mod changes;
pub mod config;
pub mod detect;
pub mod hook;
pub mod session;
pub mod setup;
pub mod status;
pub mod supervisor;
pub mod transcript;

use std::collections::BTreeMap;
use std::path::PathBuf;

pub use config::AgentConfig;
pub use session::AgentSession;
pub use status::{AgentActivity, AgentBadge, AgentReport, AgentStatus};

/// A named launch variant: extra arguments and environment for one agent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentProfile {
    pub description: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// Everything Termeric needs to recognize, launch, and steer one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDefinition {
    /// Stable identity used in configuration, workspaces, and commands.
    pub id: String,
    /// Display name, e.g. "Claude Code".
    pub name: String,
    /// Program names that identify this agent in a pane's foreground process.
    pub detect: Vec<String>,
    /// npm packages it is installed from, recognized when an interpreter
    /// runs a file inside one.
    pub packages: Vec<String>,
    /// Executable to launch.
    pub program: String,
    /// Arguments passed on every launch.
    pub args: Vec<String>,
    /// Arguments that continue the most recent conversation, when supported.
    pub resume_args: Option<Vec<String>>,
    /// Text typed into the agent to start a new conversation, when supported.
    pub new_conversation: Option<String>,
    /// Start in the focused pane's working directory.
    pub inherit_cwd: bool,
    /// Environment added on every launch.
    pub env: BTreeMap<String, String>,
    pub profiles: BTreeMap<String, AgentProfile>,
    /// Profile used when a launch names none.
    pub default_profile: Option<String>,
    /// Disabled agents are neither offered nor detected.
    pub enabled: bool,
}

/// A definition Termeric ships.
struct Builtin {
    id: &'static str,
    name: &'static str,
    program: &'static str,
    /// The npm package it is installed from, recognized when a shim runs
    /// the package's entry file directly.
    package: Option<&'static str>,
    resume_args: Option<&'static [&'static str]>,
    new_conversation: Option<&'static str>,
}

/// The agents Termeric knows about, in display order.
const BUILTINS: [Builtin; 4] = [
    Builtin {
        id: "claude-code",
        name: "Claude Code",
        program: "claude",
        package: Some("@anthropic-ai/claude-code"),
        resume_args: Some(&["--continue"]),
        new_conversation: Some("/clear"),
    },
    Builtin {
        id: "codex",
        name: "Codex",
        program: "codex",
        package: Some("@openai/codex"),
        resume_args: Some(&["resume", "--last"]),
        new_conversation: Some("/new"),
    },
    Builtin {
        id: "gemini",
        name: "Gemini CLI",
        program: "gemini",
        package: Some("@google/gemini-cli"),
        resume_args: Some(&["--resume", "latest"]),
        new_conversation: Some("/clear"),
    },
    Builtin {
        id: "opencode",
        name: "OpenCode",
        program: "opencode",
        package: Some("opencode-ai"),
        resume_args: Some(&["--continue"]),
        new_conversation: Some("/new"),
    },
];

/// Each built-in agent's id and name, in display order.
pub fn builtin_agent_names() -> impl Iterator<Item = (&'static str, &'static str)> {
    BUILTINS.iter().map(|builtin| (builtin.id, builtin.name))
}

impl AgentDefinition {
    fn builtin(builtin: &Builtin) -> Self {
        Self {
            id: builtin.id.into(),
            name: builtin.name.into(),
            detect: vec![builtin.program.into()],
            packages: builtin.package.into_iter().map(Into::into).collect(),
            program: builtin.program.into(),
            args: Vec::new(),
            resume_args: builtin
                .resume_args
                .map(|args| args.iter().map(|arg| (*arg).into()).collect()),
            new_conversation: builtin.new_conversation.map(Into::into),
            inherit_cwd: true,
            env: BTreeMap::new(),
            profiles: BTreeMap::new(),
            default_profile: None,
            enabled: true,
        }
    }

    /// Arguments and environment for one launch. `profile` names a
    /// configured profile; `None` uses the default profile, if any. An unknown
    /// name is an error rather than a silent fallback.
    pub fn launch(&self, profile: Option<&str>, resume: bool) -> Result<AgentLaunch, String> {
        let profile_name = profile
            .map(str::to_owned)
            .or_else(|| self.default_profile.clone());
        let profile = match &profile_name {
            Some(name) => Some(
                self.profiles
                    .get(name)
                    .ok_or_else(|| format!("{} has no profile named {name:?}", self.name))?,
            ),
            None => None,
        };
        let mut args = self.args.clone();
        if let Some(profile) = profile {
            args.extend(profile.args.iter().cloned());
        }
        if resume {
            let resume_args = self
                .resume_args
                .as_ref()
                .ok_or_else(|| format!("{} cannot resume a conversation", self.name))?;
            args.extend(resume_args.iter().cloned());
        }
        let mut env = self.env.clone();
        if let Some(profile) = profile {
            env.extend(profile.env.clone());
        }
        Ok(AgentLaunch {
            program: self.program.clone(),
            args,
            env,
            profile: profile_name,
        })
    }

    /// The installed executable, found the way a shell would find it.
    pub fn installed_path(&self) -> Option<PathBuf> {
        find_executable(&self.program)
    }
}

/// One resolved launch of an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLaunch {
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// The profile applied, if any.
    pub profile: Option<String>,
}

fn builtin_agents() -> Vec<AgentDefinition> {
    BUILTINS.iter().map(AgentDefinition::builtin).collect()
}

/// Built-in agents with configuration applied, plus configured custom agents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCatalog {
    agents: Vec<AgentDefinition>,
}

impl Default for AgentCatalog {
    fn default() -> Self {
        Self::from_config(&BTreeMap::new())
    }
}

impl AgentCatalog {
    /// Merge `[agents.<id>]` entries onto the built-in definitions. An entry
    /// whose id is not built in defines a custom agent.
    pub fn from_config(configured: &BTreeMap<String, AgentConfig>) -> Self {
        let mut agents = builtin_agents();
        for (id, entry) in configured {
            match agents.iter_mut().find(|agent| agent.id == *id) {
                Some(agent) => entry.apply_to(agent),
                None => agents.push(entry.to_custom(id)),
            }
        }
        Self { agents }
    }

    /// Every known agent, enabled or not.
    pub fn all(&self) -> &[AgentDefinition] {
        &self.agents
    }

    /// Enabled agents, in display order.
    pub fn enabled(&self) -> impl Iterator<Item = &AgentDefinition> {
        self.agents.iter().filter(|agent| agent.enabled)
    }

    pub fn get(&self, id: &str) -> Option<&AgentDefinition> {
        self.agents.iter().find(|agent| agent.id == id)
    }

    /// The enabled agent whose process this is, from its argument vector.
    pub fn identify(&self, argv: &[String]) -> Option<&AgentDefinition> {
        let by_name = detect::program_name(argv)
            .and_then(|name| self.enabled().find(|agent| agent.detect.contains(&name)));
        by_name.or_else(|| {
            let package = detect::package_name(argv)?;
            self.enabled()
                .find(|agent| agent.packages.contains(&package))
        })
    }
}

/// Resolve `program` through `PATH` unless it already names a path.
pub fn find_executable(program: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let is_executable = |path: &std::path::Path| {
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    };
    if program.contains('/') {
        let path = PathBuf::from(program);
        return is_executable(&path).then_some(path);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(program))
            .find(|candidate| is_executable(candidate))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn builtin_agents_know_how_to_resume_and_start_over() {
        let catalog = AgentCatalog::default();
        let ids: Vec<_> = catalog
            .all()
            .iter()
            .map(|agent| agent.id.as_str())
            .collect();
        assert_eq!(ids, ["claude-code", "codex", "gemini", "opencode"]);
        let codex = catalog.get("codex").unwrap();
        assert_eq!(codex.launch(None, true).unwrap().args, ["resume", "--last"]);
        assert_eq!(codex.new_conversation.as_deref(), Some("/new"));
        assert_eq!(
            catalog
                .get("claude-code")
                .unwrap()
                .new_conversation
                .as_deref(),
            Some("/clear")
        );
    }

    #[test]
    fn configuration_overrides_builtins_and_defines_custom_agents() {
        let configured: BTreeMap<String, AgentConfig> = toml::from_str(
            r#"
            [claude-code]
            args = ["--dangerously-skip-permissions"]
            [claude-code.profiles.review]
            args = ["--model", "opus"]
            env = { REVIEW = "1" }

            [gemini]
            enabled = false

            [aider]
            name = "Aider"
            program = "/opt/aider/bin/aider"
            new_conversation = "/clear"
            "#,
        )
        .unwrap();
        let catalog = AgentCatalog::from_config(&configured);
        let claude = catalog.get("claude-code").unwrap();
        let launch = claude.launch(Some("review"), true).unwrap();
        assert_eq!(
            launch.args,
            strings(&[
                "--dangerously-skip-permissions",
                "--model",
                "opus",
                "--continue"
            ])
        );
        assert_eq!(launch.env.get("REVIEW").map(String::as_str), Some("1"));
        assert_eq!(launch.profile.as_deref(), Some("review"));
        assert!(claude.launch(Some("missing"), false).is_err());

        assert!(!catalog.get("gemini").unwrap().enabled);
        let aider = catalog.get("aider").unwrap();
        assert_eq!(aider.name, "Aider");
        assert_eq!(aider.detect, ["aider"]);
        assert!(aider.launch(None, true).is_err(), "no resume arguments");
        assert_eq!(
            catalog
                .enabled()
                .map(|agent| agent.id.as_str())
                .collect::<Vec<_>>(),
            ["claude-code", "codex", "opencode", "aider"]
        );
    }

    #[test]
    fn processes_are_identified_by_invoked_name_including_node_scripts() {
        let catalog = AgentCatalog::default();
        let id = |argv: &[&str]| {
            catalog
                .identify(&strings(argv))
                .map(|agent| agent.id.clone())
        };
        assert_eq!(id(&["claude"]).as_deref(), Some("claude-code"));
        // Native installs often resolve to a version-numbered file; the
        // invoked name is what identifies the agent.
        assert_eq!(
            id(&["/home/me/.local/bin/claude", "--continue"]).as_deref(),
            Some("claude-code")
        );
        assert_eq!(
            id(&["node", "/home/me/.npm-global/bin/gemini"]).as_deref(),
            Some("gemini")
        );
        assert_eq!(
            id(&[
                "/usr/bin/node",
                "--no-warnings",
                "/usr/lib/node_modules/@google/gemini-cli/bundle/gemini.js"
            ])
            .as_deref(),
            Some("gemini")
        );
        assert_eq!(id(&["bash"]), None);
        assert_eq!(id(&["node", "server.js"]), None);
        // pnpm and Volta shims run the package's entry file directly.
        assert_eq!(
            id(&[
                "node",
                "/home/me/.volta/tools/image/packages/@google/gemini-cli/lib/node_modules/@google/gemini-cli/dist/index.js"
            ])
            .as_deref(),
            Some("gemini")
        );
        // A package's name is compared only with packages, not commands.
        assert_eq!(id(&["node", "/srv/node_modules/codex/lib/main.js"]), None);
        // A source file of the same name outside an installed package is
        // someone's own script.
        assert_eq!(id(&["node", "/home/me/src/gemini.js"]), None);
        assert_eq!(id(&[]), None);
    }
}
