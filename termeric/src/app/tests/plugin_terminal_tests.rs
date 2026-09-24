//! Synthetic plugin launch metadata on local raw capture shells only.
use super::*;

#[test]
fn unavailable_plugin_restart_preserves_the_original_mixed_tab() {
    let (mut app, _directory, _, captures) = harvest_rerun_capture_fixture();
    mark_capture_plugin_owner(&mut app, PaneId(0), "synthetic-capture");
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .tab_type = crate::tabs::TabType::Plugin {
        plugin_id: "synthetic-capture".into(),
    };
    app.restart_plugin("synthetic-capture");
    let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
    assert!(
        !tab.is_closing(),
        "an unavailable replacement must not close the old tab"
    );
    assert_eq!(tab.pane_manager.as_ref().unwrap().pane_count(), 3);
    assert!(
        fenced_harvest_capture(&mut app, &captures)
            .iter()
            .all(Vec::is_empty)
    );
}
