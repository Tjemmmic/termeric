//! The agent view in one window: its frame, input, and actions.
//!
//! The panel is docked right of the terminals and never modal: it takes
//! keys only while it has keyboard focus, which a click in it or its shortcut
//! gives and a click in a terminal or Escape takes back. Sessions belong to
//! their tab (`agent_view_tabs.rs`) and end with it; the agents keep their
//! logs, so they can be continued later.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use super::*;
use crate::agent_view::agent::AgentKind;
use crate::agent_view::event::EarlierConversation;
use crate::agent_view::history;
use crate::agent_view::session::{AgentProgram, AgentSession, SessionId, StartOptions};
use crate::agent_view::transcript::{Activity, DecidedBy};
use crate::ui::agent_view::colors::Colors;
use crate::ui::agent_view::layout::{
    self, AgentAvailability, Input, NewSessionInfo, PastSession, Suggestions,
};
use crate::ui::agent_view::scene::Scene;
use crate::ui::agent_view::{AgentViewState, Focus, Layout, Screen};
use crate::ui::geometry::Bounds;

/// Earlier conversations listed per folder.
const PAST_LIMIT: usize = 12;
/// How near the divider a press starts a width drag, in logical pixels.
pub(super) const DIVIDER_GRAB: f32 = 4.0;

/// The agent view's state in one window.
#[derive(Default)]
pub(super) struct AgentViewWindow {
    pub state: AgentViewState,
    /// Sessions in the order they started.
    pub sessions: Vec<AgentSession>,
    /// The last scene drawn, for hit testing and accessibility.
    pub scene: Option<Arc<Scene>>,
    colors: Option<(u64, Colors)>,
    /// Earlier Claude Code conversations for a folder, read from its logs
    /// when a page needs them.
    pub past: Option<(PathBuf, Vec<EarlierConversation>)>,
    /// Earlier conversations agents listed themselves, by agent and folder:
    /// the latest list any session there reported, kept after it ends.
    listed: std::collections::HashMap<(AgentKind, PathBuf), Vec<EarlierConversation>>,
    /// Whether each agent's executable was found, by program name. A
    /// program is missing while the search for it runs.
    installed: std::collections::HashMap<String, bool>,
    /// A search for the executable is running; its answer wakes the loop.
    looking: bool,
    /// A press in the composer that may become a selection drag.
    pub selecting: bool,
    /// The last composer press, to recognize a double-click.
    pub last_press: Option<(Instant, f32, f32)>,
    /// The pointer shows the resize cursor over the divider.
    resize_cursor: bool,
    /// Sessions already announced as needing the person.
    announced: HashSet<SessionId>,
    /// Each session's working tree as it started, for reviewing changes.
    tracking: std::collections::HashMap<SessionId, crate::agents::changes::ChangeTracking>,
    /// What the supervisor has been told about each session.
    told: std::collections::HashMap<SessionId, Told>,
    /// Every other tab's agent view, by tab id; the active tab's is `state`.
    pub tabs: std::collections::HashMap<usize, crate::ui::agent_view::TabPanel>,
    /// When the person last looked at each conversation, for the tab bar's
    /// done and failed badges.
    pub seen: std::collections::HashMap<SessionId, Instant>,
    /// An agent and folder where starting ahead of the first message
    /// failed, so it is not retried every frame.
    pub warm_failed: Option<(AgentKind, PathBuf)>,
}

/// An earlier conversation to continue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Resume {
    pub agent: AgentKind,
    /// The agent's own id for the conversation.
    pub id: String,
    pub title: String,
}

/// What the supervisor already heard about one session.
#[derive(Default)]
struct Told {
    approvals: HashSet<String>,
    turns: usize,
    /// Pending approvals the supervisor suggested an answer for: whether
    /// it suggested allowing, and the tool call the decision lands on.
    suggested: std::collections::HashMap<String, (bool, Option<String>)>,
}

impl AgentViewWindow {
    pub fn session(&self, id: SessionId) -> Option<&AgentSession> {
        self.sessions.iter().find(|session| session.id() == id)
    }

    pub fn session_mut(&mut self, id: SessionId) -> Option<&mut AgentSession> {
        self.sessions.iter_mut().find(|session| session.id() == id)
    }

    pub fn active_session_mut(&mut self) -> Option<&mut AgentSession> {
        let id = self.state.active?;
        self.session_mut(id)
    }

    pub fn baseline(&self, id: SessionId) -> Option<&crate::agents::changes::Baseline> {
        self.tracking.get(&id)?.baseline()
    }

    pub fn baseline_unavailable(&self, id: SessionId) -> Option<String> {
        self.tracking
            .get(&id)?
            .unavailable_reason()
            .map(str::to_owned)
    }

    /// Collects finished snapshots.
    pub fn poll_tracking(&mut self) {
        for tracking in self.tracking.values_mut() {
            tracking.poll();
        }
    }

    /// Whether a snapshot is still being taken.
    pub fn capturing(&self) -> bool {
        self.tracking.values().any(|tracking| {
            tracking.baseline().is_none() && tracking.unavailable_reason().is_none()
        })
    }

    /// Takes a session out of this window with its view and change
    /// tracking, forgetting what was said about it here.
    pub fn detach_session(
        &mut self,
        id: SessionId,
    ) -> Option<(
        AgentSession,
        Option<crate::ui::agent_view::ConversationView>,
        Option<crate::agents::changes::ChangeTracking>,
    )> {
        let index = self
            .sessions
            .iter()
            .position(|session| session.id() == id)?;
        let session = self.sessions.remove(index);
        self.told.remove(&id);
        self.announced.remove(&id);
        self.seen.remove(&id);
        Some((
            session,
            self.state.take_conversation(id),
            self.tracking.remove(&id),
        ))
    }

    /// Adds a session that arrived from another window.
    pub fn attach_session(
        &mut self,
        session: AgentSession,
        view: Option<crate::ui::agent_view::ConversationView>,
        tracking: Option<crate::agents::changes::ChangeTracking>,
    ) {
        let id = session.id();
        self.sessions.push(session);
        if let Some(view) = view {
            self.state.put_conversation(id, view);
        }
        if let Some(tracking) = tracking {
            self.tracking.insert(id, tracking);
        }
    }

    /// Sessions that hold a conversation, leaving out those started ahead
    /// of a first message.
    pub fn conversations(&self) -> impl Iterator<Item = &AgentSession> {
        self.sessions.iter().filter(|session| !session.is_blank())
    }

    /// Sessions most recently active first.
    pub fn by_recency(&self) -> Vec<&AgentSession> {
        let mut sessions: Vec<&AgentSession> = self.sessions.iter().collect();
        sessions.sort_by_key(|session| std::cmp::Reverse(session.last_change()));
        sessions
    }
}

/// Seconds since the Unix epoch, or zero for a time before it.
fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}
impl App {
    // ── Sessions ─────────────────────────────────────────────────────────

    /// An agent's program and environment from the agent catalog.
    pub(super) fn agent_view_program(&self, agent: AgentKind) -> AgentProgram {
        let definition = self.agents.get(agent.catalog_id());
        let program = definition.map_or_else(
            || agent.default_program().to_owned(),
            |definition| definition.program.clone(),
        );
        let env = definition
            .and_then(|definition| definition.launch(None, false).ok())
            .map(|launch| launch.env.into_iter().collect())
            .unwrap_or_default();
        AgentProgram {
            program,
            args: Vec::new(),
            env,
        }
    }

    /// Whether `agent` can start: turned on in the agent settings and
    /// installed.
    pub(super) fn agent_view_availability(&mut self, agent: AgentKind) -> AgentAvailability {
        use crate::agent_view::process::{Lookup, find_program};
        if self
            .agents
            .get(agent.catalog_id())
            .is_some_and(|definition| !definition.enabled)
        {
            return AgentAvailability::Disabled;
        }
        let program = self.agent_view_program(agent).program;
        if let Some(installed) = self.agent_view.installed.get(&program) {
            return if *installed {
                AgentAvailability::Ready
            } else {
                AgentAvailability::NotInstalled
            };
        }
        let installed = match find_program(&program, &super::agent_view_event_waker()) {
            Lookup::Found(_) => true,
            Lookup::Missing => false,
            Lookup::Searching => {
                self.agent_view.looking = true;
                return AgentAvailability::Looking;
            }
        };
        self.agent_view.installed.insert(program, installed);
        if installed {
            AgentAvailability::Ready
        } else {
            AgentAvailability::NotInstalled
        }
    }

