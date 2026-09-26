//! One conversation with an agent: its process, protocol, and transcript.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use super::claude::{self, ClaudeCodec, LaunchOptions};
use super::event::{AgentCommand, AgentEvent, ApprovalDecision, PermissionMode, model_name};
use super::process::{AgentProcess, LaunchSpec, Lookup, ProcessOutput, SendError, Waker};
use super::transcript::{
    Activity, Attachment, DecidedBy, Decision, NoticeKind, Transcript, compose_prompt,
};

/// How long a starting agent may take to answer the handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// Output lines handled per pump, so a flood cannot stall a frame.
const LINES_PER_PUMP: usize = 512;
/// Environment a Termeric terminal gives its processes. The agent view's agent
/// runs in no terminal, so an inherited value would misattribute its hooks.
const TERMINAL_ENVIRONMENT: [&str; 1] = ["TERMERIC_TERMINAL_ID"];

/// Identifies a session for the lifetime of the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(u64);

impl SessionId {
    fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    /// The id other programs use for this session over the control socket.
    pub fn control_id(self) -> u64 {
        self.0
    }
}

/// The agent executable and the environment it runs with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentProgram {
    pub program: String,
    /// Arguments placed before the protocol's own, e.g. a script to run.
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// How a session starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartOptions {
    pub cwd: PathBuf,
    pub mode: PermissionMode,
    pub model: Option<String>,
    /// Continue an earlier conversation: its agent session id and title.
    pub resume: Option<(String, String)>,
    /// The person opted in to the mode that skips every check.
    pub allow_bypass: bool,
}

/// Why a session could not start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartError {
    NotInstalled(String),
    /// Termeric is still asking the login shell where programs are.
    StillLooking,
    Launch(String),
    /// The conversation was handed to a terminal, which now writes it.
    ContinuedInTerminal,
}

impl std::fmt::Display for StartError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::NotInstalled(program) => {
                write!(formatter, "Claude Code ({program}) is not installed")
            }
            StartError::StillLooking => write!(
                formatter,
                "Still looking for Claude Code on your login shell's PATH; try again in a moment"
            ),
            StartError::Launch(error) => write!(formatter, "Claude Code could not start: {error}"),
            StartError::ContinuedInTerminal => {
                write!(formatter, "This conversation continues in a terminal")
            }
        }
    }
}

/// A person's answer to one question the agent asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionAnswer {
    pub question: String,
    pub answer: String,
}

/// A conversation with Claude Code.
pub struct AgentSession {
    id: SessionId,
    title: Option<String>,
    cwd: PathBuf,
    program: AgentProgram,
    model: Option<String>,
    process: Option<AgentProcess>,
    /// The process was asked to end and has not exited yet. It takes no
    /// more input.
    closing: bool,
    /// A message arrived while closing: relaunch once the process exits.
    relaunch_on_exit: bool,
    /// Handed to a terminal, which now writes the conversation.
    handed_off: bool,
    /// The conversation this session continued, until the agent reports
    /// its own id.
    resumed: Option<String>,
    /// The person opted in to the mode that skips every check.
    allow_bypass: bool,
    codec: ClaudeCodec,
    transcript: Transcript,
    backlog: VecDeque<String>,
    handshake_deadline: Option<Instant>,
    /// Mode to apply once an approved plan's tool call finishes.
    mode_after_plan: Option<(String, PermissionMode)>,
    /// Messages written while a turn ran, sent in order as turns end.
    queued: VecDeque<(String, Vec<Attachment>)>,
    waker: Waker,
    last_change: Instant,
}

impl std::fmt::Debug for AgentSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentSession")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("cwd", &self.cwd)
            .field("activity", &self.transcript.activity())
            .finish_non_exhaustive()
    }
}

