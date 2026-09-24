//! Coding agents in terminals: the agent picker, recognizing agents started
//! from a shell, and the commands that act on the focused agent.
//!
//! Agent commands are ordinary core palette commands. Those that need an
//! agent are hidden unless the focused terminal is running one, so the
//! palette never lists a copy of each command per agent.

use super::*;
use crate::agents::{AgentActivity, AgentDefinition, AgentReport, AgentSession, AgentStatus};
use crate::tabs::TerminalIdentity;
use crate::ui::command_palette::palette::{
    AgentPlacement, CommandSource, PickKind, RegisteredCommand,
};

/// How often terminals are re-examined for a foreground agent.
pub(super) const AGENT_SCAN_INTERVAL: Duration = Duration::from_secs(1);

/// Palette commands that only apply while the focused terminal runs an agent.
pub(super) const FOCUSED_AGENT_COMMANDS: &[&str] = &[
    "agent.new-conversation",
    "agent.restart",
    "agent.insert-context",
    "agent.review-diff",
    "agent.copy-diff",
    "agent.export",
    "agent.copy-last-response",
    "core.toggle-transcript",
];

/// Delay between typing text into an agent and pressing Enter. Agents such
/// as Codex read an Enter that arrives in the same burst as typed characters
/// as a pasted newline rather than a submission.
pub(super) const AGENT_SUBMIT_DELAY: Duration = Duration::from_millis(250);

/// An Enter key to send once typed text has settled in the agent.
#[derive(Debug)]
pub(super) struct PendingAgentSubmit {
    target: AgentTarget,
    due: Instant,
}

/// The agent in the focused terminal.
pub(super) struct FocusedAgent {
    pub identity: TerminalIdentity,
    pub definition: AgentDefinition,
    pub session: AgentSession,
    /// What the agent last reported, when its hooks are set up.
    pub status: Option<AgentStatus>,
}

/// A terminal running an agent, for the agent list.
pub(super) struct AgentTerminal {
    pub identity: TerminalIdentity,
    pub agent_id: String,
    pub name: String,
    pub status: Option<AgentStatus>,
    pub tab_title: String,
}

impl App {
    /// Re-identify agents at most once per [`AGENT_SCAN_INTERVAL`]. Returns
    /// whether any terminal changed.
    pub(super) fn refresh_agent_sessions(&mut self) -> bool {
        if self.last_agent_scan.elapsed() < AGENT_SCAN_INTERVAL {
            return false;
        }
        self.last_agent_scan = Instant::now();
        self.tabs
            .as_mut()
            .is_some_and(|tabs| tabs.refresh_agents(&self.agents))
    }

    pub(super) fn next_agent_scan_deadline(&self) -> Instant {
        self.last_agent_scan + AGENT_SCAN_INTERVAL
    }

    pub(super) fn focused_agent(&self) -> Option<FocusedAgent> {
        let tab = self.tabs.as_ref()?.active_tab()?;
        let identity = tab.focused_terminal_identity();
        let state = tab.pane_session(identity.pane_id)?;
        let session = state.agent.clone()?;
        let definition = self.agents.get(&session.agent_id)?.clone();
        Some(FocusedAgent {
            identity,
            definition,
            session,
            status: state.agent_status.clone(),
        })
    }

    /// Every terminal in this window running an agent, most urgent first.
    pub(super) fn agent_terminals(&self) -> Vec<AgentTerminal> {
        let mut terminals: Vec<AgentTerminal> = self
            .tabs
            .iter()
            .flat_map(|tabs| tabs.iter())
            .filter(|(_, tab)| !tab.closed && !tab.is_closing())
            .flat_map(|(_, tab)| {
                tab.owned_pane_ids().into_iter().filter_map(move |pane_id| {
                    let state = tab.pane_session(pane_id)?;
                    let agent = state.agent.as_ref()?;
                    Some(AgentTerminal {
                        identity: TerminalIdentity {
                            tab_id: tab.id,
                            pane_id,
                        },
                        agent_id: agent.agent_id.clone(),
                        name: self
                            .agents
                            .get(&agent.agent_id)
                            .map_or_else(|| agent.agent_id.clone(), |agent| agent.name.clone()),
                        status: state.agent_status.clone(),
                        tab_title: tab.title.clone(),
                    })
                })
            })
            .collect();
        // Stable: equally urgent agents keep their tab order.
        terminals.sort_by_key(|terminal| {
            std::cmp::Reverse(terminal.status.as_ref().and_then(AgentStatus::badge))
        });
        terminals
    }

