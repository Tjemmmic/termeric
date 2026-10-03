//! The ASCII-art splash written into a new terminal on startup.

use termeric_core::Terminal;

const SPLASH_ASCII: &str = include_str!("splash.txt");

/// Writes the splash art into the terminal grid, trimmed of blank leading
/// and trailing lines and centered both ways when it fits, then leaves the
/// cursor below it for the shell prompt.
pub fn show_splash(terminal: &mut Terminal) {
    let term_cols = terminal.grid().cols();
    let term_rows = terminal.grid().rows();

    let lines: Vec<&str> = SPLASH_ASCII
        .lines()
        .skip_while(|line| line.trim().is_empty())
        .collect();
    let content_end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map(|i| i + 1)
        .unwrap_or(0);
    let lines = &lines[..content_end];

    let art_height = lines.len();
    let start_row = if art_height < term_rows {
        (term_rows - art_height) / 2
    } else {
        0
    };

    // Char count, not byte length, so non-ASCII art still centers.
    let art_width = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let start_col = if art_width < term_cols {
        (term_cols - art_width) / 2
    } else {
        0
    };

    for (i, line) in lines.iter().enumerate() {
        let row = start_row + i;
        if row >= term_rows {
            break;
        }

        terminal.set_cursor_position(row, start_col);
        for ch in line.chars() {
            // Stop at the right edge instead of wrapping onto the next row.
            if terminal.grid().cursor().col >= term_cols {
                break;
            }
            terminal.write_char(ch);
        }
    }

    let prompt_row = (start_row + art_height + 1).min(term_rows.saturating_sub(1));
    terminal.set_cursor_position(prompt_row, 0);
}
