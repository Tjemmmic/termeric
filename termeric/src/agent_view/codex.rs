//! Codex's app-server protocol.
//!
//! `codex app-server` speaks JSON-RPC over its pipes, one message per line,
//! without the usual `jsonrpc` field. A conversation is a thread; each message
//! the person sends starts a turn on it, and the turn streams items (agent
//! text, reasoning, commands, file changes, tool calls) until `turn/completed`.
//! Codex asks before acting through requests the panel must answer; one left
//! unanswered stalls the turn, so every request gets a reply, even one this
//! host cannot serve.
//!
//! The thread starts with the first message rather than with the process, so
//! a session started ahead of its first message leaves nothing behind in
//! Codex's history. Codex's tool calls are translated into the vocabulary
//! the panel presents (see [`super::present`]): commands read as `Bash`,
//! patches as `Patch`, web searches as `WebSearch`, and MCP tools by their
//! `mcp__server__tool` name.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Map, Value, json};

use super::event::{
    AgentCommand, AgentEvent, ApprovalDecision, ApprovalRequest, ContentBlock, ContextUsage,
    DECLINED_MESSAGE, EarlierConversation, Handshake, ModeChoice, ModelChoice, RememberOption,
    Retry, SessionInfo, SlashCommand, TurnOutcome, earlier_title, invoked_name,
};
use super::present::PATCH_TOOL;
use super::protocol::{
    Exchange, Incoming, Protocol, client_info, content_text, failed_turn, finished_turn, incoming,
    line, method_not_found, plan, request_key, settled, stream_piece, stream_started, text, todo,
    tool_result, tool_use,
};

/// The mode Codex asks in: it reads freely and asks before changing
/// anything.
pub const ASK_MODE: &str = "read-only";

/// Codex's permission presets, as its own interface offers them.
pub fn modes() -> Vec<ModeChoice> {
    vec![
        ModeChoice::new(
            ASK_MODE,
            "Ask before changes",
            "Reads and runs read-only commands; asks before editing or anything else",
        ),
        ModeChoice::new(
            "auto",
            "Auto",
            "Edits and runs commands in the folder; asks before leaving it or using the network",
        ),
        ModeChoice::new(
            "full-access",
            "Full access",
            "Everything runs without asking or a sandbox; best where nothing can be harmed",
        )
        .unrestricted(),
    ]
}

/// The approval policy, thread sandbox, and turn sandbox policy for a mode.
fn policy(mode: &str) -> (&'static str, &'static str, Value) {
    match mode {
        "auto" => (
            "on-request",
            "workspace-write",
            json!({
                "type": "workspaceWrite",
                "writableRoots": [],
                "networkAccess": false,
                "excludeTmpdirEnvVar": false,
                "excludeSlashTmp": false,
            }),
        ),
        "full-access" => (
            "never",
            "danger-full-access",
            json!({ "type": "dangerFullAccess" }),
        ),
        _ => (
            "on-request",
            "read-only",
            json!({ "type": "readOnly", "networkAccess": false }),
        ),
    }
}

/// Arguments for Codex's app server.
pub fn launch_args() -> Vec<String> {
    vec!["app-server".to_owned()]
}

/// How the conversation starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    pub cwd: PathBuf,
    /// The id of the mode to start in, one of [`modes`].
    pub mode: String,
    /// A model from `model/list`; `None` keeps Codex's configured default.
    pub model: Option<String>,
    /// Continue an earlier thread by its id.
    pub resume: Option<String>,
}

/// Earlier threads listed for the folder, newest first.
const EARLIER_LIMIT: u32 = 20;
/// Where the threads a person can continue come from: Codex's terminal
/// interface, its editor extension, and app servers like this one. Scripted
/// `codex exec` runs and sub-agents are left out.
const PERSON_SOURCES: [&str; 3] = ["cli", "vscode", "appServer"];

/// Commands Codex runs through the protocol rather than as a message.
const COMPACT: &str = "compact";
const REVIEW: &str = "review";

/// What a request the panel sent was for, so its reply can be read.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Request {
    Initialize,
    Account,
    Models,
    Skills,
    /// Earlier threads in the folder: from Codex's index alone, which
    /// answers at once, or reconciled with its logs, which takes seconds.
    Threads {
        indexed: bool,
    },
    ThreadStart,
    ThreadResume,
    /// A turn: a message, a review, or compaction.
    Turn,
    Interrupt,
    Steer,
}

/// A request from Codex waiting for the person's answer.
#[derive(Clone, Debug)]
struct Asked {
    /// Codex's own id for the request, echoed in the reply.
    id: Value,
    kind: AskedKind,
}

#[derive(Clone, Debug)]
enum AskedKind {
    Command,
    FileChange,
    /// The permissions requested, granted unchanged when allowed.
    Permissions(Value),
    /// Each question's text and Codex's id for it.
    Questions(Vec<(String, String)>),
}

/// A turn waiting for the thread to open.
#[derive(Clone, Debug)]
enum Waiting {
    Message(Vec<Value>),
    Review(Value),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ThreadState {
    None,
    Opening,
    Open(String),
}

/// A skill Codex offers, run as a slash command.
#[derive(Clone, Debug)]
struct Skill {
    name: String,
    path: String,
    description: String,
}

/// Encodes commands and decodes output for one Codex app-server process.
#[derive(Debug)]
pub struct CodexCodec {
    cwd: String,
    mode: String,
    model: Option<String>,
    resume: Option<String>,
    next_request: u64,
    pending: HashMap<u64, Request>,
    user_agent: String,
    models: Option<Vec<ModelChoice>>,
    /// The model the thread opened with, to go back to: for a new thread,
    /// the one Codex's settings choose; for a continued one, the one it
    /// last used.
    thread_model: Option<String>,
    /// A turn named a model, which Codex keeps for later turns until
    /// another is named.
    model_named: bool,
    skills: Option<Vec<Skill>>,
    thread: ThreadState,
    waiting: Option<Waiting>,
    /// The running turn, once Codex names it.
    turn: Option<String>,
    /// A turn was asked for and Codex has not named it yet.
    turn_requested: bool,
    /// Stop the requested turn as soon as Codex names it.
    interrupt_requested: bool,
    asked: HashMap<String, Asked>,
    /// Each file-change item's changes, for the approval that follows it.
    patches: HashMap<String, Value>,
    /// The item whose text is streaming, whether it is reasoning, and its
    /// summary part.
    live: Option<(String, bool, i64)>,
    retries: u32,
}

impl CodexCodec {
    pub fn new(setup: Setup) -> Self {
        Self {
            cwd: setup.cwd.display().to_string(),
            mode: setup.mode,
            model: setup.model,
            resume: setup.resume,
            next_request: 0,
            pending: HashMap::new(),
            user_agent: String::new(),
            models: None,
            thread_model: None,
            model_named: false,
            skills: None,
            thread: ThreadState::None,
            waiting: None,
            turn: None,
            turn_requested: false,
            interrupt_requested: false,
            asked: HashMap::new(),
            patches: HashMap::new(),
            live: None,
            retries: 0,
        }
    }

    fn request(&mut self, kind: Request, method: &str, params: Value) -> String {
        self.next_request += 1;
        self.pending.insert(self.next_request, kind);
        line(&json!({ "id": self.next_request, "method": method, "params": params }))
    }

