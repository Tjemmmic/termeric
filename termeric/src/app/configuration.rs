//! Live configuration, settings, theme, and shortcut application.

use super::*;
use crate::input::keybindings::{BindableAction, KeyChord};
use crate::ui::dialog::{ButtonRole, DialogBlock, DialogSpec};

const CONFIG_RESET_CHOICE: &str = "config-reset";
const DISCARD_SETTINGS_CHOICE: &str = "discard-settings";
const SHORTCUT_REASSIGN_CHOICE: &str = "shortcut-reassign";

impl App {
    /// Ask before Reset Configuration replaces the file. Focus starts on
    /// Cancel, so a stray Enter cannot reset.
    pub(super) fn confirm_config_reset(&mut self) {
        let path = match self.config_file() {
            Ok(path) => path.display().to_string(),
            Err(error) => {
                self.toasts.error(format!("Reset failed: {error:#}"));
                return;
            }
        };
        let request_id = self.dialog.show(DialogSpec::confirm(
            "Reset all settings?",
            vec![
                DialogBlock::text(
                    "Every setting goes back to its default. The current file is kept beside it as a .bak copy.",
                ),
                DialogBlock::value(path),
            ],
            CONFIG_RESET_CHOICE,
            "Reset",
            ButtonRole::Destructive,
        ));
        self.dialog_flow = Some(DialogFlow::ConfigReset { request_id });
    }

    /// Reload the configuration file, first asking when that would throw
    /// away changes made in Settings and not saved.
    pub(super) fn reload_config_or_confirm_discard(&mut self) {
        if self.config_has_unsaved_changes() {
            self.confirm_discard_settings();
        } else {
            self.reload_config_from_disk();
        }
    }

    /// Ask before unsaved changes go; nothing to ask while there are none.
    pub(super) fn confirm_discard_settings(&mut self) {
        if !self.config_has_unsaved_changes() {
            return;
        }
        let request_id = self.dialog.show(DialogSpec::confirm(
            "Discard unsaved changes?",
            vec![DialogBlock::text(
                "Settings go back to what the configuration file says, and the changes made since it was last saved are lost.",
            )],
            DISCARD_SETTINGS_CHOICE,
            "Discard",
            ButtonRole::Destructive,
        ));
        self.dialog_flow = Some(DialogFlow::DiscardSettings { request_id });
    }

    /// Answer the discard confirmation. Returns whether the answer was its.
    pub(super) fn handle_discard_settings_choice(
        &mut self,
        request_id: u64,
        choice_id: &str,
    ) -> bool {
        let pending = matches!(
            self.dialog_flow,
            Some(DialogFlow::DiscardSettings { request_id: pending }) if pending == request_id
        );
        if !pending || choice_id != DISCARD_SETTINGS_CHOICE {
            return false;
        }
        self.dialog.dismiss(request_id);
        self.dialog_flow = None;
        self.reload_config_from_disk();
        true
    }

    /// Answer the reset confirmation. Returns whether the answer was its.
    pub(super) fn handle_config_reset_choice(&mut self, request_id: u64, choice_id: &str) -> bool {
        let pending = matches!(
            self.dialog_flow,
            Some(DialogFlow::ConfigReset { request_id: pending }) if pending == request_id
        );
        if !pending || choice_id != CONFIG_RESET_CHOICE {
            return false;
        }
        self.dialog.dismiss(request_id);
        self.dialog_flow = None;
        self.reset_config_to_defaults();
        true
    }

    pub(super) const CONFIG_POLL_INTERVAL: std::time::Duration =
        std::time::Duration::from_millis(750);

    /// The font size this window draws with: its zoom, or the configured
    /// `font.size`.
    pub(super) fn effective_font_size(&self) -> f32 {
        self.font_zoom.unwrap_or(self.config.font.size)
    }

    /// Zoom this window's text by `delta` points (Ctrl+= and Ctrl+-).
    pub(super) fn zoom_font(&mut self, delta: f32) {
        let size = (self.effective_font_size() + delta).clamp(
            *crate::config::FONT_SIZE_RANGE.start(),
            *crate::config::FONT_SIZE_RANGE.end(),
        );
        self.set_font_zoom(Some(size));
    }

    /// Return this window's text to the configured size (Ctrl+0).
    pub(super) fn reset_font_zoom(&mut self) {
        self.set_font_zoom(None);
    }