    /// Where a new session starts: the chosen folder, else the focused
    /// terminal's, else home.
    pub(super) fn agent_view_default_cwd(&self) -> (PathBuf, String) {
        if let Some(cwd) = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(|tab| tab.working_directory())
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
        {
            return (cwd, "From the focused terminal".to_owned());
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        (home, "Your home folder".to_owned())
    }

    pub(super) fn agent_view_new_session_info(&mut self) -> NewSessionInfo {
        let agent = self.agent_view.state.new_session.agent;
        let agents: Vec<(AgentKind, AgentAvailability)> = AgentKind::ALL
            .into_iter()
            .map(|kind| (kind, self.agent_view_availability(kind)))
            .collect();
        let availability = agents
            .iter()
            .find(|(kind, _)| *kind == agent)
            .map_or(AgentAvailability::NotInstalled, |(_, availability)| {
                *availability
            });
        let (default, source) = self.agent_view_default_cwd();
        let (cwd, cwd_source) = match &self.agent_view.state.new_session.cwd {
            Some(cwd) if *cwd != default => (cwd.clone(), "Chosen folder".to_owned()),
            _ => (default, source),
        };
        let mut cwd_choices: Vec<PathBuf> = Vec::new();
        if let Some(tabs) = &self.tabs {
            for (_, tab) in tabs.iter() {
                if let Some(path) = tab.working_directory().map(PathBuf::from)
                    && path.is_dir()
                    && !cwd_choices.contains(&path)
                {
                    cwd_choices.push(path);
                }
            }
        }
        NewSessionInfo {
            cwd,
            cwd_source,
            cwd_choices,
            agent,
            availability,
            agents,
        }
    }

    /// `agent`'s earlier conversations in `cwd`. An agent that lists its
    /// own does so when a session there connects, and the window keeps the
    /// latest list; Claude Code's are read from its logs, once per folder.
    pub(super) fn agent_view_past(
        &mut self,
        agent: AgentKind,
        cwd: &std::path::Path,
        refresh: bool,
    ) -> Vec<PastSession> {
        let listed = self
            .agent_view
            .listed
            .get(&(agent, cwd.to_path_buf()))
            .cloned();
        let earlier = match (listed, agent) {
            (Some(listed), _) => listed,
            (None, AgentKind::ClaudeCode) => {
                let stale = refresh
                    || self
                        .agent_view
                        .past
                        .as_ref()
                        .is_none_or(|(folder, _)| folder != cwd);
                if stale {
                    // Claude reads its logs from the directory its launch
                    // environment names, which the catalog may override.
                    let env = self
                        .agent_view_program(AgentKind::ClaudeCode)
                        .env
                        .into_iter()
                        .collect();
                    let past = crate::agents::setup::CLAUDE_CODE
                        .config_dir(&env)
                        .map(|dir| history::past_conversations(&dir, cwd, PAST_LIMIT))
                        .unwrap_or_default();
                    self.agent_view.past = Some((cwd.to_path_buf(), past));
                }
                self.agent_view
                    .past
                    .as_ref()
                    .map(|(_, past)| past.clone())
                    .unwrap_or_default()
            }
            // Listed once a session in this folder connects.
            (None, AgentKind::Codex | AgentKind::OpenCode) => Vec::new(),
        };
        let now = SystemTime::now();
        // Conversations open here are listed with the sessions instead.
        let open: HashSet<String> = self
            .agent_view
            .sessions
            .iter()
            .filter(|session| !session.handed_off())
            .filter_map(|session| session.conversation_id().map(str::to_owned))
            .collect();
        earlier
            .into_iter()
            .filter(|past| !open.contains(&past.id))
            .take(PAST_LIMIT)
            .map(|past| PastSession {
                agent,
                when: crate::ui::age::prose_age(unix_seconds(past.modified), unix_seconds(now)),
                id: past.id,
                title: past.title,
            })
            .collect()
    }

    /// The title of an earlier conversation the panel listed.
    pub(super) fn agent_view_past_title(&self, id: &str) -> Option<String> {
        let cached = self.agent_view.past.iter().flat_map(|(_, past)| past);
        let listed = self.agent_view.listed.values().flatten();
        cached
            .chain(listed)
            .find(|past| past.id == id)
            .map(|past| past.title.clone())
    }

    /// Starts a session from the new-session page, or continues an earlier
    /// conversation. Returns whether it started.
    pub(super) fn start_agent_session(&mut self, resume: Option<Resume>) -> bool {
        self.bind_agent_view_to_active_tab();
        let info = self.agent_view_new_session_info();
        let prompt = self
            .agent_view
            .state
            .new_session
            .composer
            .text()
            .trim()
            .to_owned();
        let agent = resume.as_ref().map_or(info.agent, |resume| resume.agent);
        // A session already started for this agent and folder takes the
        // message.
        if let Some(id) = self.agent_view.state.active
            && let Some(session) = self.agent_view.session(id)
            && session.is_blank()
        {
            if resume.is_none() && session.cwd() == info.cwd && session.agent() == agent {
                return self.send_first_agent_message(id, &prompt);
            }
            self.end_agent_session(id);
        }
        let form = &self.agent_view.state.new_session;
        // A form set up for another agent says nothing about this one.
        let (mode, model, effort, auto_approve) = if agent == form.agent {
            (
                form.mode_id(),
                form.model.clone().filter(|model| model != "default"),
                form.effort.clone(),
                form.auto_approve,
            )
        } else {
            (agent.default_mode().to_owned(), None, None, false)
        };
        // A tab holds one conversation; another one gets a tab of its own.
        if self.agent_view_conversation().is_some() {
            let layout = self.agent_view.state.layout;
            if !self.open_agent_view_tab(&info.cwd, layout) {
                return false;
            }
            self.open_agent_view(true);
        }
        let options = StartOptions {
            agent,
            cwd: info.cwd.clone(),
            mode,
            model,
            effort,
            auto_approve,
            resume: resume.map(|resume| (resume.id, resume.title)),
            allow_bypass: self.config.agent_view.allow_bypass_permissions,
        };
        let program = self.agent_view_program(agent);
        match AgentSession::start(program, options, super::agent_view_event_waker()) {
            Ok(session) => {
                let id = session.id();
                self.agent_view.sessions.push(session);
                let sent = self.send_first_agent_message(id, &prompt);
                if !sent {
                    // Nothing was said, so there is no conversation to keep.
                    self.end_agent_session(id);
                }
                sent
            }
            Err(error) => {
                self.toasts.error(error.to_string());
                false
            }
        }
    }

    /// Sends a new session its first message from the new-session page and
    /// shows the conversation. Changes are tracked from here, when the work
    /// starts.
    fn send_first_agent_message(&mut self, id: SessionId, prompt: &str) -> bool {
        let Some(session) = self.agent_view.session_mut(id) else {
            return false;
        };
        let cwd = session.cwd().to_path_buf();
        let agent = session.agent();
        if !prompt.is_empty()
            && let Err(error) = session.prompt(prompt, Vec::new())
        {
            self.toasts.error(error.to_string());
            return false;
        }
        let mut tracking = crate::agents::changes::ChangeTracking::default();
        tracking.start_capture((agent.catalog_id(), None), Some(cwd));
        self.agent_view.tracking.insert(id, tracking);
        self.agent_view.state.new_session.composer.clear();
        self.agent_view.state.show(id);
        if !prompt.is_empty() {
            self.agent_view
                .state
                .conversation_mut(id)
                .remember_sent(prompt);
        }
        self.set_agent_view_keyboard(true);
        self.request_redraw(RedrawReason::Action);
        true
    }

    /// Drains every session's output; redraws when anything changed.
    pub(super) fn process_agent_view_events(&mut self) {
        if std::mem::take(&mut self.agent_view.looking) {
            // The search for an agent's program may have finished.
            self.request_redraw(RedrawReason::Action);
        }
        let mut changed = false;
        for session in &mut self.agent_view.sessions {
            changed |= session.pump();
            if let Some(earlier) = session.take_earlier_conversations() {
                let key = (session.agent(), session.cwd().to_path_buf());
                self.agent_view.listed.insert(key, earlier);
            }
        }
        if !changed {
            return;
        }
        // Forget question choices for requests that were answered or
        // withdrawn.
        let pending: Vec<(SessionId, Vec<String>)> = self
            .agent_view
            .sessions
            .iter()
            .map(|session| {
                (
                    session.id(),
                    session
                        .transcript()
                        .approvals()
                        .iter()
                        .map(|approval| approval.request_id.clone())
                        .collect(),
                )
            })
            .collect();
        for (id, requests) in pending {
            let requests: Vec<&str> = requests.iter().map(String::as_str).collect();
            if self.agent_view.state.conversation(id).is_some() {
                self.agent_view
                    .state
                    .conversation_mut(id)
                    .retain_requests(&requests);
            }
        }
        self.announce_agent_view_attention();
        let events = self.agent_view_supervisor_events();
        self.supervise_agent_view(events);
        self.request_redraw(RedrawReason::Action);
    }

    /// What the supervisor has not yet heard: new approval requests and
    /// finished turns. Each is reported once.
    pub(super) fn agent_view_supervisor_events(
        &mut self,
    ) -> Vec<crate::agents::supervisor::SessionEvent> {
        use crate::agent_view::transcript::Item;
        use crate::agents::supervisor::SessionEvent;
        let mut events = Vec::new();
        let own = self.supervisor_session();
        let window = &mut self.agent_view;
        let live: HashSet<SessionId> = window.sessions.iter().map(AgentSession::id).collect();
        window.told.retain(|id, _| live.contains(id));
        for session in window
            .sessions
            .iter()
            .filter(|session| !session.is_blank() && Some(session.id()) != own)
        {
            let told = window.told.entry(session.id()).or_default();
            let transcript = session.transcript();
            let control_id = session.id().control_id();
            let title = session.title();
            let pending: HashSet<String> = transcript
                .approvals()
                .iter()
                .map(|approval| approval.request_id.clone())
                .collect();
            for approval in transcript.approvals() {
                if told.approvals.insert(approval.request_id.clone()) {
                    events.push(SessionEvent::Approval {
                        session: control_id,
                        title: title.clone(),
                        approval_id: approval.request_id.clone(),
                        request: crate::agent_view::present::approval_text(approval, session.cwd()),
                    });
                }
            }
            told.approvals.retain(|id| pending.contains(id));
            for approval in transcript.approvals() {
                if let Some(suggestion) = transcript.suggestion(&approval.request_id) {
                    told.suggested.insert(
                        approval.request_id.clone(),
                        (suggestion.allow, approval.tool_use_id.clone()),
                    );
                }
            }
            let settled: Vec<String> = told
                .suggested
                .keys()
                .filter(|id| !pending.contains(*id))
                .cloned()
                .collect();
            for approval_id in settled {
                let (suggested_allow, tool_use_id) = told
                    .suggested
                    .remove(&approval_id)
                    .expect("the id was just listed");
                let allowed = tool_use_id
                    .and_then(|id| transcript.tool(&id))
                    .and_then(|tool| tool.decision.as_ref())
                    .map(|decision| decision.allowed);
                events.push(SessionEvent::SuggestionSettled {
                    session: control_id,
                    title: title.clone(),
                    approval_id,
                    suggested_allow,
                    allowed,
                });
            }
            let ends: Vec<_> = transcript
                .items()
                .iter()
                .filter_map(|item| match item {
                    Item::TurnEnd(end) => Some(end),
                    _ => None,
                })
                .collect();
            if ends.len() > told.turns {
                told.turns = ends.len();
                if let Some(end) = ends.last().filter(|end| !end.interrupted) {
                    events.push(match &end.outcome.error {
                        Some(error) => SessionEvent::Failed {
                            session: control_id,
                            title: title.clone(),
                            error: error.clone(),
                        },
                        None => SessionEvent::Finished {
                            session: control_id,
                            title: title.clone(),
                            answer: transcript.last_answer().map(str::to_owned),
                        },
                    });
                }
            }
        }
        events
    }

    /// Tells the person, once per request, when a session needs them and
    /// they cannot see it.
    fn announce_agent_view_attention(&mut self) {
        let waiting: Vec<(SessionId, String, String)> = self
            .agent_view
            .sessions
            .iter()
            .filter(|session| session.activity() == Activity::NeedsYou)
            .map(|session| {
                (
                    session.id(),
                    format!("{} needs you", session.agent().name()),
                    session.title(),
                )
            })
            .collect();
        self.agent_view
            .announced
            .retain(|id| waiting.iter().any(|(waiting, ..)| waiting == id));
        let visible = |app: &App, id: SessionId| {
            app.window_focused
                && app.agent_view.state.is_open()
                && app.agent_view.state.screen == Screen::Conversation
                && app.agent_view.state.active == Some(id)
        };
        for (id, summary, title) in waiting {
            if self.agent_view.announced.contains(&id) || visible(self, id) {
                continue;
            }
            self.agent_view.announced.insert(id);
            self.notify_agent_view(&summary, &title);
        }
    }

    // ── Frame ────────────────────────────────────────────────────────────

    /// Whether the panel fills the tab, covering its terminal.
    pub(super) fn agent_view_fills_tab(&self) -> bool {
        let state = &self.agent_view.state;
        state.is_open() && state.tab.is_some() && state.layout == Layout::Full
    }

    /// The panel's area in physical window pixels: the dock, or the whole
    /// terminal area when the panel fills the tab.
    pub(super) fn agent_view_panel_bounds(&self) -> Option<Bounds> {
        let renderer = self.renderer.as_ref()?;
        let (window_width, window_height) = renderer.window_size();
        let top = renderer.content_top();
        let height = (window_height as f32 - top).max(0.0);
        if self.agent_view_fills_tab() {
            let left = renderer.tab_bar_width_for(window_width as f32);
            return Some(Bounds::new(
                left,
                top,
                (window_width as f32 - left).max(0.0),
                height,
            ));
        }
        let width = renderer.dock_width();
        if width <= 0.0 {
            return None;
        }
        Some(Bounds::new(window_width as f32 - width, top, width, height))
    }

    /// The physical width the panel and terminals share, beside any side
    /// tab bar, and the scale factor.
    pub(super) fn agent_dock_room(&self) -> Option<(f32, f32)> {
        let renderer = self.renderer.as_ref()?;
        let (window_width, _) = renderer.window_size();
        let window_width = window_width as f32;
        Some((
            window_width - renderer.tab_bar_width_for(window_width),
            renderer.scale_factor(),
        ))
    }

    pub(super) fn apply_agent_dock_width(&mut self) -> bool {
        let Some((room, scale)) = self.agent_dock_room() else {
            return false;
        };
        // A panel filling the tab covers the terminal, which keeps its size
        // underneath rather than shrinking to nothing.
        let fills_tab = self.agent_view_fills_tab();
        let width = if fills_tab {
            0.0
        } else {
            self.agent_view.state.physical_width(room, scale)
        };
        let Some(renderer) = &mut self.renderer else {
            return false;
        };
        let changed = renderer.set_dock_width(width);
        if width == 0.0 && !fills_tab && self.agent_view.state.has_keyboard() {
            // Keys must never reach a message box nobody can see.
            self.set_agent_view_keyboard(false);
        }
        changed
    }

    /// Whether the panel has room to show. Without a renderer nothing is
    /// laid out, so nothing is hidden either.
    fn agent_view_docked(&self) -> bool {
        self.agent_view_fills_tab()
            || self
                .renderer
                .as_ref()
                .is_none_or(|renderer| renderer.dock_width() > 0.0)
    }

    /// Recomputes the dock's width and fits the active tab to what is left.
    /// The dock belongs to the active tab, so other tabs keep their size.
    pub(super) fn sync_agent_dock(&mut self) {
        if self.apply_agent_dock_width() {
            self.sync_active_terminal_geometry();
        }
    }

    /// When the panel next needs attention without new input.
    pub(super) fn next_agent_view_deadline(&self, now: Instant) -> Option<Instant> {
        let mut deadlines: Vec<Instant> = self
            .agent_view
            .sessions
            .iter()
            .filter_map(AgentSession::handshake_deadline)
            .collect();
        if self.agent_view.capturing() {
            deadlines.push(now + std::time::Duration::from_millis(50));
        }
        if self.agent_view.state.is_open() {
            if self.agent_view_animating() {
                // Spinners and slides at 30 frames a second.
                deadlines.push(now + std::time::Duration::from_millis(33));
            } else if self.agent_view.state.has_keyboard() {
                deadlines.push(self.agent_view.state.next_caret_toggle(now));
            }
        }
        deadlines.into_iter().min()
    }

    /// Runs what came due: handshake timeouts, finished snapshots, and
    /// animation frames.
    pub(super) fn poll_agent_view(&mut self, now: Instant) {
        if self.agent_view.sessions.iter().any(|session| {
            session
                .handshake_deadline()
                .is_some_and(|deadline| deadline <= now)
        }) {
            self.process_agent_view_events();
        }
        self.agent_view.poll_tracking();
        self.reconcile_agent_view_tabs();
        if self.agent_view.state.is_open()
            && (self.agent_view_animating()
                || (self.agent_view.state.has_keyboard()
                    && self.agent_view.state.next_caret_toggle(now)
                        <= now + std::time::Duration::from_millis(5)))
        {
            self.request_redraw(RedrawReason::Action);
        }
    }

    /// Builds this frame's scene, or clears it when the panel is closed.
    pub(super) fn prepare_agent_view_frame(&mut self) {
        self.ensure_agent_view_warm_session();
        self.sync_agent_view_tab_bar();
        let Some(panel) = self.agent_view_panel_bounds() else {
            self.agent_view.scene = None;
            if let Some(renderer) = &mut self.renderer {
                renderer.set_agent_view(None);
            }
            return;
        };
        let theme_key = self.agent_view_theme_key();
        if self
            .agent_view
            .colors
            .as_ref()
            .is_none_or(|(key, _)| *key != theme_key)
        {
            let colors = Colors::from_theme(&self.theme, self.high_contrast_enabled());
            self.agent_view.colors = Some((theme_key, colors));
        }
        let new_session = self.agent_view_new_session_info();
        let past = match self.agent_view.state.screen {
            Screen::Sessions | Screen::NewSession => {
                let cwd = match (self.agent_view.state.screen, self.agent_view.state.active) {
                    (Screen::Sessions, Some(id)) => self.agent_view.session(id).map_or_else(
                        || new_session.cwd.clone(),
                        |session| session.cwd().to_path_buf(),
                    ),
                    _ => new_session.cwd.clone(),
                };
                // The tab's conversation's agent, else the one chosen to start.
                let agent = self
                    .agent_view
                    .state
                    .active
                    .and_then(|id| self.agent_view.session(id))
                    .filter(|session| !session.is_blank())
                    .map_or(new_session.agent, AgentSession::agent);
                self.agent_view_past(agent, &cwd, false)
            }
            Screen::Conversation => Vec::new(),
        };
        let suggestions = self.agent_view_suggestions();
        let terminal_agents = self.agent_view_terminal_rows();
        let supervisor = self.agent_view_supervisor_card();
        let banner = self.agent_view_banner();
        let supervisor_request = self.supervisor_request();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let scale = renderer.scale_factor();
        let sessions = self.agent_view.by_recency();
        let colors = &self
            .agent_view
            .colors
            .as_ref()
            .expect("colors were just set")
            .1;
        let input = Input {
            panel,
            scale,
            now: Instant::now(),
            colors,
            state: &self.agent_view.state,
            sessions: &sessions,
            suggestions,
            banner,
            terminal_agents: &terminal_agents,
            past: &past,
            supervisor,
            new_session,
            home,
            allow_bypass: self.config.agent_view.allow_bypass_permissions,
            supervisor_request,
        };
        let scene = Arc::new(layout::build(&input, renderer.text_measure()));
        renderer.set_agent_view(Some(Arc::clone(&scene)));
        self.agent_view.scene = Some(scene);
    }

    fn agent_view_theme_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.theme.meta.name.hash(&mut hasher);
        for color in [
            self.theme.ui.background,
            self.theme.ui.foreground,
            self.theme.ui.tab_active,
        ] {
            color.to_f32_array().map(f32::to_bits).hash(&mut hasher);
        }
        self.high_contrast_enabled().hash(&mut hasher);
        hasher.finish()
    }

