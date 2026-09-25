//! The Agents panel's rows, input, and actions. Its geometry and focus live
//! in `crate::ui::agents_panel`.

use super::*;
use crate::agents::AgentActivity;
use crate::ui::agents_panel::{
    AgentOrigin, AgentRow, AgentRowState, AgentsNavigationKey, AgentsPanelAction, AgentsPanelInfo,
    AgentsPanelLayout,
};
use winit::keyboard::{Key, NamedKey};

impl App {
    /// This window's agents as panel rows, the most urgent first: those in
    /// terminals, then those in the agent view.
    pub(super) fn agents_panel_rows(&self) -> Vec<AgentRow> {
        let mut rows = self.terminal_agent_rows();
        rows.extend(self.agent_view_rows());
        rows
    }

    /// Agents running in terminal panes.
    pub(super) fn terminal_agent_rows(&self) -> Vec<AgentRow> {
        self.agent_terminals()
            .into_iter()
            .map(|terminal| {
                let (state, summary) = match &terminal.status {
                    None => (
                        AgentRowState::Unknown,
                        "Running; set up agent status to see more".to_string(),
                    ),
                    Some(status) => {
                        let state = match status.activity {
                            AgentActivity::Idle => AgentRowState::Idle,
                            AgentActivity::Working => AgentRowState::Working,
                            AgentActivity::NeedsInput => AgentRowState::NeedsYou,
                            AgentActivity::Finished => AgentRowState::Done,
                            AgentActivity::Failed => AgentRowState::Failed,
                        };
                        let what = match status.activity {
                            AgentActivity::NeedsInput | AgentActivity::Failed => {
                                status.detail.clone()
                            }
                            AgentActivity::Finished => status.last_message.as_deref().map(|text| {
                                text.lines()
                                    .map(str::trim)
                                    .find(|line| !line.is_empty())
                                    .unwrap_or_default()
                                    .to_owned()
                            }),
                            AgentActivity::Idle | AgentActivity::Working => None,
                        };
                        let mut summary = format!(
                            "{} · {}",
                            status.activity.label(),
                            super::supervisor::ago(status.since.elapsed())
                        );
                        if let Some(what) = what.filter(|what| !what.is_empty()) {
                            summary.push_str(" · ");
                            summary.push_str(&what);
                        }
                        (state, summary)
                    }
                };
                AgentRow {
                    identity: AgentOrigin::Terminal(terminal.identity),
                    name: terminal.name,
                    place: terminal.tab_title,
                    state,
                    summary,
                }
            })
            .collect()
    }

    /// The panel's description for this frame.
    pub(super) fn agents_panel_info(&mut self) -> AgentsPanelInfo {
        let rows = self.agents_panel_rows();
        let supervisor = self.supervisor_summary();
        self.agents_panel.build_info(rows, supervisor)
    }

    /// Open the panel, closing any other side panel, or close it.
    pub(super) fn toggle_agents_panel(&mut self) {
        if !self.agents_panel.is_open() {
            self.commands_panel.close();
            self.session_panel.close();
            self.close_workspace_menu();
        }
        let rows = self.agents_panel_rows();
        self.agents_panel.toggle(&rows);
        self.request_agents_panel_redraw(redraw_scheduler::RedrawReason::Animation);
    }

    pub(super) fn agents_panel_layout(&self) -> AgentsPanelLayout {
        let (width, height, top, cell_width, cell_height) = self
            .renderer
            .as_ref()
            .map(|renderer| {
                let (width, height) = renderer.window_size();
                (
                    width as f32,
                    height as f32,
                    renderer.tab_bar_height(),
                    renderer.cell_width(),
                    renderer.cell_height(),
                )
            })
            .unwrap_or((800.0, 600.0, 0.0, 8.0, 16.0));
        AgentsPanelLayout::calculate(
            width,
            height,
            top,
            cell_width,
            cell_height,
            self.agents_panel.animation_progress(),
            self.agents_panel.shown_count(),
            self.supervisor_summary()
                .map(|supervisor| supervisor.entries.len()),
        )
        .scrolled(self.agents_panel.scroll_offset())
    }

