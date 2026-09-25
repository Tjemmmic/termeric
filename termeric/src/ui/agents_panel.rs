//! Agents panel: a slide-in list of this window's agents, the most urgent
//! first, and the supervisor's latest activity.
//!
//! Each row shows what one agent is doing and what it waits for; choosing a
//! row shows that agent's terminal, or its conversation in the agent view. While a supervisor runs, the foot of the panel
//! names its mode, offers Take Over or Hand Back, and lists what it did most
//! recently. Geometry, hit-testing, and keyboard focus all come from
//! [`AgentsPanelLayout`], so what is drawn is what is clicked and announced.

use crate::tabs::TerminalIdentity;
use crate::ui::animation::PanelAnimation;

/// Where an agent in the panel runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentOrigin {
    /// In a terminal pane, watched through its hooks.
    Terminal(TerminalIdentity),
    /// In the docked agent view.
    AgentView(crate::agent_view::session::SessionId),
}

/// What a row's dot shows, matching the tab badges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRowState {
    /// The agent's status hooks are not set up.
    Unknown,
    Idle,
    Working,
    NeedsYou,
    Done,
    Failed,
}

/// One agent, as the panel shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRow {
    pub identity: AgentOrigin,
    pub name: String,
    /// Where it runs: its tab's title.
    pub place: String,
    pub state: AgentRowState,
    /// State, time in it, and what it waits for or answered.
    pub summary: String,
}

/// The supervisor, while one runs in this window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorSummary {
    /// Its mode, as the person set it.
    pub mode: String,
    /// The person took over; it only watches until handed back.
    pub held: bool,
    /// Its activity, newest first, each with how long ago.
    pub entries: Vec<String>,
}

/// A control in the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentsControl {
    Close,
    /// The row at this index of the rows last shown.
    Agent(usize),
    /// Take Over, or Hand Back while taken over.
    Supervisor,
}

/// What a click or key asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentsPanelAction {
    None,
    Close,
    /// Show the agent at the row with this index.
    Show(usize),
    ToggleSupervisor,
}

/// Keys that move focus between controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentsNavigationKey {
    Next,
    Previous,
    First,
    Last,
}

/// Everything the renderer and accessibility need for one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentsPanelInfo {
    /// 0.0 hidden, 1.0 fully shown.
    pub animation: f32,
    pub scroll_offset: usize,
    pub rows: Vec<AgentRow>,
    pub supervisor: Option<SupervisorSummary>,
    pub focus: Option<AgentsControl>,
    pub hovered: Option<AgentsControl>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgentsPanelRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl AgentsPanelRect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }
}

/// Most activity entries the supervisor section shows.
const MAX_SUPERVISOR_ENTRIES: usize = 8;

/// The supervisor section at the foot of the panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SupervisorLayout {
    pub section: AgentsPanelRect,
    /// "Supervisor · Assist", beside the button.
    pub heading: AgentsPanelRect,
    pub button: AgentsPanelRect,
    pub first_entry_y: f32,
    pub entry_height: f32,
    pub entry_count: usize,
}

impl SupervisorLayout {
    pub fn entry(self, index: usize) -> AgentsPanelRect {
        AgentsPanelRect {
            x: self.heading.x,
            y: self.first_entry_y + index as f32 * self.entry_height,
            width: self.section.x + self.section.width - self.heading.x,
            height: self.entry_height,
        }
    }
}

/// Single source of truth for the panel's geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgentsPanelLayout {
    pub panel: AgentsPanelRect,
    pub padding: f32,
    pub title_y: f32,
    pub close_button: AgentsPanelRect,
    pub separator_y: f32,
    pub list_start_y: f32,
    pub row_height: f32,
    pub scroll_offset: usize,
    pub visible_count: usize,
    pub row_count: usize,
    pub supervisor: Option<SupervisorLayout>,
}

