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
