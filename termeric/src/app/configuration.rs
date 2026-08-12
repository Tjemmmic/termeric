//! Live configuration, settings, theme, and shortcut application.

use super::*;

impl App {
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
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let Some(window) = &self.window else { return };

        self.config.font.size = size;
        renderer.set_font_size(size);

        // Recalculate grid size after font change
        let window_size = window.inner_size();
        let (new_cols, new_rows) =
            renderer.calculate_grid_size(window_size.width, window_size.height);

        if new_cols != self.cols || new_rows != self.rows {
            self.cols = new_cols;
            self.rows = new_rows;

            // Resize all tabs
            if let Some(tabs) = &mut self.tabs {
                tabs.resize(self.cols, self.rows);
            }

            log::info!(
                "Font size set to {}, grid resized to {}x{}",
                size,
                self.cols,
                self.rows
            );
        }

        // Always resize pane managers when font changes — cell dimensions
        // changed even if cols/rows stayed the same.
        let cell_width = renderer.cell_width();
        let cell_height = renderer.cell_height();
        let content_width = self.cols as f32 * cell_width;
        let content_height = self.rows as f32 * cell_height;
        if let Some(tabs) = &mut self.tabs {
            for tab in tabs.all_tabs_mut() {
                if let Some(ref mut pm) = tab.pane_manager {
                    pm.resize(content_width, content_height, cell_width, cell_height);
                }
            }
        }

