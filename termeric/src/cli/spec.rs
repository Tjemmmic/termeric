//! The `termeric` command as clap describes it, for `--help`, the manual page,
//! and shell completions.
//!
//! Window and tab options depend on their order (`--tab` and `--new-window`
//! start a new group), so `parse_collected` reads them by hand and this
//! description only documents them. Its tests check that every option it
//! describes is one the parser accepts, and that the committed manual page and
//! completions are generated from it.

use super::{
    control_args::ControlCli, domain_args::DomainCli, plugin_args::PluginCli,
    workspace_args::WorkspaceCli,
};
use clap::builder::PossibleValue;
use clap::{Arg, ArgAction, Command, CommandFactory, ValueHint};

const WINDOW_GROUPING: &str = "Window grouping";
const LAUNCH_SELECTION: &str = "Launch selection";
const IDENTITY_AND_SIZE: &str = "Native identity and size";
const RENDERING: &str = "Rendering";

/// When a launch opens its window in a running Termeric, and when it starts
/// a process of its own: the rule `cli::LaunchPlan::join_tabs` and
/// `control::join_running_launch` apply.
fn joining_a_running_termeric() -> String {
    format!(
        "A plain launch opens its window in the Termeric already running on this display, so \
         tabs can move between its windows; the window uses that process's settings and \
         environment. A launch starts a process of its own instead when it passes \
         --separate-process, opens two or more windows, or uses any option besides --new-window, \
         --tab, --working-directory, --title, --command, and a COMMAND; when [window] \
         share_process is false; when the termeric executable differs from the running one; or \
         when any of these variables differs from the running process's: {}.",
        crate::control::JOIN_ENVIRONMENT_VARIABLES.join(", ")
    )
}

/// `termeric mcp`'s closing text: how an agent starts it, and what it may do.
const MCP_AFTER_HELP: &str = "Register it once with each agent that should use it, for example \
`claude mcp add termeric -- termeric mcp` or `codex mcp add termeric -- termeric mcp`. Inside a \
Termeric terminal it serves that terminal's Termeric; elsewhere, the only one running, or the one \
--process names. Its tools list agents, read terminals and agent view conversations, send \
messages to agents, and answer their approvals. While agent features are off ([agent_features] \
enabled = false), every tool but reading a terminal is refused. Sending and answering also need \
[security] allow_gui_control_input = true.";

/// Everything `termeric` accepts, apart from the internal entry points that
/// Termeric itself runs (`agent-event` and the serial bridge).
pub fn command_spec() -> Command {
    Command::new("termeric")
        .version(env!("CARGO_PKG_VERSION"))
        .about("A GPU-accelerated terminal emulator themed around roots and rhizomes")
        .long_about(
            "A GPU-accelerated terminal emulator themed around roots and rhizomes.\n\n\
             Options apply to the current tab or window. The first window and its first tab \
             exist from the start; --tab starts another tab in the current window, and \
             --new-window starts another window. Everything after -- is the command for the \
             current tab.",
        )
        .after_help(joining_a_running_termeric())
        .override_usage(
            "termeric [WINDOW/TAB OPTIONS] [-- COMMAND ARG...]\n       \
             termeric <SUBCOMMAND>",
        )
        .help_template(
            "{name} {version}\n{about-with-newline}\n{usage-heading} {usage}\n\n{all-args}{after-help}",
        )
        .disable_help_subcommand(true)
        // `-h` and `--help` print the same summary, so neither points at the
        // other the way clap's own help flag does.
        .disable_help_flag(true)
        .arg(
            Arg::new("help")
                .short('h')
                .long("help")
                .action(ArgAction::Help)
                .help("Print this help"),
        )
        .subcommand_value_name("SUBCOMMAND")
        .subcommand_help_heading("Subcommands")
        .args(window_grouping())
        .args(launch_selection())
        .args(identity_and_size())
        .arg(
            Arg::new("renderer")
                .long("renderer")
                .value_name("BACKEND")
                .value_parser([
                    PossibleValue::new("auto").help("Try the GPU, then software rendering"),
                    PossibleValue::new("gpu").help("Insist on the GPU"),
                    PossibleValue::new("software").help("Insist on software rendering"),
                ])
                .help("Choose the renderer for every window of this process")
                .help_heading(RENDERING),
        )
        .arg(
            Arg::new("argv")
                .value_name("COMMAND")
                .num_args(1..)
                .last(true)
                .value_hint(ValueHint::CommandWithArguments)
                .help("Run COMMAND in the current tab with every remaining argument exactly")
                .help_heading(LAUNCH_SELECTION),
        )
        .subcommand(DomainCli::command().name("domain"))
        .subcommand(ControlCli::command().name("control"))
        .subcommand(PluginCli::command().name("plugin"))
        .subcommand(WorkspaceCli::command().name("workspace"))
        .subcommand(
            Command::new("mcp")
                .about("Serve Termeric's agent tools to a coding agent over MCP (experimental)")
                .after_help(MCP_AFTER_HELP)
                .disable_version_flag(true)
                .arg(
                    Arg::new("process")
                        .long("process")
                        .value_name("PID")
                        .help("Serve this Termeric process instead of the one found automatically"),
                ),
        )
}

