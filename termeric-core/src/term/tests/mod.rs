mod graphics_history;
mod graphics_relay_geometry;
mod graphics_resize;
pub(super) mod reflow_prompts;
mod scroll_bce;
mod scroll_graphics;
mod scroll_limits;
mod scroll_prompts;
mod scroll_selection;

use super::keyboard::{MAX_USER_DEFINED_KEYS_BYTES, XTERM_USER_DEFINED_KEY_COUNT};
use super::*;

#[test]
fn navigation_anchor_shares_exact_live_selection_moves_and_damage() {
    let commands: &[&[u8]] = &[
        b"\r\nnext",
        b"\x1b[2S",
        b"\x1b[2T",
        b"\x1b[2;1H\x1b[L",
        b"\x1b[2;1H\x1b[M",
        b"\x1b[1;3H\x1b[2@",
        b"\x1b[1;3H\x1b[2P",
        b"\x1b[Hxx",
        b"\x1b[H\x1b[2X",
        b"\x1b[2K",
        b"\x1b[2J",
        b"\x1b[?2J",
        b"\x1b[1;2;2;5$z",
        b"\x1b[65;1;2;2;5$x",
        b"\x1b[1;1;1;4;1;2;2;1$v",
        b"\x1b[?69h\x1b[2;6s\x1b[2;4r\x1b[S",
        b"\x1b[?69h\x1b[2;6s\x1b[2;4r\x1b[T",
        b"\x1b[H\x1b]66;s=2;X\x07",
    ];
    for command in commands {
        for row in -1..4 {
            for col in 0..8 {
                let mut term = Terminal::new(8, 4);
                term.process(
                    "history\r\nab界Zdef\r\n\x1b[1\"qprotected\x1b[0\"q\r\nlast".as_bytes(),
                );
                let point = Point::new(row, col);
                if term.grid().row_at_offset(row).is_none() {
                    continue;
                }
                term.set_selection(Some(Selection::new(point, SelectionType::Simple)));
                term.set_navigation_cursor(Some(point));
                term.process(command);
                assert_eq!(
                    term.navigation_cursor(),
                    term.selection().map(|selection| selection.start),
                    "command={command:?} point={point:?}"
                );
            }
        }
    }
    let mut term = Terminal::new(8, 3);
    term.process(b"first\r\nsecond\r\nthird\r\nlast");
    term.set_navigation_cursor(Some(Point::new(-1, 0)));
    assert!(
        term.selection().is_none(),
        "navigation must never become a copy selection"
    );
    term.set_scrollback_limits(0, 0);
    assert!(term.navigation_cursor().is_none());
    for command in [b"\x1bc".as_slice(), b"\x1b[?1049h", b"\x1b[?1049l"] {
        term.set_navigation_cursor(Some(Point::new(0, 0)));
        term.process(command);
        assert!(term.navigation_cursor().is_none());
    }
}

fn assert_wide_invariants(term: &Terminal) {
    for row in 0..term.rows() {
        for col in 0..term.cols() {
            let cell = term.grid().cell(row, col).unwrap();
            if cell.is_wide_char() {
                assert!(col + 1 < term.cols(), "wide leader at final column");
                assert!(
                    term.grid()
                        .cell(row, col + 1)
                        .unwrap()
                        .is_wide_char_spacer(),
                    "wide leader at {row}:{col} has no spacer"
                );
            }
            if cell.is_wide_char_spacer() {
                assert!(col > 0, "wide spacer at column zero");
                assert!(
                    term.grid().cell(row, col - 1).unwrap().is_wide_char(),
                    "wide spacer at {row}:{col} has no leader"
                );
                assert!(cell.content.is_empty());
            }
        }
    }
}

#[test]
fn test_terminal_creation() {
    let term = Terminal::new(80, 24);
    assert_eq!(term.cols(), 80);
    assert_eq!(term.rows(), 24);
}

