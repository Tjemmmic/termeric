use crate::shell::MAX_COMMAND_TEXT_BYTES;
use crate::term::*;

const PROMPT_START: &[u8] = b"\x1b]133;A\x07";
const COMMAND_START: &[u8] = b"\x1b]133;B\x07";
const OUTPUT_START: &[u8] = b"\x1b]133;C\x07";
const FINISHED: &[u8] = b"\x1b]133;D;0\x07";

/// Show `prompt`, mark the command's start, and type `command`, as a shell
/// with OSC 133 integration does.
fn type_at_prompt(terminal: &mut Terminal, prompt: &[u8], command: &[u8]) {
    terminal.process(PROMPT_START);
    terminal.process(prompt);
    terminal.process(COMMAND_START);
    terminal.process(command);
}

/// Press Enter, print `output`, and finish with status 0.
fn run(terminal: &mut Terminal, output: &[u8]) {
    terminal.process(b"\r\n");
    terminal.process(OUTPUT_START);
    terminal.process(output);
    terminal.process(FINISHED);
}

fn last_command(terminal: &Terminal) -> (String, bool) {
    let tracker = terminal.prompt_tracker();
    let region = tracker
        .completed_region(tracker.completed_count() - 1)
        .expect("a completed command");
    (region.command_text.clone(), region.command_truncated)
}

#[test]
fn command_text_leaves_out_the_prompt_on_its_row() {
    let mut terminal = Terminal::new(40, 6);
    for _ in 0..3 {
        type_at_prompt(&mut terminal, b"[termeric]$ ", b"echo hello");
        run(&mut terminal, b"hello\r\n");
        assert_eq!(last_command(&terminal), ("echo hello".into(), false));
    }
}

#[test]
fn a_soft_wrapped_command_reads_as_one_line() {
    let mut terminal = Terminal::new(10, 6);
    type_at_prompt(&mut terminal, b"$ ", b"echo abcdefghijkl");
    run(&mut terminal, b"abcdefghijkl\r\n");
    assert_eq!(last_command(&terminal), ("echo abcdefghijkl".into(), false));
}

#[test]
fn a_prompt_that_fills_its_row_is_left_out() {
    let mut terminal = Terminal::new(10, 6);
    // The prompt ends on the last column, so the cursor waits to wrap.
    type_at_prompt(&mut terminal, b"0123456789", b"ls");
    run(&mut terminal, b"file\r\n");
    assert_eq!(last_command(&terminal), ("ls".into(), false));
}

#[test]
fn the_command_start_follows_a_reflow_while_typing() {
    for columns in [8, 30] {
        let mut terminal = Terminal::new(20, 6);
        type_at_prompt(&mut terminal, b"user@host:~/src$ ", b"ls -la");
        terminal.resize(columns, 6);
        run(&mut terminal, b"total 0\r\n");
        assert_eq!(
            last_command(&terminal),
            ("ls -la".into(), false),
            "after resizing to {columns} columns"
        );
    }
}

#[test]
fn command_text_is_bounded() {
    let mut terminal = Terminal::new(80, 6);
    let command = "x".repeat(MAX_COMMAND_TEXT_BYTES + 500);
    type_at_prompt(&mut terminal, b"$ ", command.as_bytes());
    run(&mut terminal, b"");
    let (text, truncated) = last_command(&terminal);
    assert!(truncated);
    assert_eq!(text, "x".repeat(MAX_COMMAND_TEXT_BYTES));
}

#[test]
fn a_command_marked_before_its_prompt_starts_where_typing_began() {
    // Elvish and Fish 3 mark the command's start before drawing the prompt.
    let mut terminal = Terminal::new(40, 6);
    for _ in 0..2 {
        terminal.process(PROMPT_START);
        terminal.process(COMMAND_START);
        terminal.process(b"~/roots> ");
        terminal.note_user_input();
        terminal.process(b"ls -l");
        run(&mut terminal, b"file\r\n");
        assert_eq!(last_command(&terminal), ("ls -l".into(), false));
    }

    // A shell that marks it after its prompt is left as it marked it, even
    // when typing comes later in the line.
    type_at_prompt(&mut terminal, b"$ ", b"echo ");
    terminal.note_user_input();
    terminal.process(b"hi");
    run(&mut terminal, b"hi\r\n");
    assert_eq!(last_command(&terminal), ("echo hi".into(), false));
}
