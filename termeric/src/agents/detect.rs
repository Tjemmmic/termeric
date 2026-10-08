//! Recognizing an agent from the process in the foreground of a terminal.

/// Script interpreters whose first script argument names the real program.
pub(super) const INTERPRETERS: &[&str] = &[
    "node", "nodejs", "bun", "deno", "python", "python3", "sh", "bash", "dash", "zsh",
];
/// Shells, whose `-c` argument is a command line rather than a script.
const SHELLS: &[&str] = &["sh", "bash", "dash", "zsh"];
/// Extensions of source files an interpreter can run directly.
const SCRIPT_EXTENSIONS: &[&str] = &[".js", ".mjs", ".cjs", ".ts", ".py", ".sh"];

/// The program name an argument vector was invoked as: the first word's file
/// name, or for an interpreter the name of the script it runs.
///
/// An installed program is a script without an extension (its shebang names
/// the interpreter) or a package's entry point under `node_modules`. A source
/// file elsewhere, such as `python3 claude.py`, is someone's own script, not
/// the program it happens to share a name with.
pub fn program_name(argv: &[String]) -> Option<String> {
    let first = file_name(argv.first()?)?;
    if !INTERPRETERS.contains(&first.as_str()) {
        return Some(first);
    }
    let script = script_argument(argv)?;
    let name = file_name(script)?;
    match SCRIPT_EXTENSIONS
        .iter()
        .find_map(|extension| name.strip_suffix(extension))
    {
        None => Some(name),
        Some(stem) if script.contains("/node_modules/") && !stem.is_empty() => {
            Some(stem.to_string())
        }
        Some(_) => None,
    }
}

/// The npm package whose file an interpreter runs, such as
/// `@anthropic-ai/claude-code`. Shims from pnpm and Volta run a package's
/// entry file by its full path, and that file's own name (`cli.js`) says
/// nothing.
pub fn package_name(argv: &[String]) -> Option<String> {
    let first = file_name(argv.first()?)?;
    if !INTERPRETERS.contains(&first.as_str()) {
        return None;
    }
    let (_, inside) = script_argument(argv)?.rsplit_once("/node_modules/")?;
    let mut parts = inside.split('/');
    let first = parts.next().filter(|part| !part.is_empty())?;
    if first.starts_with('@') {
        let name = parts.next().filter(|part| !part.is_empty())?;
        Some(format!("{first}/{name}"))
    } else {
        Some(first.to_string())
    }
}

/// The script an interpreter runs: its first argument that is not an
/// option, unless a shell was given a command line with `-c`.
fn script_argument(argv: &[String]) -> Option<&String> {
    let shell = SHELLS.contains(&file_name(argv.first()?)?.as_str());
    // Skip interpreter options such as `--no-warnings` to reach the script.
    for argument in &argv[1..] {
        if shell && argument == "-c" {
            return None;
        }
        if !argument.starts_with('-') {
            return Some(argument);
        }
    }
    None
}

fn file_name(arg: &str) -> Option<String> {
    let name = std::path::Path::new(arg).file_name()?.to_str()?;
    (!name.is_empty()).then(|| name.to_string())
}

/// Whether `process` is `ancestor` or was started, directly or through
/// other processes, by it.
pub fn descends_from(process: u32, ancestor: u32) -> bool {
    // Deep enough for any real launcher chain, and bounded against a
    // pathological one.
    const MAX_DEPTH: usize = 64;
    let mut current = process;
    for _ in 0..MAX_DEPTH {
        if current == ancestor {
            return true;
        }
        match parent_process(current) {
            Some(parent) if parent != 0 && parent != current => current = parent,
            _ => return false,
        }
    }
    false
}

/// The parent of `process`, from the field after its name in
/// `/proc/PID/stat`. The name may itself hold spaces and parentheses, so the
/// fields are read after its last closing parenthesis.
fn parent_process(process: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{process}/stat")).ok()?;
    let fields = &stat[stat.rfind(')')? + 1..];
    fields.split_whitespace().nth(1)?.parse().ok()
}

/// The argument vector of the process leading the terminal's foreground
/// process group.
pub fn foreground_argv(process_group: i32) -> Option<Vec<String>> {
    if process_group <= 0 {
        return None;
    }
    let raw = std::fs::read(format!("/proc/{process_group}/cmdline")).ok()?;
    let argv: Vec<String> = raw
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect();
    (!argv.is_empty()).then_some(argv)
}

/// Whether a terminal's own child, its shell or the program the tab
/// launched, holds the terminal's foreground rather than a program it
/// started.
pub(crate) fn child_holds_foreground(child: Option<i64>, foreground_group: Option<i32>) -> bool {
    child.is_some() && child == foreground_group.map(i64::from)
}

