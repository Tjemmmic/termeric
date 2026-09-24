//! The supervisor: one agent that watches the other agents in its window and,
//! as far as its mode allows, answers them.
//!
//! Termeric starts the supervisor with its MCP tools wired in, tells it about
//! other agents as they need attention, and decides every message it tries to
//! send: refused while watching, held for the person while assisting, and
//! sent within hard limits in auto mode. This module holds that policy and the
//! pieces of the launch that do not depend on the running application.

use std::collections::VecDeque;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::{AgentActivity, AgentStatus};

/// How much a supervisor may do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SupervisorMode {
    /// Sees everything, sends nothing.
    Watch,
    /// Every message waits for the person's approval.
    #[default]
    Assist,
    /// Sends on its own, within the limits.
    Auto,
}

impl SupervisorMode {
    pub const ALL: [Self; 3] = [Self::Watch, Self::Assist, Self::Auto];

    pub fn label(self) -> &'static str {
        match self {
            Self::Watch => "Watch",
            Self::Assist => "Assist",
            Self::Auto => "Auto",
        }
    }

    /// One sentence for the person choosing a mode.
    pub fn description(self) -> &'static str {
        match self {
            Self::Watch => "Reads and reports; never sends to an agent",
            Self::Assist => "Asks you before each message it sends",
            Self::Auto => "Sends on its own, within the limits",
        }
    }

    /// How the supervisor is told about its own authority.
    pub fn instructions(self) -> &'static str {
        match self {
            Self::Watch => {
                "You are in watch mode: do not call send_to_agent; Termeric refuses it. \
                 Report what needs the person's attention and suggest what to do."
            }
            Self::Assist => {
                "You are in assist mode: every send_to_agent call is shown to the person, \
                 who approves or declines it. Termeric tells you the outcome."
            }
            Self::Auto => {
                "You are in auto mode: send_to_agent delivers directly, within Termeric's \
                 limits. Act only where the rules clearly allow it; otherwise leave it to \
                 the person and say why."
            }
        }
    }
}

/// `[supervisor]` configuration.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SupervisorConfig {
    /// The agent that supervises: `claude-code` or `codex`.
    pub agent: String,
    pub mode: SupervisorMode,
    /// Messages the supervisor may send per minute, across all agents.
    pub max_sends_per_minute: u32,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            agent: "claude-code".into(),
            mode: SupervisorMode::default(),
            max_sends_per_minute: 6,
        }
    }
}

/// The agents that can supervise: they accept MCP servers and instructions
/// when they start.
pub const SUPERVISOR_AGENTS: [&str; 2] = ["claude-code", "codex"];

impl SupervisorConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            SUPERVISOR_AGENTS.contains(&self.agent.as_str()),
            "supervisor.agent must be one of {}",
            SUPERVISOR_AGENTS.join(", ")
        );
        anyhow::ensure!(
            (1..=60).contains(&self.max_sends_per_minute),
            "supervisor.max_sends_per_minute must be between 1 and 60"
        );
        Ok(())
    }
}

/// The rules file a new supervisor starts with.
pub const DEFAULT_RULES: &str = "\
# Supervisor rules

Write, in plain language, how the supervisor should direct your agents.
Termeric gives this file to the supervisor when it starts.

- When an agent asks to run tests, builds, or linters in its project, approve it.
- When an agent asks to delete files, push, publish, or touch anything outside
  its project, do not answer; tell me instead.
- When an agent finishes, check its answer against what it was asked to do. If
  it stopped early or skipped something, tell it what is missing.
- When an agent fails or seems stuck, read its terminal and summarize the
  problem for me.
";

/// The supervisor's standing instructions: its role, its tools, its mode, and
/// the person's rules.
pub fn instructions(mode: SupervisorMode, rules: &str) -> String {
    format!(
        "You are the supervisor inside Termeric, a terminal where other coding agents work. \
         Termeric sends you messages beginning \"Termeric:\" when an agent in your window needs \
         input, finishes, or fails. Use the termeric tools: list_agents to see every agent and \
         its window_id, tab_id, and pane_id; read_terminal to read an agent's screen before \
         deciding; send_to_agent to answer it. Keep each message to an agent short and specific. \
         Never send to yourself. {}\n\nThe person's rules:\n\n{}",
        mode.instructions(),
        rules.trim()
    )
}

/// The first prompt, so the supervisor starts with the current picture.
pub const KICKOFF: &str = "Call list_agents and summarize in a few lines what each agent is doing. \
Then wait for Termeric's messages.";

