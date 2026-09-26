//! The agent view's actions, keys, pointer, and wheel.

use std::time::Instant;

use winit::keyboard::{Key, NamedKey};

use super::*;
use crate::agent_view::present;
use crate::agent_view::session::{AgentSession, QuestionAnswer};
use crate::agent_view::transcript::{Attachment, DecidedBy, Item};
use crate::ui::agent_view::commands;
use crate::ui::agent_view::scene::{Bounds, MenuAction, MenuKind, Target};
use crate::ui::agent_view::{ComposerIntent, Focus, OpenMenu, Screen};
use crate::ui::text_input::TextInputEdit;
use crate::ui::text_layout::TextMeasure;

use super::agent_view::DIVIDER_GRAB;

/// Two presses this close in time and place select a word.
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);
const DOUBLE_CLICK_DISTANCE: f32 = 4.0;

/// Pixels scrolled per wheel line.
const WHEEL_LINE: f32 = 48.0;

/// Where to leave the composer after a denial, with a matching hint.
const DENIED_HINT: &str = "Tell Claude what to do instead…";
const PLAN_HINT: &str = "Tell Claude what to change in the plan…";
const ANSWER_HINT: &str = "Type your answer…";

impl App {
    // ── Actions ──────────────────────────────────────────────────────────

    /// Performs what a target does. One path for pointer, keys, and
    /// assistive technology.
    pub(super) fn activate_agent_view_target(&mut self, target: Target) {
        let active = self.agent_view.state.active;
        match target {
            Target::ShowSessions | Target::SessionTitle => {
                self.agent_view.state.screen = Screen::Sessions;
                self.agent_view.state.menu = None;
                self.agent_view.state.sessions_scroll = Default::default();
            }
            Target::NewSession => self.new_agent_session_page(),
            Target::Back => {
                if self.agent_view_conversation().is_some() {
                    self.agent_view.state.screen = Screen::Conversation;
                }
            }
            Target::OpenMenu(kind) => {
                let anchor = self
                    .agent_view
                    .scene
                    .as_ref()
                    .and_then(|scene| scene.hit_for(&Target::OpenMenu(kind)))
                    .map_or_else(Bounds::default, |hit| hit.bounds);
                let open = self
                    .agent_view
                    .state
                    .menu
                    .as_ref()
                    .is_some_and(|menu| menu.kind == kind);
                self.agent_view.state.menu = (!open).then_some(OpenMenu {
                    kind,
                    anchor,
                    highlighted: 0,
                });
            }
            Target::DismissMenu => self.agent_view.state.menu = None,
            Target::Menu(action) => {
                self.agent_view.state.menu = None;
                self.apply_agent_view_menu(action);
            }
            Target::Toggle(key) => {
                if let Some(id) = active {
                    self.agent_view.state.conversation_mut(id).toggle(key);
                }
            }
            Target::Allow { request_id } => {
                if let Some(session) = self.agent_view.active_session_mut() {
                    session.allow(&request_id, false, DecidedBy::Person);
                }
                self.focus_agent_composer();
            }
            Target::AllowAlways { request_id } => {
                if let Some(session) = self.agent_view.active_session_mut() {
                    session.allow(&request_id, true, DecidedBy::Person);
                }
                self.focus_agent_composer();
            }
            Target::Deny { request_id } => {
                if let Some(session) = self.agent_view.active_session_mut() {
                    session.deny(&request_id, None, DecidedBy::Person, true);
                }

                if let Some(id) = active {
                    self.agent_view.state.conversation_mut(id).placeholder = Some(DENIED_HINT);
                }
                self.focus_agent_composer();
            }
            Target::ChooseOption {
                request_id,
                question,
                option,
            } => {
                let multiple = self
                    .agent_view_question(&request_id, question)
                    .is_some_and(|question| question.multiple);
                if let Some(id) = active {
                    self.agent_view.state.conversation_mut(id).choose(
                        &request_id,
                        question,
                        option,
                        multiple,
                    );
                }
            }
            Target::AnswerOther {
                request_id,
                question,
            } => {
                if let Some(id) = active {
                    let view = self.agent_view.state.conversation_mut(id);
                    view.intent = ComposerIntent::Answer {
                        request_id,
                        question,
                    };
                    view.placeholder = Some(ANSWER_HINT);
                }
                self.focus_agent_composer();
            }
            Target::SubmitAnswers { request_id } => self.submit_agent_answers(&request_id),
            Target::ApprovePlan { request_id, mode } => {
                if let Some(session) = self.agent_view.active_session_mut() {
                    session.approve_plan(&request_id, mode);
                }
                self.focus_agent_composer();
            }
            Target::KeepPlanning { request_id } => {
                if let Some(session) = self.agent_view.active_session_mut() {
                    session.deny(
                        &request_id,
                        Some("Keep planning; I'll say what to change."),
                        DecidedBy::Person,
                        true,
                    );
                }
                if let Some(id) = active {
                    self.agent_view.state.conversation_mut(id).placeholder = Some(PLAN_HINT);
                }
                self.focus_agent_composer();
            }
            Target::CopyItem(index) => self.copy_agent_turn(index),
            Target::ReviewChanges(_) => {
                if let Some(id) = active {
                    self.review_agent_view_changes(id);
                }
            }
            Target::Relaunch => {
                if let Some(session) = self.agent_view.active_session_mut()
                    && let Err(error) = session.relaunch()
                {
                    let message = error.to_string();
                    self.toasts.error(message);
                }
            }
            Target::OpenInTerminal | Target::RunInTerminal => self.open_agent_view_in_terminal(),
            Target::ScrollToEnd => {
                if let Some(id) = active {
                    self.agent_view
                        .state
                        .conversation_mut(id)
                        .scroll
                        .follow_end();
                }
            }
            Target::Composer => self.focus_agent_composer(),
            Target::AttachSelection => self.attach_agent_selection(),
            Target::AttachLastCommand => self.attach_agent_last_command(),
            Target::RemoveAttachment(index) => {
                if let Some(id) = active {
                    let view = self.agent_view.state.conversation_mut(id);
                    if index < view.attachments.len() {
                        view.attachments.remove(index);
                    }
                }
            }
            Target::Command(name) => self.accept_agent_command(&name),
            Target::SetLayout(layout) => self.set_agent_view_layout(layout),
            Target::CancelQueued(index) => {
                if let Some(text) = self
                    .agent_view
                    .active_session_mut()
                    .and_then(|session| session.cancel_queued(index))
                    && let Some(id) = active
                {
                    // Back in the box, so nothing typed is lost.
                    let view = self.agent_view.state.conversation_mut(id);
                    if view.composer.is_empty() {
                        view.composer.set_text(&text);
                    }
                }
            }
            Target::Send => self.send_agent_message(),
            Target::Stop => {
                if let Some(session) = self.agent_view.active_session_mut() {
                    session.interrupt();
                }
            }
            Target::OpenSession(id) => self.show_agent_session(id),
            Target::ResumePast(id) => {
                let title = self
                    .agent_view_past_title(&id)
                    .unwrap_or_else(|| "Earlier conversation".to_owned());
                self.start_agent_session(Some((id, title)));
            }
            Target::OpenTerminalAgent(identity) => {
                if !self.activate_notification_origin(identity) {
                    self.toasts.info("That agent's terminal has closed");
                }
            }
            Target::ToggleSupervisor => {
                if self.supervisor_held() {
                    self.hand_back_to_supervisor();
                } else if self.supervisor_running() {
                    self.take_over_from_supervisor();
                }
            }
            Target::StartSession => {
                self.start_agent_session(None);
            }
        }
        self.request_agent_view_redraw();
    }

