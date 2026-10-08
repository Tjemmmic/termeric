//! The running supervisor in one window: starting it, telling it what other
//! agents need, deciding each message it sends, and letting the person take
//! over. The policy itself lives in `crate::agents::supervisor`.
//!
//! The supervisor is a conversation in the agent view, in a tab of its own:
//! Termeric tells it things as messages of that conversation, and a message
//! it wants to send in Assist mode waits there as a card for the person.

use super::*;
use crate::agent_view::session::{AgentProgram, AgentSession, SessionId, StartOptions};
use crate::agents::supervisor::{self, SendLimits, SupervisorMode};
use crate::agents::{AgentActivity, AgentStatus, AgentTarget};
use crate::tabs::TerminalIdentity;
use anyhow::Context as _;
use std::collections::VecDeque;

const MAX_QUEUED_MESSAGES: usize = 32;
const MAX_LOG_ENTRIES: usize = 200;
/// Rules are given to the agent on its command line, and read by it on every
/// turn; more than this is not a rule list.
const MAX_RULES_BYTES: usize = 16 * 1024;

/// What became of a message held for the person's approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalOutcome {
    Send,
    Decline,
    /// The session it was for ended before the person answered.
    Unanswered,
}

/// Where a supervisor message goes.
enum SendTarget {
    /// An agent in a terminal, typed into as if by the person.
    Terminal(AgentTarget),
    /// An agent view session, as a new turn of its conversation.
    Session {
        id: crate::agent_view::session::SessionId,
        name: String,
    },
}

impl SendTarget {
    fn name(&self) -> &str {
        match self {
            SendTarget::Terminal(target) => &target.agent_name,
            SendTarget::Session { name, .. } => name,
        }
    }

    /// The key its sends are rate-limited under.
    fn key(&self) -> String {
        match self {
            SendTarget::Terminal(target) => terminal_send_key(target.identity),
            SendTarget::Session { id, .. } => format!("session:{}", id.control_id()),
        }
    }
}

/// The key sends to the agent in terminal `identity` are rate-limited under.
fn terminal_send_key(identity: TerminalIdentity) -> String {
    format!("{}:{}", identity.tab_id, identity.pane_id.0)
}

/// Why a request that names the supervisor's token was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NotTheSupervisor {
    /// The token is not this window's running supervisor's.
    UnknownToken,
    /// The request came from outside the supervisor's own agent process.
    OtherProcess,
}

impl std::fmt::Display for NotTheSupervisor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnknownToken => {
                "This supervisor does not oversee that window, or it has been stopped"
            }
            Self::OtherProcess => "Only the supervisor Termeric started may act as the supervisor",
        })
    }
}

impl From<NotTheSupervisor> for String {
    fn from(refusal: NotTheSupervisor) -> Self {
        refusal.to_string()
    }
}

/// How the supervisor may carry out an action its mode and limits allow.
enum Permitted {
    /// Put it to the person first.
    Assist,
    /// Carry it out on its own.
    Auto,
}

/// A message the supervisor asked to send, waiting for the person.
struct PendingApproval {
    target: SendTarget,
    text: String,
}

pub(super) struct Supervisor {
    /// Proves a control request comes from this supervisor's MCP server.
    token: String,
    /// The supervisor's conversation in the agent view.
    session: SessionId,
    mode: SupervisorMode,
    /// The mode to return to after the person takes over.
    held_mode: Option<SupervisorMode>,
    limits: SendLimits,
    /// Messages waiting until the supervisor finishes what it is doing.
    queue: VecDeque<String>,
    log: VecDeque<(Instant, String)>,
    approval: Option<PendingApproval>,
}

impl Supervisor {
    pub(super) fn mode(&self) -> SupervisorMode {
        self.mode
    }

    fn note(&mut self, entry: String) {
        if self.log.len() == MAX_LOG_ENTRIES {
            self.log.pop_front();
        }
        self.log.push_back((Instant::now(), entry));
    }
}

impl App {
    pub(super) fn supervisor_running(&self) -> bool {
        self.supervisor.is_some()
    }

    #[cfg(test)]
    pub(super) fn supervisor_token(&self) -> Option<&str> {
        self.supervisor
            .as_ref()
            .map(|supervisor| supervisor.token.as_str())
    }

