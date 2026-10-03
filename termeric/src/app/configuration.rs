//! Live configuration, settings, theme, and shortcut application.

use super::*;

const TAB_BAR_HOVER_FADE_SECONDS: f32 = 0.1;

impl App {
    pub(super) const CONFIG_POLL_INTERVAL: std::time::Duration =
        std::time::Duration::from_millis(750);

    pub(super) fn change_font_size(&mut self, delta: f32) {
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let current = renderer.font_size();
        let new_size = (current + delta).clamp(8.0, 72.0);
        self.set_font_size(new_size);
    }

    /// Set font size to a specific value
    pub(super) fn set_font_size(&mut self, size: f32) {
        self.config.font.size = size;
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let Some(window) = self.window.clone() else {
            return;
        };

        renderer.set_font_size(size);

        // Recalculate grid size after font change
        let window_size = window.inner_size();
        self.sync_terminal_geometry(window_size.width, window_size.height);
        log::info!(
            "Font size set to {}, grid resized to {}x{}",
            size,
            self.cols,
            self.rows
        );

        self.request_redraw(RedrawReason::ConfigReload);
    }

    /// Apply layout changes immediately and schedule at most one bounded
    /// off-event-loop decode for a new or modified local background image.
    pub(super) fn sync_background_image(&mut self, force_reload: bool) {
        let policy = self.config.window.background_image.clone();
        if let Some(renderer) = &mut self.renderer {
            renderer.set_background_image_layout(policy.fit, policy.opacity);
        }
        self.request_redraw(RedrawReason::ConfigReload);

        let path_changed = self.background_image_path != policy.path;
        if !force_reload && !path_changed {
            return;
        }
        self.background_image_path = policy.path.clone();
        self.last_background_image_stamp = policy.path.as_deref().and_then(|path| {
            crate::renderer::BackgroundImageStamp::for_path(
                &crate::renderer::resolve_background_image_path(path),
            )
        });
        self.background_image_generation = self.background_image_generation.wrapping_add(1);

        if policy.path.is_none() {
            self.background_image_pending = false;
            if let Some(renderer) = &mut self.renderer {
                renderer.clear_background_image();
            }
            return;
        }
        if self.background_image_loading {
            self.background_image_pending = true;
            return;
        }
        self.start_background_image_load();
    }

    fn start_background_image_load(&mut self) {
        let Some(configured_path) = self.background_image_path.clone() else {
            return;
        };
        let Some(window_id) = self.window.as_ref().map(|window| window.id()) else {
            return;
        };
        let Some(proxy) = EVENT_LOOP_PROXY.get().cloned() else {
            return;
        };
        let generation = self.background_image_generation;
        let resolved_path = crate::renderer::resolve_background_image_path(&configured_path);
        self.background_image_loading = true;
        let spawn = std::thread::Builder::new()
            .name("termeric-background-image".into())
            .spawn(move || {
                let result = crate::renderer::load_background_image(&resolved_path);
                let _ = proxy.send_event(AppEvent::BackgroundImageLoaded {
                    window_id,
                    generation,
                    path: resolved_path,
                    result,
                });
            });
        if let Err(error) = spawn {
            self.background_image_loading = false;
            log::warn!("Could not start background image loader: {error}");
            self.toasts
                .warning(format!("Background image loader could not start: {error}"));
        }
    }

    pub(super) fn handle_background_image_loaded(
        &mut self,
        generation: u64,
        path: PathBuf,
        result: std::result::Result<crate::renderer::DecodedBackgroundImage, String>,
    ) {
        self.background_image_loading = false;
        if generation == self.background_image_generation {
            match result {
                Ok(decoded) => {
                    // Bind completion to the version actually decoded. A
                    // replacement during load remains visible to the poller.
                    self.last_background_image_stamp = Some(decoded.source_stamp);
                    if let Some(renderer) = &mut self.renderer {
                        renderer.install_background_image(decoded);
                        renderer.set_background_image_layout(
                            self.config.window.background_image.fit,
                            self.config.window.background_image.opacity,
                        );
                    }
                    log::info!("Background image loaded: {}", path.display());
                    self.request_redraw(RedrawReason::ConfigReload);
                }
                Err(error) => {
                    log::warn!(
                        "Background image load failed; keeping last known good image: {error}"
                    );
                    self.toasts.warning(format!(
                        "Background image failed; keeping last known good image: {error}"
                    ));
                }
            }
        }

        if std::mem::take(&mut self.background_image_pending)
            && self.background_image_path.is_some()
        {
            self.start_background_image_load();
        }
    }

