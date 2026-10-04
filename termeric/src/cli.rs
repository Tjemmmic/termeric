//! Deterministic command-line parsing into window-local startup plans.

mod control_args;
mod domain_args;
mod plugin_args;

use crate::panes::SplitDirection;
use anyhow::Context;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub windows: Vec<WindowLaunch>,
    /// Start a process of its own even when a running Termeric could open
    /// the window.
    pub separate_process: bool,
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
    /// Open with no tabs of its own, for a tab torn off another window of
    /// this process, which moves in as soon as the window exists.
    pub adopts_tab: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabLaunch {
    pub working_directory: Option<PathBuf>,
    pub title: Option<String>,
    pub target: LaunchTarget,
    /// Per-launch child environment overlay. Runtime control requests are
    /// validated before populating this map; ordinary CLI plans leave it empty.
    pub environment: HashMap<String, String>,
    pub environment_policy: termeric_core::pty::EnvironmentPolicy,
    pub splits: Vec<SplitDirection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchTarget {
    Shell,
    Command { program: String, args: Vec<String> },
    TerminalProfile { name: String },
    Plugin { plugin_id: String, profile: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseOutcome {
    Run(LaunchPlan),
    QuickAccess(LaunchPlan),
    Control(crate::control::ControlCommand),
    Domain(crate::domain::DomainCommand),
    Plugin(crate::plugin::runtime::PluginAdminCommand),
    SerialBridge(crate::serial::BridgeOptions),
    /// Serve MCP on standard I/O for the named (or discovered) process, as
    /// a supervisor's server when it carries that supervisor's token.
    Mcp {
        process_id: Option<u32>,
        supervisor: Option<String>,
    },
    Print(String),
}

impl Default for LaunchPlan {
    fn default() -> Self {
        Self {
            windows: vec![WindowLaunch::default()],
            separate_process: false,
        }
    }
}

impl LaunchPlan {
    /// The tabs to ask a running Termeric to open, when this launch is one
    /// it can open: a single window of shell or command tabs, each with at
    /// most a folder and a title. Anything a running process cannot honor
    /// for one window (a workspace, domain, profile, identity, size,
    /// renderer, environment, or splits) keeps a process of its own. A tab
    /// without a folder opens in `current_dir`, where this launch started.
    pub fn join_tabs(
        &self,
        current_dir: Option<&std::path::Path>,
    ) -> Option<Vec<crate::control::protocol::JoinTab>> {
        let [window] = self.windows.as_slice() else {
            return None;
        };
        let standalone = self.separate_process
            || window.workspace.is_some()
            || window.domain.is_some()
            || window.domain_snapshot.is_some()
            || window.app_id.is_some()
            || window.pixel_size.is_some()
            || window.renderer_backend.is_some()
            || window.quick_access
            || window.adopts_tab;
        if standalone {
            return None;
        }
        window
            .tabs
            .iter()
            .map(|tab| {
                if !tab.environment.is_empty()
                    || tab.environment_policy != Default::default()
                    || !tab.splits.is_empty()
                {
                    return None;
                }
                let (program, args) = match &tab.target {
                    LaunchTarget::Shell => (None, Vec::new()),
                    LaunchTarget::Command { program, args } => {
                        (Some(program.clone()), args.clone())
                    }
                    LaunchTarget::TerminalProfile { .. } | LaunchTarget::Plugin { .. } => {
                        return None;
                    }
                };
                // Relative folders are the launcher's, not the running
                // process's.
                let working_directory = match (tab.working_directory.as_deref(), current_dir) {
                    (Some(directory), Some(base)) if directory.is_relative() => {
                        Some(base.join(directory))
                    }
                    (Some(directory), _) => Some(directory.to_path_buf()),
                    (None, base) => base.map(std::path::Path::to_path_buf),
                }
                .and_then(|directory| directory.to_str().map(str::to_owned));
                Some(crate::control::protocol::JoinTab {
                    working_directory,
                    title: tab.title.clone(),
                    program,
                    args,
                })
            })
            .collect()
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
            adopts_tab: false,
        }
    }
}

impl Default for TabLaunch {
    fn default() -> Self {
        Self {
            working_directory: None,
            title: None,
            target: LaunchTarget::Shell,
            environment: HashMap::new(),
            environment_policy: Default::default(),
            splits: Vec::new(),
        }
    }
}

/// What one `termeric` run asks for.
pub enum Invocation {
    /// An agent hook. `None` when its arguments are not understood, which
    /// must stay silent: a failing hook would surface inside the agent.
    AgentEvent(Option<crate::agents::hook::HookInvocation>),
    Command(Box<ParseOutcome>),
}

pub fn parse_env() -> anyhow::Result<Invocation> {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    if arguments.first().and_then(|value| value.to_str()) == Some("agent-event") {
        return Ok(Invocation::AgentEvent(parse_agent_event(&arguments[1..])));
    }
    Ok(Invocation::Command(Box::new(parse_command(arguments)?)))
}

fn parse_command(arguments: Vec<OsString>) -> anyhow::Result<ParseOutcome> {
    parse_collected(arguments, &|| {
        Ok(crate::config::Config::load_or_default()?.domains)
    })
}

#[cfg(test)]
pub fn parse(arguments: impl IntoIterator<Item = OsString>) -> anyhow::Result<ParseOutcome> {
    let arguments: Vec<OsString> = arguments.into_iter().collect();
    parse_collected(arguments, &|| Ok(crate::config::DomainsConfig::default()))
}

/// Where `domain --host-profile` finds its profiles. Only that option reads
/// the configuration, so every other command works with a broken one.
type LoadDomains<'a> = &'a dyn Fn() -> anyhow::Result<crate::config::DomainsConfig>;

/// `mcp [--process PID] [--supervisor TOKEN]`. The token is passed only by
/// the supervisor Termeric starts.
fn parse_mcp(arguments: &[OsString]) -> anyhow::Result<ParseOutcome> {
    let mut process_id = None;
    let mut supervisor = None;
    let mut rest = arguments.iter();
    while let Some(flag) = rest.next() {
        let value = rest.next().and_then(|value| value.to_str());
        match (flag.to_str(), value) {
            (Some("--process"), Some(value)) if process_id.is_none() => {
                process_id = Some(
                    value
                        .parse::<u32>()
                        .ok()
                        .filter(|pid| *pid != 0 && *pid <= i32::MAX as u32)
                        .context("--process expects a Termeric process ID")?,
                );
            }
            (Some("--supervisor"), Some(value)) if supervisor.is_none() => {
                anyhow::ensure!(
                    crate::control::protocol::valid_request_id(value),
                    "--supervisor expects the token Termeric passed"
                );
                supervisor = Some(value.to_owned());
            }
            _ => anyhow::bail!("usage: termeric mcp [--process PID]"),
        }
    }
    Ok(ParseOutcome::Mcp {
        process_id,
        supervisor,
    })
}

/// `agent-event AGENT EVENT`, as written by Termeric's hook setup.
fn parse_agent_event(arguments: &[OsString]) -> Option<crate::agents::hook::HookInvocation> {
    let [agent_id, event] = arguments else {
        return None;
    };
    let agent_id = agent_id.to_str()?;
    let valid_id = !agent_id.is_empty()
        && agent_id.len() <= 64
        && agent_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    Some(crate::agents::hook::HookInvocation {
        agent_id: valid_id.then(|| agent_id.to_owned())?,
        event: crate::agents::hook::HookEvent::parse(event.to_str()?)?,
    })
}

fn parse_collected(
    arguments: Vec<OsString>,
    load_domains: LoadDomains,
) -> anyhow::Result<ParseOutcome> {
    if crate::serial::is_internal_command(arguments.first()) {
        return Ok(ParseOutcome::SerialBridge(
            crate::serial::BridgeOptions::parse(&arguments[1..])?,
        ));
    }
    if arguments.first().and_then(|argument| argument.to_str()) == Some("domain") {
        return parse_domain(&arguments[1..], load_domains);
    }
    if arguments.first().and_then(|argument| argument.to_str()) == Some("control") {
        return parse_control(&arguments[1..]);
    }
    if arguments.first().and_then(|argument| argument.to_str()) == Some("plugin") {
        return parse_plugin(&arguments[1..]);
    }
    if arguments.first().and_then(|argument| argument.to_str()) == Some("mcp") {
        return parse_mcp(&arguments[1..]);
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
            "--separate-process" => plan.separate_process = true,
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
                    "horizontal" | "h" => SplitDirection::Horizontal,
                    "vertical" | "v" => SplitDirection::Vertical,
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
                    anyhow::anyhow!("--profile expects AGENT:PROFILE (for example codex:review)")
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
    // The value shows in chrome, so it follows the display rule terminal
    // titles do: no control, line-break, or bidirectional formatting text.
    if value.is_empty()
        || value.chars().count() > 256
        || value.chars().any(crate::safe_text::is_unsafe_display_char)
    {
        anyhow::bail!(
            "{label} must be 1 to 256 characters on one line, without control or bidirectional formatting characters"
        );
    }
    Ok(())
}

fn help_text() -> String {
    format!(
        "Termeric {}\n\n\
Usage: termeric [WINDOW/TAB OPTIONS] [-- COMMAND ARG...]\n\n\
Window grouping:\n  \
  --new-window                 Start another independent native window\n  \
  --separate-process           Start a process of its own, even when Termeric runs\n  \
  --quick-access               Toggle or launch one top-edge drop-down window\n  \
  --tab                        Start another tab in the current window\n  \
  --split horizontal|vertical  Split the current shell tab after launch\n\n\
Launch selection:\n  \
  --working-directory PATH     Set the current tab's cwd\n  \
  --command PROGRAM            Run PROGRAM directly without arguments\n  \
  --                           Preserve all remaining command arguments exactly\n  \
  --terminal-profile NAME      Launch a configured reusable terminal profile\n  \
  --profile AGENT:PROFILE      Launch a coding agent with a named profile\n  \
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
  mcp [--process PID]          Serve MCP tools for Termeric's agents on stdio\n  \
  -h, --help                   Print this help\n  \
  -V, --version                Print the version",
        env!("CARGO_PKG_VERSION")
    )
}

/// A split's orientation on the command line; `h` and `v` work too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum Direction {
    #[value(alias = "h")]
    Horizontal,
    #[value(alias = "v")]
    Vertical,
}

impl From<Direction> for SplitDirection {
    fn from(direction: Direction) -> Self {
        match direction {
            Direction::Horizontal => SplitDirection::Horizontal,
            Direction::Vertical => SplitDirection::Vertical,
        }
    }
}

/// Parse a subcommand's `arguments` with clap: its arguments, the help it
/// asked for (to print), or an error naming the mistake.
fn parse_subcommand<C: clap::Parser>(arguments: &[OsString]) -> anyhow::Result<Result<C, String>> {
    let name = <C as clap::CommandFactory>::command().get_name().to_owned();
    match C::try_parse_from(std::iter::once(OsString::from(name)).chain(arguments.iter().cloned()))
    {
        Ok(parsed) => Ok(Ok(parsed)),
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp
                    | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            ) =>
        {
            Ok(Err(error.render().to_string().trim_end().to_owned()))
        }
        Err(error) => {
            let message = error.render().to_string();
            let message = message.strip_prefix("error: ").unwrap_or(&message);
            Err(anyhow::anyhow!("{}", message.trim_end()))
        }
    }
}

fn parse_control(arguments: &[OsString]) -> anyhow::Result<ParseOutcome> {
    Ok(
        match parse_subcommand::<control_args::ControlCli>(arguments)? {
            Ok(control) => ParseOutcome::Control(control.into_command()?),
            Err(help) => ParseOutcome::Print(help),
        },
    )
}

fn parse_plugin(arguments: &[OsString]) -> anyhow::Result<ParseOutcome> {
    Ok(
        match parse_subcommand::<plugin_args::PluginCli>(arguments)? {
            Ok(plugin) => ParseOutcome::Plugin(plugin.into_command()?),
            Err(help) => ParseOutcome::Print(help),
        },
    )
}

/// `domain` commands; `--host-profile` names resolve through `load_domains`.
fn parse_domain(arguments: &[OsString], load_domains: LoadDomains) -> anyhow::Result<ParseOutcome> {
    match parse_subcommand::<domain_args::DomainCli>(arguments)? {
        Ok(domain) => domain.into_outcome(load_domains),
        Err(help) => Ok(ParseOutcome::Print(help)),
    }
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

#[cfg(test)]
mod tests {
    #[test]
    fn exact_launch_environment_flags_are_explicit_and_order_independent() {
        for verb in ["launch-tab", "launch-window"] {
            for flags in [
                vec!["--clear-env", "--env", "EMPTY="],
                vec!["--unset-env", "PATH", "--env", "EMPTY="],
            ] {
                let mut args = vec!["control", verb, "42", "7"];
                args.extend(flags);
                args.extend(["--", "/bin/true"]);
                let outcome = parse(args.into_iter().map(OsString::from)).unwrap();
                let ParseOutcome::Control(crate::control::ControlCommand::Launch {
                    request:
                        crate::control::protocol::Request::LaunchTab {
                            environment,
                            environment_policy: policy,
                            ..
                        }
                        | crate::control::protocol::Request::LaunchWindow {
                            environment,
                            environment_policy: policy,
                            ..
                        },
                    ..
                }) = outcome
                else {
                    panic!("wrong launch command");
                };
                assert_eq!(environment[0].value, "");
                assert!(policy.clear || policy.unset == ["PATH"]);
            }
            for flags in [
                vec!["--clear-env", "--clear-env"],
                vec!["--unset-env"],
                vec!["--unset-env", "A=B"],
                vec!["--unset-env", "TERM"],
                vec!["--unset-env", "SHELL"],
                vec!["--unset-env", "PATH", "--unset-env", "PATH"],
                vec!["--clear-env", "--unset-env", "PATH"],
                vec!["--unset-env", "PATH", "--clear-env"],
                vec!["--env", "PATH=x", "--unset-env", "PATH"],
                vec!["--unset-env", "PATH", "--env", "PATH=x"],
            ] {
                let mut args = vec!["control", verb, "42", "7"];
                args.extend(flags);
                args.extend(["--", "/bin/true"]);
                assert!(parse(args.into_iter().map(OsString::from)).is_err());
            }
        }
    }
    #[test]
    fn background_command_tab_launch_flag_is_available() {
        for wait in [false, true] {
            let mut args = vec!["control", "launch-tab", "42", "7", "--background"];
            if wait {
                args.push("--wait");
            }
            args.extend(["--", "/bin/true", "--background"]);
            let ParseOutcome::Control(crate::control::ControlCommand::Launch {
                request:
                    crate::control::protocol::Request::LaunchTab {
                        background: true,
                        args,
                        ..
                    },
                wait: launch_wait,
                ..
            }) = parse(args.into_iter().map(OsString::from)).unwrap()
            else {
                panic!("background flag lost");
            };
            assert_eq!(args, ["--background"]);
            assert_eq!(launch_wait.is_some(), wait);
        }
        for args in [
            vec![
                "control",
                "launch-tab",
                "42",
                "7",
                "--background",
                "--background",
                "--",
                "/bin/true",
            ],
            vec![
                "control",
                "launch-window",
                "42",
                "7",
                "--background",
                "--",
                "/bin/true",
            ],
            vec!["control", "new-tab", "42", "7", "--background"],
        ] {
            assert!(parse(args.into_iter().map(OsString::from)).is_err());
        }
    }

    #[test]
    fn stdin_environment_launch_flag_is_available() {
        for target in ["launch-tab", "launch-window"] {
            for wait in [false, true] {
                let mut args = vec!["control", target, "42", "7", "--env-stdin"];
                if wait {
                    args.push("--wait");
                }
                args.extend(["--", "/bin/true", "--env-stdin"]);
                assert!(parse(args.into_iter().map(OsString::from)).is_ok());
            }
        }
    }

    #[test]
    fn combined_launch_wait_options_and_child_arguments() {
        for target in ["launch-tab", "launch-window"] {
            let result = parse(
                [
                    "control",
                    target,
                    "42",
                    "7",
                    "--wait",
                    "--timeout",
                    "1",
                    "--json",
                    "--",
                    "/bin/true",
                    "--wait",
                    "--timeout",
                    "0",
                ]
                .into_iter()
                .map(OsString::from),
            );
            let ParseOutcome::Control(crate::control::ControlCommand::Launch {
                process_id: 42,
                request,
                request_id: None,
                env_stdin: false,
                wait:
                    Some(crate::control::Wait {
                        timeout_seconds: Some(1),
                    }),
                json: true,
            }) = result.unwrap()
            else {
                panic!("expected combined wait")
            };
            let args = match request {
                crate::control::protocol::Request::LaunchTab { args, .. }
                | crate::control::protocol::Request::LaunchWindow { args, .. } => args,
                _ => panic!("expected launch request"),
            };
            assert_eq!(args, ["--wait", "--timeout", "0"]);
            for flags in [
                vec!["--wait", "--wait"],
                vec!["--timeout", "1"],
                vec!["--wait", "--timeout", "0"],
                vec!["--wait", "--timeout", "86401"],
                vec!["--wait", "--timeout", "-1"],
                vec!["--wait", "--timeout", "1.5"],
                vec!["--wait", "--timeout"],
                vec!["--wait", "--timeout", "1", "--timeout", "2"],
            ] {
                let mut arguments = vec!["control", target, "42", "7"];
                arguments.extend(flags);
                arguments.extend(["--", "/bin/true"]);
                assert!(parse(arguments.into_iter().map(OsString::from)).is_err());
            }
            for flags in [vec!["--wait"], vec!["--timeout", "86400", "--wait"]] {
                let mut arguments = vec!["control", target, "42", "7"];
                arguments.extend(flags);
                arguments.extend(["--", "/bin/true"]);
                assert!(parse(arguments.into_iter().map(OsString::from)).is_ok());
            }
        }
    }

    #[test]
    fn wait_launch_accepts_only_bounded_unique_options() {
        let id = "a".repeat(32);
        for args in [
            vec!["control", "wait-launch", "42", &id],
            vec![
                "control",
                "wait-launch",
                "42",
                &id,
                "--timeout",
                "86400",
                "--json",
            ],
            vec![
                "control",
                "wait-launch",
                "42",
                &id,
                "--json",
                "--timeout",
                "1",
            ],
        ] {
            assert!(matches!(
                parse(args.into_iter().map(OsString::from)).unwrap(),
                ParseOutcome::Control(crate::control::ControlCommand::WaitLaunch {
                    process_id: 42,
                    ..
                })
            ));
        }
        for args in [
            vec!["control", "wait-launch", "42"],
            vec!["control", "wait-launch", "0", &id],
            vec!["control", "wait-launch", "42", "invalid"],
            vec!["control", "wait-launch", "42", &id, "--timeout"],
            vec!["control", "wait-launch", "42", &id, "--timeout", "0"],
            vec!["control", "wait-launch", "42", &id, "--timeout", "86401"],
            vec!["control", "wait-launch", "42", &id, "--timeout", "-1"],
            vec!["control", "wait-launch", "42", &id, "--timeout", "1.5"],
            vec![
                "control",
                "wait-launch",
                "42",
                &id,
                "--timeout",
                "999999999999999",
            ],
            vec![
                "control",
                "wait-launch",
                "42",
                &id,
                "--timeout",
                "1",
                "--timeout",
                "2",
            ],
            vec!["control", "wait-launch", "42", &id, "--json", "--json"],
            vec!["control", "wait-launch", "42", &id, "--enter"],
        ] {
            assert!(parse(args.into_iter().map(OsString::from)).is_err());
        }
    }
    #[test]
    fn launch_status_requires_exact_launch_identity_and_only_optional_json() {
        let id = "a".repeat(32);
        assert!(
            matches!(parse(["control", "launch-status", "42", &id, "--json"].into_iter().map(OsString::from)).unwrap(),
            ParseOutcome::Control(crate::control::ControlCommand::LaunchStatus { process_id: 42, launch_request_id, json: true }) if launch_request_id == id)
        );
        for args in [
            vec!["control", "launch-status", "42"],
            vec!["control", "launch-status", "42", "invalid"],
            vec!["control", "launch-status", "0", &id],
            vec!["control", "launch-status", "42", &id, "--json", "--json"],
            vec!["control", "launch-status", "42", &id, "--wait"],
        ] {
            assert!(parse(args.into_iter().map(OsString::from)).is_err());
        }
    }
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
        assert_eq!(plan.windows[0].tabs[0].splits, [SplitDirection::Vertical]);
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
            vec!["--title", "two\nlines"],
            vec!["--title", "tab\tstop"],
            vec!["--title", "\u{202e}reversed"],
        ] {
            assert!(parse(arguments.into_iter().map(OsString::from)).is_err());
        }
        assert!(parse(["--title", "Ünïcode ✓"].into_iter().map(OsString::from)).is_ok());
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

        use crate::control::ControlCommand;
        use crate::control::protocol::{EnvironmentVariable, Request};
        let control = |arguments: &[&str]| {
            let mut all = vec!["control"];
            all.extend(arguments);
            match parse(all.into_iter().map(OsString::from)).unwrap() {
                ParseOutcome::Control(command) => command,
                _ => panic!("expected a control command"),
            }
        };
        let send = |request, request_id: Option<&str>, json| ControlCommand::Send {
            process_id: 42,
            request,
            request_id: request_id.map(str::to_owned),
            json,
        };
        assert_eq!(
            control(&[
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
            ]),
            send(
                Request::ReadText {
                    window_id: 7,
                    tab_id: 3,
                    pane_id: 1,
                    history_lines: 40,
                    max_bytes: 4096,
                },
                None,
                true
            )
        );
        let id = "55555555555555555555555555555555";
        assert_eq!(
            control(&[
                "send-text",
                "42",
                "7",
                "3",
                "1",
                "cargo test",
                "--paste",
                "--enter",
                "--request-id",
                id,
                "--json",
            ]),
            send(
                Request::SendText {
                    request_id: String::new(),
                    window_id: 7,
                    tab_id: 3,
                    pane_id: 1,
                    text: "cargo test".into(),
                    paste: true,
                    enter: true,
                },
                Some(id),
                true
            )
        );
        // Typed text may itself look like an option.
        assert!(matches!(
            control(&["send-text", "42", "7", "3", "1", "-n"]),
            ControlCommand::Send {
                request: Request::SendText { text, .. },
                ..
            } if text == "-n"
        ));
        assert_eq!(
            control(&["new-window", "42", "7", "--request-id", id, "--json"]),
            send(
                Request::CreateWindow {
                    request_id: String::new(),
                    source_window: 7,
                },
                Some(id),
                true
            )
        );
        assert_eq!(
            control(&["new-tab", "42", "7"]),
            send(
                Request::CreateTab {
                    request_id: String::new(),
                    window_id: 7,
                },
                None,
                false
            )
        );
        let launch_place = (
            Some("/tmp/two words".to_string()),
            vec![
                EnvironmentVariable {
                    name: "EMPTY_VALUE".into(),
                    value: String::new(),
                },
                EnvironmentVariable {
                    name: "EXACT_VALUE".into(),
                    value: "= two words $HOME;🙂".into(),
                },
            ],
        );
        let argv = ["", "two words", "$HOME;🙂", "--json"]
            .map(String::from)
            .to_vec();
        for verb in ["launch-tab", "launch-window"] {
            let request = if verb == "launch-tab" {
                Request::LaunchTab {
                    background: false,
                    request_id: String::new(),
                    window_id: 7,
                    program: "/bin/printf".into(),
                    args: argv.clone(),
                    working_directory: launch_place.0.clone(),
                    title: Some("Exact 🙂".into()),
                    environment: launch_place.1.clone(),
                    environment_policy: Default::default(),
                }
            } else {
                Request::LaunchWindow {
                    request_id: String::new(),
                    source_window: 7,
                    program: "/bin/printf".into(),
                    args: argv.clone(),
                    working_directory: launch_place.0.clone(),
                    title: Some("Exact 🙂".into()),
                    environment: launch_place.1.clone(),
                    environment_policy: Default::default(),
                }
            };
            assert_eq!(
                control(&[
                    verb,
                    "42",
                    "7",
                    "--cwd",
                    "/tmp/two words",
                    "--title",
                    "Exact 🙂",
                    "--env",
                    "EXACT_VALUE== two words $HOME;🙂",
                    "--env",
                    "EMPTY_VALUE=",
                    "--request-id",
                    id,
                    "--json",
                    "--",
                    "/bin/printf",
                    "",
                    "two words",
                    "$HOME;🙂",
                    "--json",
                ]),
                ControlCommand::Launch {
                    process_id: 42,
                    request,
                    request_id: Some(id.into()),
                    env_stdin: false,
                    wait: None,
                    json: true,
                }
            );
        }
        for (arguments, request) in [
            (
                vec!["split-pane", "42", "7", "3", "0", "vertical"],
                Request::SplitPane {
                    request_id: String::new(),
                    window_id: 7,
                    tab_id: 3,
                    pane_id: 0,
                    direction: SplitDirection::Vertical,
                },
            ),
            (
                vec!["split-pane", "42", "7", "3", "0", "h"],
                Request::SplitPane {
                    request_id: String::new(),
                    window_id: 7,
                    tab_id: 3,
                    pane_id: 0,
                    direction: SplitDirection::Horizontal,
                },
            ),
            (
                vec!["focus-window", "42", "7"],
                Request::FocusWindow {
                    request_id: String::new(),
                    window_id: 7,
                },
            ),
            (
                vec!["select-tab", "42", "7", "3"],
                Request::SelectTab {
                    request_id: String::new(),
                    window_id: 7,
                    tab_id: 3,
                },
            ),
            (
                vec!["focus-pane", "42", "7", "3", "0"],
                Request::FocusPane {
                    request_id: String::new(),
                    window_id: 7,
                    tab_id: 3,
                    pane_id: 0,
                },
            ),
            (
                vec!["close-window", "42", "7"],
                Request::CloseWindow {
                    request_id: String::new(),
                    window_id: 7,
                },
            ),
            (
                vec!["close-tab", "42", "7", "3"],
                Request::CloseTab {
                    request_id: String::new(),
                    window_id: 7,
                    tab_id: 3,
                },
            ),
            (
                vec!["close-pane", "42", "7", "3", "1"],
                Request::ClosePane {
                    request_id: String::new(),
                    window_id: 7,
                    tab_id: 3,
                    pane_id: 1,
                },
            ),
            (
                vec!["move-tab", "42", "7", "3", "9", "2"],
                Request::MoveTab {
                    request_id: String::new(),
                    source_window_id: 7,
                    tab_id: 3,
                    destination_window_id: 9,
                    destination_index: 2,
                },
            ),
            (
                vec![
                    "move-pane",
                    "42",
                    "7",
                    "3",
                    "1",
                    "9",
                    "4",
                    "2",
                    "horizontal",
                ],
                Request::MovePane {
                    request_id: String::new(),
                    source_window_id: 7,
                    source_tab_id: 3,
                    pane_id: 1,
                    destination_window_id: 9,
                    destination_tab_id: 4,
                    target_pane_id: 2,
                    direction: SplitDirection::Horizontal,
                },
            ),
            (
                vec!["resize-window", "42", "7", "900", "600"],
                Request::ResizeWindow {
                    request_id: String::new(),
                    window_id: 7,
                    width: 900,
                    height: 600,
                },
            ),
            (
                vec!["resize-divider", "42", "7", "3", "0", "650"],
                Request::ResizeDivider {
                    request_id: String::new(),
                    window_id: 7,
                    tab_id: 3,
                    divider_id: 0,
                    ratio_millis: 650,
                },
            ),
        ] {
            assert_eq!(control(&arguments), send(request, None, false));
        }
        for arguments in [
            vec!["control", "get-text", "42", "7", "3", "1", "--bytes", "0"],
            vec!["control", "send-text", "42", "7", "3", "1", "bad\rinput"],
            vec!["control", "send"],
            vec!["control", "new-window", "0", "7"],
            vec!["control", "new-window", "42", "0"],
            vec!["control", "new-window", "42", "7", "--request-id", "short"],
            vec!["control", "new-window", "42", "7", "--json", "--json"],
            vec!["control", "launch-window", "42", "7"],
            vec!["control", "launch-window", "42", "0", "--", "ok"],
            vec!["control", "launch-window", "42", "7", "--", ""],
            vec![
                "control",
                "launch-window",
                "42",
                "7",
                "--env",
                "NO_EQUALS",
                "--",
                "ok",
            ],
            vec![
                "control",
                "launch-window",
                "42",
                "7",
                "--env",
                "TERM=unsafe",
                "--",
                "ok",
            ],
            vec![
                "control",
                "launch-window",
                "42",
                "7",
                "--cwd",
                "relative",
                "--",
                "ok",
            ],
            vec![
                "control",
                "launch-window",
                "42",
                "7",
                "--json",
                "--json",
                "--",
                "ok",
            ],
            vec!["control", "new-tab", "42", "0"],
            vec!["control", "new-tab", "42", "7", "--request-id", "short"],
            vec!["control", "new-tab", "42", "7", "--json", "--json"],
            vec!["control", "launch-tab", "42", "7"],
            vec!["control", "launch-tab", "42", "7", "--", ""],
            vec![
                "control",
                "launch-tab",
                "42",
                "7",
                "--env",
                "DUPLICATE=first",
                "--env",
                "DUPLICATE=second",
                "--",
                "ok",
            ],
            vec![
                "control",
                "launch-tab",
                "42",
                "7",
                "--cwd",
                "relative",
                "--",
                "ok",
            ],
            vec![
                "control",
                "launch-tab",
                "42",
                "7",
                "--title",
                "",
                "--",
                "ok",
            ],
            vec!["control", "launch-tab", "42", "7", "ok"],
            vec![
                "control",
                "launch-tab",
                "42",
                "7",
                "--json",
                "--json",
                "--",
                "ok",
            ],
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
    fn plain_launches_join_a_running_termeric_and_others_keep_their_own() {
        let cwd = std::path::Path::new("/home/me/project");
        let tabs = |arguments: &[&str]| run(arguments).join_tabs(Some(cwd));
        let plain = tabs(&[]).unwrap();
        assert_eq!(plain.len(), 1);
        assert_eq!(
            plain[0].working_directory.as_deref(),
            Some("/home/me/project")
        );
        assert_eq!(
            (plain[0].program.as_deref(), plain[0].args.len()),
            (None, 0)
        );
        let desktop = tabs(&["--new-window"]).unwrap();
        assert_eq!(desktop.len(), 1);
        let rich = tabs(&[
            "--working-directory",
            "/srv",
            "--title",
            "Build",
            "--tab",
            "--",
            "make",
            "-j8",
        ])
        .unwrap();
        assert_eq!(rich[0].working_directory.as_deref(), Some("/srv"));
        let relative = tabs(&["--working-directory", "src"]).unwrap();
        assert_eq!(
            relative[0].working_directory.as_deref(),
            Some("/home/me/project/src")
        );
        assert_eq!(rich[0].title.as_deref(), Some("Build"));
        assert_eq!(rich[1].program.as_deref(), Some("make"));
        assert_eq!(rich[1].args, ["-j8"]);
        for standalone in [
            &["--separate-process"][..],
            &["--new-window", "--new-window"],
            &["--workspace", "main"],
            &["--split", "vertical"],
            &["--app-id", "work"],
            &["--geometry", "800x600"],
            &["--renderer", "software"],
            &["--terminal-profile", "ssh"],
            &["--profile", "codex:review"],
        ] {
            assert!(
                tabs(standalone).is_none(),
                "{standalone:?} should keep its own process"
            );
        }
        assert!(run(&["--separate-process"]).separate_process);
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
                "workspace-query:all-windows",
                "--allow",
                "navigate",
                "--allow",
                "pane-layout:all-windows",
                "--allow",
                "command-output",
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
            grants.to_string(),
            "process-spawn,command-output,workspace-query:all-windows,navigate,pane-layout:all-windows"
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
        assert!(matches!(
            parse(
                ["plugin", "configure", "sample", "--clear-default-profile"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Plugin(crate::plugin::runtime::PluginAdminCommand::Configure {
                id,
                change: crate::plugin::runtime::PluginConfigurationChange::ClearDefaultProfile,
            }) if id == "sample"
        ));
        assert!(
            parse(
                [
                    "plugin",
                    "configure",
                    "sample",
                    "--clear-default-profile",
                    "extra",
                ]
                .into_iter()
                .map(OsString::from)
            )
            .is_err()
        );
        for invalid in [
            &["--allow", "camera"][..],
            &["--allow", "terminal-text"][..],
            &["--allow", "process-spawn:all-windows"][..],
            &["--allow", "navigate", "--allow", "navigate:all-windows"][..],
        ] {
            assert!(
                parse(
                    ["plugin", "install", "/tmp/sample"]
                        .into_iter()
                        .chain(invalid.iter().copied())
                        .map(OsString::from)
                )
                .is_err(),
                "{invalid:?}"
            );
        }
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
        let ParseOutcome::Domain(crate::domain::DomainCommand::Request {
            request: crate::domain::protocol::Request::CreateDomain { name, spawn },
            json,
        }) = outcome
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
                   ParseOutcome::Domain(crate::domain::DomainCommand::Request {
        request: crate::domain::protocol::Request::ClosePane {
                       domain,
                       pane: 42,
                   },
        ..
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
                       && matches!(*command, crate::domain::DomainCommand::Request {
        request: crate::domain::protocol::Request::ListDomains,
        json: true,
        })
               ));
        assert!(matches!(
            parse(["domain", "proxy-stdio"].into_iter().map(OsString::from)).unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::ProxyStdio { start_server: true })
        ));
        assert!(matches!(
            parse(
                ["domain", "proxy-stdio", "--no-start"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::ProxyStdio {
                start_server: false
            })
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
                   ParseOutcome::Domain(crate::domain::DomainCommand::Request {
        request: crate::domain::protocol::Request::CloseTab {
                       domain,
                       tab: 7,
                   },
        ..
        }) if domain == "domain-123"
               ));
        assert!(matches!(
                   parse(
                       ["domain", "resize-split", "domain-123", "9", "375"]
                           .into_iter()
                           .map(OsString::from)
                   )
                   .unwrap(),
                   ParseOutcome::Domain(crate::domain::DomainCommand::Request {
        request: crate::domain::protocol::Request::ResizeSplit {
                       domain,
                       split: 9,
                       ratio_millis: 375,
                   },
        ..
        }) if domain == "domain-123"
               ));
        assert!(matches!(
                   parse(
                       ["domain", "rename-tab", "domain-123", "7", "Build λ"]
                           .into_iter()
                           .map(OsString::from)
                   )
                   .unwrap(),
                   ParseOutcome::Domain(crate::domain::DomainCommand::Request {
        request: crate::domain::protocol::Request::RenameTab {
                       domain,
                       tab: 7,
                       title: Some(title),
                   },
        ..
        }) if domain == "domain-123" && title == "Build λ"
               ));
        assert!(matches!(
            parse(
                ["domain", "rename-tab", "domain-123", "7", ""]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::Request {
                request: crate::domain::protocol::Request::RenameTab {
                    tab: 7,
                    title: None,
                    ..
                },
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
            ParseOutcome::Domain(crate::domain::DomainCommand::Request {
                request: crate::domain::protocol::Request::SetTabPinned {
                    tab: 7,
                    pinned: true,
                    ..
                },
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
            ParseOutcome::Domain(crate::domain::DomainCommand::Request {
                request: crate::domain::protocol::Request::MoveTab {
                    tab: 7,
                    index: 2,
                    ..
                },
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
            ParseOutcome::Domain(crate::domain::DomainCommand::Request {
                request: crate::domain::protocol::Request::SwapPaneNext { pane: 42, .. },
                ..
            })
        ));
        assert!(matches!(
            parse(
                ["domain", "balance-tab", "domain-123", "7"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::Request {
                request: crate::domain::protocol::Request::BalanceTab { tab: 7, .. },
                ..
            })
        ));
        assert!(matches!(
            parse(
                ["domain", "layout-tab", "domain-123", "7", "grid"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::Request {
                request: crate::domain::protocol::Request::ApplyPaneLayout {
                    tab: 7,
                    preset: crate::panes::PaneLayoutPreset::Grid,
                    ..
                },
                ..
            })
        ));
        assert!(matches!(
            parse(
                ["domain", "layout-tab", "domain-123", "7", "tall"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::Request {
                request: crate::domain::protocol::Request::ApplyPaneLayout {
                    tab: 7,
                    preset: crate::panes::PaneLayoutPreset::Tall,
                    ..
                },
                ..
            })
        ));
        assert!(matches!(
            parse(
                ["domain", "layout-tab", "domain-123", "7", "fat"]
                    .into_iter()
                    .map(OsString::from)
            )
            .unwrap(),
            ParseOutcome::Domain(crate::domain::DomainCommand::Request {
                request: crate::domain::protocol::Request::ApplyPaneLayout {
                    tab: 7,
                    preset: crate::panes::PaneLayoutPreset::Fat,
                    ..
                },
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
            ParseOutcome::Domain(crate::domain::DomainCommand::Request {
                request: crate::domain::protocol::Request::MovePane {
                    pane: 42,
                    tab: 7,
                    ..
                },
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
        let domains = || Ok(config.domains.clone());

        for arguments in [
            ["domain", "--host-profile", "build", "list", "--json"].as_slice(),
            ["domain", "--host-profile=build", "list", "--json"].as_slice(),
        ] {
            let arguments = arguments.iter().map(OsString::from).collect();
            let ParseOutcome::Domain(crate::domain::DomainCommand::Remote {
                destination,
                command,
            }) = parse_collected(arguments, &domains).unwrap()
            else {
                panic!("expected a remote domain command");
            };
            assert_eq!(destination, "developer@build.example");
            assert_eq!(
                *command,
                crate::domain::DomainCommand::Request {
                    request: crate::domain::protocol::Request::ListDomains,
                    json: true,
                }
            );
        }

        let arguments = ["domain", "--host-profile", "build", "attach", "domain-123"]
            .into_iter()
            .map(OsString::from)
            .collect();
        let ParseOutcome::Run(plan) = parse_collected(arguments, &domains).unwrap() else {
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
        assert!(parse_collected(nested, &domains).is_err());
    }

    #[test]
    fn only_a_host_profile_reads_the_configuration() {
        let broken = || anyhow::bail!("configuration unreadable");
        let parse_with_broken_config = |arguments: &[&str]| {
            let arguments = arguments.iter().map(OsString::from).collect();
            parse_collected(arguments, &broken)
        };
        assert!(parse_with_broken_config(&["domain", "list"]).is_ok());
        assert!(parse_with_broken_config(&["domain", "--host", "build.example", "list"]).is_ok());
        for arguments in [
            ["domain", "--host-profile", "build", "list"].as_slice(),
            ["domain", "--host-profile=build", "list"].as_slice(),
        ] {
            let error = parse_with_broken_config(arguments).unwrap_err();
            assert_eq!(error.to_string(), "configuration unreadable");
        }
    }

    #[test]
    fn free_text_values_may_start_with_a_hyphen() {
        use crate::domain::DomainCommand;
        use crate::domain::protocol::Request;
        use crate::plugin::runtime::{PluginAdminCommand, PluginConfigurationChange};
        let parse_args = |arguments: &[&str]| parse(arguments.iter().map(OsString::from)).unwrap();

        let ParseOutcome::Plugin(PluginAdminCommand::Configure { change, .. }) =
            parse_args(&["plugin", "configure", "sample", "--set", "offset", "-5"])
        else {
            panic!("expected plugin configure");
        };
        assert_eq!(
            change,
            PluginConfigurationChange::Set {
                key: "offset".into(),
                value: "-5".into(),
            }
        );
        let ParseOutcome::Plugin(PluginAdminCommand::Configure { change, .. }) =
            parse_args(&["plugin", "configure", "sample", "--default-profile", "-dev"])
        else {
            panic!("expected plugin configure");
        };
        assert_eq!(
            change,
            PluginConfigurationChange::DefaultProfile("-dev".into())
        );

        let ParseOutcome::Domain(DomainCommand::Request {
            request: Request::CreateDomain { name, spawn },
            json: true,
        }) = parse_args(&[
            "domain", "create", "-named", "--title", "-foo", "--cwd", "-dir", "--json",
        ])
        else {
            panic!("expected domain create");
        };
        assert_eq!(name, "-named");
        assert_eq!(spawn.title.as_deref(), Some("-foo"));
        assert_eq!(spawn.working_directory.as_deref(), Some("-dir"));
        let ParseOutcome::Domain(DomainCommand::Upload {
            name, json: false, ..
        }) = parse_args(&[
            "domain", "upload", "domain-1", "0", "source", "--name", "-file",
        ])
        else {
            panic!("expected domain upload");
        };
        assert_eq!(name.as_deref(), Some("-file"));

        let ParseOutcome::Control(crate::control::ControlCommand::Launch {
            request: crate::control::protocol::Request::LaunchTab { title, .. },
            ..
        }) = parse_args(&[
            "control",
            "launch-tab",
            "42",
            "7",
            "--title",
            "-foo",
            "--",
            "/bin/true",
        ])
        else {
            panic!("expected control launch-tab");
        };
        assert_eq!(title.as_deref(), Some("-foo"));
    }

    #[test]
    fn a_remote_working_directory_is_left_for_the_remote_daemon_to_check() {
        let outcome = parse(
            [
                "domain",
                "--host",
                "developer@build.example",
                "spawn",
                "domain-1",
                "--cwd",
                "/only/on/the/remote/computer",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .unwrap();
        let ParseOutcome::Domain(crate::domain::DomainCommand::Remote { command, .. }) = outcome
        else {
            panic!("expected a remote domain command");
        };
        let crate::domain::DomainCommand::Request {
            request: crate::domain::protocol::Request::SpawnTab { spawn, .. },
            ..
        } = *command
        else {
            panic!("expected domain spawn");
        };
        assert_eq!(
            spawn.working_directory.as_deref(),
            Some("/only/on/the/remote/computer")
        );
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
            vec!["domain", "upload", "domain-1", "0", "/tmp/file", "--retain"],
            vec!["domain", "uploads"],
        ] {
            assert!(parse(arguments.into_iter().map(OsString::from)).is_err());
        }
    }
}
