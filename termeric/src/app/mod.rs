//! Application state and event handling

mod event_loop;
pub mod state;

use anyhow::Result;
use std::sync::Arc;
use std::time::Instant;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::ActiveEventLoop,
    keyboard::ModifiersState,
    window::{Icon, Window, WindowId},
};

use termeric_core::TerminalEvent;

use crate::notifications::Notifications;
use crate::config::{Config, SplashMode};
use crate::harvest::HarvestMode;
use crate::input::{Action, MouseState, check_keybinding, key_to_bytes_with_modifiers, screen_to_grid};
use crate::input::keybindings::{KeyAction, KeyBindingManager};
use crate::renderer::Renderer;
use crate::terminal::search::Search;
use crate::ui::settings::{SettingsPanel, SettingsAction, SettingsKey};
use crate::terminal::splash::show_splash;
use self::state::AppState;
use crate::tabs::TabManager;
use crate::config::theme::Theme;
use crate::plugin::{PluginManager, PluginContext};
use crate::plugin::claude_code::ClaudeCodePlugin;
use crate::plugin::codex::CodexPlugin;
use crate::ui::command_palette::{CommandPalette, PaletteResult, CommandSource, RegisteredCommand};
use crate::ui::command_palette::palette::core_commands;
use crate::ui::info_overlay::InfoOverlay;
use crate::ui::diff_review::DiffReviewOverlay;
use crate::ui::context_picker::ContextPicker;
use crate::terminal::guardrails::GuardrailOverlay;

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
    /// Custom keybinding manager
    keybinding_manager: KeyBindingManager,
    /// Harvest mode (command visualization)
    harvest: Option<HarvestMode>,
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
    /// Info overlay (About/Version)
    info_overlay: InfoOverlay,
    /// Diff review overlay
    diff_review: DiffReviewOverlay,
    /// Context picker modal
    context_picker: ContextPicker,
    /// Guardrail confirmation overlay
    guardrail: GuardrailOverlay,
    /// Input line buffer for guardrail checking (accumulates until Enter)
    guardrail_input_buffer: String,
}