    /// Commands to hide from the palette for the current focus.
    pub(super) fn unavailable_agent_commands(&self) -> Vec<String> {
        use super::agent_hooks::HookSetupSummary;
        let focused = self.focused_agent();
        let mut hidden: Vec<&str> = FOCUSED_AGENT_COMMANDS
            .iter()
            .copied()
            .filter(|id| match &focused {
                None => true,
                Some(agent) => match *id {
                    "agent.new-conversation" => agent.definition.new_conversation.is_none(),
                    "agent.restart" => !agent.session.launched,
                    "agent.copy-last-response" => agent.status.as_ref().is_none_or(|status| {
                        status.last_message.is_none() && status.transcript_path.is_none()
                    }),
                    // Only when the agent printed a diff Termeric recognized.
                    "agent.review-diff" | "agent.copy-diff" => {
                        self.selected_diff(agent.identity).is_none()
                    }
                    _ => false,
                },
            })
            .collect();
        if self.agent_terminals().is_empty() {
            hidden.push("agent.go-to");
        }
        // Offered even after the agent exits, while its baseline remains.
        if self.focused_change_baseline().is_none() {
            hidden.extend(["agent.review-changes", "agent.stage-changes"]);
        }
        hidden.extend(match self.agent_hook_summary() {
            HookSetupSummary::Unavailable => &["agent.setup-status", "agent.remove-status"][..],
            HookSetupSummary::SetUp => &["agent.setup-status"][..],
            HookSetupSummary::NotSetUp => &["agent.remove-status"][..],
            HookSetupSummary::Incomplete => &[][..],
        });
        hidden.into_iter().map(str::to_owned).collect()
    }

    /// This window's agents for the control protocol.
    pub(super) fn control_agents(
        &self,
        window_id: u64,
    ) -> Vec<crate::control::protocol::AgentSummary> {
        use crate::control::protocol::{AgentState, AgentSummary, MAX_AGENT_SUMMARY_MESSAGE_BYTES};
        let printable = |text: &str| text.chars().filter(|c| !c.is_control()).collect::<String>();
        self.agent_terminals()
            .into_iter()
            .map(|terminal| {
                let status = terminal.status.as_ref();
                AgentSummary {
                    window_id,
                    tab_id: terminal.identity.tab_id as u64,
                    pane_id: terminal.identity.pane_id.0,
                    agent_id: terminal.agent_id,
                    name: printable(&terminal.name),
                    tab_title: printable(&terminal.tab_title),
                    state: status.map(|status| match status.activity {
                        AgentActivity::Idle => AgentState::Idle,
                        AgentActivity::Working => AgentState::Working,
                        AgentActivity::NeedsInput => AgentState::NeedsInput,
                        AgentActivity::Finished => AgentState::Finished,
                        AgentActivity::Failed => AgentState::Failed,
                    }),
                    detail: status.and_then(|status| status.detail.clone()),
                    last_message: status
                        .and_then(|status| status.last_message.as_deref())
                        .map(|message| {
                            let mut end = message.len().min(MAX_AGENT_SUMMARY_MESSAGE_BYTES);
                            while !message.is_char_boundary(end) {
                                end -= 1;
                            }
                            message[..end].to_owned()
                        }),
                    seconds_in_state: status.map(|status| status.since.elapsed().as_secs()),
                }
            })
            .collect()
    }

    /// Submit text sent through the control protocol to one agent.
    pub(super) fn submit_control_text_to_agent(
        &mut self,
        tab_id: u64,
        pane_id: u32,
        agent_id: &str,
        text: &str,
    ) -> Result<(), String> {
        if !self.config.security.allow_gui_control_input {
            return Err(
                "External terminal input is disabled; set security.allow_gui_control_input = true"
                    .into(),
            );
        }
        let identity = TerminalIdentity {
            tab_id: usize::try_from(tab_id)
                .map_err(|_| "Target tab ID does not fit this platform".to_string())?,
            pane_id: crate::panes::PaneId(pane_id),
        };
        let agent_name = self
            .agents
            .get(agent_id)
            .map_or_else(|| agent_id.to_string(), |agent| agent.name.clone());
        self.paste_and_submit_to_agent(
            AgentTarget {
                identity,
                agent_id: agent_id.to_string(),
                agent_name,
            },
            text,
        )
    }