    /// Observe replacement separately from completion: a completed decode may
    /// describe an older inode/version than the path currently points to.
    pub(super) fn poll_background_image(&mut self) -> bool {
        let Some(path) = self.background_image_path.as_deref() else {
            return false;
        };
        let resolved = crate::renderer::resolve_background_image_path(path);
        let stamp = crate::renderer::BackgroundImageStamp::for_path(&resolved);
        if stamp == self.last_background_image_stamp {
            return false;
        }
        self.sync_background_image(true);
        true
    }

    /// Poll the config file's mtime; if it changed since our last load,
    /// reload + re-apply + toast. The event loop wakes for this deadline, but
    /// only renders when the observed file state changes.
    pub(super) fn poll_config_for_hot_reload(&mut self) -> bool {
        if self.last_config_check.elapsed() < Self::CONFIG_POLL_INTERVAL {
            return false;
        }
        self.last_config_check = Instant::now();
        let mut changed = false;

        let path = crate::config::Config::config_path();
        let current_mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();

        if current_mtime != self.last_config_mtime {
            changed = true;
            self.last_config_mtime = current_mtime;

            // Only reload if the file exists (avoid racing with editors that
            // briefly delete + rewrite).
            if path.exists() {
                match crate::config::Config::load_or_default() {
                    Ok(config) => {
                        if let Err(error) = self.apply_config(config) {
                            log::warn!("Config hot-reload rejected: {}", error);
                            self.toasts
                                .warning(format!("Config reload rejected: {error}"));
                        } else {
                            self.mark_config_saved();
                            log::info!("Config hot-reloaded");
                            self.toasts.info("Config reloaded");
                        }
                    }
                    Err(e) => {
                        log::warn!("Config hot-reload failed: {}", e);
                        self.toasts.warning(format!("Config reload failed: {}", e));
                    }
                }
            }
        }

        let plugin_registry_mtime = std::fs::metadata(crate::plugin::runtime::registry_path())
            .and_then(|metadata| metadata.modified())
            .ok();
        if plugin_registry_mtime != self.last_plugin_registry_mtime {
            changed = true;
            self.last_plugin_registry_mtime = plugin_registry_mtime;
            self.sync_plugins_from_config();
            self.toasts.info("Third-party plugins reloaded");
            log::info!("Third-party plugin registry hot-reloaded");
        }

        changed |= self.poll_background_image();

        let Some(theme_path) = self.active_theme_path.clone() else {
            return changed;
        };
        let theme_mtime = std::fs::metadata(&theme_path)
            .and_then(|metadata| metadata.modified())
            .ok();
        if theme_mtime == self.last_theme_mtime {
            return changed;
        }
        changed = true;
        self.last_theme_mtime = theme_mtime;

        match Theme::load(&theme_path) {
            Ok(theme) => {
                self.apply_theme(theme, Some(theme_path.clone()));
                self.toasts
                    .info(format!("Theme reloaded: {}", theme_path.display()));
            }
            Err(error) => {
                log::warn!("Theme hot-reload failed: {error:#}");
                self.toasts.warning(format!(
                    "Theme reload failed; keeping current theme: {error:#}"
                ));
            }
        }
        changed
    }

    /// Validate and atomically select a new configuration before applying it
    /// to all live subsystems. Invalid reloads leave the previous config live.
    pub(super) fn apply_config(&mut self, config: Config) -> std::result::Result<(), String> {
        config.validate().map_err(|error| format!("{error:#}"))?;
        Theme::resolve(&config.theme.name, config.theme.path.as_deref())
            .map_err(|error| format!("theme rejected: {error:#}"))?;
        let keyboard_type_changed = config.keyboard.xterm_type != self.config.keyboard.xterm_type;
        if config.commands != self.config.commands {
            // Revoke ambiguous row authority before replacing the inventory or
            // accepting any queued keyboard/assistive actions for the old list.
            let retained = crate::ui::commands::navigation::retained_command_keys(
                &self.config.commands,
                &config.commands,
            );
            self.commands_panel.retain_browse_targets(&retained);
            self.retain_commands_accessibility_rows(&retained);
        }
        let agents_changed = config.agents != self.config.agents;
        self.config = config;
        self.apply_config_side_effects(agents_changed);
        if keyboard_type_changed && let Some(tabs) = &mut self.tabs {
            tabs.set_xterm_keyboard_type(self.config.keyboard.xterm_type.terminal_type(), true);
        }
        Ok(())
    }

