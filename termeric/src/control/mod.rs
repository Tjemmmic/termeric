//! Private local control plane for already-running GUI processes.
//!
//! Inventory and bounded terminal-text queries are read-only; typed mutations
//! are explicit and ID-targeted. Opt-in input accepts typed text only, while
//! command launches accept bounded exact argv and explicit environment
//! overlays. Environment values stay out of inventory, responses, and logs;
//! clipboard data and plugin/provider credentials stay hidden.

mod environment_input;
pub(crate) mod launches;
pub mod protocol;

mod client;
mod paths;
mod server;
mod wait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlCommand {
    LaunchAndWait {
        env_stdin: bool,
        process_id: u32,
        // The parser validates a placeholder ID; execution replaces it with
        // this explicit ID or one freshly generated ID before any wire I/O.
        request: protocol::Request,
        request_id: Option<String>,
        timeout_seconds: Option<u32>,
        json: bool,
    },
    WaitLaunch {
        process_id: u32,
        launch_request_id: String,
        timeout_seconds: Option<u32>,
        json: bool,
    },
    LaunchStatus {
        process_id: u32,
        launch_request_id: String,
        json: bool,
    },
    List {
        json: bool,
    },
    GetText {
        process_id: u32,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        history_lines: u16,
        max_bytes: u32,
        json: bool,
    },
    SendText {
        process_id: u32,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        text: String,
        paste: bool,
        enter: bool,
        request_id: Option<String>,
        json: bool,
    },
    NewWindow {
        process_id: u32,
        source_window: u64,
        request_id: Option<String>,
        json: bool,
    },
    LaunchWindow {
        env_stdin: bool,
        process_id: u32,
        source_window: u64,
        program: String,
        args: Vec<String>,
        working_directory: Option<String>,
        title: Option<String>,
        environment: Vec<protocol::EnvironmentVariable>,
        environment_policy: termeric_core::pty::EnvironmentPolicy,
        request_id: Option<String>,
        json: bool,
    },
    NewTab {
        process_id: u32,
        window_id: u64,
        request_id: Option<String>,
        json: bool,
    },
    LaunchTab {
        background: bool,
        env_stdin: bool,
        process_id: u32,
        window_id: u64,
        program: String,
        args: Vec<String>,
        working_directory: Option<String>,
        title: Option<String>,
        environment: Vec<protocol::EnvironmentVariable>,
        environment_policy: termeric_core::pty::EnvironmentPolicy,
        request_id: Option<String>,
        json: bool,
    },
    SplitPane {
        process_id: u32,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        direction: crate::panes::SplitDirection,
        request_id: Option<String>,
        json: bool,
    },
    FocusWindow {
        process_id: u32,
        window_id: u64,
        request_id: Option<String>,
        json: bool,
    },
    SelectTab {
        process_id: u32,
        window_id: u64,
        tab_id: u64,
        request_id: Option<String>,
        json: bool,
    },
    FocusPane {
        process_id: u32,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        request_id: Option<String>,
        json: bool,
    },
    CloseWindow {
        process_id: u32,
        window_id: u64,
        request_id: Option<String>,
        json: bool,
    },
    CloseTab {
        process_id: u32,
        window_id: u64,
        tab_id: u64,
        request_id: Option<String>,
        json: bool,
    },
    ClosePane {
        process_id: u32,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        request_id: Option<String>,
        json: bool,
    },
    MoveTab {
        process_id: u32,
        source_window_id: u64,
        tab_id: u64,
        destination_window_id: u64,
        destination_index: u16,
        request_id: Option<String>,
        json: bool,
    },
    MovePane {
        process_id: u32,
        source_window_id: u64,
        source_tab_id: u64,
        pane_id: u32,
        destination_window_id: u64,
        destination_tab_id: u64,
        target_pane_id: u32,
        direction: crate::panes::SplitDirection,
        request_id: Option<String>,
        json: bool,
    },
    ResizeWindow {
        process_id: u32,
        window_id: u64,
        width: u32,
        height: u32,
        request_id: Option<String>,
        json: bool,
    },
    ResizeDivider {
        process_id: u32,
        window_id: u64,
        tab_id: u64,
        divider_id: u64,
        ratio_millis: u16,
        request_id: Option<String>,
        json: bool,
    },
}