#[test]
fn multicell_placement_selection_matches_actual_cell_damage() {
    let commands: &[&[u8]] = &[
        b"\x1b[2;3H\x1b]66;s=2:w=2;X\x07",
        "\x1b[2;3H\x1b]66;s=2;A界\x07".as_bytes(),
        b"\x1b[4h\x1b[2;3H\x1b]66;s=2:w=1;X\x07",
        b"\x1b[2;3H\x1b]66;s=2:w=2:n=1:d=2:v=1:h=2;X\x07",
        b"\x1b]66;s=0;invalid\x07",
        b"\x1b]66;s=7:w=7;oversized\x07",
        b"\x1b[H\x1b]66;s=1:w=1;a\x07", // Same scalar, new descriptor.
    ];
    let points = [
        Point::new(0, 0),
        Point::new(1, 2),
        Point::new(1, 8),
        Point::new(2, 1),
        Point::new(3, 9),
    ];
    for &command in commands {
        for ty in [
            SelectionType::Simple,
            SelectionType::Semantic,
            SelectionType::Lines,
            SelectionType::Block,
        ] {
            for start in points {
                for end in points {
                    let mut term = Terminal::new(10, 4);
                    term.process(b"abcdefghij\r\nklmnopqrst\r\nuvwxyz0123\r\n456789ABCD");
                    let original: Vec<_> = (0..4)
                        .map(|row| term.grid().row(row).unwrap().clone())
                        .collect();
                    let mut selection = Selection::new(start, ty);
                    selection.update(end);
                    term.set_selection(Some(selection.clone()));
                    term.process(command);
                    let damaged = (0..4).any(|row| {
                        (0..10).any(|col| {
                            let before = original[row].get(col).unwrap();
                            let after = term.grid().cell(row, col).unwrap();
                            selection.contains(row as i32, col)
                                && (before.content != after.content
                                    || before.multicell != after.multicell)
                        })
                    });
                    assert_eq!(
                        term.selection().is_none(),
                        damaged,
                        "command={command:?} ty={ty:?} start={start:?} end={end:?}"
                    );
                    if !damaged {
                        let actual = term.selection().unwrap();
                        assert_eq!(
                            (actual.start, actual.end, actual.ty, actual.active),
                            (
                                selection.start,
                                selection.end,
                                selection.ty,
                                selection.active
                            )
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn multicell_placement_repairs_wide_halves_and_invalidates_complete_collisions() {
    for target in [2, 3] {
        let mut term = Terminal::new(12, 4);
        term.process("..\x1b[1\"q界\x1b[0\"q".as_bytes());
        let selected = if target == 2 { 3 } else { 2 };
        term.set_selection(Some(Selection::new(
            Point::new(0, selected),
            SelectionType::Block,
        )));
        term.grid_mut().set_cursor(0, target);
        term.process(b"\x1b]66;s=1:w=1;X\x07");
        assert!(term.selection().is_none());
        let orphan = term.grid().cell(0, selected).unwrap();
        assert!(orphan.content.is_empty());
        assert!(!orphan.is_wide_char() && !orphan.is_wide_char_spacer());
        assert_eq!(term.grid().cell(0, target).unwrap().content.to_text(), "X");
        assert!(term.grid().multicell_region_at(0, target).is_some());
    }
    for target in [0, 2] {
        for selected in [Point::new(0, 0), Point::new(1, 3)] {
            let mut term = Terminal::new(12, 4);
            term.process(b"\x1b]66;s=2:w=2;old\x07");
            term.set_selection(Some(Selection::new(selected, SelectionType::Block)));
            term.grid_mut().set_cursor(0, target);
            term.process(b"\x1b]66;s=1:w=1;X\x07");
            assert!(term.selection().is_none());
            assert!(term.grid().multicell_region_at(1, 3).is_none());
            assert!(term.grid().multicell_region_at(0, target).is_some());
        }
    }
}

#[test]
fn multicell_placement_skips_selected_lower_rows_but_insert_mode_erases_whole_blocks() {
    let mut skipped = Terminal::new(12, 4);
    skipped.process(b"\x1b]66;s=2:w=2;old\x07\x1b[2;1H");
    skipped.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
    skipped.process(b"\x1b]66;s=1:w=1;X\x07");
    assert_eq!(skipped.get_selection_text().as_deref(), Some("old"));
    assert_eq!(skipped.grid().cell(1, 4).unwrap().content.to_text(), "X");

    let mut shifted = Terminal::new(12, 4);
    shifted.process(b"......\x1b]66;s=2:w=2;old\x07\x1b[2;1H\x1b[4h");
    shifted.set_selection(Some(Selection::new(Point::new(0, 6), SelectionType::Block)));
    shifted.process(b"\x1b]66;s=1:w=1;X\x07");
    assert!(shifted.selection().is_none()); // Old leader is outside the inserted row/span.
    assert!(shifted.grid().multicell_region_at(0, 6).is_none());
}

#[test]
fn multicell_placement_wraps_selection_into_history_and_isolates_frozen_alternate_screen() {
    let mut term = Terminal::new(12, 3);
    term.process(b"anchor");
    let mut selection = Selection::new(Point::new(0, 5), SelectionType::Simple);
    selection.update(Point::new(0, 0));
    term.set_selection(Some(selection.clone()));
    term.process(b"\x1b[3;11H\x1b]66;s=2:w=2;X\x07");
    assert_eq!(term.selection().unwrap().start, Point::new(-2, 5));
    assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
    term.process(b"\x1b[?1049hanchor\x1b[H");
    term.set_selection(Some(selection));
    term.process(b"\x1b[?2026h\x1b]66;s=2:w=2;X\x07");
    assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
    term.process(b"\x1b[?2026l");
    assert!(term.selection().is_none());
}

#[test]
fn vertical_commands_clear_selection_for_multicell_boundary_and_final_repair_damage() {
    let commands: &[&[u8]] = &[
        b"\x1b[S",
        b"\x1b[T",
        b"\x1b[L",
        b"\x1b[M",
        b"\x1b[6;1H\n",
        b"\x1bM",
    ];
    for &command in commands {
        for final_repair in [false, true] {
            let mut term = Terminal::new(12, 6);
            let selected = if final_repair {
                // Only the lower half lies in the moved region. Neither
                // early crossing-row scan sees the orphaned upper half.
                term.process(b"\x1b[3;1H\x1b]66;s=2;X\x07\x1b[4;6r\x1b[4;1H");
                Point::new(2, 0)
            } else {
                term.process(b"\x1b]66;s=3;X\x07\x1b[2;6r\x1b[2;1H");
                Point::new(0, 0)
            };
            term.set_selection(Some(Selection::new(selected, SelectionType::Block)));
            term.process(command);
            assert!(
                term.selection().is_none(),
                "command={command:?} final_repair={final_repair}"
            );
            assert!(
                term.grid()
                    .cell(selected.row as usize, 0)
                    .unwrap()
                    .content
                    .is_empty()
            );
        }
    }
}

#[test]
fn partial_width_vertical_edits_report_wide_damage_after_selection_motion() {
    let commands: &[&[u8]] = &[
        b"\x1b[S",
        b"\x1b[T",
        b"\x1b[L",
        b"\x1b[M",
        b"\x1b[5;4H\n",
        b"\x1bM",
    ];
    for &command in commands {
        for row in 0..5 {
            for leader in [2, 7] {
                // Straddle either horizontal margin.
                for selected_col in [leader, leader + 1] {
                    let mut term = Terminal::new(12, 5);
                    term.grid_mut().set_cursor(row, leader);
                    term.process("\x1b[1\"q界\x1b[0\"q".as_bytes());
                    term.process(b"\x1b[?69h\x1b[4;8s\x1b[1;4H");
                    term.set_selection(Some(Selection::new(
                        Point::new(row as i32, selected_col),
                        SelectionType::Block,
                    )));
                    term.process(command);
                    assert!(
                        term.selection().is_none(),
                        "command={command:?} row={row} col={selected_col}"
                    );
                    assert!((0..5).all(|r| (0..12).all(|c| {
                        let cell = term.grid().cell(r, c).unwrap();
                        !cell.is_wide_char() && !cell.is_wide_char_spacer()
                    })));
                }
            }
        }
    }
}

#[test]
fn vertical_glyph_damage_preserves_survivors_history_and_frozen_selection() {
    for down in [false, true] {
        let mut term = Terminal::new(12, 6);
        term.process(b"\x1b[3;4H\x1b]66;s=2;X\x07");
        term.set_selection(Some(Selection::new(Point::new(2, 3), SelectionType::Block)));
        if down {
            term.scroll_down(1);
        } else {
            term.scroll_up(1);
        }
        assert_eq!(
            term.selection().unwrap().start.row,
            if down { 3 } else { 1 }
        );
        assert_eq!(term.get_selection_text().as_deref(), Some("X"));

        let mut wide = Terminal::new(12, 5);
        wide.process("\x1b[3;5H界\x1b[?69h\x1b[4;8s\x1b[1;4H".as_bytes());
        wide.set_selection(Some(Selection::new(Point::new(2, 4), SelectionType::Block)));
        if down {
            wide.scroll_down(1);
        } else {
            wide.scroll_up(1);
        }
        assert_eq!(
            wide.selection().unwrap().start.row,
            if down { 3 } else { 1 }
        );
        assert_eq!(wide.get_selection_text().as_deref(), Some("界"));
    }

    let mut history = Terminal::new(12, 6);
    history.process(b"anchor\x1b[5;1H\x1b]66;s=2;X\x07");
    let mut selection = Selection::new(Point::new(0, 5), SelectionType::Simple);
    selection.update(Point::new(0, 0));
    history.set_selection(Some(selection));
    history.scroll_up(1);
    assert_eq!(history.selection().unwrap().start.row, -1);
    assert_eq!(history.get_selection_text().as_deref(), Some("anchor"));

    let mut frozen = Terminal::new(12, 5);
    frozen.process(b"\x1b[?1049h\x1b]66;s=3;X\x07\x1b[2;5r");
    frozen.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
    frozen.process(b"\x1b[?2026h\x1b[S");
    assert_eq!(frozen.get_selection_text().as_deref(), Some("X"));
    frozen.process(b"\x1b[?2026l");
    assert!(frozen.selection().is_none());
}

#[test]
fn partial_width_multicell_erasure_reports_outside_damage_but_zero_counts_do_nothing() {
    for operation in 0..4 {
        let mut term = Terminal::new(12, 5);
        term.process(b"\x1b]66;s=2:w=2;X\x07\x1b[?69h\x1b[3;8s\x1b[1;3H");
        term.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
        for count in [0, 1] {
            match operation {
                0 => term.scroll_up(count),
                1 => term.scroll_down(count),
                2 => term.insert_lines(count),
                _ => term.delete_lines(count),
            }
            if count == 0 {
                assert_eq!(term.get_selection_text().as_deref(), Some("X"));
            } else {
                assert!(term.selection().is_none());
                assert!(term.grid().cell(0, 0).unwrap().content.is_empty());
            }
        }
    }
}

#[test]
fn vertical_boundary_erasure_retires_selection_outside_the_scrolled_rows() {
    let mut term = Terminal::new(12, 5);
    term.process(b"\x1b]66;s=3:w=1;X\x07\x1b[2;5r");
    term.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
    term.process(b"\x1b[S");
    assert!(term.grid().cell(0, 0).unwrap().content.is_empty());
    assert!(term.selection().is_none());
}

#[test]
fn new_multicell_placement_retires_overwritten_selection() {
    let mut term = Terminal::new(12, 4);
    term.process(b"anchor\x1b[H");
    let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
    selection.update(Point::new(0, 5));
    term.set_selection(Some(selection));
    term.process(b"\x1b]66;s=2:w=2;X\x07");
    assert!(term.selection().is_none());
}

#[test]
fn osc66_places_atomic_scaled_blocks_and_advances_by_their_width() {
    let mut term = Terminal::new(12, 5);
    term.process(b"\x1b]66;s=2:w=3:n=1:d=2:v=1:h=2;hello\x07");

    let region = term
        .grid()
        .multicell_region_at(1, 5)
        .expect("covered cell resolves to its block");
    assert_eq!((region.top, region.left), (0, 0));
    assert_eq!(region.descriptor.columns(), 6);
    assert_eq!(region.descriptor.rows(), 2);
    assert_eq!(region.descriptor.font_scale(), 1.0);
    assert_eq!(
        region.descriptor.vertical_alignment(),
        crate::MultiCellAlignment::End
    );
    assert_eq!(
        region.descriptor.horizontal_alignment(),
        crate::MultiCellAlignment::Center
    );
    assert_eq!(term.grid().cell(0, 0).unwrap().content.to_text(), "hello");
    assert!(term.grid().cell(0, 1).unwrap().content.is_empty());
    assert_eq!(term.grid().cursor().col, 6);
}

#[test]
fn osc66_natural_width_segments_graphemes_into_separate_blocks() {
    let mut term = Terminal::new(12, 5);
    assert!(Osc66Command::parse(&[b"s=2", "A界".as_bytes()]).is_some());
    term.process("\x1b]66;s=2;A界\x07".as_bytes());

    let ascii = term.grid().multicell_region_at(0, 1).unwrap();
    let wide = term.grid().multicell_region_at(1, 5).unwrap();
    assert_eq!((ascii.left, ascii.descriptor.columns()), (0, 2));
    assert_eq!((wide.left, wide.descriptor.columns()), (2, 4));
    assert_eq!(term.grid().cell(0, 2).unwrap().content.to_text(), "界");
    assert_eq!(term.grid().cursor().col, 6);
}

#[test]
fn normal_text_obeys_multicell_overwrite_and_lower_row_skip_rules() {
    let mut term = Terminal::new(10, 4);
    term.process(b"\x1b]66;s=2:w=2;A\x07");
    term.grid_mut().set_cursor(1, 0);
    term.write_char('x');
    assert!(term.grid().multicell_region_at(0, 0).is_some());
    assert_eq!(term.grid().cell(1, 4).unwrap().content.to_text(), "x");

    term.grid_mut().set_cursor(0, 1);
    term.write_char('y');
    assert!(term.grid().multicell_region_at(0, 0).is_none());
    assert_eq!(term.grid().cell(0, 0).unwrap().content.to_text(), " ");
    assert_eq!(term.grid().cell(0, 1).unwrap().content.to_text(), "y");
    assert_eq!(term.grid().cell(1, 0).unwrap().content.to_text(), " ");
}

#[test]
fn combining_scalar_extends_multicell_leader_without_splitting_block() {
    let mut term = Terminal::new(10, 4);
    term.process(b"\x1b]66;s=2;A\x07");
    term.process("\u{301}".as_bytes());

    assert_eq!(
        term.grid().cell(0, 0).unwrap().content.to_text(),
        "A\u{301}"
    );
    assert!(term.grid().multicell_region_at(1, 1).is_some());
    assert_eq!(term.grid().cursor().col, 2);
}

#[test]
fn osc66_discards_blocks_larger_than_the_screen() {
    let mut term = Terminal::new(4, 2);
    term.process(b"\x1b]66;s=3:w=2;x\x07");
    assert!(term.grid().cell(0, 0).unwrap().content.is_empty());
    assert_eq!(term.grid().cursor().col, 0);
}

#[test]
fn osc66_wrap_and_no_wrap_respect_horizontal_margins() {
    let mut wrapped = Terminal::new(12, 4);
    wrapped.set_left_right_margin_mode(true);
    wrapped.set_left_right_margins(2, 10);
    wrapped.grid_mut().set_cursor(0, 8);
    wrapped.process(b"\x1b]66;w=3;x\x07");
    let region = wrapped.grid().multicell_region_at(1, 1).unwrap();
    assert_eq!((region.top, region.left), (1, 1));

    let mut no_wrap = Terminal::new(12, 4);
    no_wrap.set_left_right_margin_mode(true);
    no_wrap.set_left_right_margins(2, 10);
    no_wrap.process(b"\x1b[?7l");
    no_wrap.grid_mut().set_cursor(0, 8);
    no_wrap.process(b"\x1b]66;w=3;x\x07");
    let region = no_wrap.grid().multicell_region_at(0, 8).unwrap();
    assert_eq!((region.top, region.left), (0, 7));
}

#[test]
fn osc66_horizontal_edits_preserve_whole_single_row_blocks_and_erase_multiline_blocks() {
    let mut one_row = Terminal::new(10, 4);
    one_row.process(b"..\x1b]66;s=1:w=2;X\x07");
    one_row.grid_mut().set_cursor(0, 0);
    one_row.insert_chars(1);
    let shifted = one_row.grid().multicell_region_at(0, 4).unwrap();
    assert_eq!((shifted.top, shifted.left), (0, 3));
    one_row.grid_mut().set_cursor(0, 4);
    one_row.erase_chars(1);
    assert!(one_row.grid().multicell_region_at(0, 3).is_none());

    let mut multiline = Terminal::new(10, 4);
    multiline.process(b"..\x1b]66;s=2;X\x07");
    multiline.grid_mut().set_cursor(1, 0);
    multiline.delete_chars(1);
    assert!(multiline.grid().multicell_region_at(0, 2).is_none());
}

#[test]
fn osc66_resize_retains_fitting_blocks_and_discards_clipped_blocks() {
    let mut term = Terminal::new(8, 4);
    term.process(b".\x1b]66;s=2:w=2;wide\x07");
    term.resize(10, 5);
    assert!(term.grid().multicell_region_at(1, 4).is_some());

    term.resize(3, 5);
    assert!(
        term.grid()
            .iter_cells()
            .all(|(_, _, cell)| !cell.multicell.is_present())
    );
}

#[test]
fn osc66_ansi_export_replays_descriptor_and_text_once() {
    let mut source = Terminal::new(10, 3);
    source.process(b"\x1b]66;s=1:w=3:n=1:d=2:h=1;abc\x07!");
    let line = source.grid().styled_lines().remove(0);
    assert!(line.contains("\x1b]66;s=1:w=3:n=1:d=2:h=1;abc\x07"));
    assert_eq!(line.matches("abc").count(), 1);

    let mut restored = Terminal::new(10, 3);
    restored.process(line.as_bytes());
    let region = restored.grid().multicell_region_at(0, 2).unwrap();
    assert_eq!((region.left, region.descriptor.columns()), (0, 3));
    assert_eq!(restored.grid().cell(0, 3).unwrap().content.to_text(), "!");
}

#[test]
fn osc66_selection_emits_leader_text_once_without_covered_spaces() {
    let mut term = Terminal::new(10, 3);
    term.process(b"\x1b]66;s=2:w=2;title\x07!");
    let mut selection = Selection::new(Point::new(0, 1), SelectionType::Simple);
    selection.update(Point::new(1, 4));
    term.set_selection(Some(selection));
    assert_eq!(term.get_selection_text().as_deref(), Some("title!"));
}

#[test]
fn osc66_scroll_moves_complete_blocks_and_clears_only_split_multiline_blocks() {
    let mut term = Terminal::new(8, 4);
    term.grid_mut().set_cursor(1, 1);
    term.process(b"\x1b]66;s=2:w=2;move\x07");
    term.scroll_up(1);
    assert!(term.grid().multicell_region_at(1, 4).is_some());
    assert_eq!(term.grid().cell(0, 1).unwrap().content.to_text(), "move");

    term.scroll_up(1);
    assert!(
        term.grid()
            .iter_cells()
            .all(|(_, _, cell)| !cell.multicell.is_present())
    );

    let mut history = Terminal::new(8, 2);
    history.process(b"\x1b]66;w=2;kept\x07");
    history.scroll_up(1);
    let archived = history.grid().row_at_offset(-1).unwrap();
    assert!(archived.get(0).unwrap().multicell.is_leader());
    assert_eq!(archived.get(0).unwrap().content.to_text(), "kept");
}

#[test]
fn osc66_ed22_archives_complete_blocks_before_erasing_the_viewport() {
    let mut term = Terminal::new(8, 3);
    term.process(b"\x1b]66;s=2:w=2;kept\x07!");
    term.erase_in_display(22);

    assert_eq!(term.grid().scrollback_len(), 3);
    assert!(
        term.grid()
            .iter_cells()
            .all(|(_, _, cell)| !cell.multicell.is_present())
    );
    let top = term.grid().row_at_offset(-3).unwrap();
    let lower = term.grid().row_at_offset(-2).unwrap();
    assert!(top.get(0).unwrap().multicell.is_leader());
    assert_eq!(top.get(0).unwrap().content.to_text(), "kept");
    assert!(lower.get(3).unwrap().multicell.is_present());
    assert_eq!(top.get(4).unwrap().content.to_text(), "!");
}

#[test]
fn osc66_screen_switches_and_synchronized_output_keep_blocks_isolated() {
    let mut screens = Terminal::new(8, 3);
    screens.process(b"\x1b]66;s=2:w=2;primary\x07");
    screens.enter_alt_screen();
    assert!(
        screens
            .grid()
            .iter_cells()
            .all(|(_, _, cell)| !cell.multicell.is_present())
    );
    screens.process(b"\x1b]66;w=2;alternate\x07");
    assert!(screens.grid().multicell_region_at(0, 1).is_some());
    screens.exit_alt_screen();
    assert!(screens.grid().multicell_region_at(1, 3).is_some());
    assert_eq!(
        screens.grid().cell(0, 0).unwrap().content.to_text(),
        "primary"
    );

    let mut synchronized = Terminal::new(8, 3);
    synchronized.process(b"\x1b[?2026h\x1b]66;s=2:w=2;pending\x07");
    assert!(
        synchronized
            .presentation_grid()
            .iter_cells()
            .all(|(_, _, cell)| !cell.multicell.is_present())
    );
    synchronized.process(b"\x1b[?2026l");
    assert!(
        synchronized
            .presentation_grid()
            .multicell_region_at(1, 3)
            .is_some()
    );
}

#[test]
fn osc66_selective_erase_preserves_protected_blocks_atomically() {
    let mut term = Terminal::new(8, 3);
    term.process(b"\x1b[1\"q\x1b]66;s=2:w=2;safe\x07");
    term.grid_mut().set_cursor(0, 1);
    term.selective_erase_in_line(0);
    assert!(term.grid().multicell_region_at(1, 3).is_some());

    term.process(b"\x1b[0\"q");
    term.erase_in_line(2);
    assert!(term.grid().multicell_region_at(0, 0).is_none());
}

#[test]
fn fragmented_utf8_osc66_remains_streaming_safe() {
    let mut term = Terminal::new(10, 4);
    for byte in "\x1b]66;s=2:w=2;界\x1b\\".as_bytes() {
        term.process(std::slice::from_ref(byte));
    }
    assert_eq!(term.grid().cell(0, 0).unwrap().content.to_text(), "界");
    assert!(term.grid().multicell_region_at(1, 3).is_some());
}

#[test]
fn bounded_row_extraction_never_splits_utf8_or_allocates_the_unread_suffix() {
    let mut term = Terminal::new(10, 3);
    term.process("界abc\r\nsecond".as_bytes());

    let (text, truncated) = term.bounded_rows_text(0, 1, 5);
    assert_eq!(text, "界ab");
    assert!(truncated);
    assert!(text.len() <= 5);

    let (text, truncated) = term.bounded_rows_text(0, 1, 64);
    assert_eq!(text, "界abc\nsecond");
    assert!(!truncated);

    let (text, truncated) = term.bounded_rows_text(-100, 1, 64);
    assert_eq!(text, "界abc\nsecond");
    assert!(truncated);
}

#[test]
fn test_write_char() {
    let mut term = Terminal::new(80, 24);
    term.write_char('A');
    let cursor = term.grid().cursor();
    assert_eq!(cursor.col, 1);

    let cell = term.grid().cell(0, 0).unwrap();
    assert_eq!(cell.content.char(), Some('A'));
}

#[test]
fn unicode_clusters_use_terminal_width_and_one_leader() {
    for (text, width) in [
        ("界", 2),
        ("e\u{301}", 1),
        ("·", 1),
        ("☺\u{fe0e}", 1),
        ("☺\u{fe0f}", 2),
        ("🇺🇸", 2),
        ("👍🏽", 2),
        ("👩\u{200d}💻", 2),
    ] {
        let mut term = Terminal::new(20, 3);
        term.process(text.as_bytes());
        assert_eq!(term.grid().cursor().col, width, "{text:?}");
        assert_eq!(term.grid().cell(0, 0).unwrap().content.to_text(), text);
        assert_eq!(term.grid().cell(0, 0).unwrap().width(), width);
        assert_wide_invariants(&term);
    }
}

#[test]
fn orphan_combining_mark_composes_over_a_blank_cell() {
    let mut term = Terminal::new(4, 2);
    term.process("\u{301}".as_bytes());
    assert_eq!(term.grid().cursor().col, 1);
    assert_eq!(
        term.grid().cell(0, 0).unwrap().content.to_text(),
        " \u{301}"
    );
}

#[test]
fn wide_clusters_wrap_before_the_final_column() {
    let mut term = Terminal::new(4, 3);
    term.process("abc界".as_bytes());
    assert!(term.grid().row(0).unwrap().is_wrapped());
    assert_eq!(term.grid().cell(1, 0).unwrap().content.to_text(), "界");
    assert_eq!(term.grid().cursor().col, 2);
    assert_wide_invariants(&term);

    let mut presentation_change = Terminal::new(4, 3);
    presentation_change.process("abc☺\u{fe0f}".as_bytes());
    assert!(presentation_change.grid().row(0).unwrap().is_wrapped());
    assert_eq!(
        presentation_change
            .grid()
            .cell(1, 0)
            .unwrap()
            .content
            .to_text(),
        "☺\u{fe0f}"
    );
    assert_wide_invariants(&presentation_change);
}

#[test]
fn cell_edits_and_reflow_never_orphan_wide_pairs() {
    let mut term = Terminal::new(12, 4);
    term.process("A界B👍🏽C👩\u{200d}💻D".as_bytes());
    assert_wide_invariants(&term);

    for col in 0..12 {
        term.grid_mut().set_cursor(0, col);
        term.insert_chars(1);
        assert_wide_invariants(&term);
        term.delete_chars(1);
        assert_wide_invariants(&term);
    }

    for cols in [7, 3, 1, 16, 5, 12] {
        term.resize(cols, 4);
        assert_wide_invariants(&term);
    }

    let mut one_column = Terminal::new(4, 4);
    one_column.process("界".as_bytes());
    one_column.resize(1, 4);
    assert_eq!(one_column.rows_text(0, 3), "界\n\n\n");
    one_column.resize(4, 4);
    assert!(one_column.rows_text(0, 3).contains('界'));
    assert_wide_invariants(&one_column);
}

#[test]
fn stream_selection_joins_soft_wraps_and_skips_synthetic_wide_padding() {
    for ty in [SelectionType::Simple, SelectionType::Semantic] {
        for reversed in [false, true] {
            for history in [false, true] {
                let mut term = Terminal::new(3, 2);
                term.process("ab界Z".as_bytes());
                let row = if history {
                    term.process(b"\r\nx");
                    -1
                } else {
                    0
                };
                let (start, end) = (Point::new(row, 0), Point::new(row + 1, 2));
                let (start, end) = if reversed { (end, start) } else { (start, end) };
                let mut selection = Selection::new(start, ty);
                selection.update(end);
                term.set_selection(Some(selection));
                assert_eq!(
                    term.get_selection_text().as_deref(),
                    Some("ab界Z"),
                    "type={ty:?} reversed={reversed} history={history}"
                );
            }
        }
    }
}

#[test]
fn stream_selection_preserves_hard_breaks_and_partial_bounds() {
    let mut term = Terminal::new(3, 4);
    term.process(b"abcdef\r\nghi");
    for (start, end, expected) in [
        (Point::new(0, 0), Point::new(2, 2), "abcdef\nghi"),
        (Point::new(0, 1), Point::new(1, 1), "bcde"),
        (Point::new(1, 1), Point::new(2, 1), "ef\ngh"),
    ] {
        let mut selection = Selection::new(start, SelectionType::Simple);
        selection.update(end);
        term.set_selection(Some(selection));
        assert_eq!(term.get_selection_text().as_deref(), Some(expected));
    }
}

#[test]
fn explicit_presentation_selection_preserves_unicode_and_physical_row_semantics() {
    let mut term = Terminal::new(3, 4);
    term.process("ab界Z\x1b[?2026h\x1b[?1049hsecret".as_bytes());
    for (ty, expected) in [
        (SelectionType::Simple, "ab界Z"),
        (SelectionType::Semantic, "ab界Z"),
        (SelectionType::Lines, "ab \n界Z"),
        (SelectionType::Block, "ab \n界Z"),
    ] {
        for reverse in [false, true] {
            let (start, end) = (Point::new(0, 0), Point::new(1, 2));
            let mut selection = Selection::new(if reverse { end } else { start }, ty);
            selection.update(if reverse { start } else { end });
            assert_eq!(term.presentation_selection_text(&selection), expected);
            assert!(term.selection().is_none());
        }
    }
}

#[test]
fn native_selection_reads_frozen_text_and_word_boundaries() {
    let mut term = Terminal::new(16, 3);
    term.process(b"visible word");
    let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
    selection.update(Point::new(0, 6));
    term.set_selection(Some(selection));
    term.process(b"\x1b[?2026h\x1b[H\x1b[2Khidden text");
    assert_eq!(term.get_selection_text().as_deref(), Some("visible"));
    assert_eq!(
        term.word_at(Point::new(0, 6)),
        (Point::new(0, 0), Point::new(0, 6))
    );
}

#[test]
fn projected_selection_skips_rows_and_joins_only_adjacent_soft_wraps() {
    let mut term = Terminal::new(3, 4);
    term.process("ab界Zqqq".as_bytes());
    for ty in [
        SelectionType::Simple,
        SelectionType::Semantic,
        SelectionType::Lines,
        SelectionType::Block,
    ] {
        for reversed in [false, true] {
            let (start, end) = (Point::new(0, 0), Point::new(2, 2));
            let mut selection = Selection::new(if reversed { end } else { start }, ty);
            selection.update(if reversed { start } else { end });
            let logical = matches!(ty, SelectionType::Simple | SelectionType::Semantic);
            assert_eq!(
                term.presentation_selection_text_in_rows(&selection, &[0, 2]),
                if logical { "ab\nqqq" } else { "ab \nqqq" }
            );
            assert_eq!(
                term.presentation_selection_text_in_rows(&selection, &[0, 1, 2]),
                if logical {
                    "ab界Zqqq"
                } else {
                    "ab \n界Z\nqqq"
                }
            );
            assert_eq!(
                term.presentation_selection_text_in_rows(&selection, &[]),
                ""
            );
        }
    }
}

#[test]
fn snapshot_selection_clips_scrollback_and_preserves_block_columns() {
    for ty in [
        SelectionType::Simple,
        SelectionType::Lines,
        SelectionType::Block,
    ] {
        for reversed in [false, true] {
            let mut term = Terminal::new(3, 3);
            term.process(b"abc\r\ndef\r\nghi\r\njkl\r\nmno");
            term.grid_mut().set_scroll_offset(1);
            let (start, end) = (Point::new(-2, 1), Point::new(2, 1));
            let mut selection = Selection::new(if reversed { end } else { start }, ty);
            selection.update(if reversed { start } else { end });
            term.set_selection(Some(selection));
            term.process(b"\x1b[?2026h\x1b[?1049hSECRET");
            assert_eq!(
                term.get_selection_text().as_deref(),
                Some(if ty == SelectionType::Block {
                    "e\nh\nk"
                } else {
                    "def\nghi\njkl"
                })
            );
            let selection = term.selection().unwrap();
            assert_eq!((selection.bounds().0.row, selection.bounds().1.row), (0, 2));
            assert!(!selection.active);
        }
    }
}

#[test]
fn frozen_selection_never_falls_back_to_hidden_selection_and_retires_on_resize() {
    let mut term = Terminal::new(8, 3);
    term.process(b"visible\x1b[?2026h\x1b[H\x1b[2Khidden");
    let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
    selection.update(Point::new(0, 6));
    term.set_selection(Some(selection.clone()));
    assert_eq!(term.get_selection_text().as_deref(), Some("visible"));
    assert!(
        term.selection.is_none(),
        "frozen coordinates must not survive on the live grid"
    );
    term.resize(8, 3);
    assert_eq!(term.get_selection_text().as_deref(), Some("visible"));
    term.resize(4, 6);
    assert!(term.selection().is_none());
    term.set_selection(Some(selection));
    term.process(b"\x1b[?2026l");
    assert!(term.get_selection_text().is_none());
}

#[test]
fn physical_selections_keep_rows_and_stream_selections_keep_edited_padding() {
    let mut term = Terminal::new(3, 3);
    term.process("ab界Z".as_bytes());
    for ty in [SelectionType::Block, SelectionType::Lines] {
        let mut selection = Selection::new(Point::new(0, 0), ty);
        selection.update(Point::new(1, 2));
        term.set_selection(Some(selection));
        assert_eq!(term.get_selection_text().as_deref(), Some("ab \n界Z"));
    }
    for styled in [false, true] {
        let mut term = Terminal::new(3, 3);
        term.process("ab界Z".as_bytes());
        let cell = term.grid_mut().cell_mut(0, 2).unwrap();
        if styled {
            cell.bg_spec = ColorSpec::Indexed(4);
        } else {
            cell.set_char(' ');
        }
        let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
        selection.update(Point::new(1, 2));
        term.set_selection(Some(selection));
        assert_eq!(term.get_selection_text().as_deref(), Some("ab 界Z"));
    }
}

#[test]
fn stream_selection_reflow_round_trips_unicode_and_endpoint_direction() {
    for ty in [SelectionType::Simple, SelectionType::Semantic] {
        for reverse in [false, true] {
            for spacer_start in [false, true] {
                let mut term = Terminal::new(5, 6);
                term.process("ab界Zxy".as_bytes());
                let mut start = Point::new(0, if spacer_start { 3 } else { 0 });
                let mut end = Point::new(1, 1);
                if reverse {
                    std::mem::swap(&mut start, &mut end);
                }
                let mut selection = Selection::new(start, ty);
                selection.update(end);
                selection.finish();
                term.set_selection(Some(selection));
                let text = term.get_selection_text();
                for cols in [3, 1, 7, 2, 5] {
                    term.resize(cols, 20);
                    assert_eq!(
                        term.get_selection_text(),
                        text,
                        "type={ty:?} reverse={reverse} spacer={spacer_start} cols={cols}"
                    );
                    let selection = term.selection().unwrap();
                    assert_eq!(selection.ty, ty);
                    assert!(!selection.active);
                    assert_eq!(selection.start > selection.end, reverse);
                }
            }
        }
    }
}

#[test]
fn reflow_selection_crosses_compact_history_viewport_boundary_without_aliasing() {
    for reverse in [false, true] {
        let mut term = Terminal::new(5, 2);
        term.process("prefix\r\nab界ZxyzQrst".as_bytes());
        let locate = |term: &Terminal, character| {
            (-(term.grid().scrollback_len() as i32)..term.rows() as i32)
                .find_map(|row| {
                    let cells = term.grid().row_at_offset(row).unwrap();
                    (0..cells.len())
                        .find(|&col| {
                            cells
                                .get(col)
                                .is_some_and(|cell| cell.content.char() == Some(character))
                        })
                        .map(|col| Point::new(row, col))
                })
                .unwrap()
        };
        let mut start = locate(&term, 'a');
        let mut end = locate(&term, 't');
        if reverse {
            std::mem::swap(&mut start, &mut end);
        }
        let mut selection = Selection::new(start, SelectionType::Simple);
        selection.update(end);
        term.set_selection(Some(selection));
        for cols in [3, 1, 9, 4, 5] {
            term.resize(cols, 2);
            assert_eq!(
                term.get_selection_text().as_deref(),
                Some("ab界ZxyzQrst"),
                "cols={cols} reverse={reverse}"
            );
            assert_eq!(
                term.selection().unwrap().start > term.selection().unwrap().end,
                reverse
            );
        }
    }
}

#[test]
fn reflow_selection_matches_retained_character_identities_across_history_limits() {
    let locate = |term: &Terminal, character| {
        (-(term.grid().scrollback_len() as i32)..term.rows() as i32).find_map(|row| {
            let cells = term.grid().row_at_offset(row).unwrap();
            (0..cells.len())
                .find(|&col| {
                    cells
                        .get(col)
                        .is_some_and(|cell| cell.content.char() == Some(character))
                })
                .map(|col| Point::new(row, col))
        })
    };
    for limit in [0, 1, 8] {
        for cols in [1, 2, 4, 7, 12] {
            for rows in [1, 3, 8] {
                for (first, last) in [('A', 'B'), ('C', 'J'), ('K', 'N'), ('O', 'V'), ('S', 'V')] {
                    for reverse in [false, true] {
                        let mut term = Terminal::new(8, 3);
                        term.grid_mut().set_scrollback_limit(limit);
                        term.process(b"AB\r\nCDEFGHIJ\r\nKLMN\r\nOPQRSTUV");
                        let (Some(mut start), Some(mut end)) =
                            (locate(&term, first), locate(&term, last))
                        else {
                            continue;
                        };
                        if reverse {
                            std::mem::swap(&mut start, &mut end);
                        }
                        let mut selection = Selection::new(start, SelectionType::Simple);
                        selection.update(end);
                        term.set_selection(Some(selection));
                        let original = term.get_selection_text();
                        term.resize(cols, rows);
                        if let (Some(mut start), Some(mut end)) =
                            (locate(&term, first), locate(&term, last))
                        {
                            if reverse {
                                std::mem::swap(&mut start, &mut end);
                            }
                            let selection =
                                term.selection().expect("both selected endpoints survived");
                            assert_eq!(
                                (selection.start, selection.end),
                                (start, end),
                                "limit={limit} cols={cols} rows={rows}"
                            );
                            assert_eq!(term.get_selection_text(), original);
                        } else {
                            assert!(
                                term.selection().is_none(),
                                "lost endpoint must not alias retained text"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn width_reflow_retires_evicted_clipped_padding_and_geometric_selections() {
    for limit in [0, 1] {
        let mut term = Terminal::new(8, 3);
        term.grid_mut().set_scrollback_limit(limit);
        term.process(b"abcdefgh\r\nijklmnop\r\nqrstuvwx");
        let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
        selection.update(Point::new(0, 3));
        term.set_selection(Some(selection));
        term.resize(2, 2);
        assert!(term.selection().is_none());
    }
    let mut clipped = Terminal::new(8, 3);
    clipped.process(b"abcdefgh\r\nijklmnop\r\nqrstuvwx\x1b[H");
    clipped.set_selection(Some(Selection::new(
        Point::new(2, 1),
        SelectionType::Simple,
    )));
    clipped.resize(2, 2);
    assert!(clipped.selection().is_none());

    for ty in [SelectionType::Block, SelectionType::Lines] {
        let mut term = Terminal::new(8, 3);
        term.process(b"abcdefgh");
        let mut selection = Selection::new(Point::new(0, 0), ty);
        selection.update(Point::new(0, 3));
        term.set_selection(Some(selection));
        term.resize(8, 4);
        assert!(term.selection().is_some());
        term.resize(4, 4);
        assert!(term.selection().is_none());
    }
    let mut padding = Terminal::new(8, 3);
    padding.process(b"a");
    padding.set_selection(Some(Selection::new(
        Point::new(0, 6),
        SelectionType::Simple,
    )));
    padding.resize(4, 3);
    assert!(padding.selection().is_none());
}

#[test]
fn width_reflow_maps_live_stream_selection_but_retires_frozen_projection() {
    for alternate in [false, true] {
        let mut term = Terminal::new(5, 6);
        if alternate {
            term.process(b"\x1b[?1049h");
        }
        term.process("ab界Zxy".as_bytes());
        let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
        selection.update(Point::new(1, 1));
        term.set_selection(Some(selection));
        term.process(b"\x1b[?2026h");
        term.resize(3, 6);
        assert!(term.selection().is_none());
        term.process(b"\x1b[?2026l");
        assert_eq!(term.get_selection_text().as_deref(), Some("ab界Zxy"));
    }
}

#[test]
fn width_resize_maps_existing_stream_selection_through_wide_padding() {
    let mut term = Terminal::new(5, 4);
    term.process("ab界Z".as_bytes());
    let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
    selection.update(Point::new(0, 4));
    term.set_selection(Some(selection));
    term.resize(3, 4);
    assert_eq!(term.selection().unwrap().end, Point::new(1, 2));
    assert_eq!(term.get_selection_text().as_deref(), Some("ab界Z"));
}

#[test]
fn height_resize_selection_matches_retained_row_identity() {
    for limit in [0, 1, 4] {
        for cursor_row in 0..5 {
            for new_rows in 1..=7 {
                for ty in [
                    SelectionType::Simple,
                    SelectionType::Semantic,
                    SelectionType::Block,
                    SelectionType::Lines,
                ] {
                    for selected_row in -2..5 {
                        for multiline in [false, true] {
                            for reverse in [false, true] {
                                let mut term = Terminal::new(4, 5);
                                term.grid_mut().set_scrollback_limit(limit);
                                term.process(
                                    b"aaaa\r\nbbbb\r\ncccc\r\ndddd\r\neeee\r\nffff\r\ngggg",
                                );
                                let oldest = -(term.grid().scrollback_len() as i32);
                                if selected_row < oldest {
                                    continue;
                                }
                                let mut start = Point::new(selected_row, 1);
                                let mut end =
                                    Point::new(if multiline { 4 } else { selected_row }, 2);
                                if reverse {
                                    std::mem::swap(&mut start, &mut end);
                                }
                                let mut selection = Selection::new(start, ty);
                                selection.update(end);
                                selection.finish();
                                let original: Vec<_> = selection
                                    .row_range()
                                    .map(|row| {
                                        (row, term.grid().row_at_offset(row).unwrap().to_string())
                                    })
                                    .collect();
                                term.set_selection(Some(selection.clone()));
                                let text = term.get_selection_text();
                                term.grid_mut().set_cursor(cursor_row, 0);
                                term.resize(4, new_rows);
                                let new_oldest = -(term.grid().scrollback_len() as i32);
                                let mut uniform = None;
                                let mut survives = true;
                                for (old_row, text) in original {
                                    let mapped = (new_oldest..new_rows as i32).find(|&row| {
                                        term.grid().row_at_offset(row).unwrap().to_string() == text
                                    });
                                    if let Some(row) = mapped {
                                        let shift = row - old_row;
                                        if uniform.is_some_and(|prior| prior != shift) {
                                            survives = false;
                                        }
                                        uniform = Some(shift);
                                    } else {
                                        survives = false;
                                    }
                                }
                                assert_eq!(
                                    term.selection().is_some(),
                                    survives,
                                    "limit={limit} cursor={cursor_row} rows={new_rows} type={ty:?} {start:?}..{end:?}"
                                );
                                if survives {
                                    let shift = uniform.unwrap();
                                    let actual = term.selection().unwrap();
                                    assert_eq!(
                                        actual.start,
                                        Point::new(start.row + shift, start.col)
                                    );
                                    assert_eq!(actual.end, Point::new(end.row + shift, end.col));
                                    assert_eq!(actual.ty, ty);
                                    assert!(!actual.active);
                                    assert_eq!(term.get_selection_text(), text);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn height_resize_retires_clipped_multicells_and_isolates_frozen_selection() {
    for alternate in [false, true] {
        let mut term = Terminal::new(12, 5);
        if alternate {
            term.process(b"\x1b[?1049h");
        }
        term.process(b"\x1b]66;s=3;X\x07");
        term.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
        term.resize(12, 5);
        assert_eq!(term.get_selection_text().as_deref(), Some("X"));
        term.resize(12, 2);
        assert!(term.selection().is_none());
        assert!(term.grid().cell(0, 0).unwrap().content.is_empty());

        let mut frozen = Terminal::new(12, 4);
        if alternate {
            frozen.process(b"\x1b[?1049h");
        }
        frozen.process(b"anchor\r\nsecond\r\nthird\r\nlast");
        let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
        selection.update(Point::new(0, 5));
        frozen.set_selection(Some(selection));
        frozen.process(b"\x1b[?2026h");
        frozen.resize(12, 4);
        assert_eq!(frozen.get_selection_text().as_deref(), Some("anchor"));
        frozen.resize(12, 2);
        assert!(frozen.selection().is_none()); // Resized frozen projection is retired.
        frozen.process(b"\x1b[?2026l");
        if alternate {
            assert!(frozen.selection().is_none()); // Alternate grid has no history retention.
        } else {
            assert_eq!(frozen.get_selection_text().as_deref(), Some("anchor"));
            assert_eq!(frozen.selection().unwrap().start.row, -2);
        }
    }
}

#[test]
fn height_resize_moves_selection_with_archived_rows() {
    let mut term = Terminal::new(12, 4);
    term.process(b"anchor\r\nsecond\r\nthird\r\nlast");
    let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
    selection.update(Point::new(0, 5));
    term.set_selection(Some(selection));
    term.resize(12, 2);
    assert_eq!(term.selection().unwrap().start, Point::new(-2, 0));
    assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
}

#[test]
fn stream_selection_after_resize_keeps_logical_unicode_text() {
    let mut term = Terminal::new(5, 4);
    term.process("ab界Z".as_bytes());
    term.resize(3, 4);
    let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
    selection.update(Point::new(1, 2));
    term.set_selection(Some(selection));
    assert_eq!(term.get_selection_text().as_deref(), Some("ab界Z"));
    // Starting on a wide spacer still copies the complete grapheme once.
    let mut selection = Selection::new(Point::new(1, 1), SelectionType::Simple);
    selection.update(Point::new(1, 2));
    term.set_selection(Some(selection));
    assert_eq!(term.get_selection_text().as_deref(), Some("界Z"));
}

#[test]
fn wide_text_resize_round_trip_does_not_materialize_wrap_padding() {
    let mut term = Terminal::new(5, 4);
    term.process("ab界Z".as_bytes());
    term.process(b"\x1b[1;5H");
    for cols in [3, 5, 3, 5] {
        term.resize(cols, 4);
    }
    assert_eq!(term.grid().row(0).unwrap().to_string(), "ab界Z");
    let cursor = term.grid().cursor();
    assert_eq!((cursor.row, cursor.col), (0, 4));
}

#[test]
fn native_wide_wrap_padding_survives_history_and_visible_snapshots() {
    let mut term = Terminal::new(3, 3);
    term.process("ab界Z".as_bytes());
    assert!(term.grid().row(0).unwrap().has_wide_wrap_padding());
    let mut snapshot = term.grid().visible_snapshot();
    snapshot.resize(5, 3);
    assert_eq!(snapshot.row(0).unwrap().to_string(), "ab界Z");
    term.process(b"\r\nx\r\ny\r\nz");
    assert_eq!(term.grid().scrollback_len(), 2);
    term.grid_mut().set_scroll_offset(2);
    let mut history_snapshot = term.grid().visible_snapshot();
    history_snapshot.resize(5, 3);
    assert_eq!(history_snapshot.row(0).unwrap().to_string(), "ab界Z");
    term.grid_mut().set_scroll_offset(0);
    for cols in [5, 3, 5] {
        term.resize(cols, 3);
    }
    assert_eq!(
        term.grid().scrollback_rows().next().unwrap().to_string(),
        "ab界Z"
    );

    // A variation selector can turn the last-column narrow scalar into a
    // wide cluster. Its vacated default cell is the same synthetic padding.
    let mut variation = Terminal::new(3, 3);
    variation.process("ab☺\u{fe0f}Z".as_bytes());
    variation.resize(5, 3);
    assert_eq!(variation.grid().row(0).unwrap().to_string(), "ab☺\u{fe0f}Z");
}

#[test]
fn wide_wrap_never_discards_explicit_or_edited_blank_cells() {
    for styled in [false, true] {
        let mut term = Terminal::new(3, 3);
        term.process("ab界Z".as_bytes());
        let cell = term.grid_mut().cell_mut(0, 2).unwrap();
        if styled {
            cell.bg_spec = ColorSpec::Indexed(4);
        } else {
            cell.set_char(' ');
        }
        term.resize(6, 3);
        assert_eq!(term.grid().row(0).unwrap().to_string(), "ab 界Z");
        if styled {
            assert_eq!(
                term.grid().cell(0, 2).unwrap().bg_spec,
                ColorSpec::Indexed(4)
            );
        }
    }
}

#[test]
fn resize_round_trip_retains_background_color_erasure_cells() {
    let mut term = Terminal::new(5, 4);
    term.process(b"A\x1b[44m\x1b[K\x1b[0m");
    for cols in [3, 5] {
        term.resize(cols, 4);
        assert_eq!(term.grid().cell(0, 0).unwrap().content.to_text(), "A");
        for offset in 1..5 {
            let cell = term.grid().cell(offset / cols, offset % cols).unwrap();
            assert!(cell.content.is_empty());
            assert_eq!(cell.bg_spec, ColorSpec::Indexed(4));
        }
        assert_eq!(
            term.grid().cell(1, cols - 1).unwrap().bg_spec,
            ColorSpec::Default
        );
    }
}

#[test]
fn resize_keeps_cursor_on_text_after_wide_character_reflow() {
    for (cols, expected) in [(3, (1, 2)), (1, (3, 0))] {
        let mut term = Terminal::new(5, 4);
        term.process("ab界Z".as_bytes());
        term.process(b"\x1b[1;5H");
        term.resize(cols, 4);
        let cursor = term.grid().cursor();
        assert_eq!((cursor.row, cursor.col), expected);
        assert_eq!(
            term.grid()
                .cell(cursor.row, cursor.col)
                .unwrap()
                .content
                .to_text(),
            "Z"
        );
        term.process(b"Q");
        assert_eq!(
            term.grid()
                .cell(expected.0, expected.1)
                .unwrap()
                .content
                .to_text(),
            "Q"
        );
        assert!(term.rows_text(0, 3).contains('界'));
        assert_wide_invariants(&term);
    }
}

#[test]
fn classic_csi_repetition_and_erasure_preserve_unicode_cells() {
    let mut term = Terminal::new(20, 3);
    term.process(b"\x1b[5b");
    assert!(term.grid().row(0).unwrap().to_string().is_empty());

    term.process("e\u{301}".as_bytes());
    term.process(b"\x1b[2b");
    assert_eq!(
        term.grid().row(0).unwrap().to_string(),
        "e\u{301}e\u{301}e\u{301}"
    );
    assert_eq!(term.grid().cursor().col, 3);
    for col in 0..3 {
        assert_eq!(
            term.grid().cell(0, col).unwrap().content.to_text(),
            "e\u{301}"
        );
    }

    // REP uses the current rendition rather than cloning the source cell.
    term.process(b"\r\x1b[2K\x1b[31mA\x1b[32m\x1b[2b");
    assert_eq!(
        term.grid().cell(0, 0).unwrap().fg_spec,
        ColorSpec::Indexed(1)
    );
    assert_eq!(
        term.grid().cell(0, 1).unwrap().fg_spec,
        ColorSpec::Indexed(2)
    );
    assert_eq!(
        term.grid().cell(0, 2).unwrap().fg_spec,
        ColorSpec::Indexed(2)
    );

    let mut wide = Terminal::new(10, 2);
    wide.process("A界B".as_bytes());
    wide.process(b"\x1b[41m\x1b[1;3H\x1b[X");
    assert_eq!(wide.grid().cursor().col, 2);
    assert!(wide.grid().cell(0, 1).unwrap().is_empty());
    assert!(wide.grid().cell(0, 2).unwrap().is_empty());
    assert_eq!(
        wide.grid().cell(0, 1).unwrap().bg_spec,
        ColorSpec::Indexed(1)
    );
    assert_eq!(
        wide.grid().cell(0, 2).unwrap().bg_spec,
        ColorSpec::Indexed(1)
    );
    assert_eq!(wide.grid().cell(0, 3).unwrap().content.char(), Some('B'));
    assert_wide_invariants(&wide);

    // Singular scalar parameters are required for these controls.
    wide.process(b"\x1b[1;1H\x1b[2;3b\x1b[1:2X");
    assert_eq!(wide.grid().cell(0, 0).unwrap().content.char(), Some('A'));

    wide.process(b"\x1bc\x1b[3b");
    assert!(wide.grid().row(0).unwrap().to_string().is_empty());
    wide.last_graphic = Some((
        CellContent::Grapheme("x".repeat(MAX_REPEAT_GRAPHEME_BYTES + 1).into_boxed_str()),
        CharacterSet::Ascii,
    ));
    wide.repeat_last_graphic(1);
    assert!(wide.grid().row(0).unwrap().to_string().is_empty());
}

#[test]
fn repeat_state_keeps_single_scalars_inline_and_clusters_owned() {
    let mut term = Terminal::new(10, 2);

    term.process(b"A");
    assert!(matches!(
        term.last_graphic,
        Some((CellContent::Char('A'), CharacterSet::Ascii))
    ));

    term.process("e\u{301}".as_bytes());
    assert!(matches!(
        term.last_graphic,
        Some((CellContent::Grapheme(ref cluster), CharacterSet::Ascii))
            if cluster.as_ref() == "e\u{301}"
    ));
    term.process(b"\x1b[2b");
    assert_eq!(
        term.grid().row(0).unwrap().to_string(),
        "Ae\u{301}e\u{301}e\u{301}"
    );
}

#[test]
fn erase_and_character_edits_honor_bce_without_moving_the_cursor() {
    let mut term = Terminal::new(8, 3);
    term.process(b"one\r\ntwo\r\nthree\r\nfour");
    assert!(term.grid().scrollback_len() > 0);
    let history = term.grid().scrollback_len();
    let cursor = term.grid().cursor();
    let cursor_state = (cursor.row, cursor.col, cursor.wrap_pending, cursor.visible);

    term.process(b"\x1b[44m\x1b[2J");
    let cursor = term.grid().cursor();
    assert_eq!(
        (cursor.row, cursor.col, cursor.wrap_pending, cursor.visible),
        cursor_state
    );
    assert_eq!(term.grid().scrollback_len(), history);
    for row in 0..term.rows() {
        assert!(!term.grid().row(row).unwrap().is_wrapped());
        for col in 0..term.cols() {
            let cell = term.grid().cell(row, col).unwrap();
            assert!(cell.is_empty());
            assert_eq!(cell.bg_spec, ColorSpec::Indexed(4));
            assert!(cell.attrs.is_empty());
        }
    }

    term.process(b"Z");
    let visible = term.rows_text(0, term.rows() as i32 - 1);
    let cursor = term.grid().cursor();
    let cursor_state = (cursor.row, cursor.col, cursor.wrap_pending, cursor.visible);
    term.process(b"\x1b[3J");
    assert_eq!(term.grid().scrollback_len(), 0);
    assert_eq!(term.rows_text(0, term.rows() as i32 - 1), visible);
    let cursor = term.grid().cursor();
    assert_eq!(
        (cursor.row, cursor.col, cursor.wrap_pending, cursor.visible),
        cursor_state
    );

    let mut edits = Terminal::new(8, 2);
    edits.process(b"ABCD\x1b[1;2H\x1b[42m\x1b[2@");
    assert_eq!(
        edits.grid().cell(0, 1).unwrap().bg_spec,
        ColorSpec::Indexed(2)
    );
    assert_eq!(
        edits.grid().cell(0, 2).unwrap().bg_spec,
        ColorSpec::Indexed(2)
    );
    edits.process(b"\x1b[2P");
    assert_eq!(
        edits.grid().cell(0, 6).unwrap().bg_spec,
        ColorSpec::Indexed(2)
    );
    assert_eq!(
        edits.grid().cell(0, 7).unwrap().bg_spec,
        ColorSpec::Indexed(2)
    );

    let mut wide_edits = Terminal::new(8, 2);
    wide_edits.process("A界B".as_bytes());
    wide_edits.process(b"\x1b[1;3H\x1b[43m\x1b[@");
    assert_wide_invariants(&wide_edits);
    assert_eq!(
        wide_edits.grid().cell(0, 1).unwrap().bg_spec,
        ColorSpec::Indexed(3)
    );
    assert_eq!(
        wide_edits.grid().cell(0, 3).unwrap().bg_spec,
        ColorSpec::Indexed(3)
    );
}

#[test]
fn tabulation_controls_preserve_custom_stops_across_resize_and_ris() {
    let mut term = Terminal::new(24, 3);
    term.process(b"\t\x1b[I\x1b[Z");
    assert_eq!(term.grid().cursor().col, 8);

    term.process(b"\x1b[g\r\x1b[I");
    assert_eq!(term.grid().cursor().col, 16);
    term.process(b"\x1b[3g\r\x1b[I");
    assert_eq!(term.grid().cursor().col, 23);

    term.process(b"\x1b[?5W\r\x1b[2I");
    assert_eq!(term.grid().cursor().col, 16);
    term.process(b"\x1b[2Z");
    assert_eq!(term.grid().cursor().col, 0);

    // A retained custom stop survives growth, while newly exposed columns
    // receive defaults. Stops lost to a shrink are defaulted when regrown.
    term.process(b"\x1b[3g\x1b[5G\x1bH");
    term.resize(30, 3);
    term.process(b"\r\x1b[I");
    assert_eq!(term.grid().cursor().col, 4);
    term.process(b"\x1b[I");
    assert_eq!(term.grid().cursor().col, 24);
    term.resize(3, 3);
    term.resize(20, 3);
    term.process(b"\r\x1b[2I");
    assert_eq!(term.grid().cursor().col, 16);

    term.process(b"\x1b[2;2H\x1b[3a\x1b[2e\x1b[7`");
    assert_eq!(term.grid().cursor().row, 2);
    assert_eq!(term.grid().cursor().col, 6);

    term.process(b"\x1b[3g\x1bc\t");
    assert_eq!(term.grid().cursor().col, 8);
}

#[test]
fn declrmm_negotiates_queries_resets_and_disambiguates_csi_s() {
    let mut term = Terminal::new(10, 5);
    term.grid_mut().set_cursor(2, 4);
    term.process(b"\x1b[s");
    term.grid_mut().set_cursor(0, 0);
    term.process(b"\x1b[u");
    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (2, 4));

    term.process(b"\x1b[?69h\x1b[3;8s");
    assert!(term.mode().contains(TerminalMode::LEFT_RIGHT_MARGIN));
    assert_eq!(term.horizontal_scroll_region(), 2..8);
    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (0, 0));

    term.process(b"\x1bP$qs\x1b\\");
    assert_eq!(
        term.drain_events(),
        vec![TerminalEvent::PtyWrite(b"\x1bP1$r3;8s\x1b\\".to_vec())]
    );

    term.grid_mut().set_cursor(3, 6);
    term.process(b"\x1b[8;3s\x1b[2:4s\x1b[1;2;3s");
    assert_eq!(term.horizontal_scroll_region(), 2..8);
    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (3, 6));

    term.process(b"\x1b[s");
    assert_eq!(term.horizontal_scroll_region(), 0..10);
    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (0, 0));
    term.process(b"\x1b[3;8s");
    term.grid_mut().set_cursor(3, 6);

    term.process(b"\x1b[?69l");
    assert!(!term.mode().contains(TerminalMode::LEFT_RIGHT_MARGIN));
    assert_eq!(term.horizontal_scroll_region(), 0..10);
    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (3, 6));

    // A parameterized DECSLRM-shaped sequence is not a cursor save alias
    // once mode 69 is off.
    term.process(b"\x1b[3;8s");
    term.grid_mut().set_cursor(0, 0);
    term.process(b"\x1b[u");
    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (2, 4));

    term.process(b"\x1b[?69h\x1b[2;9s");
    term.resize(8, 5);
    assert!(term.mode().contains(TerminalMode::LEFT_RIGHT_MARGIN));
    assert_eq!(term.horizontal_scroll_region(), 0..8);
    term.process(b"\x1bc");
    assert!(!term.mode().contains(TerminalMode::LEFT_RIGHT_MARGIN));
    assert_eq!(term.horizontal_scroll_region(), 0..8);
}

#[test]
fn horizontal_margins_constrain_dec_cursor_motion_origin_and_tabs() {
    let mut term = Terminal::new(10, 5);
    term.process(b"\x1b[2;4r\x1b[?69h\x1b[3;8s\x1b[?6h");
    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (1, 2));

    term.process(b"\x1b[99;99H");
    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (3, 7));
    term.process(b"\x1b[99A\x1b[99D");
    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (1, 2));
    term.process(b"\x1b[99a\x1b[99e");
    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (3, 7));

    term.process(b"\x1b[?6l\x1b[3;1H\x1b[99C");
    assert_eq!(term.grid().cursor().col, 7);
    term.process(b"\x1b[99D");
    assert_eq!(term.grid().cursor().col, 2);
    term.process(b"\x1b[99a");
    assert_eq!(term.grid().cursor().col, 9);

    term.process(b"\x1b[3;3H\x1b[I");
    assert_eq!(term.grid().cursor().col, 7);
    term.process(b"\x1b[Z");
    assert_eq!(term.grid().cursor().col, 2);

    term.process(b"\x1b[3;10H\r");
    assert_eq!(term.grid().cursor().col, 2);
    term.process(b"\x1b[3;2H\r");
    assert_eq!(term.grid().cursor().col, 0);
}

#[test]
fn margin_wrap_and_scroll_preserve_cells_outside_the_rectangle() {
    let mut term = Terminal::new(10, 4);
    term.process(
            b"AAAAAAAAAA\r\nBBBBBBBBBB\r\nCCCCCCCCCC\r\nDDDDDDDDDD\x1b[2;4r\x1b[?69h\x1b[3;8s\x1b[4;3H123456X",
        );

    assert_eq!(term.grid().row(0).unwrap().to_string(), "AAAAAAAAAA");
    assert_eq!(term.grid().row(1).unwrap().to_string(), "BBCCCCCCBB");
    assert_eq!(term.grid().row(2).unwrap().to_string(), "CC123456CC");
    assert_eq!(term.grid().row(3).unwrap().to_string(), "DDX     DD");
    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (3, 3));
    assert_eq!(term.grid().scrollback_len(), 0);
    assert!(!term.grid().row(2).unwrap().is_wrapped());

    term.process(b"\x1b[3;8H");
    term.process("界".as_bytes());
    assert_eq!(term.grid().cell(3, 2).unwrap().content.to_text(), "界");
    assert_wide_invariants(&term);

    let mut boundary = Terminal::new(6, 3);
    boundary.process(b"\x1b[2;2H");
    boundary.process("界".as_bytes());
    boundary.process(b"\x1b[?69h\x1b[3;5s\x1b[S");
    assert!(boundary.grid().cell(1, 1).unwrap().is_empty());
    assert!(boundary.grid().cell(1, 2).unwrap().is_empty());
    assert_wide_invariants(&boundary);
}

#[test]
fn margin_character_and_line_edits_are_bounded_and_use_bce() {
    let mut term = Terminal::new(10, 4);
    term.process(
            b"AAAAAAAAAA\r\nBBBBBBBBBB\r\nCCCCCCCCCC\r\nDDDDDDDDDD\x1b[2;4r\x1b[?69h\x1b[3;8s\x1b[2;4H\x1b[44m\x1b[2@",
        );
    assert_eq!(term.grid().row(1).unwrap().to_string(), "BBB  BBBBB");
    for col in 3..5 {
        assert_eq!(
            term.grid().cell(1, col).unwrap().bg_spec,
            ColorSpec::Indexed(4)
        );
    }

    term.process(b"\x1b[2P");
    assert_eq!(term.grid().row(1).unwrap().to_string(), "BBBBBB  BB");
    term.process(b"\x1b[L");
    assert_eq!(term.grid().cursor().col, 2);
    assert_eq!(term.grid().row(1).unwrap().to_string(), "BB      BB");
    assert_eq!(term.grid().row(2).unwrap().to_string(), "CCBBBB  CC");
    assert_eq!(term.grid().row(3).unwrap().to_string(), "DDCCCCCCDD");

    let preserved = term.rows_text(0, 3);
    term.process(b"\x1b[1;4H\x1b[L\x1b[M");
    assert_eq!(term.rows_text(0, 3), preserved);
    assert_wide_invariants(&term);
}

#[test]
fn margin_index_reverse_index_and_explicit_scroll_share_the_rectangle() {
    fn populated() -> Terminal {
        let mut term = Terminal::new(8, 4);
        term.process(b"AAAAAAAA\r\nBBBBBBBB\r\nCCCCCCCC\r\nDDDDDDDD\x1b[2;4r\x1b[?69h\x1b[3;6s");
        term
    }

    for sequence in [b"\x1bD".as_slice(), b"\x1b[S".as_slice()] {
        let mut term = populated();
        term.process(b"\x1b[4;4H");
        term.process(sequence);
        assert_eq!(term.grid().row(1).unwrap().to_string(), "BBCCCCBB");
        assert_eq!(term.grid().row(2).unwrap().to_string(), "CCDDDDCC");
        assert_eq!(term.grid().row(3).unwrap().to_string(), "DD    DD");
    }

    let mut next_line = populated();
    next_line.process(b"\x1b[4;5H\x1bE");
    assert_eq!(next_line.grid().row(1).unwrap().to_string(), "BBCCCCBB");
    assert_eq!(next_line.grid().cursor().col, 2);

    for sequence in [b"\x1bM".as_slice(), b"\x1b[T".as_slice()] {
        let mut term = populated();
        term.process(b"\x1b[2;4H");
        term.process(sequence);
        assert_eq!(term.grid().row(1).unwrap().to_string(), "BB    BB");
        assert_eq!(term.grid().row(2).unwrap().to_string(), "CCBBBBCC");
        assert_eq!(term.grid().row(3).unwrap().to_string(), "DDCCCCDD");
    }
}

#[test]
fn images_scroll_only_when_fully_inside_horizontal_margins() {
    let mut term = Terminal::new(10, 4);
    term.grid_mut().set_cursor(2, 3);
    term.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=1,c=1,r=1,C=1;AQIDBA==");
    term.grid_mut().set_cursor(2, 0);
    term.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=2,c=1,r=1,C=1;AQIDBA==");
    term.grid_mut().set_cursor(2, 7);
    term.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=3,c=2,r=1,C=1;AQIDBA==");

    term.process(b"\x1b[2;4r\x1b[?69h\x1b[3;8s\x1b[S");
    let placements: Vec<_> = term
        .graphics_placements()
        .map(|placement| (placement.image_id, placement.col, placement.row))
        .collect();
    assert!(placements.contains(&(ImageId(1), 3, 1)));
    assert!(placements.contains(&(ImageId(2), 0, 2)));
    assert!(placements.contains(&(ImageId(3), 7, 2)));
}

#[test]
fn decstr_resets_only_the_documented_session_state() {
    let mut term = Terminal::new(12, 3);
    term.process(b"one\r\ntwo\r\nthree\r\nfour");
    assert!(term.grid().scrollback_len() > 0);

    // Install a non-default tab stop, then establish every supported
    // DECSTR-controlled state and several unrelated modern modes.
    term.process(
            b"\x1b[3g\x1b[5G\x1bH\x1b[2;3r\x1b[?69h\x1b[3;10s\x1b[?1;6;12h\x1b[?7;25l\x1b[4;20h\x1b=\x1b[?1000;1004;1006;1007;2004h\x1b[31;42;1m\x1b[1\"q\x1b(0\x1b[6 q",
        );
    term.save_cursor();
    term.grid_mut().set_cursor(1, 4);
    term.grid_mut().set_wrap_pending(true);

    let preserved_text = term.rows_text(0, term.rows() as i32 - 1);
    let preserved_scrollback = term.grid().scrollback_len();
    let preserved_modes = TerminalMode::LINEFEED_NEWLINE
        | TerminalMode::MOUSE_TRACKING
        | TerminalMode::FOCUS_EVENTS
        | TerminalMode::MOUSE_SGR
        | TerminalMode::ALTERNATE_SCROLL
        | TerminalMode::BRACKETED_PASTE;
    assert!(term.mode().contains(preserved_modes));

    // Feed DECSTR one byte at a time so the parser's intermediate survives
    // arbitrary PTY read boundaries.
    for byte in b"\x1b[!p" {
        term.process(&[*byte]);
    }

    assert_eq!(term.rows_text(0, term.rows() as i32 - 1), preserved_text);
    assert_eq!(term.grid().scrollback_len(), preserved_scrollback);
    assert_eq!(term.scroll_region(), 0..term.rows());
    assert_eq!(term.horizontal_scroll_region(), 0..term.cols());
    let cursor = term.grid().cursor();
    assert_eq!((cursor.row, cursor.col), (1, 4));
    assert!(!cursor.wrap_pending);
    assert!(cursor.visible);
    assert!(term.mode().contains(TerminalMode::AUTOWRAP));
    assert!(term.mode().contains(preserved_modes));
    assert!(!term.mode().intersects(
        TerminalMode::INSERT
            | TerminalMode::CURSOR_KEYS_APPLICATION
            | TerminalMode::ORIGIN
            | TerminalMode::CURSOR_BLINKING
            | TerminalMode::KEYPAD_APPLICATION
            | TerminalMode::LEFT_RIGHT_MARGIN,
    ));
    assert_eq!(term.cursor_style(), None);
    assert_eq!(term.cursor_blink_override(), None);
    assert_eq!(term.character_protection_parameter(), 0);

    // SGR, DECSCA, and G0/GL return to defaults for subsequent text.
    term.write_char('q');
    let cell = term.grid().cell(1, 4).unwrap();
    assert_eq!(cell.content.char(), Some('q'));
    assert_eq!(cell.attrs, CellAttrs::empty());
    assert_eq!(cell.fg_spec, ColorSpec::Default);
    assert_eq!(cell.bg_spec, ColorSpec::Default);
    assert!(!cell.flags.contains(crate::grid::CellFlags::PROTECTED));

    // DECSTR replaces the active screen's DECSC state with home/defaults,
    // while preserving the custom tab-stop table.
    term.restore_cursor();
    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (0, 0));
    term.process(b"\t");
    assert_eq!(term.grid().cursor().col, 4);
}

#[test]
fn decstr_preserves_the_active_screen_and_rejects_malformed_forms() {
    let mut term = Terminal::new(8, 3);
    term.process(b"P\x1b[2;3H\x1b7\x1b[?1047hA\x1b[3;4H\x1b7");
    assert!(term.mode().contains(TerminalMode::ALT_SCREEN));

    // Nonzero, plural, and subparameter forms are not DECSTR.
    term.process(b"\x1b[4h\x1b[1!p\x1b[0;0!p\x1b[0:0!p");
    assert!(term.mode().contains(TerminalMode::INSERT));
    assert_eq!(term.grid().row(0).unwrap().to_string(), "A");

    term.process(b"\x1b[!p");
    assert!(term.mode().contains(TerminalMode::ALT_SCREEN));
    assert!(!term.mode().contains(TerminalMode::INSERT));
    assert_eq!(term.grid().row(0).unwrap().to_string(), "A");
    term.restore_cursor();
    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (0, 0));

    // The inactive primary screen and its saved cursor remain untouched.
    term.exit_alt_screen();
    assert_eq!(term.grid().row(0).unwrap().to_string(), "P");
    term.restore_cursor();
    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (1, 2));
}

#[test]
fn decscl_soft_resets_and_reports_bounded_level_and_c1_state() {
    let mut term = Terminal::new(12, 3);
    assert_eq!(term.dec_conformance_level(), 4);
    assert!(!term.uses_8bit_controls());

    term.process(b"keep\x1b[3g\x1b[6G\x1bH\x1b[4h\x1b[62;0\"p");
    assert_eq!(term.grid().row(0).unwrap().to_string(), "keep");
    assert!(!term.mode().contains(TerminalMode::INSERT));
    assert_eq!(term.dec_conformance_level(), 2);
    assert!(term.uses_8bit_controls());
    term.process(b"\x1bP$q\"p\x1b\\");
    assert_eq!(
        term.drain_events(),
        vec![TerminalEvent::PtyWrite(b"\x901$r62;0\"p\x9c".to_vec())]
    );

    // An invalid level or level-2+ C1 selector changes nothing, including
    // state that would have been cleared by the DECSCL soft reset.
    term.process(b"\x1b[4h\x1b[66;1\"p\x1b[63;3\"p\x1b[64;1;2\"p\x1b[64:1\"p");
    assert!(term.mode().contains(TerminalMode::INSERT));
    assert_eq!(term.dec_conformance_level(), 2);
    assert!(term.uses_8bit_controls());

    // Pc is ignored at level 1, whose replies are necessarily 7-bit.
    term.process(b"\x1b[61;9\"p\x1bP$q\"p\x1b\\");
    assert!(!term.mode().contains(TerminalMode::INSERT));
    assert_eq!(term.dec_conformance_level(), 1);
    assert!(!term.uses_8bit_controls());
    assert_eq!(
        term.drain_events(),
        vec![TerminalEvent::PtyWrite(b"\x1bP1$r61;1\"p\x1b\\".to_vec())]
    );

    // An omitted Pc defaults to 8-bit controls above level 1. RIS returns
    // to Termeric's level-4, 7-bit startup profile.
    term.process(b"\x1b[65\"p");
    assert_eq!(term.dec_conformance_level(), 5);
    assert!(term.uses_8bit_controls());
    term.process(b"\x1bc");
    assert_eq!(term.dec_conformance_level(), 4);
    assert!(!term.uses_8bit_controls());
}

#[test]
fn selection_and_export_emit_each_grapheme_once_without_spacers() {
    let mut term = Terminal::new(8, 2);
    term.process("A界e\u{301}👍🏽".as_bytes());
    let mut selection = Selection::new(
        Point::new(0, 2), // select from the spacer half of 界
        SelectionType::Simple,
    );
    selection.update(Point::new(0, 5));
    term.set_selection(Some(selection));
    assert_eq!(term.get_selection_text().as_deref(), Some("界e\u{301}👍🏽"));
    let exported = term.grid().plain_lines().join("\n");
    assert_eq!(exported, "A界e\u{301}👍🏽");
}

#[test]
fn hidden_text_remains_copyable() {
    let mut term = Terminal::new(20, 2);
    term.process(b"\x1b[8msecret\x1b[28m");
    let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
    selection.update(Point::new(0, 5));
    term.set_selection(Some(selection));

    assert_eq!(term.get_selection_text().as_deref(), Some("secret"));
    assert!(
        term.grid()
            .cell(0, 0)
            .unwrap()
            .attrs
            .contains(CellAttrs::HIDDEN)
    );
}

#[test]
fn wide_pair_property_holds_across_seeded_edit_sequences() {
    for seed in 0..64_u64 {
        let mut state = seed + 1;
        let mut term = Terminal::new(12, 4);
        term.process("A界e\u{301}👍🏽👩\u{200d}💻Z".as_bytes());
        for _ in 0..128 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let cols = term.cols();
            let col = (state as usize >> 8) % cols.max(1);
            term.grid_mut().set_cursor(0, col);
            match state % 6 {
                0 => term.insert_chars((state as usize % 3) + 1),
                1 => term.delete_chars((state as usize % 3) + 1),
                2 => term.erase_in_line((state % 3) as u16),
                3 => term.write_char('界'),
                4 => term.write_char('\u{301}'),
                _ => term.resize((state as usize % 16) + 1, 4),
            }
            assert_wide_invariants(&term);
        }
    }
}

fn write_line(term: &mut Terminal, s: &str) {
    for c in s.chars() {
        term.write_char(c);
    }
}

#[test]
fn test_word_at_simple_word() {
    let mut term = Terminal::new(80, 24);
    write_line(&mut term, "hello world");
    let (start, end) = term.word_at(Point::new(0, 2)); // inside "hello"
    assert_eq!(start, Point::new(0, 0));
    assert_eq!(end, Point::new(0, 4));
}

#[test]
fn test_word_at_whitespace_is_noop() {
    let mut term = Terminal::new(80, 24);
    write_line(&mut term, "hello world");
    let (start, end) = term.word_at(Point::new(0, 5)); // the space
    assert_eq!(start, Point::new(0, 5));
    assert_eq!(end, Point::new(0, 5));
}

#[test]
fn test_word_at_url_like_token() {
    // path- and URL-like punctuation is part of the word.
    let mut term = Terminal::new(80, 24);
    write_line(&mut term, "cat /tmp/foo.txt done");
    let (start, end) = term.word_at(Point::new(0, 8)); // inside "/tmp/foo.txt"
    assert_eq!(start, Point::new(0, 4));
    assert_eq!(end, Point::new(0, 15));
}

#[test]
fn test_line_at_trims_trailing_space() {
    let mut term = Terminal::new(80, 24);
    write_line(&mut term, "hi   ");
    let (start, end) = term.line_at(Point::new(0, 3));
    assert_eq!(start, Point::new(0, 0));
    assert_eq!(end, Point::new(0, 1)); // "hi" ends at col 1
}

#[test]
fn test_cursor_movement() {
    let mut term = Terminal::new(80, 24);
    term.set_cursor_position(5, 10);
    let cursor = term.grid().cursor();
    assert_eq!(cursor.row, 4);
    assert_eq!(cursor.col, 9);
}

#[test]
fn test_alt_screen() {
    let mut term = Terminal::new(80, 24);
    term.write_char('A');
    term.enter_alt_screen();
    assert!(term.mode().contains(TerminalMode::ALT_SCREEN));
    assert!(term.grid().cell(0, 0).unwrap().is_empty());
    term.exit_alt_screen();
    assert!(!term.mode().contains(TerminalMode::ALT_SCREEN));
    assert_eq!(term.grid().cell(0, 0).unwrap().content.char(), Some('A'));
}

#[test]
fn mouse_focus_paste_and_alternate_scroll_modes_parse_and_reset() {
    let mut term = Terminal::new(80, 24);
    term.process(b"\x1b[?9;1000;1002;1003;1004;1005;1006;1007;2004h");
    let expected = TerminalMode::MOUSE_X10
        | TerminalMode::MOUSE_TRACKING
        | TerminalMode::MOUSE_BUTTON_EVENT
        | TerminalMode::MOUSE_ANY_EVENT
        | TerminalMode::FOCUS_EVENTS
        | TerminalMode::MOUSE_UTF8
        | TerminalMode::MOUSE_SGR
        | TerminalMode::ALTERNATE_SCROLL
        | TerminalMode::BRACKETED_PASTE;
    assert!(term.mode().contains(expected));

    term.process(b"\x1b[?9;1000;1002;1003;1004;1005;1006;1007;2004l");
    assert!(!term.mode().intersects(expected));
}

#[test]
fn application_keyboard_modes_set_and_restore_through_the_parser() {
    let mut term = Terminal::new(80, 24);
    term.process(b"\x1b[?1h\x1b=");
    assert!(term.mode().contains(TerminalMode::CURSOR_KEYS_APPLICATION));
    assert!(term.mode().contains(TerminalMode::KEYPAD_APPLICATION));

    term.process(b"\x1b[?1l\x1b>");
    assert!(
        !term
            .mode()
            .intersects(TerminalMode::CURSOR_KEYS_APPLICATION | TerminalMode::KEYPAD_APPLICATION)
    );
}

#[test]
fn xterm_keyboard_types_are_exclusive_queryable_and_reset_with_xterm_lifecycle() {
    let mut term = Terminal::new(80, 24);
    assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Normal);

    term.process(b"\x1b[?1060h");
    assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Legacy);
    assert_eq!(term.mode_status(true, 1060), 1);
    assert_eq!(term.mode_status(true, 1061), 2);

    term.process(b"\x1b[?1061h");
    assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Vt220);
    assert_eq!(term.mode_status(true, 1060), 2);
    assert_eq!(term.mode_status(true, 1061), 1);

    // Xterm leaves the configured/runtime keyboard type alone on DECSTR,
    // while resetting either type mode selects the normal type.
    term.process(b"\x1b[!p");
    assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Vt220);
    term.process(b"\x1b[?1060l");
    assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Normal);

    term.process(b"\x1b[?1060h\x1bc");
    assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Normal);
}

#[test]
fn configured_xterm_keyboard_type_and_private_save_restore_have_distinct_lifecycles() {
    let mut term = Terminal::new(80, 24);
    term.configure_xterm_keyboard_type(XtermKeyboardType::Legacy, true);
    assert_eq!(
        term.initial_xterm_keyboard_type(),
        XtermKeyboardType::Legacy
    );
    assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Legacy);

    // DECRST still selects xterm's normal/default type. With no explicit
    // save, a private-mode restore falls back to the configured baseline.
    term.process(b"\x1b[?1061l\x1b[?1060r");
    assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Legacy);

    // Both mode numbers address the same saved keyboard-type slot.
    term.process(b"\x1b[?1061h\x1b[?1060s\x1b[?1060h\x1b[?1061r");
    assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Vt220);

    // Malformed subparameters do not overwrite the saved VT220 value.
    term.process(b"\x1b[?1060:1061s\x1b[?1060h\x1b[?1060r");
    assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Vt220);

    term.soft_reset();
    assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Vt220);
    term.reset();
    assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Legacy);

    // A changed live configuration establishes a new current/RIS baseline
    // and clears an obsolete saved mode value.
    term.configure_xterm_keyboard_type(XtermKeyboardType::Normal, true);
    term.process(b"\x1b[?1060r");
    assert_eq!(term.xterm_keyboard_type(), XtermKeyboardType::Normal);
}

