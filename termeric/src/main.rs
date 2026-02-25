//! Termeric - A GPU-accelerated terminal emulator
//!
//! This is the main entry point for the Termeric terminal emulator.

mod app;
mod config;
mod harvest;
mod input;
mod notifications;
mod panes;
mod renderer;
mod session;
mod tabs;
mod terminal;
mod ui;

use anyhow::Result;
use winit::event_loop::EventLoop;

fn main() -> Result<()> {
    // Initialize logging
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info")
    ).init();

    log::info!("Starting Termeric terminal emulator");

    // Create event loop
    let event_loop = EventLoop::new()?;

    // Create and run the application
    let mut app = app::App::new()?;
    event_loop.run_app(&mut app)?;

    Ok(())
}