    /// Re-apply every runtime-owned piece of `self.config`. This is the single
    /// application path for startup, settings, reset, manual reload, and hot
    /// reload. Existing PTYs keep their process; shell changes affect new tabs.
    pub(super) fn apply_config_side_effects(&mut self, agents_changed: bool) {
        self.default_font_size = self.config.font.size;
        let tab_bar_position_changed = self.renderer.as_ref().is_some_and(|renderer| {
            renderer.tab_bar_position() != self.config.window.tab_bar_position
        });
        if let Some(renderer) = &mut self.renderer {
            let font = &self.config.font;
            renderer.set_font_families(font.family.clone(), &font.fallback, &font.range_overrides);
            renderer.set_font_size(font.size);
            renderer.set_font_weights(font.weight, font.bold_weight);
            renderer.set_line_height(font.line_height);
            renderer.set_tab_bar_position(self.config.window.tab_bar_position);
            renderer.set_padding(self.config.window.padding as f32);
        }
        if tab_bar_position_changed {
            self.tab_bar_hover = None;
            // The rail's width limits the dock's.
            self.apply_agent_dock_width();
        }
        // Every geometry input is in place: resize the terminals once.
        if let Some(window) = self.window.clone() {
            let size = window.inner_size();
            self.sync_terminal_geometry(size.width, size.height);
        }
        self.sync_agent_view_config();
        if let Some(renderer) = &mut self.renderer {
            renderer.set_ligatures(self.config.font.ligatures);
            renderer.set_font_features(&self.config.font.features);
            renderer.set_bold_is_bright(self.config.font.bold_is_bright);
            renderer.set_opacity(self.config.window.opacity);
        }
        self.sync_background_image(false);
        if let Some(window) = &self.window {
            window.set_decorations(self.config.window.decorations);
            window.set_window_level(
                self.config
                    .window
                    .level
                    .native_for_window(self.launch.quick_access),
            );
        }
        self.cursor_blink_period = (self.config.cursor.blink_rate as f32 / 1000.0) * 2.0;
        self.reset_cursor_blink();
        self.bell.set_config(BellConfig {
            visual: self.config.bell.visual,
            audio: self.config.bell.audio,
            visual_duration_ms: self.config.bell.visual_duration_ms,
            ..Default::default()
        });
        match Theme::resolve(&self.config.theme.name, self.config.theme.path.as_deref()) {
            Ok((theme, source)) => self.apply_theme(theme, source),
            Err(error) => {
                log::warn!("Theme changed while applying config: {error:#}");
                self.toasts.warning(format!(
                    "Theme reload failed; keeping current theme: {error:#}"
                ));
            }
        }
        if let Some(tabs) = &mut self.tabs {
            tabs.set_scrollback_limits(
                self.config.scrollback.lines,
                self.config.scrollback.max_memory_bytes(),
            );
            tabs.set_screen_checksum_reporting(self.config.security.allow_screen_checksum_reports);
            tabs.set_kitty_file_transmission(self.config.security.allow_kitty_file_transfers);
            // Reapplying unrelated settings must not undo a profile selected
            // by the foreground application through DECSET 1060/1061.
            tabs.set_xterm_keyboard_type(self.config.keyboard.xterm_type.terminal_type(), false);
            tabs.set_shell_config(self.config.shell.clone());
            for tab in tabs.all_tabs_mut() {
                if let Some(manager) = &mut tab.pane_manager {
                    manager.set_cwd_mode(self.config.panes.cwd);
                }
            }
        }
        self.shortcut_registry = ShortcutRegistry::new();
        self.shortcut_registry
            .apply_overrides(&self.config.shortcuts);
        self.compiled_copy_patterns = self
            .config
            .copy_mode
            .patterns
            .iter()
            .filter_map(|pattern| {
                regex::Regex::new(&pattern.regex)
                    .ok()
                    .map(|regex| (pattern.name.clone(), regex))
            })
            .collect();
        if agents_changed {
            self.sync_plugins_from_config();
        }
        self.sync_harvest_from_config();
        self.sync_accessibility_preferences();
        self.refresh_launch_profile_commands();
        self.keybinding_manager
            .load(self.config.keybindings.clone());
        self.request_redraw(RedrawReason::ConfigReload);
    }

