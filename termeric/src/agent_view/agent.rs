//! The agents the agent view can drive, and what the panel knows about each
//! before the agent reports for itself.

use std::path::PathBuf;

use super::claude::{self, ClaudeCodec};
use super::codex::{self, CodexCodec};
use super::event::{ModeChoice, ModelChoice, model_name};
use super::protocol::Protocol;

/// An agent the agent view drives, and so the protocol it speaks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AgentKind {
    /// Claude Code over its stream-json protocol.
    #[default]
    ClaudeCode,
    /// Codex over its app-server protocol.
    Codex,
}

/// How one agent process starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub cwd: PathBuf,
    /// The id of the mode to start in, one of the agent's.
    pub mode: String,
    /// A model the agent offered; `None` keeps its own default.
    pub model: Option<String>,
    /// Continue an earlier conversation by the agent's id for it.
    pub resume: Option<String>,
    /// The person opted in to modes that skip every check.
    pub allow_bypass: bool,
}

impl AgentKind {
    /// Every agent the agent view can drive, in the order it offers them.
    pub const ALL: [AgentKind; 2] = [AgentKind::ClaudeCode, AgentKind::Codex];

    /// Its id in the agent catalog, `[agents.<id>]`.
    pub fn catalog_id(self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "claude-code",
            AgentKind::Codex => "codex",
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
        }
    }

    /// How the conversation refers to the agent, e.g. "Ask Claude …".
    pub fn short_name(self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "Claude",
            AgentKind::Codex => "Codex",
        }
    }

    /// The program run when the catalog names none.
    pub fn default_program(self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "claude",
            AgentKind::Codex => "codex",
        }
    }

    /// The modes it runs in, until it reports its own.
    pub fn modes(self) -> Vec<ModeChoice> {
        match self {
            AgentKind::ClaudeCode => claude::modes(),
            AgentKind::Codex => codex::modes(),
        }
    }

    /// The mode it starts in unless the person picks another: the one that
    /// asks before changing anything.
    pub fn default_mode(self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => claude::ASK_MODE,
            AgentKind::Codex => codex::ASK_MODE,
        }
    }

    /// Whether the agent must be started allowing a mode that skips every
    /// check before it can switch to one: Claude Code refuses
    /// `bypassPermissions` unless launched with
    /// `--allow-dangerously-skip-permissions`.
    pub fn unrestricted_needs_launch_allowance(self) -> bool {
        matches!(self, AgentKind::ClaudeCode)
    }

    /// Models offered before the agent has listed its own.
    pub fn fallback_models(self) -> Vec<ModelChoice> {
        let values: &[&str] = match self {
            AgentKind::ClaudeCode => &super::event::FALLBACK_MODELS,
            AgentKind::Codex => &["default"],
        };
        values
            .iter()
            .map(|value| ModelChoice {
                value: (*value).to_owned(),
                display_name: model_name(&[], value),
                description: String::new(),
            })
            .collect()
    }

    /// Arguments that continue conversation `id` in the agent's own
    /// terminal interface.
    pub fn resume_args(self, id: &str) -> Vec<String> {
        match self {
            AgentKind::ClaudeCode => vec!["--resume".to_owned(), id.to_owned()],
            AgentKind::Codex => vec!["resume".to_owned(), id.to_owned()],
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
                    model: launch.model.clone().filter(|model| model != "default"),
                    resume: launch.resume.clone(),
                })),
            ),
        }
    }
}
