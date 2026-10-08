//! Applying configuration and settings without a window.

use super::*;

#[test]
fn headless_config_application_updates_agents_harvest_and_palette() {
    let mut config = Config::default().with_agent_features();
    config.harvest.enabled = true;
    config.harvest.max_history = 7;
    config.agents.insert(
        "claude-code".into(),
        crate::agents::AgentConfig {
            enabled: Some(false),
            ..Default::default()
        },
    );

    let mut app = App::new_headless(config).unwrap();
    // One launch integration per enabled built-in agent.
    assert_eq!(app.plugin_manager.count(), 3);
    assert!(app.harvest.is_some());
    let per_agent = |app: &App| {
        app.command_palette
            .command_ids()
            .filter(|id| id.starts_with("codex.") || id.contains(".codex."))
            .count()
    };
    assert_eq!(per_agent(&app), 0, "agents add no per-agent commands");

    let mut disabled = app.config.clone();
    disabled.harvest.enabled = false;
    disabled.agents.insert(
        "codex".into(),
        crate::agents::AgentConfig {
            enabled: Some(false),
            ..Default::default()
        },
    );
    app.apply_config(disabled).unwrap();
    assert_eq!(app.plugin_manager.count(), 2);
    assert!(app.harvest.is_none());
    assert!(app.agents.get("codex").is_some_and(|agent| !agent.enabled));
}

#[test]
fn headless_settings_apply_scrollback_memory_budget() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.apply_settings_action(SettingsAction::SetScrollbackMemory(64));
    assert_eq!(app.config.scrollback.max_memory_mib, 64);
    assert_eq!(app.config.scrollback.max_memory_bytes(), 64 * 1024 * 1024);
}

#[test]
fn headless_font_size_setting_updates_the_drawn_size_and_preserves_other_config() {
    let mut app = App::new_headless(Config::default()).unwrap();
    for size in [8.0, 15.5, 72.0, 14.0] {
        let mut expected = app.config.clone();
        expected.font.size = size;
        app.apply_settings_action(SettingsAction::SetFontSize(size));
        assert_eq!(app.config.font.size, size);
        assert_eq!(app.effective_font_size(), size);
        assert_eq!(
            toml::to_string(&app.config).unwrap(),
            toml::to_string(&expected).unwrap(),
        );
    }
}

#[test]
fn headless_font_size_setting_rejects_invalid_values_before_mutation() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.apply_settings_action(SettingsAction::SetFontSize(15.5));
    let retained = toml::to_string(&app.config).unwrap();
    let geometry = (app.cols, app.rows);
    for size in [7.9, 72.1, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        app.apply_settings_action(SettingsAction::SetFontSize(size));
        assert_eq!(toml::to_string(&app.config).unwrap(), retained);
        assert_eq!(app.effective_font_size(), 15.5);
        assert_eq!((app.cols, app.rows), geometry);
    }
}

#[test]
fn headless_font_settings_preserve_order_and_apply_without_a_window() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.apply_settings_action(SettingsAction::SetFontFamily("Fira Mono".into()));
    app.apply_settings_action(SettingsAction::SetFontFallback {
        index: 0,
        family: Some("Noto Sans Mono".into()),
    });
    app.apply_settings_action(SettingsAction::SetFontFallback {
        index: 1,
        family: Some("Noto Color Emoji".into()),
    });
    app.apply_settings_action(SettingsAction::SetFontLigatures(false));
    app.apply_settings_action(SettingsAction::SetBoldIsBright(true));
    app.apply_settings_action(SettingsAction::SetFontWeight(350));
    app.apply_settings_action(SettingsAction::SetBoldFontWeight(825));
    app.apply_settings_action(SettingsAction::SetLineHeight(1.35));

    assert_eq!(app.config.font.family, "Fira Mono");
    assert_eq!(
        app.config.font.fallback,
        ["Noto Sans Mono", "Noto Color Emoji"]
    );
    assert!(!app.config.font.ligatures);
    assert!(app.config.font.bold_is_bright);
    assert_eq!(app.config.font.weight, 350);
    assert_eq!(app.config.font.bold_weight, 825);
    assert_eq!(app.config.font.line_height, 1.35);

    app.apply_settings_action(SettingsAction::SetFontWeight(900));
    app.apply_settings_action(SettingsAction::SetBoldFontWeight(200));
    assert_eq!(
        (app.config.font.weight, app.config.font.bold_weight),
        (350, 825),
        "invalid settings pairs must leave both live values unchanged"
    );

    // The primary family can't also be a fallback.
    app.apply_settings_action(SettingsAction::SetFontFallback {
        index: 1,
        family: Some("Fira Mono".into()),
    });
    assert_eq!(
        app.config.font.fallback,
        ["Noto Sans Mono", "Noto Color Emoji"]
    );

    app.apply_settings_action(SettingsAction::SetFontFallback {
        index: 0,
        family: None,
    });
    assert_eq!(app.config.font.fallback, ["Noto Color Emoji"]);

    // Promoting an existing fallback to primary removes the duplicate.
    app.apply_settings_action(SettingsAction::SetFontFamily("Noto Color Emoji".into()));
    assert!(app.config.font.fallback.is_empty());
}

#[test]
fn a_window_closed_beside_others_leaves_no_automatic_archive() {
    let temp = tempfile::tempdir().unwrap();
    crate::workspace::set_test_workspaces_base_dir(Some(temp.path().join("workspaces")));
    AppState::set_test_state_path(Some(temp.path().join("state/state.json")));
    {
        let mut config = Config::default();
        config.shell.program = Some("/bin/sh".into());
        config.shell.args = vec!["-c".into(), "sleep 30".into()];
        let mut app = App::new_headless(config).unwrap();
        app.automatic_persistence = true;
        app.allow_auto_restore = false;
        app.init_tabs().unwrap();

        app.retain_workspace_on_exit = false;
        app.auto_save_workspace();
        assert!(crate::workspace::list_workspaces().is_empty());

        // The last window's archive is kept, marked as automatic.
        app.retain_workspace_on_exit = true;
        app.auto_save_workspace();
        let listed = crate::workspace::list_workspaces();
        assert_eq!(listed.len(), 1);
        assert!(
            crate::workspace::load_workspace(&listed[0].id)
                .unwrap()
                .workspace
                .automatic
        );
    }
    crate::workspace::set_test_workspaces_base_dir(None);
    AppState::set_test_state_path(None);
}