    fn apply_agent_view_menu(&mut self, action: MenuAction) {
        let screen = self.agent_view.state.screen;
        match action {
            MenuAction::SetMode(mode) => match (screen, self.agent_view.active_session_mut()) {
                (Screen::Conversation, Some(session)) => session.set_mode(mode),
                _ if mode.skips_every_check()
                    && !self.config.agent_view.allow_bypass_permissions => {}
                (_, session) => {
                    // A session started ahead of the first message follows
                    // the form.
                    if let Some(session) = session.filter(|session| session.is_blank()) {
                        session.set_mode(mode);
                    }
                    self.agent_view.state.new_session.mode = mode;
                }
            },
            MenuAction::SetModel(model) => match (screen, self.agent_view.active_session_mut()) {
                (Screen::Conversation, Some(session)) => session.set_model(&model),
                (_, session) => {
                    if let Some(session) = session.filter(|session| session.is_blank()) {
                        session.set_model(&model);
                    }
                    self.agent_view.state.new_session.model = Some(model);
                }
            },
            MenuAction::SetFolder(path) => self.agent_view.state.new_session.cwd = Some(path),
            MenuAction::EndSession => {
                if let Some(id) = self.agent_view.state.active {
                    self.end_agent_session(id);
                }
            }
            MenuAction::OpenInTerminal => self.open_agent_view_in_terminal(),
            MenuAction::CopyConversation => self.copy_agent_conversation(),
            MenuAction::HidePanel => self.close_agent_view(),
        }
    }

    fn focus_agent_composer(&mut self) {
        self.agent_view.state.focus = Focus::Composer;
        self.set_agent_view_keyboard(true);
        self.agent_view.state.touch_caret();
    }

    fn agent_view_question(&self, request_id: &str, question: usize) -> Option<present::Question> {
        let session = self.agent_view.session(self.agent_view.state.active?)?;
        let approval = session.transcript().approval(request_id)?;
        present::questions(&approval.input)
            .into_iter()
            .nth(question)
    }

    fn agent_view_past_title(&self, id: &str) -> Option<String> {
        self.agent_view
            .past
            .as_ref()?
            .1
            .iter()
            .find(|past| past.id == id)
            .map(|past| past.title.clone())
    }

    /// Answers a question once every part has a choice or typed answer.
    fn submit_agent_answers(&mut self, request_id: &str) {
        let Some(id) = self.agent_view.state.active else {
            return;
        };
        let Some(session) = self.agent_view.session(id) else {
            return;
        };
        let Some(approval) = session.transcript().approval(request_id) else {
            return;
        };
        let questions = present::questions(&approval.input);
        let Some(view) = self.agent_view.state.conversation(id) else {
            return;
        };
        let mut answers = Vec::with_capacity(questions.len());
        for (index, question) in questions.iter().enumerate() {
            let answer = if let Some(typed) = view.typed_answer(request_id, index) {
                typed.to_owned()
            } else {
                let chosen: Vec<&str> = view
                    .chosen(request_id, index)
                    .into_iter()
                    .flatten()
                    .filter_map(|option| {
                        question
                            .options
                            .get(*option)
                            .map(|option| option.label.as_str())
                    })
                    .collect();
                if chosen.is_empty() {
                    self.toasts
                        .info("Choose an answer for every question first");
                    return;
                }
                chosen.join(", ")
            };
            answers.push(QuestionAnswer {
                question: question.question.clone(),
                answer,
            });
        }
        if let Some(session) = self.agent_view.session_mut(id) {
            session.answer_question(request_id, &answers);
        }
        self.focus_agent_composer();
    }

