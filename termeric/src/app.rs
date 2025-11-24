//! Application state and event handling

use anyhow::Result;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::ActiveEventLoop,
    window::{Window, WindowId},
};

use termeric_core::{Pty, Terminal};
use termeric_core::pty::{PtyEvent, PtyHandle, spawn_reader_thread};

use crate::config::Config;
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
}

impl App {
    /// Create a new application instance
    pub fn new() -> Result<Self> {
        let config = Config::load_or_default()?;

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

        match renderer.render(terminal.grid(), self.cursor_visible) {
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

            WindowEvent::KeyboardInput { event, .. } => {
                // Convert key event to terminal input
                if let Some(data) = crate::input::key_to_bytes(&event) {
                    self.write_to_pty(&data);
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
