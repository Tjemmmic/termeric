//! One conversation with an agent: its process, protocol, and transcript.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use super::agent::{AgentKind, Launch};
use super::event::{
    AgentCommand, AgentEvent, ApprovalDecision, DECLINED_MESSAGE, EarlierConversation, ModeChoice,
    model_name,
};
use super::process::{AgentProcess, LaunchSpec, Lookup, ProcessOutput, SendError, Waker};
use super::protocol::{Exchange, Protocol};
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
    pub agent: AgentKind,
    pub cwd: PathBuf,
    /// The id of the mode to start in, one of the agent's.
    pub mode: String,
    pub model: Option<String>,
    /// Continue an earlier conversation: its agent session id and title.
    pub resume: Option<(String, String)>,
    /// The person opted in to the mode that skips every check.
    pub allow_bypass: bool,
}

/// Why a session could not start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartError {
    NotInstalled {
        agent: AgentKind,
        program: String,
    },
    /// Termeric is still asking the login shell where programs are.
    StillLooking(AgentKind),
    Launch {
        agent: AgentKind,
        error: String,
    },
    /// The conversation was handed to a terminal, which now writes it.
    ContinuedInTerminal,
}

impl std::fmt::Display for StartError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::NotInstalled { agent, program } => {
                write!(formatter, "{} ({program}) is not installed", agent.name())
            }
            StartError::StillLooking(agent) => write!(
                formatter,
                "Still looking for {} on your login shell's PATH; try again in a moment",
                agent.name()
            ),
            StartError::Launch { agent, error } => {
                write!(formatter, "{} could not start: {error}", agent.name())
            }
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