/// `termeric mcp --help`, which its hand parser prints.
pub fn mcp_help() -> String {
    let mut spec = command_spec();
    // Building names the subcommand `termeric mcp` in its usage line.
    spec.build();
    spec.find_subcommand_mut("mcp")
        .expect("the spec describes mcp")
        .render_help()
        .to_string()
        .trim_end()
        .to_owned()
}

fn flag(name: &'static str, help: &'static str, heading: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .action(ArgAction::SetTrue)
        .help(help)
        .help_heading(heading)
}

fn option(
    name: &'static str,
    value_name: &'static str,
    help: &'static str,
    heading: &'static str,
) -> Arg {
    Arg::new(name)
        .long(name)
        .value_name(value_name)
        .action(ArgAction::Append)
        .help(help)
        .help_heading(heading)
}

fn window_grouping() -> [Arg; 5] {
    [
        flag(
            "new-window",
            "Start another window; first, a fresh one that skips restore",
            WINDOW_GROUPING,
        ),
        flag(
            "separate-process",
            "Start a process of its own, even when Termeric runs",
            WINDOW_GROUPING,
        ),
        flag(
            "quick-access",
            "Toggle or launch one top-edge drop-down window",
            WINDOW_GROUPING,
        ),
        flag(
            "tab",
            "Start another tab in the current window",
            WINDOW_GROUPING,
        ),
        option(
            "split",
            "DIRECTION",
            "Split the current tab after launch",
            WINDOW_GROUPING,
        )
        .value_parser([
            PossibleValue::new("horizontal").help("Add a shell pane below"),
            PossibleValue::new("vertical").help("Add a shell pane to the right"),
            PossibleValue::new("h").hide(true),
            PossibleValue::new("v").hide(true),
        ]),
    ]
}

fn launch_selection() -> [Arg; 6] {
    [
        option(
            "working-directory",
            "PATH",
            "Start the current tab in PATH",
            LAUNCH_SELECTION,
        )
        .visible_alias("cwd")
        .value_hint(ValueHint::DirPath),
        option(
            "command",
            "PROGRAM",
            "Run PROGRAM directly, without arguments",
            LAUNCH_SELECTION,
        )
        .value_hint(ValueHint::CommandName),
        option(
            "terminal-profile",
            "NAME",
            "Launch a configured terminal profile",
            LAUNCH_SELECTION,
        ),
        option(
            "profile",
            "AGENT:PROFILE",
            "Launch a coding agent with a named profile",
            LAUNCH_SELECTION,
        ),
        option(
            "workspace",
            "ID",
            "Restore one complete workspace into this window",
            LAUNCH_SELECTION,
        ),
        option(
            "title",
            "TITLE",
            "Set the current tab title",
            LAUNCH_SELECTION,
        ),
    ]
}

fn identity_and_size() -> [Arg; 2] {
    [
        option(
            "geometry",
            "WIDTHxHEIGHT",
            "Initial physical-pixel window size",
            IDENTITY_AND_SIZE,
        ),
        option(
            "app-id",
            "NAME",
            "Set the X11 WM_CLASS and Wayland application ID",
            IDENTITY_AND_SIZE,
        )
        .visible_alias("class"),
    ]
}

#[cfg(test)]
mod tests {
    use super::{command_spec, joining_a_running_termeric};
    use crate::cli::{ParseOutcome, parse};
    use std::ffi::OsString;
    use std::path::Path;

