//! Deterministic command-line parsing into window-local startup plans.

mod control_args;
mod domain_args;
mod plugin_args;
mod spec;
mod workspace_args;

use crate::domain::model::validate_id;
use crate::panes::SplitDirection;
use anyhow::Context;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub windows: Vec<WindowLaunch>,
    /// Start a process of its own even when a running Termeric could open
    /// the window.
    pub separate_process: bool,
    /// Renderer override for the whole process, which every window of a
    /// process shares.
    pub renderer_backend: Option<crate::config::RendererBackend>,
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
    Workspace(crate::workspace::WorkspaceCommand),
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
            renderer_backend: None,
        }
    }
}

impl LaunchPlan {
    /// Whether this launch asks for no windows or tabs of its own, so it
    /// reopens the workspaces open at the last exit when automatic restore
    /// is on. Process-wide options (`--separate-process`, `--renderer`)
    /// change how the process runs, not what it opens.
    pub fn restores_last_session(&self) -> bool {
        matches!(self.windows.as_slice(), [window] if *window == WindowLaunch::default())
    }

    /// Start each `-- COMMAND` tab where this launch started (`launched_in`)
    /// unless it names a folder, and find a relative program path such as
    /// `./script` from there, as the shell it was typed in would, whether the
    /// tab opens in a running Termeric or in a process of its own.
    pub fn anchor_commands(&mut self, launched_in: &std::path::Path) {
        for tab in self.windows.iter_mut().flat_map(|window| &mut window.tabs) {
            let LaunchTarget::Command { program, .. } = &mut tab.target else {
                continue;
            };
            if program.contains('/')
                && std::path::Path::new(program.as_str()).is_relative()
                && let Some(anchored) = launched_in.join(program.as_str()).to_str()
            {
                *program = anchored.to_owned();
            }
            // A tab's folder must be UTF-8; elsewhere it opens as before.
            if tab.working_directory.is_none() && launched_in.to_str().is_some() {
                tab.working_directory = Some(launched_in.to_path_buf());
            }
        }
    }

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
            || self.renderer_backend.is_some()
            || window.workspace.is_some()
            || window.domain.is_some()
            || window.domain_snapshot.is_some()
            || window.app_id.is_some()
            || window.pixel_size.is_some()
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
    let mut outcome = parse_command(arguments)?;
    if let (ParseOutcome::Run(plan) | ParseOutcome::QuickAccess(plan), Ok(launched_in)) =
        (&mut outcome, std::env::current_dir())
    {
        plan.anchor_commands(&launched_in);
    }
    Ok(Invocation::Command(Box::new(outcome)))
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
    while let Some(argument) = rest.next() {
        let (flag, inline) = split_inline_value(argument);
        if inline.is_none() && matches!(flag.to_str(), Some("-h" | "--help")) {
            return Ok(ParseOutcome::Print(spec::mcp_help()));
        }
        let value = inline.or_else(|| rest.next().cloned());
        match (
            flag.to_str(),
            value.as_ref().and_then(|value| value.to_str()),
        ) {
            (Some("--process"), Some(value)) if process_id.is_none() => {
                process_id = Some(
                    value
                        .parse::<u32>()
                        .ok()
                        .filter(|pid| crate::control::protocol::valid_process_id(*pid))
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
    Some(crate::agents::hook::HookInvocation {
        agent_id: crate::control::protocol::valid_agent_id(agent_id)
            .then(|| agent_id.to_owned())?,
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
    if arguments.first().and_then(|argument| argument.to_str()) == Some("workspace") {
        return parse_workspace(&arguments[1..]);
    }
    let mut plan = LaunchPlan::default();
    let mut index = 0usize;
    while index < arguments.len() {
        let (argument, mut inline) = split_inline_value(&arguments[index]);
        let argument = argument
            .to_str()
            .context("command-line options must be valid UTF-8")?;
        match argument {
            "-h" | "--help" => {
                reject_inline_value(argument, inline.as_ref())?;
                return Ok(ParseOutcome::Print(help_text()));
            }
            "-V" | "--version" => {
                reject_inline_value(argument, inline.as_ref())?;
                return Ok(ParseOutcome::Print(format!(
                    "termeric {}",
                    env!("CARGO_PKG_VERSION")
                )));
            }
            "--separate-process" => plan.separate_process = true,
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
                let value = option_utf8(&arguments, &mut index, &mut inline, "--split")?;
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
                let value = option_os(&arguments, &mut index, &mut inline, "--working-directory")?;
                let window = current_window(&mut plan);
                reject_workspace_mix(window, "--working-directory")?;
                current_tab(window).working_directory = Some(PathBuf::from(value));
            }
            "--title" => {
                let value = option_utf8(&arguments, &mut index, &mut inline, "--title")?;
                validate_single_line("window/tab title", &value)?;
                let window = current_window(&mut plan);
                reject_workspace_mix(window, "--title")?;
                current_tab(window).title = Some(value);
            }
            "--command" => {
                let program = option_utf8(&arguments, &mut index, &mut inline, "--command")?;
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
                let name = option_utf8(&arguments, &mut index, &mut inline, "--terminal-profile")?;
                crate::config::validate_terminal_profile_name(&name)?;
                let window = current_window(&mut plan);
                reject_workspace_mix(window, "--terminal-profile")?;
                set_target(
                    current_tab(window),
                    LaunchTarget::TerminalProfile { name },
                    "--terminal-profile",
                )?;
            }
            "--profile" => {
                let value = option_utf8(&arguments, &mut index, &mut inline, "--profile")?;
                let (plugin_id, profile) = value.split_once(':').ok_or_else(|| {
                    anyhow::anyhow!("--profile expects AGENT:PROFILE (for example codex:review)")
                })?;
                crate::plugin::validate_plugin_id("--profile agent", plugin_id)?;
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
                let value = option_utf8(&arguments, &mut index, &mut inline, "--workspace")?;
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
                let value = option_utf8(&arguments, &mut index, &mut inline, "--geometry")?;
                current_window(&mut plan).pixel_size = Some(parse_geometry(&value)?);
            }
            "--class" | "--app-id" => {
                let value = option_utf8(&arguments, &mut index, &mut inline, argument)?;
                validate_id("application identity", &value)?;
                current_window(&mut plan).app_id = Some(value);
            }
            "--renderer" => {
                let value = option_utf8(&arguments, &mut index, &mut inline, "--renderer")?;
                let parsed = match value.as_str() {
                    "auto" => crate::config::RendererBackend::Auto,
                    "gpu" => crate::config::RendererBackend::Gpu,
                    "software" => crate::config::RendererBackend::Software,
                    _ => anyhow::bail!("--renderer expects auto, gpu, or software, got {value:?}"),
                };
                if plan.renderer_backend.replace(parsed).is_some() {
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
        // Every option that takes a value has taken its inline one.
        reject_inline_value(argument, inline.as_ref())?;
        index += 1;
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

/// Splits `--option=value` at its first `=` into the option and its value,
/// as clap and the shell completions spell it. Any other argument comes back
/// whole.
fn split_inline_value(argument: &OsStr) -> (&OsStr, Option<OsString>) {
    use std::os::unix::ffi::OsStrExt;
    let bytes = argument.as_bytes();
    match bytes.iter().position(|&byte| byte == b'=') {
        // `--=x` names no option, so it stays whole and is rejected as one.
        Some(equals) if equals > 2 && bytes.starts_with(b"--") => (
            OsStr::from_bytes(&bytes[..equals]),
            Some(OsStr::from_bytes(&bytes[equals + 1..]).to_owned()),
        ),
        _ => (argument, None),
    }
}

/// An option that takes no value refuses the one given with `=`.
fn reject_inline_value(option: &str, inline: Option<&OsString>) -> anyhow::Result<()> {
    if inline.is_some() {
        anyhow::bail!("{option} does not take a value");
    }
    Ok(())
}

/// The value of `option`: what followed its `=` (`--title=Logs`), else the
/// next argument.
fn option_os(
    arguments: &[OsString],
    index: &mut usize,
    inline: &mut Option<OsString>,
    option: &str,
) -> anyhow::Result<OsString> {
    if let Some(value) = inline.take() {
        return Ok(value);
    }
    *index += 1;
    arguments
        .get(*index)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{option} requires a value"))
}

fn option_utf8(
    arguments: &[OsString],
    index: &mut usize,
    inline: &mut Option<OsString>,
    option: &str,
) -> anyhow::Result<String> {
    option_os(arguments, index, inline, option)?
        .into_string()
        .map_err(|_| anyhow::anyhow!("{option} value must be valid UTF-8"))
}

/// A window size: the sizes a running GUI's windows may be resized to.
fn parse_geometry(value: &str) -> anyhow::Result<(u32, u32)> {
    use crate::control::protocol::{MAX_WINDOW_DIMENSION, MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH};
    let (width, height) = value
        .split_once(['x', 'X'])
        .ok_or_else(|| anyhow::anyhow!("--geometry expects WIDTHxHEIGHT in physical pixels"))?;
    let width: u32 = width.parse().context("invalid --geometry width")?;
    let height: u32 = height.parse().context("invalid --geometry height")?;
    if !(MIN_WINDOW_WIDTH..=MAX_WINDOW_DIMENSION).contains(&width)
        || !(MIN_WINDOW_HEIGHT..=MAX_WINDOW_DIMENSION).contains(&height)
    {
        anyhow::bail!(
            "--geometry must be between {MIN_WINDOW_WIDTH}x{MIN_WINDOW_HEIGHT} and {MAX_WINDOW_DIMENSION}x{MAX_WINDOW_DIMENSION} pixels"
        );
    }
    Ok((width, height))
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
    spec::command_spec()
        .render_help()
        .to_string()
        .trim_end()
        .to_owned()
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

fn parse_workspace(arguments: &[OsString]) -> anyhow::Result<ParseOutcome> {
    Ok(
        match parse_subcommand::<workspace_args::WorkspaceCli>(arguments)? {
            Ok(workspace) => ParseOutcome::Workspace(workspace.into_command()),
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

#[cfg(test)]
mod tests;
