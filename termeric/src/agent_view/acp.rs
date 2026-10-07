//! The Agent Client Protocol (ACP), which OpenCode speaks as `opencode acp`.
//!
//! ACP is JSON-RPC 2.0 over the process's pipes, one message per line. A
//! conversation is a session. Each message the person sends is a
//! `session/prompt` request that stays open for the whole turn, while
//! `session/update` notifications stream its text, reasoning, and tool calls.
//! The agent asks before acting through `session/request_permission`. Every
//! request the agent makes gets a reply, even one this host does not serve:
//! the agent reads and writes files and runs commands itself, and one left
//! unanswered could stall it.
//!
//! The session opens as soon as the agent accepts the connection, because
//! the models, modes, effort levels, and slash commands an agent offers come
//! with a session; a session nothing was said in is the host's to forget (see
//! [`super::agent::AgentKind::forget_args`]). Permission requests can be
//! answered here without asking, as an agent's own interface may do when told
//! to approve everything. Tool calls are
//! translated into the vocabulary the panel presents (see [`super::present`])
//! from each call's kind, title, and input, so reads read as `Read`, commands
//! as `Bash`, and edits as `Edit`, `Write`, or `Patch`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Map, Value, json};

use super::event::{
    AgentCommand, AgentEvent, ApprovalDecision, ApprovalRequest, AssistantMessage, BlockStart,
    ContentBlock, ContextUsage, DECLINED_MESSAGE, EarlierConversation, EffortChoice, Handshake,
    ModeChoice, ModelChoice, RememberOption, SessionInfo, SlashCommand, StreamDelta, Thread,
    TurnOutcome, earlier_title,
};
use super::present::PATCH_TOOL;
use super::protocol::{
    Exchange, Incoming, Protocol, failed_turn, incoming, line, preview, text, tool_result, tool_use,
};
use crate::safe_text::capitalized;

/// The protocol version this host speaks.
const PROTOCOL_VERSION: u64 = 1;
/// Earlier sessions listed for the folder, newest first.
const EARLIER_LIMIT: usize = 20;
/// The error an agent answers with until its person signs in.
const AUTH_REQUIRED: i64 = -32000;

/// How the conversation starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    /// The agent's name, as the panel shows it.
    pub agent: &'static str,
    pub cwd: PathBuf,
    /// The id of the mode to run in.
    pub mode: String,
    /// A model the agent offered; `None` keeps the one its settings choose.
    pub model: Option<String>,
    /// A level of effort the model offered; `None` keeps the agent's.
    pub effort: Option<String>,
    /// Allow everything the agent asks about without asking the person.
    pub auto_approve: bool,
    /// Continue an earlier session by its id.
    pub resume: Option<String>,
}

/// What a request the panel sent was for, so its reply can be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Request {
    Initialize,
    List,
    NewSession,
    Resume,
    /// A change of mode or model.
    Configure,
    Prompt,
}

/// A permission request waiting for the person's answer.
#[derive(Clone, Debug)]
struct Asked {
    /// The agent's own id for the request, echoed in the reply.
    id: Value,
    /// Each option's id and kind, e.g. `allow_once` or `reject_once`.
    options: Vec<(String, String)>,
}