    /// Asks for the folder's earlier threads, from the index alone or
    /// reconciled with Codex's logs.
    fn list_threads(&mut self, indexed: bool) -> String {
        let params = json!({
            "cwd": self.cwd,
            "limit": EARLIER_LIMIT,
            "sortKey": "updated_at",
            "sourceKinds": PERSON_SOURCES,
            "useStateDbOnly": indexed,
        });
        self.request(Request::Threads { indexed }, "thread/list", params)
    }

    fn thread_id(&self) -> Option<&str> {
        match &self.thread {
            ThreadState::Open(id) => Some(id),
            _ => None,
        }
    }

    // ── Commands ─────────────────────────────────────────────────────────

    fn encode_prompt(&mut self, text: &str) -> Exchange {
        let name = invoked_name(text);
        // The name follows the slash at the start of the trimmed text.
        let rest = name.map_or("", |name| text.trim_start()[1 + name.len()..].trim());
        let skill = name.and_then(|name| {
            self.skills
                .iter()
                .flatten()
                .find(|skill| skill.name == name)
                .cloned()
        });
        let waiting = match (name, skill) {
            (Some(COMPACT), _) => return self.compact(),
            (Some(REVIEW), _) => Waiting::Review(if rest.is_empty() {
                json!({ "type": "uncommittedChanges" })
            } else {
                json!({ "type": "custom", "instructions": rest })
            }),
            (Some(_), Some(skill)) => Waiting::Message(vec![
                text_input(text),
                json!({ "type": "skill", "name": skill.name, "path": skill.path }),
            ]),
            // Codex's other commands work only in its own terminal, which
            // the panel offers; the model never sees them.
            (Some(_), None) => {
                return Exchange {
                    events: vec![finished_turn()],
                    lines: Vec::new(),
                };
            }
            (None, _) => Waiting::Message(vec![text_input(text)]),
        };
        self.turn_requested = true;
        match self.thread_id().map(str::to_owned) {
            Some(thread) => Exchange::send(vec![self.start_turn(&thread, waiting)]),
            None => {
                self.waiting = Some(waiting);
                if self.thread == ThreadState::None {
                    Exchange::send(vec![self.open_thread()])
                } else {
                    Exchange::default()
                }
            }
        }
    }

    fn compact(&mut self) -> Exchange {
        let Some(thread) = self.thread_id().map(str::to_owned) else {
            return Exchange {
                events: vec![failed_turn("There is nothing to compact yet")],
                lines: Vec::new(),
            };
        };
        self.turn_requested = true;
        Exchange::send(vec![self.request(
            Request::Turn,
            "thread/compact/start",
            json!({ "threadId": thread }),
        )])
    }

    /// Starts a new thread, or continues the earlier one.
    fn open_thread(&mut self) -> String {
        self.thread = ThreadState::Opening;
        let (approval, sandbox, _) = policy(&self.mode);
        // The model goes with each turn, so the thread reports the one
        // Codex's settings choose, to go back to.
        let mut params = json!({
            "cwd": self.cwd,
            "approvalPolicy": approval,
            "sandbox": sandbox,
        });
        match self.resume.clone() {
            Some(thread) => {
                params["threadId"] = Value::String(thread);
                // The panel already shows the conversation.
                params["excludeTurns"] = Value::Bool(true);
                self.request(Request::ThreadResume, "thread/resume", params)
            }
            None => self.request(Request::ThreadStart, "thread/start", params),
        }
    }

    fn start_turn(&mut self, thread: &str, waiting: Waiting) -> String {
        match waiting {
            Waiting::Message(input) => {
                let (approval, _, sandbox) = policy(&self.mode);
                let mut params = json!({
                    "threadId": thread,
                    "input": input,
                    "approvalPolicy": approval,
                    "sandboxPolicy": sandbox,
                });
                let model = match &self.model {
                    Some(model) => Some(model.clone()),
                    // Back to the default after naming another.
                    None if self.model_named => self.thread_model.clone(),
                    None => None,
                };
                if let Some(model) = model {
                    self.model_named = true;
                    params["model"] = Value::String(model);
                }
                self.request(Request::Turn, "turn/start", params)
            }
            Waiting::Review(target) => self.request(
                Request::Turn,
                "review/start",
                json!({ "threadId": thread, "target": target, "delivery": "inline" }),
            ),
        }
    }

    fn encode_answer(&mut self, request_id: &str, decision: &ApprovalDecision) -> Exchange {
        let Some(asked) = self.asked.remove(request_id) else {
            return Exchange::default();
        };
        let result = match (&asked.kind, decision) {
            (
                AskedKind::Command | AskedKind::FileChange,
                ApprovalDecision::Allow { remember, .. },
            ) => {
                json!({ "decision": remember.clone().unwrap_or_else(|| json!("accept")) })
            }
            (
                AskedKind::Command | AskedKind::FileChange,
                ApprovalDecision::Deny { interrupt, .. },
            ) => {
                json!({ "decision": if *interrupt { "cancel" } else { "decline" } })
            }
            (AskedKind::Permissions(requested), ApprovalDecision::Allow { remember, .. }) => {
                json!({
                    "permissions": requested,
                    "scope": if remember.is_some() { "session" } else { "turn" },
                })
            }
            (AskedKind::Permissions(_), ApprovalDecision::Deny { .. }) => {
                json!({ "permissions": {}, "scope": "turn" })
            }
            (AskedKind::Questions(questions), ApprovalDecision::Allow { input, .. }) => {
                let chosen = input.get("answers");
                let answers: Map<String, Value> = questions
                    .iter()
                    .filter_map(|(question, id)| {
                        let answer = chosen?.get(question)?.as_str()?;
                        Some((id.clone(), json!({ "answers": [answer] })))
                    })
                    .collect();
                json!({ "answers": answers })
            }
            (AskedKind::Questions(_), ApprovalDecision::Deny { .. }) => json!({ "answers": {} }),
        };
        let mut lines = vec![line(&json!({ "id": asked.id, "result": result }))];
        // Codex takes no reason with a refusal; the person's words reach it
        // as input to the running turn.
        if let ApprovalDecision::Deny {
            message,
            interrupt: false,
        } = decision
            && message != DECLINED_MESSAGE
            && let (Some(thread), Some(turn)) = (self.thread_id(), self.turn.clone())
        {
            let params = json!({
                "threadId": thread,
                "input": [text_input(message)],
                "expectedTurnId": turn,
            });
            lines.push(self.request(Request::Steer, "turn/steer", params));
        }
        Exchange::send(lines)
    }

    fn encode_interrupt(&mut self) -> Exchange {
        if let (Some(thread), Some(turn)) = (self.thread_id(), self.turn.clone()) {
            let params = json!({ "threadId": thread, "turnId": turn });
            return Exchange::send(vec![self.request(
                Request::Interrupt,
                "turn/interrupt",
                params,
            )]);
        }
        if self.waiting.take().is_some() {
            // The turn never reached Codex.
            self.turn_requested = false;
            return Exchange {
                events: vec![finished_turn()],
                lines: Vec::new(),
            };
        }
        if self.turn_requested {
            self.interrupt_requested = true;
        }
        Exchange::default()
    }

    // ── Output ───────────────────────────────────────────────────────────

