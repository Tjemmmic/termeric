use super::*;

thread_local! {
    static SHELL_REFLOW_ROW_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Counts rows walked by `shell_anchor_rows`, so tests can prove that anchor
/// mapping stays a single pass over history instead of one scan per command.
pub(super) fn count_shell_reflow_row_visit() {
    SHELL_REFLOW_ROW_VISITS.with(|visits| visits.set(visits.get() + 1));
}

// Scalar reference mappings that walk history once per call. The batched
// production capture and restore must agree with them.
fn shell_logical_anchor(grid: &Grid, target_row: i32) -> Option<ShellLogicalAnchor> {
    let oldest = -(grid.scrollback_len() as i32);
    let newest = grid.rows().saturating_sub(1) as i32;
    if !(oldest..=newest).contains(&target_row) {
        return None;
    }
    let mut line = 0_usize;
    let mut cell = 0_usize;
    for (row_index, row) in shell_anchor_rows(grid) {
        if row_index == target_row {
            return Some(ShellLogicalAnchor { line, cell });
        }
        if row.is_wrapped() {
            cell = cell.saturating_add(row.reflow_unit_count());
        } else {
            line = line.saturating_add(1);
            cell = 0;
        }
    }
    None
}

fn shell_row_from_anchor(grid: &Grid, target: ShellLogicalAnchor) -> Option<i32> {
    let mut line = 0_usize;
    let mut remaining = target.cell;
    for (row_index, row) in shell_anchor_rows(grid) {
        if line == target.line {
            let units = row.reflow_unit_count();
            if remaining < units || !row.is_wrapped() {
                return Some(row_index);
            }
            remaining = remaining.saturating_sub(units);
        }
        if !row.is_wrapped() {
            if line == target.line {
                return Some(row_index);
            }
            line = line.saturating_add(1);
            remaining = target.cell;
        }
    }
    None
}

#[test]
fn shell_row_bookmark_tracks_wide_reflow_padding() {
    let mut term = Terminal::new(5, 4);
    term.process("ab界ZQ".as_bytes());
    let anchor = shell_logical_anchor(term.grid(), 1).unwrap();
    term.resize(3, 4);
    assert_eq!(shell_row_from_anchor(term.grid(), anchor), Some(2));
    assert_eq!(term.grid().row(2).unwrap().to_string(), "Q");
}

#[test]
fn batched_shell_anchors_match_scalar_mapping_through_reflow_and_eviction() {
    for columns in [1, 2, 3, 5, 8] {
        for limit in [0, 1, 20] {
            for new_columns in [1, 3, 4, 12] {
                for new_rows in [1, 3, 6] {
                    let mut terminal = Terminal::new(columns, 4);
                    terminal.set_scrollback_limits(limit, 1024 * 1024);
                    terminal
                        .process("ab界XQr界S\r\n\r\nabcdefghi\r\n👩‍💻e\u{301}界\r\nlast".as_bytes());
                    let oldest = -(terminal.grid().scrollback_len() as i32);
                    let mut targets: Vec<_> = (oldest - 2..6).rev().collect();
                    targets.extend([0, 0, i32::MIN, i32::MAX]);
                    let actual = capture_shell_anchors(terminal.grid(), targets.clone());
                    targets.sort_unstable();
                    targets.dedup();
                    let expected: Vec<_> = targets
                        .into_iter()
                        .filter_map(|row| {
                            shell_logical_anchor(terminal.grid(), row).map(|anchor| (row, anchor))
                        })
                        .collect();
                    assert_eq!(actual, expected);
                    let outcome = terminal.grid_mut().resize_with_points(
                        new_columns,
                        new_rows,
                        &mut [None; 2],
                    );
                    let retained: Vec<_> = actual
                        .into_iter()
                        .filter_map(|(row, anchor)| {
                            anchor
                                .retained_after_resize(outcome)
                                .map(|anchor| (row, anchor))
                        })
                        .collect();
                    let expected: HashMap<_, _> = retained
                        .iter()
                        .filter_map(|&(old_row, anchor)| {
                            shell_row_from_anchor(terminal.grid(), anchor).map(|row| (old_row, row))
                        })
                        .collect();
                    assert_eq!(
                        restore_shell_anchors(terminal.grid(), retained),
                        expected,
                        "{columns}x4 -> {new_columns}x{new_rows}, limit={limit}"
                    );
                    // Probe starts, interiors, trimmed ends, and nonexistent
                    // logical lines independently of captured row boundaries.
                    let mut probes: Vec<_> = (0..10)
                        .flat_map(|line| {
                            [0, 1, 2, 3, 10, usize::MAX]
                                .into_iter()
                                .map(move |cell| ShellLogicalAnchor { line, cell })
                        })
                        .enumerate()
                        .map(|(id, anchor)| (id as i32, anchor))
                        .collect();
                    probes.reverse();
                    let expected: HashMap<_, _> = probes
                        .iter()
                        .filter_map(|&(id, anchor)| {
                            shell_row_from_anchor(terminal.grid(), anchor).map(|row| (id, row))
                        })
                        .collect();
                    assert_eq!(restore_shell_anchors(terminal.grid(), probes), expected);
                }
            }
        }
    }
}

#[test]
fn empty_shell_anchor_batches_do_not_scan_history() {
    let terminal = Terminal::new(8, 3);
    SHELL_REFLOW_ROW_VISITS.with(|visits| visits.set(0));
    assert!(capture_shell_anchors(terminal.grid(), vec![]).is_empty());
    assert!(capture_shell_anchors(terminal.grid(), vec![i32::MIN, i32::MAX]).is_empty());
    assert!(restore_shell_anchors(terminal.grid(), vec![]).is_empty());
    assert_eq!(SHELL_REFLOW_ROW_VISITS.with(|visits| visits.get()), 0);
}

#[test]
fn shell_anchor_mapping_scans_history_at_most_once_per_pass() {
    let mut terminal = Terminal::new(8, 3);
    terminal.set_scrollback_limits(20_000, 64 * 1024 * 1024);
    for _ in 0..10_000 {
        terminal.process(b"row\r\n");
    }
    let grid = terminal.grid();
    let oldest = -(grid.scrollback_len() as i32);
    let targets: Vec<_> = (oldest..3).step_by(10).collect();
    SHELL_REFLOW_ROW_VISITS.with(|visits| visits.set(0));
    let started = Instant::now();
    let captured = capture_shell_anchors(grid, targets.clone());
    let restored = restore_shell_anchors(grid, captured);
    let elapsed = started.elapsed();
    let visits = SHELL_REFLOW_ROW_VISITS.with(|visits| visits.get());
    assert_eq!(restored.len(), targets.len());
    for row in targets {
        assert_eq!(restored[&row], row);
    }
    eprintln!(
        "shell anchor round trip: {} targets, {} rows, {visits} visits, {elapsed:?}",
        restored.len(),
        grid.scrollback_len() + grid.rows()
    );
    assert!(
        visits <= 2 * (grid.scrollback_len() + grid.rows()),
        "repeated history scans: {visits}"
    );
}

#[test]
fn resize_eviction_cannot_rebind_commands_to_new_oldest_lines() {
    let mut terminal = Terminal::new(8, 4);
    terminal.set_scrollback_limits(3, 1024 * 1024);
    terminal.process(b"\x1b]133;A\x07\x1b]133;B\x07AAAAAAAA\r\n\x1b]133;C\x07BBBBBBBB\r\n\x1b]133;D;7\x07CCCCCCCC\r\nDDDDDDDD\r\nEEEEEEEE\r\nFFFFFFFF");
    let original = terminal.prompt_tracker.completed_region(0).unwrap().clone();
    assert!(original.content_available);
    assert_eq!(
        terminal.rows_text(original.prompt_row, original.prompt_row),
        "AAAAAAAA"
    );
    terminal.resize(4, 4);
    let retained = terminal
        .prompt_tracker
        .completed_region_by_id(original.id)
        .unwrap();
    assert!(
        !retained.content_available,
        "evicted command rebound to {:?}",
        retained
    );
    assert_eq!(retained.exit_code, Some(7));
    assert!(terminal.prompt_tracker.all_prompts().next().is_none());
}

#[test]
fn resize_command_ids_and_navigation_follow_independently_identified_text() {
    for (limit, bytes) in [
        (0, 0),
        (0, 1 << 20),
        (1, 1 << 20),
        (3, 1 << 20),
        (20, 120),
        (20, 1 << 20),
    ] {
        for cols in [1, 3, 4, 8, 12] {
            for rows in [2, 4, 6] {
                let mut terminal = Terminal::new(8, 4);
                terminal.set_scrollback_limits(limit, bytes);
                for (index, letter) in (b'A'..=b'H').enumerate() {
                    if index > 0 {
                        terminal.process(b"\r\n");
                    }
                    terminal.process(b"\x1b]133;A\x07\x1b]133;B\x07");
                    terminal.process(&[letter, b'x', b'x', b'x', b'x', b'x', b'x', b'x']);
                    terminal.process(b"\x1b]133;C\x07\x1b]133;D;7\x07");
                }
                let before: Vec<_> = terminal
                    .prompt_tracker
                    .completed_regions_after(0)
                    .cloned()
                    .collect();
                terminal.reveal_prompt(before.last().unwrap().prompt_row);
                terminal.resize(cols, rows);
                let oldest = -(terminal.grid().scrollback_len() as i32);
                let locate = |letter| {
                    (oldest..rows as i32).find(|&row| {
                        terminal
                            .grid()
                            .row_at_offset(row)
                            .unwrap()
                            .get(0)
                            .is_some_and(|cell| cell.content.char() == Some(char::from(letter)))
                    })
                };
                for original in before {
                    let expected = original
                        .content_available
                        .then(|| locate(b'A' + original.id as u8 - 1))
                        .flatten();
                    let actual = terminal
                        .prompt_tracker
                        .completed_region_by_id(original.id)
                        .unwrap();
                    assert_eq!(
                        actual.content_available,
                        expected.is_some(),
                        "limit={limit} bytes={bytes} size={cols}x{rows} id={}",
                        original.id
                    );
                    assert_eq!(actual.exit_code, Some(7));
                    if let Some(row) = expected {
                        assert_eq!(actual.prompt_row, row);
                        assert_eq!(actual.command_row, Some(row));
                        assert_eq!(actual.output_row, Some(row));
                        assert_eq!(actual.finish_row, Some(row));
                    }
                }
                assert_eq!(
                    terminal.prompt_navigation.map(|navigation| navigation.row),
                    locate(b'H')
                );
                let retained_prompts: Vec<_> = (b'A'..=b'H').filter_map(locate).collect();
                assert_eq!(
                    terminal.prompt_tracker.all_prompts().collect::<Vec<_>>(),
                    retained_prompts
                );
            }
        }
    }
}

#[test]
fn partial_logical_line_eviction_uses_wide_grapheme_units_not_physical_columns() {
    for limit in [0, 1, 20] {
        for cols in [1, 2, 3, 4] {
            let mut terminal = Terminal::new(5, 3);
            terminal.set_scrollback_limits(limit, 1 << 20);
            terminal.process(
                "ab界XQ\x1b]133;A\x07\x1b]133;B\x07r界S\x1b]133;C\x07\x1b]133;D;3\x07T".as_bytes(),
            );
            assert_eq!(
                terminal
                    .prompt_tracker
                    .completed_region(0)
                    .unwrap()
                    .prompt_row,
                1
            );
            terminal.reveal_prompt(1);
            terminal.resize(cols, 3);
            let oldest = -(terminal.grid().scrollback_len() as i32);
            let expected = (oldest..3).find(|&row| {
                terminal
                    .grid()
                    .row_at_offset(row)
                    .unwrap()
                    .iter()
                    .any(|cell| cell.content.char() == Some('Q'))
            });
            let region = terminal.prompt_tracker.completed_region(0).unwrap();
            assert_eq!(
                region.content_available,
                expected.is_some(),
                "limit={limit} cols={cols}"
            );
            assert_eq!(region.exit_code, Some(3));
            if let Some(row) = expected {
                assert_eq!(region.prompt_row, row, "limit={limit} cols={cols}");
            }
            assert_eq!(
                terminal.prompt_navigation.map(|navigation| navigation.row),
                expected
            );
        }
    }
}

#[test]
fn blank_command_boundaries_survive_only_when_their_logical_line_is_retained() {
    for limit in [0, 1, 3, 20] {
        let mut terminal = Terminal::new(8, 4);
        terminal.set_scrollback_limits(limit, 1 << 20);
        terminal.process(b"AAAAAAAA\r\n\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07\x1b]133;D;0\x07\r\nBBBBBBBB\r\nCCCCCCCC");
        terminal.reveal_prompt(1);
        terminal.resize(4, 4);
        let region = terminal.prompt_tracker.completed_region(0).unwrap();
        assert_eq!(region.content_available, limit > 0);
        if limit > 0 {
            assert_eq!(region.prompt_row, -1);
            assert_eq!(terminal.rows_text(-1, -1), "");
        }
        assert_eq!(
            terminal.prompt_navigation.map(|navigation| navigation.row),
            (limit > 0).then_some(-1)
        );
    }
}

#[test]
fn active_commands_keep_completion_identity_after_resize_retention_or_eviction() {
    for early in [false, true] {
        for cols in [1, 4] {
            let mut terminal = Terminal::new(8, 4);
            terminal.set_scrollback_limits(1, 1 << 20);
            for (index, letter) in (b'A'..=b'D').enumerate() {
                if index > 0 {
                    terminal.process(b"\r\n");
                }
                if index == if early { 0 } else { 3 } {
                    terminal.process(b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07");
                }
                terminal.process(&[letter, b'x', b'x', b'x', b'x', b'x', b'x', b'x']);
            }
            let id = terminal.prompt_tracker.active_region().unwrap().id;
            terminal.resize(cols, 4);
            let letter = if early { 'A' } else { 'D' };
            let oldest = -(terminal.grid().scrollback_len() as i32);
            let expected = (oldest..4).find(|&row| {
                terminal
                    .grid()
                    .row_at_offset(row)
                    .unwrap()
                    .iter()
                    .any(|cell| cell.content.char() == Some(letter))
            });
            let active = terminal.prompt_tracker.active_region().unwrap();
            assert_eq!(active.id, id);
            assert_eq!(active.content_available, expected.is_some());
            if let Some(row) = expected {
                assert_eq!(active.prompt_row, row);
            }
            terminal.process(b"\x1b]133;D;9\x07");
            let completed = terminal.prompt_tracker.completed_region_by_id(id).unwrap();
            assert_eq!(completed.content_available, expected.is_some());
            assert_eq!(completed.exit_code, Some(9));
        }
    }
}
