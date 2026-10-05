//! Each tab's own agent view.
//!
//! The panel always shows the active tab's conversation: switching tabs
//! switches the conversation, a tab moved to another window takes its
//! conversation along, and closing a tab ends it. A session also starts
//! ahead of the first message, as soon as the new-session page has the
//! keyboard, so the agent's commands and models are ready to pick.

use std::collections::HashSet;
use std::time::Instant;

use super::*;
use crate::agent_view::agent::AgentKind;
use crate::agent_view::session::{AgentSession, SessionId, StartOptions};
use crate::agent_view::transcript::Activity;
use crate::agents::AgentBadge;
use crate::tabs::TabAgentView;
use crate::ui::agent_view::layout::AgentAvailability;
use crate::ui::agent_view::{ConversationView, Layout, Screen, TabPanel};

/// A tab's agent view on its way to another window.
pub(super) struct AgentViewTransfer {
    panel: TabPanel,
    session: Option<AgentSession>,
    view: Option<ConversationView>,
    tracking: Option<crate::agents::changes::ChangeTracking>,
}

impl App {
    fn active_tab_id(&self) -> Option<usize> {
        self.tabs.as_ref().and_then(TabManager::active_tab_id)
    }

    /// Shows the active tab's agent view, keeping the one it replaces for
    /// its tab. Runs whenever focus moves between tabs.
    pub(super) fn bind_agent_view_to_active_tab(&mut self) {
        let tab = self.active_tab_id();
        if tab == self.agent_view.state.tab {
            return;
        }
        // A session started ahead of a message serves only the page showing
        // it; coming back to the tab starts another.
        if let Some(id) = self.agent_view.state.active
            && self
                .agent_view
                .session(id)
                .is_some_and(AgentSession::is_blank)
        {
            self.end_agent_session(id);
        }
        let window = &mut self.agent_view;
        if let Some(previous) = window.state.tab {
            let panel = window.state.stash();
            window.tabs.insert(previous, panel);
        }
        // What the newly shown terminal was last told about focus: it lost
        // it if its panel held the keyboard when the tab was left.
        let told_unfocused = match tab {
            Some(tab) => {
                let panel = window.tabs.remove(&tab).unwrap_or_default();
                let told_unfocused = panel.keyboard;
                window.state.restore(tab, panel);
                told_unfocused
            }
            None => {
                window.state.unbind();
                false
            }
        };
        self.report_terminal_focus(self.window_focused && !told_unfocused);
        // Only the tab now shown takes the dock's width; the others keep
        // their size until they show.
        self.apply_agent_dock_width();
        self.sync_active_terminal_geometry();
        self.request_redraw(RedrawReason::Action);
    }

    /// Ends the conversations of tabs that closed.
    pub(super) fn reconcile_agent_view_tabs(&mut self) {
        let live: HashSet<usize> = self
            .tabs
            .iter()
            .flat_map(|tabs| tabs.iter().map(|(_, tab)| tab.id))
            .collect();
        let gone: Vec<usize> = self
            .agent_view
            .tabs
            .keys()
            .filter(|tab| !live.contains(tab))
            .copied()
            .collect();
        for tab in gone {
            let panel = self
                .agent_view
                .tabs
                .remove(&tab)
                .expect("the tab was just listed");
            if let Some(id) = panel.session {
                self.end_agent_session(id);
            }
        }
        if let Some(tab) = self.agent_view.state.tab
            && !live.contains(&tab)
        {
            if let Some(id) = self.agent_view.state.active {
                self.end_agent_session(id);
            }
            self.agent_view.state.unbind();
            self.bind_agent_view_to_active_tab();
        }
    }

    /// Ends every tab's conversation, before a new set of tabs replaces
    /// the window's, as when a workspace opens: tab ids start over there, so
    /// nothing may carry over by id.
    pub(super) fn retire_agent_view_tabs(&mut self) {
        let window = &mut self.agent_view;
        let mut ended: Vec<SessionId> = window
            .tabs
            .drain()
            .filter_map(|(_, panel)| panel.session)
            .collect();
        ended.extend(window.state.active);
        window.state.unbind();
        for id in ended {
            self.end_agent_session(id);
        }
    }