    /// List the agents running in this window, with what each is doing.
    pub(super) fn open_agent_list(&mut self) {
        let terminals = self.agent_terminals();
        if terminals.is_empty() {
            self.toasts.info("No agent is running in this window");
            return;
        }
        let items = terminals
            .iter()
            .map(|terminal| {
                let state = match &terminal.status {
                    Some(status) => status.activity.label(),
                    None => "Running",
                };
                RegisteredCommand {
                    id: format!(
                        "{}:{}",
                        terminal.identity.tab_id, terminal.identity.pane_id.0
                    ),
                    title: format!("{} · {state}", terminal.name),
                    category: terminal
                        .status
                        .as_ref()
                        .and_then(|status| status.detail.clone())
                        .unwrap_or_else(|| terminal.tab_title.clone()),
                    shortcut: None,
                    icon: None,
                    source: CommandSource::Core,
                    destructive: false,
                }
            })
            .collect();
        self.command_palette.begin_pick(
            PickKind::AgentTerminal,
            items,
            "Go to which agent…",
            "agent",
            None,
        );
    }

    /// Show the terminal picked from the agent list.
    pub(super) fn focus_agent_terminal(&mut self, pick_id: &str) {
        let identity = pick_id.split_once(':').and_then(|(tab, pane)| {
            Some(TerminalIdentity {
                tab_id: tab.parse().ok()?,
                pane_id: crate::panes::PaneId(pane.parse().ok()?),
            })
        });
        if !identity.is_some_and(|identity| self.activate_notification_origin(identity)) {
            self.toasts.warning("That agent's terminal has closed");
        }
    }

    /// Copy the focused agent's most recent answer.
    pub(super) fn copy_last_agent_response(&mut self) {
        match self.focused_agent_answer() {
            Ok((name, text)) => {
                self.copy_to_clipboard(text, &format!("Copied {name}'s last answer"))
            }
            Err(reason) => self.toasts.info(reason),
        }
    }

    /// The focused agent's name and most recent answer: as its hook reported
    /// it, else from the end of its own conversation log.
    pub(super) fn focused_agent_answer(&self) -> Result<(String, String), String> {
        let agent = self
            .focused_agent()
            .ok_or("Focus an agent terminal first")?;
        let name = agent.definition.name;
        let status = agent
            .status
            .ok_or("Set up agent status to copy an agent's answers")?;
        status
            .last_message
            .or_else(|| {
                status
                    .transcript_path
                    .as_deref()
                    .and_then(crate::agents::transcript::last_response)
            })
            .map(|text| (name.clone(), text))
            .ok_or_else(|| format!("{name} has not finished an answer yet"))
    }

    /// Open the palette with commands that do not apply hidden.
    pub(super) fn open_command_palette(&mut self) {
        let unavailable = self.unavailable_agent_commands();
        self.command_palette.set_unavailable(unavailable);
        self.command_palette.open();
    }

    /// Show installed agents (and their profiles) to start at `placement`.
    pub(super) fn open_agent_picker(&mut self, placement: AgentPlacement) {
        let items = agent_pick_items(&self.agents, placement, |agent| {
            agent.installed_path().is_some()
        });
        if items.is_empty() {
            self.toasts.info(match placement {
                AgentPlacement::Resume => {
                    "No installed agent can resume a conversation".to_string()
                }
                _ => "No coding agents found on PATH. Install one, or add it under [agents] in config.toml".to_string(),
            });
            return;
        }
        let placeholder = match placement {
            AgentPlacement::Resume => "Resume which agent…",
            _ => "Start which agent…",
        };
        self.command_palette.begin_pick(
            PickKind::Agent(placement),
            items,
            placeholder,
            "agent",
            None,
        );
    }

