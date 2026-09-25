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
         deciding; send_to_agent to answer it. Keep each message to an agent short and specific, \
         in plain words: Termeric refuses messages starting with `!` or `/`, which the agent \
         would run as commands, and any message while the person is typing to that agent. \
         Never send to yourself. {}\n\nThe person's rules:\n\n{}",
        mode.instructions(),
        rules.trim()
    )
}

/// The first prompt, so the supervisor starts with the current picture.
pub const KICKOFF: &str = "Call list_agents and summarize in a few lines what each agent is doing. \
Then wait for Termeric's messages.";

/// The name of the supervisor's MCP server, distinct from a `termeric`
/// server the person may have configured, so neither replaces the other.
pub const SERVER_NAME: &str = "termeric_supervisor";
/// The tools the supervisor may call without asking the person, as Claude
/// Code names them.
const CLAUDE_TOOLS: &str = "mcp__termeric_supervisor__list_agents,\
mcp__termeric_supervisor__read_terminal,mcp__termeric_supervisor__send_to_agent";

/// How the supervisor's MCP server reaches the Termeric that started it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorServer<'a> {
    pub termeric: &'a Path,
    pub token: &'a str,
    /// This Termeric's process ID, named so that nothing is discovered.
    pub process_id: u32,
    /// This Termeric's `XDG_RUNTIME_DIR`, where its control socket lives.
    /// Codex starts MCP servers with only a few basic variables, so it is
    /// passed on explicitly.
    pub runtime_directory: Option<&'a str>,
}

/// Arguments that start `agent_id` as a supervisor whose MCP server runs
/// `termeric mcp --process PID --supervisor TOKEN`.
pub fn launch_args(
    agent_id: &str,
    server: &SupervisorServer,
    instructions: &str,
) -> Option<Vec<String>> {
    let program = server.termeric.to_string_lossy().into_owned();
    let process = server.process_id.to_string();
    let server_args = ["mcp", "--process", &process, "--supervisor", server.token];
    let environment: Vec<(&str, &str)> = server
        .runtime_directory
        .map(|directory| ("XDG_RUNTIME_DIR", directory))
        .into_iter()
        .collect();
    match agent_id {
        "claude-code" => {
            let mut definition = serde_json::json!({"command": program, "args": server_args});
            if !environment.is_empty() {
                definition["env"] = environment
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), serde_json::json!(value)))
                    .collect::<serde_json::Map<_, _>>()
                    .into();
            }
            let config = serde_json::json!({"mcpServers": {SERVER_NAME: definition}});
            Some(vec![
                "--mcp-config".into(),
                config.to_string(),
                "--allowedTools".into(),
                CLAUDE_TOOLS.into(),
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
            let mut launch = vec![
                "-c".into(),
                format!(
                    "mcp_servers.{SERVER_NAME}.command={}",
                    toml_string(&program)
                ),
                "-c".into(),
                format!("mcp_servers.{SERVER_NAME}.args=[{args}]"),
                // Termeric decides every send itself; Codex asking the person
                // again for each tool call would only repeat that.
                "-c".into(),
                format!("mcp_servers.{SERVER_NAME}.default_tools_approval_mode=\"approve\""),
            ];
            if !environment.is_empty() {
                let table = environment
                    .iter()
                    .map(|(name, value)| format!("{name} = {}", toml_string(value)))
                    .collect::<Vec<_>>()
                    .join(", ");
                launch.push("-c".into());
                launch.push(format!("mcp_servers.{SERVER_NAME}.env={{ {table} }}"));
            }
            launch.push(format!("{instructions}\n\n{KICKOFF}"));
            Some(launch)
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

/// Something an agent view session did that the supervisor should hear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// It waits for an answer to one approval request.
    Approval {
        session: u64,
        title: String,
        approval_id: String,
        request: String,
    },
    Finished {
        session: u64,
        title: String,
        answer: Option<String>,
    },
    Failed {
        session: u64,
        title: String,
        error: String,
    },
}

/// What the supervisor is told about an agent view session.
pub fn session_message(window_id: u64, event: &SessionEvent) -> String {
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
    let (session, title, what) = match event {
        SessionEvent::Approval {
            session,
            title,
            approval_id,
            request,
        } => (
            session,
            title,
            format!(
                "asks: {}. Decide with answer_approval (approval_id {approval_id}), \
                 read it with read_conversation, or leave it for the person.",
                first_line(request)
            ),
        ),
        SessionEvent::Finished {
            session,
            title,
            answer,
        } => (
            session,
            title,
            match answer {
                Some(answer) => format!("finished. Its answer begins: {}", first_line(answer)),
                None => "finished its turn".to_owned(),
            },
        ),
        SessionEvent::Failed {
            session,
            title,
            error,
        } => (
            session,
            title,
            format!("stopped with an error: {}", first_line(error)),
        ),
    };
    format!(
        "Termeric: Claude Code in the agent view (window {window_id}, session {session}, \
         \"{}\") {what}",
        first_line(title)
    )
}

/// Why a message sent through Termeric's agent tools may not be sent now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendRefusal {
    Watching,
    RateLimited {
        per_minute: u32,
    },
    Repeated,
    Empty,
    TooLong,
    /// Control or direction-changing characters, which would make the text
    /// the person approves differ from the text the agent receives.
    HiddenCharacters,
    /// Text the agent would run itself, as a shell command (`!`) or one of
    /// its own commands (`/`), bypassing its approvals.
    AgentCommand,
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
                "That message was just sent to that agent; read its terminal before repeating it",
            ),
            Self::Empty => formatter.write_str("The message is empty"),
            Self::TooLong => write!(
                formatter,
                "Messages are limited to {MAX_MESSAGE_BYTES} bytes and {MAX_MESSAGE_LINES} lines; send something shorter"
            ),
            Self::HiddenCharacters => formatter.write_str(
                "The message contains control or text-direction characters; send plain text",
            ),
            Self::AgentCommand => formatter.write_str(
                "Messages may not start with `!` or `/`, which the agent would run as a command; rephrase it as a request",
            ),
        }
    }
}

