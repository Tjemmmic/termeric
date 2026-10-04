mod history_view;

use super::*;

#[test]
fn grid_revisions_cover_mutation_entry_points_and_separate_row_movement() {
    type Mutation = fn(&mut Grid);
    let mutations: &[(&str, Mutation, bool)] = &[
        (
            "cell",
            |grid| grid.cell_mut(0, 0).unwrap().set_char('x'),
            false,
        ),
        (
            "row",
            |grid| grid.row_mut(0).unwrap().set_wrapped(true),
            false,
        ),
        ("clear", Grid::clear, false),
        (
            "scroll up",
            |grid| {
                grid.scroll_up();
            },
            true,
        ),
        ("scroll down", Grid::scroll_down, true),
        (
            "region up",
            |grid| {
                grid.scroll_region_up(0, 1);
            },
            true,
        ),
        ("region down", |grid| grid.scroll_region_down(0, 1), true),
        (
            "archive",
            |grid| {
                grid.archive_visible();
            },
            true,
        ),
        ("clear history", Grid::clear_scrollback, true),
        ("row limit", |grid| grid.set_scrollback_limit(0), true),
        ("byte limit", |grid| grid.set_scrollback_byte_limit(0), true),
        ("resize", |grid| grid.resize(3, 3), true),
    ];
    for &(name, mutate, moves_rows) in mutations {
        let mut grid = Grid::new(4, 2);
        grid.cell_mut(0, 0).unwrap().set_char('a');
        grid.scroll_up();
        assert_eq!(grid.scrollback_len(), 1);
        let content = grid.content_revision();
        let mapping = grid.row_mapping_revision();
        mutate(&mut grid);
        assert_ne!(grid.content_revision(), content, "{name}");
        assert_eq!(grid.row_mapping_revision() != mapping, moves_rows, "{name}");
    }
}

#[test]
fn grid_revisions_ignore_navigation_and_distinguish_replacement_and_snapshot() {
    let mut grid = Grid::new(4, 2);
    grid.scroll_up();
    let content = grid.content_revision();
    let mapping = grid.row_mapping_revision();
    grid.set_cursor(1, 2);
    grid.set_cursor_visible(false);
    grid.set_wrap_pending(true);
    grid.set_scroll_offset(1);
    grid.set_scrollback_limit(100);
    grid.set_scrollback_byte_limit(1024 * 1024);
    grid.resize(4, 2);
    assert!(grid.cell_mut(100, 100).is_none());
    assert!(grid.row_mut(100).is_none());
    assert_eq!(grid.content_revision(), content);
    assert_eq!(grid.row_mapping_revision(), mapping);
    let snapshot = grid.visible_snapshot();
    assert_ne!(snapshot.content_revision(), content);
    let frozen = snapshot.content_revision();
    grid.cell_mut(0, 0).unwrap().set_char('x');
    assert_eq!(snapshot.content_revision(), frozen);
    assert_ne!(
        Grid::new(4, 2).content_revision(),
        Grid::new(4, 2).content_revision()
    );
}

#[test]
fn multicell_repair_leaves_ordinary_rows_clean_and_unchanged() {
    let mut grid = Grid::new(8, 4);
    for row in 0..4 {
        grid.cell_mut(row, 0).unwrap().set_grapheme("e\u{301}");
        grid.cell_mut(row, 1).unwrap().set_char('界');
        grid.cell_mut(row, 1)
            .unwrap()
            .flags
            .insert(CellFlags::WIDE_CHAR);
        grid.cell_mut(row, 2)
            .unwrap()
            .flags
            .insert(CellFlags::WIDE_CHAR_SPACER);
    }
    let before: Vec<_> = (0..4)
        .map(|row| grid.row(row).unwrap().iter().cloned().collect::<Vec<_>>())
        .collect();
    grid.repair_multicells(&Cell::default());
    for (row, cells) in before.iter().enumerate() {
        assert_eq!(
            grid.row(row).unwrap().iter().collect::<Vec<_>>(),
            cells.iter().collect::<Vec<_>>()
        );
    }
}