    /// Start the agent chosen in the picker.
    pub(super) fn start_picked_agent(&mut self, placement: AgentPlacement, pick_id: &str) {
        let (agent_id, profile) = match pick_id.split_once('/') {
            Some((agent, profile)) => (agent, Some(profile)),
            None => (pick_id, None),
        };
        let Some(agent) = self
            .agents
            .get(agent_id)
            .filter(|agent| agent.enabled)
            .cloned()
        else {
            return;
        };
        match placement {
            AgentPlacement::NewTab | AgentPlacement::Resume => {
                let ctx = self.build_plugin_context();
                let resume = placement == AgentPlacement::Resume;
                match crate::plugin::agent::open_tab_action(&agent, &ctx, profile, resume, None) {
                    Ok(action) => self.execute_plugin_actions(vec![action]),
                    Err(error) => self.toasts.error(error),
                }
            }
            AgentPlacement::SplitRight | AgentPlacement::SplitDown => {
                let direction = if placement == AgentPlacement::SplitRight {
                    SplitDirection::Vertical
                } else {
                    SplitDirection::Horizontal
                };
                let Some(target) = self
                    .tabs
                    .as_ref()
                    .and_then(TabManager::active_tab)
                    .map(|tab| tab.focused_terminal_identity())
                else {
                    return;
                };
                if let Err(error) = self.launch_plugin_pane(target, direction, &agent.id, profile) {
                    self.toasts
                        .error(format!("Could not open {}: {error:#}", agent.name));
                }
            }
        }
        // A launched agent is recognized immediately, not at the next scan.
        if let Some(tabs) = &mut self.tabs {
            tabs.refresh_agents(&self.agents);
        }
    }

    /// Type the focused agent's own "start over" command, such as `/clear`,
    /// then submit it.
    pub(super) fn start_new_agent_conversation(&mut self) {
        let Some(agent) = self.focused_agent() else {
            return;
        };
        let Some(text) = agent.definition.new_conversation.clone() else {
            return;
        };
        let target = AgentTarget {
            identity: agent.identity,
            agent_id: agent.definition.id.clone(),
            agent_name: agent.definition.name.clone(),
        };
        match self.submit_to_agent(target, text.as_bytes()) {
            Ok(()) => self.toasts.info(format!(
                "Started a new {} conversation",
                agent.definition.name
            )),
            Err(reason) => self.toasts.warning(reason),
        }
    }

    /// Relaunch the focused agent that Termeric started, in place.
    pub(super) fn restart_focused_agent(&mut self) {
        let Some(agent) = self.focused_agent().filter(|agent| agent.session.launched) else {
            return;
        };
        self.restart_plugin_at(&agent.definition.id, Some(agent.identity));
    }

    /// Run one `agent.*` palette command. Returns false for other ids.
    pub(super) fn run_agent_command(&mut self, id: &str) -> bool {
        match id {
            "agent.new" => self.open_agent_picker(AgentPlacement::NewTab),
            "agent.resume" => self.open_agent_picker(AgentPlacement::Resume),
            "agent.split-right" => self.open_agent_picker(AgentPlacement::SplitRight),
            "agent.split-down" => self.open_agent_picker(AgentPlacement::SplitDown),
            "agent.go-to" => self.open_agent_list(),
            "agent.review-changes" => self.review_agent_changes(),
            "agent.stage-changes" => self.stage_all_agent_changes(),
            "agent.copy-last-response" => self.copy_last_agent_response(),
            "agent.setup-status" => self.open_agent_hook_setup(true),
            "agent.remove-status" => self.open_agent_hook_setup(false),
            "agent.new-conversation" => self.start_new_agent_conversation(),
            "agent.restart" => self.restart_focused_agent(),
            "agent.insert-context" | "agent.review-diff" | "agent.copy-diff" | "agent.export" => {
                let Some(agent) = self.focused_agent() else {
                    self.toasts.warning("Focus an agent terminal first");
                    return true;
                };
                match id {
                    "agent.insert-context" => {
                        self.open_context_picker_for(agent.identity, &agent.definition)
                    }
                    "agent.review-diff" => self.open_agent_diff_review(agent.identity),
                    "agent.copy-diff" => self.copy_agent_diff(agent.identity),
                    _ => self.export_terminal_session(agent.identity),
                }
            }
            _ => return false,
        }
        true
    }
}