    /// Hand Settings the installed and enabled state of each built-in agent.
    pub(super) fn sync_agent_settings_rows(&mut self) {
        let rows = crate::agents::builtin_agent_names()
            .filter_map(|(id, label)| {
                let agent = self.agents.get(id)?;
                Some(crate::ui::settings::AgentSettingsRow {
                    id,
                    label,
                    installed: agent.installed_path().is_some(),
                    enabled: agent.enabled,
                })
            })
            .collect();
        let custom: Vec<&str> = self
            .agents
            .all()
            .iter()
            .filter(|agent| crate::agents::builtin_agent_names().all(|(id, _)| id != agent.id))
            .map(|agent| agent.name.as_str())
            .collect();
        let summary = if custom.is_empty() {
            "None (add [agents.<id>] to config.toml)".to_string()
        } else {
            custom.join(", ")
        };
        let status = self.agent_hook_summary().label().to_string();
        self.settings_panel.set_agents(rows, summary, status);
    }

    pub(super) fn sync_plugins_from_config(&mut self) {
        self.plugin_runtime_generation = self.plugin_runtime_generation.wrapping_add(1).max(1);
        self.structured_plugin_worker
            .set_generation(self.plugin_runtime_generation);
        self.plugin_manager.clear();
        self.agents = crate::agents::AgentCatalog::from_config(&self.config.agents);
        self.sync_agent_settings_rows();
        for agent in self.agents.enabled() {
            let plugin = crate::plugin::agent::AgentPlugin::new(agent.clone());
            if let Err(error) = self.plugin_manager.register(Box::new(plugin)) {
                log::error!("Failed to register agent {:?}: {error}", agent.id);
                self.toasts.error(format!("{} failed: {error}", agent.name));
            }
        }
        let (runtime_plugins, statuses, registry_error) =
            match self.discover_runtime_plugins_for_app() {
                Ok((plugins, statuses)) => (plugins, statuses, None),
                Err(error) => {
                    let error = format!("{error:#}");
                    log::error!("{error}");
                    self.toasts
                        .error(format!("Third-party plugin registry rejected: {error}"));
                    let summary = crate::safe_text::truncate_with_ellipsis(&error, 512);
                    (Vec::new(), Vec::new(), Some(summary))
                }
            };
        for plugin in runtime_plugins {
            let id = crate::plugin::hooks::Plugin::id(&plugin).to_string();
            // Custom agents can take an installed plugin's ID after it was
            // installed; built-in agent IDs are refused at install time.
            if let Some(agent) = self.agents.get(&id) {
                log::error!("Third-party plugin {id:?} was not loaded: agent {id:?} uses its ID");
                self.toasts.error(format!(
                    "Third-party plugin {id} was not loaded: the agent {} uses the same ID",
                    agent.name
                ));
                continue;
            }
            if let Err(error) = self.plugin_manager.register(Box::new(plugin)) {
                log::error!("Third-party plugin {id:?} was not loaded: {error:#}");
                self.toasts
                    .error(format!("Third-party plugin {id} failed: {error:#}"));
            }
        }
        let summary = if let Some(error) = registry_error {
            format!("Registry rejected: {error}")
        } else if statuses.is_empty() {
            "None installed (use `termeric plugin --help`)".into()
        } else {
            statuses
                .iter()
                .take(4)
                .map(|status| {
                    format!(
                        "{}:{} req=[{}] grant=[{}] settings={}{}",
                        status.id,
                        if status.enabled { "on" } else { "off" },
                        status.requested,
                        status.granted,
                        status.settings.len(),
                        status.diagnostic.as_ref().map_or("", |_| " error")
                    )
                })
                .collect::<Vec<_>>()
                .join("; ")
        };
        self.settings_panel.set_runtime_plugin_summary(summary);
        self.refresh_plugin_commands();
        self.refresh_launch_profile_commands();
        log::info!("Loaded {} plugin(s)", self.plugin_manager.count());
    }

    pub(super) fn sync_harvest_from_config(&mut self) {
        self.command_palette.register_commands(core_commands(
            self.config.harvest.enabled,
            &self.shortcut_registry,
        ));
        if self.config.harvest.enabled {
            if let Some(harvest) = &mut self.harvest {
                harvest
                    .cellar
                    .set_max_commands(self.config.harvest.max_history);
            } else {
                self.harvest = Some(HarvestMode::with_capacity(self.config.harvest.max_history));
            }
        } else {
            self.harvest = None;
        }
    }