#[test]
fn decudk_definitions_are_bounded_preservable_and_reset_only_by_ris() {
    let mut term = Terminal::new(80, 24);
    term.process(b"\x1bP0;1|17/68656c6c6f;1/1b5b41;34/00ff;35/aa;18/0;19/zz\x1b\\");
    assert_eq!(term.xterm_user_defined_key(17), Some(b"hello".as_slice()));
    assert_eq!(term.xterm_user_defined_key(1), Some(b"\x1b[A".as_slice()));
    assert_eq!(term.xterm_user_defined_key(34), Some(b"\0\xff".as_slice()));
    assert_eq!(term.xterm_user_defined_key(18), None);
    assert_eq!(term.xterm_user_defined_key(19), None);
    assert_eq!(term.xterm_user_defined_key(35), None);

    // Nonzero first parameters preserve other entries. Xterm accepts the
    // lock parameter but keeps UDKs unlocked, so a later definition wins.
    term.process(b"\x1bP1;0|17/776f726c64;18/21\x1b\\");
    assert_eq!(term.xterm_user_defined_key(17), Some(b"world".as_slice()));
    assert_eq!(term.xterm_user_defined_key(18), Some(b"!".as_slice()));
    assert_eq!(term.xterm_user_defined_key(1), Some(b"\x1b[A".as_slice()));

    // DECSTR, as the parser receives it, keeps every definition.
    term.process(b"\x1b[!p");
    assert_eq!(term.xterm_user_defined_key(17), Some(b"world".as_slice()));
    // A definition split across arbitrary chunks still arrives whole.
    let split = b"\x1bP1;1|20/6368756e6b6564\x1b\\";
    for chunk in split.chunks(3) {
        term.process(chunk);
    }
    assert_eq!(term.xterm_user_defined_key(20), Some(b"chunked".as_slice()));
    term.process(b"\x1bc");
    assert_eq!(term.xterm_user_defined_key(17), None);
    assert_eq!(term.xterm_user_defined_key_bytes(), 0);

    // Level 1 predates DECUDK. Returning to level 2 admits it again.
    term.process(b"\x1b[61\"p\x1bP0;1|17/61\x1b\\");
    assert_eq!(term.xterm_user_defined_key(17), None);
    term.process(b"\x1b[62;1\"p\x1bP0;1|17/62\x1b\\");
    assert_eq!(term.xterm_user_defined_key(17), Some(b"b".as_slice()));

    let oversized = "ab".repeat(MAX_USER_DEFINED_KEY_BYTES + 1);
    term.process(format!("\x1bP1;1|18/{oversized}\x1b\\").as_bytes());
    assert_eq!(term.xterm_user_defined_key(18), None);

    let maximum = "ab".repeat(MAX_USER_DEFINED_KEY_BYTES);
    for code in 0..XTERM_USER_DEFINED_KEY_COUNT as u16 {
        term.process(format!("\x1bP1;1|{code}/{maximum}\x1b\\").as_bytes());
    }
    assert!(term.xterm_user_defined_key_bytes() <= MAX_USER_DEFINED_KEYS_BYTES);
    assert_eq!(
        term.xterm_user_defined_keys().count(),
        MAX_USER_DEFINED_KEYS_BYTES / MAX_USER_DEFINED_KEY_BYTES
    );
}