        // Request redraw
        window.request_redraw();
    }

    /// Set font family to a specific value
    pub(super) fn set_font_family(&mut self, family: String) {
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let Some(window) = &self.window else { return };

        self.config.font.family = family.clone();
        renderer.set_font_families(family, &self.config.font.fallback);
        // Sync ligature setting and log resolved font info
        renderer.set_ligatures(self.config.font.ligatures);
        log::debug!(
            "Font set: family='{}', glyph_size={}, ligatures={}",
            renderer.font_family(),
            renderer.glyph_font_size(),
            self.config.font.ligatures,
        );

        let window_size = window.inner_size();
        let (new_cols, new_rows) =
            renderer.calculate_grid_size(window_size.width, window_size.height);

        if new_cols != self.cols || new_rows != self.rows {
            self.cols = new_cols;
            self.rows = new_rows;

            if let Some(tabs) = &mut self.tabs {
                tabs.resize(self.cols, self.rows);
            }

            log::info!(
                "Font family updated, grid resized to {}x{}",
                self.cols,
                self.rows
            );
        }

        // Always resize pane managers — cell dimensions changed
        let cell_width = renderer.cell_width();
        let cell_height = renderer.cell_height();
        let content_width = self.cols as f32 * cell_width;
        let content_height = self.rows as f32 * cell_height;
        if let Some(tabs) = &mut self.tabs {
            for tab in tabs.all_tabs_mut() {
                if let Some(ref mut pm) = tab.pane_manager {
                    pm.resize(content_width, content_height, cell_width, cell_height);
                }
            }
        }

        window.request_redraw();
    }

    /// Set line-height multiplier and propagate the resulting cell geometry.
    pub(super) fn set_line_height(&mut self, multiplier: f32) {
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let Some(window) = &self.window else { return };

        self.config.font.line_height = multiplier;
        renderer.set_line_height(multiplier);
        let window_size = window.inner_size();
        let (new_cols, new_rows) =
            renderer.calculate_grid_size(window_size.width, window_size.height);
        if new_cols != self.cols || new_rows != self.rows {
            self.cols = new_cols;
            self.rows = new_rows;
            if let Some(tabs) = &mut self.tabs {
                tabs.resize(self.cols, self.rows);
            }
        }

        let cell_width = renderer.cell_width();
        let cell_height = renderer.cell_height();
        if let Some(tabs) = &mut self.tabs {
            for tab in tabs.all_tabs_mut() {
                if let Some(ref mut panes) = tab.pane_manager {
                    panes.resize(
                        self.cols as f32 * cell_width,
                        self.rows as f32 * cell_height,
                        cell_width,
                        cell_height,
                    );
                }
            }
        }
        window.request_redraw();
    }

    /// Set padding around terminal content
    pub(super) fn set_padding(&mut self, padding: u32) {
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let Some(window) = &self.window else { return };

        self.config.window.padding = padding;
        renderer.set_padding(padding as f32);

        let window_size = window.inner_size();
        let (new_cols, new_rows) =
            renderer.calculate_grid_size(window_size.width, window_size.height);

        if new_cols != self.cols || new_rows != self.rows {
            self.cols = new_cols;
            self.rows = new_rows;

            if let Some(tabs) = &mut self.tabs {
                tabs.resize(self.cols, self.rows);
            }

            log::info!(
                "Padding updated, grid resized to {}x{}",
                self.cols,
                self.rows
            );
        }

        // Resize pane managers — content area changed
        let cell_width = renderer.cell_width();
        let cell_height = renderer.cell_height();
        let content_width = self.cols as f32 * cell_width;
        let content_height = self.rows as f32 * cell_height;
        if let Some(tabs) = &mut self.tabs {
            for tab in tabs.all_tabs_mut() {
                if let Some(ref mut pm) = tab.pane_manager {
                    pm.resize(content_width, content_height, cell_width, cell_height);
                }
            }
        }

        window.request_redraw();
    }

    /// Poll the config file's mtime; if it changed since our last load,
    /// reload + re-apply + toast. Called once per frame but only hits the
    /// filesystem at most every 750ms.
    pub(super) fn poll_config_for_hot_reload(&mut self) {
        const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(750);
        if self.last_config_check.elapsed() < POLL_INTERVAL {
            return;
        }
        self.last_config_check = Instant::now();

        let path = crate::config::Config::config_path();
        let current_mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();

        if current_mtime != self.last_config_mtime {
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

        let Some(theme_path) = self.active_theme_path.clone() else {
            return;
        };
        let theme_mtime = std::fs::metadata(&theme_path)
            .and_then(|metadata| metadata.modified())
            .ok();
        if theme_mtime == self.last_theme_mtime {
            return;
        }
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
    }

    /// Validate and atomically select a new configuration before applying it
    /// to all live subsystems. Invalid reloads leave the previous config live.
    pub(super) fn apply_config(&mut self, config: Config) -> std::result::Result<(), String> {
        config.validate().map_err(|error| format!("{error:#}"))?;
        Theme::resolve(&config.theme.name, config.theme.path.as_deref())
            .map_err(|error| format!("theme rejected: {error:#}"))?;
        self.config = config;
        self.apply_config_side_effects();
        Ok(())
    }

    /// Re-apply every runtime-owned piece of `self.config`. This is the single
    /// application path for startup, settings, reset, manual reload, and hot
    /// reload. Existing PTYs keep their process; shell changes affect new tabs.
    pub(super) fn apply_config_side_effects(&mut self) {
        self.default_font_size = self.config.font.size;
        self.set_font_family(self.config.font.family.clone());
        self.set_font_size(self.config.font.size);
        self.set_line_height(self.config.font.line_height);
        self.set_padding(self.config.window.padding);
        if let Some(renderer) = &mut self.renderer {
            renderer.set_ligatures(self.config.font.ligatures);
            renderer.set_opacity(self.config.window.opacity);
        }
        if let Some(window) = &self.window {
            window.set_decorations(self.config.window.decorations);
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
            tabs.set_scrollback_limit(self.config.scrollback.lines);
            tabs.set_shell_config(self.config.shell.clone());
        }
        self.shortcut_registry = ShortcutRegistry::new();
        self.shortcut_registry
            .apply_overrides(&self.config.shortcuts);
        self.sync_plugins_from_config();
        self.sync_harvest_from_config();
        self.reload_dynamic_keybindings();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    pub(super) fn sync_plugins_from_config(&mut self) {
        let ctx = self.build_plugin_context();
        self.plugin_manager.deregister("claude-code");
        self.plugin_manager.deregister("codex");
        if self.config.plugins.claude_code.enabled {
            let plugin = ClaudeCodePlugin::new(self.config.plugins.claude_code.clone());
            if let Err(error) = self.plugin_manager.register(Box::new(plugin), &ctx) {
                log::error!("Failed to register Claude Code plugin: {error}");
                self.toasts
                    .error(format!("Claude Code plugin failed: {error}"));
            }
        }
        if self.config.plugins.codex.enabled {
            let plugin = CodexPlugin::new(self.config.plugins.codex.clone());
            if let Err(error) = self.plugin_manager.register(Box::new(plugin), &ctx) {
                log::error!("Failed to register Codex plugin: {error}");
                self.toasts.error(format!("Codex plugin failed: {error}"));
            }
        }
        self.refresh_plugin_commands();
        log::info!("Loaded {} plugin(s)", self.plugin_manager.count());
    }

    pub(super) fn sync_harvest_from_config(&mut self) {
        self.command_palette
            .register_commands(core_commands(self.config.harvest.enabled));
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

    pub(super) fn reload_dynamic_keybindings(&mut self) {
        let mut bindings = self.config.keybindings.clone();
        for plugin_binding in self.plugin_manager.all_keybindings() {
            bindings.push(crate::input::keybindings::KeyBinding {
                key: plugin_binding.key,
                modifiers: crate::input::keybindings::KeyModifiers {
                    ctrl: plugin_binding.ctrl,
                    alt: plugin_binding.alt,
                    shift: plugin_binding.shift,
                    super_key: false,
                },
                action: KeyAction::PluginCommand {
                    command_id: plugin_binding.command_id,
                },
            });
        }
        self.keybinding_manager.load(bindings);
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
        if let Some(renderer) = &mut self.renderer {
            renderer.set_theme(&self.theme);
            // Clear glyph cache so text is re-rasterized with new colors
            renderer.clear_glyph_cache();
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Apply a settings panel action to the app config
    pub(super) fn apply_settings_action(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::SetFontSize(size) => {
                self.set_font_size(size);
            }
            SettingsAction::SetFontFamily(family) => {
                self.set_font_family(family);
            }
            SettingsAction::SetTheme(name) => match Theme::resolve(&name, None) {
                Ok((theme, source)) => {
                    let display = name.clone();
                    self.config.theme.name = name;
                    self.config.theme.path = None;
                    self.apply_theme(theme, source);
                    self.toasts.info(format!("Theme: {}", display));
                }
                Err(err) => {
                    self.toasts
                        .error(format!("Theme '{}' failed: {}", name, err));
                }
            },
            SettingsAction::SetPadding(padding) => {
                self.set_padding(padding);
            }
            SettingsAction::SetOpacity(opacity) => {
                self.config.window.opacity = opacity;
                if let Some(renderer) = &mut self.renderer {
                    renderer.set_opacity(opacity);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            SettingsAction::SetScrollback(lines) => {
                self.config.scrollback.lines = lines;
                if let Some(tabs) = &mut self.tabs {
                    tabs.set_scrollback_limit(lines);
                }
            }
            SettingsAction::SetScrollSpeed(speed) => {
                self.config.scrollback.scroll_speed = speed;
            }
            SettingsAction::SetDecorations(enabled) => {
                self.config.window.decorations = enabled;
                if let Some(window) = &self.window {
                    window.set_decorations(enabled);
                }
            }
            SettingsAction::SetCursorStyle(style_name) => {
                use crate::config::CursorStyle;
                let style = match style_name.as_str() {
                    "Underline" => CursorStyle::Underline,
                    "Beam" => CursorStyle::Beam,
                    _ => CursorStyle::Block,
                };
                self.config.cursor.style = style;
            }
            SettingsAction::SetCursorBlink(enabled) => {
                self.config.cursor.blink = enabled;
            }
            SettingsAction::SetCursorBlinkRate(rate) => {
                self.config.cursor.blink_rate = rate;
            }
            SettingsAction::SetVisualBell(enabled) => {
                self.config.bell.visual = enabled;
                self.bell.set_visual(enabled);
            }
            SettingsAction::SetAudioBell(enabled) => {
                self.config.bell.audio = enabled;
                self.bell.set_audio(enabled);
            }
            SettingsAction::SetLineHeight(height) => {
                self.config.font.line_height = height;
                // Trigger a re-render to apply new line height
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            SettingsAction::SetBellDuration(ms) => {
                self.config.bell.visual_duration_ms = ms;
                self.bell.set_config(BellConfig {
                    visual: self.config.bell.visual,
                    audio: self.config.bell.audio,
                    visual_duration_ms: ms,
                    ..Default::default()
                });
            }
            SettingsAction::SetSplashMode(mode) => {
                self.config.splash.mode = mode;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
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
            SettingsAction::SetSessionAutoSave(enabled) => {
                self.config.session.auto_save = enabled;
            }
            SettingsAction::SetSessionAutoRestore(enabled) => {
                self.config.session.auto_restore = enabled;
            }
            SettingsAction::SetSessionSaveScrollback(enabled) => {
                self.config.session.save_scrollback = enabled;
            }
            SettingsAction::SetHarvestEnabled(enabled) => {
                self.config.harvest.enabled = enabled;
            }
            SettingsAction::SetHarvestIndicators(enabled) => {
                self.config.harvest.show_indicators = enabled;
            }
            SettingsAction::SetHarvestMaxHistory(limit) => {
                self.config.harvest.max_history = limit;
            }
            SettingsAction::SetPluginEnabled { plugin_id, enabled } => match plugin_id.as_str() {
                "claude-code" => {
                    self.config.plugins.claude_code.enabled = enabled;
                }
                "codex" => {
                    self.config.plugins.codex.enabled = enabled;
                }
                _ => log::warn!("Unknown plugin: {}", plugin_id),
            },
            SettingsAction::SetPluginModel { plugin_id, model } => {
                let model_val = if model == "(default)" {
                    None
                } else {
                    Some(model)
                };
                match plugin_id.as_str() {
                    "claude-code" => self.config.plugins.claude_code.model = model_val,
                    "codex" => self.config.plugins.codex.model = model_val,
                    _ => {}
                }
            }
            SettingsAction::SetPluginApprovalMode { plugin_id, mode } => match plugin_id.as_str() {
                "claude-code" => {
                    self.config.plugins.claude_code.approval_mode = if mode == "(interactive)" {
                        None
                    } else {
                        Some(mode)
                    };
                }
                "codex" => {
                    self.config.plugins.codex.approval_mode = mode;
                }
                _ => {}
            },
            SettingsAction::None => return,
        }

        let config = self.config.clone();
        if let Err(error) = self.apply_config(config) {
            self.toasts
                .error(format!("Setting could not be applied: {error}"));
        }
    }

    /// Apply a shortcuts panel action
    pub(super) fn apply_shortcuts_action(&mut self, action: ShortcutsAction) {
        match action {
            ShortcutsAction::SetShortcut { action, chord } => {
                self.shortcut_registry.set_binding(action, chord);
            }
            ShortcutsAction::None => {}
        }
    }
}