    pub(super) fn supervisor_held(&self) -> bool {
        self.supervisor
            .as_ref()
            .is_some_and(|supervisor| supervisor.held_mode.is_some())
    }

    /// The supervisor's conversation, while it is open.
    pub(super) fn supervisor_session(&self) -> Option<SessionId> {
        let id = self.supervisor.as_ref()?.session;
        self.agent_view.session(id).map(AgentSession::id)
    }

    /// Whether `id` is the supervisor's own conversation, which it neither
    /// oversees nor sends to.
    pub(super) fn is_supervisor_session(&self, id: SessionId) -> bool {
        self.supervisor
            .as_ref()
            .is_some_and(|supervisor| supervisor.session == id)
    }

    /// The rules file and what it says, written with the default rules
    /// first if it does not exist yet.
    fn ensure_rules_file(&self) -> anyhow::Result<(&std::path::Path, String)> {
        let path = self
            .supervisor_rules_path
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!(crate::paths::NO_HOME_DIRECTORY))?;
        match crate::safe_fs::read_linked_text(path, MAX_RULES_BYTES) {
            Ok(rules) => Ok((path, rules)),
            // Only when nothing is there: replacing a link whose file is gone
            // would quietly disconnect it from its real location.
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && std::fs::symlink_metadata(path).is_err() =>
            {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                crate::safe_fs::atomic_replace(path, |writer| {
                    writer.write_all(supervisor::DEFAULT_RULES.as_bytes())
                })
                .with_context(|| format!("write {}", path.display()))?;
                Ok((path, supervisor::DEFAULT_RULES.to_string()))
            }
            Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
        }
    }

    /// Start the configured agent as this window's supervisor, in a tab of
    /// its own filled by its conversation, or show the one already running.
    pub(super) fn start_supervisor(&mut self) {
        if let Some(id) = self.supervisor_session() {
            self.show_agent_session(id);
            return;
        }
        if let Err(error) = self.launch_supervisor() {
            self.toasts.error(error);
            return;
        }
        // Without status hooks the supervisor is never told that an agent
        // in a terminal needs input, finished, or failed.
        if matches!(
            self.agent_hook_summary(),
            super::agent_hooks::HookSetupSummary::NotSetUp
                | super::agent_hooks::HookSetupSummary::Incomplete
        ) {
            self.open_agent_hook_setup_because(
                true,
                Some("The supervisor hears when an agent in a terminal needs you, finishes, or fails only through these hooks."),
            );
        }
    }

    fn launch_supervisor(&mut self) -> Result<(), String> {
        let config = self.config.supervisor.clone();
        // The supervisor is an agent view conversation, so it is an agent
        // the agent view drives.
        let Some(kind) = crate::agent_view::agent::AgentKind::from_catalog_id(&config.agent) else {
            return Err(format!(
                "The agent view cannot run {} as the supervisor",
                config.agent
            ));
        };
        let agent = self
            .agents
            .get(&config.agent)
            .filter(|agent| agent.enabled)
            .cloned()
            .ok_or_else(|| format!("The supervisor runs {}, which is turned off", kind.name()))?;
        let (_, rules) = self
            .ensure_rules_file()
            .map_err(|error| format!("Could not read the supervisor rules: {error:#}"))?;
        // 32 lowercase hex digits.
        let token = termeric_core::hex::random(16)
            .map_err(|error| format!("Could not create a supervisor token: {error}"))?;
        let executable = super::agent_hooks::termeric_executable()?;
        let runtime_directory = std::env::var("XDG_RUNTIME_DIR").ok();
        let server = supervisor::SupervisorServer {
            termeric: &executable,
            token: &token,
            process_id: std::process::id(),
            runtime_directory: runtime_directory.as_deref(),
        };
        let extra = supervisor::launch_args(
            &agent.id,
            &server,
            &supervisor::instructions(config.mode, &rules),
        )
        .ok_or_else(|| format!("{} cannot be a supervisor", agent.name))?;
        let base = self.agent_view_program(kind);
        let program = AgentProgram {
            program: base.program,
            args: base.args.into_iter().chain(extra).collect(),
            env: base.env,
        };
        let (cwd, _) = self.agent_view_default_cwd();
        let options = StartOptions {
            cwd: cwd.clone(),
            agent: kind,
            mode: kind.default_mode().into(),
            model: None,
            effort: None,
            auto_approve: false,
            resume: None,
            allow_bypass: false,
        };
        // Started before its tab opens, so a failure leaves nothing behind.
        let mut session = AgentSession::start(program, options, super::agent_view_event_waker())
            .map_err(|error| error.to_string())?;
        session.set_title("Supervisor");
        let id = session.id();
        session
            .prompt(supervisor::KICKOFF, Vec::new())
            .map_err(|error| error.to_string())?;
        if !self.open_agent_view_tab(&cwd, crate::ui::agent_view::Layout::Full) {
            return Err("Could not open a tab for the supervisor".into());
        }
        self.open_agent_view(true);
        self.agent_view.sessions.push(session);
        self.agent_view.state.show(id);
        let mut started = Supervisor {
            token,
            session: id,
            mode: config.mode,
            held_mode: None,
            limits: SendLimits::default(),
            queue: VecDeque::new(),
            log: VecDeque::new(),
            approval: None,
        };
        started.note(format!(
            "Started {} as the supervisor in {} mode",
            agent.name,
            config.mode.label().to_lowercase()
        ));
        self.supervisor = Some(started);
        self.request_redraw(RedrawReason::Action);
        Ok(())
    }

    /// Forget the supervisor, and any message it left waiting.
    fn end_supervision(&mut self) {
        self.supervisor = None;
        self.request_redraw(RedrawReason::Action);
    }

    /// Tell the supervisor that text meant for another agent was dropped.
    pub(super) fn report_undelivered(&mut self, target: &AgentTarget, reason: &str) {
        if self.supervisor.is_none() {
            return;
        }
        let name = &target.agent_name;
        if let Some(supervisor) = &mut self.supervisor {
            supervisor.note(format!("Not delivered to {name}: {reason}"));
        }
        self.tell_supervisor(format!(
            "Termeric: a message to {name} was not delivered: {reason}"
        ));
    }

    /// End supervision, its conversation, and the tab it had.
    pub(super) fn stop_supervisor(&mut self) {
        let session = self.supervisor_session();
        let tab = session.and_then(|id| self.agent_session_tab(id));
        self.end_supervision();
        if let Some(id) = session {
            self.end_agent_session(id);
        }
        if let Some(tab) = tab
            && let Err(error) = self.close_tab_for(Requester::Plugin, tab as u64)
        {
            self.toasts
                .warning(format!("The supervisor's tab stayed open: {error}"));
        }
        self.toasts.info("Supervisor stopped");
    }

    /// Forget a supervisor whose conversation ended, as when its tab closed.
    /// Returns whether it did.
    pub(super) fn retire_closed_supervisor(&mut self) -> bool {
        if self.supervisor.is_none() || self.supervisor_session().is_some() {
            return false;
        }
        self.end_supervision();
        self.toasts.info("The supervisor's tab closed");
        true
    }

    /// Stop the supervisor from sending until the person hands back.
    pub(super) fn take_over_from_supervisor(&mut self) {
        let Some(supervisor) = &mut self.supervisor else {
            return;
        };
        if supervisor.held_mode.is_none() {
            supervisor.held_mode = Some(supervisor.mode);
            supervisor.mode = SupervisorMode::Watch;
            supervisor.note("The person took over; watching only".into());
        }
        self.tell_supervisor(
            "Termeric: the person has taken over. You are in watch mode until they hand back; do not send to agents."
                .into(),
        );
        self.toasts
            .info("You have taken over; the supervisor only watches");
    }

    pub(super) fn hand_back_to_supervisor(&mut self) {
        let Some(supervisor) = &mut self.supervisor else {
            return;
        };
        let Some(mode) = supervisor.held_mode.take() else {
            return;
        };
        supervisor.mode = mode;
        supervisor.note(format!(
            "Handed back in {} mode",
            mode.label().to_lowercase()
        ));
        self.tell_supervisor(format!(
            "Termeric: the person handed back control. You are in {} mode again.",
            mode.label().to_lowercase()
        ));
        self.toasts.info("The supervisor is back in charge");
    }

    /// Change the supervisor's mode, for now and for later supervisors.
    pub(super) fn set_supervisor_mode(&mut self, mode: SupervisorMode) {
        let mut config = self.config.clone();
        config.supervisor.mode = mode;
        if let Err(error) = self.apply_config(config) {
            self.toasts.error(format!("{error:#}"));
            return;
        }
        if let Some(supervisor) = &mut self.supervisor {
            supervisor.held_mode = None;
            supervisor.mode = mode;
            supervisor.note(format!("Mode set to {}", mode.label().to_lowercase()));
            self.tell_supervisor(format!(
                "Termeric: your mode changed. {}",
                mode.instructions()
            ));
        }
        self.toasts
            .info(format!("Supervisor mode: {}", mode.label()));
    }

    /// Show the mode picker.
    pub(super) fn open_supervisor_mode_picker(&mut self) {
        let current = self
            .supervisor
            .as_ref()
            .map_or(self.config.supervisor.mode, |supervisor| {
                supervisor.held_mode.unwrap_or(supervisor.mode)
            });
        let items = SupervisorMode::ALL
            .into_iter()
            .map(
                |mode| crate::ui::command_palette::palette::RegisteredCommand {
                    id: mode.label().to_lowercase(),
                    title: mode.label().into(),
                    category: mode.description().into(),
                    shortcut: None,
                    source: crate::ui::command_palette::palette::CommandSource::Core,
                    destructive: false,
                },
            )
            .collect();
        self.command_palette.begin_pick(
            crate::ui::command_palette::palette::PickKind::SupervisorMode,
            items,
            "Supervisor mode…",
            "mode",
            Some(&current.label().to_lowercase()),
        );
    }

    pub(super) fn pick_supervisor_mode(&mut self, id: &str) {
        if let Some(mode) = SupervisorMode::ALL
            .into_iter()
            .find(|mode| mode.label().eq_ignore_ascii_case(id))
        {
            self.set_supervisor_mode(mode);
        }
    }

    /// Open the rules file in the person's editor, in a new tab.
    pub(super) fn edit_supervisor_rules(&mut self) {
        let path = match self.ensure_rules_file() {
            Ok((path, _)) => path.display().to_string(),
            Err(error) => {
                self.toasts
                    .error(format!("Could not create the supervisor rules: {error:#}"));
                return;
            }
        };
        let opened = self.tabs.as_mut().map(|tabs| {
            tabs.new_tab_with_command(
                "/bin/sh",
                &[
                    "-c".into(),
                    "exec ${VISUAL:-${EDITOR:-vi}} \"$1\"".into(),
                    "termeric-edit".into(),
                    path,
                ],
                None,
                Some("Supervisor rules"),
                None,
                std::collections::HashMap::new(),
            )
        });
        match opened {
            Some(Ok(_)) => self.refresh_focused_terminal_surfaces(),
            Some(Err(error)) => self
                .toasts
                .error(format!("Could not open an editor: {error:#}")),
            None => {}
        }
    }

    /// The supervisor as the Agents panel shows it: its mode and what it
    /// has seen and done, newest first.
    pub(super) fn supervisor_summary(&self) -> Option<crate::ui::agents_panel::SupervisorSummary> {
        let supervisor = self.supervisor.as_ref()?;
        Some(crate::ui::agents_panel::SupervisorSummary {
            mode: match supervisor.held_mode {
                Some(_) => "Watch, you took over".into(),
                None => supervisor.mode.label().into(),
            },
            held: supervisor.held_mode.is_some(),
            entries: supervisor
                .log
                .iter()
                .rev()
                .map(|(when, entry)| {
                    format!("{} · {entry}", crate::ui::age::compact_age(when.elapsed()))
                })
                .collect(),
        })
    }

    /// Tell the supervisor something now, or once it has finished what it is
    /// doing.
    fn tell_supervisor(&mut self, message: String) {
        let Some(supervisor) = &mut self.supervisor else {
            return;
        };
        if supervisor.queue.len() == MAX_QUEUED_MESSAGES {
            supervisor.queue.pop_front();
        }
        supervisor.queue.push_back(message);
        self.deliver_supervisor_messages();
    }

    /// Deliver every waiting message as one message of its conversation,
    /// once it has finished its turn and while the person is not writing to
    /// it. Returns whether it delivered.
    pub(super) fn deliver_supervisor_messages(&mut self) -> bool {
        let Some(id) = self.supervisor_session() else {
            return false;
        };
        if self
            .supervisor
            .as_ref()
            .is_none_or(|supervisor| supervisor.queue.is_empty())
        {
            return false;
        }
        let busy = self.agent_view.session(id).is_none_or(|session| {
            session.transcript().turn_active() || session.queued().next().is_some()
        });
        if busy || self.person_writing_to(id) {
            return false;
        }
        let Some(supervisor) = &mut self.supervisor else {
            return false;
        };
        let message = supervisor.queue.drain(..).collect::<Vec<_>>().join("\n");
        let sent = self
            .agent_view
            .session_mut(id)
            .map(|session| session.prompt(&message, Vec::new()));
        if let Some(Err(error)) = sent
            && let Some(supervisor) = &mut self.supervisor
        {
            supervisor.note(format!("Could not reach the supervisor: {error}"));
        }
        self.request_redraw(RedrawReason::Action);
        true
    }

    /// React to an agent's changed status: tell the supervisor about other
    /// agents that need attention, and deliver waiting messages once the
    /// supervisor itself is free.
    pub(super) fn supervise_status_change(
        &mut self,
        identity: TerminalIdentity,
        before: Option<AgentActivity>,
        status: Option<&AgentStatus>,
    ) {
        if self.supervisor.is_none() {
            return;
        }
        let Some(status) = status.filter(|status| before != Some(status.activity)) else {
            return;
        };
        let name = self.agent_display_name(&status.agent_id);
        let where_ = format!(
            "window {}, tab {}, pane {}",
            self.control_window_id, identity.tab_id, identity.pane_id.0
        );
        if let Some(message) = supervisor::event_message(&name, &where_, status) {
            if let Some(supervisor) = &mut self.supervisor {
                supervisor.note(message.trim_start_matches("Termeric: ").to_owned());
            }
            self.tell_supervisor(message);
        }
    }

    /// Whether `token` belongs to this window's supervisor.
    pub(super) fn owns_supervisor_token(&self, token: &str) -> bool {
        self.supervisor
            .as_ref()
            .is_some_and(|supervisor| supervisor.token == token)
    }

    /// Whether `process` runs under the supervisor's own agent process. The
    /// token is on the supervisor's command line, where any program of the
    /// same person can read it, so it alone does not show that a request
    /// comes from the supervisor rather than, say, an agent it oversees.
    fn is_supervisor_process(&self, process: Option<u32>) -> bool {
        let Some(process) = process else {
            return false;
        };
        self.supervisor_session()
            .and_then(|id| self.agent_view.session(id)?.process_id())
            .is_some_and(|agent| crate::agents::detect::descends_from(process, agent))
    }

    /// Decide a message the supervisor asked to send. Returns whether it is
    /// waiting for the person's approval.
    pub(super) fn supervised_submit(
        &mut self,
        token: &str,
        peer_process: Option<u32>,
        tab_id: u64,
        pane_id: u32,
        agent_id: &str,
        text: &str,
    ) -> Result<bool, String> {
        self.check_supervisor_request(token, peer_process)?;
        let identity = TerminalIdentity {
            tab_id: usize::try_from(tab_id)
                .map_err(|_| "Target tab ID does not fit this platform".to_string())?,
            pane_id: crate::panes::PaneId(pane_id),
        };
        let agent_name = self.agent_display_name(agent_id);
        let target = AgentTarget {
            identity,
            agent_id: agent_id.to_string(),
            agent_name: agent_name.clone(),
        };
        // The target must still run that agent, without the person typing to
        // it, before anything is decided.
        self.recheck_agent_target(&target)?;
        self.refuse_while_person_types(&target)?;
        let key = terminal_send_key(identity);
        match self.permit_supervisor_action(&key, text, &format!("a message to {agent_name}"))? {
            Permitted::Auto => {
                self.paste_and_submit_for_supervisor(target, text)?;
                self.record_supervisor_action(
                    &key,
                    text,
                    format!("Sent to {agent_name}: {}", one_line(text)),
                );
                Ok(false)
            }
            Permitted::Assist => self.hold_supervisor_message(SendTarget::Terminal(target), text),
        }
    }

    /// Whether the supervisor may now do `action` to the agent whose sends
    /// are limited under `key`: not in Watch mode, and within its limits. A
    /// refusal is logged as refusing `what`, and returned.
    fn permit_supervisor_action(
        &mut self,
        key: &str,
        action: &str,
        what: &str,
    ) -> Result<Permitted, String> {
        let per_minute = self.config.supervisor.max_sends_per_minute;
        let supervisor = self
            .supervisor
            .as_mut()
            .ok_or(NotTheSupervisor::UnknownToken)?;
        let mode = supervisor.mode;
        if let Err(refusal) = supervisor
            .limits
            .check(mode, per_minute, key, action, Instant::now())
        {
            supervisor.note(format!("Refused {what}: {refusal}"));
            return Err(refusal.to_string());
        }
        Ok(match mode {
            SupervisorMode::Watch => unreachable!("the limits refuse watch mode"),
            SupervisorMode::Assist => Permitted::Assist,
            SupervisorMode::Auto => Permitted::Auto,
        })
    }

    /// Count an action the supervisor carried out toward its limits, and log
    /// it as `entry`.
    fn record_supervisor_action(&mut self, key: &str, action: &str, entry: String) {
        if let Some(supervisor) = &mut self.supervisor {
            supervisor.limits.record(key, action, Instant::now());
            supervisor.note(entry);
        }
    }

    /// Holds a message for the person, as a card in the supervisor's
    /// conversation. Returns that it waits for them.
    fn hold_supervisor_message(&mut self, target: SendTarget, text: &str) -> Result<bool, String> {
        let supervisor = self.supervisor.as_mut().expect("the token was checked");
        if supervisor.approval.is_some() {
            return Err(
                "Another message is waiting for the person's approval; wait for its outcome".into(),
            );
        }
        let name = target.name().to_owned();
        supervisor.note(format!("Asked you to approve a message to {name}"));
        supervisor.approval = Some(PendingApproval {
            target,
            text: text.to_owned(),
        });
        let session = supervisor.session;
        if !self.agent_conversation_visible(session) {
            self.notify_agent_view(
                "The supervisor needs you",
                &format!("The supervisor wants to message {name}"),
            );
        }
        self.request_redraw(RedrawReason::Action);
        Ok(true)
    }

    /// The message waiting for the person, as the supervisor's conversation
    /// shows it.
    pub(super) fn supervisor_request(
        &self,
    ) -> Option<crate::ui::agent_view::layout::SupervisorRequest> {
        let supervisor = self.supervisor.as_ref()?;
        let approval = supervisor.approval.as_ref()?;
        Some(crate::ui::agent_view::layout::SupervisorRequest {
            session: supervisor.session,
            recipient: approval.target.name().to_owned(),
            text: approval.text.clone(),
        })
    }

    /// The person's answer to the waiting message.
    pub(super) fn answer_supervisor_request(&mut self, send: bool) {
        self.resolve_supervisor_approval(if send {
            ApprovalOutcome::Send
        } else {
            ApprovalOutcome::Decline
        });
        self.request_redraw(RedrawReason::Action);
    }

    /// Settle a waiting message whose recipient ended, which can no longer
    /// be sent. Returns whether one was settled.
    pub(super) fn settle_orphaned_supervisor_approval(&mut self) -> bool {
        let target = match self
            .supervisor
            .as_ref()
            .and_then(|supervisor| supervisor.approval.as_ref())
            .map(|approval| &approval.target)
        {
            Some(SendTarget::Session { id, .. }) => Err(*id),
            Some(SendTarget::Terminal(target)) => Ok(target.clone()),
            None => return false,
        };
        let orphaned = match target {
            Err(id) => self.agent_view.session(id).is_none(),
            Ok(target) => self.recheck_agent_target(&target).is_err(),
        };
        if orphaned {
            self.resolve_supervisor_approval(ApprovalOutcome::Unanswered);
        }
        orphaned
    }

    fn resolve_supervisor_approval(&mut self, outcome: ApprovalOutcome) {
        let Some(approval) = self
            .supervisor
            .as_mut()
            .and_then(|supervisor| supervisor.approval.take())
        else {
            return;
        };
        let name = approval.target.name().to_owned();
        let key = approval.target.key();
        // Declined and unanswered messages count toward the limits too, so a
        // supervisor cannot ask faster than it could send.
        if let Some(supervisor) = &mut self.supervisor {
            supervisor
                .limits
                .record(&key, &approval.text, Instant::now());
        }
        let (entry, outcome) = match outcome {
            ApprovalOutcome::Send => {
                let sent = match approval.target {
                    SendTarget::Terminal(target) => {
                        self.paste_and_submit_for_supervisor(target, &approval.text)
                    }
                    SendTarget::Session { id, .. } => {
                        self.prompt_agent_view_session(id, &approval.text)
                    }
                };
                match sent {
                    Ok(()) => (
                        format!("You approved; sent to {name}: {}", one_line(&approval.text)),
                        format!(
                            "Termeric: the person approved your message to {name}; it was sent."
                        ),
                    ),
                    Err(error) => (
                        format!("Approved, but not sent to {name}: {error}"),
                        format!(
                            "Termeric: the person approved your message to {name}, but it could not be sent: {error}"
                        ),
                    ),
                }
            }
            ApprovalOutcome::Decline => (
                format!("You declined a message to {name}"),
                format!("Termeric: the person declined your message to {name}; it was not sent."),
            ),
            ApprovalOutcome::Unanswered => (
                format!("A message to {name} was dropped: {name} ended"),
                format!(
                    "Termeric: your message to {name} was not sent; {name} ended before the person answered."
                ),
            ),
        };
        if let Some(supervisor) = &mut self.supervisor {
            supervisor.note(entry);
        }
        self.tell_supervisor(outcome);
    }
}

