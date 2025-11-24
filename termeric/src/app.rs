//! Application state and event handling

use anyhow::Result;
use std::sync::Arc;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::ActiveEventLoop,
    keyboard::ModifiersState,
    window::{Window, WindowId},
};

use crate::config::Config;
use crate::input::{Action, MouseState, check_keybinding, key_to_bytes_with_modifiers, screen_to_grid};
use crate::renderer::Renderer;
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
    /// Cursor blink state
    cursor_visible: bool,
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
            cursor_visible: true,
            mouse_state: MouseState::new(),
            modifiers: ModifiersState::empty(),
            clipboard,
            last_cursor_pos: None,
            mouse_pressed: false,
            default_font_size: 35.0,
        })
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
        let Some(renderer) = &mut self.renderer else { return };
        let Some(tabs) = &self.tabs else { return };
        let Some(tab) = tabs.active_tab() else { return };

        let terminal = &tab.terminal;
        let tab_info = tabs.tab_info();

        match renderer.render(terminal.grid(), terminal.selection(), self.cursor_visible, &tab_info) {
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
            }
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

    /// Handle mouse input
    fn handle_mouse_input(&mut self, button: MouseButton, state: ElementState, position: (f64, f64)) {
        let Some(renderer) = &self.renderer else { return };
        let (cell_width, cell_height) = renderer.cell_size();
        let grid_pos = screen_to_grid(position.0, position.1, cell_width, cell_height, 0);

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
        let grid_pos = screen_to_grid(position.0, position.1, cell_width, cell_height, 0);

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
        let window_attrs = Window::default_attributes()
            .with_title("Termeric")
            .with_inner_size(LogicalSize::new(800, 600));

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
                // Check for keybindings first
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