#[test]
fn reserved_xterm_string_key_resource_is_queryable_resettable_and_encoder_neutral() {
    let mut term = Terminal::new(80, 24);
    term.process(
        b"\x1b[?5m\x1b[?5g\x1b[>5;4m\x1b[>5;1f\x1b[?5m\x1b[?5g\x1bP$q>5m\x1b\\\x1bP$q>5f\x1b\\",
    );
    assert_eq!(
        term.xterm_key_resource(XtermKeyResource::String),
        XtermKeySetting::new(XtermKeyMode::Extended, XtermKeyFormat::CsiU)
    );
    assert_eq!(
        term.drain_events(),
        vec![
            TerminalEvent::PtyWrite(b"\x1b[>5;0m".to_vec()),
            TerminalEvent::PtyWrite(b"\x1b[>5;0f".to_vec()),
            TerminalEvent::PtyWrite(b"\x1b[>5;4m".to_vec()),
            TerminalEvent::PtyWrite(b"\x1b[>5;1f".to_vec()),
            TerminalEvent::PtyWrite(b"\x1bP1$r>5;4m\x1b\\".to_vec()),
            TerminalEvent::PtyWrite(b"\x1bP1$r>5;1f\x1b\\".to_vec()),
        ]
    );

    // Out-of-range and malformed shapes are atomic. The reserved state is
    // global across screens and has no effect on ordinary screen content.
    term.process(b"\x1b[>5;9m\x1b[>5;2f\x1b[>5:1m\x1b[?1049hX");
    assert_eq!(
        term.xterm_key_resource(XtermKeyResource::String),
        XtermKeySetting::new(XtermKeyMode::Extended, XtermKeyFormat::CsiU)
    );
    assert_eq!(term.grid().cell(0, 0).unwrap().content.char(), Some('X'));

    term.process(b"\x1b[>5n");
    assert_eq!(
        term.xterm_key_resource(XtermKeyResource::String).mode(),
        XtermKeyMode::Disabled
    );
    term.process(b"\x1b[>5m");
    assert_eq!(
        term.xterm_key_resource(XtermKeyResource::String).mode(),
        XtermKeyMode::Original
    );
    term.process(b"\x1b[>5;4m\x1b[>5;1f\x1b[!p");
    assert_eq!(
        term.xterm_key_resource(XtermKeyResource::String),
        XtermKeyResource::String.default_setting()
    );
    term.process(b"\x1b[>5;4m\x1b[>5;1f\x1bc");
    assert_eq!(
        term.xterm_key_resource(XtermKeyResource::String),
        XtermKeyResource::String.default_setting()
    );
}

#[test]
fn decrqss_reports_legacy_geometry_without_enabling_window_mutation() {
    let mut term = Terminal::new(132, 40);
    term.process(b"\x1bP$qt\x1b\\\x1bP$q$|\x1b\\\x1bP$q*|\x1b\\\x1bP$q$}\x1b\\\x1bP$q$~\x1b\\");
    assert_eq!(
        term.drain_events(),
        vec![
            TerminalEvent::PtyWrite(b"\x1bP1$r39t\x1b\\".to_vec()),
            TerminalEvent::PtyWrite(b"\x1bP1$r80$|\x1b\\".to_vec()),
            TerminalEvent::PtyWrite(b"\x1bP1$r40*|\x1b\\".to_vec()),
            TerminalEvent::PtyWrite(b"\x1bP0$r\x1b\\".to_vec()),
            TerminalEvent::PtyWrite(b"\x1bP0$r\x1b\\".to_vec()),
        ]
    );

    // Geometry and DEC status-line mutations remain application/window
    // policy operations and are deliberately ignored by the core.
    term.process(b"\x1b[50t\x1b[132$|\x1b[99*|\x1b[1$}\x1b[1$~");
    assert_eq!((term.cols(), term.rows()), (132, 40));
    assert!(term.drain_events().is_empty());

    // External geometry remains observable. Current xterm reports a
    // minimum DECSLPP value of 24, while DECSNLS reports the exact rows.
    term.resize(100, 24);
    term.process(b"\x1b G\x90$qt\x9c\x90$q$|\x9c\x90$q*|\x9c");
    assert_eq!(
        term.drain_events(),
        vec![
            TerminalEvent::PtyWrite(b"\x901$r24t\x9c".to_vec()),
            TerminalEvent::PtyWrite(b"\x901$r80$|\x9c".to_vec()),
            TerminalEvent::PtyWrite(b"\x901$r24*|\x9c".to_vec()),
        ]
    );
}

/// Helper: collect all non-empty characters from scrollback + visible area.
/// This is a content-only check — ignores line boundaries.
fn extract_all_chars(term: &Terminal) -> String {
    let grid = term.grid();
    let mut chars = String::new();

    // Scrollback
    for row in grid.scrollback_rows() {
        for cell in row.iter() {
            if let Some(c) = cell.content.char() {
                chars.push(c);
            }
        }
    }

    // Visible area
    for r in 0..grid.rows() {
        if let Some(row) = grid.row(r) {
            for cell in row.iter() {
                if let Some(c) = cell.content.char() {
                    chars.push(c);
                }
            }
        }
    }

    chars
}

#[test]
fn test_process_and_resize_preserves_content() {
    // Process a few lines of text at 40 cols, then shrink to 10 cols.
    // All characters must still be present.
    let mut term = Terminal::new(40, 24);
    term.process(b"Hello World\r\n");
    term.process(b"ABCDEFGHIJKLMNOPQRSTUVWXYZ\r\n");
    term.process(b"1234567890\r\n");

    let before = extract_all_chars(&term);

    // Shrink to 10 columns
    term.resize(10, 24);

    let after = extract_all_chars(&term);
    assert_eq!(before, after, "Characters lost after shrink to 10 cols");

    // Grow back to 40 columns
    term.resize(40, 24);

    let after_grow = extract_all_chars(&term);
    assert_eq!(
        before, after_grow,
        "Characters lost after grow back to 40 cols"
    );
}

#[test]
fn test_process_long_line_reflow_to_small_width() {
    // Write a 50-char line in a 50-col terminal, then shrink to 5 cols.
    // All 50 characters must be preserved.
    let mut term = Terminal::new(50, 24);
    let text = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuv";
    term.process(text.as_bytes());
    term.process(b"\r\n");

    let before = extract_all_chars(&term);
    assert_eq!(before.len(), text.len());

    term.resize(5, 24);

    let after = extract_all_chars(&term);
    assert_eq!(before, after, "Characters lost after shrink to 5 cols");
}

#[test]
fn test_process_resize_to_very_small_height() {
    // Write multiple lines, then shrink to 3 rows.
    // Content should go to scrollback, not be lost.
    let mut term = Terminal::new(20, 10);
    for i in 0..8 {
        term.process(format!("Line {}\r\n", i).as_bytes());
    }

    let before = extract_all_chars(&term);

    term.resize(20, 3);

    let after = extract_all_chars(&term);
    assert_eq!(before, after, "Characters lost after shrink to 3 rows");
}

#[test]
fn test_process_resize_both_dimensions_small() {
    // Start at 40x10, write content, shrink to 5x3.
    // This is the scenario the user reported as corrupting text.
    let mut term = Terminal::new(40, 10);
    term.process(b"Hello World\r\n");
    term.process(b"ABCDEFGHIJKLMNOPQRSTUVWXYZ\r\n");
    term.process(b"$ ");

    let before = extract_all_chars(&term);

    term.resize(5, 3);

    let after = extract_all_chars(&term);
    assert_eq!(before, after, "Characters lost after shrink to 5x3");
}

#[test]
fn test_autowrap_sets_wrapped_flag() {
    // Write enough chars to fill exactly one row and trigger autowrap.
    // The first row should be marked as wrapped.
    let mut term = Terminal::new(10, 5);
    // Write exactly 10 chars to fill row 0
    term.process(b"ABCDEFGHIJ");
    // The cursor is now at col 10 (past end). Write one more char to trigger autowrap.
    term.process(b"K");

    // Row 0 should be wrapped
    assert!(
        term.grid().row(0).unwrap().is_wrapped(),
        "Row 0 should be marked as wrapped after autowrap"
    );
    // Row 1 should have 'K'
    assert_eq!(
        term.grid().cell(1, 0).unwrap().content.char(),
        Some('K'),
        "Character after autowrap should be on row 1"
    );
}

#[test]
fn test_autowrap_reflow_round_trip() {
    // Write 20 chars in a 10-col terminal (autowrap creates 2 rows).
    // Grow to 20 cols — should merge back to 1 row.
    // Shrink back to 10 cols — should split to 2 rows again.
    let mut term = Terminal::new(10, 5);
    term.process(b"ABCDEFGHIJKLMNOPQRST"); // 20 chars, fills 2 rows

    assert!(term.grid().row(0).unwrap().is_wrapped());

    let original_chars = extract_all_chars(&term);
    assert_eq!(original_chars.len(), 20);

    // Grow to 20 cols — should merge into 1 row
    term.resize(20, 5);
    let after_grow = extract_all_chars(&term);
    assert_eq!(original_chars, after_grow, "Chars lost on grow");

    let row0_text = term.grid().row(0).unwrap().to_string();
    assert_eq!(row0_text, "ABCDEFGHIJKLMNOPQRST");

    // Shrink back to 10
    term.resize(10, 5);
    let after_shrink = extract_all_chars(&term);
    assert_eq!(original_chars, after_shrink, "Chars lost on shrink back");
}

#[test]
fn test_initial_small_terminal_no_corruption() {
    // Create a very small terminal and write text that exceeds its width.
    // This tests the scenario where the pane is "too small to begin with."
    let mut term = Terminal::new(5, 3);
    term.process(b"Hello World!\r\n");
    term.process(b"ABCDEFGHIJ\r\n");

    let chars = extract_all_chars(&term);
    // "Hello World!" is 12 chars, "ABCDEFGHIJ" is 10 chars — total 22
    assert_eq!(chars.len(), 22, "Expected 22 chars, got: '{}'", chars);
    assert!(chars.contains("Hello"), "Missing 'Hello' in output");
    assert!(chars.contains("World"), "Missing 'World' in output");
    assert!(
        chars.contains("ABCDEFGHIJ"),
        "Missing 'ABCDEFGHIJ' in output"
    );
}

#[test]
fn test_small_terminal_then_grow() {
    // Write into a 5x3 terminal, then grow to 40x24.
    // All content must be preserved (in scrollback or visible).
    let mut term = Terminal::new(5, 3);
    term.process(b"ABCDEFGHIJKLMNO\r\n"); // 15 chars
    term.process(b"XYZ\r\n");

    let before = extract_all_chars(&term);

    term.resize(40, 24);

    let after = extract_all_chars(&term);
    assert_eq!(
        before, after,
        "Characters lost after growing from 5x3 to 40x24"
    );
}

#[test]
fn test_pane_close_reflow_simulation() {
    // Simulate the pane lifecycle: start at full width, shrink to pane width,
    // write text that wraps, then grow back to full width.
    // This mirrors what happens when a pane is created and then closed.
    let mut term = Terminal::new(80, 24);

    // Phase 1: Resize to pane width (simulating split)
    term.resize(40, 24);
    assert_eq!(term.cols(), 40);

    // Phase 2: Write text that wraps at 40 cols
    // A 60-char line will wrap: 40 chars on row 0 (wrapped), 20 chars on row 1
    let long_line = "ABCDEFGHIJ".repeat(6); // 60 chars
    term.process(long_line.as_bytes());
    term.process(b"\r\n");
    term.process(b"Short line\r\n");
    term.process(b"$ "); // prompt

    let before = extract_all_chars(&term);
    assert!(
        before.contains(&long_line),
        "Long line should be fully present"
    );

    // Phase 3: Resize back to full width (simulating pane close)
    term.resize(80, 24);

    let after = extract_all_chars(&term);
    assert_eq!(
        before, after,
        "Characters lost after expanding from 40 to 80 cols"
    );

    // The 60-char line should now fit on a single row (no wrapping needed at 80 cols)
    // Check that row 0 has the full content (either in scrollback or visible)
    let grid = term.grid();
    let mut found_full_line = false;
    for r in 0..grid.rows() {
        if let Some(row) = grid.row(r) {
            let text = row.to_string();
            if text == long_line {
                found_full_line = true;
                assert!(
                    !row.is_wrapped(),
                    "60-char line should not be wrapped at 80 cols"
                );
                break;
            }
        }
    }
    // Also check scrollback
    for row in grid.scrollback_rows() {
        let text = row.to_string();
        if text == long_line {
            found_full_line = true;
            assert!(
                !row.is_wrapped(),
                "60-char line in scrollback should not be wrapped at 80 cols"
            );
            break;
        }
    }
    assert!(
        found_full_line,
        "Should find the full 60-char line on a single row after resize to 80 cols"
    );
}

#[test]
fn test_pane_reflow_multiple_wrapped_lines() {
    // Multiple auto-wrapped lines should all reflow when growing.
    let mut term = Terminal::new(20, 24);

    // Write 3 lines that wrap at 20 cols
    let line1 = "A".repeat(30); // wraps to 2 rows
    let line2 = "B".repeat(25); // wraps to 2 rows
    let line3 = "C".repeat(40); // wraps to 2 rows

    term.process(line1.as_bytes());
    term.process(b"\r\n");
    term.process(line2.as_bytes());
    term.process(b"\r\n");
    term.process(line3.as_bytes());
    term.process(b"\r\n");

    let before = extract_all_chars(&term);

    // Grow to 80 cols — all lines should unwrap
    term.resize(80, 24);

    let after = extract_all_chars(&term);
    assert_eq!(
        before, after,
        "Characters lost after expanding from 20 to 80 cols"
    );
}

#[test]
fn horizontal_protocol_selection_mapping_obeys_counts_and_margins() {
    let scenarios: &[(&[u8], usize, usize, usize, bool)] = &[
        (b"\x1b[H", 0, 0, 8, true),
        (b"\x1b[2;4H", 1, 3, 8, true),
        (b"\x1b[?69h\x1b[2;7s\x1b[2;3r\x1b[2;3H", 1, 2, 7, true),
        (b"\x1b[?69h\x1b[2;7s\x1b[2;3r\x1b[1;3H", 0, 2, 8, true),
        (b"\x1b[?69h\x1b[2;7s\x1b[2;3r\x1b[2;1H", 1, 0, 7, false),
        (b"\x1b[?69h\x1b[2;7s\x1b[2;3r\x1b[2;8H", 1, 7, 7, false),
        (
            b"\x1b[?69h\x1b[2;7s\x1b[2;3r\x1b[?6h\x1b[1;2H",
            1,
            2,
            7,
            true,
        ),
    ];
    for &(prefix, edit_row, left, right, enabled) in scenarios {
        for inserting in [false, true] {
            for count in [0_usize, 1, 2, 99] {
                for index in 0..24 {
                    let mut term = Terminal::new(8, 3);
                    term.process(b"abcdefgh\r\nijklmnop\r\nqrstuvwx");
                    term.process(prefix);
                    let point = Point::new(index / 8, index as usize % 8);
                    term.set_selection(Some(Selection::new(point, SelectionType::Block)));
                    let text = term.get_selection_text();
                    let cursor = term.grid().cursor();
                    term.process(
                        format!("\x1b[{count}{}", if inserting { '@' } else { 'P' }).as_bytes(),
                    );
                    let expected = if enabled
                        && point.row == edit_row as i32
                        && (left..right).contains(&point.col)
                    {
                        let distance = count.max(1).min(right - left) as isize;
                        point
                            .col
                            .checked_add_signed(if inserting { distance } else { -distance })
                            .filter(|col| (left..right).contains(col))
                            .map(|col| Point::new(point.row, col))
                    } else {
                        Some(point)
                    };
                    assert_eq!(
                        term.selection().map(|selection| selection.start),
                        expected,
                        "prefix={prefix:?} insert={inserting} count={count} point={point:?}"
                    );
                    if expected.is_some() {
                        assert_eq!(term.get_selection_text(), text);
                    }
                    assert_eq!(
                        (term.grid().cursor().row, term.grid().cursor().col),
                        (cursor.row, cursor.col)
                    );
                }
            }
        }
    }
}

#[test]
fn horizontal_edits_preserve_whole_glyphs_and_retire_boundary_damage() {
    for inserting in [false, true] {
        for wide in [false, true] {
            let mut term = Terminal::new(12, 4);
            term.process(b"..\x1b[1\"q");
            if wide {
                term.process("界".as_bytes());
            } else {
                term.process(b"\x1b]66;s=1:w=2;A\x07");
            }
            term.grid_mut().set_cursor(0, 0);
            term.set_selection(Some(Selection::new(Point::new(0, 2), SelectionType::Block)));
            let text = term.get_selection_text();
            term.shift_chars(1, inserting);
            assert_eq!(
                term.selection().unwrap().start.col,
                if inserting { 3 } else { 1 }
            );
            assert_eq!(term.get_selection_text(), text);
        }

        for selected_col in [2, 3] {
            let mut term = Terminal::new(12, 4);
            term.process("..界".as_bytes());
            term.process(b"\x1b[?69h\x1b[1;3s\x1b[H");
            term.set_selection(Some(Selection::new(
                Point::new(0, selected_col),
                SelectionType::Block,
            )));
            term.shift_chars(1, inserting);
            assert!(term.selection().is_none()); // Repair covers both moved and stationary halves.
        }

        let mut block = Terminal::new(12, 4);
        block.process(b"....\x1b]66;s=1:w=4;A\x07\x1b[?69h\x1b[1;6s\x1b[H");
        block.set_selection(Some(Selection::new(Point::new(0, 7), SelectionType::Block)));
        block.shift_chars(1, inserting);
        assert!(block.selection().is_none());
        assert!((0..12).all(|col| !block.grid().cell(0, col).unwrap().multicell.is_present()));

        let mut tall = Terminal::new(12, 4);
        tall.process(b"..\x1b]66;s=2;A\x07\x1b[2;1H");
        tall.set_selection(Some(Selection::new(Point::new(0, 2), SelectionType::Block)));
        tall.shift_chars(0, inserting);
        assert_eq!(tall.get_selection_text().as_deref(), Some("A"));
        tall.shift_chars(1, inserting);
        assert!(tall.selection().is_none()); // Whole block erased outside the edited row.
        assert!(tall.grid().cell(0, 2).unwrap().content.is_empty());
    }
}

#[test]
fn horizontal_edits_isolate_frozen_history_and_mixed_motion_selections() {
    for inserting in [false, true] {
        let mut term = Terminal::new(12, 3);
        term.process(b"..anchor\r\nother");
        let mut selection = Selection::new(Point::new(0, 7), SelectionType::Simple);
        selection.update(Point::new(0, 2));
        term.set_selection(Some(selection.clone()));
        term.process(b"\x1b[22J");
        term.shift_chars(1, inserting);
        assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
        term.process(b"\x1b[?1049h..anchor\x1b[H");
        term.set_selection(Some(selection));
        term.process(b"\x1b[?2026h");
        term.shift_chars(1, inserting);
        assert_eq!(term.selection().unwrap().start, Point::new(0, 7));
        assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
        term.process(b"\x1b[?2026l");
        assert_eq!(
            term.selection().unwrap().start.col,
            if inserting { 8 } else { 6 }
        );
        assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));

        let mut mixed = Selection::new(Point::new(0, 2), SelectionType::Block);
        mixed.update(Point::new(1, 4));
        term.set_selection(Some(mixed));
        term.shift_chars(1, inserting);
        assert!(term.selection().is_none());
    }
}

#[test]
fn horizontal_edits_follow_surviving_selected_text() {
    let mut term = Terminal::new(12, 3);
    term.process(b"..anchor\x1b[H");
    let mut selection = Selection::new(Point::new(0, 2), SelectionType::Simple);
    selection.update(Point::new(0, 7));
    term.set_selection(Some(selection));
    term.process(b"\x1b[2@");
    assert_eq!(term.selection().unwrap().start, Point::new(0, 4));
    assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
    term.process(b"\x1b[2P");
    assert_eq!(term.selection().unwrap().start, Point::new(0, 2));
    assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
}

