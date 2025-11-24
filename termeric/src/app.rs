//! Application state and event handling

use anyhow::Result;
use std::sync::mpsc::Receiver;
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

/// Main application state
pub struct App {
    /// Application configuration
    config: Config,
    /// The window (created on resume)
    window: Option<Window>,
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
}

impl App {
    /// Create a new application instance
    pub fn new() -> Result<Self> {
        let config = Config::load_or_default()?;

        Ok(Self {
            config,
            window: None,
            terminal: None,
            pty: None,
            pty_rx: None,
            should_exit: false,
            cols: 80,
            rows: 24,
        })
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
        // Calculate new terminal dimensions based on cell size
        // For now, use approximate cell dimensions
        let cell_width = 10.0_f32;
        let cell_height = 20.0_f32;

        let new_cols = ((width as f32) / cell_width).floor() as usize;
        let new_rows = ((height as f32) / cell_height).floor() as usize;

        if new_cols != self.cols || new_rows != self.rows {
            self.cols = new_cols.max(1);
            self.rows = new_rows.max(1);

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

                // Calculate initial terminal size
                let size = window.inner_size();
                self.handle_resize(size.width, size.height);

                self.window = Some(window);

                // Initialize terminal and PTY
                if let Err(e) = self.init_terminal() {
                    log::error!("Failed to initialize terminal: {}", e);
                    event_loop.exit();
                }
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

                // TODO: Render the terminal grid
                // For now, just request another redraw to keep processing events
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
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
