#![no_main]

use libfuzzer_sys::fuzz_target;
use termeric_core::{CellFlags, Terminal, term::TerminalMode};

const MAX_INPUT_BYTES: usize = 64 * 1024;
const MAX_SCROLLBACK_LINES: usize = 512;
const MAX_SCROLLBACK_BYTES: usize = 64 * 1024;

fuzz_target!(|data: &[u8]| {
    let data = &data[..data.len().min(MAX_INPUT_BYTES)];
    if data.len() < 3 {
        return;
    }

    let cols = data[0] as usize % 160 + 1;
    let rows = data[1] as usize % 80 + 1;
    let chunk_size = data[2] as usize % 32 + 1;
    let mut terminal = Terminal::new(cols, rows);
    terminal.set_scrollback_limits(MAX_SCROLLBACK_LINES, MAX_SCROLLBACK_BYTES);

    for (index, chunk) in data[3..].chunks(chunk_size).enumerate() {
        terminal.process(chunk);
        if index % 11 == 0 && chunk.len() >= 2 {
            terminal.resize(chunk[0] as usize % 160 + 1, chunk[1] as usize % 80 + 1);
        }
        assert_invariants(&terminal);
    }

    assert_history_invariants(&terminal);
    let _ = terminal.drain_events();
});

fn assert_invariants(terminal: &Terminal) {
    let grid = terminal.grid();
    assert_eq!(grid.cols(), terminal.cols());
    assert_eq!(grid.rows(), terminal.rows());
    assert!(grid.scroll_offset() <= grid.scrollback_len());
    assert!(grid.scrollback_len() <= MAX_SCROLLBACK_LINES);
    let active_usage = grid.memory_usage();
    let expected_active_limits = if terminal.mode().contains(TerminalMode::ALT_SCREEN) {
        (0, 0)
    } else {
        (MAX_SCROLLBACK_LINES, MAX_SCROLLBACK_BYTES)
    };
    assert_eq!(
        (
            active_usage.scrollback_line_limit,
            active_usage.scrollback_byte_limit
        ),
        expected_active_limits
    );
    assert!(active_usage.scrollback_bytes <= MAX_SCROLLBACK_BYTES);
    let total_usage = terminal.memory_usage();
    assert_eq!(total_usage.scrollback_line_limit, MAX_SCROLLBACK_LINES);
    assert_eq!(total_usage.scrollback_byte_limit, MAX_SCROLLBACK_BYTES);
    assert!(total_usage.scrollback_lines <= MAX_SCROLLBACK_LINES);
    assert!(total_usage.scrollback_bytes <= MAX_SCROLLBACK_BYTES);
    let scroll_region = terminal.scroll_region();
    assert!(scroll_region.start < scroll_region.end);
    assert!(scroll_region.end <= terminal.rows());
    let horizontal_region = terminal.horizontal_scroll_region();
    assert!(horizontal_region.start < horizontal_region.end);
    assert!(horizontal_region.end <= terminal.cols());
    let cursor = grid.cursor();
    assert!(cursor.row < grid.rows());
    assert!(cursor.col < grid.cols());

    for row_index in 0..grid.rows() {
        let row = grid.row(row_index).unwrap();
        assert_eq!(row.len(), grid.cols());
        for col in 0..grid.cols() {
            let cell = row.get(col).unwrap();
            if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                assert!(col > 0);
                assert!(
                    row.get(col - 1)
                        .unwrap()
                        .flags
                        .contains(CellFlags::WIDE_CHAR)
                );
            }
            if cell.flags.contains(CellFlags::WIDE_CHAR) {
                assert!(col + 1 < grid.cols());
                assert!(
                    row.get(col + 1)
                        .unwrap()
                        .flags
                        .contains(CellFlags::WIDE_CHAR_SPACER)
                );
            }
        }
    }
}

fn assert_history_invariants(terminal: &Terminal) {
    let grid = terminal.grid();
    for history_index in 1..=grid.scrollback_len() {
        let row = grid
            .row_at_offset(-(history_index as i32))
            .expect("retained scrollback row must exist");
        assert_eq!(row.len(), grid.cols());
        for col in 0..grid.cols() {
            let cell = row.get(col).expect("history cell must exist");
            if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                assert!(col > 0);
                assert!(
                    row.get(col - 1)
                        .unwrap()
                        .flags
                        .contains(CellFlags::WIDE_CHAR)
                );
            }
            if cell.flags.contains(CellFlags::WIDE_CHAR) {
                assert!(col + 1 < grid.cols());
                assert!(
                    row.get(col + 1)
                        .unwrap()
                        .flags
                        .contains(CellFlags::WIDE_CHAR_SPACER)
                );
            }
        }
    }
}
