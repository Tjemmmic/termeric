use std::path::PathBuf;
use std::time::Instant;

use serde_json::json;

use super::*;
use crate::agent_view::event::{
    AgentEvent, ApprovalRequest, AssistantMessage, ContentBlock, Handshake, RememberOption, Thread,
    ToolResult, TurnOutcome,
};
use crate::agent_view::session::AgentSession;
use crate::agent_view::transcript::Transcript;
use crate::config::theme::Theme;
use crate::ui::agent_view::scene::{ExpandKey, Node};
use crate::ui::agent_view::{OpenMenu, Screen};
use crate::ui::text_layout::FixedMeasure;

const PANEL: Bounds = Bounds::new(1000.0, 34.0, 440.0, 866.0);

fn measure() -> FixedMeasure {
    FixedMeasure { advance: 7.0 }
}

fn colors() -> Colors {
    Colors::from_theme(&Theme::default(), false)
}

fn new_session_info() -> NewSessionInfo {
    NewSessionInfo {
        cwd: PathBuf::from("/home/me/dev/termeric"),
        cwd_source: "From the focused terminal".into(),
        cwd_choices: vec![PathBuf::from("/home/me/dev/other")],
        availability: AgentAvailability::Ready,
    }
}

fn scene_for(state: &AgentViewState, sessions: &[&AgentSession], panel: Bounds) -> Scene {
    scene_with_request(state, sessions, panel, None)
}

fn scene_with_request(
    state: &AgentViewState,
    sessions: &[&AgentSession],
    panel: Bounds,
    supervisor_request: Option<SupervisorRequest>,
) -> Scene {
    let colors = colors();
    let input = Input {
        panel,
        scale: 1.0,
        // After the opening slide.
        now: Instant::now() + std::time::Duration::from_secs(1),
        colors: &colors,
        state,
        sessions,
        suggestions: Suggestions {
            selection_lines: Some(3),
            last_command: Some(("cargo test".into(), 21, true)),
        },
        banner: None,
        terminal_agents: &[],
        past: &[],
        supervisor: None,
        new_session: new_session_info(),
        home: Some(PathBuf::from("/home/me")),
        allow_bypass: false,
        supervisor_request,
    };
    let scene = build(&input, &mut measure());
    assert_unique_targets(&scene);
    scene
}

/// Hit targets name accessibility nodes and focus stops, so a scene may
/// hold each at most once.
fn assert_unique_targets(scene: &Scene) {
    let mut seen = std::collections::HashSet::new();
    for hit in &scene.hits {
        assert!(seen.insert(&hit.target), "{:?} appears twice", hit.target);
    }
}

fn message(transcript: &mut Transcript, id: &str, blocks: Vec<ContentBlock>) {
    transcript.apply(AgentEvent::Message {
        thread: Thread::Main,
        message: AssistantMessage {
            id: id.into(),
            blocks,
            error: None,
        },
    });
}

fn tool_use(id: &str, name: &str, input: serde_json::Value) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.into(),
        name: name.into(),
        input,
    }
}

fn result(transcript: &mut Transcript, id: &str) {
    transcript.apply(AgentEvent::ToolResult {
        thread: Thread::Main,
        result: ToolResult {
            tool_use_id: id.into(),
            content: "ok".into(),
            is_error: false,
            detail: None,
        },
    });
}

