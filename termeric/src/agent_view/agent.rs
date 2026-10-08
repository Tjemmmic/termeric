//! The agents the agent view can drive, and what the panel knows about each
//! before the agent reports for itself.

use std::path::PathBuf;

use super::acp::{self, AcpCodec};
use super::claude::{self, ClaudeCodec};
use super::codex::{self, CodexCodec};
use super::event::{DEFAULT_MODEL, ModeChoice, ModelChoice, model_name};
use super::protocol::Protocol;

/// An agent the agent view drives, and so the protocol it speaks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AgentKind {
    /// Claude Code over its stream-json protocol.
    #[default]
    ClaudeCode,
    /// Codex over its app-server protocol.
    Codex,
    /// OpenCode over the Agent Client Protocol.
    OpenCode,
}

/// How one agent process starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub cwd: PathBuf,
    /// The id of the mode to start in, one of the agent's.
    pub mode: String,
    /// A model the agent offered; `None` keeps its own default.
    pub model: Option<String>,
    /// A level of effort the agent offered; `None` keeps its own.
    pub effort: Option<String>,
    /// Allow everything the agent asks about, for agents that offer it
    /// apart from their modes.
    pub auto_approve: bool,
    /// Continue an earlier conversation by the agent's id for it.
    pub resume: Option<String>,
    /// The person opted in to modes that skip every check.
    pub allow_bypass: bool,
}

impl AgentKind {
    /// Every agent the agent view can drive, in the order it offers them.
    pub const ALL: [AgentKind; 3] = [AgentKind::ClaudeCode, AgentKind::Codex, AgentKind::OpenCode];

