use super::*;
use crate::plugin::hooks::{Plugin, PluginCommand};

fn overflow_drag_fixture() -> App {
    let mut app = scrollbar_owner_fixture();
    let tabs = app.tabs.as_mut().unwrap();
    tabs.new_tab().unwrap();
    tabs.switch_to(0);
    app.tab_dragging = Some(TabDrag {
        tab_id: tabs.active_tab_id().unwrap(),
        grab: None,
        live: None,
    });
    app
}

fn live_tab_drag_fixture() -> App {
    let mut app = overflow_drag_fixture();
    for (index, tab) in app
        .tabs
        .as_mut()
        .unwrap()
        .all_tabs_mut()
        .iter_mut()
        .enumerate()
    {
        tab.live_domain = Some(crate::tabs::LiveDomainTab {
            domain_id: "drag-domain".into(),
            tab_id: index as u32 + 10,
            pane_ids: HashMap::new(),
        });
    }
    app.tab_dragging.as_mut().unwrap().live = Some(LiveDomainTabDrag {
        domain_id: "drag-domain".into(),
        remote_tab_id: 10,
        initial_index: 0,
    });
    app
}

fn scrollbar_owner_fixture() -> App {
    let mut app = App::new_headless(Config::default()).unwrap();
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 5".into()],
        env: Default::default(),
    };
    let mut tabs = TabManager::new(80, 24, shell, 100, 1024 * 1024);
    for _ in 0..2 {
        tabs.new_tab().unwrap();
        tabs.active_tab_mut()
            .unwrap()
            .focused_terminal_mut()
            .unwrap()
            .process(b"history\r\n".repeat(80).as_slice());
    }
    tabs.switch_to(0);
    app.tabs = Some(tabs);
    app
}

fn divider_owner_fixture() -> App {
    let mut app = scrollbar_owner_fixture();
    let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
    let tab_id = tab.id;
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 5".into()],
        env: Default::default(),
    };
    let panes = tab.panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
    panes.split(SplitDirection::Vertical).unwrap();
    let divider = panes.get_dividers()[0];
    app.divider_dragging = Some(DividerDrag {
        tab_id,
        domain: None,
        target: divider.target,
        last_ratio_millis: divider.ratio_millis,
    });
    app
}

fn domain_divider_pending_fixture() -> App {
    use crate::domain::model::{DomainSnapshot, LivePaneLayout, LiveTab};
    let mut tab = LiveTab::fixture(
        10,
        LivePaneLayout::split(
            44,
            crate::panes::SplitDirection::Vertical,
            500,
            LivePaneLayout::pane(100),
            LivePaneLayout::pane(101),
        ),
    );
    tab.focused_pane = 100;
    let snapshot = DomainSnapshot::fixture("divider-pending", 10, vec![tab]);
    let relay = crate::domain::DomainRelayCommand::local("/bin/true");
    let mut tabs = TabManager::new(80, 24, ShellConfig::default(), 100, 1024 * 1024);
    tabs.attach_domain(&snapshot, &relay).unwrap();
    let tab = tabs.active_tab_mut().unwrap();
    tab.panes.resize(
        crate::terminal::geometry::TerminalArea::from_available_pixels(800.0, 480.0, 10.0, 20.0),
    );
    let drag = DividerDrag {
        tab_id: tab.id,
        domain: Some(("divider-pending".into(), 10)),
        target: crate::panes::DividerTarget::Domain(44),
        last_ratio_millis: 500,
    };
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    app.divider_dragging = Some(drag);
    app.live_domain_relay_command = Some(relay);
    app.live_domain_topologies
        .insert(snapshot.id.clone(), (&snapshot).into());
    app
}

#[test]
fn native_window_title_tracks_the_active_tab_without_losing_product_identity() {
    assert_eq!(native_window_title(false, None), "Termeric");
    assert_eq!(native_window_title(false, Some("  ")), "Termeric");
    assert_eq!(
        native_window_title(false, Some("build; tests 🌱")),
        "build; tests 🌱 — Termeric"
    );
    assert_eq!(
        native_window_title(true, Some("server")),
        "server — Termeric Quick Access"
    );
}