impl Asked {
    /// The first option of any of `kinds`, in the order given.
    fn option(&self, kinds: &[&str]) -> Option<String> {
        kinds.iter().find_map(|kind| {
            self.options
                .iter()
                .find(|(_, option_kind)| option_kind == kind)
                .map(|(id, _)| id.clone())
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SessionState {
    None,
    Opening,
    Open(String),
}

/// A message streaming in.
#[derive(Clone, Debug)]
struct Live {
    id: String,
    thinking: bool,
    text: String,
}

/// Everything the agent reported about one tool call. Updates carry only
/// what changed, so each call is kept whole here.
#[derive(Clone, Debug, Default, PartialEq)]
struct ToolState {
    kind: String,
    title: String,
    status: String,
    raw_input: Value,
    raw_output: Value,
    locations: Vec<String>,
    content: Vec<Value>,
}

impl ToolState {
    /// Takes the fields `update` names; `locations` and `content` replace
    /// the ones held rather than adding to them.
    fn update(&mut self, update: &Value) {
        for (key, target) in [
            ("kind", &mut self.kind),
            ("title", &mut self.title),
            ("status", &mut self.status),
        ] {
            if let Some(value) = update.get(key).and_then(Value::as_str) {
                value.clone_into(target);
            }
        }
        if let Some(input) = update.get("rawInput").filter(|input| !input.is_null()) {
            self.raw_input = input.clone();
        }
        if let Some(output) = update.get("rawOutput").filter(|output| !output.is_null()) {
            self.raw_output = output.clone();
        }
        if let Some(locations) = update.get("locations").and_then(Value::as_array) {
            self.locations = locations
                .iter()
                .filter_map(|location| text(location, "path"))
                .collect();
        }
        if let Some(content) = update.get("content").and_then(Value::as_array) {
            self.content.clone_from(content);
        }
    }

    /// Fills in what this call lacks from another report of it, such as the
    /// one a permission request carries.
    fn fill(&mut self, other: &Value) {
        let mut reported = ToolState::default();
        reported.update(other);
        for (field, value) in [
            (&mut self.kind, reported.kind),
            (&mut self.title, reported.title),
        ] {
            if field.is_empty() {
                *field = value;
            }
        }
        if is_empty(&self.raw_input) {
            self.raw_input = reported.raw_input;
        }
        if self.locations.is_empty() {
            self.locations = reported.locations;
        }
        if self.content.is_empty() {
            self.content = reported.content;
        }
    }

    /// Each proposed or made change: its path, the text before (none for a
    /// new file), and the text after.
    fn diffs(&self) -> Vec<(&str, Option<&str>, &str)> {
        self.content
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("diff"))
            .filter_map(|diff| {
                Some((
                    diff.get("path")?.as_str()?,
                    diff.get("oldText").and_then(Value::as_str),
                    diff.get("newText")?.as_str()?,
                ))
            })
            .collect()
    }

    /// The text the model saw from the call.
    fn output(&self) -> String {
        let texts: Vec<&str> = self
            .content
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("content"))
            .filter_map(|item| item.pointer("/content/text").and_then(Value::as_str))
            .collect();
        if !texts.is_empty() {
            return texts.join("\n");
        }
        ["output", "error"]
            .iter()
            .find_map(|key| text(&self.raw_output, key))
            .unwrap_or_default()
    }
}

/// Encodes commands and decodes output for one ACP agent process.
#[derive(Debug)]
pub struct AcpCodec {
    agent: &'static str,
    cwd: String,
    mode: String,
    model: Option<String>,
    resume: Option<String>,
    next_request: u64,
    pending: HashMap<u64, Request>,
    initialized: bool,
    can_list: bool,
    can_resume: bool,
    version: String,
    /// What the agent says to do when it needs its person to sign in.
    sign_in: Option<String>,
    session: SessionState,
    /// The session's settings (`configOptions`), as the agent last sent them.
    options: Vec<Value>,
    /// The session's modes, from agents that name them apart from settings.
    listed_modes: Option<Value>,
    effort: Option<String>,
    auto_approve: bool,
    /// A model change is on its way, and the effort levels it offers are
    /// the new model's: set the effort once it lands.
    effort_after_model: bool,
    commands: Vec<SlashCommand>,
    /// Setting changes the agent has not answered, as (request id, setting
    /// id, value) in the order sent; a message waits for them.
    in_flight: Vec<(u64, String, String)>,
    /// A message waiting for the session to open or its settings to apply.
    waiting: Option<String>,
    /// A turn is running: its prompt has not been answered.
    prompting: bool,
    turn_started: Option<Instant>,
    turns: u64,
    asked: HashMap<String, Asked>,
    tools: HashMap<String, ToolState>,
    live: Option<Live>,
    /// Messages the agent gave no id, numbered.
    unnamed: u64,
    /// The session's cost so far, when the agent reports one.
    cost: Option<f64>,
}

impl AcpCodec {
    pub fn new(setup: Setup) -> Self {
        Self {
            agent: setup.agent,
            cwd: setup.cwd.display().to_string(),
            mode: setup.mode,
            model: setup.model,
            effort: setup.effort,
            auto_approve: setup.auto_approve,
            resume: setup.resume,
            next_request: 0,
            pending: HashMap::new(),
            initialized: false,
            can_list: false,
            can_resume: false,
            version: String::new(),
            sign_in: None,
            session: SessionState::None,
            options: Vec::new(),
            listed_modes: None,
            effort_after_model: false,
            commands: Vec::new(),
            in_flight: Vec::new(),
            waiting: None,
            prompting: false,
            turn_started: None,
            turns: 0,
            asked: HashMap::new(),
            tools: HashMap::new(),
            live: None,
            unnamed: 0,
            cost: None,
        }
    }

    fn request(&mut self, kind: Request, method: &str, params: Value) -> String {
        self.next_request += 1;
        self.pending.insert(self.next_request, kind);
        line(&json!({
            "jsonrpc": "2.0",
            "id": self.next_request,
            "method": method,
            "params": params,
        }))
    }

    fn session_id(&self) -> Option<&str> {
        match &self.session {
            SessionState::Open(id) => Some(id),
            _ => None,
        }
    }

    // ── Settings ─────────────────────────────────────────────────────────

    /// The session's setting in `category`, e.g. `mode` or `model`.
    fn option(&self, category: &str) -> Option<&Value> {
        self.options.iter().find(|option| {
            text(option, "category").as_deref() == Some(category)
                && text(option, "type").as_deref() == Some("select")
        })
    }

    /// Opens a session: continues the earlier one when asked to, else
    /// starts a new one.
    fn open_session(&mut self) -> String {
        self.session = SessionState::Opening;
        let mut params = json!({ "cwd": self.cwd, "mcpServers": [] });
        match self.resume.clone() {
            Some(session) => {
                params["sessionId"] = Value::String(session);
                self.request(Request::Resume, "session/resume", params)
            }
            None => self.request(Request::NewSession, "session/new", params),
        }
    }

    /// The change that sets the session's `category` setting to `value`, as
    /// (setting id, value), when the session offers that value and runs
    /// another.
    fn change(&self, category: &str, value: &str) -> Option<(String, String)> {
        let option = self.option(category)?;
        let id = text(option, "id").unwrap_or_default();
        let offered = choices(option).iter().any(|(choice, ..)| choice == value);
        // A change on its way counts as made.
        let current = self
            .in_flight
            .iter()
            .rev()
            .find(|(_, setting, _)| *setting == id)
            .map(|(_, _, value)| value.clone())
            .or_else(|| text(option, "currentValue"));
        (offered && current.as_deref() != Some(value)).then(|| (id, value.to_owned()))
    }

    /// Sends one setting change.
    fn send_change(&mut self, method: &str, params: Value, setting: &str, value: &str) -> String {
        let line = self.request(Request::Configure, method, params);
        self.in_flight
            .push((self.next_request, setting.to_owned(), value.to_owned()));
        line
    }

    /// Asks the session to run in the chosen mode, model, and effort where it
    /// runs in others. A message waits until the agent answers.
    fn configure(&mut self) -> Exchange {
        let mut exchange = Exchange::default();
        let Some(session) = self.session_id().map(str::to_owned) else {
            return exchange;
        };
        let mut changes = Vec::new();
        if let Some(option) = self.option("mode") {
            let offered = choices(option)
                .iter()
                .any(|(choice, ..)| *choice == self.mode);
            match self.change("mode", &self.mode) {
                Some(change) => changes.push(change),
                // The chosen mode is current or already on its way.
                None if offered => {}
                None => {
                    // A mode this session does not have: say which it runs in.
                    if let Some(current) = self
                        .option("mode")
                        .and_then(|option| text(option, "currentValue"))
                        .filter(|current| *current != self.mode)
                    {
                        self.mode.clone_from(&current);
                        exchange.events.push(AgentEvent::ModeChanged(current));
                    }
                }
            }
        } else if let Some(modes) = &self.listed_modes {
            let offered = modes
                .get("availableModes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .any(|mode| text(mode, "id").as_deref() == Some(self.mode.as_str()));
            let pending = self
                .in_flight
                .iter()
                .rev()
                .find(|(_, setting, _)| setting == "session/set_mode")
                .map(|(_, _, value)| value.clone())
                .or_else(|| text(modes, "currentModeId"));
            if offered && pending.as_deref() != Some(self.mode.as_str()) {
                let params = json!({ "sessionId": session, "modeId": self.mode });
                let mode = self.mode.clone();
                let line = self.send_change("session/set_mode", params, "session/set_mode", &mode);
                exchange.lines.push(line);
            }
        }
        let model = self
            .model
            .as_deref()
            .and_then(|model| self.change("model", model));
        match model {
            Some(change) => {
                changes.push(change);
                self.effort_after_model = self.effort.is_some();
            }
            None => {
                let model_on_its_way = self
                    .option("model")
                    .and_then(|option| text(option, "id"))
                    .is_some_and(|id| self.in_flight.iter().any(|(_, setting, _)| *setting == id));
                if model_on_its_way {
                    // The effort levels are the new model's: wait for it.
                    self.effort_after_model |= self.effort.is_some();
                } else {
                    changes.extend(
                        self.effort
                            .as_deref()
                            .and_then(|effort| self.change("thought_level", effort)),
                    );
                }
            }
        }
        for (option, value) in changes {
            let params = json!({ "sessionId": session, "configId": option, "value": value });
            let line = self.send_change("session/set_config_option", params, &option, &value);
            exchange.lines.push(line);
        }
        exchange
    }

    fn handshake(&self) -> Handshake {
        let current = |category: &str| {
            self.option(category)
                .and_then(|option| text(option, "currentValue"))
        };
        let models = self.option("model").map(model_choices).unwrap_or_default();
        let efforts = self
            .option("thought_level")
            .map(choices)
            .unwrap_or_default()
            .into_iter()
            .map(|(value, name, description)| EffortChoice {
                value,
                label: capitalized(&name),
                description,
            })
            .collect();
        let modes = match (self.option("mode"), &self.listed_modes) {
            (Some(option), _) => choices(option)
                .into_iter()
                .map(|(id, name, description)| mode_choice(&id, &name, &description))
                .collect(),
            (None, Some(modes)) => modes
                .get("availableModes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|mode| {
                    let id = text(mode, "id")?;
                    let name = text(mode, "name").unwrap_or_else(|| id.clone());
                    Some(mode_choice(
                        &id,
                        &name,
                        &text(mode, "description").unwrap_or_default(),
                    ))
                })
                .collect(),
            (None, None) => Vec::new(),
        };
        Handshake {
            models,
            commands: self.commands.clone(),
            modes,
            permission_mode: None,
            current_model: current("model"),
            efforts,
            current_effort: current("thought_level"),
        }
    }

    /// Replaces the session's settings. A mode the agent switched to on its
    /// own becomes the session's.
    fn take_options(&mut self, options: Option<&Value>) -> Vec<AgentEvent> {
        let Some(options) = options.and_then(Value::as_array) else {
            return Vec::new();
        };
        self.options.clone_from(options);
        let mut events = vec![AgentEvent::Ready(self.handshake())];
        if let Some(current) = self
            .option("mode")
            .and_then(|option| text(option, "currentValue"))
            .filter(|current| *current != self.mode && self.in_flight.is_empty())
        {
            self.mode.clone_from(&current);
            events.push(AgentEvent::ModeChanged(current));
        }
        events
    }

    // ── Commands ─────────────────────────────────────────────────────────

    fn encode_prompt(&mut self, text: &str) -> Exchange {
        if self.session_id().is_some() && self.in_flight.is_empty() {
            return Exchange::send(vec![self.send_prompt(text)]);
        }
        self.waiting = Some(text.to_owned());
        // A session that could not open is tried again for the message.
        if self.initialized && self.session == SessionState::None {
            return Exchange::send(vec![self.open_session()]);
        }
        Exchange::default()
    }

    fn send_prompt(&mut self, text: &str) -> String {
        let session = self.session_id().unwrap_or_default().to_owned();
        self.prompting = true;
        self.turn_started = Some(Instant::now());
        self.turns += 1;
        let params = json!({
            "sessionId": session,
            "prompt": [{ "type": "text", "text": text }],
        });
        self.request(Request::Prompt, "session/prompt", params)
    }

    /// Sends the waiting message once the session is open and settled.
    fn send_waiting(&mut self) -> Option<String> {
        if self.session_id().is_none() || !self.in_flight.is_empty() {
            return None;
        }
        let text = self.waiting.take()?;
        Some(self.send_prompt(&text))
    }

    fn encode_answer(&mut self, request_id: &str, decision: &ApprovalDecision) -> Exchange {
        let Some(asked) = self.asked.remove(request_id) else {
            return Exchange::default();
        };
        let chosen = match decision {
            ApprovalDecision::Allow {
                remember: Some(remember),
                ..
            } => remember
                .as_str()
                .map(str::to_owned)
                .or_else(|| asked.option(&["allow_once", "allow_always"])),
            ApprovalDecision::Allow { remember: None, .. } => {
                asked.option(&["allow_once", "allow_always"])
            }
            ApprovalDecision::Deny { .. } => asked.option(&["reject_once", "reject_always"]),
        };
        let outcome = match chosen {
            Some(option) => json!({ "outcome": "selected", "optionId": option }),
            None => json!({ "outcome": "cancelled" }),
        };
        let mut exchange = Exchange::send(vec![reply(&asked.id, json!({ "outcome": outcome }))]);
        match decision {
            ApprovalDecision::Deny {
                interrupt: true, ..
            } => {
                let cancelled = self.cancel_turn();
                exchange.lines.extend(cancelled.lines);
                exchange.events.extend(cancelled.events);
            }
            // ACP carries no reason with a refusal.
            ApprovalDecision::Deny { message, .. } if message != DECLINED_MESSAGE => {
                exchange.events.push(AgentEvent::Warning(format!(
                    "{} does not hear a reason with a refusal, so say it in your next message",
                    self.agent
                )));
            }
            _ => {}
        }
        exchange
    }

    /// Stops the running turn. The agent then answers the prompt as
    /// cancelled; every question it asked is withdrawn.
    fn cancel_turn(&mut self) -> Exchange {
        let Some(session) = self.session_id().filter(|_| self.prompting) else {
            return Exchange::default();
        };
        let mut exchange = Exchange::send(vec![line(&json!({
            "jsonrpc": "2.0",
            "method": "session/cancel",
            "params": { "sessionId": session },
        }))]);
        for (key, asked) in self.asked.drain() {
            exchange.lines.push(reply(
                &asked.id,
                json!({ "outcome": { "outcome": "cancelled" } }),
            ));
            exchange
                .events
                .push(AgentEvent::ApprovalWithdrawn { request_id: key });
        }
        exchange
    }

    fn encode_interrupt(&mut self) -> Exchange {
        if self.prompting {
            return self.cancel_turn();
        }
        if self.waiting.take().is_some() {
            // The message never reached the agent.
            return Exchange {
                events: vec![AgentEvent::TurnFinished(TurnOutcome {
                    success: true,
                    ..TurnOutcome::default()
                })],
                lines: Vec::new(),
            };
        }
        Exchange::default()
    }

    // ── Output ───────────────────────────────────────────────────────────

    fn decode_response(&mut self, id: u64, message: &Map<String, Value>) -> Exchange {
        let Some(kind) = self.pending.remove(&id) else {
            return Exchange::default();
        };
        let result = message.get("result").cloned().unwrap_or(Value::Null);
        let error = message.get("error").map(|error| {
            let text = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("The agent refused the request")
                .to_owned();
            let code = error.get("code").and_then(Value::as_i64);
            (code, text)
        });
        let mut exchange = Exchange::default();
        match (kind, error) {
            (Request::Initialize, Some((_, error))) => {
                exchange.events.push(AgentEvent::HandshakeFailed(error));
            }
            (Request::Initialize, None) => {
                let version = result
                    .get("protocolVersion")
                    .and_then(Value::as_u64)
                    .unwrap_or(PROTOCOL_VERSION);
                if version != PROTOCOL_VERSION {
                    exchange.events.push(AgentEvent::HandshakeFailed(format!(
                        "it speaks version {version} of the Agent Client Protocol, and \
                         Termeric speaks version {PROTOCOL_VERSION}"
                    )));
                    return exchange;
                }
                self.initialized = true;
                let capability = |pointer: &str| {
                    result
                        .pointer(pointer)
                        .is_some_and(|capability| !capability.is_null())
                };
                self.can_list = capability("/agentCapabilities/sessionCapabilities/list");
                self.can_resume = capability("/agentCapabilities/sessionCapabilities/resume");
                self.version = result
                    .pointer("/agentInfo/version")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                self.sign_in = result
                    .get("authMethods")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .find_map(|method| text(method, "description"))
                    .filter(|description| !description.trim().is_empty());
                // Accepted: the person may write while the session opens.
                exchange.events.push(AgentEvent::Ready(self.handshake()));
                if self.can_list {
                    let params = json!({ "cwd": self.cwd });
                    exchange
                        .lines
                        .push(self.request(Request::List, "session/list", params));
                }
                if self.resume.is_some() && !self.can_resume {
                    self.resume = None;
                    exchange.events.push(AgentEvent::Warning(format!(
                        "{} cannot continue an earlier conversation here, so this starts a \
                         new one",
                        self.agent
                    )));
                }
                // The session opens at once, so what it offers is listed
                // before the first message.
                exchange.lines.push(self.open_session());
            }
            (Request::List, error) => match error {
                Some((_, error)) => {
                    log::debug!("agent view: {} sessions unavailable: {error}", self.agent);
                }
                None => exchange
                    .events
                    .push(AgentEvent::EarlierConversations(earlier(
                        result.get("sessions"),
                    ))),
            },
            (kind @ (Request::NewSession | Request::Resume), None) => {
                let session = match kind {
                    Request::Resume => self.resume.take(),
                    _ => text(&result, "sessionId"),
                };
                let Some(session) = session else {
                    self.session = SessionState::None;
                    exchange.events.extend(
                        self.fail_waiting(&format!("{} did not open a conversation", self.agent)),
                    );
                    return exchange;
                };
                self.session = SessionState::Open(session.clone());
                self.listed_modes = result
                    .get("modes")
                    .filter(|modes| !modes.is_null())
                    .cloned();
                // The session switches to the person's choice of mode below,
                // rather than taking the one it opened in.
                if let Some(options) = result.get("configOptions").and_then(Value::as_array) {
                    self.options.clone_from(options);
                }
                exchange.events.push(AgentEvent::Ready(self.handshake()));
                exchange
                    .events
                    .push(AgentEvent::SessionStarted(SessionInfo {
                        session_id: session,
                        model: self
                            .option("model")
                            .and_then(|option| text(option, "currentValue"))
                            .unwrap_or_default(),
                        cwd: self.cwd.clone(),
                        permission_mode: None,
                        version: self.version.clone(),
                    }));
                let configured = self.configure();
                exchange.events.extend(configured.events);
                exchange.lines.extend(configured.lines);
                exchange.lines.extend(self.send_waiting());
            }
            (Request::NewSession, Some((code, error))) => {
                self.session = SessionState::None;
                let error = format!(
                    "{} could not start the conversation: {}",
                    self.agent,
                    self.explain(code, &error)
                );
                // Refused ahead of any message, it is said at once; the
                // first message tries again.
                let failed = self
                    .fail_waiting(&error)
                    .unwrap_or(AgentEvent::Warning(error));
                exchange.events.push(failed);
            }
            (Request::Resume, Some((_, error))) => {
                self.resume = None;
                self.session = SessionState::None;
                exchange.events.push(AgentEvent::Warning(format!(
                    "{} could not continue the earlier conversation ({error}), so this \
                     starts a new one",
                    self.agent
                )));
                exchange.lines.push(self.open_session());
            }
            (Request::Configure, error) => {
                self.in_flight.retain(|(request, ..)| *request != id);
                match error {
                    Some((_, error)) => {
                        exchange.events.push(AgentEvent::Warning(format!(
                            "{} did not change the setting: {error}",
                            self.agent
                        )));
                        // Show the mode the agent still runs in.
                        if let Some(current) = self
                            .option("mode")
                            .and_then(|option| text(option, "currentValue"))
                            .filter(|current| *current != self.mode)
                        {
                            self.mode.clone_from(&current);
                            exchange.events.push(AgentEvent::ModeChanged(current));
                        }
                    }
                    None => exchange
                        .events
                        .extend(self.take_options(result.get("configOptions"))),
                }
                if self.in_flight.is_empty() && std::mem::take(&mut self.effort_after_model) {
                    let configured = self.configure();
                    exchange.events.extend(configured.events);
                    exchange.lines.extend(configured.lines);
                }
                exchange.lines.extend(self.send_waiting());
            }
            (Request::Prompt, error) => {
                self.prompting = false;
                exchange.events.extend(self.settle());
                // The agent no longer waits for anything it asked.
                for key in self.asked.drain().map(|(key, _)| key) {
                    exchange
                        .events
                        .push(AgentEvent::ApprovalWithdrawn { request_id: key });
                }
                let outcome = match error {
                    Some((code, error)) => TurnOutcome {
                        success: false,
                        error: Some(self.explain(code, &error)),
                        ..self.outcome(None)
                    },
                    None => self.outcome(text(&result, "stopReason").as_deref()),
                };
                exchange.events.push(AgentEvent::TurnFinished(outcome));
            }
        }
        exchange
    }

    /// An error in words, with how to sign in when that is what it needs.
    fn explain(&self, code: Option<i64>, error: &str) -> String {
        match (&self.sign_in, code) {
            (Some(sign_in), Some(AUTH_REQUIRED)) => {
                format!("{} {}", sentence(error), sentence(sign_in))
            }
            _ => error.to_owned(),
        }
    }

    /// Ends a turn whose message could not reach a session.
    fn fail_waiting(&mut self, error: &str) -> Option<AgentEvent> {
        self.waiting.take()?;
        Some(failed_turn(error))
    }

    fn outcome(&self, stop: Option<&str>) -> TurnOutcome {
        let (success, interrupted, error) = match stop {
            Some("cancelled") => (true, true, None),
            Some("max_tokens") => (
                false,
                false,
                Some("The model reached the most it can write in one reply".to_owned()),
            ),
            Some("max_turn_requests") => (
                false,
                false,
                Some(format!(
                    "{} reached its limit of requests in one turn",
                    self.agent
                )),
            ),
            Some("refusal") => (
                false,
                true,
                Some(
                    "The model refused, so this message was left out of the conversation"
                        .to_owned(),
                ),
            ),
            _ => (true, false, None),
        };
        TurnOutcome {
            success,
            error,
            duration: self
                .turn_started
                .map_or(Duration::ZERO, |started| started.elapsed()),
            cost_usd: self.cost,
            session_id: self.session_id().map(str::to_owned),
            denials: 0,
            interrupted,
        }
    }

    fn decode_request(&mut self, id: Value, method: &str, params: &Value) -> Exchange {
        if method != "session/request_permission" {
            // Files and commands stay the agent's own; it falls back to
            // doing them itself.
            return Exchange::send(vec![line(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("Termeric does not handle {method}") },
            }))]);
        }
        let key = format!("acp-{}", request_key(&id));
        let options: Vec<(String, String)> = params
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|option| Some((text(option, "optionId")?, text(option, "kind")?)))
            .collect();
        let remember = params
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|option| text(option, "kind").as_deref() == Some("allow_always"))
            .and_then(|option| {
                Some(RememberOption {
                    label: text(option, "name").unwrap_or_else(|| "Always allow".to_owned()),
                    updates: Value::String(text(option, "optionId")?),
                })
            });
        let approved = self
            .auto_approve
            .then(|| {
                let asked = Asked {
                    id: id.clone(),
                    options: options.clone(),
                };
                asked.option(&["allow_once", "allow_always"])
            })
            .flatten();
        let reported = params.get("toolCall").cloned().unwrap_or(Value::Null);
        let tool_id = text(&reported, "toolCallId");
        let mut state = tool_id
            .as_ref()
            .and_then(|id| self.tools.get(id))
            .cloned()
            .unwrap_or_default();
        state.fill(&reported);
        let (name, input) = present_tool(&state);
        let mut events = Vec::new();
        if let Some(tool_id) = &tool_id {
            events.extend(self.settle());
            // The call reads as it will be decided.
            events.push(tool_use(tool_id, &name, input.clone()));
            self.tools.insert(tool_id.clone(), state.clone());
        }
        // Approving everything, as the agent's own interface can: the call
        // runs without a card.
        if let Some(option) = approved {
            return Exchange {
                events,
                lines: vec![reply(
                    &id,
                    json!({ "outcome": { "outcome": "selected", "optionId": option } }),
                )],
            };
        }
        events.push(AgentEvent::ApprovalRequested(ApprovalRequest {
            request_id: key.clone(),
            tool_use_id: tool_id,
            tool_name: name,
            input,
            title: Some(state.title).filter(|title| !title.trim().is_empty()),
            description: None,
            reason: None,
            remember,
            from_sub_agent: false,
        }));
        self.asked.insert(key, Asked { id, options });
        Exchange {
            events,
            lines: Vec::new(),
        }
    }

    fn decode_notification(&mut self, method: &str, params: &Value) -> Exchange {
        match method {
            "session/update" => {
                let own = match (text(params, "sessionId"), self.session_id()) {
                    (Some(named), Some(own)) => named == own,
                    _ => true,
                };
                Exchange {
                    events: match params.get("update") {
                        Some(update) if own => self.update(update),
                        _ => Vec::new(),
                    },
                    lines: Vec::new(),
                }
            }
            // The agent no longer waits for an answer it asked for; the
            // request ends as cancelled.
            "$/cancel_request" => {
                let Some(key) = params
                    .get("requestId")
                    .map(|id| format!("acp-{}", request_key(id)))
                else {
                    return Exchange::default();
                };
                let Some(asked) = self.asked.remove(&key) else {
                    return Exchange::default();
                };
                Exchange {
                    events: vec![AgentEvent::ApprovalWithdrawn { request_id: key }],
                    lines: vec![line(&json!({
                        "jsonrpc": "2.0",
                        "id": asked.id,
                        "error": { "code": -32800, "message": "Request cancelled" },
                    }))],
                }
            }
            _ => Exchange::default(),
        }
    }

    fn update(&mut self, update: &Value) -> Vec<AgentEvent> {
        match text(update, "sessionUpdate").as_deref() {
            Some("agent_message_chunk") => self.chunk(update, false),
            Some("agent_thought_chunk") => self.chunk(update, true),
            Some(kind @ ("tool_call" | "tool_call_update")) => {
                self.tool_update(update, kind == "tool_call")
            }
            Some("plan") => {
                let id = format!("plan-{}", self.turns);
                let todos: Vec<Value> = update
                    .get("entries")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|entry| todo(&text(entry, "content")?, text(entry, "status")))
                    .collect();
                vec![
                    tool_use(&id, "TodoWrite", json!({ "todos": todos })),
                    tool_result(&id, String::new(), false),
                ]
            }
            Some("available_commands_update") => {
                self.commands = update
                    .get("availableCommands")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|command| {
                        Some(SlashCommand {
                            name: text(command, "name")?,
                            description: text(command, "description").unwrap_or_default(),
                            argument_hint: command
                                .pointer("/input/hint")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                        })
                    })
                    .collect();
                vec![AgentEvent::Ready(self.handshake())]
            }
            Some("current_mode_update") => {
                let Some(mode) = text(update, "currentModeId") else {
                    return Vec::new();
                };
                if let Some(modes) = &mut self.listed_modes {
                    modes["currentModeId"] = Value::String(mode.clone());
                }
                self.mode.clone_from(&mode);
                vec![AgentEvent::ModeChanged(mode)]
            }
            Some("config_option_update") => self.take_options(update.get("configOptions")),
            Some("usage_update") => {
                if let Some(cost) = update.pointer("/cost/amount").and_then(Value::as_f64) {
                    self.cost = Some(cost);
                }
                match (
                    update.get("used").and_then(Value::as_u64),
                    update.get("size").and_then(Value::as_u64),
                ) {
                    (Some(used_tokens), Some(max_tokens)) => {
                        vec![AgentEvent::ContextUsage(ContextUsage {
                            used_tokens,
                            max_tokens,
                        })]
                    }
                    _ => Vec::new(),
                }
            }
            // The person's own words come back only when a session is
            // replayed, and titles and the rest show nothing new.
            _ => Vec::new(),
        }
    }

    /// Streams a piece of a message, starting it on its first piece.
    fn chunk(&mut self, update: &Value, thinking: bool) -> Vec<AgentEvent> {
        if update.pointer("/content/type").and_then(Value::as_str) != Some("text") {
            return Vec::new();
        }
        let Some(piece) = update.pointer("/content/text").and_then(Value::as_str) else {
            return Vec::new();
        };
        let named = text(update, "messageId");
        let continues = self.live.as_ref().is_some_and(|live| {
            live.thinking == thinking && named.as_ref().is_none_or(|id| *id == live.id)
        });
        let mut events = Vec::new();
        if !continues {
            events.extend(self.settle());
            let id = named.unwrap_or_else(|| {
                self.unnamed += 1;
                format!("acp-message-{}", self.unnamed)
            });
            events.push(AgentEvent::Stream {
                thread: Thread::Main,
                delta: StreamDelta::MessageStarted {
                    message_id: id.clone(),
                },
            });
            events.push(AgentEvent::Stream {
                thread: Thread::Main,
                delta: StreamDelta::BlockStarted {
                    index: 0,
                    block: if thinking {
                        BlockStart::Thinking
                    } else {
                        BlockStart::Text
                    },
                },
            });
            self.live = Some(Live {
                id,
                thinking,
                text: String::new(),
            });
        }
        if let Some(live) = &mut self.live {
            live.text.push_str(piece);
        }
        let text = piece.to_owned();
        events.push(AgentEvent::Stream {
            thread: Thread::Main,
            delta: if thinking {
                StreamDelta::Thinking { index: 0, text }
            } else {
                StreamDelta::Text { index: 0, text }
            },
        });
        events
    }

    /// Completes the message that was streaming, if any.
    fn settle(&mut self) -> Vec<AgentEvent> {
        let Some(live) = self.live.take() else {
            return Vec::new();
        };
        vec![
            AgentEvent::Message {
                thread: Thread::Main,
                message: AssistantMessage {
                    id: live.id,
                    blocks: vec![if live.thinking {
                        ContentBlock::Thinking(live.text)
                    } else {
                        ContentBlock::Text(live.text)
                    }],
                    error: None,
                },
            },
            AgentEvent::Stream {
                thread: Thread::Main,
                delta: StreamDelta::BlockStopped { index: 0 },
            },
        ]
    }

    fn tool_update(&mut self, update: &Value, created: bool) -> Vec<AgentEvent> {
        let Some(id) = text(update, "toolCallId") else {
            return Vec::new();
        };
        let mut events = Vec::new();
        // A new call ends the text before it. An update for a call never
        // announced counts as its announcement.
        if created || !self.tools.contains_key(&id) {
            events.extend(self.settle());
        }
        let state = self.tools.entry(id.clone()).or_default();
        state.update(update);
        let (name, input) = present_tool(state);
        let finished = match state.status.as_str() {
            "completed" => Some(false),
            "failed" => Some(true),
            _ => None,
        };
        let output = state.output();
        events.push(tool_use(&id, &name, input));
        if let Some(failed) = finished {
            events.push(tool_result(&id, output, failed));
        }
        events
    }
}