    /// The tab a session belongs to.
    pub(super) fn agent_session_tab(&self, id: SessionId) -> Option<usize> {
        let window = &self.agent_view;
        if window.state.active == Some(id) {
            return window.state.tab;
        }
        window
            .tabs
            .iter()
            .find(|(_, panel)| panel.session == Some(id))
            .map(|(tab, _)| *tab)
    }

    /// The active tab's session, when it holds a conversation rather than
    /// having only started ahead of one.
    pub(super) fn agent_view_conversation(&self) -> Option<SessionId> {
        let id = self.agent_view.state.active?;
        self.agent_view
            .session(id)
            .filter(|session| !session.is_blank())
            .map(AgentSession::id)
    }

    /// Opens a shell tab in `cwd` for a new conversation, with the panel in
    /// `layout`. Returns false when no tab could open.
    pub(super) fn open_agent_view_tab(&mut self, cwd: &std::path::Path, layout: Layout) -> bool {
        match self.create_local_tab_in(true, Some(cwd)) {
            Ok(_) => {
                // Opening the tab bound the panel to it.
                self.agent_view.state.layout = layout;
                true
            }
            Err(error) => {
                self.toasts.error(error);
                false
            }
        }
    }

    /// Opens a new tab in the focused terminal's folder with the agent view
    /// on its new-session page, where earlier conversations can also be
    /// continued.
    pub(super) fn open_agent_in_view(&mut self, agent: AgentKind, layout: Layout) {
        let (cwd, _) = self.agent_view_default_cwd();
        if !self.open_agent_view_tab(&cwd, layout) {
            return;
        }
        self.open_agent_view(true);
        self.agent_view.state.screen = Screen::NewSession;
        self.agent_view.state.new_session.cwd = None;
        self.agent_view.state.new_session.choose_agent(agent);
        self.request_redraw(RedrawReason::Action);
    }

    /// Switches how the panel shares the tab with its terminal.
    pub(super) fn set_agent_view_layout(&mut self, layout: Layout) {
        let terminal_had_focus = self.terminal_has_focus();
        self.agent_view.state.layout = layout;
        // Filling the tab hides the terminal, so the panel takes the keys.
        if layout == Layout::Full {
            self.agent_view.state.set_keyboard(true);
        }
        self.report_terminal_focus(terminal_had_focus);
        self.sync_agent_dock();
        self.request_redraw(RedrawReason::Action);
    }

    /// Starts a session ahead of the first message on an open new-session
    /// page, so the agent's commands, models, and earlier conversations are
    /// ready. It follows the page's agent and folder, including a `cd` in
    /// the terminal beside it while the terminal has the keyboard: a
    /// session started for another agent or folder ends first.
    pub(super) fn ensure_agent_view_warm_session(&mut self) {
        let state = &self.agent_view.state;
        if !state.is_open() || state.screen != Screen::NewSession || state.tab.is_none() {
            return;
        }
        let cwd = self.agent_view_new_session_info().cwd;
        let agent = self.agent_view.state.new_session.agent;
        if let Some(id) = self.agent_view.state.active {
            match self.agent_view.session(id) {
                Some(session) if !session.is_blank() => return,
                Some(session) if session.cwd() == cwd && session.agent() == agent => return,
                Some(_) => self.end_agent_session(id),
                None => self.agent_view.state.active = None,
            }
        }
        if self.agent_view.warm_failed.as_ref() == Some(&(agent, cwd.clone()))
            || self.agent_view_availability(agent) != AgentAvailability::Ready
        {
            return;
        }
        let form = &self.agent_view.state.new_session;
        let options = StartOptions {
            agent: form.agent,
            cwd: cwd.clone(),
            mode: form.mode_id(),
            model: form.model.clone().filter(|model| model != "default"),
            effort: form.effort.clone(),
            auto_approve: form.auto_approve,
            resume: None,
            allow_bypass: self.config.agent_view.allow_bypass_permissions,
        };
        match AgentSession::start(
            self.agent_view_program(agent),
            options,
            super::agent_view_event_waker(),
        ) {
            Ok(session) => {
                self.agent_view.state.active = Some(session.id());
                self.agent_view.sessions.push(session);
            }
            Err(error) => {
                // Said once per agent and folder; a message then reports it
                // again.
                log::warn!("agent view: could not start ahead of a message: {error}");
                self.agent_view.warm_failed = Some((agent, cwd));
            }
        }
    }