#[test]
fn multicell_repair_rejects_foreign_descriptors_sharing_a_valid_anchor() {
    let leader = MultiCell::new(
        2,
        1,
        0,
        0,
        MultiCellAlignment::Start,
        MultiCellAlignment::Start,
        0,
        0,
    )
    .unwrap();
    let blank = Cell {
        bg: Color::rgb(17, 31, 59),
        ..Cell::default()
    };
    for (scale, width) in [(3, 1), (2, 2), (3, 2)] {
        let foreign = MultiCell::new(
            scale,
            width,
            1,
            2,
            MultiCellAlignment::Center,
            MultiCellAlignment::End,
            0,
            0,
        )
        .unwrap();
        for relative_row in 0..foreign.rows() {
            for relative_col in 0..foreign.columns() {
                if relative_row < leader.rows() && relative_col < leader.columns() {
                    continue;
                }
                for stray_text in [false, true] {
                    let mut grid = Grid::new(10, 6);
                    let (top, left) = (1, 2);
                    for row in 0..leader.rows() {
                        for col in 0..leader.columns() {
                            let cell = grid.cell_mut(top + row, left + col).unwrap();
                            cell.multicell = leader.at(col, row).unwrap();
                            if row == 0 && col == 0 {
                                cell.set_grapheme("large");
                                cell.multicell = leader;
                            }
                        }
                    }
                    let (row, col) = (top + relative_row, left + relative_col);
                    let cell = grid.cell_mut(row, col).unwrap();
                    if stray_text {
                        cell.set_char('x');
                    }
                    cell.multicell = foreign.at(relative_col, relative_row).unwrap();
                    assert!(grid.multicell_region_at(top, left).is_some());
                    assert!(grid.multicell_region_at(row, col).is_none());
                    // Exercise cached presence and ensure repair preserves all
                    // unrelated cells, not just the legitimate leader.
                    for row in 0..grid.rows() {
                        grid.row_mut(row).unwrap().has_multicells();
                    }
                    let before: Vec<_> = grid.iter_cells().map(|(_, _, c)| c.clone()).collect();
                    // Synchronized-output capture uses the same repair path
                    // on newly copied rows, with cold descriptor caches.
                    let snapshot = grid.visible_snapshot();
                    assert!(!snapshot.cell(row, col).unwrap().multicell.is_present());
                    assert!(snapshot.cell(row, col).unwrap().content.is_empty());
                    assert!(snapshot.multicell_region_at(top, left).is_some());
                    assert!(grid.cell(row, col).unwrap().multicell.is_present());
                    grid.repair_multicells(&blank);
                    assert_eq!(
                        grid.cell(row, col),
                        Some(&blank),
                        "foreign={foreign:?} row={row} col={col} text={stray_text}"
                    );
                    for (r, c, cell) in grid.iter_cells() {
                        if (r, c) != (row, col) {
                            assert_eq!(cell, &before[r * grid.cols() + c]);
                        }
                    }
                    assert!(grid.multicell_region_at(top, left).is_some());
                }
            }
        }
    }
}

#[test]
fn multicell_repair_matches_two_pass_reference_for_every_block_subset() {
    // Frozen pre-optimization algorithm. Every subset of a 4x2 block
    // includes empty grids, intact blocks, missing leaders, and holes.
    fn reference(grid: &mut Grid, blank: &Cell) {
        let mut valid = HashSet::new();
        for row in 0..grid.rows {
            for col in 0..grid.cols {
                if grid
                    .cell(row, col)
                    .is_some_and(|cell| cell.multicell.is_leader())
                    && let Some(region) = grid.multicell_region_at(row, col)
                {
                    valid.insert((region.top, region.left));
                }
            }
        }
        for row in 0..grid.rows {
            for col in 0..grid.cols {
                let keep = grid.cell(row, col).is_none_or(|cell| {
                    !cell.multicell.is_present()
                        || row
                            .checked_sub(cell.multicell.relative_row())
                            .zip(col.checked_sub(cell.multicell.relative_col()))
                            .is_some_and(|anchor| valid.contains(&anchor))
                });
                if !keep && let Some(cell) = grid.cell_mut(row, col) {
                    *cell = blank.clone();
                }
            }
        }
    }
    let descriptor = MultiCell::new(
        2,
        2,
        0,
        0,
        MultiCellAlignment::Start,
        MultiCellAlignment::Start,
        0,
        0,
    )
    .unwrap();
    let blank = Cell {
        bg: Color::rgb(17, 31, 59),
        ..Cell::default()
    };
    for mask in 0..256 {
        for empty_leader in [false, true] {
            let mut actual = Grid::new(8, 4);
            let mut expected = Grid::new(8, 4);
            for grid in [&mut actual, &mut expected] {
                // Prime cached absence before descriptor insertion so the
                // parity check also covers cache invalidation on edits.
                for row in 0..4 {
                    assert!(!grid.row_mut(row).unwrap().has_multicells());
                }
                grid.cell_mut(0, 0).unwrap().set_char('x');
                for offset in 0..8 {
                    if mask & (1 << offset) != 0 {
                        let cell = grid.cell_mut(1 + offset / 4, 2 + offset % 4).unwrap();
                        if offset == 0 && !empty_leader {
                            cell.set_grapheme("large");
                        }
                        cell.multicell = descriptor.at(offset % 4, offset / 4).unwrap();
                    }
                }
            }
            actual.repair_multicells(&blank);
            reference(&mut expected, &blank);
            for row in 0..4 {
                for col in 0..8 {
                    assert_eq!(
                        actual.cell(row, col),
                        expected.cell(row, col),
                        "mask={mask} empty_leader={empty_leader} row={row} col={col}"
                    );
                }
            }
        }
    }
}