impl App {
    /// Checks that a request comes from this window's supervisor.
    pub(super) fn check_supervisor_request(
        &self,
        token: &str,
        peer_process: Option<u32>,
    ) -> Result<(), NotTheSupervisor> {
        if !self.owns_supervisor_token(token) {
            return Err(NotTheSupervisor::UnknownToken);
        }
        if !self.is_supervisor_process(peer_process) {
            return Err(NotTheSupervisor::OtherProcess);
        }
        Ok(())
    }

    /// The supervisor answers an agent view approval: in Auto mode it
    /// decides, in Assist mode its answer is shown to the person as a
    /// suggestion, and in Watch mode it is refused. Returns whether it
    /// decided.
    pub(super) fn supervised_session_answer(
        &mut self,
        token: &str,
        peer_process: Option<u32>,
        id: crate::agent_view::session::SessionId,
        approval_id: &str,
        allow: bool,
        reason: &str,
    ) -> Result<bool, String> {
        use crate::agent_view::transcript::DecidedBy;
        self.check_supervisor_request(token, peer_process)?;
        if self.is_supervisor_session(id) {
            return Err("The supervisor cannot answer its own requests".into());
        }
        let mode = self
            .supervisor
            .as_ref()
            .expect("the token was checked")
            .mode;
        let reason = reason.trim();
        let session = self
            .agent_view
            .session_mut(id)
            .ok_or_else(|| "That agent view session has closed".to_string())?;
        let title = session.title();
        let verb = if allow { "allow" } else { "deny" };
        if mode == SupervisorMode::Watch {
            return Err(
                "The supervisor only watches; answer approvals in Assist or Auto mode".into(),
            );
        }
        // Answers count toward the same limits as messages, so a supervisor
        // cannot act faster than it may send.
        let key = SendTarget::Session {
            id,
            name: format!("\"{title}\""),
        }
        .key();
        let action = format!("{verb} approval {approval_id}");
        let permitted = self.permit_supervisor_action(
            &key,
            &action,
            &format!("to {verb} a request in \"{title}\""),
        )?;
        let session = self
            .agent_view
            .session_mut(id)
            .ok_or_else(|| "That agent view session has closed".to_string())?;
        let decided = match permitted {
            Permitted::Assist => {
                if !session.suggest(approval_id, allow, reason) {
                    return Err(format!("Approval {approval_id} is no longer pending"));
                }
                false
            }
            Permitted::Auto => {
                let note = (!reason.is_empty()).then(|| reason.to_owned());
                let answered = if allow {
                    session.answer(
                        approval_id,
                        crate::agent_view::event::ApprovalDecision::Allow {
                            input: session
                                .transcript()
                                .approval(approval_id)
                                .map(|approval| approval.input.clone())
                                .unwrap_or_default(),
                            remember: None,
                        },
                        DecidedBy::Supervisor,
                        note,
                    )
                } else {
                    session.answer(
                        approval_id,
                        crate::agent_view::event::ApprovalDecision::Deny {
                            message: if reason.is_empty() {
                                "The supervisor declined this action.".to_owned()
                            } else {
                                reason.to_owned()
                            },
                            interrupt: false,
                        },
                        DecidedBy::Supervisor,
                        note,
                    )
                };
                if !answered {
                    return Err(format!("Approval {approval_id} is no longer pending"));
                }
                true
            }
        };
        self.record_supervisor_action(
            &key,
            &action,
            if decided {
                format!("Chose to {verb} a request in \"{title}\"")
            } else {
                format!("Suggested you {verb} a request in \"{title}\"")
            },
        );
        self.request_redraw(RedrawReason::Action);
        Ok(decided)
    }