    /// Sends the composer: a message, or a typed answer to a question.
    pub(super) fn send_agent_message(&mut self) {
        let Some(id) = self.agent_view.state.active else {
            return;
        };
        let view = self.agent_view.state.conversation_mut(id);
        let text = view.composer.text().trim().to_owned();
        if let ComposerIntent::Answer {
            request_id,
            question,
        } = view.intent.clone()
        {
            if text.is_empty() {
                return;
            }
            view.set_typed_answer(&request_id, question, text);
            view.composer.clear();
            view.intent = ComposerIntent::Message;
            view.placeholder = None;
            let complete = self
                .agent_view
                .session(id)
                .and_then(|session| session.transcript().approval(&request_id))
                .map(|approval| present::questions(&approval.input).len())
                .is_some_and(|count| {
                    let view = self.agent_view.state.conversation(id).expect("view exists");
                    (0..count).all(|index| {
                        view.typed_answer(&request_id, index).is_some()
                            || view
                                .chosen(&request_id, index)
                                .is_some_and(|chosen| !chosen.is_empty())
                    })
                });
            if complete {
                self.submit_agent_answers(&request_id);
            }
            return;
        }
        if text.is_empty() && view.attachments.is_empty() {
            return;
        }
        let attachments = std::mem::take(&mut view.attachments);
        view.remember_sent(&text);
        view.composer.clear();
        view.placeholder = None;
        view.scroll.follow_end();
        let Some(session) = self.agent_view.session_mut(id) else {
            return;
        };
        if let Err(error) = session.prompt(&text, attachments.clone()) {
            let view = self.agent_view.state.conversation_mut(id);
            view.composer.set_text(&text);
            view.attachments = attachments;
            self.toasts.error(error.to_string());
        }
    }

    fn attach_agent_selection(&mut self) {
        let Some(text) = self.agent_view_selection_text() else {
            self.toasts.info("Select text in a terminal first");
            return;
        };
        let lines = text.lines().count().max(1);
        self.attach_agent_context(Attachment {
            label: format!("Terminal selection · {}", plural(lines, "line", "lines")),
            heading: "Selected text from my terminal".to_owned(),
            text,
        });
    }

    fn attach_agent_last_command(&mut self) {
        let Some((command, output, failed)) = self.agent_view_last_command() else {
            self.toasts
                .info("No finished command with output in this terminal");
            return;
        };
        let lines = output.lines().count().max(1);
        let status = if failed { ", which failed" } else { "" };
        self.attach_agent_context(Attachment {
            label: format!("{command} · {}", plural(lines, "line", "lines")),
            heading: format!("Output of `{command}` from my terminal{status}"),
            text: output,
        });
    }

    fn attach_agent_context(&mut self, attachment: Attachment) {
        match (self.agent_view.state.screen, self.agent_view.state.active) {
            (Screen::Conversation, Some(id)) => {
                let view = self.agent_view.state.conversation_mut(id);
                if !view
                    .attachments
                    .iter()
                    .any(|existing| existing.label == attachment.label)
                {
                    view.attachments.push(attachment);
                }
            }
            _ => {
                // A new session's first message carries the context inline.
                let composer = &mut self.agent_view.state.new_session.composer;
                let prompt = attachment.as_prompt();
                if !composer.is_empty() {
                    composer.move_to_edge(true, false);
                    composer.insert("\n\n");
                }
                composer.insert(&prompt);
            }
        }
        self.focus_agent_composer();
    }

    /// Copies the answer that ended at a turn end.
    fn copy_agent_turn(&mut self, turn_end: usize) {
        let Some(session) = self
            .agent_view
            .state
            .active
            .and_then(|id| self.agent_view.session(id))
        else {
            return;
        };
        let items = session.transcript().items();
        let start = items[..turn_end.min(items.len())]
            .iter()
            .rposition(|item| matches!(item, Item::User { .. } | Item::TurnEnd(_)))
            .map_or(0, |index| index + 1);
        let answer: Vec<&str> = items[start..turn_end.min(items.len())]
            .iter()
            .filter_map(|item| match item {
                Item::Text { text, .. } if !text.trim().is_empty() => Some(text.as_str()),
                _ => None,
            })
            .collect();
        if answer.is_empty() {
            self.toasts.info("That turn has no answer to copy");
            return;
        }
        let text = answer.join("\n\n");
        self.copy_agent_view_text(&text, "Copied the answer");
    }

