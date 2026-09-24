//! Recognizing an agent from the process in the foreground of a terminal.

/// Script interpreters whose first script argument names the real program.
const INTERPRETERS: &[&str] = &["node", "nodejs", "bun", "deno", "python", "python3"];

/// The program name an argument vector was invoked as: the first word's file
/// name, or for an interpreter its script's file name without extension.
pub fn program_name(argv: &[String]) -> Option<String> {
    let first = file_stem(argv.first()?, false)?;
    if !INTERPRETERS.contains(&first.as_str()) {
        return Some(first);
    }
    // Skip interpreter options such as `--no-warnings` to reach the script.
    let script = argv[1..].iter().find(|arg| !arg.starts_with('-'))?;
    file_stem(script, true)
}

fn file_stem(arg: &str, strip_extension: bool) -> Option<String> {
    let name = std::path::Path::new(arg).file_name()?.to_str()?;
    let name = if strip_extension {
        name.strip_suffix(".js")
            .or_else(|| name.strip_suffix(".mjs"))
            .or_else(|| name.strip_suffix(".cjs"))
            .or_else(|| name.strip_suffix(".ts"))
            .or_else(|| name.strip_suffix(".py"))
            .unwrap_or(name)
    } else {
        name
    };
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
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn foreground_argv_reads_this_process() {
        let argv = foreground_argv(std::process::id() as i32).unwrap();
        assert!(!argv.is_empty());
        assert_eq!(foreground_argv(0), None);
    }
}
