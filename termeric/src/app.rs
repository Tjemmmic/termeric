//! Application state and event handling

use anyhow::Result;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::ActiveEventLoop,
    keyboard::ModifiersState,
    window::{Window, WindowId},
};

use termeric_core::{Pty, Terminal};
use termeric_core::pty::{PtyEvent, PtyHandle, spawn_reader_thread};

use crate::config::Config;
use crate::input::{Action, MouseState, check_keybinding, screen_to_grid};
use crate::renderer::Renderer;

/// Main application state
pub struct App {
    /// Application configuration
    #[allow(dead_code)]
    config: Config,
    /// The window (created on resume)
    window: Option<Arc<Window>>,
    /// The renderer
    renderer: Option<Renderer>,
    /// The terminal state
    terminal: Option<Terminal>,
    /// PTY handle for writing to shell
    pty: Option<PtyHandle>,
    /// Receiver for PTY output
    pty_rx: Option<Receiver<PtyEvent>>,
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
            terminal: None,
            pty: None,
            pty_rx: None,
            should_exit: false,
            cols: 80,
            rows: 24,
            cursor_visible: true,
            mouse_state: MouseState::new(),
            modifiers: ModifiersState::empty(),
            clipboard,
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

    /// Initialize the terminal and PTY
    fn init_terminal(&mut self) -> Result<()> {
        let cols = self.cols as u16;
        let rows = self.rows as u16;

        // Create terminal
        let terminal = Terminal::new(self.cols, self.rows);

        // Create PTY and spawn shell
        let pty = Pty::new(cols, rows)?;
        let (handle, reader) = pty.take_reader();

        // Spawn reader thread
        let rx = spawn_reader_thread(reader);

        self.terminal = Some(terminal);
        self.pty = Some(handle);
        self.pty_rx = Some(rx);

        log::info!("Terminal initialized: {}x{}", cols, rows);

        Ok(())
    }

    /// Process PTY events
    fn process_pty_events(&mut self) {
        let Some(rx) = &self.pty_rx else { return };
        let Some(terminal) = &mut self.terminal else { return };

        // Process all available events
        while let Ok(event) = rx.try_recv() {
            match event {
                PtyEvent::Output(data) => {
                    terminal.process(&data);
                }
                PtyEvent::Closed => {
                    log::info!("Shell exited");
                    self.should_exit = true;
                }
                PtyEvent::Error(e) => {
                    log::error!("PTY error: {}", e);
                    self.should_exit = true;
                }
            }
        }
    }

    /// Write input to the PTY
    fn write_to_pty(&mut self, data: &[u8]) {
        if let Some(pty) = &mut self.pty {
            if let Err(e) = pty.write_all(data) {
                log::error!("Failed to write to PTY: {}", e);
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

                if let Some(terminal) = &mut self.terminal {
                    terminal.resize(self.cols, self.rows);
                }

                if let Some(pty) = &self.pty {
                    if let Err(e) = pty.resize(self.cols as u16, self.rows as u16) {
                        log::error!("Failed to resize PTY: {}", e);
                    }
                }

                log::debug!("Resized to {}x{}", self.cols, self.rows);
            }
        }
    }

    /// Render the terminal
    fn render(&mut self) {
        let Some(renderer) = &mut self.renderer else { return };
        let Some(terminal) = &self.terminal else { return };

        match renderer.render(terminal.grid(), terminal.selection(), self.cursor_visible) {
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
                if let Some(terminal) = &mut self.terminal {
                    let offset = terminal.grid().scroll_offset();
                    terminal.grid_mut().set_scroll_offset(offset + lines);
                }
            }
            Action::ScrollDown(lines) => {
                if let Some(terminal) = &mut self.terminal {
                    let offset = terminal.grid().scroll_offset();
                    terminal.grid_mut().set_scroll_offset(offset.saturating_sub(lines));
                }
            }
            Action::ScrollToTop => {
                if let Some(terminal) = &mut self.terminal {
                    let max_offset = terminal.grid().scrollback_len();
                    terminal.grid_mut().set_scroll_offset(max_offset);
                }
            }
            Action::ScrollToBottom => {
                if let Some(terminal) = &mut self.terminal {
                    terminal.grid_mut().set_scroll_offset(0);
                }
            }
            Action::ClearSelection => {
                self.mouse_state.clear_selection();
                if let Some(terminal) = &mut self.terminal {
                    terminal.set_selection(None);
                }
            }
            Action::IncreaseFontSize | Action::DecreaseFontSize | Action::ResetFontSize => {
                // TODO: Implement font size changes
                log::debug!("Font size change requested: {:?}", action);
            }
            Action::None => {}
        }
    }

    /// Copy selection to clipboard
    fn copy_selection(&mut self) {
        let Some(terminal) = &self.terminal else { return };
        let Some(text) = terminal.get_selection_text() else { return };

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
                if let Some(selection) = self.mouse_state.handle_press(button, grid_pos, self.modifiers) {
                    if let Some(terminal) = &mut self.terminal {
                        terminal.set_selection(Some(selection));
                    }
                }
            }
            ElementState::Released => {
                if let Some(selection) = self.mouse_state.handle_release(button) {
                    if let Some(terminal) = &mut self.terminal {
                        terminal.set_selection(Some(selection));
                    }
                }
            }
        }
    }

    /// Handle mouse movement
    fn handle_mouse_move(&mut self, position: (f64, f64)) {
        let Some(renderer) = &self.renderer else { return };
        let (cell_width, cell_height) = renderer.cell_size();
        let grid_pos = screen_to_grid(position.0, position.1, cell_width, cell_height, 0);

        if let Some(selection) = self.mouse_state.handle_move(grid_pos) {
            if let Some(terminal) = &mut self.terminal {
                terminal.set_selection(Some(selection));
            }
            // Request redraw for selection update
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    /// Handle scroll wheel
    fn handle_scroll(&mut self, delta: f64) {
        if let Some(terminal) = &mut self.terminal {
            let lines = (delta.abs() / 3.0).max(1.0) as usize;
            let offset = terminal.grid().scroll_offset();

            if delta > 0.0 {
                terminal.grid_mut().set_scroll_offset(offset + lines);
            } else {
                terminal.grid_mut().set_scroll_offset(offset.saturating_sub(lines));
            }

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

                // Initialize terminal and PTY
                if let Err(e) = self.init_terminal() {
                    log::error!("Failed to initialize terminal: {}", e);
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
                if let Some(data) = crate::input::key_to_bytes(&event) {
                    self.write_to_pty(&data);
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                // Get current cursor position from mouse state
                if let Some(pos) = self.mouse_state.last_position {
                    let renderer_cell_size = self.renderer.as_ref().map(|r| r.cell_size());
                    if let Some((cw, ch)) = renderer_cell_size {
                        let screen_x = pos.col as f64 * cw as f64;
                        let screen_y = pos.row as f64 * ch as f64;
                        self.handle_mouse_input(button, state, (screen_x, screen_y));
                    }
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
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