    pub(super) fn sync_accessibility_preferences(&mut self) {
        let reduced_motion = self.reduced_motion_enabled();
        let high_contrast = self.high_contrast_enabled();
        self.settings_panel.set_reduced_motion(reduced_motion);
        self.session_panel.set_reduced_motion(reduced_motion);
        self.commands_panel.set_reduced_motion(reduced_motion);
        self.agents_panel.set_reduced_motion(reduced_motion);
        self.command_palette.set_reduced_motion(reduced_motion);
        self.domain_picker
            .palette_mut()
            .set_reduced_motion(reduced_motion);
        self.unicode_picker
            .palette_mut()
            .set_reduced_motion(reduced_motion);
        self.shortcuts_panel.set_reduced_motion(reduced_motion);
        self.info_overlay.set_reduced_motion(reduced_motion);
        self.diff_review.set_reduced_motion(reduced_motion);
        self.context_picker.set_reduced_motion(reduced_motion);
        self.guardrail.set_reduced_motion(reduced_motion);
        self.toasts.set_reduced_motion(reduced_motion);
        if let Some(tabs) = &mut self.tabs {
            tabs.set_reduced_motion(reduced_motion);
        }
        if let Some(harvest) = &mut self.harvest {
            harvest.set_reduced_motion(reduced_motion);
        }
        if reduced_motion {
            self.copy_flash_start = None;
        }
        if let Some(renderer) = &mut self.renderer {
            renderer.set_high_contrast(&self.theme, high_contrast);
        }
    }

    pub(super) fn reduced_motion_enabled(&self) -> bool {
        self.config
            .accessibility
            .reduce_motion
            .resolve(self.system_accessibility.reduce_motion)
    }

    /// Fade-in of the hovered tab bar control, from 0 to 1. It restarts
    /// whenever the hovered control changes and ends at once when hover ends.
    pub(super) fn tab_bar_hover_progress(&self) -> f32 {
        if self.tab_bar_hover.is_none() {
            return 0.0;
        }
        if self.reduced_motion_enabled() {
            return 1.0;
        }
        (self.tab_bar_hover_since.elapsed().as_secs_f32() / TAB_BAR_HOVER_FADE_SECONDS).min(1.0)
    }

    pub(super) fn high_contrast_enabled(&self) -> bool {
        self.config
            .accessibility
            .high_contrast
            .resolve(self.system_accessibility.high_contrast)
    }

    pub(super) fn set_system_accessibility_preferences(
        &mut self,
        preferences: super::system_accessibility::SystemAccessibilityPreferences,
    ) {
        if self.system_accessibility == preferences {
            return;
        }
        let previous_reduce_motion = self.reduced_motion_enabled();
        let previous_high_contrast = self.high_contrast_enabled();
        self.system_accessibility = preferences;
        if previous_reduce_motion == self.reduced_motion_enabled()
            && previous_high_contrast == self.high_contrast_enabled()
        {
            return;
        }
        self.sync_accessibility_preferences();
        self.request_redraw(RedrawReason::ConfigReload);
    }

    pub(super) fn apply_theme(&mut self, theme: Theme, source: Option<std::path::PathBuf>) {
        self.settings_panel.set_theme_meta(theme.meta.clone());
        self.theme = theme;
        self.last_theme_mtime = source.as_ref().and_then(|path| {
            std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        });
        self.active_theme_path = source;
        // Glyphs are coverage masks tinted per vertex, so only the colors
        // change; the atlas stays valid and previews switch without stalls.
        if let Some(renderer) = &mut self.renderer {
            renderer.set_theme(&self.theme);
        }
        self.request_redraw(RedrawReason::ConfigReload);
    }

