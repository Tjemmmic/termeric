use super::*;
use crate::plugin::PluginAction;

#[test]
fn headless_config_application_updates_plugins_harvest_and_palette() {
    let mut config = Config::default();
    config.harvest.enabled = true;
    config.harvest.max_history = 7;
    config.plugins.claude_code.enabled = false;
    config.plugins.codex.enabled = true;

    let mut app = App::new_headless(config).unwrap();
    assert_eq!(app.plugin_manager.count(), 1);
    assert!(app.harvest.is_some());
    app.command_palette.open();
    let enabled_count = app.command_palette.build_info().total_count;

    let mut disabled = app.config.clone();
    disabled.harvest.enabled = false;
    disabled.plugins.codex.enabled = false;
    app.apply_config(disabled).unwrap();
    assert_eq!(app.plugin_manager.count(), 0);
    assert!(app.harvest.is_none());
    app.command_palette.open();
    let disabled_count = app.command_palette.build_info().total_count;
    assert_eq!(enabled_count - disabled_count, 16);
}

#[test]
fn headless_plugin_dispatch_surfaces_missing_target() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.execute_plugin_actions(vec![PluginAction::InjectContext {
        plugin_id: "codex".into(),
        text: "context".into(),
    }]);
    let messages: Vec<_> = app
        .toasts
        .iter()
        .map(|toast| toast.message.as_str())
        .collect();
    assert_eq!(messages, vec!["No writable codex tab found"]);
}

#[test]
fn headless_overlay_close_states_do_not_block_future_input() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.command_palette.open();
    assert!(app.command_palette.is_active());
    app.command_palette.close();
    assert!(!app.command_palette.is_active());

    app.settings_panel.toggle();
    assert!(app.settings_panel.is_active());
    app.settings_panel.close();
    assert!(!app.settings_panel.is_active());
    app.settings_panel.toggle();
    assert!(app.settings_panel.is_active());

    app.commands_panel.toggle();
    assert!(app.commands_panel.is_open());
    app.commands_panel.close();
    assert!(!app.commands_panel.is_open());
}