    fn copy_agent_conversation(&mut self) {
        let Some(session) = self
            .agent_view
            .state
            .active
            .and_then(|id| self.agent_view.session(id))
        else {
            return;
        };
        let mut text = String::new();
        for item in session.transcript().items() {
            let part = match item {
                Item::User { text, attachments } => {
                    let mut part = format!("**You:** {text}");
                    for attachment in attachments {
                        part.push_str("\n\n");
                        part.push_str(&attachment.as_prompt());
                    }
                    part
                }
                Item::Text { text, .. } if !text.trim().is_empty() => format!("**Claude:** {text}"),
                Item::Tool(tool) => {
                    let summary = present::summarize(tool, session.cwd());
                    format!("_{} {}_", summary.verb, summary.subject).replace("_ _", "")
                }
                _ => continue,
            };
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&part);
        }
        self.copy_agent_view_text(&text, "Copied the conversation");
    }

    // ── Keys ─────────────────────────────────────────────────────────────

    /// Handles a key while the panel has keyboard focus. Returns true when
    /// the key belonged to the panel.
    pub(super) fn handle_agent_view_key(&mut self, key: &Key, text: Option<&str>) -> bool {
        if !self.agent_view.state.has_keyboard() {
            return false;
        }
        if self.alt_graph_active && text.is_none() {
            return true;
        }
        let modifiers = self.modifiers;
        let control = modifiers.control_key();
        let shift = modifiers.shift_key();
        let plain = modifiers.is_empty();

        if self.agent_view.state.menu.is_some() {
            self.handle_agent_view_menu_key(key);
            self.request_agent_view_redraw();
            return true;
        }
        if self.handle_agent_command_menu_key(key) {
            self.agent_view.state.touch_caret();
            self.request_agent_view_redraw();
            return true;
        }
        let typed_before = self.agent_view_typed_command();
        match key {
            Key::Named(NamedKey::Tab) if !control => {
                self.move_agent_view_focus(if shift { -1 } else { 1 });
            }
            Key::Named(NamedKey::Escape) if plain => self.agent_view_escape(),
            Key::Named(NamedKey::PageUp | NamedKey::PageDown) => {
                let direction = if matches!(key, Key::Named(NamedKey::PageUp)) {
                    -1.0
                } else {
                    1.0
                };
                self.scroll_agent_view_page(direction);
            }
            _ => match self.agent_view.state.focus.clone() {
                Focus::Target(target) => match key {
                    Key::Named(NamedKey::Enter | NamedKey::Space) if plain => {
                        self.activate_agent_view_target(target);
                    }
                    Key::Character(character) if plain && character == " " => {
                        self.activate_agent_view_target(target);
                    }
                    // Typing jumps back to the composer.
                    _ if text.is_some_and(|text| !text.chars().any(char::is_control))
                        && !control =>
                    {
                        self.focus_agent_composer();
                        return self.handle_agent_view_key(key, text);
                    }
                    _ => {}
                },
                Focus::Composer => {
                    if !self.handle_agent_composer_key(key, text) {
                        return true;
                    }
                    if self.agent_view_typed_command() != typed_before {
                        self.agent_view.state.commands.highlighted = 0;
                    }
                }
            },
        }
        self.agent_view.state.touch_caret();
        self.ime.clear_preedit();
        self.request_agent_view_redraw();
        true
    }

    /// The agent's commands matching what is typed, as the menu shows them:
    /// each name and whether it takes an argument.
    pub(super) fn agent_command_matches(&self) -> Vec<(String, bool)> {
        let sessions: Vec<&AgentSession> = self.agent_view.sessions.iter().collect();
        let offered = commands::offered(&self.agent_view.state, &sessions);
        commands::shown(&self.agent_view.state, offered)
            .into_iter()
            .map(|command| (command.name.clone(), !command.argument_hint.is_empty()))
            .collect()
    }

    fn agent_view_typed_command(&self) -> Option<String> {
        let composer = self.agent_view.state.composer()?;
        commands::typed_name(composer.text(), composer.caret()).map(str::to_owned)
    }

    /// Keys for the command menu while it shows. Returns false for keys it
    /// leaves to the message box.
    fn handle_agent_command_menu_key(&mut self, key: &Key) -> bool {
        let matches = self.agent_command_matches();
        if matches.is_empty() {
            return false;
        }
        let modifiers = self.modifiers;
        let plain = modifiers.is_empty();
        let count = matches.len();
        let highlighted = self.agent_view.state.commands.highlighted.min(count - 1);
        match key {
            Key::Named(NamedKey::ArrowDown) if plain => {
                self.agent_view.state.commands.highlighted = (highlighted + 1) % count;
            }
            Key::Named(NamedKey::ArrowUp) if plain => {
                self.agent_view.state.commands.highlighted = (highlighted + count - 1) % count;
            }
            Key::Named(NamedKey::Escape) if plain => {
                self.agent_view.state.commands.dismissed = self.agent_view_typed_command();
            }
            Key::Named(NamedKey::Tab) if plain => {
                let (name, _) = &matches[highlighted];
                self.complete_agent_command(name);
            }
            Key::Named(NamedKey::Enter) if plain => {
                let (name, _) = matches[highlighted].clone();
                self.accept_agent_command(&name);
            }
            _ => return false,
        }
        true
    }

    /// Puts `/name ` in the message box, keeping anything after the first
    /// word.
    fn complete_agent_command(&mut self, name: &str) {
        let state = &mut self.agent_view.state;
        let composer = state.composer_mut();
        let (text, caret) = commands::complete(composer.text(), name);
        composer.set_text(&text);
        composer.place(caret, false);
        state.commands = commands::CommandMenu::default();
        state.focus = Focus::Composer;
    }

    /// Chooses a command: completes it, and sends it at once when it takes
    /// no argument, as Claude Code's own prompt does.
    fn accept_agent_command(&mut self, name: &str) {
        let takes_argument = {
            let sessions: Vec<&AgentSession> = self.agent_view.sessions.iter().collect();
            commands::offered(&self.agent_view.state, &sessions)
                .iter()
                .find(|command| command.name == name)
                .is_some_and(|command| !command.argument_hint.is_empty())
        };
        self.complete_agent_command(name);
        self.set_agent_view_keyboard(true);
        if takes_argument {
            return;
        }
        match self.agent_view.state.screen {
            Screen::NewSession => {
                self.start_agent_session(None);
            }
            Screen::Conversation => self.send_agent_message(),
            Screen::Sessions => {}
        }
    }

    fn handle_agent_view_menu_key(&mut self, key: &Key) {
        let Some(menu) = self.agent_view.state.menu.clone() else {
            return;
        };
        let items = self.agent_view_menu_actions(menu.kind);
        match key {
            Key::Named(NamedKey::Escape) => self.agent_view.state.menu = None,
            Key::Named(NamedKey::ArrowDown | NamedKey::Tab) if !items.is_empty() => {
                if let Some(open) = &mut self.agent_view.state.menu {
                    open.highlighted = (open.highlighted + 1) % items.len();
                }
            }
            Key::Named(NamedKey::ArrowUp) if !items.is_empty() => {
                if let Some(open) = &mut self.agent_view.state.menu {
                    open.highlighted = (open.highlighted + items.len() - 1) % items.len();
                }
            }
            Key::Named(NamedKey::Enter | NamedKey::Space) => {
                if let Some(action) = items.get(menu.highlighted).cloned() {
                    self.activate_agent_view_target(Target::Menu(action));
                }
            }
            _ => {}
        }
    }

    fn agent_view_menu_actions(&self, kind: MenuKind) -> Vec<MenuAction> {
        let Some(scene) = &self.agent_view.scene else {
            return Vec::new();
        };
        scene
            .hits
            .iter()
            .filter_map(|hit| match &hit.target {
                Target::Menu(action) if menu_kind_of(action) == kind => Some(action.clone()),
                _ => None,
            })
            .collect()
    }

    fn move_agent_view_focus(&mut self, direction: i32) {
        let Some(scene) = self.agent_view.scene.clone() else {
            return;
        };
        let order: Vec<&Target> = scene.focus_order().collect();
        if order.is_empty() {
            return;
        }
        let current = match &self.agent_view.state.focus {
            Focus::Composer => order.iter().position(|target| **target == Target::Composer),
            Focus::Target(target) => order.iter().position(|candidate| *candidate == target),
        };
        let next = match current {
            Some(index) => {
                (index as i64 + i64::from(direction)).rem_euclid(order.len() as i64) as usize
            }
            None if direction > 0 => 0,
            None => order.len() - 1,
        };
        let target = order[next].clone();
        self.agent_view.state.focus_visible = true;
        self.agent_view.state.focus = if target == Target::Composer {
            Focus::Composer
        } else {
            Focus::Target(target.clone())
        };
        self.reveal_agent_view_target(&target);
    }

    /// Scrolls the conversation so a focused target inside it is visible.
    pub(super) fn reveal_agent_view_target(&mut self, target: &Target) {
        let Some(scene) = self.agent_view.scene.clone() else {
            return;
        };
        let Some(hit) = scene.hit_for(target) else {
            return;
        };
        if hit.clip.is_none() {
            return;
        }
        let viewport = scene.feed_viewport;
        let max = (scene.feed_height - viewport.height).max(0.0);
        let now = Instant::now();
        let Some(scroll) = self.agent_view_scroll_mut() else {
            return;
        };
        let current = scroll.offset(max, now);
        let margin = 12.0;
        let top = hit.bounds.y - viewport.y + current;
        let bottom = top + hit.bounds.height;
        if top < current + margin {
            scroll.glide_to(top - margin, max, now);
        } else if bottom > current + viewport.height - margin {
            scroll.glide_to(bottom - viewport.height + margin, max, now);
        }
    }

    fn agent_view_scroll_mut(&mut self) -> Option<&mut crate::ui::agent_view::Scroll> {
        let state = &mut self.agent_view.state;
        match state.screen {
            Screen::Conversation => {
                let id = state.active?;
                Some(&mut state.conversation_mut(id).scroll)
            }
            Screen::Sessions => Some(&mut state.sessions_scroll),
            Screen::NewSession => Some(&mut state.new_session.scroll),
        }
    }

    fn scroll_agent_view_page(&mut self, direction: f32) {
        let Some(scene) = self.agent_view.scene.clone() else {
            return;
        };
        let max = (scene.feed_height - scene.feed_viewport.height).max(0.0);
        let page = (scene.feed_viewport.height * 0.85).max(40.0);
        let now = Instant::now();
        if let Some(scroll) = self.agent_view_scroll_mut() {
            let target = scroll.offset(max, now) + page * direction;
            scroll.glide_to(target, max, now);
        }
    }

    fn agent_view_escape(&mut self) {
        let state = &mut self.agent_view.state;
        if state.focus != Focus::Composer {
            state.focus = Focus::Composer;
            return;
        }
        if let Some(id) = state.active
            && state.screen == Screen::Conversation
            && let ComposerIntent::Answer { .. } = state.conversation_mut(id).intent
        {
            let view = state.conversation_mut(id);
            view.intent = ComposerIntent::Message;
            view.placeholder = None;
            return;
        }
        let working = state.screen == Screen::Conversation
            && self
                .agent_view
                .state
                .active
                .and_then(|id| self.agent_view.session(id))
                .is_some_and(|session| session.transcript().turn_active());
        if working {
            if let Some(session) = self.agent_view.active_session_mut() {
                session.interrupt();
            }
            return;
        }
        // Nothing left to cancel: give the keyboard back to the terminals,
        // unless the panel covers them.
        if !self.agent_view_fills_tab() {
            self.set_agent_view_keyboard(false);
        }
    }

    /// Edits the composer. Returns false when the key did nothing.
    fn handle_agent_composer_key(&mut self, key: &Key, text: Option<&str>) -> bool {
        let modifiers = self.modifiers;
        let control = modifiers.control_key();
        let shift = modifiers.shift_key();
        let alt = modifiers.alt_key();
        let screen = self.agent_view.state.screen;
        if screen == Screen::Sessions {
            return false;
        }
        let edit = |edit| Some((edit, shift));
        let grapheme_edit = match key {
            Key::Named(NamedKey::ArrowLeft) if control => edit(TextInputEdit::WordLeft),
            Key::Named(NamedKey::ArrowRight) if control => edit(TextInputEdit::WordRight),
            Key::Named(NamedKey::ArrowLeft) => edit(TextInputEdit::Left),
            Key::Named(NamedKey::ArrowRight) => edit(TextInputEdit::Right),
            Key::Named(NamedKey::Home) if !control => edit(TextInputEdit::Home),
            Key::Named(NamedKey::End) if !control => edit(TextInputEdit::End),
            Key::Named(NamedKey::Backspace) if control => {
                Some((TextInputEdit::BackspaceWord, false))
            }
            Key::Named(NamedKey::Delete) if control => Some((TextInputEdit::DeleteWord, false)),
            Key::Named(NamedKey::Backspace) => Some((TextInputEdit::Backspace, false)),
            Key::Named(NamedKey::Delete) => Some((TextInputEdit::Delete, false)),
            _ => None,
        };
        if let Some((edit, extend)) = grapheme_edit {
            self.agent_view.state.composer_mut().edit(edit, extend);
            return true;
        }
        match key {
            Key::Named(NamedKey::Home | NamedKey::End) if control => {
                let end = matches!(key, Key::Named(NamedKey::End));
                self.agent_view
                    .state
                    .composer_mut()
                    .move_to_edge(end, shift);
                true
            }
            Key::Named(NamedKey::Enter) if shift => {
                self.agent_view.state.composer_mut().insert("\n");
                true
            }
            Key::Named(NamedKey::Enter) if !alt => {
                match screen {
                    Screen::NewSession => {
                        if !self
                            .agent_view
                            .state
                            .new_session
                            .composer
                            .text()
                            .trim()
                            .is_empty()
                        {
                            self.start_agent_session(None);
                        }
                    }
                    _ => self.send_agent_message(),
                }
                true
            }
            Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown) => {
                let direction = if matches!(key, Key::Named(NamedKey::ArrowUp)) {
                    -1
                } else {
                    1
                };
                self.move_agent_composer_vertically(direction, shift);
                true
            }
            Key::Character(character) if control && !alt => match character.to_lowercase().as_str()
            {
                "a" => {
                    self.agent_view.state.composer_mut().select_all();
                    true
                }
                "c" => {
                    if let Some(selected) = self
                        .agent_view
                        .state
                        .composer_mut()
                        .selected_text()
                        .map(str::to_owned)
                    {
                        self.copy_agent_view_text(&selected, "");
                    }
                    true
                }
                "x" => {
                    let composer = self.agent_view.state.composer_mut();
                    if let Some(selected) = composer.selected_text().map(str::to_owned) {
                        composer.edit(TextInputEdit::Delete, false);
                        self.copy_agent_view_text(&selected, "");
                    }
                    true
                }
                "v" => {
                    if let Some(pasted) = self.agent_view_clipboard_text() {
                        self.agent_view.state.composer_mut().insert(&pasted);
                    }
                    true
                }
                "z" if shift => self.agent_view.state.composer_mut().redo(),
                "z" => self.agent_view.state.composer_mut().undo(),
                "y" => self.agent_view.state.composer_mut().redo(),
                _ => false,
            },
            _ => match text {
                Some(text)
                    if !control && !text.is_empty() && !text.chars().all(char::is_control) =>
                {
                    self.agent_view.state.composer_mut().insert(text);
                    true
                }
                _ => false,
            },
        }
    }

    /// The panel holds the keyboard and no modal surface is above it.
    pub(super) fn agent_view_has_keyboard(&self) -> bool {
        self.agent_view.state.has_keyboard()
    }

    /// Whether typed or pasted text has somewhere to go.
    pub(super) fn agent_view_accepts_text(&self) -> bool {
        self.agent_view.state.screen != Screen::Sessions
    }

    /// Clipboard text pasted into the composer.
    pub(super) fn paste_into_agent_view(&mut self, text: &str) -> bool {
        /// Pastes beyond this are refused rather than freezing layout.
        const MAX_PASTE_BYTES: usize = 1024 * 1024;
        if text.is_empty() || !self.agent_view_accepts_text() {
            return false;
        }
        if text.len() > MAX_PASTE_BYTES {
            self.toasts
                .warning("That paste is too large for a message; attach a file instead");
            return false;
        }
        self.commit_agent_view_text(text)
    }

    fn move_agent_composer_vertically(&mut self, direction: i32, extend: bool) {
        let geometry = self
            .agent_view
            .scene
            .as_ref()
            .and_then(|scene| scene.composer_text.clone());
        let screen = self.agent_view.state.screen;
        let active = self.agent_view.state.active;
        let moved = match (geometry, self.renderer.as_mut()) {
            (Some(geometry), Some(renderer)) => {
                let font_px = geometry.size.font_px;
                let measure = renderer.text_measure();
                let composer = self.agent_view.state.composer_mut();
                let layout = geometry.layout(composer.text(), measure);
                let mut width =
                    |text: &str| measure.width(text, crate::ui::text_layout::UiFace::SANS, font_px);
                composer.move_vertically(&layout, direction, extend, &mut width)
            }
            _ => false,
        };
        // At the first or last line, Up and Down recall earlier messages.
        if !moved
            && !extend
            && screen == Screen::Conversation
            && let Some(id) = active
        {
            let view = self.agent_view.state.conversation_mut(id);
            if view.composer.is_empty() || view.recalling() {
                view.recall(direction < 0);
            }
        }
    }

    /// Text the IME committed while the panel has the keyboard.
    pub(super) fn commit_agent_view_text(&mut self, text: &str) -> bool {
        if !self.agent_view.state.has_keyboard() || !self.agent_view_accepts_text() {
            return false;
        }
        if self.agent_view.state.focus != Focus::Composer {
            self.focus_agent_composer();
        }
        self.agent_view.state.composer_mut().insert(text);
        self.agent_view.state.touch_caret();
        self.request_agent_view_redraw();
        true
    }

    // ── Pointer ──────────────────────────────────────────────────────────

    /// Whether a point is on the panel or its divider.
    pub(super) fn agent_view_contains(&self, x: f32, y: f32) -> bool {
        if self.agent_view_fills_tab() {
            return self
                .agent_view_panel_bounds()
                .is_some_and(|panel| panel.contains(x, y));
        }
        let Some(renderer) = &self.renderer else {
            return false;
        };
        let width = renderer.dock_width();
        if width <= 0.0 {
            return false;
        }
        let left = renderer.content_right();
        let grab = DIVIDER_GRAB * renderer.scale_factor();
        x >= left - grab && y >= renderer.content_top()
    }

    fn on_agent_view_divider(&self, x: f32) -> bool {
        // A panel filling the tab has no divider to drag.
        if self.agent_view_fills_tab() {
            return false;
        }
        let Some(renderer) = &self.renderer else {
            return false;
        };
        let left = renderer.content_right();
        (x - left).abs() <= DIVIDER_GRAB * renderer.scale_factor()
    }

    /// A press on the panel. Returns true when the panel took it.
    pub(super) fn handle_agent_view_press(
        &mut self,
        x: f32,
        y: f32,
        button: winit::event::MouseButton,
    ) -> bool {
        if !self.agent_view_contains(x, y) {
            // A press anywhere else gives the keyboard back to the terminals.
            if self.agent_view.state.has_keyboard() {
                self.set_agent_view_keyboard(false);
                self.request_agent_view_redraw();
            }
            return false;
        }
        if button != winit::event::MouseButton::Left {
            return true;
        }
        if self.on_agent_view_divider(x) {
            if let Some((room, scale)) = self.agent_dock_room() {
                self.agent_view.state.begin_resize(x, room, scale);
            }
            return true;
        }
        self.set_agent_view_keyboard(true);
        self.agent_view.state.focus_visible = false;
        let hit = self
            .agent_view
            .scene
            .as_ref()
            .and_then(|scene| scene.hit_at(x, y).cloned());
        match hit {
            Some(hit) if hit.target == Target::Composer => {
                self.agent_view.state.focus = Focus::Composer;
                self.agent_view.state.menu = None;
                let now = Instant::now();
                let double = self
                    .agent_view
                    .last_press
                    .is_some_and(|(at, last_x, last_y)| {
                        now.saturating_duration_since(at) <= DOUBLE_CLICK
                            && (x - last_x).abs() <= DOUBLE_CLICK_DISTANCE
                            && (y - last_y).abs() <= DOUBLE_CLICK_DISTANCE
                    });
                if double {
                    self.place_agent_caret(x, y, false);
                    let composer = self.agent_view.state.composer_mut();
                    let caret = composer.caret();
                    composer.select_word(caret);
                    self.agent_view.last_press = None;
                } else {
                    self.place_agent_caret(x, y, self.modifiers.shift_key());
                    self.agent_view.selecting = true;
                    self.agent_view.last_press = Some((now, x, y));
                }
            }
            Some(hit) => {
                if hit.focusable {
                    self.agent_view.state.focus = Focus::Target(hit.target.clone());
                }
                self.activate_agent_view_target(hit.target);
                // Pressing a button leaves typing where it was.
                if !matches!(self.agent_view.state.focus, Focus::Composer)
                    && self.agent_view.state.menu.is_none()
                {
                    self.agent_view.state.focus = Focus::Composer;
                }
            }
            None => self.agent_view.state.menu = None,
        }
        self.agent_view.state.touch_caret();
        self.request_agent_view_redraw();
        true
    }

    fn place_agent_caret(&mut self, x: f32, y: f32, extend: bool) {
        let Some(geometry) = self
            .agent_view
            .scene
            .as_ref()
            .and_then(|scene| scene.composer_text.clone())
        else {
            return;
        };
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let font_px = geometry.size.font_px;
        let measure = renderer.text_measure();
        let composer = self.agent_view.state.composer_mut();
        let layout = geometry.layout(composer.text(), measure);
        let mut width =
            |text: &str| measure.width(text, crate::ui::text_layout::UiFace::SANS, font_px);
        let offset = composer.offset_at(
            &layout,
            x - geometry.origin[0],
            y - geometry.origin[1],
            &mut width,
        );
        composer.place(offset, extend);
    }

    /// Pointer movement: resizing, selecting, or hover. Returns true when
    /// the panel used it.
    pub(super) fn handle_agent_view_motion(&mut self, x: f32, y: f32) -> bool {
        if self.agent_view.state.resizing() {
            let Some((room, scale)) = self.agent_dock_room() else {
                return true;
            };
            if self.agent_view.state.drag_resize(x, room, scale) {
                self.sync_agent_dock();
                self.request_agent_view_redraw();
            }
            self.set_agent_view_cursor(true);
            return true;
        }
        if self.agent_view.selecting {
            self.place_agent_caret(x, y, true);
            self.request_agent_view_redraw();
            return true;
        }
        let inside = self.agent_view_contains(x, y);
        let on_divider = inside && self.on_agent_view_divider(x);
        self.set_agent_view_cursor(on_divider);
        let hover = if inside && !on_divider {
            self.agent_view
                .scene
                .as_ref()
                .and_then(|scene| scene.hit_at(x, y))
                .map(|hit| hit.target.clone())
                .filter(|target| !matches!(target, Target::DismissMenu))
        } else {
            None
        };
        if hover != self.agent_view.state.hover {
            self.agent_view.state.hover = hover;
            self.request_agent_view_redraw();
        }
        inside
    }

    pub(super) fn handle_agent_view_release(&mut self) -> bool {
        let mut used = false;
        if self.agent_view.state.resizing() {
            self.agent_view.state.end_resize();
            used = true;
        }
        if self.agent_view.selecting {
            self.agent_view.selecting = false;
            used = true;
        }
        used
    }

    /// Ends pointer gestures, as when the window loses focus.
    pub(super) fn cancel_agent_view_gestures(&mut self) {
        self.agent_view.state.end_resize();
        self.agent_view.selecting = false;
        self.agent_view.state.hover = None;
    }

    /// Wheel over the panel scrolls its page. Returns true when it did.
    pub(super) fn handle_agent_view_wheel(
        &mut self,
        x: f32,
        y: f32,
        delta: winit::event::MouseScrollDelta,
    ) -> bool {
        if !self.agent_view_contains(x, y) {
            return false;
        }
        let Some(scene) = self.agent_view.scene.clone() else {
            return true;
        };
        let scale = self
            .renderer
            .as_ref()
            .map_or(1.0, |renderer| renderer.scale_factor());
        let pixels = match delta {
            winit::event::MouseScrollDelta::LineDelta(_, lines) => -lines * WHEEL_LINE * scale,
            winit::event::MouseScrollDelta::PixelDelta(position) => -(position.y as f32),
        };
        if !pixels.is_finite() || pixels == 0.0 {
            return true;
        }
        let max = (scene.feed_height - scene.feed_viewport.height).max(0.0);
        let now = Instant::now();
        if let Some(scroll) = self.agent_view_scroll_mut() {
            scroll.scroll_by(pixels, max, now);
        }
        self.request_agent_view_redraw();
        true
    }
}

