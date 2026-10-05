//! The agent features switch: off by default, and every entry point follows
//! it at runtime.

use super::*;
use crate::agents::config::{AGENT_FEATURES_OFF, AGENT_FEATURES_OFF_REPLY};
use crate::ui::settings::SettingsSection;

fn agent_command_count(app: &App) -> usize {
    app.command_palette
        .command_ids()
        .filter(|id| {
            id.starts_with("agent.")
                || id.starts_with("supervisor.")
                || *id == "core.toggle-transcript"
        })
        .count()
}

/// A window with one tab running a quiet shell.
fn app_with_shell(config: Config) -> App {
    let mut app = App::new_headless(config).unwrap();
    let mut tabs = TabManager::for_test(crate::test_support::sleeping_shell());
    tabs.new_tab().unwrap();
    app.tabs = Some(tabs);
    app
}

fn focused_integration(app: &App) -> &crate::session::terminal_state::TerminalIntegration {
    app.tabs
        .as_ref()
        .unwrap()
        .active_tab()
        .unwrap()
        .focused_integration()
        .unwrap()
}

fn set_agent_features(app: &mut App, enabled: bool) {
    app.apply_settings_action(SettingsAction::SetAgentFeaturesEnabled(enabled));
    assert_eq!(app.agent_features_enabled(), enabled);
}

/// A complete patch, as an agent prints one.
const PATCH: &[u8] = b"--- a/file\n+++ b/file\n@@ -1 +1 @@\n-old\n+new\nfinished\n";

/// Make the focused terminal one Termeric started for Codex, and return
/// what feeds its output, in place of its PTY.
fn codex_terminal(app: &mut App) -> std::sync::mpsc::Sender<termeric_core::pty::PtyEvent> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let pane = app
        .tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .focused_pane_mut()
        .unwrap();
    pane.launch = crate::workspace::WorkspaceLaunch::Plugin {
        plugin_id: "codex".into(),
        profile: None,
    };
    pane.pty_rx = receiver;
    sender
}

fn print(app: &mut App, output: &std::sync::mpsc::Sender<termeric_core::pty::PtyEvent>) {
    output
        .send(termeric_core::pty::PtyEvent::Output(PATCH.to_vec()))
        .unwrap();
    app.process_pty_events();
}

/// The unreviewed diffs the active tab's badge shows, and those it holds.
fn diff_counts(app: &App) -> (usize, usize) {
    let tabs = app.tabs.as_ref().unwrap();
    (
        tabs.tab_info()[tabs.active_index()].unreviewed_diff_count,
        tabs.active_tab().unwrap().unreviewed_diff_count(),
    )
}

#[test]
fn a_fresh_configuration_offers_no_agent_features() {
    assert!(!Config::default().agent_features_enabled());
    let app = App::new_headless(Config::default()).unwrap();
    assert!(!app.agent_features_enabled());
    for (id, _) in crate::agents::builtin_agent_names() {
        assert!(!app.plugin_manager.contains(id), "{id} is registered");
    }
    assert_eq!(agent_command_count(&app), 0);
    assert_eq!(app.next_agent_scan_deadline(), None);
}

#[test]
fn agent_shortcuts_reach_the_terminal_while_agent_features_are_off() {
    let mut app = App::new_headless_running(
        "stty raw -echo; printf READY; dd bs=1 count=2 2>/dev/null | od -An -tx1; sleep 0.1",
        "agent-chords",
    );
    app.wait_for_screen_text("READY");
    let toasts = app.toasts.iter().count();
    app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
    for key in ["A", "I"] {
        assert!(
            !app.handle_bound_key(&winit::keyboard::Key::Character(key.into())),
            "Ctrl+Shift+{key} is left to the terminal"
        );
    }
    // What the keyboard handler then writes for each key.
    app.write_to_pty(b"\x01\x09");
    app.wait_for_screen_text("01 09");
    assert_eq!(app.toasts.iter().count(), toasts, "no notice");
    assert!(!app.agent_view.state.is_open());
    assert!(!app.command_palette.is_active());
    // However else the actions are asked for, they do nothing.
    for action in [Action::ToggleAgentView, Action::NewAgent] {
        assert!(!app.handle_action(action));
    }
    assert_eq!(app.toasts.iter().count(), toasts);

    set_agent_features(&mut app, true);
    assert!(app.handle_bound_key(&winit::keyboard::Key::Character("A".into())));
    assert!(app.agent_view.state.is_open());
}