impl AgentsPanelLayout {
    /// Geometry of the panel, which slides in from the right edge below the
    /// toolbar (`top`). `supervisor_entries` is `None` without a supervisor.
    #[allow(clippy::too_many_arguments)]
    pub fn calculate(
        window_width: f32,
        window_height: f32,
        top: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
        row_count: usize,
        supervisor_entries: Option<usize>,
    ) -> Self {
        // The other side panels' width, so they line up.
        let panel_width = (cell_width * 44.0).max(380.0).min(window_width * 0.45);
        let panel_x = window_width - panel_width * animation;
        let padding = cell_width * 1.5;
        let title_y = top + cell_height * 0.9;
        let control = cell_height * 1.5;
        let close_button = AgentsPanelRect {
            x: panel_x + panel_width - padding * 0.5 - control,
            y: title_y,
            width: control,
            height: control,
        };
        let separator_y = title_y + control + cell_height * 0.6;
        let list_start_y = separator_y + cell_height * 0.5;
        let bottom = window_height.max(top);
        let supervisor = supervisor_entries.map(|entries| {
            let entry_height = cell_height * 1.3;
            let header = cell_height * 2.4;
            // Leave room for at least two agent rows above the section.
            let room = (bottom - list_start_y - header - cell_height * 6.8).max(0.0);
            let entry_count = entries
                .min(MAX_SUPERVISOR_ENTRIES)
                .min((room / entry_height).floor() as usize);
            let height = header + entry_count as f32 * entry_height + cell_height * 0.8;
            let section = AgentsPanelRect {
                x: panel_x,
                y: (bottom - height).max(list_start_y),
                width: panel_width,
                height,
            };
            let button_width = cell_width * 12.0;
            let button = AgentsPanelRect {
                x: panel_x + panel_width - padding - button_width,
                y: section.y + cell_height * 0.6,
                width: button_width,
                height: control,
            };
            SupervisorLayout {
                section,
                heading: AgentsPanelRect {
                    x: panel_x + padding,
                    y: button.y,
                    width: (button.x - panel_x - padding * 1.5).max(0.0),
                    height: control,
                },
                button,
                first_entry_y: section.y + header,
                entry_height,
                entry_count,
            }
        });
        let list_bottom = supervisor.map_or(bottom, |supervisor| supervisor.section.y);
        let row_height = cell_height * 3.0;
        let visible_count =
            (((list_bottom - list_start_y).max(0.0) / row_height).floor() as usize).min(row_count);
        Self {
            panel: AgentsPanelRect {
                x: panel_x,
                y: top,
                width: panel_width,
                height: (window_height - top).max(0.0),
            },
            padding,
            title_y,
            close_button,
            separator_y,
            list_start_y,
            row_height,
            scroll_offset: 0,
            visible_count,
            row_count,
            supervisor,
        }
    }

    /// Whole rows only, so no clipped row is a hidden click target.
    pub fn scrolled(mut self, offset: usize) -> Self {
        self.scroll_offset = if self.visible_count == 0 {
            0
        } else {
            offset.min(self.row_count - self.visible_count)
        };
        self
    }

    pub fn visible_range(self) -> std::ops::Range<usize> {
        self.scroll_offset..self.scroll_offset + self.visible_count
    }

    pub fn row(self, index: usize) -> AgentsPanelRect {
        let relative = index as f32 - self.scroll_offset as f32;
        AgentsPanelRect {
            x: self.panel.x,
            y: self.list_start_y + relative * self.row_height,
            width: self.panel.width,
            height: self.row_height,
        }
    }

    /// The painted control at a point.
    pub fn control_at(self, x: f32, y: f32) -> Option<AgentsControl> {
        if self.close_button.contains(x, y) {
            return Some(AgentsControl::Close);
        }
        if self
            .supervisor
            .is_some_and(|supervisor| supervisor.button.contains(x, y))
        {
            return Some(AgentsControl::Supervisor);
        }
        self.visible_range()
            .find(|index| {
                let row = self.row(*index);
                row.contains(x, y) && y < row.y + row.height
            })
            .map(AgentsControl::Agent)
    }

