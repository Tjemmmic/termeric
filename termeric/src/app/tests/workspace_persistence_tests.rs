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

    app.force_save_workspace(crate::app::workspaces::WorkspaceSave::Update);
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
    let id = app
        .save_workspace(crate::app::workspaces::WorkspaceSave::New)
        .unwrap()
        .id;
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

    let saved = app.save_workspace(crate::app::workspaces::WorkspaceSave::New);
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

use crate::app::workspaces::WorkspaceSave;

fn workspace_ids() -> Vec<String> {
    let mut ids: Vec<_> = crate::workspace::list_workspaces()
        .into_iter()
        .map(|workspace| workspace.id)
        .collect();
    ids.sort();
    ids
}

#[test]
fn save_workspace_updates_the_open_workspace_and_save_as_starts_another() {
    let root = tempfile::tempdir().unwrap();
    let mut app = persistence_app(root.path());
    app.tabs.as_mut().unwrap().new_tab().unwrap();

    let first = app.save_workspace(WorkspaceSave::Update).unwrap().id;
    app.tabs.as_mut().unwrap().new_tab().unwrap();
    let again = app.save_workspace(WorkspaceSave::Update).unwrap().id;
    let after_two_saves = workspace_ids();
    let updated = crate::workspace::load_workspace(&first).unwrap().workspace;

    let second = app.save_workspace(WorkspaceSave::New).unwrap().id;
    let after_save_as = workspace_ids();
    let current_after_save_as = app.current_workspace_id.clone();

    // A workspace opened in the window is the one Save Workspace updates.
    assert!(app.restore_saved_workspace(&first));
    app.tabs.as_mut().unwrap().new_tab().unwrap();
    let reopened = app.save_workspace(WorkspaceSave::Update).unwrap().id;
    let after_reopening = workspace_ids();
    let reopened_tabs = crate::workspace::load_workspace(&first)
        .unwrap()
        .workspace
        .tabs
        .len();
    end_persistence_test();

    assert_eq!(again, first);
    assert_eq!(after_two_saves, std::slice::from_ref(&first));
    assert_eq!(updated.tabs.len(), 2, "the second save updated the archive");
    assert!(!updated.automatic);
    assert_ne!(second, first);
    assert_eq!(after_save_as.len(), 2);
    assert_eq!(current_after_save_as.as_deref(), Some(second.as_str()));
    assert_eq!(reopened, first);
    assert_eq!(after_reopening.len(), 2);
    assert_eq!(reopened_tabs, 3);
}

#[test]
fn saving_an_automatic_workspace_makes_it_the_persons() {
    let root = tempfile::tempdir().unwrap();
    let mut app = persistence_app(root.path());
    app.automatic_persistence = true;
    let tabs = app.tabs.as_mut().unwrap();
    tabs.new_tab().unwrap();
    tabs.active_tab_mut().unwrap().title = "notes".into();

    app.auto_save_workspace();
    let automatic = crate::workspace::list_workspaces();
    let saved = app.save_workspace(WorkspaceSave::Update).unwrap().id;
    let promoted = crate::workspace::load_workspace(&saved).unwrap().workspace;
    end_persistence_test();

    let [automatic] = automatic.as_slice() else {
        panic!("closing did not save one automatic workspace: {automatic:?}");
    };
    assert_eq!(automatic.name, "Last workspace");
    assert_eq!(saved, automatic.id, "the automatic workspace was updated");
    assert!(!promoted.automatic, "it is never pruned now");
    assert_eq!(promoted.name, "notes workspace");
}

/// Two saved workspaces, A of two tabs and B of one, with the window
/// showing B and belonging to no workspace.
fn two_workspaces(app: &mut App) -> (String, String) {
    let tabs = app.tabs.as_mut().unwrap();
    tabs.new_tab().unwrap();
    tabs.new_tab().unwrap();
    let a = app.save_workspace(WorkspaceSave::New).unwrap().id;
    let tabs = app.tabs.as_mut().unwrap();
    assert!(tabs.close_tab(tabs.active_index()));
    tabs.set_reduced_motion(true);
    tabs.cleanup_closed(false, false);
    let b = app.save_workspace(WorkspaceSave::New).unwrap().id;
    app.current_workspace_id = None;
    (a, b)
}

fn tab_count(app: &App) -> usize {
    app.tabs.as_ref().unwrap().count()
}

#[test]
fn a_window_of_idle_shells_opens_a_workspace_at_once() {
    let root = tempfile::tempdir().unwrap();
    let mut app = persistence_app(root.path());
    let (a, _) = two_workspaces(&mut app);

    app.request_open_workspace(&a);
    end_persistence_test();
    assert!(!app.dialog.is_active(), "nothing to ask");
    assert_eq!(tab_count(&app), 2);
    assert_eq!(app.current_workspace_id.as_deref(), Some(a.as_str()));
}