    /// Its id in the agent catalog, `[agents.<id>]`.
    pub fn catalog_id(self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "claude-code",
            AgentKind::Codex => "codex",
            AgentKind::OpenCode => "opencode",
        }
    }

    /// The agent for a catalog id, when the agent view drives it.
    pub fn from_catalog_id(id: &str) -> Option<AgentKind> {
        AgentKind::ALL
            .into_iter()
            .find(|kind| kind.catalog_id() == id)
    }

    pub fn name(self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "Claude Code",
            AgentKind::Codex => "Codex",
            AgentKind::OpenCode => "OpenCode",
        }
    }

    /// How the conversation refers to the agent, e.g. "Ask Claude …".
    pub fn short_name(self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "Claude",
            AgentKind::Codex => "Codex",
            AgentKind::OpenCode => "OpenCode",
        }
    }

    /// The modes it runs in, until it reports its own.
    pub fn modes(self) -> Vec<ModeChoice> {
        match self {
            AgentKind::ClaudeCode => claude::modes(),
            AgentKind::Codex => codex::modes(),
            AgentKind::OpenCode => opencode_modes(),
        }
    }

    /// The mode it starts in unless the person picks another: the one that
    /// asks before changing anything.
    pub fn default_mode(self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => claude::ASK_MODE,
            AgentKind::Codex => codex::ASK_MODE,
            // OpenCode asks only where its own permission settings say to,
            // in any mode.
            AgentKind::OpenCode => OPENCODE_BUILD_MODE,
        }
    }

    /// Whether the agent must be started allowing a mode that skips every
    /// check before it can switch to one: Claude Code refuses
    /// `bypassPermissions` unless launched with
    /// `--allow-dangerously-skip-permissions`.
    pub fn unrestricted_needs_launch_allowance(self) -> bool {
        matches!(self, AgentKind::ClaudeCode)
    }

    /// Whether the agent's own interface can approve everything it would
    /// ask about as a switch of its own, beside its modes, as OpenCode's
    /// auto-approve does. Other agents approve everything through a mode.
    pub fn offers_auto_approve(self) -> bool {
        matches!(self, AgentKind::OpenCode)
    }

    /// Arguments that delete conversation `id` from the agent's history, for
    /// agents that keep a conversation as soon as it opens: the agent view
    /// opens one ahead of the first message, and one never used is forgotten.
    pub fn forget_args(self, id: &str) -> Option<Vec<String>> {
        match self {
            AgentKind::OpenCode => Some(vec![
                "session".to_owned(),
                "delete".to_owned(),
                id.to_owned(),
            ]),
            // Claude Code and Codex keep nothing before the first message.
            AgentKind::ClaudeCode | AgentKind::Codex => None,
        }
    }

    /// Models offered before the agent has listed its own.
    pub fn fallback_models(self) -> Vec<ModelChoice> {
        let values: &[&str] = match self {
            AgentKind::ClaudeCode => &[DEFAULT_MODEL, "sonnet", "opus", "haiku"],
            AgentKind::Codex | AgentKind::OpenCode => &[DEFAULT_MODEL],
        };
        values
            .iter()
            .map(|value| ModelChoice {
                value: (*value).to_owned(),
                display_name: model_name(&[], value),
                description: String::new(),
                group: None,
            })
            .collect()
    }

    /// Arguments that continue conversation `id` in the agent's own
    /// terminal interface.
    pub fn resume_args(self, id: &str) -> Vec<String> {
        match self {
            AgentKind::ClaudeCode => vec!["--resume".to_owned(), id.to_owned()],
            AgentKind::Codex => vec!["resume".to_owned(), id.to_owned()],
            AgentKind::OpenCode => vec!["--session".to_owned(), id.to_owned()],
        }
    }

    /// The protocol arguments for one process, and the protocol to speak
    /// with it.
    pub fn connect(self, launch: &Launch) -> (Vec<String>, Box<dyn Protocol>) {
        match self {
            AgentKind::ClaudeCode => (
                claude::launch_args(&claude::LaunchOptions {
                    mode: launch.mode.clone(),
                    model: launch.model.clone(),
                    resume: launch.resume.clone(),
                    allow_bypass: launch.allow_bypass,
                }),
                Box::new(ClaudeCodec::new()),
            ),
            // Codex needs no allowance to switch to full access: the panel
            // offers that mode only after the person opted in.
            AgentKind::Codex => (
                codex::launch_args(),
                Box::new(CodexCodec::new(codex::Setup {
                    cwd: launch.cwd.clone(),
                    mode: launch.mode.clone(),
                    // Codex takes no model named "default"; naming none
                    // keeps its own.
                    model: launch.model.clone().filter(|model| model != DEFAULT_MODEL),
                    resume: launch.resume.clone(),
                })),
            ),
            AgentKind::OpenCode => (
                vec!["acp".to_owned()],
                Box::new(AcpCodec::new(acp::Setup {
                    agent: self.name(),
                    cwd: launch.cwd.clone(),
                    mode: launch.mode.clone(),
                    model: launch.model.clone().filter(|model| model != DEFAULT_MODEL),
                    effort: launch.effort.clone(),
                    auto_approve: launch.auto_approve,
                    resume: launch.resume.clone(),
                })),
            ),
        }
    }
}

/// OpenCode's mode that works on the task: its default agent.
const OPENCODE_BUILD_MODE: &str = "build";

/// OpenCode's built-in modes, until a session lists its own, which include
/// any agents the person configured.
fn opencode_modes() -> Vec<ModeChoice> {
    vec![
        ModeChoice::new(
            OPENCODE_BUILD_MODE,
            "Build",
            "Works on the task, asking where your OpenCode permission settings say to",
        ),
        ModeChoice::new("plan", "Plan", "Reads and plans without editing anything").plans(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The agent view looks each agent up in the catalog without a fallback,
    /// so every one must be a built-in agent, under the same name.
    #[test]
    fn every_agent_is_a_built_in_catalog_agent() {
        let builtins: Vec<_> = crate::agents::builtin_agent_names().collect();
        for kind in AgentKind::ALL {
            assert!(
                builtins.contains(&(kind.catalog_id(), kind.name())),
                "{kind:?}"
            );
        }
    }
}