#[test]
fn agent_commands_say_how_to_turn_agent_features_on() {
    let mut app = App::new_headless(Config::default()).unwrap();
    for id in ["agent.panel", "agent.view-new", "supervisor.start"] {
        assert!(app.run_agent_command(id), "{id} is an agent command");
        assert_eq!(latest_toast(&app), Some(AGENT_FEATURES_OFF));
    }
    assert!(!app.agents_panel.is_open());
    assert!(!app.agent_view.state.is_open());
    assert!(!app.supervisor_running());
}

#[test]
fn terminals_keep_no_agent_while_agent_features_are_off() {
    let mut app = app_with_shell(Config::default());
    // What a pane arriving from a window with agent features on carries.
    let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
    let state = tab.focused_integration_mut().unwrap();
    state.set_agent(Some(crate::agents::AgentSession {
        agent_id: "codex".into(),
        launched: false,
        profile: None,
        process: None,
    }));
    state.agent_status = crate::agents::AgentStatus::after(
        None,
        crate::agents::AgentReport::from_wire(
            "codex".into(),
            crate::control::protocol::AgentEventKind::Working,
            None,
            None,
            None,
        ),
        Instant::now(),
    );
    assert!(
        app.tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .agent_badge()
            .is_some()
    );

    assert!(app.refresh_agent_sessions());
    let state = focused_integration(&app);
    assert!(state.agent.is_none() && state.agent_status.is_none());
    assert!(
        app.tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .agent_badge()
            .is_none()
    );
    assert!(!app.refresh_agent_sessions(), "nothing left to forget");
    assert_eq!(app.next_agent_scan_deadline(), None, "no scan is scheduled");
}

#[test]
fn hook_reports_are_refused_while_agent_features_are_off() {
    let mut app = app_with_shell(Config::default());
    let terminal_id = app
        .tabs
        .as_ref()
        .unwrap()
        .active_tab()
        .unwrap()
        .lone_pty()
        .unwrap()
        .terminal_id()
        .unwrap()
        .to_owned();
    let report = || {
        crate::agents::AgentReport::from_wire(
            "codex".into(),
            crate::control::protocol::AgentEventKind::NeedsInput,
            None,
            None,
            None,
        )
    };
    assert_eq!(
        app.record_agent_report("1.00", report(), None),
        Ok(false),
        "another process's terminal is still not this window's"
    );
    assert_eq!(
        app.record_agent_report(&terminal_id, report(), None),
        Err(AGENT_FEATURES_OFF_REPLY.to_owned())
    );
    assert!(focused_integration(&app).agent_status.is_none());

    set_agent_features(&mut app, true);
    assert_eq!(
        app.record_agent_report(&terminal_id, report(), None),
        Ok(true)
    );
}

#[test]
fn programs_asking_for_agents_are_told_agent_features_are_off() {
    let mut app = app_with_shell(Config::default());
    let tab_id = app.tabs.as_ref().unwrap().active_tab_id().unwrap() as u64;
    assert!(app.control_agents(1).is_empty());
    assert_eq!(
        app.submit_control_text_to_agent(tab_id, 0, "codex", "hello", None, None),
        Err(AGENT_FEATURES_OFF_REPLY.to_owned())
    );
    assert_eq!(
        app.read_agent_view_conversation(1),
        Err(AGENT_FEATURES_OFF_REPLY.to_owned())
    );
    assert_eq!(
        app.submit_to_agent_view(1, "hello", None, None),
        Err(AGENT_FEATURES_OFF_REPLY.to_owned())
    );
    let error = app
        .validate_plugin_launch_profile("codex", None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("Codex is a coding agent"), "{error}");
    assert!(error.contains("[agent_features]"), "{error}");
}