    /// The panel is modal: while it is open, no key reaches a terminal.
    pub(super) fn handle_agents_panel_key(&mut self, key: &Key) -> bool {
        if !self.agents_panel.is_open() {
            return false;
        }
        if self.alt_graph_active {
            return true;
        }
        let plain = self.modifiers.is_empty();
        let navigation = match key {
            Key::Named(NamedKey::Tab) if self.modifiers == ModifiersState::SHIFT => {
                Some(AgentsNavigationKey::Previous)
            }
            Key::Named(NamedKey::Tab | NamedKey::ArrowDown) if plain => {
                Some(AgentsNavigationKey::Next)
            }
            Key::Named(NamedKey::ArrowUp) if plain => Some(AgentsNavigationKey::Previous),
            Key::Named(NamedKey::Home) if plain => Some(AgentsNavigationKey::First),
            Key::Named(NamedKey::End) if plain => Some(AgentsNavigationKey::Last),
            _ => None,
        };
        let layout = self.agents_panel_layout();
        if let Some(navigation) = navigation {
            self.agents_panel.navigate(navigation, layout);
        } else {
            let action = match key {
                Key::Named(NamedKey::Escape) if plain => AgentsPanelAction::Close,
                Key::Named(NamedKey::Enter | NamedKey::Space) if plain => {
                    self.agents_panel.activate(layout)
                }
                Key::Character(text) if plain && text == " " => self.agents_panel.activate(layout),
                _ => return true,
            };
            self.apply_agents_panel_action(action);
        }
        self.clear_agents_accessibility_focus();
        self.ime.clear_preedit();
        self.request_agents_panel_redraw(redraw_scheduler::RedrawReason::KeyboardInput);
        true
    }

    /// A click anywhere while the panel is open belongs to it.
    pub(super) fn handle_agents_panel_click(&mut self, x: f32, y: f32) {
        let action = self.agents_panel_layout().click(x, y);
        self.apply_agents_panel_action(action);
        self.request_agents_panel_redraw(redraw_scheduler::RedrawReason::PointerInput);
    }

    pub(super) fn update_agents_panel_hover(&mut self, x: f32, y: f32) {
        let control = self.agents_panel_layout().control_at(x, y);
        self.agents_panel.set_hover(control);
        self.request_agents_panel_redraw(redraw_scheduler::RedrawReason::PointerInput);
    }

    /// Wheel events stay with the panel while it is open.
    pub(super) fn handle_agents_panel_scroll(
        &mut self,
        delta: winit::event::MouseScrollDelta,
    ) -> bool {
        if !self.agents_panel.is_open() {
            return false;
        }
        let lines = match delta {
            winit::event::MouseScrollDelta::LineDelta(_, y) => f64::from(y),
            winit::event::MouseScrollDelta::PixelDelta(pos) => pos.y / 20.0,
        };
        if lines.is_finite() {
            let rows = if lines > 0.0 {
                -(lines.ceil() as isize)
            } else {
                (-lines).ceil() as isize
            };
            let layout = self.agents_panel_layout();
            if self.agents_panel.scroll_rows(rows, layout) {
                self.request_agents_panel_redraw(redraw_scheduler::RedrawReason::PointerInput);
            }
        }
        true
    }

    /// One path for pointer, keyboard, and assistive actions.
    pub(super) fn apply_agents_panel_action(&mut self, action: AgentsPanelAction) {
        match action {
            AgentsPanelAction::None => {}
            AgentsPanelAction::Close => self.agents_panel.close(),
            AgentsPanelAction::Show(index) => {
                // The row the person saw, even if the list moved since.
                match self.agents_panel.shown_agent(index) {
                    Some(AgentOrigin::Terminal(identity)) => {
                        self.agents_panel.close();
                        if !self.activate_notification_origin(identity) {
                            self.toasts.info("That agent's terminal has closed");
                        }
                    }
                    Some(AgentOrigin::AgentView(id)) => {
                        self.agents_panel.close();
                        if self.agent_view.session(id).is_some() {
                            self.show_agent_session(id);
                        } else {
                            self.toasts.info("That agent view session has closed");
                        }
                    }
                    None => {}
                }
            }
            AgentsPanelAction::ToggleSupervisor => {
                if self.supervisor_held() {
                    self.hand_back_to_supervisor();
                } else if self.supervisor_running() {
                    self.take_over_from_supervisor();
                }
            }
        }
        self.clear_agents_accessibility_focus();
    }

    fn request_agents_panel_redraw(&mut self, reason: redraw_scheduler::RedrawReason) {
        self.redraw_scheduler.request(reason);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}