    /// Whether the panel needs frames soon without new input: an animation,
    /// a spinner, or a blinking caret.
    pub(super) fn agent_view_animating(&self) -> bool {
        if !self.agent_view.state.is_open() {
            return false;
        }
        let now = Instant::now();
        let state = &self.agent_view.state;
        if state.animating(now)
            || state.sessions_scroll.animating(now)
            || state
                .menu
                .as_ref()
                .is_some_and(|menu| menu.scroll.animating(now))
        {
            return true;
        }
        let active = state.active.and_then(|id| self.agent_view.session(id));
        // Working sessions show spinners; the caret blinks on its own
        // schedule (see `next_agent_view_deadline`).
        let busy =
            self.agent_view.sessions.iter().any(|session| {
                matches!(session.activity(), Activity::Working | Activity::Starting)
            });
        busy || active
            .and_then(|session| state.conversation(session.id()))
            .is_some_and(|view| view.scroll.animating(now))
    }

    // ── Opening and focus ────────────────────────────────────────────────

    /// Opens the panel with keyboard focus, or, if it already has focus,
    /// closes it.
    pub(super) fn toggle_agent_view(&mut self) {
        let state = &mut self.agent_view.state;
        if state.is_open() && state.has_keyboard() {
            self.close_agent_view();
        } else {
            self.open_agent_view(true);
        }
    }

