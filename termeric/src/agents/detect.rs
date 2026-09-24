//! Recognizing an agent from the process in the foreground of a terminal.

/// Script interpreters whose first script argument names the real program.
const INTERPRETERS: &[&str] = &[
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
    let shell = SHELLS.contains(&first.as_str());
    // Skip interpreter options such as `--no-warnings` to reach the script.
    let mut arguments = argv[1..].iter();
    let script = loop {
        let argument = arguments.next()?;
        if shell && argument == "-c" {
            return None;
        }
        if !argument.starts_with('-') {
            break argument;
        }
    };
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

fn file_name(arg: &str) -> Option<String> {
    let name = std::path::Path::new(arg).file_name()?.to_str()?;
    (!name.is_empty()).then(|| name.to_string())
}

/// The argument vector of the process leading the terminal's foreground
/// process group.
#[cfg(target_os = "linux")]
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

#[cfg(not(target_os = "linux"))]
pub fn foreground_argv(_process_group: i32) -> Option<Vec<String>> {
    None
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
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn foreground_argv_reads_this_process() {
        let argv = foreground_argv(std::process::id() as i32).unwrap();
        assert!(!argv.is_empty());
        assert_eq!(foreground_argv(0), None);
    }
}
