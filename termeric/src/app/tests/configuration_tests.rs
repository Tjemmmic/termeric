//! Applying configuration and settings without a window.

use super::*;

#[test]
fn headless_config_application_updates_agents_harvest_and_palette() {
    let mut config = Config::default();
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
fn headless_font_size_setting_updates_reset_size_and_preserves_other_config() {
    let mut app = App::new_headless(Config::default()).unwrap();
    for size in [8.0, 15.5, 72.0, 14.0] {
        let mut expected = app.config.clone();
        expected.font.size = size;
        app.apply_settings_action(SettingsAction::SetFontSize(size));
        assert_eq!(app.config.font.size, size);
        assert_eq!(app.default_font_size, size);
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
        assert_eq!(app.default_font_size, 15.5);
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
    let mut app = App::new_headless(Config::default()).unwrap();
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

    app.apply_settings_action(SettingsAction::SetReduceMotion(
        crate::config::AccessibilityPreference::Off,
    ));
    assert!(!app.reduced_motion_enabled());
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
    app.config_path = path.clone();
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