    /// Why tab `tab` may not move to `destination`: a window has one
    /// supervisor, and the supervisor's tab takes it along; and a
    /// conversation cannot move to a window whose configuration has agent
    /// features off, even while they are held on there.
    pub(super) fn agent_view_tab_move_refusal(
        &self,
        destination: &App,
        tab: usize,
    ) -> Option<String> {
        let supervisor_tab = self
            .supervisor_session()
            .and_then(|id| self.agent_session_tab(id));
        if supervisor_tab == Some(tab) && destination.supervisor_running() {
            return Some("The other window already has a supervisor".to_owned());
        }
        let conversation = self
            .agent_view
            .conversations()
            .any(|session| self.agent_session_tab(session.id()) == Some(tab));
        (conversation && !destination.agent_features_configured())
            .then(|| "Agent features are off in the other window".to_owned())
    }

    /// Moves tab `from`'s conversation to `destination`'s tab `to`, and the
    /// supervisor with it when the tab is the supervisor's.
    pub(super) fn move_agent_view_tab(
        source: &mut App,
        destination: &mut App,
        from: usize,
        to: usize,
    ) {
        let supervisor_moves = source
            .supervisor_session()
            .and_then(|id| source.agent_session_tab(id))
            == Some(from);
        if let Some(transfer) = source.take_agent_view_tab(from) {
            destination.adopt_agent_view_tab(to, transfer);
        }
        if supervisor_moves {
            destination.supervisor = source.supervisor.take();
        }
    }

    /// Takes a tab's agent view out of this window, for the window the tab
    /// moves to.
    pub(super) fn take_agent_view_tab(&mut self, tab: usize) -> Option<AgentViewTransfer> {
        let window = &mut self.agent_view;
        let panel = if window.state.tab == Some(tab) {
            let panel = window.state.stash();
            window.state.unbind();
            panel
        } else {
            window.tabs.remove(&tab)?
        };
        let detached = panel.session.and_then(|id| window.detach_session(id));
        let (session, view, tracking) = match detached {
            Some((session, view, tracking)) => (Some(session), view, tracking),
            None => (None, None, None),
        };
        Some(AgentViewTransfer {
            panel,
            session,
            view,
            tracking,
        })
    }

    /// Adopts a tab's agent view that moved here with tab `tab`.
    pub(super) fn adopt_agent_view_tab(&mut self, tab: usize, transfer: AgentViewTransfer) {
        let features = self.agent_features_enabled();
        let window = &mut self.agent_view;
        let mut panel = transfer.panel;
        match transfer.session {
            // With agent features off here, only a blank session can have
            // come along, and it ends with the panel it served.
            Some(_) if !features => panel.session = None,
            Some(session) => window.attach_session(session, transfer.view, transfer.tracking),
            None => panel.session = None,
        }
        panel.open &= features;
        window.tabs.insert(tab, panel);
    }