#[test]
fn only_agent_settings_reregister_plugins() {
    let mut app = App::new_headless(Config::default().with_agent_features()).unwrap();
    assert!(app.plugin_manager.contains("codex"));
    let generation = app.plugin_runtime_generation;

    app.apply_settings_action(SettingsAction::SetCursorBlink(false));
    app.apply_settings_action(SettingsAction::SetPadding(6));
    assert!(!app.config.cursor.blink);
    assert_eq!(app.config.window.padding, 6);
    assert_eq!(app.plugin_runtime_generation, generation);

    app.apply_settings_action(SettingsAction::SetAgentEnabled {
        agent_id: "codex".into(),
        enabled: false,
    });
    assert_ne!(app.plugin_runtime_generation, generation);
    assert!(!app.plugin_manager.contains("codex"));
}

#[test]
fn headless_config_reload_preserves_valid_font_range_overrides_atomically() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let mut config = app.config.clone();
    config.font.range_overrides = vec![crate::config::FontRangeOverride {
        start: 0xE000,
        end: 0xF8FF,
        family: "Symbols Nerd Font Mono".into(),
    }];
    app.apply_config(config).unwrap();
    assert_eq!(app.config.font.range_overrides[0].start, 0xE000);

    let retained = app.config.font.range_overrides.clone();
    let mut invalid = app.config.clone();
    invalid
        .font
        .range_overrides
        .push(crate::config::FontRangeOverride {
            start: 0xF000,
            end: 0xFFFF,
            family: "Overlapping Symbols".into(),
        });
    assert!(app.apply_config(invalid).is_err());
    assert_eq!(app.config.font.range_overrides, retained);
}

#[test]
fn headless_config_reload_preserves_valid_font_features_atomically() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let mut config = app.config.clone();
    config.font.features.insert("ss01".into(), 1);
    config.font.features.insert("cv01".into(), 2);
    app.apply_config(config).unwrap();
    assert_eq!(app.config.font.features.get("ss01"), Some(&1));
    assert_eq!(app.config.font.features.get("cv01"), Some(&2));

    let retained = app.config.font.features.clone();
    let mut invalid = app.config.clone();
    invalid.font.features.insert("kern".into(), 1);
    assert!(app.apply_config(invalid).is_err());
    assert_eq!(app.config.font.features, retained);
}

#[test]
fn headless_window_level_setting_updates_persistent_policy() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.apply_settings_action(SettingsAction::SetWindowLevel(
        crate::config::WindowLevelConfig::AlwaysOnBottom,
    ));
    assert_eq!(
        app.config.window.level,
        crate::config::WindowLevelConfig::AlwaysOnBottom
    );
}

#[test]
fn headless_tab_bar_position_setting_updates_persistent_policy() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.apply_settings_action(SettingsAction::SetTabBarPosition(
        crate::config::TabBarPosition::Left,
    ));
    assert_eq!(
        app.config.window.tab_bar_position,
        crate::config::TabBarPosition::Left
    );
}

#[test]
fn headless_background_image_settings_update_persistent_layout_policy() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.apply_settings_action(SettingsAction::SetBackgroundImageFit(
        crate::config::BackgroundImageFit::Contain,
    ));
    app.apply_settings_action(SettingsAction::SetBackgroundImageOpacity(0.45));
    assert_eq!(
        app.config.window.background_image.fit,
        crate::config::BackgroundImageFit::Contain
    );
    assert!((app.config.window.background_image.opacity - 0.45).abs() < f32::EPSILON);
}

#[test]
fn headless_accessibility_settings_apply_without_transition_frames() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.apply_settings_action(SettingsAction::SetReduceMotion(
        crate::config::AccessibilityPreference::On,
    ));
    app.apply_settings_action(SettingsAction::SetHighContrast(
        crate::config::AccessibilityPreference::On,
    ));
    assert_eq!(
        app.config.accessibility.reduce_motion,
        crate::config::AccessibilityPreference::On
    );
    assert_eq!(
        app.config.accessibility.high_contrast,
        crate::config::AccessibilityPreference::On
    );

    app.settings_panel.toggle();
    assert_eq!(app.settings_panel.animation_progress(), 1.0);
    app.settings_panel.close();
    assert!(!app.settings_panel.needs_redraw());
    assert!(!app.settings_panel.is_visible());
}

#[test]
fn headless_system_accessibility_preferences_respect_explicit_overrides() {
    let mut config = Config::default();
    config.accessibility.high_contrast = crate::config::AccessibilityPreference::Off;
    let mut app = App::new_headless(config).unwrap();

    app.set_system_accessibility_preferences(
        super::system_accessibility::SystemAccessibilityPreferences {
            reduce_motion: Some(true),
            high_contrast: Some(true),
        },
    );
    assert!(app.reduced_motion_enabled());
    assert!(!app.high_contrast_enabled());
    app.settings_panel.toggle();
    assert_eq!(app.settings_panel.animation_progress(), 1.0);
    assert!(app.agent_view.state.reduced_motion());

    app.apply_settings_action(SettingsAction::SetReduceMotion(
        crate::config::AccessibilityPreference::Off,
    ));
    assert!(!app.reduced_motion_enabled());
    assert!(!app.agent_view.state.reduced_motion());
}

#[test]
fn background_image_completions_are_generation_checked_and_fail_visibly() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.background_image_generation = 7;
    app.background_image_loading = true;
    app.handle_background_image_loaded(6, PathBuf::from("stale.png"), Err("stale decode".into()));
    assert!(!app.background_image_loading);
    assert!(
        app.toasts
            .iter()
            .all(|toast| !toast.message.contains("stale"))
    );

    app.background_image_loading = true;
    app.handle_background_image_loaded(
        7,
        PathBuf::from("current.png"),
        Err("current decode".into()),
    );
    assert!(!app.background_image_loading);
    assert!(
        latest_toast(&app).is_some_and(|message| message.contains("current decode")),
        "the current failure must be visible while preserving the prior texture"
    );
}

