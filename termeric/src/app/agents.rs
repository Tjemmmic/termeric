//! Coding agents in terminals: the agent picker, recognizing agents started
//! from a shell, and the commands that act on the focused agent.
//!
//! Agent commands are ordinary core palette commands. Those that need an
//! agent are hidden unless the focused terminal is running one, so the
//! palette never lists a copy of each command per agent.

use super::*;
use crate::agents::{AgentDefinition, AgentSession};
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
        let session = tab.pane_session(identity.pane_id)?.agent.clone()?;
        let definition = self.agents.get(&session.agent_id)?.clone();
        Some(FocusedAgent {
            identity,
            definition,
            session,
        })
    }

    /// Commands to hide from the palette for the current focus.
    pub(super) fn unavailable_agent_commands(&self) -> Vec<String> {
        let focused = self.focused_agent();
        FOCUSED_AGENT_COMMANDS
            .iter()
            .filter(|id| match &focused {
                None => true,
                Some(agent) => match **id {
                    "agent.new-conversation" => agent.definition.new_conversation.is_none(),
                    "agent.restart" => !agent.session.launched,
                    _ => false,
                },
            })
            .map(|id| (*id).to_string())
            .collect()
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