/// Arguments that start `agent_id` as a supervisor whose MCP server runs
/// `termeric mcp --supervisor TOKEN`.
pub fn launch_args(
    agent_id: &str,
    termeric: &Path,
    token: &str,
    instructions: &str,
) -> Option<Vec<String>> {
    let program = termeric.to_string_lossy().into_owned();
    let server_args = ["mcp", "--supervisor", token];
    match agent_id {
        "claude-code" => {
            let config = serde_json::json!({
                "mcpServers": {"termeric": {"command": program, "args": server_args}}
            });
            Some(vec![
                "--mcp-config".into(),
                config.to_string(),
                "--allowedTools".into(),
                "mcp__termeric__list_agents,mcp__termeric__read_terminal,mcp__termeric__send_to_agent"
                    .into(),
                "--append-system-prompt".into(),
                instructions.into(),
                KICKOFF.into(),
            ])
        }
        "codex" => {
            let toml_string = |value: &str| toml::Value::String(value.into()).to_string();
            let args = server_args
                .iter()
                .map(|arg| toml_string(arg))
                .collect::<Vec<_>>()
                .join(", ");
            Some(vec![
                "-c".into(),
                format!("mcp_servers.termeric.command={}", toml_string(&program)),
                "-c".into(),
                format!("mcp_servers.termeric.args=[{args}]"),
                format!("{instructions}\n\n{KICKOFF}"),
            ])
        }
        _ => None,
    }
}

/// What the supervisor is told when another agent's state changes, if it is
/// worth telling.
pub fn event_message(name: &str, where_: &str, status: &AgentStatus) -> Option<String> {
    let first_line = |text: &str| {
        let line = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("");
        match line.char_indices().nth(300) {
            Some((cut, _)) => format!("{}…", &line[..cut]),
            None => line.to_owned(),
        }
    };
    let what = match status.activity {
        AgentActivity::NeedsInput => format!(
            "needs input: {}",
            status
                .detail
                .as_deref()
                .unwrap_or("it is waiting for an answer")
        ),
        AgentActivity::Finished => match status.last_message.as_deref() {
            Some(message) => format!("finished. Its answer begins: {}", first_line(message)),
            None => "finished its turn".into(),
        },
        AgentActivity::Failed => format!(
            "stopped with an error: {}",
            status.detail.as_deref().unwrap_or("unknown")
        ),
        AgentActivity::Idle | AgentActivity::Working => return None,
    };
    Some(format!("Termeric: {name} ({where_}) {what}"))
}

/// Why a supervisor's message may not be sent now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendRefusal {
    Watching,
    RateLimited { per_minute: u32 },
    Repeated,
}

impl std::fmt::Display for SendRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Watching => formatter.write_str(
                "The supervisor is in watch mode and cannot send; tell the person instead",
            ),
            Self::RateLimited { per_minute } => write!(
                formatter,
                "The supervisor may send at most {per_minute} messages a minute; wait before sending again"
            ),
            Self::Repeated => formatter.write_str(
                "That exact message was just sent to that agent; read its terminal before repeating it",
            ),
        }
    }
}

/// How long a message counts toward the rate limit.
const RATE_WINDOW: Duration = Duration::from_secs(60);
/// The same message to the same agent within this long is a loop.
const REPEAT_WINDOW: Duration = Duration::from_secs(30);

/// Hard limits on what a supervisor sends, whatever its instructions say.
#[derive(Debug, Default)]
pub struct SendLimits {
    /// Recent sends: when, to which terminal, and what.
    sent: VecDeque<(Instant, String, String)>,
}

impl SendLimits {
    /// Whether a message may go now, in `mode`, to the agent at `target`.
    pub fn check(
        &mut self,
        mode: SupervisorMode,
        per_minute: u32,
        target: &str,
        text: &str,
        now: Instant,
    ) -> Result<(), SendRefusal> {
        if mode == SupervisorMode::Watch {
            return Err(SendRefusal::Watching);
        }
        while self
            .sent
            .front()
            .is_some_and(|(at, ..)| now.duration_since(*at) >= RATE_WINDOW)
        {
            self.sent.pop_front();
        }
        if self.sent.iter().any(|(at, to, sent)| {
            to == target && sent == text && now.duration_since(*at) < REPEAT_WINDOW
        }) {
            return Err(SendRefusal::Repeated);
        }
        if self.sent.len() >= per_minute as usize {
            return Err(SendRefusal::RateLimited { per_minute });
        }
        Ok(())
    }