#[test]
fn test_grid_creation() {
    let grid = Grid::new(80, 24);
    assert_eq!(grid.cols(), 80);
    assert_eq!(grid.rows(), 24);
}

#[test]
fn test_cursor_movement() {
    let mut grid = Grid::new(80, 24);
    grid.set_cursor(10, 20);
    let cursor = grid.cursor();
    assert_eq!(cursor.row, 10);
    assert_eq!(cursor.col, 20);
}

#[test]
fn test_cursor_clamping() {
    let mut grid = Grid::new(80, 24);
    grid.set_cursor(100, 100);
    let cursor = grid.cursor();
    assert_eq!(cursor.row, 23);
    assert_eq!(cursor.col, 79);
}

#[test]
fn multicell_regions_validate_and_clear_atomically() {
    let mut grid = Grid::new(8, 4);
    let leader = MultiCell::new(
        2,
        2,
        1,
        2,
        MultiCellAlignment::Center,
        MultiCellAlignment::End,
        0,
        0,
    )
    .unwrap();
    for relative_row in 0..leader.rows() {
        for relative_col in 0..leader.columns() {
            let cell = grid.cell_mut(1 + relative_row, 2 + relative_col).unwrap();
            cell.multicell = leader.at(relative_col, relative_row).unwrap();
        }
    }
    grid.cell_mut(1, 2).unwrap().set_grapheme("large");
    grid.cell_mut(1, 2).unwrap().multicell = leader;

    let region = grid.multicell_region_at(2, 5).unwrap();
    assert_eq!((region.top, region.left), (1, 2));
    assert_eq!((region.bottom(), region.right()), (3, 6));
    assert!(region.contains(1, 2));
    assert!(region.contains(2, 5));
    assert!(!region.contains(3, 5));

    let blank = Cell::default();
    assert!(grid.clear_multicell_at(1, 4, &blank, true));
    for row in 1..3 {
        for col in 2..6 {
            let cell = grid.cell(row, col).unwrap();
            assert_eq!(cell.content, CellContent::Char(' '));
            assert!(!cell.multicell.is_present());
        }
    }
}

#[test]
fn malformed_multicell_descriptor_cannot_clear_neighbors() {
    let mut grid = Grid::new(4, 2);
    grid.cell_mut(0, 0).unwrap().set_char('a');
    grid.cell_mut(0, 1).unwrap().set_char('b');
    grid.cell_mut(0, 1).unwrap().multicell = MultiCell::new(
        2,
        2,
        0,
        0,
        MultiCellAlignment::Start,
        MultiCellAlignment::Start,
        1,
        0,
    )
    .unwrap();

    assert!(grid.multicell_region_at(0, 1).is_none());
    assert!(!grid.clear_multicell_at(0, 1, &Cell::default(), false));
    assert_eq!(grid.cell(0, 0).unwrap().content, CellContent::Char('a'));
    assert!(grid.cell(0, 1).unwrap().is_empty());
}

#[test]
fn visible_snapshot_copies_the_scrolled_view_without_history() {
    let mut grid = Grid::new(4, 2);
    for (col, character) in "old!".chars().enumerate() {
        grid.cell_mut(0, col).unwrap().set_char(character);
    }
    grid.scroll_up();
    for (col, character) in "live".chars().enumerate() {
        grid.cell_mut(1, col).unwrap().set_char(character);
    }
    grid.set_scroll_offset(1);

    let snapshot = grid.visible_snapshot();

    assert_eq!(snapshot.scrollback_len(), 0);
    assert_eq!(snapshot.scroll_offset(), 0);
    assert_eq!(snapshot.row(0).unwrap().to_string(), "old!");
    assert!(!snapshot.cursor().visible);
}