#[test]
fn rectangular_write_selection_matches_cellwise_damage_across_selection_shapes() {
    let commands: &[&[u8]] = &[
        b"\x1b[88;1;3;2;5$x",
        b"\x1b[1;1;2;4;1;2;3;1$v",
        b"\x1b[2;3;3;6;1;1;1;1$v",
        b"\x1b[1;1;3;8;1;99;99;1$v",
        b"\x1b[9;1;1;3;8$x",       // Nonprintable fill: no-op.
        b"\x1b[88;3;5;1;2$x",      // Reversed rectangle: no-op.
        b"\x1b[1;1;3;8;1;1;1;1$v", // Self-copy: no-op.
        b"\x1b[?69h\x1b[2;6s\x1b[2;3r\x1b[?6h\x1b[88;1;2;2;3$x",
        b"\x1b[?69h\x1b[2;6s\x1b[2;3r\x1b[?6h\x1b[1;1;1;2;1;2;3;1$v",
    ];
    for &command in commands {
        for ty in [
            SelectionType::Simple,
            SelectionType::Semantic,
            SelectionType::Lines,
            SelectionType::Block,
        ] {
            for index in 0..24 {
                for reverse in [false, true] {
                    let mut term = Terminal::new(8, 3);
                    term.process(b"abcdefgh\r\nijklmnop\r\nqrstuvwx");
                    let point = Point::new(index / 8, index as usize % 8);
                    let other = Point::new(1, 3);
                    let (start, end) = if reverse {
                        (other, point)
                    } else {
                        (point, other)
                    };
                    let mut selection = Selection::new(start, ty);
                    selection.update(end);
                    let original: Vec<_> = (0..3)
                        .map(|row| term.grid().row(row).unwrap().clone())
                        .collect();
                    term.set_selection(Some(selection.clone()));
                    term.process(command);
                    let damaged = (0..3).any(|row| {
                        (0..8).any(|col| {
                            selection.contains(row as i32, col)
                                && original[row].get(col).unwrap().content
                                    != term.grid().cell(row, col).unwrap().content
                        })
                    });
                    assert_eq!(
                        term.selection().is_none(),
                        damaged,
                        "command={command:?} type={ty:?} start={start:?} end={end:?}"
                    );
                    if !damaged {
                        let current = term.selection().unwrap();
                        assert_eq!(
                            (current.start, current.end, current.ty, current.active),
                            (
                                selection.start,
                                selection.end,
                                selection.ty,
                                selection.active
                            )
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn rectangular_writes_invalidate_erased_wide_and_multicell_parts_outside_destination() {
    for copy in [false, true] {
        for wide in [false, true] {
            for target_leader in [false, true] {
                let mut term = Terminal::new(12, 4);
                // Protection does not prevent DECFRA/DECCRA overwriting.
                term.process(b"\x1b[1\"q");
                if wide {
                    term.process("界".as_bytes());
                } else {
                    term.process(b"\x1b]66;s=2:w=2;A\x07");
                }
                term.process(b"\x1b[0\"q\x1b[4;10HZ");
                let covered = Point::new(if wide { 0 } else { 1 }, 1);
                let leader = Point::new(0, 0);
                let (target, selected) = if target_leader {
                    (leader, covered)
                } else {
                    (covered, leader)
                };
                term.set_selection(Some(Selection::new(selected, SelectionType::Block)));
                if copy {
                    term.copy_rectangular_area(
                        4,
                        10,
                        4,
                        10,
                        1,
                        target.row as usize + 1,
                        target.col + 1,
                        1,
                    );
                } else {
                    term.fill_rectangular_area(
                        b'Z' as u16,
                        target.row as usize + 1,
                        target.col + 1,
                        target.row as usize + 1,
                        target.col + 1,
                    );
                }
                assert!(
                    term.selection().is_none(),
                    "copy={copy} wide={wide} target_leader={target_leader}"
                );
                assert!(
                    term.grid()
                        .cell(selected.row as usize, selected.col)
                        .unwrap()
                        .content
                        .is_empty()
                );
            }
        }
    }
}

#[test]
fn rectangular_copy_preserves_source_selection_and_valid_copied_multicells() {
    let mut term = Terminal::new(12, 5);
    term.process(b"\x1b]66;s=2:w=2;A\x07");
    let source = term.grid().multicell_region_at(0, 0).unwrap();
    term.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
    // A clipped source produces no valid destination block, but reading it
    // must not invalidate the selected, untouched original block.
    term.copy_rectangular_area(1, 1, 1, 1, 1, 4, 8, 1);
    assert_eq!(term.get_selection_text().as_deref(), Some("A"));
    assert!(!term.grid().cell(3, 7).unwrap().multicell.is_present());
    term.copy_rectangular_area(1, 1, source.bottom(), source.right(), 1, 4, 8, 1);
    assert_eq!(term.get_selection_text().as_deref(), Some("A"));
    assert!(term.grid().multicell_region_at(3, 7).is_some());
}

#[test]
fn rectangular_writes_keep_history_and_frozen_selection_but_retire_identical_replacement() {
    for copy in [false, true] {
        let command: &[u8] = if copy {
            b"\x1b[2;1;2;6;1;1;1;1$v"
        } else {
            b"\x1b[97;1;1;1;6$x"
        };
        let mut term = Terminal::new(12, 3);
        term.process(b"aaaaaa\r\naaaaaa");
        let mut selection = Selection::new(Point::new(0, 5), SelectionType::Simple);
        selection.update(Point::new(0, 0));
        term.set_selection(Some(selection.clone()));
        term.process(command);
        assert!(term.selection().is_none()); // Even identical replacement retires the anchor.

        term.set_selection(Some(selection.clone()));
        term.process(b"\x1b[22J");
        term.process(command);
        assert_eq!(term.get_selection_text().as_deref(), Some("aaaaaa"));

        term.process(b"\x1b[?1049hanchor\r\nsource");
        term.set_selection(Some(selection));
        term.process(b"\x1b[?2026h");
        term.process(command);
        assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
        term.process(b"\x1b[?2026l");
        assert!(term.selection().is_none());
    }
}

#[test]
fn rectangular_fill_and_copy_invalidate_selected_destination() {
    for command in [b"\x1b[88;1;1;1;3$x".as_slice(), b"\x1b[1;5;1;7;1;1;1;1$v"] {
        let mut term = Terminal::new(12, 3);
        term.process(b"old new");
        let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
        selection.update(Point::new(0, 2));
        term.set_selection(Some(selection));
        term.process(command);
        assert!(term.selection().is_none(), "command={command:?}");
    }
}

#[test]
fn explicit_erase_invalidates_a_selection_over_removed_text() {
    let mut term = Terminal::new(12, 3);
    term.process(b"selected");
    let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
    selection.update(Point::new(0, 7));
    term.set_selection(Some(selection));
    term.process(b"\x1b[2K");
    assert!(term.selection().is_none());
}

#[test]
fn erase_command_selection_damage_matches_actual_cell_changes() {
    let commands: &[&[u8]] = &[
        b"\x1b[J",
        b"\x1b[1J",
        b"\x1b[2J",
        b"\x1b[K",
        b"\x1b[1K",
        b"\x1b[2K",
        b"\x1b[2X",
        b"\x1b[?J",
        b"\x1b[?1J",
        b"\x1b[?2J",
        b"\x1b[?K",
        b"\x1b[?1K",
        b"\x1b[?2K",
        b"\x1b[2;4;2;5$z",
        b"\x1b[2;4;2;5${",
    ];
    for &command in commands {
        for row in 0..3 {
            for col in 0..8 {
                let mut term = Terminal::new(8, 3);
                term.process(b"abcdefgh\r\nijklmnop\r\nqrstuvwx\x1b[2;4H");
                for protected_row in 0..3 {
                    for protected_col in [1, 4, 7] {
                        term.grid_mut()
                            .cell_mut(protected_row, protected_col)
                            .unwrap()
                            .flags
                            .insert(CellFlags::PROTECTED);
                    }
                }
                let original = term.grid().cell(row, col).unwrap().content.to_text();
                term.set_selection(Some(Selection::new(
                    Point::new(row as i32, col),
                    SelectionType::Block,
                )));
                term.process(command);
                let unchanged = term.grid().cell(row, col).unwrap().content.to_text() == original;
                assert_eq!(
                    term.selection().is_some(),
                    unchanged,
                    "command={command:?} row={row} col={col}"
                );
                assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (1, 3));
            }
        }
    }
}

#[test]
fn selective_erase_preserves_protected_selection_and_invalidates_expanded_glyph_damage() {
    for protected in [false, true] {
        for selective in [false, true] {
            for wide in [false, true] {
                let mut term = Terminal::new(10, 4);
                if protected {
                    term.process(b"\x1b[1\"q");
                }
                if wide {
                    term.process("界".as_bytes());
                } else {
                    term.process(b"\x1b]66;s=2:w=2;A\x07");
                }
                term.process(b"\x1b[0\"q");
                term.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
                let text = term.get_selection_text();
                // Target only the wide spacer or a covered lower-row cell,
                // outside the selection's requested row/column rectangle.
                let row = if wide { 1 } else { 2 };
                if selective {
                    term.selective_erase_rectangular_area(row, 2, row, 2);
                } else {
                    term.erase_rectangular_area(row, 2, row, 2);
                }
                assert_eq!(
                    term.selection().is_some(),
                    protected && selective,
                    "protected={protected} selective={selective} wide={wide}"
                );
                if protected && selective {
                    assert_eq!(term.get_selection_text(), text);
                }
            }
        }
    }
}

#[test]
fn erasure_keeps_archived_and_frozen_selection_but_clears_erased_alternate_content() {
    let mut term = Terminal::new(12, 3);
    term.process(b"anchor");
    let mut selection = Selection::new(Point::new(0, 5), SelectionType::Simple);
    selection.update(Point::new(0, 0));
    term.set_selection(Some(selection));
    term.process(b"\x1b[22J");
    assert_eq!(term.selection().unwrap().start, Point::new(-3, 5));
    assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
    term.process(b"\x1b[2J\x1b[?2J");
    assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));

    term.process(b"\x1b[?1049hanchor");
    let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
    selection.update(Point::new(0, 5));
    term.set_selection(Some(selection));
    term.process(b"\x1b[?2026h\x1b[2J");
    assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));
    term.process(b"\x1b[?2026l");
    assert!(term.selection().is_none());

    term.process("\x1b[H界".as_bytes());
    term.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
    term.grid_mut().set_cursor(0, 1);
    term.erase_chars(0); // The core API's empty range must not erase half a pair.
    term.erase_in_display(99);
    term.selective_erase_in_line(99);
    term.erase_rectangular_area(3, 3, 1, 1);
    assert_eq!(term.get_selection_text().as_deref(), Some("界"));
}

#[test]
fn ordinary_printing_invalidates_a_selection_over_replaced_text() {
    let mut term = Terminal::new(12, 3);
    term.process(b"selected");
    let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
    selection.update(Point::new(0, 7));
    selection.finish();
    term.set_selection(Some(selection));
    term.process(b"\x1b[Hchanged!");
    assert!(term.selection().is_none());
}

#[test]
fn ordinary_writes_preserve_unaffected_history_and_frozen_selections() {
    for kind in [
        SelectionType::Simple,
        SelectionType::Semantic,
        SelectionType::Lines,
        SelectionType::Block,
    ] {
        for reverse in [false, true] {
            let mut term = Terminal::new(12, 3);
            term.process(b"alpha beta\r\nmiddle\r\nlast");
            let (start, end) = if reverse { (4, 0) } else { (0, 4) };
            let mut selection = Selection::new(Point::new(0, start), kind);
            selection.update(Point::new(0, end));
            term.set_selection(Some(selection));
            let text = term.get_selection_text();
            term.process(b"\x1b[3;1HX\x1b[1;1H\x1b[31m");
            assert_eq!(term.get_selection_text(), text);
            assert_eq!(term.selection().unwrap().start, Point::new(0, start));
            // Even an identical printable rewrite replaces selected cells.
            term.process(b"a");
            assert!(term.selection().is_none());
        }
    }
    let mut term = Terminal::new(12, 3);
    term.process(b"anchor\r\none\r\ntwo\r\nthree");
    let mut selection = Selection::new(Point::new(-1, 0), SelectionType::Simple);
    selection.update(Point::new(-1, 5));
    term.set_selection(Some(selection));
    term.process(b"\x1b[Hchanged");
    assert_eq!(term.get_selection_text().as_deref(), Some("anchor"));

    let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
    selection.update(Point::new(0, 6));
    term.set_selection(Some(selection));
    term.process(b"\x1b[?2026h\x1b[Hhidden!");
    assert_eq!(term.get_selection_text().as_deref(), Some("changed"));
    term.process(b"\x1b[?2026l");
    assert!(term.selection().is_none());
}

#[test]
fn grapheme_growth_erases_top_row_collisions_and_preserves_original_rendition() {
    for selected in [Point::new(0, 0), Point::new(1, 2), Point::new(3, 11)] {
        let mut term = Terminal::new(12, 4);
        term.process("\x1b[1;31;44m\x1b[1\"q❤".as_bytes());
        let original = term.grid().cell(0, 0).unwrap().clone();
        term.process(b"\x1b[0m\x1b[0\"q\x1b]66;s=2:w=2;old\x07\x1b[1;2H");
        term.set_selection(Some(Selection::new(selected, SelectionType::Block)));
        term.process("\u{fe0f}".as_bytes());
        assert_eq!(term.selection().is_some(), selected.row == 3);
        let grown = term.grid().cell(0, 0).unwrap();
        assert_eq!(grown.content.to_text(), "❤\u{fe0f}");
        assert_eq!(
            (grown.fg_spec, grown.bg_spec, grown.attrs),
            (original.fg_spec, original.bg_spec, original.attrs)
        );
        assert!(grown.flags.contains(CellFlags::PROTECTED));
        assert!((0..4).all(|row| {
            (0..12).all(|col| !term.grid().cell(row, col).unwrap().multicell.is_present())
        }));
        assert_wide_invariants(&term);
    }
}

#[test]
fn grapheme_growth_skips_lower_multicell_rows_without_invalidating_them() {
    for cols in [6, 12] {
        for autowrap in [false, true] {
            for selected_block in [false, true] {
                let mut term = Terminal::new(cols, 4);
                term.process(b"\x1b[1;2H\x1b]66;s=2:w=2;old\x07\x1b[2;1H");
                term.process("❤".as_bytes());
                if !autowrap {
                    term.process(b"\x1b[?7l");
                }
                let destination = if cols == 6 {
                    Point::new(2, 0)
                } else {
                    Point::new(1, 5)
                };
                let selected = if selected_block {
                    Point::new(0, 1)
                } else {
                    destination
                };
                term.set_selection(Some(Selection::new(selected, SelectionType::Block)));
                term.process("\u{fe0f}".as_bytes());
                assert_eq!(term.selection().is_some(), selected_block);
                if selected_block {
                    assert_eq!(term.get_selection_text().as_deref(), Some("old"));
                }
                assert!(term.grid().multicell_region_at(0, 1).is_some());
                assert!(term.grid().cell(1, 0).unwrap().content.is_empty());
                assert_eq!(
                    term.grid()
                        .cell(destination.row as usize, destination.col)
                        .unwrap()
                        .content
                        .to_text(),
                    "❤\u{fe0f}"
                );
                assert_wide_invariants(&term);
            }
        }
    }
}

#[test]
fn grapheme_growth_wrap_collisions_preserve_erase_space_and_frozen_selection_rules() {
    for covered_top in [false, true] {
        for frozen in [false, true] {
            let mut term = Terminal::new(12, 4);
            term.process(b"\x1b[?1049h\x1b[2;1H\x1b]66;s=2:w=2;old\x07");
            if covered_top {
                term.process(b"\x1b[?69h\x1b[2;11s\x1b[1;11H");
            } else {
                term.process(b"\x1b[1;12H");
            }
            term.process("❤".as_bytes());
            term.set_selection(Some(Selection::new(Point::new(1, 0), SelectionType::Block)));
            if frozen {
                term.process(b"\x1b[?2026h");
            }
            term.process("\u{fe0f}".as_bytes());
            let left = usize::from(covered_top);
            assert_eq!(
                term.grid().cell(1, left).unwrap().content.to_text(),
                "❤\u{fe0f}"
            );
            assert!(!term.grid().cell(2, 3).unwrap().multicell.is_present());
            assert_eq!(
                term.grid().cell(2, 3).unwrap().content.char(),
                covered_top.then_some(' ')
            );
            if frozen {
                assert_eq!(term.get_selection_text().as_deref(), Some("old"));
                term.process(b"\x1b[?2026l");
            }
            assert!(term.selection().is_none());
            assert_wide_invariants(&term);
        }
    }
}

#[test]
fn grapheme_width_growth_cannot_leave_partial_multicell_descriptors() {
    let mut term = Terminal::new(12, 4);
    term.process("❤\x1b]66;s=2:w=2;old\x07\x1b[1;2H".as_bytes());
    term.process("\u{fe0f}".as_bytes());
    assert_wide_invariants(&term);
    for (row, col, cell) in term.grid().iter_cells() {
        assert!(
            !cell.multicell.is_present() || term.grid().multicell_region_at(row, col).is_some(),
            "orphan descriptor at {row},{col}"
        );
    }
}

#[test]
fn ordinary_unicode_writes_invalidate_complete_glyph_damage_including_wrap_destinations() {
    for (selected_col, write_col) in [(0, 1), (1, 0)] {
        let mut term = Terminal::new(6, 3);
        term.process("界x".as_bytes());
        term.set_selection(Some(Selection::new(
            Point::new(0, selected_col),
            SelectionType::Block,
        )));
        term.grid_mut().set_cursor(0, write_col);
        term.write_char('a');
        assert!(term.selection().is_none());
    }
    for initial in ["e", "❤"] {
        let mut term = Terminal::new(6, 3);
        term.process(initial.as_bytes());
        term.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
        term.process("\u{301}".as_bytes());
        assert!(term.selection().is_none());
    }
    let mut term = Terminal::new(6, 3);
    term.process("❤x".as_bytes());
    term.set_selection(Some(Selection::new(Point::new(0, 1), SelectionType::Block)));
    term.grid_mut().set_cursor(0, 1);
    term.process("\u{fe0f}".as_bytes());
    assert!(term.selection().is_none());
    for point in [Point::new(0, 2), Point::new(1, 0)] {
        let mut term = Terminal::new(3, 3);
        term.process("\x1b[2;1HQ\x1b[1;1Hab❤".as_bytes());
        term.set_selection(Some(Selection::new(point, SelectionType::Block)));
        term.process("\u{fe0f}".as_bytes());
        assert!(term.selection().is_none(), "{point:?}");
    }
    let mut term = Terminal::new(6, 3);
    term.process("界x".as_bytes());
    term.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
    term.grid_mut().set_cursor(0, 2);
    term.write_char('y');
    assert_eq!(term.get_selection_text().as_deref(), Some("界"));
}

#[test]
fn ordinary_writes_invalidate_changed_multicell_blocks_but_not_skipped_lower_rows() {
    let mut term = Terminal::new(10, 4);
    term.process(b"\x1b]66;s=2:w=2;A\x07");
    term.set_selection(Some(Selection::new(Point::new(0, 0), SelectionType::Block)));
    term.grid_mut().set_cursor(1, 0);
    term.write_char('x'); // Skips the block and writes at column four.
    assert_eq!(term.get_selection_text().as_deref(), Some("A"));
    term.set_selection(Some(Selection::new(Point::new(1, 0), SelectionType::Block)));
    term.grid_mut().set_cursor(0, 1);
    term.write_char('y');
    assert!(term.selection().is_none());

    let mut term = Terminal::new(10, 4);
    term.process(b"\x1b]66;s=2;A\x07");
    term.set_selection(Some(Selection::new(Point::new(1, 1), SelectionType::Block)));
    term.process("\u{301}".as_bytes());
    assert!(term.selection().is_none());

    let mut term = Terminal::new(12, 4);
    term.process(b"....\x1b]66;s=2;A\x07");
    term.set_selection(Some(Selection::new(Point::new(1, 4), SelectionType::Block)));
    term.process(b"\x1b[H\x1b[4hx");
    assert!(term.selection().is_none());
    assert!(term.grid().multicell_region_at(1, 4).is_none());
}

#[test]
fn partial_scroll_keeps_selection_on_its_moved_text() {
    let mut term = Terminal::new(8, 5);
    term.process(b"zero\r\none\r\ntwo\r\nthree\r\nfour");
    let mut selection = Selection::new(Point::new(2, 0), SelectionType::Simple);
    selection.update(Point::new(2, 2));
    term.set_selection(Some(selection));
    term.process(b"\x1b[2;4r\x1b[S");
    assert_eq!(term.selection().unwrap().start, Point::new(1, 0));
    assert_eq!(term.get_selection_text().as_deref(), Some("two"));
}

#[test]
fn vertical_region_commands_preserve_survivors_and_clear_lost_or_split_selections() {
    let commands: &[(&[u8], i32)] = &[
        (b"\x1b[S", -1),
        (b"\x1b[T", 1),
        (b"\x1b[2;3H\x1b[L", 1),
        (b"\x1b[2;3H\x1b[M", -1),
        (b"\x1b[4;3H\n", -1),
        (b"\x1b[2;3H\x1bM", 1),
    ];
    for partial_width in [false, true] {
        for &(command, delta) in commands {
            for kind in [
                SelectionType::Simple,
                SelectionType::Semantic,
                SelectionType::Block,
                SelectionType::Lines,
            ] {
                for reverse in [false, true] {
                    let mut term = Terminal::new(8, 5);
                    term.process(
                        b"00aaaa00\r\n11bbbb11\r\n22cccc22\r\n33dddd33\r\n44eeee44\x1b[2;4r",
                    );
                    if partial_width {
                        term.process(b"\x1b[?69h\x1b[3;6s");
                    }
                    let (start, end) = if reverse { (5, 2) } else { (2, 5) };
                    let mut selection = Selection::new(Point::new(2, start), kind);
                    selection.update(Point::new(2, end));
                    selection.finish();
                    term.set_selection(Some(selection));
                    let text = term.get_selection_text();
                    term.process(command);
                    if partial_width && kind == SelectionType::Lines {
                        assert!(term.selection().is_none()); // A whole line splits at side margins.
                    } else {
                        let selection = term.selection().unwrap();
                        assert_eq!(selection.start, Point::new(2 + delta, start));
                        assert_eq!(selection.end, Point::new(2 + delta, end));
                        assert!(!selection.active);
                        assert_eq!(term.get_selection_text(), text);
                    }
                }
            }
        }
    }
}

#[test]
fn region_selection_preserves_outside_cells_and_rejects_erasure_or_mixed_motion() {
    for (start, end, keep) in [
        (Point::new(2, 0), Point::new(2, 1), true),
        (Point::new(0, 2), Point::new(0, 5), true),
        (Point::new(1, 2), Point::new(1, 5), false),
        (Point::new(2, 1), Point::new(2, 5), false),
        (Point::new(0, 2), Point::new(2, 5), false),
    ] {
        let mut term = Terminal::new(8, 5);
        term.process(
            b"00aaaa00\r\n11bbbb11\r\n22cccc22\r\n33dddd33\r\n44eeee44\x1b[2;4r\x1b[?69h\x1b[3;6s",
        );
        let mut selection = Selection::new(start, SelectionType::Simple);
        selection.update(end);
        term.set_selection(Some(selection));
        let text = term.get_selection_text();
        term.process(b"\x1b[S");
        assert_eq!(term.selection().is_some(), keep, "{start:?} {end:?}");
        if keep {
            assert_eq!(term.get_selection_text(), text);
        }
    }
    let mut term = Terminal::new(8, 5);
    term.process(b"zero\r\none\r\ntwo\r\nthree\r\nfour");
    let mut selection = Selection::new(Point::new(2, 0), SelectionType::Simple);
    selection.update(Point::new(2, 2));
    term.set_selection(Some(selection));
    term.process(b"\x1b[2;4r\x1b[?2026h\x1b[S");
    assert_eq!(term.selection().unwrap().start.row, 2);
    assert_eq!(term.get_selection_text().as_deref(), Some("two"));
    term.process(b"\x1b[?2026l");
    assert_eq!(term.selection().unwrap().start.row, 1);
    term.process(b"\x1b[99S");
    assert!(term.selection().is_none());
}

#[test]
fn clearing_history_retires_only_selections_with_removed_backing_rows() {
    let mut term = Terminal::new(12, 3);
    term.process(b"history\r\nvisible\r\ntail\r\nlast");
    let mut selection = Selection::new(Point::new(-1, 0), SelectionType::Simple);
    selection.update(Point::new(0, 6));
    term.set_selection(Some(selection));
    term.process(b"\x1b[3J");
    assert_eq!(term.grid().scrollback_len(), 0);
    assert!(term.selection().is_none());

    let mut selection = Selection::new(Point::new(0, 0), SelectionType::Simple);
    selection.update(Point::new(0, 6));
    term.set_selection(Some(selection));
    term.process(b"\x1b[3J");
    assert_eq!(term.get_selection_text().as_deref(), Some("visible"));

    // Removing hidden history must not remove a frozen visible selection.
    term.process(b"\x1b[?2026h\r\nnew\r\nnew\r\nnew\x1b[3J");
    assert_eq!(term.get_selection_text().as_deref(), Some("visible"));
}

#[test]
fn byte_eviction_clears_selection_that_no_longer_has_backing_rows() {
    let mut term = Terminal::new(20, 2);
    term.set_scrollback_limits(10_000, 512);
    term.set_selection(Some(Selection::new(
        Point::new(0, 0),
        SelectionType::Simple,
    )));
    for line in 0..100 {
        term.process(format!("line-{line:03}\r\n").as_bytes());
    }
    assert!(term.selection().is_none());
    assert!(term.memory_usage().scrollback_bytes <= 512);
}

#[test]
fn alternate_screen_is_viewport_only_and_cannot_double_the_history_budget() {
    let mut term = Terminal::new(24, 2);
    term.enter_alt_screen();
    for line in 0..50 {
        term.process(format!("default-alt-{line:03}\r\n").as_bytes());
    }
    assert_eq!(term.grid().scrollback_len(), 0);
    term.exit_alt_screen();

    term.set_scrollback_limits(10_000, 8 * 1024);
    for line in 0..100 {
        term.process(format!("primary-{line:03}\r\n").as_bytes());
    }
    let primary = term.memory_usage();
    assert!(primary.scrollback_lines > 0);
    assert!(primary.scrollback_bytes <= 8 * 1024);

    term.enter_alt_screen();
    for line in 0..500 {
        term.process(format!("\x1b[3{}malt-{line:03}-界-e\u{301}\x1b[0m\r\n", line % 8).as_bytes());
    }
    assert_eq!(term.grid().scrollback_len(), 0);
    let with_alternate_output = term.memory_usage();
    assert_eq!(
        with_alternate_output.scrollback_bytes,
        primary.scrollback_bytes
    );
    assert_eq!(
        with_alternate_output.scrollback_lines,
        primary.scrollback_lines
    );

    // Runtime reductions remain authoritative even while the primary
    // screen is inactive, and alternate output cannot consume the share.
    term.set_scrollback_limits(10_000, 512);
    let reduced = term.memory_usage();
    assert!(reduced.scrollback_bytes <= 512);
    assert_eq!(term.grid().scrollback_len(), 0);

    term.exit_alt_screen();
    assert!(term.grid().scrollback_len() > 0);
    assert!(term.memory_usage().scrollback_bytes <= 512);
}

#[test]
fn hyperlink_registry_is_bounded_and_pruned_with_history() {
    let mut term = Terminal::new(20, 2);
    term.set_scrollback_limits(10_000, 4 * 1024);
    for line in 0..700 {
        term.set_hyperlink(
            format!("https://example.com/{line}/{}", "x".repeat(1024)),
            None,
        );
        term.process(b"x");
        term.reset_hyperlink();
        term.process(b"\r\n");
    }
    let usage = term.memory_usage();
    assert!(usage.scrollback_bytes <= 4 * 1024);
    assert!(usage.hyperlink_bytes < 1024 * 1024);
    assert!(term.hyperlink_registry().len() < 256);
}

#[test]
fn dynamic_colors_are_terminal_local_resettable_and_generation_checked() {
    use crate::event::{DynamicColorOperation, DynamicColorTarget};
    let mut first = Terminal::new(10, 2);
    let second = Terminal::new(10, 2);
    let color = Color::rgb(12, 34, 56);
    first.apply_dynamic_color(DynamicColorOperation::Set {
        target: DynamicColorTarget::Palette(42),
        color,
    });
    assert_eq!(first.dynamic_colors().palette(42), Some(color));
    assert_eq!(second.dynamic_colors().palette(42), None);
    let generation = first.dynamic_colors().generation();
    first.apply_dynamic_color(DynamicColorOperation::Set {
        target: DynamicColorTarget::Palette(42),
        color,
    });
    assert_eq!(first.dynamic_colors().generation(), generation);
    first.apply_dynamic_color(DynamicColorOperation::Reset(DynamicColorTarget::Palette(
        42,
    )));
    assert_eq!(first.dynamic_colors().palette(42), None);
    assert!(first.dynamic_colors().generation() > generation);
}

#[test]
fn synchronized_output_retains_presented_hyperlinks_until_release() {
    let mut term = Terminal::new(30, 2);
    term.process(b"\x1b]8;;https://visible.test\x1b\\old\x1b]8;;\x1b\\");
    let visible_id = term.grid().cell(0, 0).unwrap().hyperlink_id.unwrap();
    term.process(b"\x1b[?2026h\x1b[H\x1b]8;;https://hidden.test\x1b\\new\x1b]8;;\x1b\\");
    term.set_scrollback_limits(0, 0);
    assert_eq!(
        term.get_hyperlink_url(visible_id),
        Some("https://visible.test")
    );
    assert_eq!(
        term.presentation_hyperlink_at(0, 0),
        Some("https://visible.test")
    );
    term.process(b"\x1b[?2026l");
    term.set_scrollback_limits(0, 0);
    assert_eq!(term.get_hyperlink_url(visible_id), None);
    assert_eq!(
        term.presentation_hyperlink_at(0, 0),
        Some("https://hidden.test")
    );
}

#[test]
fn synchronized_hyperlink_hit_testing_keeps_scrolled_screen_identity_until_timeout() {
    let mut term = Terminal::new(12, 2);
    term.process(b"\x1b]8;;https://history.test\x1b\\old\x1b]8;;\x1b\\\r\none\r\n\x1b]8;;https://offscreen.test\x1b\\two\x1b]8;;\x1b\\");
    term.grid_mut().set_scroll_offset(1);
    assert_eq!(term.presentation_hyperlink_at(2, 0), None);
    assert_eq!(
        term.presentation_hyperlink_at(0, 0),
        Some("https://history.test")
    );
    term.process(b"\x1b[?2026h");
    term.grid_mut().set_scroll_offset(0);
    term.process(b"\x1b[?1049h\x1b]8;;https://alternate.test\x1b\\new\x1b]8;;\x1b\\");
    assert_eq!(
        term.presentation_hyperlink_at(0, 0),
        Some("https://history.test")
    );
    assert_eq!(term.presentation_hyperlink_at(2, 0), None);
    assert_eq!(term.presentation_hyperlink_at(0, 12), None);
    assert!(term.expire_synchronized_output(term.synchronized_output_deadline().unwrap()));
    assert_eq!(
        term.presentation_hyperlink_at(0, 0),
        Some("https://alternate.test")
    );
}

#[test]
fn synchronized_output_freezes_text_colors_and_images_at_the_mode_boundary() {
    use crate::event::{DynamicColorOperation, DynamicColorTarget};

    let mut term = Terminal::new(12, 3);
    term.process(b"old");
    term.apply_dynamic_color(DynamicColorOperation::Set {
        target: DynamicColorTarget::Foreground,
        color: Color::rgb(1, 2, 3),
    });
    term.process(b"\x1b[3 q");
    term.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=7,C=1;AQIDBA==");
    let live_pixels = term.get_image(ImageId(7)).unwrap().data.clone();

    // Output after DECSET 2026 in the same parser call must remain live
    // internally without becoming presentable.
    term.process(b"\x1b[?2026h\rnew");
    term.apply_dynamic_color(DynamicColorOperation::Set {
        target: DynamicColorTarget::Foreground,
        color: Color::rgb(9, 8, 7),
    });
    term.process(b"\x1b[6 q");
    term.handle_graphics_command(b"a=d,d=a");

    assert!(term.is_synchronized_output());
    assert!(term.rows_text(0, 0).starts_with("new"));
    assert_eq!(term.bounded_presentation_text(0, 64).0, "old");
    assert_eq!(term.presentation_grid().row(0).unwrap().to_string(), "old");
    assert_eq!(
        term.presentation_dynamic_colors().foreground(),
        Some(Color::rgb(1, 2, 3))
    );
    assert_eq!(term.graphics_placement_count(), 0);
    assert_eq!(term.cursor_style_parameter(), 6);
    assert_eq!(
        term.presentation_cursor_style()
            .map(TerminalCursorStyle::parameter),
        Some(3)
    );
    assert_eq!(term.presentation_images().len(), 1);
    assert!(std::sync::Arc::ptr_eq(
        &live_pixels,
        &term.presentation_images()[0].image.data
    ));

    term.process(b"\x1b[?2026l");
    assert!(!term.is_synchronized_output());
    assert!(
        term.presentation_grid()
            .row(0)
            .unwrap()
            .to_string()
            .starts_with("new")
    );
    assert_eq!(
        term.presentation_dynamic_colors().foreground(),
        Some(Color::rgb(9, 8, 7))
    );
    assert!(term.presentation_images().is_empty());
    assert_eq!(
        term.presentation_cursor_style()
            .map(TerminalCursorStyle::parameter),
        Some(6)
    );
}

#[test]
fn bounded_presentation_text_is_utf8_safe_and_alternate_screen_is_viewport_only() {
    let mut term = Terminal::new(8, 2);
    term.process(b"old-one\r\nold-two\r\n");
    let (text, history, truncated) = term.bounded_presentation_text(8, 4);
    assert!(std::str::from_utf8(text.as_bytes()).is_ok());
    assert!(text.len() <= 4);
    assert!(truncated);
    assert!(history <= 8);

    term.enter_alt_screen();
    term.process("界".as_bytes());
    let (text, history, _) = term.bounded_presentation_text(8, 4);
    assert_eq!(text, "界");
    assert_eq!(history, 0);
    assert!(term.alternate_screen_active());

    term.exit_alt_screen();
    term.process(b"\x1b[?2026h\x1b[?1049halt");
    assert!(term.alternate_screen_active());
    assert!(!term.presentation_alternate_screen_active());
    assert!(!term.bounded_presentation_text(8, 64).0.contains("alt"));
    term.process(b"\x1b[?2026l");
    assert!(term.presentation_alternate_screen_active());
}

#[test]
fn synchronized_output_repeated_begin_does_not_extend_the_recovery_deadline() {
    let mut term = Terminal::new(8, 2);
    term.process(b"\x1b[?2026h");
    let deadline = term.synchronized_output_deadline().unwrap();

    term.process(b"\x1b[?2026h");
    assert_eq!(term.synchronized_output_deadline(), Some(deadline));
    assert!(!term.expire_synchronized_output(deadline - Duration::from_millis(1)));
    assert!(term.expire_synchronized_output(deadline));
    assert!(!term.is_synchronized_output());
    assert!(term.synchronized_output_deadline().is_none());
}

#[test]
fn dec_special_graphics_maps_the_complete_vt100_repertoire() {
    let mut term = Terminal::new(80, 2);
    term.process(b"\x1b(0_`abcdefghijklmnopqrstuvwxyz{|}~");
    assert_eq!(term.rows_text(0, 0), " ◆▒␉␌␍␊°±␤␋┘┐┌└┼⎺⎻─⎼⎽├┤┴┬│≤≥π≠£·");
}

#[test]
fn g0_g1_shift_save_restore_alt_screen_and_ris_do_not_leak() {
    let mut term = Terminal::new(20, 2);
    term.process(b"q\x1b)0q\x0eq\x0fq\x1b(A#");
    assert_eq!(term.rows_text(0, 0), "qq─q£");

    let mut saved = Terminal::new(20, 2);
    saved.process(b"\x1b(0\x1b7\x1b(Bq\x1b8q");
    assert_eq!(saved.rows_text(0, 0), "─");

    let mut alternate = Terminal::new(20, 2);
    alternate.process(b"\x1b(0\x1b[?1049h\x1b(Bq\x1b[?1049lq");
    assert_eq!(alternate.rows_text(0, 0), "─");

    alternate.process(b"\x1bcq");
    assert_eq!(alternate.rows_text(0, 0), "q");
}

#[test]
fn g2_g3_single_and_locking_shifts_are_one_shot_saved_and_resettable() {
    let mut term = Terminal::new(20, 2);
    term.process(b"\x1b*0\x1b+A\x1b(B\x1bNqq\x1bO##\x1bnq\x1bo#");
    assert_eq!(term.rows_text(0, 0), "─q£#─£");

    let mut saved = Terminal::new(10, 2);
    saved.process(b"\x1b*0\x1b+A\x1bn\x1bO\x1b7\x1b*B\x1b(B\x1b8#q");
    assert_eq!(saved.rows_text(0, 0), "£─");

    saved.process(b"\x1bc\x1bNq");
    assert_eq!(saved.rows_text(0, 0), "q");
}

#[test]
fn presentation_state_reports_are_byte_exact_bounded_and_strict() {
    let mut term = Terminal::new(12, 4);
    term.process(
            b"\x1b(A\x1b)0\x1b*A\x1b+0\x1bo\x1b~\x1b[1;2;3;4:3;6;7m\x1b[1\"q\x1b[?6h\x1b[4;12HX\x1bN\x1b[1$w",
        );
    assert_eq!(
        term.drain_events(),
        vec![TerminalEvent::PtyWrite(
            b"\x1bP1$u4;12;1;O;A;K;3;1;@;A0A0\x1b\\".to_vec()
        )]
    );

    term.process(b"\x1b[3g\x1b[1;3H\x1bH\x1b[1;7H\x1bH\x1b[2$w");
    assert_eq!(
        term.drain_events(),
        vec![TerminalEvent::PtyWrite(b"\x1bP2$u3/7\x1b\\".to_vec())]
    );

    // Zero/unknown reports, plural/subparameter forms, and unrelated
    // intermediates are ignored without a misleading response.
    term.process(b"\x1b[$w\x1b[0$w\x1b[3$w\x1b[1;2$w\x1b[1:0$w\x1b[1*w");
    assert!(term.drain_events().is_empty());
}

#[test]
fn presentation_state_restore_round_trips_and_rejects_atomically() {
    let mut source = Terminal::new(12, 4);
    source.process(
            b"\x1b(A\x1b)0\x1b*A\x1b+0\x1bo\x1b~\x1b[1;4:3;6;7m\x1b[1\"q\x1b[?6h\x1b[4;12HX\x1bN\x1b[1$w",
        );
    let mut events = source.drain_events();
    let Some(TerminalEvent::PtyWrite(mut cursor_report)) = events.pop() else {
        panic!("expected one cursor information report");
    };
    assert!(events.is_empty());
    cursor_report[4] = b't';

    source.process(b"\x1b[3g\x1b[1;3H\x1bH\x1b[1;7H\x1bH\x1b[2$w");
    let mut events = source.drain_events();
    let Some(TerminalEvent::PtyWrite(mut tab_report)) = events.pop() else {
        panic!("expected one tab stop report");
    };
    assert!(events.is_empty());
    tab_report[4] = b't';

    let mut restored = Terminal::new(12, 4);
    restored.process(b"\x1b[8;9m");
    restored.process(&cursor_report);
    restored.process(&tab_report);
    assert!(restored.current_attrs.contains(CellAttrs::HIDDEN));
    assert!(restored.current_attrs.contains(CellAttrs::STRIKE));

    restored.process(b"\x1b[1$w\x1b[2$w");
    assert_eq!(
        restored.drain_events(),
        vec![
            TerminalEvent::PtyWrite(b"\x1bP1$u4;12;1;O;A;K;3;1;@;A0A0\x1b\\".to_vec()),
            TerminalEvent::PtyWrite(b"\x1bP2$u3/7\x1b\\".to_vec()),
        ]
    );

    // Invalid cursor coordinates/designators and malformed tab lists are
    // all-or-nothing and cannot partially replace live state.
    restored.process(
            b"\x1bP1$t99;12;1;O;A;K;3;1;@;A0A0\x1b\\\x1bP1$t4;12;1;O;A;K;3;1;@;A0AZ\x1b\\\x1bP1$t4;12;1;O;A;F;3;1;@;A0A0\x1b\\\x1bP2$t3//7\x1b\\",
        );
    restored.restore_presentation_state(1, &[b'1'; 129]);
    restored.restore_presentation_state(2, &[b'1'; 12 * 3 + 1]);
    restored.process(b"\x1b[1$w\x1b[2$w");
    assert_eq!(
        restored.drain_events(),
        vec![
            TerminalEvent::PtyWrite(b"\x1bP1$u4;12;1;O;A;K;3;1;@;A0A0\x1b\\".to_vec()),
            TerminalEvent::PtyWrite(b"\x1bP2$u3/7\x1b\\".to_vec()),
        ]
    );
}

#[test]
fn dec_saved_cursor_restores_origin_delayed_wrap_and_screen_local_state() {
    let mut term = Terminal::new(4, 4);
    term.process(b"\x1b[2;4r\x1b[?6h\x1b[2;4HX\x1b7");
    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (2, 3));
    assert!(term.grid().cursor().wrap_pending);

    term.process(b"\x1b[?6l\x1b[HY\x1b8");
    assert!(term.mode().contains(TerminalMode::ORIGIN));
    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (2, 3));
    assert!(term.grid().cursor().wrap_pending);

    // Restored pending wrap is observable on the next graphic, and the
    // saved state remains available for repeated DECRC operations.
    term.process(b"Z");
    assert_eq!(term.grid().cell(3, 0).unwrap().content.char(), Some('Z'));
    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (3, 1));
    term.process(b"\x1b8");
    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (2, 3));
    assert!(term.grid().cursor().wrap_pending);

    // DECAWM is a live mode, not part of the saved cursor. Changing it
    // after DECSC remains authoritative after DECRC.
    term.process(b"\x1b[?7l\x1b7\x1b[?7h\x1b8");
    assert!(term.mode().contains(TerminalMode::AUTOWRAP));

    // Primary and alternate screens retain independent saved states.
    let mut screens = Terminal::new(6, 4);
    screens.process(b"\x1b[1;2H\x1b7\x1b[?47h\x1b[2;4r\x1b[?6h\x1b[2;5H\x1b7");
    screens.process(b"\x1b[?47l\x1b[H\x1b8");
    assert!(!screens.mode().contains(TerminalMode::ORIGIN));
    assert_eq!(
        (screens.grid().cursor().row, screens.grid().cursor().col),
        (0, 1)
    );
    screens.process(b"\x1b[?47h\x1b[H\x1b8");
    assert!(screens.mode().contains(TerminalMode::ORIGIN));
    assert_eq!(
        (screens.grid().cursor().row, screens.grid().cursor().col),
        (2, 4)
    );

    // With no prior DECSC, DECRC restores the defined home/default state.
    let mut unsaved = Terminal::new(6, 4);
    unsaved.process(b"\x1b[31m\x1b[1\"q\x1b[?6h\x1b[3;3H\x1b8Q");
    assert!(!unsaved.mode().contains(TerminalMode::ORIGIN));
    assert_eq!(
        (unsaved.grid().cursor().row, unsaved.grid().cursor().col),
        (0, 1)
    );
    let cell = unsaved.grid().cell(0, 0).unwrap();
    assert_eq!(cell.content.char(), Some('Q'));
    assert_eq!(cell.fg_spec, ColorSpec::Default);
    assert!(!cell.flags.contains(CellFlags::PROTECTED));

    // A saved absolute position remains memory-safe across a resize and
    // is clamped by the destination grid while retaining delayed wrap.
    let mut resized = Terminal::new(8, 4);
    resized.process(b"\x1b[4;8HX\x1b7");
    resized.resize(3, 2);
    resized.process(b"\x1b8");
    assert_eq!(
        (resized.grid().cursor().row, resized.grid().cursor().col),
        (1, 2)
    );
    assert!(resized.grid().cursor().wrap_pending);
}