    /// What a click at a point asks for. A click beside the panel, or above
    /// it on the toolbar, closes it.
    pub fn click(self, x: f32, y: f32) -> AgentsPanelAction {
        if x < self.panel.x || y < self.panel.y {
            return AgentsPanelAction::Close;
        }
        match self.control_at(x, y) {
            Some(control) => action_for(control),
            None => AgentsPanelAction::None,
        }
    }

    /// The bounds of a control, if it is painted.
    pub fn control_rect(self, control: AgentsControl) -> Option<AgentsPanelRect> {
        match control {
            AgentsControl::Close => Some(self.close_button),
            AgentsControl::Supervisor => self.supervisor.map(|supervisor| supervisor.button),
            AgentsControl::Agent(index) if self.visible_range().contains(&index) => {
                Some(self.row(index))
            }
            AgentsControl::Agent(_) => None,
        }
    }
}

fn action_for(control: AgentsControl) -> AgentsPanelAction {
    match control {
        AgentsControl::Close => AgentsPanelAction::Close,
        AgentsControl::Agent(index) => AgentsPanelAction::Show(index),
        AgentsControl::Supervisor => AgentsPanelAction::ToggleSupervisor,
    }
}

/// The panel's state between frames.
pub struct AgentsPanel {
    animation: PanelAnimation,
    scroll_offset: usize,
    /// Keyboard focus, by the identity of the agent it is on, so it stays
    /// on that agent when urgency reorders the rows.
    focus: Option<Focus>,
    hovered: Option<AgentsControl>,
    /// The agents in the order last shown. Clicks and keys act on what the
    /// person saw, even when the list has changed since.
    shown: Vec<AgentOrigin>,
    /// When the rows were last described, to keep "3 min" current.
    described_at: Option<std::time::Instant>,
}

/// How often an open panel redraws to keep its times current.
const REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Close,
    Agent(AgentOrigin),
    Supervisor,
}

impl Default for AgentsPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentsPanel {
    pub fn new() -> Self {
        Self {
            animation: PanelAnimation::with_duration(0.25),
            scroll_offset: 0,
            focus: None,
            hovered: None,
            shown: Vec::new(),
            described_at: None,
        }
    }

    /// Whether an open panel's times are due for a redraw. The next one is
    /// then counted from now, whether or not the frame is drawn, so a
    /// window the compositor is not showing does not wake the loop again
    /// and again.
    pub fn take_refresh_due(&mut self, now: std::time::Instant) -> bool {
        let due = self.refresh_deadline().is_some_and(|due| due <= now);
        if due {
            self.described_at = Some(now);
        }
        due
    }

    /// When an open panel next needs a redraw to keep its times current.
    pub fn refresh_deadline(&self) -> Option<std::time::Instant> {
        self.is_open()
            .then(|| self.described_at.map(|at| at + REFRESH_INTERVAL))
            .flatten()
    }

    /// Open, with focus on the most urgent agent, or close.
    pub fn toggle(&mut self, rows: &[AgentRow]) {
        if !self.animation.is_open() {
            self.scroll_offset = 0;
            self.hovered = None;
            self.shown = rows.iter().map(|row| row.identity).collect();
            self.focus = Some(
                rows.first()
                    .map_or(Focus::Close, |row| Focus::Agent(row.identity)),
            );
        }
        self.animation.toggle();
    }

    pub fn close(&mut self) {
        if self.animation.is_open() {
            self.animation.close();
        }
    }

    pub fn is_open(&self) -> bool {
        self.animation.is_open()
    }

    pub fn is_closed(&self) -> bool {
        self.animation.is_closed()
    }

    pub fn needs_redraw(&self) -> bool {
        self.animation.needs_redraw()
    }