impl AgentSession {
    pub fn start(
        program: AgentProgram,
        options: StartOptions,
        waker: Waker,
    ) -> Result<Self, StartError> {
        let mut transcript = Transcript::new(options.mode);
        let (title, resume) = match options.resume {
            Some((session_id, title)) => {
                transcript.push_notice(NoticeKind::Info, "Continuing an earlier conversation");
                (Some(title), Some(session_id))
            }
            None => (None, None),
        };
        let mut session = Self {
            id: SessionId::next(),
            title,
            cwd: options.cwd,
            program,
            model: options.model,
            process: None,
            closing: false,
            relaunch_on_exit: false,
            handed_off: false,
            resumed: resume.clone(),
            allow_bypass: options.allow_bypass,
            codec: ClaudeCodec::new(),
            transcript,
            backlog: VecDeque::new(),
            handshake_deadline: None,
            mode_after_plan: None,
            queued: VecDeque::new(),
            waker,
            last_change: Instant::now(),
        };
        session.launch(resume)?;
        Ok(session)
    }

    fn launch(&mut self, resume: Option<String>) -> Result<(), StartError> {
        let program = match super::process::find_program(&self.program.program, &self.waker) {
            Lookup::Found(program) => program,
            Lookup::Missing => return Err(StartError::NotInstalled(self.program.program.clone())),
            Lookup::Searching => return Err(StartError::StillLooking),
        };
        let mut args = self.program.args.clone();
        args.extend(claude::launch_args(&LaunchOptions {
            mode: self.transcript.mode(),
            model: self.model.clone(),
            resume,
            allow_bypass: self.allow_bypass,
        }));
        let spec = LaunchSpec {
            program,
            args,
            cwd: self.cwd.clone(),
            env: self.program.env.clone(),
            env_remove: TERMINAL_ENVIRONMENT.map(String::from).to_vec(),
        };
        let process = AgentProcess::spawn(&spec, self.waker.clone())
            .map_err(|error| StartError::Launch(error.to_string()))?;
        self.codec = ClaudeCodec::new();
        self.backlog.clear();
        self.process = Some(process);
        let initialize = self.codec.initialize();
        self.send_line(initialize);
        self.handshake_deadline = Some(Instant::now() + HANDSHAKE_TIMEOUT);
        Ok(())
    }