#[test]
fn kitty_apc_survives_arbitrary_process_boundaries() {
    let mut term = Terminal::new(8, 4);
    term.process(b"\x1b_Ga=t,f=32,s=1,");
    term.process(b"v=1,i=44;AQ");
    term.process(b"IDBA==\x1b");
    term.process(b"\\");

    assert_eq!(
        term.get_image(ImageId(44)).unwrap().data.as_ref(),
        [1, 2, 3, 4]
    );
    assert!(term.drain_events().iter().any(|event| matches!(
        event,
        TerminalEvent::PtyWrite(bytes) if bytes == b"\x1b_Gi=44;OK\x1b\\"
    )));
}

#[test]
fn kitty_unicode_placeholders_follow_grid_text_and_exact_color_ids() {
    let mut term = Terminal::new(4, 3);
    term.set_pixel_geometry(4, 3);
    let cursor = term.grid().cursor();
    term.handle_graphics_command(b"a=T,U=1,i=42,p=7,c=2,r=2,f=32,s=2,v=2;AQIDBAUGBwgJCgsMDQ4PEA==");
    assert_eq!(term.grid().cursor().row, cursor.row);
    assert_eq!(term.grid().cursor().col, cursor.col);
    assert_eq!(term.graphics_placements().count(), 0);

    term.process(
            "\x1b[38;5;42;58;5;7m\u{10eeee}\u{0305}\u{0305}\u{10eeee}\r\n\u{10eeee}\u{030d}\u{10eeee}\x1b[0m"
                .as_bytes(),
        );
    let first_cell = term.grid().cell(0, 0).unwrap();
    assert_eq!(first_cell.fg_spec, ColorSpec::Indexed(42));
    assert_eq!(first_cell.underline_color_spec, Some(ColorSpec::Indexed(7)));

    let presentations = term.presentation_images();
    assert_eq!(presentations.len(), 2);
    let positions: Vec<_> = presentations
        .iter()
        .map(|presentation| {
            (
                presentation.placement.col,
                presentation.placement.row,
                presentation.source_uv,
            )
        })
        .collect();
    assert_eq!(
        positions,
        vec![
            (0, 0, Some([0.0, 0.0, 1.0, 0.5])),
            (0, 1, Some([0.0, 0.5, 1.0, 1.0])),
        ]
    );

    // The displayed instances are derived from the underlying cells, not
    // protocol placements, so editing text removes only that tile.
    term.set_cursor_position(1, 1);
    term.process(b"X");
    assert_eq!(term.presentation_images().len(), 2);
    term.handle_graphics_command(b"a=d,d=a");
    assert_eq!(term.presentation_images().len(), 2);
    term.handle_graphics_command(b"a=d,d=i,i=42,p=7");
    assert!(term.presentation_images().is_empty());
    assert!(term.get_image(ImageId(42)).is_some());
}

#[test]
fn kitty_unicode_placeholders_follow_scrollback_and_screen_ownership() {
    let mut term = Terminal::new(3, 2);
    term.set_pixel_geometry(3, 2);
    term.handle_graphics_command(b"a=T,U=1,i=9,c=1,r=1,f=32,s=1,v=1;AQIDBA==");
    term.process("\x1b[38;5;9m\u{10eeee}\x1b[0m\r\none\r\ntwo".as_bytes());
    term.grid_mut().set_scroll_offset(1);
    let history = term.presentation_images();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].placement.row, -1);

    term.grid_mut().set_scroll_offset(0);
    term.process(b"\x1b[?1049h");
    term.process("\x1b[38;5;9m\u{10eeee}\x1b[0m".as_bytes());
    assert!(term.presentation_images().is_empty());
    term.process(b"\x1b[?1049l");
    term.grid_mut().set_scroll_offset(1);
    assert_eq!(term.presentation_images().len(), 1);

    term.grid_mut().set_scroll_offset(0);
    term.process(b"\x1b[?1049h");
    term.handle_graphics_command(b"a=T,U=1,i=10,c=1,r=1,f=32,s=1,v=1;AQIDBA==");
    term.process("\x1b[38;5;10m\u{10eeee}\x1b[0m".as_bytes());
    assert_eq!(term.presentation_images().len(), 1);
    term.process(b"\x1b[?1049l\x1b[?1049h");
    term.process("\x1b[38;5;10m\u{10eeee}\x1b[0m".as_bytes());
    assert!(term.presentation_images().is_empty());
}

#[test]
fn kitty_animation_revisions_reach_presentation_without_leaking_through_sync() {
    let mut term = Terminal::new(4, 2);
    term.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=71,c=1,r=1,C=1;AQIDBA==");
    term.handle_graphics_command(b"a=f,f=32,s=1,v=1,i=71,z=10;BQYHCA==");
    term.handle_graphics_command(b"a=a,i=71,r=1,z=10,s=3");

    let root = term.presentation_images()[0].image.clone();
    assert_eq!(root.data.as_ref(), &[1, 2, 3, 4]);
    let deadline = term.graphics_animation_deadline().unwrap();
    term.process(b"\x1b[?2026h");
    assert!(term.advance_graphics_animations(deadline));
    let frozen = term.presentation_images()[0].image.clone();
    assert_eq!(frozen.data.as_ref(), &[1, 2, 3, 4]);
    assert_eq!(frozen.revision, root.revision);

    term.process(b"\x1b[?2026l");
    let animated = term.presentation_images()[0].image.clone();
    assert_eq!(animated.data.as_ref(), &[5, 6, 7, 8]);
    assert_ne!(animated.revision, root.revision);
}

#[test]
fn live_domain_graphics_relay_restores_decoded_namespace_once_without_replies() {
    let pixels = STANDARD.encode([
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
    ]);
    let alternate = STANDARD.encode([
        0, 0, 0, 0, 32, 32, 32, 128, 64, 64, 64, 255, 96, 96, 96, 255,
    ]);
    let mut source = Terminal::new(8, 3);
    source.process(format!("\x1b_Ga=t,f=32,s=2,v=2,I=44;{pixels}\x1b\\").as_bytes());
    source.process(b"\x1b[2;3H\x1b_Ga=p,I=44,p=5,c=2,r=2,C=1\x1b\\");
    source.process(format!("\x1b_Ga=f,f=32,s=2,v=2,i=1,r=2,z=80;{alternate}\x1b\\").as_bytes());
    source.process(b"\x1b_Ga=a,i=1,s=1,c=2\x1b\\");
    source.drain_events();
    let expected = source.presentation_images();
    assert_eq!(expected.len(), 1);
    assert_eq!(
        expected[0].image.data.as_ref(),
        STANDARD.decode(alternate).unwrap()
    );

    let snapshot = source.graphics_relay_snapshot(Instant::now());
    let mut chunks = Vec::new();
    snapshot
        .for_each_escape_chunk(|chunk| {
            chunks.push(chunk);
            true
        })
        .unwrap();
    assert!(chunks.iter().all(|chunk| chunk.len() < 64 * 1024));

    let mut replica = Terminal::new(8, 3);
    replica.set_graphics_relay_snapshot_allowed(true);
    for chunk in &chunks {
        for fragment in chunk.as_bytes().chunks(7) {
            replica.process(fragment);
        }
    }
    let restored = replica.presentation_images();
    assert_eq!(restored.len(), expected.len());
    assert_eq!(
        restored[0].placement.image_id,
        expected[0].placement.image_id
    );
    assert_eq!(restored[0].placement.placement_id, Some(5));
    assert_eq!(
        (restored[0].placement.col, restored[0].placement.row),
        (2, 1)
    );
    assert_eq!(restored[0].image.data, expected[0].image.data);
    assert_eq!(restored[0].image.revision, expected[0].image.revision);
    assert!(replica.drain_events().is_empty());

    // Image-number aliases remain usable by later live PTY output.
    replica.process(b"\x1b[1;1H\x1b_Ga=p,I=44,p=6,C=1\x1b\\");
    assert_eq!(replica.presentation_images().len(), 2);

    // The internal import permission consumed itself at commit. A later
    // application-emitted begin marker cannot clear the restored state.
    replica.process(b"\x1b_TG;begin;0;cbf29ce484222325\x1b\\");
    assert_eq!(replica.presentation_images().len(), 2);

    let mut ordinary = Terminal::new(8, 3);
    for chunk in &chunks {
        ordinary.process(chunk.as_bytes());
    }
    // Standard quiet Kitty pixel loads are harmless, but the private
    // placement/counter commit is ignored outside an opted-in relay pane.
    assert!(ordinary.presentation_images().is_empty());
    assert!(ordinary.drain_events().is_empty());

    let mut rejected = Terminal::new(8, 3);
    rejected.set_graphics_relay_snapshot_allowed(true);
    rejected.process(b"\x1b_TG;begin;1;0000000000000000\x1b\\");
    rejected.process(b"\x1b_TG;chunk;%%%\x1b\\");
    for chunk in &chunks {
        rejected.process(chunk.as_bytes());
    }
    assert!(rejected.presentation_images().is_empty());
    assert!(rejected.drain_events().is_empty());
}

