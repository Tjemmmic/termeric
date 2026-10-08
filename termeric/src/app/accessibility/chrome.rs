//! F6 keyboard focus through the window chrome: the focus ring's order,
//! moving it, and activating the control it is on.

use super::*;

impl App {
    pub(in crate::app) fn chrome_focus_render_target(
        &self,
    ) -> Option<crate::renderer::TabBarClick> {
        self.accessibility.keyboard_focus.render_target()
    }

    pub(in crate::app) fn handle_chrome_keyboard(
        &mut self,
        key: &winit::keyboard::Key,
        state: ElementState,
    ) -> bool {
        use winit::keyboard::{Key, NamedKey};

        let is_f6 = matches!(key, Key::Named(NamedKey::F6));
        if state != ElementState::Pressed {
            return self.accessibility.keyboard_focus != ChromeFocus::Terminal && !is_f6;
        }

        let sequence = self.chrome_focus_sequence();
        if is_f6 {
            let next = if self.modifiers.shift_key() {
                sequence.last().copied().unwrap_or(ChromeFocus::Terminal)
            } else {
                sequence.first().copied().unwrap_or(ChromeFocus::Terminal)
            };
            if self.accessibility.keyboard_focus == ChromeFocus::Terminal {
                self.set_chrome_focus(next);
            } else {
                self.move_chrome_focus(&sequence, self.modifiers.shift_key());
            }
            return true;
        }
        if self.accessibility.keyboard_focus == ChromeFocus::Terminal {
            return false;
        }

        match key {
            Key::Named(NamedKey::Escape) => self.set_chrome_focus(ChromeFocus::Terminal),
            Key::Named(NamedKey::Tab) => {
                self.move_chrome_focus(&sequence, self.modifiers.shift_key())
            }
            Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp) => {
                self.move_chrome_focus(&sequence, true)
            }
            Key::Named(NamedKey::ArrowRight | NamedKey::ArrowDown) => {
                self.move_chrome_focus(&sequence, false)
            }
            Key::Named(NamedKey::Home) => {
                if let Some(first) = sequence.first().copied() {
                    self.set_chrome_focus(first);
                }
            }
            Key::Named(NamedKey::End) => {
                if let Some(last) = sequence.last().copied() {
                    self.set_chrome_focus(last);
                }
            }
            Key::Named(NamedKey::Enter | NamedKey::Space) => self.activate_chrome_focus(),
            _ => {}
        }
        true
    }

    pub(super) fn chrome_focus_sequence(&self) -> Vec<ChromeFocus> {
        let count = self.tabs.as_ref().map(TabManager::count).unwrap_or(0);
        let mut sequence = Vec::with_capacity(count * 2 + 4);
        for index in 0..count {
            sequence.push(ChromeFocus::Tab(index));
            sequence.push(ChromeFocus::CloseTab(index));
        }
        let tabs = self
            .tabs
            .as_ref()
            .map(TabManager::tab_info)
            .unwrap_or_default();
        let (width, height) = self
            .renderer
            .as_ref()
            .map(|renderer| {
                let (width, height) = renderer.window_size();
                (width as f32, height as f32)
            })
            .unwrap_or((0.0, 0.0));
        if self.renderer.as_ref().is_some_and(|renderer| {
            renderer
                .tab_bar_element_rect(
                    crate::renderer::TabBarClick::PreviousTab,
                    &tabs,
                    width,
                    height,
                )
                .is_some()
        }) {
            sequence.push(ChromeFocus::PreviousTab);
        }
        if self.renderer.as_ref().is_some_and(|renderer| {
            renderer
                .tab_bar_element_rect(crate::renderer::TabBarClick::NextTab, &tabs, width, height)
                .is_some()
        }) {
            sequence.push(ChromeFocus::NextTab);
        }
        sequence.extend([
            ChromeFocus::NewTab,
            ChromeFocus::Commands,
            ChromeFocus::Workspaces,
            ChromeFocus::Settings,
        ]);
        sequence
    }

    fn move_chrome_focus(&mut self, sequence: &[ChromeFocus], backwards: bool) {
        let Some(current) = sequence
            .iter()
            .position(|focus| *focus == self.accessibility.keyboard_focus)
        else {
            self.set_chrome_focus(ChromeFocus::Terminal);
            return;
        };
        let next = if backwards {
            current.checked_sub(1).unwrap_or(sequence.len() - 1)
        } else {
            (current + 1) % sequence.len()
        };
        self.set_chrome_focus(sequence[next]);
    }

    pub(super) fn set_chrome_focus(&mut self, focus: ChromeFocus) {
        self.accessibility.keyboard_focus = focus;
        self.accessibility.focus_override = focus.node();
        self.ime.clear_preedit();
        if let Some(renderer) = &mut self.renderer {
            renderer.set_tab_bar_keyboard_focus(focus.render_target());
        }
        self.request_redraw(RedrawReason::Accessibility);
        self.refresh_accessibility();
    }

    /// Call after normal pointer dispatch: the keyboard-anchored tab viewport
    /// must remain intact until hit testing has resolved the clicked target.
    pub(in crate::app) fn finish_pointer_chrome_focus(
        &mut self,
        button: MouseButton,
        state: ElementState,
    ) {
        if button == MouseButton::Left
            && state == ElementState::Pressed
            && self.accessibility.keyboard_focus != ChromeFocus::Terminal
        {
            self.set_chrome_focus(ChromeFocus::Terminal);
        }
    }

    pub(super) fn activate_chrome_focus(&mut self) {
        match self.accessibility.keyboard_focus {
            ChromeFocus::Terminal => {}
            ChromeFocus::Tab(index) => {
                self.handle_action(crate::input::Action::SwitchToTab(index));
            }
            ChromeFocus::CloseTab(index) => {
                self.handle_action(crate::input::Action::SwitchToTab(index));
                self.handle_action(crate::input::Action::CloseTab);
                self.set_chrome_focus(ChromeFocus::Terminal);
                return;
            }
            ChromeFocus::PreviousTab => {
                self.handle_action(crate::input::Action::PrevTab);
                self.set_chrome_focus(ChromeFocus::Terminal);
                return;
            }
            ChromeFocus::NextTab => {
                self.handle_action(crate::input::Action::NextTab);
                self.set_chrome_focus(ChromeFocus::Terminal);
                return;
            }
            ChromeFocus::NewTab => {
                self.handle_action(crate::input::Action::NewTab);
                self.set_chrome_focus(ChromeFocus::Terminal);
                return;
            }
            ChromeFocus::Workspaces => self.toggle_workspace_menu(true),
            ChromeFocus::Commands => self.commands_panel.toggle(),
            ChromeFocus::Settings => self.toggle_settings_surface(),
        }
        if !matches!(self.accessibility.keyboard_focus, ChromeFocus::Tab(_)) {
            self.set_chrome_focus(ChromeFocus::Terminal);
        } else {
            self.request_redraw(RedrawReason::Accessibility);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ChromeFocus {
    #[default]
    Terminal,
    Tab(usize),
    CloseTab(usize),
    PreviousTab,
    NextTab,
    NewTab,
    Workspaces,
    Commands,
    Settings,
}

impl ChromeFocus {
    pub(super) fn node(self) -> Option<NodeId> {
        match self {
            Self::Terminal => None,
            Self::Tab(index) => Some(tab_node(index)),
            Self::CloseTab(index) => Some(tab_close_node(index)),
            Self::PreviousTab => Some(PREVIOUS_TAB),
            Self::NextTab => Some(NEXT_TAB),
            Self::NewTab => Some(NEW_TAB),
            Self::Workspaces => Some(WORKSPACES),
            Self::Commands => Some(COMMANDS),
            Self::Settings => Some(SETTINGS),
        }
    }

    fn render_target(self) -> Option<crate::renderer::TabBarClick> {
        use crate::renderer::TabBarClick;
        match self {
            Self::Terminal => None,
            Self::Tab(index) => Some(TabBarClick::Tab(index)),
            Self::CloseTab(index) => Some(TabBarClick::CloseTab(index)),
            Self::PreviousTab => Some(TabBarClick::PreviousTab),
            Self::NextTab => Some(TabBarClick::NextTab),
            Self::NewTab => Some(TabBarClick::NewTab),
            Self::Workspaces => Some(TabBarClick::WorkspacesMenu),
            Self::Commands => Some(TabBarClick::CommandsButton),
            Self::Settings => Some(TabBarClick::Gear),
        }
    }
}