/// Chords the composer edits with, which global shortcuts must not take
/// while it has the keyboard.
pub(super) fn composer_chord(key: &Key, modifiers: winit::keyboard::ModifiersState) -> bool {
    let control = modifiers.control_key();
    let others = modifiers.alt_key() || modifiers.super_key();
    if others {
        return false;
    }
    match key {
        Key::Named(named) => {
            let editing = matches!(
                named,
                NamedKey::ArrowLeft
                    | NamedKey::ArrowRight
                    | NamedKey::ArrowUp
                    | NamedKey::ArrowDown
                    | NamedKey::Home
                    | NamedKey::End
                    | NamedKey::Backspace
                    | NamedKey::Delete
                    | NamedKey::Enter
                    | NamedKey::Tab
                    | NamedKey::Escape
                    | NamedKey::PageUp
                    | NamedKey::PageDown
                    | NamedKey::Space
            );
            // Scrolling the terminal to its ends and jumping between
            // prompts stay global even while typing.
            let global = control
                && modifiers.shift_key()
                && matches!(
                    named,
                    NamedKey::Home
                        | NamedKey::End
                        | NamedKey::ArrowUp
                        | NamedKey::ArrowDown
                        | NamedKey::Tab
                );
            editing && !global
        }
        Key::Character(text) => {
            if !control {
                return true;
            }
            let lower = text.to_lowercase();
            if modifiers.shift_key() {
                lower == "z"
            } else {
                matches!(lower.as_str(), "a" | "c" | "v" | "x" | "y" | "z")
            }
        }
        _ => false,
    }
}

fn menu_kind_of(action: &MenuAction) -> MenuKind {
    match action {
        MenuAction::SetMode(_) => MenuKind::Mode,
        MenuAction::SetModel(_) => MenuKind::Model,
        MenuAction::SetFolder(_) => MenuKind::Folder,
        MenuAction::EndSession
        | MenuAction::OpenInTerminal
        | MenuAction::CopyConversation
        | MenuAction::HidePanel => MenuKind::More,
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}
