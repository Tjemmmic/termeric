//! Private local control plane for already-running GUI processes.
//!
//! Inventory and bounded terminal-text queries are read-only; typed mutations
//! are explicit and ID-targeted. Opt-in input accepts typed text only, while
//! command launches accept bounded exact argv and explicit environment
//! overlays. Environment values stay out of inventory, responses, and logs;
//! clipboard data and plugin/provider credentials stay hidden.

use std::os::unix::ffi::OsStringExt as _;

mod environment_input;
pub(crate) mod launches;
pub mod protocol;

mod client;
mod paths;
mod server;
mod wait;

/// One `termeric control` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlCommand {
    List {
        json: bool,
    },
    LaunchStatus {
        process_id: u32,
        launch_request_id: String,
        json: bool,
    },
    WaitLaunch {
        process_id: u32,
        launch_request_id: String,
        timeout_seconds: Option<u32>,
        json: bool,
    },
    /// One request to GUI process `process_id`. The parser checks an action
    /// under a placeholder ID; sending fills in `request_id`, or a fresh one.
    Send {
        process_id: u32,
        request: protocol::Request,
        request_id: Option<String>,
        json: bool,
    },
    /// A `LaunchTab` or `LaunchWindow` request, which may also take its
    /// environment from standard input and wait for its child to exit.
    Launch {
        process_id: u32,
        request: protocol::Request,
        request_id: Option<String>,
        env_stdin: bool,
        wait: Option<Wait>,
        json: bool,
    },
}

/// `--wait`, and how long it may wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wait {
    pub timeout_seconds: Option<u32>,
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

/// The variables that shape the Termeric process itself: its logging,
/// renderer, terminal identity, and where it reads configuration and keeps
/// data. A launch whose values differ starts a process of its own, since a
/// running process cannot take them on; the rest of the launcher's
/// environment is never carried to a running process.
pub const JOIN_ENVIRONMENT_VARIABLES: [&str; 11] = [
    "RUST_LOG",
    "WGPU_BACKEND",
    "WGPU_ADAPTER_NAME",
    "WGPU_POWER_PREF",
    "LIBGL_ALWAYS_SOFTWARE",
    "TERMERIC_TERM",
    "TERMERIC_TERMINFO_DIR",
    "TERMINFO",
    "TERMINFO_DIRS",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
];

/// Digits in an environment identity.
pub const ENVIRONMENT_IDENTITY_DIGITS: usize = 16;

/// A fingerprint of this process's `JOIN_ENVIRONMENT_VARIABLES`, as first
/// asked. A process asks at startup, so what it reports is what it runs
/// with.
pub fn environment_identity() -> &'static str {
    static IDENTITY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    IDENTITY.get_or_init(|| {
        fingerprint_environment(|name| std::env::var_os(name).map(std::ffi::OsString::into_vec))
    })
}

/// The fingerprint of the values `lookup` gives each of
/// `JOIN_ENVIRONMENT_VARIABLES`. An unset variable differs from an empty
/// one, and every value is length-prefixed, so no two environments share an
/// encoding.
fn fingerprint_environment(lookup: impl Fn(&str) -> Option<Vec<u8>>) -> String {
    let mut encoded = Vec::new();
    for name in JOIN_ENVIRONMENT_VARIABLES {
        encoded.extend_from_slice(name.as_bytes());
        match lookup(name) {
            Some(value) => {
                encoded.push(1);
                encoded.extend_from_slice(&(value.len() as u64).to_le_bytes());
                encoded.extend_from_slice(&value);
            }
            None => encoded.push(0),
        }
    }
    format!(
        "{:0width$x}",
        termeric_core::checksum::fnv1a64(&encoded),
        width = ENVIRONMENT_IDENTITY_DIGITS
    )
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
    let environment = environment_identity();
    let variable = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let token = variable("XDG_ACTIVATION_TOKEN").or_else(|| variable("DESKTOP_STARTUP_ID"));
    match client::join_running(&display, &build, environment, token, tabs) {
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
    let reading = protocol::Request::ReadText {
        window_id,
        tab_id,
        pane_id,
        history_lines,
        max_bytes: protocol::MAX_TERMINAL_TEXT_BYTES,
    };
    client::send(process_id, reading, None, false)
}

pub struct ControlOutput {
    pub text: String,
    pub exit_code: u8,
}

pub fn execute(command: ControlCommand) -> anyhow::Result<ControlOutput> {
    let text = match command {
        ControlCommand::List { json } => client::list(json)?,
        ControlCommand::LaunchStatus {
            process_id,
            launch_request_id,
            json,
        } => client::launch_status(process_id, launch_request_id, json)?,
        ControlCommand::WaitLaunch {
            process_id,
            launch_request_id,
            timeout_seconds,
            json,
        } => return wait::launch(process_id, launch_request_id, timeout_seconds, json),
        ControlCommand::Send {
            process_id,
            request,
            request_id,
            json,
        } => client::send(process_id, request, request_id, json)?,
        ControlCommand::Launch {
            process_id,
            mut request,
            request_id,
            env_stdin,
            wait,
            json,
        } => {
            if env_stdin {
                environment_input::merge_stdin(&mut request)?;
            }
            if let Some(Wait { timeout_seconds }) = wait {
                return wait::launch_and_wait(
                    process_id,
                    request,
                    request_id,
                    timeout_seconds,
                    json,
                );
            }
            client::send(process_id, request, request_id, json)?
        }
    };
    Ok(ControlOutput { text, exit_code: 0 })
}

#[cfg(test)]
mod tests {
    use super::{ENVIRONMENT_IDENTITY_DIGITS, JOIN_ENVIRONMENT_VARIABLES, fingerprint_environment};

    #[test]
    fn environment_identity_depends_only_on_the_listed_variables() {
        let unset = fingerprint_environment(|_| None);
        assert!(termeric_core::hex::is_lower(
            &unset,
            ENVIRONMENT_IDENTITY_DIGITS
        ));
        assert_eq!(unset, fingerprint_environment(|_| None), "deterministic");
        assert_eq!(
            super::environment_identity(),
            super::environment_identity(),
            "asked once"
        );

        let with = |set: &'static str, value: &'static str| {
            fingerprint_environment(move |name| (name == set).then(|| value.as_bytes().to_vec()))
        };
        // An empty value is not an unset one, and each listed variable counts.
        assert_ne!(with("RUST_LOG", ""), unset);
        let mut seen = std::collections::HashSet::from([unset.clone()]);
        for name in JOIN_ENVIRONMENT_VARIABLES {
            assert!(seen.insert(with(name, "x")), "{name}");
        }
        assert_ne!(with("RUST_LOG", "debug"), with("RUST_LOG", "info"));
        // Other variables, such as the launcher's working environment, never do.
        assert_eq!(with("PATH", "/opt/bin"), unset);
        assert_eq!(with("EDITOR", "vim"), unset);
    }
}
