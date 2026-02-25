//! Splash screen display
//!
//! Shows ASCII art splash screen on terminal startup.

use termeric_core::Terminal;

/// Embedded ASCII art (compiled into binary)
const SPLASH_ASCII: &str = include_str!("../../../assets/termeric-ascii.txt");

/// Display the splash screen in the terminal
///
/// Writes the ASCII art directly to the terminal grid.
/// The art is trimmed of excess whitespace and centered both horizontally and vertically.
pub fn show_splash(terminal: &mut Terminal) {
    // Get terminal dimensions
    let term_cols = terminal.grid().cols();
    let term_rows = terminal.grid().rows();

    // Parse and trim the ASCII art
    let lines: Vec<&str> = SPLASH_ASCII
        .lines()
        .skip_while(|line| line.trim().is_empty()) // Skip leading empty lines
        .collect();

    // Find where actual content ends (trim trailing empty lines)
    let content_end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map(|i| i + 1)
        .unwrap_or(0);

    let lines = &lines[..content_end];

    // Calculate starting row to vertically center (if art fits)
    let art_height = lines.len();
    let start_row = if art_height < term_rows {
        (term_rows - art_height) / 2
    } else {
        0
    };

    // Find the maximum width of the art
    let art_width = lines.iter().map(|line| line.len()).max().unwrap_or(0);

    // Calculate starting column to horizontally center (if art fits)
    let start_col = if art_width < term_cols {
        (term_cols - art_width) / 2
    } else {
        0
    };

    // Write each line to the terminal
    for (i, line) in lines.iter().enumerate() {
        let row = start_row + i;
        if row >= term_rows {
            break; // Don't write beyond visible area
        }

        // Set cursor position and write the line
        terminal.set_cursor_position(row, start_col);

        for ch in line.chars() {
            // Check cursor position to avoid wrapping
            if terminal.grid().cursor().col >= term_cols {
                break;
            }
            terminal.write_char(ch);
        }
    }

    // Position cursor below the art for shell prompt
    let prompt_row = (start_row + art_height + 1).min(term_rows.saturating_sub(1));
    terminal.set_cursor_position(prompt_row, 0);
}
