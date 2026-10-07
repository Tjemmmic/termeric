//! `termeric control`: commands for already-running Termeric GUI processes.

use super::Direction;
use crate::control::protocol::{self, EnvironmentVariable, Request};
use crate::control::{ControlCommand, Wait};
use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "termeric control",
    about = "Script the windows, tabs, and panes of a running Termeric (experimental)",
    after_help = "Process, window, tab, pane, and divider IDs come from `termeric control list` \
and last only while Termeric runs. Every command takes --json, and every command that changes \
something takes --request-id ID: retrying with the same ID never acts twice.\n\nAny program \
running as your user can use these commands to list windows, read pane text, and launch \
programs. Typing into panes with send-text stays off until you set [security] \
allow_gui_control_input = true. Nothing here reads a terminal's environment, working \
directory, clipboard, or more than 256 lines of its scrollback.",
    arg_required_else_help = true,
    disable_version_flag = true
)]
pub(super) struct ControlCli {
    #[command(subcommand)]
    command: ControlArgs,
}

#[derive(Subcommand)]
enum ControlArgs {
    /// List windows, tabs, panes, and dividers with their IDs
    List {
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Wait for a launched program to exit, then exit with its code
    WaitLaunch {
        #[command(flatten)]
        launch: LaunchTarget,
        /// Stop waiting after SECONDS (1-86400); the program keeps running
        #[arg(long, value_name = "SECONDS", value_parser = timeout)]
        timeout: Option<u32>,
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show whether a launched program is still running, or how it exited
    LaunchStatus {
        #[command(flatten)]
        launch: LaunchTarget,
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Print the text on a pane's screen, and optionally some scrollback
    GetText {
        #[command(flatten)]
        pane: PaneTarget,
        /// Lines of scrollback to include above the screen, at most 256
        #[arg(long, value_name = "N", default_value_t = 0)]
        history_lines: u16,
        /// Most bytes of text to print, at most 65536; the oldest text is dropped first
        #[arg(long = "bytes", value_name = "N", default_value_t = protocol::MAX_TERMINAL_TEXT_BYTES)]
        max_bytes: u32,
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Type text into a pane; needs [security] allow_gui_control_input = true
    SendText {
        #[command(flatten)]
        pane: PaneTarget,
        /// Printable characters, tabs, and newlines, at most 64 KiB
        #[arg(allow_hyphen_values = true)]
        text: String,
        /// Send it as a bracketed paste, so a shell doesn't run each line as it arrives
        #[arg(long)]
        paste: bool,
        /// Press Enter after it
        #[arg(long)]
        enter: bool,
        #[command(flatten)]
        action: Action,
    },
    /// Open a window like the source window, in its focused tab's directory
    NewWindow {
        #[command(flatten)]
        source: SourceWindow,
        #[command(flatten)]
        action: Action,
    },
    /// Run a program in a new window, without a shell
    LaunchWindow {
        #[command(flatten)]
        source: SourceWindow,
        #[command(flatten)]
        launch: Launch,
    },
    /// Open a shell tab and print its ID
    NewTab {
        #[command(flatten)]
        window: WindowTarget,
        #[command(flatten)]
        action: Action,
    },
    /// Run a program in a new tab, without a shell
    LaunchTab {
        #[command(flatten)]
        window: WindowTarget,
        /// Open the tab without selecting it
        #[arg(long)]
        background: bool,
        #[command(flatten)]
        launch: Launch,
    },
    /// Split a pane and print the new pane's ID
    SplitPane {
        #[command(flatten)]
        pane: PaneTarget,
        direction: Direction,
        #[command(flatten)]
        action: Action,
    },
    /// Ask the desktop to focus a window; refused on Wayland, where the compositor decides
    FocusWindow {
        #[command(flatten)]
        window: WindowTarget,
        #[command(flatten)]
        action: Action,
    },
    /// Select a tab without raising its window
    SelectTab {
        #[command(flatten)]
        tab: TabTarget,
        #[command(flatten)]
        action: Action,
    },
    /// Select a tab and focus one of its panes
    FocusPane {
        #[command(flatten)]
        pane: PaneTarget,
        #[command(flatten)]
        action: Action,
    },
    /// Close a window
    CloseWindow {
        #[command(flatten)]
        window: WindowTarget,
        #[command(flatten)]
        action: Action,
    },
    /// Close a tab and end its programs
    CloseTab {
        #[command(flatten)]
        tab: TabTarget,
        #[command(flatten)]
        action: Action,
    },
    /// Close a pane and end its program; a tab's last pane closes only with the tab
    ClosePane {
        #[command(flatten)]
        pane: PaneTarget,
        #[command(flatten)]
        action: Action,
    },
    /// Move a tab to a position in the same or another window and print its new ID
    MoveTab {
        #[command(flatten)]
        tab: TabTarget,
        /// The window to move it to
        #[arg(value_parser = window_id)]
        destination_window: u64,
        /// Its position in that window's tab bar, counting from 0
        destination_index: u16,
        #[command(flatten)]
        action: Action,
    },
    /// Move a pane next to a pane in another tab, keeping its program running
    MovePane {
        #[command(flatten)]
        pane: PaneTarget,
        /// The window to move it to
        #[arg(value_parser = window_id)]
        destination_window: u64,
        /// The tab to move it to
        destination_tab: u64,
        /// The pane to put it next to
        target_pane: u32,
        /// Below (horizontal) or to the right of (vertical) that pane
        direction: Direction,
        #[command(flatten)]
        action: Action,
    },
    /// Ask for a window size in physical pixels; the window manager may adjust it
    ResizeWindow {
        #[command(flatten)]
        window: WindowTarget,
        /// Width in physical pixels
        width: u32,
        /// Height in physical pixels
        height: u32,
        #[command(flatten)]
        action: Action,
    },
    /// Set the ratio of the split at one divider
    ResizeDivider {
        #[command(flatten)]
        tab: TabTarget,
        /// Divider ID, as `list` shows it
        divider: u64,
        /// The first side's share, in thousandths from 100 to 900
        ratio: u16,
        #[command(flatten)]
        action: Action,
    },
}

#[derive(Args)]
struct LaunchTarget {
    /// Termeric process ID, as `list` shows it
    #[arg(value_parser = process_id)]
    process: u32,
    /// The request ID the launch printed
    #[arg(value_parser = request_id)]
    request_id: String,
}

#[derive(Args)]
struct SourceWindow {
    /// Termeric process ID, as `list` shows it
    #[arg(value_parser = process_id)]
    process: u32,
    /// The window whose configuration and directory to copy
    #[arg(value_parser = window_id)]
    source_window: u64,
}

#[derive(Args)]
struct WindowTarget {
    /// Termeric process ID, as `list` shows it
    #[arg(value_parser = process_id)]
    process: u32,
    /// Window ID within that process
    #[arg(value_parser = window_id)]
    window: u64,
}

#[derive(Args)]
struct TabTarget {
    #[command(flatten)]
    window: WindowTarget,
    /// Tab ID within that window
    tab: u64,
}

#[derive(Args)]
struct PaneTarget {
    #[command(flatten)]
    tab: TabTarget,
    /// Pane ID within that tab
    pane: u32,
}

/// What every action takes.
#[derive(Args)]
struct Action {
    /// Use this ID (32 lowercase hex digits), so a retry with it never acts twice
    #[arg(long, value_name = "ID", value_parser = request_id)]
    request_id: Option<String>,
    /// Print the result as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct Launch {
    /// Start in this absolute directory
    #[arg(long, value_name = "ABSOLUTE_DIRECTORY")]
    cwd: Option<String>,
    /// Title for the tab
    #[arg(long, allow_hyphen_values = true)]
    title: Option<String>,
    /// Set an environment variable; repeat for more. Process listings show it, so pass secrets with --env-stdin
    #[arg(long = "env", value_name = "NAME=VALUE", value_parser = environment_variable)]
    environment: Vec<EnvironmentVariable>,
    /// Read more variables from standard input, as one JSON object of names to strings
    #[arg(long)]
    env_stdin: bool,
    /// Remove an inherited variable; repeat for more
    #[arg(long = "unset-env", value_name = "NAME")]
    unset: Vec<String>,
    /// Start from a minimal environment instead of Termeric's
    #[arg(long)]
    clear_env: bool,
    /// Wait for the program to exit, then exit with its code
    #[arg(long)]
    wait: bool,
    /// With --wait, stop waiting after SECONDS (1-86400); the program keeps running
    #[arg(long, value_name = "SECONDS", value_parser = timeout, requires = "wait")]
    timeout: Option<u32>,
    #[command(flatten)]
    action: Action,
    /// The program and its arguments, after `--`
    #[arg(last = true, required = true, value_name = "PROGRAM")]
    command: Vec<String>,
}

impl ControlCli {
    pub(super) fn into_command(self) -> anyhow::Result<ControlCommand> {
        let (process_id, request, action) = match self.command {
            ControlArgs::List { json } => return Ok(ControlCommand::List { json }),
            ControlArgs::WaitLaunch {
                launch,
                timeout,
                json,
            } => {
                return Ok(ControlCommand::WaitLaunch {
                    process_id: launch.process,
                    launch_request_id: launch.request_id,
                    timeout_seconds: timeout,
                    json,
                });
            }
            ControlArgs::LaunchStatus { launch, json } => {
                return Ok(ControlCommand::LaunchStatus {
                    process_id: launch.process,
                    launch_request_id: launch.request_id,
                    json,
                });
            }
            ControlArgs::GetText {
                pane,
                history_lines,
                max_bytes,
                json,
            } => {
                let request = Request::ReadText {
                    window_id: pane.tab.window.window,
                    tab_id: pane.tab.tab,
                    pane_id: pane.pane,
                    history_lines,
                    max_bytes,
                };
                let action = Action {
                    request_id: None,
                    json,
                };
                (pane.tab.window.process, request, action)
            }
            ControlArgs::SendText {
                pane,
                text,
                paste,
                enter,
                action,
            } => (
                pane.tab.window.process,
                Request::SendText {
                    request_id: String::new(),
                    window_id: pane.tab.window.window,
                    tab_id: pane.tab.tab,
                    pane_id: pane.pane,
                    text,
                    paste,
                    enter,
                },
                action,
            ),
            ControlArgs::NewWindow { source, action } => (
                source.process,
                Request::CreateWindow {
                    request_id: String::new(),
                    source_window: source.source_window,
                },
                action,
            ),
            ControlArgs::LaunchWindow { source, launch } => {
                return launch.into_command(source.process, |program, args, launch| {
                    Request::LaunchWindow {
                        request_id: String::new(),
                        source_window: source.source_window,
                        program,
                        args,
                        working_directory: launch.cwd,
                        title: launch.title,
                        environment: launch.environment,
                        environment_policy: launch.policy,
                    }
                });
            }
            ControlArgs::NewTab { window, action } => (
                window.process,
                Request::CreateTab {
                    request_id: String::new(),
                    window_id: window.window,
                },
                action,
            ),
            ControlArgs::LaunchTab {
                window,
                background,
                launch,
            } => {
                return launch.into_command(window.process, |program, args, launch| {
                    Request::LaunchTab {
                        background,
                        request_id: String::new(),
                        window_id: window.window,
                        program,
                        args,
                        working_directory: launch.cwd,
                        title: launch.title,
                        environment: launch.environment,
                        environment_policy: launch.policy,
                    }
                });
            }
            ControlArgs::SplitPane {
                pane,
                direction,
                action,
            } => (
                pane.tab.window.process,
                Request::SplitPane {
                    request_id: String::new(),
                    window_id: pane.tab.window.window,
                    tab_id: pane.tab.tab,
                    pane_id: pane.pane,
                    direction: direction.into(),
                },
                action,
            ),
            ControlArgs::FocusWindow { window, action } => (
                window.process,
                Request::FocusWindow {
                    request_id: String::new(),
                    window_id: window.window,
                },
                action,
            ),
            ControlArgs::SelectTab { tab, action } => (
                tab.window.process,
                Request::SelectTab {
                    request_id: String::new(),
                    window_id: tab.window.window,
                    tab_id: tab.tab,
                },
                action,
            ),
            ControlArgs::FocusPane { pane, action } => (
                pane.tab.window.process,
                Request::FocusPane {
                    request_id: String::new(),
                    window_id: pane.tab.window.window,
                    tab_id: pane.tab.tab,
                    pane_id: pane.pane,
                },
                action,
            ),
            ControlArgs::CloseWindow { window, action } => (
                window.process,
                Request::CloseWindow {
                    request_id: String::new(),
                    window_id: window.window,
                },
                action,
            ),
            ControlArgs::CloseTab { tab, action } => (
                tab.window.process,
                Request::CloseTab {
                    request_id: String::new(),
                    window_id: tab.window.window,
                    tab_id: tab.tab,
                },
                action,
            ),
            ControlArgs::ClosePane { pane, action } => (
                pane.tab.window.process,
                Request::ClosePane {
                    request_id: String::new(),
                    window_id: pane.tab.window.window,
                    tab_id: pane.tab.tab,
                    pane_id: pane.pane,
                },
                action,
            ),
            ControlArgs::MoveTab {
                tab,
                destination_window,
                destination_index,
                action,
            } => (
                tab.window.process,
                Request::MoveTab {
                    request_id: String::new(),
                    source_window_id: tab.window.window,
                    tab_id: tab.tab,
                    destination_window_id: destination_window,
                    destination_index,
                },
                action,
            ),
            ControlArgs::MovePane {
                pane,
                destination_window,
                destination_tab,
                target_pane,
                direction,
                action,
            } => (
                pane.tab.window.process,
                Request::MovePane {
                    request_id: String::new(),
                    source_window_id: pane.tab.window.window,
                    source_tab_id: pane.tab.tab,
                    pane_id: pane.pane,
                    destination_window_id: destination_window,
                    destination_tab_id: destination_tab,
                    target_pane_id: target_pane,
                    direction: direction.into(),
                },
                action,
            ),
            ControlArgs::ResizeWindow {
                window,
                width,
                height,
                action,
            } => (
                window.process,
                Request::ResizeWindow {
                    request_id: String::new(),
                    window_id: window.window,
                    width,
                    height,
                },
                action,
            ),
            ControlArgs::ResizeDivider {
                tab,
                divider,
                ratio,
                action,
            } => (
                tab.window.process,
                Request::ResizeDivider {
                    request_id: String::new(),
                    window_id: tab.window.window,
                    tab_id: tab.tab,
                    divider_id: divider,
                    ratio_millis: ratio,
                },
                action,
            ),
        };
        let request = checked(request, &action.request_id)?;
        Ok(ControlCommand::Send {
            process_id,
            request,
            request_id: action.request_id,
            json: action.json,
        })
    }
}

/// What a launch request takes from [`Launch`] besides its argv.
struct LaunchPlace {
    cwd: Option<String>,
    title: Option<String>,
    environment: Vec<EnvironmentVariable>,
    policy: termeric_core::pty::EnvironmentPolicy,
}

impl Launch {
    fn into_command(
        self,
        process_id: u32,
        request: impl FnOnce(String, Vec<String>, LaunchPlace) -> Request,
    ) -> anyhow::Result<ControlCommand> {
        let mut command = self.command.into_iter();
        let program = command.next().expect("clap requires PROGRAM");
        let place = LaunchPlace {
            cwd: self.cwd,
            title: self.title,
            environment: self.environment,
            policy: termeric_core::pty::EnvironmentPolicy {
                clear: self.clear_env,
                unset: self.unset,
            },
        };
        let request = checked(
            request(program, command.collect(), place),
            &self.action.request_id,
        )?;
        Ok(ControlCommand::Launch {
            process_id,
            request,
            request_id: self.action.request_id,
            env_stdin: self.env_stdin,
            wait: self.wait.then_some(Wait {
                timeout_seconds: self.timeout,
            }),
            json: self.action.json,
        })
    }
}

/// `request`, checked before anything connects. An action is checked under
/// its explicit ID or a placeholder; sending fills in the real one.
fn checked(mut request: Request, request_id: &Option<String>) -> anyhow::Result<Request> {
    let mut probe = request.clone();
    if let Some(slot) = probe.request_id_mut() {
        *slot = request_id.clone().unwrap_or_else(|| "0".repeat(32));
    }
    probe.canonicalize();
    probe.validate().map_err(anyhow::Error::msg)?;
    request.canonicalize();
    Ok(request)
}

fn process_id(value: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .ok()
        .filter(|process| protocol::valid_process_id(*process))
        .ok_or_else(|| format!("GUI process IDs are 1 to {}", protocol::MAX_PROCESS_ID))
}

fn window_id(value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|window| *window != 0)
        .ok_or_else(|| "GUI window IDs are nonzero integers".into())
}

fn request_id(value: &str) -> Result<String, String> {
    if protocol::valid_request_id(value) {
        Ok(value.to_owned())
    } else {
        Err("request IDs are exactly 32 lowercase hex digits".into())
    }
}

fn timeout(value: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .ok()
        .filter(|seconds| (1..=crate::control::MAX_WAIT_SECONDS).contains(seconds))
        .ok_or_else(|| {
            format!(
                "--timeout must be 1-{} whole seconds",
                crate::control::MAX_WAIT_SECONDS
            )
        })
}

fn environment_variable(value: &str) -> Result<EnvironmentVariable, String> {
    let (name, value) = value
        .split_once('=')
        .ok_or_else(|| "--env requires NAME=VALUE".to_string())?;
    Ok(EnvironmentVariable {
        name: name.to_owned(),
        value: value.to_owned(),
    })
}