    /// The supervisor sends a message to an agent view session, within its
    /// mode and limits. Returns whether it waits for the person.
    pub(super) fn supervised_session_submit(
        &mut self,
        token: &str,
        peer_process: Option<u32>,
        id: crate::agent_view::session::SessionId,
        text: &str,
    ) -> Result<bool, String> {
        self.check_supervisor_request(token, peer_process)?;
        if self.is_supervisor_session(id) {
            return Err("The supervisor cannot send to itself".into());
        }
        let name = self
            .agent_view
            .session(id)
            .map(|session| format!("\"{}\"", session.title()))
            .ok_or_else(|| "That agent view session has closed".to_string())?;
        self.refuse_while_person_writes(id)?;
        let target = SendTarget::Session {
            id,
            name: name.clone(),
        };
        let key = target.key();
        match self.permit_supervisor_action(&key, text, &format!("a message to {name}"))? {
            Permitted::Auto => {
                self.prompt_agent_view_session(id, text)?;
                self.record_supervisor_action(
                    &key,
                    text,
                    format!("Sent to {name}: {}", one_line(text)),
                );
                Ok(false)
            }
            Permitted::Assist => self.hold_supervisor_message(target, text),
        }
    }

    /// Tells the supervisor, once per request, what agent view sessions
    /// need, and when their turns end.
    pub(super) fn supervise_agent_view(
        &mut self,
        events: Vec<crate::agents::supervisor::SessionEvent>,
    ) {
        if self.supervisor.is_none() || events.is_empty() {
            return;
        }
        for event in events {
            let message =
                crate::agents::supervisor::session_message(self.control_window_id, &event);
            if let Some(supervisor) = &mut self.supervisor {
                supervisor.note(message.trim_start_matches("Termeric: ").to_owned());
            }
            self.tell_supervisor(message);
        }
    }
}

