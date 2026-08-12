//! Application state and event handling

mod event_loop;
pub mod state;

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
    /// Number of command regions already processed by harvest
    harvest_region_cursor: usize,
    /// Prompt row of the command currently running (used to drive the growth
    /// indicator). None when the shell is at a fresh prompt with no command.
    harvest_active_prompt_row: Option<usize>,
    /// URL detector for auto-detecting URLs in terminal output
    url_detector: UrlDetector,
    /// Cached URL lookup table, reused between frames
    url_cache: UrlCache,
}

impl App {
    /// Create a new application instance
    pub fn new() -> Result<Self> {
        let (config, startup_config_warning) = Config::load_for_startup();
        let default_font_size = config.font.size;
        let cursor_blink_period = (config.cursor.blink_rate as f32 / 1000.0) * 2.0;
        let theme = Self::load_theme_from_config(&config);

        // Initialize bell from config
        let bell = Bell::new(BellConfig {
            visual: config.bell.visual,
            audio: config.bell.audio,
            visual_duration_ms: config.bell.visual_duration_ms,
            ..Default::default()
        });

        // Initialize clipboard (may fail on some systems)
        let clipboard = arboard::Clipboard::new().ok();

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
            cols: 80,
            rows: 24,
            cursor_blink_start: Instant::now(),
            cursor_blink_period,
            cursor_display_col: 0.0,
            cursor_display_row: 0.0,
            last_frame_time: Instant::now(),
            last_config_check: Instant::now(),
            last_config_mtime: std::fs::metadata(crate::config::Config::config_path())
                .and_then(|m| m.modified())
                .ok(),
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
            app_state: AppState::load(),
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
            harvest_region_cursor: 0,
            harvest_active_prompt_row: None,
            url_detector: UrlDetector::new(),
            url_cache: UrlCache::new(0),
        });

        let mut app = result?;

        // Initialize command palette and apply all runtime config through the
        // same path used by reload, settings changes, and reset.
        app.command_palette.register_commands(core_commands());
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

    /// Initialize the tab manager with the first tab or restore from session
    fn init_tabs(&mut self) -> Result<()> {
        let mut tabs = TabManager::new(
            self.cols,
            self.rows,
            self.config.shell.clone(),
            self.config.scrollback.lines,
        );

        // Always start with a fresh tab (sessions are per-tab, loaded on demand)
        tabs.new_tab()?;

        self.tabs = Some(tabs);

        // Show splash screen based on configuration
        self.maybe_show_splash(false);

        log::info!("Tab manager initialized: {}x{}", self.cols, self.rows);

        // Notify plugins that the app is ready
        self.dispatch_plugin_hook(crate::plugin::Hook::AppReady);

        Ok(())
    }

    /// Check splash mode and show splash if appropriate
    ///
    /// `is_new_tab` indicates whether this is a new tab (for NewTab mode)
    fn maybe_show_splash(&mut self, is_new_tab: bool) {
        let should_show = match self.config.splash.mode {
            SplashMode::Never => false,
            SplashMode::FirstBoot => {
                // Only show if first boot hasn't been completed
                !self.app_state.first_boot_complete
            }
            SplashMode::NewWindow => {
                // Show for new windows (not tabs)
                !is_new_tab
            }
            SplashMode::NewTab => {
                // Show for every new tab (and new windows)
                true
            }
        };

        if should_show {
            if let Some(tabs) = &mut self.tabs {
                if let Some(tab) = tabs.active_tab_mut() {
                    show_splash(&mut tab.terminal);
                    log::debug!("Splash screen displayed");
                }
            }

            // Mark first boot as complete if this was a FirstBoot splash
            if self.config.splash.mode == SplashMode::FirstBoot
                && !self.app_state.first_boot_complete
            {
                if let Err(e) = self.app_state.mark_first_boot_complete() {
                    log::warn!("Failed to save first boot state: {}", e);
                }
            }
        }
    }

    /// Save the current session to disk (auto-save path, respects config)
    fn save_current_session(&mut self) {
        if !self.config.session.enabled || !self.config.session.auto_save {
            return;
        }

        self.force_save_session();
    }

    /// Unconditionally save the current session (explicit user action)
    fn force_save_session(&mut self) {
        let Some(tabs) = &mut self.tabs else { return };
        let save_scrollback = self.config.session.save_scrollback;
        let count = tabs.count();
        tabs.save_all_sessions(save_scrollback);
        log::info!("Sessions saved");
        let label = if count == 1 { "session" } else { "sessions" };
        self.toasts.success(format!("Saved {} {}", count, label));
    }

    /// Process PTY events for all tabs and panes
    fn process_pty_events(&mut self) {
        // Process events and collect results
        let (events, quick_exits, should_exit) = {
            let Some(tabs) = &mut self.tabs else { return };
            let (_, events) = tabs.process_all_events();

            // Detect plugin tabs that exited too quickly (< 5s) — before cleanup removes them
            let quick_exits = tabs.drain_quick_exit_plugin_info();

            let should_exit =
                tabs.cleanup_closed(self.config.session.save_scrollback) && tabs.is_empty();
            (events, quick_exits, should_exit)
        };

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

        // Feed newly completed command regions to harvest mode and notify
        // plugins. Also track the currently-running command so growth
        // indicators have time to animate before the completion pulse.
        let mut completed_hooks: Vec<Option<i32>> = Vec::new();
        let show_indicators = self.config.harvest.show_indicators;
        if let Some(tabs) = &self.tabs {
            if let Some(tab) = tabs.active_tab() {
                let tracker = &tab.terminal.shell_integration().prompt_tracker;

                // 1. Start indicator for the in-progress command (if any)
                //    whose output has actually begun. We skip the idle prompt
                //    state (no command typed yet) so the indicator only
                //    appears for running work, not every empty prompt.
                let active_row = tracker
                    .active_region()
                    .filter(|r| r.output_row.is_some())
                    .map(|r| r.prompt_row);
                if active_row != self.harvest_active_prompt_row {
                    // If we previously had an active row that never reached
                    // `completed_region`, clear it — the shell probably
                    // didn't emit OSC 133;D. Harmless.
                    self.harvest_active_prompt_row = active_row;
                    if let (Some(row), Some(ref mut harvest)) = (active_row, self.harvest.as_mut())
                    {
                        if show_indicators {
                            harvest.start_command(row);
                        }
                    }
                }

                // 2. Drain any newly-completed regions.
                let total = tracker.completed_count();
                while self.harvest_region_cursor < total {
                    if let Some(region) = tracker.completed_region(self.harvest_region_cursor) {
                        if let Some(ref mut harvest) = self.harvest {
                            harvest.record_command(region);
                            if show_indicators {
                                // If we never saw it in the "active" state
                                // (e.g. instant command), emit start first
                                // so the indicator has something to animate
                                // out of.
                                if harvest
                                    .indicators
                                    .iter()
                                    .all(|i| i.prompt_row != region.prompt_row)
                                {
                                    harvest.start_command(region.prompt_row);
                                }
                                harvest.complete_command(
                                    region.prompt_row,
                                    region.exit_code == Some(0),
                                );
                            }
                        }
                        // The command whose indicator was active just
                        // finished — clear the watch.
                        if self.harvest_active_prompt_row == Some(region.prompt_row) {
                            self.harvest_active_prompt_row = None;
                        }
                        completed_hooks.push(region.exit_code);
                    }
                    self.harvest_region_cursor += 1;
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

    /// Handle a terminal event
    fn handle_terminal_event(&mut self, event: TerminalEvent) {
        match event {
            TerminalEvent::Bell => {
                self.bell.ring();
            }
            TerminalEvent::Notification(req) => {
                let title = req.title.as_deref().unwrap_or("Terminal");
                self.notifications.notify(title, &req.body);
            }
            TerminalEvent::TitleChanged(_title) => {
                // Title is handled via shell cwd detection
            }
            TerminalEvent::ClipboardRequest(_which) => {
                // OSC 52 read: echo the clipboard contents back to the PTY
                // as a base64-encoded OSC 52 reply so the requesting app
                // doesn't time out. If the clipboard is unavailable we
                // still emit an empty reply so the app fails fast.
                let text = self
                    .clipboard
                    .as_mut()
                    .and_then(|c| c.get_text().ok())
                    .unwrap_or_default();
                let encoded = base64_encode_standard(text.as_bytes());
                let reply = format!("\x1b]52;c;{}\x07", encoded);
                self.write_to_pty(reply.as_bytes());
            }
            TerminalEvent::ClipboardWrite(text) => {
                if let Some(clipboard) = &mut self.clipboard {
                    if let Err(e) = clipboard.set_text(&text) {
                        log::error!("Failed to set clipboard: {}", e);
                    }
                }
            }
            TerminalEvent::HyperlinkActivated(url) => {
                if let Err(e) = crate::terminal::url::UrlDetector::open_url(&url) {
                    log::error!("Failed to open URL: {}", e);
                }
            }
            TerminalEvent::CursorVisibilityChanged(_) => {
                // Cursor visibility is handled in render
            }
            TerminalEvent::PtyWrite(_) => {
                // Handled in Tab::process_events (written back to tab PTY)
            }
            TerminalEvent::Exit => {
                // Individual tab exit is handled by Tab::process_events
            }
        }
    }

    /// Write input to the active tab's PTY, with custom command alias expansion
    fn write_to_pty(&mut self, data: &[u8]) {
        // Reset cursor blink so cursor is fully visible when typing
        self.reset_cursor_blink();

        // Track the input line buffer for alias expansion + git completion
        if data == b"\r" {
            // If git completions are visible and a selection is active, accept it
            if self.git_completions_visible {
                if let Some(idx) = self.git_completion_index {
                    if let Some(branch) = self.git_completions.get(idx).cloned() {
                        // Determine how many chars of partial text to erase
                        if let Some(partial) =
                            crate::git::detect_git_completion(&self.input_line_buffer)
                        {
                            let partial_len = partial.len();
                            // Send backspaces to erase the partial, then type the branch name
                            if let Some(tabs) = &mut self.tabs {
                                if let Some(tab) = tabs.active_tab_mut() {
                                    for _ in 0..partial_len {
                                        let _ = tab.write(b"\x7f"); // backspace
                                    }
                                    let _ = tab.write(branch.as_bytes());
                                    let _ = tab.write(b"\r"); // then Enter
                                }
                            }
                            self.input_line_buffer.clear();
                            self.git_completions_visible = false;
                            self.git_completions.clear();
                            self.git_completion_index = None;
                            return;
                        }
                    }
                }
                // Dismiss completions
                self.git_completions_visible = false;
                self.git_completions.clear();
                self.git_completion_index = None;
            }

            // Enter pressed — guardrail check for plugin tabs
            let is_plugin_tab = self
                .tabs
                .as_ref()
                .and_then(|tabs| {
                    tabs.active_tab()
                        .map(|t| matches!(t.tab_type, crate::tabs::TabType::Plugin { .. }))
                })
                .unwrap_or(false);

            if is_plugin_tab {
                let input = self.input_line_buffer.trim().to_string();
                if let Some(matched) = crate::terminal::guardrails::check_command(&input) {
                    log::warn!(
                        "Guardrail triggered: {} — {}",
                        matched.command,
                        matched.reason
                    );
                    self.guardrail.show(matched, data.to_vec());
                    self.input_line_buffer.clear();
                    return;
                }
            }

            // Enter pressed — check if first word matches a custom command name or alias
            let trimmed = self.input_line_buffer.trim().to_string();
            let (first_word, rest) = match trimmed.find(' ') {
                Some(pos) => (trimmed[..pos].to_string(), trimmed[pos..].to_string()),
                None => (trimmed.clone(), String::new()),
            };
            let matched_command = self
                .config
                .commands
                .iter()
                .find(|cmd| cmd.name == first_word || cmd.alias.as_deref() == Some(&first_word))
                .map(|cmd| cmd.command.clone());

            // If a git command was run, invalidate the watcher to refresh branch state
            if trimmed.starts_with("git ") {
                self.git_watcher.invalidate();
            }

            self.input_line_buffer.clear();

            if let Some(real_command) = matched_command {
                // Build expanded command: real_command + any trailing args
                let expanded = format!("{}{}", real_command, rest);
                // Clear the typed alias from the terminal line (Ctrl+U)
                // then send the real command + Enter
                if let Some(tabs) = &mut self.tabs {
                    if let Some(tab) = tabs.active_tab_mut() {
                        tab.terminal.grid_mut().set_scroll_offset(0);
                        let _ = tab.write(b"\x15"); // Ctrl+U: clear line
                        let cmd_with_enter = format!("{}\r", expanded);
                        let _ = tab.write(cmd_with_enter.as_bytes());
                    }
                }
                return;
            }
        } else if data == b"\x7f" {
            // Backspace — remove last char from buffer
            self.input_line_buffer.pop();
        } else if data == b"\x15" {
            // Ctrl+U — clear buffer
            self.input_line_buffer.clear();
        } else if data == b"\x17" {
            // Ctrl+W — delete last word
            let trimmed = self.input_line_buffer.trim_end();
            if let Some(pos) = trimmed.rfind(' ') {
                self.input_line_buffer.truncate(pos + 1);
            } else {
                self.input_line_buffer.clear();
            }
        } else if data.len() == 1 && data[0] < 0x20 {
            // Other control characters (Ctrl+C, Ctrl+D, etc.) — reset buffer
            self.input_line_buffer.clear();
        } else if data.len() >= 2 && data[0] == 0x1b {
            // Escape sequences (arrow keys, etc.) — can affect cursor position, reset buffer
            self.input_line_buffer.clear();
        } else {
            // Regular characters — append to buffer
            if let Ok(s) = std::str::from_utf8(data) {
                self.input_line_buffer.push_str(s);
            }
        }

        // Update git completions based on current input
        if let Some(partial) = crate::git::detect_git_completion(&self.input_line_buffer) {
            let completions = self.git_watcher.complete_branch(partial);
            if !completions.is_empty() {
                self.git_completions = completions;
                self.git_completion_index = Some(0);
                self.git_completions_visible = true;
            } else {
                self.git_completions_visible = false;
                self.git_completions.clear();
                self.git_completion_index = None;
            }
        } else {
            self.git_completions_visible = false;
            self.git_completions.clear();
            self.git_completion_index = None;
        }

        // Route keyboard input to the active tab — through pane manager if split,
        // otherwise directly to the tab's PTY
        if let Some(tabs) = &mut self.tabs {
            if let Some(tab) = tabs.active_tab_mut() {
                if let Some(ref mut pm) = tab.pane_manager {
                    if let Some(pane) = pm.focused_pane_mut() {
                        pane.terminal.grid_mut().set_scroll_offset(0);
                        if let Err(e) = pane.write(data) {
                            log::error!("Failed to write to pane PTY: {}", e);
                        }
                    }
                } else {
                    tab.terminal.grid_mut().set_scroll_offset(0);
                    if let Err(e) = tab.write(data) {
                        log::error!("Failed to write to PTY: {}", e);
                    }
                }
            }
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

        // Compute frame delta time for animations
        let frame_dt = self.last_frame_time.elapsed().as_secs_f32();

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
        if let Some(ref mut harvest) = self.harvest {
            harvest.update(frame_dt);
            harvest.cellar.update(frame_dt);
            if harvest.has_active_indicators()
                || harvest.cellar.is_visible()
                || harvest.sprout_menu.is_some()
            {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }

            // Update sprout menu animation
            if let Some(ref mut menu) = harvest.sprout_menu {
                menu.update(frame_dt);
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
                harvest
                    .indicators
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
                let cell_width = renderer.cell_width();
                let cell_height = renderer.cell_height();
                let content_top = renderer.tab_bar_height() + renderer.chrome_bar_height();
                SproutMenuRenderInfo {
                    x: renderer.padding() + menu.col as f32 * cell_width,
                    y: content_top + renderer.padding() + menu.row as f32 * cell_height,
                    actions: menu
                        .actions
                        .iter()
                        .map(|a| (a.name().to_string(), a.icon()))
                        .collect(),
                    selected: menu.selected,
                    opacity: menu.opacity(),
                    width: menu.width() as f32 * cell_width,
                    height: menu.height() as f32 * cell_height,
                }
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
                if let Some(window) = &self.window {
                    let size = window.inner_size();
                    renderer.resize(size.width, size.height);
                }
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
                log::info!("Run command requested: {}", command);
                false
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
                self.settings_panel.toggle();
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
                        let (anchor_row, anchor_col, entry) = if let Some(ref h) = self.harvest {
                            if let Some(latest) = h.cellar.latest().cloned() {
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
                        match Theme::load_by_name(theme_name) {
                            Ok(theme) => {
                                self.config.theme.name = theme_name.to_string();
                                self.apply_theme(theme);
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

    /// Build a PluginContext from current app state.
    fn build_plugin_context(&self) -> PluginContext {
        let cwd = self
            .tabs
            .as_ref()
            .and_then(|t| t.active_tab())
            .and_then(|tab| tab.working_directory());

        let tab_count = self.tabs.as_ref().map_or(0, |t| t.count());
        let active_tab = self.tabs.as_ref().map_or(0, |t| t.active_index());

        PluginContext {
            cwd,
            tab_count,
            active_tab,
            terminal_size: (self.cols, self.rows),
        }
    }

    /// Dispatch a plugin hook and execute any returned actions.
    fn dispatch_plugin_hook(&mut self, hook: crate::plugin::Hook) {
        let ctx = self.build_plugin_context();
        log::debug!(
            "Plugin hook: {:?} (tabs={}, active={}, size={}x{})",
            hook,
            ctx.tab_count,
            ctx.active_tab,
            ctx.terminal_size.0,
            ctx.terminal_size.1
        );
        let actions = self.plugin_manager.dispatch(&hook, &ctx);
        self.execute_plugin_actions(actions);
    }

    /// Execute a plugin command by ID.
    ///
    /// Commands that require direct App state access (review-diff, copy-diff, etc.)
    /// are matched by suffix so they work for any AI plugin (claude-code, codex, etc.).
    fn execute_plugin_command(&mut self, command_id: &str) {
        // Extract the suffix after the first '.' for plugin-agnostic matching
        if let Some(suffix) = command_id.split_once('.').map(|(_, s)| s) {
            let plugin_id = command_id.split_once('.').map(|(p, _)| p).unwrap_or("");
            match suffix {
                "review-diff" => {
                    self.open_diff_review();
                    return;
                }
                "copy-diff" => {
                    self.copy_latest_diff();
                    return;
                }
                "insert-context" => {
                    self.open_context_picker_for(plugin_id);
                    return;
                }
                "export-session" => {
                    self.export_plugin_session(plugin_id);
                    return;
                }
                "jump-to-error" => {
                    self.jump_to_last_error(plugin_id);
                    return;
                }
                "select-profile" => {
                    self.execute_plugin_actions(vec![crate::plugin::PluginAction::ListProfiles {
                        plugin_id: plugin_id.to_string(),
                    }]);
                    return;
                }
                _ => {}
            }
        }
        let ctx = self.build_plugin_context();
        let actions = self.plugin_manager.execute_command(command_id, &ctx);
        if !actions.is_empty() {
            self.execute_plugin_actions(actions);
        }
    }

    /// Open the diff review overlay with the latest unreviewed diff
    fn open_diff_review(&mut self) {
        if let Some(tabs) = &self.tabs {
            // Find the first tab with unreviewed diffs
            for (i, tab) in tabs.iter() {
                if tab.diff_detector.unreviewed_count() > 0 {
                    if let Some(diff) = tab.diff_detector.latest_unreviewed() {
                        self.diff_review.show(diff.clone(), i);
                        return;
                    }
                }
            }
        }
        log::info!("No unreviewed diffs to show");
    }

    /// Copy the latest detected diff to clipboard
    fn copy_latest_diff(&mut self) {
        if let Some(tabs) = &self.tabs {
            for (_, tab) in tabs.iter() {
                if let Some(diff) = tab.diff_detector.latest_unreviewed() {
                    if let Some(clipboard) = &mut self.clipboard {
                        let _ = clipboard.set_text(diff.raw.clone());
                        log::info!("Copied diff for '{}' to clipboard", diff.filename);
                    }
                    return;
                }
            }
        }
        log::info!("No diffs to copy");
    }

    /// Open the context picker modal targeting a specific plugin tab
    fn open_context_picker_for(&mut self, plugin_id: &str) {
        let cwd = if let Some(tabs) = &self.tabs {
            tabs.plugin_tab_cwd(plugin_id).or_else(|| {
                tabs.iter()
                    .find(|(_, t)| !t.closed)
                    .and_then(|(_, t)| t.working_directory())
            })
        } else {
            None
        };
        self.context_picker.target_plugin_id = Some(plugin_id.to_string());
        self.context_picker.open(cwd);
    }

    /// Gather context and inject it into the target plugin tab
    fn inject_context(&mut self) {
        // Get terminal output from active shell tab (if selected)
        let terminal_output = if let Some(tabs) = &self.tabs {
            // Get output from the first non-plugin tab
            tabs.iter()
                .find(|(_, t)| matches!(t.tab_type, crate::tabs::TabType::Shell) && !t.closed)
                .map(|(_, tab)| {
                    let lines = tab.terminal.grid().export_scrollback_text();
                    let start = lines.len().saturating_sub(50);
                    lines[start..].join("\n")
                })
        } else {
            None
        };

        let assembled = self.context_picker.gather(terminal_output);

        // Inject into the target plugin tab's PTY
        let target = self
            .context_picker
            .target_plugin_id
            .clone()
            .unwrap_or_else(|| "claude-code".to_string());
        if let Some(tabs) = &mut self.tabs {
            // Wrap in a clear marker so the AI tool can parse it
            let injected = format!("\n--- Context ---\n{}\n--- End Context ---\n", assembled);
            tabs.write_to_plugin_tab(&target, injected.as_bytes());
            log::info!(
                "Injected {} chars of context into {}",
                injected.len(),
                target
            );
        }

        self.context_picker.dismiss();
    }

    /// Export the session transcript for a plugin tab to a Markdown file.
    ///
    /// Copies the terminal scrollback as text and writes it to a file in the
    /// session data directory. If a clipboard is available, also copies the path.
    fn export_plugin_session(&mut self, plugin_id: &str) {
        let Some(tabs) = &self.tabs else {
            log::info!("No {} tab found for export", plugin_id);
            return;
        };
        let Some(idx) = tabs.find_plugin_tab_index(plugin_id) else {
            log::info!("No {} tab found for export", plugin_id);
            return;
        };

        // Try JSONL transcript export first (richer structured format)
        let transcript_path = tabs
            .iter()
            .find(|(i, _)| *i == idx)
            .and_then(|(_, t)| t.transcript.as_ref().map(|tr| tr.path().to_path_buf()));

        if let Some(ref path) = transcript_path {
            match crate::session::export::export_to_markdown(path) {
                Ok(md) => {
                    if let Some(base) = crate::session::sessions_base_dir() {
                        let _ = std::fs::create_dir_all(&base);
                        let id = crate::session::generate_session_id();
                        let filename = format!("{}-{}-export.md", plugin_id, id);
                        let dest = base.join(&filename);
                        match std::fs::write(&dest, &md) {
                            Ok(()) => {
                                log::info!("Exported transcript to {}", dest.display());
                                if let Some(clipboard) = &mut self.clipboard {
                                    let _ = clipboard.set_text(dest.display().to_string());
                                }
                            }
                            Err(e) => log::error!("Failed to write export: {}", e),
                        }
                        return;
                    }
                }
                Err(e) => log::debug!(
                    "Transcript export failed, falling back to scrollback: {}",
                    e
                ),
            }
        }

        // Fallback: export raw scrollback text
        let content = tabs.iter().find(|(i, _)| *i == idx).map(|(_, t)| {
            let lines = t.terminal.grid().export_scrollback_text();
            lines.join("\n")
        });

        if let Some(text) = content {
            if text.trim().is_empty() {
                log::info!("No session content to export for '{}'", plugin_id);
                return;
            }
            if let Some(base) = crate::session::sessions_base_dir() {
                let _ = std::fs::create_dir_all(&base);
                let id = crate::session::generate_session_id();
                let filename = format!("{}-{}-export.md", plugin_id, id);
                let path = base.join(&filename);
                let md = format!("# {} Session Export\n\n```\n{}\n```\n", plugin_id, text);
                match std::fs::write(&path, &md) {
                    Ok(()) => {
                        log::info!("Exported session to {}", path.display());
                        if let Some(clipboard) = &mut self.clipboard {
                            let _ = clipboard.set_text(path.display().to_string());
                        }
                    }
                    Err(e) => log::error!("Failed to export session: {}", e),
                }
            }
        }
    }

    /// Scroll the plugin tab to the last line containing an error-like pattern.
    ///
    /// First switches to the plugin tab (if not already active), then scans
    /// scrollback for error/failed/panic/exception patterns and scrolls there.
    fn jump_to_last_error(&mut self, plugin_id: &str) {
        let Some(tabs) = &mut self.tabs else { return };

        // Switch to the plugin tab so active_tab_mut() works on it
        if let Some(idx) = tabs.find_plugin_tab_index(plugin_id) {
            tabs.switch_to(idx);
        } else {
            log::info!("No '{}' tab found for jump-to-error", plugin_id);
            return;
        }

        if let Some(tab) = tabs.active_tab_mut() {
            let lines = tab.terminal.grid().export_scrollback_text();
            let error_pos = lines.iter().rposition(|line| {
                let lower = line.to_lowercase();
                lower.contains("error")
                    || lower.contains("failed")
                    || lower.contains("panic")
                    || lower.contains("exception")
            });
            if let Some(pos) = error_pos {
                let offset = lines.len().saturating_sub(pos);
                tab.terminal.grid_mut().set_scroll_offset(offset);
                log::info!("Jumped to error at line {} in '{}'", pos, plugin_id);
            } else {
                log::info!("No errors found in '{}' tab output", plugin_id);
            }
        }
    }

    /// Execute a list of plugin actions.
    fn execute_plugin_actions(&mut self, actions: Vec<crate::plugin::PluginAction>) {
        use crate::plugin::PluginAction;
        use crate::tabs::TabType;

        for action in actions {
            match action {
                PluginAction::OpenTab {
                    program,
                    args,
                    cwd,
                    title,
                    plugin_id,
                    env,
                } => {
                    // Proactively check if the binary actually works before spawning a tab.
                    // We try running it with --version; if the binary doesn't exist or
                    // immediately fails, we show an in-app overlay instead of opening
                    // a ghost tab that exits instantly.
                    let binary_works = std::process::Command::new(&program)
                        .arg("--version")
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);

                    if !binary_works {
                        self.info_overlay.show(
                            format!("Plugin: '{}' not available", program),
                            vec![
                                format!("The '{}' binary could not be found or", program),
                                "is not working correctly.".into(),
                                String::new(),
                                "Please install it and ensure it is in your PATH,".into(),
                                "then try again.".into(),
                            ],
                        );
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                        log::warn!(
                            "Plugin binary '{}' not available, showing info overlay",
                            program
                        );
                        continue;
                    }

                    let tab_type = plugin_id.map(|id| TabType::Plugin { plugin_id: id });
                    if let Some(tabs) = &mut self.tabs {
                        match tabs.new_tab_with_command(
                            &program,
                            &args,
                            cwd.as_deref(),
                            title.as_deref(),
                            tab_type,
                            env,
                        ) {
                            Ok(id) => {
                                log::info!("Plugin opened tab {} running '{}'", id, program);
                                self.dispatch_plugin_hook(crate::plugin::Hook::TabCreated {
                                    tab_id: id,
                                });
                            }
                            Err(e) => {
                                self.info_overlay.show(
                                    format!("Failed to launch '{}'", program),
                                    vec![format!("{}", e)],
                                );
                                if let Some(window) = &self.window {
                                    window.request_redraw();
                                }
                                log::error!("Plugin failed to open tab for '{}': {}", program, e);
                            }
                        }
                    }
                }
                PluginAction::WriteToPty { data } => {
                    self.write_to_pty(&data);
                }
                PluginAction::Notify { title, body } => {
                    self.notifications.notify(&title, &body);
                }
                PluginAction::ShowMessage { message } => {
                    log::info!("Plugin message: {}", message);
                }
                PluginAction::ShowNotice { title, lines } => {
                    log::info!("Plugin notice [{}]: {}", title, lines.join(" / "));
                    self.info_overlay.show(title, lines);
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                PluginAction::ClearPluginTab { plugin_id } => {
                    if let Some(tabs) = &mut self.tabs {
                        // Send Ctrl+C then clear
                        tabs.write_to_plugin_tab(&plugin_id, b"\x03");
                        tabs.write_to_plugin_tab(&plugin_id, b"clear\n");
                    }
                }
                PluginAction::CopyFromPluginTab { plugin_id, lines } => {
                    if let Some(tabs) = &self.tabs {
                        if let Some(text) = tabs.copy_from_plugin_tab(&plugin_id, lines) {
                            if let Some(clipboard) = &mut self.clipboard {
                                let _ = clipboard.set_text(text);
                                log::info!("Copied last {} lines from '{}' tab", lines, plugin_id);
                            }
                        }
                    }
                }
                PluginAction::RestartPlugin { plugin_id } => {
                    // Close existing plugin tab
                    if let Some(tabs) = &mut self.tabs {
                        if let Some(idx) = tabs.find_plugin_tab_index(&plugin_id) {
                            tabs.close_tab(idx);
                        }
                    }
                    // Re-open via plugin command
                    let open_cmd = format!("{}.open", plugin_id);
                    let ctx = self.build_plugin_context();
                    let actions = self.plugin_manager.execute_command(&open_cmd, &ctx);
                    if !actions.is_empty() {
                        self.execute_plugin_actions(actions);
                    }
                }
                PluginAction::ApplyDiff { diff_content, cwd } => {
                    self.run_git_apply("apply", &[], &diff_content, cwd.as_deref());
                }
                PluginAction::StageDiff { diff_content, cwd } => {
                    self.run_git_apply("stage", &["--cached"], &diff_content, cwd.as_deref());
                }
                PluginAction::RevertDiff { diff_content, cwd } => {
                    self.run_git_apply("revert", &["--reverse"], &diff_content, cwd.as_deref());
                }
                PluginAction::InjectContext { text } => {
                    self.inject_context_to_plugin(&text);
                }
                PluginAction::ToggleTranscript => {
                    if let Some(tabs) = &mut self.tabs {
                        if let Some(tab) = tabs.active_tab_mut() {
                            if let Some(ref mut session) = tab.ai_session {
                                session.toggle_recording();
                                if session.recording {
                                    // Start recording — create a new TranscriptRecorder
                                    if let Some(base) = crate::session::sessions_base_dir() {
                                        let transcript_dir = base.join("transcripts");
                                        let _ = std::fs::create_dir_all(&transcript_dir);
                                        match crate::session::transcript::TranscriptRecorder::new(
                                            &transcript_dir,
                                            session.repo_name.as_deref(),
                                            session.branch.as_deref(),
                                            &session.host.badge_label(),
                                            &session.profile,
                                        ) {
                                            Ok(recorder) => {
                                                let path = recorder.path().display().to_string();
                                                tab.transcript = Some(recorder);
                                                log::info!(
                                                    "Transcript recording started: {}",
                                                    path
                                                );
                                                self.info_overlay.show(
                                                    "Transcript Started".to_string(),
                                                    vec![
                                                        "Recording session output to:".into(),
                                                        format!("  {}", path),
                                                    ],
                                                );
                                            }
                                            Err(e) => {
                                                session.recording = false; // revert
                                                log::error!("Failed to start transcript: {}", e);
                                                self.info_overlay.show(
                                                    "Transcript Error".to_string(),
                                                    vec![format!(
                                                        "Failed to start recording: {}",
                                                        e
                                                    )],
                                                );
                                            }
                                        }
                                    }
                                } else {
                                    // Stop recording — drop the recorder
                                    if let Some(recorder) = tab.transcript.take() {
                                        let path = recorder.path().display().to_string();
                                        drop(recorder);
                                        log::info!("Transcript recording stopped: {}", path);
                                        self.info_overlay.show(
                                            "Transcript Stopped".to_string(),
                                            vec![
                                                "Transcript saved to:".into(),
                                                format!("  {}", path),
                                            ],
                                        );
                                    }
                                }
                            }
                        }
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                PluginAction::ListProfiles { plugin_id } => {
                    // Gather profile names from config
                    let profiles: Vec<String> = match plugin_id.as_str() {
                        "claude-code" => self
                            .config
                            .plugins
                            .claude_code
                            .profiles
                            .keys()
                            .cloned()
                            .collect(),
                        "codex" => self.config.plugins.codex.profiles.keys().cloned().collect(),
                        _ => Vec::new(),
                    };

                    if profiles.is_empty() {
                        self.info_overlay.show(
                            format!("No Profiles Configured"),
                            vec![
                                format!("No profiles found for '{}'.", plugin_id),
                                String::new(),
                                "Add profiles in config.toml under:".into(),
                                format!(
                                    "  [plugins.{}.profiles.<name>]",
                                    plugin_id.replace('-', "_")
                                ),
                            ],
                        );
                    } else {
                        let mut lines = vec!["Press a number key to select:".into(), String::new()];
                        for (i, name) in profiles.iter().enumerate() {
                            if i < 9 {
                                lines.push(format!("  {}  {}", i + 1, name));
                            }
                        }
                        // Also show current profile
                        if let Some(tabs) = &self.tabs {
                            if let Some(tab) = tabs.active_tab() {
                                if let Some(ref session) = tab.ai_session {
                                    lines.push(String::new());
                                    lines.push(format!("Current: {}", session.profile));
                                }
                            }
                        }
                        self.info_overlay
                            .show(format!("Select Profile — {}", plugin_id), lines);
                        self.pending_profile_selection = Some((plugin_id, profiles));
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                PluginAction::ApplyProfile {
                    plugin_id,
                    profile_name,
                } => {
                    if let Some(tabs) = &mut self.tabs {
                        if let Some(tab) = tabs.active_tab_mut() {
                            if let Some(ref mut session) = tab.ai_session {
                                session.profile = profile_name.clone();
                                log::info!("Profile for '{}' set to '{}'", plugin_id, profile_name);
                            }
                        }
                    }
                    self.info_overlay.show(
                        "Profile Applied".to_string(),
                        vec![
                            format!("Active profile: {}", profile_name),
                            String::new(),
                            "This profile will be used for the".into(),
                            "next session restart.".into(),
                        ],
                    );
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
        }
    }

    /// Run `git apply` with optional extra flags, piping diff_content via stdin.
    ///
    /// Change font size by delta
    /// Run `git apply` with optional extra args, piping diff_content to stdin.
    fn run_git_apply(
        &self,
        label: &str,
        extra_args: &[&str],
        diff_content: &str,
        cwd: Option<&str>,
    ) {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let mut cmd = Command::new("git");
        cmd.arg("apply");
        for arg in extra_args {
            cmd.arg(arg);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        match cmd.spawn() {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(diff_content.as_bytes());
                }
                match child.wait_with_output() {
                    Ok(output) if output.status.success() => {
                        log::info!("git {} succeeded", label);
                    }
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        log::error!("git {} failed: {}", label, stderr.trim());
                    }
                    Err(e) => {
                        log::error!("git {} wait failed: {}", label, e);
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to spawn git {}: {}", label, e);
            }
        }
    }

    /// Toggle transcript recording for the active plugin session
    fn toggle_transcript(&mut self) {
        self.execute_plugin_actions(vec![crate::plugin::PluginAction::ToggleTranscript]);
    }

    /// Inject context text into the active plugin tab
    fn inject_context_to_plugin(&mut self, text: &str) {
        if let Some(tabs) = &mut self.tabs {
            let injected = format!("\n--- Context ---\n{}\n--- End Context ---\n", text);
            tabs.write_to_plugin_tab("claude-code", injected.as_bytes());
            log::info!(
                "Injected {} chars of context into Claude Code",
                injected.len()
            );
        }
    }

    /// Get pane render info from the active tab's pane manager (if in pane mode)
    fn pane_render_info(&self) -> Vec<PaneRenderInfo> {
        let Some(tabs) = &self.tabs else {
            return Vec::new();
        };
        let Some(tab) = tabs.active_tab() else {
            return Vec::new();
        };
        let Some(ref pm) = tab.pane_manager else {
            return Vec::new();
        };
        let layout = pm.calculate_layout();
        let focused = pm.focused_id();
        layout
            .into_iter()
            .map(|(id, rect)| PaneRenderInfo {
                rect,
                is_focused: id == focused,
                id,
            })
            .collect()
    }

    /// Access the focused terminal through the TerminalContainer trait
    fn focused_terminal_via_container(&self) -> Option<&termeric_core::Terminal> {
        let tabs = self.tabs.as_ref()?;
        let tab = tabs.active_tab()?;
        // Try pane manager first (SplitContainer path)
        if let Some(ref pm) = tab.pane_manager {
            let sc: &dyn SplitContainer = pm;
            if !sc.is_closed() {
                return sc.focused_terminal();
            }
        }
        // Fall back to single-tab container
        let tc: &dyn TerminalContainer = tab;
        tc.focused_terminal()
    }

    /// Split the current tab into panes
    fn split_pane(&mut self, direction: SplitDirection) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let Some(tabs) = &mut self.tabs else { return };
        let Some(tab) = tabs.active_tab_mut() else {
            return;
        };

        let cols = tab.terminal.grid().cols();
        let rows = tab.terminal.grid().rows();
        let cell_width = renderer.cell_width();
        let cell_height = renderer.cell_height();
        let total_width = cols as f32 * cell_width;
        let total_height = rows as f32 * cell_height;

        if let Some(ref mut pm) = tab.pane_manager {
            // Already in pane mode — split the focused pane
            match pm.split(direction) {
                Ok(new_id) => {
                    log::info!("Split pane {:?}, created pane {:?}", direction, new_id);
                }
                Err(e) => log::error!("Failed to split pane: {}", e),
            }
        } else {
            // Create pane manager for the first split
            match PaneManager::new(
                cols,
                rows,
                self.config.shell.clone(),
                self.config.scrollback.lines,
                total_width,
                total_height,
                cell_width,
                cell_height,
            ) {
                Ok(mut pm) => {
                    // Swap pane 0's fresh terminal/pty with the tab's existing ones,
                    // so pane 0 inherits the tab's live session and content.
                    // The tab gets the fresh placeholder (dormant while in pane mode).
                    if let Some(pane0) = pm.get_pane_mut(PaneId(0)) {
                        std::mem::swap(&mut tab.terminal, &mut pane0.terminal);
                        std::mem::swap(&mut tab.pty, &mut pane0.pty);
                        std::mem::swap(&mut tab.pty_rx, &mut pane0.pty_rx);
                    }
                    match pm.split(direction) {
                        Ok(new_id) => {
                            log::info!("Entered pane mode, created pane {:?}", new_id);
                            tab.pane_manager = Some(pm);
                        }
                        Err(e) => log::error!("Failed initial pane split: {}", e),
                    }
                }
                Err(e) => log::error!("Failed to create pane manager: {}", e),
            }
        }

        // Request redraw to show new pane layout
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Run a sprout-menu action against the command anchored at
    /// `target_row` (falls back to the latest recorded cellar entry if
    /// row is unknown).
    fn execute_sprout_action(
        &mut self,
        action: crate::harvest::sprouts::SproutAction,
        target_row: Option<usize>,
    ) {
        use crate::harvest::sprouts::SproutAction;
        // Resolve the target command row. Prefer the explicit target, then
        // fall back to the latest cellar entry.
        let row = target_row.or_else(|| {
            self.harvest
                .as_ref()
                .and_then(|h| h.cellar.latest().map(|e| e.row))
        });
        let Some(row) = row else {
            self.toasts.warning("No command under cursor");
            return;
        };

        // Pull the command region from the active tab's shell integration.
        let region = {
            let Some(tabs) = &self.tabs else { return };
            let Some(tab) = tabs.active_tab() else { return };
            tab.terminal
                .shell_integration()
                .command_at(row as i32)
                .cloned()
        };
        let Some(region) = region else {
            self.toasts
                .warning("No shell-integration data for that command");
            return;
        };

        match action {
            SproutAction::CopyCommand => {
                let text = self.extract_command_text(&region);
                if text.trim().is_empty() {
                    self.toasts.warning("Command text is empty");
                    return;
                }
                self.copy_to_clipboard(text, "Copied command");
            }
            SproutAction::CopyOutput => {
                let text = self.extract_output_text(&region);
                if text.trim().is_empty() {
                    self.toasts.warning("Output is empty");
                    return;
                }
                self.copy_to_clipboard(text, "Copied output");
            }
            SproutAction::Rerun => {
                let text = self.extract_command_text(&region);
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    self.toasts.warning("Nothing to rerun");
                    return;
                }
                // Send the command + newline to the active PTY.
                let mut payload = trimmed.as_bytes().to_vec();
                payload.push(b'\n');
                self.write_to_pty(&payload);
                self.toasts.info("Re-ran command");
            }
            SproutAction::ScrollTo => {
                // Scroll the terminal so the prompt row is visible at the
                // top of the viewport (modulo scrollback mechanics).
                if let Some(terminal) = self.active_terminal_mut() {
                    let scrollback_len = terminal.grid().scrollback_len() as i32;
                    // `row` is an absolute prompt row from the time of
                    // capture. Convert it to a scroll offset; anything
                    // older than the current viewport requires a positive
                    // offset.
                    let rows_on_screen = terminal.rows() as i32;
                    let target = (row as i32).max(0);
                    let offset = (scrollback_len + rows_on_screen - 1 - target).max(0) as usize;
                    terminal.grid_mut().set_scroll_offset(offset);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            SproutAction::SearchOutput => {
                // Open search (scoping to output region is a follow-up —
                // for now we just activate the search bar so the user can
                // type a query over the full scrollback).
                if !self.search.is_active() {
                    self.search.start();
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            SproutAction::Explain => {
                // Hand the command off to the Claude Code plugin with a
                // short "explain this" preamble injected as context.
                let cmd = self.extract_command_text(&region);
                let out = self.extract_output_text(&region);
                if cmd.trim().is_empty() {
                    self.toasts.warning("No command to explain");
                    return;
                }
                let context = format!(
                    "Please explain this command and its output.\n\n$ {}\n\n{}",
                    cmd.trim(),
                    out.trim(),
                );
                self.execute_plugin_actions(vec![crate::plugin::PluginAction::InjectContext {
                    text: context,
                }]);
            }
        }
    }

    /// Copy `text` to the clipboard, emit a success toast with `label`.
    fn copy_to_clipboard(&mut self, text: String, label: &str) {
        if let Some(clipboard) = &mut self.clipboard {
            match clipboard.set_text(&text) {
                Ok(()) => {
                    self.copy_flash_start = Some(Instant::now());
                    self.toasts.success(label.to_string());
                }
                Err(e) => {
                    log::error!("Clipboard write failed: {}", e);
                    self.toasts.error(format!("Copy failed: {}", e));
                }
            }
        } else {
            self.toasts.warning("Clipboard not available");
        }
    }

    /// Extract just the command line entered by the user (between command
    /// start and output start).
    fn extract_command_text(&self, region: &termeric_core::CommandRegion) -> String {
        let Some(tabs) = &self.tabs else {
            return String::new();
        };
        let Some(tab) = tabs.active_tab() else {
            return String::new();
        };
        let start = region.command_row.unwrap_or(region.prompt_row) as i32;
        // End at the row just before output starts, or the prompt row if
        // output never began.
        let end = region
            .output_row
            .map(|r| r.saturating_sub(1) as i32)
            .unwrap_or(start);
        let end = end.max(start);
        tab.terminal.rows_text(start, end)
    }

    /// Extract the output block of a command (between output start and
    /// finish).
    fn extract_output_text(&self, region: &termeric_core::CommandRegion) -> String {
        let Some(tabs) = &self.tabs else {
            return String::new();
        };
        let Some(tab) = tabs.active_tab() else {
            return String::new();
        };
        let Some(start) = region.output_row else {
            return String::new();
        };
        let end = region
            .finish_row
            .map(|r| r.saturating_sub(1) as i32)
            .unwrap_or(start as i32);
        let start = start as i32;
        let end = end.max(start);
        tab.terminal.rows_text(start, end)
    }

    /// Move focus to the adjacent pane in `dir`. No-op if no pane manager
    /// or no pane in that direction; in the no-neighbor case we surface a
    /// short info toast so the user knows the key was registered but the
    /// layout has nowhere to go.
    fn navigate_focused_pane(&mut self, dir: crate::panes::PaneDir) {
        let moved = {
            let mut moved = false;
            if let Some(tabs) = &mut self.tabs {
                if let Some(tab) = tabs.active_tab_mut() {
                    if let Some(ref mut pm) = tab.pane_manager {
                        moved = pm.navigate(dir);
                    }
                }
            }
            moved
        };
        if !moved {
            use crate::panes::PaneDir;
            let label = match dir {
                PaneDir::Up => "No pane above",
                PaneDir::Down => "No pane below",
                PaneDir::Left => "No pane to the left",
                PaneDir::Right => "No pane to the right",
            };
            self.toasts.info(label);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Close the focused pane, collapsing back to a single-terminal tab if
    /// only one pane remains.
    fn close_focused_pane(&mut self) {
        if let Some(tabs) = &mut self.tabs {
            if let Some(tab) = tabs.active_tab_mut() {
                if let Some(ref mut pm) = tab.pane_manager {
                    pm.close_focused();
                    if pm.pane_count() <= 1 {
                        if let Some(remaining) = pm.take_remaining_pane() {
                            tab.terminal = remaining.terminal;
                            tab.pty = remaining.pty;
                            tab.pty_rx = remaining.pty_rx;
                        }
                        tab.pane_manager = None;
                        if let Err(e) = tab.resize(self.cols, self.rows) {
                            log::error!("Failed to resize tab after pane close: {}", e);
                        }
                    }
                }
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn change_font_size(&mut self, delta: f32) {
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let current = renderer.font_size();
        let new_size = (current + delta).clamp(8.0, 72.0);
        self.set_font_size(new_size);
    }

    /// Set font size to a specific value
    fn set_font_size(&mut self, size: f32) {
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
    fn set_font_family(&mut self, family: String) {
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
    fn set_line_height(&mut self, multiplier: f32) {
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
    fn set_padding(&mut self, padding: u32) {
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

    /// Load a theme from the config, returning it along with a human-readable
    /// error description if any of the attempted loads failed. The caller is
    /// expected to surface that description to the user.
    fn load_theme_from_config_with_error(config: &Config) -> (Theme, Option<String>) {
        if let Some(path) = &config.theme.path {
            match Theme::load(path) {
                Ok(theme) => return (theme, None),
                Err(err) => {
                    let msg = format!("Failed to load theme at {}: {}", path.display(), err);
                    log::warn!("{}", msg);
                    // Fall through to the named builtin lookup, but keep the
                    // message so the user sees the original failure.
                    match Theme::load_builtin(&config.theme.name) {
                        Ok(theme) => return (theme, Some(msg)),
                        Err(_) => return (Theme::default(), Some(msg)),
                    }
                }
            }
        }

        match Theme::load_builtin(&config.theme.name) {
            Ok(theme) => (theme, None),
            Err(err) => {
                let msg = format!("Failed to load theme '{}': {}", config.theme.name, err);
                log::warn!("{}", msg);
                (Theme::default(), Some(msg))
            }
        }
    }

    fn load_theme_from_config(config: &Config) -> Theme {
        Self::load_theme_from_config_with_error(config).0
    }

    /// Poll the config file's mtime; if it changed since our last load,
    /// reload + re-apply + toast. Called once per frame but only hits the
    /// filesystem at most every 750ms.
    fn poll_config_for_hot_reload(&mut self) {
        const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(750);
        if self.last_config_check.elapsed() < POLL_INTERVAL {
            return;
        }
        self.last_config_check = Instant::now();

        let path = crate::config::Config::config_path();
        let current_mtime = match std::fs::metadata(&path).and_then(|m| m.modified()) {
            Ok(t) => Some(t),
            Err(_) => None,
        };

        // Nothing to do if neither side has a timestamp or they match.
        if current_mtime == self.last_config_mtime {
            return;
        }
        self.last_config_mtime = current_mtime;

        // Only reload if the file exists (avoid racing with editors that
        // briefly delete + rewrite).
        if !path.exists() {
            return;
        }

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

    /// Validate and atomically select a new configuration before applying it
    /// to all live subsystems. Invalid reloads leave the previous config live.
    fn apply_config(&mut self, config: Config) -> std::result::Result<(), String> {
        config.validate().map_err(|error| format!("{error:#}"))?;
        self.config = config;
        self.apply_config_side_effects();
        Ok(())
    }

    /// Re-apply every runtime-owned piece of `self.config`. This is the single
    /// application path for startup, settings, reset, manual reload, and hot
    /// reload. Existing PTYs keep their process; shell changes affect new tabs.
    fn apply_config_side_effects(&mut self) {
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
        let (theme, theme_err) = Self::load_theme_from_config_with_error(&self.config);
        self.apply_theme(theme);
        if let Some(msg) = theme_err {
            // Keep it as a toast — startup shouldn't demand a modal dismiss.
            self.toasts.warning(format!("Theme: {}", msg));
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

    fn sync_plugins_from_config(&mut self) {
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

    fn sync_harvest_from_config(&mut self) {
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
            self.harvest_region_cursor = 0;
            self.harvest_active_prompt_row = None;
        }
    }

    fn reload_dynamic_keybindings(&mut self) {
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

    fn apply_theme(&mut self, theme: Theme) {
        self.settings_panel.set_theme_meta(theme.meta.clone());
        self.theme = theme;
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
    fn apply_settings_action(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::SetFontSize(size) => {
                self.set_font_size(size);
            }
            SettingsAction::SetFontFamily(family) => {
                self.set_font_family(family);
            }
            SettingsAction::SetTheme(name) => match Theme::load_builtin(&name) {
                Ok(theme) => {
                    let display = name.clone();
                    self.config.theme.name = name;
                    self.config.theme.path = None;
                    self.apply_theme(theme);
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
    fn apply_shortcuts_action(&mut self, action: ShortcutsAction) {
        match action {
            ShortcutsAction::SetShortcut { action, chord } => {
                self.shortcut_registry.set_binding(action, chord);
            }
            ShortcutsAction::None => {}
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
        if let Some(tabs) = &mut self.tabs {
            if let Some(tab) = tabs.active_tab_mut() {
                log::info!("Running custom command: {}", command);
                let cmd_with_newline = format!("{}\n", command);
                let _ = tab.write(cmd_with_newline.as_bytes());
            }
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

    /// Handle mouse input
    fn handle_mouse_input(
        &mut self,
        button: MouseButton,
        state: ElementState,
        position: (f64, f64),
    ) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let (cell_width, cell_height) = renderer.cell_size();
        let tab_bar_height = renderer.tab_bar_height();
        let chrome_bar_height = renderer.chrome_bar_height();
        let padding = renderer.padding();
        // Account for search bar height if visible
        let search_bar_height = if self.search.is_active() || self.search.has_results() {
            cell_height + 8.0 * 2.0 // Same calculation as renderer
        } else {
            0.0
        };
        let content_top = tab_bar_height + chrome_bar_height + search_bar_height;
        let scroll_offset = self
            .tabs
            .as_ref()
            .and_then(|tabs| {
                tabs.active_tab()
                    .map(|tab| tab.terminal.grid().scroll_offset())
            })
            .and_then(|offset| i32::try_from(offset).ok())
            .unwrap_or(0);
        let grid_pos = screen_to_grid(
            position.0,
            position.1,
            cell_width,
            cell_height,
            content_top, // Use content_top instead of just tab_bar_height
            padding,
            scroll_offset,
        );

        match state {
            ElementState::Pressed => {
                if button == MouseButton::Left {
                    // Check if click is on the scrollbar
                    if self.is_click_on_scrollbar(position.0, position.1) {
                        self.scrollbar_dragging = true;
                        self.scroll_to_scrollbar_position(position.1);
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                        return;
                    }

                    // Check if click is on a pane divider (for drag-to-resize)
                    if let Some(tabs) = &self.tabs {
                        if let Some(tab) = tabs.active_tab() {
                            if let Some(ref pm) = tab.pane_manager {
                                let local_x = position.0 as f32 - padding;
                                let local_y = position.1 as f32 - content_top;
                                if pm.hit_test_divider(local_x, local_y).is_some() {
                                    self.divider_dragging = true;
                                    if let Some(window) = &self.window {
                                        window.request_redraw();
                                    }
                                    return;
                                }
                            }
                        }
                    }

                    // Ctrl+Click to open hyperlinks
                    if self.modifiers.control_key() {
                        // Check if there's a hyperlink at this position
                        if let Some(tabs) = &self.tabs {
                            if let Some(tab) = tabs.active_tab() {
                                let grid = tab.terminal.grid();
                                // grid_pos.row is already scroll-adjusted
                                let row = grid_pos.row;
                                let col = grid_pos.col;

                                // For visible grid, row should be non-negative
                                if row >= 0 {
                                    if let Some(cell) = grid.cell(row as usize, col) {
                                        if let Some(hyperlink_id) = cell.hyperlink_id {
                                            if let Some(url) =
                                                tab.terminal.get_hyperlink_url(hyperlink_id)
                                            {
                                                log::info!("Opening hyperlink: {}", url);
                                                if let Err(e) =
                                                    crate::terminal::url::UrlDetector::open_url(url)
                                                {
                                                    log::error!("Failed to open URL: {}", e);
                                                }
                                                return;
                                            }
                                        }
                                    }

                                    // Fallback: check auto-detected URLs via UrlCache
                                    if row >= 0 {
                                        if let Some(detected) =
                                            self.url_cache.url_at(row as usize, col)
                                        {
                                            let url = &detected.url;
                                            log::info!("Opening auto-detected URL: {}", url);
                                            if let Err(e) =
                                                crate::terminal::url::UrlDetector::open_url(url)
                                            {
                                                log::error!("Failed to open URL: {}", e);
                                            }
                                            return;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Check for tab bar clicks (tabs, close buttons, + button, gear)
                    if let Some(renderer) = &self.renderer {
                        let px = position.0 as f32;
                        let py = position.1 as f32;

                        // If commands panel is open, intercept all clicks
                        if self.commands_panel.is_open() {
                            let window_size = self.window.as_ref().map(|w| w.inner_size());
                            if let Some(size) = window_size {
                                use crate::ui::commands::panel::CommandsPanelAction;
                                let info = self.commands_panel.build_info(&self.config.commands);
                                let action = renderer.commands_panel_hit_test(
                                    px,
                                    py,
                                    &self.config.commands,
                                    size.width as f32,
                                    size.height as f32,
                                    info.animation,
                                    &info.edit_mode,
                                );
                                match action {
                                    CommandsPanelAction::Run(idx) => {
                                        if let Some(cmd) = self.config.commands.get(idx) {
                                            let command = cmd.command.clone();
                                            self.commands_panel.close();
                                            self.run_custom_command(&command);
                                        }
                                    }
                                    CommandsPanelAction::Delete(idx) => {
                                        if idx < self.config.commands.len() {
                                            let name = self.config.commands[idx].name.clone();
                                            self.config.commands.remove(idx);
                                            log::info!("Deleted custom command '{}'", name);
                                            match self.save_config() {
                                                Ok(msg) => {
                                                    log::info!("{}", msg);
                                                    self.toasts.success(format!(
                                                        "Deleted command '{}'",
                                                        name
                                                    ));
                                                }
                                                Err(e) => {
                                                    log::error!(
                                                        "Deleted '{}' but save failed: {}",
                                                        name,
                                                        e
                                                    );
                                                    self.toasts
                                                        .error(format!("Save failed: {}", e));
                                                }
                                            }
                                        }
                                    }
                                    CommandsPanelAction::AddSubmit { name, command } => {
                                        if name == "SAVE" && command == "SAVE" {
                                            // Save button clicked
                                            if let Some(cmd) = self.commands_panel.try_submit() {
                                                let cmd_name = cmd.name.clone();
                                                self.config.commands.push(cmd);
                                                match self.save_config() {
                                                    Ok(msg) => {
                                                        log::info!("{}", msg);
                                                        self.toasts.success(format!(
                                                            "Saved command '{}'",
                                                            cmd_name
                                                        ));
                                                    }
                                                    Err(e) => {
                                                        log::error!(
                                                            "Failed to save custom command: {}",
                                                            e
                                                        );
                                                        self.toasts
                                                            .error(format!("Save failed: {}", e));
                                                    }
                                                }
                                            } else {
                                                // Form was incomplete — give feedback.
                                                self.toasts
                                                    .warning("Both name and command are required");
                                            }
                                        } else if name.is_empty() && command.is_empty() {
                                            // Click on "+ New Command" button => start editing
                                            self.commands_panel.start_add();
                                        } else {
                                            // Actually add the command
                                            self.config.commands.push(
                                                crate::config::CustomCommand {
                                                    name,
                                                    command,
                                                    alias: None,
                                                },
                                            );
                                            if let Ok(msg) = self.save_config() {
                                                log::info!("{}", msg);
                                            }
                                        }
                                    }
                                    CommandsPanelAction::ClickName => {
                                        self.commands_panel.focus_name();
                                    }
                                    CommandsPanelAction::ClickCommand => {
                                        self.commands_panel.focus_command();
                                    }
                                    CommandsPanelAction::Close => {
                                        self.commands_panel.close();
                                    }
                                    CommandsPanelAction::None => {}
                                }
                                if let Some(window) = &self.window {
                                    window.request_redraw();
                                }
                                return;
                            }
                        }

                        // If session management panel is open, intercept all clicks
                        if self.session_panel.is_open() {
                            let window_size = self.window.as_ref().map(|w| w.inner_size());
                            if let Some(size) = window_size {
                                use crate::session::panel::SessionPanelAction;
                                let action = renderer.session_panel_hit_test(
                                    px,
                                    py,
                                    self.session_panel.sessions(),
                                    size.width as f32,
                                    size.height as f32,
                                    self.session_panel.build_info().animation,
                                );
                                match action {
                                    SessionPanelAction::Load(name) => {
                                        use crate::session::restore::restore_named_session;
                                        match restore_named_session(&name) {
                                            Ok(Some(session)) => {
                                                if let Some(tabs) = &mut self.tabs {
                                                    match tabs.new_tab_from_session(&session) {
                                                        Ok(_) => log::info!(
                                                            "Loaded session '{}' from panel",
                                                            name
                                                        ),
                                                        Err(e) => log::error!(
                                                            "Failed to restore session: {}",
                                                            e
                                                        ),
                                                    }
                                                }
                                            }
                                            Ok(None) => {
                                                log::warn!("Session '{}' not found", name);
                                                self.toasts.warning(format!(
                                                    "Session '{}' not found",
                                                    name
                                                ));
                                            }
                                            Err(e) => {
                                                log::error!("Failed to load session: {}", e);
                                                self.toasts.error(format!("Load failed: {}", e));
                                            }
                                        }
                                        self.session_panel.close();
                                    }
                                    SessionPanelAction::Delete(name) => {
                                        match crate::session::delete_session(&name) {
                                            Ok(()) => {
                                                log::info!("Deleted session '{}'", name);
                                                self.toasts
                                                    .success(format!("Deleted session '{}'", name));
                                            }
                                            Err(e) => {
                                                log::error!(
                                                    "Failed to delete session '{}': {}",
                                                    name,
                                                    e
                                                );
                                                self.toasts.error(format!("Delete failed: {}", e));
                                            }
                                        }
                                        // Refresh the list
                                        self.session_panel.refresh_sessions();
                                    }
                                    SessionPanelAction::Close => {
                                        self.session_panel.close();
                                    }
                                    SessionPanelAction::None => {}
                                }
                                if let Some(window) = &self.window {
                                    window.request_redraw();
                                }
                                return;
                            }
                        }

                        // If session dropdown is open, intercept clicks anywhere
                        // (dropdown extends below tab bar)
                        if self.session_dropdown_open {
                            let window_size = self.window.as_ref().map(|w| w.inner_size());
                            if let Some(size) = window_size {
                                use crate::renderer::SessionDropdownClick;
                                let click = renderer.session_dropdown_hit_test(
                                    px,
                                    py,
                                    &self.session_dropdown_items,
                                    size.width as f32,
                                );
                                match click {
                                    SessionDropdownClick::LoadSession(name) => {
                                        use crate::session::restore::restore_named_session;
                                        match restore_named_session(&name) {
                                            Ok(Some(session)) => {
                                                if let Some(tabs) = &mut self.tabs {
                                                    match tabs.new_tab_from_session(&session) {
                                                        Ok(_) => {
                                                            log::info!("Loaded session '{}'", name)
                                                        }
                                                        Err(e) => log::error!(
                                                            "Failed to restore session: {}",
                                                            e
                                                        ),
                                                    }
                                                }
                                            }
                                            Ok(None) => log::warn!("Session '{}' not found", name),
                                            Err(e) => log::error!("Failed to load session: {}", e),
                                        }
                                        self.session_dropdown_open = false;
                                    }

                                    SessionDropdownClick::ManageSessions => {
                                        self.session_dropdown_open = false;
                                        self.session_panel.open();
                                    }
                                    SessionDropdownClick::Dismiss => {
                                        self.session_dropdown_open = false;
                                    }
                                    SessionDropdownClick::None => {}
                                }
                                if let Some(window) = &self.window {
                                    window.request_redraw();
                                }
                                return;
                            }
                        }

                        // Settings modal intercepts all clicks (inside: handle;
                        // outside: dismiss). Must come before tab bar / search bar
                        // so the modal can't be undercut by chrome under the scrim.
                        if self.settings_panel.is_active() {
                            let window_size = self.window.as_ref().map(|w| w.inner_size());
                            if let Some(size) = window_size {
                                let action = self.settings_panel.handle_click(
                                    px,
                                    py,
                                    &self.config,
                                    &self.shortcut_registry,
                                    size.width as f32,
                                    size.height as f32,
                                    cell_width,
                                    cell_height,
                                );
                                self.apply_settings_action(action);
                                if let Some(window) = &self.window {
                                    window.request_redraw();
                                }
                                return;
                            }
                        }

                        if py < renderer.tab_bar_height() {
                            let window_size = self.window.as_ref().map(|w| w.inner_size());
                            if let Some(size) = window_size {
                                use crate::renderer::TabBarClick;
                                let tab_infos =
                                    self.tabs.as_ref().map(|t| t.tab_info()).unwrap_or_default();
                                let click = renderer.tab_bar_hit_test(
                                    px,
                                    py,
                                    &tab_infos,
                                    size.width as f32,
                                );
                                match click {
                                    TabBarClick::Gear => {
                                        self.settings_panel.toggle();
                                    }
                                    TabBarClick::NewTab => {
                                        if let Some(tabs) = &mut self.tabs {
                                            match tabs.new_tab() {
                                                Ok(id) => {
                                                    log::info!("Created new tab {} via mouse", id);
                                                    self.maybe_show_splash(true);
                                                    self.dispatch_plugin_hook(
                                                        crate::plugin::Hook::TabCreated {
                                                            tab_id: id,
                                                        },
                                                    );
                                                }
                                                Err(e) => {
                                                    log::error!("Failed to create new tab: {}", e)
                                                }
                                            }
                                        }
                                    }
                                    TabBarClick::CloseTab(idx) => {
                                        let tab_id = self.tabs.as_ref().and_then(|t| {
                                            t.iter().find(|(i, _)| *i == idx).map(|(_, tab)| tab.id)
                                        });
                                        if let Some(tabs) = &mut self.tabs {
                                            tabs.close_tab(idx);
                                            if tabs.is_empty() {
                                                log::info!("Last tab closed, exiting");
                                                self.should_exit = true;
                                            }
                                        }
                                        if let Some(id) = tab_id {
                                            self.dispatch_plugin_hook(
                                                crate::plugin::Hook::TabClosed { tab_id: id },
                                            );
                                        }
                                    }
                                    TabBarClick::Tab(idx) => {
                                        let from = self
                                            .tabs
                                            .as_ref()
                                            .map(|t| t.active_index())
                                            .unwrap_or(0);
                                        if let Some(tabs) = &mut self.tabs {
                                            tabs.switch_to(idx);
                                        }
                                        if from != idx {
                                            self.dispatch_plugin_hook(
                                                crate::plugin::Hook::TabSwitched { from, to: idx },
                                            );
                                        }
                                    }
                                    TabBarClick::SessionDropdown => {
                                        self.session_dropdown_open = !self.session_dropdown_open;
                                        if self.session_dropdown_open {
                                            self.session_dropdown_items =
                                                crate::session::list_sessions_with_titles();
                                        }
                                    }
                                    TabBarClick::CommandsButton => {
                                        self.commands_panel.toggle();
                                    }
                                    TabBarClick::None => {}
                                }
                                if click != TabBarClick::None {
                                    if let Some(window) = &self.window {
                                        window.request_redraw();
                                    }
                                    return;
                                }
                            }
                        }
                    }

                    // Intercept clicks for search bar
                    if self.search.is_active() || self.search.has_results() {
                        if let Some(renderer) = &self.renderer {
                            let window_size = self.window.as_ref().map(|w| w.inner_size());
                            if let Some(size) = window_size {
                                use crate::renderer::SearchBarClick;
                                let click = renderer.search_bar_hit_test(
                                    position.0 as f32,
                                    position.1 as f32,
                                    size.width as f32,
                                    self.search.match_count(),
                                    self.search.current_match_index(),
                                    self.search.query().is_empty(),
                                    self.search.is_filter_mode(),
                                    self.search.matching_line_count(),
                                );
                                let grid_ref = self.tabs.as_ref().and_then(|tabs| {
                                    tabs.active_tab().map(|tab| tab.terminal.grid())
                                });

                                match click {
                                    SearchBarClick::ToggleCaseSensitive => {
                                        if let Some(grid) = grid_ref {
                                            self.search.toggle_case_sensitive(grid);
                                        }
                                        if let Some(window) = &self.window {
                                            window.request_redraw();
                                        }
                                        return;
                                    }
                                    SearchBarClick::ToggleRegex => {
                                        if let Some(grid) = grid_ref {
                                            self.search.toggle_regex(grid);
                                        }
                                        if let Some(window) = &self.window {
                                            window.request_redraw();
                                        }
                                        return;
                                    }
                                    SearchBarClick::ToggleFilter => {
                                        self.search.toggle_filter();
                                        if let Some(window) = &self.window {
                                            window.request_redraw();
                                        }
                                        return;
                                    }
                                    SearchBarClick::PrevMatch => {
                                        self.search.prev();
                                        self.scroll_to_current_match();
                                        if let Some(window) = &self.window {
                                            window.request_redraw();
                                        }
                                        return;
                                    }
                                    SearchBarClick::NextMatch => {
                                        self.search.next();
                                        self.scroll_to_current_match();
                                        if let Some(window) = &self.window {
                                            window.request_redraw();
                                        }
                                        return;
                                    }
                                    SearchBarClick::None => {}
                                }
                            }
                        }
                    }
                }
                // Focus pane under click in multi-pane mode
                if let Some(tabs) = &mut self.tabs {
                    if let Some(tab) = tabs.active_tab_mut() {
                        if let Some(ref mut pm) = tab.pane_manager {
                            if let Some(renderer) = &self.renderer {
                                let px = position.0 as f32;
                                let py = position.1 as f32;
                                let content_top =
                                    renderer.tab_bar_height() + renderer.chrome_bar_height();
                                let padding = renderer.padding();
                                let rel_x = px - padding;
                                let rel_y = py - content_top;
                                let layout = pm.calculate_layout();
                                for (id, rect) in &layout {
                                    if rel_x >= rect.x
                                        && rel_x < rect.x + rect.width
                                        && rel_y >= rect.y
                                        && rel_y < rect.y + rect.height
                                    {
                                        pm.focus(*id);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }

                if let Some(mut selection) =
                    self.mouse_state
                        .handle_press(button, grid_pos, self.modifiers)
                {
                    self.mouse_pressed = true;
                    // Double-click expands to the word under the cursor;
                    // triple-click expands to the full line. Simple/Block
                    // selections pass through unchanged.
                    if let Some(terminal) = self.active_terminal_mut() {
                        match selection.ty {
                            termeric_core::SelectionType::Semantic => {
                                let (start, end) = terminal.word_at(grid_pos);
                                selection.start = start;
                                selection.end = end;
                            }
                            termeric_core::SelectionType::Lines => {
                                let (start, end) = terminal.line_at(grid_pos);
                                selection.start = start;
                                selection.end = end;
                            }
                            _ => {}
                        }
                        terminal.set_selection(Some(selection.clone()));
                    }
                    // Keep mouse_state in sync so drags extend the expanded
                    // selection. Written after the terminal borrow ends.
                    self.mouse_state.selection = Some(selection);
                }
            }
            ElementState::Released => {
                if button == MouseButton::Left {
                    self.mouse_pressed = false;
                    self.scrollbar_dragging = false;
                    self.divider_dragging = false;
                }
                // Finalize selection on release
                let _ = self.mouse_state.handle_release(button);
                // Clear active selection state (but keep the selection visible)
                // The selection in terminal remains, but mouse_state no longer tracks it
            }
        }

        // Request redraw to show selection changes
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Handle mouse movement
    fn handle_mouse_move(&mut self, position: (f64, f64)) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let (cell_width, cell_height) = renderer.cell_size();
        let tab_bar_height = renderer.tab_bar_height();
        let chrome_bar_height = renderer.chrome_bar_height();
        let padding = renderer.padding();
        // Account for search bar height if visible
        let search_bar_height = if self.search.is_active() || self.search.has_results() {
            cell_height + 8.0 * 2.0 // Same calculation as renderer
        } else {
            0.0
        };
        let content_top = tab_bar_height + chrome_bar_height + search_bar_height;
        let scroll_offset = self
            .tabs
            .as_ref()
            .and_then(|tabs| {
                tabs.active_tab()
                    .map(|tab| tab.terminal.grid().scroll_offset())
            })
            .and_then(|offset| i32::try_from(offset).ok())
            .unwrap_or(0);
        let grid_pos = screen_to_grid(
            position.0,
            position.1,
            cell_width,
            cell_height,
            content_top,
            padding,
            scroll_offset,
        );

        // ── Tab bar hover detection for icon animations ──
        {
            let px = position.0 as f32;
            let py = position.1 as f32;
            let window_size = self.window.as_ref().map(|w| w.inner_size());
            let tab_infos = self.tabs.as_ref().map(|t| t.tab_info()).unwrap_or_default();
            let new_hover = if py < tab_bar_height {
                if let Some(renderer) = &self.renderer {
                    if let Some(size) = window_size {
                        let hit = renderer.tab_bar_hit_test(px, py, &tab_infos, size.width as f32);
                        if hit != crate::renderer::TabBarClick::None {
                            Some(hit)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };
            if new_hover != self.tab_bar_hover {
                self.tab_bar_hover = new_hover;
                self.tab_bar_hover_progress = 0.0;
            }
        }

        // Handle scrollbar dragging
        if self.scrollbar_dragging {
            self.scroll_to_scrollbar_position(position.1);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        // Handle pane divider dragging
        if self.divider_dragging {
            if let Some(tabs) = &mut self.tabs {
                if let Some(tab) = tabs.active_tab_mut() {
                    if let Some(ref mut pm) = tab.pane_manager {
                        let local_x = position.0 as f32 - padding;
                        let local_y = position.1 as f32 - content_top;
                        if pm.resize_divider_at(local_x, local_y, f32::MAX) {
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                        }
                    }
                }
            }
            return;
        }

        // Set resize cursor when hovering over pane dividers
        if let Some(tabs) = &self.tabs {
            if let Some(tab) = tabs.active_tab() {
                if let Some(ref pm) = tab.pane_manager {
                    let local_x = position.0 as f32 - padding;
                    let local_y = position.1 as f32 - content_top;
                    if let Some(is_vertical) = pm.hit_test_divider(local_x, local_y) {
                        if let Some(window) = &self.window {
                            let cursor = if is_vertical {
                                winit::window::CursorIcon::EwResize
                            } else {
                                winit::window::CursorIcon::NsResize
                            };
                            window.set_cursor(cursor);
                        }
                        return;
                    } else if let Some(window) = &self.window {
                        window.set_cursor(winit::window::CursorIcon::Default);
                    }
                }
            }
        }

        // Don't update selection when an overlay panel is open,
        // but DO track hover state for visual highlights.
        if self.settings_panel.is_active() {
            let px = position.0 as f32;
            let py = position.1 as f32;
            if let Some(renderer) = &self.renderer {
                let (cell_width, cell_height) = renderer.cell_size();
                if let Some(size) = self.window.as_ref().map(|w| w.inner_size()) {
                    let entries = self
                        .settings_panel
                        .build_entries(&self.config, &self.shortcut_registry);
                    let layout = crate::ui::settings::SettingsModalLayout::calculate(
                        size.width as f32,
                        size.height as f32,
                        cell_width,
                        cell_height,
                        self.settings_panel.animation_progress(),
                        entries.len(),
                    );

                    // Row hover (suppressed when the dropdown overlay is open
                    // so we don't hover-highlight rows behind it).
                    let hovered = if self.settings_panel.is_dropdown_open() {
                        None
                    } else {
                        layout.visible_row_at(py)
                    };
                    self.settings_panel.set_hover(hovered);

                    // Dropdown option hover (only when the overlay is open).
                    if self.settings_panel.is_dropdown_open() {
                        let dd_hover = self
                            .settings_panel
                            .dropdown_layout(&layout, &entries)
                            .filter(|(rect, _)| rect.contains(px, py))
                            .and_then(|(rect, option_count)| {
                                rect.option_at(py).filter(|&i| i < option_count)
                            });
                        self.settings_panel.set_dropdown_hover(dd_hover);
                    } else {
                        self.settings_panel.set_dropdown_hover(None);
                    }
                }
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        if self.commands_panel.is_open() {
            let px = position.0 as f32;
            let py = position.1 as f32;
            if let Some(renderer) = &self.renderer {
                let (ui_cw, cell_height) = renderer.cell_size();
                let window_size = self.window.as_ref().map(|w| w.inner_size());
                if let Some(size) = window_size {
                    let window_width = size.width as f32;
                    let anim = self.commands_panel.animation_progress();
                    let panel_width = (ui_cw * 36.0).max(340.0).min(window_width * 0.45);
                    let panel_x = window_width - panel_width * anim;
                    let padding = ui_cw * 1.8;

                    if px >= panel_x {
                        let header_height = cell_height * 2.8;
                        let btn_y = header_height + cell_height * 0.8;
                        let btn_height = cell_height * 2.0;

                        // Check "+ New Command" button hover
                        let hovered_add = py >= btn_y && py <= btn_y + btn_height;

                        // Calculate list start (skip form if editing)
                        let mut list_start_y = btn_y + btn_height + cell_height * 0.8;
                        if self.commands_panel.is_editing() {
                            let field_height = cell_height * 1.8;
                            let form_total_h = field_height * 2.0 + cell_height * 3.2;
                            list_start_y += form_total_h + cell_height * 0.5;
                        }
                        if !self.config.commands.is_empty() {
                            list_start_y += cell_height * 1.5;
                        }

                        let row_height = cell_height * 3.0;
                        let mut hovered_index = None;
                        let mut hovered_delete = None;
                        for i in 0..self.config.commands.len() {
                            let item_y = list_start_y + i as f32 * row_height;
                            if py >= item_y && py < item_y + row_height {
                                hovered_index = Some(i);
                                // Check delete button
                                let del_x = panel_x + panel_width - padding - ui_cw * 1.5;
                                if px >= del_x - ui_cw * 0.5 && px <= del_x + ui_cw * 1.5 {
                                    hovered_delete = Some(i);
                                }
                                break;
                            }
                        }
                        self.commands_panel
                            .set_hover(hovered_index, hovered_add, hovered_delete);
                    } else {
                        self.commands_panel.set_hover(None, false, None);
                    }
                }
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        if self.session_panel.is_open() {
            let px = position.0 as f32;
            let py = position.1 as f32;
            if let Some(renderer) = &self.renderer {
                let (ui_cw, cell_height) = renderer.cell_size();
                let window_size = self.window.as_ref().map(|w| w.inner_size());
                if let Some(size) = window_size {
                    let window_width = size.width as f32;
                    let anim = self.session_panel.animation_progress();
                    let panel_width = (ui_cw * 35.0).max(320.0).min(window_width * 0.45);
                    let panel_x = window_width - panel_width * anim;
                    let padding_y = cell_height * 0.5;
                    let row_height = (cell_height * 3.5).max(52.0);
                    let title_y = cell_height * 1.5;
                    let sep_y = title_y + cell_height * 2.0;
                    let list_start_y = sep_y + padding_y + 4.0;

                    let hovered = if px >= panel_x {
                        let sessions = self.session_panel.sessions();
                        let mut found = None;
                        for i in 0..sessions.len() {
                            let item_y = list_start_y + i as f32 * row_height;
                            if py >= item_y && py < item_y + row_height {
                                found = Some(i);
                                break;
                            }
                        }
                        found
                    } else {
                        None
                    };
                    self.session_panel.set_hover(hovered);
                }
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        if self.session_dropdown_open {
            let px = position.0 as f32;
            let py = position.1 as f32;
            if let Some(renderer) = &self.renderer {
                let window_size = self.window.as_ref().map(|w| w.inner_size());
                if let Some(size) = window_size {
                    let (cell_width, cell_height) = renderer.cell_size();
                    let tab_bar_height = renderer.tab_bar_height();
                    let window_width = size.width as f32;

                    let gear_button_width = tab_bar_height;
                    let session_button_width = tab_bar_height;
                    let dropdown_anchor_x = window_width - gear_button_width - session_button_width;

                    let row_height = (cell_height * 2.0).max(28.0);
                    let dropdown_width = (cell_width * 22.0).max(200.0);
                    let dropdown_x =
                        (dropdown_anchor_x + session_button_width - dropdown_width).max(0.0);
                    let dropdown_y = tab_bar_height;

                    let session_count = self.session_dropdown_items.len().min(5);
                    let display_rows = session_count.max(1);
                    let total_items = display_rows + 1; // + "Manage Sessions"
                    let dropdown_height = total_items as f32 * row_height + 4.0;

                    self.session_dropdown_hovered = if px >= dropdown_x
                        && px <= dropdown_x + dropdown_width
                        && py >= dropdown_y
                        && py <= dropdown_y + dropdown_height
                    {
                        let relative_y = py - dropdown_y - 2.0;
                        let row = (relative_y / row_height) as usize;
                        if row < total_items { Some(row) } else { None }
                    } else {
                        None
                    };
                }
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        // Only update selection when mouse is pressed (dragging)
        if self.mouse_pressed {
            if let Some(selection) = self.mouse_state.handle_move(grid_pos) {
                if let Some(terminal) = self.active_terminal_mut() {
                    terminal.set_selection(Some(selection));
                }
                // Request redraw for selection update
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        } else {
            // Just update position tracking when not dragging
            self.mouse_state.last_position = Some(grid_pos);
        }
    }

    /// Handle scroll wheel
    fn handle_scroll(&mut self, delta: f64) {
        // Filter out near-zero deltas (release artifacts from trackpad/mouse)
        // These appear as -0.000 or very small values on scroll release
        if delta.abs() < 0.001 {
            return;
        }

        if let Some(terminal) = self.active_terminal_mut() {
            let lines = delta.abs().max(1.0) as usize;
            let offset = terminal.grid().scroll_offset();

            let new_offset = if delta > 0.0 {
                offset + lines
            } else {
                offset.saturating_sub(lines)
            };

            terminal.grid_mut().set_scroll_offset(new_offset);

            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    /// Check if a click position is within the scrollbar area
    fn is_click_on_scrollbar(&self, x: f64, _y: f64) -> bool {
        let Some(renderer) = &self.renderer else {
            return false;
        };
        let (window_width, _) = renderer.window_size();

        // Scrollbar is 6px wide + 2px margin from right edge, with an expanded hit area
        let scrollbar_hit_width = 16.0_f32;
        let scrollbar_x = window_width as f32 - scrollbar_hit_width;

        // Check if there's actually scrollback content
        let has_scrollback = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.active_tab())
            .map(|tab| tab.terminal.grid().scrollback_len() > 0)
            .unwrap_or(false);

        has_scrollback && x as f32 >= scrollbar_x
    }

    /// Map a vertical mouse position to a scroll offset and apply it
    fn scroll_to_scrollbar_position(&mut self, y: f64) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let tab_bar_height = renderer.tab_bar_height();
        let padding = renderer.padding();
        let (_, window_height) = renderer.window_size();

        let search_bar_height = if self.search.is_active() || self.search.has_results() {
            let (_, cell_height) = renderer.cell_size();
            cell_height + 8.0 * 2.0
        } else {
            0.0
        };
        let content_top = tab_bar_height + search_bar_height;
        let scrollbar_top = content_top + padding;
        let scrollbar_bottom = window_height as f32 - padding;
        let scrollbar_height = scrollbar_bottom - scrollbar_top;

        if scrollbar_height <= 0.0 {
            return;
        }

        // Fraction from top (0.0=top, 1.0=bottom)
        let fraction = ((y as f32 - scrollbar_top) / scrollbar_height).clamp(0.0, 1.0);

        // Invert: top of scrollbar = max scroll, bottom = 0 scroll
        let scroll_fraction = 1.0 - fraction;

        if let Some(tabs) = &mut self.tabs {
            if let Some(tab) = tabs.active_tab_mut() {
                let max_offset = tab.terminal.grid().scrollback_len();
                let new_offset = (scroll_fraction * max_offset as f32) as usize;
                tab.terminal.grid_mut().set_scroll_offset(new_offset);
            }
        }
    }
}

fn cursor_opacity_at(blink: bool, elapsed_seconds: f32, period_seconds: f32) -> f32 {
    if !blink {
        return 1.0;
    }
    let phase = (elapsed_seconds / period_seconds) * std::f32::consts::TAU;
    0.65 + 0.35 * phase.cos()
}

/// Minimal RFC 4648 base64 encoder (standard alphabet) used for the OSC 52
/// clipboard reply. Keeps us off an external `base64` dependency.
fn base64_encode_standard(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
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
    use super::{base64_encode_standard, cursor_opacity_at};

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
