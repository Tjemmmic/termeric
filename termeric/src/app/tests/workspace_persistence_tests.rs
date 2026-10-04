//! Saving windows as workspaces, and closing and reopening tab snapshots.

use super::*;

/// A headless window of sleeping shells whose workspaces are kept under
/// `root`. Call [`end_persistence_test`] before `root` is dropped.
fn persistence_app(root: &std::path::Path) -> App {
    crate::workspace::set_test_workspaces_base_dir(Some(root.join("workspaces")));
    AppState::set_test_state_path(Some(root.join("state/state.json")));
    let config = Config {
        shell: crate::test_support::sleeping_shell(),
        ..Config::default()
    };
    let mut app = App::new_headless(config).unwrap();
    app.tabs = Some(TabManager::for_test(crate::test_support::sleeping_shell()));
    app
}

fn end_persistence_test() {
    crate::workspace::set_test_workspaces_base_dir(None);
    AppState::set_test_state_path(None);
}

#[test]
fn save_workspace_names_a_long_titled_tab_within_the_name_limit() {
    let root = tempfile::tempdir().unwrap();
    let mut app = persistence_app(root.path());
    let tabs = app.tabs.as_mut().unwrap();
    tabs.new_tab().unwrap();
    tabs.active_tab_mut().unwrap().title = "x".repeat(300);

    app.force_save_workspace();
    let saved = crate::workspace::list_workspaces();
    end_persistence_test();

    let [saved] = saved.as_slice() else {
        panic!("Save Workspace did not save: {saved:?}");
    };
    assert_eq!(
        saved.name.chars().count(),
        crate::workspace::MAX_WORKSPACE_NAME_CHARS
    );
    assert!(saved.name.ends_with("x… workspace"), "{}", saved.name);
}

/// Give the active tab a command so it is worth keeping, close it, and
/// return the ID of the closed-tab snapshot saved for it.
fn close_active_tab(app: &mut App) -> String {
    let tabs = app.tabs.as_mut().unwrap();
    tabs.active_tab_mut()
        .unwrap()
        .focused_pane_mut()
        .unwrap()
        .last_command = Some("make".into());
    assert!(tabs.close_tab(tabs.active_index()));
    tabs.set_reduced_motion(true);
    assert!(tabs.cleanup_closed(true, true));
    match tabs.drain_persistence_notices().as_slice() {
        [crate::tabs::PersistenceNotice::Saved { id, .. }] => id.clone(),
        notices => panic!("the closed tab was not saved: {notices:?}"),
    }
}

fn toast_messages(app: &App) -> Vec<String> {
    app.toasts
        .iter()
        .map(|toast| toast.message.clone())
        .collect()
}

#[test]
fn a_reopened_closed_tab_is_active_and_closing_it_again_updates_its_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let mut app = persistence_app(root.path());
    let tabs = app.tabs.as_mut().unwrap();
    tabs.new_tab().unwrap();
    tabs.active_tab_mut().unwrap().title = "kept".into();
    tabs.new_tab().unwrap();
    tabs.active_tab_mut().unwrap().title = "closed".into();
    let id = close_active_tab(&mut app);
    assert_eq!(app.tabs.as_ref().unwrap().count(), 1);
    // Date the snapshot back, so a re-save that reset it would show.
    let mut saved = crate::workspace::load_workspace(&id).unwrap().workspace;
    saved.created_at = "2020-01-01T00:00:00Z".into();
    crate::workspace::save_workspace(&mut saved).unwrap();

    app.restore_closed_tab(&id);
    let tabs = app.tabs.as_ref().unwrap();
    assert_eq!(tabs.count(), 2);
    let reopened = tabs.active_tab().unwrap();
    assert_eq!(tabs.active_index(), 1);
    assert_eq!(reopened.title, "closed");
    assert_eq!(
        reopened
            .saved_snapshot
            .as_ref()
            .map(|saved| saved.id.as_str()),
        Some(id.as_str())
    );

    let resaved = close_active_tab(&mut app);
    let snapshots = crate::workspace::list_closed_tabs();
    let resaved_archive = crate::workspace::load_workspace(&id).unwrap().workspace;
    end_persistence_test();
    assert_eq!(resaved, id);
    assert_eq!(resaved_archive.created_at, "2020-01-01T00:00:00Z");
    assert_eq!(snapshots.len(), 1, "{snapshots:?}");
    assert_eq!(snapshots[0].id, id);
}