/// A conversation with an agent.
pub struct AgentSession {
    id: SessionId,
    /// The agent, and so the protocol, this session drives.
    agent: AgentKind,
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
    /// The running process was started with that opt-in, so it may switch
    /// to such a mode without restarting.
    launched_with_bypass: bool,
    /// An unrestricted mode chosen during a turn, which needs a restart
    /// that waits for the turn to end.
    mode_after_turn: Option<String>,
    /// The protocol of the latest process.
    protocol: Option<Box<dyn Protocol>>,
    transcript: Transcript,
    backlog: VecDeque<String>,
    handshake_deadline: Option<Instant>,
    /// Mode to apply once an approved plan's tool call finishes.
    mode_after_plan: Option<(String, String)>,
    /// Messages written while a turn ran, sent in order as turns end.
    queued: VecDeque<(String, Vec<Attachment>)>,
    /// The agent's latest list of its earlier conversations in this
    /// folder, until the window takes it.
    earlier: Option<Vec<EarlierConversation>>,
    /// Something shown outside the conversation changed since the last pump.
    updated: bool,
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
        // A mode that skips every check needs the person's opt-in, whatever
        // asked for it.
        let unrestricted = options
            .agent
            .modes()
            .iter()
            .any(|mode| mode.unrestricted && mode.id == options.mode);
        let mode = if unrestricted && !options.allow_bypass {
            options.agent.default_mode().to_owned()
        } else {
            options.mode
        };
        let mut transcript = Transcript::new(options.agent.name(), mode);
        let (title, resume) = match options.resume {
            Some((session_id, title)) => {
                transcript.push_notice(NoticeKind::Info, "Continuing an earlier conversation");
                (Some(title), Some(session_id))
            }
            None => (None, None),
        };
        let mut session = Self {
            id: SessionId::next(),
            agent: options.agent,
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
            launched_with_bypass: false,
            mode_after_turn: None,
            protocol: None,
            transcript,
            backlog: VecDeque::new(),
            handshake_deadline: None,
            mode_after_plan: None,
            queued: VecDeque::new(),
            earlier: None,
            updated: false,
            waker,
            last_change: Instant::now(),
        };
        session.launch(resume)?;
        Ok(session)
    }

    fn launch(&mut self, resume: Option<String>) -> Result<(), StartError> {
        let agent = self.agent;
        let program = match super::process::find_program(&self.program.program, &self.waker) {
            Lookup::Found(program) => program,
            Lookup::Missing => {
                return Err(StartError::NotInstalled {
                    agent,
                    program: self.program.program.clone(),
                });
            }
            Lookup::Searching => return Err(StartError::StillLooking(agent)),
        };
        let (protocol_args, protocol) = agent.connect(&Launch {
            cwd: self.cwd.clone(),
            mode: self.transcript.mode().to_owned(),
            model: self.model.clone(),
            resume,
            allow_bypass: self.allow_bypass,
        });
        let mut args = self.program.args.clone();
        args.extend(protocol_args);
        let spec = LaunchSpec {
            program,
            args,
            cwd: self.cwd.clone(),
            env: self.program.env.clone(),
            env_remove: TERMINAL_ENVIRONMENT.map(String::from).to_vec(),
        };
        let process =
            AgentProcess::spawn(&spec, self.waker.clone()).map_err(|error| StartError::Launch {
                agent,
                error: error.to_string(),
            })?;
        self.backlog.clear();
        self.process = Some(process);
        self.launched_with_bypass = self.allow_bypass;
        let protocol = self.protocol.insert(protocol);
        for line in protocol.start() {
            self.send_line(line);
        }
        self.handshake_deadline = Some(Instant::now() + HANDSHAKE_TIMEOUT);
        Ok(())
    }

    /// A session with no agent process, for tests. Its program cannot
    /// exist, so nothing a test does can start a real agent.
    #[cfg(test)]
    pub fn detached(cwd: PathBuf, transcript: Transcript) -> Self {
        Self {
            id: SessionId::next(),
            agent: AgentKind::ClaudeCode,
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
            launched_with_bypass: false,
            mode_after_turn: None,
            protocol: None,
            transcript,
            backlog: VecDeque::new(),
            handshake_deadline: None,
            mode_after_plan: None,
            queued: VecDeque::new(),
            earlier: None,
            updated: false,
            waker: std::sync::Arc::new(|| {}),
            last_change: Instant::now(),
        }
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn agent(&self) -> AgentKind {
        self.agent
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

    /// The agent's earlier conversations in this session's folder, newest
    /// first, when it has listed them since the last call. The window keeps
    /// them, so they outlive the session. Agents that keep no such list, like
    /// Claude Code whose logs Termeric reads instead, never report one.
    pub fn take_earlier_conversations(&mut self) -> Option<Vec<EarlierConversation>> {
        self.earlier.take()
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
        let changed = self.transcript.generation() != before || std::mem::take(&mut self.updated);
        if changed {
            self.last_change = Instant::now();
        }
        changed
    }

    fn handle_line(&mut self, line: &str) {
        let Some(protocol) = self.protocol.as_mut() else {
            return;
        };
        let exchange = protocol.decode(line);
        self.apply(exchange);
    }

    /// Sends a protocol's lines and applies its events.
    fn apply(&mut self, exchange: Exchange) {
        for line in exchange.lines {
            self.send_line(line);
        }
        let mut refused = false;
        for event in exchange.events {
            // Not part of the conversation: the session keeps them.
            if let AgentEvent::EarlierConversations(earlier) = event {
                self.earlier = Some(earlier);
                self.updated = true;
                continue;
            }
            match &event {
                AgentEvent::Ready(_) => self.handshake_deadline = None,
                AgentEvent::HandshakeFailed(_) => refused = true,
                AgentEvent::Unrecognized(text) => {
                    log::debug!("agent view: unrecognized agent output: {text}");
                }
                AgentEvent::ToolResult { result, .. }
                    if self
                        .mode_after_plan
                        .as_ref()
                        .is_some_and(|(tool_use_id, _)| *tool_use_id == result.tool_use_id) =>
                {
                    let (_, mode) = self.mode_after_plan.take().expect("just matched");
                    if !result.is_error {
                        self.set_mode(&mode);
                    }
                }
                _ => {}
            }
            self.transcript.apply(event);
        }
        if refused {
            self.close();
            return;
        }
        if !self.transcript.turn_active()
            && let Some(mode) = self.mode_after_turn.take()
        {
            self.set_mode(&mode);
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
            format!(
                "{} did not answer. This version may not support the agent view; \
                 open it in a terminal instead.",
                self.agent.name()
            ),
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
        let Some(protocol) = self.protocol.as_mut() else {
            return;
        };
        let exchange = protocol.encode(command);
        self.apply(exchange);
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
        let text = note.clone().unwrap_or_else(|| DECLINED_MESSAGE.to_owned());
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
    pub fn approve_plan(&mut self, request_id: &str, mode: &str) -> bool {
        let tool_use_id = self
            .transcript
            .approval(request_id)
            .and_then(|approval| approval.tool_use_id.clone());
        if !self.allow(request_id, false, DecidedBy::Person) {
            return false;
        }
        match tool_use_id {
            Some(id) => self.mode_after_plan = Some((id, mode.to_owned())),
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

    /// The modes the agent offers: those it reported, else those it is
    /// known to have.
    pub fn modes(&self) -> Vec<ModeChoice> {
        self.transcript
            .handshake()
            .map(|handshake| handshake.modes.clone())
            .filter(|modes| !modes.is_empty())
            .unwrap_or_else(|| self.agent.modes())
    }

    /// Whether the agent runs in a mode that asks about nothing.
    pub fn unrestricted(&self) -> bool {
        self.modes()
            .iter()
            .any(|mode| mode.unrestricted && mode.id == self.transcript.mode())
    }

    /// Switches to the mode with id `mode`. An unrestricted mode is refused
    /// unless the person opted in to such modes.
    pub fn set_mode(&mut self, mode: &str) {
        let unrestricted = self
            .modes()
            .iter()
            .any(|choice| choice.id == mode && choice.unrestricted);
        if unrestricted && !self.allow_bypass {
            return;
        }
        self.mode_after_turn = None;
        let restart = unrestricted
            && self.agent.unrestricted_needs_launch_allowance()
            && self.is_running()
            && !self.launched_with_bypass;
        if restart {
            if self.transcript.turn_active() {
                self.mode_after_turn = Some(mode.to_owned());
                self.transcript.push_notice(
                    NoticeKind::Info,
                    format!(
                        "{} restarts in this mode once the turn ends",
                        self.agent.name()
                    ),
                );
                return;
            }
            // Started without the allowance: restart with it, continuing
            // the conversation in the new mode.
            self.transcript.set_mode(mode);
            self.close();
            self.relaunch_on_exit = true;
            return;
        }
        if self.is_running() {
            self.command(&AgentCommand::SetMode(mode.to_owned()));
        }
        self.transcript.set_mode(mode);
    }

    /// Follows the person's opt-in to the mode that skips every check. A
    /// session in that mode when the opt-in is withdrawn goes back to asking.
    /// A running agent keeps the launch allowance it had; the panel just no
    /// longer offers or accepts the mode.
    pub fn set_bypass_allowed(&mut self, allowed: bool) {
        self.allow_bypass = allowed;
        if !allowed && self.unrestricted() {
            let asking = self.agent.default_mode();
            if self.is_running() {
                self.command(&AgentCommand::SetMode(asking.to_owned()));
            }
            self.transcript.set_mode(asking);
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
                agent: AgentKind::ClaudeCode,
                mode: "default".into(),
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
                agent: AgentKind::ClaudeCode,
                mode: "default".into(),
                model: None,
                resume: None,
                allow_bypass: false,
            },
            Arc::new(|| {}),
        )
        .unwrap_err();
        assert_eq!(
            error,
            StartError::NotInstalled {
                agent: AgentKind::ClaudeCode,
                program: "/nonexistent/claude".into(),
            }
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
        assert!(session.approve_plan(&request, "acceptEdits"));
        pump_until(&mut session, |session| {
            session.transcript().mode() == "acceptEdits"
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
    fn claude_restarts_to_bypass_when_the_opt_in_came_after_it_started() {
        let Some(mut program) = fake() else {
            return;
        };
        program.env = vec![("FAKE_CLAUDE_DELAY".into(), "0.02".into())];
        let mut session = start(program);
        session.prompt("hello", Vec::new()).unwrap();
        pump_until(&mut session, |session| {
            session.activity() == Activity::Finished { success: true }
        });
        let conversation = session.conversation_id().unwrap().to_owned();
        session.set_bypass_allowed(true);

        // Mid-turn, the restart waits for the turn to end.
        session.prompt("go slow", Vec::new()).unwrap();
        session.set_mode("bypassPermissions");
        assert_eq!(session.transcript().mode(), "default");
        session.interrupt();
        pump_until(&mut session, |session| {
            session.transcript().mode() == "bypassPermissions" && session.is_running()
        });
        pump_until(&mut session, |session| {
            session.transcript().handshake().is_some()
        });
        session.prompt("and now", Vec::new()).unwrap();
        pump_until(&mut session, |session| {
            session.activity() == Activity::Finished { success: true }
        });
        let info = session.transcript().session().unwrap();
        assert_eq!(info.permission_mode.as_deref(), Some("bypassPermissions"));
        assert_eq!(session.conversation_id(), Some(conversation.as_str()));
        // Refused switches would have left a warning.
        assert!(!session.transcript().items().iter().any(|item| matches!(
            item,
            Item::Notice(notice) if notice.kind == NoticeKind::Warning
        )));
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
                agent: AgentKind::ClaudeCode,
                mode: "default".into(),
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

    fn fake_codex() -> Option<AgentProgram> {
        let python = crate::agents::find_executable("python3")?;
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agent_view/fake_codex.py");
        Some(AgentProgram {
            program: python.display().to_string(),
            args: vec![script.display().to_string()],
            env: vec![("FAKE_CODEX_DELAY".into(), "0".into())],
        })
    }

    fn start_codex(program: AgentProgram, resume: Option<(String, String)>) -> AgentSession {
        AgentSession::start(
            program,
            StartOptions {
                cwd: std::env::temp_dir(),
                agent: AgentKind::Codex,
                mode: "read-only".into(),
                model: None,
                resume,
                allow_bypass: false,
            },
            Arc::new(|| {}),
        )
        .unwrap()
    }

    fn finished(session: &AgentSession) -> bool {
        matches!(session.activity(), Activity::Finished { .. })
    }

    #[test]
    fn a_codex_conversation_runs_through_the_fake_app_server() {
        let Some(program) = fake_codex() else {
            eprintln!("skipping: python3 is unavailable");
            return;
        };
        let mut session = start_codex(program, None);
        pump_until(&mut session, |session| {
            session
                .transcript()
                .handshake()
                .is_some_and(|handshake| handshake.models.len() > 1)
        });
        // The thread opens with the first message, so nothing is kept yet.
        assert_eq!(session.conversation_id(), None);
        let handshake = session.transcript().handshake().unwrap();
        let models: Vec<&str> = handshake
            .models
            .iter()
            .map(|model| model.value.as_str())
            .collect();
        assert_eq!(models, ["default", "gpt-fake-5", "gpt-fake-mini"]);
        pump_until(&mut session, |session| {
            let commands = &session.transcript().handshake().unwrap().commands;
            commands.iter().any(|command| command.name == "deploy")
        });
        let commands: Vec<String> = session
            .transcript()
            .handshake()
            .unwrap()
            .commands
            .iter()
            .map(|command| command.name.clone())
            .collect();
        assert_eq!(commands, ["compact", "review", "deploy"]);
        let deploy = &session.transcript().handshake().unwrap().commands[2];
        assert_eq!(deploy.description, "Deploy the app");
        assert_eq!(session.model_label(), "Default");

        session.prompt("hello there", Vec::new()).unwrap();
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
        let context = session.transcript().context().unwrap();
        assert_eq!((context.used_tokens, context.max_tokens), (64_000, 256_000));
        let thread = session.conversation_id().unwrap().to_owned();
        assert_eq!(session.transcript().session().unwrap().session_id, thread);

        session.prompt("please fail", Vec::new()).unwrap();
        pump_until(&mut session, finished);
        let Some(Item::TurnEnd(end)) = session.transcript().items().last() else {
            panic!()
        };
        assert!(!end.outcome.success);
        assert_eq!(
            end.outcome.error.as_deref(),
            Some("The fake Codex failed on purpose. Codex's usage limit was reached.")
        );
    }

    #[test]
    fn codex_commands_and_patches_are_approved_and_declined() {
        let Some(program) = fake_codex() else {
            return;
        };
        let mut session = start_codex(program, None);
        session.prompt("please run the tests", Vec::new()).unwrap();
        pump_until(&mut session, |session| {
            session.activity() == Activity::NeedsYou
        });
        let approval = session.transcript().approvals()[0].clone();
        assert_eq!(approval.tool_name, "Bash");
        // The login shell Codex wraps commands in is not shown.
        assert_eq!(
            approval.input["command"],
            "cargo test -p termeric scrollbar"
        );
        assert_eq!(approval.reason.as_deref(), Some("It runs the test suite"));
        assert_eq!(
            approval.remember.as_ref().unwrap().label,
            "Always allow cargo test commands"
        );
        assert!(session.allow(&approval.request_id, true, DecidedBy::Person));
        pump_until(&mut session, finished);
        assert!(
            session
                .transcript()
                .last_answer()
                .unwrap()
                .contains("acceptWithExecpolicyAmendment"),
            "the remembered rule reached Codex"
        );
        let tool = session
            .transcript()
            .tool(&approval.tool_use_id.clone().unwrap())
            .unwrap();
        assert_eq!(tool.state(false), ToolState::Succeeded);

        session.prompt("edit the answer", Vec::new()).unwrap();
        pump_until(&mut session, |session| {
            session.activity() == Activity::NeedsYou
        });
        let approval = session.transcript().approvals()[0].clone();
        assert_eq!(approval.tool_name, crate::agent_view::present::PATCH_TOOL);
        assert_eq!(
            crate::agent_view::present::patch_paths(&approval.input).len(),
            2
        );
        // Codex takes no reason with a refusal; the note follows as input.
        assert!(session.deny(
            &approval.request_id,
            Some("not now"),
            DecidedBy::Person,
            false
        ));
        pump_until(&mut session, finished);
        assert!(
            session
                .transcript()
                .last_answer()
                .unwrap()
                .ends_with("You said: not now")
        );
        let tool = session
            .transcript()
            .tool(&approval.tool_use_id.clone().unwrap())
            .unwrap();
        assert_eq!(tool.state(false), ToolState::Denied);
        let Some(Item::TurnEnd(end)) = session.transcript().items().last() else {
            panic!()
        };
        assert!(end.files_changed.is_empty());

        session.prompt("edit it after all", Vec::new()).unwrap();
        pump_until(&mut session, |session| {
            session.activity() == Activity::NeedsYou
        });
        let request = session.transcript().approvals()[0].request_id.clone();
        assert!(session.allow(&request, false, DecidedBy::Person));
        pump_until(&mut session, finished);
        let Some(Item::TurnEnd(end)) = session.transcript().items().last() else {
            panic!()
        };
        assert_eq!(end.files_changed.len(), 2);
    }

    #[test]
    fn codex_lists_the_folders_earlier_threads_and_continues_one() {
        let Some(program) = fake_codex() else {
            return;
        };
        let mut session = start_codex(program.clone(), None);
        let started = Instant::now();
        let listed = loop {
            session.pump();
            if let Some(listed) = session.take_earlier_conversations() {
                break listed;
            }
            assert!(started.elapsed() < Duration::from_secs(20));
            std::thread::sleep(Duration::from_millis(5));
        };
        let earlier: Vec<(&str, &str)> = listed
            .iter()
            .map(|past| (past.id.as_str(), past.title.as_str()))
            .collect();
        // Scripted `codex exec` runs are not offered.
        assert_eq!(
            earlier,
            [
                ("earlier-named", "Fix the flaky test"),
                ("earlier-preview", "Tidy the README")
            ]
        );

        let mut continued = start_codex(
            program,
            Some(("earlier-named".into(), "Fix the flaky test".into())),
        );
        assert_eq!(continued.title(), "Fix the flaky test");
        continued.prompt("hello", Vec::new()).unwrap();
        pump_until(&mut continued, finished);
        assert_eq!(continued.conversation_id(), Some("earlier-named"));
    }

    #[test]
    fn a_person_denying_a_codex_change_stops_the_turn() {
        let Some(program) = fake_codex() else {
            return;
        };
        let mut session = start_codex(program, None);
        session.prompt("edit the answer", Vec::new()).unwrap();
        pump_until(&mut session, |session| {
            session.activity() == Activity::NeedsYou
        });
        let request = session.transcript().approvals()[0].request_id.clone();
        assert!(session.deny(&request, None, DecidedBy::Person, true));
        pump_until(&mut session, |session| !session.transcript().turn_active());
        assert_eq!(session.activity(), Activity::Stopped);
    }

    #[test]
    fn codex_questions_plans_and_reasoning_reach_the_transcript() {
        let Some(program) = fake_codex() else {
            return;
        };
        let mut session = start_codex(program, None);
        session.prompt("ask me something", Vec::new()).unwrap();
        pump_until(&mut session, |session| {
            session.activity() == Activity::NeedsYou
        });
        let approval = session.transcript().approvals()[0].clone();
        assert_eq!(approval.tool_name, "AskUserQuestion");
        assert!(session.answer_question(
            &approval.request_id,
            &[QuestionAnswer {
                question: "Which approval style should new sessions use?".into(),
                answer: "Auto".into(),
            }]
        ));
        pump_until(&mut session, finished);
        assert!(
            session
                .transcript()
                .last_answer()
                .unwrap()
                .contains("**Auto**")
        );

        session.prompt("keep a todo list", Vec::new()).unwrap();
        pump_until(&mut session, finished);
        let todo = session
            .transcript()
            .items()
            .iter()
            .find_map(|item| match item {
                Item::Tool(tool) if tool.name == "TodoWrite" => Some(tool),
                _ => None,
            })
            .unwrap();
        let steps = crate::agent_view::present::todos(todo).unwrap();
        assert_eq!(steps.len(), 3);

        session.prompt("edit", Vec::new()).unwrap();
        pump_until(&mut session, |session| {
            session.activity() == Activity::NeedsYou
        });
        assert!(session.transcript().items().iter().any(|item| matches!(
            item,
            Item::Thinking { text, .. } if text == "The answer is off by one."
        )));
    }

    #[test]
    fn codex_takes_the_mode_model_and_skill_with_each_message() {
        let Some(program) = fake_codex() else {
            return;
        };
        let mut session = start_codex(program, None);
        let answer = |session: &mut AgentSession, prompt: &str| {
            session.prompt(prompt, Vec::new()).unwrap();
            pump_until(session, finished);
            session.transcript().last_answer().unwrap().to_owned()
        };
        assert_eq!(
            answer(&mut session, "settings"),
            "sandbox readOnly, approval on-request, model gpt-fake-5, skills none"
        );
        session.set_mode("auto");
        session.set_model("gpt-fake-mini");
        assert_eq!(
            answer(&mut session, "settings"),
            "sandbox workspaceWrite, approval on-request, model gpt-fake-mini, skills none"
        );
        // Back to the default: the model the thread opened with.
        session.set_model("default");
        pump_until(&mut session, |session| {
            session
                .transcript()
                .handshake()
                .is_some_and(|handshake| handshake.commands.len() > 2)
        });
        assert_eq!(
            answer(&mut session, "/deploy settings"),
            "sandbox workspaceWrite, approval on-request, model gpt-fake-5, skills deploy"
        );
        // Full access needs the opt-in.
        session.set_mode("full-access");
        assert_eq!(session.transcript().mode(), "auto");
        session.set_bypass_allowed(true);
        session.set_mode("full-access");
        assert!(session.unrestricted());
        assert_eq!(
            answer(&mut session, "settings"),
            "sandbox dangerFullAccess, approval never, model gpt-fake-5, skills none"
        );
        session.set_bypass_allowed(false);
        assert_eq!(session.transcript().mode(), "read-only");
    }

    #[test]
    fn a_session_never_starts_unrestricted_without_the_opt_in() {
        let Some(program) = fake_codex() else {
            return;
        };
        let mut session = AgentSession::start(
            program,
            StartOptions {
                cwd: std::env::temp_dir(),
                agent: AgentKind::Codex,
                mode: "full-access".into(),
                model: Some("default".into()),
                resume: None,
                allow_bypass: false,
            },
            Arc::new(|| {}),
        )
        .unwrap();
        assert_eq!(session.transcript().mode(), "read-only");
        session.prompt("settings", Vec::new()).unwrap();
        pump_until(&mut session, finished);
        assert_eq!(
            session.transcript().last_answer(),
            Some("sandbox readOnly, approval on-request, model gpt-fake-5, skills none")
        );
    }

    #[test]
    fn codex_compacts_reviews_and_refuses_terminal_commands() {
        let Some(program) = fake_codex() else {
            return;
        };
        let mut session = start_codex(program, None);
        session.prompt("/compact", Vec::new()).unwrap();
        pump_until(&mut session, finished);
        let Some(Item::TurnEnd(end)) = session.transcript().items().last() else {
            panic!()
        };
        assert_eq!(
            end.outcome.error.as_deref(),
            Some("There is nothing to compact yet")
        );

        session
            .prompt("/review focus on errors", Vec::new())
            .unwrap();
        pump_until(&mut session, |session| {
            session.activity() == Activity::Finished { success: true }
        });
        assert_eq!(
            session.transcript().last_answer(),
            Some("Reviewed focus on errors: looks good.")
        );
        session.prompt("/compact", Vec::new()).unwrap();
        pump_until(&mut session, |session| {
            session.activity() == Activity::Finished { success: true }
        });
        assert!(!session.transcript().compacting());

        // A command Codex runs only in its terminal never reaches the model.
        let answers = session.transcript().items().len();
        session.prompt("/status", Vec::new()).unwrap();
        assert_eq!(session.activity(), Activity::Finished { success: true });
        assert_eq!(session.transcript().items().len(), answers + 2);
    }

    #[test]
    fn codex_turns_stop_and_a_closed_codex_continues_its_thread() {
        let Some(mut program) = fake_codex() else {
            return;
        };
        program.env = vec![("FAKE_CODEX_DELAY".into(), "0.02".into())];
        let mut session = start_codex(program, None);
        session.prompt("go slow", Vec::new()).unwrap();
        pump_until(&mut session, |session| {
            session
                .transcript()
                .items()
                .iter()
                .any(|item| matches!(item, Item::Text { text, .. } if !text.is_empty()))
        });
        session.interrupt();
        pump_until(&mut session, |session| !session.transcript().turn_active());
        assert_eq!(session.activity(), Activity::Stopped);
        let thread = session.conversation_id().unwrap().to_owned();

        session.close();
        pump_until(&mut session, |session| {
            session.activity() == Activity::Exited
        });
        session.prompt("hello again", Vec::new()).unwrap();
        pump_until(&mut session, finished);
        assert_eq!(session.conversation_id(), Some(thread.as_str()));
    }

    #[test]
    fn a_codex_thread_that_cannot_continue_starts_anew_and_says_so() {
        let Some(program) = fake_codex() else {
            return;
        };
        let mut session = start_codex(program, Some(("missing".into(), "Gone".into())));
        session.prompt("hello", Vec::new()).unwrap();
        pump_until(&mut session, finished);
        assert_eq!(session.activity(), Activity::Finished { success: true });
        assert_ne!(session.conversation_id(), Some("missing"));
        assert!(session.transcript().items().iter().any(|item| matches!(
            item,
            Item::Notice(notice) if notice.text.starts_with("Codex could not continue")
        )));
    }

    #[test]
    fn a_signed_out_codex_is_explained_before_the_first_message() {
        let Some(mut program) = fake_codex() else {
            return;
        };
        program
            .env
            .push(("FAKE_CODEX_SIGNED_OUT".into(), "1".into()));
        let mut session = start_codex(program, None);
        pump_until(&mut session, |session| {
            session
                .transcript()
                .items()
                .iter()
                .any(|item| matches!(item, Item::Notice(_)))
        });
        let Some(Item::Notice(notice)) = session.transcript().items().first() else {
            panic!()
        };
        assert!(notice.text.starts_with("Codex is not signed in"));
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