    /// Apply a settings panel action to the app config
    pub(super) fn apply_settings_action(&mut self, action: SettingsAction) {
        // Configuration changes edit a candidate that `apply_config`
        // validates and applies once: one geometry pass, and plugins
        // re-register only when agent configuration changed.
        let mut config = self.config.clone();
        let mut applied_message = None;
        match action {
            SettingsAction::PlaceFilterCaret(cursor) => {
                if self.settings_panel.place_filter_caret(cursor) {
                    self.ime.clear_preedit();
                    self.request_redraw(RedrawReason::KeyboardInput);
                }
                return;
            }
            SettingsAction::SetFontSize(size) => config.font.size = size,
            SettingsAction::SetFontFamily(family) => {
                config
                    .font
                    .fallback
                    .retain(|fallback| !fallback.eq_ignore_ascii_case(&family));
                config.font.family = family;
            }
            SettingsAction::SetFontWeight(weight) => config.font.weight = weight,
            SettingsAction::SetBoldFontWeight(weight) => config.font.bold_weight = weight,
            SettingsAction::SetFontFallback { index, family } => {
                if let Err(message) = set_font_fallback(&mut config.font, index, family) {
                    self.toasts.warning(message);
                    return;
                }
            }
            SettingsAction::ManageFontRangeOverrides => {
                self.open_font_range_manager();
                return;
            }
            SettingsAction::ManageFontFeatures => {
                self.open_font_feature_manager();
                return;
            }
            SettingsAction::SetFontLigatures(enabled) => config.font.ligatures = enabled,
            SettingsAction::SetBoldIsBright(enabled) => config.font.bold_is_bright = enabled,
            SettingsAction::SetTheme(name) => {
                applied_message = Some(format!("Theme: {name}"));
                config.theme.name = name;
                config.theme.path = None;
            }
            SettingsAction::SetPadding(padding) => config.window.padding = padding,
            SettingsAction::SetOpacity(opacity) => config.window.opacity = opacity,
            SettingsAction::SetBackgroundImageOpacity(opacity) => {
                config.window.background_image.opacity = opacity;
            }
            SettingsAction::SetBackgroundImageFit(fit) => {
                config.window.background_image.fit = fit;
            }
            SettingsAction::SetScrollback(lines) => config.scrollback.lines = lines,
            SettingsAction::SetScrollbackMemory(memory_mib) => {
                config.scrollback.max_memory_mib = memory_mib;
            }
            SettingsAction::SetScrollSpeed(speed) => config.scrollback.scroll_speed = speed,
            SettingsAction::SetDecorations(enabled) => config.window.decorations = enabled,
            SettingsAction::SetWindowLevel(level) => config.window.level = level,
            SettingsAction::SetTabBarPosition(position) => {
                config.window.tab_bar_position = position;
            }
            SettingsAction::SetCursorStyle(style_name) => {
                use crate::config::CursorStyle;
                config.cursor.style = match style_name.as_str() {
                    "Underline" => CursorStyle::Underline,
                    "Beam" => CursorStyle::Beam,
                    _ => CursorStyle::Block,
                };
            }
            SettingsAction::SetCursorBlink(enabled) => config.cursor.blink = enabled,
            SettingsAction::SetCursorBlinkRate(rate) => config.cursor.blink_rate = rate,
            SettingsAction::SetVisualBell(enabled) => config.bell.visual = enabled,
            SettingsAction::SetAudioBell(enabled) => config.bell.audio = enabled,
            SettingsAction::SetLineHeight(height) => config.font.line_height = height,
            SettingsAction::SetBellDuration(ms) => config.bell.visual_duration_ms = ms,
            SettingsAction::SetSplashMode(mode) => config.splash.mode = mode,
            SettingsAction::SetReduceMotion(preference) => {
                config.accessibility.reduce_motion = preference;
            }
            SettingsAction::SetHighContrast(preference) => {
                config.accessibility.high_contrast = preference;
            }
            SettingsAction::SaveConfig => {
                match self.save_config() {
                    Ok(path) => self.toasts.success(format!("Configuration saved: {path}")),
                    Err(error) => self
                        .toasts
                        .error(format!("Configuration save failed: {error}")),
                };
                return;
            }
            SettingsAction::ResetConfig => {
                if let Err(error) = self.apply_config(Config::default()) {
                    self.toasts.error(format!("Reset failed: {error}"));
                } else {
                    match self.save_config() {
                        Ok(path) => self.toasts.success(format!("Configuration reset: {path}")),
                        Err(error) => self
                            .toasts
                            .error(format!("Reset could not be saved: {error}")),
                    };
                }
                return;
            }
            SettingsAction::SetSessionAutoSave(enabled) => config.session.auto_save = enabled,
            SettingsAction::SetSessionAutoRestore(enabled) => {
                config.session.auto_restore = enabled;
            }
            SettingsAction::SetSessionSaveScrollback(enabled) => {
                config.session.save_scrollback = enabled;
            }
            SettingsAction::SetHarvestEnabled(enabled) => config.harvest.enabled = enabled,
            SettingsAction::SetHarvestIndicators(enabled) => {
                config.harvest.show_indicators = enabled;
            }
            SettingsAction::SetHarvestMaxHistory(limit) => config.harvest.max_history = limit,
            SettingsAction::SetAgentEnabled { agent_id, enabled } => {
                // Built-in agents are on by default: record only an opt-out.
                let entry = config.agents.entry(agent_id.clone()).or_default();
                entry.enabled = (!enabled).then_some(false);
                if *entry == crate::agents::AgentConfig::default() {
                    config.agents.remove(&agent_id);
                }
            }
            SettingsAction::SetAgentNotifications(notify) => config.agent_status.notify = notify,
            SettingsAction::SetAllowBypassPermissions(allowed) => {
                config.agent_view.allow_bypass_permissions = allowed;
            }
            SettingsAction::SetOpenAgentsIn(place) => config.agent_view.open_agents_in = place,
            SettingsAction::SetSupervisorAgent(agent) => config.supervisor.agent = agent,
            SettingsAction::SetSupervisorMode(mode) => {
                // Also applies to a supervisor that is already running.
                self.set_supervisor_mode(mode);
                return;
            }
            SettingsAction::SetUpAgentStatus => {
                // Up to date: offer removal instead of a no-op.
                let install =
                    self.agent_hook_summary() != super::agent_hooks::HookSetupSummary::SetUp;
                self.open_agent_hook_setup(install);
                return;
            }
            SettingsAction::ManageRuntimePlugins => {
                self.open_runtime_plugin_manager();
                return;
            }
            SettingsAction::OpenKeyboardShortcuts => {
                self.settings_panel.close();
                self.shortcuts_panel.open();
                self.request_redraw(RedrawReason::ConfigReload);
                return;
            }
            SettingsAction::None => return,
        }

        match self.apply_config(config) {
            Ok(()) => {
                if let Some(message) = applied_message {
                    self.toasts.info(message);
                }
            }
            Err(error) => self
                .toasts
                .error(format!("Setting could not be applied: {error}")),
        }
    }