#[test]
fn live_domain_graphics_relay_restores_virtual_and_relative_placement_identity() {
    const PLACEHOLDERS: &str = "\x1b[38;5;42;58;5;7m\u{10eeee}\u{0305}\u{0305}\u{10eeee}\r\n\u{10eeee}\u{030d}\u{10eeee}\x1b[0m";
    let mut source = Terminal::new(6, 3);
    source.set_pixel_geometry(6, 3);
    source
        .handle_graphics_command(b"a=T,U=1,i=42,p=7,c=2,r=2,f=32,s=2,v=2;AQIDBAUGBwgJCgsMDQ4PEA==");
    source.process(PLACEHOLDERS.as_bytes());
    source.process(b"\x1b[2;3H\x1b_Ga=p,i=42,p=8,c=2,r=2,C=1\x1b\\");
    source.process(b"\x1b_Ga=p,i=42,p=9,P=42,Q=8,H=-1,V=1,c=2,r=2,C=1\x1b\\");
    source.drain_events();
    let expected = source.presentation_images();
    assert_eq!(expected.len(), 4);

    let snapshot = source.graphics_relay_snapshot(Instant::now());
    let mut replica = Terminal::new(6, 3);
    replica.set_pixel_geometry(6, 3);
    replica.process(PLACEHOLDERS.as_bytes());
    replica.set_graphics_relay_snapshot_allowed(true);
    snapshot
        .for_each_escape_chunk(|chunk| {
            replica.process(chunk.as_bytes());
            true
        })
        .unwrap();
    let restored = replica.presentation_images();
    assert_eq!(restored.len(), expected.len());
    for (restored, expected) in restored.iter().zip(&expected) {
        assert_eq!(restored.placement.image_id, expected.placement.image_id);
        assert_eq!(
            restored.placement.placement_id,
            expected.placement.placement_id
        );
        assert_eq!(
            (restored.placement.col, restored.placement.row),
            (expected.placement.col, expected.placement.row)
        );
        assert_eq!(restored.pixel_rect, expected.pixel_rect);
        assert_eq!(restored.source_uv, expected.source_uv);
    }

    // Deleting the restored named parent still cascades to its relative
    // child, proving that stable internal placement UIDs survived.
    replica.handle_graphics_command(b"a=d,d=i,i=42,p=8");
    assert_eq!(replica.presentation_images().len(), 2);
}

#[test]
fn kitty_unicode_placeholders_follow_reflow_and_synchronized_output() {
    let mut term = Terminal::new(4, 2);
    term.set_pixel_geometry(4, 2);
    term.handle_graphics_command(b"a=T,U=1,i=5,c=1,r=1,f=32,s=1,v=1;AQIDBA==");
    term.process("AB\x1b[38;5;5m\u{10eeee}\x1b[0m".as_bytes());
    assert_eq!(
        (
            term.presentation_images()[0].placement.col,
            term.presentation_images()[0].placement.row,
        ),
        (2, 0)
    );

    term.resize(2, 3);
    let reflowed = term.presentation_images();
    assert_eq!(reflowed.len(), 1);
    assert_eq!(
        (reflowed[0].placement.col, reflowed[0].placement.row),
        (0, 1)
    );

    term.process(b"\x1b[?2026h");
    term.process(b"\x1b[2;1HX");
    assert_eq!(term.presentation_images().len(), 1);
    term.process(b"\x1b[?2026l");
    assert!(term.presentation_images().is_empty());
}

#[test]
fn kitty_relative_placements_follow_unicode_parent_text_and_lifetime() {
    let mut term = Terminal::new(10, 6);
    term.handle_graphics_command(b"a=T,U=1,i=42,p=7,c=1,r=1,f=32,s=1,v=1;AQIDBA==");
    term.handle_graphics_command(b"a=t,i=8,f=32,s=1,v=1;BQYHCA==");
    let cursor_before = term.grid().cursor();
    term.handle_graphics_command(b"a=p,i=8,p=8,P=42,Q=7,H=1,V=2,c=1,r=1");
    assert_eq!(
        (term.grid().cursor().col, term.grid().cursor().row),
        (cursor_before.col, cursor_before.row)
    );
    assert!(term.presentation_images().is_empty());

    term.process(b"\x1b[4;5H\x1b[38;5;42;58;5;7m");
    term.process("\u{10eeee}\u{0305}\u{0305}".as_bytes());
    let positions = |term: &Terminal| {
        term.presentation_images()
            .into_iter()
            .map(|image| {
                (
                    image.placement.image_id,
                    image.placement.col,
                    image.placement.row,
                )
            })
            .collect::<HashSet<_>>()
    };
    assert_eq!(
        positions(&term),
        HashSet::from([(ImageId(42), 4, 3), (ImageId(8), 5, 5)])
    );

    // The virtual parent's origin is the independent minimum x/y across
    // all live placeholder cells that refer to its prototype.
    term.process(b"\x1b[5;3H");
    term.process("\u{10eeee}\u{0305}\u{0305}".as_bytes());
    assert!(positions(&term).contains(&(ImageId(8), 3, 5)));

    term.handle_graphics_command(b"a=d,d=i,i=42,p=7");
    assert!(term.presentation_images().is_empty());
    assert!(term.get_image(ImageId(8)).is_none());
}

#[test]
fn kitty_image_numbers_allocate_reply_and_place_at_the_terminal_cursor() {
    let mut term = Terminal::new(8, 4);
    term.set_cursor_position(3, 2);
    term.handle_graphics_command(b"a=T,I=13,N=1,f=32,s=1,v=1,p=7,c=1,r=1,C=1;AQIDBA==");

    let placement = term.graphics_placements().next().unwrap();
    assert_eq!(placement.image_id, ImageId(1));
    assert_eq!(placement.placement_id, Some(7));
    assert_eq!((placement.col, placement.row), (1, 2));
    assert_eq!(
        term.get_image(ImageId(1)).unwrap().data.as_ref(),
        [1, 2, 3, 4]
    );
    assert!(term.drain_events().iter().any(|event| matches!(
        event,
        TerminalEvent::PtyWrite(bytes)
            if bytes == b"\x1b_Gi=1,I=13,p=7;OK\x1b\\"
    )));
}

#[test]
fn kitty_placements_follow_screen_scroll_clear_and_alternate_lifecycle() {
    let mut term = Terminal::new(8, 4);
    term.set_cursor_position(3, 2);
    term.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=1,c=1,r=1,C=1;AQIDBA==");
    assert_eq!(term.graphics_placements().next().unwrap().row, 2);

    term.scroll_up(1);
    assert_eq!(term.graphics_placements().next().unwrap().row, 1);
    term.enter_alt_screen();
    assert!(term.graphics_placements().next().is_none());
    term.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=2,c=1,r=1,C=1;AQIDBA==");
    assert_eq!(term.graphics_placements().count(), 1);
    term.exit_alt_screen();
    assert_eq!(
        term.graphics_placements().next().unwrap().image_id,
        ImageId(1)
    );
    term.enter_alt_screen();
    assert!(term.graphics_placements().next().is_none());
    term.exit_alt_screen();

    term.erase_in_display(2);
    assert!(term.graphics_placements().next().is_none());
    // ED2 clears visibility but leaves reusable image data intact.
    assert!(term.get_image(ImageId(1)).is_some());
}

#[test]
fn oversized_kitty_apc_is_bounded_and_reports_e2big() {
    let mut term = Terminal::new(8, 4);
    let mut sequence = b"\x1b_Ga=t,i=3;".to_vec();
    sequence.extend(std::iter::repeat_n(b'A', MAX_KITTY_APC_BYTES + 1));
    sequence.extend_from_slice(b"\x1b\\");
    term.process(&sequence);
    assert!(term.drain_events().iter().any(|event| matches!(
        event,
        TerminalEvent::PtyWrite(bytes) if bytes.starts_with(b"\x1b_Gi=0;E2BIG:")
    )));
    assert!(term.get_image(ImageId(3)).is_none());
}

#[test]
fn current_kitty_128_kib_chunks_cross_apc_framing_without_replies() {
    let mut term = Terminal::new(120, 40);
    let mut first = b"\x1b_Ga=T,q=2,f=32,s=256,v=192,m=1,U=1,c=103,r=52,i=77;".to_vec();
    first.extend(std::iter::repeat_n(b'A', 128 * 1024));
    first.extend_from_slice(b"\x1b\\");
    term.process(&first);

    let mut final_chunk = b"\x1b_Gq=2,m=0;".to_vec();
    final_chunk.extend(std::iter::repeat_n(b'A', 128 * 1024));
    final_chunk.extend_from_slice(b"\x1b\\");
    term.process(&final_chunk);

    assert_eq!(
        term.get_image(ImageId(77)).unwrap().data.len(),
        256 * 192 * 4
    );
    assert!(term.drain_events().is_empty());
}

#[test]
fn current_kitty_natural_placement_uses_image_pixels_and_moves_the_cursor() {
    let mut term = Terminal::new(100, 38);
    term.set_pixel_geometry(800, 760);
    let pixels = vec![0x7f; 80 * 80 * 4];
    let payload = format!(
        "\x1b_Ga=T,q=2,f=32,s=80,v=80;{}\x1b\\",
        STANDARD.encode(pixels)
    );

    term.process(payload.as_bytes());

    let placement = term.graphics_placements().next().unwrap();
    assert_eq!((placement.width, placement.height), (Some(10), Some(4)));
    assert_eq!(
        (placement.pixel_width, placement.pixel_height),
        (Some(80), Some(80))
    );
    assert_eq!(
        (term.grid().cursor().col, term.grid().cursor().row),
        (10, 4)
    );
    assert_eq!(term.presentation_images().len(), 1);
    assert!(term.drain_events().is_empty());
}

#[test]
fn kitty_q2_suppresses_parse_and_overflow_errors() {
    let mut term = Terminal::new(8, 4);
    term.process(b"\x1b_Ga=T,q=2;%%%\x1b\\");

    let mut oversized = b"\x1b_Ga=t,q=2,i=3;".to_vec();
    oversized.extend(std::iter::repeat_n(b'A', MAX_KITTY_APC_BYTES + 1));
    oversized.extend_from_slice(b"\x1b\\");
    term.process(&oversized);

    assert!(term.drain_events().is_empty());
    assert!(term.get_image(ImageId(3)).is_none());
}

#[test]
fn kitty_placements_remain_addressable_in_primary_scrollback() {
    let mut term = Terminal::new(8, 4);
    term.set_cursor_position(1, 1);
    term.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=6,c=1,r=1,C=1;AQIDBA==");
    term.scroll_up(1);
    let placement = term.graphics_placements().next().unwrap();
    assert_eq!(placement.row, -1);
    assert_eq!(term.grid().scrollback_len(), 1);
    term.grid_mut().set_scroll_offset(1);
    assert_eq!(term.graphics_placements().next().unwrap().row, -1);
}

#[test]
fn sixel_and_iterm2_framing_render_per_terminal_without_pty_leakage() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, 2, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255])
            .unwrap();
    }
    let encoded = STANDARD.encode(png_bytes);
    let osc =
        format!("\x1b]1337;File=inline=1;width=2;height=1;preserveAspectRatio=0:{encoded}\x07");

    let mut first_pane = Terminal::new(20, 10);
    first_pane.set_pixel_geometry(200, 100);
    first_pane.process(b"\x1b_Ga=T,f=32,s=1,v=1,i=7,C=1;AQIDBA==\x1b\\");
    first_pane.process(b"\x1bP0;1q\"1;1;4;6#1;2;100;0;0!4~\x1b\\");
    first_pane.process(osc.as_bytes());

    assert_eq!(first_pane.graphics_placement_count(), 3);
    assert_eq!(first_pane.graphics_image_count(), 3);
    assert!(first_pane.graphics_memory_usage() <= 64 * 1024 * 1024);
    assert!(first_pane.graphics_placements().any(|placement| {
        placement.pixel_width == Some(4) && placement.pixel_height == Some(6)
    }));
    assert!(first_pane.graphics_placements().any(|placement| {
        placement.pixel_width == Some(20) && placement.pixel_height == Some(10)
    }));

    let second_pane = Terminal::new(20, 10);
    assert_eq!(second_pane.graphics_placement_count(), 0);
    assert_eq!(second_pane.graphics_image_count(), 0);
    assert!(first_pane.drain_events().iter().all(
        |event| matches!(event, TerminalEvent::PtyWrite(bytes) if bytes.starts_with(b"\x1b_G"))
    ));
}

#[test]
fn fragmented_sixel_aspect_controls_display_footprint_and_cursor_advance() {
    let mut term = Terminal::new(20, 10);
    term.set_pixel_geometry(40, 30);
    term.set_cursor_position(2, 4);

    term.process(b"\x1bP7q\"2;");
    assert_eq!(term.graphics_placement_count(), 0);
    term.process(b"1;2;6!2~\x1b");
    assert_eq!(term.graphics_placement_count(), 0);
    term.process(b"\\");

    let placement = term.graphics_placements().next().unwrap();
    assert_eq!((placement.col, placement.row), (3, 1));
    assert_eq!((placement.width, placement.height), (Some(1), Some(4)));
    assert_eq!(
        (placement.pixel_width, placement.pixel_height),
        (Some(2), Some(12))
    );
    let image = term.get_image(placement.image_id).unwrap();
    assert_eq!((image.width, image.height), (2, 6));
    assert_eq!(term.graphics_memory_usage(), 2 * 6 * 4);
    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (5, 3));
}

#[test]
fn repeated_sixel_images_remain_inside_shared_resource_limits() {
    let mut term = Terminal::new(8, 4);
    term.set_pixel_geometry(80, 40);
    for _ in 0..300 {
        term.process(b"\x1bP0;1q!4~\x1b\\");
    }
    assert!(term.graphics_image_count() <= 256);
    assert!(term.graphics_placement_count() <= 256);
    assert!(term.graphics_memory_usage() <= 64 * 1024 * 1024);

    term.erase_in_display(2);
    assert_eq!(term.graphics_image_count(), 0);
    assert_eq!(term.graphics_memory_usage(), 0);
}

#[test]
fn selective_erase_preserves_protected_unicode_and_cell_style() {
    let mut term = Terminal::new(8, 3);
    term.process(b"\x1b[1\"qA\x1b[0\"qB\x1b[1\"q");
    term.process("界".as_bytes());
    term.process(b"\x1b[2\"qC\x1b[48;2;1;2;3m\x1b[?2K");

    assert_eq!((term.grid().cursor().row, term.grid().cursor().col), (0, 5));
    assert_eq!(term.rows_text(0, 0), "A 界");
    for col in [0, 2, 3] {
        assert!(
            term.grid()
                .cell(0, col)
                .unwrap()
                .flags
                .contains(CellFlags::PROTECTED)
        );
    }
    assert!(term.grid().cell(0, 2).unwrap().is_wide_char());
    assert!(term.grid().cell(0, 3).unwrap().is_wide_char_spacer());
    for col in [1, 4, 6, 7] {
        let cell = term.grid().cell(0, col).unwrap();
        assert!(cell.content.is_empty());
        assert_eq!(cell.bg_spec, ColorSpec::Default);
        assert!(!cell.flags.contains(CellFlags::PROTECTED));
    }

    // Invalid, plural, and subparameter DECSCA forms are ignored. The
    // following character therefore remains protected.
    term.process(b"\x1b[1\"q\x1b[0m\x1b[9\"q\x1b[1;0\"q\x1b[1:0\"q\x1b7\x1b[0\"q\x1b8D");
    assert!(
        term.grid()
            .cell(0, 5)
            .unwrap()
            .flags
            .contains(CellFlags::PROTECTED)
    );

    // Ordinary EL ignores selective protection, and RIS returns DECSCA
    // to erasable without changing the semantics of SGR.
    term.process(b"\x1b[2K\x1b[1\"q\x1bcE");
    let cell = term.grid().cell(0, 0).unwrap();
    assert_eq!(cell.content.char(), Some('E'));
    assert!(!cell.flags.contains(CellFlags::PROTECTED));

    // Protection is durable cell state, not parser-only state: compact
    // scrollback and subsequent reflow retain it.
    let mut history = Terminal::new(4, 2);
    history.process(b"\x1b[1\"qA\r\n\x1b[0\"qB\r\nC");
    history.resize(2, 2);
    let oldest = -(history.grid().scrollback_len() as i32);
    let retained = (oldest..history.rows() as i32).any(|row| {
        history.grid().row_at_offset(row).is_some_and(|row| {
            row.iter().any(|cell| {
                cell.content.char() == Some('A') && cell.flags.contains(CellFlags::PROTECTED)
            })
        })
    });
    assert!(retained);
}

#[test]
fn rectangular_fill_and_erase_apply_attributes_clipping_and_origin() {
    let mut term = Terminal::new(8, 4);
    term.process(b"abcdefgh\r\nijklmnop\r\nqrstuvwx\r\nyz012345");
    term.process(b"\x1b[2;7H\x1b[1;38;5;2;48;2;1;2;3m\x1b[1\"q");
    let cursor = term.grid().cursor();
    term.process(b"\x1b[88;2;2;3;4$x");

    assert_eq!(
        (term.grid().cursor().row, term.grid().cursor().col),
        (cursor.row, cursor.col)
    );
    assert_eq!(term.grid().cursor().wrap_pending, cursor.wrap_pending);
    assert_eq!(
        term.rows_text(0, 3),
        "abcdefgh\niXXXmnop\nqXXXuvwx\nyz012345"
    );
    for row in 1..=2 {
        for col in 1..=3 {
            let cell = term.grid().cell(row, col).unwrap();
            assert_eq!(cell.content.char(), Some('X'));
            assert!(cell.attrs.contains(CellAttrs::BOLD));
            assert_eq!(cell.fg_spec, ColorSpec::Indexed(2));
            assert_eq!(cell.bg_spec, ColorSpec::TrueColor(Color::rgb(1, 2, 3)));
            assert!(cell.flags.contains(CellFlags::PROTECTED));
        }
    }

    // Invalid fill characters, reversed rectangles, extra parameters, and
    // subparameters are ignored without moving the cursor.
    let before = term.rows_text(0, 3);
    term.process(b"\x1b[127;1;1;4;8$x\x1b[88;3;3;2;4$x\x1b[88;1;1;2;2;9$x\x1b[88:1;1;1;2;2$x");
    assert_eq!(term.rows_text(0, 3), before);
    assert_eq!(
        (term.grid().cursor().row, term.grid().cursor().col),
        (cursor.row, cursor.col)
    );

    // DECERA ignores protection, applies BCE, and clips oversized bounds.
    term.process(b"\x1b[48;5;5m\x1b[2;3;99;99$z");
    assert_eq!(term.rows_text(0, 3), "abcdefgh\niX\nqX\nyz");
    for row in 1..4 {
        for col in 2..8 {
            let cell = term.grid().cell(row, col).unwrap();
            assert!(cell.content.is_empty());
            assert_eq!(cell.bg_spec, ColorSpec::Indexed(5));
            assert!(!cell.flags.contains(CellFlags::PROTECTED));
        }
    }

    // DECOM makes rectangle coordinates relative to both margin pairs.
    term.process(b"\x1b[0m\x1b[2;4r\x1b[?69h\x1b[3;7s\x1b[?6h\x1b[90;1;1;2;2$x");
    assert_eq!(term.grid().cell(1, 2).unwrap().content.char(), Some('Z'));
    assert_eq!(term.grid().cell(2, 3).unwrap().content.char(), Some('Z'));
    assert_ne!(term.grid().cell(0, 0).unwrap().content.char(), Some('Z'));
    assert_ne!(term.grid().cell(3, 4).unwrap().content.char(), Some('Z'));

    let mut defaulted = Terminal::new(3, 2);
    defaulted.process(b"\x1b(0\x1b[113$x");
    assert_eq!(defaulted.rows_text(0, 1), "───\n───");
    defaulted.process(b"\x1b[$z");
    assert_eq!(defaulted.rows_text(0, 1), "\n");
    defaulted.process(b"\x1b[160;1;1;1;1$x");
    assert_eq!(
        defaulted.grid().cell(0, 0).unwrap().content.char(),
        Some('\u{a0}')
    );
}

#[test]
fn rectangular_selective_erase_retains_cell_attributes_and_wide_integrity() {
    let mut term = Terminal::new(8, 2);
    term.process(b"\x1b[1;31mA\x1b[1\"qB\x1b[0\"q");
    term.process("界".as_bytes());
    term.process(b"C");
    let cursor = term.grid().cursor();
    term.process(b"\x1b[1;1;1;4${");

    assert_eq!(
        (term.grid().cursor().row, term.grid().cursor().col),
        (cursor.row, cursor.col)
    );
    assert_eq!(term.grid().cursor().wrap_pending, cursor.wrap_pending);
    let erased = term.grid().cell(0, 0).unwrap();
    assert!(erased.content.is_empty());
    assert!(erased.attrs.contains(CellAttrs::BOLD));
    assert_eq!(erased.fg_spec, ColorSpec::Indexed(1));
    assert_eq!(term.grid().cell(0, 1).unwrap().content.char(), Some('B'));
    assert!(
        term.grid()
            .cell(0, 1)
            .unwrap()
            .flags
            .contains(CellFlags::PROTECTED)
    );
    for col in 2..=3 {
        let cell = term.grid().cell(0, col).unwrap();
        assert!(cell.content.is_empty());
        assert!(cell.attrs.contains(CellAttrs::BOLD));
        assert!(!cell.is_wide_char());
        assert!(!cell.is_wide_char_spacer());
    }
    assert_eq!(term.grid().cell(0, 4).unwrap().content.char(), Some('C'));
}

#[test]
fn rectangular_copy_is_overlap_safe_clipped_and_copies_complete_state() {
    let mut term = Terminal::new(8, 3);
    term.process(b"\x1b[1;34mAB\x1b[1\"qC\x1b[0\"qDEFGH");
    let cursor = term.grid().cursor();
    term.process(b"\x1b[1;1;1;6;1;1;3;1$v");
    assert_eq!(term.rows_text(0, 0), "ABABCDEF");
    assert_eq!(
        (term.grid().cursor().row, term.grid().cursor().col),
        (cursor.row, cursor.col)
    );
    assert_eq!(term.grid().cursor().wrap_pending, cursor.wrap_pending);
    assert!(
        term.grid()
            .cell(0, 4)
            .unwrap()
            .flags
            .contains(CellFlags::PROTECTED)
    );
    assert_eq!(
        term.grid().cell(0, 2).unwrap().fg_spec,
        ColorSpec::Indexed(4)
    );

    // Reverse-direction overlap must also read from the pre-copy snapshot.
    term.process(b"\x1b[1;3;1;8;9;1;1;7$v");
    assert_eq!(term.rows_text(0, 0), "ABCDEFEF");

    // Destination coordinates beyond the sole page clamp to its last cell;
    // page parameters likewise resolve to the only available page.
    term.process(b"\x1b[1;1;3;8;42;99;99;77$v");
    assert_eq!(term.grid().cell(2, 7).unwrap().content.char(), Some('A'));

    // A source boundary that intersects a wide pair cannot leave an orphan
    // half at the destination or damage the cursor.
    let mut wide = Terminal::new(6, 2);
    wide.process("A界BCD".as_bytes());
    let cursor = wide.grid().cursor();
    wide.process(b"\x1b[1;3;1;5;1;2;2;1$v");
    assert_eq!(
        (wide.grid().cursor().row, wide.grid().cursor().col),
        (cursor.row, cursor.col)
    );
    assert_eq!(wide.grid().cursor().wrap_pending, cursor.wrap_pending);
    for row in 0..wide.rows() {
        for col in 0..wide.cols() {
            let cell = wide.grid().cell(row, col).unwrap();
            if cell.is_wide_char() {
                assert!(
                    wide.grid()
                        .cell(row, col + 1)
                        .unwrap()
                        .is_wide_char_spacer()
                );
            }
            if cell.is_wide_char_spacer() {
                assert!(col > 0 && wide.grid().cell(row, col - 1).unwrap().is_wide_char());
            }
        }
    }
}

#[test]
fn rectangular_text_operations_share_image_placement_geometry() {
    let mut erased = Terminal::new(8, 4);
    erased.process(b"\x1b_Ga=T,f=32,s=1,v=1,i=7,c=4,r=4,C=1;AQIDBA==\x1b\\");
    assert_eq!(erased.graphics_placement_count(), 1);
    erased.process(b"\x1b[88;2;2;3;3$x");
    let fragments: Vec<_> = erased.graphics_placements().collect();
    assert_eq!(fragments.len(), 4);
    for row in 1..3_i64 {
        for col in 1..3_u32 {
            assert!(fragments.iter().all(|placement| {
                col < placement.col
                    || col >= placement.col + placement.width.unwrap_or(1)
                    || row < placement.row
                    || row >= placement.row + i64::from(placement.height.unwrap_or(1))
            }));
        }
    }

    let mut copied = Terminal::new(8, 4);
    copied.process(b"\x1b_Ga=T,f=32,s=1,v=1,i=8,c=2,r=2,C=1;AQIDBA==\x1b\\");
    copied.process(b"\x1b[1;1;2;2;1;3;5;1$v");
    assert_eq!(copied.graphics_placement_count(), 2);
    assert!(copied.graphics_placements().any(|placement| {
        placement.col == 4
            && placement.row == 2
            && placement.width == Some(2)
            && placement.height == Some(2)
    }));

    // DECSERA is character-selective; image pixels do not carry DECSCA.
    copied.process(b"\x1b[3;5;4;6${");
    assert_eq!(copied.graphics_placement_count(), 2);
}

#[test]
fn attribute_change_extent_obeys_stream_rectangle_origin_and_reset_lifecycle() {
    let mut term = Terminal::new(6, 3);
    term.process(b"AB\x1b[2;3HCD\x1b[3;5HEF");
    let cursor = term.grid().cursor();

    // Stream mode is the default. It wraps over the full page width but
    // changes only cells that have actually been drawn.
    term.process(b"\x1b[1;2;3;5;1$r");
    for (row, col) in [(0, 1), (1, 2), (1, 3), (2, 4)] {
        assert!(
            term.grid()
                .cell(row, col)
                .unwrap()
                .attrs
                .contains(CellAttrs::BOLD)
        );
    }
    for (row, col) in [(0, 0), (2, 5)] {
        assert!(
            !term
                .grid()
                .cell(row, col)
                .unwrap()
                .attrs
                .contains(CellAttrs::BOLD)
        );
    }
    let untouched = term.grid().cell(1, 0).unwrap();
    assert!(untouched.content.is_empty());
    assert!(untouched.attrs.is_empty());

    // Exact rectangle mode changes only the requested columns and turns
    // never-drawn cells into explicit spaces. A later stream operation
    // therefore recognizes those cells as drawn.
    term.process(b"\x1b[2*x\x1b[1;2;3;5;7$r");
    let materialized = term.grid().cell(1, 1).unwrap();
    assert_eq!(materialized.content.char(), Some(' '));
    assert!(materialized.attrs.contains(CellAttrs::INVERSE));
    assert!(term.grid().cell(1, 0).unwrap().content.is_empty());
    term.process(b"\x1b[1*x\x1b[1;2;3;5;8$r");
    assert!(
        term.grid()
            .cell(1, 1)
            .unwrap()
            .attrs
            .contains(CellAttrs::HIDDEN)
    );

    assert_eq!(
        (term.grid().cursor().row, term.grid().cursor().col),
        (cursor.row, cursor.col)
    );
    assert_eq!(term.grid().cursor().wrap_pending, cursor.wrap_pending);

    // DECOM affects both endpoints before stream wrapping. Horizontal
    // margins do not constrain the wrapped portion itself.
    let mut origin = Terminal::new(8, 4);
    origin.process(b"abcdefgh\r\nijklmnop\r\nqrstuvwx\r\nyz012345");
    origin.process(b"\x1b[2;4r\x1b[?69h\x1b[3;7s\x1b[?6h\x1b[1;2;2;3;1$r");
    assert!(
        !origin
            .grid()
            .cell(0, 0)
            .unwrap()
            .attrs
            .contains(CellAttrs::BOLD)
    );
    assert!(
        !origin
            .grid()
            .cell(1, 2)
            .unwrap()
            .attrs
            .contains(CellAttrs::BOLD)
    );
    assert!(
        origin
            .grid()
            .cell(1, 3)
            .unwrap()
            .attrs
            .contains(CellAttrs::BOLD)
    );
    assert!(
        origin
            .grid()
            .cell(1, 7)
            .unwrap()
            .attrs
            .contains(CellAttrs::BOLD)
    );
    assert!(
        origin
            .grid()
            .cell(2, 0)
            .unwrap()
            .attrs
            .contains(CellAttrs::BOLD)
    );
    assert!(
        origin
            .grid()
            .cell(2, 4)
            .unwrap()
            .attrs
            .contains(CellAttrs::BOLD)
    );
    assert!(
        !origin
            .grid()
            .cell(2, 5)
            .unwrap()
            .attrs
            .contains(CellAttrs::BOLD)
    );

    assert_eq!(origin.attribute_change_extent_parameter(), 1);
    origin.process(b"\x1b[2*x\x1b[3*x\x1b[2;1*x\x1b[2:1*x");
    assert_eq!(origin.attribute_change_extent_parameter(), 2);
    origin.process(b"\x1b[!p");
    assert_eq!(origin.attribute_change_extent_parameter(), 2);
    origin.process(b"\x1bc");
    assert_eq!(origin.attribute_change_extent_parameter(), 1);
}