#[test]
fn background_image_completion_does_not_mark_a_newer_file_as_loaded() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("background.png");
    image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]))
        .save(&path)
        .unwrap();
    let old_time = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(10);
    std::fs::File::open(&path)
        .unwrap()
        .set_modified(old_time)
        .unwrap();
    let decoded = crate::renderer::load_background_image(&path).unwrap();
    let old_stamp = decoded.source_stamp;
    let mut config = Config::default();
    config.window.background_image.path = Some(path.clone());
    let mut app = App::new_headless(config).unwrap();
    app.background_image_loading = true;

    let replacement = directory.path().join("replacement.png");
    image::RgbaImage::from_pixel(2, 2, image::Rgba([40, 50, 60, 255]))
        .save(&replacement)
        .unwrap();
    let new_time = old_time + Duration::from_secs(10);
    std::fs::File::open(&replacement)
        .unwrap()
        .set_modified(new_time)
        .unwrap();
    std::fs::rename(replacement, &path).unwrap();
    app.handle_background_image_loaded(0, path.clone(), Ok(decoded));
    assert_eq!(app.last_background_image_stamp, Some(old_stamp));
    assert!(app.poll_background_image());
    assert_eq!(app.background_image_generation, 1);
    assert!(
        !app.poll_background_image(),
        "one unchanged replacement needs only one reload"
    );
    let replacement = crate::renderer::load_background_image(&path).unwrap();
    assert_eq!(&replacement.rgba[..4], &[40, 50, 60, 255]);
    app.handle_background_image_loaded(1, path, Ok(replacement));
    assert!(!app.poll_background_image());
}

#[test]
fn background_image_same_time_replacements_coalesce_and_disabled_results_stay_stale() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("background.png");
    image::RgbaImage::new(2, 2).save(&path).unwrap();
    let old = crate::renderer::load_background_image(&path).unwrap();
    let mut config = Config::default();
    config.window.background_image.path = Some(path.clone());
    let mut app = App::new_headless(config).unwrap();
    app.background_image_loading = true;
    for generation in 1..=2 {
        let replacement = directory.path().join("replacement.png");
        std::fs::copy(&path, &replacement).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::File::open(&replacement)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        std::fs::rename(replacement, &path).unwrap();
        assert!(app.poll_background_image());
        assert_eq!(app.background_image_generation, generation);
        assert!(app.background_image_loading);
        assert!(app.background_image_pending);
        assert!(!app.poll_background_image());
    }
    let newest = app.last_background_image_stamp;
    app.handle_background_image_loaded(0, path.clone(), Ok(old));
    assert_eq!(
        app.last_background_image_stamp, newest,
        "stale completion cannot retire the newest file version"
    );
    assert!(!app.background_image_pending);
    // A headless fixture cannot start the coalesced worker, so complete the
    // latest generation explicitly through the production event handler.
    let latest = crate::renderer::load_background_image(&path).unwrap();
    app.handle_background_image_loaded(2, path.clone(), Ok(latest.clone()));
    assert!(!app.poll_background_image());
    app.config.window.background_image.path = None;
    app.sync_background_image(false);
    app.handle_background_image_loaded(2, path, Ok(latest));
    assert!(app.last_background_image_stamp.is_none());
    assert!(app.background_image_path.is_none());
    assert!(!app.poll_background_image());
}

#[test]
fn config_reload_only_overrides_application_keyboard_type_when_profile_changes() {
    let mut config = Config::default();
    config.keyboard.xterm_type = crate::config::XtermKeyboardTypeConfig::Legacy;
    let mut app = App::new_headless(config).unwrap();
    let mut tabs = TabManager::for_test(ShellConfig::default());
    tabs.set_terminal_policy(
        crate::terminal::policy::TerminalPolicy {
            xterm_keyboard_type: termeric_core::term::XtermKeyboardType::Legacy,
            ..crate::terminal::policy::TerminalPolicy::with_scrollback(1_000, 8 * 1024 * 1024)
        },
        true,
    );
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "sleep 1".into()],
        None,
        Some("config-keyboard-profile"),
        None,
        HashMap::new(),
    )
    .unwrap();
    app.tabs = Some(tabs);
    app.active_terminal_mut().unwrap().process(b"\x1b[?1061h");

    let mut unrelated = app.config.clone();
    unrelated.cursor.blink_rate += 1;
    app.apply_config(unrelated).unwrap();
    assert_eq!(
        app.active_terminal().unwrap().xterm_keyboard_type(),
        termeric_core::term::XtermKeyboardType::Vt220
    );

    let mut changed = app.config.clone();
    changed.keyboard.xterm_type = crate::config::XtermKeyboardTypeConfig::Normal;
    app.apply_config(changed).unwrap();
    let terminal = app.active_terminal().unwrap();
    assert_eq!(
        terminal.xterm_keyboard_type(),
        termeric_core::term::XtermKeyboardType::Normal
    );
    assert_eq!(
        terminal.initial_xterm_keyboard_type(),
        termeric_core::term::XtermKeyboardType::Normal
    );
}

/// A headless window whose configuration file, holding `text`, is its own.
fn app_with_config_file(text: &str) -> (tempfile::TempDir, std::path::PathBuf, App) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, text).unwrap();
    let mut app = App::new_headless(Config::load_from(&path).unwrap()).unwrap();
    app.config_path = Some(path.clone());
    (directory, path, app)
}

#[test]
fn saving_from_settings_writes_only_the_changed_setting() {
    let original = "# Mine.\n[window]\npadding = 4 # tight\n";
    let (_directory, path, mut app) = app_with_config_file(original);
    app.apply_settings_action(SettingsAction::SetVisualBell(false));
    assert!(app.config_has_unsaved_changes());
    app.apply_settings_action(SettingsAction::SaveConfig);
    assert!(!app.config_has_unsaved_changes());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        format!("{original}\n[bell]\nvisual = false\n")
    );
}

