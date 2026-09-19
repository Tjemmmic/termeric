use super::*;

#[test]
fn resize_eviction_cannot_rebind_commands_to_new_oldest_lines() {
    let mut terminal = Terminal::new(8, 4);
    terminal.set_scrollback_limits(3, 1024 * 1024);
    terminal.process(b"\x1b]133;A\x07\x1b]133;B\x07AAAAAAAA\r\n\x1b]133;C\x07BBBBBBBB\r\n\x1b]133;D;7\x07CCCCCCCC\r\nDDDDDDDD\r\nEEEEEEEE\r\nFFFFFFFF");
    let original = terminal
        .shell
        .prompt_tracker
        .completed_region(0)
        .unwrap()
        .clone();
    assert!(original.content_available);
    assert_eq!(
        terminal.rows_text(original.prompt_row, original.prompt_row),
        "AAAAAAAA"
    );
    terminal.resize(4, 4);
    let retained = terminal
        .shell
        .prompt_tracker
        .completed_region_by_id(original.id)
        .unwrap();
    assert!(
        !retained.content_available,
        "evicted command rebound to {:?}",
        retained
    );
    assert_eq!(retained.exit_code, Some(7));
    assert!(terminal.shell.prompt_tracker.all_prompts().next().is_none());
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
                    .shell
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
                        .shell
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
                    terminal
                        .shell
                        .prompt_tracker
                        .all_prompts()
                        .collect::<Vec<_>>(),
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
                    .shell
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
            let region = terminal.shell.prompt_tracker.completed_region(0).unwrap();
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
        let region = terminal.shell.prompt_tracker.completed_region(0).unwrap();
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
            let id = terminal.shell.prompt_tracker.active_region().unwrap().id;
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
            let active = terminal.shell.prompt_tracker.active_region().unwrap();
            assert_eq!(active.id, id);
            assert_eq!(active.content_available, expected.is_some());
            if let Some(row) = expected {
                assert_eq!(active.prompt_row, row);
            }
            terminal.process(b"\x1b]133;D;9\x07");
            let completed = terminal
                .shell
                .prompt_tracker
                .completed_region_by_id(id)
                .unwrap();
            assert_eq!(completed.content_available, expected.is_some());
            assert_eq!(completed.exit_code, Some(9));
        }
    }
}