/// The largest message, small enough for the person to read in full before
/// approving it.
/// Shown indented in a dialog whose lines hold 4096 characters, a
/// message's longest line still shows in full.
pub const MAX_MESSAGE_BYTES: usize = 4000;
pub const MAX_MESSAGE_LINES: usize = 80;

/// Whether `text` may be given to an agent as a prompt at all.
pub fn check_message(text: &str) -> Result<(), SendRefusal> {
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return Err(SendRefusal::Empty);
    }
    if text.len() > MAX_MESSAGE_BYTES || text.lines().count() > MAX_MESSAGE_LINES {
        return Err(SendRefusal::TooLong);
    }
    let hidden = |character: char| {
        (character.is_control() && !matches!(character, '\n' | '\t'))
            || matches!(
                character,
                '\u{200E}' | '\u{200F}' | '\u{061C}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
            )
    };
    if text.chars().any(hidden) {
        return Err(SendRefusal::HiddenCharacters);
    }
    if trimmed.starts_with(['!', '/']) {
        return Err(SendRefusal::AgentCommand);
    }
    Ok(())
}

/// The words of `text`, so that spacing and case do not make a repeated
/// message look new.
fn normalized(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// How long a message counts toward the rate limit.
const RATE_WINDOW: Duration = Duration::from_secs(60);
/// The same message to the same agent within this long is a loop.
const REPEAT_WINDOW: Duration = Duration::from_secs(30);

/// Hard limits on what a supervisor sends, whatever its instructions say.
#[derive(Debug, Default)]
pub struct SendLimits {
    /// Recent messages: when, to which terminal, and their normalized words.
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
        check_message(text)?;
        while self
            .sent
            .front()
            .is_some_and(|(at, ..)| now.duration_since(*at) >= RATE_WINDOW)
        {
            self.sent.pop_front();
        }
        let words = normalized(text);
        if self.sent.iter().any(|(at, to, sent)| {
            to == target && *sent == words && now.duration_since(*at) < REPEAT_WINDOW
        }) {
            return Err(SendRefusal::Repeated);
        }
        if self.sent.len() >= per_minute as usize {
            return Err(SendRefusal::RateLimited { per_minute });
        }
        Ok(())
    }

    /// Count a message that was sent, or put to the person and not sent: a
    /// supervisor may not keep asking faster than it may send.
    pub fn record(&mut self, target: &str, text: &str, now: Instant) {
        self.sent
            .push_back((now, target.to_owned(), normalized(text)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_messages_name_the_session_and_how_to_answer() {
        let approval = session_message(
            2,
            &SessionEvent::Approval {
                session: 7,
                title: "Fix the build".into(),
                approval_id: "req-1".into(),
                request: "Run cargo test\nsecond line".into(),
            },
        );
        assert_eq!(
            approval,
            "Termeric: Claude Code in the agent view (window 2, session 7, \"Fix the build\") \
             asks: Run cargo test. Decide with answer_approval (approval_id req-1), read it \
             with read_conversation, or leave it for the person."
        );
        let finished = session_message(
            2,
            &SessionEvent::Finished {
                session: 7,
                title: "Fix".into(),
                answer: Some("\nAll green.".into()),
            },
        );
        assert!(finished.ends_with("finished. Its answer begins: All green."));
    }
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
        assert_eq!(
            limits.check(SupervisorMode::Auto, 6, "a", "  SAME\n", later),
            Err(SendRefusal::Repeated),
            "spacing and case do not make a message new"
        );
        limits
            .check(SupervisorMode::Auto, 6, "a", "same", later + REPEAT_WINDOW)
            .expect("repeating later is fine");
    }

    #[test]
    fn messages_are_plain_requests_the_person_can_read_in_full() {
        assert_eq!(
            check_message("Run the tests again.\n\tThen report."),
            Ok(())
        );
        assert_eq!(check_message("  \n"), Err(SendRefusal::Empty));
        assert_eq!(
            check_message("!rm -rf target"),
            Err(SendRefusal::AgentCommand)
        );
        assert_eq!(
            check_message("\n  /permissions"),
            Err(SendRefusal::AgentCommand)
        );
        assert_eq!(check_message("Use a/b paths"), Ok(()));
        assert_eq!(
            check_message("approve \u{202E}lmth.exe"),
            Err(SendRefusal::HiddenCharacters)
        );
        assert_eq!(
            check_message("paste\u{1b}[201~ends"),
            Err(SendRefusal::HiddenCharacters)
        );
        assert_eq!(
            check_message(&"x".repeat(MAX_MESSAGE_BYTES + 1)),
            Err(SendRefusal::TooLong)
        );
        assert_eq!(
            check_message(&"x\n".repeat(MAX_MESSAGE_LINES + 1)),
            Err(SendRefusal::TooLong)
        );
    }

    #[test]
    fn launches_wire_in_the_supervisor_server_for_each_agent() {
        let token = "a".repeat(32);
        let server = SupervisorServer {
            termeric: Path::new("/opt/termeric/bin/termeric"),
            token: &token,
            process_id: 4242,
            runtime_directory: Some("/run/user/1000"),
        };
        let expected_args = ["mcp", "--process", "4242", "--supervisor", token.as_str()];
        let claude = launch_args("claude-code", &server, "RULES").unwrap();
        let config: serde_json::Value = serde_json::from_str(&claude[1]).unwrap();
        let definition = &config["mcpServers"]["termeric_supervisor"];
        assert_eq!(definition["args"], serde_json::json!(expected_args));
        assert_eq!(definition["env"]["XDG_RUNTIME_DIR"], "/run/user/1000");
        assert!(
            claude[3]
                .split(',')
                .all(|tool| tool.starts_with("mcp__termeric_supervisor__"))
        );
        assert_eq!(claude[3].split(',').count(), 3);
        assert_eq!(claude[5], "RULES");
        assert_eq!(claude.last().unwrap(), KICKOFF);

        let codex = launch_args("codex", &server, "RULES").unwrap();
        // Each `-c` override is TOML, parsed as Codex parses it.
        let overrides: toml::Table = codex
            .chunks(2)
            .filter(|pair| pair[0] == "-c")
            .map(|pair| pair[1].replacen("mcp_servers.termeric_supervisor.", "", 1))
            .collect::<Vec<_>>()
            .join("\n")
            .parse()
            .unwrap();
        assert_eq!(
            overrides["command"].as_str(),
            Some("/opt/termeric/bin/termeric")
        );
        assert_eq!(
            overrides["args"],
            toml::Value::Array(
                expected_args
                    .map(|arg| toml::Value::String(arg.into()))
                    .to_vec()
            )
        );
        assert_eq!(
            overrides["default_tools_approval_mode"].as_str(),
            Some("approve")
        );
        assert_eq!(
            overrides["env"]["XDG_RUNTIME_DIR"].as_str(),
            Some("/run/user/1000")
        );
        let prompt = codex.last().unwrap();
        assert!(prompt.starts_with("RULES") && prompt.ends_with(KICKOFF));

        let without_runtime = SupervisorServer {
            runtime_directory: None,
            ..server.clone()
        };
        let codex = launch_args("codex", &without_runtime, "RULES").unwrap();
        assert!(!codex.iter().any(|arg| arg.contains(".env=")));
        assert_eq!(launch_args("gemini", &server, "RULES"), None);
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