#[test]
fn a_failed_save_says_why() {
    let (directory, _path, mut app) = app_with_config_file("");
    // A configuration path under a regular file cannot be resolved; the
    // operating system's reason is the part worth reading.
    let blocker = directory.path().join("blocker");
    std::fs::write(&blocker, "").unwrap();
    app.config_path = Some(blocker.join("config.toml"));
    app.apply_settings_action(SettingsAction::SetVisualBell(false));
    app.apply_settings_action(SettingsAction::SaveConfig);
    let message = &app.toasts.iter().last().unwrap().message;
    assert!(
        message.starts_with("Configuration save failed"),
        "{message}"
    );
    assert!(message.contains("Not a directory"), "{message}");
}

#[test]
fn without_a_configuration_directory_defaults_run_and_nothing_is_saved() {
    crate::paths::without_home(|| {
        let (_, warning) = Config::load_for_startup();
        assert!(warning.is_some(), "startup says settings cannot be saved");
        let mut app = App::new_headless(Config::default()).unwrap();
        assert_eq!(app.config_path, None);
        assert_eq!(app.supervisor_rules_path, None);
        assert_eq!(app.runtime_plugin_paths, None);
        app.apply_settings_action(SettingsAction::SetVisualBell(false));
        assert!(app.save_config().is_err());
        assert!(app.config_has_unsaved_changes());
        assert!(app.discover_runtime_plugins_for_app().unwrap().0.is_empty());
    });
}

#[test]
fn a_save_that_keeps_an_edit_made_elsewhere_leaves_it_for_the_reload() {
    let (_directory, path, mut app) = app_with_config_file("[window]\npadding = 4\n");
    // A save that leaves the file holding just the live settings needs no
    // reload.
    app.apply_settings_action(SettingsAction::SetVisualBell(false));
    app.save_config().unwrap();
    let saved = std::fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(app.last_config_mtime, Some(saved));

    // Another program changes the file before the poller sees it.
    std::fs::write(&path, "[window]\npadding = 20\n\n[bell]\nvisual = false\n").unwrap();
    app.apply_settings_action(SettingsAction::SetAudioBell(true));
    app.save_config().unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "[window]\npadding = 20\n\n[bell]\nvisual = false\naudio = true\n"
    );
    // The poller still sees a change, and its reload brings in padding = 20.
    assert_eq!(app.last_config_mtime, Some(saved));
}

#[test]
fn a_recorded_shortcut_saves_only_its_entry_and_saves_keep_working() {
    use crate::input::keybindings::{BindableAction, KeyChord};
    use crate::ui::shortcuts_panel::ShortcutsAction;
    // Out of the editor's order, with comments, and spelled by hand.
    let original = "\
# Zoom the focused pane with Ctrl+Shift+Z
[[shortcuts]]
action = \"toggle_pane_zoom\"
key = \"z\"
ctrl = true
shift = true

# Remove the default Alt+Left binding so the key reaches your shell
[[shortcuts]]
action = \"navigate_pane_left\"

[[shortcuts]]
action = \"scroll_page_up\"
key = \"PageUp\"
ctrl = true
";
    let (_directory, path, mut app) = app_with_config_file(original);
    let saved_without_reload = |app: &App| {
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        app.last_config_mtime == Some(modified)
    };
    assert!(!app.config_has_unsaved_changes());

    app.apply_shortcuts_action(ShortcutsAction::SetShortcut {
        action: BindableAction::Copy,
        chord: Some(KeyChord::new("k", true, false, false, false)),
    });
    assert!(app.config_has_unsaved_changes());
    app.save_config().unwrap();
    assert!(!app.config_has_unsaved_changes());
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.starts_with(original), "{saved}");
    assert!(saved_without_reload(&app), "a plain save needs no reload");

    // Rebinding an action with an entry changes that entry alone.
    app.apply_shortcuts_action(ShortcutsAction::SetShortcut {
        action: BindableAction::TogglePaneZoom,
        chord: Some(KeyChord::new("m", true, false, true, false)),
    });
    app.save_config().unwrap();
    let rebound = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        rebound.replacen("key = \"m\"", "key = \"z\"", 1),
        saved,
        "{rebound}"
    );
    assert!(saved_without_reload(&app));

    // Saves from elsewhere keep working, and the poller sees nothing new.
    app.apply_settings_action(SettingsAction::SetVisualBell(false));
    app.apply_settings_action(SettingsAction::SaveConfig);
    assert!(saved_without_reload(&app));
    app.last_config_check = Instant::now()
        .checked_sub(App::CONFIG_POLL_INTERVAL)
        .expect("the clock is past one poll interval");
    app.poll_config_for_hot_reload();
    let messages: Vec<_> = app.toasts.iter().map(|toast| &toast.message).collect();
    assert!(
        messages
            .iter()
            .all(|message| message.starts_with("Configuration saved")),
        "{messages:?}"
    );

    let loaded = Config::load_from(&path).unwrap();
    let registry = crate::input::ShortcutRegistry::with_overrides(&loaded.shortcuts);
    for action in BindableAction::all() {
        assert_eq!(
            registry.binding_for(*action),
            app.shortcut_registry.binding_for(*action),
            "{action:?}"
        );
    }
    assert!(!loaded.bell.visual);
}