pub use client::{
    answer_approval, list_agents, read_conversation, report_agent_event, running_processes,
    submit_to_agent, submit_to_session, toggle_existing_quick_access,
};
pub(crate) use paths::QuickAccessGuard;
pub(crate) use server::ControlServer;

/// The display this process draws on: its Wayland socket, else its X11
/// display. Windows of one process share one display connection, so a
/// launch may join only a process with the same identity.
pub fn display_identity() -> Option<String> {
    let variable = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    variable("WAYLAND_DISPLAY")
        .map(|display| format!("wayland:{display}"))
        .or_else(|| variable("DISPLAY").map(|display| format!("x11:{display}")))
}

/// This process's build: its executable and when that file last changed,
/// as first asked. A process asks at startup, so a later rebuild of the
/// same file reads as another build.
pub fn build_identity() -> Option<String> {
    static IDENTITY: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    IDENTITY
        .get_or_init(|| {
            let executable = std::env::current_exe().ok()?.canonicalize().ok()?;
            let changed = std::fs::metadata(&executable)
                .ok()?
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?;
            Some(format!(
                "{}@{}.{}",
                executable.to_str()?,
                changed.as_secs(),
                changed.subsec_nanos()
            ))
        })
        .clone()
}

/// Opens `plan`'s window in a running Termeric on this display, when the
/// plan and settings allow: every window then shares one process, so tabs
/// can move between them. Returns the process and window, or `None` when
/// this launch should start a process of its own. Any failure to reach a
/// running process falls back to that.
pub fn join_running_launch(plan: &crate::cli::LaunchPlan) -> Option<(u32, u64)> {
    let current_dir = std::env::current_dir().ok();
    let tabs = plan.join_tabs(current_dir.as_deref())?;
    // A configuration that does not load reports itself at startup.
    let config = crate::config::Config::load_or_default().ok()?;
    if !config.window.share_process {
        return None;
    }
    let display = display_identity()?;
    let build = build_identity()?;
    let variable = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let token = variable("XDG_ACTIVATION_TOKEN").or_else(|| variable("DESKTOP_STARTUP_ID"));
    match client::join_running(&display, &build, token, tabs) {
        Ok(joined) => joined,
        Err(error) => {
            log::debug!("Could not look for a running Termeric: {error:#}");
            None
        }
    }
}

/// A pane's recent terminal text, oldest first.
pub fn read_terminal_text(
    process_id: u32,
    window_id: u64,
    tab_id: u64,
    pane_id: u32,
    history_lines: u16,
) -> anyhow::Result<String> {
    client::get_text(
        process_id,
        window_id,
        tab_id,
        pane_id,
        history_lines,
        protocol::MAX_TERMINAL_TEXT_BYTES,
        false,
    )
}

pub struct ControlOutput {
    pub text: String,
    pub exit_code: u8,
}

pub fn execute(mut command: ControlCommand) -> anyhow::Result<ControlOutput> {
    environment_input::prepare(&mut command)?;
    if let ControlCommand::LaunchAndWait {
        env_stdin: _,
        process_id,
        request,
        request_id,
        timeout_seconds,
        json,
    } = command
    {
        return wait::launch_and_wait(process_id, request, request_id, timeout_seconds, json);
    }
    if let ControlCommand::WaitLaunch {
        process_id,
        launch_request_id,
        timeout_seconds,
        json,
    } = command
    {
        return wait::launch(process_id, launch_request_id, timeout_seconds, json);
    }
    execute_once(command).map(|text| ControlOutput { text, exit_code: 0 })
}