impl Protocol for AcpCodec {
    fn start(&mut self) -> Vec<String> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            // The agent keeps its own files and terminal.
            "clientCapabilities": {
                "fs": { "readTextFile": false, "writeTextFile": false },
                "terminal": false,
            },
            "clientInfo": {
                "name": "termeric",
                "title": "Termeric",
                "version": env!("CARGO_PKG_VERSION"),
            },
        });
        vec![self.request(Request::Initialize, "initialize", params)]
    }

    fn encode(&mut self, command: &AgentCommand) -> Exchange {
        match command {
            AgentCommand::Prompt(text) => self.encode_prompt(text),
            AgentCommand::Answer {
                request_id,
                decision,
                ..
            } => self.encode_answer(request_id, decision),
            AgentCommand::Interrupt => self.encode_interrupt(),
            AgentCommand::SetMode(mode) => {
                self.mode.clone_from(mode);
                self.configure()
            }
            AgentCommand::SetModel(model) => {
                self.model = (model != "default").then(|| model.clone());
                self.configure()
            }
            AgentCommand::SetEffort(effort) => {
                self.effort = Some(effort.clone());
                self.configure()
            }
            // Questions already asked still wait for the person.
            AgentCommand::SetAutoApprove(approve) => {
                self.auto_approve = *approve;
                Exchange::default()
            }
            // The agent reports usage on its own.
            AgentCommand::QueryContextUsage => Exchange::default(),
        }
    }

    fn decode(&mut self, line: &str) -> Exchange {
        match incoming(line) {
            Incoming::Request { id, method, params } => self.decode_request(id, &method, &params),
            Incoming::Notification { method, params } => self.decode_notification(&method, &params),
            Incoming::Response { id, message } => self.decode_response(id, &message),
            Incoming::Stray => Exchange::default(),
            Incoming::Rejected(error) => {
                log::warn!("agent view: agent rejected a request it could not parse: {error}");
                Exchange::default()
            }
            Incoming::Unreadable => Exchange {
                events: vec![AgentEvent::Unrecognized(preview(line))],
                lines: Vec::new(),
            },
        }
    }
}

