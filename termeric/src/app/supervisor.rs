//! The running supervisor in one window: starting it, telling it what other
//! agents need, deciding each message it sends, and letting the person take
//! over. The policy itself lives in `crate::agents::supervisor`.

use super::agents::AgentTarget;
use super::*;
use crate::agents::supervisor::{self, SendLimits, SupervisorMode};
use crate::agents::{AgentActivity, AgentStatus};
use crate::tabs::TerminalIdentity;
use crate::ui::info_overlay::InfoOverlayChoice;
use std::collections::VecDeque;

const MAX_QUEUED_MESSAGES: usize = 32;
const MAX_LOG_ENTRIES: usize = 200;
const SEND_CHOICE: &str = "supervisor-send";
const DECLINE_CHOICE: &str = "supervisor-decline";
/// Rules are given to the agent on its command line, and read by it on every
/// turn; more than this is not a rule list.
const MAX_RULES_BYTES: u64 = 16 * 1024;

/// What became of a message held for the person's approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalOutcome {
    Send,
    Decline,
    /// Another dialog replaced it before the person answered.
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
            SendTarget::Terminal(target) => {
                format!("{}:{}", target.identity.tab_id, target.identity.pane_id.0)
            }
            SendTarget::Session { id, .. } => format!("session:{}", id.control_id()),
        }
    }
}

/// A message the supervisor asked to send, waiting for the person.
struct PendingApproval {
    request_id: u64,
    target: SendTarget,
    text: String,
    shown_at: Instant,
}

/// How long the approval dialog ignores **Send** after it opens. It opens on
/// its own, while the person may be typing elsewhere, and keys meant for
/// that, such as Tab then Enter, must not approve it.
const APPROVAL_ARMING: Duration = Duration::from_secs(1);