impl App {
    /// Apply a report from an agent's hooks to the terminal it names.
    /// Returns whether this window owns that terminal.
    pub(super) fn record_agent_report(&mut self, terminal_id: &str, report: AgentReport) -> bool {
        let Some(tabs) = self.tabs.as_mut() else {
            return false;
        };
        let Some(identity) = tabs.terminal_with_id(terminal_id) else {
            return false;
        };
        // Disabled agents are not tracked, but the report was delivered.
        if self
            .agents
            .get(&report.agent_id)
            .is_none_or(|agent| !agent.enabled)
        {
            return true;
        }
        let Some(tab) = tabs.tab_mut_by_id(identity.tab_id) else {
            return false;
        };
        let Some(session) = tab.pane_session_mut(identity.pane_id) else {
            return false;
        };
        let before = session.agent_status.as_ref().map(|status| status.activity);
        session.agent_status =
            AgentStatus::after(session.agent_status.as_ref(), report, Instant::now());
        let after = session.agent_status.clone();
        // A reporting agent is recognized now, not at the next scan.
        tab.refresh_agents(&self.agents);
        self.acknowledge_focused_agent();
        if let Some(status) =
            after.filter(|status| status.needs_attention() && before != Some(status.activity))
        {
            self.notify_agent_attention(identity, &status);
        }
        self.redraw_scheduler
            .request(super::redraw_scheduler::RedrawReason::TerminalOutput);
        true
    }

    /// Mark the focused terminal's finished, failed, or waiting agent as seen
    /// while the person is looking at it. Returns whether anything changed.
    pub(super) fn acknowledge_focused_agent(&mut self) -> bool {
        if !self.window_focused {
            return false;
        }
        let Some(tab) = self.tabs.as_mut().and_then(TabManager::active_tab_mut) else {
            return false;
        };
        let pane_id = tab.focused_terminal_identity().pane_id;
        let Some(status) = tab
            .pane_session_mut(pane_id)
            .and_then(|session| session.agent_status.as_mut())
            .filter(|status| status.needs_attention())
        else {
            return false;
        };
        status.seen = true;
        true
    }

    /// Tell the person an agent they are not looking at wants them.
    fn notify_agent_attention(&mut self, identity: TerminalIdentity, status: &AgentStatus) {
        if !self.config.agent_status.notify {
            return;
        }
        let Some(origin) = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.origin_for_pane(identity.tab_id, identity.pane_id))
        else {
            return;
        };
        if !self.should_present_terminal_notification(
            origin,
            termeric_core::NotificationOccasion::Unfocused,
        ) {
            return;
        }
        let name = self
            .agents
            .get(&status.agent_id)
            .map_or(status.agent_id.as_str(), |agent| agent.name.as_str());
        let request = attention_notification(name, identity, status);
        if let Some(error) = self.notifications.notify_terminal(identity, request) {
            self.toasts.warning(error);
        }
    }
}

/// The desktop notification for an agent that wants the person. Each
/// terminal has one, replaced as its agent moves on, and activating it
/// focuses that terminal.
fn attention_notification(
    name: &str,
    identity: TerminalIdentity,
    status: &AgentStatus,
) -> termeric_core::NotificationRequest {
    let title = match status.activity {
        AgentActivity::NeedsInput => format!("{name} needs you"),
        AgentActivity::Failed => format!("{name} stopped with an error"),
        _ => format!("{name} is done"),
    };
    let body = status
        .detail
        .clone()
        .or_else(|| status.last_message.as_deref().map(first_line))
        .unwrap_or_default();
    termeric_core::NotificationRequest {
        title: Some(title),
        body,
        identifier: Some(format!("agent-{}-{}", identity.tab_id, identity.pane_id.0)),
        application_name: Some("Termeric".into()),
        occasion: termeric_core::NotificationOccasion::Unfocused,
        focus_on_activation: true,
        ..termeric_core::NotificationRequest::default()
    }
}

/// The first non-empty line of `text`, shortened for a notification.
fn first_line(text: &str) -> String {
    const LIMIT: usize = 160;
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    match line.char_indices().nth(LIMIT) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_string(),
    }
}

/// An agent terminal to send text to, as it was when chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AgentTarget {
    pub identity: TerminalIdentity,
    pub agent_id: String,
    pub agent_name: String,
}