    fn decode_response(&mut self, id: u64, message: &Map<String, Value>) -> Exchange {
        let Some(kind) = self.pending.remove(&id) else {
            return Exchange::default();
        };
        let result = message.get("result");
        let error = message.get("error").map(|error| {
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Codex refused the request")
                .to_owned()
        });
        let mut exchange = Exchange::default();
        match (kind, error) {
            (Request::Initialize, Some(error)) => {
                exchange.events.push(AgentEvent::HandshakeFailed(error));
            }
            (Request::Initialize, None) => {
                self.user_agent = result
                    .and_then(|result| result.get("userAgent"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                exchange
                    .lines
                    .push(line(&json!({ "method": "initialized" })));
                exchange
                    .lines
                    .push(self.request(Request::Account, "account/read", json!({})));
                exchange
                    .lines
                    .push(self.request(Request::Models, "model/list", json!({})));
                let skills = json!({ "cwds": [self.cwd] });
                exchange
                    .lines
                    .push(self.request(Request::Skills, "skills/list", skills));
                exchange.lines.push(self.list_threads(true));
                // Accepted: the person may write while the lists load.
                exchange.events.push(AgentEvent::Ready(self.handshake()));
            }
            (Request::Account, None) => {
                let result = result.cloned().unwrap_or(Value::Null);
                if result.get("account").is_none_or(Value::is_null)
                    && result.get("requiresOpenaiAuth").and_then(Value::as_bool) == Some(true)
                {
                    exchange.events.push(AgentEvent::Warning(
                        "Codex is not signed in. Run `codex login` in a terminal, then send \
                         your message again."
                            .to_owned(),
                    ));
                }
            }
            (Request::Models, error) => {
                if let Some(error) = error {
                    log::debug!("agent view: Codex models unavailable: {error}");
                }
                self.models = Some(models(result.and_then(|result| result.get("data"))));
                exchange.events.push(AgentEvent::Ready(self.handshake()));
            }
            (Request::Skills, error) => {
                if let Some(error) = error {
                    log::debug!("agent view: Codex skills unavailable: {error}");
                }
                self.skills = Some(skills(result));
                exchange.events.push(AgentEvent::Ready(self.handshake()));
            }
            (Request::Threads { indexed }, error) => {
                match error {
                    Some(error) => log::debug!("agent view: Codex threads unavailable: {error}"),
                    None => exchange
                        .events
                        .push(AgentEvent::EarlierConversations(earlier(
                            result.and_then(|result| result.get("data")),
                        ))),
                }
                // The index can miss threads its logs hold; the full list,
                // seconds later, replaces the quick one.
                if indexed {
                    exchange.lines.push(self.list_threads(false));
                }
            }
            (kind @ (Request::ThreadStart | Request::ThreadResume), None) => {
                let result = result.cloned().unwrap_or(Value::Null);
                let Some(thread) = result.pointer("/thread/id").and_then(Value::as_str) else {
                    self.thread = ThreadState::None;
                    exchange
                        .events
                        .extend(self.fail_waiting("Codex did not open a conversation"));
                    return exchange;
                };
                self.thread = ThreadState::Open(thread.to_owned());
                self.thread_model = text(&result, "model");
                if kind == Request::ThreadStart {
                    self.resume = None;
                }
                exchange
                    .events
                    .push(AgentEvent::SessionStarted(SessionInfo {
                        session_id: thread.to_owned(),
                        model: text(&result, "model").unwrap_or_default(),
                        cwd: text(&result, "cwd").unwrap_or_default(),
                        permission_mode: None,
                        version: self.user_agent.clone(),
                    }));
                if let Some(waiting) = self.waiting.take() {
                    let thread = thread.to_owned();
                    exchange.lines.push(self.start_turn(&thread, waiting));
                }
            }
            (Request::ThreadStart, Some(error)) => {
                self.thread = ThreadState::None;
                exchange.events.extend(
                    self.fail_waiting(&format!("Codex could not start the conversation: {error}")),
                );
            }
            (Request::ThreadResume, Some(error)) => {
                self.resume = None;
                exchange.events.push(AgentEvent::Warning(format!(
                    "Codex could not continue the earlier conversation ({error}), so this \
                     starts a new one"
                )));
                exchange.lines.push(self.open_thread());
            }
            (Request::Turn, None) => {
                if let Some(turn) = result
                    .and_then(|result| result.pointer("/turn/id"))
                    .and_then(Value::as_str)
                {
                    self.turn = Some(turn.to_owned());
                }
                if std::mem::take(&mut self.interrupt_requested) {
                    exchange.lines.extend(self.encode_interrupt().lines);
                }
            }
            (Request::Turn, Some(error)) => {
                self.turn_requested = false;
                self.interrupt_requested = false;
                exchange.events.push(failed_turn(&error));
            }
            (Request::Interrupt, Some(error)) => exchange.events.push(AgentEvent::Warning(
                format!("Codex could not stop the turn: {error}"),
            )),
            (Request::Steer, Some(error)) => exchange.events.push(AgentEvent::Warning(format!(
                "Codex did not receive your note: {error}"
            ))),
            (Request::Account | Request::Interrupt | Request::Steer, _) => {}
        }
        exchange
    }

    /// Ends a turn that waited for a thread that did not open.
    fn fail_waiting(&mut self, error: &str) -> Option<AgentEvent> {
        self.waiting.take()?;
        self.turn_requested = false;
        Some(failed_turn(error))
    }

    fn handshake(&self) -> Handshake {
        let mut commands = vec![
            SlashCommand {
                name: COMPACT.to_owned(),
                description: "Summarize the conversation to free up context".to_owned(),
                argument_hint: String::new(),
            },
            SlashCommand {
                name: REVIEW.to_owned(),
                description: "Review uncommitted changes, or what you describe".to_owned(),
                argument_hint: "[instructions]".to_owned(),
            },
        ];
        for skill in self.skills.iter().flatten() {
            if !commands.iter().any(|command| command.name == skill.name) {
                commands.push(SlashCommand {
                    name: skill.name.clone(),
                    description: skill.description.clone(),
                    argument_hint: String::new(),
                });
            }
        }
        Handshake {
            models: self.models.clone().unwrap_or_else(|| models(None)),
            commands,
            modes: modes(),
            permission_mode: None,
            ..Handshake::default()
        }
    }

    fn decode_request(&mut self, id: Value, method: &str, params: &Value) -> Exchange {
        let key = request_key("codex", &id);
        let item = text(params, "itemId");
        let reason = text(params, "reason");
        let from_sub_agent = self.other_thread(params);
        let approval = |tool_name: &str, input: Value, reason: Option<String>, remember| {
            AgentEvent::ApprovalRequested(ApprovalRequest {
                request_id: key.clone(),
                tool_use_id: item.clone(),
                tool_name: tool_name.to_owned(),
                input,
                title: None,
                description: None,
                reason,
                remember,
                from_sub_agent,
            })
        };
        let (kind, event) = match method {
            "item/commandExecution/requestApproval" => {
                let command = text(params, "command").map(|command| display_command(&command));
                let network = params
                    .pointer("/networkApprovalContext/host")
                    .and_then(Value::as_str)
                    .map(|host| format!("It needs network access to {host}"));
                let remember = match params
                    .get("proposedExecpolicyAmendment")
                    .and_then(Value::as_array)
                    .filter(|prefix| !prefix.is_empty())
                {
                    Some(prefix) => {
                        let words: Vec<&str> = prefix.iter().filter_map(Value::as_str).collect();
                        RememberOption {
                            label: format!("Always allow {} commands", words.join(" ")),
                            updates: json!({
                                "acceptWithExecpolicyAmendment": { "execpolicy_amendment": prefix },
                            }),
                        }
                    }
                    None => RememberOption {
                        label: "Allow this command for the rest of the session".to_owned(),
                        updates: json!("acceptForSession"),
                    },
                };
                (
                    AskedKind::Command,
                    approval(
                        "Bash",
                        json!({ "command": command.unwrap_or_default() }),
                        reason.or(network),
                        Some(remember),
                    ),
                )
            }
            "item/fileChange/requestApproval" => {
                let changes = item
                    .as_ref()
                    .and_then(|item| self.patches.get(item))
                    .cloned()
                    .unwrap_or_else(|| json!([]));
                let root = text(params, "grantRoot")
                    .map(|root| format!("It wants to write anywhere under {root}"));
                (
                    AskedKind::FileChange,
                    approval(
                        PATCH_TOOL,
                        json!({ "changes": changes }),
                        reason.or(root),
                        Some(RememberOption {
                            label: "Allow edits to these files for the rest of the session"
                                .to_owned(),
                            updates: json!("acceptForSession"),
                        }),
                    ),
                )
            }
            "item/permissions/requestApproval" => {
                let requested = params.get("permissions").cloned().unwrap_or(json!({}));
                (
                    AskedKind::Permissions(requested.clone()),
                    approval(
                        "Permissions",
                        json!({ "permissions": requested }),
                        reason,
                        Some(RememberOption {
                            label: "Grant these for the rest of the session".to_owned(),
                            updates: json!("session"),
                        }),
                    ),
                )
            }
            "item/tool/requestUserInput" => {
                let mut ids = Vec::new();
                let questions: Vec<Value> = params
                    .get("questions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|question| {
                        let text_of = |key| text(question, key);
                        let asked = text_of("question")?;
                        ids.push((asked.clone(), text_of("id")?));
                        let options: Vec<Value> = question
                            .get("options")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .map(|option| {
                                json!({
                                    "label": text(option, "label").unwrap_or_default(),
                                    "description": text(option, "description").unwrap_or_default(),
                                })
                            })
                            .collect();
                        Some(json!({
                            "question": asked,
                            "header": text_of("header").unwrap_or_default(),
                            "options": options,
                            "multiSelect": false,
                        }))
                    })
                    .collect();
                (
                    AskedKind::Questions(ids),
                    approval(
                        "AskUserQuestion",
                        json!({ "questions": questions }),
                        None,
                        None,
                    ),
                )
            }
            "mcpServer/elicitation/request" => {
                let server = text(params, "serverName").unwrap_or_else(|| "An MCP server".into());
                return Exchange {
                    events: vec![AgentEvent::Warning(format!(
                        "{server} asked for input the agent view cannot show yet, so it was \
                         declined"
                    ))],
                    lines: vec![line(&json!({
                        "id": id,
                        "result": { "action": "decline", "content": null, "_meta": null },
                    }))],
                };
            }
            _ => {
                return Exchange::send(vec![line(
                    &json!({ "id": id, "error": method_not_found(method) }),
                )]);
            }
        };
        self.asked.insert(key, Asked { id, kind });
        Exchange {
            events: vec![event],
            lines: Vec::new(),
        }
    }

    fn decode_notification(&mut self, method: &str, params: &Value) -> Vec<AgentEvent> {
        // Every thread's requests are answered here, so every thread's
        // settle here too.
        if method == "serverRequest/resolved" {
            return self.resolved(params);
        }
        // Sub-agents run threads of their own on the same connection; only
        // the conversation's own thread shapes it.
        if self.other_thread(params) {
            return Vec::new();
        }
        match method {
            "item/started" | "item/completed" => params
                .get("item")
                .map(|item| self.item(item, method == "item/completed"))
                .unwrap_or_default(),
            "item/agentMessage/delta" => self.delta(params, false, 0),
            "item/reasoning/summaryTextDelta" => {
                let part = params
                    .get("summaryIndex")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                self.delta(params, true, part)
            }
            "item/fileChange/patchUpdated" => {
                let (Some(item), Some(changes)) = (text(params, "itemId"), params.get("changes"))
                else {
                    return Vec::new();
                };
                self.patches.insert(item.clone(), changes.clone());
                vec![tool_use(&item, PATCH_TOOL, json!({ "changes": changes }))]
            }
            "turn/plan/updated" => {
                let id = format!("plan-{}", text(params, "turnId").unwrap_or_default());
                let todos: Vec<Value> = params
                    .get("plan")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|step| {
                        let content = text(step, "step")?;
                        let status = match text(step, "status").as_deref() {
                            Some("completed") => "completed",
                            Some("inProgress") => "in_progress",
                            _ => "pending",
                        };
                        Some(todo(&content, status))
                    })
                    .collect();
                plan(&id, todos)
            }
            "turn/started" => {
                if let Some(turn) = params.pointer("/turn/id").and_then(Value::as_str) {
                    self.turn = Some(turn.to_owned());
                }
                Vec::new()
            }
            "turn/completed" => {
                self.turn = None;
                self.turn_requested = false;
                self.interrupt_requested = false;
                self.live = None;
                self.retries = 0;
                vec![AgentEvent::TurnFinished(self.outcome(params.get("turn")))]
            }
            "thread/tokenUsage/updated" => {
                let usage = params.get("tokenUsage");
                let used = usage
                    .and_then(|usage| usage.pointer("/last/totalTokens"))
                    .and_then(Value::as_u64);
                let window = usage
                    .and_then(|usage| usage.get("modelContextWindow"))
                    .and_then(Value::as_u64);
                match (used, window) {
                    (Some(used_tokens), Some(max_tokens)) => {
                        vec![AgentEvent::ContextUsage(ContextUsage {
                            used_tokens,
                            max_tokens,
                        })]
                    }
                    _ => Vec::new(),
                }
            }
            "error" => {
                if params.get("willRetry").and_then(Value::as_bool) != Some(true) {
                    // The turn ends with this error; its outcome reports it.
                    return Vec::new();
                }
                self.retries += 1;
                vec![AgentEvent::Retrying(Retry {
                    attempt: self.retries,
                    max_attempts: 0,
                    delay: Duration::ZERO,
                    error: params
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("The request failed")
                        .to_owned(),
                })]
            }
            "warning" | "guardianWarning" => text(params, "message")
                .map(AgentEvent::Warning)
                .into_iter()
                .collect(),
            "configWarning" | "deprecationNotice" => text(params, "summary")
                .map(AgentEvent::Warning)
                .into_iter()
                .collect(),
            "model/rerouted" => match (text(params, "fromModel"), text(params, "toModel")) {
                (Some(from), Some(to)) => vec![AgentEvent::Warning(format!(
                    "Codex moved this turn from {from} to {to}"
                ))],
                _ => Vec::new(),
            },
            "mcpServer/startupStatus/updated"
                if text(params, "status").as_deref() == Some("failed") =>
            {
                let name = text(params, "name").unwrap_or_else(|| "An MCP server".into());
                let error =
                    text(params, "error").map_or_else(String::new, |error| format!(": {error}"));
                vec![AgentEvent::Warning(format!("{name} did not start{error}"))]
            }
            // Status, settings, account, and the many other notifications
            // carry nothing the panel shows that the ones above do not.
            _ => Vec::new(),
        }
    }

