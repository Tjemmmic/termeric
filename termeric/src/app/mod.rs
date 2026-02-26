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
use crate::ui::command_palette::{CommandMode, CommandResult};
use crate::config::{Config, SplashMode};
use crate::harvest::HarvestMode;
use crate::input::{Action, MouseState, check_keybinding, key_to_bytes_with_modifiers, screen_to_grid};
use crate::input::keybindings::{KeyAction, KeyBindingManager};
use crate::renderer::{CommandBarInfo, Renderer};
use crate::terminal::search::Search;
use crate::ui::settings::{SettingsPanel, SettingsAction, SettingsKey};
use crate::terminal::splash::show_splash;
use self::state::AppState;
use crate::tabs::TabManager;
use crate::config::theme::Theme;

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
    /// Command mode state
    command_mode: CommandMode,
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
    /// Which tab bar element is currently hovered (for icon animations)
    tab_bar_hover: Option<crate::renderer::TabBarClick>,
    /// Smooth hover animation progress (0.0 = not hovered, 1.0 = fully hovered)
    tab_bar_hover_progress: f32,
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

        Ok(Self {
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
            command_mode: CommandMode::new(),
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
            tab_bar_hover: None,
            tab_bar_hover_progress: 0.0,
        })
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

    /// Write input to the active tab's PTY
    fn write_to_pty(&mut self, data: &[u8]) {
        // Reset cursor blink so cursor is fully visible when typing
        self.reset_cursor_blink();

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

        // Update command mode (for message timeout)
        self.command_mode.update();

        // Smooth hover animation progress for tab bar icons
        if self.tab_bar_hover.is_some() {
            self.tab_bar_hover_progress = (self.tab_bar_hover_progress + 0.2).min(1.0);
        } else {
            self.tab_bar_hover_progress = (self.tab_bar_hover_progress - 0.2).max(0.0);
        }

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

        // Build command bar info if in command mode
        let command_bar = if self.command_mode.is_active() {
            Some(CommandBarInfo {
                input: self.command_mode.input().to_string(),
                cursor_pos: self.command_mode.cursor_position(),
                message: self.command_mode.message().map(|m| m.text.clone()),
                is_error: self.command_mode.message().map(|m| m.is_error).unwrap_or(false),
                suggestions: self.command_mode.suggestions().to_vec(),
                selected_suggestion: self.command_mode.selected_suggestion(),
                phase: self.command_mode.phase(),
                message_scroll_offset: self.command_mode.message_scroll_offset(),
            })
        } else if let Some(msg) = self.command_mode.message() {
            // Show message briefly after command mode exits
            Some(CommandBarInfo {
                input: String::new(),
                cursor_pos: 0,
                message: Some(msg.text.clone()),
                is_error: msg.is_error,
                suggestions: Vec::new(),
                selected_suggestion: 0,
                phase: crate::ui::command_palette::CommandInputPhase::SelectingCommand,
                message_scroll_offset: self.command_mode.message_scroll_offset(),
            })
        } else {
            None
        };

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

        renderer.set_tab_bar_hover(self.tab_bar_hover, self.tab_bar_hover_progress);
        match renderer.render(terminal.grid(), terminal.selection(), cursor_opacity, cursor_display_pos, &tab_info, copy_flash, focus_dim, command_bar.as_ref(), settings_info.as_ref(), search_info.as_ref(), &self.search, images, session_dropdown_info.as_ref(), session_panel_info.as_ref()) {
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
                self.command_mode.activate();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                true
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

    /// Handle an action triggered by a keybinding
    fn handle_action(&mut self, action: Action) {
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

    /// Handle keyboard input when in command mode
    /// Returns true if the input was handled by command mode
    fn handle_command_mode_input(&mut self, event: &winit::event::KeyEvent) -> bool {
        use winit::keyboard::{Key, NamedKey};

        if event.state != winit::event::ElementState::Pressed {
            return self.command_mode.is_active();
        }

        if !self.command_mode.is_active() {
            return false;
        }

        match &event.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.command_mode.deactivate();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::Enter) => {
                // Handle Enter with phase-aware logic
                let should_execute = self.command_mode.handle_enter();

                if should_execute {
                    if let Some(result) = self.command_mode.execute() {
                        self.handle_command_result(result);
                    }
                }

                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::Backspace) => {
                self.command_mode.handle_backspace();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::Delete) => {
                self.command_mode.handle_delete();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                // If showing message AND input is empty, scroll the message
                // Otherwise, navigate suggestions/history as normal
                if self.command_mode.message().is_some() && self.command_mode.input().is_empty() {
                    self.command_mode.scroll_message_up();
                } else if self.modifiers.control_key() {
                    // Ctrl+Up for history
                    self.command_mode.history_prev();
                } else {
                    // Regular Up for suggestions
                    self.command_mode.prev_suggestion();
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::ArrowDown) => {
                // If showing message AND input is empty, scroll the message
                // Otherwise, navigate suggestions/history as normal
                if self.command_mode.message().is_some() && self.command_mode.input().is_empty() {
                    self.command_mode.scroll_message_down();
                } else if self.modifiers.control_key() {
                    // Ctrl+Down for history
                    self.command_mode.history_next();
                } else {
                    // Regular Down for suggestions
                    self.command_mode.next_suggestion();
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.command_mode.cursor_left();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.command_mode.cursor_right();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::Tab) => {
                // Only apply suggestion if we have suggestions
                if !self.command_mode.suggestions().is_empty() {
                    self.command_mode.apply_suggestion();
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                // Don't consume Tab if no suggestions - let it fall through
            }
            Key::Named(NamedKey::Home) => {
                self.command_mode.cursor_home();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::End) => {
                self.command_mode.cursor_end();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Key::Named(NamedKey::PageUp) => {
                // Scroll message up by multiple lines
                if self.command_mode.message().is_some() {
                    for _ in 0..5 {
                        self.command_mode.scroll_message_up();
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            Key::Named(NamedKey::PageDown) => {
                // Scroll message down by multiple lines
                if self.command_mode.message().is_some() {
                    for _ in 0..5 {
                        self.command_mode.scroll_message_down();
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            Key::Character(c) => {
                for ch in c.chars() {
                    self.command_mode.handle_char(ch);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }

        true // Input was handled by command mode
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

    /// Handle the result of executing a command
    fn handle_command_result(&mut self, result: CommandResult) {
        match result {
            CommandResult::SetTheme(name) => {
                match Theme::load_builtin(&name) {
                    Ok(theme) => {
                        self.config.theme.name = name.clone();
                        self.config.theme.path = None;
                        self.apply_theme(theme);
                        log::info!("Theme switched to: {}", name);
                        self.command_mode.set_message(format!("Theme set to: {}", name), false);
                    }
                    Err(err) => {
                        let msg = format!("Theme '{}' not found: {}", name, err);
                        self.command_mode.set_message(msg, true);
                    }
                }
            }
            CommandResult::SetFontSize(size) => {
                self.set_font_size(size);
                self.command_mode.set_message(format!("Font size set to: {}", size), false);
            }
            CommandResult::SetFontFamily(family) => {
                self.set_font_family(family.clone());
                log::info!("Font family changed to: {}", family);
                self.command_mode.set_message(format!("Font family set to: {}", family), false);
            }
            CommandResult::SetOpacity(opacity) => {
                self.config.window.opacity = opacity;
                if let Some(renderer) = &mut self.renderer {
                    renderer.set_opacity(opacity);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                log::info!("Opacity set to: {}", opacity);
                self.command_mode.set_message(format!("Opacity set to: {}", opacity), false);
            }
            CommandResult::SetPadding(padding) => {
                self.set_padding(padding);
                log::info!("Padding set to: {}", padding);
                self.command_mode.set_message(format!("Padding set to: {}", padding), false);
            }
            CommandResult::SetScrollback(lines) => {
                self.config.scrollback.lines = lines;
                if let Some(tabs) = &mut self.tabs {
                    tabs.set_scrollback_limit(lines);
                }
                log::info!("Scrollback set to: {} lines", lines);
                self.command_mode.set_message(format!("Scrollback set to: {} lines", lines), false);
            }
            CommandResult::ShowConfig => {
                let shell = self.config.shell.program.as_deref().unwrap_or("default");
                let config_text = format!(
                    "Current Configuration:\n\n\
                     Font Family: {}\n\
                     Font Size: {}\n\
                     Window Opacity: {}\n\
                     Window Padding: {}\n\
                     Scrollback Lines: {}\n\
                     Shell: {}",
                    self.config.font.family,
                    self.config.font.size,
                    self.config.window.opacity,
                    self.config.window.padding,
                    self.config.scrollback.lines,
                    shell
                );
                self.command_mode.set_message(config_text, false);
            }
            CommandResult::ResetConfig => {
                self.config = Config::default();
                self.default_font_size = self.config.font.size;
                self.set_font_family(self.config.font.family.clone());
                // Apply default font size
                self.set_font_size(self.config.font.size);
                self.set_padding(self.config.window.padding);
                let theme = Self::load_theme_from_config(&self.config);
                self.apply_theme(theme);
                if let Some(tabs) = &mut self.tabs {
                    tabs.set_scrollback_limit(self.config.scrollback.lines);
                }
                self.command_mode.set_message("Configuration reset to defaults".to_string(), false);
                log::info!("Configuration reset to defaults");
            }
            CommandResult::OpenConfigUI => {
                self.settings_panel.toggle();
                log::info!("Settings panel toggled");
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            CommandResult::SaveConfig => {
                match self.save_config() {
                    Ok(path) => {
                        self.command_mode.set_message(format!("Config saved to: {}", path), false);
                    }
                    Err(e) => {
                        self.command_mode.set_message(format!("Failed to save: {}", e), true);
                    }
                }
            }
            CommandResult::ReloadConfig => {
                match Config::load_or_default() {
                    Ok(config) => {
                        self.config = config;
                        self.default_font_size = self.config.font.size;
                        self.set_font_family(self.config.font.family.clone());
                        self.set_font_size(self.config.font.size);
                        self.set_padding(self.config.window.padding);
                        let theme = Self::load_theme_from_config(&self.config);
                        self.apply_theme(theme);
                        if let Some(tabs) = &mut self.tabs {
                            tabs.set_scrollback_limit(self.config.scrollback.lines);
                        }
                        self.command_mode.set_message("Config reloaded".to_string(), false);
                    }
                    Err(e) => {
                        self.command_mode.set_message(format!("Failed to reload: {}", e), true);
                    }
                }
            }
            CommandResult::ShowHelp(text) => {
                // Help text is already displayed as message in command module
                log::info!("Help: {}", text);
            }
            CommandResult::ListThemes => {
                let themes = "Available Themes:\n\n\
                             • turmeric (default) - Warm, earthy colors inspired by turmeric root\n\
                             • solarized-dark - Precision colors for machines and people\n\
                             • solarized-light - Light variant of Solarized\n\
                             • dracula - A dark theme for vampires\n\
                             • nord - Arctic, north-bluish clean theme\n\n\
                             Use 'theme <name>' to switch themes.";
                self.command_mode.set_message(themes.to_string(), false);
            }
            CommandResult::ListCommands => {
                let mut commands_text = String::from("Available Commands:\n\n");
                let registry = self.command_mode.registry();

                // Group commands by category
                let mut categories: std::collections::HashMap<&str, Vec<&crate::ui::command_palette::CommandDef>> = std::collections::HashMap::new();
                for cmd in registry.all() {
                    categories.entry(cmd.category).or_insert_with(Vec::new).push(cmd);
                }

                // Sort categories
                let mut cat_names: Vec<&str> = categories.keys().copied().collect();
                cat_names.sort();

                for category in cat_names {
                    commands_text.push_str(&format!("{}:\n", category));
                    if let Some(cmds) = categories.get(category) {
                        for cmd in cmds {
                            let alias_str = cmd.alias.map(|a| format!(" ({})", a)).unwrap_or_default();
                            commands_text.push_str(&format!("  • {}{} - {}\n", cmd.name, alias_str, cmd.description));
                        }
                    }
                    commands_text.push('\n');
                }

                self.command_mode.set_message(commands_text, false);
            }
            CommandResult::ListKeybindings => {
                let keybindings = "Keyboard Shortcuts:\n\n\
                                   COMMAND PALETTE:\n\
                                   Ctrl+Shift+P       - Open command palette\n\
                                   Escape             - Close command palette\n\n\
                                   CLIPBOARD:\n\
                                   Ctrl+Shift+C       - Copy selection\n\
                                   Ctrl+Shift+V       - Paste\n\n\
                                   SCROLLING:\n\
                                   Shift+PageUp       - Scroll up 10 lines\n\
                                   Shift+PageDown     - Scroll down 10 lines\n\
                                   Ctrl+Shift+Home    - Scroll to top\n\
                                   Ctrl+Shift+End     - Scroll to bottom\n\n\
                                   FONT SIZE:\n\
                                   Ctrl+Plus/Equal    - Increase font size\n\
                                   Ctrl+Minus         - Decrease font size\n\
                                   Ctrl+0             - Reset font size\n\n\
                                   TABS:\n\
                                   Ctrl+Shift+T       - New tab\n\
                                   Ctrl+Shift+W       - Close tab\n\
                                   Ctrl+Tab           - Next tab\n\
                                   Ctrl+Shift+Tab     - Previous tab\n\
                                   Ctrl+1-9           - Switch to tab by number\n\n\
                                   SELECTION:\n\
                                   Escape             - Clear selection\n\
                                   Click              - Simple selection\n\
                                   Double-click       - Word selection\n\
                                   Triple-click       - Line selection\n\
                                   Alt+Click          - Block selection";
                self.command_mode.set_message(keybindings.to_string(), false);
            }
            CommandResult::ShowVersion => {
                let version = env!("CARGO_PKG_VERSION");
                let version_info = format!(
                    "Termeric v{}\n\n\
                     A GPU-accelerated terminal emulator built with Rust.\n\
                     Features smooth rendering, warm turmeric-inspired theme,\n\
                     and modern terminal capabilities.\n\n\
                     Repository: https://github.com/tjemmmic/termeric",
                    version
                );
                self.command_mode.set_message(version_info, false);
            }
            CommandResult::ShowAbout => {
                let about = "Termeric Terminal Emulator\n\n\
                            A modern, GPU-accelerated terminal emulator\n\
                            built with Rust and wgpu.\n\n\
                            Built with:\n\
                            • Rust - Systems programming language\n\
                            • wgpu - GPU rendering\n\
                            • cosmic-text - Text shaping\n\
                            • termeric-core - Terminal emulation\n\n\
                            License: MIT or Apache 2.0\n\n\
                            Use 'version' to see version info.";
                self.command_mode.set_message(about.to_string(), false);
            }
            CommandResult::Success(_) | CommandResult::Error(_) | CommandResult::Cancel => {
                // Already handled in CommandMode
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
    fn is_click_on_scrollbar(&self, x: f64, y: f64) -> bool {
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

