//! Deterministic command-line parsing into window-local startup plans.

use anyhow::Context;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub windows: Vec<WindowLaunch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowLaunch {
    /// Explicitly request a fresh first window and suppress automatic restore.
    pub force_new: bool,
    pub workspace: Option<String>,
    pub domain: Option<crate::domain::DomainTarget>,
    /// Authenticated snapshot prepared off the event-loop thread by an in-app
    /// attachment workflow. Direct CLI attachment leaves this empty.
    pub domain_snapshot: Option<crate::domain::model::DomainSnapshot>,
    pub tabs: Vec<TabLaunch>,
    pub app_id: Option<String>,
    pub pixel_size: Option<(u32, u32)>,
    /// Process-wide renderer override copied to every initial window.
    pub renderer_backend: Option<crate::config::RendererBackend>,
    /// Launch the process-owned drop-down window, or toggle the existing one.
    pub quick_access: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabLaunch {
    pub working_directory: Option<PathBuf>,
    pub title: Option<String>,
    pub target: LaunchTarget,
    pub splits: Vec<CliSplitDirection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchTarget {
    Shell,
    Command { program: String, args: Vec<String> },
    TerminalProfile { name: String },
    Plugin { plugin_id: String, profile: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliSplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseOutcome {
    Run(LaunchPlan),
    QuickAccess(LaunchPlan),
    Control(crate::control::ControlCommand),
    Domain(crate::domain::DomainCommand),
    Plugin(crate::plugin::runtime::PluginAdminCommand),
    SerialBridge(crate::serial::BridgeOptions),
    Print(String),
}

impl Default for LaunchPlan {
    fn default() -> Self {
        Self {
            windows: vec![WindowLaunch::default()],
        }
    }
}

impl Default for WindowLaunch {
    fn default() -> Self {
        Self {
            force_new: false,
            workspace: None,
            domain: None,
            domain_snapshot: None,
            tabs: vec![TabLaunch::default()],
            app_id: None,
            pixel_size: None,
            renderer_backend: None,
            quick_access: false,
        }
    }
}

impl Default for TabLaunch {
    fn default() -> Self {
        Self {
            working_directory: None,
            title: None,
            target: LaunchTarget::Shell,
            splits: Vec::new(),
        }
    }
}

pub fn parse_env() -> anyhow::Result<ParseOutcome> {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    if arguments.first().and_then(|value| value.to_str()) == Some("domain")
        && arguments.get(1).and_then(|value| value.to_str()) == Some("--host-profile")
    {
        let config = crate::config::Config::load_or_default()?;
        return parse_collected(arguments, &config.domains);
    }
    parse_collected(arguments, &crate::config::DomainsConfig::default())
}

#[cfg(test)]
pub fn parse(arguments: impl IntoIterator<Item = OsString>) -> anyhow::Result<ParseOutcome> {
    let arguments: Vec<OsString> = arguments.into_iter().collect();
    parse_collected(arguments, &crate::config::DomainsConfig::default())
}

fn parse_collected(
    arguments: Vec<OsString>,
    domains: &crate::config::DomainsConfig,
) -> anyhow::Result<ParseOutcome> {
    if crate::serial::is_internal_command(arguments.first()) {
        return Ok(ParseOutcome::SerialBridge(
            crate::serial::BridgeOptions::parse(&arguments[1..])?,
        ));
    }
    if arguments.first().and_then(|argument| argument.to_str()) == Some("domain") {
        return parse_domain(&arguments[1..], domains);
    }
    if arguments.first().and_then(|argument| argument.to_str()) == Some("control") {
        return parse_control(&arguments[1..]);
    }
    if arguments.first().and_then(|argument| argument.to_str()) == Some("plugin") {
        return parse_plugin(&arguments[1..]);
    }
    let mut plan = LaunchPlan::default();
    let mut renderer_backend = None;
    let mut index = 0usize;
    while index < arguments.len() {
        let argument = arguments[index]
            .to_str()
            .context("command-line options must be valid UTF-8")?;
        match argument {
            "-h" | "--help" => return Ok(ParseOutcome::Print(help_text())),
            "-V" | "--version" => {
                return Ok(ParseOutcome::Print(format!(
                    "termeric {}",
                    env!("CARGO_PKG_VERSION")
                )));
            }
            // A leading --new-window is a desktop-launch compatibility flag:
            // this invocation already creates a native window. Later uses
            // delimit additional window groups in the same process.
            "--new-window" if index == 0 => current_window(&mut plan).force_new = true,
            "--new-window" => plan.windows.push(WindowLaunch::default()),
            "--quick-access" => {
                let window = current_window(&mut plan);
                if window.quick_access {
                    anyhow::bail!("--quick-access may only be specified once");
                }
                window.quick_access = true;
                window.force_new = true;
            }
            "--tab" => {
                let window = current_window(&mut plan);
                reject_workspace_mix(window, "--tab")?;
                window.tabs.push(TabLaunch::default());
            }
            "--split" => {
                let value = next_utf8(&arguments, &mut index, "--split")?;
                let direction = match value.as_str() {
                    "horizontal" | "h" => CliSplitDirection::Horizontal,
                    "vertical" | "v" => CliSplitDirection::Vertical,
                    _ => anyhow::bail!("--split expects horizontal or vertical, got {value:?}"),
                };
                let window = current_window(&mut plan);
                reject_workspace_mix(window, "--split")?;
                current_tab(window).splits.push(direction);
            }
            "--working-directory" | "--cwd" => {
                let value = next_os(&arguments, &mut index, "--working-directory")?;
                let window = current_window(&mut plan);
                reject_workspace_mix(window, "--working-directory")?;
                current_tab(window).working_directory = Some(PathBuf::from(value));
            }
            "--title" => {
                let value = next_utf8(&arguments, &mut index, "--title")?;
                validate_single_line("window/tab title", &value)?;
                let window = current_window(&mut plan);
                reject_workspace_mix(window, "--title")?;
                current_tab(window).title = Some(value);
            }
            "--command" => {
                let program = next_utf8(&arguments, &mut index, "--command")?;
                let window = current_window(&mut plan);
                reject_workspace_mix(window, "--command")?;
                set_target(
                    current_tab(window),
                    LaunchTarget::Command {
                        program,
                        args: Vec::new(),
                    },
                    "--command",
                )?;
            }
            "--terminal-profile" => {
                let name = next_utf8(&arguments, &mut index, "--terminal-profile")?;
                validate_identifier("terminal profile", &name)?;
                let window = current_window(&mut plan);
                reject_workspace_mix(window, "--terminal-profile")?;
                set_target(
                    current_tab(window),
                    LaunchTarget::TerminalProfile { name },
                    "--terminal-profile",
                )?;
            }
            "--profile" => {
                let value = next_utf8(&arguments, &mut index, "--profile")?;
                let (plugin_id, profile) = value.split_once(':').ok_or_else(|| {
                    anyhow::anyhow!("--profile expects PLUGIN:PROFILE (for example codex:review)")
                })?;
                validate_identifier("plugin", plugin_id)?;
                validate_single_line("profile", profile)?;
                let window = current_window(&mut plan);
                reject_workspace_mix(window, "--profile")?;
                set_target(
                    current_tab(window),
                    LaunchTarget::Plugin {
                        plugin_id: plugin_id.to_string(),
                        profile: profile.to_string(),
                    },
                    "--profile",
                )?;
            }
            "--workspace" => {
                let value = next_utf8(&arguments, &mut index, "--workspace")?;
                crate::workspace::validate_workspace_id(&value)?;
                let window = current_window(&mut plan);
                if !window.tabs_are_default() {
                    anyhow::bail!(
                        "--workspace cannot be combined with tab, cwd, command, profile, title, or split options in the same window"
                    );
                }
                window.workspace = Some(value);
            }
            "--geometry" => {
                let value = next_utf8(&arguments, &mut index, "--geometry")?;
                current_window(&mut plan).pixel_size = Some(parse_geometry(&value)?);
            }
            "--class" | "--app-id" => {
                let value = next_utf8(&arguments, &mut index, argument)?;
                validate_identifier("application identity", &value)?;
                current_window(&mut plan).app_id = Some(value);
            }
            "--renderer" => {
                let value = next_utf8(&arguments, &mut index, "--renderer")?;
                let parsed = match value.as_str() {
                    "auto" => crate::config::RendererBackend::Auto,
                    "gpu" => crate::config::RendererBackend::Gpu,
                    "software" => crate::config::RendererBackend::Software,
                    _ => anyhow::bail!("--renderer expects auto, gpu, or software, got {value:?}"),
                };
                if renderer_backend.replace(parsed).is_some() {
                    anyhow::bail!("--renderer may only be specified once");
                }
            }
            "--" => {
                let remaining = &arguments[index + 1..];
                if remaining.is_empty() {
                    anyhow::bail!("-- must be followed by a command");
                }
                let values: Vec<String> = remaining
                    .iter()
                    .map(|value| {
                        value
                            .to_str()
                            .map(String::from)
                            .context("command argv must be valid UTF-8")
                    })
                    .collect::<anyhow::Result<_>>()?;
                let window = current_window(&mut plan);
                reject_workspace_mix(window, "--")?;
                set_target(
                    current_tab(window),
                    LaunchTarget::Command {
                        program: values[0].clone(),
                        args: values[1..].to_vec(),
                    },
                    "--",
                )?;
                index = arguments.len();
                continue;
            }
            _ if argument.starts_with('-') => {
                anyhow::bail!("unknown option {argument:?}; use --help for supported options")
            }
            _ => anyhow::bail!(
                "unexpected positional argument {argument:?}; put arbitrary commands after --"
            ),
        }
        index += 1;
    }
    for window in &mut plan.windows {
        window.renderer_backend = renderer_backend;
    }
    let quick_access_windows = plan
        .windows
        .iter()
        .filter(|window| window.quick_access)
        .count();
    if quick_access_windows > 0 {
        if plan.windows.len() != 1 || quick_access_windows != 1 {
            anyhow::bail!("--quick-access requires exactly one window group");
        }
        Ok(ParseOutcome::QuickAccess(plan))
    } else {
        Ok(ParseOutcome::Run(plan))
    }
}

impl WindowLaunch {
    fn tabs_are_default(&self) -> bool {
        self.tabs == [TabLaunch::default()]
    }
}

fn current_window(plan: &mut LaunchPlan) -> &mut WindowLaunch {
    plan.windows
        .last_mut()
        .expect("LaunchPlan always contains one window")
}

fn current_tab(window: &mut WindowLaunch) -> &mut TabLaunch {
    window
        .tabs
        .last_mut()
        .expect("WindowLaunch always contains one tab")
}

fn reject_workspace_mix(window: &WindowLaunch, option: &str) -> anyhow::Result<()> {
    if window.workspace.is_some() {
        anyhow::bail!("{option} cannot be combined with --workspace in the same window");
    }
    Ok(())
}

fn set_target(tab: &mut TabLaunch, target: LaunchTarget, option: &str) -> anyhow::Result<()> {
    if !matches!(tab.target, LaunchTarget::Shell) {
        anyhow::bail!("{option} conflicts with an earlier command or profile for this tab");
    }
    tab.target = target;
    Ok(())
}

fn next_os(arguments: &[OsString], index: &mut usize, option: &str) -> anyhow::Result<OsString> {
    *index += 1;
    arguments
        .get(*index)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{option} requires a value"))
}

fn next_utf8(arguments: &[OsString], index: &mut usize, option: &str) -> anyhow::Result<String> {
    next_os(arguments, index, option)?
        .into_string()
        .map_err(|_| anyhow::anyhow!("{option} value must be valid UTF-8"))
}

fn parse_geometry(value: &str) -> anyhow::Result<(u32, u32)> {
    let (width, height) = value
        .split_once(['x', 'X'])
        .ok_or_else(|| anyhow::anyhow!("--geometry expects WIDTHxHEIGHT in physical pixels"))?;
    let width: u32 = width.parse().context("invalid --geometry width")?;
    let height: u32 = height.parse().context("invalid --geometry height")?;
    if !(160..=16_384).contains(&width) || !(120..=16_384).contains(&height) {
        anyhow::bail!("--geometry must be between 160x120 and 16384x16384 pixels");
    }
    Ok((width, height))
}

fn validate_identifier(label: &str, value: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        anyhow::bail!("invalid {label} {value:?}");
    }
    Ok(())
}

fn validate_single_line(label: &str, value: &str) -> anyhow::Result<()> {
    if value.is_empty() || value.chars().count() > 256 || value.contains(['\n', '\r', '\0']) {
        anyhow::bail!("{label} must contain 1 to 256 single-line characters");
    }
    Ok(())
}

fn help_text() -> String {
    format!(
        "Termeric {}\n\n\
Usage: termeric [WINDOW/TAB OPTIONS] [-- COMMAND ARG...]\n\n\
Window grouping:\n  \
  --new-window                 Start another independent native window\n  \
  --quick-access               Toggle or launch one top-edge drop-down window\n  \
  --tab                        Start another tab in the current window\n  \
  --split horizontal|vertical  Split the current shell tab after launch\n\n\
Launch selection:\n  \
  --working-directory PATH     Set the current tab's cwd\n  \
  --command PROGRAM            Run PROGRAM directly without arguments\n  \
  --                           Preserve all remaining command arguments exactly\n  \
  --terminal-profile NAME      Launch a configured reusable terminal profile\n  \
  --profile PLUGIN:PROFILE     Launch a built-in plugin with a named profile\n  \
  --workspace ID               Restore one complete workspace (exclusive)\n  \
  --title TITLE                Set the current tab title\n\n\
Native identity and size:\n  \
  --geometry WIDTHxHEIGHT      Initial physical-pixel window size\n  \
  --class NAME, --app-id NAME  Set X11 WM_CLASS / Wayland application ID\n\n\
Rendering:\n  \
  --renderer auto|gpu|software Override adapter selection for every window\n\n\
General:\n  \
  domain SUBCOMMAND            Manage persistent local terminal domains\n  \
  control SUBCOMMAND           Inspect or explicitly control a running GUI\n  \
  plugin SUBCOMMAND            Manage declarative third-party plugins\n  \
  -h, --help                   Print this help\n  \
  -V, --version                Print the version",
        env!("CARGO_PKG_VERSION")
    )
}

fn parse_control(arguments: &[OsString]) -> anyhow::Result<ParseOutcome> {
    let Some(command) = arguments.first().and_then(|argument| argument.to_str()) else {
        return Ok(ParseOutcome::Print(control_help_text()));
    };
    if matches!(command, "-h" | "--help" | "help") {
        return Ok(ParseOutcome::Print(control_help_text()));
    }
    match command {
        "list" => Ok(ParseOutcome::Control(
            crate::control::ControlCommand::List {
                json: parse_only_json(&arguments[1..], "control list")?,
            },
        )),
        "get-text" => {
            let process_id = parse_control_process_id(arguments.get(1))?;
            let window_id = parse_control_window_id(arguments.get(2))?;
            let tab_id = parse_control_tab_id(arguments.get(3))?;
            let pane_id = parse_control_pane_id(arguments.get(4))?;
            let mut history_lines = 0_u16;
            let mut max_bytes = crate::control::protocol::MAX_TERMINAL_TEXT_BYTES;
            let mut history_seen = false;
            let mut bytes_seen = false;
            let mut json = false;
            let mut index = 5;
            while index < arguments.len() {
                match domain_utf8(arguments.get(index), "invalid get-text option")?.as_str() {
                    "--history-lines" if !history_seen => {
                        index += 1;
                        history_lines = domain_utf8(
                            arguments.get(index),
                            "--history-lines requires a line count",
                        )?
                        .parse::<u16>()
                        .context("invalid terminal-text history line count")?;
                        if history_lines > crate::control::protocol::MAX_TEXT_HISTORY_LINES {
                            anyhow::bail!(
                                "terminal-text history is limited to {} lines",
                                crate::control::protocol::MAX_TEXT_HISTORY_LINES
                            );
                        }
                        history_seen = true;
                    }
                    "--bytes" if !bytes_seen => {
                        index += 1;
                        max_bytes =
                            domain_utf8(arguments.get(index), "--bytes requires a byte count")?
                                .parse::<u32>()
                                .context("invalid terminal-text byte limit")?;
                        if !(1..=crate::control::protocol::MAX_TERMINAL_TEXT_BYTES)
                            .contains(&max_bytes)
                        {
                            anyhow::bail!(
                                "terminal-text byte limit must be between 1 and {}",
                                crate::control::protocol::MAX_TERMINAL_TEXT_BYTES
                            );
                        }
                        bytes_seen = true;
                    }
                    "--json" if !json => json = true,
                    option => anyhow::bail!("unknown or duplicate get-text option {option:?}"),
                }
                index += 1;
            }
            Ok(ParseOutcome::Control(
                crate::control::ControlCommand::GetText {
                    process_id,
                    window_id,
                    tab_id,
                    pane_id,
                    history_lines,
                    max_bytes,
                    json,
                },
            ))
        }
        "send-text" => {
            let process_id = parse_control_process_id(arguments.get(1))?;
            let window_id = parse_control_window_id(arguments.get(2))?;
            let tab_id = parse_control_tab_id(arguments.get(3))?;
            let pane_id = parse_control_pane_id(arguments.get(4))?;
            let text = domain_utf8(arguments.get(5), "control send-text requires TEXT")?;
            let mut paste = false;
            let mut enter = false;
            let mut request_id = None;
            let mut json = false;
            let mut index = 6;
            while index < arguments.len() {
                match domain_utf8(arguments.get(index), "invalid send-text option")?.as_str() {
                    "--paste" if !paste => paste = true,
                    "--enter" if !enter => enter = true,
                    "--json" if !json => json = true,
                    "--request-id" if request_id.is_none() => {
                        index += 1;
                        let value = domain_utf8(
                            arguments.get(index),
                            "--request-id requires 32 lowercase hex digits",
                        )?;
                        if !crate::control::protocol::valid_request_id(&value) {
                            anyhow::bail!("--request-id must be exactly 32 lowercase hex digits");
                        }
                        request_id = Some(value);
                    }
                    option => anyhow::bail!("unknown or duplicate send-text option {option:?}"),
                }
                index += 1;
            }
            if text.len() > crate::control::protocol::MAX_CONTROL_INPUT_BYTES {
                anyhow::bail!(
                    "control input is limited to {} UTF-8 bytes",
                    crate::control::protocol::MAX_CONTROL_INPUT_BYTES
                );
            }
            if text.is_empty() && !enter {
                anyhow::bail!("control input must include text or --enter");
            }
            if !crate::control::protocol::valid_control_input_text(&text) {
                anyhow::bail!("control input permits printable UTF-8, tab, and newline only");
            }
            Ok(ParseOutcome::Control(
                crate::control::ControlCommand::SendText {
                    process_id,
                    window_id,
                    tab_id,
                    pane_id,
                    text,
                    paste,
                    enter,
                    request_id,
                    json,
                },
            ))
        }
        "new-window" => {
            let process_id = parse_control_process_id(arguments.get(1))?;
            let source_window = parse_control_window_id(arguments.get(2))?;
            let (request_id, json) = parse_control_action_options(arguments, 3, "new-window")?;
            Ok(ParseOutcome::Control(
                crate::control::ControlCommand::NewWindow {
                    process_id,
                    source_window,
                    request_id,
                    json,
                },
            ))
        }
        "new-tab" => {
            let process_id = parse_control_process_id(arguments.get(1))?;
            let window_id = parse_control_window_id(arguments.get(2))?;
            let (request_id, json) = parse_control_action_options(arguments, 3, "new-tab")?;
            Ok(ParseOutcome::Control(
                crate::control::ControlCommand::NewTab {
                    process_id,
                    window_id,
                    request_id,
                    json,
                },
            ))
        }
        "split-pane" => {
            let process_id = parse_control_process_id(arguments.get(1))?;
            let window_id = parse_control_window_id(arguments.get(2))?;
            let tab_id = parse_control_tab_id(arguments.get(3))?;
            let pane_id = parse_control_pane_id(arguments.get(4))?;
            let direction = match domain_utf8(
                arguments.get(5),
                "control split-pane requires horizontal or vertical",
            )?
            .as_str()
            {
                "horizontal" => crate::control::protocol::SplitDirection::Horizontal,
                "vertical" => crate::control::protocol::SplitDirection::Vertical,
                other => anyhow::bail!(
                    "invalid split direction {other:?}; expected horizontal or vertical"
                ),
            };
            let (request_id, json) = parse_control_action_options(arguments, 6, "split-pane")?;
            Ok(ParseOutcome::Control(
                crate::control::ControlCommand::SplitPane {
                    process_id,
                    window_id,
                    tab_id,
                    pane_id,
                    direction,
                    request_id,
                    json,
                },
            ))
        }
        "focus-window" => {
            let process_id = parse_control_process_id(arguments.get(1))?;
            let window_id = parse_control_window_id(arguments.get(2))?;
            let (request_id, json) = parse_control_action_options(arguments, 3, "focus-window")?;
            Ok(ParseOutcome::Control(
                crate::control::ControlCommand::FocusWindow {
                    process_id,
                    window_id,
                    request_id,
                    json,
                },
            ))
        }
        "select-tab" => {
            let process_id = parse_control_process_id(arguments.get(1))?;
            let window_id = parse_control_window_id(arguments.get(2))?;
            let tab_id = parse_control_tab_id(arguments.get(3))?;
            let (request_id, json) = parse_control_action_options(arguments, 4, "select-tab")?;
            Ok(ParseOutcome::Control(
                crate::control::ControlCommand::SelectTab {
                    process_id,
                    window_id,
                    tab_id,
                    request_id,
                    json,
                },
            ))
        }
        "focus-pane" => {
            let process_id = parse_control_process_id(arguments.get(1))?;
            let window_id = parse_control_window_id(arguments.get(2))?;
            let tab_id = parse_control_tab_id(arguments.get(3))?;
            let pane_id = parse_control_pane_id(arguments.get(4))?;
            let (request_id, json) = parse_control_action_options(arguments, 5, "focus-pane")?;
            Ok(ParseOutcome::Control(
                crate::control::ControlCommand::FocusPane {
                    process_id,
                    window_id,
                    tab_id,
                    pane_id,
                    request_id,
                    json,
                },
            ))
        }
        "close-window" => {
            let process_id = parse_control_process_id(arguments.get(1))?;
            let window_id = parse_control_window_id(arguments.get(2))?;
            let (request_id, json) = parse_control_action_options(arguments, 3, "close-window")?;
            Ok(ParseOutcome::Control(
                crate::control::ControlCommand::CloseWindow {
                    process_id,
                    window_id,
                    request_id,
                    json,
                },
            ))
        }
        "close-tab" => {
            let process_id = parse_control_process_id(arguments.get(1))?;
            let window_id = parse_control_window_id(arguments.get(2))?;
            let tab_id = parse_control_tab_id(arguments.get(3))?;
            let (request_id, json) = parse_control_action_options(arguments, 4, "close-tab")?;
            Ok(ParseOutcome::Control(
                crate::control::ControlCommand::CloseTab {
                    process_id,
                    window_id,
                    tab_id,
                    request_id,
                    json,
                },
            ))
        }
        "close-pane" => {
            let process_id = parse_control_process_id(arguments.get(1))?;
            let window_id = parse_control_window_id(arguments.get(2))?;
            let tab_id = parse_control_tab_id(arguments.get(3))?;
            let pane_id = parse_control_pane_id(arguments.get(4))?;
            let (request_id, json) = parse_control_action_options(arguments, 5, "close-pane")?;
            Ok(ParseOutcome::Control(
                crate::control::ControlCommand::ClosePane {
                    process_id,
                    window_id,
                    tab_id,
                    pane_id,
                    request_id,
                    json,
                },
            ))
        }
        "move-tab" => {
            let process_id = parse_control_process_id(arguments.get(1))?;
            let source_window_id = parse_control_window_id(arguments.get(2))?;
            let tab_id = parse_control_tab_id(arguments.get(3))?;
            let destination_window_id = parse_control_window_id(arguments.get(4))?;
            let destination_index = domain_utf8(
                arguments.get(5),
                "control move-tab requires DESTINATION_INDEX",
            )?
            .parse::<u16>()
            .context("invalid destination tab index")?;
            if usize::from(destination_index) > crate::control::protocol::MAX_TABS_PER_WINDOW {
                anyhow::bail!("destination tab index exceeds the window tab limit");
            }
            let (request_id, json) = parse_control_action_options(arguments, 6, "move-tab")?;
            Ok(ParseOutcome::Control(
                crate::control::ControlCommand::MoveTab {
                    process_id,
                    source_window_id,
                    tab_id,
                    destination_window_id,
                    destination_index,
                    request_id,
                    json,
                },
            ))
        }
        "move-pane" => {
            let process_id = parse_control_process_id(arguments.get(1))?;
            let source_window_id = parse_control_window_id(arguments.get(2))?;
            let source_tab_id = parse_control_tab_id(arguments.get(3))?;
            let pane_id = parse_control_pane_id(arguments.get(4))?;
            let destination_window_id = parse_control_window_id(arguments.get(5))?;
            let destination_tab_id = parse_control_tab_id(arguments.get(6))?;
            let target_pane_id = parse_control_pane_id(arguments.get(7))?;
            let direction = match domain_utf8(
                arguments.get(8),
                "control move-pane requires horizontal or vertical",
            )?
            .as_str()
            {
                "horizontal" => crate::control::protocol::SplitDirection::Horizontal,
                "vertical" => crate::control::protocol::SplitDirection::Vertical,
                other => anyhow::bail!(
                    "invalid split direction {other:?}; expected horizontal or vertical"
                ),
            };
            let (request_id, json) = parse_control_action_options(arguments, 9, "move-pane")?;
            Ok(ParseOutcome::Control(
                crate::control::ControlCommand::MovePane {
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
                },
            ))
        }
        "resize-window" => {
            let process_id = parse_control_process_id(arguments.get(1))?;
            let window_id = parse_control_window_id(arguments.get(2))?;
            let width = domain_utf8(arguments.get(3), "control resize-window requires WIDTH")?
                .parse::<u32>()
                .context("invalid window width")?;
            let height = domain_utf8(arguments.get(4), "control resize-window requires HEIGHT")?
                .parse::<u32>()
                .context("invalid window height")?;
            if !(crate::control::protocol::MIN_WINDOW_WIDTH
                ..=crate::control::protocol::MAX_WINDOW_DIMENSION)
                .contains(&width)
                || !(crate::control::protocol::MIN_WINDOW_HEIGHT
                    ..=crate::control::protocol::MAX_WINDOW_DIMENSION)
                    .contains(&height)
            {
                anyhow::bail!(
                    "window dimensions must be between {}x{} and {}x{} pixels",
                    crate::control::protocol::MIN_WINDOW_WIDTH,
                    crate::control::protocol::MIN_WINDOW_HEIGHT,
                    crate::control::protocol::MAX_WINDOW_DIMENSION,
                    crate::control::protocol::MAX_WINDOW_DIMENSION
                );
            }
            let (request_id, json) = parse_control_action_options(arguments, 5, "resize-window")?;
            Ok(ParseOutcome::Control(
                crate::control::ControlCommand::ResizeWindow {
                    process_id,
                    window_id,
                    width,
                    height,
                    request_id,
                    json,
                },
            ))
        }
        "resize-divider" => {
            let process_id = parse_control_process_id(arguments.get(1))?;
            let window_id = parse_control_window_id(arguments.get(2))?;
            let tab_id = parse_control_tab_id(arguments.get(3))?;
            let divider_id =
                domain_utf8(arguments.get(4), "control resize-divider requires DIVIDER")?
                    .parse::<u64>()
                    .context("invalid target divider ID")?;
            let ratio_millis =
                domain_utf8(arguments.get(5), "control resize-divider requires RATIO")?
                    .parse::<u16>()
                    .context("invalid divider ratio")?;
            if !(100..=900).contains(&ratio_millis) {
                anyhow::bail!("divider ratio must be between 100 and 900 thousandths");
            }
            let (request_id, json) = parse_control_action_options(arguments, 6, "resize-divider")?;
            Ok(ParseOutcome::Control(
                crate::control::ControlCommand::ResizeDivider {
                    process_id,
                    window_id,
                    tab_id,
                    divider_id,
                    ratio_millis,
                    request_id,
                    json,
                },
            ))
        }
        _ => anyhow::bail!("unknown control command {command:?}; use `termeric control --help`"),
    }
}

fn parse_control_action_options(
    arguments: &[OsString],
    start: usize,
    command: &str,
) -> anyhow::Result<(Option<String>, bool)> {
    let mut request_id = None;
    let mut json = false;
    let mut index = start;
    while index < arguments.len() {
        let option = domain_utf8(arguments.get(index), "invalid control option")?;
        match option.as_str() {
            "--json" if !json => json = true,
            "--request-id" if request_id.is_none() => {
                index += 1;
                let value = domain_utf8(
                    arguments.get(index),
                    "--request-id requires 32 lowercase hex digits",
                )?;
                if !crate::control::protocol::valid_request_id(&value) {
                    anyhow::bail!("--request-id must be exactly 32 lowercase hex digits");
                }
                request_id = Some(value);
            }
            _ => anyhow::bail!("unknown or duplicate control {command} option {option:?}"),
        }
        index += 1;
    }
    Ok((request_id, json))
}

fn parse_control_process_id(value: Option<&OsString>) -> anyhow::Result<u32> {
    let value = domain_utf8(value, "control action requires PROCESS")?;
    let parsed = value.parse::<u32>().context("invalid GUI process ID")?;
    if parsed == 0 || parsed > i32::MAX as u32 {
        anyhow::bail!("GUI process ID must be between 1 and {}", i32::MAX);
    }
    Ok(parsed)
}

fn parse_control_window_id(value: Option<&OsString>) -> anyhow::Result<u64> {
    let value = domain_utf8(value, "control action requires WINDOW")?;
    let parsed = value.parse::<u64>().context("invalid GUI window ID")?;
    if parsed == 0 {
        anyhow::bail!("GUI window ID must be nonzero");
    }
    Ok(parsed)
}

fn parse_control_tab_id(value: Option<&OsString>) -> anyhow::Result<u64> {
    domain_utf8(value, "control action requires TAB")?
        .parse::<u64>()
        .context("invalid target tab ID")
}

fn parse_control_pane_id(value: Option<&OsString>) -> anyhow::Result<u32> {
    domain_utf8(value, "control action requires PANE")?
        .parse::<u32>()
        .context("invalid target pane ID")
}

fn control_help_text() -> String {
    "Usage: termeric control SUBCOMMAND\n\n\
Commands for already-running local GUI processes:\n  \
  list [--json]\n      List bounded window, tab, and pane metadata\n  \
  get-text PROCESS WINDOW TAB PANE [--history-lines N] [--bytes N] [--json]\n      Read bounded presented UTF-8 text from one exact pane\n  \
  send-text PROCESS WINDOW TAB PANE TEXT [--paste] [--enter] [--request-id ID] [--json]\n      Send opt-in bounded UTF-8 text to one exact pane\n  \
  new-window PROCESS SOURCE_WINDOW [--request-id ID] [--json]\n      Create a window using the exact source window's configuration and cwd\n  \
  new-tab PROCESS WINDOW [--request-id ID] [--json]\n      Create a local tab in one exact stable window\n  \
  split-pane PROCESS WINDOW TAB PANE horizontal|vertical [--request-id ID] [--json]\n      Split one exact stable local pane and return the new pane ID\n  \
  focus-window PROCESS WINDOW [--request-id ID] [--json]\n      Request native focus for one exact stable window\n  \
  select-tab PROCESS WINDOW TAB [--request-id ID] [--json]\n      Select one exact stable tab without focusing its native window\n  \
  focus-pane PROCESS WINDOW TAB PANE [--request-id ID] [--json]\n      Select the tab and focus one exact stable pane\n  \
  close-window PROCESS WINDOW [--request-id ID] [--json]\n      Schedule one exact stable native window to close\n  \
  close-tab PROCESS WINDOW TAB [--request-id ID] [--json]\n      Schedule one exact stable local tab to close\n  \
  close-pane PROCESS WINDOW TAB PANE [--request-id ID] [--json]\n      Close one exact stable local pane immediately\n  \
  move-tab PROCESS SOURCE_WINDOW TAB DESTINATION_WINDOW DESTINATION_INDEX [--request-id ID] [--json]\n      Move one exact local tab to a zero-based visual index\n  \
  move-pane PROCESS SOURCE_WINDOW SOURCE_TAB PANE DESTINATION_WINDOW DESTINATION_TAB TARGET_PANE horizontal|vertical [--request-id ID] [--json]\n      Move one exact local pane beside an exact destination pane\n  \
  resize-window PROCESS WINDOW WIDTH HEIGHT [--request-id ID] [--json]\n      Request an exact native window inner size in physical pixels\n  \
  resize-divider PROCESS WINDOW TAB DIVIDER RATIO [--request-id ID] [--json]\n      Set one exact local divider to 100-900 thousandths\n\n\
The endpoint is private to the current Unix user. Text access is explicit and\n\
bounded; input is opt-in and typed, while argv, environment, clipboard data, and provider credentials remain hidden."
        .into()
}

fn parse_plugin(arguments: &[OsString]) -> anyhow::Result<ParseOutcome> {
    use crate::plugin::runtime::{PluginAdminCommand, PluginConfigurationChange, PluginPermission};
    use std::collections::BTreeSet;

    let Some(command) = arguments.first().and_then(|argument| argument.to_str()) else {
        return Ok(ParseOutcome::Print(plugin_help_text()));
    };
    if matches!(command, "-h" | "--help" | "help") {
        return Ok(ParseOutcome::Print(plugin_help_text()));
    }
    let values = &arguments[1..];
    let admin = match command {
        "list" => PluginAdminCommand::List {
            json: parse_only_json(values, "plugin list")?,
        },
        "show" => {
            let id = domain_utf8(values.first(), "plugin show requires ID")?;
            let json = parse_only_json(&values[1..], "plugin show ID")?;
            PluginAdminCommand::Show { id, json }
        }
        "validate" => {
            require_count(values, 1, "plugin validate DIRECTORY")?;
            PluginAdminCommand::Validate {
                directory: PathBuf::from(&values[0]),
            }
        }
        "install" => {
            let directory = values
                .first()
                .ok_or_else(|| anyhow::anyhow!("plugin install requires DIRECTORY"))?;
            let mut grants = BTreeSet::new();
            let mut enable = false;
            let mut replace = false;
            let mut index = 1;
            while index < values.len() {
                match values[index].to_str() {
                    Some("--allow") => {
                        index += 1;
                        let permission = values
                            .get(index)
                            .and_then(|value| value.to_str())
                            .ok_or_else(|| anyhow::anyhow!("--allow requires PERMISSION"))?
                            .parse::<PluginPermission>()?;
                        grants.insert(permission);
                    }
                    Some("--enable") => enable = true,
                    Some("--replace") => replace = true,
                    Some(value) => anyhow::bail!("unknown plugin install option {value:?}"),
                    None => anyhow::bail!("plugin install options must be valid UTF-8"),
                }
                index += 1;
            }
            PluginAdminCommand::Install {
                directory: PathBuf::from(directory),
                grants,
                enable,
                replace,
            }
        }
        "update" | "enable" | "disable" | "remove" => {
            require_count(values, 1, &format!("plugin {command} ID"))?;
            let id = domain_utf8(values.first(), "plugin command requires ID")?;
            match command {
                "update" => PluginAdminCommand::Update { id },
                "enable" => PluginAdminCommand::Enable { id },
                "disable" => PluginAdminCommand::Disable { id },
                _ => PluginAdminCommand::Remove { id },
            }
        }
        "configure" => {
            let id = domain_utf8(values.first(), "plugin configure requires ID")?;
            let change = match values.get(1).and_then(|value| value.to_str()) {
                Some("--default-profile") => {
                    require_count(values, 3, "plugin configure ID --default-profile PROFILE")?;
                    PluginConfigurationChange::DefaultProfile(domain_utf8(
                        values.get(2),
                        "--default-profile requires PROFILE",
                    )?)
                }
                Some("--set") => {
                    require_count(values, 4, "plugin configure ID --set KEY VALUE")?;
                    PluginConfigurationChange::Set {
                        key: domain_utf8(values.get(2), "--set requires KEY VALUE")?,
                        value: domain_utf8(values.get(3), "--set requires KEY VALUE")?,
                    }
                }
                Some("--unset") => {
                    require_count(values, 3, "plugin configure ID --unset KEY")?;
                    PluginConfigurationChange::Unset {
                        key: domain_utf8(values.get(2), "--unset requires KEY")?,
                    }
                }
                Some("--reset-settings") => {
                    require_count(values, 2, "plugin configure ID --reset-settings")?;
                    PluginConfigurationChange::ResetSettings
                }
                _ => anyhow::bail!(
                    "plugin configure expects --default-profile, --set, --unset, or --reset-settings"
                ),
            };
            PluginAdminCommand::Configure { id, change }
        }
        _ => anyhow::bail!("unknown plugin command {command:?}; use `termeric plugin --help`"),
    };
    Ok(ParseOutcome::Plugin(admin))
}

fn plugin_help_text() -> String {
    "Termeric declarative plugin management\n\n\
Usage: termeric plugin SUBCOMMAND\n\n\
  list [--json]\n\
  show ID [--json]\n\
  validate DIRECTORY\n\
  install DIRECTORY [--allow PERMISSION] [--enable] [--replace]\n\
  update ID\n\
  enable ID\n\
  disable ID\n\
  configure ID --default-profile PROFILE\n\
  configure ID --set KEY VALUE\n\
  configure ID --unset KEY\n\
  configure ID --reset-settings\n\
  remove ID\n\n\
API v1 supports process-spawn; v2 adds workspace-query. API v3 adds bounded\n\
event-subscribe handlers and permissioned informational ui-actions. API v4 adds\n\
separately granted terminal-text for bounded command-completed events. API v5 adds opt-in\n\
correlated persistent event handlers. API v6 adds a granted current-window workspace query.\n\
API v7 adds separately granted stable-ID tab selection and pane focus.\n\
API v8 adds separately granted tab ordering and divider-ratio adjustment.\n\
API v9 adds bounded namespaced handler commands with exact-owner dispatch.\n\
API v10 adds bounded typed non-secret settings with explicit user overrides.\n\
API v11 adds a separately granted bounded exact-pane presented-terminal query.\n\
API v12 adds separately granted exact ordinary-pane creation.\n\
API v13 adds separately granted ordinary-tab creation in the current window.\n\
Installation records the\n\
canonical local source and a manifest fingerprint; it never downloads code."
        .into()
}

fn require_count(values: &[OsString], expected: usize, usage: &str) -> anyhow::Result<()> {
    if values.len() != expected {
        anyhow::bail!("usage: termeric {usage}");
    }
    Ok(())
}

fn parse_domain(
    arguments: &[OsString],
    domains: &crate::config::DomainsConfig,
) -> anyhow::Result<ParseOutcome> {
    use crate::domain::DomainCommand;
    use crate::domain::model::SplitDirection;

    let Some(command) = arguments.first().and_then(|argument| argument.to_str()) else {
        return Ok(ParseOutcome::Print(domain_help_text()));
    };
    if matches!(command, "-h" | "--help" | "help") {
        return Ok(ParseOutcome::Print(domain_help_text()));
    }
    if command == "--host" {
        if arguments.len() < 3 {
            anyhow::bail!("usage: termeric domain --host SSH_DESTINATION SUBCOMMAND ...");
        }
        let destination = domain_utf8(arguments.get(1), "--host requires SSH_DESTINATION")?;
        crate::domain::validate_ssh_destination(&destination)?;
        return apply_remote_authority(destination, parse_domain(&arguments[2..], domains)?);
    }
    if command == "--host-profile" {
        if arguments.len() < 3 {
            anyhow::bail!("usage: termeric domain --host-profile NAME SUBCOMMAND ...");
        }
        let name = domain_utf8(
            arguments.get(1),
            "--host-profile requires a configured profile name",
        )?;
        let destination = domains.ssh_host(&name)?.destination.clone();
        return apply_remote_authority(destination, parse_domain(&arguments[2..], domains)?);
    }
    let values = &arguments[1..];
    let parsed = match command {
        "protocol-version" => DomainCommand::ProtocolVersion {
            json: parse_only_json(values, "domain protocol-version")?,
        },
        "hosts" => DomainCommand::Hosts {
            json: parse_only_json(values, "domain hosts")?,
        },
        "server" => {
            require_exact(values, &["--foreground"], "domain server --foreground")?;
            DomainCommand::ServerForeground
        }
        "proxy-stdio" => {
            require_len(values, 0, "domain proxy-stdio")?;
            DomainCommand::ProxyStdio
        }
        "list" => DomainCommand::List {
            json: parse_only_json(values, "domain list")?,
        },
        "uploads" => DomainCommand::Uploads {
            json: parse_only_json(values, "domain uploads")?,
        },
        "cancel-upload" => {
            if values.is_empty() || values.len() > 2 {
                anyhow::bail!("usage: termeric domain cancel-upload UPLOAD_ID [--json]");
            }
            let upload_id = domain_utf8(values.first(), "cancel-upload requires UPLOAD_ID")?;
            crate::domain::protocol::validate_upload_id(&upload_id)?;
            let json = values.len() == 2
                && domain_utf8(values.get(1), "invalid cancel-upload option")? == "--json";
            if values.len() == 2 && !json {
                anyhow::bail!("unknown cancel-upload option");
            }
            DomainCommand::CancelUpload { upload_id, json }
        }
        "workspaces" | "panes" | "show" => {
            let (domain, json) = parse_id_and_json(values, command)?;
            match command {
                "workspaces" => DomainCommand::Workspaces { domain, json },
                "panes" => DomainCommand::Panes { domain, json },
                _ => DomainCommand::Show { domain, json },
            }
        }
        "create" => {
            let name = domain_utf8(values.first(), "domain create requires NAME")?;
            let (spawn, json) = parse_spawn_options(&values[1..])?;
            DomainCommand::Create { name, spawn, json }
        }
        "spawn" => {
            let domain = domain_utf8(values.first(), "domain spawn requires DOMAIN")?;
            crate::domain::model::validate_id("domain id", &domain)?;
            let (spawn, json) = parse_spawn_options(&values[1..])?;
            DomainCommand::Spawn {
                domain,
                spawn,
                json,
            }
        }
        "split" => {
            if values.len() < 3 {
                anyhow::bail!(
                    "domain split requires DOMAIN PANE horizontal|vertical [launch options]"
                );
            }
            let domain = domain_utf8(values.first(), "domain split requires DOMAIN")?;
            crate::domain::model::validate_id("domain id", &domain)?;
            let pane = parse_pane_id(values.get(1))?;
            let direction =
                match domain_utf8(values.get(2), "domain split requires direction")?.as_str() {
                    "horizontal" | "h" => SplitDirection::Horizontal,
                    "vertical" | "v" => SplitDirection::Vertical,
                    value => anyhow::bail!(
                        "domain split direction must be horizontal or vertical, got {value:?}"
                    ),
                };
            let (spawn, json) = parse_spawn_options(&values[3..])?;
            DomainCommand::Split {
                domain,
                pane,
                direction,
                spawn,
                json,
            }
        }
        "resize-split" => {
            require_len(values, 3, "domain resize-split DOMAIN SPLIT RATIO_MILLIS")?;
            let ratio_millis: u16 =
                domain_utf8(values.get(2), "domain resize-split requires RATIO_MILLIS")?
                    .parse()
                    .context("split ratio must be an unsigned integer")?;
            if !(100..=900).contains(&ratio_millis) {
                anyhow::bail!("split ratio must be between 100 and 900 thousandths");
            }
            DomainCommand::ResizeSplit {
                domain: validated_domain_id(values.first())?,
                split: parse_split_id(values.get(1))?,
                ratio_millis,
            }
        }
        "rename-tab" => {
            require_len(values, 3, "domain rename-tab DOMAIN TAB TITLE")?;
            let title = domain_utf8(values.get(2), "domain rename-tab requires TITLE")?;
            let title = (!title.trim().is_empty()).then_some(title);
            if let Some(title) = &title {
                crate::domain::model::validate_name("tab title", title)?;
            }
            DomainCommand::RenameTab {
                domain: validated_domain_id(values.first())?,
                tab: parse_tab_id(values.get(1))?,
                title,
            }
        }
        "pin-tab" | "unpin-tab" => {
            require_len(values, 2, &format!("domain {command} DOMAIN TAB"))?;
            DomainCommand::SetTabPinned {
                domain: validated_domain_id(values.first())?,
                tab: parse_tab_id(values.get(1))?,
                pinned: command == "pin-tab",
            }
        }
        "move-tab" => {
            require_len(values, 3, "domain move-tab DOMAIN TAB INDEX")?;
            let index = domain_utf8(values.get(2), "domain move-tab requires INDEX")?
                .parse()
                .context("tab index must be an unsigned integer")?;
            DomainCommand::MoveTab {
                domain: validated_domain_id(values.first())?,
                tab: parse_tab_id(values.get(1))?,
                index,
            }
        }
        "swap-pane" => {
            require_len(values, 2, "domain swap-pane DOMAIN PANE")?;
            DomainCommand::SwapPaneNext {
                domain: validated_domain_id(values.first())?,
                pane: parse_pane_id(values.get(1))?,
            }
        }
        "balance-tab" => {
            require_len(values, 2, "domain balance-tab DOMAIN TAB")?;
            DomainCommand::BalanceTab {
                domain: validated_domain_id(values.first())?,
                tab: parse_tab_id(values.get(1))?,
            }
        }
        "layout-tab" => {
            require_len(values, 3, "domain layout-tab DOMAIN TAB columns|rows|grid")?;
            let preset =
                match domain_utf8(values.get(2), "domain layout-tab requires PRESET")?.as_str() {
                    "columns" => crate::domain::model::LivePaneLayoutPreset::Columns,
                    "rows" => crate::domain::model::LivePaneLayoutPreset::Rows,
                    "grid" => crate::domain::model::LivePaneLayoutPreset::Grid,
                    value => anyhow::bail!(
                        "domain layout preset must be columns, rows, or grid, got {value:?}"
                    ),
                };
            DomainCommand::ApplyPaneLayout {
                domain: validated_domain_id(values.first())?,
                tab: parse_tab_id(values.get(1))?,
                preset,
            }
        }
        "move-pane" => {
            require_len(values, 3, "domain move-pane DOMAIN PANE TAB")?;
            DomainCommand::MovePane {
                domain: validated_domain_id(values.first())?,
                pane: parse_pane_id(values.get(1))?,
                tab: parse_tab_id(values.get(2))?,
            }
        }
        "focus" => {
            require_len(values, 2, "domain focus DOMAIN PANE")?;
            DomainCommand::Focus {
                domain: validated_domain_id(values.first())?,
                pane: parse_pane_id(values.get(1))?,
            }
        }
        "close-pane" => {
            require_len(values, 2, "domain close-pane DOMAIN PANE")?;
            DomainCommand::ClosePane {
                domain: validated_domain_id(values.first())?,
                pane: parse_pane_id(values.get(1))?,
            }
        }
        "close-tab" => {
            require_len(values, 2, "domain close-tab DOMAIN TAB")?;
            DomainCommand::CloseTab {
                domain: validated_domain_id(values.first())?,
                tab: parse_tab_id(values.get(1))?,
            }
        }
        "send" => parse_domain_send(values)?,
        "get-text" => parse_domain_get_text(values)?,
        "upload" => parse_domain_upload(values)?,
        "attach" => {
            if values.is_empty() {
                anyhow::bail!(
                    "usage: termeric domain attach DOMAIN [--geometry WIDTHxHEIGHT] [--app-id NAME]"
                );
            }
            let domain_id = validated_domain_id(values.first())?;
            let mut window = WindowLaunch {
                domain: Some(crate::domain::DomainTarget {
                    authority: crate::domain::DomainAuthority::Local,
                    domain_id,
                }),
                ..WindowLaunch::default()
            };
            let mut index = 1;
            while index < values.len() {
                let option = domain_utf8(values.get(index), "invalid domain attach option")?;
                match option.as_str() {
                    "--geometry" => {
                        index += 1;
                        window.pixel_size = Some(parse_geometry(&domain_utf8(
                            values.get(index),
                            "--geometry requires WIDTHxHEIGHT",
                        )?)?);
                    }
                    "--class" | "--app-id" => {
                        index += 1;
                        let identity = domain_utf8(values.get(index), "--app-id requires NAME")?;
                        validate_identifier("application identity", &identity)?;
                        window.app_id = Some(identity);
                    }
                    _ => anyhow::bail!("unknown domain attach option {option:?}"),
                }
                index += 1;
            }
            return Ok(ParseOutcome::Run(LaunchPlan {
                windows: vec![window],
            }));
        }
        "relay" => {
            require_len(values, 2, "domain relay DOMAIN PANE")?;
            DomainCommand::Relay {
                domain: validated_domain_id(values.first())?,
                pane: parse_pane_id(values.get(1))?,
            }
        }
        "kill" => {
            require_len(values, 1, "domain kill DOMAIN")?;
            DomainCommand::Kill {
                domain: validated_domain_id(values.first())?,
            }
        }
        "stop-server" => {
            require_len(values, 0, "domain stop-server")?;
            DomainCommand::StopServer
        }
        _ => anyhow::bail!("unknown domain command {command:?}; use `termeric domain --help`"),
    };
    Ok(ParseOutcome::Domain(parsed))
}

fn apply_remote_authority(
    destination: String,
    outcome: ParseOutcome,
) -> anyhow::Result<ParseOutcome> {
    match outcome {
        ParseOutcome::Domain(
            crate::domain::DomainCommand::ServerForeground
            | crate::domain::DomainCommand::ProxyStdio
            | crate::domain::DomainCommand::ProtocolVersion { .. }
            | crate::domain::DomainCommand::Hosts { .. }
            | crate::domain::DomainCommand::Remote { .. },
        ) => anyhow::bail!(
            "server, inventory, protocol-version, and transport commands cannot be nested remotely"
        ),
        ParseOutcome::Domain(command) => {
            Ok(ParseOutcome::Domain(crate::domain::DomainCommand::Remote {
                destination,
                command: Box::new(command),
            }))
        }
        ParseOutcome::Run(mut plan) => {
            for window in &mut plan.windows {
                if let Some(target) = &mut window.domain {
                    target.authority = crate::domain::DomainAuthority::Ssh {
                        destination: destination.clone(),
                    };
                }
            }
            Ok(ParseOutcome::Run(plan))
        }
        ParseOutcome::QuickAccess(mut plan) => {
            for window in &mut plan.windows {
                if let Some(target) = &mut window.domain {
                    target.authority = crate::domain::DomainAuthority::Ssh {
                        destination: destination.clone(),
                    };
                }
            }
            Ok(ParseOutcome::QuickAccess(plan))
        }
        ParseOutcome::Print(text) => Ok(ParseOutcome::Print(text)),
        ParseOutcome::SerialBridge(_) => {
            anyhow::bail!("the internal serial bridge cannot be nested through a remote domain")
        }
        ParseOutcome::Plugin(_) | ParseOutcome::Control(_) => {
            unreachable!("domain parser cannot return plugin or GUI control commands")
        }
    }
}

fn parse_spawn_options(
    arguments: &[OsString],
) -> anyhow::Result<(crate::domain::model::SpawnSpec, bool)> {
    let mut spawn = crate::domain::model::SpawnSpec::default();
    let mut json = false;
    let mut index = 0usize;
    while index < arguments.len() {
        let argument = domain_utf8(arguments.get(index), "invalid domain launch option")?;
        match argument.as_str() {
            "--working-directory" | "--cwd" => {
                index += 1;
                let value = arguments
                    .get(index)
                    .cloned()
                    .context("--working-directory requires PATH")?;
                spawn.working_directory = Some(
                    value
                        .into_string()
                        .map_err(|_| anyhow::anyhow!("domain cwd must be valid UTF-8"))?,
                );
            }
            "--title" => {
                index += 1;
                spawn.title = Some(domain_utf8(arguments.get(index), "--title requires TITLE")?);
            }
            "--size" => {
                index += 1;
                let value = domain_utf8(arguments.get(index), "--size requires COLSxROWS")?;
                let (cols, rows) = parse_cell_geometry(&value)?;
                spawn.cols = cols;
                spawn.rows = rows;
            }
            "--json" => json = true,
            "--" => {
                let command = &arguments[index + 1..];
                if command.is_empty() {
                    anyhow::bail!("-- must be followed by a domain command program");
                }
                let command: Vec<String> = command
                    .iter()
                    .map(|value| {
                        value
                            .to_str()
                            .map(String::from)
                            .context("domain command argv must be valid UTF-8")
                    })
                    .collect::<anyhow::Result<_>>()?;
                spawn.program = Some(command[0].clone());
                spawn.args = command[1..].to_vec();
                index = arguments.len();
                continue;
            }
            _ => anyhow::bail!("unknown domain launch option {argument:?}"),
        }
        index += 1;
    }
    spawn.validate()?;
    Ok((spawn, json))
}

fn parse_domain_send(arguments: &[OsString]) -> anyhow::Result<crate::domain::DomainCommand> {
    if arguments.len() < 3 || arguments.len() > 4 {
        anyhow::bail!("usage: termeric domain send DOMAIN PANE TEXT [--enter]");
    }
    let domain = validated_domain_id(arguments.first())?;
    let pane = parse_pane_id(arguments.get(1))?;
    let text = domain_utf8(arguments.get(2), "domain send requires TEXT")?;
    let enter =
        arguments.len() == 4 && domain_utf8(arguments.get(3), "invalid send option")? == "--enter";
    if arguments.len() == 4 && !enter {
        anyhow::bail!("the only domain send option is --enter");
    }
    let mut data = text.into_bytes();
    if enter {
        data.push(b'\r');
    }
    if data.len() > crate::domain::protocol::MAX_STREAM_CHUNK_BYTES {
        anyhow::bail!("domain send input exceeds 64 KiB");
    }
    Ok(crate::domain::DomainCommand::Send { domain, pane, data })
}

fn parse_domain_get_text(arguments: &[OsString]) -> anyhow::Result<crate::domain::DomainCommand> {
    if arguments.len() < 2 {
        anyhow::bail!("usage: termeric domain get-text DOMAIN PANE [--bytes N] [--json]");
    }
    let domain = validated_domain_id(arguments.first())?;
    let pane = parse_pane_id(arguments.get(1))?;
    let mut max_bytes = 64 * 1024;
    let mut json = false;
    let mut index = 2;
    while index < arguments.len() {
        match domain_utf8(arguments.get(index), "invalid get-text option")?.as_str() {
            "--bytes" => {
                index += 1;
                max_bytes = domain_utf8(arguments.get(index), "--bytes requires N")?
                    .parse()
                    .context("--bytes must be a positive integer")?;
                if !(1..=crate::domain::model::MAX_TEXT_RESPONSE_BYTES).contains(&max_bytes) {
                    anyhow::bail!("--bytes must be between 1 and 1048576");
                }
            }
            "--json" => json = true,
            option => anyhow::bail!("unknown get-text option {option:?}"),
        }
        index += 1;
    }
    Ok(crate::domain::DomainCommand::GetText {
        domain,
        pane,
        max_bytes,
        json,
    })
}

fn parse_domain_upload(arguments: &[OsString]) -> anyhow::Result<crate::domain::DomainCommand> {
    if arguments.len() < 3 {
        anyhow::bail!(
            "usage: termeric domain upload DOMAIN PANE SOURCE [--name NAME] [--retain|--resume ID] [--json]"
        );
    }
    let domain = validated_domain_id(arguments.first())?;
    let pane = parse_pane_id(arguments.get(1))?;
    let source =
        std::path::PathBuf::from(arguments.get(2).context("domain upload requires SOURCE")?);
    let mut name = None;
    let mut retain = false;
    let mut resume = None;
    let mut json = false;
    let mut index = 3;
    while index < arguments.len() {
        match domain_utf8(arguments.get(index), "invalid upload option")?.as_str() {
            "--name" => {
                index += 1;
                let value = domain_utf8(arguments.get(index), "--name requires NAME")?;
                crate::domain::protocol::validate_upload_name(&value)?;
                name = Some(value);
            }
            "--json" => json = true,
            "--retain" => retain = true,
            "--resume" => {
                index += 1;
                let upload_id = domain_utf8(arguments.get(index), "--resume requires UPLOAD_ID")?;
                crate::domain::protocol::validate_upload_id(&upload_id)?;
                resume = Some(upload_id);
            }
            option => anyhow::bail!("unknown upload option {option:?}"),
        }
        index += 1;
    }
    if retain && resume.is_some() {
        anyhow::bail!("--retain and --resume cannot be used together");
    }
    Ok(crate::domain::DomainCommand::Upload {
        domain,
        pane,
        source,
        name,
        retain,
        resume,
        json,
    })
}

fn parse_id_and_json(arguments: &[OsString], command: &str) -> anyhow::Result<(String, bool)> {
    if arguments.is_empty() || arguments.len() > 2 {
        anyhow::bail!("usage: termeric domain {command} DOMAIN [--json]");
    }
    let domain = validated_domain_id(arguments.first())?;
    let json = arguments.len() == 2 && domain_utf8(arguments.get(1), "invalid option")? == "--json";
    if arguments.len() == 2 && !json {
        anyhow::bail!("the only supported option is --json");
    }
    Ok((domain, json))
}

fn parse_only_json(arguments: &[OsString], usage: &str) -> anyhow::Result<bool> {
    match arguments {
        [] => Ok(false),
        [argument] if argument.to_str() == Some("--json") => Ok(true),
        _ => anyhow::bail!("usage: termeric {usage} [--json]"),
    }
}

fn validated_domain_id(value: Option<&OsString>) -> anyhow::Result<String> {
    let value = domain_utf8(value, "domain id is required")?;
    crate::domain::model::validate_id("domain id", &value)?;
    Ok(value)
}

fn parse_pane_id(value: Option<&OsString>) -> anyhow::Result<u32> {
    domain_utf8(value, "pane id is required")?
        .parse()
        .context("pane id must be an unsigned integer")
}

fn parse_tab_id(value: Option<&OsString>) -> anyhow::Result<u32> {
    domain_utf8(value, "tab id is required")?
        .parse()
        .context("tab id must be an unsigned integer")
}

fn parse_split_id(value: Option<&OsString>) -> anyhow::Result<u32> {
    domain_utf8(value, "split id is required")?
        .parse()
        .context("split id must be an unsigned integer")
}

fn parse_cell_geometry(value: &str) -> anyhow::Result<(u16, u16)> {
    let (cols, rows) = value
        .split_once(['x', 'X'])
        .context("--size expects COLSxROWS")?;
    let cols: u16 = cols.parse().context("invalid --size columns")?;
    let rows: u16 = rows.parse().context("invalid --size rows")?;
    if cols == 0 || rows == 0 || cols > 1_000 || rows > 1_000 {
        anyhow::bail!("--size must be between 1x1 and 1000x1000 cells");
    }
    Ok((cols, rows))
}

fn domain_utf8(value: Option<&OsString>, message: &str) -> anyhow::Result<String> {
    value
        .and_then(|value| value.to_str())
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("{message}"))
}

fn require_len(arguments: &[OsString], length: usize, usage: &str) -> anyhow::Result<()> {
    if arguments.len() != length {
        anyhow::bail!("usage: termeric {usage}");
    }
    Ok(())
}

fn require_exact(arguments: &[OsString], exact: &[&str], usage: &str) -> anyhow::Result<()> {
    if arguments.len() != exact.len()
        || arguments
            .iter()
            .zip(exact)
            .any(|(actual, expected)| actual.to_str() != Some(*expected))
    {
        anyhow::bail!("usage: termeric {usage}");
    }
    Ok(())
}

fn domain_help_text() -> String {
    "Persistent terminal domains (experimental)\n\n\
Usage: termeric domain SUBCOMMAND ...\n\n\
Remote control: termeric domain --host SSH_DESTINATION SUBCOMMAND ...\n\n\
Saved remote: termeric domain --host-profile NAME SUBCOMMAND ...\n\n\
Inspect:\n  \
  protocol-version [--json]             Print the local client's wire version\n  \
  hosts [--json]                       List configured SSH host profiles\n  \
  list [--json]                         List live domains\n  \
  uploads [--json]                      List retained partial uploads\n  \
  show DOMAIN [--json]                  Describe complete live state\n  \
  workspaces DOMAIN [--json]            List live workspaces\n  \
  panes DOMAIN [--json]                 List panes\n\n\
Create and control:\n  \
  create NAME [OPTIONS] [-- COMMAND...] Create a persistent domain\n  \
  spawn DOMAIN [OPTIONS] [-- COMMAND...] Add a tab\n  \
  split DOMAIN PANE h|v [OPTIONS] [-- COMMAND...]\n  \
  resize-split DOMAIN SPLIT RATIO       Resize a stable divider (100-900)\n  \
  rename-tab DOMAIN TAB TITLE           Set title; an empty title clears it\n  \
  pin-tab DOMAIN TAB                    Pin a tab\n  \
  unpin-tab DOMAIN TAB                  Unpin a tab\n  \
  move-tab DOMAIN TAB INDEX             Move in its pin group (zero-based)\n  \
  swap-pane DOMAIN PANE                 Swap with next pane in visual order\n  \
  balance-tab DOMAIN TAB                Reset every divider to 50%\n  \
  layout-tab DOMAIN TAB PRESET          Apply columns, rows, or grid\n  \
  move-pane DOMAIN PANE TAB             Move a pane to a stable destination tab\n  \
  focus DOMAIN PANE                     Set persisted focus\n  \
  close-pane DOMAIN PANE                Terminate and remove one split pane\n  \
  close-tab DOMAIN TAB                  Terminate and remove one tab\n  \
  send DOMAIN PANE TEXT [--enter]       Send bounded UTF-8 input\n  \
  get-text DOMAIN PANE [--bytes N] [--json]\n  \
  upload DOMAIN PANE SOURCE [--name NAME] [--retain|--resume ID] [--json]\n  \
                                          Upload one file/directory without overwrite\n  \
  cancel-upload UPLOAD_ID [--json]       Delete one inactive retained upload\n  \
  attach DOMAIN [WINDOW OPTIONS]        Open the domain in a GUI\n  \
  kill DOMAIN                           Terminate its processes\n  \
  stop-server                           Stop an empty daemon\n\n\
Launch OPTIONS: --working-directory PATH, --title TITLE, --size COLSxROWS\n\n\
Attach WINDOW OPTIONS: --geometry WIDTHxHEIGHT, --app-id NAME\n\n\
The daemon socket always remains local and user-private. `--host` starts a\n\
non-interactive OpenSSH stdio bridge using an already trusted host key and\n\
key/agent authentication. `--host-profile NAME` resolves a validated entry in\n\
`[domains.ssh_hosts]`; OpenSSH config remains authoritative for ports, keys,\n\
proxies, certificates, and host policy. Remote attachment uses the resolved\n\
authority for topology, controls, focus, and every pane relay."
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(arguments: &[&str]) -> LaunchPlan {
        let result = parse(arguments.iter().map(OsString::from)).unwrap();
        let ParseOutcome::Run(plan) = result else {
            panic!("expected launch plan");
        };
        plan
    }

    #[test]
    fn arbitrary_argv_after_separator_is_preserved_without_option_parsing() {
        let plan = run(&["--", "printf", "--help", "two words", "λ"]);
        assert_eq!(
            plan.windows[0].tabs[0].target,
            LaunchTarget::Command {
                program: "printf".into(),
                args: vec!["--help".into(), "two words".into(), "λ".into()]
            }
        );
    }

    #[test]
    fn reusable_terminal_profiles_are_distinct_from_plugin_profiles() {
        let plan = run(&[
            "--terminal-profile",
            "dev.rust",
            "--working-directory",
            "/tmp",
            "--title",
            "Override",
            "--split",
            "vertical",
            "--tab",
            "--profile",
            "codex:review",
        ]);
        assert_eq!(
            plan.windows[0].tabs[0].target,
            LaunchTarget::TerminalProfile {
                name: "dev.rust".into()
            }
        );
        assert_eq!(
            plan.windows[0].tabs[0].working_directory,
            Some(PathBuf::from("/tmp"))
        );
        assert_eq!(plan.windows[0].tabs[0].title.as_deref(), Some("Override"));
        assert_eq!(
            plan.windows[0].tabs[1].target,
            LaunchTarget::Plugin {
                plugin_id: "codex".into(),
                profile: "review".into()
            }
        );
    }

    #[test]
    fn unicode_cwd_tabs_splits_windows_identity_and_geometry_are_grouped() {
        let plan = run(&[
            "--working-directory",
            "/tmp/根",
            "--split",
            "vertical",
            "--tab",
            "--profile",
            "codex:review",
            "--new-window",
            "--geometry",
            "1000x700",
            "--app-id",
            "termeric.review",
            "--workspace",
            "workspace-27",
        ]);
        assert_eq!(plan.windows.len(), 2);
        assert_eq!(plan.windows[0].tabs.len(), 2);
        assert_eq!(
            plan.windows[0].tabs[0].working_directory,
            Some(PathBuf::from("/tmp/根"))
        );
        assert_eq!(
            plan.windows[0].tabs[0].splits,
            [CliSplitDirection::Vertical]
        );
        assert_eq!(plan.windows[1].pixel_size, Some((1000, 700)));
        assert_eq!(plan.windows[1].app_id.as_deref(), Some("termeric.review"));
        assert_eq!(plan.windows[1].workspace.as_deref(), Some("workspace-27"));
    }

    #[test]
    fn conflicting_and_invalid_arguments_fail_with_specific_errors() {
        for arguments in [
            vec!["--workspace", "workspace-1", "--tab"],
            vec!["--profile", "missing-colon"],
            vec!["--terminal-profile", "bad name"],
            vec!["--terminal-profile", "one", "--command", "echo"],
            vec!["--command", "echo", "--profile", "codex:review"],
            vec!["--geometry", "80x24"],
            vec!["--"],
            vec!["positional"],
        ] {
            assert!(parse(arguments.into_iter().map(OsString::from)).is_err());
        }
    }

    #[test]
    fn help_and_version_do_not_create_a_launch_plan() {
        assert!(matches!(
            parse([OsString::from("--help")]).unwrap(),
            ParseOutcome::Print(text) if text.contains("--new-window")
        ));
        assert!(matches!(
            parse([OsString::from("--version")]).unwrap(),
            ParseOutcome::Print(text) if text.starts_with("termeric ")
        ));
    }

    #[test]
    fn quick_access_is_one_explicit_single_window_launch() {
        let ParseOutcome::QuickAccess(plan) = parse(
            ["--quick-access", "--terminal-profile", "dev.rust"]
                .into_iter()
                .map(OsString::from),
        )
        .unwrap() else {
            panic!("expected quick-access launch plan")
        };
        assert_eq!(plan.windows.len(), 1);
        assert!(plan.windows[0].quick_access);
        assert!(plan.windows[0].force_new);
        assert!(matches!(
            plan.windows[0].tabs[0].target,
            LaunchTarget::TerminalProfile { ref name } if name == "dev.rust"
        ));
        assert!(
            parse(
                ["--quick-access", "--new-window"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap_err()
            .to_string()
            .contains("exactly one window")
        );
        assert!(
            parse(
                ["--quick-access", "--quick-access"]
                    .into_iter()
                    .map(OsString::from)
            )
            .is_err()
        );
        assert!(matches!(
            parse(
                ["domain", "uploads", "--json"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::Uploads { json: true })
        ));
        let resume_id = "0123456789abcdef0123456789abcdef";
        assert!(matches!(
            parse(
                ["domain", "cancel-upload", resume_id, "--json"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::CancelUpload {
                upload_id,
                json: true,
            }) if upload_id == resume_id
        ));
        assert!(matches!(
            parse(
                [
                    "domain",
                    "upload",
                    "domain-123",
                    "42",
                    "/tmp/file",
                    "--resume",
                    resume_id,
                ]
                .into_iter()
                .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::Upload {
                retain: false,
                resume: Some(upload_id),
                ..
            }) if upload_id == resume_id
        ));
        assert!(
            parse(
                [
                    "domain",
                    "upload",
                    "domain-123",
                    "42",
                    "/tmp/file",
                    "--retain",
                    "--resume",
                    resume_id,
                ]
                .into_iter()
                .map(OsString::from)
            )
            .is_err()
        );
    }

    #[test]
    fn running_gui_controls_are_display_free_explicit_and_id_targeted() {
        assert!(matches!(
            parse(
                ["control", "list", "--json"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Control(crate::control::ControlCommand::List { json: true })
        ));
        assert!(matches!(
            parse(["control", "--help"].into_iter().map(OsString::from)).unwrap(),
            ParseOutcome::Print(text) if text.contains("Text access is explicit")
        ));
        assert!(matches!(
            parse(
                [
                    "control",
                    "get-text",
                    "42",
                    "7",
                    "3",
                    "1",
                    "--history-lines",
                    "40",
                    "--bytes",
                    "4096",
                    "--json",
                ]
                .into_iter()
                .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Control(crate::control::ControlCommand::GetText {
                process_id: 42,
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
                history_lines: 40,
                max_bytes: 4096,
                json: true,
            })
        ));
        assert!(
            parse(
                ["control", "get-text", "42", "7", "3", "1", "--bytes", "0"]
                    .into_iter()
                    .map(OsString::from)
            )
            .is_err()
        );
        assert!(matches!(
            parse(
                [
                    "control",
                    "send-text",
                    "42",
                    "7",
                    "3",
                    "1",
                    "cargo test",
                    "--paste",
                    "--enter",
                    "--request-id",
                    "55555555555555555555555555555555",
                    "--json",
                ]
                .into_iter()
                .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Control(crate::control::ControlCommand::SendText {
                process_id: 42,
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
                text,
                paste: true,
                enter: true,
                request_id: Some(request_id),
                json: true,
            }) if text == "cargo test" && request_id == "55555555555555555555555555555555"
        ));
        assert!(
            parse(
                ["control", "send-text", "42", "7", "3", "1", "bad\rinput"]
                    .into_iter()
                    .map(OsString::from)
            )
            .is_err()
        );
        assert!(
            parse(["control", "send"].into_iter().map(OsString::from)).is_err(),
            "arbitrary input must not become an implicit control mutation"
        );
        assert!(matches!(
            parse(
                [
                    "control",
                    "new-window",
                    "42",
                    "7",
                    "--request-id",
                    "0123456789abcdef0123456789abcdef",
                    "--json",
                ]
                .into_iter()
                .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Control(crate::control::ControlCommand::NewWindow {
                process_id: 42,
                source_window: 7,
                request_id: Some(request_id),
                json: true,
            }) if request_id == "0123456789abcdef0123456789abcdef"
        ));
        assert!(matches!(
            parse(
                [
                    "control",
                    "new-tab",
                    "42",
                    "7",
                    "--request-id",
                    "fedcba9876543210fedcba9876543210",
                    "--json",
                ]
                .into_iter()
                .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Control(crate::control::ControlCommand::NewTab {
                process_id: 42,
                window_id: 7,
                request_id: Some(request_id),
                json: true,
            }) if request_id == "fedcba9876543210fedcba9876543210"
        ));
        assert!(matches!(
            parse(
                [
                    "control",
                    "split-pane",
                    "42",
                    "7",
                    "3",
                    "0",
                    "vertical",
                    "--request-id",
                    "1234567890abcdef1234567890abcdef",
                    "--json",
                ]
                .into_iter()
                .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Control(crate::control::ControlCommand::SplitPane {
                process_id: 42,
                window_id: 7,
                tab_id: 3,
                pane_id: 0,
                direction: crate::control::protocol::SplitDirection::Vertical,
                request_id: Some(request_id),
                json: true,
            }) if request_id == "1234567890abcdef1234567890abcdef"
        ));
        assert!(matches!(
            parse(
                ["control", "focus-window", "42", "7", "--json"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Control(crate::control::ControlCommand::FocusWindow {
                process_id: 42,
                window_id: 7,
                request_id: None,
                json: true,
            })
        ));
        assert!(matches!(
            parse(
                ["control", "select-tab", "42", "7", "3"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Control(crate::control::ControlCommand::SelectTab {
                process_id: 42,
                window_id: 7,
                tab_id: 3,
                request_id: None,
                json: false,
            })
        ));
        assert!(matches!(
            parse(
                ["control", "focus-pane", "42", "7", "3", "0"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Control(crate::control::ControlCommand::FocusPane {
                process_id: 42,
                window_id: 7,
                tab_id: 3,
                pane_id: 0,
                request_id: None,
                json: false,
            })
        ));
        assert!(matches!(
            parse(
                ["control", "close-window", "42", "7", "--json"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Control(crate::control::ControlCommand::CloseWindow {
                process_id: 42,
                window_id: 7,
                request_id: None,
                json: true,
            })
        ));
        assert!(matches!(
            parse(
                ["control", "close-tab", "42", "7", "3"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Control(crate::control::ControlCommand::CloseTab {
                process_id: 42,
                window_id: 7,
                tab_id: 3,
                request_id: None,
                json: false,
            })
        ));
        assert!(matches!(
            parse(
                ["control", "close-pane", "42", "7", "3", "1"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Control(crate::control::ControlCommand::ClosePane {
                process_id: 42,
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
                request_id: None,
                json: false,
            })
        ));
        assert!(matches!(
            parse(
                ["control", "move-tab", "42", "7", "3", "9", "2", "--json",]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Control(crate::control::ControlCommand::MoveTab {
                process_id: 42,
                source_window_id: 7,
                tab_id: 3,
                destination_window_id: 9,
                destination_index: 2,
                request_id: None,
                json: true,
            })
        ));
        assert!(matches!(
            parse(
                [
                    "control",
                    "move-pane",
                    "42",
                    "7",
                    "3",
                    "1",
                    "9",
                    "4",
                    "2",
                    "horizontal",
                    "--json",
                ]
                .into_iter()
                .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Control(crate::control::ControlCommand::MovePane {
                process_id: 42,
                source_window_id: 7,
                source_tab_id: 3,
                pane_id: 1,
                destination_window_id: 9,
                destination_tab_id: 4,
                target_pane_id: 2,
                direction: crate::control::protocol::SplitDirection::Horizontal,
                request_id: None,
                json: true,
            })
        ));
        assert!(matches!(
            parse(
                [
                    "control",
                    "resize-window",
                    "42",
                    "7",
                    "900",
                    "600",
                    "--json"
                ]
                .into_iter()
                .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Control(crate::control::ControlCommand::ResizeWindow {
                process_id: 42,
                window_id: 7,
                width: 900,
                height: 600,
                request_id: None,
                json: true,
            })
        ));
        assert!(matches!(
            parse(
                ["control", "resize-divider", "42", "7", "3", "0", "650"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Control(crate::control::ControlCommand::ResizeDivider {
                process_id: 42,
                window_id: 7,
                tab_id: 3,
                divider_id: 0,
                ratio_millis: 650,
                request_id: None,
                json: false,
            })
        ));
        for arguments in [
            vec!["control", "new-window", "0", "7"],
            vec!["control", "new-window", "42", "0"],
            vec!["control", "new-window", "42", "7", "--request-id", "short"],
            vec!["control", "new-window", "42", "7", "--json", "--json"],
            vec!["control", "new-tab", "42", "0"],
            vec!["control", "new-tab", "42", "7", "--request-id", "short"],
            vec!["control", "new-tab", "42", "7", "--json", "--json"],
            vec!["control", "split-pane", "42", "7", "3", "0", "diagonal"],
            vec!["control", "split-pane", "42", "7", "3", "-1", "vertical"],
            vec!["control", "focus-window", "42", "0"],
            vec!["control", "select-tab", "42", "7"],
            vec!["control", "focus-pane", "42", "7", "3", "-1"],
            vec!["control", "close-window", "42", "0"],
            vec!["control", "close-tab", "42", "7"],
            vec!["control", "close-pane", "42", "7", "3", "-1"],
            vec!["control", "move-tab", "42", "7", "3", "0", "2"],
            vec!["control", "move-tab", "42", "7", "3", "9", "257"],
            vec!["control", "resize-window", "42", "7", "159", "600"],
            vec!["control", "resize-window", "42", "7", "900", "16385"],
            vec!["control", "resize-divider", "42", "7", "3", "1", "99"],
            vec!["control", "resize-divider", "42", "7", "3", "1", "901"],
            vec![
                "control",
                "move-pane",
                "42",
                "7",
                "3",
                "1",
                "9",
                "4",
                "2",
                "diagonal",
            ],
            vec![
                "control",
                "focus-pane",
                "42",
                "7",
                "3",
                "0",
                "--json",
                "--json",
            ],
            vec![
                "control",
                "split-pane",
                "42",
                "7",
                "3",
                "0",
                "vertical",
                "--json",
                "--json",
            ],
        ] {
            assert!(parse(arguments.into_iter().map(OsString::from)).is_err());
        }
    }

    #[test]
    fn leading_new_window_flag_creates_one_window_and_later_delimiters_add_more() {
        assert_eq!(run(&["--new-window"]).windows.len(), 1);
        assert_eq!(run(&["--new-window", "--new-window"]).windows.len(), 2);
    }

    #[test]
    fn renderer_override_is_validated_and_applies_to_every_window() {
        let plan = run(&["--new-window", "--renderer", "software"]);
        assert_eq!(plan.windows.len(), 1);
        assert_eq!(
            plan.windows[0].renderer_backend,
            Some(crate::config::RendererBackend::Software)
        );

        let plan = run(&["--renderer", "gpu", "--new-window", "--new-window"]);
        assert!(plan.windows.iter().all(|window| {
            window.renderer_backend == Some(crate::config::RendererBackend::Gpu)
        }));
        assert!(parse([OsString::from("--renderer"), OsString::from("cpu")]).is_err());
        assert!(
            parse(
                ["--renderer", "auto", "--renderer", "software"]
                    .into_iter()
                    .map(OsString::from)
            )
            .is_err()
        );
    }

    #[test]
    fn plugin_admin_commands_are_display_free_and_permission_explicit() {
        let ParseOutcome::Plugin(crate::plugin::runtime::PluginAdminCommand::Install {
            directory,
            grants,
            enable,
            replace,
        }) = parse(
            [
                "plugin",
                "install",
                "/tmp/sample",
                "--allow",
                "process-spawn",
                "--allow",
                "workspace-query",
                "--allow",
                "workspace-actions",
                "--allow",
                "workspace-topology",
                "--allow",
                "terminal-text",
                "--enable",
                "--replace",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .unwrap()
        else {
            panic!("expected plugin install command");
        };
        assert_eq!(directory, PathBuf::from("/tmp/sample"));
        assert_eq!(
            grants,
            std::collections::BTreeSet::from([
                crate::plugin::runtime::PluginPermission::ProcessSpawn,
                crate::plugin::runtime::PluginPermission::WorkspaceQuery,
                crate::plugin::runtime::PluginPermission::WorkspaceActions,
                crate::plugin::runtime::PluginPermission::WorkspaceTopology,
                crate::plugin::runtime::PluginPermission::TerminalText,
            ])
        );
        assert!(enable && replace);

        assert!(matches!(
            parse(["plugin", "list", "--json"].into_iter().map(OsString::from)).unwrap(),
            ParseOutcome::Plugin(crate::plugin::runtime::PluginAdminCommand::List { json: true })
        ));
        assert!(matches!(
            parse(
                ["plugin", "configure", "sample", "--set", "line-count", "42"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Plugin(crate::plugin::runtime::PluginAdminCommand::Configure {
                id,
                change: crate::plugin::runtime::PluginConfigurationChange::Set { key, value },
            }) if id == "sample" && key == "line-count" && value == "42"
        ));
        assert!(matches!(
            parse(
                ["plugin", "configure", "sample", "--reset-settings"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Plugin(crate::plugin::runtime::PluginAdminCommand::Configure {
                change: crate::plugin::runtime::PluginConfigurationChange::ResetSettings,
                ..
            })
        ));
        assert!(
            parse(
                ["plugin", "install", "/tmp/sample", "--allow", "camera"]
                    .into_iter()
                    .map(OsString::from)
            )
            .is_err()
        );
        assert!(
            parse(
                ["plugin", "configure", "sample"]
                    .into_iter()
                    .map(OsString::from)
            )
            .is_err()
        );
    }

    #[test]
    fn domain_commands_preserve_unicode_argv_and_parse_scriptable_controls() {
        let outcome = parse(
            [
                "domain",
                "create",
                "daily",
                "--cwd",
                "/tmp",
                "--size",
                "90x30",
                "--json",
                "--",
                "printf",
                "two words",
                "λ",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .unwrap();
        let ParseOutcome::Domain(crate::domain::DomainCommand::Create { name, spawn, json }) =
            outcome
        else {
            panic!("expected domain create command");
        };
        assert_eq!(name, "daily");
        assert_eq!(spawn.program.as_deref(), Some("printf"));
        assert_eq!(spawn.args, ["two words", "λ"]);
        assert_eq!((spawn.cols, spawn.rows), (90, 30));
        assert!(json);

        let ParseOutcome::Run(plan) = parse(
            [
                "domain",
                "attach",
                "domain-123",
                "--geometry",
                "900x600",
                "--app-id",
                "termeric.domain",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .unwrap() else {
            panic!("expected domain GUI launch");
        };
        assert_eq!(
            plan.windows[0].domain,
            Some(crate::domain::DomainTarget {
                authority: crate::domain::DomainAuthority::Local,
                domain_id: "domain-123".into(),
            })
        );
        assert_eq!(plan.windows[0].pixel_size, Some((900, 600)));
        assert_eq!(plan.windows[0].app_id.as_deref(), Some("termeric.domain"));

        let ParseOutcome::Run(remote_plan) = parse(
            [
                "domain",
                "--host",
                "dev@example.test",
                "attach",
                "domain-123",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .unwrap() else {
            panic!("expected remote domain GUI launch");
        };
        assert_eq!(
            remote_plan.windows[0].domain,
            Some(crate::domain::DomainTarget {
                authority: crate::domain::DomainAuthority::Ssh {
                    destination: "dev@example.test".into(),
                },
                domain_id: "domain-123".into(),
            })
        );

        assert!(matches!(
            parse(
                ["domain", "close-pane", "domain-123", "42"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::ClosePane {
                domain,
                pane: 42,
            }) if domain == "domain-123"
        ));
        assert!(matches!(
            parse(
                ["domain", "--host", "dev@example.test", "list", "--json"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::Remote {
                destination,
                command,
            }) if destination == "dev@example.test"
                && matches!(*command, crate::domain::DomainCommand::List { json: true })
        ));
        assert!(matches!(
            parse(["domain", "proxy-stdio"].into_iter().map(OsString::from)).unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::ProxyStdio)
        ));
        assert!(matches!(
            parse(
                ["domain", "protocol-version", "--json"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::ProtocolVersion { json: true })
        ));
        assert!(matches!(
            parse(
                ["domain", "close-tab", "domain-123", "7"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::CloseTab {
                domain,
                tab: 7,
            }) if domain == "domain-123"
        ));
        assert!(matches!(
            parse(
                ["domain", "resize-split", "domain-123", "9", "375"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::ResizeSplit {
                domain,
                split: 9,
                ratio_millis: 375,
            }) if domain == "domain-123"
        ));
        assert!(matches!(
            parse(
                ["domain", "rename-tab", "domain-123", "7", "Build λ"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::RenameTab {
                domain,
                tab: 7,
                title: Some(title),
            }) if domain == "domain-123" && title == "Build λ"
        ));
        assert!(matches!(
            parse(
                ["domain", "rename-tab", "domain-123", "7", ""]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::RenameTab {
                tab: 7,
                title: None,
                ..
            })
        ));
        assert!(matches!(
            parse(
                ["domain", "pin-tab", "domain-123", "7"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::SetTabPinned {
                tab: 7,
                pinned: true,
                ..
            })
        ));
        assert!(matches!(
            parse(
                ["domain", "move-tab", "domain-123", "7", "2"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::MoveTab {
                tab: 7,
                index: 2,
                ..
            })
        ));
        assert!(matches!(
            parse(
                ["domain", "swap-pane", "domain-123", "42"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::SwapPaneNext { pane: 42, .. })
        ));
        assert!(matches!(
            parse(
                ["domain", "balance-tab", "domain-123", "7"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::BalanceTab { tab: 7, .. })
        ));
        assert!(matches!(
            parse(
                ["domain", "layout-tab", "domain-123", "7", "grid"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::ApplyPaneLayout {
                tab: 7,
                preset: crate::domain::model::LivePaneLayoutPreset::Grid,
                ..
            })
        ));
        assert!(matches!(
            parse(
                ["domain", "move-pane", "domain-123", "42", "7"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::MovePane {
                pane: 42,
                tab: 7,
                ..
            })
        ));
        assert!(matches!(
            parse(
                [
                    "domain",
                    "--host",
                    "dev@example.test",
                    "upload",
                    "domain-123",
                    "42",
                    "/tmp/local artifact.bin",
                    "--name",
                    "remote λ.bin",
                    "--json",
                ]
                .into_iter()
                .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::Remote {
                destination,
                command,
            }) if destination == "dev@example.test"
                && matches!(
                    command.as_ref(),
                    crate::domain::DomainCommand::Upload {
                        domain,
                        pane: 42,
                        source,
                        name: Some(name),
                        json: true,
                        ..
                    } if domain == "domain-123"
                        && source == std::path::Path::new("/tmp/local artifact.bin")
                        && name == "remote λ.bin"
                )
        ));
        assert!(
            parse(
                [
                    "domain",
                    "upload",
                    "domain-123",
                    "42",
                    "/tmp/file",
                    "--name",
                    "../escape",
                ]
                .into_iter()
                .map(OsString::from)
            )
            .is_err()
        );
    }

    #[test]
    fn saved_ssh_profiles_resolve_for_controls_and_gui_without_re_resolving_relays() {
        let config: crate::config::Config = toml::from_str(
            r#"
                [domains.ssh_hosts.build]
                destination = "developer@build.example"
                description = "Build farm"
            "#,
        )
        .unwrap();
        config.validate().unwrap();

        let arguments = ["domain", "--host-profile", "build", "list", "--json"]
            .into_iter()
            .map(OsString::from)
            .collect();
        assert!(matches!(
            parse_collected(arguments, &config.domains).unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::Remote {
                destination,
                command,
            }) if destination == "developer@build.example"
                && matches!(*command, crate::domain::DomainCommand::List { json: true })
        ));

        let arguments = ["domain", "--host-profile", "build", "attach", "domain-123"]
            .into_iter()
            .map(OsString::from)
            .collect();
        let ParseOutcome::Run(plan) = parse_collected(arguments, &config.domains).unwrap() else {
            panic!("expected saved-host GUI launch");
        };
        let target = plan.windows[0].domain.as_ref().unwrap();
        assert_eq!(
            target.authority,
            crate::domain::DomainAuthority::Ssh {
                destination: "developer@build.example".into(),
            }
        );
        assert_eq!(
            target
                .authority
                .relay_command("/opt/termeric".into())
                .pane_arguments("domain-123", 7),
            [
                "domain",
                "--host",
                "developer@build.example",
                "relay",
                "domain-123",
                "7",
            ]
        );

        assert!(matches!(
            parse(
                ["domain", "hosts", "--json"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::Hosts { json: true })
        ));
        assert!(
            parse(
                ["domain", "--host-profile", "build", "list"]
                    .into_iter()
                    .map(OsString::from)
            )
            .is_err(),
            "ordinary deterministic parsing has no implicit profile source"
        );
        let nested = ["domain", "--host-profile", "build", "hosts"]
            .into_iter()
            .map(OsString::from)
            .collect();
        assert!(parse_collected(nested, &config.domains).is_err());
    }

    #[test]
    fn invalid_domain_commands_fail_before_connecting() {
        for arguments in [
            vec!["domain", "attach", "bad/id"],
            vec!["domain", "split", "domain-1", "x", "vertical"],
            vec!["domain", "split", "domain-1", "0", "diagonal"],
            vec!["domain", "get-text", "domain-1", "0", "--bytes", "0"],
            vec!["domain", "close-pane", "domain-1", "not-a-pane"],
            vec!["domain", "close-tab", "domain-1", "not-a-tab"],
            vec!["domain", "resize-split", "domain-1", "x", "500"],
            vec!["domain", "resize-split", "domain-1", "0", "99"],
            vec!["domain", "resize-split", "domain-1", "0", "901"],
            vec!["domain", "rename-tab", "domain-1", "0", "two\nlines"],
            vec!["domain", "pin-tab", "domain-1", "not-a-tab"],
            vec!["domain", "move-tab", "domain-1", "0", "not-an-index"],
            vec!["domain", "swap-pane", "domain-1", "not-a-pane"],
            vec!["domain", "balance-tab", "domain-1", "not-a-tab"],
            vec!["domain", "layout-tab", "domain-1", "0", "diagonal"],
            vec!["domain", "move-pane", "domain-1", "x", "0"],
            vec!["domain", "move-pane", "domain-1", "0", "x"],
            vec!["domain", "--host", "-unsafe", "list"],
            vec!["domain", "--host", "two words", "list"],
            vec!["domain", "--host", "host", "server", "--foreground"],
            vec!["domain", "--host", "host", "protocol-version"],
            vec!["domain", "--host", "host", "--host", "other", "list"],
            vec!["domain", "create", "name", "--size", "0x24"],
            vec!["domain", "server"],
        ] {
            assert!(parse(arguments.into_iter().map(OsString::from)).is_err());
        }
    }
}