    /// Withdraws a request Codex settled without waiting for an answer, as
    /// when its turn ended.
    fn resolved(&mut self, params: &Value) -> Vec<AgentEvent> {
        let Some(id) = params.get("requestId") else {
            return Vec::new();
        };
        let key = request_key("codex", id);
        if self.asked.remove(&key).is_some() {
            vec![AgentEvent::ApprovalWithdrawn { request_id: key }]
        } else {
            Vec::new()
        }
    }

    /// Whether a message names a thread other than the conversation's.
    fn other_thread(&self, params: &Value) -> bool {
        match (
            params.get("threadId").and_then(Value::as_str),
            self.thread_id(),
        ) {
            (Some(named), Some(own)) => named != own,
            _ => false,
        }
    }

    /// Streams a text or reasoning delta, starting its block on the first.
    fn delta(&mut self, params: &Value, thinking: bool, part: i64) -> Vec<AgentEvent> {
        let (Some(item), Some(delta)) = (
            text(params, "itemId"),
            params.get("delta").and_then(Value::as_str),
        ) else {
            return Vec::new();
        };
        let mut events = Vec::new();
        let mut delta = delta.to_owned();
        match &mut self.live {
            Some((live, live_thinking, live_part))
                if *live == item && *live_thinking == thinking =>
            {
                if *live_part != part {
                    *live_part = part;
                    delta.insert_str(0, "\n\n");
                }
            }
            _ => {
                events.extend(stream_started(&item, thinking));
                self.live = Some((item, thinking, part));
            }
        }
        events.push(stream_piece(thinking, delta));
        events
    }