pub(super) struct Supervisor {
    /// Proves a control request comes from this supervisor's MCP server.
    token: String,
    /// The supervisor terminal's process identity, which survives moves.
    terminal_id: String,
    agent_id: String,
    agent_name: String,
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

/// A random token, as 32 lowercase hex digits.
fn new_token() -> Option<String> {
    use std::io::Read;
    let mut bytes = [0_u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .ok()?;
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
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

    /// The supervisor's terminal, while it is open.
    fn supervisor_terminal(&self) -> Option<TerminalIdentity> {
        let supervisor = self.supervisor.as_ref()?;
        self.tabs
            .as_ref()?
            .terminal_with_id(&supervisor.terminal_id)
    }

    fn supervisor_target(&self) -> Option<AgentTarget> {
        let supervisor = self.supervisor.as_ref()?;
        Some(AgentTarget {
            identity: self.supervisor_terminal()?,
            agent_id: supervisor.agent_id.clone(),
            agent_name: supervisor.agent_name.clone(),
        })
    }

    /// Whether `identity` is the supervisor's own terminal.
    pub(super) fn is_supervisor_terminal(&self, identity: TerminalIdentity) -> bool {
        self.supervisor_terminal() == Some(identity)
    }

    fn ensure_rules_file(&self) -> anyhow::Result<String> {
        let path = &self.supervisor_rules_path;
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.len() > MAX_RULES_BYTES => anyhow::bail!(
                "{} is larger than {} KiB; shorten it",
                path.display(),
                MAX_RULES_BYTES / 1024
            ),
            _ => {}
        }
        match std::fs::read_to_string(path) {
            Ok(rules) => Ok(rules),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(path, supervisor::DEFAULT_RULES)?;
                Ok(supervisor::DEFAULT_RULES.to_string())
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Start the configured agent as this window's supervisor, or show the
    /// one already running.
    pub(super) fn start_supervisor(&mut self) {
        if let Some(identity) = self.supervisor_terminal() {
            self.activate_notification_origin(identity);
            return;
        }
        let result = (|| -> Result<(), String> {
            let config = self.config.supervisor.clone();
            let agent = self
                .agents
                .get(&config.agent)
                .filter(|agent| agent.enabled && agent.installed_path().is_some())
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "The supervisor runs {}, which is not installed or is turned off",
                        config.agent
                    )
                })?;
            let rules = self
                .ensure_rules_file()
                .map_err(|error| format!("Could not read the supervisor rules: {error:#}"))?;
            let token = new_token().ok_or("Could not create a supervisor token")?;
            let executable = std::env::current_exe()
                .and_then(|path| path.canonicalize())
                .map_err(|error| format!("Could not locate the Termeric executable: {error}"))?;
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
            let ctx = self.build_plugin_context();
            let mut action =
                crate::plugin::agent::open_tab_action(&agent, &ctx, None, false, None)?;
            if let crate::plugin::hooks::PluginAction::OpenTab { args, title, .. } = &mut action {
                args.extend(extra);
                *title = Some("Supervisor".into());
            }
            let before = self.tabs.as_ref().map(TabManager::active_tab_id);
            self.execute_plugin_actions(vec![action]);
            let tab = self
                .tabs
                .as_ref()
                .and_then(TabManager::active_tab)
                .filter(|tab| Some(Some(tab.id)) != before)
                .ok_or_else(|| format!("{} did not start", agent.name))?;
            let terminal_id = tab
                .pane_terminal_id(tab.focused_terminal_identity().pane_id)
                .ok_or("The supervisor's terminal has no identity")?
                .to_owned();
            let mut started = Supervisor {
                token,
                terminal_id,
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
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
            if let Some(tabs) = &mut self.tabs {
                tabs.refresh_agents(&self.agents);
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.toasts.error(error);
            return;
        }
        // Without status hooks the supervisor is never told that an agent
        // needs input, finished, or failed, and only sees what it asks for.
        if matches!(
            self.agent_hook_summary(),
            super::agent_hooks::HookSetupSummary::NotSetUp
                | super::agent_hooks::HookSetupSummary::Incomplete
        ) {
            self.open_agent_hook_setup_because(
                true,
                Some("The supervisor hears when an agent needs you, finishes, or fails only through these hooks."),
            );
        }
    }

    /// Forget the supervisor, and close the approval dialog it left open.
    fn end_supervision(&mut self) {
        if let Some(approval) = self
            .supervisor
            .take()
            .and_then(|supervisor| supervisor.approval)
        {
            self.info_overlay.dismiss(approval.request_id);
        }
    }

    /// Tell the supervisor that text meant for another agent was dropped.
    pub(super) fn report_undelivered(&mut self, target: &AgentTarget, reason: &str) {
        if self.supervisor.is_none() || self.is_supervisor_terminal(target.identity) {
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

    /// End supervision and close the supervisor's terminal: its pane, or
    /// its tab when it is the only pane there.
    pub(super) fn stop_supervisor(&mut self) {
        let terminal = self.supervisor_terminal();
        self.end_supervision();
        if let Some(identity) = terminal {
            let alone = self
                .tabs
                .as_ref()
                .and_then(|tabs| tabs.tab_by_id(identity.tab_id))
                .is_some_and(|tab| tab.owned_pane_ids().len() <= 1);
            let closed = if alone {
                self.close_plugin_tab(identity.tab_id as u64).map(|_| ())
            } else {
                self.close_control_pane(identity.tab_id as u64, identity.pane_id.0)
                    .map(|_| ())
            };
            if let Err(error) = closed {
                self.toasts
                    .warning(format!("The supervisor's terminal stayed open: {error}"));
            }
        }
        self.toasts.info("Supervisor stopped");
    }

    /// Forget a supervisor whose terminal has closed. Returns whether it did.
    pub(super) fn retire_closed_supervisor(&mut self) -> bool {
        if self.supervisor.is_none() || self.supervisor_terminal().is_some() {
            return false;
        }
        self.end_supervision();
        self.toasts.info("The supervisor's terminal closed");
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
                    icon: None,
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
        if let Err(error) = self.ensure_rules_file() {
            self.toasts
                .error(format!("Could not create the supervisor rules: {error:#}"));
            return;
        }
        let path = self.supervisor_rules_path.display().to_string();
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
                .map(|(when, entry)| format!("{} · {entry}", ago(when.elapsed())))
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

    /// Deliver every waiting message as one prompt, unless the supervisor is
    /// busy or the person is typing to it. Returns whether it delivered.
    pub(super) fn deliver_supervisor_messages(&mut self) -> bool {
        let Some(target) = self.supervisor_target() else {
            return false;
        };
        let Some(supervisor) = &self.supervisor else {
            return false;
        };
        if supervisor.queue.is_empty() {
            return false;
        }
        let busy = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.tab_by_id(target.identity.tab_id))
            .and_then(|tab| tab.pane_session(target.identity.pane_id))
            .and_then(|session| session.agent_status.as_ref())
            .is_some_and(|status| {
                matches!(
                    status.activity,
                    AgentActivity::Working | AgentActivity::NeedsInput
                )
            });
        if busy || self.refuse_while_person_types(&target).is_err() {
            return false;
        }
        let Some(supervisor) = &mut self.supervisor else {
            return false;
        };
        let message = supervisor.queue.drain(..).collect::<Vec<_>>().join("\n");
        if let Err(error) = self.paste_and_submit_to_agent(target, &message)
            && let Some(supervisor) = &mut self.supervisor
        {
            supervisor.note(format!("Could not reach the supervisor: {error}"));
        }
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
        if self.is_supervisor_terminal(identity) {
            self.deliver_supervisor_messages();
            return;
        }
        let Some(status) = status.filter(|status| before != Some(status.activity)) else {
            return;
        };
        let name = self
            .agents
            .get(&status.agent_id)
            .map_or_else(|| status.agent_id.clone(), |agent| agent.name.clone());
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

    /// Whether `process` runs under the supervisor's terminal. The token is
    /// on the supervisor's command line, where any program of the same
    /// person can read it, so it alone does not show that a request comes
    /// from the supervisor rather than, say, an agent it oversees.
    fn is_supervisor_process(&self, process: Option<u32>) -> bool {
        let Some(process) = process else {
            return false;
        };
        self.supervisor_terminal()
            .and_then(|identity| {
                self.tabs
                    .as_ref()?
                    .tab_by_id(identity.tab_id)?
                    .pane_child_process(identity.pane_id)
            })
            .is_some_and(|child| crate::agents::detect::descends_from(process, child))
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
        if !self.owns_supervisor_token(token) {
            return Err(
                "This supervisor does not oversee that window, or it has been stopped".into(),
            );
        }
        if !self.is_supervisor_process(peer_process) {
            return Err("Only the supervisor Termeric started may send as the supervisor".into());
        }
        let identity = TerminalIdentity {
            tab_id: usize::try_from(tab_id)
                .map_err(|_| "Target tab ID does not fit this platform".to_string())?,
            pane_id: crate::panes::PaneId(pane_id),
        };
        if self.is_supervisor_terminal(identity) {
            return Err("The supervisor cannot send to itself".into());
        }
        let agent_name = self
            .agents
            .get(agent_id)
            .map_or_else(|| agent_id.to_string(), |agent| agent.name.clone());
        let target = AgentTarget {
            identity,
            agent_id: agent_id.to_string(),
            agent_name: agent_name.clone(),
        };
        // The target must still run that agent, without the person typing to
        // it, before anything is decided.
        self.recheck_agent_target(&target)?;
        self.refuse_while_person_types(&target)?;
        let per_minute = self.config.supervisor.max_sends_per_minute;
        let key = format!("{}:{}", identity.tab_id, identity.pane_id.0);
        let supervisor = self
            .supervisor
            .as_mut()
            .expect("the token was checked above");
        if let Err(refusal) =
            supervisor
                .limits
                .check(supervisor.mode, per_minute, &key, text, Instant::now())
        {
            supervisor.note(format!("Refused a message to {agent_name}: {refusal}"));
            return Err(refusal.to_string());
        }
        match supervisor.mode {
            SupervisorMode::Watch => unreachable!("watch mode was refused above"),
            SupervisorMode::Auto => {
                self.paste_and_submit_for_supervisor(target, text)?;
                let supervisor = self.supervisor.as_mut().expect("still running");
                supervisor.limits.record(&key, text, Instant::now());
                supervisor.note(format!("Sent to {agent_name}: {}", one_line(text)));
                Ok(false)
            }
            SupervisorMode::Assist => {
                if supervisor.approval.is_some() {
                    return Err(
                        "Another message is waiting for the person's approval; wait for its outcome"
                            .into(),
                    );
                }
                // Never replace a dialog the person is reading or answering.
                if self.info_overlay.is_awaiting_current() {
                    return Err("The person has another dialog open; try again in a minute".into());
                }
                let mut lines = vec![
                    format!("The supervisor wants to send this to {agent_name}:"),
                    String::new(),
                ];
                // The whole message, which the limits keep short enough to
                // read: the person approves exactly what is sent.
                lines.extend(text.lines().map(|line| format!("  {line}")));
                lines.push(String::new());
                let choice_line = lines.len();
                // No single-key shortcuts, and Send after Don't send and
                // disarmed for a moment: the dialog can open while the
                // person is typing, and stray keys must not approve it.
                lines.push("  Don't send".into());
                lines.push("  Send".into());
                let request_id = self.info_overlay.show_choices(
                    "Supervisor message",
                    lines,
                    vec![
                        InfoOverlayChoice::new(DECLINE_CHOICE, "Don't send", choice_line, None),
                        InfoOverlayChoice::new(SEND_CHOICE, "Send", choice_line + 1, None),
                    ],
                );
                let supervisor = self.supervisor.as_mut().expect("still running");
                supervisor.note(format!("Asked you to approve a message to {agent_name}"));
                supervisor.approval = Some(PendingApproval {
                    request_id,
                    target: SendTarget::Terminal(target),
                    text: text.to_owned(),
                    shown_at: Instant::now(),
                });
                Ok(true)
            }
        }
    }

    /// Answer an approval dialog. Returns whether the choice was its.
    pub(super) fn handle_supervisor_choice(&mut self, request_id: u64, choice_id: &str) -> bool {
        if !matches!(choice_id, SEND_CHOICE | DECLINE_CHOICE) {
            return false;
        }
        let Some(supervisor) = &mut self.supervisor else {
            return false;
        };
        let Some(approval) = supervisor
            .approval
            .as_ref()
            .filter(|approval| approval.request_id == request_id)
        else {
            return false;
        };
        if choice_id == SEND_CHOICE && approval.shown_at.elapsed() < APPROVAL_ARMING {
            // Too soon to be a decision about this message; it stays open.
            self.toasts
                .info("Read the message first; Send works in a moment");
            return true;
        }
        self.info_overlay.dismiss(request_id);
        self.resolve_supervisor_approval(if choice_id == SEND_CHOICE {
            ApprovalOutcome::Send
        } else {
            ApprovalOutcome::Decline
        });
        true
    }

    /// Make the waiting approval old enough to accept **Send**.
    #[cfg(test)]
    pub(super) fn arm_supervisor_approval(&mut self) {
        if let Some(approval) = self
            .supervisor
            .as_mut()
            .and_then(|supervisor| supervisor.approval.as_mut())
        {
            approval.shown_at -= APPROVAL_ARMING;
        }
    }

    /// Treat a dismissed approval dialog as declined.
    pub(super) fn cancel_supervisor_approval(&mut self, request_id: u64) {
        if self
            .supervisor
            .as_ref()
            .and_then(|supervisor| supervisor.approval.as_ref())
            .is_some_and(|approval| approval.request_id == request_id)
        {
            self.resolve_supervisor_approval(ApprovalOutcome::Decline);
        }
    }

    /// Settle an approval whose dialog another dialog replaced: it can no
    /// longer be answered, and the supervisor would otherwise wait for it
    /// forever. Returns whether one was settled.
    pub(super) fn settle_replaced_supervisor_approval(&mut self) -> bool {
        let replaced = self
            .supervisor
            .as_ref()
            .and_then(|supervisor| supervisor.approval.as_ref())
            .is_some_and(|approval| !self.info_overlay.is_awaiting(approval.request_id));
        if replaced {
            self.resolve_supervisor_approval(ApprovalOutcome::Unanswered);
        }
        replaced
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
                format!("A message to {name} was closed without an answer"),
                format!(
                    "Termeric: your message to {name} was not sent; the person closed it without answering. Ask again later only if it still matters."
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
    fn check_supervisor_request(
        &self,
        token: &str,
        peer_process: Option<u32>,
    ) -> Result<(), String> {
        if !self.owns_supervisor_token(token) {
            return Err(
                "This supervisor does not oversee that window, or it has been stopped".into(),
            );
        }
        if !self.is_supervisor_process(peer_process) {
            return Err("Only the supervisor Termeric started may act as the supervisor".into());
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
        let decided = match mode {
            SupervisorMode::Watch => {
                return Err(
                    "The supervisor only watches; answer approvals in Assist or Auto mode".into(),
                );
            }
            SupervisorMode::Assist => {
                if !session.suggest(approval_id, allow, reason) {
                    return Err(format!("Approval {approval_id} is no longer pending"));
                }
                false
            }
            SupervisorMode::Auto => {
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
        if let Some(supervisor) = &mut self.supervisor {
            supervisor.note(if decided {
                format!("Chose to {verb} a request in \"{title}\"")
            } else {
                format!("Suggested you {verb} a request in \"{title}\"")
            });
        }
        self.request_agent_view_redraw();
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
        let name = self
            .agent_view
            .session(id)
            .map(|session| format!("\"{}\"", session.title()))
            .ok_or_else(|| "That agent view session has closed".to_string())?;
        if self
            .agent_view
            .state
            .conversation(id)
            .is_some_and(|view| !view.composer.is_empty())
        {
            return Err("The person is writing a message to that session; try again later".into());
        }
        let target = SendTarget::Session {
            id,
            name: name.clone(),
        };
        let key = target.key();
        let per_minute = self.config.supervisor.max_sends_per_minute;
        let supervisor = self.supervisor.as_mut().expect("the token was checked");
        if let Err(refusal) =
            supervisor
                .limits
                .check(supervisor.mode, per_minute, &key, text, Instant::now())
        {
            supervisor.note(format!("Refused a message to {name}: {refusal}"));
            return Err(refusal.to_string());
        }
        match supervisor.mode {
            SupervisorMode::Watch => unreachable!("watch mode was refused above"),
            SupervisorMode::Auto => {
                self.prompt_agent_view_session(id, text)?;
                let supervisor = self.supervisor.as_mut().expect("still running");
                supervisor.limits.record(&key, text, Instant::now());
                supervisor.note(format!("Sent to {name}: {}", one_line(text)));
                Ok(false)
            }
            SupervisorMode::Assist => {
                if supervisor.approval.is_some() {
                    return Err(
                        "Another message is waiting for the person's approval; wait for its outcome"
                            .into(),
                    );
                }
                if self.info_overlay.is_awaiting_current() {
                    return Err("The person has another dialog open; try again in a minute".into());
                }
                let mut lines = vec![
                    format!("The supervisor wants to send this to {name} in the agent view:"),
                    String::new(),
                ];
                lines.extend(text.lines().map(|line| format!("  {line}")));
                lines.push(String::new());
                let choice_line = lines.len();
                lines.push("  Don't send".into());
                lines.push("  Send".into());
                let request_id = self.info_overlay.show_choices(
                    "Supervisor message",
                    lines,
                    vec![
                        InfoOverlayChoice::new(DECLINE_CHOICE, "Don't send", choice_line, None),
                        InfoOverlayChoice::new(SEND_CHOICE, "Send", choice_line + 1, None),
                    ],
                );
                let supervisor = self.supervisor.as_mut().expect("still running");
                supervisor.note(format!("Asked you to approve a message to {name}"));
                supervisor.approval = Some(PendingApproval {
                    request_id,
                    target,
                    text: text.to_owned(),
                    shown_at: Instant::now(),
                });
                Ok(true)
            }
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

/// The first line of `text`, shortened for the activity log.
fn one_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    match line.char_indices().nth(80) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_owned(),
    }
}

/// How long ago, coarsely.
pub(super) fn ago(elapsed: Duration) -> String {
    let minutes = elapsed.as_secs() / 60;
    match minutes {
        0 => "now".into(),
        1..=59 => format!("{minutes} min"),
        _ => format!("{} h", minutes / 60),
    }
}
