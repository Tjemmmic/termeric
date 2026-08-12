//! Application state and event handling

mod configuration;
mod desktop_io;
mod event_loop;
mod mouse;
mod plugin_actions;
mod sessions;
pub mod state;
#[cfg(test)]
mod tests;
mod workspace_actions;

use anyhow::Result;
use std::sync::Arc;
use std::time::Instant;
use winit::{
    event::{ElementState, MouseButton},
    keyboard::ModifiersState,
    window::{Icon, Window},
};

use termeric_core::TerminalEvent;

use self::state::AppState;
use crate::config::theme::Theme;
use crate::config::{Config, SplashMode};
use crate::harvest::HarvestMode;
use crate::input::keybindings::{KeyAction, KeyBindingManager};
use crate::input::{Action, MouseState, ShortcutRegistry, screen_to_grid};
use crate::notifications::{Bell, BellConfig, Notifications};
use crate::panes::{PaneId, PaneManager, SplitDirection};
use crate::plugin::claude_code::ClaudeCodePlugin;
use crate::plugin::codex::CodexPlugin;
use crate::plugin::{PluginContext, PluginManager};
use crate::renderer::Renderer;
use crate::renderer::types::{
    CellarEntryInfo, CellarRenderInfo, HarvestRenderInfo, IndicatorRenderInfo, PaneRenderData,
    SproutMenuRenderInfo,
};
use crate::tabs::TabManager;
use crate::terminal::container::{PaneRenderInfo, SplitContainer, TerminalContainer};
use crate::terminal::guardrails::GuardrailOverlay;
use crate::terminal::search::Search;
use crate::terminal::splash::show_splash;
use crate::terminal::url::{UrlCache, UrlDetector};
use crate::ui::command_palette::palette::core_commands;
use crate::ui::command_palette::{CommandPalette, PaletteResult};
use crate::ui::context_picker::ContextPicker;
use crate::ui::diff_review::DiffReviewOverlay;
use crate::ui::info_overlay::InfoOverlay;
use crate::ui::settings::{SettingsAction, SettingsPanel};
use crate::ui::shortcuts_panel::{ShortcutsAction, ShortcutsPanel};

/// Main application state
///
/// IMPORTANT: Drop order matters! The renderer's wgpu Surface holds a reference
/// to the window, so the renderer MUST be dropped before the window.
/// We implement Drop explicitly to guarantee this ordering.
pub struct App {
    /// Application configuration
    #[allow(dead_code)]
    config: Config,
    /// The window (created on resume)
    window: Option<Arc<Window>>,
    /// The renderer
    renderer: Option<Renderer>,
    /// Tab manager for multiple terminals
    tabs: Option<TabManager>,
    /// Whether the application should exit
    should_exit: bool,
    /// Surface rendering is paused until a real resize safely reconfigures it.
    surface_lost: bool,
    /// Terminal dimensions
    cols: usize,
    rows: usize,
    /// Cursor animation start time (for smooth blink)
    cursor_blink_start: Instant,
    /// Cursor blink period in seconds
    cursor_blink_period: f32,
    /// Animated cursor display position (for smooth movement)
    cursor_display_col: f32,
    cursor_display_row: f32,
    /// Last frame time for delta calculation
    last_frame_time: Instant,
    /// Last time we polled the config file for hot-reload
    last_config_check: Instant,
    /// Last observed mtime of the config file (for change detection)
    last_config_mtime: Option<std::time::SystemTime>,
    /// File backing the active custom/explicit theme, when applicable.
    active_theme_path: Option<std::path::PathBuf>,
    /// Last observed mtime of the active file-backed theme.
    last_theme_mtime: Option<std::time::SystemTime>,
    /// Last known cursor visibility (for snap detection)
    last_cursor_visible: bool,
    /// Last active tab ID (for animation state)
    last_active_tab_id: Option<usize>,
    /// Highest tab ID we've seen (to detect truly new tabs)
    max_seen_tab_id: Option<usize>,
    /// Mouse input state
    mouse_state: MouseState,
    /// Current keyboard modifiers
    modifiers: ModifiersState,
    /// Clipboard
    clipboard: Option<arboard::Clipboard>,
    /// Last cursor screen position (for mouse input events)
    last_cursor_pos: Option<(f64, f64)>,
    /// Whether mouse is currently pressed (for selection)
    mouse_pressed: bool,
    /// Whether we're dragging the scrollbar
    scrollbar_dragging: bool,
    /// Whether we're dragging a pane divider
    divider_dragging: bool,
    /// Default font size (for reset)
    default_font_size: f32,
    /// When copy flash started (for animation), None if not flashing
    copy_flash_start: Option<Instant>,
    /// Whether the window is currently focused
    window_focused: bool,
    /// When focus state last changed (for smooth transition)
    focus_change_time: Instant,
    /// Active UI theme
    theme: Theme,
    /// Settings panel state
    settings_panel: SettingsPanel,
    /// Search state
    search: Search,
    /// Cursor position in search input
    search_cursor_pos: usize,
    /// Notification manager for desktop notifications
    notifications: Notifications,
    /// Bell handler (visual/audio bell)
    bell: Bell,
    /// Unified shortcut registry (all bindable actions)
    shortcut_registry: ShortcutRegistry,
    /// Plugin/custom keybinding manager (SendText, RunCommand, PluginCommand)
    keybinding_manager: KeyBindingManager,
    /// Application runtime state (for first boot tracking, etc.)
    app_state: AppState,
    /// Whether the session dropdown is currently open
    session_dropdown_open: bool,
    /// Cached list of session names for the dropdown
    session_dropdown_items: Vec<(String, String)>,
    /// Mouse-hovered row in the session dropdown
    session_dropdown_hovered: Option<usize>,
    /// Session management panel
    session_panel: crate::session::panel::SessionPanel,
    /// Custom commands panel
    commands_panel: crate::ui::commands::panel::CommandsPanel,
    /// Buffer tracking what the user has typed on the current line (for alias expansion)
    input_line_buffer: String,
    /// Git repository watcher (tracks branch, dirty status per tab)
    git_watcher: crate::git::GitWatcher,
    /// Current git completion candidates
    git_completions: Vec<String>,
    /// Selected index in git completion dropdown (None = no selection)
    git_completion_index: Option<usize>,
    /// Whether git completion dropdown is visible
    git_completions_visible: bool,
    /// Which tab bar element is currently hovered (for icon animations)
    tab_bar_hover: Option<crate::renderer::TabBarClick>,
    /// Smooth hover animation progress (0.0 = not hovered, 1.0 = fully hovered)
    tab_bar_hover_progress: f32,
    /// Plugin manager
    plugin_manager: PluginManager,
    /// Command palette
    command_palette: CommandPalette,
    /// Shortcuts panel (standalone centered modal)
    shortcuts_panel: ShortcutsPanel,
    /// Info overlay (About/Version)
    info_overlay: InfoOverlay,
    /// Transient toast notifications stacked in the bottom-right
    toasts: crate::ui::toast::ToastManager,
    /// Diff review overlay
    diff_review: DiffReviewOverlay,
    /// Context picker modal
    context_picker: ContextPicker,
    /// Guardrail confirmation overlay
    guardrail: GuardrailOverlay,
    /// Pending profile selection: (plugin_id, [profile_name, ...])
    /// When set, the info overlay is showing a profile list and number keys select one.
    pending_profile_selection: Option<(String, Vec<String>)>,
    /// Harvest mode (command visualization)
    harvest: Option<HarvestMode>,
    /// URL detector for auto-detecting URLs in terminal output
    url_detector: UrlDetector,
    /// Cached URL lookup table, reused between frames
    url_cache: UrlCache,
}

impl App {
    /// Create a new application instance
    pub fn new() -> Result<Self> {
        let (config, startup_config_warning) = Config::load_for_startup();
        Self::from_config(config, startup_config_warning, true)
    }

    /// Construct the controller without a window, renderer, PTY, clipboard,
    /// or persisted app-state dependency. This is the integration-test spine
    /// for dispatch and live configuration behavior.
    #[cfg(test)]
    fn new_headless(config: Config) -> Result<Self> {
        config.validate()?;
        Self::from_config(config, None, false)
    }