    /// Events for an item that started or completed.
    fn item(&mut self, item: &Value, completed: bool) -> Vec<AgentEvent> {
        let (Some(kind), Some(id)) = (text(item, "type"), text(item, "id")) else {
            return Vec::new();
        };
        let status = text(item, "status");
        let failed = matches!(status.as_deref(), Some("failed" | "declined"));
        match kind.as_str() {
            "agentMessage" | "plan" if completed => {
                let text = text(item, "text").unwrap_or_default();
                self.settle(&id, ContentBlock::Text(text))
            }
            "exitedReviewMode" if completed => {
                let text = text(item, "review").unwrap_or_default();
                self.settle(&id, ContentBlock::Text(text))
            }
            "reasoning" if completed => {
                let joined = |key: &str| {
                    item.get(key)
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n\n")
                };
                let summary = joined("summary");
                let text = if summary.trim().is_empty() {
                    joined("content")
                } else {
                    summary
                };
                self.settle(&id, ContentBlock::Thinking(text))
            }
            "commandExecution" => {
                let command = text(item, "command").map(|command| display_command(&command));
                let mut events = vec![tool_use(
                    &id,
                    "Bash",
                    json!({ "command": command.unwrap_or_default() }),
                )];
                if completed {
                    let exit_code = item.get("exitCode").and_then(Value::as_i64);
                    events.push(tool_result(
                        &id,
                        text(item, "aggregatedOutput").unwrap_or_default(),
                        failed || exit_code.is_some_and(|code| code != 0),
                    ));
                }
                events
            }
            "fileChange" => {
                let changes = item.get("changes").cloned().unwrap_or_else(|| json!([]));
                self.patches.insert(id.clone(), changes.clone());
                let mut events = vec![tool_use(&id, PATCH_TOOL, json!({ "changes": changes }))];
                if completed {
                    self.patches.remove(&id);
                    let error = match status.as_deref() {
                        Some("failed") => "The changes could not be applied",
                        Some("declined") => "The changes were declined",
                        _ => "",
                    };
                    events.push(tool_result(&id, error.to_owned(), failed));
                }
                events
            }
            "mcpToolCall" => {
                let name = format!(
                    "mcp__{}__{}",
                    text(item, "server").unwrap_or_default(),
                    text(item, "tool").unwrap_or_default()
                );
                let arguments = item.get("arguments").cloned().unwrap_or(Value::Null);
                let mut events = vec![tool_use(&id, &name, arguments)];
                if completed {
                    let content = item
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| content_text(item.pointer("/result/content")));
                    events.push(tool_result(&id, content, failed));
                }
                events
            }
            "webSearch" => {
                let query = text(item, "query").unwrap_or_default();
                let mut events = vec![tool_use(&id, "WebSearch", json!({ "query": query }))];
                if completed {
                    events.push(tool_result(&id, String::new(), false));
                }
                events
            }
            "collabAgentToolCall" => {
                let description = text(item, "prompt")
                    .and_then(|prompt| prompt.lines().next().map(str::to_owned))
                    .unwrap_or_default();
                let mut events = vec![tool_use(
                    &id,
                    "Agent",
                    json!({ "description": description, "subagent_type": text(item, "tool") }),
                )];
                if completed {
                    events.push(tool_result(&id, String::new(), failed));
                }
                events
            }
            "contextCompaction" => vec![AgentEvent::Compacting(!completed)],
            // The person's own message, hooks, images, and the rest show
            // nothing the conversation does not already.
            _ => Vec::new(),
        }
    }

    /// The complete text of a streamed or unstreamed item.
    fn settle(&mut self, id: &str, block: ContentBlock) -> Vec<AgentEvent> {
        let streamed = self.live.as_ref().is_some_and(|(live, ..)| live == id);
        if streamed {
            self.live = None;
        }
        settled(id, block, streamed)
    }

    fn outcome(&self, turn: Option<&Value>) -> TurnOutcome {
        let turn = turn.cloned().unwrap_or(Value::Null);
        let status = text(&turn, "status");
        let success = status.as_deref() != Some("failed");
        let error = (!success).then(|| {
            let message = turn
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("The turn ended with an error");
            match turn
                .pointer("/error/codexErrorInfo")
                .and_then(Value::as_str)
            {
                Some("usageLimitExceeded") => format!("{message} Codex's usage limit was reached."),
                Some("unauthorized") => {
                    format!("{message} Run `codex login` in a terminal, then try again.")
                }
                Some("contextWindowExceeded") => {
                    format!("{message} Try /compact, or start a new session.")
                }
                _ => message.to_owned(),
            }
        });
        TurnOutcome {
            success,
            error,
            duration: Duration::from_millis(
                turn.get("durationMs").and_then(Value::as_u64).unwrap_or(0),
            ),
            cost_usd: None,
            session_id: self.thread_id().map(str::to_owned),
            denials: 0,
            interrupted: status.as_deref() == Some("interrupted"),
        }
    }
}