#[test]
fn zoom_changes_only_the_window_and_is_never_saved() {
    let (_directory, path, mut app) = app_with_config_file("[font]\nsize = 12\n");
    app.zoom_font(2.0);
    app.zoom_font(2.0);
    assert_eq!(app.effective_font_size(), 16.0);
    assert_eq!(app.config.font.size, 12.0);
    assert!(!app.config_has_unsaved_changes(), "zoom is not a setting");

    // Saving another change keeps the configured size in the file.
    app.apply_settings_action(SettingsAction::SetVisualBell(false));
    app.save_config().unwrap();
    let saved = Config::load_from(&path).unwrap();
    assert_eq!(saved.font.size, 12.0);
    assert!(!saved.bell.visual);
    // An unrelated change keeps the zoom, and Ctrl+0 returns to the
    // configured size rather than the zoomed one.
    assert_eq!(app.effective_font_size(), 16.0);
    app.reset_font_zoom();
    assert_eq!(app.effective_font_size(), 12.0);

    // Zooming stays within the configured bounds.
    for _ in 0..40 {
        app.zoom_font(2.0);
    }
    assert_eq!(app.effective_font_size(), 72.0);
}

#[test]
fn changing_the_configured_size_ends_a_zoom() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.zoom_font(-2.0);
    assert_eq!(app.effective_font_size(), 10.0);
    app.apply_settings_action(SettingsAction::SetFontSize(14.0));
    assert_eq!(app.effective_font_size(), 14.0);

    // A reload that changes the size ends it too; one that does not keeps it.
    app.zoom_font(2.0);
    let mut reloaded = app.config.clone();
    reloaded.cursor.blink = !reloaded.cursor.blink;
    app.apply_config(reloaded).unwrap();
    assert_eq!(app.effective_font_size(), 16.0);
    let mut reloaded = app.config.clone();
    reloaded.font.size = 13.0;
    app.apply_config(reloaded).unwrap();
    assert_eq!(app.effective_font_size(), 13.0);
}

#[test]
fn the_command_line_renderer_survives_reloads_and_is_never_saved() {
    use crate::config::RendererBackend;
    let (_directory, path, mut app) = app_with_config_file("[window]\npadding = 4\n");
    app.renderer_backend_override = Some(RendererBackend::Software);
    assert_eq!(app.renderer_backend(), RendererBackend::Software);

    app.apply_settings_action(SettingsAction::SetVisualBell(false));
    app.save_config().unwrap();
    assert_eq!(
        Config::load_from(&path).unwrap().renderer.backend,
        RendererBackend::Auto
    );
    std::fs::write(&path, "[window]\npadding = 20\n").unwrap();
    poll_config_now(&mut app);
    assert_eq!(app.config.window.padding, 20);
    assert_eq!(app.renderer_backend(), RendererBackend::Software);
}

/// Make the next poll look at the configuration file as if it had changed.
fn poll_config_now(app: &mut App) {
    app.last_config_check = Instant::now()
        .checked_sub(App::CONFIG_POLL_INTERVAL)
        .expect("the clock is past one poll interval");
    app.last_config_mtime = None;
    app.poll_config_for_hot_reload();
}

/// Messages of the toasts that stay until dismissed.
fn lasting_toasts(app: &App) -> Vec<String> {
    let later = Instant::now() + std::time::Duration::from_secs(3600);
    app.toasts
        .iter()
        .filter(|toast| !toast.is_expired(later))
        .map(|toast| toast.message.clone())
        .collect()
}

#[test]
fn a_reload_with_a_typo_keeps_the_settings_and_says_so_until_fixed() {
    let (_directory, path, mut app) = app_with_config_file("[window]\npadding = 4\n");
    std::fs::write(&path, "[window]\npadding = 6\npading = 7\n").unwrap();
    poll_config_now(&mut app);
    assert_eq!(app.config.window.padding, 4, "the previous settings stay");
    let problems = lasting_toasts(&app);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains("unknown field `pading`"),
        "{problems:?}"
    );
    assert!(problems[0].contains("line 3"), "{problems:?}");

    // A second problem replaces the first rather than stacking.
    std::fs::write(&path, "[window]\npadding = 9999\n").unwrap();
    poll_config_now(&mut app);
    let problems = lasting_toasts(&app);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("window.padding"), "{problems:?}");

    std::fs::write(&path, "[window]\npadding = 6\n").unwrap();
    poll_config_now(&mut app);
    assert_eq!(app.config.window.padding, 6);
    assert!(lasting_toasts(&app).is_empty());
}

#[test]
fn a_theme_that_does_not_load_stays_configured_until_another_is_chosen() {
    let original = "[theme]\nname = \"arrowroot\"\npath = \"/nonexistent/arrowroot.toml\"\n";
    let (_directory, path, mut app) = app_with_config_file(original);
    assert_eq!(app.theme.meta.name, "Turmeric");
    assert_eq!(app.config.theme.name, "arrowroot");
    let problems = lasting_toasts(&app);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].starts_with("Theme could not be loaded"),
        "{problems:?}"
    );
    assert!(!app.config_has_unsaved_changes());

    // Other settings still apply and save; the theme stays as written.
    app.apply_settings_action(SettingsAction::SetVisualBell(false));
    assert!(!app.config.bell.visual);
    app.config.commands.push(crate::config::CustomCommand {
        name: "logs".into(),
        command: "journalctl -f".into(),
        alias: None,
    });
    app.save_config().unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.starts_with(original), "{saved}");
    assert_eq!(
        lasting_toasts(&app).len(),
        1,
        "the notice stays, deduplicated"
    );

    // A cancelled picker puts back the theme that was on screen.
    app.open_theme_picker();
    app.command_palette.select_command("dracula");
    app.sync_theme_preview();
    app.command_palette.close();
    app.sync_theme_preview();
    assert_eq!(app.theme.meta.name, "Turmeric");

    // Choosing a theme, even the one on screen, replaces the configured one.
    app.commit_theme("turmeric");
    let saved = Config::load_from(&path).unwrap();
    assert_eq!(saved.theme.name, "turmeric");
    assert_eq!(saved.theme.path, None);
    assert!(lasting_toasts(&app).is_empty());
}

/// Make the next poll look at the files it watches, without treating the
/// configuration file as changed.
fn poll_files_now(app: &mut App) {
    app.last_config_mtime = app.config_path.as_deref().and_then(super::file_mtime);
    app.last_config_check = Instant::now()
        .checked_sub(App::CONFIG_POLL_INTERVAL)
        .expect("the clock is past one poll interval");
    app.poll_config_for_hot_reload();
}

