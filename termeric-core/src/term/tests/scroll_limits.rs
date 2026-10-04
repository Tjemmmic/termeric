use crate::term::*;

fn fixture(alternate: bool, margins: usize, cursor_row: usize) -> Terminal {
    let mut terminal = Terminal::new(12, 6);
    terminal.process(b"retained\r\nretained\r\nretained\r\nretained\r\nretained\r\nretained\r\n");
    if alternate {
        terminal.enter_alt_screen();
    }
    for row in 0..6 {
        terminal.process(format!("\x1b[{};1Hrow{row}\u{754c}", row + 1).as_bytes());
    }
    terminal.process(b"\x1b[3;4H\x1b]66;s=2;X\x07");
    terminal.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=1,c=1,r=1,C=1;AQIDBA==");
    match margins {
        1 => terminal.process(b"\x1b[1;4r"),
        2 => terminal.process(b"\x1b[2;5r"),
        3 => terminal.process(b"\x1b[2;5r\x1b[?69h\x1b[4;10s"),
        _ => {}
    }
    terminal.process(b"\x1b[44m");
    terminal.grid_mut().set_cursor(cursor_row, 4);
    terminal.set_selection(Some(Selection::new(
        Point::new(0, 0),
        SelectionType::Simple,
    )));
    terminal.drain_events();
    terminal
}

fn operate(terminal: &mut Terminal, operation: u8, count: usize) {
    match operation {
        b'S' => terminal.scroll_up(count),
        b'T' => terminal.scroll_down(count),
        b'L' => terminal.insert_lines(count),
        b'M' => terminal.delete_lines(count),
        _ => unreachable!(),
    }
}

fn effective_count(terminal: &Terminal, operation: u8, count: usize) -> usize {
    let first_row = if matches!(operation, b'L' | b'M') {
        let cursor = terminal.grid().cursor();
        if !terminal.row_inside_vertical_margins(cursor.row)
            || !terminal.col_inside_horizontal_margins(cursor.col)
        {
            return 0;
        }
        cursor.row
    } else {
        terminal.scroll_top
    };
    count.min(terminal.scroll_bottom - first_row)
}

fn assert_equivalent(actual: &Terminal, expected: &Terminal) {
    assert_eq!(
        actual.grid().scrollback_len(),
        expected.grid().scrollback_len()
    );
    assert_eq!(actual.grid().styled_lines(), expected.grid().styled_lines());
    for row in 0..6 {
        let a = actual.grid().row(row).unwrap();
        let b = expected.grid().row(row).unwrap();
        assert!(a.iter().eq(b.iter()), "row {row}");
        assert_eq!(a.is_wrapped(), b.is_wrapped());
    }
    let a = actual.grid().cursor();
    let b = expected.grid().cursor();
    assert_eq!(
        (a.row, a.col, a.wrap_pending),
        (b.row, b.col, b.wrap_pending)
    );
    let selection = |term: &Terminal| term.selection().map(|s| (s.start, s.end, s.ty, s.active));
    assert_eq!(selection(actual), selection(expected));
    assert_eq!(actual.get_selection_text(), expected.get_selection_text());
    assert_eq!(
        format!("{:?}", actual.graphics_placements().collect::<Vec<_>>()),
        format!("{:?}", expected.graphics_placements().collect::<Vec<_>>())
    );
    // The revision counts actual row-movement operations, avoiding a flaky
    // wall-clock timeout as evidence that large counts have bounded work.
    assert_eq!(
        actual.grid().row_mapping_revision().1,
        expected.grid().row_mapping_revision().1
    );
}

#[test]
fn oversized_scroll_up_preserves_real_history_without_synthetic_blank_rows() {
    let mut terminal = Terminal::new(8, 3);
    terminal.process(b"one\r\ntwo\r\nthree\x1b[65535S");
    assert_eq!(terminal.grid().scrollback_len(), 3);
    for (row, text) in [(-3, "one"), (-2, "two"), (-1, "three")] {
        assert_eq!(
            terminal.grid().row_at_offset(row).unwrap().to_string(),
            text
        );
    }
}

#[test]
fn oversized_line_operations_match_effective_regions_and_bounded_work() {
    for alternate in [false, true] {
        for margins in 0..4 {
            for cursor_row in [0, 2, 5] {
                for operation in *b"STLM" {
                    for count in [0, 1, 7, 65_535, usize::MAX] {
                        let mut actual = fixture(alternate, margins, cursor_row);
                        let mut expected = fixture(alternate, margins, cursor_row);
                        let effective = effective_count(&expected, operation, count);
                        operate(&mut expected, operation, effective);
                        let before = actual.grid().row_mapping_revision().1;
                        operate(&mut actual, operation, count);
                        assert_eq!(
                            actual.grid().row_mapping_revision().1 - before,
                            effective as u64
                        );
                        assert_equivalent(&actual, &expected);
                    }
                }
            }
        }
    }
}

#[test]
fn parser_line_counts_default_saturate_and_survive_fragmentation() {
    for operation in *b"STLM" {
        for (parameter, count) in [
            ("", 1),
            ("0", 1),
            ("1", 1),
            ("65535", 65_535),
            ("999999999999999999999999", 65_535),
        ] {
            let bytes = format!("\x1b[{parameter}{}", operation as char);
            for split in 0..=bytes.len() {
                let mut actual = fixture(false, 0, 0);
                let mut expected = fixture(false, 0, 0);
                let effective = effective_count(&expected, operation, count);
                operate(&mut expected, operation, effective);
                actual.process(&bytes.as_bytes()[..split]);
                actual.process(&bytes.as_bytes()[split..]);
                assert_equivalent(&actual, &expected);
            }
        }
    }
}