impl Protocol for CodexCodec {
    fn start(&mut self) -> Vec<String> {
        let params = json!({
            "clientInfo": client_info(),
            "capabilities": { "experimentalApi": false, "requestAttestation": false },
        });
        let mut lines = vec![self.request(Request::Initialize, "initialize", params)];
        // A continued conversation reopens at once, so it can be stopped,
        // renamed, and resumed like one that never closed.
        if self.resume.is_some() {
            lines.push(self.open_thread());
        }
        lines
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
            // Both apply from the next turn on, which is where Codex takes
            // them.
            AgentCommand::SetMode(mode) => {
                self.mode.clone_from(mode);
                Exchange::default()
            }
            AgentCommand::SetModel(model) => {
                self.model = (model != "default").then(|| model.clone());
                Exchange::default()
            }
            // Codex reports usage after every request on its own.
            AgentCommand::QueryContextUsage => Exchange::default(),
            // Codex's modes already pair approvals with a sandbox, and its
            // effort follows its settings.
            AgentCommand::SetEffort(_) | AgentCommand::SetAutoApprove(_) => Exchange::default(),
        }
    }

    fn decode(&mut self, line: &str) -> Exchange {
        match incoming(line) {
            Ok(Incoming::Request { id, method, params }) => {
                self.decode_request(id, &method, &params)
            }
            Ok(Incoming::Notification { method, params }) => Exchange {
                events: self.decode_notification(&method, &params),
                lines: Vec::new(),
            },
            Ok(Incoming::Response { id, message }) => self.decode_response(id, &message),
            Err(unhandled) => unhandled.exchange(),
        }
    }
}

fn text_input(text: &str) -> Value {
    json!({ "type": "text", "text": text, "text_elements": [] })
}

/// Codex's models, led by the one its settings choose.
fn models(listed: Option<&Value>) -> Vec<ModelChoice> {
    let mut models = vec![ModelChoice {
        value: "default".to_owned(),
        display_name: "Default".to_owned(),
        description: "The model your Codex settings choose".to_owned(),
        group: None,
    }];
    models.extend(
        listed
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|model| model.get("hidden").and_then(Value::as_bool) != Some(true))
            .filter_map(|model| {
                let value = text(model, "model")?;
                Some(ModelChoice {
                    display_name: text(model, "displayName").unwrap_or_else(|| value.clone()),
                    description: text(model, "description").unwrap_or_default(),
                    value,
                    group: None,
                })
            }),
    );
    models
}

/// Threads a person can continue, titled by their name or first message.
fn earlier(listed: Option<&Value>) -> Vec<EarlierConversation> {
    listed
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|thread| {
            let id = text(thread, "id")?;
            let named = text(thread, "name").filter(|name| !name.trim().is_empty());
            let title = named
                .or_else(|| text(thread, "preview"))
                .map(|title| earlier_title(&title))
                .filter(|title| !title.is_empty())?;
            let seconds = thread
                .get("updatedAt")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            Some(EarlierConversation {
                id,
                title,
                modified: std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(seconds),
            })
        })
        .collect()
}

fn skills(result: Option<&Value>) -> Vec<Skill> {
    result
        .and_then(|result| result.get("data"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("skills")?.as_array())
        .flatten()
        .filter(|skill| skill.get("enabled").and_then(Value::as_bool) != Some(false))
        .filter_map(|skill| {
            // The short description suits a one-line menu.
            let description = skill
                .pointer("/interface/shortDescription")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| text(skill, "shortDescription"))
                .or_else(|| text(skill, "description"))
                .unwrap_or_default();
            Some(Skill {
                name: text(skill, "name")?,
                path: text(skill, "path")?,
                description,
            })
        })
        .collect()
}

/// A command as the person would type it: Codex runs commands through the
/// login shell, `bash -lc '…'`, which says nothing about the command.
fn display_command(command: &str) -> String {
    let Some((program, rest)) = command.split_once(' ') else {
        return command.to_owned();
    };
    let shell = program.rsplit('/').next().unwrap_or(program);
    if matches!(shell, "bash" | "zsh" | "sh")
        && let Some(script) = rest.strip_prefix("-lc ")
        && let Some(script) = unquote(script)
    {
        return script;
    }
    command.to_owned()
}