/// Re-identify the coding agent in the terminal on `pty`, launched as
/// `launch`, from its foreground process, recording it in `session`. A
/// terminal Termeric launched as an agent keeps that identity even where
/// the platform cannot report the foreground process. Returns whether the
/// terminal's agent changed.
pub(crate) fn observe_terminal(
    catalog: &super::AgentCatalog,
    pty: &termeric_core::pty::PtyHandle,
    launch: &crate::workspace::WorkspaceLaunch,
    session: &mut crate::session::terminal_state::TerminalIntegration,
) -> bool {
    let (launched_id, profile) = match launch {
        crate::workspace::WorkspaceLaunch::Plugin { plugin_id, profile } => (
            catalog
                .get(plugin_id)
                .filter(|agent| agent.enabled)
                .map(|agent| agent.id.as_str()),
            profile,
        ),
        _ => (None, &None),
    };
    let foreground_group = pty.foreground_process_group();
    let foreground = foreground_group
        .and_then(foreground_argv)
        .and_then(|argv| catalog.identify(&argv).map(|agent| agent.id.clone()));
    // An agent that reports through hooks stays recognized, even when its
    // process name is unfamiliar (for example one started through `npx`),
    // only while the process group it reported from still holds the
    // foreground. Anything started after it, such as a shell or `ssh`, is
    // not the agent.
    let child = pty.child_id().map(i64::from);
    // The terminal's own child (its shell) in front means no agent is,
    // whatever a program elsewhere that inherited this terminal's identity
    // reports.
    let child_in_front = child_holds_foreground(child, foreground_group);
    let reported = session
        .agent_status
        .as_ref()
        .filter(|status| {
            !child_in_front
                && foreground_group.is_some()
                && status.process_group == foreground_group
        })
        .map(|status| status.agent_id.clone());
    // The agent Termeric started, itself in front, is known exactly; a guess
    // from its program name could name another agent that shares the
    // program.
    let launched_in_front = launched_id.filter(|_| child_in_front);
    let observed = launched_in_front
        .map(|agent_id| (agent_id.to_owned(), child))
        .or_else(|| foreground.map(|agent_id| (agent_id, foreground_group.map(i64::from))))
        .or_else(|| reported.map(|agent_id| (agent_id, foreground_group.map(i64::from))))
        .or_else(|| launched_id.map(|agent_id| (agent_id.to_owned(), child)))
        .map(|(agent_id, process)| {
            let launched = launched_id == Some(agent_id.as_str());
            super::AgentSession {
                profile: launched.then(|| profile.clone()).flatten(),
                launched,
                agent_id,
                process,
            }
        });
    let mut changed = false;
    if session.agent_status.as_ref().is_some_and(|status| {
        observed
            .as_ref()
            .is_none_or(|agent| agent.agent_id != status.agent_id)
    }) {
        // The agent that reported is gone, even if it never said so.
        session.agent_status = None;
        changed = true;
    }
    changed |= session.set_agent(observed);
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_names_follow_scripts_through_interpreters() {
        let name = |argv: &[&str]| {
            program_name(
                &argv
                    .iter()
                    .map(|arg| (*arg).to_string())
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(name(&["codex"]).as_deref(), Some("codex"));
        assert_eq!(
            name(&["/usr/bin/opencode", "run"]).as_deref(),
            Some("opencode")
        );
        assert_eq!(name(&["python3", "-m"]), None);
        assert_eq!(
            name(&["python3", "/usr/bin/aider"]).as_deref(),
            Some("aider")
        );
        assert_eq!(name(&["node"]), None);
        // Installed entry points are extensionless or live in a package.
        assert_eq!(
            name(&[
                "node",
                "--no-warnings",
                "/usr/lib/node_modules/@openai/codex/bin/codex.js"
            ])
            .as_deref(),
            Some("codex")
        );
        assert_eq!(
            name(&["/bin/sh", "/usr/local/bin/goose"]).as_deref(),
            Some("goose")
        );
        assert_eq!(name(&["python3", "-m", "aider"]).as_deref(), Some("aider"));
        // Someone's own source file, or a shell's command line, is not one.
        assert_eq!(name(&["python3", "tools/claude.py"]), None);
        assert_eq!(name(&["node", "codex.js"]), None);
        assert_eq!(name(&["bash", "deploy.sh"]), None);
        assert_eq!(name(&["sh", "-c", "codex"]), None);
        assert_eq!(name(&["bash"]), None);

        let package = |argv: &[&str]| {
            package_name(
                &argv
                    .iter()
                    .map(|arg| (*arg).to_string())
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            package(&[
                "node",
                "/home/me/.local/share/pnpm/global/5/.pnpm/@anthropic-ai+claude-code@2.0.0/node_modules/@anthropic-ai/claude-code/cli.js"
            ])
            .as_deref(),
            Some("@anthropic-ai/claude-code")
        );
        assert_eq!(
            package(&["node", "/opt/node_modules/opencode-ai/bin/opencode"]).as_deref(),
            Some("opencode-ai")
        );
        assert_eq!(package(&["node", "/home/me/src/cli.js"]), None);
        assert_eq!(package(&["/usr/lib/node_modules/x/bin/tool"]), None);
    }

    #[test]
    fn ancestry_follows_parents_to_the_terminal_child() {
        /// Stops the child however the test ends.
        struct Reaped(std::process::Child);
        impl Drop for Reaped {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let child = Reaped(
            std::process::Command::new("sleep")
                .arg("5")
                .spawn()
                .unwrap(),
        );
        let me = std::process::id();
        assert!(descends_from(child.0.id(), me));
        assert!(descends_from(me, me));
        assert!(!descends_from(me, child.0.id()));
        assert!(!descends_from(u32::MAX, me));
    }

    #[test]
    fn foreground_argv_reads_this_process() {
        let argv = foreground_argv(std::process::id() as i32).unwrap();
        assert!(!argv.is_empty());
        assert_eq!(foreground_argv(0), None);
    }
}