/// A conversation with exploration, an edit, and a command awaiting approval.
fn busy_session() -> AgentSession {
    let mut transcript = Transcript::new(PermissionMode::AskBeforeEdits);
    transcript.apply(AgentEvent::Ready(Handshake::default()));
    transcript.push_user("Fix the scrollbar capture test.".into(), Vec::new());
    message(
        &mut transcript,
        "m1",
        vec![ContentBlock::Text(
            "I'll look at **focus** handling in `refresh_focus_surfaces()`.".into(),
        )],
    );
    for (id, name, input) in [
        (
            "r1",
            "Read",
            json!({"file_path": "/home/me/dev/termeric/src/app/mouse.rs"}),
        ),
        ("r2", "Grep", json!({"pattern": "scrollbar_capture"})),
        (
            "r3",
            "Read",
            json!({"file_path": "/home/me/dev/termeric/src/app/focus.rs"}),
        ),
    ] {
        message(&mut transcript, id, vec![tool_use(id, name, input)]);
        result(&mut transcript, id);
    }
    message(
        &mut transcript,
        "e1",
        vec![tool_use(
            "e1",
            "Edit",
            json!({"file_path": "/home/me/dev/termeric/src/app/focus.rs", "old_string": "a\nb", "new_string": "a\nc\nd"}),
        )],
    );
    result(&mut transcript, "e1");
    message(
        &mut transcript,
        "b1",
        vec![tool_use(
            "b1",
            "Bash",
            json!({"command": "cargo test -p termeric scrollbar", "description": "Run the scrollbar tests"}),
        )],
    );
    transcript.apply(AgentEvent::ApprovalRequested(ApprovalRequest {
        request_id: "req".into(),
        tool_use_id: Some("b1".into()),
        tool_name: "Bash".into(),
        input: json!({"command": "cargo test -p termeric scrollbar"}),
        title: None,
        description: Some("Run the scrollbar tests".into()),
        reason: None,
        remember: Some(RememberOption {
            label: "Always allow cargo test in this project".into(),
            updates: json!([]),
        }),
        from_sub_agent: false,
    }));
    AgentSession::detached(PathBuf::from("/home/me/dev/termeric"), transcript)
}

fn open_on(session: &AgentSession) -> AgentViewState {
    let mut state = AgentViewState::default();
    state.open();
    state.show(session.id());
    state
}

fn targets(scene: &Scene) -> Vec<Target> {
    scene.hits.iter().map(|hit| hit.target.clone()).collect()
}

#[test]
fn a_pending_approval_offers_every_answer_and_is_in_view() {
    let session = busy_session();
    let state = open_on(&session);
    let scene = scene_for(&state, &[&session], PANEL);
    let request_id = "req".to_owned();
    for target in [
        Target::Allow {
            request_id: request_id.clone(),
        },
        Target::AllowAlways {
            request_id: request_id.clone(),
        },
        Target::Deny {
            request_id: request_id.clone(),
        },
    ] {
        let hit = scene
            .hit_for(&target)
            .unwrap_or_else(|| panic!("{target:?} missing"));
        // The conversation follows its end, so the approval is visible.
        assert!(hit.visible_bounds().is_some(), "{target:?} scrolled away");
        assert_eq!(
            scene
                .hit_at(hit.bounds.x + 2.0, hit.bounds.y + 2.0)
                .unwrap()
                .target,
            target
        );
    }
    // Header, composer, and suggestions are all reachable.
    let all = targets(&scene);
    for target in [
        Target::ShowSessions,
        Target::NewSession,
        Target::Composer,
        Target::AttachSelection,
        Target::AttachLastCommand,
        Target::OpenMenu(MenuKind::Mode),
        Target::OpenMenu(MenuKind::Model),
        Target::Toggle(ExpandKey::Explored("r1".into())),
        Target::Toggle(ExpandKey::Tool("e1".into())),
    ] {
        assert!(all.contains(&target), "{target:?} missing from {all:?}");
    }
    // A running turn with an empty composer offers Stop, not Send.
    assert!(all.contains(&Target::Stop));
    assert!(!all.contains(&Target::Send));
}

#[test]
fn tab_order_reads_header_then_conversation_then_composer() {
    let session = busy_session();
    let state = open_on(&session);
    let scene = scene_for(&state, &[&session], PANEL);
    let order: Vec<&Target> = scene.focus_order().collect();
    let position = |target: &Target| {
        order
            .iter()
            .position(|candidate| *candidate == target)
            .unwrap()
    };
    assert!(
        position(&Target::ShowSessions)
            < position(&Target::Allow {
                request_id: "req".into()
            })
    );
    assert!(
        position(&Target::Deny {
            request_id: "req".into()
        }) < position(&Target::Composer)
    );
    assert!(position(&Target::Composer) < position(&Target::Stop));
    // Every hit and every read text lies inside the panel.
    for hit in &scene.hits {
        assert!(
            hit.bounds.x >= PANEL.x && hit.bounds.right() <= PANEL.right() + 0.5,
            "{hit:?}"
        );
    }
    assert!(scene.nodes.iter().any(
        |node| matches!(node, Node::Text(text) if text.text.contains("refresh_focus_surfaces()"))
    ));
}