/// The content of one shell-quoted word, when `text` is exactly that.
fn unquote(text: &str) -> Option<String> {
    if let Some(inner) = text
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        // POSIX quoting writes a single quote as '\''; any other quote ends
        // the word.
        let escaped = "'\\''";
        return (!inner.replace(escaped, "").contains('\'')).then(|| inner.replace(escaped, "'"));
    }
    if let Some(inner) = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        let special = ['$', '`', '\\', '"'];
        return (!inner.contains(special)).then(|| inner.to_owned());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_view::event::{BlockStart, EARLIER_TITLE_CHARS, StreamDelta, Thread};
    use crate::agent_view::protocol::testing::{decode, parse};

    fn codec() -> CodexCodec {
        CodexCodec::new(Setup {
            cwd: PathBuf::from("/w"),
            mode: ASK_MODE.to_owned(),
            model: None,
            resume: None,
        })
    }

    /// Answers the handshake and opens a thread with a turn running.
    fn with_turn() -> CodexCodec {
        let mut codec = codec();
        codec.start();
        decode(
            &mut codec,
            json!({ "id": 1, "result": { "userAgent": "codex/1" } }),
        );
        let opened = codec.encode(&AgentCommand::Prompt("hello".into()));
        let open = parse(&opened.lines[0]);
        assert_eq!(open["method"], "thread/start");
        assert_eq!(open["params"]["approvalPolicy"], "on-request");
        assert_eq!(open["params"]["sandbox"], "read-only");
        assert!(open["params"].get("model").is_none());
        let exchange = decode(
            &mut codec,
            json!({ "id": open["id"], "result": { "thread": { "id": "t1" }, "model": "m", "cwd": "/w" } }),
        );
        let turn = parse(&exchange.lines[0]);
        assert_eq!(turn["method"], "turn/start");
        assert_eq!(turn["params"]["threadId"], "t1");
        assert_eq!(turn["params"]["input"][0]["text"], "hello");
        assert_eq!(turn["params"]["input"][0]["text_elements"], json!([]));
        assert_eq!(turn["params"]["sandboxPolicy"]["type"], "readOnly");
        decode(
            &mut codec,
            json!({ "id": turn["id"], "result": { "turn": { "id": "u1" } } }),
        );
        codec
    }

    #[test]
    fn the_handshake_is_ready_at_once_and_fills_in_as_lists_arrive() {
        let mut codec = codec();
        let start = codec.start();
        assert_eq!(start.len(), 1);
        let initialize = parse(&start[0]);
        assert_eq!(initialize["method"], "initialize");
        assert!(initialize.get("jsonrpc").is_none());
        assert_eq!(
            initialize["params"]["capabilities"]["experimentalApi"],
            false
        );
        let exchange = decode(
            &mut codec,
            json!({ "id": 1, "result": { "userAgent": "codex/1" } }),
        );
        let methods: Vec<String> = exchange
            .lines
            .iter()
            .map(|line| parse(line)["method"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            methods,
            [
                "initialized",
                "account/read",
                "model/list",
                "skills/list",
                "thread/list"
            ]
        );
        let threads = parse(&exchange.lines[4]);
        assert_eq!(threads["params"]["cwd"], "/w");
        // The index answers at once; the full list follows it.
        assert_eq!(threads["params"]["useStateDbOnly"], true);
        assert_eq!(
            threads["params"]["sourceKinds"],
            json!(["cli", "vscode", "appServer"])
        );
        let [AgentEvent::Ready(handshake)] = exchange.events.as_slice() else {
            panic!("{:?}", exchange.events)
        };
        assert_eq!(handshake.models.len(), 1);
        assert_eq!(handshake.modes, modes());

        let refused = decode(
            &mut codec,
            json!({ "id": 3, "error": { "code": 1, "message": "offline" } }),
        );
        let [AgentEvent::Ready(handshake)] = refused.events.as_slice() else {
            panic!()
        };
        assert_eq!(handshake.models[0].value, "default");
    }

    #[test]
    fn a_refused_initialize_fails_the_handshake() {
        let mut codec = codec();
        codec.start();
        let exchange = decode(
            &mut codec,
            json!({ "id": 1, "error": { "code": -1, "message": "old" } }),
        );
        assert_eq!(exchange.events, [AgentEvent::HandshakeFailed("old".into())]);
    }

    #[test]
    fn a_continued_thread_reopens_with_the_process() {
        let mut codec = CodexCodec::new(Setup {
            cwd: PathBuf::from("/w"),
            mode: "auto".into(),
            model: None,
            resume: Some("t0".into()),
        });
        let start = codec.start();
        let resume = parse(&start[1]);
        assert_eq!(resume["method"], "thread/resume");
        assert_eq!(resume["params"]["threadId"], "t0");
        assert_eq!(resume["params"]["excludeTurns"], true);
        assert_eq!(resume["params"]["sandbox"], "workspace-write");
        // A message written while it reopens waits for it.
        let waiting = codec.encode(&AgentCommand::Prompt("more".into()));
        assert!(waiting.lines.is_empty());
        let opened = decode(
            &mut codec,
            json!({ "id": resume["id"], "result": { "thread": { "id": "t0" } } }),
        );
        assert!(matches!(opened.events[0], AgentEvent::SessionStarted(_)));
        assert_eq!(parse(&opened.lines[0])["method"], "turn/start");
    }

    #[test]
    fn stopping_before_codex_names_the_turn_waits_for_the_name() {
        let mut codec = codec();
        codec.start();
        decode(&mut codec, json!({ "id": 1, "result": {} }));
        let open = parse(&codec.encode(&AgentCommand::Prompt("hi".into())).lines[0]);
        // Stopped before the thread opened: the turn never reaches Codex.
        let stopped = codec.encode(&AgentCommand::Interrupt);
        assert!(stopped.lines.is_empty());
        assert!(matches!(stopped.events[..], [AgentEvent::TurnFinished(_)]));
        let opened = decode(
            &mut codec,
            json!({ "id": open["id"], "result": { "thread": { "id": "t1" } } }),
        );
        assert!(opened.lines.is_empty());

        let turn = parse(&codec.encode(&AgentCommand::Prompt("again".into())).lines[0]);
        assert!(codec.encode(&AgentCommand::Interrupt).lines.is_empty());
        let named = decode(
            &mut codec,
            json!({ "id": turn["id"], "result": { "turn": { "id": "u2" } } }),
        );
        let interrupt = parse(&named.lines[0]);
        assert_eq!(interrupt["method"], "turn/interrupt");
        assert_eq!(
            interrupt["params"],
            json!({ "threadId": "t1", "turnId": "u2" })
        );
    }

    #[test]
    fn every_request_codex_makes_is_answered() {
        let mut codec = with_turn();
        let unknown = decode(
            &mut codec,
            json!({ "id": 7, "method": "item/tool/call", "params": {} }),
        );
        assert!(unknown.events.is_empty());
        let reply = parse(&unknown.lines[0]);
        assert_eq!(reply["id"], 7);
        assert_eq!(reply["error"]["code"], -32601);

        let form = decode(
            &mut codec,
            json!({ "id": "e1", "method": "mcpServer/elicitation/request",
                    "params": { "threadId": "t1", "serverName": "docs", "mode": "form" } }),
        );
        assert_eq!(parse(&form.lines[0])["result"]["action"], "decline");
        assert!(
            matches!(&form.events[0], AgentEvent::Warning(text) if text.starts_with("docs asked"))
        );
    }

    #[test]
    fn approvals_carry_codex_ids_and_answer_in_its_terms() {
        let mut codec = with_turn();
        let asked = decode(
            &mut codec,
            json!({ "id": 0, "method": "item/commandExecution/requestApproval", "params": {
                "threadId": "t1", "turnId": "u1", "itemId": "c1", "command": "/bin/zsh -lc \"ls -la\"",
                "networkApprovalContext": { "host": "crates.io", "protocol": "https" } } }),
        );
        let [AgentEvent::ApprovalRequested(request)] = asked.events.as_slice() else {
            panic!()
        };
        assert_eq!(request.request_id, "codex-0");
        assert_eq!(request.tool_use_id.as_deref(), Some("c1"));
        assert_eq!(request.input["command"], "ls -la");
        assert_eq!(
            request.reason.as_deref(),
            Some("It needs network access to crates.io")
        );
        assert!(!request.from_sub_agent);
        let remember = request.remember.clone().unwrap();
        assert_eq!(remember.updates, json!("acceptForSession"));

        let deny = |interrupt| AgentCommand::Answer {
            request_id: "codex-0".into(),
            tool_use_id: None,
            decision: ApprovalDecision::Deny {
                message: DECLINED_MESSAGE.into(),
                interrupt,
            },
        };
        let cancelled = codec.encode(&deny(true));
        assert_eq!(
            parse(&cancelled.lines[0]),
            json!({ "id": 0, "result": { "decision": "cancel" } })
        );
        // Answered once; a second answer has nothing to answer.
        assert!(codec.encode(&deny(false)).lines.is_empty());

        // A sub-agent's thread asks too, and is still answered.
        let other = decode(
            &mut codec,
            json!({ "id": 5, "method": "item/fileChange/requestApproval",
                    "params": { "threadId": "t9", "itemId": "p1" } }),
        );
        let [AgentEvent::ApprovalRequested(request)] = other.events.as_slice() else {
            panic!()
        };
        assert!(request.from_sub_agent);
        assert_eq!(request.tool_name, PATCH_TOOL);
        let resolved = decode(
            &mut codec,
            json!({ "method": "serverRequest/resolved", "params": { "threadId": "t9", "requestId": 5 } }),
        );
        assert_eq!(
            resolved.events,
            [AgentEvent::ApprovalWithdrawn {
                request_id: "codex-5".into()
            }]
        );
    }

    #[test]
    fn other_threads_do_not_shape_the_conversation() {
        let mut codec = with_turn();
        let finished = decode(
            &mut codec,
            json!({ "method": "turn/completed", "params": { "threadId": "sub", "turn": { "status": "completed" } } }),
        );
        assert!(finished.events.is_empty());
        let own = decode(
            &mut codec,
            json!({ "method": "turn/completed", "params": { "threadId": "t1", "turn": { "status": "interrupted", "durationMs": 1500 } } }),
        );
        let [AgentEvent::TurnFinished(outcome)] = own.events.as_slice() else {
            panic!()
        };
        assert!(outcome.success && outcome.interrupted);
        assert_eq!(outcome.duration, Duration::from_millis(1500));
        assert_eq!(outcome.session_id.as_deref(), Some("t1"));
    }

    #[test]
    fn deltas_stream_into_blocks_the_complete_item_confirms() {
        let mut codec = with_turn();
        let delta = |codec: &mut CodexCodec, method: &str, extra: Value| {
            let mut params = json!({ "threadId": "t1", "itemId": "r1", "delta": "x" });
            params
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            decode(codec, json!({ "method": method, "params": params })).events
        };
        let first = delta(
            &mut codec,
            "item/reasoning/summaryTextDelta",
            json!({ "summaryIndex": 0 }),
        );
        assert_eq!(first.len(), 3);
        assert!(matches!(
            &first[1],
            AgentEvent::Stream {
                delta: StreamDelta::BlockStarted {
                    block: BlockStart::Thinking,
                    ..
                },
                ..
            }
        ));
        let next = delta(
            &mut codec,
            "item/reasoning/summaryTextDelta",
            json!({ "summaryIndex": 1 }),
        );
        assert_eq!(
            next,
            [AgentEvent::Stream {
                thread: Thread::Main,
                delta: StreamDelta::Thinking {
                    index: 0,
                    text: "\n\nx".into()
                },
            }]
        );
        let done = decode(
            &mut codec,
            json!({ "method": "item/completed", "params": { "threadId": "t1",
                    "item": { "type": "reasoning", "id": "r1", "summary": ["a", "b"], "content": [] } } }),
        );
        assert!(
            matches!(&done.events[0], AgentEvent::Message { message, .. }
            if message.blocks == [ContentBlock::Thinking("a\n\nb".into())])
        );
        assert!(matches!(
            done.events[1],
            AgentEvent::Stream {
                delta: StreamDelta::BlockStopped { index: 0 },
                ..
            }
        ));
    }

    #[test]
    fn a_plan_reads_as_a_settled_todo_list() {
        let mut codec = with_turn();
        let updated = decode(
            &mut codec,
            json!({ "method": "turn/plan/updated", "params": { "threadId": "t1", "turnId": "u1",
                    "plan": [
                        { "step": "Read", "status": "completed" },
                        { "step": "Edit", "status": "inProgress" },
                        { "step": "Test", "status": "pending" },
                        { "status": "pending" },
                    ] } }),
        );
        assert_eq!(
            updated.events,
            [
                tool_use(
                    "plan-u1",
                    "TodoWrite",
                    json!({ "todos": [
                        { "content": "Read", "status": "completed", "activeForm": "Read" },
                        { "content": "Edit", "status": "in_progress", "activeForm": "Edit" },
                        { "content": "Test", "status": "pending", "activeForm": "Test" },
                    ] })
                ),
                tool_result("plan-u1", String::new(), false),
            ]
        );
    }

    #[test]
    fn retries_and_warnings_are_reported() {
        let mut codec = with_turn();
        let retry = decode(
            &mut codec,
            json!({ "method": "error", "params": { "threadId": "t1", "willRetry": true,
                    "error": { "message": "Reconnecting" } } }),
        );
        let [AgentEvent::Retrying(retry)] = retry.events.as_slice() else {
            panic!()
        };
        assert_eq!((retry.attempt, retry.max_attempts), (1, 0));
        let fatal = decode(
            &mut codec,
            json!({ "method": "error", "params": { "threadId": "t1", "willRetry": false, "error": {} } }),
        );
        assert!(fatal.events.is_empty());
        let warning = decode(
            &mut codec,
            json!({ "method": "configWarning", "params": { "summary": "Unknown key" } }),
        );
        assert_eq!(warning.events, [AgentEvent::Warning("Unknown key".into())]);
        assert_eq!(
            decode(&mut codec, json!("not an object")).events.len(),
            1,
            "unrecognized output is reported for the log"
        );
    }

    #[test]
    fn a_note_with_a_refusal_reaches_the_running_turn() {
        let mut codec = with_turn();
        decode(
            &mut codec,
            json!({ "id": 3, "method": "item/fileChange/requestApproval",
                    "params": { "threadId": "t1", "itemId": "p1" } }),
        );
        let exchange = codec.encode(&AgentCommand::Answer {
            request_id: "codex-3".into(),
            tool_use_id: Some("p1".into()),
            decision: ApprovalDecision::Deny {
                message: "use a constant".into(),
                interrupt: false,
            },
        });
        assert_eq!(parse(&exchange.lines[0])["result"]["decision"], "decline");
        let steer = parse(&exchange.lines[1]);
        assert_eq!(steer["method"], "turn/steer");
        assert_eq!(steer["params"]["expectedTurnId"], "u1");
        assert_eq!(steer["params"]["input"][0]["text"], "use a constant");
    }

    #[test]
    fn the_quick_thread_list_is_followed_by_the_full_one() {
        let mut listing = codec();
        listing.start();
        let exchange = decode(&mut listing, json!({ "id": 1, "result": {} }));
        let quick = parse(&exchange.lines[4]);
        let listed = json!({ "data": [{ "id": "t", "name": "Quick", "updatedAt": 1 }] });
        let answered = decode(&mut listing, json!({ "id": quick["id"], "result": listed }));
        assert!(matches!(
            &answered.events[..],
            [AgentEvent::EarlierConversations(earlier)] if earlier[0].title == "Quick"
        ));
        let full = parse(&answered.lines[0]);
        assert_eq!(full["method"], "thread/list");
        assert_eq!(full["params"]["useStateDbOnly"], false);
        let listed = json!({ "data": [] });
        let answered = decode(&mut listing, json!({ "id": full["id"], "result": listed }));
        assert_eq!(
            answered.events,
            [AgentEvent::EarlierConversations(Vec::new())]
        );
        assert!(answered.lines.is_empty(), "the full list is the last");

        // A Codex that refuses the quick list still gets asked for the full one.
        let mut listing = codec();
        listing.start();
        let exchange = decode(&mut listing, json!({ "id": 1, "result": {} }));
        let quick = parse(&exchange.lines[4]);
        let refused = decode(
            &mut listing,
            json!({ "id": quick["id"], "error": { "code": -32602, "message": "unknown field" } }),
        );
        assert!(refused.events.is_empty());
        assert_eq!(parse(&refused.lines[0])["params"]["useStateDbOnly"], false);
    }

    #[test]
    fn earlier_threads_are_titled_by_name_else_their_first_line() {
        let long = "x".repeat(200);
        let listed = json!([
            { "id": "a", "name": "Named", "preview": "ignored", "updatedAt": 60 },
            { "id": "b", "name": " ", "preview": "\n  First line \nsecond", "updatedAt": 30 },
            { "id": "c", "name": null, "preview": "" },
            { "id": "d", "preview": long },
            { "name": "No id" },
        ]);
        let earlier = earlier(Some(&listed));
        let titles: Vec<&str> = earlier.iter().map(|past| past.title.as_str()).collect();
        assert_eq!(titles[..2], ["Named", "First line"]);
        assert_eq!(
            earlier.len(),
            3,
            "untitled and unidentified threads are left out"
        );
        assert_eq!(earlier[2].title.chars().count(), EARLIER_TITLE_CHARS);
        assert_eq!(
            earlier[0].modified,
            std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(60)
        );
    }

    #[test]
    fn commands_read_as_typed() {
        assert_eq!(display_command("/bin/bash -lc 'cargo test'"), "cargo test");
        assert_eq!(display_command("bash -lc 'echo '\\''hi'\\'''"), "echo 'hi'");
        assert_eq!(display_command("zsh -lc \"ls\""), "ls");
        // Anything the quoting rules would change stays as Codex wrote it.
        assert_eq!(
            display_command("bash -lc \"echo $HOME\""),
            "bash -lc \"echo $HOME\""
        );
        assert_eq!(display_command("bash -lc 'a' 'b'"), "bash -lc 'a' 'b'");
        assert_eq!(display_command("python3 -lc 'x'"), "python3 -lc 'x'");
        assert_eq!(display_command("ls"), "ls");
    }
}
