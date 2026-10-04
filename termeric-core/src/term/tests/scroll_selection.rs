use crate::term::*;

fn populated(limit: usize) -> Terminal {
    let mut terminal = Terminal::new(8, 4);
    terminal.grid_mut().set_scrollback_limit(limit);
    terminal.process(b"A\r\nB\r\nC\r\nD\r\nE\r\nF\r\nG\r\nH\r\nI\r\nJ\x1b[1;3r");
    terminal
}

#[test]
fn top_origin_archival_preserves_selection_and_navigation_identity() {
    for limit in [20, 6] {
        for command in [b"\x1b[S".as_slice(), b"\x1b[M", b"\x1b[3;1H\n"] {
            for kind in [
                SelectionType::Simple,
                SelectionType::Semantic,
                SelectionType::Lines,
                SelectionType::Block,
            ] {
                for first in -6..=3 {
                    for last in first..=3 {
                        for reversed in [false, true] {
                            let mut terminal = populated(limit);
                            let mut start = Point::new(first, 0);
                            let mut end = Point::new(last, 1);
                            if reversed {
                                std::mem::swap(&mut start, &mut end);
                            }
                            let mut selection = Selection::new(start, kind);
                            selection.update(end);
                            selection.finish();
                            terminal.set_selection(Some(selection));
                            terminal.set_navigation_cursor(Some(start));
                            let before = terminal.get_selection_text();
                            terminal.process(command);
                            assert_eq!(terminal.hyperlink_gc_evictions, usize::from(limit == 6));
                            let oldest = -(terminal.grid().scrollback_len() as i32);
                            let moved = |mut point: Point| {
                                if point.row < 3 {
                                    point.row -= 1;
                                }
                                (point.row >= oldest).then_some(point)
                            };
                            let mapped_start = moved(start);
                            let mapped_end = moved(end);
                            assert_eq!(
                                terminal.navigation_cursor(),
                                mapped_start,
                                "limit={limit} command={command:?} start={start:?}"
                            );
                            if first < 3 && last >= 3
                                || mapped_start.is_none()
                                || mapped_end.is_none()
                            {
                                assert!(
                                    terminal.selection().is_none(),
                                    "mixed/evicted range survived: {first}..{last}"
                                );
                            } else {
                                let selection = terminal
                                    .selection()
                                    .expect("all selected cells survived together");
                                assert_eq!(Some(selection.start), mapped_start);
                                assert_eq!(Some(selection.end), mapped_end);
                                assert_eq!(selection.ty, kind);
                                assert!(!selection.active);
                                assert_eq!(terminal.get_selection_text(), before);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn regional_selection_archival_honors_zero_and_byte_history_limits() {
    for byte_limit in [0, 1] {
        for command in [b"\x1b[S".as_slice(), b"\x1b[M"] {
            let mut terminal = populated(20);
            terminal.set_scrollback_limits(20, byte_limit);
            terminal.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
            terminal.set_navigation_cursor(Some(Point::new(0, 0)));
            terminal.process(command);
            assert_eq!(terminal.grid().scrollback_len(), 0);
            assert!(terminal.selection().is_none());
            assert!(terminal.navigation_cursor().is_none());
        }
    }
    let mut terminal = populated(20);
    let budget = terminal.grid().memory_usage().scrollback_bytes;
    terminal.set_scrollback_limits(20, budget);
    terminal
        .grid_mut()
        .cell_mut(0, 0)
        .unwrap()
        .set_grapheme(&format!("G{}", "\u{301}".repeat(budget / 4)));
    let mut selection = Selection::new(Point::new(-6, 0), SelectionType::Simple);
    selection.update(Point::new(-1, 0));
    terminal.set_selection(Some(selection));
    terminal.set_navigation_cursor(Some(Point::new(-1, 0)));
    terminal.process(b"\x1b[S");
    assert!(terminal.grid().scrollback_len() < 6);
    assert!(terminal.selection().is_none());
    assert_eq!(terminal.navigation_cursor(), Some(Point::new(-2, 0)));
    assert_eq!(terminal.grid().row_at_offset(-2).unwrap().to_string(), "F");
}

#[test]
fn nonarchiving_regions_do_not_preserve_discarded_top_row_anchors() {
    for alternate in [false, true] {
        for horizontal in [false, true] {
            for command in [b"\x1b[S".as_slice(), b"\x1b[M"] {
                let mut terminal = populated(20);
                if alternate {
                    terminal.enter_alt_screen();
                }
                terminal.process(b"\x1b[1;1Hrowzero\x1b[2;1Hrowone\x1b[1;3r");
                if horizontal {
                    terminal.process(b"\x1b[?69h\x1b[2;7s");
                } else if !alternate {
                    terminal.process(b"\x1b[2;3r");
                }
                let top = terminal.scroll_top as i32;
                // DECSLRM homes outside its left margin without DECOM. DL is
                // correctly ignored there, so explicitly place it in-region.
                terminal.grid_mut().set_cursor(top as usize, 2);
                terminal.set_selection(Some(Selection::new(
                    Point::new(top, 2),
                    SelectionType::Block,
                )));
                terminal.set_navigation_cursor(Some(Point::new(top, 2)));
                let history = terminal.grid().scrollback_len();
                terminal.process(command);
                assert!(
                    terminal.selection().is_none(),
                    "alternate={alternate} horizontal={horizontal} command={command:?}"
                );
                assert!(terminal.navigation_cursor().is_none());
                assert_eq!(terminal.grid().scrollback_len(), history);
            }
        }
    }
}

#[test]
fn archival_selection_keeps_frozen_presentation_and_full_height_delete_identity() {
    let mut terminal = populated(20);
    let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
    selection.update(Point::new(0, 1));
    terminal.set_selection(Some(selection));
    terminal.set_navigation_cursor(Some(Point::new(0, 0)));
    let text = terminal.get_selection_text();
    terminal.process(b"\x1b[?2026h\x1b[S");
    assert_eq!(terminal.selection().unwrap().start, Point::new(0, 0));
    assert_eq!(terminal.get_selection_text(), text);
    assert_eq!(
        terminal.selection.as_ref().unwrap().start,
        Point::new(-1, 0)
    );
    assert_eq!(terminal.navigation_cursor(), Some(Point::new(-1, 0)));
    terminal.process(b"\x1b[?2026l\x1b[r\x1b[M");
    assert_eq!(terminal.selection().unwrap().start, Point::new(-2, 0));
    assert_eq!(terminal.navigation_cursor(), Some(Point::new(-2, 0)));
    assert_eq!(terminal.get_selection_text(), text);
}