    /// A session with no agent process, for tests. Its program cannot
    /// exist, so nothing a test does can start a real agent.
    #[cfg(test)]
    pub fn detached(cwd: PathBuf, transcript: Transcript) -> Self {
        Self {
            id: SessionId::next(),
            title: None,
            cwd,
            program: AgentProgram {
                program: "/nonexistent/termeric-test-agent".into(),
                args: Vec::new(),
                env: Vec::new(),
            },
            model: None,
            process: None,
            closing: false,
            relaunch_on_exit: false,
            handed_off: false,
            resumed: None,
            allow_bypass: false,
            codec: ClaudeCodec::new(),
            transcript,
            backlog: VecDeque::new(),
            handshake_deadline: None,
            mode_after_plan: None,
            queued: VecDeque::new(),
            waker: std::sync::Arc::new(|| {}),
            last_change: Instant::now(),
        }
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    pub fn cwd(&self) -> &std::path::Path {
        &self.cwd
    }

    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    pub fn activity(&self) -> Activity {
        self.transcript.activity()
    }

    /// Whether an agent process is running and taking input.
    pub fn is_running(&self) -> bool {
        self.process.is_some() && !self.closing
    }

    /// Whether nothing was said yet: a session started ahead of its first
    /// message, so the agent's commands and models are ready. It stays out
    /// of every list of conversations until a message is sent.
    pub fn is_blank(&self) -> bool {
        self.resumed.is_none() && self.transcript.first_prompt().is_none() && self.queued.is_empty()
    }

    /// The arguments given before the protocol's own.
    #[cfg(test)]
    pub fn program_args(&self) -> &[String] {
        &self.program.args
    }

    /// Names the conversation, instead of its first message.
    pub fn set_title(&mut self, title: &str) {
        self.title = Some(title.to_owned());
    }

    /// The running agent's process id.
    pub fn process_id(&self) -> Option<u32> {
        self.process
            .as_ref()
            .filter(|_| !self.closing)
            .map(AgentProcess::process_id)
    }

    /// Whether the conversation was handed to a terminal.
    pub fn handed_off(&self) -> bool {
        self.handed_off
    }

    /// The agent's id for this conversation, accepted by `--resume`.
    pub fn conversation_id(&self) -> Option<&str> {
        self.transcript
            .session()
            .map(|info| info.session_id.as_str())
            .filter(|id| !id.is_empty())
            .or(self.resumed.as_deref())
    }

    /// When the conversation last changed.
    pub fn last_change(&self) -> Instant {
        self.last_change
    }

    /// A short name: the explicit title, else the first prompt's first line.
    pub fn title(&self) -> String {
        if let Some(title) = &self.title {
            return title.clone();
        }
        self.transcript
            .first_prompt()
            .and_then(|prompt| prompt.lines().map(str::trim).find(|line| !line.is_empty()))
            .map_or_else(|| "New session".to_owned(), str::to_owned)
    }

    /// The model's display name, as the agent reported it.
    pub fn model_label(&self) -> String {
        let offered = self
            .transcript
            .handshake()
            .map_or(&[][..], |handshake| &handshake.models);
        model_name(offered, self.model.as_deref().unwrap_or("default"))
    }

    /// Handles everything the agent wrote since the last pump. Returns
    /// whether the conversation changed.
    pub fn pump(&mut self) -> bool {
        let before = self.transcript.generation();
        let mut handled = 0;
        while handled < LINES_PER_PUMP {
            let Some(output) = self.process.as_mut().and_then(AgentProcess::try_next) else {
                break;
            };
            handled += 1;
            match output {
                ProcessOutput::Line(line) => self.handle_line(&line),
                ProcessOutput::Oversized => self
                    .transcript
                    .push_notice(NoticeKind::Warning, "Skipped a message too large to show"),
                ProcessOutput::Exited(report) => {
                    self.process = None;
                    self.closing = false;
                    self.handshake_deadline = None;
                    self.transcript.apply(AgentEvent::Exited(report));
                    if std::mem::take(&mut self.relaunch_on_exit)
                        && let Err(error) = self.relaunch()
                    {
                        self.transcript
                            .push_notice(NoticeKind::Error, error.to_string());
                    }
                }
            }
        }
        if handled == LINES_PER_PUMP {
            // More may be waiting; come back next frame rather than now.
            (self.waker)();
        }
        self.flush_backlog();
        self.check_handshake();
        self.send_next_queued();
        let changed = self.transcript.generation() != before;
        if changed {
            self.last_change = Instant::now();
        }
        changed
    }

    fn handle_line(&mut self, line: &str) {
        let decoded = self.codec.decode(line);
        for reply in decoded.replies {
            self.send_line(reply);
        }
        let mut refused = false;
        for event in decoded.events {
            match &event {
                AgentEvent::Ready(_) => self.handshake_deadline = None,
                AgentEvent::HandshakeFailed(_) => refused = true,
                AgentEvent::Unrecognized(text) => {
                    log::debug!("agent view: unrecognized agent output: {text}");
                }
                AgentEvent::SessionStarted(info) => {
                    if let Some(version) = claude::parse_version(&info.version)
                        && version < claude::MINIMUM_VERSION
                    {
                        self.transcript.push_notice(
                            NoticeKind::Warning,
                            format!(
                                "Claude Code {} is older than the agent view supports; \
                                 update it if anything looks wrong",
                                info.version
                            ),
                        );
                    }
                }
                AgentEvent::ToolResult { result, .. } => {
                    if let Some((tool_use_id, mode)) = &self.mode_after_plan
                        && *tool_use_id == result.tool_use_id
                    {
                        let mode = *mode;
                        self.mode_after_plan = None;
                        if !result.is_error {
                            self.set_mode(mode);
                        }
                    }
                }
                AgentEvent::TurnFinished(_) => {
                    let query = self.codec.encode(&AgentCommand::QueryContextUsage);
                    self.send_line(query);
                }
                _ => {}
            }
            self.transcript.apply(event);
        }
        if refused {
            self.close();
        }
    }

    fn check_handshake(&mut self) {
        let Some(deadline) = self.handshake_deadline else {
            return;
        };
        if Instant::now() < deadline {
            return;
        }
        self.transcript.push_notice(
            NoticeKind::Error,
            "Claude Code did not answer. This version may not support the agent view; \
             open it in a terminal instead.",
        );
        self.close();
    }

    /// Seconds until the handshake times out, for scheduling a check.
    pub fn handshake_deadline(&self) -> Option<Instant> {
        self.handshake_deadline
    }

    fn send_line(&mut self, line: String) {
        if !self.backlog.is_empty() {
            self.backlog.push_back(line);
            return;
        }
        let Some(process) = self.process.as_ref().filter(|_| !self.closing) else {
            return;
        };
        match process.send(line) {
            Ok(()) | Err((SendError::Closed, _)) => {}
            Err((SendError::Busy, line)) => self.backlog.push_back(line),
        }
    }

    fn flush_backlog(&mut self) {
        let Some(process) = &self.process else {
            self.backlog.clear();
            return;
        };
        while let Some(line) = self.backlog.pop_front() {
            match process.send(line) {
                Ok(()) => {}
                Err((SendError::Busy, line)) => {
                    self.backlog.push_front(line);
                    break;
                }
                Err((SendError::Closed, _)) => {
                    self.backlog.clear();
                    break;
                }
            }
        }
    }

    fn command(&mut self, command: &AgentCommand) {
        let line = self.codec.encode(command);
        self.send_line(line);
    }

    /// Relaunches a closed agent on the same conversation. An agent still
    /// closing relaunches once it has exited.
    pub fn relaunch(&mut self) -> Result<(), StartError> {
        if self.handed_off {
            return Err(StartError::ContinuedInTerminal);
        }
        if self.closing {
            self.relaunch_on_exit = true;
            return Ok(());
        }
        if self.process.is_some() {
            return Ok(());
        }
        self.launch(self.conversation_id().map(str::to_owned))?;
        self.transcript.reopen();
        Ok(())
    }

    /// Sends a message, relaunching the agent if it had closed. While a turn
    /// runs, the message waits and goes when the turn ends, so every
    /// message starts a turn of its own.
    pub fn prompt(&mut self, text: &str, attachments: Vec<Attachment>) -> Result<(), StartError> {
        if compose_prompt(text, &attachments).is_empty() {
            return Ok(());
        }
        self.relaunch()?;
        // Earlier waiting messages go first, and nothing reaches an agent
        // that is still closing.
        if self.closing || self.transcript.turn_active() || !self.queued.is_empty() {
            self.queued.push_back((text.trim().to_owned(), attachments));
            (self.waker)();
        } else {
            self.send_prompt(text.trim().to_owned(), attachments);
        }
        self.last_change = Instant::now();
        Ok(())
    }

    fn send_prompt(&mut self, text: String, attachments: Vec<Attachment>) {
        let prompt = compose_prompt(&text, &attachments);
        self.transcript.push_user(text, attachments);
        self.command(&AgentCommand::Prompt(prompt));
    }

    /// Sends the oldest waiting message once the agent is free.
    fn send_next_queued(&mut self) {
        if self.transcript.turn_active() || !self.is_running() {
            return;
        }
        if let Some((text, attachments)) = self.queued.pop_front() {
            self.send_prompt(text, attachments);
            self.last_change = Instant::now();
        }
    }

    /// Messages waiting for the current turn to end, oldest first.
    pub fn queued(&self) -> impl Iterator<Item = &str> {
        self.queued.iter().map(|(text, _)| text.as_str())
    }

    /// Withdraws a waiting message, returning its text.
    pub fn cancel_queued(&mut self, index: usize) -> Option<String> {
        let (text, _) = self.queued.remove(index)?;
        self.last_change = Instant::now();
        Some(text)
    }

    /// Answers a pending approval. Returns false if it was no longer pending.
    pub fn answer(
        &mut self,
        request_id: &str,
        decision: ApprovalDecision,
        by: DecidedBy,
        note: Option<String>,
    ) -> bool {
        let Some(approval) = self.transcript.approval(request_id).cloned() else {
            return false;
        };
        let (allowed, remembered, input) = match &decision {
            ApprovalDecision::Allow { remember, input } => (
                true,
                remember
                    .as_ref()
                    .and(approval.remember.as_ref())
                    .map(|option| option.label.clone()),
                Some(input.clone()),
            ),
            ApprovalDecision::Deny { .. } => (false, None, None),
        };
        self.command(&AgentCommand::Answer {
            request_id: request_id.to_owned(),
            tool_use_id: approval.tool_use_id.clone(),
            decision,
        });
        self.transcript.record_decision(
            request_id,
            Decision {
                allowed,
                by,
                remembered,
                note,
            },
            input,
        );
        self.last_change = Instant::now();
        true
    }

    /// Records the supervisor's advice on a pending approval for the person.
    pub fn suggest(&mut self, request_id: &str, allow: bool, reason: &str) -> bool {
        let suggested = self.transcript.suggest(
            request_id,
            crate::agent_view::transcript::Suggestion {
                allow,
                reason: reason.trim().to_owned(),
            },
        );
        if suggested {
            self.last_change = Instant::now();
        }
        suggested
    }

    /// Allows a pending approval with its own input.
    pub fn allow(&mut self, request_id: &str, remember: bool, by: DecidedBy) -> bool {
        let Some(approval) = self.transcript.approval(request_id).cloned() else {
            return false;
        };
        let remember = remember
            .then(|| approval.remember.map(|option| option.updates))
            .flatten();
        self.answer(
            request_id,
            ApprovalDecision::Allow {
                input: approval.input,
                remember,
            },
            by,
            None,
        )
    }

    /// Denies a pending approval, telling the agent why. With `interrupt`
    /// the agent also ends its turn and waits for the person.
    pub fn deny(
        &mut self,
        request_id: &str,
        message: Option<&str>,
        by: DecidedBy,
        interrupt: bool,
    ) -> bool {
        let note = message
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .map(str::to_owned);
        let text = note
            .clone()
            .unwrap_or_else(|| "The person declined this action.".to_owned());
        self.answer(
            request_id,
            ApprovalDecision::Deny {
                message: text,
                interrupt,
            },
            by,
            note,
        )
    }

    /// Answers a question the agent asked with the chosen options.
    pub fn answer_question(&mut self, request_id: &str, answers: &[QuestionAnswer]) -> bool {
        let Some(approval) = self.transcript.approval(request_id).cloned() else {
            return false;
        };
        let mut input = match approval.input {
            Value::Object(object) => object,
            _ => Map::new(),
        };
        let chosen: Map<String, Value> = answers
            .iter()
            .map(|answer| {
                (
                    answer.question.clone(),
                    Value::String(answer.answer.clone()),
                )
            })
            .collect();
        input.insert("answers".into(), Value::Object(chosen));
        self.answer(
            request_id,
            ApprovalDecision::Allow {
                input: Value::Object(input),
                remember: None,
            },
            DecidedBy::Person,
            None,
        )
    }

    /// Approves a plan and switches to `mode` once the agent leaves planning.
    pub fn approve_plan(&mut self, request_id: &str, mode: PermissionMode) -> bool {
        let tool_use_id = self
            .transcript
            .approval(request_id)
            .and_then(|approval| approval.tool_use_id.clone());
        if !self.allow(request_id, false, DecidedBy::Person) {
            return false;
        }
        match tool_use_id {
            Some(id) => self.mode_after_plan = Some((id, mode)),
            None => self.set_mode(mode),
        }
        true
    }

    pub fn interrupt(&mut self) {
        if !self.transcript.turn_active() || !self.is_running() {
            return;
        }
        self.command(&AgentCommand::Interrupt);
        self.transcript.mark_interrupting();
    }

    /// Switches the permission mode. The mode that skips every check is
    /// refused unless the person opted in to it.
    pub fn set_mode(&mut self, mode: PermissionMode) {
        if mode.skips_every_check() && !self.allow_bypass {
            return;
        }
        if self.is_running() {
            self.command(&AgentCommand::SetMode(mode));
        }
        self.transcript.set_mode(mode);
    }

    /// Follows the person's opt-in to the mode that skips every check. A
    /// session in that mode when the opt-in is withdrawn goes back to asking.
    /// A running agent keeps the launch allowance it had; the panel just no
    /// longer offers or accepts the mode.
    pub fn set_bypass_allowed(&mut self, allowed: bool) {
        self.allow_bypass = allowed;
        if !allowed && self.transcript.mode().skips_every_check() {
            if self.is_running() {
                self.command(&AgentCommand::SetMode(PermissionMode::AskBeforeEdits));
            }
            self.transcript.set_mode(PermissionMode::AskBeforeEdits);
        }
    }

    pub fn set_model(&mut self, value: &str) {
        if self.is_running() {
            self.command(&AgentCommand::SetModel(value.to_owned()));
        }
        self.model = Some(value.to_owned());
        self.last_change = Instant::now();
    }

    /// Ends the agent process; the conversation stays and can continue.
    pub fn close(&mut self) {
        self.handshake_deadline = None;
        if let Some(process) = &mut self.process
            && !self.closing
        {
            process.terminate();
            self.closing = true;
        }
    }

    /// Ends the agent here because a terminal continues the conversation.
    /// Two processes must never write one conversation, so this session
    /// refuses to relaunch.
    pub fn hand_off(&mut self) {
        self.close();
        self.handed_off = true;
        self.relaunch_on_exit = false;
        self.transcript
            .push_notice(NoticeKind::Info, "Continued in a terminal");
    }
}

impl Drop for AgentSession {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::agent_view::transcript::{Item, ToolState};
    use std::sync::Arc;