#[test]
fn narrow_panels_wrap_buttons_instead_of_overflowing() {
    let session = busy_session();
    let state = open_on(&session);
    let narrow = Bounds::new(1000.0, 0.0, 320.0, 800.0);
    let scene = scene_for(&state, &[&session], narrow);
    let allow = scene
        .hit_for(&Target::Allow {
            request_id: "req".into(),
        })
        .unwrap();
    let always = scene
        .hit_for(&Target::AllowAlways {
            request_id: "req".into(),
        })
        .unwrap();
    assert!(
        always.bounds.y > allow.bounds.y,
        "the long button moved to its own row"
    );
    for hit in &scene.hits {
        assert!(hit.bounds.right() <= narrow.right() + 0.5, "{hit:?}");
    }
}

#[test]
fn scrolling_away_offers_a_way_back_and_hides_offscreen_text() {
    let session = busy_session();
    let mut state = open_on(&session);
    state
        .conversation_mut(session.id())
        .scroll
        .jump_to(0.0, 10_000.0);
    let small = Bounds::new(1000.0, 0.0, 440.0, 420.0);
    let scene = scene_for(&state, &[&session], small);
    assert!(scene.feed_height > scene.feed_viewport.height);
    assert!(targets(&scene).contains(&Target::ScrollToEnd));
    let allow = scene
        .hit_for(&Target::Allow {
            request_id: "req".into(),
        })
        .unwrap();
    assert!(allow.visible_bounds().is_none());
    assert!(
        scene
            .hit_at(allow.bounds.x + 2.0, allow.bounds.y + 2.0)
            .map(|hit| &hit.target)
            != Some(&allow.target)
    );
    for node in &scene.nodes {
        if let Node::Text(text) = node
            && text.clip.is_some()
        {
            assert!(text.bounds.intersect(scene.feed_viewport).is_some());
        }
    }
}

#[test]
fn an_open_menu_lists_its_choices_above_everything() {
    let mut transcript = Transcript::new(PermissionMode::AskBeforeEdits);
    transcript.apply(AgentEvent::Ready(Handshake {
        models: vec![crate::agent_view::event::ModelChoice {
            value: "sonnet".into(),
            display_name: "Sonnet 5".into(),
            description: "Everyday".into(),
        }],
        ..Handshake::default()
    }));
    let session = AgentSession::detached(PathBuf::from("/w"), transcript);
    let mut state = open_on(&session);
    state.set_keyboard(true);
    state.menu = Some(OpenMenu {
        kind: MenuKind::Mode,
        anchor: Bounds::new(1020.0, 820.0, 120.0, 26.0),
        highlighted: 0,
    });
    let scene = scene_for(&state, &[&session], PANEL);
    let modes: Vec<_> = scene
        .hits
        .iter()
        .filter_map(|hit| match &hit.target {
            Target::Menu(MenuAction::SetMode(mode)) => Some((*mode, hit.role)),
            _ => None,
        })
        .collect();
    // Every mode but the one that skips every check, which needs an opt-in.
    assert_eq!(
        modes.iter().map(|(mode, _)| *mode).collect::<Vec<_>>(),
        PermissionMode::selectable(false)
    );
    assert_eq!(modes.len(), 5);
    assert_eq!(
        modes[0],
        (
            PermissionMode::AskBeforeEdits,
            HitRole::MenuItem { checked: true }
        )
    );
    // The menu sits above its anchor, since the composer is low in the panel.
    let first = scene
        .hit_for(&Target::Menu(MenuAction::SetMode(
            PermissionMode::AskBeforeEdits,
        )))
        .unwrap();
    assert!(first.bounds.bottom() <= 820.0);
    assert_eq!(
        scene
            .hit_at(first.bounds.x + 4.0, first.bounds.y + 4.0)
            .unwrap()
            .target,
        first.target
    );
    // Elsewhere, a click lands on the dismiss layer.
    assert_eq!(
        scene.hit_at(PANEL.x + 5.0, PANEL.y + 5.0).unwrap().target,
        Target::DismissMenu
    );
    let items = menu_items(
        &Input {
            panel: PANEL,
            scale: 1.0,
            now: Instant::now(),
            colors: &colors(),
            state: &state,
            sessions: &[&session],
            suggestions: Suggestions::default(),
            banner: None,
            terminal_agents: &[],
            past: &[],
            supervisor: None,
            new_session: new_session_info(),
            home: None,
            allow_bypass: false,
            supervisor_request: None,
        },
        MenuKind::Model,
    );
    let opted_in = menu_items(
        &Input {
            panel: PANEL,
            scale: 1.0,
            now: Instant::now(),
            colors: &colors(),
            state: &state,
            sessions: &[&session],
            suggestions: Suggestions::default(),
            banner: None,
            terminal_agents: &[],
            past: &[],
            supervisor: None,
            new_session: new_session_info(),
            home: None,
            allow_bypass: true,
            supervisor_request: None,
        },
        MenuKind::Mode,
    );
    assert_eq!(
        opted_in.last().map(|item| item.action.clone()),
        Some(MenuAction::SetMode(PermissionMode::BypassPermissions))
    );
    assert_eq!(items[0].text, "Sonnet 5");
}