    pub(super) fn open_agent_view(&mut self, keyboard: bool) {
        self.bind_agent_view_to_active_tab();
        if self.agent_view_conversation().is_none() {
            self.agent_view.state.screen = Screen::NewSession;
        }
        self.agent_view.state.open();
        self.agent_view.state.focus = Focus::Composer;
        self.sync_agent_dock();
        let docked = self.agent_view_docked();
        if keyboard && !docked {
            self.toasts
                .info("The window is too narrow for the agent view; it shows when there is room");
        }
        self.set_agent_view_keyboard(keyboard && docked);
        self.request_redraw(RedrawReason::Action);
    }

    pub(super) fn close_agent_view(&mut self) {
        // A session started ahead of a message that never came holds no
        // conversation worth keeping.
        if let Some(id) = self.agent_view.state.active
            && self
                .agent_view
                .session(id)
                .is_some_and(AgentSession::is_blank)
        {
            self.end_agent_session(id);
        }
        let terminal_had_focus = self.terminal_has_focus();
        self.agent_view.state.close();
        self.report_terminal_focus(terminal_had_focus);
        self.sync_agent_dock();
        self.request_redraw(RedrawReason::Action);
    }

    /// Whether keys go to a terminal: the window is focused and the panel
    /// does not hold the keyboard.
    pub(super) fn terminal_has_focus(&self) -> bool {
        self.window_focused && !self.agent_view.state.has_keyboard()
    }

    /// Gives the keyboard to the panel or back to the terminals, telling
    /// terminal programs that track focus, as a window focus change would.
    pub(super) fn set_agent_view_keyboard(&mut self, keyboard: bool) {
        let terminal_had_focus = self.terminal_has_focus();
        self.agent_view.state.set_keyboard(keyboard);
        self.report_terminal_focus(terminal_had_focus);
    }

    pub(super) fn report_terminal_focus(&mut self, terminal_had_focus: bool) {
        let focused = self.terminal_has_focus();
        if focused != terminal_had_focus
            && let Some(report) =
                crate::input::encode_focus_report(self.active_keyboard_mode(), focused)
        {
            self.write_terminal_protocol_input(report);
        }
    }

    /// Opens the panel on a new-session page. A tab holds one conversation,
    /// so a tab that already has one opens a new tab beside it.
    pub(super) fn new_agent_session_page(&mut self) {
        self.bind_agent_view_to_active_tab();
        if let Some(id) = self.agent_view_conversation() {
            let (cwd, agent) = self
                .agent_view
                .session(id)
                .map(|session| (session.cwd().to_path_buf(), session.agent()))
                .expect("the conversation exists");
            let layout = self.agent_view.state.layout;
            if !self.open_agent_view_tab(&cwd, layout) {
                return;
            }
            // Another conversation with the same agent, unless the person
            // picks another.
            self.agent_view.state.new_session.choose_agent(agent);
        }
        self.open_agent_view(true);
        self.agent_view.state.screen = Screen::NewSession;
        self.agent_view.state.new_session.cwd = None;
        self.agent_view.state.menu = None;
        // Look again: an agent may have been installed, and conversations
        // finished, since the page last showed.
        self.agent_view.installed.clear();
        self.agent_view.past = None;
        self.request_redraw(RedrawReason::Action);
    }

    /// Applies `[agent_view]` settings to the panel and every session.
    pub(super) fn sync_agent_view_config(&mut self) {
        let allowed = self.config.agent_view.allow_bypass_permissions;
        for session in &mut self.agent_view.sessions {
            session.set_bypass_allowed(allowed);
        }
        let form = &mut self.agent_view.state.new_session;
        let unrestricted = form
            .agent
            .modes()
            .iter()
            .any(|mode| mode.unrestricted && Some(&mode.id) == form.mode.as_ref());
        if !allowed && unrestricted {
            form.mode = None;
        }
        // The mode menu reads the setting each frame; an open panel shows
        // the change on its next one.
        if self.agent_view.state.is_open() {
            self.request_redraw(RedrawReason::Action);
        }
    }

