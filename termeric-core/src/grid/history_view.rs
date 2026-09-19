use super::*;

fn populated(limit: usize) -> Grid {
    let mut grid = Grid::new(8, 4);
    grid.set_scrollback_limit(limit);
    for character in 'A'..='F' {
        grid.cell_mut(0, 0).unwrap().set_char(character);
        grid.scroll_up();
    }
    for (row, character) in ('G'..='J').enumerate() {
        grid.cell_mut(row, 0).unwrap().set_char(character);
    }
    grid
}

fn viewed_text(grid: &Grid) -> String {
    grid.cell_scrolled(0, 0).unwrap().content.to_text()
}

#[test]
fn history_appends_preserve_viewed_identity_until_actual_eviction() {
    for limit in [6, 20] {
        for offset in [0, 1, 3, 6] {
            for operation in ["full", "region", "archive"] {
                let mut grid = populated(limit);
                grid.set_scroll_offset(offset);
                let anchor = viewed_text(&grid);
                match operation {
                    "full" => {
                        grid.scroll_up();
                    }
                    "region" => {
                        grid.scroll_region_up(0, 3);
                    }
                    "archive" => {
                        grid.archive_visible();
                    }
                    _ => unreachable!(),
                }
                if offset == 0 {
                    assert_eq!(grid.scroll_offset(), 0);
                } else {
                    // Locate the actual retained row by its unique content;
                    // do not duplicate the offset-adjustment implementation.
                    let history: Vec<_> =
                        grid.scrollback_rows().map(|row| row.to_string()).collect();
                    let position = history.iter().position(|row| row == &anchor).unwrap_or(0);
                    assert_eq!(
                        grid.scroll_offset(),
                        history.len() - position,
                        "limit={limit} offset={offset} operation={operation}"
                    );
                    assert_eq!(viewed_text(&grid), history[position]);
                }
                if operation == "region" {
                    assert_eq!(grid.cell(3, 0).unwrap().content.to_text(), "J");
                }
            }
        }
    }
}

#[test]
fn history_view_tracks_appends_that_evict_multiple_rows_by_bytes() {
    for region in [false, true] {
        let mut grid = populated(20);
        let budget = grid.memory_usage().scrollback_bytes;
        grid.set_scrollback_byte_limit(budget);
        // One large grapheme replaces several small archival rows, without
        // making byte-charge/layout assumptions about CompactRow's internals.
        let text = format!("G{}", "\u{301}".repeat(budget / 4));
        grid.cell_mut(0, 0).unwrap().set_grapheme(&text);
        grid.set_scroll_offset(1);
        assert_eq!(viewed_text(&grid), "F");
        let evicted = if region {
            grid.scroll_region_up(0, 3)
        } else {
            grid.scroll_up()
        };
        assert!(evicted >= 2);
        assert!(grid.scrollback_len() >= 2);
        assert_eq!(grid.scroll_offset(), 2);
        assert_eq!(viewed_text(&grid), "F");
        assert!(grid.memory_usage().scrollback_bytes <= budget);
    }
}

#[test]
fn nonarchiving_or_invalid_regions_do_not_move_the_history_view() {
    for (top, bottom) in [(1, 3), (0, 0), (0, 5), (3, 2)] {
        let mut grid = populated(20);
        grid.set_scroll_offset(3);
        let anchor = viewed_text(&grid);
        grid.scroll_region_up(top, bottom);
        assert_eq!(grid.scroll_offset(), 3);
        assert_eq!(viewed_text(&grid), anchor);
        assert_eq!(grid.scrollback_len(), 6);
    }
}

#[test]
fn total_byte_eviction_retires_the_history_view_without_an_invalid_offset() {
    for region in [false, true] {
        let mut grid = populated(20);
        let budget = grid.memory_usage().scrollback_bytes;
        grid.set_scrollback_byte_limit(budget);
        grid.set_scroll_offset(3);
        grid.cell_mut(0, 0)
            .unwrap()
            .set_grapheme(&format!("G{}", "\u{301}".repeat(budget)));
        if region {
            grid.scroll_region_up(0, 3);
        } else {
            grid.scroll_up();
        }
        assert_eq!(grid.scrollback_len(), 0);
        assert_eq!(grid.scroll_offset(), 0);
        assert_eq!(viewed_text(&grid), "H");
    }
}

#[test]
fn terminal_region_output_keeps_a_scrolled_back_view_and_fixed_footer() {
    use crate::Terminal;
    for sequence in [b"\x1b[S".as_slice(), b"\x1b[M", b"\x1b[3;1H\n"] {
        let mut terminal = Terminal::new(8, 4);
        terminal.grid_mut().set_scrollback_limit(6);
        terminal.process(b"A\r\nB\r\nC\r\nD\r\nE\r\nF\r\nG\r\nH\r\nI\r\nJ");
        terminal.process(b"\x1b[1;3r");
        terminal.grid_mut().set_scroll_offset(2);
        let anchor = viewed_text(terminal.grid());
        for _ in 0..3 {
            terminal.process(sequence);
            assert_eq!(viewed_text(terminal.grid()), anchor);
            assert_eq!(terminal.grid().cell(3, 0).unwrap().content.to_text(), "J");
        }
        assert_eq!(terminal.grid().scroll_offset(), 5);
    }
}