    /// Apply one custom-command panel action through a single path shared by
    /// pointer, keyboard, and accessibility input.
    pub(super) fn apply_commands_panel_action(
        &mut self,
        action: crate::ui::commands::panel::CommandsPanelAction,
    ) {
        use crate::ui::commands::panel::CommandsPanelAction;

        if !matches!(action, CommandsPanelAction::None) {
            self.clear_commands_accessibility_focus();
            // Focus, dismissal, and form replacement retire composition at
            // the old caret for pointer and accessibility actions as well.
            self.ime.clear_preedit();
        }
        match action {
            CommandsPanelAction::Run(index) => {
                if let Some(command) = self
                    .config
                    .commands
                    .get(index)
                    .map(|command| command.command.clone())
                {
                    self.commands_panel.close();
                    self.run_custom_command(&command);
                }
            }
            CommandsPanelAction::Delete(index) => {
                if index >= self.config.commands.len() {
                    return;
                }
                let name = self.config.commands.remove(index).name;
                self.retire_commands_accessibility_rows();
                // Every input route must agree on the safe post-delete target.
                // A duplicate can otherwise inherit the deleted row's identity.
                // An open form keeps its draft and caret instead of browse focus.
                let layout = self.commands_panel_layout();
                self.commands_panel.focus_browse_control(
                    crate::ui::commands::navigation::CommandControl::New,
                    &self.config.commands,
                    layout,
                );
                log::info!("Deleted custom command '{}'", name);
                match self.save_config() {
                    Ok(message) => {
                        log::info!("{}", message);
                        self.toasts.success(format!("Deleted command '{}'", name));
                    }
                    Err(error) => {
                        log::error!("Deleted '{}' but save failed: {}", name, error);
                        self.toasts.error(format!("Save failed: {}", error));
                    }
                }
            }
            CommandsPanelAction::StartAdd => self.commands_panel.start_add(),
            CommandsPanelAction::Submit => {
                if let Some(command) = self.commands_panel.try_submit() {
                    let name = command.name.clone();
                    self.config.commands.push(command);
                    match self.save_config() {
                        Ok(message) => {
                            log::info!("{}", message);
                            self.toasts.success(format!("Saved command '{}'", name));
                        }
                        Err(error) => {
                            log::error!("Failed to save custom command: {}", error);
                            self.toasts.error(format!("Save failed: {}", error));
                        }
                    }
                } else {
                    self.toasts.warning("Both name and command are required");
                }
            }
            CommandsPanelAction::ClickName => self.commands_panel.focus_name(),
            CommandsPanelAction::ClickCommand => self.commands_panel.focus_command(),
            CommandsPanelAction::PlaceCaret { field, cursor } => {
                self.commands_panel.place_form_caret(field, cursor);
            }
            CommandsPanelAction::CancelEdit => self.commands_panel.cancel_edit(),
            CommandsPanelAction::Close => self.commands_panel.close(),
            CommandsPanelAction::None => {}
        }
    }