    /// Ends a session and removes it from the panel. The agent keeps the
    /// conversation; Claude Code's are offered under earlier conversations
    /// again.
    pub(super) fn end_agent_session(&mut self, id: SessionId) {
        let window = &mut self.agent_view;
        let Some(index) = window
            .sessions
            .iter()
            .position(|session| session.id() == id)
        else {
            return;
        };
        // Dropping the session ends its process group.
        drop(window.sessions.remove(index));
        window.tracking.remove(&id);
        window.told.remove(&id);
        window.announced.remove(&id);
        window.seen.remove(&id);
        window.past = None;
        for panel in window.tabs.values_mut() {
            if panel.session == Some(id) {
                panel.session = None;
                panel.screen = Screen::NewSession;
            }
        }
        // The tab keeps its panel, now on a new-session page.
        window.state.forget(id, Screen::NewSession);
        self.request_redraw(RedrawReason::Action);
    }

    /// Shows a session in its tab, switching to that tab.
    pub(super) fn show_agent_session(&mut self, id: SessionId) {
        if self.agent_view.session(id).is_none() {
            return;
        }
        self.bind_agent_view_to_active_tab();
        if let Some(tab) = self.agent_session_tab(id)
            && Some(tab) != self.agent_view.state.tab
            && let Err(error) = self.select_tab_for(Requester::Control, tab as u64)
        {
            self.toasts.error(error);
            return;
        }
        self.open_agent_view(true);
        self.agent_view.state.show(id);
        self.request_redraw(RedrawReason::Action);
    }
}

impl App {
    // ── Services the panel uses ──────────────────────────────────────────

    /// Copies text; an empty `label` copies quietly.
    pub(super) fn copy_agent_view_text(&mut self, text: &str, label: &str) {
        if label.is_empty() {
            match &mut self.clipboard {
                Some(clipboard) => {
                    if let Err(error) = clipboard.set_text(text) {
                        self.toasts.error(format!("Copy failed: {error}"));
                    }
                }
                None => self.toasts.warning("Clipboard not available"),
            }
        } else {
            self.copy_to_clipboard(text.to_owned(), label);
        }
    }

    pub(super) fn agent_view_clipboard_text(&mut self) -> Option<String> {
        let clipboard = self.clipboard.as_mut()?;
        match clipboard.get_text() {
            Ok(text) => Some(text),
            Err(error) => {
                log::debug!("agent view: clipboard read failed: {error}");
                None
            }
        }
    }

    pub(super) fn agent_view_selection_text(&self) -> Option<String> {
        self.native_selection_text()
            .filter(|text| !text.trim().is_empty())
    }

    /// The focused terminal's last finished command: its text, output, and
    /// whether it failed.
    pub(super) fn agent_view_last_command(&self) -> Option<(String, String, bool)> {
        let terminal = self.active_terminal()?;
        if terminal
            .mode()
            .contains(termeric_core::term::TerminalMode::ALT_SCREEN)
        {
            return None;
        }
        let tracker = &terminal.shell_integration().prompt_tracker;
        let region = tracker.completed_region(tracker.completed_count().checked_sub(1)?)?;
        let (command, _) = App::extract_command_text(terminal, region);
        let command = command.trim().to_owned();
        let (output, truncated) = App::extract_output_text(terminal, region);
        if command.is_empty() || output.trim().is_empty() {
            return None;
        }
        let output = if truncated {
            format!("{}\n[output shortened]", output.trim_end())
        } else {
            output.trim_end().to_owned()
        };
        Some((command, output, region.is_failure()))
    }

    /// What the composer offers to attach. Cheap enough to run per frame:
    /// it counts rows rather than copying text.
    fn agent_view_suggestions(&self) -> Suggestions {
        let terminal = self.active_terminal();
        let selection_lines = terminal
            .and_then(|terminal| terminal.selection())
            .filter(|selection| selection.start != selection.end)
            .map(|selection| (selection.end.row - selection.start.row).unsigned_abs() as usize + 1);
        let last_command = terminal.and_then(|terminal| {
            if terminal
                .mode()
                .contains(termeric_core::term::TerminalMode::ALT_SCREEN)
            {
                return None;
            }
            let tracker = &terminal.shell_integration().prompt_tracker;
            let region = tracker.completed_region(tracker.completed_count().checked_sub(1)?)?;
            let (command, _) = App::extract_command_text(terminal, region);
            let command = command.trim().to_owned();
            let rows = match (region.output_row, region.finish_row) {
                (Some(start), Some(end)) if end > start => usize::try_from(end - start).ok()?,
                _ => return None,
            };
            (!command.is_empty()).then(|| (shorten_command(&command), rows, region.is_failure()))
        });
        Suggestions {
            selection_lines,
            last_command,
        }
    }

    fn agent_view_terminal_rows(&self) -> Vec<layout::TerminalAgentRow> {
        self.terminal_agent_rows()
            .into_iter()
            .filter_map(|row| match row.identity {
                crate::ui::agents_panel::AgentOrigin::Terminal(identity) => {
                    Some(layout::TerminalAgentRow {
                        identity,
                        name: row.name,
                        place: format!("Terminal · {}", row.place),
                        needs_you: matches!(
                            row.state,
                            crate::ui::agents_panel::AgentRowState::NeedsYou
                        ),
                        summary: row.summary,
                    })
                }
                crate::ui::agents_panel::AgentOrigin::AgentView(_) => None,
            })
            .collect()
    }

    /// The agent view's sessions as Agents panel rows.
    pub(super) fn agent_view_rows(&self) -> Vec<crate::ui::agents_panel::AgentRow> {
        use crate::ui::agents_panel::{AgentOrigin, AgentRow, AgentRowState};
        self.agent_view
            .by_recency()
            .into_iter()
            .filter(|session| !session.is_blank())
            .map(|session| {
                let activity = session.activity();
                let state = match activity {
                    Activity::Starting | Activity::Working => AgentRowState::Working,
                    Activity::NeedsYou => AgentRowState::NeedsYou,
                    Activity::Finished { success: true } => AgentRowState::Done,
                    Activity::Finished { success: false } => AgentRowState::Failed,
                    Activity::Idle | Activity::Stopped | Activity::Exited => AgentRowState::Idle,
                };
                AgentRow {
                    identity: AgentOrigin::AgentView(session.id()),
                    name: format!("{} · {}", session.agent().name(), session.title()),
                    place: "Agent view".to_owned(),
                    state,
                    summary: format!(
                        "{} · {}",
                        activity.label(),
                        crate::ui::agent_view::layout::session_summary(session)
                    ),
                }
            })
            .collect()
    }

    fn agent_view_supervisor_card(&self) -> Option<layout::SupervisorCard> {
        let summary = self.supervisor_summary()?;
        Some(layout::SupervisorCard {
            status: summary.mode,
            latest: summary.entries.first().cloned(),
            held: summary.held,
        })
    }

    /// While a supervisor runs, a conversation says what it may do there.
    fn agent_view_banner(&self) -> Option<layout::Banner> {
        let summary = self.supervisor_summary()?;
        let own = self
            .agent_view
            .state
            .active
            .is_some_and(|id| self.is_supervisor_session(id));
        // In the supervisor's own conversation: what it may do everywhere.
        let (title, detail) = if own {
            use crate::agents::supervisor::SupervisorMode;
            match (
                summary.held,
                self.supervisor.as_ref().map(|supervisor| supervisor.mode()),
            ) {
                (true, _) => (
                    "You took over",
                    "The supervisor watches and sends nothing until you hand back.",
                ),
                (false, Some(SupervisorMode::Auto)) => (
                    "Auto: the supervisor steers",
                    "It answers agents on its own, following your rules and Termeric's limits.",
                ),
                (false, Some(SupervisorMode::Assist)) => (
                    "Assist: you approve what it sends",
                    "Messages it wants to send to agents wait here for you.",
                ),
                _ => (
                    "Watch: the supervisor reports",
                    "It reads what agents do and sends nothing.",
                ),
            }
        } else if summary.held {
            (
                "You took over from the supervisor",
                "It watches but answers nothing until you hand back.",
            )
        } else {
            match self.supervisor.as_ref().map(|supervisor| supervisor.mode()) {
                Some(crate::agents::supervisor::SupervisorMode::Auto) => (
                    "The supervisor is steering",
                    "It may answer approvals and send messages here, following your rules.",
                ),
                Some(crate::agents::supervisor::SupervisorMode::Assist) => (
                    "The supervisor is assisting",
                    "It suggests answers to approvals; you decide.",
                ),
                _ => (
                    "The supervisor is watching",
                    "It reads this conversation but acts on nothing.",
                ),
            }
        };
        Some(layout::Banner {
            title: title.to_owned(),
            detail: detail.to_owned(),
            action: Some((
                if summary.held {
                    "Hand back"
                } else {
                    "Take over"
                }
                .to_owned(),
                crate::ui::agent_view::scene::Target::ToggleSupervisor,
            )),
            supervisor: true,
        })
    }