#[test]
fn saving_and_renaming_a_workspace_keep_when_it_was_created() {
    let root = tempfile::tempdir().unwrap();
    let mut app = persistence_app(root.path());
    app.automatic_persistence = true;
    app.tabs.as_mut().unwrap().new_tab().unwrap();
    let id = app.save_workspace_as("Project".into()).unwrap().id;
    let mut saved = crate::workspace::load_workspace(&id).unwrap().workspace;
    saved.created_at = "2020-01-01T00:00:00Z".into();
    crate::workspace::save_workspace(&mut saved).unwrap();

    // Quitting saves the window back into the workspace it last saved.
    app.tabs.as_mut().unwrap().new_tab().unwrap();
    app.auto_save_workspace();
    let quit = crate::workspace::load_workspace(&id).unwrap().workspace;
    crate::workspace::rename_workspace(&id, "Renamed").unwrap();
    let renamed = crate::workspace::load_workspace(&id).unwrap().workspace;
    end_persistence_test();

    assert_eq!(quit.tabs.len(), 2, "the window was not saved on quit");
    assert_eq!(quit.created_at, "2020-01-01T00:00:00Z");
    assert_eq!(renamed.name, "Renamed");
    assert_eq!(renamed.created_at, "2020-01-01T00:00:00Z");
}

#[test]
fn a_reopened_pinned_tab_joins_the_pinned_group_and_the_window_still_saves() {
    let root = tempfile::tempdir().unwrap();
    let mut app = persistence_app(root.path());
    let tabs = app.tabs.as_mut().unwrap();
    tabs.new_tab().unwrap();
    tabs.new_tab().unwrap();
    tabs.active_tab_mut().unwrap().title = "pinned".into();
    assert!(tabs.toggle_pin_active());
    let id = close_active_tab(&mut app);
    let tabs = app.tabs.as_mut().unwrap();
    tabs.new_tab().unwrap();
    assert_eq!(tabs.count(), 2);

    app.restore_closed_tab(&id);
    let tabs = app.tabs.as_ref().unwrap();
    let pins: Vec<_> = tabs.iter().map(|(_, tab)| tab.pinned).collect();
    assert_eq!(pins, [true, false, false]);
    assert_eq!(tabs.active_index(), 0);
    assert_eq!(tabs.active_tab().unwrap().title, "pinned");

    let saved = app.save_workspace_as("After reopening".into());
    end_persistence_test();
    saved.unwrap();
}

#[test]
fn only_a_one_tab_snapshot_reopens_as_a_tab() {
    let root = tempfile::tempdir().unwrap();
    let mut app = persistence_app(root.path());
    let tabs = app.tabs.as_mut().unwrap();
    tabs.new_tab().unwrap();
    tabs.new_tab().unwrap();
    let mut two_tabs = tabs
        .capture_workspace("Two tabs".into(), false, Default::default())
        .unwrap();
    two_tabs.closed_tab = true;
    crate::workspace::save_workspace(&mut two_tabs).unwrap();

    app.restore_closed_tab(&two_tabs.id);
    end_persistence_test();
    assert_eq!(app.tabs.as_ref().unwrap().count(), 2);
    let messages = toast_messages(&app);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("does not hold exactly one tab")),
        "{messages:?}"
    );
}

#[test]
fn reopening_reports_both_archive_and_relaunch_warnings() {
    let root = tempfile::tempdir().unwrap();
    let mut app = persistence_app(root.path());
    let tabs = app.tabs.as_mut().unwrap();
    tabs.new_tab().unwrap();
    tabs.new_tab().unwrap();
    tabs.active_tab_mut()
        .unwrap()
        .focused_terminal_mut()
        .unwrap()
        .process(b"saved history\r\n");
    let id = close_active_tab(&mut app);

    // The pane's directory has gone, and so has its saved scrollback.
    let mut snapshot = crate::workspace::load_workspace(&id).unwrap().workspace;
    snapshot.tabs[0].panes[0].working_directory = Some("/definitely/missing/termeric-cwd".into());
    crate::workspace::save_workspace(&mut snapshot).unwrap();
    let scrollback = snapshot.tabs[0].panes[0].scrollback_file.clone().unwrap();
    std::fs::remove_file(
        crate::workspace::workspace_dir(&id)
            .unwrap()
            .join(scrollback),
    )
    .unwrap();

    app.restore_closed_tab(&id);
    end_persistence_test();
    assert_eq!(app.tabs.as_ref().unwrap().count(), 2);
    let messages = toast_messages(&app);
    for expected in [
        "scrollback was skipped",
        "is unavailable; used the process default",
    ] {
        assert!(
            messages.iter().any(|message| message.contains(expected)),
            "no {expected:?} in {messages:?}"
        );
    }
}
