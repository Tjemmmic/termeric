//! The agent view panel: a docked, non-modal side panel for conversations
//! with coding agents driven through their structured protocols.
//!
//! This module holds the panel's own state — whether it is open, how wide,
//! what has keyboard focus, what each conversation's composer holds and how
//! far it is scrolled. The conversations themselves live in
//! `crate::agent_view`; [`layout`] turns both into a [`scene::Scene`].

pub mod colors;
pub mod commands;
pub mod layout;
pub mod scene;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::agent_view::agent::AgentKind;
use crate::agent_view::session::SessionId;
use crate::agent_view::transcript::Attachment;
use crate::ui::text_area::TextArea;

use scene::{Bounds, ExpandKey, MenuKind, Target};

/// Default width in logical pixels.
pub const DEFAULT_WIDTH: f32 = 440.0;
/// Narrowest the panel may be, in logical pixels.
pub const MIN_WIDTH: f32 = 320.0;
/// Room always left for terminals, in logical pixels.
pub const MIN_TERMINAL_WIDTH: f32 = 360.0;
/// How long the panel's contents take to slide in.
const OPEN_DURATION: Duration = Duration::from_millis(200);
/// How long a smooth scroll takes.
const SCROLL_DURATION: Duration = Duration::from_millis(150);
/// The caret stays visible this long after it moves, then blinks.
pub const CARET_HOLD: Duration = Duration::from_millis(600);
/// Each blink phase.
pub const CARET_PHASE: Duration = Duration::from_millis(530);

/// Which page the panel shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Screen {
    Conversation,
    Sessions,
    #[default]
    NewSession,
}

/// How the panel shares its tab with the tab's terminal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Layout {
    /// Beside the terminal.
    #[default]
    Split,
    /// Over the whole tab. The terminal keeps running underneath.
    Full,
}

/// One tab's agent view, kept while another tab shows.
#[derive(Clone, Debug, Default)]
pub struct TabPanel {
    pub open: bool,
    pub layout: Layout,
    /// The tab's conversation.
    pub session: Option<SessionId>,
    pub screen: Screen,
    /// The panel held the keyboard when the tab was left, so the tab's
    /// terminal was last told it lost focus.
    pub keyboard: bool,
    /// The tab's new-session page: its draft, folder, mode, and model.
    pub new_session: NewSessionForm,
}

/// A scroll position that can follow new content and animate.
#[derive(Clone, Debug, PartialEq)]
pub struct Scroll {
    offset: f32,
    /// Stay at the end as content grows.
    follow: bool,
    animation: Option<(f32, f32, Instant)>,
}

impl Default for Scroll {
    fn default() -> Self {
        Self {
            offset: 0.0,
            follow: true,
            animation: None,
        }
    }
}

impl Scroll {
    /// The offset to draw at `now`, within `0..=max`.
    pub fn offset(&self, max: f32, now: Instant) -> f32 {
        if self.follow && self.animation.is_none() {
            return max.max(0.0);
        }
        let offset = match self.animation {
            Some((from, to, started)) => {
                let t = (now.saturating_duration_since(started).as_secs_f32()
                    / SCROLL_DURATION.as_secs_f32())
                .min(1.0);
                let eased = 1.0 - (1.0 - t).powi(3);
                from + (to - from) * eased
            }
            None => self.offset,
        };
        offset.clamp(0.0, max.max(0.0))
    }

    pub fn animating(&self, now: Instant) -> bool {
        self.animation
            .is_some_and(|(_, _, started)| now.saturating_duration_since(started) < SCROLL_DURATION)
    }

    pub fn following(&self) -> bool {
        self.follow
    }

    /// Scrolls by `delta` pixels; reaching the end resumes following.
    pub fn scroll_by(&mut self, delta: f32, max: f32, now: Instant) {
        let current = self.offset(max, now);
        self.jump_to(current + delta, max);
    }

    /// Moves immediately, as a wheel or drag does.
    pub fn jump_to(&mut self, offset: f32, max: f32) {
        let max = max.max(0.0);
        self.offset = offset.clamp(0.0, max);
        self.animation = None;
        self.follow = self.offset >= max - 0.5;
    }