#[test]
fn attribute_change_and_reverse_apply_only_the_documented_rendition_subset() {
    let mut term = Terminal::new(5, 2);
    term.process(
        b"\x1b]8;;https://example.com\x07\x1b[1\"q\x1b[1;2;4:3;6;7;8;31;44mX\x1b]8;;\x07\x1b[0\"q",
    );
    let cursor = term.grid().cursor();
    let original = term.grid().cell(0, 0).unwrap().clone();
    assert!(
        original
            .flags
            .contains(CellFlags::PROTECTED | CellFlags::HYPERLINK)
    );

    term.process(b"\x1b[1;1;1;1;0$r");
    let attrs = term.grid().cell(0, 0).unwrap().attrs;
    assert!(attrs.contains(CellAttrs::DIM | CellAttrs::RAPID_BLINK | CellAttrs::HIDDEN));
    assert!(!attrs.intersects(
        CellAttrs::BOLD | CellAttrs::ALL_UNDERLINES | CellAttrs::BLINK | CellAttrs::INVERSE
    ));

    term.process(b"\x1b[1;1;1;1;1;4;5;7;8$r");
    let attrs = term.grid().cell(0, 0).unwrap().attrs;
    assert!(attrs.contains(
        CellAttrs::BOLD
            | CellAttrs::UNDERLINE
            | CellAttrs::BLINK
            | CellAttrs::RAPID_BLINK
            | CellAttrs::INVERSE
            | CellAttrs::HIDDEN
            | CellAttrs::DIM
    ));
    assert!(!attrs.intersects(
        CellAttrs::DOUBLE_UNDERLINE
            | CellAttrs::CURLY_UNDERLINE
            | CellAttrs::DOTTED_UNDERLINE
            | CellAttrs::DASHED_UNDERLINE
    ));

    term.process(b"\x1b[1;1;1;1;22;24;25;27;28$r");
    let attrs = term.grid().cell(0, 0).unwrap().attrs;
    assert_eq!(attrs, CellAttrs::DIM | CellAttrs::RAPID_BLINK);

    term.process(b"\x1b[1;1;1;1;0;8$t");
    let reversed = term.grid().cell(0, 0).unwrap().attrs;
    assert!(reversed.contains(
        CellAttrs::DIM
            | CellAttrs::RAPID_BLINK
            | CellAttrs::BOLD
            | CellAttrs::UNDERLINE
            | CellAttrs::BLINK
            | CellAttrs::INVERSE
            | CellAttrs::HIDDEN
    ));
    term.process(b"\x1b[1;1;1;1;1;1;4;4;5;5;7;7;8;8$t");
    assert_eq!(term.grid().cell(0, 0).unwrap().attrs, reversed);

    // Empty/unknown parameter lists and malformed subparameters are true
    // no-ops. Exact DECRARA materializes untouched cells without changing
    // colors, protection, hyperlinks, cursor state, or wide-cell flags.
    term.process(b"\x1b[1;1;1;1$t\x1b[1;1;1;1;99$t\x1b[1:2;1;1;1;1$r");
    let retained = term.grid().cell(0, 0).unwrap();
    assert_eq!(retained.attrs, reversed);
    assert_eq!(retained.content, original.content);
    assert_eq!(retained.fg_spec, ColorSpec::Indexed(1));
    assert_eq!(retained.bg_spec, ColorSpec::Indexed(4));
    assert_eq!(retained.hyperlink_id, original.hyperlink_id);
    assert!(
        retained
            .flags
            .contains(CellFlags::PROTECTED | CellFlags::HYPERLINK)
    );
    term.process(b"\x1b[2*x\x1b[2;2;2;2;7$t");
    let blank = term.grid().cell(1, 1).unwrap();
    assert_eq!(blank.content.char(), Some(' '));
    assert!(blank.attrs.contains(CellAttrs::INVERSE));
    assert_eq!(blank.fg_spec, ColorSpec::Default);
    assert_eq!(blank.bg_spec, ColorSpec::Default);
    assert_eq!(
        (term.grid().cursor().row, term.grid().cursor().col),
        (cursor.row, cursor.col)
    );
    assert_eq!(term.grid().cursor().wrap_pending, cursor.wrap_pending);

    let mut wide = Terminal::new(4, 1);
    wide.process("界".as_bytes());
    wide.process(b"\x1b[2*x\x1b[1;2;1;2;7$r");
    assert!(wide.grid().cell(0, 0).unwrap().is_wide_char());
    assert!(wide.grid().cell(0, 1).unwrap().is_wide_char_spacer());
}

#[test]
fn rectangular_checksum_is_policy_gated_bounded_and_vt420_compatible() {
    let mut term = Terminal::new(5, 2);
    term.process(b"A B");
    let cursor = term.grid().cursor();

    // A child cannot scrape visible cells until the embedding application
    // explicitly enables reports.
    term.process(b"\x1b[7;99;1;1;1;3*y");
    assert!(term.drain_events().is_empty());
    assert!(!term.screen_checksum_reporting());

    term.set_screen_checksum_reporting(true);
    term.process(b"\x1b[7;99;1;1;1;3*y");
    assert_eq!(
        term.drain_events(),
        vec![TerminalEvent::PtyWrite(b"\x1bP7!~FF7D\x1b\\".to_vec())]
    );

    // Reversed rectangles produce a zero report. Excess and subparameter
    // forms are malformed and cannot accidentally trigger a reply.
    term.process(b"\x1b[8;1;2;2;1;1*y\x1b[1;1;1;1;1;1;1*y\x1b[1:2;1*y");
    assert_eq!(
        term.drain_events(),
        vec![TerminalEvent::PtyWrite(b"\x1bP8!~0000\x1b\\".to_vec())]
    );
    assert_eq!(
        (term.grid().cursor().row, term.grid().cursor().col),
        (cursor.row, cursor.col)
    );
    assert_eq!(term.grid().cursor().wrap_pending, cursor.wrap_pending);

    // The host policy is not terminal state and survives both resets.
    term.process(b"\x1b[!p\x1bc\x1b[9;1;1;1;1;1*y");
    assert!(term.screen_checksum_reporting());
    assert_eq!(
        term.drain_events(),
        vec![TerminalEvent::PtyWrite(b"\x1bP9!~0000\x1b\\".to_vec())]
    );
}

#[test]
fn kitty_file_policy_defaults_off_and_survives_terminal_resets() {
    let mut term = Terminal::new(5, 2);
    assert!(!term.kitty_file_transmission());
    term.set_kitty_file_transmission(true);
    term.process(b"\x1b[!p\x1bc");
    assert!(term.kitty_file_transmission());
    term.set_kitty_file_transmission(false);
    assert!(!term.kitty_file_transmission());
}

#[test]
fn rectangular_checksum_reports_legacy_attributes_origin_and_dec_graphics() {
    let mut attributes = Terminal::new(3, 1);
    attributes.set_screen_checksum_reporting(true);
    attributes.process(b"\x1b[1\"q\x1b[1;2;3;4:2;5;6;7;8;9;38;2;1;2;3;48;5;4mA");
    attributes.process(b"\x1b[4;1;1;1;1;1*y");
    assert_eq!(
        attributes.drain_events(),
        vec![TerminalEvent::PtyWrite(b"\x1bP4!~FEC3\x1b\\".to_vec())]
    );

    // Never-drawn cells are skipped. The first drawn space is included,
    // while later ordinary spaces are omitted like DEC/xterm defaults.
    let mut blanks = Terminal::new(4, 1);
    blanks.set_screen_checksum_reporting(true);
    blanks.process(b" A B\x1b[5;1;1;1;1;4*y");
    assert_eq!(
        blanks.drain_events(),
        vec![TerminalEvent::PtyWrite(b"\x1bP5!~FF5D\x1b\\".to_vec())]
    );

    let mut origin = Terminal::new(4, 3);
    origin.set_screen_checksum_reporting(true);
    origin.process(b"ABCD\r\nEFGH\r\nIJKL");
    origin.process(b"\x1b[2;3r\x1b[?69h\x1b[2;4s\x1b[?6h\x1b[6;0;1;1;1;1*y");
    assert_eq!(
        origin.drain_events(),
        vec![TerminalEvent::PtyWrite(b"\x1bP6!~FFBA\x1b\\".to_vec())]
    );

    let mut graphics = Terminal::new(2, 1);
    graphics.set_screen_checksum_reporting(true);
    graphics.process(b"\x1b(0q_\x1b[10;1;1;1;1;1*y\x1b[11;1;1;2;1;2*y");
    assert_eq!(
        graphics.grid().cell(0, 0).unwrap().content.char(),
        Some('─')
    );
    assert_eq!(
        graphics.grid().cell(0, 1).unwrap().content.char(),
        Some(' ')
    );
    assert_eq!(
        graphics.drain_events(),
        vec![
            TerminalEvent::PtyWrite(b"\x1bP10!~FF8F\x1b\\".to_vec()),
            TerminalEvent::PtyWrite(b"\x1bP11!~FFA1\x1b\\".to_vec()),
        ]
    );

    let mut uk = Terminal::new(1, 1);
    uk.set_screen_checksum_reporting(true);
    uk.process(b"\x1b(A#\x1b[12;1;1;1;1;1*y");
    assert_eq!(uk.grid().cell(0, 0).unwrap().content.char(), Some('£'));
    assert_eq!(
        uk.drain_events(),
        vec![TerminalEvent::PtyWrite(b"\x1bP12!~FFDD\x1b\\".to_vec())]
    );

    let mut repeated = Terminal::new(2, 1);
    repeated.set_screen_checksum_reporting(true);
    repeated.process(b"\x1b(0q\x1b(B\x1b[b\x1b[13;1;1;2;1;2*y");
    assert_eq!(
        repeated.grid().cell(0, 1).unwrap().content.char(),
        Some('─')
    );
    assert_eq!(
        repeated.drain_events(),
        vec![TerminalEvent::PtyWrite(b"\x1bP13!~FF8F\x1b\\".to_vec())]
    );
}

#[test]
fn decsed_modes_are_bounded_and_reject_malformed_requests() {
    let mut term = Terminal::new(4, 3);
    term.process(b"\x1b[1\"qA\x1b[0\"qa\r\n\x1b[1\"qB\x1b[0\"qb\r\n\x1b[1\"qC\x1b[0\"qc");

    // Mode 0 starts at the cursor, preserving both protected cells and
    // erasable text before the cursor.
    term.process(b"\x1b[2;2H\x1b[?0J");
    assert_eq!(term.rows_text(0, 2), "Aa\nB\nC");

    // Restore erasable suffixes, then mode 1 erases through the cursor.
    term.process(b"\x1b[2;2Ha\x1b[3;2Hb\x1b[2;2H\x1b[?1J");
    assert_eq!(term.rows_text(0, 2), "A\nB\nCb");

    // Plural and subparameter private requests are malformed and must not
    // accidentally fall through to ordinary ED. Singular mode 2 then
    // erases every erasable cell while retaining protected letters.
    term.process(b"\x1b[?1;2J\x1b[?2:0J");
    assert_eq!(term.rows_text(0, 2), "A\nB\nCb");
    term.process(b"\x1b[?2J");
    assert_eq!(term.rows_text(0, 2), "A\nB\nC");
}

#[test]
fn shell_regions_follow_scrollback_and_retire_live_text_after_eviction() {
    let mut term = Terminal::new(16, 3);
    term.process(
        b"\x1b]133;A\x07\x1b]133;B\x07echo stable\r\n\x1b]133;C\x07stable\r\n\x1b]133;D;0\x07",
    );
    let id = term.prompt_tracker().completed_region(0).unwrap().id;
    term.process(b"one\r\ntwo\r\n");

    let region = term.prompt_tracker().completed_region_by_id(id).unwrap();
    assert_eq!(region.prompt_row, -2);
    assert_eq!(region.output_row, Some(-1));
    assert_eq!(
        term.rows_text(region.command_row.unwrap(), region.command_row.unwrap()),
        "echo stable"
    );

    term.set_scrollback_limits(1, 1024 * 1024);
    let evicted = term.prompt_tracker().completed_region_by_id(id).unwrap();
    assert!(!evicted.content_available);
    assert!(
        term.prompt_tracker()
            .command_at(evicted.prompt_row)
            .is_none()
    );
}

#[test]
fn shell_region_boundaries_follow_column_reflow() {
    let mut term = Terminal::new(12, 4);
    term.process(
            b"\x1b]133;A\x07\x1b]133;B\x07echo 12345678\r\n\x1b]133;C\x07result value\r\n\x1b]133;D;0\x07",
        );
    term.process(b"later one\r\nlater two\r\n");
    let before = term.prompt_tracker().completed_region(0).unwrap().clone();
    assert!(before.prompt_row < 0);
    let command_before = term.rows_text(
        before.command_row.unwrap(),
        before.output_row.unwrap().saturating_sub(1),
    );
    let output_before = term.rows_text(
        before.output_row.unwrap(),
        before.finish_row.unwrap().saturating_sub(1),
    );

    term.resize(6, 7);

    let after = term
        .prompt_tracker()
        .completed_region_by_id(before.id)
        .unwrap();
    let command_after = term.rows_text(
        after.command_row.unwrap(),
        after.output_row.unwrap().saturating_sub(1),
    );
    let output_after = term.rows_text(
        after.output_row.unwrap(),
        after.finish_row.unwrap().saturating_sub(1),
    );
    let compact = |text: &str| text.split_whitespace().collect::<String>();
    assert_eq!(compact(&command_after), compact(&command_before));
    assert_eq!(compact(&output_after), compact(&output_before));
    assert_eq!(
        term.prompt_tracker()
            .command_at(after.output_row.unwrap())
            .unwrap()
            .id,
        before.id
    );
}

#[test]
fn failed_command_navigation_reveals_only_non_zero_exits_in_history() {
    let mut term = Terminal::new(20, 4);
    for (index, exit) in [0, 3, 0, 1, 0].into_iter().enumerate() {
        term.process(b"\x1b]133;A\x07\x1b]133;B\x07");
        term.process(format!("cmd {index}\r\n").as_bytes());
        term.process(format!("\x1b]133;C\x07out\r\n\x1b]133;D;{exit}\x07").as_bytes());
    }
    term.process(b"\x1b]133;A\x07");
    let tracker = term.prompt_tracker();
    let failed: Vec<i32> = (0..5)
        .filter_map(|index| tracker.completed_region(index))
        .filter(|region| region.is_failure())
        .map(|region| region.prompt_row)
        .collect();
    assert_eq!(failed.len(), 2);

    assert_eq!(term.previous_failed_command(), Some(failed[1]));
    assert_eq!(term.previous_failed_command(), Some(failed[0]));
    assert!(term.grid().scroll_offset() > 0, "history is revealed");
    assert_eq!(term.previous_failed_command(), None);
    assert_eq!(term.next_failed_command(), Some(failed[1]));
    assert_eq!(term.next_failed_command(), None);
}

#[test]
fn prompt_navigation_walks_history_and_survives_reflow_without_stale_rows() {
    let mut term = Terminal::new(14, 4);
    for index in 0..5 {
        term.process(b"\x1b]133;A\x07\x1b]133;B\x07");
        term.process(format!("echo {index}\r\n").as_bytes());
        term.process(b"\x1b]133;C\x07output\r\n\x1b]133;D;0\x07");
    }
    term.process(b"\x1b]133;A\x07");

    let cursor_row = i32::try_from(term.grid().cursor().row).unwrap();
    let prompts = term.prompt_tracker().all_prompts().collect::<Vec<_>>();
    let backwards = prompts
        .iter()
        .copied()
        .filter(|row| *row < cursor_row)
        .rev()
        .collect::<Vec<_>>();
    assert!(backwards.len() >= 4);
    assert!(backwards.iter().any(|row| *row < 0));

    for expected in &backwards {
        assert_eq!(term.previous_prompt(), Some(*expected));
        assert_eq!(
            term.grid().scroll_offset(),
            usize::try_from(expected.saturating_neg()).unwrap_or(0)
        );
    }
    assert_eq!(term.previous_prompt(), None);

    for expected in prompts
        .iter()
        .copied()
        .filter(|row| *row > *backwards.last().unwrap())
    {
        assert_eq!(term.next_prompt(), Some(expected));
    }
    assert_eq!(term.next_prompt(), None);

    // Direct scrolling invalidates the navigation cursor. Starting again
    // from the live viewport must select the newest prior prompt, not
    // continue from an invisible stale selection.
    let newest_previous = backwards[0];
    assert_eq!(term.previous_prompt(), Some(newest_previous));
    let reflow_target = term.previous_prompt().unwrap();
    assert!(reflow_target < 0);
    let target_region_id = term.prompt_tracker().command_at(reflow_target).unwrap().id;
    term.grid_mut().set_scroll_offset(0);
    assert_eq!(term.previous_prompt(), Some(newest_previous));
    assert_eq!(term.previous_prompt(), Some(reflow_target));

    term.resize(7, 6);
    let remapped_target = term
        .prompt_tracker()
        .completed_region_by_id(target_region_id)
        .unwrap()
        .prompt_row;
    let expected_older = term.prompt_tracker().previous_prompt(remapped_target);
    assert_eq!(term.previous_prompt(), expected_older);

    term.set_scrollback_limits(0, 0);
    assert!(term.prompt_tracker().all_prompts().all(|row| row >= 0));
    assert!(term.previous_prompt().is_none_or(|row| row >= 0));

    term.process(b"\x1b[?1049h");
    assert_eq!(term.previous_prompt(), None);
    assert_eq!(term.next_prompt(), None);
}

#[test]
fn selected_prompt_stays_visible_during_output_until_the_user_scrolls() {
    let mut term = Terminal::new(14, 4);
    for index in 0..3 {
        term.process(b"\x1b]133;A\x07\x1b]133;B\x07");
        term.process(format!("echo {index}\r\n").as_bytes());
        term.process(b"\x1b]133;C\x07output\r\n\x1b]133;D;0\x07");
    }
    term.process(b"\x1b]133;A\x07");

    let selected = term.previous_prompt().unwrap();
    assert!(selected >= 0);
    let selected_id = term.prompt_tracker().command_at(selected).unwrap().id;
    assert_eq!(term.grid().scroll_offset(), 0);

    term.process(b"more\r\nmore\r\n");
    let moved = term
        .prompt_tracker()
        .completed_region_by_id(selected_id)
        .unwrap()
        .prompt_row;
    assert!(moved < 0);
    assert_eq!(
        term.grid().scroll_offset(),
        usize::try_from(moved.saturating_neg()).unwrap()
    );

    // A direct scroll is an explicit change of navigation origin. Later
    // PTY output must not pull the user back to the old selected prompt.
    term.grid_mut().set_scroll_offset(0);
    term.process(b"new-output\r\n");
    assert_eq!(term.grid().scroll_offset(), 0);
}

#[test]
fn destructive_screen_lifecycle_retires_only_erased_shell_content() {
    let mut term = Terminal::new(16, 4);
    term.process(b"\x1b]133;A\x07\x1b]133;B\x07echo old\r\n\x1b]133;C\x07old\r\n\x1b]133;D;0\x07");
    term.process(b"one\r\ntwo\r\nthree\r\nfour\r\n");
    let historical = term.prompt_tracker().completed_region(0).unwrap().clone();
    assert!(historical.finish_row.unwrap() < 0);

    term.process(b"\x1b]133;A\x07\x1b]133;B\x07echo active\x1b]133;C\x07");
    let active_id = term.prompt_tracker().active_region().unwrap().id;
    assert!(term.prompt_tracker().active_region().unwrap().prompt_row >= 0);

    term.process(b"\x1b[2J");
    assert!(
        term.prompt_tracker()
            .completed_region_by_id(historical.id)
            .unwrap()
            .content_available
    );
    assert!(
        !term
            .prompt_tracker()
            .active_region()
            .unwrap()
            .content_available
    );
    assert!(term.prompt_tracker().all_prompts().all(|row| row < 0));

    // Completion identity survives the erase and is still emitted once;
    // only its authority to resolve erased terminal text is retired.
    term.process(b"\x1b]133;D;1\x07");
    let active_completion = term
        .prompt_tracker()
        .completed_region_by_id(active_id)
        .unwrap();
    assert!(!active_completion.content_available);
    assert_eq!(active_completion.exit_code, Some(1));

    term.clear_scrollback();
    assert_eq!(term.grid().scrollback_len(), 0);
    assert!(term.prompt_tracker().all_prompts().all(|row| row >= 0));
    assert!(
        !term
            .prompt_tracker()
            .completed_region_by_id(historical.id)
            .unwrap()
            .content_available
    );
}

#[test]
fn ris_preserves_valid_historical_marks_but_retires_the_erased_viewport() {
    let mut term = Terminal::new(16, 4);
    term.process(b"\x1b]133;A\x07\x1b]133;B\x07echo old\r\n\x1b]133;C\x07old\r\n\x1b]133;D;0\x07");
    term.process(b"one\r\ntwo\r\nthree\r\nfour\r\n");
    let historical = term.prompt_tracker().completed_region(0).unwrap().clone();
    assert!(historical.finish_row.unwrap() < 0);

    term.process(b"\x1b]133;A\x07\x1b]133;B\x07echo visible\x1b]133;C\x07\x1b]133;D;0\x07");
    let visible = term.prompt_tracker().completed_region(1).unwrap().clone();
    assert!(visible.prompt_row >= 0);
    let oldest_offset = term.grid().scrollback_len();
    term.grid_mut().set_scroll_offset(oldest_offset);

    term.process(b"\x1bc");
    assert_eq!(term.grid().scroll_offset(), 0);
    assert!(
        term.prompt_tracker()
            .completed_region_by_id(historical.id)
            .unwrap()
            .content_available
    );
    assert!(
        !term
            .prompt_tracker()
            .completed_region_by_id(visible.id)
            .unwrap()
            .content_available
    );
    assert!(term.prompt_tracker().all_prompts().all(|row| row < 0));
}

#[test]
fn alternate_screen_cannot_publish_primary_shell_marks() {
    let mut term = Terminal::new(16, 4);
    term.process(b"\x1b]133;A\x07");
    let prompts_before = term.prompt_tracker().all_prompts().collect::<Vec<_>>();
    let regions_before = term.prompt_tracker().completed_count();

    term.process(b"\x1b[?1049h");
    term.process(b"\x1b]133;A\x07\x1b]133;B\x07untrusted\x1b]133;C\x07\x1b]133;D;9\x07");
    term.process(b"\x1b[3J\x1b[?1049l");

    assert_eq!(
        term.prompt_tracker().all_prompts().collect::<Vec<_>>(),
        prompts_before
    );
    assert_eq!(term.prompt_tracker().completed_count(), regions_before);
}

#[test]
fn fragmented_osc_133_lifecycle_is_strict_and_streaming_safe() {
    let mut term = Terminal::new(16, 4);
    let valid =
        b"\x1b]133;A\x07\x1b]133;B\x07echo split\r\n\x1b]133;C\x07out\r\n\x1b]133;D;255\x07";
    for byte in valid {
        term.process(std::slice::from_ref(byte));
    }
    let first = term.prompt_tracker().completed_region(0).unwrap();
    assert_eq!(first.exit_code, Some(255));

    // Prompt-only and out-of-order cycles must not add completed regions.
    let malformed = b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;D;0\x07\x1b]133;C\x07\x1b]133;D;0\x07";
    for split in malformed.chunks(2) {
        term.process(split);
    }
    assert_eq!(term.prompt_tracker().completed_count(), 1);

    term.process(
        b"\x1b]133;A\x07\x1b]133;B\x07echo recover\r\n\x1b]133;C\x07out\r\n\x1b]133;D;256\x07",
    );
    let recovered = term.prompt_tracker().completed_region(1).unwrap();
    assert_eq!(recovered.exit_code, None);
}

#[test]
fn completed_notification_icons_have_a_separate_pending_event_byte_ceiling() {
    let mut term = Terminal::new(8, 2);
    for index in 0..3 {
        term.push_event(TerminalEvent::Notification(
            crate::event::NotificationRequest {
                title: Some(format!("icon-{index}")),
                icon_cache_id: Some(format!("cache-{index}")),
                icon_data: Some(vec![index as u8; MAX_NOTIFICATION_ICON_BYTES].into_boxed_slice()),
                ..crate::event::NotificationRequest::default()
            },
        ));
    }
    let events = term.drain_events();
    assert_eq!(events.len(), 3);
    assert!(
        matches!(&events[0], TerminalEvent::Notification(request) if request.icon_data.is_some())
    );
    assert!(
        matches!(&events[1], TerminalEvent::Notification(request) if request.icon_data.is_some())
    );
    assert!(
        matches!(&events[2], TerminalEvent::Notification(request) if request.icon_data.is_none() && request.icon_cache_id.is_none())
    );

    term.push_event(TerminalEvent::Notification(
        crate::event::NotificationRequest {
            title: Some("after-drain".into()),
            icon_data: Some(vec![0; MAX_NOTIFICATION_ICON_BYTES].into_boxed_slice()),
            ..crate::event::NotificationRequest::default()
        },
    ));
    assert!(matches!(
        term.drain_events().as_slice(),
        [TerminalEvent::Notification(request)] if request.icon_data.is_some()
    ));
}