fn execute_once(command: ControlCommand) -> anyhow::Result<String> {
    match command {
        ControlCommand::LaunchAndWait { .. } => unreachable!("wait dispatched separately"),
        ControlCommand::WaitLaunch { .. } => unreachable!("wait dispatched separately"),
        ControlCommand::LaunchStatus {
            process_id,
            launch_request_id,
            json,
        } => client::launch_status(process_id, launch_request_id, json),
        ControlCommand::List { json } => client::list(json),
        ControlCommand::GetText {
            process_id,
            window_id,
            tab_id,
            pane_id,
            history_lines,
            max_bytes,
            json,
        } => client::get_text(
            process_id,
            window_id,
            tab_id,
            pane_id,
            history_lines,
            max_bytes,
            json,
        ),
        ControlCommand::SendText {
            process_id,
            window_id,
            tab_id,
            pane_id,
            text,
            paste,
            enter,
            request_id,
            json,
        } => client::send_text(
            process_id, window_id, tab_id, pane_id, text, paste, enter, request_id, json,
        ),
        ControlCommand::NewWindow {
            process_id,
            source_window,
            request_id,
            json,
        } => client::new_window(process_id, source_window, request_id, json),
        ControlCommand::LaunchWindow {
            env_stdin: _,
            process_id,
            source_window,
            program,
            args,
            working_directory,
            title,
            environment,
            environment_policy,
            request_id,
            json,
        } => client::launch_window(
            process_id,
            source_window,
            program,
            args,
            working_directory,
            title,
            environment,
            environment_policy,
            request_id,
            json,
        ),
        ControlCommand::NewTab {
            process_id,
            window_id,
            request_id,
            json,
        } => client::new_tab(process_id, window_id, request_id, json),
        ControlCommand::LaunchTab {
            background,
            env_stdin: _,
            process_id,
            window_id,
            program,
            args,
            working_directory,
            title,
            environment,
            environment_policy,
            request_id,
            json,
        } => client::launch_tab(
            background,
            process_id,
            window_id,
            program,
            args,
            working_directory,
            title,
            environment,
            environment_policy,
            request_id,
            json,
        ),
        ControlCommand::SplitPane {
            process_id,
            window_id,
            tab_id,
            pane_id,
            direction,
            request_id,
            json,
        } => client::split_pane(
            process_id, window_id, tab_id, pane_id, direction, request_id, json,
        ),
        ControlCommand::FocusWindow {
            process_id,
            window_id,
            request_id,
            json,
        } => client::focus_window(process_id, window_id, request_id, json),
        ControlCommand::SelectTab {
            process_id,
            window_id,
            tab_id,
            request_id,
            json,
        } => client::select_tab(process_id, window_id, tab_id, request_id, json),
        ControlCommand::FocusPane {
            process_id,
            window_id,
            tab_id,
            pane_id,
            request_id,
            json,
        } => client::focus_pane(process_id, window_id, tab_id, pane_id, request_id, json),
        ControlCommand::CloseWindow {
            process_id,
            window_id,
            request_id,
            json,
        } => client::close_window(process_id, window_id, request_id, json),
        ControlCommand::CloseTab {
            process_id,
            window_id,
            tab_id,
            request_id,
            json,
        } => client::close_tab(process_id, window_id, tab_id, request_id, json),
        ControlCommand::ClosePane {
            process_id,
            window_id,
            tab_id,
            pane_id,
            request_id,
            json,
        } => client::close_pane(process_id, window_id, tab_id, pane_id, request_id, json),
        ControlCommand::MoveTab {
            process_id,
            source_window_id,
            tab_id,
            destination_window_id,
            destination_index,
            request_id,
            json,
        } => client::move_tab(
            process_id,
            source_window_id,
            tab_id,
            destination_window_id,
            destination_index,
            request_id,
            json,
        ),
        ControlCommand::MovePane {
            process_id,
            source_window_id,
            source_tab_id,
            pane_id,
            destination_window_id,
            destination_tab_id,
            target_pane_id,
            direction,
            request_id,
            json,
        } => client::move_pane(
            process_id,
            source_window_id,
            source_tab_id,
            pane_id,
            destination_window_id,
            destination_tab_id,
            target_pane_id,
            direction,
            request_id,
            json,
        ),
        ControlCommand::ResizeWindow {
            process_id,
            window_id,
            width,
            height,
            request_id,
            json,
        } => client::resize_window(process_id, window_id, width, height, request_id, json),
        ControlCommand::ResizeDivider {
            process_id,
            window_id,
            tab_id,
            divider_id,
            ratio_millis,
            request_id,
            json,
        } => client::resize_divider(
            process_id,
            window_id,
            tab_id,
            divider_id,
            ratio_millis,
            request_id,
            json,
        ),
    }
}