    /// Glides to `offset`, as a key press does.
    pub fn glide_to(&mut self, offset: f32, max: f32, now: Instant) {
        let max = max.max(0.0);
        let from = self.offset(max, now);
        let to = offset.clamp(0.0, max);
        self.offset = to;
        self.follow = to >= max - 0.5;
        self.animation = Some((from, to, now));
    }

    /// Follows the end again.
    pub fn follow_end(&mut self) {
        self.follow = true;
        self.animation = None;
    }
}

/// What the composer's next message means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerIntent {
    /// An ordinary message.
    Message,
    /// The answer to one question the agent asked, typed instead of chosen.
    Answer { request_id: String, question: usize },
}

/// The panel's view of one conversation.
#[derive(Clone, Debug)]
pub struct ConversationView {
    pub composer: TextArea,
    pub attachments: Vec<Attachment>,
    pub scroll: Scroll,
    /// Items whose expansion differs from their default.
    flipped: HashSet<ExpandKey>,
    /// Chosen options per pending question, by request.
    choices: HashMap<String, Vec<BTreeSet<usize>>>,
    /// Typed answers per pending question, by request.
    typed: HashMap<String, HashMap<usize, String>>,
    pub intent: ComposerIntent,
    /// A hint replacing the usual placeholder, e.g. after a denial.
    pub placeholder: Option<&'static str>,
    /// Messages sent, oldest first, for recalling with the Up key.
    history: Vec<String>,
    recall: Option<usize>,
}

impl Default for ConversationView {
    fn default() -> Self {
        Self {
            composer: TextArea::new(),
            attachments: Vec::new(),
            scroll: Scroll::default(),
            flipped: HashSet::new(),
            choices: HashMap::new(),
            typed: HashMap::new(),
            intent: ComposerIntent::Message,
            placeholder: None,
            history: Vec::new(),
            recall: None,
        }
    }
}

impl ConversationView {
    pub fn expanded(&self, key: &ExpandKey, default: bool) -> bool {
        default != self.flipped.contains(key)
    }

    pub fn toggle(&mut self, key: ExpandKey) {
        if !self.flipped.remove(&key) {
            self.flipped.insert(key);
        }
    }

    /// The options chosen for a question.
    pub fn chosen(&self, request_id: &str, question: usize) -> Option<&BTreeSet<usize>> {
        self.choices.get(request_id)?.get(question)
    }

    pub fn typed_answer(&self, request_id: &str, question: usize) -> Option<&str> {
        self.typed
            .get(request_id)?
            .get(&question)
            .map(String::as_str)
    }

    /// Chooses an option; a single-answer question replaces its choice.
    pub fn choose(&mut self, request_id: &str, question: usize, option: usize, multiple: bool) {
        let questions = self.choices.entry(request_id.to_owned()).or_default();
        if questions.len() <= question {
            questions.resize(question + 1, BTreeSet::new());
        }
        let chosen = &mut questions[question];
        if multiple {
            if !chosen.remove(&option) {
                chosen.insert(option);
            }
        } else {
            chosen.clear();
            chosen.insert(option);
        }
        if let Some(typed) = self.typed.get_mut(request_id) {
            typed.remove(&question);
        }
    }

    pub fn set_typed_answer(&mut self, request_id: &str, question: usize, answer: String) {
        if let Some(questions) = self.choices.get_mut(request_id)
            && let Some(chosen) = questions.get_mut(question)
        {
            chosen.clear();
        }
        self.typed
            .entry(request_id.to_owned())
            .or_default()
            .insert(question, answer);
    }

    /// Forgets choices for requests that are no longer pending.
    pub fn retain_requests(&mut self, pending: &[&str]) {
        self.choices
            .retain(|request, _| pending.contains(&request.as_str()));
        self.typed
            .retain(|request, _| pending.contains(&request.as_str()));
        if let ComposerIntent::Answer { request_id, .. } = &self.intent
            && !pending.contains(&request_id.as_str())
        {
            self.intent = ComposerIntent::Message;
        }
    }

