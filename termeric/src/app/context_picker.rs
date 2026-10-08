//! The context picker: choosing what to send an agent, gathering it off the
//! event loop, and typing it into the agent's terminal.

use super::text_field_keys::{FieldKey, types_text};
use super::*;
use crate::ui::context_picker::{ContextPickerAction, ContextPickerPhase};
use winit::keyboard::{Key, NamedKey};

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

    /// Keys for the context picker: Tab and Shift+Tab move focus through
    /// the list or field and the buttons, Enter and Space press the focused
    /// button (from the list, Enter inserts and Space toggles the selected
    /// source), Escape cancels or goes back, the arrows and Page Up/Down
    /// move through the sources, and the focused field edits and types like
    /// every other field, Space included.
    pub(super) fn handle_context_picker_key(&mut self, key: &Key, repeat: bool) {
        let request_id = self.context_picker.request_id();
        let phase = self.context_picker.phase();
        let page = self.context_picker.viewport_rows() as isize;
        let move_by = |delta| ContextPickerAction::Move { request_id, delta };
        let action = match key {
            Key::Named(NamedKey::Escape) => ContextPickerAction::Cancel { request_id },
            Key::Named(NamedKey::Tab) => {
                self.ime.clear_preedit();
                self.context_picker
                    .cycle_focus(request_id, self.modifiers.shift_key());
                ContextPickerAction::None
            }
            // A held key must not press something it did not mean to reach.
            Key::Named(NamedKey::Enter) if repeat => ContextPickerAction::None,
            Key::Named(NamedKey::Enter) => self.context_picker.enter_action(request_id),
            _ if self.context_picker.file_input_focused() => {
                self.context_picker_field_key(key, request_id);
                ContextPickerAction::None
            }
            Key::Named(NamedKey::Space) if repeat => ContextPickerAction::None,
            Key::Named(NamedKey::Space) => self.context_picker.space_action(request_id),
            Key::Named(named) if phase == ContextPickerPhase::Sources => match named {
                NamedKey::ArrowUp => move_by(-1),
                NamedKey::ArrowDown => move_by(1),
                NamedKey::PageUp => move_by(-page),
                NamedKey::PageDown => move_by(page),
                _ => ContextPickerAction::None,
            },
            _ => ContextPickerAction::None,
        };
        self.apply_context_picker_action(action);
        self.request_redraw(RedrawReason::KeyboardInput);
    }

    /// A key while the file field is focused: the shared editing keys, and
    /// text (Space included) under the modifiers that type it. Under AltGr a
    /// character key types, even where the layout reports Ctrl with it.
    fn context_picker_field_key(&mut self, key: &Key, request_id: u64) {
        let changed = match self.text_edit_for_key(key) {
            Some(FieldKey::Edit(edit)) => self.context_picker.edit_file_input(request_id, edit),
            Some(FieldKey::Consume) => false,
            None if types_text(self.modifiers, self.alt_graph_active) => match key {
                Key::Named(NamedKey::Space) => {
                    self.context_picker.insert_file_text(request_id, " ")
                }
                Key::Character(text) => self
                    .context_picker
                    .insert_file_text(request_id, text.as_str()),
                _ => false,
            },
            None => false,
        };
        if changed {
            self.ime.clear_preedit();
        }
    }

    /// A primary press on the context picker: a button or source row takes
    /// focus and is pressed, and the file field takes focus.
    pub(super) fn press_context_picker(&mut self, x: f32, y: f32) {
        let snapshot = self.context_picker.snapshot();
        let Some(target) = self.context_picker_layout().target_at(x, y, &snapshot) else {
            return;
        };
        let action = self.context_picker.press(snapshot.request_id, target);
        self.apply_context_picker_action(action);
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

    pub(super) fn apply_context_picker_action(&mut self, action: ContextPickerAction) {
        let start = matches!(
            action,
            ContextPickerAction::Primary { request_id }
                if self.context_picker.is_awaiting(request_id)
                    && self.context_picker.phase() == ContextPickerPhase::Sources
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::context_picker::{ContextPickerButton, ContextPickerFocus, ContextSourceId};

    fn open_in(app: &mut App, directory: &std::path::Path) -> u64 {
        app.context_picker.set_reduced_motion(true);
        app.context_picker.open(
            Some(directory.display().to_string()),
            crate::agents::AgentTarget {
                identity: crate::tabs::TerminalIdentity {
                    tab_id: usize::MAX,
                    pane_id: crate::panes::PaneId(0),
                },
                agent_id: "fixture-agent".into(),
                agent_name: "Fixture Agent".into(),
            },
        )
    }

    fn press(app: &mut App, key: NamedKey) {
        app.handle_context_picker_key(&Key::Named(key), false);
    }

    #[test]
    fn letter_keys_do_nothing_beside_the_sources() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::new_headless(Config::default()).unwrap();
        let request = open_in(&mut app, directory.path());
        let before = app.context_picker.snapshot().sources;
        for letter in ["e", "E", "x"] {
            app.handle_context_picker_key(&Key::Character(letter.into()), false);
        }
        assert!(app.context_picker.is_awaiting(request));
        assert_eq!(app.context_picker.phase(), ContextPickerPhase::Sources);
        assert_eq!(app.context_picker.focus(), ContextPickerFocus::Sources);
        assert_eq!(app.context_picker.snapshot().sources, before);
    }

    fn type_text(app: &mut App, text: &str) {
        for character in text.chars() {
            let key = if character == ' ' {
                Key::Named(NamedKey::Space)
            } else {
                Key::Character(character.to_string().into())
            };
            app.handle_context_picker_key(&key, false);
        }
    }

    #[test]
    fn the_file_field_types_spaces_and_edits_at_its_caret_like_every_field() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::new_headless(Config::default()).unwrap();
        let request = open_in(&mut app, directory.path());
        app.apply_context_picker_action(ContextPickerButton::ChooseFiles.action(request));
        assert_eq!(app.context_picker.focus(), ContextPickerFocus::FileInput);

        type_text(&mut app, "my notes.md");
        app.handle_context_picker_key(&Key::Named(NamedKey::Space), true);
        assert_eq!(
            app.context_picker.snapshot().file_input,
            "my notes.md ",
            "a held Space types in a field"
        );
        press(&mut app, NamedKey::Backspace);
        press(&mut app, NamedKey::Home);
        type_text(&mut app, "docs/");
        app.modifiers = ModifiersState::CONTROL;
        press(&mut app, NamedKey::ArrowRight);
        app.handle_context_picker_key(&Key::Character("a".into()), false);
        app.handle_context_picker_key(&Key::Named(NamedKey::Space), false);
        app.modifiers = ModifiersState::empty();
        type_text(&mut app, "🌱");
        assert_eq!(
            app.context_picker.snapshot().file_input,
            "docs/my🌱 notes.md",
            "Ctrl moves by word and types nothing"
        );
        press(&mut app, NamedKey::Backspace);
        press(&mut app, NamedKey::Delete);
        assert_eq!(
            app.context_picker.snapshot().file_input,
            "docs/mynotes.md",
            "Backspace and Delete remove whole graphemes"
        );
        assert_eq!(app.context_picker.phase(), ContextPickerPhase::FilePaths);
    }

    #[test]
    fn choose_files_is_reached_with_tab_and_pressed_with_enter_or_space() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("fixture.rs"), "fn fixture() {}\n").unwrap();
        let mut app = App::new_headless(Config::default()).unwrap();
        let request = open_in(&mut app, directory.path());
        press(&mut app, NamedKey::Tab);
        assert_eq!(
            app.context_picker.focus(),
            ContextPickerFocus::Button(ContextPickerButton::ChooseFiles)
        );
        app.handle_context_picker_key(&Key::Named(NamedKey::Enter), true);
        assert_eq!(
            app.context_picker.phase(),
            ContextPickerPhase::Sources,
            "a held key presses nothing"
        );
        press(&mut app, NamedKey::Space);
        assert_eq!(app.context_picker.phase(), ContextPickerPhase::FilePaths);
        assert_eq!(app.context_picker.focus(), ContextPickerFocus::FileInput);

        for character in "fixture.rs".chars() {
            app.handle_context_picker_key(&Key::Character(character.to_string().into()), false);
        }
        press(&mut app, NamedKey::Tab);
        app.handle_context_picker_key(&Key::Character("x".into()), false);
        assert_eq!(
            app.context_picker.snapshot().file_input,
            "fixture.rs",
            "only the focused field takes typing"
        );
        press(&mut app, NamedKey::Tab);
        assert_eq!(
            app.context_picker.focus(),
            ContextPickerFocus::Button(ContextPickerButton::Primary)
        );
        press(&mut app, NamedKey::Enter);
        assert_eq!(app.context_picker.phase(), ContextPickerPhase::Sources);
        assert_eq!(
            app.context_picker.focus(),
            ContextPickerFocus::Button(ContextPickerButton::ChooseFiles)
        );
        let files = app
            .context_picker
            .sources()
            .iter()
            .find(|source| source.id == ContextSourceId::SpecificFiles)
            .unwrap();
        assert!(files.selected);

        press(&mut app, NamedKey::Enter);
        press(&mut app, NamedKey::Escape);
        assert_eq!(
            app.context_picker.phase(),
            ContextPickerPhase::Sources,
            "Escape goes back from the editor"
        );
        press(&mut app, NamedKey::ArrowDown);
        assert_eq!(
            app.context_picker.focus(),
            ContextPickerFocus::Sources,
            "the arrows return focus to the list they move"
        );
        press(&mut app, NamedKey::Escape);
        assert!(!app.context_picker.is_awaiting(request));
    }

    #[test]
    fn a_press_on_choose_files_opens_the_editor_from_its_drawn_bounds() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::new_headless(Config::default()).unwrap();
        open_in(&mut app, directory.path());
        let [x, y] = app
            .context_picker_layout()
            .choose_files_button
            .unwrap()
            .center();
        app.press_context_picker(x, y);
        assert_eq!(app.context_picker.phase(), ContextPickerPhase::FilePaths);
        assert_eq!(app.context_picker.focus(), ContextPickerFocus::FileInput);
    }
}
