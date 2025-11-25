//! Application state and event handling

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

use crate::command::{CommandMode, CommandResult};
use crate::config::Config;
use crate::input::{Action, MouseState, check_keybinding, key_to_bytes_with_modifiers, screen_to_grid};
use crate::renderer::{CommandBarInfo, Renderer};
use crate::tabs::TabManager;

/// Main application state
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
}

impl App {
    /// Create a new application instance
    pub fn new() -> Result<Self> {
        let config = Config::load_or_default()?;

        // Initialize clipboard (may fail on some systems)
        let clipboard = arboard::Clipboard::new().ok();

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
            default_font_size: 35.0,
            copy_flash_start: None,
            window_focused: true,
            focus_change_time: Instant::now(),
            command_mode: CommandMode::new(),
        })
    }

    /// Load window icon from embedded PNG data
    ///
    /// The icon is embedded directly in the binary at compile time,
    /// so it works on all platforms without requiring external files.
    fn load_icon() -> Option<Icon> {
        // Icon is embedded in the binary at compile time
        let icon_bytes = include_bytes!("../../assets/icons/termeric.png");

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
        let renderer = Renderer::new(window.clone())?;

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

    /// Initialize the tab manager with the first tab
    fn init_tabs(&mut self) -> Result<()> {
        let mut tabs = TabManager::new(self.cols, self.rows);
        tabs.new_tab()?;

        self.tabs = Some(tabs);

        log::info!("Tab manager initialized with first tab: {}x{}", self.cols, self.rows);

        Ok(())
    }

    /// Process PTY events for all tabs
    fn process_pty_events(&mut self) {
        let Some(tabs) = &mut self.tabs else { return };

        tabs.process_all_events();

        // Check if all tabs are closed
        if tabs.cleanup_closed() && tabs.is_empty() {
            log::info!("All tabs closed, exiting");
            self.should_exit = true;
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

    /// Render the terminal
    fn render(&mut self) {
        // Update command mode (for message timeout)
        self.command_mode.update();

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
                phase: crate::command::CommandInputPhase::SelectingCommand,
                message_scroll_offset: self.command_mode.message_scroll_offset(),
            })
        } else {
            None
        };

        match renderer.render(terminal.grid(), terminal.selection(), cursor_opacity, cursor_display_pos, &tab_info, copy_flash, focus_dim, command_bar.as_ref()) {
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
                        Ok(id) => log::info!("Created new tab {}", id),
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
                // TODO: Implement search UI
                log::debug!("Toggle search requested");
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

    /// Handle the result of executing a command
    fn handle_command_result(&mut self, result: CommandResult) {
        match result {
            CommandResult::SetTheme(name) => {
                // TODO: Implement theme switching
                log::info!("Theme switch requested: {}", name);
                self.command_mode.set_message(format!("Theme set to: {}", name), false);
            }
            CommandResult::SetFontSize(size) => {
                self.set_font_size(size);
                self.command_mode.set_message(format!("Font size set to: {}", size), false);
            }
            CommandResult::SetFontFamily(family) => {
                self.config.font.family = family.clone();
                // TODO: Apply font family to renderer
                log::info!("Font family changed to: {}", family);
                self.command_mode.set_message(format!("Font family set to: {} (restart required)", family), false);
            }
            CommandResult::SetOpacity(opacity) => {
                self.config.window.opacity = opacity;
                // TODO: Apply opacity to window
                log::info!("Opacity set to: {}", opacity);
                self.command_mode.set_message(format!("Opacity set to: {}", opacity), false);
            }
            CommandResult::SetPadding(padding) => {
                self.config.window.padding = padding;
                // TODO: Apply padding to renderer
                log::info!("Padding set to: {}", padding);
                self.command_mode.set_message(format!("Padding set to: {}", padding), false);
            }
            CommandResult::SetScrollback(lines) => {
                self.config.scrollback.lines = lines;
                // TODO: Apply scrollback limit to terminal
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
                // Apply default font size
                self.set_font_size(self.config.font.size);
                self.command_mode.set_message("Configuration reset to defaults".to_string(), false);
                log::info!("Configuration reset to defaults");
            }
            CommandResult::OpenConfigUI => {
                // TODO: Implement configuration UI
                log::info!("Config UI requested");
                self.command_mode.set_message("Config UI coming soon!".to_string(), false);
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
                        // Apply font size from config
                        self.set_font_size(config.font.size);
                        self.config = config;
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
                let mut categories: std::collections::HashMap<&str, Vec<&crate::command::CommandDef>> = std::collections::HashMap::new();
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

        // Build current config
        let font_size = self.renderer.as_ref().map(|r| r.font_size()).unwrap_or(self.default_font_size);

        let config_content = format!(
r#"# Termeric Configuration

[font]
size = {}

[theme]
name = "{}"

[window]
width = {}
height = {}
"#,
            font_size,
            self.config.theme.name,
            self.config.window.width,
            self.config.window.height,
        );

        std::fs::write(&config_path, config_content)?;
        Ok(config_path.display().to_string())
    }

    /// Handle mouse input
    fn handle_mouse_input(&mut self, button: MouseButton, state: ElementState, position: (f64, f64)) {
        let Some(renderer) = &self.renderer else { return };
        let (cell_width, cell_height) = renderer.cell_size();
        let tab_bar_height = renderer.tab_bar_height();
        let grid_pos = screen_to_grid(position.0, position.1, cell_width, cell_height, tab_bar_height, 0);

        match state {
            ElementState::Pressed => {
                if button == MouseButton::Left {
                    self.mouse_pressed = true;
                }
                if let Some(selection) = self.mouse_state.handle_press(button, grid_pos, self.modifiers) {
                    if let Some(terminal) = self.active_terminal_mut() {
                        terminal.set_selection(Some(selection));
                    }
                }
            }
            ElementState::Released => {
                if button == MouseButton::Left {
                    self.mouse_pressed = false;
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
        let grid_pos = screen_to_grid(position.0, position.1, cell_width, cell_height, tab_bar_height, 0);

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
            let lines = (delta.abs() / 3.0).max(1.0) as usize;
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
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        // Create window
        let mut window_attrs = Window::default_attributes()
            .with_title("Termeric")
            .with_inner_size(LogicalSize::new(800, 600));

        // Load and set window icon if available
        if let Some(icon) = Self::load_icon() {
            window_attrs = window_attrs.with_window_icon(Some(icon));
        }

        match event_loop.create_window(window_attrs) {
            Ok(window) => {
                log::info!("Window created");
                let window = Arc::new(window);

                // Initialize renderer
                if let Err(e) = self.init_renderer(window.clone()) {
                    log::error!("Failed to initialize renderer: {}", e);
                    event_loop.exit();
                    return;
                }

                // Initialize tabs with first tab
                if let Err(e) = self.init_tabs() {
                    log::error!("Failed to initialize tabs: {}", e);
                    event_loop.exit();
                    return;
                }

                // Initial render
                self.render();
            }
            Err(e) => {
                log::error!("Failed to create window: {}", e);
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                log::info!("Close requested");
                event_loop.exit();
            }

            WindowEvent::Resized(size) => {
                self.handle_resize(size.width, size.height);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }

            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }

            WindowEvent::KeyboardInput { event, .. } => {
                // Handle command mode input first
                if self.handle_command_mode_input(&event) {
                    return;
                }

                // Check for Ctrl+Shift+P to enter command mode
                if event.state == winit::event::ElementState::Pressed {
                    use winit::keyboard::{Key, NamedKey};

                    // Ctrl+Shift+P activates command palette
                    if self.modifiers.control_key() && self.modifiers.shift_key() {
                        if let Key::Character(c) = &event.logical_key {
                            if c.eq_ignore_ascii_case("p") {
                                self.command_mode.activate();
                                if let Some(window) = &self.window {
                                    window.request_redraw();
                                }
                                return;
                            }
                        }
                    }
                }

                // Check for keybindings
                if let Some(action) = check_keybinding(&event, self.modifiers) {
                    self.handle_action(action);
                    return;
                }

                // Convert key event to terminal input
                if let Some(data) = key_to_bytes_with_modifiers(&event, self.modifiers) {
                    self.write_to_pty(&data);
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                // Use tracked cursor position
                if let Some(pos) = self.last_cursor_pos {
                    self.handle_mouse_input(button, state, pos);
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.last_cursor_pos = Some((position.x, position.y));
                self.handle_mouse_move((position.x, position.y));
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let y_delta = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y as f64 * 3.0,
                    winit::event::MouseScrollDelta::PixelDelta(pos) => pos.y / 10.0,
                };
                self.handle_scroll(y_delta);
            }

            WindowEvent::Focused(focused) => {
                if self.window_focused != focused {
                    self.window_focused = focused;
                    self.focus_change_time = Instant::now();
                    log::debug!("Window focus changed: {}", focused);
                }
            }

            WindowEvent::RedrawRequested => {
                // Process any pending PTY events
                self.process_pty_events();

                if self.should_exit {
                    event_loop.exit();
                    return;
                }

                // Render
                self.render();
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Process PTY events before waiting
        self.process_pty_events();

        // Request redraw if we have new content
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}