    /// Records a sent message for recall.
    pub fn remember_sent(&mut self, text: &str) {
        let text = text.trim();
        if !text.is_empty() && self.history.last().is_none_or(|last| last != text) {
            self.history.push(text.to_owned());
        }
        self.recall = None;
    }

    /// Replaces the composer with an earlier message (`older`) or a later
    /// one. Returns false when there is nothing further to recall.
    pub fn recall(&mut self, older: bool) -> bool {
        let next = match (self.recall, older) {
            (None, true) => self.history.len().checked_sub(1),
            (None, false) => None,
            (Some(index), true) => index.checked_sub(1),
            (Some(index), false) => Some(index + 1),
        };
        match next {
            Some(index) if index < self.history.len() => {
                self.recall = Some(index);
                let text = self.history[index].clone();
                self.composer.set_text(&text);
                true
            }
            Some(_) if !older => {
                self.recall = None;
                self.composer.set_text("");
                true
            }
            _ => false,
        }
    }

    /// Whether Up should recall instead of moving the caret.
    pub fn recalling(&self) -> bool {
        self.recall.is_some()
    }
}

/// The settings for a session about to start.
#[derive(Clone, Debug)]
pub struct NewSessionForm {
    pub composer: TextArea,
    /// `None` uses the focused terminal's directory.
    pub cwd: Option<PathBuf>,
    /// The agent to start.
    pub agent: AgentKind,
    /// The id of the mode to start in; `None` is the agent's default.
    pub mode: Option<String>,
    pub model: Option<String>,
    /// The level of effort to start at; `None` is the agent's.
    pub effort: Option<String>,
    /// Allow everything the agent asks about, where it offers that.
    pub auto_approve: bool,
    pub scroll: Scroll,
}

impl NewSessionForm {
    /// Switches to another agent; the mode and model were that agent's.
    pub fn choose_agent(&mut self, agent: AgentKind) {
        if self.agent != agent {
            self.agent = agent;
            self.mode = None;
            self.model = None;
            self.effort = None;
            self.auto_approve = false;
        }
    }

    /// The id of the mode the agent will start in.
    pub fn mode_id(&self) -> String {
        self.mode
            .clone()
            .unwrap_or_else(|| self.agent.default_mode().to_owned())
    }
}

impl Default for NewSessionForm {
    fn default() -> Self {
        Self {
            composer: TextArea::new(),
            cwd: None,
            agent: AgentKind::default(),
            mode: None,
            model: None,
            effort: None,
            auto_approve: false,
            scroll: Scroll::default(),
        }
    }
}

/// Where keyboard focus is inside the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Focus {
    Composer,
    Target(Target),
}

/// An open menu and the control it drops from.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenMenu {
    pub kind: MenuKind,
    pub anchor: Bounds,
    /// The highlighted item, for keyboard use.
    pub highlighted: usize,
}

/// The agent view panel's state in one window.
#[derive(Debug)]
pub struct AgentViewState {
    open: bool,
    opened_at: Option<Instant>,
    /// Width in logical pixels.
    width: f32,
    /// The panel, not a terminal, receives keys.
    keyboard: bool,
    pub screen: Screen,
    pub active: Option<SessionId>,
    pub focus: Focus,
    /// Focus moved by keyboard, so it shows a ring. Pointer use hides it.
    pub focus_visible: bool,
    pub hover: Option<Target>,
    pub menu: Option<OpenMenu>,
    conversations: HashMap<SessionId, ConversationView>,
    pub new_session: NewSessionForm,
    pub sessions_scroll: Scroll,
    /// A width drag: pointer x and width when it started.
    resize: Option<(f32, f32)>,
    /// When the caret last moved, so it blinks from visible.
    pub caret_moved: Instant,
    /// The `/` command menu above the message box.
    pub commands: commands::CommandMenu,
    /// The tab whose agent view this is. Each tab has its own; the others
    /// wait as [`TabPanel`]s.
    pub tab: Option<usize>,
    pub layout: Layout,
}

