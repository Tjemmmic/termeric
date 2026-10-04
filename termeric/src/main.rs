//! Termeric - A GPU-accelerated terminal emulator
//!
//! This is the main entry point for the Termeric terminal emulator.

// UI dispatch intentionally uses nested conditions to preserve input-priority
// boundaries and short borrow scopes. Renderer builders intentionally receive
// explicit geometry/state rather than hiding unrelated inputs in generic bags.
#![allow(clippy::collapsible_if, clippy::too_many_arguments)]

mod agent_view;
mod agents;
mod app;
mod cli;
mod config;
mod control;
mod domain;
mod git;
mod harvest;
mod input;
mod line_reader;
mod mcp;
mod notifications;
mod panes;
mod paths;
mod plugin;
mod renderer;
mod safe_fs;
mod safe_text;
mod serial;
mod session;
mod tabs;
mod terminal;
#[cfg(test)]
mod test_support;
mod ui;
mod workspace;

use anyhow::Result;
use winit::event_loop::EventLoop;

fn main() -> Result<std::process::ExitCode> {
    let outcome = match cli::parse_env()? {
        // Hooks run inside an agent's turn: no logging setup, no output, and
        // success whatever happens.
        cli::Invocation::AgentEvent(invocation) => {
            match invocation {
                Some(invocation) => agents::hook::run(&invocation),
                // A hook from a newer setup, still owed a read of its input.
                None => agents::hook::discard_input(),
            }
            return Ok(std::process::ExitCode::SUCCESS);
        }
        cli::Invocation::Command(outcome) => *outcome,
    };

    // Initialize logging after deterministic parsing but before either GUI or
    // local-domain execution.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut _quick_access_guard = None;
    // Recorded before anything can rebuild the executable, so a launch of a
    // newer build never joins this process.
    let _ = control::build_identity();
    let launch = match outcome {
        cli::ParseOutcome::Run(launch) => {
            if let Some((process_id, window_id)) = control::join_running_launch(&launch) {
                log::info!("Opened window {window_id} in the running Termeric ({process_id})");
                return Ok(std::process::ExitCode::SUCCESS);
            }
            launch
        }
        cli::ParseOutcome::Mcp {
            process_id,
            supervisor,
        } => {
            // Standard output carries the protocol; logging stays on stderr.
            mcp::serve(mcp::ControlBackend {
                process_id,
                supervisor,
            })?;
            return Ok(std::process::ExitCode::SUCCESS);
        }
        cli::ParseOutcome::QuickAccess(launch) => {
            if let Some(output) = control::toggle_existing_quick_access()? {
                println!("{output}");
                return Ok(std::process::ExitCode::SUCCESS);
            }
            _quick_access_guard = Some(control::QuickAccessGuard::acquire()?);
            launch
        }
        cli::ParseOutcome::Control(command) => {
            let output = control::execute(command)?;
            if !output.text.is_empty() {
                println!("{}", output.text);
            }
            return Ok(std::process::ExitCode::from(output.exit_code));
        }
        cli::ParseOutcome::Domain(command) => {
            if let Some(output) = domain::execute(command)?
                && !output.is_empty()
            {
                println!("{output}");
            }
            return Ok(std::process::ExitCode::SUCCESS);
        }
        cli::ParseOutcome::Plugin(command) => {
            let output = plugin::runtime::execute_admin(command)?;
            if !output.is_empty() {
                println!("{output}");
            }
            return Ok(std::process::ExitCode::SUCCESS);
        }
        cli::ParseOutcome::SerialBridge(options) => {
            serial::run_bridge(options)?;
            return Ok(std::process::ExitCode::SUCCESS);
        }
        cli::ParseOutcome::Print(text) => {
            println!("{text}");
            return Ok(std::process::ExitCode::SUCCESS);
        }
    };

    log::info!("Starting Termeric terminal emulator");

    // Resolve configuration and validate every window/tab launch before winit
    // connects to a display server or creates native resources. CLI failures
    // therefore remain ordinary nonzero command errors with no window flash.
    let mut app = app::Application::new(launch)?;

    // Create event loop
    let event_loop = EventLoop::<app::AppEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    app::install_event_loop_proxy(proxy.clone());
    let _control_server = match control::ControlServer::start(proxy) {
        Ok(server) => Some(server),
        Err(error) => {
            log::warn!("Running-GUI control is unavailable: {error:#}");
            None
        }
    };

    // Create and run the native windows.
    event_loop.run_app(&mut app)?;
    let fatal = app.take_fatal_startup_error();
    // Closing the windows ends their agents; an agent that keeps a
    // conversation as soon as it opens is told to forget one nothing was
    // said in, which must not be cut short by exiting.
    drop(app);
    agent_view::session::wait_for_forgetting(std::time::Duration::from_secs(5));
    if let Some(error) = fatal {
        anyhow::bail!(error);
    }

    Ok(std::process::ExitCode::SUCCESS)
}