    pub fn animation_progress(&self) -> f32 {
        self.animation.progress()
    }

    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.animation.set_reduced_motion(reduced_motion);
    }

    pub fn set_hover(&mut self, control: Option<AgentsControl>) {
        self.hovered = control;
    }

    /// The row the last frame showed an agent at.
    pub fn shown_index(&self, identity: AgentOrigin) -> Option<usize> {
        self.shown.iter().position(|shown| *shown == identity)
    }

    /// How many rows the last frame showed.
    pub fn shown_count(&self) -> usize {
        self.shown.len()
    }

    /// The agent a row of the last frame showed.
    pub fn shown_agent(&self, index: usize) -> Option<AgentOrigin> {
        self.shown.get(index).copied()
    }

    /// Record this frame's rows and describe the panel for the renderer.
    /// Focus stays on its agent; if that agent is gone, it moves to the
    /// first row.
    pub fn build_info(
        &mut self,
        rows: Vec<AgentRow>,
        supervisor: Option<SupervisorSummary>,
    ) -> AgentsPanelInfo {
        self.shown = rows.iter().map(|row| row.identity).collect();
        self.described_at = Some(std::time::Instant::now());
        let focus_lost = match self.focus {
            Some(Focus::Agent(identity)) => !self.shown.contains(&identity),
            Some(Focus::Supervisor) => supervisor.is_none(),
            _ => false,
        };
        if focus_lost {
            self.focus = Some(
                self.shown
                    .first()
                    .map_or(Focus::Close, |identity| Focus::Agent(*identity)),
            );
        }
        if let Some(AgentsControl::Agent(index)) = self.hovered
            && index >= rows.len()
        {
            self.hovered = None;
        }
        AgentsPanelInfo {
            animation: self.animation_progress(),
            scroll_offset: self.scroll_offset,
            focus: self.focus_control(),
            hovered: self.hovered,
            rows,
            supervisor,
        }
    }

    fn focus_control(&self) -> Option<AgentsControl> {
        Some(match self.focus? {
            Focus::Close => AgentsControl::Close,
            Focus::Supervisor => AgentsControl::Supervisor,
            Focus::Agent(identity) => {
                AgentsControl::Agent(self.shown.iter().position(|shown| *shown == identity)?)
            }
        })
    }

    /// Every control in focus order: the agents, the supervisor's button,
    /// then Close.
    fn controls(&self, supervisor: bool) -> Vec<Focus> {
        self.shown
            .iter()
            .map(|identity| Focus::Agent(*identity))
            .chain(supervisor.then_some(Focus::Supervisor))
            .chain(std::iter::once(Focus::Close))
            .collect()
    }

    /// Move keyboard focus, and scroll to keep a focused row in view.
    pub fn navigate(&mut self, key: AgentsNavigationKey, layout: AgentsPanelLayout) {
        let controls = self.controls(layout.supervisor.is_some());
        let current = self
            .focus
            .and_then(|focus| controls.iter().position(|control| *control == focus));
        let last = controls.len() - 1;
        let next = match (key, current) {
            (AgentsNavigationKey::First, _) | (AgentsNavigationKey::Next, None) => 0,
            (AgentsNavigationKey::Last, _) | (AgentsNavigationKey::Previous, None) => last,
            (AgentsNavigationKey::Next, Some(index)) => (index + 1) % controls.len(),
            (AgentsNavigationKey::Previous, Some(index)) => (index + last) % controls.len(),
        };
        self.focus = Some(controls[next]);
        self.reveal_focus(layout);
    }

    /// Focus a control directly, as assistive technology may.
    pub fn focus_control_at(&mut self, control: AgentsControl, layout: AgentsPanelLayout) -> bool {
        let focus = match control {
            AgentsControl::Close => Focus::Close,
            AgentsControl::Supervisor if layout.supervisor.is_some() => Focus::Supervisor,
            AgentsControl::Supervisor => return false,
            AgentsControl::Agent(index) => match self.shown_agent(index) {
                Some(identity) => Focus::Agent(identity),
                None => return false,
            },
        };
        self.focus = Some(focus);
        self.reveal_focus(layout);
        true
    }

    fn reveal_focus(&mut self, layout: AgentsPanelLayout) {
        let Some(AgentsControl::Agent(index)) = self.focus_control() else {
            return;
        };
        let current = layout.scrolled(self.scroll_offset);
        if current.visible_count == 0 {
            return;
        }
        self.scroll_offset = if index < current.scroll_offset {
            index
        } else if index >= current.visible_range().end {
            index + 1 - current.visible_count
        } else {
            current.scroll_offset
        };
        self.hovered = None;
    }

    /// What Enter or Space on the focused control asks for. A row scrolled
    /// out of view is brought into view first, never acted on unseen.
    pub fn activate(&mut self, layout: AgentsPanelLayout) -> AgentsPanelAction {
        let Some(control) = self.focus_control() else {
            return AgentsPanelAction::None;
        };
        if let AgentsControl::Agent(index) = control
            && !layout
                .scrolled(self.scroll_offset)
                .visible_range()
                .contains(&index)
        {
            self.reveal_focus(layout);
            return AgentsPanelAction::None;
        }
        action_for(control)
    }

    /// Scroll the rows against the current geometry.
    pub fn scroll_rows(&mut self, delta: isize, layout: AgentsPanelLayout) -> bool {
        if !self.is_open() || delta == 0 || layout.visible_count == 0 {
            return false;
        }
        let current = layout.scrolled(self.scroll_offset).scroll_offset;
        let next = layout
            .scrolled(current.saturating_add_signed(delta))
            .scroll_offset;
        self.scroll_offset = next;
        if next == current {
            return false;
        }
        self.hovered = None;
        true
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panes::PaneId;

    fn row(tab_id: usize, name: &str) -> AgentRow {
        AgentRow {
            identity: AgentOrigin::Terminal(TerminalIdentity {
                tab_id,
                pane_id: PaneId(0),
            }),
            name: name.into(),
            place: format!("tab {tab_id}"),
            state: AgentRowState::Working,
            summary: "Working · now".into(),
        }
    }

    fn layout(rows: usize, supervisor: Option<usize>) -> AgentsPanelLayout {
        AgentsPanelLayout::calculate(1280.0, 800.0, 40.0, 9.0, 18.0, 1.0, rows, supervisor)
    }

    #[test]
    fn clicks_resolve_to_what_is_drawn_and_outside_closes() {
        let layout = layout(3, Some(4));
        let row = layout.row(1);
        assert_eq!(
            layout.click(row.x + 20.0, row.y + row.height / 2.0),
            AgentsPanelAction::Show(1)
        );
        let close = layout.close_button;
        assert_eq!(
            layout.click(close.x + 1.0, close.y + 1.0),
            AgentsPanelAction::Close
        );
        let supervisor = layout.supervisor.unwrap();
        assert_eq!(
            layout.click(supervisor.button.x + 1.0, supervisor.button.y + 1.0),
            AgentsPanelAction::ToggleSupervisor
        );
        // Beside the panel, or on the toolbar above it.
        assert_eq!(layout.click(10.0, 400.0), AgentsPanelAction::Close);
        assert_eq!(
            layout.click(layout.panel.x + 20.0, 10.0),
            AgentsPanelAction::Close
        );
        // Empty panel space does nothing.
        assert_eq!(
            layout.click(
                layout.panel.x + 20.0,
                layout.row(2).y + layout.row_height + 5.0
            ),
            AgentsPanelAction::None
        );
        // The supervisor section sits below the rows it leaves room for.
        assert!(layout.row(layout.visible_count - 1).y + layout.row_height <= supervisor.section.y);
        assert_eq!(supervisor.entry_count, 4);
    }

    #[test]
    fn rows_scroll_whole_and_the_supervisor_leaves_room_for_two() {
        let small = AgentsPanelLayout::calculate(1280.0, 330.0, 40.0, 9.0, 18.0, 1.0, 20, Some(8));
        assert!(small.visible_count >= 2, "{small:?}");
        let scrolled = small.scrolled(100);
        assert_eq!(scrolled.visible_range().end, 20);
        let tall = layout(20, None).scrolled(3);
        assert_eq!(tall.scroll_offset, 3);
        assert!(
            tall.control_rect(AgentsControl::Agent(2)).is_none(),
            "off-screen rows are not targets"
        );
        assert!(tall.control_rect(AgentsControl::Agent(3)).is_some());
    }

    #[test]
    fn focus_follows_its_agent_through_reordering_and_wraps() {
        let mut panel = AgentsPanel::new();
        let rows = vec![row(1, "a"), row(2, "b")];
        panel.toggle(&rows);
        let info = panel.build_info(rows.clone(), None);
        assert_eq!(
            info.focus,
            Some(AgentsControl::Agent(0)),
            "the most urgent first"
        );
        let layout = layout(2, None);
        panel.navigate(AgentsNavigationKey::Next, layout);
        assert_eq!(
            panel.build_info(rows.clone(), None).focus,
            Some(AgentsControl::Agent(1))
        );
        // Urgency swaps the rows; focus stays on the same agent.
        let reordered = vec![row(2, "b"), row(1, "a")];
        let info = panel.build_info(reordered.clone(), None);
        assert_eq!(info.focus, Some(AgentsControl::Agent(0)));
        assert_eq!(panel.shown_agent(0), Some(reordered[0].identity));
        panel.navigate(AgentsNavigationKey::Next, layout);
        panel.navigate(AgentsNavigationKey::Next, layout);
        assert_eq!(
            panel.build_info(reordered.clone(), None).focus,
            Some(AgentsControl::Close)
        );
        panel.navigate(AgentsNavigationKey::Next, layout);
        assert_eq!(
            panel.build_info(reordered.clone(), None).focus,
            Some(AgentsControl::Agent(0)),
            "wraps"
        );
        // Its agent gone, focus moves to the first row.
        let info = panel.build_info(vec![row(1, "a")], None);
        assert_eq!(info.focus, Some(AgentsControl::Agent(0)));
        assert_eq!(panel.activate(layout), AgentsPanelAction::Show(0));
    }

    #[test]
    fn activation_reveals_an_offscreen_row_before_acting_on_it() {
        let mut panel = AgentsPanel::new();
        let rows: Vec<_> = (0..30).map(|tab| row(tab, "agent")).collect();
        panel.toggle(&rows);
        panel.build_info(rows.clone(), None);
        let layout = layout(30, None);
        panel.navigate(AgentsNavigationKey::Previous, layout);
        panel.navigate(AgentsNavigationKey::Previous, layout);
        let info = panel.build_info(rows.clone(), None);
        assert_eq!(info.focus, Some(AgentsControl::Agent(29)));
        assert!(
            layout
                .scrolled(panel.scroll_offset())
                .visible_range()
                .contains(&29)
        );
        assert_eq!(panel.activate(layout), AgentsPanelAction::Show(29));
        assert!(panel.scroll_rows(-100, layout));
        assert_eq!(
            panel.activate(layout),
            AgentsPanelAction::None,
            "revealed first"
        );
        assert_eq!(panel.activate(layout), AgentsPanelAction::Show(29));
    }

    #[test]
    fn an_empty_window_focuses_close() {
        let mut panel = AgentsPanel::new();
        panel.toggle(&[]);
        let info = panel.build_info(Vec::new(), None);
        assert_eq!(info.focus, Some(AgentsControl::Close));
        assert_eq!(panel.activate(layout(0, None)), AgentsPanelAction::Close);
    }
}