#[test]
fn the_switch_turns_every_entry_point_on_and_off_at_runtime() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let on_page = |app: &mut App| {
        app.settings_panel.open_to_section(SettingsSection::Agents);
        app.settings_panel
            .build_entries(&app.config, &app.shortcut_registry)
            .into_iter()
            .filter(|entry| !entry.is_header)
            .map(|entry| entry.label)
            .collect::<Vec<_>>()
    };
    let listed_shortcuts = |app: &App| {
        app.shortcuts_panel.is_visible_action(
            crate::input::keybindings::BindableAction::ToggleAgentView,
            &app.shortcut_registry,
        )
    };
    assert_eq!(on_page(&mut app), ["Agent Features"]);
    assert!(!listed_shortcuts(&app));

    set_agent_features(&mut app, true);
    assert!(app.plugin_manager.contains("codex"));
    assert!(agent_command_count(&app) > 0);
    assert!(app.next_agent_scan_deadline().is_some());
    let page = on_page(&mut app);
    assert_eq!(page[0], "Agent Features");
    assert!(page.contains(&"Codex") && page.contains(&"Supervisor"));
    app.settings_panel.close();
    app.shortcuts_panel.open();
    assert!(listed_shortcuts(&app));
    app.shortcuts_panel.close_immediately();
    app.handle_action(Action::ToggleAgentView);
    assert!(app.agent_view.state.is_open());
    app.agents_panel.toggle(&[]);
    assert!(app.agents_panel.is_open());

    // Turning them off closes what was open: the panel's blank session ends
    // with it.
    set_agent_features(&mut app, false);
    assert!(!app.agent_view.state.is_open());
    assert!(app.agent_view.sessions.is_empty());
    assert!(!app.agents_panel.is_open());
    assert!(!app.plugin_manager.contains("codex"));
    assert_eq!(agent_command_count(&app), 0);
    assert_eq!(on_page(&mut app), ["Agent Features"]);
}

#[test]
fn the_settings_switch_saves_only_its_own_key() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    let original = "# Mine.\n[window]\npadding = 4\n";
    std::fs::write(&path, original).unwrap();
    let mut app = App::new_headless(Config::load_from(&path).unwrap()).unwrap();
    app.config_path = path.clone();

    set_agent_features(&mut app, true);
    app.apply_settings_action(SettingsAction::SaveConfig);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        format!("{original}\n[agent_features]\nenabled = true\n")
    );
    assert!(Config::load_from(&path).unwrap().agent_features_enabled());

    set_agent_features(&mut app, false);
    app.apply_settings_action(SettingsAction::SaveConfig);
    assert!(!Config::load_from(&path).unwrap().agent_features_enabled());
}

#[test]
fn agent_features_live_in_their_own_table() {
    let config: Config = toml::from_str("[agent_features]\nenabled = true\n").unwrap();
    assert!(config.agent_features_enabled());
    // `[agents]` holds only agent entries, so a switch there is no setting.
    assert!(toml::from_str::<Config>("[agents]\nenabled = true\n").is_err());
    assert!(toml::from_str::<Config>("[agent_features]\nenable = true\n").is_err());
}

#[test]
fn no_diff_is_captured_or_counted_while_agent_features_are_off() {
    let mut app = app_with_shell(Config::default());
    let output = codex_terminal(&mut app);
    print(&mut app, &output);
    assert_eq!(diff_counts(&app), (0, 0), "nothing is captured while off");

    set_agent_features(&mut app, true);
    print(&mut app, &output);
    assert_eq!(diff_counts(&app), (1, 1));

    // Turning them off forgets the diff rather than hiding it, so it cannot
    // come back unreviewed, or keep counting, with nothing to review it.
    set_agent_features(&mut app, false);
    assert_eq!(diff_counts(&app), (0, 0));
    print(&mut app, &output);
    assert_eq!(diff_counts(&app), (0, 0));

    set_agent_features(&mut app, true);
    assert_eq!(diff_counts(&app), (0, 0));
    print(&mut app, &output);
    assert_eq!(diff_counts(&app), (1, 1));
}

#[test]
fn a_tab_moved_in_from_a_window_with_agent_features_on_brings_no_diffs() {
    let mut source = app_with_shell(Config::default());
    set_agent_features(&mut source, true);
    let output = codex_terminal(&mut source);
    print(&mut source, &output);
    assert_eq!(diff_counts(&source), (1, 1));

    let mut destination = app_with_shell(Config::default());
    let tab = source
        .tabs
        .as_mut()
        .unwrap()
        .detach_active_tab_for_window_transfer()
        .unwrap();
    destination
        .tabs
        .as_mut()
        .unwrap()
        .attach_window_transferred_tab(tab);
    assert_eq!(diff_counts(&destination).0, 0, "no badge, even at once");
    assert!(destination.refresh_agent_sessions());
    assert_eq!(diff_counts(&destination), (0, 0));
    print(&mut destination, &output);
    assert_eq!(diff_counts(&destination), (0, 0));
}