    /// Count a message that was sent.
    pub fn record(&mut self, target: &str, text: &str, now: Instant) {
        self.sent
            .push_back((now, target.to_owned(), text.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentReport;
    use crate::control::protocol::AgentEventKind;

    #[test]
    fn modes_and_limits_decide_every_send() {
        let mut limits = SendLimits::default();
        let now = Instant::now();
        assert_eq!(
            limits.check(SupervisorMode::Watch, 6, "a", "go", now),
            Err(SendRefusal::Watching)
        );
        for index in 0..2 {
            let text = format!("message {index}");
            limits
                .check(SupervisorMode::Auto, 2, "a", &text, now)
                .unwrap();
            limits.record("a", &text, now);
        }
        assert_eq!(
            limits.check(SupervisorMode::Auto, 2, "b", "other", now),
            Err(SendRefusal::RateLimited { per_minute: 2 })
        );
        let later = now + RATE_WINDOW;
        limits
            .check(SupervisorMode::Assist, 2, "b", "other", later)
            .unwrap();
        limits.record("a", "same", later);
        assert_eq!(
            limits.check(
                SupervisorMode::Auto,
                6,
                "a",
                "same",
                later + Duration::from_secs(5)
            ),
            Err(SendRefusal::Repeated)
        );
        limits
            .check(SupervisorMode::Auto, 6, "b", "same", later)
            .expect("the same words to another agent are fine");
        limits
            .check(SupervisorMode::Auto, 6, "a", "same", later + REPEAT_WINDOW)
            .expect("repeating later is fine");
    }

    #[test]
    fn launches_wire_in_the_supervisor_server_for_each_agent() {
        let exe = Path::new("/opt/termeric/bin/termeric");
        let token = "a".repeat(32);
        let claude = launch_args("claude-code", exe, &token, "RULES").unwrap();
        let config: serde_json::Value = serde_json::from_str(&claude[1]).unwrap();
        assert_eq!(
            config["mcpServers"]["termeric"]["args"],
            serde_json::json!(["mcp", "--supervisor", token])
        );
        assert_eq!(claude[5], "RULES");
        assert_eq!(claude.last().unwrap(), KICKOFF);

        let codex = launch_args("codex", exe, &token, "RULES").unwrap();
        assert_eq!(
            codex[1],
            "mcp_servers.termeric.command=\"/opt/termeric/bin/termeric\""
        );
        let args: toml::Value =
            toml::from_str(&codex[3].replace("mcp_servers.termeric.args", "args")).unwrap();
        assert_eq!(
            args["args"],
            toml::Value::Array(
                ["mcp", "--supervisor", token.as_str()]
                    .map(|arg| toml::Value::String(arg.into()))
                    .to_vec()
            )
        );
        assert!(codex[4].starts_with("RULES") && codex[4].ends_with(KICKOFF));
        assert_eq!(launch_args("gemini", exe, &token, "RULES"), None);
    }

    #[test]
    fn only_attention_states_are_worth_telling_the_supervisor() {
        let status = |event, detail: Option<&str>, message: Option<&str>| {
            AgentStatus::after(
                None,
                AgentReport {
                    agent_id: "codex".into(),
                    event,
                    detail: detail.map(Into::into),
                    last_message: message.map(Into::into),
                    transcript_path: None,
                },
                Instant::now(),
            )
            .unwrap()
        };
        let where_ = "window 1, tab 3, pane 0";
        assert_eq!(
            event_message(
                "Codex",
                where_,
                &status(AgentEventKind::NeedsInput, Some("Approve shell: ls"), None)
            )
            .unwrap(),
            "Termeric: Codex (window 1, tab 3, pane 0) needs input: Approve shell: ls"
        );
        assert_eq!(
            event_message(
                "Codex",
                where_,
                &status(
                    AgentEventKind::Finished,
                    None,
                    Some("\nAll tests pass.\nMore.")
                )
            )
            .unwrap(),
            "Termeric: Codex (window 1, tab 3, pane 0) finished. Its answer begins: All tests pass."
        );
        assert_eq!(
            event_message(
                "Codex",
                where_,
                &status(AgentEventKind::Working, None, None)
            ),
            None
        );
    }

    #[test]
    fn instructions_carry_the_mode_and_the_rules() {
        let text = instructions(SupervisorMode::Watch, "\n- Be careful.\n");
        assert!(text.contains("watch mode"));
        assert!(text.ends_with("- Be careful."));
        let config = SupervisorConfig::default();
        config.validate().unwrap();
        assert!(
            SupervisorConfig {
                agent: "gemini".into(),
                ..SupervisorConfig::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            SupervisorConfig {
                max_sends_per_minute: 0,
                ..SupervisorConfig::default()
            }
            .validate()
            .is_err()
        );
    }
}