#[derive(Debug)]
struct ThirdPartyReservedSuffixFixture;

impl Plugin for ThirdPartyReservedSuffixFixture {
    fn id(&self) -> &str {
        "third-party"
    }

    fn name(&self) -> &str {
        "Third-party reserved suffix fixture"
    }

    fn on_hook(
        &mut self,
        hook: &crate::plugin::Hook,
        _context: &crate::plugin::PluginContext,
    ) -> Option<PluginAction> {
        if matches!(
            hook,
            crate::plugin::Hook::CommandInvoked { command_id }
                if command_id == "third-party.review-diff"
        ) {
            Some(crate::plugin::PluginAction::ShowMessage {
                message: "third-party command routed".into(),
            })
        } else {
            None
        }
    }

    fn commands(&self) -> Vec<PluginCommand> {
        vec![PluginCommand {
            id: "third-party.review-diff".into(),
            name: "Third-party review".into(),
            description: String::new(),
            category: None,
        }]
    }
}

#[test]
fn third_party_command_suffixes_do_not_invoke_builtin_ai_actions() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.plugin_manager
        .register(Box::new(ThirdPartyReservedSuffixFixture))
        .unwrap();

    app.execute_plugin_command("third-party.review-diff");

    assert_eq!(
        app.toasts.iter().last().map(|toast| toast.message.as_str()),
        Some("third-party command routed")
    );
    assert!(!app.diff_review.is_active());
}

pub(super) fn kitty_capture_command(
    action: termeric_core::KittyFileTransferAction,
) -> termeric_core::KittyFileTransferCommand {
    termeric_core::KittyFileTransferCommand {
        action,
        compression: termeric_core::KittyFileTransferCompression::None,
        file_type: termeric_core::KittyFileTransferType::Regular,
        transmission_type: termeric_core::KittyFileTransmissionType::Simple,
        id: "capture-session".into(),
        file_id: String::new(),
        quiet: 0,
        mtime_ns: None,
        permissions: None,
        size: None,
        name: None,
        status: None,
        parent: None,
        data: Box::default(),
        bypass_present: false,
        reply_uses_8bit_controls: false,
    }
}

use crate::config::ShellConfig;
use crate::panes::{PaneManager, SplitDirection};
use crate::plugin::PluginAction;
use crate::tabs::{RoutedTerminalEvent, Tab, TerminalEventOrigin};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use termeric_core::event::ClipboardRequest;

use std::os::unix::fs::PermissionsExt;

/// The `action-result` a plugin receives for one action targeting `app`, its
/// origin window.
fn plugin_action_event(
    app: &mut App,
    plugin_id: &str,
    action: crate::plugin::structured::StructuredResponseAction,
) -> serde_json::Value {
    let outcome = app.run_plugin_window_action(&action);
    let event = super::plugin_actions::plugin_action_result_event(
        plugin_id,
        app.control_window_id,
        &action,
        outcome,
    )
    .unwrap();
    serde_json::from_str(&event).unwrap()
}

/// The `terminal-query-result` a plugin receives for one pane of `app`, its
/// origin window.
fn plugin_terminal_query_event(
    app: &App,
    query_id: &str,
    tab_id: u64,
    pane_id: u32,
    history_lines: u16,
    max_bytes: u32,
) -> serde_json::Value {
    let query = crate::plugin::structured::StructuredResponseAction::QueryTerminal {
        query_id: query_id.into(),
        window_id: None,
        tab_id,
        pane_id,
        history_lines,
        max_bytes,
    };
    let event = super::plugin_actions::plugin_terminal_query_event(
        "terminal-reader",
        app.control_window_id,
        &query,
        |bytes| app.read_control_text(tab_id, pane_id, history_lines, bytes),
    )
    .unwrap();
    serde_json::from_str(&event).unwrap()
}