/// The first of `keys` that holds text.
fn first_text<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
}

fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Object(object) => object.is_empty(),
        _ => false,
    }
}

/// A request id as text, for keying the question it asks.
fn request_key(id: &Value) -> String {
    match id {
        Value::String(id) => id.clone(),
        other => other.to_string(),
    }
}

fn reply(id: &Value, result: Value) -> String {
    line(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// `text` ending in a full stop.
fn sentence(text: &str) -> String {
    let text = text.trim();
    if text.ends_with(['.', '!', '?']) {
        text.to_owned()
    } else {
        format!("{text}.")
    }
}

/// A select setting's choices as (value, name, description), whether listed
/// flat or in groups.
fn choices(option: &Value) -> Vec<(String, String, String)> {
    let entry = |choice: &Value| {
        let value = text(choice, "value")?;
        Some((
            value.clone(),
            text(choice, "name").unwrap_or(value),
            text(choice, "description").unwrap_or_default(),
        ))
    };
    option
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(
            |choice| match choice.get("options").and_then(Value::as_array) {
                Some(group) => group.iter().filter_map(entry).collect::<Vec<_>>(),
                None => entry(choice).into_iter().collect(),
            },
        )
        .collect()
}

/// A model setting's choices with who provides each: the group an agent
/// lists them in, else the provider its name leads with, as OpenCode names
/// its models `Provider/Model`.
fn model_choices(option: &Value) -> Vec<ModelChoice> {
    let entry = |choice: &Value, group: Option<String>| {
        let value = text(choice, "value")?;
        let name = text(choice, "name").unwrap_or_else(|| value.clone());
        let (group, display_name) = match group {
            Some(group) => (Some(group), name),
            None => match name.split_once('/') {
                Some((provider, model))
                    if !provider.trim().is_empty() && !model.trim().is_empty() =>
                {
                    (Some(provider.trim().to_owned()), model.trim().to_owned())
                }
                _ => (None, name),
            },
        };
        Some(ModelChoice {
            value,
            display_name,
            description: text(choice, "description").unwrap_or_default(),
            group,
        })
    };
    option
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(
            |choice| match choice.get("options").and_then(Value::as_array) {
                Some(listed) => {
                    let group = text(choice, "name").or_else(|| text(choice, "group"));
                    listed
                        .iter()
                        .filter_map(|model| entry(model, group.clone()))
                        .collect::<Vec<_>>()
                }
                None => entry(choice, None).into_iter().collect(),
            },
        )
        .collect()
}

/// A mode as the panel offers it. Plan modes plan; a mode that runs every
/// action without asking needs the person's opt-in.
fn mode_choice(id: &str, name: &str, description: &str) -> ModeChoice {
    let mode = ModeChoice::new(id, &capitalized(name), description);
    match id {
        "plan" => mode.plans(),
        "yolo" => mode.unrestricted(),
        _ => mode,
    }
}

/// One to-do as the panel reads it; a cancelled one counts as done.
fn todo(content: &str, status: Option<String>) -> Option<Value> {
    let status = match status.as_deref() {
        Some("completed" | "cancelled") => "completed",
        Some("in_progress") => "in_progress",
        _ => "pending",
    };
    Some(json!({ "content": content, "status": status, "activeForm": content }))
}

/// A tool call in the panel's vocabulary: its name and input, from the
/// kind, title, input, and content the agent reported. Titles name the tool
/// before its input arrives, as with `write` or `todowrite`.
fn present_tool(state: &ToolState) -> (String, Value) {
    let input = &state.raw_input;
    let title = state.title.trim();
    let named = title.to_ascii_lowercase();
    let diffs = state.diffs();
    let path = first_text(input, &["filePath", "filepath", "file_path", "path"])
        .or_else(|| diffs.first().map(|(path, ..)| *path))
        .or_else(|| state.locations.first().map(String::as_str))
        .unwrap_or_default()
        .to_owned();
    let command = || {
        first_text(input, &["command", "cmd"])
            .unwrap_or(title)
            .to_owned()
    };
    let named_tool = |name: &str, input: Value| (name.to_owned(), input);
    match state.kind.as_str() {
        "read" => {
            let mut read = json!({ "file_path": path });
            for key in ["offset", "limit"] {
                if let Some(value) = input.get(key).and_then(Value::as_u64) {
                    read[key] = json!(value);
                }
            }
            named_tool("Read", read)
        }
        "edit" => present_edit(state, &named, &path, &diffs),
        "delete" => {
            let paths: Vec<&str> = if state.locations.is_empty() {
                vec![path.as_str()]
            } else {
                state.locations.iter().map(String::as_str).collect()
            };
            let changes: Vec<Value> = paths
                .into_iter()
                .filter(|path| !path.is_empty())
                .map(|path| json!({ "path": path, "kind": { "type": "delete" }, "diff": "" }))
                .collect();
            named_tool(PATCH_TOOL, json!({ "changes": changes }))
        }
        "search" => {
            let pattern = first_text(input, &["pattern", "query"]).unwrap_or(title);
            if named == "glob" {
                named_tool("Glob", json!({ "pattern": pattern }))
            } else {
                let mut grep = json!({ "pattern": pattern });
                if let Some(path) = first_text(input, &["path"]) {
                    grep["path"] = json!(path);
                }
                named_tool("Grep", grep)
            }
        }
        "execute" => {
            let mut bash = json!({ "command": command() });
            if let Some(description) = first_text(input, &["description"]) {
                bash["description"] = json!(description);
            }
            named_tool("Bash", bash)
        }
        "fetch" => match first_text(input, &["query"]) {
            Some(query) => named_tool("WebSearch", json!({ "query": query })),
            None => named_tool(
                "WebFetch",
                json!({ "url": first_text(input, &["url"]).unwrap_or(title) }),
            ),
        },
        "think" if input.get("subagent_type").is_some() || input.get("prompt").is_some() => {
            named_tool(
                "Agent",
                json!({
                    "description": first_text(input, &["description"]).unwrap_or(title),
                    "subagent_type": first_text(input, &["subagent_type"]),
                }),
            )
        }
        _ if named == "todowrite" || input.get("todos").is_some_and(Value::is_array) => {
            let todos: Vec<Value> = input
                .get("todos")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|entry| todo(&text(entry, "content")?, text(entry, "status")))
                .collect();
            named_tool("TodoWrite", json!({ "todos": todos }))
        }
        // Anything else reads as its own title, e.g. an MCP tool's name.
        _ => {
            let name = if title.is_empty() { "Tool" } else { title };
            named_tool(name, input.clone())
        }
    }
}

/// An edit as `Edit` (a replacement), `Write` (a whole file), or `Patch`
/// (a diff, or several files).
fn present_edit(
    state: &ToolState,
    named: &str,
    path: &str,
    diffs: &[(&str, Option<&str>, &str)],
) -> (String, Value) {
    let input = &state.raw_input;
    let edit = |path: &str, old: &str, new: &str| {
        (
            "Edit".to_owned(),
            json!({ "file_path": path, "old_string": old, "new_string": new }),
        )
    };
    let write = |path: &str, content: &str| {
        (
            "Write".to_owned(),
            json!({ "file_path": path, "content": content }),
        )
    };
    let patch = |changes: Vec<Value>| (PATCH_TOOL.to_owned(), json!({ "changes": changes }));
    if let (Some(old), Some(new)) = (
        first_text(input, &["oldString", "old_string"]),
        first_text(input, &["newString", "new_string"]),
    ) {
        return edit(path, old, new);
    }
    if let Some(content) = first_text(input, &["content"]) {
        return write(path, content);
    }
    if let Some(diff) = first_text(input, &["diff"]).filter(|_| !path.is_empty()) {
        return patch(vec![
            json!({ "path": path, "kind": { "type": "update" }, "diff": diff }),
        ]);
    }
    match diffs {
        [(path, None, new)] => write(path, new),
        [(path, Some(old), new)] if !matches!(named, "apply_patch" | "patch") => {
            edit(path, old, new)
        }
        [] if named == "write" => write(path, ""),
        [] if !matches!(named, "apply_patch" | "patch") => {
            ("Edit".to_owned(), json!({ "file_path": path }))
        }
        diffs => patch(
            diffs
                .iter()
                .map(|(path, old, new)| {
                    json!({
                        "path": path,
                        "kind": { "type": if old.is_some() { "update" } else { "add" } },
                        "old_text": old.unwrap_or_default(),
                        "new_text": new,
                    })
                })
                .collect(),
        ),
    }
}

/// Earlier sessions, newest first, titled as the agent titled them.
fn earlier(listed: Option<&Value>) -> Vec<EarlierConversation> {
    let mut sessions: Vec<EarlierConversation> = listed
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|session| {
            let id = text(session, "sessionId")?;
            let title = text(session, "title")
                .map(|title| earlier_title(&title))
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| "Untitled conversation".to_owned());
            let seconds = text(session, "updatedAt")
                .and_then(|updated| crate::session::parse_utc_timestamp(&updated))
                .unwrap_or_default();
            Some(EarlierConversation {
                id,
                title,
                modified: SystemTime::UNIX_EPOCH + Duration::from_secs(seconds),
            })
        })
        .collect();
    sessions.sort_by_key(|session| std::cmp::Reverse(session.modified));
    sessions.truncate(EARLIER_LIMIT);
    sessions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_view::transcript::{Item, Transcript};

    fn codec() -> AcpCodec {
        AcpCodec::new(codec_setup())
    }

    fn parse(line: &str) -> Value {
        assert!(line.ends_with('\n'));
        serde_json::from_str(line).unwrap()
    }

    fn decode(codec: &mut AcpCodec, value: Value) -> Exchange {
        codec.decode(&value.to_string())
    }

    fn methods(exchange: &Exchange) -> Vec<String> {
        exchange
            .lines
            .iter()
            .map(|line| parse(line)["method"].as_str().unwrap_or("").to_owned())
            .collect()
    }

    fn initialized() -> Value {
        json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": true,
                "sessionCapabilities": { "close": {}, "list": {}, "resume": {} },
            },
            "authMethods": [{
                "id": "opencode-login",
                "name": "Login with opencode",
                "description": "Run `opencode auth login` in the terminal",
            }],
            "agentInfo": { "name": "OpenCode", "version": "1.18.32" },
        })
    }

    fn options(mode: &str, model: &str) -> Value {
        let mut options = json!([
            {
                "id": "model", "name": "Model", "category": "model", "type": "select",
                "currentValue": model,
                "options": [
                    { "value": "opencode/big-pickle", "name": "OpenCode Zen/Big Pickle" },
                    { "value": "zai/glm-5.2", "name": "Z.AI/GLM-5.2" },
                ],
            },
            {
                "id": "mode", "name": "Session Mode", "category": "mode", "type": "select",
                "currentValue": mode,
                "options": [
                    { "value": "build", "name": "build", "description": "The default agent." },
                    { "value": "plan", "name": "plan", "description": "Disallows all edit tools." },
                ],
            },
        ]);
        // Models with variants offer levels of effort.
        if model == "zai/glm-5.2" {
            options.as_array_mut().unwrap().push(json!({
                "id": "effort", "name": "Effort", "category": "thought_level", "type": "select",
                "currentValue": "high",
                "options": [
                    { "value": "high", "name": "high" },
                    { "value": "max", "name": "max" },
                    { "value": "default", "name": "default" },
                ],
            }));
        }
        options
    }

    /// Connects, sends "hello", and opens a session running a turn.
    /// Connects, which opens a session at once, and sends "hello".
    fn with_turn(codec: &mut AcpCodec) -> u64 {
        open(codec, options("build", "opencode/big-pickle"));
        let prompt = parse(&codec.encode(&AgentCommand::Prompt("hello".into())).lines[0]);
        assert_eq!(prompt["method"], "session/prompt");
        prompt["id"].as_u64().unwrap()
    }

    /// Connects and answers the session it opens with `options`.
    fn open(codec: &mut AcpCodec, options: Value) -> Exchange {
        codec.start();
        let connected = decode(
            codec,
            json!({ "jsonrpc": "2.0", "id": 1, "result": initialized() }),
        );
        let open = parse(connected.lines.last().unwrap());
        assert_eq!(open["method"], "session/new");
        decode(
            codec,
            json!({ "jsonrpc": "2.0", "id": open["id"], "result": {
                "sessionId": "ses_1", "configOptions": options,
            }}),
        )
    }

    fn update(codec: &mut AcpCodec, update: Value) -> Vec<AgentEvent> {
        decode(
            codec,
            json!({ "jsonrpc": "2.0", "method": "session/update",
                    "params": { "sessionId": "ses_1", "update": update } }),
        )
        .events
    }

    fn tool_uses(events: &[AgentEvent]) -> Vec<(String, Value)> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Message { message, .. } => match &message.blocks[..] {
                    [ContentBlock::ToolUse { name, input, .. }] => {
                        Some((name.clone(), input.clone()))
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_handshake_asks_for_no_files_or_terminal_and_lists_the_folders_sessions() {
        let mut codec = codec();
        let start = codec.start();
        let initialize = parse(&start[0]);
        assert_eq!(initialize["jsonrpc"], "2.0");
        assert_eq!(initialize["method"], "initialize");
        assert_eq!(initialize["params"]["protocolVersion"], 1);
        assert_eq!(
            initialize["params"]["clientCapabilities"],
            json!({ "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false })
        );
        let exchange = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "id": 1, "result": initialized() }),
        );
        assert!(matches!(exchange.events[0], AgentEvent::Ready(_)));
        // The session opens at once, so what it offers is listed before the
        // first message.
        assert_eq!(methods(&exchange), ["session/list", "session/new"]);
        let list = parse(&exchange.lines[0]);
        assert_eq!(list["params"]["cwd"], "/w");
        let exchange = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "id": list["id"], "result": { "sessions": [
                { "sessionId": "ses_old", "cwd": "/w", "title": "Fix the parser",
                  "updatedAt": "2026-09-20T10:00:00.000Z" },
                { "sessionId": "ses_new", "cwd": "/w", "title": "Add a tab picture",
                  "updatedAt": "2026-09-26T10:00:00Z" },
                { "sessionId": "ses_bare", "cwd": "/w" },
            ]}}),
        );
        let [AgentEvent::EarlierConversations(earlier)] = &exchange.events[..] else {
            panic!("{:?}", exchange.events)
        };
        let listed: Vec<(&str, &str)> = earlier
            .iter()
            .map(|session| (session.id.as_str(), session.title.as_str()))
            .collect();
        assert_eq!(
            listed,
            [
                ("ses_new", "Add a tab picture"),
                ("ses_old", "Fix the parser"),
                ("ses_bare", "Untitled conversation"),
            ]
        );
    }

    #[test]
    fn another_protocol_version_is_refused() {
        let mut codec = codec();
        codec.start();
        let exchange = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "id": 1, "result": { "protocolVersion": 2 } }),
        );
        assert!(
            matches!(&exchange.events[..], [AgentEvent::HandshakeFailed(error)] if error.contains("version 2"))
        );
    }

    #[test]
    fn the_session_opens_at_once_in_the_chosen_mode_model_and_effort() {
        let mut codec = AcpCodec::new(Setup {
            model: Some("zai/glm-5.2".into()),
            mode: "plan".into(),
            effort: Some("max".into()),
            ..codec_setup()
        });
        codec.start();
        let exchange = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "id": 1, "result": initialized() }),
        );
        let open = parse(&exchange.lines[1]);
        assert_eq!(open["params"], json!({ "cwd": "/w", "mcpServers": [] }));
        // Written before the session opened: it waits.
        assert!(
            codec
                .encode(&AgentCommand::Prompt("hi".into()))
                .lines
                .is_empty()
        );
        let exchange = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "id": open["id"], "result": {
                "sessionId": "ses_1", "configOptions": options("build", "opencode/big-pickle"),
            }}),
        );
        let started = exchange.events.iter().find_map(|event| match event {
            AgentEvent::SessionStarted(info) => Some(info.clone()),
            _ => None,
        });
        let started = started.unwrap();
        assert_eq!(started.session_id, "ses_1");
        assert_eq!(started.version, "1.18.32");
        let handshake = exchange.events.iter().rev().find_map(|event| match event {
            AgentEvent::Ready(handshake) => Some(handshake.clone()),
            _ => None,
        });
        let handshake = handshake.unwrap();
        // The agent's own models, and the one it runs.
        let models: Vec<&str> = handshake
            .models
            .iter()
            .map(|model| model.value.as_str())
            .collect();
        assert_eq!(models, ["opencode/big-pickle", "zai/glm-5.2"]);
        assert_eq!(
            handshake.current_model.as_deref(),
            Some("opencode/big-pickle")
        );
        assert!(handshake.efforts.is_empty());
        let modes: Vec<(&str, &str, bool)> = handshake
            .modes
            .iter()
            .map(|mode| (mode.id.as_str(), mode.label.as_str(), mode.plans))
            .collect();
        assert_eq!(modes, [("build", "Build", false), ("plan", "Plan", true)]);
        // The mode and model change first; the effort waits for the model,
        // whose levels it is, and the message for all three.
        assert_eq!(
            methods(&exchange),
            ["session/set_config_option", "session/set_config_option"]
        );
        let (mode, model) = (parse(&exchange.lines[0]), parse(&exchange.lines[1]));
        assert_eq!(
            mode["params"],
            json!({ "sessionId": "ses_1", "configId": "mode", "value": "plan" })
        );
        assert_eq!(model["params"]["value"], "zai/glm-5.2");
        let exchange = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "id": mode["id"], "result": {
                "configOptions": options("plan", "opencode/big-pickle"),
            }}),
        );
        assert!(exchange.lines.is_empty());
        let exchange = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "id": model["id"], "result": {
                "configOptions": options("plan", "zai/glm-5.2"),
            }}),
        );
        let handshake = exchange.events.iter().find_map(|event| match event {
            AgentEvent::Ready(handshake) => Some(handshake.clone()),
            _ => None,
        });
        let handshake = handshake.unwrap();
        let efforts: Vec<(&str, &str)> = handshake
            .efforts
            .iter()
            .map(|effort| (effort.value.as_str(), effort.label.as_str()))
            .collect();
        assert_eq!(
            efforts,
            [("high", "High"), ("max", "Max"), ("default", "Default")]
        );
        assert_eq!(handshake.current_effort.as_deref(), Some("high"));
        assert_eq!(methods(&exchange), ["session/set_config_option"]);
        let effort = parse(&exchange.lines[0]);
        assert_eq!(
            effort["params"],
            json!({ "sessionId": "ses_1", "configId": "effort", "value": "max" })
        );
        let mut settled = options("plan", "zai/glm-5.2");
        settled[2]["currentValue"] = json!("max");
        let exchange = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "id": effort["id"], "result": { "configOptions": settled } }),
        );
        let prompt = parse(&exchange.lines[0]);
        assert_eq!(prompt["method"], "session/prompt");
        assert_eq!(
            prompt["params"],
            json!({ "sessionId": "ses_1", "prompt": [{ "type": "text", "text": "hi" }] })
        );
        // Settled, a change goes at once, and one already in effect sends
        // nothing.
        let exchange = codec.encode(&AgentCommand::SetEffort("high".into()));
        assert_eq!(parse(&exchange.lines[0])["params"]["value"], "high");
        assert!(
            codec
                .encode(&AgentCommand::SetModel("zai/glm-5.2".into()))
                .lines
                .is_empty()
        );
    }

    fn codec_setup() -> Setup {
        Setup {
            agent: "OpenCode",
            cwd: PathBuf::from("/w"),
            mode: "build".to_owned(),
            model: None,
            effort: None,
            auto_approve: false,
            resume: None,
        }
    }

    #[test]
    fn an_effort_chosen_while_a_model_change_is_on_its_way_waits_for_it() {
        let mut codec = codec();
        open(&mut codec, options("build", "opencode/big-pickle"));
        let model = codec.encode(&AgentCommand::SetModel("zai/glm-5.2".into()));
        let model_request = parse(&model.lines[0]);
        // The old model offers no effort, so the choice waits for the new one.
        assert!(
            codec
                .encode(&AgentCommand::SetEffort("max".into()))
                .lines
                .is_empty()
        );
        let exchange = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "id": model_request["id"],
                    "result": { "configOptions": options("build", "zai/glm-5.2") } }),
        );
        let effort = parse(&exchange.lines[0]);
        assert_eq!(effort["params"]["configId"], "effort");
        assert_eq!(effort["params"]["value"], "max");
    }

    #[test]
    fn a_mode_on_its_way_survives_other_setting_changes_before_the_answer() {
        let mut codec = codec();
        open(&mut codec, options("build", "opencode/big-pickle"));
        let mode = codec.encode(&AgentCommand::SetMode("plan".into()));
        assert_eq!(parse(&mode.lines[0])["params"]["value"], "plan");
        // The agent hasn't answered yet, so it still reports "build" as
        // current; picking a model and an effort must not take that as the
        // session's mode, nor send "build" back.
        for command in [
            AgentCommand::SetModel("zai/glm-5.2".into()),
            AgentCommand::SetEffort("high".into()),
        ] {
            let exchange = codec.encode(&command);
            assert!(
                !exchange
                    .events
                    .iter()
                    .any(|event| matches!(event, AgentEvent::ModeChanged(_))),
                "{:?}",
                exchange.events
            );
            for line in &exchange.lines {
                assert_ne!(parse(line)["params"]["configId"], "mode", "{line}");
            }
        }
        assert_eq!(codec.mode, "plan");
    }

    #[test]
    fn a_mode_the_session_lacks_gives_way_to_the_one_it_runs_in() {
        let mut codec = AcpCodec::new(Setup {
            mode: "docs".into(),
            ..codec_setup()
        });
        let exchange = open(&mut codec, options("build", "opencode/big-pickle"));
        assert!(
            exchange
                .events
                .contains(&AgentEvent::ModeChanged("build".into()))
        );
        assert!(exchange.lines.is_empty());
        let prompt = codec.encode(&AgentCommand::Prompt("hi".into()));
        assert_eq!(methods(&prompt), ["session/prompt"]);
    }

    #[test]
    fn text_and_reasoning_stream_and_settle_when_something_else_starts() {
        let mut codec = codec();
        let prompt = with_turn(&mut codec);
        let mut transcript = Transcript::new("OpenCode", "build");
        let mut events = Vec::new();
        for (kind, text) in [
            ("agent_thought_chunk", "Look at "),
            ("agent_thought_chunk", "the parser."),
            ("agent_message_chunk", "It is "),
            ("agent_message_chunk", "**fixed**."),
        ] {
            events.extend(update(
                &mut codec,
                json!({ "sessionUpdate": kind, "messageId": "msg_1",
                        "content": { "type": "text", "text": text } }),
            ));
        }
        events.extend(update(
            &mut codec,
            json!({ "sessionUpdate": "usage_update", "used": 12_000, "size": 200_000,
                    "cost": { "amount": 0.25, "currency": "USD" } }),
        ));
        events.extend(
            decode(
                &mut codec,
                json!({ "jsonrpc": "2.0", "id": prompt, "result": { "stopReason": "end_turn" } }),
            )
            .events,
        );
        for event in events {
            transcript.apply(event);
        }
        let texts: Vec<(bool, &str, bool)> = transcript
            .items()
            .iter()
            .filter_map(|item| match item {
                Item::Thinking {
                    text, streaming, ..
                } => Some((true, text.as_str(), *streaming)),
                Item::Text {
                    text, streaming, ..
                } => Some((false, text.as_str(), *streaming)),
                _ => None,
            })
            .collect();
        assert_eq!(
            texts,
            [
                (true, "Look at the parser.", false),
                (false, "It is **fixed**.", false)
            ]
        );
        assert_eq!(transcript.context().unwrap().max_tokens, 200_000);
        let Some(Item::TurnEnd(end)) = transcript.items().last() else {
            panic!("{:#?}", transcript.items())
        };
        assert!(end.outcome.success);
        assert_eq!(end.outcome.cost_usd, Some(0.25));
        assert_eq!(end.outcome.session_id.as_deref(), Some("ses_1"));
    }

    #[test]
    fn tool_calls_read_in_the_panels_vocabulary() {
        let mut codec = codec();
        with_turn(&mut codec);
        let mut call = |id: &str, kind: &str, title: &str, input: Value| {
            let events = update(
                &mut codec,
                json!({ "sessionUpdate": "tool_call", "toolCallId": id, "kind": kind,
                        "title": title, "status": "pending", "rawInput": input,
                        "locations": [] }),
            );
            tool_uses(&events).remove(0)
        };
        assert_eq!(
            call(
                "t1",
                "read",
                "read",
                json!({ "filePath": "/w/src/lib.rs", "offset": 10, "limit": 20 })
            ),
            (
                "Read".to_owned(),
                json!({ "file_path": "/w/src/lib.rs", "offset": 10, "limit": 20 })
            )
        );
        assert_eq!(
            call(
                "t2",
                "execute",
                "cargo test",
                json!({ "command": "cargo test", "description": "Runs the tests" })
            ),
            (
                "Bash".to_owned(),
                json!({ "command": "cargo test", "description": "Runs the tests" })
            )
        );
        assert_eq!(
            call(
                "t3",
                "search",
                "grep",
                json!({ "pattern": "fn main", "path": "/w/src" })
            ),
            (
                "Grep".to_owned(),
                json!({ "pattern": "fn main", "path": "/w/src" })
            )
        );
        assert_eq!(
            call("t4", "search", "glob", json!({ "pattern": "**/*.rs" })),
            ("Glob".to_owned(), json!({ "pattern": "**/*.rs" }))
        );
        assert_eq!(
            call(
                "t5",
                "edit",
                "edit",
                json!({ "filePath": "/w/a.rs", "oldString": "1", "newString": "2" })
            ),
            (
                "Edit".to_owned(),
                json!({ "file_path": "/w/a.rs", "old_string": "1", "new_string": "2" })
            )
        );
        assert_eq!(
            call(
                "t6",
                "edit",
                "write",
                json!({ "filePath": "/w/b.rs", "content": "fn b() {}" })
            ),
            (
                "Write".to_owned(),
                json!({ "file_path": "/w/b.rs", "content": "fn b() {}" })
            )
        );
        assert_eq!(
            call(
                "t7",
                "fetch",
                "https://example.com",
                json!({ "url": "https://example.com" })
            ),
            (
                "WebFetch".to_owned(),
                json!({ "url": "https://example.com" })
            )
        );
        assert_eq!(
            call(
                "t8",
                "think",
                "task",
                json!({ "description": "Find uses", "prompt": "…", "subagent_type": "explore" })
            ),
            (
                "Agent".to_owned(),
                json!({ "description": "Find uses", "subagent_type": "explore" })
            )
        );
        assert_eq!(
            call(
                "t9",
                "other",
                "todowrite",
                json!({ "todos": [
                    { "content": "Parse", "status": "completed", "priority": "high", "id": "1" },
                    { "content": "Render", "status": "in_progress", "priority": "high", "id": "2" },
                    { "content": "Ship", "status": "cancelled", "priority": "low", "id": "3" },
                ]})
            ),
            (
                "TodoWrite".to_owned(),
                json!({ "todos": [
                    { "content": "Parse", "status": "completed", "activeForm": "Parse" },
                    { "content": "Render", "status": "in_progress", "activeForm": "Render" },
                    { "content": "Ship", "status": "completed", "activeForm": "Ship" },
                ]})
            )
        );
        // Anything else reads as its title, with its own input.
        assert_eq!(
            call(
                "t10",
                "other",
                "context7_resolve-library-id",
                json!({ "libraryName": "wgpu" })
            ),
            (
                "context7_resolve-library-id".to_owned(),
                json!({ "libraryName": "wgpu" })
            )
        );
    }

    #[test]
    fn a_patch_reads_one_change_per_file_and_updates_replace_what_they_name() {
        let mut codec = codec();
        with_turn(&mut codec);
        let created = update(
            &mut codec,
            json!({ "sessionUpdate": "tool_call", "toolCallId": "p1", "kind": "edit",
                    "title": "apply_patch", "status": "pending", "rawInput": {} }),
        );
        assert_eq!(
            tool_uses(&created)[0],
            (PATCH_TOOL.to_owned(), json!({ "changes": [] }))
        );
        let completed = update(
            &mut codec,
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": "p1", "status": "completed",
            "content": [
                { "type": "content", "content": { "type": "text", "text": "Applied" } },
                { "type": "diff", "path": "/w/a.rs", "oldText": "a\n", "newText": "b\n" },
                { "type": "diff", "path": "/w/new.rs", "newText": "fn new() {}\n" },
            ]}),
        );
        let (name, input) = tool_uses(&completed).remove(0);
        assert_eq!(name, PATCH_TOOL);
        assert_eq!(
            input["changes"],
            json!([
                { "path": "/w/a.rs", "kind": { "type": "update" }, "old_text": "a\n", "new_text": "b\n" },
                { "path": "/w/new.rs", "kind": { "type": "add" }, "old_text": "", "new_text": "fn new() {}\n" },
            ])
        );
        assert_eq!(input["changes"][0]["new_text"], "b\n");
        let mut transcript = Transcript::new("OpenCode", "build");
        for event in created.iter().chain(&completed) {
            transcript.apply(event.clone());
        }
        let Some(Item::Tool(tool)) = transcript.items().last() else {
            panic!("{:#?}", transcript.items())
        };
        let diffs = crate::agent_view::present::file_diffs(tool, std::path::Path::new("/w"));
        let summary: Vec<(&str, usize, usize, bool)> = diffs
            .iter()
            .map(|diff| (diff.path.as_str(), diff.added, diff.removed, diff.created))
            .collect();
        assert_eq!(summary, [("a.rs", 1, 1, false), ("new.rs", 1, 0, true)]);
        assert!(matches!(
            completed.last(),
            Some(AgentEvent::ToolResult { result, .. }) if result.content == "Applied" && !result.is_error
        ));
        let failed = update(
            &mut codec,
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t9", "kind": "execute",
                    "title": "false", "status": "failed",
                    "rawOutput": { "error": "exit status 1" } }),
        );
        assert!(matches!(
            failed.last(),
            Some(AgentEvent::ToolResult { result, .. }) if result.content == "exit status 1" && result.is_error
        ));
    }

    fn permission(codec: &mut AcpCodec, id: u64) -> Exchange {
        decode(
            codec,
            json!({ "jsonrpc": "2.0", "id": id, "method": "session/request_permission", "params": {
                "sessionId": "ses_1",
                "toolCall": {
                    "toolCallId": "e1", "title": "a.rs", "kind": "edit",
                    "rawInput": { "filepath": "/w/a.rs", "diff": "@@ -1 +1 @@\n-a\n+b\n" },
                    "content": [{ "type": "diff", "path": "/w/a.rs", "oldText": "a\nz\n", "newText": "b\nz\n" }],
                },
                "options": [
                    { "optionId": "once", "kind": "allow_once", "name": "Allow once" },
                    { "optionId": "always", "kind": "allow_always", "name": "Always allow" },
                    { "optionId": "reject", "kind": "reject_once", "name": "Reject" },
                ],
            }}),
        )
    }

    fn approval(exchange: &Exchange) -> ApprovalRequest {
        exchange
            .events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ApprovalRequested(request) => Some(request.clone()),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn permissions_read_as_the_call_they_decide_and_answer_with_the_chosen_option() {
        let mut codec = codec();
        with_turn(&mut codec);
        update(
            &mut codec,
            json!({ "sessionUpdate": "tool_call", "toolCallId": "e1", "kind": "edit", "title": "edit",
                    "status": "pending",
                    "rawInput": { "filePath": "/w/a.rs", "oldString": "a", "newString": "b" } }),
        );
        let asked = permission(&mut codec, 0);
        assert!(asked.lines.is_empty());
        let request = approval(&asked);
        // The call's own replacement, not the whole file the agent compared.
        assert_eq!(request.tool_name, "Edit");
        assert_eq!(request.input["old_string"], "a");
        assert_eq!(request.tool_use_id.as_deref(), Some("e1"));
        assert_eq!(request.title.as_deref(), Some("edit"));
        let remember = request.remember.clone().unwrap();
        assert_eq!(remember.label, "Always allow");
        let answered = codec.encode(&AgentCommand::Answer {
            request_id: request.request_id.clone(),
            tool_use_id: request.tool_use_id.clone(),
            decision: ApprovalDecision::Allow {
                input: request.input.clone(),
                remember: Some(remember.updates),
            },
        });
        assert_eq!(
            parse(&answered.lines[0]),
            json!({ "jsonrpc": "2.0", "id": 0, "result": { "outcome": { "outcome": "selected", "optionId": "always" } } })
        );

        // A permission for a call not seen yet reads from its own report.
        let asked = permission(&mut codec, 1);
        let request = approval(&asked);
        let _ = request;
        let fresh = permission(&mut AcpCodec::new(codec_setup()), 5);
        let request = approval(&fresh);
        assert_eq!(request.tool_name, PATCH_TOOL);
        assert_eq!(request.input["changes"][0]["diff"], "@@ -1 +1 @@\n-a\n+b\n");

        // Declining with a reason says the agent will not hear it.
        let request = approval(&asked);
        let declined = codec.encode(&AgentCommand::Answer {
            request_id: request.request_id.clone(),
            tool_use_id: None,
            decision: ApprovalDecision::Deny {
                message: "Use a match".into(),
                interrupt: false,
            },
        });
        assert_eq!(
            parse(&declined.lines[0])["result"]["outcome"]["optionId"],
            "reject"
        );
        assert!(
            matches!(&declined.events[..], [AgentEvent::Warning(text)] if text.contains("next message"))
        );
    }

    #[test]
    fn stopping_cancels_the_turn_and_withdraws_what_it_asked() {
        let mut codec = codec();
        let prompt = with_turn(&mut codec);
        let asked = permission(&mut codec, 7);
        let request = approval(&asked);
        let stopped = codec.encode(&AgentCommand::Interrupt);
        assert_eq!(
            parse(&stopped.lines[0]),
            json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": "ses_1" } })
        );
        assert_eq!(
            parse(&stopped.lines[1]),
            json!({ "jsonrpc": "2.0", "id": 7, "result": { "outcome": { "outcome": "cancelled" } } })
        );
        assert_eq!(
            stopped.events,
            [AgentEvent::ApprovalWithdrawn {
                request_id: request.request_id
            }]
        );
        let finished = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "id": prompt, "result": { "stopReason": "cancelled" } }),
        );
        let Some(AgentEvent::TurnFinished(outcome)) = finished.events.last() else {
            panic!()
        };
        assert!(outcome.success && outcome.interrupted);
    }

    #[test]
    fn a_denial_that_stops_also_cancels_the_turn() {
        let mut codec = codec();
        with_turn(&mut codec);
        let request = approval(&permission(&mut codec, 3));
        let denied = codec.encode(&AgentCommand::Answer {
            request_id: request.request_id,
            tool_use_id: None,
            decision: ApprovalDecision::Deny {
                message: DECLINED_MESSAGE.into(),
                interrupt: true,
            },
        });
        assert_eq!(
            parse(&denied.lines[0])["result"]["outcome"]["optionId"],
            "reject"
        );
        assert_eq!(parse(&denied.lines[1])["method"], "session/cancel");
        assert!(denied.events.is_empty());
    }

    #[test]
    fn the_agents_file_and_terminal_requests_are_refused_so_it_does_them_itself() {
        let mut codec = codec();
        with_turn(&mut codec);
        let exchange = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "id": 4, "method": "fs/write_text_file",
                    "params": { "sessionId": "ses_1", "path": "/w/a.rs", "content": "b" } }),
        );
        let reply = parse(&exchange.lines[0]);
        assert_eq!(reply["id"], 4);
        assert_eq!(reply["error"]["code"], -32601);
        assert!(exchange.events.is_empty());
    }

    #[test]
    fn the_agent_can_withdraw_a_question() {
        let mut codec = codec();
        with_turn(&mut codec);
        let request = approval(&permission(&mut codec, 9));
        let exchange = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "method": "$/cancel_request", "params": { "requestId": 9 } }),
        );
        assert_eq!(
            exchange.events,
            [AgentEvent::ApprovalWithdrawn {
                request_id: request.request_id
            }]
        );
        assert_eq!(
            parse(&exchange.lines[0]),
            json!({ "jsonrpc": "2.0", "id": 9, "error": { "code": -32800, "message": "Request cancelled" } })
        );
    }

    #[test]
    fn signing_in_is_explained_and_stop_reasons_end_turns_in_words() {
        let mut codec = codec();
        let prompt = with_turn(&mut codec);
        let exchange = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "id": prompt, "error": {
                "code": -32000, "message": "Authentication required: provider authentication required",
            }}),
        );
        let Some(AgentEvent::TurnFinished(outcome)) = exchange.events.last() else {
            panic!()
        };
        assert_eq!(
            outcome.error.as_deref(),
            Some(
                "Authentication required: provider authentication required. Run `opencode auth \
                 login` in the terminal."
            )
        );
        for (stop, success, interrupted) in [
            ("max_tokens", false, false),
            ("max_turn_requests", false, false),
            ("refusal", false, true),
            ("end_turn", true, false),
        ] {
            let outcome = codec.outcome(Some(stop));
            assert_eq!(
                (outcome.success, outcome.interrupted),
                (success, interrupted),
                "{stop}"
            );
        }
    }

    #[test]
    fn stopping_before_the_session_opens_drops_the_message() {
        let mut codec = codec();
        codec.start();
        decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "id": 1, "result": initialized() }),
        );
        codec.encode(&AgentCommand::Prompt("hi".into()));
        let stopped = codec.encode(&AgentCommand::Interrupt);
        assert!(stopped.lines.is_empty());
        assert!(
            matches!(&stopped.events[..], [AgentEvent::TurnFinished(outcome)] if outcome.success)
        );
    }

    #[test]
    fn a_session_refused_ahead_of_a_message_is_said_and_tried_again() {
        let mut codec = codec();
        codec.start();
        let exchange = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "id": 1, "result": initialized() }),
        );
        let open = parse(&exchange.lines[1]);
        let exchange = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "id": open["id"], "error": {
                "code": -32000, "message": "Authentication required",
            }}),
        );
        assert!(matches!(
            &exchange.events[..],
            [AgentEvent::Warning(text)] if text.ends_with("Run `opencode auth login` in the terminal.")
        ));
        let retried = codec.encode(&AgentCommand::Prompt("hi".into()));
        assert_eq!(methods(&retried), ["session/new"]);
    }

    #[test]
    fn auto_approve_answers_permissions_without_asking() {
        let mut codec = AcpCodec::new(Setup {
            auto_approve: true,
            ..codec_setup()
        });
        with_turn(&mut codec);
        let exchange = permission(&mut codec, 4);
        assert_eq!(
            parse(&exchange.lines[0]),
            json!({ "jsonrpc": "2.0", "id": 4, "result": { "outcome": { "outcome": "selected", "optionId": "once" } } })
        );
        // The call still shows; nothing asks.
        assert!(
            !exchange
                .events
                .iter()
                .any(|event| matches!(event, AgentEvent::ApprovalRequested(_)))
        );
        assert_eq!(tool_uses(&exchange.events)[0].0, PATCH_TOOL);
        // Turned off, the next request asks again.
        codec.encode(&AgentCommand::SetAutoApprove(false));
        let exchange = permission(&mut codec, 5);
        assert!(exchange.lines.is_empty());
        approval(&exchange);
    }

    #[test]
    fn an_earlier_session_continues_at_once_or_a_new_one_starts_and_says_so() {
        let mut codec = AcpCodec::new(Setup {
            resume: Some("ses_old".into()),
            ..codec_setup()
        });
        codec.start();
        let exchange = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "id": 1, "result": initialized() }),
        );
        assert_eq!(methods(&exchange), ["session/list", "session/resume"]);
        let resume = parse(&exchange.lines[1]);
        assert_eq!(
            resume["params"],
            json!({ "cwd": "/w", "mcpServers": [], "sessionId": "ses_old" })
        );
        let exchange = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "id": resume["id"], "result": {
                "configOptions": options("build", "opencode/big-pickle"),
            }}),
        );
        assert!(exchange.events.iter().any(|event| matches!(
            event, AgentEvent::SessionStarted(info) if info.session_id == "ses_old"
        )));

        let mut codec = AcpCodec::new(Setup {
            resume: Some("ses_gone".into()),
            ..codec_setup()
        });
        codec.start();
        let exchange = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "id": 1, "result": initialized() }),
        );
        let resume = parse(&exchange.lines[1]);
        codec.encode(&AgentCommand::Prompt("hi".into()));
        let exchange = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "id": resume["id"], "error": { "code": -32602, "message": "Session not found" } }),
        );
        assert!(
            matches!(&exchange.events[..], [AgentEvent::Warning(text)] if text.contains("Session not found"))
        );
        assert_eq!(methods(&exchange), ["session/new"]);
    }

    #[test]
    fn commands_and_modes_follow_the_agents_updates() {
        let mut codec = codec();
        with_turn(&mut codec);
        let events = update(
            &mut codec,
            json!({ "sessionUpdate": "available_commands_update", "availableCommands": [
                { "name": "init", "description": "Create AGENTS.md" },
                { "name": "review", "description": "Review changes", "input": { "hint": "[branch]" } },
            ]}),
        );
        let [AgentEvent::Ready(handshake)] = &events[..] else {
            panic!()
        };
        let commands: Vec<(&str, &str)> = handshake
            .commands
            .iter()
            .map(|command| (command.name.as_str(), command.argument_hint.as_str()))
            .collect();
        assert_eq!(commands, [("init", ""), ("review", "[branch]")]);
        let events = update(
            &mut codec,
            json!({ "sessionUpdate": "config_option_update", "configOptions": options("plan", "opencode/big-pickle") }),
        );
        assert!(events.contains(&AgentEvent::ModeChanged("plan".into())));
        // Switching sends the change at once.
        let exchange = codec.encode(&AgentCommand::SetMode("build".into()));
        assert_eq!(parse(&exchange.lines[0])["params"]["value"], "build");
    }

    #[test]
    fn updates_for_other_sessions_are_ignored() {
        let mut codec = codec();
        with_turn(&mut codec);
        let exchange = decode(
            &mut codec,
            json!({ "jsonrpc": "2.0", "method": "session/update", "params": {
                "sessionId": "ses_other",
                "update": { "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "x" } },
            }}),
        );
        assert!(exchange.events.is_empty());
    }

    #[test]
    fn models_are_grouped_by_who_provides_them() {
        let listed = model_choices(&json!({ "options": [
            { "value": "github-copilot/claude-opus-5", "name": "GitHub Copilot/Claude Opus 5" },
            { "value": "local", "name": "Local model" },
            { "group": "anthropic", "name": "Anthropic", "options": [
                { "value": "anthropic/sonnet", "name": "Sonnet 5" },
            ]},
        ]}));
        let read: Vec<(&str, &str, Option<&str>)> = listed
            .iter()
            .map(|model| {
                (
                    model.value.as_str(),
                    model.display_name.as_str(),
                    model.group.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            read,
            [
                (
                    "github-copilot/claude-opus-5",
                    "Claude Opus 5",
                    Some("GitHub Copilot")
                ),
                ("local", "Local model", None),
                ("anthropic/sonnet", "Sonnet 5", Some("Anthropic")),
            ]
        );
    }
}