impl App {
    /// The focused agent terminal, else the only agent terminal open.
    pub(super) fn preferred_agent_target(&self) -> Result<AgentTarget, String> {
        let target = |identity: TerminalIdentity, session: &AgentSession| AgentTarget {
            identity,
            agent_id: session.agent_id.clone(),
            agent_name: self
                .agents
                .get(&session.agent_id)
                .map_or_else(|| session.agent_id.clone(), |agent| agent.name.clone()),
        };
        let focused = self
            .focused_agent()
            .map(|agent| target(agent.identity, &agent.session));
        let open: Vec<AgentTarget> = self
            .tabs
            .iter()
            .flat_map(|tabs| tabs.iter())
            .filter(|(_, tab)| !tab.closed && !tab.is_closing())
            .flat_map(|(_, tab)| {
                tab.owned_pane_ids().into_iter().filter_map(move |pane_id| {
                    let session = tab.pane_session(pane_id)?.agent.as_ref()?;
                    Some(target(
                        TerminalIdentity {
                            tab_id: tab.id,
                            pane_id,
                        },
                        session,
                    ))
                })
            })
            .collect();
        super::select_agent_target(focused, &open)
    }

    /// Paste `text` into the agent at `target`, provided that terminal still
    /// runs the same agent. Pasting (bracketed when the agent asks for it)
    /// keeps a multi-line block from being submitted line by line.
    pub(super) fn send_to_agent(&mut self, target: AgentTarget, text: &str) -> bool {
        let written = self.agent_terminal_mode(&target).and_then(|mode| {
            let framed = crate::input::encode_paste(mode, text);
            self.write_to_agent(&target, &framed)
        });
        match written {
            Ok(()) => {
                self.toasts
                    .success(format!("Sent to {}", target.agent_name));
                true
            }
            Err(reason) => {
                self.toasts.warning(reason);
                false
            }
        }
    }

    /// Paste `text` into the agent at `target` (bracketed when the agent asks
    /// for it, so several lines arrive as one prompt) and submit it.
    pub(super) fn paste_and_submit_to_agent(
        &mut self,
        target: AgentTarget,
        text: &str,
    ) -> Result<(), String> {
        let mode = self.agent_terminal_mode(&target)?;
        let framed = crate::input::encode_paste(mode, text);
        self.submit_to_agent(target, &framed)
    }

    /// Type `input` into the agent at `target` as keystrokes and press Enter
    /// once it has settled, so the agent submits it.
    pub(super) fn submit_to_agent(
        &mut self,
        target: AgentTarget,
        input: &[u8],
    ) -> Result<(), String> {
        self.write_to_agent(&target, input)?;
        self.pending_agent_submits
            .retain(|pending| pending.target.identity != target.identity);
        self.pending_agent_submits.push(PendingAgentSubmit {
            target,
            due: Instant::now() + AGENT_SUBMIT_DELAY,
        });
        Ok(())
    }

    /// Press Enter for every typed submission whose delay has passed.
    pub(super) fn submit_due_agent_input(&mut self, now: Instant) {
        let (due, waiting) = std::mem::take(&mut self.pending_agent_submits)
            .into_iter()
            .partition(|pending| pending.due <= now);
        self.pending_agent_submits = waiting;
        for PendingAgentSubmit { target, .. } in due {
            if let Err(reason) = self.write_to_agent(&target, b"\r") {
                self.toasts.warning(reason);
            }
        }
    }

    pub(super) fn next_agent_submit_deadline(&self) -> Option<Instant> {
        self.pending_agent_submits
            .iter()
            .map(|pending| pending.due)
            .min()
    }

    fn gone_message(target: &AgentTarget) -> String {
        format!(
            "{} is no longer running in that terminal",
            target.agent_name
        )
    }

    /// The input modes of `target`'s terminal while it still runs the same
    /// agent.
    fn agent_terminal_mode(
        &self,
        target: &AgentTarget,
    ) -> Result<termeric_core::term::TerminalMode, String> {
        let tab = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.tab_by_id(target.identity.tab_id))
            .ok_or_else(|| Self::gone_message(target))?;
        let still_agent = tab
            .pane_session(target.identity.pane_id)
            .and_then(|state| state.agent.as_ref())
            .is_some_and(|agent| agent.agent_id == target.agent_id);
        if !still_agent {
            return Err(Self::gone_message(target));
        }
        tab.pane_terminal(target.identity.pane_id)
            .map(|terminal| terminal.mode())
            .ok_or_else(|| Self::gone_message(target))
    }

    /// Write `bytes` to `target` if it still runs the same agent and accepts
    /// input.
    fn write_to_agent(&mut self, target: &AgentTarget, bytes: &[u8]) -> Result<(), String> {
        self.agent_terminal_mode(target)?;
        let tabs = self
            .tabs
            .as_mut()
            .ok_or_else(|| Self::gone_message(target))?;
        let origin = tabs
            .origin_for_pane(target.identity.tab_id, target.identity.pane_id)
            .ok_or_else(|| Self::gone_message(target))?;
        if tabs.origin_read_only(origin) == Some(true) {
            return Err(format!("{}'s terminal is read-only", target.agent_name));
        }
        tabs.write_user_to_origin(origin, bytes)
            .map_err(|error| format!("Could not send to {}: {error}", target.agent_name))
    }
}

