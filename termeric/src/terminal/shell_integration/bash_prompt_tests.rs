use super::*;
use std::time::{Duration, Instant};
use termeric_core::Pty;
use termeric_core::pty::{PtyEvent, spawn_reader_thread};

fn probe(prompt_command: &str, integrated: bool) -> Option<(String, Vec<u8>)> {
    let program = super::tests::executable_on_path("bash")?;
    // Indexed PROMPT_COMMAND semantics were added in Bash 5.1. Do not compare
    // the array fixture against a shell that never supported that contract.
    let modern = std::process::Command::new(&program)
        .args([
            "--noprofile",
            "--norc",
            "-c",
            "(( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1) ))",
        ])
        .env_clear()
        .status()
        .ok()?;
    if !modern.success() {
        return None;
    }
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let rc = format!(
        r#"PS1='TEST> '
PS0='USER_PRECOMMAND $(printf expanded) '
HISTFILE=/dev/null
first_hook() {{
    local saved=$?
    printf 'first:%s\n' "$saved" >> "$HOME/hooks"
    return 17
}}
second_hook() {{
    local saved=$?
    printf 'second:%s\n' "$saved" >> "$HOME/hooks"
    return 23
}}
{prompt_command}
"#
    );
    std::fs::write(home.join(".bashrc"), &rc).unwrap();
    let config = ShellConfig {
        program: Some(program.clone()),
        args: vec!["--noprofile".into(), "-i".into()],
        env: Default::default(),
    };
    let args = if integrated {
        prepare_shell_launch_in(&config, &temp.path().join("generated"))
            .unwrap()
            .args
    } else {
        vec![
            "--noprofile".into(),
            "--rcfile".into(),
            home.join(".bashrc").display().to_string(),
            "-i".into(),
        ]
    };
    let pty = Pty::new_with_env(
        100,
        30,
        Some(&program),
        &args,
        &[("HOME".into(), home.display().to_string())],
    )
    .unwrap();
    let (mut handle, reader) = pty.take_reader();
    let receive = spawn_reader_thread(reader);
    handle.write_all(b"true\rfalse\rexit\r").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut bytes = Vec::new();
    let mut closed = false;
    while Instant::now() < deadline {
        match receive.recv_timeout(Duration::from_millis(100)) {
            Ok(PtyEvent::Output(chunk)) => bytes.extend(chunk),
            Ok(PtyEvent::Closed) => {
                closed = true;
                break;
            }
            Ok(PtyEvent::Error(error)) => panic!("Bash prompt PTY: {error}"),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    assert!(
        closed || !handle.is_running(),
        "Bash prompt fixture timed out"
    );
    assert_eq!(std::fs::read_to_string(home.join(".bashrc")).unwrap(), rc);
    Some((std::fs::read_to_string(home.join("hooks")).unwrap(), bytes))
}

fn assert_prompt_preserved(command: &str) {
    let Some((baseline_hooks, baseline_bytes)) = probe(command, false) else {
        eprintln!("skipping Bash prompt composition: Bash 5.1+ unavailable");
        return;
    };
    let (hooks, bytes) = probe(command, true).unwrap();
    // Ignore the startup status: preparing the integration runs commands after
    // the user's rc. All actual command statuses and hook ordering must match.
    assert_eq!(
        hooks.lines().skip(2).collect::<Vec<_>>(),
        baseline_hooks.lines().skip(2).collect::<Vec<_>>()
    );
    assert_eq!(
        hooks.lines().count(),
        6,
        "each hook must run once per prompt"
    );
    let output = String::from_utf8_lossy(&bytes);
    assert_eq!(
        String::from_utf8_lossy(&baseline_bytes)
            .matches("USER_PRECOMMAND expanded ")
            .count(),
        3,
        "the native baseline must exercise PS0 for each submitted command"
    );
    assert_eq!(
        output.matches("USER_PRECOMMAND expanded ").count(),
        String::from_utf8_lossy(&baseline_bytes)
            .matches("USER_PRECOMMAND expanded ")
            .count()
    );
    let mut terminal = termeric_core::Terminal::new(100, 30);
    terminal.process(&bytes);
    let mut harvest = crate::harvest::HarvestTerminalState::default();
    let regions = harvest.observe(&terminal.shell_integration().prompt_tracker, true);
    assert_eq!(
        regions
            .iter()
            .filter_map(|region| region.exit_code)
            .collect::<Vec<_>>(),
        vec![0, 1],
        "user hooks must not replace the command exit status"
    );
}

#[test]
fn bash_scalar_prompt_hooks_preserve_status_and_precommand_prompt() {
    assert_prompt_preserved("PROMPT_COMMAND='first_hook; second_hook # trailing comment'");
}

#[test]
fn bash_array_prompt_hooks_preserve_order_status_and_precommand_prompt() {
    assert_prompt_preserved("PROMPT_COMMAND=('first_hook # keep comment' 'second_hook;')");
}

#[test]
fn bash_sparse_prompt_hooks_preserve_order_status_and_precommand_prompt() {
    assert_prompt_preserved("PROMPT_COMMAND=([2]='first_hook' [9]='second_hook')");
}
