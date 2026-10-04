//! Complete terminal-workspace snapshots, and closed tabs kept as one-tab
//! workspaces. They preserve organization and relaunch metadata, but never
//! file descriptors or live processes.

mod model;
mod storage;

pub use model::*;
pub use storage::*;

/// Styled scrollback and screen rows to save with a terminal. Blank rows and
/// a final bare prompt, `exit`, or `logout` are dropped from the end, so the
/// restored shell's own prompt doesn't appear below a stale one.
pub fn restorable_scrollback(grid: &termeric_core::Grid) -> Vec<String> {
    let mut lines = grid.styled_lines();
    let first = -(grid.scrollback_len().min(i32::MAX as usize) as i32);
    let disposable = (first..grid.rows() as i32)
        .rev()
        .map_while(|offset| grid.row_at_offset(offset))
        .take_while(|row| ends_a_finished_session(row.plain_text(grid.cols()).trim()))
        .count();
    lines.truncate(lines.len().saturating_sub(disposable));
    lines
}

fn ends_a_finished_session(line: &str) -> bool {
    line.is_empty() || line == "logout" || line == "exit" || is_bare_prompt(line)
}

/// A short line ending in a common prompt character, like `$`, `[~]$`,
/// `user@host:~$`, `#`, `%`, or `>`; longer lines are likely output.
fn is_bare_prompt(line: &str) -> bool {
    line.len() <= 120 && line.ends_with(['$', '#', '%', '>'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_after(output: &[u8]) -> Vec<String> {
        let mut terminal = termeric_core::Terminal::new(20, 4);
        terminal.process(output);
        restorable_scrollback(terminal.grid())
    }

    #[test]
    fn restorable_scrollback_drops_a_trailing_prompt_exit_and_blank_rows() {
        let saved = lines_after(b"one\r\n\x1b[31mtwo\x1b[0m\r\nexit\r\nuser@host:~$ ");
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[0], "one");
        assert!(saved[1].contains("\x1b[") && saved[1].contains("two"));
    }

    #[test]
    fn restorable_scrollback_keeps_output_that_merely_ends_like_a_prompt() {
        let long = format!("{}$", "x".repeat(130));
        let mut terminal = termeric_core::Terminal::new(200, 3);
        terminal.process(format!("{long}\r\nlast line\r\n$ ").as_bytes());
        assert_eq!(
            restorable_scrollback(terminal.grid()),
            [long, "last line".into()]
        );
        assert!(lines_after(b"$ \r\n#").is_empty());
    }
}