/// Two shell tabs, the second active; the first is split into panes 0 and 1
/// with pane 1 focused. Returns the app and the two stable tab IDs.
fn background_split_tab_app() -> (App, usize, usize) {
    let shell = crate::test_support::sleeping_shell();
    let mut tabs = TabManager::for_test(shell.clone());
    let mut sleeping_tab = |title| {
        tabs.new_tab_with_command(
            "/bin/sh",
            &crate::test_support::sleeping_args(),
            None,
            Some(title),
            None,
            HashMap::new(),
        )
        .unwrap()
    };
    let first = sleeping_tab("first");
    let second = sleeping_tab("second");
    let first_tab = tabs.tab_mut_by_id(first).unwrap();
    let panes = first_tab.panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
    panes.split(SplitDirection::Vertical).unwrap();
    assert_eq!(panes.focused_id(), PaneId(1));
    assert_eq!(tabs.active_tab_id(), Some(second));

    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    (app, first, second)
}

fn split_pane_mut(app: &mut App, tab_id: usize, pane_id: PaneId) -> &mut crate::panes::Pane {
    app.tabs
        .as_mut()
        .unwrap()
        .tab_mut_by_id(tab_id)
        .unwrap()
        .panes
        .get_pane_mut(pane_id)
        .unwrap()
}

fn tab_order(app: &App) -> Vec<usize> {
    app.tabs
        .as_ref()
        .unwrap()
        .all_tabs()
        .iter()
        .map(|tab| tab.id)
        .collect()
}

fn harvest_rerun_capture_fixture() -> (
    App,
    tempfile::TempDir,
    crate::harvest::CommandEntry,
    Vec<(crate::tabs::TerminalEventOrigin, std::path::PathBuf)>,
) {
    harvest_rerun_capture_fixture_in(true)
}