    fn from_config(
        mut config: Config,
        mut startup_config_warning: Option<String>,
        use_system_state: bool,
    ) -> Result<Self> {
        let default_font_size = config.font.size;
        let cursor_blink_period = (config.cursor.blink_rate as f32 / 1000.0) * 2.0;
        let (theme, active_theme_path) =
            match Theme::resolve(&config.theme.name, config.theme.path.as_deref()) {
                Ok(resolved) => resolved,
                Err(error) => {
                    let warning = format!("Theme could not be loaded; using turmeric: {error:#}");
                    startup_config_warning = Some(match startup_config_warning {
                        Some(existing) => format!("{existing}\n{warning}"),
                        None => warning,
                    });
                    config.theme = crate::config::ThemeConfig::default();
                    (Theme::default(), None)
                }
            };
        let last_theme_mtime = active_theme_path.as_ref().and_then(|path| {
            std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        });

        // Initialize bell from config
        let bell = Bell::new(BellConfig {
            visual: config.bell.visual,
            audio: config.bell.audio,
            visual_duration_ms: config.bell.visual_duration_ms,
            ..Default::default()
        });

        // Initialize clipboard (may fail on some systems)
        let clipboard = use_system_state
            .then(arboard::Clipboard::new)
            .transpose()
            .ok()
            .flatten();

        // Initialize unified shortcut registry with defaults + user overrides
        let mut shortcut_registry = ShortcutRegistry::new();
        shortcut_registry.apply_overrides(&config.shortcuts);

        // Initialize plugin/custom keybinding manager
        let mut keybinding_manager = KeyBindingManager::new();
        keybinding_manager.load(config.keybindings.clone());

        let result: Result<Self> = Ok(Self {
            config,
            window: None,
            renderer: None,
            tabs: None,
            should_exit: false,
            surface_lost: false,
            cols: 80,
            rows: 24,
            cursor_blink_start: Instant::now(),
            cursor_blink_period,
            cursor_display_col: 0.0,
            cursor_display_row: 0.0,
            last_frame_time: Instant::now(),
            last_config_check: Instant::now(),
            last_config_mtime: use_system_state
                .then(|| std::fs::metadata(crate::config::Config::config_path()))
                .and_then(Result::ok)
                .and_then(|metadata| metadata.modified().ok()),
            active_theme_path,
            last_theme_mtime,
            last_cursor_visible: true,
            last_active_tab_id: None,
            max_seen_tab_id: None,
            mouse_state: MouseState::new(),
            modifiers: ModifiersState::empty(),
            clipboard,
            last_cursor_pos: None,
            mouse_pressed: false,
            scrollbar_dragging: false,
            divider_dragging: false,
            default_font_size,
            copy_flash_start: None,
            window_focused: true,
            focus_change_time: Instant::now(),
            theme,
            settings_panel: SettingsPanel::new(),
            search: Search::new(),
            search_cursor_pos: 0,
            notifications: Notifications::new(),
            bell,
            shortcut_registry,
            keybinding_manager,
            app_state: if use_system_state {
                AppState::load()
            } else {
                AppState::default()
            },
            session_dropdown_open: false,
            session_dropdown_items: Vec::new(),
            session_dropdown_hovered: None,
            session_panel: crate::session::panel::SessionPanel::new(),
            commands_panel: crate::ui::commands::panel::CommandsPanel::new(),
            input_line_buffer: String::new(),
            git_watcher: crate::git::GitWatcher::new(),
            git_completions: Vec::new(),
            git_completion_index: None,
            git_completions_visible: false,
            tab_bar_hover: None,
            tab_bar_hover_progress: 0.0,
            plugin_manager: PluginManager::new(),
            command_palette: CommandPalette::new(),
            shortcuts_panel: ShortcutsPanel::new(),
            info_overlay: InfoOverlay::new(),
            toasts: crate::ui::toast::ToastManager::new(),
            diff_review: DiffReviewOverlay::new(),
            context_picker: ContextPicker::new(),
            guardrail: GuardrailOverlay::new(),

            pending_profile_selection: None,
            harvest: None,
            url_detector: UrlDetector::new(),
            url_cache: UrlCache::new(0),
        });

        let mut app = result?;

        // Initialize command palette and apply all runtime config through the
        // same path used by reload, settings changes, and reset.
        app.command_palette.register_commands(core_commands(
            app.config.harvest.enabled,
            &app.shortcut_registry,
        ));
        app.apply_config_side_effects();
        if let Some(warning) = startup_config_warning {
            log::error!("{}", warning);
            app.toasts.error(warning);
        }

        Ok(app)
    }