    /// The variables a person sets to change how Termeric runs, for the
    /// manual page's ENVIRONMENT section.
    const ENVIRONMENT: [(&str, &str); 5] = [
        (
            "TERMERIC_TERM",
            "The TERM programs in Termeric see: auto (the default) uses xterm-termeric when its \
             terminfo entry is installed and xterm-256color otherwise; xterm-256color (or \
             compatibility) always uses xterm-256color; xterm-termeric insists on Termeric's own \
             entry and falls back with a logged warning.",
        ),
        (
            "TERMERIC_TERMINFO_DIR",
            "A directory searched first for the compiled xterm-termeric entry, before TERMINFO, \
             ~/.terminfo, $XDG_DATA_HOME/terminfo, TERMINFO_DIRS, the install prefix's \
             share/terminfo, and the system directories.",
        ),
        (
            "WGPU_BACKEND",
            "The graphics API to render with: vulkan or gl. With --renderer software, gl and \
             LIBGL_ALWAYS_SOFTWARE=true use Mesa's software OpenGL.",
        ),
        (
            "RUST_LOG",
            "What Termeric logs to standard error, for example warn,termeric=debug.",
        ),
        (
            "XDG_CONFIG_HOME, XDG_DATA_HOME",
            "Where Termeric keeps its configuration (termeric/config.toml, custom themes, \
             plugins) and its data (workspaces, tab snapshots, agent transcripts), by default \
             ~/.config and ~/.local/share.",
        ),
    ];

    #[test]
    fn spec_is_internally_consistent() {
        command_spec().debug_assert();
    }

    /// Every option the description offers, under every name, is one the
    /// hand parser understands, with each value it suggests.
    #[test]
    fn parser_accepts_every_described_option() {
        let mut spec = command_spec();
        // Building settles each argument's value count.
        spec.build();
        for argument in spec.get_arguments() {
            let Some(long) = argument.get_long() else {
                continue;
            };
            let mut names = vec![long];
            names.extend(argument.get_all_aliases().unwrap_or_default());
            let takes_value = argument
                .get_num_args()
                .is_some_and(|range| range.takes_values());
            let values: Vec<String> = if !takes_value {
                vec![]
            } else if argument.get_possible_values().is_empty() {
                vec!["x".to_owned()]
            } else {
                argument
                    .get_possible_values()
                    .iter()
                    .map(|value| value.get_name().to_owned())
                    .collect()
            };
            for name in names {
                let option = format!("--{name}");
                // A value follows its option as the next argument or, as the
                // completions offer it, after `=`.
                let runs: Vec<Vec<OsString>> = if values.is_empty() {
                    vec![vec![option.clone().into()]]
                } else {
                    values
                        .iter()
                        .flat_map(|value| {
                            [
                                vec![option.clone().into(), value.into()],
                                vec![format!("{option}={value}").into()],
                            ]
                        })
                        .collect()
                };
                for arguments in runs {
                    match parse(arguments.clone()) {
                        Ok(_) => {}
                        // Free-form values may be invalid; the option itself
                        // must still be known.
                        Err(error) if argument.get_possible_values().is_empty() => assert!(
                            !error.to_string().contains("unknown option"),
                            "{arguments:?}: {error}"
                        ),
                        Err(error) => panic!("{arguments:?} was rejected: {error}"),
                    }
                }
            }
        }
    }

    #[test]
    fn subcommands_are_the_ones_the_parser_dispatches() {
        let spec = command_spec();
        let names: Vec<&str> = spec
            .get_subcommands()
            .map(|command| command.get_name())
            .collect();
        assert_eq!(names, ["domain", "control", "plugin", "workspace", "mcp"]);
        let parse_strs = |arguments: &[&str]| parse(arguments.iter().map(OsString::from));
        for name in ["domain", "control", "plugin", "workspace", "mcp"] {
            assert!(
                matches!(parse_strs(&[name, "--help"]), Ok(ParseOutcome::Print(help)) if help.contains(name)),
                "{name} is not a subcommand the parser dispatches"
            );
        }
        assert!(matches!(
            parse_strs(&["mcp", "--help"]),
            Ok(ParseOutcome::Print(help)) if help.contains("termeric mcp [OPTIONS]") && help.contains("--process <PID>")
        ));
        for arguments in [&["mcp", "--process", "42"][..], &["mcp", "--process=42"]] {
            assert!(
                matches!(
                    parse_strs(arguments),
                    Ok(ParseOutcome::Mcp {
                        process_id: Some(42),
                        ..
                    })
                ),
                "{arguments:?}"
            );
        }
        assert!(parse_strs(&["mcp", "--help=x"]).is_err());
        assert!(parse_strs(&["mcp", "--process="]).is_err());
    }

    /// `argument` with its aliases named after its help, as `--help` names
    /// them: clap_mangen leaves them out of the manual page.
    fn with_aliases_in_help(argument: clap::Arg) -> clap::Arg {
        let aliases = argument.get_visible_aliases().unwrap_or_default();
        if aliases.is_empty() {
            return argument;
        }
        let label = if aliases.len() == 1 {
            "alias"
        } else {
            "aliases"
        };
        let names = aliases
            .iter()
            .map(|alias| format!("--{alias}"))
            .collect::<Vec<_>>()
            .join(", ");
        let help = argument
            .get_help()
            .map(ToString::to_string)
            .unwrap_or_default();
        argument.help(format!("{help} [{label}: {names}]"))
    }