/// Write `text` as the theme file at `path`, modified `second` seconds into
/// the epoch, so each write is a change however coarse the file clock is.
fn write_theme(path: &std::path::Path, text: &str, second: u64) {
    std::fs::write(path, text).unwrap();
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(second))
        .unwrap();
}

/// Turmeric's palette under another name.
fn custom_theme(name: &str) -> String {
    include_str!("../../../themes/roots/turmeric.toml").replacen(
        "name = \"Turmeric\"",
        &format!("name = \"{name}\""),
        1,
    )
}

#[test]
fn a_theme_that_breaks_while_running_keeps_a_notice_until_it_loads() {
    let directory = tempfile::tempdir().unwrap();
    let theme_path = directory.path().join("arrowroot.toml");
    write_theme(&theme_path, &custom_theme("Arrowroot"), 1_000);
    let config = format!("[theme]\npath = {:?}\n", theme_path.display().to_string());
    let (_config_directory, _path, mut app) = app_with_config_file(&config);
    assert_eq!(app.theme.meta.name, "Arrowroot");

    write_theme(&theme_path, "[meta\nname = \"Arrowroot\"\n", 1_001);
    poll_files_now(&mut app);
    assert_eq!(app.theme.meta.name, "Arrowroot", "the colors stay");
    let problems = lasting_toasts(&app);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].starts_with("Theme could not be loaded; showing Arrowroot"),
        "{problems:?}"
    );
    assert!(problems[0].contains("arrowroot.toml"), "{problems:?}");

    // Nothing changed: no reload, and the notice stays.
    poll_files_now(&mut app);
    assert_eq!(lasting_toasts(&app), problems);

    write_theme(&theme_path, &custom_theme("Arrowroot Two"), 1_002);
    poll_files_now(&mut app);
    assert_eq!(app.theme.meta.name, "Arrowroot Two");
    assert!(lasting_toasts(&app).is_empty());
}

#[test]
fn a_missing_theme_file_loads_when_it_appears_and_follows_edits_after() {
    let directory = tempfile::tempdir().unwrap();
    let theme_path = directory.path().join("arrowroot.toml");
    let config = format!("[theme]\npath = {:?}\n", theme_path.display().to_string());
    let (_config_directory, _path, mut app) = app_with_config_file(&config);
    assert_eq!(app.theme.meta.name, "Turmeric");
    assert_eq!(lasting_toasts(&app).len(), 1);

    // Still missing: nothing to do.
    poll_files_now(&mut app);
    assert_eq!(lasting_toasts(&app).len(), 1);

    write_theme(&theme_path, &custom_theme("Arrowroot"), 1_000);
    poll_files_now(&mut app);
    assert_eq!(app.theme.meta.name, "Arrowroot");
    assert!(lasting_toasts(&app).is_empty());

    write_theme(&theme_path, &custom_theme("Arrowroot Two"), 1_001);
    poll_files_now(&mut app);
    assert_eq!(app.theme.meta.name, "Arrowroot Two");

    // Gone again: the colors stay, and the notice says why.
    std::fs::remove_file(&theme_path).unwrap();
    poll_files_now(&mut app);
    assert_eq!(app.theme.meta.name, "Arrowroot Two");
    assert_eq!(lasting_toasts(&app).len(), 1);
}

#[test]
fn a_theme_edited_during_a_preview_is_the_one_cancelling_puts_back() {
    let directory = tempfile::tempdir().unwrap();
    let theme_path = directory.path().join("arrowroot.toml");
    write_theme(&theme_path, &custom_theme("Arrowroot"), 1_000);
    let config = format!("[theme]\npath = {:?}\n", theme_path.display().to_string());
    let (_config_directory, _path, mut app) = app_with_config_file(&config);

    app.open_theme_picker();
    app.command_palette.select_command("dracula");
    app.sync_theme_preview();
    write_theme(&theme_path, &custom_theme("Arrowroot Two"), 1_001);
    poll_files_now(&mut app);
    assert_eq!(app.theme.meta.name, "Dracula", "the preview stays");

    // Cancelling puts back the configured theme as it is now.
    app.command_palette.close();
    app.sync_theme_preview();
    assert_eq!(app.theme.meta.name, "Arrowroot Two");
}