    /// Shows the tab bar each tab's conversation: its state as a badge, and
    /// its title while the agent view fills the tab.
    pub(super) fn sync_agent_view_tab_bar(&mut self) {
        let now = Instant::now();
        // A message the supervisor holds for the person needs them too.
        let waiting = self.supervisor_request().map(|request| request.session);
        let window = &mut self.agent_view;
        // What the person is looking at counts as seen.
        if window.state.is_open()
            && window.state.screen == Screen::Conversation
            && self.window_focused
            && let Some(id) = window.state.active
        {
            window.seen.insert(id, now);
        }
        // Each tab's panel: open, layout, and conversation.
        let mut panels: Vec<(usize, bool, Layout, Option<SessionId>)> = window
            .tabs
            .iter()
            .map(|(tab, panel)| (*tab, panel.open, panel.layout, panel.session))
            .collect();
        if let Some(tab) = window.state.tab {
            let state = &window.state;
            panels.push((tab, state.is_open(), state.layout, state.active));
        }
        let shown: std::collections::HashMap<usize, TabAgentView> = panels
            .into_iter()
            .filter_map(|(tab, open, layout, session)| {
                let session = window.session(session?)?;
                if session.is_blank() && layout == Layout::Split {
                    return None;
                }
                let unseen = window
                    .seen
                    .get(&session.id())
                    .is_none_or(|seen| session.last_change() > *seen);
                let badge = match session.activity() {
                    _ if waiting == Some(session.id()) => Some(AgentBadge::NeedsYou),
                    Activity::Working | Activity::Starting if !session.is_blank() => {
                        Some(AgentBadge::Working)
                    }
                    Activity::NeedsYou => Some(AgentBadge::NeedsYou),
                    Activity::Finished { success: true } if unseen => Some(AgentBadge::Done),
                    Activity::Finished { success: false } if unseen => Some(AgentBadge::Failed),
                    _ => None,
                };
                let title = (open && layout == Layout::Full).then(|| {
                    if session.is_blank() {
                        session.agent().name().to_owned()
                    } else {
                        session.title()
                    }
                });
                Some((tab, TabAgentView { title, badge }))
            })
            .collect();
        let live: HashSet<SessionId> = window.sessions.iter().map(AgentSession::id).collect();
        window.seen.retain(|id, _| live.contains(id));
        let Some(tabs) = &mut self.tabs else {
            return;
        };
        for tab in tabs.all_tabs_mut().iter_mut() {
            let view = shown.get(&tab.id).cloned().unwrap_or_default();
            if tab.agent_view != view {
                tab.agent_view = view;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_view::event::{AgentEvent, Handshake};
    use crate::agent_view::transcript::Transcript;
    use crate::config::{Config, ShellConfig};

    /// A window with two tabs running a quiet shell, the first one active.
    fn two_tabs(config: Config) -> App {
        let mut app = App::new_headless(config).unwrap();
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 30".into()],
            env: Default::default(),
        };
        let mut tabs = TabManager::new(80, 24, shell, 100, 1024 * 1024);
        tabs.new_tab().unwrap();
        tabs.new_tab().unwrap();
        tabs.switch_to(0);
        app.tabs = Some(tabs);
        app
    }

    fn tab_ids(app: &App) -> Vec<usize> {
        app.tabs
            .as_ref()
            .unwrap()
            .iter()
            .map(|(_, tab)| tab.id)
            .collect()
    }

    fn switch(app: &mut App, index: usize) {
        app.tabs.as_mut().unwrap().switch_to(index);
        app.refresh_focused_terminal_surfaces();
    }

    /// Shows a new conversation in the active tab's panel.
    fn converse(app: &mut App, prompt: &str) -> SessionId {
        let mut transcript = Transcript::new("Claude Code", "default");
        transcript.apply(AgentEvent::Ready(Handshake::default()));
        transcript.push_user(prompt.into(), Vec::new());
        let session = AgentSession::detached(std::env::temp_dir(), transcript);
        let id = session.id();
        app.agent_view.sessions.push(session);
        app.open_agent_view(true);
        app.agent_view.state.show(id);
        id
    }

    #[test]
    fn each_tab_shows_its_own_conversation() {
        let mut app = two_tabs(Config::default());
        let tabs = tab_ids(&app);
        let first = converse(&mut app, "first tab's work");

        switch(&mut app, 1);
        assert!(!app.agent_view.state.is_open());
        assert_eq!(app.agent_view.state.active, None);
        let second = converse(&mut app, "second tab's work");

        switch(&mut app, 0);
        assert!(app.agent_view.state.is_open());
        assert_eq!(app.agent_view.state.active, Some(first));
        assert_eq!(app.agent_view.state.screen, Screen::Conversation);
        assert_eq!(app.agent_session_tab(second), Some(tabs[1]));

        // Showing another tab's conversation goes to that tab.
        app.show_agent_session(second);
        assert_eq!(app.tabs.as_ref().unwrap().active_index(), 1);
        assert_eq!(app.agent_view.state.active, Some(second));
        assert_eq!(app.agent_session_tab(first), Some(tabs[0]));
    }

    #[test]
    fn a_new_conversation_in_a_busy_tab_gets_a_tab_of_its_own() {
        let mut app = two_tabs(Config::default());
        let first = converse(&mut app, "first");
        app.new_agent_session_page();
        assert_eq!(tab_ids(&app).len(), 3);
        assert_eq!(app.agent_view.state.screen, Screen::NewSession);
        assert_eq!(app.agent_view.state.active, None);
        assert_eq!(app.agent_session_tab(first), Some(tab_ids(&app)[0]));
    }

    #[test]
    fn closing_a_tab_ends_its_conversation() {
        let mut app = two_tabs(Config::default());
        let first = converse(&mut app, "first");
        switch(&mut app, 1);
        let closing = tab_ids(&app)[0];
        assert!(app.tabs.as_mut().unwrap().close_tab_by_id(closing));
        app.tabs.as_mut().unwrap().set_reduced_motion(true);
        app.tabs.as_mut().unwrap().cleanup_closed(false, false);
        assert_eq!(tab_ids(&app).len(), 1);
        app.reconcile_agent_view_tabs();
        assert!(app.agent_view.session(first).is_none());
        assert!(app.agent_view.tabs.is_empty());
    }

    #[test]
    fn a_tab_moving_windows_takes_its_conversation() {
        let mut source = two_tabs(Config::default());
        let tab = tab_ids(&source)[0];
        let first = converse(&mut source, "first");
        source
            .agent_view
            .state
            .composer_mut()
            .insert("unsent draft");
        let transfer = source.take_agent_view_tab(tab).unwrap();
        assert!(source.agent_view.session(first).is_none());

        let mut destination = App::new_headless(Config::default().with_agent_features()).unwrap();
        destination.adopt_agent_view_tab(42, transfer);
        assert!(destination.agent_view.session(first).is_some());
        assert_eq!(destination.agent_session_tab(first), Some(42));
        let draft = destination
            .agent_view
            .state
            .conversation(first)
            .map(|view| view.composer.text().to_owned());
        assert_eq!(draft.as_deref(), Some("unsent draft"));
    }

    #[test]
    fn each_tab_keeps_its_own_new_session_page() {
        let mut app = two_tabs(Config::default());
        app.open_agent_view(true);
        app.agent_view
            .state
            .new_session
            .composer
            .insert("draft for tab one");
        switch(&mut app, 1);
        app.open_agent_view(true);
        assert!(app.agent_view.state.new_session.composer.is_empty());
        switch(&mut app, 0);
        assert_eq!(
            app.agent_view.state.new_session.composer.text(),
            "draft for tab one"
        );
    }

    #[test]
    fn opening_a_workspace_ends_the_old_tabs_conversations() {
        let mut app = two_tabs(Config::default());
        let first = converse(&mut app, "first");
        switch(&mut app, 1);
        let second = converse(&mut app, "second");
        app.retire_agent_view_tabs();
        assert!(app.agent_view.session(first).is_none());
        assert!(app.agent_view.session(second).is_none());
        assert!(app.agent_view.tabs.is_empty());
        assert_eq!(app.agent_view.state.tab, None);
    }

    #[test]
    fn a_press_outside_a_panel_filling_the_tab_keeps_the_keyboard() {
        let mut app = two_tabs(Config::default());
        converse(&mut app, "first");
        app.set_agent_view_layout(Layout::Full);
        // Without a renderer every press is outside the panel, like one on
        // the tab bar.
        app.handle_agent_view_press(5.0, 5.0, winit::event::MouseButton::Left);
        assert!(app.agent_view.state.has_keyboard());
        app.set_agent_view_layout(Layout::Split);
        app.handle_agent_view_press(5.0, 5.0, winit::event::MouseButton::Left);
        assert!(!app.agent_view.state.has_keyboard());
    }

    #[test]
    fn filling_the_tab_takes_the_keyboard_and_names_the_tab() {
        let mut app = two_tabs(Config::default());
        let first = converse(&mut app, "Fix the flaky test\nwith details");
        app.set_agent_view_layout(Layout::Full);
        assert!(app.agent_view_fills_tab());
        assert!(app.agent_view.state.has_keyboard());
        app.sync_agent_view_tab_bar();
        let title = |app: &App, index: usize| {
            app.tabs.as_ref().unwrap().all_tabs()[index]
                .display_title()
                .to_owned()
        };
        assert_eq!(title(&app, 0), "Fix the flaky test");

        // Another tab keeps its terminal; coming back keeps the layout.
        switch(&mut app, 1);
        assert!(!app.agent_view_fills_tab());
        switch(&mut app, 0);
        assert!(app.agent_view_fills_tab());
        assert!(app.agent_view.state.has_keyboard());
        assert_eq!(app.agent_view.state.active, Some(first));

        app.set_agent_view_layout(Layout::Split);
        app.sync_agent_view_tab_bar();
        assert_ne!(title(&app, 0), "Fix the flaky test");
    }

    #[test]
    fn a_session_started_ahead_stays_out_of_every_list_until_its_first_message() {
        let fake = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agent_view/fake_claude.py");
        assert!(
            crate::agents::find_executable("python3").is_some(),
            "python3 is required by this test"
        );
        let mut config = Config::default();
        config.agents.insert(
            "claude-code".into(),
            crate::agents::AgentConfig {
                program: Some(fake.display().to_string()),
                env: [("FAKE_CLAUDE_DELAY".to_owned(), "0".to_owned())].into(),
                ..Default::default()
            },
        );
        let mut app = two_tabs(config);
        app.open_agent_view(true);
        app.ensure_agent_view_warm_session();
        let warm = app
            .agent_view
            .state
            .active
            .expect("a session started ahead");
        assert!(app.agent_view.session(warm).unwrap().is_blank());
        assert_eq!(app.agent_view.state.screen, Screen::NewSession);
        assert!(
            app.control_agents(1)
                .iter()
                .all(|agent| agent.session_id != Some(warm.control_id()))
        );
        assert!(app.agent_view_rows().is_empty());

        // Hiding the panel ends a session that never got a message.
        app.close_agent_view();
        assert!(app.agent_view.session(warm).is_none());

        // The first message goes to the session started ahead.
        app.open_agent_view(true);
        app.ensure_agent_view_warm_session();
        let warm = app.agent_view.state.active.unwrap();
        app.agent_view
            .state
            .new_session
            .composer
            .insert("hello there");
        assert!(app.start_agent_session(None));
        assert_eq!(app.agent_view.state.active, Some(warm));
        assert_eq!(app.agent_view.state.screen, Screen::Conversation);
        assert!(!app.agent_view.session(warm).unwrap().is_blank());
        assert_eq!(app.agent_view.sessions.len(), 1);
    }

    #[test]
    fn choosing_another_agent_starts_it_ahead_instead() {
        assert!(
            crate::agents::find_executable("python3").is_some(),
            "python3 is required by this test"
        );
        let fixture = |name: &str| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/agent_view")
                .join(name)
                .display()
                .to_string()
        };
        let mut config = Config::default();
        for (id, script, delay) in [
            ("claude-code", "fake_claude.py", "FAKE_CLAUDE_DELAY"),
            ("codex", "fake_codex.py", "FAKE_CODEX_DELAY"),
        ] {
            config.agents.insert(
                id.into(),
                crate::agents::AgentConfig {
                    program: Some(fixture(script)),
                    env: [(delay.to_owned(), "0".to_owned())].into(),
                    ..Default::default()
                },
            );
        }
        let mut app = two_tabs(config);
        app.open_agent_view(true);
        app.ensure_agent_view_warm_session();
        let claude = app.agent_view.state.active.unwrap();
        assert_eq!(
            app.agent_view.session(claude).unwrap().agent(),
            AgentKind::ClaudeCode
        );

        app.activate_agent_view_target(crate::ui::agent_view::scene::Target::ChooseAgent(
            AgentKind::Codex,
        ));
        app.ensure_agent_view_warm_session();
        assert!(app.agent_view.session(claude).is_none());
        let codex = app.agent_view.state.active.unwrap();
        assert_eq!(
            app.agent_view.session(codex).unwrap().agent(),
            AgentKind::Codex
        );

        app.agent_view
            .state
            .new_session
            .composer
            .insert("hello there");
        assert!(app.start_agent_session(None));
        assert_eq!(app.agent_view.state.active, Some(codex));
        let row = &app.agent_view_rows()[0];
        assert_eq!(row.name, "Codex · hello there");
        let summary = &app.agent_view_summaries(1)[0];
        assert_eq!(
            (summary.agent_id.as_str(), summary.name.as_str()),
            ("codex", "Codex")
        );

        // Another conversation from this tab starts with the same agent.
        app.new_agent_session_page();
        assert_eq!(app.agent_view.state.new_session.agent, AgentKind::Codex);
    }

    #[test]
    fn earlier_codex_conversations_are_offered_and_continued_with_codex() {
        assert!(
            crate::agents::find_executable("python3").is_some(),
            "python3 is required by this test"
        );
        let fake = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agent_view/fake_codex.py");
        let mut config = Config::default();
        config.agents.insert(
            "codex".into(),
            crate::agents::AgentConfig {
                program: Some(fake.display().to_string()),
                env: [("FAKE_CODEX_DELAY".to_owned(), "0".to_owned())].into(),
                ..Default::default()
            },
        );
        let mut app = two_tabs(config);
        app.open_agent_view(true);
        app.agent_view
            .state
            .new_session
            .choose_agent(AgentKind::Codex);
        app.ensure_agent_view_warm_session();
        let cwd = app.agent_view_new_session_info().cwd;
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        let past = loop {
            app.process_agent_view_events();
            let past = app.agent_view_past(AgentKind::Codex, &cwd, false);
            if !past.is_empty() {
                break past;
            }
            assert!(Instant::now() < deadline, "Codex never listed its threads");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(past[0].agent, AgentKind::Codex);
        assert_eq!(past[0].title, "Fix the flaky test");
        // Hiding the panel ends the session started ahead; the list stays.
        app.close_agent_view();
        assert!(app.agent_view.sessions.is_empty());
        assert_eq!(app.agent_view_past(AgentKind::Codex, &cwd, false), past);
        app.open_agent_view(true);
        // Claude Code's logs say nothing about Codex's conversations.
        assert!(
            app.agent_view_past(AgentKind::ClaudeCode, &cwd, false)
                .iter()
                .all(|past| past.id != "earlier-named")
        );

        app.activate_agent_view_target(crate::ui::agent_view::scene::Target::ResumePast {
            agent: AgentKind::Codex,
            id: "earlier-named".into(),
        });
        let id = app.agent_view.state.active.unwrap();
        let session = app.agent_view.session(id).unwrap();
        assert_eq!(session.agent(), AgentKind::Codex);
        assert_eq!(session.conversation_id(), Some("earlier-named"));
        assert_eq!(session.title(), "Fix the flaky test");
        // An open conversation is listed with the sessions, not as earlier.
        assert!(
            app.agent_view_past(AgentKind::Codex, &cwd, false)
                .iter()
                .all(|past| past.id != "earlier-named")
        );
    }

    #[test]
    fn the_session_started_ahead_follows_the_folder_while_the_terminal_types() {
        assert!(
            crate::agents::find_executable("python3").is_some(),
            "python3 is required by this test"
        );
        let fake = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agent_view/fake_claude.py");
        let mut config = Config::default();
        config.agents.insert(
            "claude-code".into(),
            crate::agents::AgentConfig {
                program: Some(fake.display().to_string()),
                env: [("FAKE_CLAUDE_DELAY".to_owned(), "0".to_owned())].into(),
                ..Default::default()
            },
        );
        let mut app = two_tabs(config);
        // Open beside the terminal, which keeps the keyboard.
        app.open_agent_view(false);
        assert!(!app.agent_view.state.has_keyboard());
        app.ensure_agent_view_warm_session();
        let first = app
            .agent_view
            .state
            .active
            .expect("a session started ahead");
        let folder = tempfile::tempdir().unwrap();
        app.agent_view.state.new_session.cwd = Some(folder.path().to_path_buf());
        app.ensure_agent_view_warm_session();
        let second = app.agent_view.state.active.unwrap();
        assert_ne!(first, second);
        assert!(app.agent_view.session(first).is_none());
        assert_eq!(app.agent_view.session(second).unwrap().cwd(), folder.path());
    }
}