    #[test]
    fn the_manual_page_names_every_alias_help_does() {
        let page = String::from_utf8(manual_page().unwrap()).unwrap();
        let help = crate::cli::help_text();
        let spec = command_spec();
        let aliases: Vec<&str> = spec
            .get_arguments()
            .flat_map(|argument| argument.get_visible_aliases().unwrap_or_default())
            .collect();
        assert_eq!(aliases, ["cwd", "class"]);
        for alias in aliases {
            let named = format!("[alias: --{alias}]");
            assert!(help.contains(&named), "{named} is not in --help");
            assert!(
                page.contains(&named.replace('-', "\\-")),
                "{named} is not in the manual page"
            );
        }
    }

    /// One manual page for the whole command. clap_mangen's own subcommand
    /// section names a page per subcommand, which Termeric doesn't ship, so
    /// the subcommands point at their `--help` instead.
    fn manual_page() -> std::io::Result<Vec<u8>> {
        use roff::{Roff, bold, roman};
        let spec = command_spec().mut_args(with_aliases_in_help);
        let man = clap_mangen::Man::new(spec.clone());
        let mut page = Vec::new();
        man.render_title(&mut page)?;
        man.render_name_section(&mut page)?;
        man.render_synopsis_section(&mut page)?;
        man.render_description_section(&mut page)?;
        man.render_options_section(&mut page)?;
        let mut sections = Roff::new();
        sections.control("SH", ["SUBCOMMANDS"]);
        for subcommand in spec
            .get_subcommands()
            .filter(|command| !command.is_hide_set())
        {
            let name = format!("termeric {}", subcommand.get_name());
            let about = subcommand
                .get_about()
                .map(ToString::to_string)
                .unwrap_or_default();
            sections.control("TP", []);
            sections.text([bold(name.as_str())]);
            sections.text([
                roman(format!("{about}. Run ")),
                bold(format!("{name} --help")),
                roman(" for its commands and options."),
            ]);
        }
        sections.control("SH", ["JOINING A RUNNING TERMERIC"]);
        sections.text([roman(joining_a_running_termeric())]);
        sections.control("SH", ["ENVIRONMENT"]);
        for (name, description) in ENVIRONMENT {
            sections.control("TP", []);
            sections.text([bold(name)]);
            sections.text([roman(description)]);
        }
        sections.control("SH", ["FILES"]);
        sections.control("TP", []);
        sections.text([bold("~/.config/termeric/config.toml")]);
        sections.text([roman(
            "The configuration, reloaded when it is saved. config.example.toml, in Termeric's \
             documentation directory (such as /usr/share/doc/termeric), lists every setting with \
             its default.",
        )]);
        sections.control("SH", ["SEE ALSO"]);
        sections.text([roman(format!(
            "The project, with its README and plugin examples: {}",
            env!("CARGO_PKG_REPOSITORY")
        ))]);
        sections.to_writer(&mut page)?;
        man.render_version_section(&mut page)?;
        Ok(page)
    }

    /// The generated files committed under `assets/`, and what each holds now.
    fn generated_files() -> Vec<(&'static str, Vec<u8>)> {
        let manual = manual_page().expect("render the manual page");
        let completion = |shell: clap_complete::Shell| {
            let mut output = Vec::new();
            clap_complete::generate(shell, &mut command_spec(), "termeric", &mut output);
            output
        };
        vec![
            ("man/termeric.1", manual),
            (
                "completions/termeric.bash",
                completion(clap_complete::Shell::Bash),
            ),
            (
                "completions/_termeric",
                completion(clap_complete::Shell::Zsh),
            ),
            (
                "completions/termeric.fish",
                completion(clap_complete::Shell::Fish),
            ),
        ]
    }

    #[test]
    fn committed_manual_page_and_completions_are_current() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets");
        let update = std::env::var("TERMERIC_UPDATE_GENERATED").as_deref() == Ok("1");
        let mut stale = Vec::new();
        for (relative, generated) in generated_files() {
            let path = assets.join(relative);
            if update {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, &generated).unwrap();
            }
            if std::fs::read(&path).ok().as_deref() != Some(generated.as_slice()) {
                stale.push(relative);
            }
        }
        assert!(
            stale.is_empty(),
            "stale generated files under assets/: {stale:?}; rerun with \
             TERMERIC_UPDATE_GENERATED=1 to regenerate them"
        );
    }
}