/// Picker rows: each agent that `available` accepts, then one row per
/// profile. Row ids are `agent` or `agent/profile`.
pub(super) fn agent_pick_items(
    catalog: &crate::agents::AgentCatalog,
    placement: AgentPlacement,
    available: impl Fn(&AgentDefinition) -> bool,
) -> Vec<RegisteredCommand> {
    let mut items = Vec::new();
    for agent in catalog.enabled().filter(|agent| available(agent)) {
        if placement == AgentPlacement::Resume && agent.resume_args.is_none() {
            continue;
        }
        let row = |id: String, title: String, detail: String| RegisteredCommand {
            id,
            title,
            category: detail,
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        };
        let default_detail = agent
            .default_profile
            .as_ref()
            .map_or_else(String::new, |profile| format!("{profile} profile"));
        items.push(row(agent.id.clone(), agent.name.clone(), default_detail));
        for (name, profile) in &agent.profiles {
            items.push(row(
                format!("{}/{name}", agent.id),
                format!("{} · {name}", agent.name),
                profile.description.clone(),
            ));
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attention_notifications_say_what_the_agent_wants() {
        use crate::agents::AgentReport;
        use crate::control::protocol::AgentEventKind;
        let identity = TerminalIdentity {
            tab_id: 3,
            pane_id: crate::panes::PaneId(1),
        };
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
        let needs = attention_notification(
            "Codex",
            identity,
            &status(AgentEventKind::NeedsInput, Some("Approve shell: ls"), None),
        );
        assert_eq!(needs.title.as_deref(), Some("Codex needs you"));
        assert_eq!(needs.body, "Approve shell: ls");
        assert_eq!(needs.identifier.as_deref(), Some("agent-3-1"));
        assert!(needs.focus_on_activation);

        let long = "word ".repeat(100);
        let done = attention_notification(
            "Codex",
            identity,
            &status(
                AgentEventKind::Finished,
                None,
                Some(&format!("\n\n  {long}\nsecond line")),
            ),
        );
        assert_eq!(done.title.as_deref(), Some("Codex is done"));
        assert!(done.body.starts_with("word word"));
        assert!(done.body.ends_with('…'));
        assert!(!done.body.contains("second line"));
        let failed = attention_notification(
            "Codex",
            identity,
            &status(AgentEventKind::Failed, Some("rate_limit"), None),
        );
        assert_eq!(failed.title.as_deref(), Some("Codex stopped with an error"));
        assert_eq!(failed.body, "rate_limit");
    }

    #[test]
    fn picker_lists_available_agents_then_their_profiles() {
        let configured = toml::from_str(
            r#"
            [claude-code.profiles.review]
            description = "Read-only review"
            [opencode]
            resume_args = []
            "#,
        )
        .unwrap();
        let catalog = crate::agents::AgentCatalog::from_config(&configured);
        let installed = |agent: &AgentDefinition| agent.id != "gemini";
        let items = agent_pick_items(&catalog, AgentPlacement::NewTab, installed);
        let ids: Vec<_> = items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(
            ids,
            ["claude-code", "claude-code/review", "codex", "opencode"]
        );
        assert_eq!(items[1].title, "Claude Code · review");
        assert_eq!(items[1].category, "Read-only review");
        assert!(
            agent_pick_items(&catalog, AgentPlacement::Resume, installed)
                .iter()
                .all(|item| item.id != "gemini")
        );
    }

    #[test]
    fn agent_commands_are_hidden_until_an_agent_is_focused() {
        let app = App::new_headless(Config::default()).unwrap();
        let hidden = app.unavailable_agent_commands();
        for id in FOCUSED_AGENT_COMMANDS {
            assert!(hidden.iter().any(|hidden| hidden == id), "{id}");
        }
    }
}