#[test]
fn opening_over_running_programs_asks_first() {
    let root = tempfile::tempdir().unwrap();
    let mut app = persistence_app(root.path());
    let (a, _) = two_workspaces(&mut app);
    // A profile the tab launched is a program besides the shell.
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .focused_pane_mut()
        .unwrap()
        .launch = crate::workspace::WorkspaceLaunch::Profile { name: "ssh".into() };

    app.request_open_workspace(&a);
    let asked = app.dialog.is_active();
    let request_id = app.dialog.request_id();
    let title = app.dialog.title().to_owned();
    let open_role = app
        .dialog
        .buttons()
        .iter()
        .find(|button| {
            button.action == crate::ui::dialog::ButtonAction::Answer("workspace-open".into())
        })
        .map(|button| button.role);
    let kept_while_asking = tab_count(&app);
    app.answer_dialog(crate::ui::dialog::CANCEL);
    let kept_after_cancel = tab_count(&app);

    app.request_open_workspace(&a);
    assert_eq!(app.dialog.request_id(), request_id + 1);
    app.answer_dialog("workspace-open");
    end_persistence_test();

    assert!(asked);
    // The question names the workspace, and opening it is the button that
    // ends the running program.
    assert!(title.starts_with("Open \u{201c}"), "{title}");
    assert_eq!(open_role, Some(crate::ui::dialog::ButtonRole::Destructive));
    assert_eq!(kept_while_asking, 1);
    assert_eq!(kept_after_cancel, 1);
    assert!(app.dialog_flow.is_none());
    assert_eq!(tab_count(&app), 2, "agreeing opened the workspace");
    assert_eq!(app.current_workspace_id.as_deref(), Some(a.as_str()));
}

#[test]
fn opening_from_a_named_auto_saved_workspace_saves_it_back_first() {
    let root = tempfile::tempdir().unwrap();
    let mut app = persistence_app(root.path());
    let (a, b) = two_workspaces(&mut app);
    app.current_workspace_id = Some(b.clone());
    app.automatic_persistence = true;
    // Busy, yet nothing is lost: the window goes back into B.
    let tabs = app.tabs.as_mut().unwrap();
    tabs.new_tab().unwrap();
    tabs.active_tab_mut()
        .unwrap()
        .focused_pane_mut()
        .unwrap()
        .launch = crate::workspace::WorkspaceLaunch::Profile { name: "ssh".into() };

    app.request_open_workspace(&a);
    let saved_back = crate::workspace::load_workspace(&b).unwrap().workspace;
    end_persistence_test();

    assert!(!app.dialog.is_active());
    assert_eq!(saved_back.tabs.len(), 2, "B holds the window as it was");
    assert_eq!(tab_count(&app), 2);
    assert_eq!(app.current_workspace_id.as_deref(), Some(a.as_str()));
}

#[test]
fn opening_from_an_automatic_workspace_asks_like_an_unsaved_window() {
    let root = tempfile::tempdir().unwrap();
    let mut app = persistence_app(root.path());
    let (a, b) = two_workspaces(&mut app);
    // B is a "Last workspace" the window was restored from.
    let mut automatic = crate::workspace::load_workspace(&b).unwrap().workspace;
    automatic.automatic = true;
    crate::workspace::save_workspace(&mut automatic).unwrap();
    app.current_workspace_id = Some(b.clone());
    app.automatic_persistence = true;
    let tabs = app.tabs.as_mut().unwrap();
    tabs.new_tab().unwrap();
    tabs.active_tab_mut()
        .unwrap()
        .focused_pane_mut()
        .unwrap()
        .launch = crate::workspace::WorkspaceLaunch::Profile { name: "ssh".into() };

    app.request_open_workspace(&a);
    let asked = app.dialog.is_active();
    let untouched = crate::workspace::load_workspace(&b).unwrap().workspace;
    let request_id = app.dialog.request_id();
    app.apply_dialog_action(crate::ui::dialog::DialogAction::Answer {
        request_id,
        id: crate::ui::dialog::CANCEL.into(),
    });
    let kept_after_cancel = tab_count(&app);
    end_persistence_test();

    assert!(asked, "an automatic workspace keeps nothing, so it asks");
    assert_eq!(untouched.tabs.len(), 1, "B was not saved over");
    assert!(untouched.automatic);
    assert_eq!(kept_after_cancel, 2);
    assert_eq!(app.current_workspace_id.as_deref(), Some(b.as_str()));
}

#[test]
fn deleting_a_workspace_asks_first() {
    let root = tempfile::tempdir().unwrap();
    let mut app = persistence_app(root.path());
    let (a, b) = two_workspaces(&mut app);
    app.current_workspace_id = Some(a.clone());

    app.confirm_workspace_delete(&a, false);
    let asked = app.dialog.is_active();
    let request_id = app.dialog.request_id();
    app.apply_dialog_action(crate::ui::dialog::DialogAction::Answer {
        request_id,
        id: crate::ui::dialog::CANCEL.into(),
    });
    let after_cancel = workspace_ids();

    app.confirm_workspace_delete(&a, false);
    let request_id = app.dialog.request_id();
    app.apply_dialog_action(crate::ui::dialog::DialogAction::Answer {
        request_id,
        id: "workspace-delete".into(),
    });
    let after_delete = workspace_ids();
    end_persistence_test();

    assert!(asked);
    assert_eq!(after_cancel.len(), 2, "cancelling kept it");
    assert_eq!(after_delete, [b]);
    assert!(app.current_workspace_id.is_none());
}

#[test]
fn only_a_program_besides_an_idle_shell_counts_as_running() {
    let mut tabs = TabManager::for_test(ShellConfig::default());
    // An interactive shell gives what it starts a foreground job of its own.
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-i".into()],
        None,
        None,
        None,
        HashMap::from([("ENV".to_owned(), "/dev/null".to_owned())]),
    )
    .unwrap();
    let tab = tabs.active_tab_mut().unwrap();
    assert!(
        crate::test_support::poll_until(Duration::from_secs(5), || {
            let pty = tab.panes.lone_pane().map(|pane| &pane.pty).unwrap();
            pty.foreground_process_group().is_some()
        }),
        "the shell never took its terminal"
    );
    assert!(!tab.has_running_program(), "an idle shell runs nothing");
    tab.write(b"sleep 30\r").unwrap();
    assert!(
        crate::test_support::poll_until(Duration::from_secs(5), || tab.has_running_program()),
        "a program started at the prompt runs"
    );
}