#[test]
fn the_sessions_page_groups_by_what_needs_attention() {
    let waiting = busy_session();
    let mut done = Transcript::new(PermissionMode::AskBeforeEdits);
    done.apply(AgentEvent::Ready(Handshake::default()));
    done.push_user("Summarize the diff".into(), Vec::new());
    message(
        &mut done,
        "m",
        vec![ContentBlock::Text("Three files changed.".into())],
    );
    done.apply(AgentEvent::TurnFinished(TurnOutcome {
        success: true,
        ..TurnOutcome::default()
    }));
    let finished = AgentSession::detached(PathBuf::from("/w"), done);
    let mut state = open_on(&waiting);
    state.screen = Screen::Sessions;
    let scene = scene_for(&state, &[&finished, &waiting], PANEL);
    let headings: Vec<&str> = scene
        .nodes
        .iter()
        .filter_map(|node| match node {
            Node::Text(text) if text.heading => Some(text.text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(headings, ["Needs you", "Recent"]);
    let rows: Vec<_> = scene
        .hits
        .iter()
        .filter(|hit| matches!(hit.target, Target::OpenSession(_)))
        .collect();
    assert_eq!(rows[0].target, Target::OpenSession(waiting.id()));
    assert!(
        rows[0]
            .label
            .contains("Wants to run cargo test -p termeric scrollbar")
    );
    assert!(rows[1].label.contains("Three files changed."));
    // No composer on this page.
    assert!(!targets(&scene).contains(&Target::Composer));
}

#[test]
fn the_new_session_page_offers_folder_start_and_the_terminal() {
    let mut state = AgentViewState::default();
    state.open();
    state.new_session.composer.insert("Explain the renderer");
    let scene = scene_for(&state, &[], PANEL);
    let all = targets(&scene);
    for target in [
        Target::OpenMenu(MenuKind::Folder),
        Target::OpenInTerminal,
        Target::StartSession,
        Target::ShowSessions,
    ] {
        assert!(all.contains(&target), "{target:?} missing");
    }
    let folder = scene.hit_for(&Target::OpenMenu(MenuKind::Folder)).unwrap();
    assert!(folder.label.contains("~/dev/termeric"));
    assert!(scene.caret.is_some());
    assert!(scene.composer_text.is_some());
}

#[test]
fn the_model_chip_names_the_chosen_model() {
    let mut state = AgentViewState::default();
    state.open();
    let chip = |state: &AgentViewState, sessions: &[&AgentSession]| {
        scene_for(state, sessions, PANEL)
            .hit_for(&Target::OpenMenu(MenuKind::Model))
            .unwrap()
            .label
            .clone()
    };
    assert_eq!(chip(&state, &[]), "Model: Default model");
    state.new_session.model = Some("haiku".into());
    assert_eq!(chip(&state, &[]), "Model: Haiku");
    // Once any session has listed the account's models, their names apply.
    let mut transcript = Transcript::new(PermissionMode::AskBeforeEdits);
    transcript.apply(AgentEvent::Ready(Handshake {
        models: vec![ModelChoice {
            value: "haiku".into(),
            display_name: "Haiku 4.5".into(),
            description: String::new(),
        }],
        ..Handshake::default()
    }));
    let session = AgentSession::detached(PathBuf::from("/home/me"), transcript);
    assert_eq!(chip(&state, &[&session]), "Model: Haiku 4.5");
}

#[test]
fn caret_movement_follows_text_changed_since_the_last_frame() {
    let mut state = AgentViewState::default();
    state.open();
    state
        .new_session
        .composer
        .insert("first line\nsecond ééé line");
    let scene = scene_for(&state, &[], PANEL);
    let geometry = scene.composer_text.clone().unwrap();
    // A key in the same batch of events shortens the text before Up/Down.
    state.new_session.composer.clear();
    state.new_session.composer.insert("é\né");
    state.new_session.composer.place(0, false);
    let mut measure = measure();
    let layout = geometry.layout(state.new_session.composer.text(), &mut measure);
    let mut width = |text: &str| text.chars().count() as f32 * 7.0;
    let composer = &mut state.new_session.composer;
    assert!(composer.move_vertically(&layout, 1, false, &mut width));
    assert!(composer.text().is_char_boundary(composer.caret()));
    composer.insert("x");
    assert_eq!(composer.text(), "é\nxé");
}

fn session_with_commands(prompt: Option<&str>) -> AgentSession {
    let mut transcript = Transcript::new(PermissionMode::AskBeforeEdits);
    transcript.apply(AgentEvent::Ready(Handshake {
        commands: ["compact", "context", "review"]
            .into_iter()
            .map(|name| crate::agent_view::event::SlashCommand {
                name: name.into(),
                description: format!("What /{name} does"),
                argument_hint: if name == "review" {
                    "<pr>".into()
                } else {
                    String::new()
                },
            })
            .collect(),
        ..Handshake::default()
    }));
    if let Some(prompt) = prompt {
        transcript.push_user(prompt.into(), Vec::new());
    }
    AgentSession::detached(PathBuf::from("/w"), transcript)
}

#[test]
fn typing_a_slash_lists_the_agents_matching_commands() {
    let session = session_with_commands(None);
    let mut state = open_on(&session);
    state.set_keyboard(true);
    state.composer_mut().insert("/co");
    let scene = scene_for(&state, &[&session], PANEL);
    let listed: Vec<_> = scene
        .hits
        .iter()
        .filter_map(|hit| match &hit.target {
            Target::Command(name) => Some((name.as_str(), hit.role)),
            _ => None,
        })
        .collect();
    assert_eq!(
        listed,
        [
            ("compact", HitRole::Option { selected: true }),
            ("context", HitRole::Option { selected: false }),
        ]
    );
    // The list sits above the message box, which keeps the keyboard.
    let composer = scene.hit_for(&Target::Composer).unwrap();
    let first = scene.hit_for(&Target::Command("compact".into())).unwrap();
    assert!(first.bounds.bottom() <= composer.bounds.y);

    // Past the command's name, or without the keyboard, it is gone.
    state.composer_mut().insert(" now");
    let scene = scene_for(&state, &[&session], PANEL);
    assert!(
        !scene
            .hits
            .iter()
            .any(|hit| matches!(hit.target, Target::Command(_)))
    );
}

#[test]
fn only_a_command_the_agent_did_not_list_offers_its_terminal() {
    for (prompt, offered) in [
        ("/permissions", true),
        ("/compact", false),
        ("/etc/hosts is broken", false),
        ("hello", false),
    ] {
        let session = session_with_commands(Some(prompt));
        let state = open_on(&session);
        let scene = scene_for(&state, &[&session], PANEL);
        assert_eq!(
            targets(&scene).contains(&Target::RunInTerminal),
            offered,
            "{prompt}"
        );
    }
}

#[test]
fn a_supervisor_message_waits_as_a_card_in_its_conversation_only() {
    let supervisor = session_with_commands(Some("Call list_agents"));
    let worker = session_with_commands(Some("Fix the build"));
    let request = SupervisorRequest {
        session: supervisor.id(),
        recipient: "Codex".into(),
        text: "yes, run it".into(),
    };
    let decisions = |state: &AgentViewState| {
        let scene =
            scene_with_request(state, &[&supervisor, &worker], PANEL, Some(request.clone()));
        let all = targets(&scene);
        (
            all.contains(&Target::SupervisorSend),
            all.contains(&Target::SupervisorDecline),
            scene
                .nodes
                .iter()
                .any(|node| matches!(node, Node::Text(text) if text.text == "yes, run it")),
        )
    };
    assert_eq!(decisions(&open_on(&supervisor)), (true, true, true));
    assert_eq!(decisions(&open_on(&worker)), (false, false, false));
}