    /// Load window icon from embedded PNG data
    ///
    /// The icon is embedded directly in the binary at compile time,
    /// so it works on all platforms without requiring external files.
    fn load_icon() -> Option<Icon> {
        // Icon is embedded in the binary at compile time
        let icon_bytes = include_bytes!("../../../assets/icons/termeric.png");

        match image::load_from_memory(icon_bytes) {
            Ok(img) => {
                // Resize to 256x256 for optimal display across platforms
                let img = img.resize_exact(256, 256, image::imageops::FilterType::Lanczos3);
                let rgba = img.to_rgba8();
                let (width, height) = rgba.dimensions();
                let icon_data = rgba.into_raw();

                match Icon::from_rgba(icon_data, width, height) {
                    Ok(icon) => {
                        log::info!("Successfully loaded window icon ({}x{})", width, height);
                        Some(icon)
                    }
                    Err(e) => {
                        log::error!("Failed to create icon from image data: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to decode embedded icon image: {}", e);
                None
            }
        }
    }

    /// Initialize the renderer
    fn init_renderer(&mut self, window: Arc<Window>) -> Result<()> {
        let scale_factor = window.scale_factor() as f32;
        let mut renderer = Renderer::new(
            window.clone(),
            self.config.font.size,
            &self.config.font.family,
            &self.config.font.fallback,
            self.config.font.line_height,
            self.config.window.padding as f32,
            scale_factor,
            &self.theme,
            self.config.font.ligatures,
        )?;

        // Apply initial opacity from config
        renderer.set_opacity(self.config.window.opacity);

        // Discover available monospace fonts and update settings panel
        let fonts = renderer.get_monospace_fonts();
        log::info!("Discovered {} monospace fonts", fonts.len());
        self.settings_panel.set_available_fonts(fonts);

        // Calculate grid size from renderer's cell size
        let size = window.inner_size();
        let (cols, rows) = renderer.calculate_grid_size(size.width, size.height);
        self.cols = cols;
        self.rows = rows;

        self.renderer = Some(renderer);
        self.window = Some(window);

        log::info!("Renderer initialized, grid size: {}x{}", cols, rows);

        Ok(())
    }

    /// Process PTY events for all tabs and panes
    fn process_pty_events(&mut self) {
        // Process events and collect results
        let (events, quick_exits, persistence_notices, user_notices, should_exit) = {
            let Some(tabs) = &mut self.tabs else { return };
            let (_, events) = tabs.process_all_events();

            // Detect plugin tabs that exited too quickly (< 5s) — before cleanup removes them
            let quick_exits = tabs.drain_quick_exit_plugin_info();

            let auto_save = self.config.session.enabled && self.config.session.auto_save;
            let should_exit = tabs.cleanup_closed(auto_save, self.config.session.save_scrollback)
                && tabs.is_empty();
            let persistence_notices = tabs.drain_persistence_notices();
            let user_notices = tabs.drain_user_notices();
            (
                events,
                quick_exits,
                persistence_notices,
                user_notices,
                should_exit,
            )
        };

        self.show_persistence_notices(persistence_notices, true);
        for notice in user_notices {
            self.toasts.warning(notice);
        }

        // Show overlay for any plugin that exited immediately
        for (plugin_id, output_lines) in quick_exits {
            let display_name = match plugin_id.as_str() {
                "claude-code" => "Claude Code",
                "codex" => "Codex",
                _ => &plugin_id,
            };

            let mut overlay_lines = vec![
                format!("{} exited immediately.", display_name),
                String::new(),
            ];

            if output_lines.is_empty() {
                overlay_lines.push("No output was captured.".into());
                overlay_lines.push("Check that the tool is installed and".into());
                overlay_lines.push("configured correctly.".into());
            } else {
                overlay_lines.push("Output:".into());
                // Show up to 8 lines of output
                for line in output_lines.iter().take(8) {
                    overlay_lines.push(format!("  {}", line));
                }
                if output_lines.len() > 8 {
                    overlay_lines.push(format!("  ... ({} more lines)", output_lines.len() - 8));
                }
            }

            self.info_overlay
                .show(format!("{} failed to start", display_name), overlay_lines);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            log::warn!("Plugin '{}' exited within 5 seconds of launch", plugin_id);
        }

        // Handle terminal events (notifications, bells, etc.)
        for event in events {
            self.handle_terminal_event(event);
        }

        // Observe every terminal independently so inactive tabs and panes do
        // not lose or duplicate semantic command regions.
        let mut completed_hooks: Vec<Option<i32>> = Vec::new();
        let show_indicators = self.harvest.is_some() && self.config.harvest.show_indicators;
        if let Some(tabs) = &mut self.tabs {
            for tab in tabs.all_tabs_mut() {
                if let Some(panes) = &mut tab.pane_manager {
                    for pane in panes.all_panes_mut() {
                        let regions = pane.harvest.observe(
                            &pane.terminal.shell_integration().prompt_tracker,
                            show_indicators,
                        );
                        for region in regions {
                            if let Some(harvest) = &mut self.harvest {
                                harvest.record_command(
                                    crate::harvest::HarvestSource {
                                        tab_id: tab.id,
                                        pane_id: Some(pane.id.0),
                                    },
                                    &region,
                                );
                            }
                            completed_hooks.push(region.exit_code);
                        }
                    }
                } else {
                    let regions = tab.harvest.observe(
                        &tab.terminal.shell_integration().prompt_tracker,
                        show_indicators,
                    );
                    for region in regions {
                        if let Some(harvest) = &mut self.harvest {
                            harvest.record_command(
                                crate::harvest::HarvestSource {
                                    tab_id: tab.id,
                                    pane_id: None,
                                },
                                &region,
                            );
                        }
                        completed_hooks.push(region.exit_code);
                    }
                }
            }
        }
        for exit_code in completed_hooks {
            self.dispatch_plugin_hook(crate::plugin::Hook::CommandExecuted {
                command: String::new(),
                exit_code,
            });
        }

        // Check if all tabs are closed
        if should_exit {
            log::info!("All tabs closed, exiting");
            self.should_exit = true;
        }
    }

    /// Handle window resize
    fn handle_resize(&mut self, width: u32, height: u32) {
        // An in-flight divider drag would leave stale hit-test rects after
        // the geometry changes — cancel it so the next click starts fresh.
        self.divider_dragging = false;
        // Same idea for the scrollbar drag (the scrollbar extents move).
        self.scrollbar_dragging = false;
        // Resize renderer
        if let Some(renderer) = &mut self.renderer {
            renderer.resize(width, height);

            // Calculate new terminal dimensions
            let (new_cols, new_rows) = renderer.calculate_grid_size(width, height);

            if new_cols != self.cols || new_rows != self.rows {
                self.cols = new_cols;
                self.rows = new_rows;

                // Resize all tabs and their pane managers
                if let Some(tabs) = &mut self.tabs {
                    tabs.resize(self.cols, self.rows);

                    let cell_width = renderer.cell_width();
                    let cell_height = renderer.cell_height();
                    let content_width = new_cols as f32 * cell_width;
                    let content_height = new_rows as f32 * cell_height;
                    for tab in tabs.all_tabs_mut() {
                        if let Some(ref mut pm) = tab.pane_manager {
                            pm.resize(content_width, content_height, cell_width, cell_height);
                        }
                    }
                }

                log::debug!("Resized to {}x{}", self.cols, self.rows);
            }
        }
    }

    fn handle_scale_factor_change(
        &mut self,
        scale_factor: f64,
        new_inner_size: winit::dpi::PhysicalSize<u32>,
    ) {
        if let Some(renderer) = &mut self.renderer {
            renderer.set_scale_factor(scale_factor as f32);
            renderer.resize(new_inner_size.width, new_inner_size.height);

            let (new_cols, new_rows) =
                renderer.calculate_grid_size(new_inner_size.width, new_inner_size.height);
            if new_cols != self.cols || new_rows != self.rows {
                self.cols = new_cols;
                self.rows = new_rows;

                if let Some(tabs) = &mut self.tabs {
                    tabs.resize(self.cols, self.rows);

                    // Resize pane managers — cell dimensions changed with scale factor
                    let cell_width = renderer.cell_width();
                    let cell_height = renderer.cell_height();
                    let content_width = new_cols as f32 * cell_width;
                    let content_height = new_rows as f32 * cell_height;
                    for tab in tabs.all_tabs_mut() {
                        if let Some(ref mut pm) = tab.pane_manager {
                            pm.resize(content_width, content_height, cell_width, cell_height);
                        }
                    }
                }
            }
        }
    }

    /// Render the terminal
    fn render(&mut self) {
        // Do not run animation/update paths that enqueue more redraws while a
        // lost surface is waiting for Winit's next real resize event.
        if self.surface_lost {
            return;
        }

        // Skip rendering if synchronized output mode is active (Mode 2026)
        // This prevents partial frames during batch terminal updates
        if let Some(tabs) = &self.tabs {
            if let Some(tab) = tabs.active_tab() {
                if tab.terminal.is_synchronized_output() {
                    // Request a redraw for when sync mode ends
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
            }
        }

        // Smooth hover animation progress for tab bar icons
        if self.tab_bar_hover.is_some() {
            self.tab_bar_hover_progress = (self.tab_bar_hover_progress + 0.2).min(1.0);
        } else {
            self.tab_bar_hover_progress = (self.tab_bar_hover_progress - 0.2).max(0.0);
        }

        // Update git watcher with active tab's working directory
        let working_dir = self
            .tabs
            .as_ref()
            .and_then(|t| t.active_tab())
            .and_then(|tab| tab.working_directory());
        self.git_watcher.update(working_dir.as_deref());

        // Build pane render info (before renderer borrow)
        let pane_info = self.pane_render_info();
        for pi in &pane_info {
            log::trace!(
                "Pane {:?}: ({}, {}, {}x{}) focused={}",
                pi.id,
                pi.rect.x,
                pi.rect.y,
                pi.rect.width,
                pi.rect.height,
                pi.is_focused
            );
        }

        // Verify container trait access (before renderer borrow)
        let _has_terminal = self.focused_terminal_via_container().is_some();

        // Update AI session chrome bar for plugin tabs
        let chrome_bar_data = {
            let branch = self.git_watcher.branch().map(|s| s.to_string());
            let dirty = self.git_watcher.is_dirty();
            let git_status = self.git_watcher.status();
            let wd = working_dir.clone();

            if let Some(tabs) = &mut self.tabs {
                if let Some(tab) = tabs.active_tab_mut() {
                    if let Some(ref mut session) = tab.ai_session {
                        session.update_git(branch.as_deref(), dirty, wd.as_deref());
                        Some(crate::ui::claude_chrome::ChromeBarInfo {
                            repo_name: session.repo_name.clone(),
                            branch: session.branch.clone(),
                            dirty: session.dirty,
                            ahead: git_status.map_or(0, |s| s.ahead),
                            behind: git_status.map_or(0, |s| s.behind),
                            host_label: session.host.badge_label(),
                            profile: session.profile.clone(),
                            recording: session.recording,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        };

        // Hot-reload the config if the file changed on disk.
        self.poll_config_for_hot_reload();

        // Get cursor target position and calculate animations before borrowing
        let cursor_opacity = self.cursor_opacity();
        let copy_flash = self.copy_flash_intensity();
        let focus_dim = self.focus_dim();

        // Get cursor target position and active tab ID from grid
        // When tab has pane_manager, read cursor from the focused pane's terminal
        let cursor_info = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.active_tab())
            .and_then(|tab| {
                if let Some(ref pm) = tab.pane_manager {
                    pm.focused_pane().map(|pane| {
                        let cursor = pane.terminal.grid().cursor();
                        (cursor.col, cursor.row, cursor.visible, tab.id)
                    })
                } else {
                    let cursor = tab.terminal.grid().cursor();
                    Some((cursor.col, cursor.row, cursor.visible, tab.id))
                }
            });

        // Update cursor animation if we have a target
        let cursor_display_pos = if let Some((col, row, visible, tab_id)) = cursor_info {
            self.update_cursor_animation(col, row, visible, Some(tab_id))
        } else {
            // No active tab - snap to origin and mark as not visible
            self.update_cursor_animation(0, 0, false, None)
        };

        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let Some(tabs) = &self.tabs else { return };
        let Some(tab) = tabs.active_tab() else { return };
        let harvest_indicators: &[crate::harvest::GrowthIndicator] =
            if let Some(panes) = &tab.pane_manager {
                panes
                    .focused_pane()
                    .map(|pane| pane.harvest.indicators.as_slice())
                    .unwrap_or_default()
            } else {
                &tab.harvest.indicators
            };

        let terminal = &tab.terminal;
        let tab_info = tabs.tab_info();

        // Build settings panel info if visible
        let settings_info = if self.settings_panel.is_visible() {
            // Tell the panel how many rows it can scroll through so its
            // scroll/selection logic stays in sync with the rendered viewport.
            let (sw, sh) = renderer.window_size();
            let layout = crate::ui::settings::SettingsModalLayout::calculate(
                sw as f32,
                sh as f32,
                renderer.cell_width(),
                renderer.cell_height(),
                self.settings_panel.animation_progress(),
                0,
            );
            self.settings_panel.set_max_visible(layout.rows_that_fit);
            Some(
                self.settings_panel
                    .build_info(&self.config, &self.shortcut_registry),
            )
        } else {
            None
        };

        // Build search bar info if search is active or has results
        let search_info = if self.search.is_active() || self.search.has_results() {
            Some(self.search.build_info(self.search_cursor_pos))
        } else {
            None
        };

        // Update settings panel animation
        if self.settings_panel.update_animation() {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        // Update session panel animation
        if self.session_panel.needs_redraw() {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        // Collect image placements for rendering
        let image_data: Vec<_> = terminal
            .graphics_placements()
            .iter()
            .filter_map(|p| terminal.get_image(p.image_id).map(|img| (p, img)))
            .collect();
        let images = if image_data.is_empty() {
            None
        } else {
            Some(image_data.as_slice())
        };

        // Build session dropdown info if dropdown is open
        let session_dropdown_info = if self.session_dropdown_open {
            Some(crate::renderer::SessionDropdownInfo {
                sessions: self.session_dropdown_items.clone(),
                hovered_index: self.session_dropdown_hovered,
            })
        } else {
            None
        };
        // Build session panel info if panel is open or animating
        self.session_panel.update();
        let session_panel_info = if !self.session_panel.is_closed() {
            Some(self.session_panel.build_info())
        } else {
            None
        };
        // Update commands panel animation
        if self.commands_panel.needs_redraw() {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        // Update visual bell animation
        self.bell.update();
        if self.bell.is_visual_active() {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        // Update harvest mode animations
        let has_active_harvest_indicator = !harvest_indicators.is_empty();
        if let Some(ref mut harvest) = self.harvest {
            if harvest.cellar.update()
                || harvest.cellar.is_visible()
                || harvest.sprout_menu.is_some()
                || has_active_harvest_indicator
            {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }

        // Build commands panel info if panel is open or animating
        let commands_panel_info = if !self.commands_panel.is_closed() {
            Some(self.commands_panel.build_info(&self.config.commands))
        } else {
            None
        };

        // Update command palette animation
        if self.command_palette.update_animation() {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        // Build palette info if active or animating
        let palette_info = if self.command_palette.is_active() {
            Some(self.command_palette.build_info())
        } else {
            None
        };

        // Update shortcuts panel animation
        if self.shortcuts_panel.update_animation() {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        // Auto-cancel shortcut recording after 15s of no input so users
        // can't get stuck in recording mode.
        if self
            .shortcuts_panel
            .tick_recording_timeout(std::time::Duration::from_secs(15))
        {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        // Build shortcuts panel info if visible
        let shortcuts_info = if self.shortcuts_panel.is_visible() {
            Some(self.shortcuts_panel.build_info(&self.shortcut_registry))
        } else {
            None
        };

        // Build info overlay if visible
        let info_overlay_data = if self.info_overlay.is_visible() {
            Some(crate::renderer::InfoOverlayInfo {
                title: self.info_overlay.title.clone(),
                lines: self.info_overlay.lines.clone(),
                animation: self.info_overlay.animation(),
                mouse_pos: self.last_cursor_pos.map(|(x, y)| (x as f32, y as f32)),
                mouse_pressed: self.mouse_pressed,
            })
        } else {
            None
        };
        renderer.set_chrome_bar(chrome_bar_data);
        renderer.set_info_overlay(info_overlay_data);

        // Update info overlay animation
        if self.info_overlay.update_animation() {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        // Toast lifecycle: prune expired, push snapshot into renderer.
        if self.toasts.update() {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        renderer.set_toasts(self.toasts.build_info());

        // Build diff review overlay if visible
        let diff_review_data = if self.diff_review.is_visible() {
            self.diff_review.diff.as_ref().map(|diff| {
                use crate::terminal::diff_detector::DiffLine;
                let mut lines = Vec::new();
                for hunk in &diff.hunks {
                    lines.push(crate::renderer::DiffReviewLine::Header(hunk.header.clone()));
                    for dl in &hunk.lines {
                        lines.push(match dl {
                            DiffLine::Context(s) => {
                                crate::renderer::DiffReviewLine::Context(s.clone())
                            }
                            DiffLine::Added(s) => crate::renderer::DiffReviewLine::Added(s.clone()),
                            DiffLine::Removed(s) => {
                                crate::renderer::DiffReviewLine::Removed(s.clone())
                            }
                        });
                    }
                    lines.push(crate::renderer::DiffReviewLine::Blank);
                }
                crate::renderer::DiffReviewInfo {
                    filename: diff.filename.clone(),
                    lines,
                    scroll_offset: self.diff_review.scroll_offset,
                    animation: self.diff_review.animation(),
                }
            })
        } else {
            None
        };
        renderer.set_diff_review(diff_review_data);

        // Update diff review animation
        if self.diff_review.update_animation() {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        // Build context picker overlay if visible
        let context_picker_data = if self.context_picker.is_visible() {
            Some(crate::renderer::ContextPickerInfo {
                sources: self
                    .context_picker
                    .sources
                    .iter()
                    .map(|s| crate::renderer::ContextPickerSource {
                        label: s.label.clone(),
                        selected: s.selected,
                        available: s.available,
                    })
                    .collect(),
                selected_index: self.context_picker.selected_index,
                animation: self.context_picker.animation(),
            })
        } else {
            None
        };
        renderer.set_context_picker(context_picker_data);

        // Update context picker animation
        if self.context_picker.update_animation() {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        // Build guardrail overlay if visible
        let guardrail_data = if self.guardrail.is_visible() {
            self.guardrail
                .matched
                .as_ref()
                .map(|m| crate::renderer::GuardrailInfo {
                    command: m.command.clone(),
                    reason: m.reason.clone(),
                    is_critical: m.severity == crate::terminal::guardrails::Severity::Critical,
                    animation: self.guardrail.animation(),
                    target: m.target.clone(),
                    user: m.user.clone(),
                })
        } else {
            None
        };
        renderer.set_guardrail(guardrail_data);

        // Update guardrail animation
        if self.guardrail.update_animation() {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        renderer.set_tab_bar_hover(self.tab_bar_hover, self.tab_bar_hover_progress);
        renderer.set_git_completions(
            if self.git_completions_visible && !self.git_completions.is_empty() {
                Some(crate::renderer::GitCompletionInfo {
                    completions: self.git_completions.clone(),
                    selected_index: self.git_completion_index,
                })
            } else {
                None
            },
        );
        renderer.set_bell_intensity(self.bell.visual_intensity());

        // Scan visible rows for auto-detected URLs (reuse cached storage)
        {
            let grid = terminal.grid();
            let rows = grid.rows();
            let cols = grid.cols();
            self.url_cache.resize(rows);
            self.url_cache.clear();
            for row in 0..rows {
                // Build row text from cell characters
                let mut line = String::with_capacity(cols);
                for col in 0..cols {
                    if let Some(cell) = grid.cell_scrolled(row, col) {
                        if let Some(ch) = cell.content.char() {
                            line.push(ch);
                        } else {
                            line.push(' ');
                        }
                    } else {
                        line.push(' ');
                    }
                }
                let urls = self.url_detector.detect_in_line(&line, row);
                if !urls.is_empty() {
                    self.url_cache.set_row(row, urls);
                }
            }
            // Swap the populated cache into the renderer for this frame
            let mut frame_cache = UrlCache::new(0);
            std::mem::swap(&mut frame_cache, &mut self.url_cache);
            renderer.set_url_cache(Some(frame_cache));
        }

        // Build pane render data for multi-pane mode (per-tab)
        let pane_render_data: Vec<PaneRenderData> = if let Some(ref pm) = tab.pane_manager {
            // Render pane dividers
            let dividers = pm.get_dividers();
            renderer.add_pane_dividers(&dividers);

            // Build per-pane render data with animated cursor for focused pane
            pm.iter_panes()
                .iter()
                .map(|(term, rect, focused)| {
                    let cpos = if *focused {
                        cursor_display_pos
                    } else {
                        let c = term.grid().cursor();
                        (c.col as f32, c.row as f32)
                    };
                    PaneRenderData {
                        grid: term.grid(),
                        selection: term.selection(),
                        rect: *rect,
                        is_focused: *focused,
                        cursor_pos: cpos,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        // Build harvest render info
        let show_indicators = self.config.harvest.show_indicators;
        let harvest_render_info = self.harvest.as_ref().map(|harvest| {
            // Growth indicators (respect the config toggle)
            let indicators: Vec<IndicatorRenderInfo> = if show_indicators {
                harvest_indicators
                    .iter()
                    .filter(|ind| !ind.is_finished())
                    .map(|ind| IndicatorRenderInfo {
                        row: ind.prompt_row,
                        character: ind.phase.character(),
                        color: ind.color(),
                        scale: ind.scale(),
                        opacity: if ind.is_finished() { 0.0 } else { 1.0 },
                    })
                    .collect()
            } else {
                Vec::new()
            };
            // Cellar panel (visible while open or animating out)
            let cellar = if harvest.cellar.is_visible() {
                Some(CellarRenderInfo {
                    entries: harvest
                        .cellar
                        .entries()
                        .map(|e| CellarEntryInfo {
                            row: e.row,
                            exit_code: e.exit_code,
                            success: e.success,
                            health: e.health(),
                            health_color: e.health_color(),
                            opacity: e.visual_state.opacity,
                            depth: e.visual_state.depth,
                        })
                        .collect(),
                    animation_progress: harvest.cellar.animation.progress(),
                    success_rate: harvest.cellar.success_rate(),
                    scroll: harvest.cellar.scroll as usize,
                })
            } else {
                None
            };

            // Sprout menu (if visible)
            let sprout_menu = harvest.sprout_menu.as_ref().map(|menu| {
                sprout_menu_render_info(
                    menu,
                    renderer.padding(),
                    renderer.cell_width(),
                    renderer.cell_height(),
                    renderer.tab_bar_height() + renderer.chrome_bar_height(),
                )
            });

            HarvestRenderInfo {
                indicators,
                cellar,
                sprout_menu,
            }
        });

        match renderer.render(
            terminal.grid(),
            terminal.selection(),
            cursor_opacity,
            cursor_display_pos,
            &tab_info,
            copy_flash,
            focus_dim,
            settings_info.as_ref(),
            search_info.as_ref(),
            &self.search,
            images,
            session_dropdown_info.as_ref(),
            session_panel_info.as_ref(),
            commands_panel_info.as_ref(),
            palette_info.as_ref(),
            shortcuts_info.as_ref(),
            self.config.cursor.style,
            &pane_render_data,
            harvest_render_info.as_ref(),
        ) {
            Ok(()) => {}
            Err(wgpu::SurfaceError::Lost) => {
                // A final X11 redraw can race WM_DELETE after the drawable is
                // already gone. Reconfiguring then is a WGPU validation error.
                // A live surface receives a subsequent Resized event, whose
                // normal path safely reconfigures it with current dimensions.
                if !self.surface_lost {
                    log::warn!("GPU surface lost; waiting for a resize event to reconfigure it");
                }
                self.surface_lost = true;
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                log::error!("Out of GPU memory");
                self.should_exit = true;
            }
            Err(e) => {
                log::warn!("Render error: {:?}", e);
            }
        }
    }

    /// Get active terminal mutably (helper)
    ///
    /// Routes through the pane manager's focused pane when in split mode,
    /// falls back to the tab's own terminal in single-terminal mode.
    fn active_terminal_mut(&mut self) -> Option<&mut termeric_core::Terminal> {
        let tab = self.tabs.as_mut()?.active_tab_mut()?;
        if let Some(ref mut pm) = tab.pane_manager {
            pm.focused_pane_mut().map(|p| &mut p.terminal)
        } else {
            Some(&mut tab.terminal)
        }
    }

    /// Calculate cursor opacity for smooth blink animation
    /// Returns a value between 0.0 (invisible) and 1.0 (fully visible)
    fn cursor_opacity(&self) -> f32 {
        let elapsed = self.cursor_blink_start.elapsed().as_secs_f32();
        cursor_opacity_at(self.config.cursor.blink, elapsed, self.cursor_blink_period)
    }

    /// Reset cursor blink animation (e.g., after typing)
    fn reset_cursor_blink(&mut self) {
        self.cursor_blink_start = Instant::now();
    }

    /// Update animated cursor position, interpolating toward target
    /// Returns the current display position (col, row) as floats
    ///
    /// Snaps immediately (no animation) when:
    /// - Cursor visibility changes (reappears after being hidden)
    /// - A new tab is created (to avoid animating from old tab's position)
    ///
    /// Animates smoothly for:
    /// - Switching between existing tabs
    /// - All cursor movement including large jumps (e.g., after `cd`, `clear`)
    fn update_cursor_animation(
        &mut self,
        target_col: usize,
        target_row: usize,
        cursor_visible: bool,
        active_tab_id: Option<usize>,
    ) -> (f32, f32) {
        let now = Instant::now();
        let delta = now.duration_since(self.last_frame_time).as_secs_f32();
        self.last_frame_time = now;

        let target_col_f = target_col as f32;
        let target_row_f = target_row as f32;

        // Check for snap conditions:
        // - Visibility change: cursor was hidden and reappears (e.g., command finished)
        // - New tab: tab ID is higher than any we've EVER seen (truly new tab)
        // We explicitly DON'T snap on tab switches - animate between existing tabs!
        // We explicitly DON'T snap on large jumps - that's the animation we want!
        let visibility_changed = cursor_visible != self.last_cursor_visible;

        // Detect truly new tab: compare against the highest ID we've ever seen,
        // not just the last active tab (which could be lower when switching tabs)
        let is_new_tab = match (active_tab_id, self.max_seen_tab_id) {
            (Some(current), Some(max)) => current > max,
            (Some(_), None) => true, // First tab ever
            _ => false,
        };

        // Update tracking state
        self.last_cursor_visible = cursor_visible;
        self.last_active_tab_id = active_tab_id;
        // Update max seen tab ID
        if let Some(current) = active_tab_id {
            self.max_seen_tab_id =
                Some(self.max_seen_tab_id.map_or(current, |max| max.max(current)));
        }

        // Snap immediately if cursor reappears or it's a truly new tab
        if visibility_changed || is_new_tab {
            self.cursor_display_col = target_col_f;
            self.cursor_display_row = target_row_f;
            return (self.cursor_display_col, self.cursor_display_row);
        }

        // Easing speed - higher = faster snap to target
        // 15.0 gives a quick but visible ease (reaches target in ~100-150ms)
        let ease_speed = 15.0;
        let t = (ease_speed * delta).min(1.0);

        // Lerp toward target
        self.cursor_display_col += (target_col_f - self.cursor_display_col) * t;
        self.cursor_display_row += (target_row_f - self.cursor_display_row) * t;

        // Snap if very close to avoid endless tiny movements
        if (self.cursor_display_col - target_col_f).abs() < 0.01 {
            self.cursor_display_col = target_col_f;
        }
        if (self.cursor_display_row - target_row_f).abs() < 0.01 {
            self.cursor_display_row = target_row_f;
        }

        (self.cursor_display_col, self.cursor_display_row)
    }

    /// Handle a plugin/custom keybinding action (SendText, RunCommand, PluginCommand).
    /// Returns true if the action was handled.
    fn handle_key_action(&mut self, key_action: KeyAction) -> bool {
        // Try to convert to internal Action first (PluginCommand)
        if let Some(action) = key_action.to_action() {
            self.handle_action(action);
            return true;
        }

        match key_action {
            KeyAction::SendText { text } => {
                self.write_to_pty(text.as_bytes());
                true
            }
            KeyAction::RunCommand { command } => {
                self.run_custom_command(&command);
                true
            }
            _ => false,
        }
    }

    /// Handle an action triggered by a keybinding.
    /// Returns true if the action was handled and the key should be consumed.
    fn handle_action(&mut self, action: Action) -> bool {
        match action {
            Action::Copy => {
                // If nothing was copied, don't consume the keystroke — let it
                // pass through (e.g. Ctrl+Shift+C falls through to the PTY).
                if !self.copy_selection() {
                    return false;
                }
            }
            Action::Paste => {
                self.paste_clipboard();
            }
            Action::ScrollUp(lines) => {
                if let Some(terminal) = self.active_terminal_mut() {
                    let offset = terminal.grid().scroll_offset();
                    terminal.grid_mut().set_scroll_offset(offset + lines);
                }
            }
            Action::ScrollDown(lines) => {
                if let Some(terminal) = self.active_terminal_mut() {
                    let offset = terminal.grid().scroll_offset();
                    terminal
                        .grid_mut()
                        .set_scroll_offset(offset.saturating_sub(lines));
                }
            }
            Action::ScrollToTop => {
                if let Some(terminal) = self.active_terminal_mut() {
                    let max_offset = terminal.grid().scrollback_len();
                    terminal.grid_mut().set_scroll_offset(max_offset);
                }
            }
            Action::ScrollToBottom => {
                if let Some(terminal) = self.active_terminal_mut() {
                    terminal.grid_mut().set_scroll_offset(0);
                }
            }
            Action::ClearSelection => {
                // Only consume the key if there's actually a selection to clear
                if self.mouse_state.current_selection().is_none() {
                    return false;
                }
                self.mouse_state.clear_selection();
                if let Some(terminal) = self.active_terminal_mut() {
                    terminal.set_selection(None);
                }
            }
            Action::IncreaseFontSize => {
                self.change_font_size(2.0);
            }
            Action::DecreaseFontSize => {
                self.change_font_size(-2.0);
            }
            Action::ResetFontSize => {
                self.set_font_size(self.default_font_size);
            }
            Action::NewTab => {
                if let Some(tabs) = &mut self.tabs {
                    match tabs.new_tab() {
                        Ok(id) => {
                            log::info!("Created new tab {}", id);
                            self.maybe_show_splash(true);
                            self.dispatch_plugin_hook(crate::plugin::Hook::TabCreated {
                                tab_id: id,
                            });
                        }
                        Err(e) => log::error!("Failed to create new tab: {}", e),
                    }
                }
            }
            Action::CloseTab => {
                let tab_id = self
                    .tabs
                    .as_ref()
                    .and_then(|t| t.active_tab().map(|tab| tab.id));
                if let Some(tabs) = &mut self.tabs {
                    tabs.close_active_tab();
                    if tabs.is_empty() {
                        log::info!("Last tab closed, exiting");
                        self.should_exit = true;
                    }
                }
                if let Some(id) = tab_id {
                    self.dispatch_plugin_hook(crate::plugin::Hook::TabClosed { tab_id: id });
                }
            }
            Action::NextTab => {
                let from = self.tabs.as_ref().map(|t| t.active_index()).unwrap_or(0);
                if let Some(tabs) = &mut self.tabs {
                    tabs.next_tab();
                }
                let to = self.tabs.as_ref().map(|t| t.active_index()).unwrap_or(0);
                if from != to {
                    self.dispatch_plugin_hook(crate::plugin::Hook::TabSwitched { from, to });
                }
            }
            Action::PrevTab => {
                let from = self.tabs.as_ref().map(|t| t.active_index()).unwrap_or(0);
                if let Some(tabs) = &mut self.tabs {
                    tabs.prev_tab();
                }
                let to = self.tabs.as_ref().map(|t| t.active_index()).unwrap_or(0);
                if from != to {
                    self.dispatch_plugin_hook(crate::plugin::Hook::TabSwitched { from, to });
                }
            }
            Action::SwitchToTab(index) => {
                let from = self.tabs.as_ref().map(|t| t.active_index()).unwrap_or(0);
                if let Some(tabs) = &mut self.tabs {
                    tabs.switch_to(index);
                }
                let to = self.tabs.as_ref().map(|t| t.active_index()).unwrap_or(0);
                if from != to {
                    self.dispatch_plugin_hook(crate::plugin::Hook::TabSwitched { from, to });
                }
            }
            Action::ToggleSearch => {
                if self.search.is_active() {
                    self.search.cancel();
                } else {
                    self.search.start();
                    self.search_cursor_pos = 0;
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Action::ToggleSettings => {
                self.toggle_settings_surface();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Action::PreviousPrompt => {
                if let Some(terminal) = self.active_terminal_mut() {
                    if let Some(row) = terminal.previous_prompt() {
                        // Scroll to make the prompt visible
                        let cursor_row = terminal.grid().cursor().row;
                        // Calculate how much to scroll
                        if row < cursor_row {
                            let diff = cursor_row - row;
                            let current_offset = terminal.grid().scroll_offset();
                            terminal.grid_mut().set_scroll_offset(current_offset + diff);
                        }
                        log::debug!("Navigate to previous prompt at row {}", row);
                    }
                }
            }
            Action::NextPrompt => {
                if let Some(terminal) = self.active_terminal_mut() {
                    if let Some(row) = terminal.next_prompt() {
                        // Scroll to make the prompt visible
                        let cursor_row = terminal.grid().cursor().row;
                        // Calculate how much to scroll
                        let current_offset = terminal.grid().scroll_offset();
                        if row > cursor_row && current_offset > 0 {
                            let diff = row - cursor_row;
                            terminal
                                .grid_mut()
                                .set_scroll_offset(current_offset.saturating_sub(diff));
                        }
                        log::debug!("Navigate to next prompt at row {}", row);
                    }
                }
            }
            Action::PluginCommand(command_id) => {
                self.execute_plugin_command(&command_id);
            }
            Action::ToggleCommandPalette => {
                if self.command_palette.is_active() {
                    self.command_palette.close();
                } else {
                    self.command_palette.open();
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Action::SplitVertical => {
                self.split_pane(SplitDirection::Vertical);
            }
            Action::SplitHorizontal => {
                self.split_pane(SplitDirection::Horizontal);
            }
            Action::NavigatePane(dir) => {
                if let Some(d) = crate::panes::PaneDir::parse(&dir) {
                    self.navigate_focused_pane(d);
                } else {
                    log::warn!("Unknown pane navigation direction: {}", dir);
                }
            }
            Action::ClosePane => {
                self.close_focused_pane();
            }
            Action::ClearScrollback => {
                if let Some(terminal) = self.active_terminal_mut() {
                    terminal.grid_mut().clear_scrollback();
                }
            }
            Action::ResetTerminal => {
                if let Some(terminal) = self.active_terminal_mut() {
                    terminal.reset();
                }
            }
        }
        true
    }

    /// Toggle Settings as an exclusive chrome surface. Side panels can cover
    /// the settings button and previously consumed the first click merely to
    /// close themselves, making Settings appear intermittently unresponsive.
    fn toggle_settings_surface(&mut self) {
        if self.settings_panel.is_active() {
            self.settings_panel.close();
            return;
        }

        self.commands_panel.close();
        self.session_panel.close();
        self.session_dropdown_open = false;
        self.settings_panel.toggle();
    }

    /// Handle the result of executing a command from the palette
    fn handle_palette_result(&mut self, result: PaletteResult) {
        match result {
            PaletteResult::RunAction(id) => {
                let action = match id.as_str() {
                    "core.new-tab" => Action::NewTab,
                    "core.close-tab" => Action::CloseTab,
                    "core.next-tab" => Action::NextTab,
                    "core.prev-tab" => Action::PrevTab,
                    "core.find" => Action::ToggleSearch,
                    "core.settings" => Action::ToggleSettings,
                    "core.shortcuts" => {
                        self.shortcuts_panel.toggle();
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                        return;
                    }
                    "core.save-config" => {
                        if let Ok(msg) = self.save_config() {
                            log::info!("{}", msg);
                        }
                        return;
                    }
                    "core.reload-config" => {
                        match crate::config::Config::load_or_default() {
                            Ok(config) => {
                                if let Err(error) = self.apply_config(config) {
                                    self.toasts.error(format!("Reload failed: {error}"));
                                } else {
                                    log::info!("Configuration reloaded");
                                    self.toasts.success("Configuration reloaded");
                                }
                            }
                            Err(e) => {
                                log::error!("Failed to reload config: {}", e);
                                self.toasts.error(format!("Reload failed: {}", e));
                            }
                        }
                        return;
                    }
                    "core.reset-config" => {
                        if let Err(error) = self.apply_config(Config::default()) {
                            self.toasts.error(format!("Reset failed: {error}"));
                        } else {
                            match self.save_config() {
                                Ok(path) => {
                                    self.toasts.success(format!("Configuration reset: {path}"))
                                }
                                Err(error) => self
                                    .toasts
                                    .error(format!("Reset could not be saved: {error}")),
                            };
                        }
                        return;
                    }
                    "core.session-manager" => {
                        if self.session_panel.is_open() {
                            self.session_panel.close();
                        } else {
                            self.session_panel.open();
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                        return;
                    }
                    "core.save-session" => {
                        self.force_save_session();
                        return;
                    }
                    "core.plugins" => {
                        use crate::ui::settings::SettingsSection;
                        self.settings_panel
                            .open_to_section(SettingsSection::Plugins);
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                        return;
                    }
                    "core.font-size-increase" => Action::IncreaseFontSize,
                    "core.font-size-decrease" => Action::DecreaseFontSize,
                    "core.font-size-reset" => Action::ResetFontSize,
                    "core.copy" => Action::Copy,
                    "core.paste" => Action::Paste,
                    "core.version" => {
                        self.info_overlay.show_version();
                        return;
                    }
                    "core.about" => {
                        self.info_overlay.show_about();
                        return;
                    }
                    "core.export-session" => {
                        // Export the active plugin tab's transcript
                        let plugin_id = self
                            .tabs
                            .as_ref()
                            .and_then(|tabs| tabs.active_tab())
                            .and_then(|tab| {
                                if let crate::tabs::TabType::Plugin { plugin_id } = &tab.tab_type {
                                    Some(plugin_id.clone())
                                } else {
                                    None
                                }
                            });
                        if let Some(id) = plugin_id {
                            self.export_plugin_session(&id);
                        } else {
                            log::info!("No plugin tab active for export");
                            self.toasts
                                .warning("Activate an AI plugin tab before exporting");
                        }
                        return;
                    }
                    "core.harvest-cellar" => {
                        if let Some(ref mut harvest) = self.harvest {
                            harvest.cellar.toggle_expanded();
                        }
                        return;
                    }
                    "core.show-sprout-menu" => {
                        // Anchor the menu to the latest recorded command
                        // so the actions target something the user can
                        // reason about. If no command has completed yet
                        // (e.g. fresh shell), fall back to cursor row.
                        let source = self.active_harvest_source();
                        let (anchor_row, anchor_col, entry) =
                            if let (Some(h), Some(source)) = (self.harvest.as_ref(), source) {
                                if let Some(latest) = h.cellar.latest_for_source(source).cloned() {
                                    let col = 0;
                                    (latest.row, col, Some(latest))
                                } else {
                                    let cur = self
                                        .active_terminal_mut()
                                        .map(|t| t.grid().cursor())
                                        .map(|c| (c.row, c.col))
                                        .unwrap_or((0, 0));
                                    (cur.0, cur.1, None)
                                }
                            } else {
                                (0, 0, None)
                            };
                        if entry.is_none() {
                            let message = self.active_shell_integration_message();
                            self.toasts.warning(message);
                            return;
                        }
                        if let Some(ref mut harvest) = self.harvest {
                            harvest.show_sprout_menu(anchor_row, anchor_col, entry);
                        }
                        return;
                    }
                    "core.hide-sprout-menu" => {
                        if let Some(ref mut harvest) = self.harvest {
                            harvest.hide_sprout_menu();
                        }
                        return;
                    }
                    "core.clear-harvest" => {
                        if let Some(ref mut harvest) = self.harvest {
                            harvest.cellar.clear();
                        }
                        return;
                    }
                    "core.toggle-transcript" => {
                        self.toggle_transcript();
                        return;
                    }
                    "core.split-vertical" => {
                        self.split_pane(SplitDirection::Vertical);
                        return;
                    }
                    "core.split-horizontal" => {
                        self.split_pane(SplitDirection::Horizontal);
                        return;
                    }
                    "core.close-pane" => {
                        self.close_focused_pane();
                        return;
                    }
                    "core.navigate-pane-up" => {
                        self.navigate_focused_pane(crate::panes::PaneDir::Up);
                        return;
                    }
                    "core.navigate-pane-down" => {
                        self.navigate_focused_pane(crate::panes::PaneDir::Down);
                        return;
                    }
                    "core.navigate-pane-left" => {
                        self.navigate_focused_pane(crate::panes::PaneDir::Left);
                        return;
                    }
                    "core.navigate-pane-right" => {
                        self.navigate_focused_pane(crate::panes::PaneDir::Right);
                        return;
                    }
                    id if id.starts_with("core.theme.") => {
                        let theme_name = id.strip_prefix("core.theme.").unwrap_or("");
                        match Theme::resolve(theme_name, None) {
                            Ok((theme, source)) => {
                                self.config.theme.name = theme_name.to_string();
                                self.config.theme.path = None;
                                self.apply_theme(theme, source);
                                match self.save_config() {
                                    Ok(msg) => {
                                        log::info!("Theme changed to {}: {}", theme_name, msg);
                                        self.toasts.info(format!("Theme: {}", theme_name));
                                    }
                                    Err(e) => {
                                        log::error!("Theme applied but save failed: {}", e);
                                        self.toasts.warning(format!(
                                            "Theme '{}' applied but not saved: {}",
                                            theme_name, e
                                        ));
                                    }
                                }
                            }
                            Err(err) => {
                                self.toasts
                                    .error(format!("Theme '{}' failed: {}", theme_name, err));
                            }
                        }
                        return;
                    }
                    _ => {
                        log::warn!("Unknown palette action: {}", id);
                        return;
                    }
                };
                self.handle_action(action);
            }
            PaletteResult::RunPluginCommand(id) => {
                self.execute_plugin_command(&id);
            }
            PaletteResult::Cancelled => {}
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Copy selection to clipboard. Returns true if anything was copied so
    /// the caller can decide whether to consume the keybinding.
    fn copy_selection(&mut self) -> bool {
        let Some(tabs) = &self.tabs else { return false };
        let Some(tab) = tabs.active_tab() else {
            return false;
        };
        let Some(text) = tab.terminal.get_selection_text() else {
            return false;
        };
        if text.is_empty() {
            return false;
        }

        if let Some(clipboard) = &mut self.clipboard {
            if let Err(e) = clipboard.set_text(&text) {
                log::error!("Failed to copy to clipboard: {}", e);
                return false;
            }
            log::debug!("Copied {} bytes to clipboard", text.len());
            self.copy_flash_start = Some(Instant::now());
            return true;
        }
        false
    }

    /// Calculate copy flash intensity (0.0 = no flash, 1.0 = full flash)
    /// Flash fades out over ~150ms
    fn copy_flash_intensity(&self) -> f32 {
        const FLASH_DURATION: f32 = 0.15; // 150ms

        match self.copy_flash_start {
            Some(start) => {
                let elapsed = start.elapsed().as_secs_f32();
                if elapsed >= FLASH_DURATION {
                    0.0
                } else {
                    // Fade out: 1.0 -> 0.0
                    1.0 - (elapsed / FLASH_DURATION)
                }
            }
            None => 0.0,
        }
    }

    /// Calculate focus dim amount (0.0 = fully bright/focused, 1.0 = dimmed/unfocused)
    /// Smoothly transitions over ~100ms when focus changes
    fn focus_dim(&self) -> f32 {
        const TRANSITION_DURATION: f32 = 0.1; // 100ms transition
        const DIM_AMOUNT: f32 = 0.3; // How much to dim when unfocused (30%)

        let elapsed = self.focus_change_time.elapsed().as_secs_f32();
        let t = (elapsed / TRANSITION_DURATION).min(1.0);

        // Ease-out for smooth transition
        let eased_t = 1.0 - (1.0 - t) * (1.0 - t);

        if self.window_focused {
            // Transitioning to focused: dim -> bright
            DIM_AMOUNT * (1.0 - eased_t)
        } else {
            // Transitioning to unfocused: bright -> dim
            DIM_AMOUNT * eased_t
        }
    }

    /// Paste from clipboard
    fn paste_clipboard(&mut self) {
        let text = match self.clipboard.as_mut().map(|c| c.get_text()) {
            Some(Ok(text)) => text,
            Some(Err(e)) => {
                log::error!("Failed to paste from clipboard: {}", e);
                self.toasts.error(format!("Paste failed: {}", e));
                return;
            }
            None => {
                self.toasts.warning("Clipboard not available");
                return;
            }
        };
        self.write_to_pty(text.as_bytes());
        // Clear any active selection — users expect paste to replace the
        // visual selection state, not leave it dangling.
        self.mouse_state.clear_selection();
        if let Some(terminal) = self.active_terminal_mut() {
            terminal.set_selection(None);
        }
    }

    /// Handle keyboard input when in search mode
    /// Returns true if the input was handled by search mode
    fn handle_search_mode_input(&mut self, event: &winit::event::KeyEvent) -> bool {
        use winit::keyboard::{Key, NamedKey};

        if event.state != winit::event::ElementState::Pressed {
            return self.search.is_active();
        }

        if !self.search.is_active() {
            return false;
        }

        // Get the grid for updating matches
        let grid_ref = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.active_tab().map(|tab| tab.terminal.grid()));

        match &event.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.search.cancel();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::Enter) => {
                // Confirm search and close input (but keep highlights)
                self.search.confirm();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::Backspace) => {
                if self.search_cursor_pos > 0 {
                    if let Some(grid) = grid_ref {
                        // Find the previous character boundary
                        let query = self.search.query();
                        if let Some((prev_pos, _)) =
                            query[..self.search_cursor_pos].char_indices().next_back()
                        {
                            self.search.pop_char(grid);
                            self.search_cursor_pos = prev_pos;
                        }
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                // Previous match
                self.search.prev();
                self.scroll_to_current_match();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::ArrowDown) => {
                // Next match
                self.search.next();
                self.scroll_to_current_match();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::ArrowLeft) => {
                // Move to previous character boundary
                let query = self.search.query();
                if self.search_cursor_pos > 0 {
                    if let Some((prev_pos, _)) =
                        query[..self.search_cursor_pos].char_indices().next_back()
                    {
                        self.search_cursor_pos = prev_pos;
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::ArrowRight) => {
                // Move to next character boundary
                let query = self.search.query();
                if self.search_cursor_pos < query.len() {
                    if let Some((_, ch)) = query[self.search_cursor_pos..].char_indices().next() {
                        self.search_cursor_pos += ch.len_utf8();
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::Home) => {
                self.search_cursor_pos = 0;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::End) => {
                self.search_cursor_pos = self.search.query().len();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Character(c) => {
                // Check for special key combinations
                if self.modifiers.alt_key() {
                    match c.as_str() {
                        "c" | "C" => {
                            // Toggle case sensitivity (Alt+C)
                            if let Some(grid) = grid_ref {
                                self.search.toggle_case_sensitive(grid);
                            }
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                            return true;
                        }
                        "r" | "R" => {
                            // Toggle regex mode (Alt+R)
                            if let Some(grid) = grid_ref {
                                self.search.toggle_regex(grid);
                            }
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                            return true;
                        }
                        "f" | "F" => {
                            // Toggle filter mode (Alt+F)
                            self.search.toggle_filter();
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                            return true;
                        }
                        _ => {}
                    }
                }

                // Regular character input
                if let Some(grid) = grid_ref {
                    for ch in c.chars() {
                        self.search.push_char(ch, grid);
                        self.search_cursor_pos += ch.len_utf8();
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }

        true // Input was handled by search mode
    }

    /// Scroll the terminal view to show the current search match
    fn scroll_to_current_match(&mut self) {
        let Some(search_match) = self.search.current() else {
            return;
        };
        let match_row = search_match.row;

        let Some(tabs) = &mut self.tabs else { return };
        let Some(tab) = tabs.active_tab_mut() else {
            return;
        };

        let grid = tab.terminal.grid_mut();
        let visible_rows = grid.rows() as i32;
        let scrollback_len = grid.scrollback_len() as i32;

        // Calculate the required scroll offset to bring the match into view
        // match_row is negative for scrollback, 0+ for visible grid
        if match_row < 0 {
            // Match is in scrollback - need to scroll up
            // Convert negative row index to scroll offset
            let lines_back = (-match_row) as usize;
            // Add a few lines of context
            let target_offset = lines_back.saturating_add(visible_rows as usize / 2);
            let clamped = target_offset.min(scrollback_len as usize);
            grid.set_scroll_offset(clamped);
        } else {
            // Match is in visible area or below current view
            let current_offset = grid.scroll_offset();
            if current_offset > 0 {
                // If scrolled up, check if match is visible
                let effective_row = match_row + current_offset as i32;
                if effective_row < 0 || effective_row >= visible_rows {
                    // Match is not in view, adjust scroll
                    grid.set_scroll_offset(0);
                }
            }
        }
    }

    /// Run a custom command in the current terminal tab
    fn run_custom_command(&mut self, command: &str) {
        log::info!("Running custom command: {}", command);
        for chunk in command_keystrokes(command) {
            self.write_to_pty(&chunk);
        }
    }

    /// Re-sync the command palette with currently registered plugins.
    ///
    /// Called after a plugin is registered or deregistered at runtime.
    fn refresh_plugin_commands(&mut self) {
        use crate::ui::command_palette::palette::{CommandSource, RegisteredCommand};

        let plugin_commands = self.plugin_manager.all_commands();
        for pc in &plugin_commands {
            log::debug!("Refreshing plugin command: {} - {}", pc.id, pc.description);
        }
        let keybindings = self.plugin_manager.all_keybindings();

        let palette_cmds: Vec<RegisteredCommand> = plugin_commands
            .iter()
            .map(|pc| {
                let shortcut = keybindings.iter().find(|k| k.command_id == pc.id).map(|k| {
                    let mut parts = Vec::new();
                    if k.ctrl {
                        parts.push("Ctrl".to_string());
                    }
                    if k.alt {
                        parts.push("Alt".to_string());
                    }
                    if k.shift {
                        parts.push("Shift".to_string());
                    }
                    parts.push(k.key.to_uppercase());
                    parts.join("+")
                });
                let plugin_source = pc
                    .id
                    .split_once('.')
                    .map(|(prefix, _)| prefix.to_string())
                    .unwrap_or_else(|| "plugin".to_string());
                RegisteredCommand {
                    id: pc.id.clone(),
                    title: pc.name.clone(),
                    category: pc.category.clone().unwrap_or_else(|| "Plugins".to_string()),
                    shortcut,
                    icon: None,
                    source: CommandSource::Plugin(plugin_source),
                    destructive: false,
                }
            })
            .collect();

        self.command_palette.replace_plugin_commands(palette_cmds);

        log::info!(
            "Refreshed plugin commands: {} plugin(s), {} command(s)",
            self.plugin_manager.count(),
            plugin_commands.len(),
        );
    }

    /// Save current configuration to file
    fn save_config(&mut self) -> Result<String> {
        let config_path = Config::config_path();

        // Ensure config directory exists
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Build current config, syncing any live values
        let mut config = self.config.clone();
        if let Some(renderer) = &self.renderer {
            config.font.size = renderer.font_size();
        }
        config.shortcuts = self.shortcut_registry.user_overrides();
        config.validate()?;

        let config_content = toml::to_string_pretty(&config)?;

        std::fs::write(&config_path, config_content)?;
        // Capture the new mtime so the hot-reload poller doesn't treat
        // our own write as an external change.
        self.last_config_mtime = std::fs::metadata(&config_path)
            .and_then(|m| m.modified())
            .ok();
        Ok(config_path.display().to_string())
    }
}

fn cursor_opacity_at(blink: bool, elapsed_seconds: f32, period_seconds: f32) -> f32 {
    if !blink {
        return 1.0;
    }
    let phase = (elapsed_seconds / period_seconds) * std::f32::consts::TAU;
    0.65 + 0.35 * phase.cos()
}

fn select_plugin_target(
    active_plugin_id: Option<&str>,
    available_plugin_ids: &[String],
) -> std::result::Result<String, String> {
    if let Some(active) = active_plugin_id {
        if available_plugin_ids.iter().any(|id| id == active) {
            return Ok(active.to_string());
        }
    }

    let mut unique = available_plugin_ids.to_vec();
    unique.sort();
    unique.dedup();
    match unique.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err("Open an AI plugin tab before using Explain".into()),
        _ => Err("Multiple AI plugin tabs are open; activate the intended one first".into()),
    }
}

fn command_keystrokes(command: &str) -> [Vec<u8>; 3] {
    [
        b"\x15".to_vec(),
        command.trim_end_matches(['\r', '\n']).as_bytes().to_vec(),
        b"\r".to_vec(),
    ]
}

fn sprout_menu_render_info(
    menu: &crate::harvest::SproutMenu,
    padding: f32,
    cell_width: f32,
    cell_height: f32,
    content_top: f32,
) -> SproutMenuRenderInfo {
    let action_count = menu.actions.len() as f32;
    SproutMenuRenderInfo {
        x: padding + menu.col as f32 * cell_width,
        y: content_top + padding + menu.row as f32 * cell_height,
        actions: menu
            .actions
            .iter()
            .map(|action| (action.name().to_string(), action.icon()))
            .collect(),
        selected: menu.selected,
        opacity: menu.opacity(),
        width: menu.width() as f32 * cell_width,
        height: action_count * cell_height * 1.4 + cell_width,
    }
}

/// Minimal RFC 4648 base64 encoder (standard alphabet) used for the OSC 52
/// clipboard reply. Keeps us off an external `base64` dependency.
fn base64_encode_standard(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut chunks = input.chunks_exact(3);
    for chunk in &mut chunks {
        let n = ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8) | (chunk[2] as u32);
        out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
        out.push(ALPHABET[(n & 0x3F) as usize] as char);
    }
    let rem = chunks.remainder();
    match rem.len() {
        1 => {
            let n = (rem[0] as u32) << 16;
            out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let n = ((rem[0] as u32) << 16) | ((rem[1] as u32) << 8);
            out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
            out.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod base64_tests {
    use super::{
        base64_encode_standard, command_keystrokes, cursor_opacity_at, select_plugin_target,
    };

    #[test]
    fn disabled_cursor_blink_is_fully_opaque() {
        assert_eq!(cursor_opacity_at(false, 50.0, 0.2), 1.0);
    }

    #[test]
    fn cursor_blink_rate_controls_the_cycle() {
        assert!((cursor_opacity_at(true, 0.0, 1.0) - 1.0).abs() < f32::EPSILON);
        assert!((cursor_opacity_at(true, 0.5, 1.0) - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn plugin_target_prefers_active_then_unambiguous_tab() {
        let plugins = vec!["claude-code".to_string(), "codex".to_string()];
        assert_eq!(
            select_plugin_target(Some("codex"), &plugins).unwrap(),
            "codex"
        );
        assert_eq!(
            select_plugin_target(None, &["claude-code".to_string()]).unwrap(),
            "claude-code"
        );
        assert!(select_plugin_target(None, &plugins).is_err());
        assert!(select_plugin_target(None, &[]).is_err());
    }

    #[test]
    fn dynamic_run_command_builds_single_enter_payload() {
        assert_eq!(
            command_keystrokes("cargo test"),
            [b"\x15".to_vec(), b"cargo test".to_vec(), b"\r".to_vec()]
        );
        assert_eq!(
            command_keystrokes("cargo test\n"),
            [b"\x15".to_vec(), b"cargo test".to_vec(), b"\r".to_vec()]
        );
    }

    #[test]
    fn test_base64_empty() {
        assert_eq!(base64_encode_standard(b""), "");
    }

    #[test]
    fn test_base64_rfc_vectors() {
        assert_eq!(base64_encode_standard(b"f"), "Zg==");
        assert_eq!(base64_encode_standard(b"fo"), "Zm8=");
        assert_eq!(base64_encode_standard(b"foo"), "Zm9v");
        assert_eq!(base64_encode_standard(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode_standard(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode_standard(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn test_base64_binary() {
        // 00 FF 80 → AP+A, 7F 01 → fwE=
        let bytes = [0x00, 0xff, 0x80, 0x7f, 0x01];
        assert_eq!(base64_encode_standard(&bytes), "AP+AfwE=");
    }
}