    pub(super) fn notify_agent_view(&mut self, summary: &str, title: &str) {
        if !self.config.agent_status.notify {
            return;
        }
        if let Some(error) = self.notifications.notify(summary, title) {
            self.toasts.error(error);
        }
    }

    /// Continues the shown conversation, or starts the chosen agent, in a
    /// new terminal tab.
    pub(super) fn open_agent_view_in_terminal(&mut self) {
        let (kind, cwd, resume) = match (self.agent_view.state.screen, self.agent_view.state.active)
        {
            (Screen::Conversation, Some(id)) => match self.agent_view.session(id) {
                Some(session) => (
                    session.agent(),
                    session.cwd().to_path_buf(),
                    session.conversation_id().map(str::to_owned),
                ),
                None => return,
            },
            _ => {
                let info = self.agent_view_new_session_info();
                (info.agent, info.cwd, None)
            }
        };
        let Some(agent) = self
            .agents
            .get(kind.catalog_id())
            .filter(|agent| agent.enabled)
            .cloned()
        else {
            self.toasts
                .error(format!("{} is disabled in the agent settings", kind.name()));
            return;
        };
        let context = self.build_plugin_context();
        let cwd_text = cwd.display().to_string();
        match crate::plugin::agent::open_tab_action(&agent, &context, None, false, Some(&cwd_text))
        {
            Ok(mut action) => {
                if let (Some(resume), crate::plugin::PluginAction::OpenTab { args, .. }) =
                    (resume, &mut action)
                {
                    args.extend(kind.resume_args(&resume));
                    // Two processes must not write one conversation.
                    if let Some(session) = self.agent_view.active_session_mut() {
                        session.hand_off();
                    }
                }
                self.execute_plugin_action(action);
                self.set_agent_view_keyboard(false);
            }
            Err(error) => self.toasts.error(error),
        }
    }

    pub(super) fn set_agent_view_cursor(&mut self, resize: bool) {
        if self.agent_view.resize_cursor == resize {
            return;
        }
        self.agent_view.resize_cursor = resize;
        if let Some(window) = &self.window {
            window.set_cursor(if resize {
                winit::window::CursorIcon::EwResize
            } else {
                winit::window::CursorIcon::Default
            });
        }
    }
}

/// A long command, shortened for a chip.
fn shorten_command(command: &str) -> String {
    crate::safe_text::first_line_truncated(command, 40)
}

impl App {
    // ── The control socket and the supervisor ───────────────────────────

    /// This window's agent view sessions as control summaries.
    pub(super) fn agent_view_summaries(
        &self,
        window_id: u64,
    ) -> Vec<crate::control::protocol::AgentSummary> {
        use crate::control::protocol::{
            AgentState, AgentSummary, MAX_AGENT_EVENT_DETAIL_BYTES,
            MAX_AGENT_SUMMARY_MESSAGE_BYTES, MAX_METADATA_BYTES,
        };
        self.agent_view
            .conversations()
            .map(|session| {
                let transcript = session.transcript();
                let state = match session.activity() {
                    Activity::Starting | Activity::Working => AgentState::Working,
                    Activity::NeedsYou => AgentState::NeedsInput,
                    Activity::Finished { success: true } => AgentState::Finished,
                    Activity::Finished { success: false } => AgentState::Failed,
                    Activity::Idle | Activity::Stopped | Activity::Exited => AgentState::Idle,
                };
                AgentSummary {
                    window_id,
                    tab_id: 0,
                    pane_id: 0,
                    session_id: Some(session.id().control_id()),
                    agent_id: session.agent().catalog_id().to_owned(),
                    name: session.agent().name().to_owned(),
                    tab_title: crate::safe_text::single_line_within(
                        &format!("Agent view: {}", session.title()),
                        MAX_METADATA_BYTES,
                    ),
                    state: Some(state),
                    detail: (session.activity() == Activity::NeedsYou).then(|| {
                        crate::safe_text::single_line_within(
                            &crate::ui::agent_view::layout::session_summary(session),
                            MAX_AGENT_EVENT_DETAIL_BYTES,
                        )
                    }),
                    last_message: transcript.last_answer().map(|answer| {
                        crate::safe_text::single_line_within(
                            answer,
                            MAX_AGENT_SUMMARY_MESSAGE_BYTES,
                        )
                    }),
                    seconds_in_state: Some(session.last_change().elapsed().as_secs()),
                }
            })
            .collect()
    }

    fn control_session(&self, session_id: u64) -> Result<SessionId, String> {
        // A session started ahead of its first message is not a conversation
        // anyone outside may join.
        self.agent_view
            .conversations()
            .map(AgentSession::id)
            .find(|id| id.control_id() == session_id)
            .ok_or_else(|| format!("Agent view session {session_id} does not exist"))
    }

    /// One session's recent conversation for another program to read.
    pub(super) fn read_agent_view_conversation(
        &self,
        session_id: u64,
    ) -> Result<(String, bool), String> {
        let id = self.control_session(session_id)?;
        let session = self.agent_view.session(id).expect("the id was just found");
        let transcript = session.transcript();
        Ok(crate::agent_view::present::conversation_text(
            session.agent().short_name(),
            transcript.items(),
            transcript.approvals(),
            session.cwd(),
            crate::control::protocol::MAX_TERMINAL_TEXT_BYTES as usize,
        ))
    }

    /// Answers a session's pending approval for another program: the
    /// supervisor, or a person's own tooling when control input is allowed.
    /// Returns whether it was answered rather than suggested.
    pub(super) fn answer_agent_view_approval(
        &mut self,
        session_id: u64,
        approval_id: &str,
        allow: bool,
        reason: &str,
        supervisor: Option<&str>,
        peer_process: Option<u32>,
    ) -> Result<bool, String> {
        let id = self.control_session(session_id)?;
        if let Some(token) = supervisor {
            return self.supervised_session_answer(
                token,
                peer_process,
                id,
                approval_id,
                allow,
                reason,
            );
        }
        if !self.config.security.allow_gui_control_input {
            return Err(
                "External control input is disabled; set security.allow_gui_control_input = true"
                    .into(),
            );
        }
        let session = self
            .agent_view
            .session_mut(id)
            .expect("the id was just found");
        let answered = if allow {
            session.allow(approval_id, false, DecidedBy::Person)
        } else {
            session.deny(approval_id, Some(reason), DecidedBy::Person, false)
        };
        if !answered {
            return Err(format!("Approval {approval_id} is no longer pending"));
        }
        self.request_redraw(RedrawReason::Action);
        Ok(true)
    }

    /// Sends a message to a session for another program. Returns whether it
    /// waits for the person's approval.
    pub(super) fn submit_to_agent_view(
        &mut self,
        session_id: u64,
        text: &str,
        supervisor: Option<&str>,
        peer_process: Option<u32>,
    ) -> Result<bool, String> {
        let id = self.control_session(session_id)?;
        if let Some(token) = supervisor {
            return self.supervised_session_submit(token, peer_process, id, text);
        }
        if !self.config.security.allow_gui_control_input {
            return Err(
                "External control input is disabled; set security.allow_gui_control_input = true"
                    .into(),
            );
        }
        crate::agents::supervisor::check_message(text).map_err(|refusal| refusal.to_string())?;
        self.prompt_agent_view_session(id, text)?;
        Ok(false)
    }