    fn fake() -> Option<AgentProgram> {
        let python = crate::agents::find_executable("python3")?;
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agent_view/fake_claude.py");
        Some(AgentProgram {
            program: python.display().to_string(),
            args: vec![script.display().to_string()],
            env: vec![("FAKE_CLAUDE_DELAY".into(), "0".into())],
        })
    }

    fn start(program: AgentProgram) -> AgentSession {
        AgentSession::start(
            program,
            StartOptions {
                cwd: std::env::temp_dir(),
                mode: PermissionMode::AskBeforeEdits,
                model: None,
                resume: None,
                allow_bypass: false,
            },
            Arc::new(|| {}),
        )
        .unwrap()
    }

    fn pump_until(session: &mut AgentSession, done: impl Fn(&AgentSession) -> bool) {
        let started = Instant::now();
        while !done(session) {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "timed out; items: {:#?}",
                session.transcript().items()
            );
            session.pump();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_missing_program_is_reported_before_anything_starts() {
        let error = AgentSession::start(
            AgentProgram {
                program: "/nonexistent/claude".into(),
                args: Vec::new(),
                env: Vec::new(),
            },
            StartOptions {
                cwd: std::env::temp_dir(),
                mode: PermissionMode::AskBeforeEdits,
                model: None,
                resume: None,
                allow_bypass: false,
            },
            Arc::new(|| {}),
        )
        .unwrap_err();
        assert_eq!(
            error,
            StartError::NotInstalled("/nonexistent/claude".into())
        );
    }

    #[test]
    fn a_conversation_runs_through_the_fake_agent() {
        let Some(program) = fake() else {
            eprintln!("skipping: python3 is unavailable");
            return;
        };
        let mut session = start(program);
        pump_until(&mut session, |session| {
            session.transcript().handshake().is_some()
        });
        assert_eq!(session.activity(), Activity::Idle);
        assert_eq!(session.model_label(), "Default");
        session.prompt("hello there", Vec::new()).unwrap();
        assert_eq!(session.title(), "hello there");
        pump_until(&mut session, |session| {
            session.activity() == Activity::Finished { success: true }
        });
        assert!(
            session
                .transcript()
                .last_answer()
                .unwrap()
                .contains("**bold**")
        );
        pump_until(&mut session, |session| {
            session.transcript().context().is_some()
        });
        assert_eq!(session.transcript().context().unwrap().max_tokens, 200_000);
        session.set_model("haiku");
        assert_eq!(session.model_label(), "Haiku 4.5");
    }

    #[test]
    fn approvals_round_trip_and_remember_rules() {
        let Some(program) = fake() else {
            return;
        };
        let mut session = start(program);
        session.prompt("please run the tests", Vec::new()).unwrap();
        pump_until(&mut session, |session| {
            session.activity() == Activity::NeedsYou
        });
        let approval = session.transcript().approvals()[0].clone();
        assert_eq!(approval.tool_name, "Bash");
        assert_eq!(
            approval.remember.as_ref().unwrap().label,
            "Always allow cargo test in this project"
        );
        assert!(session.allow(&approval.request_id, true, DecidedBy::Person));
        assert!(!session.allow(&approval.request_id, true, DecidedBy::Person));
        pump_until(&mut session, |session| {
            session.activity() == Activity::Finished { success: true }
        });
        let tool = session
            .transcript()
            .items()
            .iter()
            .find_map(|item| match item {
                Item::Tool(tool) if tool.name == "Bash" => Some(tool),
                _ => None,
            })
            .unwrap();
        assert_eq!(tool.state(false), ToolState::Succeeded);
        assert_eq!(
            tool.decision.as_ref().unwrap().remembered.as_deref(),
            Some("Always allow cargo test in this project")
        );
    }

    #[test]
    fn denials_questions_and_plans_are_answered() {
        let Some(program) = fake() else {
            return;
        };
        let mut session = start(program);
        session.prompt("edit the file", Vec::new()).unwrap();
        pump_until(&mut session, |session| {
            session.activity() == Activity::NeedsYou
        });
        let request = session.transcript().approvals()[0].request_id.clone();
        assert!(session.deny(&request, Some("not now"), DecidedBy::Person, false));
        pump_until(&mut session, |session| {
            matches!(session.activity(), Activity::Finished { .. })
        });
        assert!(
            session
                .transcript()
                .last_answer()
                .unwrap()
                .contains("left the file alone")
        );

        session.prompt("ask me something", Vec::new()).unwrap();
        pump_until(&mut session, |session| {
            session.activity() == Activity::NeedsYou
        });
        let request = session.transcript().approvals()[0].request_id.clone();
        assert!(session.answer_question(
            &request,
            &[QuestionAnswer {
                question: "Which approval style should new sessions use?".into(),
                answer: "Edit without asking".into(),
            }]
        ));
        pump_until(&mut session, |session| {
            matches!(session.activity(), Activity::Finished { .. })
        });
        assert!(
            session
                .transcript()
                .last_answer()
                .unwrap()
                .contains("Edit without asking")
        );

        session.prompt("make a plan", Vec::new()).unwrap();
        pump_until(&mut session, |session| {
            session.activity() == Activity::NeedsYou
        });
        let request = session.transcript().approvals()[0].request_id.clone();
        assert!(session.approve_plan(&request, PermissionMode::EditWithoutAsking));
        pump_until(&mut session, |session| {
            session.transcript().mode() == PermissionMode::EditWithoutAsking
                && matches!(session.activity(), Activity::Finished { .. })
        });
    }

    #[test]
    fn interrupting_ends_the_turn_and_a_closed_agent_relaunches() {
        let Some(mut program) = fake() else {
            return;
        };
        program.env = vec![("FAKE_CLAUDE_DELAY".into(), "0.02".into())];
        let mut session = start(program);
        session.prompt("go slow", Vec::new()).unwrap();
        pump_until(&mut session, |session| {
            session
                .transcript()
                .items()
                .iter()
                .any(|item| matches!(item, Item::Text { text, .. } if !text.is_empty()))
        });
        session.interrupt();
        assert!(session.transcript().interrupting());
        pump_until(&mut session, |session| !session.transcript().turn_active());
        let Some(Item::TurnEnd(end)) = session.transcript().items().last() else {
            panic!()
        };
        assert!(end.interrupted);

        session.close();
        assert!(!session.is_running());
        pump_until(&mut session, |session| {
            session.activity() == Activity::Exited
        });
        session.prompt("hello again", Vec::new()).unwrap();
        assert!(session.is_running());
        pump_until(&mut session, |session| {
            session.activity() == Activity::Finished { success: true }
        });
    }

    #[test]
    fn a_message_sent_while_closing_relaunches_once_the_agent_exits() {
        let Some(program) = fake() else {
            return;
        };
        let mut session = start(program);
        pump_until(&mut session, |session| {
            session.transcript().handshake().is_some()
        });
        session.close();
        // The old process takes no more input; the message waits for it.
        session.prompt("after closing", Vec::new()).unwrap();
        assert_eq!(session.queued().collect::<Vec<_>>(), ["after closing"]);
        pump_until(&mut session, |session| {
            session.activity() == Activity::Finished { success: true }
        });
        assert_eq!(session.queued().count(), 0);
        assert_eq!(session.transcript().first_prompt(), Some("after closing"));
    }

    #[test]
    fn a_continued_conversation_keeps_its_id_across_relaunches() {
        let Some(program) = fake() else {
            return;
        };
        let mut session = AgentSession::start(
            program,
            StartOptions {
                cwd: std::env::temp_dir(),
                mode: PermissionMode::AskBeforeEdits,
                model: None,
                resume: Some(("earlier-id".into(), "Earlier".into())),
                allow_bypass: false,
            },
            Arc::new(|| {}),
        )
        .unwrap();
        assert_eq!(session.conversation_id(), Some("earlier-id"));
        // Closed before any turn reported the id.
        session.close();
        pump_until(&mut session, |session| {
            session.activity() == Activity::Exited
        });
        session.prompt("carry on", Vec::new()).unwrap();
        pump_until(&mut session, |session| {
            session.activity() == Activity::Finished { success: true }
        });
        let reported = session.transcript().session().unwrap();
        assert_eq!(reported.session_id, "earlier-id");
    }

    #[test]
    fn a_conversation_handed_to_a_terminal_is_never_written_here_again() {
        let Some(program) = fake() else {
            return;
        };
        let mut session = start(program);
        session.prompt("hello", Vec::new()).unwrap();
        pump_until(&mut session, |session| {
            session.activity() == Activity::Finished { success: true }
        });
        session.hand_off();
        assert!(session.handed_off());
        assert_eq!(
            session.prompt("more", Vec::new()),
            Err(StartError::ContinuedInTerminal)
        );
        assert_eq!(session.relaunch(), Err(StartError::ContinuedInTerminal));
        pump_until(&mut session, |session| {
            session.activity() == Activity::Exited
        });
        assert!(!session.is_running());
    }

    #[test]
    fn messages_written_during_a_turn_wait_for_it_to_end() {
        let Some(mut program) = fake() else {
            return;
        };
        program.env = vec![("FAKE_CLAUDE_DELAY".into(), "0.02".into())];
        let mut session = start(program);
        session.prompt("go slow", Vec::new()).unwrap();
        session.prompt("and then this", Vec::new()).unwrap();
        session.prompt("and this", Vec::new()).unwrap();
        assert_eq!(
            session.queued().collect::<Vec<_>>(),
            ["and then this", "and this"]
        );
        assert_eq!(session.cancel_queued(1).as_deref(), Some("and this"));
        let users = |session: &AgentSession| {
            session
                .transcript()
                .items()
                .iter()
                .filter(|item| matches!(item, Item::User { .. }))
                .count()
        };
        assert_eq!(users(&session), 1);
        session.interrupt();
        pump_until(&mut session, |session| users(session) == 2);
        assert_eq!(session.queued().count(), 0);
        pump_until(&mut session, |session| {
            session.activity() == Activity::Finished { success: true }
        });
    }

    #[test]
    fn a_crash_is_explained_from_stderr() {
        let Some(program) = fake() else {
            return;
        };
        let mut session = start(program);
        session.prompt("crash now", Vec::new()).unwrap();
        pump_until(&mut session, |session| {
            session.activity() == Activity::Exited
        });
        let Some(Item::Notice(notice)) = session.transcript().items().last() else {
            panic!()
        };
        assert!(
            notice
                .text
                .ends_with("Error: the fake agent crashed on purpose"),
            "{}",
            notice.text
        );
    }
}