impl App {
    /// Create a new application instance
    pub fn new() -> Result<Self> {
        let config = Config::load_or_default()?;
        let default_font_size = config.font.size;
        let theme = Self::load_theme_from_config(&config);

        // Initialize clipboard (may fail on some systems)
        let clipboard = arboard::Clipboard::new().ok();

        // Initialize keybinding manager with custom bindings from config
        let mut keybinding_manager = KeyBindingManager::new();
        keybinding_manager.load(config.keybindings.clone());

        // Initialize harvest mode if enabled
        let harvest = if config.harvest.enabled {
            Some(HarvestMode::new())
        } else {
            None
        };

        let result: Result<Self> = Ok(Self {
            config,
            window: None,
            renderer: None,
            tabs: None,
            should_exit: false,
            cols: 80,
            rows: 24,
            cursor_blink_start: Instant::now(),
            cursor_blink_period: 1.0, // 1 second full cycle (fade out + fade in)
            cursor_display_col: 0.0,
            cursor_display_row: 0.0,
            last_frame_time: Instant::now(),
            last_cursor_visible: true,
            last_active_tab_id: None,
            max_seen_tab_id: None,
            mouse_state: MouseState::new(),
            modifiers: ModifiersState::empty(),
            clipboard,
            last_cursor_pos: None,
            mouse_pressed: false,
            scrollbar_dragging: false,
            default_font_size,
            copy_flash_start: None,
            window_focused: true,
            focus_change_time: Instant::now(),
            theme,
            settings_panel: SettingsPanel::new(),
            search: Search::new(),
            search_cursor_pos: 0,
            notifications: Notifications::new(),
            keybinding_manager,
            harvest,
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
            info_overlay: InfoOverlay::new(),
            diff_review: DiffReviewOverlay::new(),
            context_picker: ContextPicker::new(),
            guardrail: GuardrailOverlay::new(),
            guardrail_input_buffer: String::new(),
        });

        let mut app = result?;

        // Register enabled plugins
        let plugin_ctx = PluginContext {
            cwd: None,
            tab_count: 0,
            active_tab: 0,
            terminal_size: (app.cols, app.rows),
        };

        if app.config.plugins.claude_code.enabled {
            let plugin = ClaudeCodePlugin::new(app.config.plugins.claude_code.clone());
            if let Err(e) = app.plugin_manager.register(Box::new(plugin), &plugin_ctx) {
                log::error!("Failed to register Claude Code plugin: {}", e);
            }
        }

        if app.config.plugins.codex.enabled {
            let plugin = CodexPlugin::new(app.config.plugins.codex.clone());
            if let Err(e) = app.plugin_manager.register(Box::new(plugin), &plugin_ctx) {
                log::error!("Failed to register Codex plugin: {}", e);
            }
        }

        log::info!("Loaded {} plugin(s)", app.plugin_manager.count());

        // Register plugin keybindings with the keybinding manager
        let plugin_keybindings = app.plugin_manager.all_keybindings();
        if !plugin_keybindings.is_empty() {
            let mut all_bindings = app.config.keybindings.clone();
            for pk in &plugin_keybindings {
                all_bindings.push(crate::input::keybindings::KeyBinding {
                    key: pk.key.clone(),
                    modifiers: crate::input::keybindings::KeyModifiers {
                        ctrl: pk.ctrl,
                        alt: pk.alt,
                        shift: pk.shift,
                        super_key: false,
                    },
                    action: crate::input::keybindings::KeyAction::PluginCommand {
                        command_id: pk.command_id.clone(),
                    },
                });
            }
            app.keybinding_manager.load(all_bindings);
            log::info!("Registered {} plugin keybinding(s)", plugin_keybindings.len());
        }

        // Initialize command palette with core commands
        app.command_palette.register_commands(core_commands());

        // Register plugin commands in the palette
        let plugin_commands = app.plugin_manager.all_commands();
        if !plugin_commands.is_empty() {
            let keybindings = app.plugin_manager.all_keybindings();
            let palette_cmds: Vec<RegisteredCommand> = plugin_commands.iter().map(|pc| {
                let shortcut = keybindings.iter()
                    .find(|k| k.command_id == pc.id)
                    .map(|k| {
                        let mut parts = Vec::new();
                        if k.ctrl { parts.push("Ctrl".to_string()); }
                        if k.alt { parts.push("Alt".to_string()); }
                        if k.shift { parts.push("Shift".to_string()); }
                        parts.push(k.key.to_uppercase());
                        parts.join("+")
                    });
                let plugin_source = pc.id.split_once('.')
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
            }).collect();
            app.command_palette.register_commands(palette_cmds);
            log::info!("Registered {} plugin command(s) in palette", plugin_commands.len());
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
            if self.config.splash.mode == SplashMode::FirstBoot && !self.app_state.first_boot_complete {
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
        tabs.save_all_sessions(save_scrollback);
        log::info!("Sessions saved");
    }

    /// Process PTY events for all tabs
    fn process_pty_events(&mut self) {
        // Process events and collect results
        let (events, should_exit) = {
            let Some(tabs) = &mut self.tabs else { return };
            let (_, events) = tabs.process_all_events();
            let should_exit = tabs.cleanup_closed(self.config.session.save_scrollback) && tabs.is_empty();
            (events, should_exit)
        };

        // Handle terminal events (notifications, bells, etc.)
        for event in events {
            self.handle_terminal_event(event);
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
                // TODO: Visual bell flash, audio bell
                log::debug!("Bell event received");
            }
            TerminalEvent::Notification(req) => {
                let title = req.title.as_deref().unwrap_or("Terminal");
                self.notifications.notify(title, &req.body);
            }
            TerminalEvent::TitleChanged(_title) => {
                // Title is handled via shell cwd detection
            }
            TerminalEvent::ClipboardRequest(_) => {
                // TODO: Handle clipboard read request
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
                        if let Some(partial) = crate::git::detect_git_completion(&self.input_line_buffer) {
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
            let is_plugin_tab = self.tabs.as_ref()
                .and_then(|tabs| tabs.active_tab().map(|t| matches!(t.tab_type, crate::tabs::TabType::Plugin { .. })))
                .unwrap_or(false);

            if is_plugin_tab {
                let input = self.input_line_buffer.trim().to_string();
                if let Some(matched) = crate::terminal::guardrails::check_command(&input) {
                    log::warn!("Guardrail triggered: {} — {}", matched.command, matched.reason);
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
            let matched_command = self.config.commands.iter().find(|cmd| {
                cmd.name == first_word
                    || cmd.alias.as_deref() == Some(&first_word)
            }).map(|cmd| cmd.command.clone());

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

        if let Some(tabs) = &mut self.tabs {
            if let Some(tab) = tabs.active_tab_mut() {
                // Jump to bottom when user types while scrolled up
                tab.terminal.grid_mut().set_scroll_offset(0);

                if let Err(e) = tab.write(data) {
                    log::error!("Failed to write to PTY: {}", e);
                }
            }
        }
    }

    /// Handle window resize
    fn handle_resize(&mut self, width: u32, height: u32) {
        // Resize renderer
        if let Some(renderer) = &mut self.renderer {
            renderer.resize(width, height);

            // Calculate new terminal dimensions
            let (new_cols, new_rows) = renderer.calculate_grid_size(width, height);

            if new_cols != self.cols || new_rows != self.rows {
                self.cols = new_cols;
                self.rows = new_rows;

                // Resize all tabs
                if let Some(tabs) = &mut self.tabs {
                    tabs.resize(self.cols, self.rows);
                }

                log::debug!("Resized to {}x{}", self.cols, self.rows);
            }
        }
    }

    fn handle_scale_factor_change(&mut self, scale_factor: f64, new_inner_size: winit::dpi::PhysicalSize<u32>) {
        if let Some(renderer) = &mut self.renderer {
            renderer.set_scale_factor(scale_factor as f32);
            renderer.resize(new_inner_size.width, new_inner_size.height);

            let (new_cols, new_rows) = renderer.calculate_grid_size(new_inner_size.width, new_inner_size.height);
            if new_cols != self.cols || new_rows != self.rows {
                self.cols = new_cols;
                self.rows = new_rows;

                if let Some(tabs) = &mut self.tabs {
                    tabs.resize(self.cols, self.rows);
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
        let working_dir = self.tabs.as_ref()
            .and_then(|t| t.active_tab())
            .and_then(|tab| tab.working_directory());
        self.git_watcher.update(working_dir.as_deref());

        // Update AI session chrome bar for plugin tabs
        let chrome_bar_data = {
            let branch = self.git_watcher.branch().map(|s| s.to_string());
            let dirty = self.git_watcher.is_dirty();
            let wd = working_dir.clone();

            if let Some(tabs) = &mut self.tabs {
                if let Some(tab) = tabs.active_tab_mut() {
                    if let Some(ref mut session) = tab.ai_session {
                        session.update_git(branch.as_deref(), dirty, wd.as_deref());
                        Some(crate::ui::claude_chrome::ChromeBarInfo {
                            plugin_id: session.plugin_id.clone(),
                            display_name: session.display_name.clone(),
                            repo_name: session.repo_name.clone(),
                            branch: session.branch.clone(),
                            dirty: session.dirty,
                            host_label: session.host.badge_label(),
                            profile: session.profile.clone(),
                            recording: session.recording,
                            approval_mode: session.approval_mode.clone(),
                            height: 0.0, // set by renderer
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

        // Get cursor target position and calculate animations before borrowing
        let cursor_opacity = self.cursor_opacity();
        let copy_flash = self.copy_flash_intensity();
        let focus_dim = self.focus_dim();

        // Get cursor target position and active tab ID from grid
        let cursor_info = self.tabs
            .as_ref()
            .and_then(|tabs| tabs.active_tab().map(|tab| (tab, tabs)))
            .map(|(tab, _tabs)| {
                let cursor = tab.terminal.grid().cursor();
                (cursor.col, cursor.row, cursor.visible, tab.id)
            });

        // Update cursor animation if we have a target
        let cursor_display_pos = if let Some((col, row, visible, tab_id)) = cursor_info {
            self.update_cursor_animation(col, row, visible, Some(tab_id))
        } else {
            // No active tab - snap to origin and mark as not visible
            self.update_cursor_animation(0, 0, false, None)
        };

        let Some(renderer) = &mut self.renderer else { return };
        let Some(tabs) = &self.tabs else { return };
        let Some(tab) = tabs.active_tab() else { return };

        let terminal = &tab.terminal;
        let tab_info = tabs.tab_info();


        // Build settings panel info if visible
        let settings_info = if self.settings_panel.is_visible() {
            Some(self.settings_panel.build_info(&self.config))
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
        let image_data: Vec<_> = terminal.graphics_placements()
            .iter()
            .filter_map(|p| terminal.get_image(p.image_id).map(|img| (p, img)))
            .collect();
        let images = if image_data.is_empty() { None } else { Some(image_data.as_slice()) };

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

        // Build info overlay if visible
        let info_overlay_data = if self.info_overlay.is_visible() {
            Some(crate::renderer::InfoOverlayInfo {
                title: self.info_overlay.title.clone(),
                lines: self.info_overlay.lines.clone(),
                animation: self.info_overlay.animation(),
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

        // Build diff review overlay if visible
        let diff_review_data = if self.diff_review.is_visible() {
            self.diff_review.diff.as_ref().map(|diff| {
                use crate::terminal::diff_detector::DiffLine;
                let mut lines = Vec::new();
                for hunk in &diff.hunks {
                    lines.push(crate::renderer::DiffReviewLine::Header(hunk.header.clone()));
                    for dl in &hunk.lines {
                        lines.push(match dl {
                            DiffLine::Context(s) => crate::renderer::DiffReviewLine::Context(s.clone()),
                            DiffLine::Added(s) => crate::renderer::DiffReviewLine::Added(s.clone()),
                            DiffLine::Removed(s) => crate::renderer::DiffReviewLine::Removed(s.clone()),
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
                sources: self.context_picker.sources.iter().map(|s| {
                    crate::renderer::ContextPickerSource {
                        label: s.label.clone(),
                        selected: s.selected,
                        available: s.available,
                    }
                }).collect(),
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
            self.guardrail.matched.as_ref().map(|m| {
                crate::renderer::GuardrailInfo {
                    command: m.command.clone(),
                    reason: m.reason.clone(),
                    is_critical: m.severity == crate::terminal::guardrails::Severity::Critical,
                    animation: self.guardrail.animation(),
                    target: m.target.clone(),
                    user: m.user.clone(),
                }
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
        renderer.set_git_completions(if self.git_completions_visible && !self.git_completions.is_empty() {
            Some(crate::renderer::GitCompletionInfo {
                completions: self.git_completions.clone(),
                selected_index: self.git_completion_index,
            })
        } else {
            None
        });
        match renderer.render(terminal.grid(), terminal.selection(), cursor_opacity, cursor_display_pos, &tab_info, copy_flash, focus_dim, settings_info.as_ref(), search_info.as_ref(), &self.search, images, session_dropdown_info.as_ref(), session_panel_info.as_ref(), commands_panel_info.as_ref(), palette_info.as_ref()) {
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
    fn active_terminal_mut(&mut self) -> Option<&mut termeric_core::Terminal> {
        self.tabs.as_mut()?.active_tab_mut().map(|t| &mut t.terminal)
    }

    /// Calculate cursor opacity for smooth blink animation
    /// Returns a value between 0.0 (invisible) and 1.0 (fully visible)
    fn cursor_opacity(&self) -> f32 {
        let elapsed = self.cursor_blink_start.elapsed().as_secs_f32();
        let phase = (elapsed / self.cursor_blink_period) * std::f32::consts::TAU;
        // Use cosine for smooth fade: cos goes from 1 to -1
        // We want 1.0 (visible) -> 0.3 (dim but visible) -> 1.0
        // Map cos [-1, 1] to [0.3, 1.0] for a subtle blink
        let cos_val = phase.cos();
        0.65 + 0.35 * cos_val // Range: 0.3 to 1.0
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
            self.max_seen_tab_id = Some(
                self.max_seen_tab_id.map_or(current, |max| max.max(current))
            );
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

    /// Handle a custom keybinding action
    /// Returns true if the action was handled
    fn handle_key_action(&mut self, key_action: KeyAction) -> bool {
        // Try to convert to internal Action first
        if let Some(action) = key_action.to_action() {
            self.handle_action(action);
            return true;
        }

        // Handle special actions that don't map to Action
        match key_action {
            KeyAction::SendText { text } => {
                self.write_to_pty(text.as_bytes());
                true
            }
            KeyAction::CommandPalette => {
                // Command palette removed — no-op
                false
            }
            KeyAction::RunCommand { command } => {
                // TODO: Implement running command in new pane when split panes are ready
                log::info!("Run command requested: {}", command);
                false
            }
            // Split pane actions - will be handled when implemented
            KeyAction::SplitVertical
            | KeyAction::SplitHorizontal
            | KeyAction::NavigatePane { .. }
            | KeyAction::ClosePane => {
                log::debug!("Split pane action not yet implemented");
                false
            }
            // These should have been handled by to_action()
            _ => false,
        }
    }

    /// Handle an action triggered by a keybinding.
    /// Returns true if the action was handled and the key should be consumed.
    fn handle_action(&mut self, action: Action) -> bool {
        match action {
            Action::Copy => {
                self.copy_selection();
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
                    terminal.grid_mut().set_scroll_offset(offset.saturating_sub(lines));
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
                            // Show splash for NewTab mode
                            self.maybe_show_splash(true);
                        }
                        Err(e) => log::error!("Failed to create new tab: {}", e),
                    }
                }
            }
            Action::CloseTab => {
                if let Some(tabs) = &mut self.tabs {
                    tabs.close_active_tab();
                    if tabs.is_empty() {
                        log::info!("Last tab closed, exiting");
                        self.should_exit = true;
                    }
                }
            }
            Action::NextTab => {
                if let Some(tabs) = &mut self.tabs {
                    tabs.next_tab();
                }
            }
            Action::PrevTab => {
                if let Some(tabs) = &mut self.tabs {
                    tabs.prev_tab();
                }
            }
            Action::SwitchToTab(index) => {
                if let Some(tabs) = &mut self.tabs {
                    tabs.switch_to(index);
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
                            terminal.grid_mut().set_scroll_offset(current_offset.saturating_sub(diff));
                        }
                        log::debug!("Navigate to next prompt at row {}", row);
                    }
                }
            }
            Action::None => {}
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
                    "core.save-config" => {
                        if let Ok(msg) = self.save_config() {
                            log::info!("{}", msg);
                        }
                        return;
                    }
                    "core.reload-config" => {
                        if let Ok(config) = crate::config::Config::load_or_default() {
                            self.config = config;
                            log::info!("Configuration reloaded");
                        }
                        return;
                    }
                    "core.reset-config" => {
                        self.config = crate::config::Config::default();
                        if let Ok(msg) = self.save_config() {
                            log::info!("Config reset: {}", msg);
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
                    id if id.starts_with("core.theme.") => {
                        let theme_name = id.strip_prefix("core.theme.").unwrap_or("");
                        if let Ok(theme) = Theme::load_builtin(theme_name) {
                            self.config.theme.name = theme_name.to_string();
                            self.apply_theme(theme);
                            if let Ok(msg) = self.save_config() {
                                log::info!("Theme changed to {}: {}", theme_name, msg);
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
        let cwd = self.tabs.as_ref()
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
                    log::info!("Profile selection not yet implemented — use config.toml profiles");
                    return;
                }
                _ => {}
            }
        }
        let ctx = self.build_plugin_context();
        if let Some(action) = self.plugin_manager.execute_command(command_id, &ctx) {
            self.execute_plugin_actions(vec![action]);
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
            tabs.plugin_tab_cwd(plugin_id)
                .or_else(|| tabs.iter().find(|(_, t)| !t.closed).and_then(|(_, t)| t.working_directory()))
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
        let target = self.context_picker.target_plugin_id.clone()
            .unwrap_or_else(|| "claude-code".to_string());
        if let Some(tabs) = &mut self.tabs {
            // Wrap in a clear marker so the AI tool can parse it
            let injected = format!("\n--- Context ---\n{}\n--- End Context ---\n", assembled);
            tabs.write_to_plugin_tab(&target, injected.as_bytes());
            log::info!("Injected {} chars of context into {}", injected.len(), target);
        }

        self.context_picker.dismiss();
    }

    /// Export the session transcript for a plugin tab to a Markdown file.
    ///
    /// Copies the terminal scrollback as text and writes it to a file in the
    /// session data directory. If a clipboard is available, also copies the path.
    fn export_plugin_session(&mut self, plugin_id: &str) {
        let content = if let Some(tabs) = &self.tabs {
            tabs.find_plugin_tab_index(plugin_id).and_then(|idx| {
                tabs.iter()
                    .find(|(i, _)| *i == idx)
                    .map(|(_, t)| {
                        let lines = t.terminal.grid().export_scrollback_text();
                        lines.join("\n")
                    })
            })
        } else {
            None
        };

        if let Some(text) = content {
            if text.trim().is_empty() {
                log::info!("No session content to export for '{}'", plugin_id);
                return;
            }
            // Write to a file in the sessions directory
            if let Some(base) = crate::session::sessions_base_dir() {
                let _ = std::fs::create_dir_all(&base);
                let id = crate::session::generate_session_id();
                let filename = format!("{}-{}-export.md", plugin_id, id);
                let path = base.join(&filename);
                let md = format!(
                    "# {} Session Export\n\n```\n{}\n```\n",
                    plugin_id, text
                );
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
        } else {
            log::info!("No {} tab found for export", plugin_id);
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
                PluginAction::OpenTab { program, args, cwd, title, plugin_id, env } => {
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
                            }
                            Err(e) => log::error!("Plugin failed to open tab for '{}': {}", program, e),
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
                    if let Some(action) = self.plugin_manager.execute_command(&open_cmd, &ctx) {
                        self.execute_plugin_actions(vec![action]);
                    }
                }
                PluginAction::ToggleTranscript => {
                    if let Some(tabs) = &mut self.tabs {
                        if let Some(tab) = tabs.active_tab_mut() {
                            if let Some(ref mut session) = tab.ai_session {
                                session.toggle_recording();
                                log::info!("Transcript recording: {}", if session.recording { "started" } else { "stopped" });
                            }
                        }
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
                    if let Some(tabs) = &mut self.tabs {
                        if let Some(tab) = tabs.active_tab_mut() {
                            if matches!(tab.tab_type, crate::tabs::TabType::Plugin { .. }) {
                                let injected = format!(
                                    "\n--- Context ---\n{}\n--- End Context ---\n",
                                    text
                                );
                                let _ = tab.write(injected.as_bytes());
                                log::info!("Injected {} chars of context into active plugin tab", text.len());
                            } else {
                                log::warn!("InjectContext: active tab is not a plugin tab");
                            }
                        }
                    }
                }
            }
        }
    }

    /// Run `git apply` with optional extra flags, piping diff_content via stdin.
    ///
    /// `label` is used for log messages (e.g. "apply", "stage", "revert").
    /// `extra_args` are appended to `git apply` (e.g. `["--cached"]` or `["--reverse"]`).
    fn run_git_apply(&self, label: &str, extra_args: &[&str], diff_content: &str, cwd: Option<&str>) {
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

    /// Apply a diff via `git apply`
    fn apply_diff_via_git(&mut self, diff_content: &str, cwd: Option<&str>, staged: bool) {
        use std::process::Command;
        let mut cmd = Command::new("git");
        cmd.arg("apply");
        if staged {
            cmd.arg("--cached");
        }
        cmd.stdin(std::process::Stdio::piped());
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        match cmd.spawn() {
            Ok(mut child) => {
                if let Some(stdin) = child.stdin.as_mut() {
                    use std::io::Write;
                    let _ = stdin.write_all(diff_content.as_bytes());
                }
                match child.wait() {
                    Ok(status) if status.success() => {
                        let action = if staged { "staged" } else { "applied" };
                        log::info!("Diff {} successfully", action);
                        self.notifications.notify("Diff Applied", &format!("Diff {} successfully", action));
                    }
                    Ok(status) => {
                        log::error!("git apply failed with status: {}", status);
                        self.notifications.notify("Diff Failed", "git apply returned an error");
                    }
                    Err(e) => {
                        log::error!("Failed to wait for git apply: {}", e);
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to spawn git apply: {}", e);
                self.notifications.notify("Diff Failed", &format!("Failed to run git: {}", e));
            }
        }
    }

    /// Revert a diff via `git apply --reverse`
    fn revert_diff_via_git(&mut self, diff_content: &str, cwd: Option<&str>) {
        use std::process::Command;
        let mut cmd = Command::new("git");
        cmd.args(["apply", "--reverse"]);
        cmd.stdin(std::process::Stdio::piped());
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        match cmd.spawn() {
            Ok(mut child) => {
                if let Some(stdin) = child.stdin.as_mut() {
                    use std::io::Write;
                    let _ = stdin.write_all(diff_content.as_bytes());
                }
                match child.wait() {
                    Ok(status) if status.success() => {
                        log::info!("Diff reverted successfully");
                        self.notifications.notify("Diff Reverted", "Changes have been undone");
                    }
                    Ok(status) => {
                        log::error!("git apply --reverse failed with status: {}", status);
                        self.notifications.notify("Revert Failed", "git apply --reverse returned an error");
                    }
                    Err(e) => {
                        log::error!("Failed to wait for git apply: {}", e);
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to spawn git apply: {}", e);
                self.notifications.notify("Revert Failed", &format!("Failed to run git: {}", e));
            }
        }
    }

    /// Toggle transcript recording for the active Claude session
    fn toggle_transcript(&mut self) {
        // TODO: Implement transcript toggle when transcript system is added
        log::info!("Transcript toggle requested (not yet implemented)");
    }

    /// Inject context text into the active plugin tab
    fn inject_context_to_plugin(&mut self, text: &str) {
        if let Some(tabs) = &mut self.tabs {
            let injected = format!("\n--- Context ---\n{}\n--- End Context ---\n", text);
            tabs.write_to_plugin_tab("claude-code", injected.as_bytes());
            log::info!("Injected {} chars of context into Claude Code", injected.len());
        }
    }

    /// Change font size by delta
    fn change_font_size(&mut self, delta: f32) {
        let Some(renderer) = &mut self.renderer else { return };
        let current = renderer.font_size();
        let new_size = (current + delta).clamp(8.0, 72.0);
        self.set_font_size(new_size);
    }

    /// Set font size to a specific value
    fn set_font_size(&mut self, size: f32) {
        let Some(renderer) = &mut self.renderer else { return };
        let Some(window) = &self.window else { return };

        self.config.font.size = size;
        renderer.set_font_size(size);

        // Recalculate grid size after font change
        let window_size = window.inner_size();
        let (new_cols, new_rows) = renderer.calculate_grid_size(window_size.width, window_size.height);

        if new_cols != self.cols || new_rows != self.rows {
            self.cols = new_cols;
            self.rows = new_rows;

            // Resize all tabs
            if let Some(tabs) = &mut self.tabs {
                tabs.resize(self.cols, self.rows);
            }

            log::info!("Font size set to {}, grid resized to {}x{}", size, self.cols, self.rows);
        }

        // Request redraw
        window.request_redraw();
    }

    /// Set font family to a specific value
    fn set_font_family(&mut self, family: String) {
        let Some(renderer) = &mut self.renderer else { return };
        let Some(window) = &self.window else { return };

        self.config.font.family = family.clone();
        renderer.set_font_family(family);

        let window_size = window.inner_size();
        let (new_cols, new_rows) = renderer.calculate_grid_size(window_size.width, window_size.height);

        if new_cols != self.cols || new_rows != self.rows {
            self.cols = new_cols;
            self.rows = new_rows;

            if let Some(tabs) = &mut self.tabs {
                tabs.resize(self.cols, self.rows);
            }

            log::info!("Font family updated, grid resized to {}x{}", self.cols, self.rows);
        }

        window.request_redraw();
    }

    /// Set padding around terminal content
    fn set_padding(&mut self, padding: u32) {
        let Some(renderer) = &mut self.renderer else { return };
        let Some(window) = &self.window else { return };

        self.config.window.padding = padding;
        renderer.set_padding(padding as f32);

        let window_size = window.inner_size();
        let (new_cols, new_rows) = renderer.calculate_grid_size(window_size.width, window_size.height);

        if new_cols != self.cols || new_rows != self.rows {
            self.cols = new_cols;
            self.rows = new_rows;

            if let Some(tabs) = &mut self.tabs {
                tabs.resize(self.cols, self.rows);
            }

            log::info!("Padding updated, grid resized to {}x{}", self.cols, self.rows);
        }

        window.request_redraw();
    }

    fn load_theme_from_config(config: &Config) -> Theme {
        if let Some(path) = &config.theme.path {
            match Theme::load(path) {
                Ok(theme) => return theme,
                Err(err) => {
                    log::warn!("Failed to load theme at {}: {}", path.display(), err);
                }
            }
        }

        match Theme::load_builtin(&config.theme.name) {
            Ok(theme) => theme,
            Err(err) => {
                log::warn!("Failed to load theme '{}': {}", config.theme.name, err);
                Theme::default()
            }
        }
    }

    fn apply_theme(&mut self, theme: Theme) {
        self.theme = theme;
        if let Some(renderer) = &mut self.renderer {
            renderer.set_theme(&self.theme);
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
            SettingsAction::SetTheme(name) => {
                if let Ok(theme) = Theme::load_builtin(&name) {
                    self.config.theme.name = name;
                    self.config.theme.path = None;
                    self.apply_theme(theme);
                }
            }
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
            }
            SettingsAction::SetAudioBell(enabled) => {
                self.config.bell.audio = enabled;
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
            }
            SettingsAction::SetSplashMode(mode) => {
                self.config.splash.mode = mode;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            SettingsAction::SaveConfig => {
                match self.save_config() {
                    Ok(path) => log::info!("Config saved to: {}", path),
                    Err(e) => log::error!("Failed to save config: {}", e),
                }
            }
            SettingsAction::ResetConfig => {
                self.config = Config::default();
                self.default_font_size = self.config.font.size;
                self.set_font_family(self.config.font.family.clone());
                self.set_font_size(self.config.font.size);
                self.set_padding(self.config.window.padding);
                let theme = Self::load_theme_from_config(&self.config);
                self.apply_theme(theme);
                if let Some(tabs) = &mut self.tabs {
                    tabs.set_scrollback_limit(self.config.scrollback.lines);
                }
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
            SettingsAction::None => {}
        }
    }

    /// Copy selection to clipboard
    fn copy_selection(&mut self) {
        let Some(tabs) = &self.tabs else { return };
        let Some(tab) = tabs.active_tab() else { return };
        let Some(text) = tab.terminal.get_selection_text() else { return };

        if let Some(clipboard) = &mut self.clipboard {
            if let Err(e) = clipboard.set_text(&text) {
                log::error!("Failed to copy to clipboard: {}", e);
            } else {
                log::debug!("Copied {} bytes to clipboard", text.len());
                // Start copy flash animation
                self.copy_flash_start = Some(Instant::now());
            }
        }
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
        let Some(clipboard) = &mut self.clipboard else { return };

        match clipboard.get_text() {
            Ok(text) => {
                self.write_to_pty(text.as_bytes());
            }
            Err(e) => {
                log::error!("Failed to paste from clipboard: {}", e);
            }
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
        let grid_ref = self.tabs
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
                        if let Some((prev_pos, _)) = query[..self.search_cursor_pos]
                            .char_indices()
                            .next_back()
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
                    if let Some((prev_pos, _)) = query[..self.search_cursor_pos]
                        .char_indices()
                        .next_back()
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
        let Some(search_match) = self.search.current() else { return };
        let match_row = search_match.row;

        let Some(tabs) = &mut self.tabs else { return };
        let Some(tab) = tabs.active_tab_mut() else { return };

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

    /// Save current configuration to file
    fn save_config(&self) -> Result<String> {
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

        let config_content = toml::to_string_pretty(&config)?;

        std::fs::write(&config_path, config_content)?;
        Ok(config_path.display().to_string())
    }

    /// Handle mouse input
    fn handle_mouse_input(&mut self, button: MouseButton, state: ElementState, position: (f64, f64)) {
        let Some(renderer) = &self.renderer else { return };
        let (cell_width, cell_height) = renderer.cell_size();
        let tab_bar_height = renderer.tab_bar_height();
        let padding = renderer.padding();
        // Account for search bar height if visible
        let search_bar_height = if self.search.is_active() || self.search.has_results() {
            cell_height + 8.0 * 2.0 // Same calculation as renderer
        } else {
            0.0
        };
        let content_top = tab_bar_height + search_bar_height;
        let scroll_offset = self.tabs
            .as_ref()
            .and_then(|tabs| tabs.active_tab().map(|tab| tab.terminal.grid().scroll_offset()))
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
                                            if let Some(url) = tab.terminal.get_hyperlink_url(hyperlink_id) {
                                                log::info!("Opening hyperlink: {}", url);
                                                if let Err(e) = crate::terminal::url::UrlDetector::open_url(url) {
                                                    log::error!("Failed to open URL: {}", e);
                                                }
                                                return;
                                            }
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
                                    px, py,
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
                                            if let Ok(msg) = self.save_config() {
                                                log::info!("{}", msg);
                                            }
                                        }
                                    }
                                    CommandsPanelAction::AddSubmit { name, command } => {
                                        if name == "SAVE" && command == "SAVE" {
                                            // Save button clicked
                                            if let Some(cmd) = self.commands_panel.try_submit() {
                                                self.config.commands.push(cmd);
                                                if let Ok(msg) = self.save_config() {
                                                    log::info!("{}", msg);
                                                }
                                            }
                                        } else if name.is_empty() && command.is_empty() {
                                            // Click on "+ New Command" button => start editing
                                            self.commands_panel.start_add();
                                        } else {
                                            // Actually add the command
                                            self.config.commands.push(crate::config::CustomCommand {
                                                name,
                                                command,
                                                alias: None,
                                            });
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
                                    px, py,
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
                                                        Ok(_) => log::info!("Loaded session '{}' from panel", name),
                                                        Err(e) => log::error!("Failed to restore session: {}", e),
                                                    }
                                                }
                                            }
                                            Ok(None) => log::warn!("Session '{}' not found", name),
                                            Err(e) => log::error!("Failed to load session: {}", e),
                                        }
                                        self.session_panel.close();
                                    }
                                    SessionPanelAction::Delete(name) => {
                                        if let Err(e) = crate::session::delete_session(&name) {
                                            log::error!("Failed to delete session '{}': {}", name, e);
                                        } else {
                                            log::info!("Deleted session '{}'", name);
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
                                    px, py, &self.session_dropdown_items, size.width as f32,
                                );
                                match click {
                                    SessionDropdownClick::LoadSession(name) => {
                                        use crate::session::restore::restore_named_session;
                                        match restore_named_session(&name) {
                                            Ok(Some(session)) => {
                                                if let Some(tabs) = &mut self.tabs {
                                                    match tabs.new_tab_from_session(&session) {
                                                        Ok(_) => log::info!("Loaded session '{}'", name),
                                                        Err(e) => log::error!("Failed to restore session: {}", e),
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

                        if py < renderer.tab_bar_height() {
                            let window_size = self.window.as_ref().map(|w| w.inner_size());
                            if let Some(size) = window_size {
                                use crate::renderer::TabBarClick;
                                let tab_infos = self.tabs.as_ref()
                                    .map(|t| t.tab_info())
                                    .unwrap_or_default();
                                let click = renderer.tab_bar_hit_test(
                                    px, py, &tab_infos, size.width as f32,
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
                                                }
                                                Err(e) => log::error!("Failed to create new tab: {}", e),
                                            }
                                        }
                                    }
                                    TabBarClick::CloseTab(idx) => {
                                        if let Some(tabs) = &mut self.tabs {
                                            tabs.close_tab(idx);
                                            if tabs.is_empty() {
                                                log::info!("Last tab closed, exiting");
                                                self.should_exit = true;
                                            }
                                        }
                                    }
                                    TabBarClick::Tab(idx) => {
                                        if let Some(tabs) = &mut self.tabs {
                                            tabs.switch_to(idx);
                                        }
                                    }
                                    TabBarClick::SessionDropdown => {
                                        self.session_dropdown_open = !self.session_dropdown_open;
                                        if self.session_dropdown_open {
                                            self.session_dropdown_items = crate::session::list_sessions_with_titles();
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
                                let grid_ref = self.tabs
                                    .as_ref()
                                    .and_then(|tabs| tabs.active_tab().map(|tab| tab.terminal.grid()));

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

                    // Intercept clicks for settings panel
                    if self.settings_panel.is_active() {
                        let window_size = self.window.as_ref().map(|w| w.inner_size());
                        if let Some(size) = window_size {
                            let action = self.settings_panel.handle_click(
                                position.0 as f32,
                                position.1 as f32,
                                &self.config,
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
                }
                if let Some(selection) = self.mouse_state.handle_press(button, grid_pos, self.modifiers) {
                    self.mouse_pressed = true;
                    if let Some(terminal) = self.active_terminal_mut() {
                        terminal.set_selection(Some(selection));
                    }
                }
            }
            ElementState::Released => {
                if button == MouseButton::Left {
                    self.mouse_pressed = false;
                    self.scrollbar_dragging = false;
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
        let Some(renderer) = &self.renderer else { return };
        let (cell_width, cell_height) = renderer.cell_size();
        let tab_bar_height = renderer.tab_bar_height();
        let padding = renderer.padding();
        // Account for search bar height if visible
        let search_bar_height = if self.search.is_active() || self.search.has_results() {
            cell_height + 8.0 * 2.0 // Same calculation as renderer
        } else {
            0.0
        };
        let content_top = tab_bar_height + search_bar_height;
        let scroll_offset = self.tabs
            .as_ref()
            .and_then(|tabs| tabs.active_tab().map(|tab| tab.terminal.grid().scroll_offset()))
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
            let tab_infos = self.tabs.as_ref()
                .map(|t| t.tab_info())
                .unwrap_or_default();
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

        // Don't update selection when an overlay panel is open,
        // but DO track hover state for visual highlights.
        if self.settings_panel.is_active() {
            let px = position.0 as f32;
            let py = position.1 as f32;
            if let Some(renderer) = &self.renderer {
                let (cell_width, cell_height) = renderer.cell_size();
                let window_size = self.window.as_ref().map(|w| w.inner_size());
                if let Some(size) = window_size {
                    use crate::ui::settings::SettingsPanelLayout;
                    let layout = SettingsPanelLayout::calculate(
                        size.width as f32,
                        size.height as f32,
                        cell_width,
                        cell_height,
                        self.settings_panel.animation_progress(),
                    );
                    let items = self.settings_panel.items_for_section(&self.config);
                    let is_dropdown = self.settings_panel.is_dropdown_open();
                    // When dropdown is open, suppress background content hover
                    let hovered_item = if !is_dropdown && layout.is_in_content(px, py) {
                        layout.item_at_y(py, items.len())
                    } else {
                        None
                    };
                    let section_count = crate::ui::settings::SettingsSection::ALL.len();
                    let hovered_section = if layout.is_in_sidebar(px, py) {
                        layout.section_at_y(py, section_count)
                    } else {
                        None
                    };
                    self.settings_panel.set_hover(hovered_item, hovered_section);

                    // Also detect hover within open dropdown overlay
                    if self.settings_panel.is_dropdown_open() {
                        let items = self.settings_panel.items_for_section(&self.config);
                        let selected_item = self.settings_panel.selected_item_index();
                        if let Some(item) = items.get(selected_item) {
                            let (options_text, option_count): (Vec<String>, usize) = match &item.kind {
                                crate::ui::settings::SettingKind::Dropdown { options, .. } => {
                                    (options.clone(), options.len())
                                }
                                crate::ui::settings::SettingKind::SteppedList { values, .. } => {
                                    (values.iter().map(|v| v.to_string()).collect(), values.len())
                                }
                                _ => (vec![], 0),
                            };
                            if option_count > 0 {
                                let max_label_len = items.iter().map(|i| i.label.len()).max().unwrap_or(0);
                                let label_end = layout.padding_x + max_label_len as f32 * cell_width;
                                let min_gap = cell_width * 2.0;
                                let max_value_area = layout.content_width * 0.65;
                                let value_area_offset = (label_end + min_gap).min(max_value_area);
                                let dd_value_area_x = layout.content_x + value_area_offset;
                                let anchor_y = layout.items_start_y + selected_item as f32 * layout.row_height + layout.row_height;

                                let max_option_len = options_text.iter().map(|s| s.len()).max().unwrap_or(10);
                                let min_dd_width = (max_option_len as f32 + 4.0) * cell_width;
                                let remaining_space = layout.content_width - value_area_offset;
                                let dd_width = min_dd_width.max(remaining_space).min(layout.content_width - cell_width * 2.0);

                                let dd_x = if dd_value_area_x + dd_width > layout.panel_x + layout.panel_width - cell_width {
                                    (layout.panel_x + layout.panel_width - dd_width - cell_width).max(layout.content_x + cell_width)
                                } else {
                                    dd_value_area_x
                                };

                                let option_h = layout.row_height * 0.85;
                                let max_visible = 8usize;
                                let visible_count = option_count.min(max_visible);
                                let dd_height = visible_count as f32 * option_h + 8.0;

                                let dd_hovered = if px >= dd_x && px <= dd_x + dd_width
                                    && py >= anchor_y && py <= anchor_y + dd_height
                                {
                                    let relative_y = py - anchor_y - 4.0;
                                    if relative_y >= 0.0 {
                                        let idx = (relative_y / option_h) as usize;
                                        if idx < visible_count { Some(idx) } else { None }
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                };
                                self.settings_panel.set_dropdown_hover(dd_hovered);
                            } else {
                                self.settings_panel.set_dropdown_hover(None);
                            }
                        } else {
                            self.settings_panel.set_dropdown_hover(None);
                        }
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
                        self.commands_panel.set_hover(hovered_index, hovered_add, hovered_delete);
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
                    let dropdown_x = (dropdown_anchor_x + session_button_width - dropdown_width).max(0.0);
                    let dropdown_y = tab_bar_height;

                    let session_count = self.session_dropdown_items.len().min(5);
                    let display_rows = session_count.max(1);
                    let total_items = display_rows + 1; // + "Manage Sessions"
                    let dropdown_height = total_items as f32 * row_height + 4.0;

                    self.session_dropdown_hovered = if px >= dropdown_x && px <= dropdown_x + dropdown_width
                        && py >= dropdown_y && py <= dropdown_y + dropdown_height
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
        let Some(renderer) = &self.renderer else { return false };
        let (window_width, _) = renderer.window_size();

        // Scrollbar is 6px wide + 2px margin from right edge, with an expanded hit area
        let scrollbar_hit_width = 16.0_f32;
        let scrollbar_x = window_width as f32 - scrollbar_hit_width;

        // Check if there's actually scrollback content
        let has_scrollback = self.tabs
            .as_ref()
            .and_then(|tabs| tabs.active_tab())
            .map(|tab| tab.terminal.grid().scrollback_len() > 0)
            .unwrap_or(false);

        has_scrollback && x as f32 >= scrollbar_x
    }

    /// Map a vertical mouse position to a scroll offset and apply it
    fn scroll_to_scrollbar_position(&mut self, y: f64) {
        let Some(renderer) = &self.renderer else { return };
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

        if scrollbar_height <= 0.0 { return; }

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