    /// Sends a message on another program's behalf, refusing while the
    /// person is writing one, so the two never interleave.
    pub(super) fn prompt_agent_view_session(
        &mut self,
        id: SessionId,
        text: &str,
    ) -> Result<(), String> {
        if self
            .agent_view
            .state
            .conversation(id)
            .is_some_and(|view| !view.composer.is_empty())
        {
            return Err("The person is writing a message to that session; try again later".into());
        }
        let session = self
            .agent_view
            .session_mut(id)
            .ok_or_else(|| "That agent view session has closed".to_string())?;
        session
            .prompt(text, Vec::new())
            .map_err(|error| error.to_string())?;
        self.request_redraw(RedrawReason::Action);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_view::event::{AgentEvent, ApprovalRequest, Handshake};
    use crate::agent_view::transcript::Transcript;
    use crate::control::protocol::AgentState;

    /// A window with one agent view session waiting for approval `r1`.
    fn app_with_waiting_session() -> (App, SessionId) {
        app_with_session_waiting_on("Run the tests", "cargo test")
    }

    /// A window with one session, started by `prompt`, waiting for approval
    /// `r1` to run `command`.
    fn app_with_session_waiting_on(prompt: &str, command: &str) -> (App, SessionId) {
        let mut app = App::new_headless(crate::config::Config::default()).unwrap();
        let mut transcript = Transcript::new("Claude Code", "default");
        transcript.apply(AgentEvent::Ready(Handshake::default()));
        transcript.push_user(prompt.into(), Vec::new());
        transcript.apply(AgentEvent::ApprovalRequested(ApprovalRequest {
            request_id: "r1".into(),
            tool_use_id: Some("t1".into()),
            tool_name: "Bash".into(),
            input: serde_json::json!({"command": command}),
            title: None,
            description: Some("Run the tests".into()),
            reason: None,
            remember: None,
            from_sub_agent: false,
        }));
        let session = AgentSession::detached(PathBuf::from("/work"), transcript);
        let id = session.id();
        app.agent_view.sessions.push(session);
        (app, id)
    }

    #[test]
    fn sessions_are_listed_and_read_over_the_control_socket() {
        let (app, id) = app_with_waiting_session();
        let listed = app.control_agents(4);
        let session = listed
            .iter()
            .find(|agent| agent.session_id == Some(id.control_id()))
            .expect("the session is listed");
        assert_eq!(
            (session.window_id, session.tab_id, session.pane_id),
            (4, 0, 0)
        );
        assert_eq!(session.state, Some(AgentState::NeedsInput));
        assert_eq!(session.detail.as_deref(), Some("Wants to run cargo test"));
        crate::control::protocol::validate_agent_summaries(&listed).unwrap();
        let (text, truncated) = app.read_agent_view_conversation(id.control_id()).unwrap();
        assert!(!truncated);
        assert!(text.starts_with("Person: Run the tests"));
        assert!(text.ends_with("Waiting for approval r1: Bash cargo test (Run the tests)"));
        assert!(
            app.read_agent_view_conversation(id.control_id() + 1000)
                .is_err()
        );
    }

    #[test]
    fn long_commands_and_prompts_stay_within_summary_bounds() {
        use crate::control::protocol::{MAX_AGENT_EVENT_DETAIL_BYTES, MAX_METADATA_BYTES};
        let prompt = "é".repeat(MAX_METADATA_BYTES);
        let command = format!("git commit -m \"{}\"", "ü".repeat(2_000));
        let (app, _) = app_with_session_waiting_on(&prompt, &command);
        let listed = app.control_agents(1);
        crate::control::protocol::validate_agent_summaries(&listed).unwrap();
        let session = listed
            .iter()
            .find(|agent| agent.session_id.is_some())
            .unwrap();
        assert!(session.tab_title.len() <= MAX_METADATA_BYTES);
        let detail = session.detail.as_deref().unwrap();
        assert!(detail.starts_with("Wants to run git commit"));
        assert!(detail.len() <= MAX_AGENT_EVENT_DETAIL_BYTES);
    }

    #[test]
    fn ending_a_session_removes_it_and_everything_kept_for_it() {
        let (mut app, id) = app_with_waiting_session();
        app.agent_view.state.open();
        app.agent_view.state.show(id);
        app.agent_view.state.composer_mut().insert("draft");
        app.agent_view.tracking.insert(id, Default::default());
        app.agent_view.announced.insert(id);
        app.end_agent_session(id);
        assert!(app.agent_view.session(id).is_none());
        assert!(app.agent_view.state.active.is_none());
        assert_eq!(app.agent_view.state.screen, Screen::NewSession);
        assert!(app.agent_view.state.conversation(id).is_none());
        assert!(!app.agent_view.tracking.contains_key(&id));
        assert!(!app.agent_view.announced.contains(&id));
        assert!(
            app.control_agents(1)
                .iter()
                .all(|agent| agent.session_id != Some(id.control_id()))
        );
        // Ending it again is harmless.
        app.end_agent_session(id);
    }

    #[test]
    fn bypass_permissions_needs_the_opt_in_and_ends_with_it() {
        let (mut app, id) = app_with_waiting_session();
        let mode = |app: &App| {
            app.agent_view
                .session(id)
                .unwrap()
                .transcript()
                .mode()
                .to_owned()
        };
        app.agent_view
            .session_mut(id)
            .unwrap()
            .set_mode("bypassPermissions");
        assert_eq!(mode(&app), "default");

        app.config.agent_view.allow_bypass_permissions = true;
        app.sync_agent_view_config();
        app.agent_view
            .session_mut(id)
            .unwrap()
            .set_mode("bypassPermissions");
        assert_eq!(mode(&app), "bypassPermissions");
        app.agent_view.state.new_session.mode = Some("bypassPermissions".into());

        app.config.agent_view.allow_bypass_permissions = false;
        app.sync_agent_view_config();
        assert_eq!(mode(&app), "default");
        assert_eq!(app.agent_view.state.new_session.mode, None);
    }

    #[test]
    fn the_command_menu_takes_arrows_tab_and_escape_while_it_shows() {
        use winit::keyboard::{Key, NamedKey};
        let mut app = App::new_headless(crate::config::Config::default()).unwrap();
        let mut transcript = Transcript::new("Claude Code", "default");
        transcript.apply(AgentEvent::Ready(Handshake {
            commands: ["compact", "context", "review"]
                .into_iter()
                .map(|name| crate::agent_view::event::SlashCommand {
                    name: name.into(),
                    description: String::new(),
                    argument_hint: if name == "review" {
                        "<pr>".into()
                    } else {
                        String::new()
                    },
                })
                .collect(),
            ..Handshake::default()
        }));
        let session = AgentSession::detached(PathBuf::from("/work"), transcript);
        let id = session.id();
        app.agent_view.sessions.push(session);
        app.open_agent_view(true);
        app.agent_view.state.show(id);
        let named = |key| Key::Named(key);
        let text = |app: &App| app.agent_view.state.composer().unwrap().text().to_owned();

        app.agent_view.state.composer_mut().insert("/co");
        assert!(app.handle_agent_view_key(&named(NamedKey::ArrowDown), None));
        assert!(app.handle_agent_view_key(&named(NamedKey::Tab), None));
        assert_eq!(text(&app), "/context ");

        // Enter on a command that takes an argument completes it only.
        app.agent_view.state.composer_mut().set_text("/rev");
        assert!(app.handle_agent_view_key(&named(NamedKey::Enter), None));
        assert_eq!(text(&app), "/review ");

        // Escape hides the menu for what was typed; typing more shows it.
        app.agent_view.state.composer_mut().set_text("/c");
        assert!(app.handle_agent_view_key(&named(NamedKey::Escape), None));
        assert!(app.agent_command_matches().is_empty());
        assert!(app.agent_view.state.has_keyboard());
        app.agent_view.state.composer_mut().insert("o");
        assert_eq!(app.agent_command_matches().len(), 2);

        // With nothing matching, keys go to the message box as usual.
        app.agent_view.state.composer_mut().set_text("/zz");
        assert!(app.agent_command_matches().is_empty());
    }

    #[test]
    fn show_agents_is_offered_for_agent_view_sessions_alone() {
        let mut app = App::new_headless(crate::config::Config::default()).unwrap();
        let hidden = |app: &App| app.unavailable_agent_commands();
        assert!(hidden(&app).contains(&"agent.panel".to_owned()));
        let (with_session, _) = app_with_waiting_session();
        app = with_session;
        assert!(!hidden(&app).contains(&"agent.panel".to_owned()));
    }

    #[test]
    fn outside_answers_need_the_external_input_opt_in() {
        let (mut app, id) = app_with_waiting_session();
        let session = id.control_id();
        let refused = app
            .answer_agent_view_approval(session, "r1", true, "", None, None)
            .unwrap_err();
        assert!(refused.contains("allow_gui_control_input"), "{refused}");
        app.config.security.allow_gui_control_input = true;
        assert!(
            app.answer_agent_view_approval(session, "r9", true, "", None, None)
                .unwrap_err()
                .contains("no longer pending")
        );
        assert!(
            app.answer_agent_view_approval(session, "r1", true, "", None, None)
                .unwrap()
        );
        assert!(
            app.agent_view
                .session(id)
                .unwrap()
                .transcript()
                .approvals()
                .is_empty()
        );
        // Messages follow the same rules as those typed to terminal agents.
        assert!(
            app.submit_to_agent_view(session, "/compact", None, None)
                .unwrap_err()
                .contains("run as a command")
        );
        // A person's unsent draft is never interleaved with another message.
        app.agent_view
            .state
            .conversation_mut(id)
            .composer
            .insert("half written");
        assert!(
            app.submit_to_agent_view(session, "next step", None, None)
                .unwrap_err()
                .contains("writing a message")
        );
    }

    #[test]
    fn only_the_running_supervisor_may_act_on_sessions() {
        let (mut app, id) = app_with_waiting_session();
        let token = "0".repeat(32);
        let refused = app
            .answer_agent_view_approval(id.control_id(), "r1", true, "", Some(&token), None)
            .unwrap_err();
        assert!(refused.contains("does not oversee"), "{refused}");
        assert!(
            app.submit_to_agent_view(id.control_id(), "hello", Some(&token), None)
                .unwrap_err()
                .contains("does not oversee")
        );
        assert_eq!(
            app.agent_view
                .session(id)
                .unwrap()
                .transcript()
                .approvals()
                .len(),
            1
        );
    }

    #[test]
    fn the_supervisor_hears_each_request_and_turn_once() {
        use crate::agents::supervisor::SessionEvent;
        let (mut app, id) = app_with_waiting_session();
        let first = app.agent_view_supervisor_events();
        assert!(matches!(
            first.as_slice(),
            [SessionEvent::Approval { approval_id, request, .. }]
                if approval_id == "r1" && request == "Bash cargo test (Run the tests)"
        ));
        assert!(app.agent_view_supervisor_events().is_empty());
        let session = app.agent_view.session_mut(id).unwrap();
        session.deny("r1", None, DecidedBy::Person, false);
        assert!(app.agent_view_supervisor_events().is_empty());
    }

    #[test]
    fn the_supervisor_hears_how_its_suggestion_was_settled() {
        use crate::agents::supervisor::SessionEvent;
        let (mut app, id) = app_with_waiting_session();
        app.agent_view_supervisor_events();
        let session = app.agent_view.session_mut(id).unwrap();
        assert!(session.suggest("r1", true, "tests are safe"));
        assert!(app.agent_view_supervisor_events().is_empty());
        let session = app.agent_view.session_mut(id).unwrap();
        session.deny("r1", None, DecidedBy::Person, false);
        let settled = app.agent_view_supervisor_events();
        assert!(matches!(
            settled.as_slice(),
            [SessionEvent::SuggestionSettled {
                approval_id,
                suggested_allow: true,
                allowed: Some(false),
                ..
            }] if approval_id == "r1"
        ));
        assert!(app.agent_view_supervisor_events().is_empty());
    }

    #[test]
    fn the_dock_leaves_terminals_their_minimum_width() {
        let mut state = AgentViewState::default();
        state.open();
        // The rail's width comes out of what the dock may take.
        assert_eq!(state.physical_width(1000.0 - 200.0, 1.0), 440.0);
        assert_eq!(state.physical_width(700.0 - 200.0, 1.0), 0.0);
    }

    /// Builds the panel's scene as a frame would, with fixed text measures.
    fn build_test_scene(app: &mut App) {
        let colors = Colors::from_theme(&crate::config::theme::Theme::default(), false);
        let new_session = app.agent_view_new_session_info();
        let scene = {
            let sessions = app.agent_view.by_recency();
            let input = layout::Input {
                panel: crate::ui::geometry::Bounds::new(1000.0, 34.0, 440.0, 866.0),
                scale: 1.0,
                now: Instant::now() + std::time::Duration::from_secs(1),
                colors: &colors,
                state: &app.agent_view.state,
                sessions: &sessions,
                suggestions: Default::default(),
                banner: None,
                terminal_agents: &[],
                past: &[],
                supervisor: None,
                new_session,
                home: None,
                allow_bypass: false,
                supervisor_request: None,
            };
            layout::build(
                &input,
                &mut crate::ui::text_layout::FixedMeasure { advance: 7.0 },
            )
        };
        app.agent_view.scene = Some(Arc::new(scene));
    }

    #[test]
    fn a_long_menu_is_searched_by_typing_and_scrolled_by_keys_and_wheel() {
        use crate::agent_view::agent::AgentKind;
        use crate::agent_view::event::ModelChoice;
        use crate::ui::agent_view::scene::{MenuKind, Target};
        use winit::keyboard::{Key, NamedKey};
        let mut app = App::new_headless(crate::config::Config::default()).unwrap();
        let models: Vec<ModelChoice> = (0..30)
            .map(|index| ModelChoice {
                value: format!("model-{index}"),
                display_name: if index == 3 {
                    "Claude Opus 5".into()
                } else {
                    format!("Model {index}")
                },
                description: String::new(),
                group: Some(if index < 15 { "GitHub Copilot" } else { "Z.AI" }.into()),
            })
            .collect();
        let mut transcript = Transcript::new("OpenCode", "build");
        transcript.apply(AgentEvent::Ready(Handshake {
            current_model: Some("model-29".into()),
            models,
            ..Handshake::default()
        }));
        app.agent_view.sessions.push(AgentSession::detached_agent(
            AgentKind::OpenCode,
            PathBuf::from("/work"),
            transcript,
        ));
        app.open_agent_view(true);
        app.agent_view
            .state
            .new_session
            .choose_agent(AgentKind::OpenCode);
        let open = |app: &mut App| {
            build_test_scene(app);
            app.activate_agent_view_target(Target::OpenMenu(MenuKind::Model));
            build_test_scene(app);
        };
        let key = |app: &mut App, key: Key, text: Option<&str>| {
            assert!(app.handle_agent_view_key(&key, text));
            build_test_scene(app);
        };
        let character = |text: &str| Key::Character(text.into());

        // Typing narrows the menu, and Enter takes what is highlighted.
        open(&mut app);
        for letter in ["o", "p", "u", "s"] {
            key(&mut app, character(letter), Some(letter));
        }
        assert_eq!(
            app.agent_view.state.menu.as_ref().unwrap().filter.text(),
            "opus"
        );
        key(&mut app, Key::Named(NamedKey::Enter), None);
        assert!(app.agent_view.state.menu.is_none());
        assert_eq!(
            app.agent_view.state.new_session.model.as_deref(),
            Some("model-3")
        );

        // Escape clears a search before it closes the menu; Backspace edits it.
        open(&mut app);
        key(&mut app, character("z"), Some("z"));
        key(&mut app, character("e\u{301}"), Some("e\u{301}"));
        key(&mut app, Key::Named(NamedKey::Backspace), None);
        assert_eq!(
            app.agent_view.state.menu.as_ref().unwrap().filter.text(),
            "z"
        );
        // The search is one bounded line.
        key(&mut app, character("\u{2028}"), Some("\u{2028}"));
        for _ in 0..200 {
            key(&mut app, character("x"), Some("x"));
        }
        assert_eq!(
            app.agent_view.state.menu.as_ref().unwrap().filter.text(),
            format!("z{}", "x".repeat(127))
        );
        key(&mut app, Key::Named(NamedKey::Escape), None);
        key(&mut app, character("z"), Some("z"));
        key(&mut app, Key::Named(NamedKey::Escape), None);
        let menu = app.agent_view.state.menu.as_ref().unwrap();
        assert!(menu.filter.text().is_empty());
        key(&mut app, Key::Named(NamedKey::Escape), None);
        assert!(app.agent_view.state.menu.is_none());

        // It opens on the checked model, far down; the wheel scrolls from
        // there, and a key down from the last model wraps to the top.
        app.agent_view.state.new_session.model = Some("model-29".into());
        open(&mut app);
        let scene = app.agent_view.scene.clone().unwrap();
        let viewport = scene.menu_viewport.unwrap();
        let opened_at = scene.menu_scroll;
        assert!(opened_at > 0.0);
        assert!(app.scroll_agent_view_menu(viewport.x + 10.0, viewport.y + 10.0, -60.0));
        build_test_scene(&mut app);
        let scrolled = app.agent_view.scene.as_ref().unwrap().menu_scroll;
        assert!(
            (scrolled - (opened_at - 60.0)).abs() < 1.0,
            "{scrolled} {opened_at}"
        );
        assert!(!app.scroll_agent_view_menu(viewport.x + 10.0, viewport.y - 10.0, 60.0));
        key(&mut app, Key::Named(NamedKey::ArrowDown), None);
        let menu = app.agent_view.state.menu.as_ref().unwrap();
        assert_eq!(menu.highlighted, 0);
        let max = scene.menu_height - viewport.height;
        let later = Instant::now() + std::time::Duration::from_secs(1);
        assert_eq!(menu.scroll.offset(max, later), 0.0);
    }
}
