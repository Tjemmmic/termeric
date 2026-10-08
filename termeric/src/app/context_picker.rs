//! The context picker: choosing what to send an agent, gathering it off the
//! event loop, and typing it into the agent's terminal.

use super::*;

impl App {
    /// The context picker's geometry, shared by drawing, pointer hits, and
    /// accessibility.
    pub(super) fn context_picker_layout(&self) -> crate::ui::context_picker::ContextPickerLayout {
        let metrics = self.ui_metrics();
        crate::ui::context_picker::ContextPickerLayout::calculate(
            metrics.width,
            metrics.height,
            metrics.cell_width,
            metrics.cell_height,
            self.context_picker.animation(),
            self.context_picker.phase(),
            self.context_picker.sources().len(),
        )
    }

    /// Keys for the context picker: choosing sources, then typing file paths.
    pub(super) fn handle_context_picker_key(&mut self, key: &winit::keyboard::Key) {
        use crate::ui::context_picker::{ContextPickerAction, ContextPickerPhase};
        use winit::keyboard::{Key, NamedKey};
        let request_id = self.context_picker.request_id();
        let action = match (self.context_picker.phase(), key) {
            (_, Key::Named(NamedKey::Escape)) => ContextPickerAction::Cancel { request_id },
            (_, Key::Named(NamedKey::Enter)) => ContextPickerAction::Primary { request_id },
            (ContextPickerPhase::Sources, Key::Named(NamedKey::ArrowUp)) => {
                ContextPickerAction::Move {
                    request_id,
                    delta: -1,
                }
            }
            (ContextPickerPhase::Sources, Key::Named(NamedKey::ArrowDown)) => {
                ContextPickerAction::Move {
                    request_id,
                    delta: 1,
                }
            }
            (ContextPickerPhase::Sources, Key::Named(NamedKey::PageUp)) => {
                ContextPickerAction::Move {
                    request_id,
                    delta: -(self.context_picker.viewport_rows() as isize),
                }
            }
            (ContextPickerPhase::Sources, Key::Named(NamedKey::PageDown)) => {
                ContextPickerAction::Move {
                    request_id,
                    delta: self.context_picker.viewport_rows() as isize,
                }
            }
            (ContextPickerPhase::Sources, Key::Named(NamedKey::Space)) => self
                .context_picker
                .selected_source()
                .map_or(ContextPickerAction::None, |source| {
                    ContextPickerAction::Toggle { request_id, source }
                }),
            (ContextPickerPhase::Sources, Key::Character(key))
                if key.as_str().eq_ignore_ascii_case("e") =>
            {
                ContextPickerAction::EditFiles { request_id }
            }
            (ContextPickerPhase::FilePaths, Key::Named(NamedKey::Backspace)) => {
                ContextPickerAction::BackspaceFileInput { request_id }
            }
            (ContextPickerPhase::FilePaths, Key::Character(text)) => {
                text.chars()
                    .next()
                    .map_or(ContextPickerAction::None, |character| {
                        ContextPickerAction::AppendFileInput {
                            request_id,
                            character,
                        }
                    })
            }
            _ => ContextPickerAction::None,
        };
        self.apply_context_picker_action(action);
        self.request_redraw(RedrawReason::KeyboardInput);
    }