    /// Zoom is view state: it changes how this window draws, never the
    /// configuration, so it is not saved and leaves Settings clean.
    fn set_font_zoom(&mut self, zoom: Option<f32>) {
        let zoom = zoom.filter(|size| *size != self.config.font.size);
        if zoom == self.font_zoom {
            return;
        }
        self.store_font_zoom(zoom);
        let size = self.effective_font_size();
        let Some(renderer) = &mut self.renderer else {
            return;
        };

        renderer.set_font_size(size);
        self.sync_terminal_geometry();
        log::info!(
            "Font size set to {}, grid resized to {}x{}",
            size,
            self.cols,
            self.rows
        );

        self.request_redraw(RedrawReason::ConfigReload);
    }

    /// The one place the zoom changes, so Settings always shows it.
    fn store_font_zoom(&mut self, zoom: Option<f32>) {
        self.font_zoom = zoom;
        self.settings_panel.set_window_font_size(zoom);
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
        self.last_background_image_stamp = policy
            .path
            .as_deref()
            .and_then(crate::renderer::resolve_background_image_path)
            .and_then(|path| crate::renderer::BackgroundImageStamp::for_path(&path));
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
        if self.window.is_none() {
            return;
        }
        let window_key = self.window_key();
        let Some(proxy) = EVENT_LOOP_PROXY.get().cloned() else {
            return;
        };
        let Some(resolved_path) = crate::renderer::resolve_background_image_path(&configured_path)
        else {
            self.toasts.warning(format!(
                "Background image {} is relative to the configuration directory: {}",
                configured_path.display(),
                crate::paths::NO_HOME_DIRECTORY
            ));
            return;
        };
        let generation = self.background_image_generation;
        self.background_image_loading = true;
        let spawn = std::thread::Builder::new()
            .name("termeric-background-image".into())
            .spawn(move || {
                let result = crate::renderer::load_background_image(&resolved_path);
                let _ = proxy.send_event(AppEvent::WindowResult {
                    window: window_key,
                    result: WindowResult::BackgroundImageLoaded {
                        generation,
                        path: resolved_path,
                        result,
                    },
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
        let stamp = crate::renderer::resolve_background_image_path(path)
            .and_then(|resolved| crate::renderer::BackgroundImageStamp::for_path(&resolved));
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

        let current_mtime = self.config_path.as_deref().and_then(super::file_mtime);

        if current_mtime != self.last_config_mtime {
            changed = true;
            self.last_config_mtime = current_mtime;

            // Only reload if the file exists (avoid racing with editors that
            // briefly delete + rewrite).
            if let Some(path) = self.config_path.clone().filter(|path| path.exists()) {
                match crate::config::Config::load_from(&path) {
                    Ok(config) => {
                        if let Err(error) = self.apply_config(config) {
                            self.report_config_problem(
                                crate::ui::toast::ToastSeverity::Warning,
                                format!(
                                    "Config reload rejected; keeping the current settings: {error}"
                                ),
                            );
                        } else {
                            self.mark_config_saved();
                            self.clear_config_problem();
                            log::info!("Config hot-reloaded");
                            self.toasts.info("Config reloaded");
                        }
                    }
                    Err(error) => self.report_config_problem(
                        crate::ui::toast::ToastSeverity::Warning,
                        format!("Config reload failed; keeping the current settings: {error:#}"),
                    ),
                }
            }
        }

        changed |= self.poll_background_image();
        changed |= self.poll_configured_theme();
        changed
    }

    /// Load the configured theme again when its file appeared, changed, or
    /// went away since it was last loaded.
    fn poll_configured_theme(&mut self) -> bool {
        let Some(file) = self.theme_file.clone() else {
            return false;
        };
        if modified_time(&file) == self.theme_file_modified {
            return false;
        }
        if self.load_configured_theme() {
            self.toasts
                .info(format!("Theme reloaded: {}", file.display()));
        }
        true
    }

    /// Show the configured theme and watch the file it is read from. A theme
    /// that does not load leaves the one on screen, with a notice that stays
    /// until a configured theme loads. Returns whether it loaded.
    pub(super) fn load_configured_theme(&mut self) -> bool {
        // Themes are reloading, so a custom theme file may have appeared or
        // gone away.
        self.sync_settings_theme_names();
        let name = self.config.theme.name.clone();
        let path = self.config.theme.path.clone();
        // Noted before reading, so a change made during the read is seen by
        // the next poll.
        self.theme_file = Theme::configured_file(&name, path.as_deref());
        self.theme_file_modified = self.theme_file.as_deref().and_then(modified_time);
        match Theme::resolve(&name, path.as_deref()) {
            Ok(theme) => {
                self.show_configured_theme(theme);
                self.theme_problem.clear(&mut self.toasts);
                true
            }
            Err(error) => {
                let message = format!(
                    "Theme could not be loaded; showing {}: {error:#}",
                    self.theme.meta.name
                );
                log::warn!("{message}");
                self.theme_problem.show(
                    &mut self.toasts,
                    crate::ui::toast::ToastSeverity::Warning,
                    message,
                );
                false
            }
        }
    }

    /// Validate and atomically select a new configuration before applying it
    /// to all live subsystems. Invalid reloads leave the previous config live.
    /// A theme that does not load is not invalid: the theme on screen stays,
    /// and a notice says why until a configured theme loads. Neither is one
    /// that turns agent features off while a conversation they serve runs:
    /// they stay on in this window until it ends.
    pub(super) fn apply_config(&mut self, config: Config) -> std::result::Result<(), String> {
        config.validate().map_err(|error| format!("{error:#}"))?;
        let agent_features_were_on = self.agent_features_enabled();
        let hold_agent_features = self.holds_agent_features_against(&config);
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
        // The switch registers or removes every agent's launch integration.
        let agents_changed = config.agents != self.config.agents
            || agent_features_were_on != (config.agent_features_enabled() || hold_agent_features);
        // A new configured size is what the person now wants to see, so it
        // ends a zoom; other changes keep it.
        if config.font.size != self.config.font.size {
            self.store_font_zoom(None);
        }
        self.config = config;
        self.agent_features_held = hold_agent_features;
        self.apply_config_side_effects(agents_changed);
        let policy = self.terminal_policy();
        if keyboard_type_changed && let Some(tabs) = &mut self.tabs {
            tabs.set_terminal_policy(policy, true);
        }
        Ok(())
    }

    /// Re-apply every runtime-owned piece of `self.config`. This is the single
    /// application path for startup, settings, reset, manual reload, and hot
    /// reload. Existing PTYs keep their process; shell changes affect new tabs.
    pub(super) fn apply_config_side_effects(&mut self, agents_changed: bool) {
        let font_size = self.effective_font_size();
        let tab_bar_position_changed = self.renderer.as_ref().is_some_and(|renderer| {
            renderer.tab_bar_position() != self.config.window.tab_bar_position
        });
        if let Some(renderer) = &mut self.renderer {
            let font = &self.config.font;
            renderer.set_font_families(font.family.clone(), &font.fallback, &font.range_overrides);
            renderer.set_font_size(font_size);
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
        self.sync_terminal_geometry();
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
        self.cursor_blink_half_period = cursor_blink_half_period(&self.config.cursor);
        self.reset_cursor_blink();
        self.bell.set_config(self.config.bell.clone());
        self.load_configured_theme();
        let policy = self.terminal_policy();
        if let Some(tabs) = &mut self.tabs {
            // Reapplying unrelated settings must not undo a keyboard type
            // selected by the foreground application through DECSET 1060/1061.
            tabs.set_terminal_policy(policy, false);
            tabs.set_shell_config(self.config.shell.clone());
        }
        self.shortcut_registry = ShortcutRegistry::new();
        self.shortcut_registry
            .set_agent_features(self.agent_features_enabled());
        self.shortcut_registry
            .apply_overrides(&self.config.shortcuts);
        self.refresh_displaced_shortcuts_notice();
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
            self.reload_plugins_for_agents();
        }
        self.sync_agent_features();
        self.sync_harvest_from_config();
        self.sync_accessibility_preferences();
        self.refresh_launch_profile_commands();
        self.keybinding_manager
            .load(self.config.keybindings.clone());
        self.request_redraw(RedrawReason::ConfigReload);
    }

    /// The host settings this window's terminals run under.
    pub(super) fn terminal_policy(&self) -> crate::terminal::policy::TerminalPolicy {
        crate::terminal::policy::TerminalPolicy::from_config(
            &self.config,
            self.agent_features_enabled(),
        )
    }

    /// Hand Settings the installed and enabled state of each built-in agent.
    /// With agent features off the page shows only their switch, so nothing
    /// is looked up.
    pub(super) fn sync_agent_settings_rows(&mut self) {
        if !self.agent_features_enabled() {
            self.settings_panel
                .set_agents(Vec::new(), String::new(), String::new());
            return;
        }
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

    /// Reload plugins after this window changed them in the plugin manager:
    /// a new host generation stops every handler, and the Application brings
    /// the other windows to it.
    pub(super) fn reload_plugins(&mut self) {
        self.plugin_host.advance_generation();
        self.sync_plugins_from_config();
    }

    /// Reload plugins after this window's agents changed, since their IDs can
    /// shadow plugins. Each window reloads its configuration on its own, so
    /// one change reaches every window; only the first starts a new host
    /// generation, and the rest rebuild their plugins in it.
    fn reload_plugins_for_agents(&mut self) {
        self.plugin_host
            .advance_generation_for_agents(&self.config.agents);
        self.sync_plugins_from_config();
    }

    /// Rebuild this window's plugins from the registry and its agents at the
    /// host's current generation.
    pub(super) fn sync_plugins_from_config(&mut self) {
        self.plugin_runtime_generation = self.plugin_host.generation();
        self.plugin_manager.clear();
        self.agents = crate::agents::AgentCatalog::from_config(&self.config.agents);
        self.sync_agent_settings_rows();
        let features = self.agent_features_enabled();
        for agent in self.agents.enabled().filter(|_| features) {
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
        self.settings_panel
            .set_runtime_plugins(crate::ui::settings::InstalledPlugins {
                statuses,
                error: registry_error,
            });
        self.refresh_plugin_commands();
        self.refresh_launch_profile_commands();
        log::info!(
            "Loaded {}",
            plural(self.plugin_manager.count(), "plugin", "plugins")
        );
    }

    /// Register the built-in commands the configuration offers, with the
    /// current shortcut hints.
    pub(super) fn refresh_core_commands(&mut self) {
        self.command_palette.replace_core_commands(
            crate::ui::command_palette::palette::core_commands(
                self.config.harvest.enabled,
                self.agent_features_enabled(),
                &self.shortcut_registry,
            ),
        );
    }

    pub(super) fn sync_harvest_from_config(&mut self) {
        self.refresh_core_commands();
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
        self.workspace_panel.set_reduced_motion(reduced_motion);
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
        self.dialog.set_reduced_motion(reduced_motion);
        self.diff_review.set_reduced_motion(reduced_motion);
        self.context_picker.set_reduced_motion(reduced_motion);
        self.guardrail.set_reduced_motion(reduced_motion);
        self.toasts.set_reduced_motion(reduced_motion);
        self.agent_view.state.set_reduced_motion(reduced_motion);
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

    /// Hand Settings every selectable theme. Listing the themes directory
    /// touches the file system, so Settings is told here, when it opens and
    /// when themes reload, rather than reading it each time it builds rows.
    pub(super) fn sync_settings_theme_names(&mut self) {
        self.settings_panel.set_theme_names(Theme::list_all());
    }

    /// Show `theme`, which may be a preview rather than the configured one.
    pub(super) fn apply_theme(&mut self, theme: Theme) {
        self.settings_panel.set_theme_meta(theme.meta.clone());
        self.theme = theme;
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
            SettingsAction::OpenSubPage(sub_page) => {
                self.settings_panel.open_sub_page(sub_page);
                return;
            }
            SettingsAction::FontRange(action) => {
                self.apply_font_range_action(action);
                return;
            }
            SettingsAction::FontFeature(action) => {
                self.apply_font_feature_action(action);
                return;
            }
            SettingsAction::CheckFontCoverage => {
                self.ask_font_coverage(String::new(), None);
                return;
            }
            SettingsAction::SetFontLigatures(enabled) => config.font.ligatures = enabled,
            SettingsAction::SetBoldIsBright(enabled) => config.font.bold_is_bright = enabled,
            SettingsAction::SetCopyOnSelect(enabled) => config.selection.copy_on_select = enabled,
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
            SettingsAction::SetCursorStyle(style) => config.cursor.style = style,
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
            SettingsAction::DiscardChanges => {
                self.confirm_discard_settings();
                return;
            }
            SettingsAction::SaveConfig => {
                match self.save_config() {
                    Ok(path) => self.toasts.success(format!("Configuration saved: {path}")),
                    Err(error) => self
                        .toasts
                        .error(format!("Configuration save failed: {error:#}")),
                };
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
            SettingsAction::SetAgentFeaturesEnabled(enabled) => {
                config.agent_features.enabled = enabled;
            }
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
            SettingsAction::Plugin(action) => {
                self.apply_plugin_action(action);
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
                    self.set_shortcut(action, None);
                }
            }
            ShortcutsAction::SetShortcut { action, chord } => self.request_shortcut(action, chord),
            ShortcutsAction::None => {}
        }
    }

    /// Bind `action` to `chord`, first asking when another action uses the
    /// chord: nothing changes unless the person agrees to leave that action
    /// without a shortcut.
    fn request_shortcut(&mut self, action: BindableAction, chord: Option<KeyChord>) {
        if let Some(chord) = &chord
            && let Some(holder) = self.shortcut_registry.conflict_for(chord)
            && holder != action
        {
            self.confirm_shortcut_reassign(action, chord.clone(), holder);
            return;
        }
        self.set_shortcut(action, chord);
    }

    /// Ask before `chord` moves from `holder` to `action`. No letter key
    /// answers a dialog, so a key still held from recording cannot.
    fn confirm_shortcut_reassign(
        &mut self,
        action: BindableAction,
        chord: KeyChord,
        holder: BindableAction,
    ) {
        let (label, holder_label) = (action.meta().label, holder.meta().label);
        let request_id = self.dialog.show(DialogSpec::confirm(
            format!("Use {chord} for {label}?"),
            vec![DialogBlock::text(format!(
                "It's the shortcut for {holder_label}, which will be left without one."
            ))],
            SHORTCUT_REASSIGN_CHOICE,
            "Replace",
            ButtonRole::Destructive,
        ));
        self.dialog_flow = Some(DialogFlow::ShortcutReassign {
            request_id,
            action,
            chord,
            holder,
        });
    }

    /// Answer the reassign confirmation. Returns whether the answer was its.
    pub(super) fn handle_shortcut_reassign_choice(
        &mut self,
        request_id: u64,
        choice_id: &str,
    ) -> bool {
        let (action, chord, holder) = match &self.dialog_flow {
            Some(DialogFlow::ShortcutReassign {
                request_id: pending,
                action,
                chord,
                holder,
            }) if *pending == request_id => (*action, chord.clone(), *holder),
            _ => return false,
        };
        if choice_id != SHORTCUT_REASSIGN_CHOICE {
            return false;
        }
        self.dialog.dismiss(request_id);
        self.dialog_flow = None;
        // A reload while the question was open may have moved the chord;
        // the person agreed to take it from `holder` alone.
        if self.shortcut_registry.conflict_for(&chord) == Some(holder) {
            self.set_shortcut(action, Some(chord));
        } else {
            self.request_shortcut(action, Some(chord));
        }
        self.request_redraw(RedrawReason::Action);
        true
    }

    /// Bind `action` to `chord` as the shortcuts editor does, and record the
    /// change in the configured `[[shortcuts]]`, so a save writes the entries
    /// it touched and leaves every other entry as the file has it.
    fn set_shortcut(&mut self, action: BindableAction, chord: Option<KeyChord>) {
        self.shortcut_registry.set_binding(action, chord);
        self.shortcut_registry
            .record_overrides(&mut self.config.shortcuts);
        self.refresh_core_commands();
        self.refresh_displaced_shortcuts_notice();
    }

    /// Say which actions a `[[shortcuts]]` entry left without their default
    /// shortcut. The notice is shown again only when that list changes, so
    /// a dismissed one stays dismissed across reloads of the same file.
    fn refresh_displaced_shortcuts_notice(&mut self) {
        let displaced = ShortcutRegistry::displaced_defaults(
            &self.config.shortcuts,
            self.agent_features_enabled(),
        );
        if displaced.is_empty() {
            self.displaced_shortcuts.clear(&mut self.toasts);
            return;
        }
        let message = displaced
            .iter()
            .map(crate::input::keybindings::DisplacedDefault::message)
            .collect::<Vec<_>>()
            .join(". ");
        self.displaced_shortcuts.show(
            &mut self.toasts,
            crate::ui::toast::ToastSeverity::Info,
            message,
        );
    }
}

/// When the file at `path` was last modified, or `None` when there is none.
fn modified_time(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
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