impl Default for AgentViewState {
    fn default() -> Self {
        Self {
            open: false,
            opened_at: None,
            width: DEFAULT_WIDTH,
            keyboard: false,
            screen: Screen::NewSession,
            active: None,
            focus: Focus::Composer,
            focus_visible: false,
            hover: None,
            menu: None,
            conversations: HashMap::new(),
            new_session: NewSessionForm::default(),
            sessions_scroll: Scroll::default(),
            resize: None,
            caret_moved: Instant::now(),
            commands: commands::CommandMenu::default(),
            tab: None,
            layout: Layout::Split,
        }
    }
}

impl AgentViewState {
    /// The agent the panel speaks of: the shown conversation's, else the
    /// one chosen on the new-session page.
    pub fn agent(&self, sessions: &[&crate::agent_view::session::AgentSession]) -> AgentKind {
        let shown = match self.screen {
            Screen::Conversation => self
                .active
                .and_then(|id| sessions.iter().find(|session| session.id() == id))
                .filter(|session| !session.is_blank()),
            _ => None,
        };
        shown.map_or(self.new_session.agent, |session| session.agent())
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Takes this tab's panel, to keep while another tab shows.
    pub fn stash(&mut self) -> TabPanel {
        TabPanel {
            open: self.open,
            layout: self.layout,
            session: self.active,
            screen: self.screen,
            keyboard: self.has_keyboard(),
            new_session: std::mem::take(&mut self.new_session),
        }
    }

    /// Shows no tab's panel, as when the window has no tabs.
    pub fn unbind(&mut self) {
        self.restore(0, TabPanel::default());
        self.tab = None;
    }

    /// A conversation's view state, leaving with its session.
    pub fn take_conversation(&mut self, id: SessionId) -> Option<ConversationView> {
        self.conversations.remove(&id)
    }

    /// A conversation's view state, arriving with its session.
    pub fn put_conversation(&mut self, id: SessionId, view: ConversationView) {
        self.conversations.insert(id, view);
    }

    /// Shows `tab`'s panel as it was left, without the opening slide. The
    /// keyboard stays with the terminals unless the panel fills the tab.
    pub fn restore(&mut self, tab: usize, panel: TabPanel) {
        self.tab = Some(tab);
        self.open = panel.open;
        self.opened_at = None;
        self.layout = panel.layout;
        self.active = panel.session;
        self.screen = panel.screen;
        self.new_session = panel.new_session;
        self.keyboard = panel.open && panel.layout == Layout::Full;
        self.focus = Focus::Composer;
        self.menu = None;
        self.hover = None;
        self.resize = None;
        self.commands = commands::CommandMenu::default();
    }

    pub fn open(&mut self) {
        if !self.open {
            self.open = true;
            self.opened_at = Some(Instant::now());
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        self.opened_at = None;
        self.keyboard = false;
        self.menu = None;
        self.hover = None;
        self.resize = None;
    }

    /// Progress of the opening slide, from 0 to 1.
    pub fn open_progress(&self, now: Instant) -> f32 {
        match self.opened_at {
            Some(started) => {
                let t = (now.saturating_duration_since(started).as_secs_f32()
                    / OPEN_DURATION.as_secs_f32())
                .min(1.0);
                1.0 - (1.0 - t).powi(3)
            }
            None if self.open => 1.0,
            None => 0.0,
        }
    }

    pub fn animating(&self, now: Instant) -> bool {
        self.opened_at
            .is_some_and(|started| now.saturating_duration_since(started) < OPEN_DURATION)
    }

    pub fn has_keyboard(&self) -> bool {
        self.open && self.keyboard
    }

    pub fn set_keyboard(&mut self, keyboard: bool) {
        self.keyboard = keyboard && self.open;
        if !self.keyboard {
            self.menu = None;
        }
    }

    /// Width in logical pixels, before fitting to the window.
    #[cfg(test)]
    pub fn width(&self) -> f32 {
        self.width
    }

    /// The panel's width in physical pixels for a window, leaving terminals
    /// at least [`MIN_TERMINAL_WIDTH`]. Zero when closed or too narrow.
    pub fn physical_width(&self, window_width: f32, scale: f32) -> f32 {
        if !self.open {
            return 0.0;
        }
        let available = window_width - MIN_TERMINAL_WIDTH * scale;
        if available < MIN_WIDTH * scale {
            return 0.0;
        }
        (self.width * scale).min(available).round()
    }

    /// Starts a width drag from the width shown in a window `window_width`
    /// physical pixels wide, which may be less than the width asked for.
    pub fn begin_resize(&mut self, pointer_x: f32, window_width: f32, scale: f32) {
        let scale = scale.max(0.1);
        let shown = self.physical_width(window_width, scale) / scale;
        self.width = shown.max(MIN_WIDTH);
        self.resize = Some((pointer_x, self.width));
    }

    pub fn resizing(&self) -> bool {
        self.resize.is_some()
    }

    /// Follows a width drag in a window `window_width` physical pixels wide,
    /// never past what the window leaves room for. Returns whether the width
    /// changed.
    pub fn drag_resize(&mut self, pointer_x: f32, window_width: f32, scale: f32) -> bool {
        let Some((start, width)) = self.resize else {
            return false;
        };
        let scale = scale.max(0.1);
        let widest = (window_width / scale - MIN_TERMINAL_WIDTH).max(MIN_WIDTH);
        // The divider is the panel's left edge: moving left widens it.
        let next = (width + (start - pointer_x) / scale).clamp(MIN_WIDTH, widest);
        let changed = (next - self.width).abs() >= 0.5;
        self.width = next;
        changed
    }

    pub fn end_resize(&mut self) {
        self.resize = None;
    }

    pub fn conversation(&self, id: SessionId) -> Option<&ConversationView> {
        self.conversations.get(&id)
    }

    pub fn conversation_mut(&mut self, id: SessionId) -> &mut ConversationView {
        self.conversations.entry(id).or_default()
    }

    /// Forgets a conversation that ended. If it was showing, the panel
    /// moves to `next`.
    pub fn forget(&mut self, id: SessionId, next: Screen) {
        self.conversations.remove(&id);
        if self.active == Some(id) {
            self.active = None;
            self.screen = next;
            self.menu = None;
            self.focus = Focus::Composer;
        }
    }

    /// Shows a conversation.
    pub fn show(&mut self, id: SessionId) {
        self.active = Some(id);
        self.screen = Screen::Conversation;
        self.focus = Focus::Composer;
        self.menu = None;
        self.conversations.entry(id).or_default();
    }

    pub fn composer_mut(&mut self) -> &mut TextArea {
        match (self.screen, self.active) {
            (Screen::Conversation, Some(id)) => {
                &mut self.conversations.entry(id).or_default().composer
            }
            _ => &mut self.new_session.composer,
        }
    }

    pub fn composer(&self) -> Option<&TextArea> {
        match (self.screen, self.active) {
            (Screen::Conversation, Some(id)) => {
                self.conversations.get(&id).map(|view| &view.composer)
            }
            (Screen::NewSession, _) => Some(&self.new_session.composer),
            _ => None,
        }
    }

    /// Restarts the caret blink after the caret moves.
    pub fn touch_caret(&mut self) {
        self.caret_moved = Instant::now();
    }

    /// When the blinking caret next appears or disappears.
    pub fn next_caret_toggle(&self, now: Instant) -> Instant {
        let elapsed = now.saturating_duration_since(self.caret_moved);
        if elapsed < CARET_HOLD {
            return self.caret_moved + CARET_HOLD;
        }
        let phases = elapsed.as_millis() / CARET_PHASE.as_millis() + 1;
        self.caret_moved + CARET_PHASE * u32::try_from(phases).unwrap_or(u32::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolling_follows_the_end_until_moved_away() {
        let now = Instant::now();
        let mut scroll = Scroll::default();
        assert_eq!(scroll.offset(500.0, now), 500.0);
        scroll.scroll_by(-100.0, 500.0, now);
        assert!(!scroll.following());
        assert_eq!(scroll.offset(900.0, now), 400.0);
        scroll.jump_to(900.0, 900.0);
        assert!(scroll.following());
        scroll.glide_to(0.0, 900.0, now);
        assert!(scroll.animating(now));
        assert_eq!(scroll.offset(900.0, now), 900.0);
        assert_eq!(scroll.offset(900.0, now + SCROLL_DURATION), 0.0);
    }

    #[test]
    fn width_leaves_room_for_terminals_and_hides_when_it_cannot() {
        let mut state = AgentViewState::default();
        assert_eq!(state.physical_width(1440.0, 1.0), 0.0);
        state.open();
        assert_eq!(state.physical_width(1440.0, 1.0), 440.0);
        assert_eq!(state.physical_width(2880.0, 2.0), 880.0);
        assert_eq!(state.physical_width(900.0, 1.0), 440.0);
        assert_eq!(state.physical_width(800.0, 1.0), 440.0);
        assert_eq!(state.physical_width(700.0, 1.0), 340.0);
        assert_eq!(state.physical_width(600.0, 1.0), 0.0);
        state.begin_resize(1000.0, 1440.0, 1.0);
        assert!(state.drag_resize(900.0, 1440.0, 1.0));
        assert_eq!(state.width(), 540.0);
        state.drag_resize(2000.0, 1440.0, 1.0);
        assert_eq!(state.width(), MIN_WIDTH);
        // Dragging far past the window's room stops at it.
        state.drag_resize(-5000.0, 1440.0, 1.0);
        assert_eq!(state.width(), 1440.0 - MIN_TERMINAL_WIDTH);
        state.end_resize();
        assert!(!state.drag_resize(0.0, 1440.0, 1.0));
        // In a narrower window, a new drag starts from the width shown.
        state.begin_resize(500.0, 800.0, 1.0);
        assert_eq!(state.width(), 440.0);
        // It follows the pointer at once, with no dead zone.
        assert!(state.drag_resize(510.0, 800.0, 1.0));
        assert_eq!(state.width(), 430.0);
    }

    #[test]
    fn choices_replace_or_toggle_and_typed_answers_clear_them() {
        let mut view = ConversationView::default();
        view.choose("r", 0, 1, false);
        view.choose("r", 0, 2, false);
        assert_eq!(
            view.chosen("r", 0).unwrap().iter().collect::<Vec<_>>(),
            [&2]
        );
        view.choose("r", 1, 0, true);
        view.choose("r", 1, 3, true);
        view.choose("r", 1, 0, true);
        assert_eq!(
            view.chosen("r", 1).unwrap().iter().collect::<Vec<_>>(),
            [&3]
        );
        view.set_typed_answer("r", 0, "custom".into());
        assert!(view.chosen("r", 0).unwrap().is_empty());
        assert_eq!(view.typed_answer("r", 0), Some("custom"));
        view.intent = ComposerIntent::Answer {
            request_id: "r".into(),
            question: 0,
        };
        view.retain_requests(&[]);
        assert!(view.chosen("r", 1).is_none());
        assert_eq!(view.intent, ComposerIntent::Message);
    }

    #[test]
    fn sent_messages_can_be_recalled_in_order() {
        let mut view = ConversationView::default();
        view.remember_sent("first");
        view.remember_sent("second");
        view.remember_sent("second");
        assert!(!view.recall(false));
        assert!(view.recall(true));
        assert_eq!(view.composer.text(), "second");
        assert!(view.recall(true));
        assert_eq!(view.composer.text(), "first");
        assert!(!view.recall(true));
        assert!(view.recall(false));
        assert_eq!(view.composer.text(), "second");
        assert!(view.recall(false));
        assert_eq!(view.composer.text(), "");
        assert!(!view.recalling());
    }

    #[test]
    fn expansion_flips_its_default() {
        let mut view = ConversationView::default();
        let key = ExpandKey::Tool("t".into());
        assert!(view.expanded(&key, true));
        view.toggle(key.clone());
        assert!(!view.expanded(&key, true));
        assert!(view.expanded(&key, false));
        view.toggle(key.clone());
        assert!(view.expanded(&key, true));
    }
}