    /// Apply one workspace/session panel action through the same path for
    /// pointer, keyboard, and accessibility input.
    pub(super) fn apply_session_panel_action(
        &mut self,
        action: crate::session::panel::SessionPanelAction,
    ) {
        use crate::session::panel::SessionPanelAction;

        let workspace_mode = self.session_panel.is_workspace_mode();
        match action {
            SessionPanelAction::Load(id) => {
                if workspace_mode {
                    self.restore_saved_workspace(&id);
                } else {
                    self.restore_saved_session(&id);
                }
                self.session_panel.close();
            }
            SessionPanelAction::Delete(id) => {
                let result = if workspace_mode {
                    crate::workspace::delete_workspace(&id)
                } else {
                    crate::session::delete_session(&id)
                };
                match result {
                    Ok(()) => {
                        let noun = if workspace_mode {
                            "workspace"
                        } else {
                            "session"
                        };
                        log::info!("Deleted {noun} '{}'", id);
                        self.toasts.success(format!("Deleted {noun} '{}'", id));
                        if workspace_mode {
                            if self.current_workspace_id.as_deref() == Some(id.as_str()) {
                                self.current_workspace_id = None;
                            }
                            if let Err(error) = self.app_state.forget_workspace(&id) {
                                log::warn!(
                                    "Could not clear deleted workspace from state: {error:#}"
                                );
                            }
                        }
                    }
                    Err(error) => {
                        log::error!("Failed to delete session '{}': {}", id, error);
                        self.toasts.error(format!("Delete failed: {}", error));
                    }
                }
                self.session_panel.refresh_sessions();
            }
            SessionPanelAction::Rename(id) => {
                self.ime.clear_preedit();
                self.session_panel.begin_rename(&id);
            }
            SessionPanelAction::SubmitRename => {
                self.ime.clear_preedit();
                if let Some((id, name)) = self.session_panel.take_rename() {
                    match crate::workspace::rename_workspace(&id, &name) {
                        Ok(()) => {
                            self.session_panel.refresh_sessions();
                            self.toasts.success("Workspace renamed");
                        }
                        Err(error) => self.toasts.error(format!("Rename failed: {error:#}")),
                    }
                }
            }
            SessionPanelAction::CancelRename => {
                self.ime.clear_preedit();
                self.session_panel.cancel_rename();
            }
            SessionPanelAction::PlaceRenameCaret { cursor } => {
                if self.session_panel.place_rename_caret(cursor) {
                    self.ime.clear_preedit();
                    self.request_redraw(RedrawReason::PointerInput);
                }
            }
            SessionPanelAction::Close => {
                self.ime.clear_preedit();
                self.session_panel.close();
            }
            SessionPanelAction::Select(id) => {
                self.session_panel.select_id(&id);
            }
            SessionPanelAction::ShowSnapshots => {
                self.ime.clear_preedit();
                self.session_panel.open_sessions();
            }
            SessionPanelAction::ShowWorkspaces => {
                self.ime.clear_preedit();
                self.session_panel.open_workspaces();
            }
            SessionPanelAction::None => {}
        }
    }

    /// Apply a shortcuts panel action
    pub(super) fn apply_shortcuts_action(&mut self, action: ShortcutsAction) {
        match action {
            ShortcutsAction::StartRecording(action) => {
                if self
                    .shortcuts_panel
                    .select_action(action, &self.shortcut_registry)
                {
                    self.shortcuts_panel.start_recording(action);
                }
            }
            ShortcutsAction::ClearShortcut(action) => {
                if self
                    .shortcuts_panel
                    .select_action(action, &self.shortcut_registry)
                {
                    self.shortcut_registry.set_binding(action, None);
                    self.command_palette.register_commands(core_commands(
                        self.config.harvest.enabled,
                        &self.shortcut_registry,
                    ));
                }
            }
            ShortcutsAction::SetShortcut { action, chord } => {
                self.shortcut_registry.set_binding(action, chord);
                self.command_palette.register_commands(core_commands(
                    self.config.harvest.enabled,
                    &self.shortcut_registry,
                ));
            }
            ShortcutsAction::None => {}
        }
    }
}

/// Set, append, or remove one position in the ordered font fallback chain;
/// removing shifts later families forward. A family already in the chain, or
/// the primary family, is refused.
fn set_font_fallback(
    font: &mut crate::config::FontConfig,
    index: usize,
    family: Option<String>,
) -> std::result::Result<(), String> {
    if index > font.fallback.len() || index >= crate::config::MAX_FONT_FALLBACKS {
        return Err("Font fallback position is out of range".into());
    }
    if let Some(family) = family.as_ref()
        && (family.eq_ignore_ascii_case(&font.family)
            || font.fallback.iter().enumerate().any(|(other, configured)| {
                other != index && configured.eq_ignore_ascii_case(family)
            }))
    {
        return Err(format!("Font fallback is already configured: {family}"));
    }
    match family {
        Some(family) if index == font.fallback.len() => font.fallback.push(family),
        Some(family) => font.fallback[index] = family,
        None if index < font.fallback.len() => {
            font.fallback.remove(index);
        }
        None => {}
    }
    Ok(())
}
