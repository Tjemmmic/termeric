use super::*;

fn cell_without_dirty(mut cell: Cell) -> Cell {
    cell.flags.remove(CellFlags::DIRTY);
    cell
}

#[test]
fn vertical_edits_fill_only_exposed_cells_with_the_active_background() {
    for alternate in [false, true] {
        for (top, bottom, left, right) in [(0, 5, 0, 8), (1, 4, 0, 8), (1, 4, 2, 6)] {
            for operation in *b"STLM" {
                for count in [1, 2, 65_535] {
                    for rendition in ["49", "44", "48;5;200", "48;2;1;2;3", "48;2;28;20;16"] {
                        let mut terminal = Terminal::new(8, 5);
                        if alternate {
                            terminal.enter_alt_screen();
                        }
                        terminal.process(
                            b"\x1b[31;43;3m\x1b[1\"q\x1b]8;;https://example.invalid\x1b\\",
                        );
                        for row in 0..5 {
                            terminal.process(format!("\x1b[{};1Hrow{row}xxxx", row + 1).as_bytes());
                        }
                        terminal.process(b"\x1b]8;;\x1b\\");
                        terminal.process(format!("\x1b[{};{}r", top + 1, bottom).as_bytes());
                        if left != 0 {
                            terminal.process(
                                format!("\x1b[?69h\x1b[{};{}s", left + 1, right).as_bytes(),
                            );
                        }
                        let cursor_row = top + 1;
                        terminal.grid_mut().set_cursor(cursor_row, left);
                        // A blank must not inherit these live text attributes,
                        // protection, or hyperlink, even when inverse is active.
                        terminal.process(
                            format!(
                                "\x1b[1;4;7;32;{rendition}m\x1b]8;;https://active.invalid\x1b\\"
                            )
                            .as_bytes(),
                        );
                        let blank = Cell {
                            bg: terminal.current_bg,
                            bg_spec: terminal.current_bg_spec,
                            ..Cell::default()
                        };
                        let original: Vec<Vec<Cell>> = (0..5)
                            .map(|row| {
                                terminal
                                    .grid()
                                    .row(row)
                                    .unwrap()
                                    .iter()
                                    .cloned()
                                    .map(cell_without_dirty)
                                    .collect()
                            })
                            .collect();
                        let first = if matches!(operation, b'L' | b'M') {
                            cursor_row
                        } else {
                            top
                        };
                        let effective = count.min(bottom - first);
                        let down = matches!(operation, b'T' | b'L');
                        terminal.process(format!("\x1b[{count}{}", operation as char).as_bytes());
                        for (row, original_row) in original.iter().enumerate() {
                            for (col, original_cell) in original_row.iter().enumerate() {
                                let expected = if (first..bottom).contains(&row)
                                    && (left..right).contains(&col)
                                {
                                    let source = if down {
                                        row.checked_sub(effective).filter(|&r| r >= first)
                                    } else {
                                        (row + effective < bottom).then_some(row + effective)
                                    };
                                    source.map_or(&blank, |r| &original[r][col])
                                } else {
                                    original_cell
                                };
                                assert_eq!(
                                    &cell_without_dirty(
                                        terminal.grid().cell(row, col).unwrap().clone()
                                    ),
                                    expected,
                                    "alternate={alternate} region={top}..{bottom}/{left}..{right} operation={} count={count} rendition={rendition} row={row} col={col}",
                                    operation as char
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn implicit_scrolling_uses_bce_for_linefeed_index_reverse_index_and_wrap() {
    for alternate in [false, true] {
        for (cursor, operation, exposed, first_blank) in [
            (b"\x1b[3;1H".as_slice(), b"\n".as_slice(), 2, 0),
            (b"\x1b[3;1H", b"\x1bD", 2, 0),
            (b"\x1b[3;2H", b"\x1bE", 2, 0),
            (b"\x1b[1;1H", b"\x1bM", 0, 0),
            (b"\x1b[3;4H", b"XY", 2, 1),
        ] {
            let mut terminal = Terminal::new(4, 3);
            if alternate {
                terminal.enter_alt_screen();
            }
            terminal.process(b"AAAA\r\nBBBB\r\nCCCC\x1b[1;4;7;48;2;1;2;3m");
            terminal.process(cursor);
            terminal.process(operation);
            for col in first_blank..4 {
                assert_eq!(
                    cell_without_dirty(terminal.grid().cell(exposed, col).unwrap().clone()),
                    Cell {
                        bg: Color::rgb(1, 2, 3),
                        bg_spec: ColorSpec::TrueColor(Color::rgb(1, 2, 3)),
                        ..Cell::default()
                    },
                    "operation={operation:?} alternate={alternate} col={col}"
                );
            }
        }
    }
}

#[test]
fn scrolled_backgrounds_survive_history_and_respect_synchronized_presentation() {
    let mut terminal = Terminal::new(4, 3);
    terminal.process(b"AAAA\r\nBBBB\r\nCCCC\x1b[44m\x1b[?2026h\x1b[S");
    assert_eq!(
        terminal.presentation_grid().row(2).unwrap().to_string(),
        "CCCC"
    );
    for col in 0..4 {
        assert_eq!(
            terminal.grid().cell(2, col).unwrap().bg_spec,
            ColorSpec::Indexed(4)
        );
        assert_eq!(
            terminal.presentation_grid().cell(2, col).unwrap().bg_spec,
            ColorSpec::Default
        );
    }
    terminal.process(b"\x1b[?2026l");
    assert_eq!(
        terminal.presentation_grid().cell(2, 0).unwrap().bg_spec,
        ColorSpec::Indexed(4)
    );
    terminal.process(b"\x1b[45m\x1b[3S");
    assert_eq!(terminal.grid().scrollback_len(), 4);
    let archived = terminal.grid().row_at_offset(-1).unwrap();
    for col in 0..4 {
        assert_eq!(archived.get(col).unwrap().bg_spec, ColorSpec::Indexed(4));
        assert_eq!(
            terminal.grid().cell(2, col).unwrap().bg_spec,
            ColorSpec::Indexed(5)
        );
    }
    terminal.process(b"\x1b[49m\x1b[T");
    for col in 0..4 {
        assert_eq!(
            cell_without_dirty(terminal.grid().cell(0, col).unwrap().clone()),
            Cell::default()
        );
    }
}