pub(super) fn harvest_rerun_capture_fixture_in(
    split: bool,
) -> (
    App,
    tempfile::TempDir,
    crate::harvest::CommandEntry,
    Vec<(crate::tabs::TerminalEventOrigin, std::path::PathBuf)>,
) {
    let directory = tempfile::tempdir().unwrap();
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec![
            "-c".into(),
            "stty raw -echo; printf READY; cat > \"$CAPTURE_DIR/$$\"".into(),
        ],
        env: std::collections::BTreeMap::from([(
            "CAPTURE_DIR".into(),
            directory.path().display().to_string(),
        )]),
    };
    let mut tabs = TabManager::new(80, 24, shell.clone(), 100, 1024 * 1024);
    let tab_id = tabs
        .new_tab_with_command(
            "/bin/sh",
            &shell.args,
            None,
            Some("capture"),
            None,
            shell.env.clone().into_iter().collect(),
        )
        .unwrap();
    let panes = tabs
        .active_tab_mut()
        .unwrap()
        .panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
    let source = panes.focused_id();
    if split {
        panes.split(SplitDirection::Vertical).unwrap();
        panes.split(SplitDirection::Horizontal).unwrap();
    }
    let captures = panes
        .pane_ids()
        .into_iter()
        .map(|id| {
            let pid = panes.get_pane(id).unwrap().pty.child_id().unwrap();
            (
                crate::tabs::TerminalEventOrigin {
                    tab_id,
                    pane_id: id,
                },
                directory.path().join(pid.to_string()),
            )
        })
        .collect::<Vec<_>>();
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    let deadline = Instant::now() + std::time::Duration::from_secs(3);
    loop {
        app.process_pty_events();
        if captures.iter().all(|(origin, _)| {
            app.tabs
                .as_ref()
                .unwrap()
                .terminal_for_origin(*origin)
                .unwrap()
                .rows_text(0, 23)
                .contains("READY")
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "capture PTYs did not become ready"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let mut entry = crate::harvest::CommandEntry::new_for_source(
        crate::harvest::HarvestSource {
            tab_id,
            pane_id: source.0,
        },
        0,
        Some(0),
    );
    entry.command = "echo harvest".into();
    (app, directory, entry, captures)
}

pub(super) fn fenced_harvest_capture(
    app: &mut App,
    captures: &[(crate::tabs::TerminalEventOrigin, std::path::PathBuf)],
) -> Vec<Vec<u8>> {
    let previous_lengths = captures
        .iter()
        .map(|(_, path)| std::fs::metadata(path).map_or(0, |metadata| metadata.len() as usize))
        .collect::<Vec<_>>();
    for (origin, _) in captures {
        // A protocol-only fence drains prior input even when testing read-only refusal.
        app.tabs
            .as_mut()
            .unwrap()
            .write_to_origin(*origin, b"\0")
            .unwrap();
    }
    let deadline = Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let bytes = captures
            .iter()
            .map(|(_, path)| std::fs::read(path).unwrap_or_default())
            .collect::<Vec<_>>();
        if bytes
            .iter()
            .zip(&previous_lengths)
            .all(|(data, previous)| data.len() > *previous && data.last() == Some(&0))
        {
            return bytes
                .into_iter()
                .map(|mut data| {
                    data.pop();
                    data
                })
                .collect();
        }
        assert!(Instant::now() < deadline, "capture fences did not arrive");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

pub(super) fn split_harvest_capture_owner(app: &mut App, directory: &std::path::Path) -> PaneId {
    let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec![
            "-c".into(),
            "stty raw -echo; printf READY; cat > \"$CAPTURE_DIR/$$\"".into(),
        ],
        env: std::collections::BTreeMap::from([(
            "CAPTURE_DIR".into(),
            directory.display().to_string(),
        )]),
    };
    tab.panes_for_test(shell, 800.0, 480.0, 10.0, 20.0)
        .split(SplitDirection::Vertical)
        .unwrap()
}

pub(super) fn current_harvest_capture_routes(
    app: &mut App,
    directory: &std::path::Path,
) -> Vec<(crate::tabs::TerminalEventOrigin, std::path::PathBuf)> {
    use crate::tabs::TerminalEventOrigin;
    let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
    let captures = tab
        .panes
        .pane_ids()
        .into_iter()
        .map(|pane_id| {
            let pid = tab.panes.get_pane(pane_id).unwrap().pty.child_id().unwrap();
            (
                TerminalEventOrigin {
                    tab_id: tab.id,
                    pane_id,
                },
                directory.join(pid.to_string()),
            )
        })
        .collect::<Vec<_>>();
    let deadline = Instant::now() + std::time::Duration::from_secs(3);
    loop {
        app.process_pty_events();
        if captures.iter().all(|(_, path)| path.is_file()) {
            return captures;
        }
        assert!(
            Instant::now() < deadline,
            "capture readers did not become ready"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

mod plugin_terminal_tests;

mod plugin_restart_tests;

mod terminal_session_tests;

mod workspace_plugin_tests;

mod plugin_pane_creation_tests;

mod plugin_pane_transfer_tests;

/// A custom agent definition with `id`, for fixtures that mark raw local
/// capture shells as agents.
fn synthetic_agent(id: &str) -> crate::agents::AgentDefinition {
    crate::agents::AgentCatalog::from_config(&std::collections::BTreeMap::from([(
        id.to_string(),
        crate::agents::AgentConfig::default(),
    )]))
    .get(id)
    .unwrap()
    .clone()
}

/// Make `plugin_id` a configured agent, so that the check made before each
/// write to an agent recognizes the terminal Termeric launched it in.
fn register_capture_agent(app: &mut App, plugin_id: &str) {
    app.apply_config(app.config.clone().with_agent_features())
        .unwrap();
    app.agents = crate::agents::AgentCatalog::from_config(&std::collections::BTreeMap::from([(
        plugin_id.to_string(),
        crate::agents::AgentConfig {
            program: Some(plugin_id.into()),
            ..Default::default()
        },
    )]));
}

fn mark_capture_plugin_owner(app: &mut App, pane_id: PaneId, plugin_id: &str) {
    let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
    let launch = crate::workspace::WorkspaceLaunch::Plugin {
        plugin_id: plugin_id.into(),
        profile: None,
    };
    tab.panes.get_pane_mut(pane_id).unwrap().launch = launch;
    // A tab's only terminal also decides what kind of tab it is.
    if tab.panes.pane_count() == 1 {
        tab.reconcile_primary_pane_launch();
    }
    // What an agent scan records for a terminal Termeric launched.
    tab.pane_integration_mut(pane_id).unwrap().agent = Some(crate::agents::AgentSession {
        agent_id: plugin_id.into(),
        launched: true,
        profile: None,
        process: None,
    });
}

/// Two agent tabs: Codex working in the first, Claude Code waiting in the
/// second, which is left active.
pub(super) fn two_agent_tabs() -> (App, usize, usize) {
    use crate::agents::{AgentReport, AgentStatus};
    use crate::control::protocol::AgentEventKind;
    let mut tabs = TabManager::for_test(ShellConfig::default());
    let mut ids = Vec::new();
    for (agent, event, detail) in [
        ("codex", AgentEventKind::Working, None),
        (
            "claude-code",
            AgentEventKind::NeedsInput,
            Some("Approve shell: cargo test"),
        ),
    ] {
        let id = tabs
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "sleep 30".into()],
                None,
                None,
                None,
                HashMap::new(),
            )
            .unwrap();
        let tab = tabs.tab_mut_by_id(id).unwrap();
        let pane_id = tab.focused_terminal_identity().pane_id;
        let session = tab.pane_integration_mut(pane_id).unwrap();
        session.agent = Some(crate::agents::AgentSession {
            agent_id: agent.into(),
            launched: false,
            profile: None,
            process: None,
        });
        session.agent_status = AgentStatus::after(
            None,
            AgentReport {
                agent_id: agent.into(),
                event,
                detail: detail.map(Into::into),
                last_message: None,
                transcript_path: None,
            },
            Instant::now(),
        );
        ids.push(id);
    }
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    (app, ids[0], ids[1])
}

fn cursor_terminal(tab_id: usize, pane: u32) -> Option<crate::tabs::TerminalIdentity> {
    Some(crate::tabs::TerminalIdentity {
        tab_id,
        pane_id: PaneId(pane),
    })
}

fn wait_for_plugin_fixture(app: &mut App, plugin_id: &str, needle: &str) {
    let found = crate::test_support::poll_until(Duration::from_secs(5), || {
        if let Some(tabs) = &mut app.tabs {
            let (_, events) = tabs.poll_events();
            for event in events {
                app.handle_terminal_event(event);
            }
        }
        app.tabs
            .as_ref()
            .and_then(|tabs| tabs.find_plugin_tab_index(plugin_id))
            .and_then(|index| tabs_at(app, index))
            .and_then(|tab| tab.focused_terminal())
            .is_some_and(|terminal| terminal.grid().plain_lines().join("\n").contains(needle))
    });
    assert!(
        found,
        "timed out waiting for {plugin_id} fake CLI output containing {needle:?}"
    );
}

fn tabs_at(app: &App, index: usize) -> Option<&Tab> {
    app.tabs
        .as_ref()?
        .iter()
        .find_map(|(candidate, tab)| (candidate == index).then_some(tab))
}

fn wait_for_file_text(path: &std::path::Path, needle: &str) {
    let found = crate::test_support::poll_until(Duration::from_secs(5), || {
        std::fs::read_to_string(path)
            .ok()
            .is_some_and(|text| text.contains(needle))
    });
    assert!(
        found,
        "timed out waiting for {needle:?} in {}",
        path.display()
    );
}

fn latest_toast(app: &App) -> Option<&str> {
    app.toasts.iter().last().map(|toast| toast.message.as_str())
}

mod notification_owner_tests;

mod agent_features_tests;
mod agent_tests;
mod configuration_tests;
mod control_tests;
mod file_transfer_tests;
mod frame_scheduling_tests;
mod harvest_tests;
mod hyperlink_tests;
mod input_owner_tests;
mod notification_tests;
mod overlay_clipboard_tests;
mod pane_tests;
mod plugin_host_tests;
mod pointer_gesture_tests;
mod real_pty_tests;
mod search_tests;
mod selection_tests;
mod workspace_launch_tests;
mod workspace_persistence_tests;
