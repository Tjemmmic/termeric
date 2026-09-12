//! Private local control plane for already-running GUI processes.
//!
//! Inventory and bounded terminal-text queries are read-only; typed mutations
//! are explicit and ID-targeted. Opt-in input accepts typed text only, while
//! command launches accept bounded exact argv and non-secret environment
//! overlays. Environment values stay out of inventory, responses, and logs;
//! clipboard data and plugin/provider credentials stay hidden.

pub mod protocol;

#[cfg(unix)]
mod client;
#[cfg(unix)]
mod paths;
#[cfg(unix)]
mod server;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlCommand {
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
        process_id: u32,
        source_window: u64,
        program: String,
        args: Vec<String>,
        working_directory: Option<String>,
        title: Option<String>,
        environment: Vec<protocol::EnvironmentVariable>,
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
        process_id: u32,
        window_id: u64,
        program: String,
        args: Vec<String>,
        working_directory: Option<String>,
        title: Option<String>,
        environment: Vec<protocol::EnvironmentVariable>,
        request_id: Option<String>,
        json: bool,
    },
    SplitPane {
        process_id: u32,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        direction: protocol::SplitDirection,
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
        direction: protocol::SplitDirection,
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

#[cfg(unix)]
pub(crate) use paths::QuickAccessGuard;
#[cfg(unix)]
pub(crate) use server::ControlServer;

pub fn toggle_existing_quick_access() -> anyhow::Result<Option<String>> {
    #[cfg(unix)]
    {
        client::toggle_existing_quick_access()
    }
    #[cfg(not(unix))]
    {
        Ok(None)
    }
}

pub fn execute(command: ControlCommand) -> anyhow::Result<String> {
    #[cfg(unix)]
    {
        match command {
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
                process_id,
                source_window,
                program,
                args,
                working_directory,
                title,
                environment,
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
                process_id,
                window_id,
                program,
                args,
                working_directory,
                title,
                environment,
                request_id,
                json,
            } => client::launch_tab(
                process_id,
                window_id,
                program,
                args,
                working_directory,
                title,
                environment,
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
    #[cfg(not(unix))]
    {
        let _ = command;
        anyhow::bail!("running-GUI control currently requires Unix-domain sockets")
    }
}