    /// Open the context picker for the agent in `terminal`. Terminal output
    /// comes from another pane of the same tab when there is one, else from
    /// the first shell tab.
    pub(super) fn open_context_picker_for(
        &mut self,
        terminal: crate::tabs::TerminalIdentity,
        agent: &crate::agents::AgentDefinition,
    ) {
        let Some(tab) = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.tab_by_id(terminal.tab_id))
        else {
            return;
        };
        let cwd = tab.pane_working_directory(terminal.pane_id);
        self.context_picker.open(
            cwd,
            crate::agents::AgentTarget {
                identity: terminal,
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
            },
        );
    }

    pub(super) fn apply_context_picker_action(
        &mut self,
        action: crate::ui::context_picker::ContextPickerAction,
    ) {
        let start = matches!(
            action,
            crate::ui::context_picker::ContextPickerAction::Primary { request_id }
                if self.context_picker.is_awaiting(request_id)
                    && self.context_picker.phase()
                        == crate::ui::context_picker::ContextPickerPhase::Sources
        );
        let request_id = self.context_picker.request_id();
        self.context_picker.apply(action);
        if start {
            self.start_context_gather(request_id);
        }
    }

    fn start_context_gather(&mut self, request_id: u64) {
        let terminal_output = self
            .context_picker
            .target()
            .and_then(|target| self.context_source_output(target.identity));
        let Some(request) = self
            .context_picker
            .begin_gather(request_id, terminal_output)
        else {
            return;
        };
        if self.window.is_none() {
            self.handle_context_gathered(
                request_id,
                Err("Context gathering requires a native window".into()),
            );
            return;
        }
        let window_key = self.window_key();
        let Some(proxy) = super::EVENT_LOOP_PROXY.get().cloned() else {
            self.handle_context_gathered(
                request_id,
                Err("Context gathering event channel is unavailable".into()),
            );
            return;
        };
        let spawn = std::thread::Builder::new()
            .name("termeric-context-gather".into())
            .spawn(move || {
                let result = crate::agents::context::gather_context(request);
                let _ = proxy.send_event(super::AppEvent::WindowResult {
                    window: window_key,
                    result: super::WindowResult::ContextGathered { request_id, result },
                });
            });
        if let Err(error) = spawn {
            self.handle_context_gathered(
                request_id,
                Err(format!("Could not start context gathering: {error}")),
            );
        }
    }

    pub(super) fn handle_context_gathered(
        &mut self,
        request_id: u64,
        result: Result<String, String>,
    ) {
        use crate::ui::context_picker::ContextPickerCompletion;
        match self.context_picker.complete(request_id, result) {
            ContextPickerCompletion::Ignored => return,
            ContextPickerCompletion::Failed => {}
            ContextPickerCompletion::Ready { target, text } => {
                // It says itself whether the text arrived.
                self.send_to_agent(target, &text);
            }
        }
        self.request_redraw(RedrawReason::WindowEvent);
    }

    /// The last 50 lines of the terminal to draw context from: a non-agent
    /// pane of the agent's tab, else the first open shell tab whose focused
    /// terminal is not running an agent.
    pub(super) fn context_source_output(
        &self,
        agent: crate::tabs::TerminalIdentity,
    ) -> Option<String> {
        let tabs = self.tabs.as_ref()?;
        let tail = |terminal: &termeric_core::Terminal| {
            let lines = terminal.grid().plain_lines();
            let start = lines.len().saturating_sub(50);
            lines[start..].join("\n")
        };
        // A shell tab can host an agent started by hand; its screen is the
        // agent's conversation, not the user's work.
        let not_an_agent = |tab: &crate::tabs::Tab, id| {
            tab.pane_integration(id)
                .is_some_and(|state| state.agent.is_none())
        };
        let tab = tabs.tab_by_id(agent.tab_id)?;
        let sibling = tab
            .owned_pane_ids()
            .into_iter()
            .filter(|id| *id != agent.pane_id)
            .find(|id| not_an_agent(tab, *id))
            .and_then(|id| tab.pane_terminal(id));
        if let Some(terminal) = sibling {
            return Some(tail(terminal));
        }
        tabs.iter()
            .find(|(_, t)| {
                matches!(
                    t.tab_type,
                    crate::tabs::TabType::Shell | crate::tabs::TabType::Profile { .. }
                ) && !t.closed
                    && t.id != agent.tab_id
                    && not_an_agent(t, t.focused_terminal_identity().pane_id)
            })
            .and_then(|(_, tab)| tab.focused_terminal().map(tail))
    }
}