#[test]
fn reset_configuration_asks_first_and_keeps_the_old_file() {
    let original = "# Mine.\n[bell]\nvisual = false\n";
    let (directory, path, mut app) = app_with_config_file(original);
    let backups = |directory: &std::path::Path| {
        std::fs::read_dir(directory)
            .unwrap()
            .filter_map(|entry| {
                let name = entry.unwrap().file_name().into_string().unwrap();
                name.ends_with(".bak").then_some(name)
            })
            .collect::<Vec<_>>()
    };

    // Cancelling changes nothing.
    app.handle_action(Action::ResetConfig);
    let request = app.dialog.request_id();
    assert_eq!(app.dialog.title(), "Reset all settings?");
    // A stray Enter cancels.
    assert_eq!(
        app.dialog.focus(),
        Some(crate::ui::dialog::DialogFocus::Button(0))
    );
    app.apply_dialog_action(crate::ui::dialog::DialogAction::Answer {
        request_id: request,
        id: crate::ui::dialog::CANCEL.into(),
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    assert!(backups(directory.path()).is_empty());
    assert!(!app.config.bell.visual);

    // Confirming replaces the file, keeps the old one, and applies defaults.
    app.handle_action(Action::ResetConfig);
    let request = app.dialog.request_id();
    app.apply_dialog_action(crate::ui::dialog::DialogAction::Answer {
        request_id: request,
        id: "config-reset".into(),
    });
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        crate::config::save::RESET_TEXT
    );
    let kept = backups(directory.path());
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(
        std::fs::read_to_string(directory.path().join(&kept[0])).unwrap(),
        original
    );
    assert!(app.config.bell.visual);
    assert!(!app.config_has_unsaved_changes());
    assert!(
        app.toasts
            .iter()
            .any(|toast| toast.message.contains(&kept[0])),
        "the notice names the copy"
    );
}

#[test]
fn ctrl_r_in_settings_does_not_reset() {
    let original = "[bell]\nvisual = false\n";
    let (_directory, path, mut app) = app_with_config_file(original);
    app.settings_panel.toggle();
    app.modifiers = ModifiersState::CONTROL;
    app.handle_settings_panel_key(&winit::keyboard::Key::Character("r".into()));
    assert!(!app.dialog.is_active());
    assert!(!app.config.bell.visual);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
}

/// Record `key` with `modifiers` for `action` in the shortcuts editor
/// through the keyboard, as a person does.
fn record_shortcut(
    app: &mut App,
    action: crate::input::keybindings::BindableAction,
    key: &str,
    modifiers: ModifiersState,
) {
    use crate::ui::shortcuts_panel::ShortcutsAction;
    if !app.shortcuts_panel.is_active() {
        app.shortcuts_panel.open();
    }
    app.apply_shortcuts_action(ShortcutsAction::StartRecording(action));
    assert!(app.shortcuts_panel.is_recording());
    app.modifiers = modifiers;
    app.handle_shortcuts_panel_key(&winit::keyboard::Key::Character(key.into()));
    app.modifiers = ModifiersState::empty();
}

#[test]
fn recording_a_chord_another_action_uses_asks_before_changing_anything() {
    use crate::input::keybindings::{BindableAction, KeyChord};
    use crate::ui::dialog::{CANCEL, DialogAction, DialogFocus};
    use crate::ui::modal::ModalLayer;
    let new_tab = KeyChord::new("t", true, false, true, false);
    let ctrl_shift = ModifiersState::CONTROL | ModifiersState::SHIFT;
    let mut app = App::new_headless(Config::default()).unwrap();
    let rename_tab = app
        .shortcut_registry
        .binding_for(BindableAction::RenameTab)
        .cloned();
    let unchanged = |app: &App| {
        assert_eq!(
            app.shortcut_registry.binding_for(BindableAction::NewTab),
            Some(&new_tab)
        );
        assert_eq!(
            app.shortcut_registry
                .binding_for(BindableAction::RenameTab)
                .cloned(),
            rename_tab
        );
        assert!(app.config.shortcuts.is_empty());
    };

    // The question names the chord and both actions, and nothing has
    // changed while it is open.
    record_shortcut(&mut app, BindableAction::RenameTab, "T", ctrl_shift);
    assert_eq!(app.top_modal_layer(), Some(ModalLayer::Dialog));
    assert_eq!(app.dialog.title(), "Use Ctrl+Shift+T for Rename Tab?");
    assert!(app.dialog_text().contains("New Tab"));
    unchanged(&app);

    // Cancel, Escape, and Enter on the initial focus (Cancel) all keep
    // both.
    let request_id = app.dialog.request_id();
    app.apply_dialog_action(DialogAction::Answer {
        request_id,
        id: CANCEL.into(),
    });
    assert!(!app.dialog.is_awaiting_current());
    assert!(app.shortcuts_panel.is_active());
    unchanged(&app);
    for key in [
        winit::keyboard::NamedKey::Escape,
        winit::keyboard::NamedKey::Enter,
    ] {
        record_shortcut(&mut app, BindableAction::RenameTab, "T", ctrl_shift);
        assert!(app.dialog.is_awaiting_current());
        app.handle_dialog_key(&winit::keyboard::Key::Named(key), false);
        assert!(!app.dialog.is_awaiting_current(), "{key:?}");
        assert!(app.dialog_flow.is_none());
        unchanged(&app);
    }

    // Tab reaches Replace, and Enter takes the chord, leaving New Tab
    // unbound in the file too.
    record_shortcut(&mut app, BindableAction::RenameTab, "T", ctrl_shift);
    app.handle_dialog_key(
        &winit::keyboard::Key::Named(winit::keyboard::NamedKey::Tab),
        false,
    );
    assert_eq!(app.dialog.focus(), Some(DialogFocus::Button(1)));
    app.handle_dialog_key(
        &winit::keyboard::Key::Named(winit::keyboard::NamedKey::Enter),
        false,
    );
    assert!(!app.dialog.is_awaiting_current());
    assert_eq!(
        app.shortcut_registry.binding_for(BindableAction::RenameTab),
        Some(&new_tab)
    );
    assert_eq!(
        app.shortcut_registry.binding_for(BindableAction::NewTab),
        None
    );
    assert_eq!(
        crate::input::ShortcutRegistry::displaced_defaults(&app.config.shortcuts, false),
        []
    );
    app.config.validate().unwrap();

    // A chord nothing uses is taken without a question.
    record_shortcut(&mut app, BindableAction::NewTab, "N", ctrl_shift);
    assert!(!app.dialog.is_awaiting_current());
    assert_eq!(
        app.shortcut_registry.binding_for(BindableAction::NewTab),
        Some(&KeyChord::new("n", true, false, true, false))
    );
}

#[test]
fn a_reassignment_answered_after_a_reload_moved_the_chord_asks_again() {
    use crate::input::keybindings::{BindableAction, KeyChord};
    use crate::ui::dialog::DialogAction;
    let chord = KeyChord::new("t", true, false, true, false);
    let (_directory, path, mut app) = app_with_config_file("");
    record_shortcut(
        &mut app,
        BindableAction::RenameTab,
        "T",
        ModifiersState::CONTROL | ModifiersState::SHIFT,
    );
    let request_id = app.dialog.request_id();

    // Meanwhile the file gives Ctrl+Shift+T to Copy instead.
    std::fs::write(
        &path,
        "[[shortcuts]]\naction = \"copy\"\nkey = \"t\"\nctrl = true\nshift = true\n",
    )
    .unwrap();
    app.reload_config_from_disk();
    assert_eq!(
        app.shortcut_registry.conflict_for(&chord),
        Some(BindableAction::Copy)
    );

    // Agreeing to take it from New Tab is no answer for Copy.
    app.apply_dialog_action(DialogAction::Answer {
        request_id,
        id: "shortcut-reassign".into(),
    });
    assert_eq!(
        app.shortcut_registry.conflict_for(&chord),
        Some(BindableAction::Copy)
    );
    assert!(app.dialog.is_awaiting_current());
    assert!(app.dialog_text().contains("Copy Selection"));
}

#[test]
fn an_entry_taking_a_default_shortcut_is_noted_once_until_that_changes() {
    use crate::input::keybindings::{BindableAction, KeyChord};
    let notices = |app: &App| -> Vec<String> {
        app.toasts
            .iter()
            .map(|toast| toast.message.clone())
            .filter(|message| message.contains("has no shortcut"))
            .collect()
    };
    let source = "[[shortcuts]]\naction = \"rename_tab\"\nkey = \"t\"\nctrl = true\nshift = true\n";
    let (_directory, path, mut app) = app_with_config_file(source);
    assert_eq!(
        app.shortcut_registry.binding_for(BindableAction::RenameTab),
        Some(&KeyChord::new("t", true, false, true, false))
    );
    assert_eq!(
        notices(&app),
        ["Ctrl+Shift+T now runs Rename Tab; New Tab has no shortcut"]
    );

    // Reloading the same file, or changing an unrelated setting, does not
    // repeat it.
    app.reload_config_from_disk();
    app.apply_settings_action(SettingsAction::SetVisualBell(false));
    assert_eq!(notices(&app).len(), 1);

    // Giving New Tab a shortcut of its own ends it.
    record_shortcut(
        &mut app,
        BindableAction::NewTab,
        "N",
        ModifiersState::CONTROL | ModifiersState::SHIFT,
    );
    assert!(notices(&app).is_empty());

    // A file that unbinds New Tab itself says what it means: no notice.
    std::fs::write(
        &path,
        format!("{source}\n[[shortcuts]]\naction = \"new_tab\"\n"),
    )
    .unwrap();
    app.reload_config_from_disk();
    assert!(notices(&app).is_empty());
}

#[test]
fn agent_shortcuts_are_no_conflict_while_agent_features_are_off() {
    use crate::input::keybindings::{BindableAction, KeyChord};
    let agent_view = KeyChord::new("a", true, false, true, false);
    let mut app = App::new_headless(Config::default()).unwrap();
    assert!(!app.agent_features_enabled());
    record_shortcut(
        &mut app,
        BindableAction::RenameTab,
        "A",
        ModifiersState::CONTROL | ModifiersState::SHIFT,
    );
    assert!(
        !app.dialog.is_awaiting_current(),
        "no question names Agent View while it is off"
    );
    assert!(app.dialog_flow.is_none());
    assert_eq!(
        app.shortcut_registry.binding_for(BindableAction::RenameTab),
        Some(&agent_view)
    );

    // The chord stays Rename Tab's once agent features are on; Agent View
    // has none, as the saved configuration says.
    app.apply_settings_action(SettingsAction::SetAgentFeaturesEnabled(true));
    assert_eq!(
        app.shortcut_registry
            .binding_for(BindableAction::ToggleAgentView),
        None
    );
    assert_eq!(
        app.shortcut_registry.conflict_for(&agent_view),
        Some(BindableAction::RenameTab)
    );
}

#[test]
fn agent_shortcuts_come_back_with_agent_features() {
    use crate::input::keybindings::BindableAction;
    let mut app = App::new_headless(Config::default()).unwrap();
    let pressed = |app: &App| {
        app.shortcut_registry.check(
            &winit::keyboard::Key::Character("A".into()),
            ModifiersState::CONTROL | ModifiersState::SHIFT,
        )
    };
    assert_eq!(pressed(&app), None);
    app.apply_settings_action(SettingsAction::SetAgentFeaturesEnabled(true));
    assert_eq!(pressed(&app), Some(BindableAction::ToggleAgentView));
    app.apply_settings_action(SettingsAction::SetAgentFeaturesEnabled(false));
    assert_eq!(pressed(&app), None);
}

#[test]
fn a_displaced_agent_shortcut_is_noted_only_while_agent_features_are_on() {
    let notices = |app: &App| -> Vec<String> {
        app.toasts
            .iter()
            .map(|toast| toast.message.clone())
            .filter(|message| message.contains("has no shortcut"))
            .collect()
    };
    let source = "[[shortcuts]]\naction = \"rename_tab\"\nkey = \"a\"\nctrl = true\nshift = true\n";
    let (_directory, _path, mut app) = app_with_config_file(source);
    assert!(notices(&app).is_empty());

    app.apply_settings_action(SettingsAction::SetAgentFeaturesEnabled(true));
    assert_eq!(
        notices(&app),
        ["Ctrl+Shift+A now runs Rename Tab; Agent View has no shortcut"]
    );
    app.apply_settings_action(SettingsAction::SetAgentFeaturesEnabled(false));
    assert!(notices(&app).is_empty());
}

#[test]
fn settings_lists_the_themes_directory_when_it_opens_not_as_it_draws() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let themes_dir = Config::themes_dir().expect("test builds have a configuration directory");
    std::fs::create_dir_all(&themes_dir).unwrap();
    let theme_path = themes_dir.join("settings-opening-probe.toml");
    std::fs::write(&theme_path, custom_theme("Settings Opening Probe")).unwrap();
    let offers_probe = |app: &App| {
        app.settings_panel
            .build_entries(&app.config, &app.shortcut_registry)
            .iter()
            .any(|entry| {
                matches!(
                    &entry.kind,
                    crate::ui::settings::SettingKind::Dropdown { options, .. }
                        if options.iter().any(|name| name == "settings-opening-probe")
                )
            })
    };

    // Building rows, which happens every frame Settings is open, does not
    // read the themes directory; opening Settings does.
    let before_opening = offers_probe(&app);
    app.toggle_settings_surface();
    let after_opening = offers_probe(&app);
    std::fs::remove_file(&theme_path).unwrap();
    assert!(!before_opening);
    assert!(after_opening);
}
