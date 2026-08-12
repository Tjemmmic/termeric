//! Termeric - A GPU-accelerated terminal emulator
//!
//! This is the main entry point for the Termeric terminal emulator.

// UI dispatch intentionally uses nested conditions to preserve input-priority
// boundaries and short borrow scopes. Renderer builders intentionally receive
// explicit geometry/state rather than hiding unrelated inputs in generic bags.
#![allow(clippy::collapsible_if, clippy::too_many_arguments)]

mod app;
mod config;
mod git;
mod harvest;
mod input;
mod notifications;
#[allow(dead_code)]
mod panes;
mod plugin;
mod renderer;
mod session;
mod tabs;
mod terminal;
#[cfg(test)]
mod test_support;
mod ui;

use anyhow::Result;
use winit::event_loop::EventLoop;

fn main() -> Result<()> {
    // Initialize logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    log::info!("Starting Termeric terminal emulator");

    // Create event loop
    let event_loop = EventLoop::new()?;

    // Create and run the application
    let mut app = app::App::new()?;
    event_loop.run_app(&mut app)?;

    Ok(())
}