/// `text` as the activity log quotes it.
fn one_line(text: &str) -> String {
    crate::safe_text::first_line_truncated(text, 80)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_are_written_privately_read_through_links_and_bounded() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config").join("supervisor.md");
        let mut app = App::new_headless(Config::default()).unwrap();
        app.supervisor_rules_path = Some(path.clone());

        let (_, rules) = app.ensure_rules_file().unwrap();
        assert_eq!(rules, supervisor::DEFAULT_RULES);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            supervisor::DEFAULT_RULES
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
            0
        );

        // A dotfile manager's link is read, not replaced.
        let real = temp.path().join("dotfiles-supervisor.md");
        std::fs::write(&real, "my rules").unwrap();
        std::fs::remove_file(&path).unwrap();
        symlink(&real, &path).unwrap();
        assert_eq!(app.ensure_rules_file().unwrap().1, "my rules");
        assert!(std::fs::symlink_metadata(&path).unwrap().is_symlink());

        // A link whose file is gone is reported, never replaced.
        std::fs::remove_file(&real).unwrap();
        assert!(app.ensure_rules_file().is_err());
        assert!(std::fs::symlink_metadata(&path).unwrap().is_symlink());

        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "x".repeat(MAX_RULES_BYTES + 1)).unwrap();
        let error = app.ensure_rules_file().unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(std::io::ErrorKind::InvalidData)
        );
    }
}
