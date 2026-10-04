use crate::term::*;

fn populated(limit: usize) -> Terminal {
    let mut terminal = Terminal::new(8, 4);
    terminal.grid_mut().set_scrollback_limit(limit);
    terminal.process(b"A\r\nB\r\nC\r\nD\r\nE\r\nF\r\nG\r\nH\r\nI\r\nJ\x1b[1;3r");
    terminal
}

#[test]
fn regional_archival_maps_command_identity_and_retires_mixed_ranges() {
    for limit in [6, 20] {
        for command in [b"\x1b[S".as_slice(), b"\x1b[M", b"\x1b[3;1H\n"] {
            for first in -6..=3 {
                for last in first..=3 {
                    let mut terminal = populated(limit);
                    // Seed every retained interval, including historical commands.
                    let tracker = &mut terminal.prompt_tracker;
                    tracker.mark_prompt_start((first + 6) as usize);
                    tracker.mark_command_start((first + 6) as usize);
                    tracker.mark_output_start((first + 6) as usize);
                    tracker.mark_command_finished((last + 6) as usize, Some(7));
                    tracker.adjust_for_scroll(6, -6);
                    let before = tracker.completed_region(0).unwrap().clone();
                    let text_before = terminal.rows_text(first, last);
                    terminal.reveal_prompt(first);
                    terminal.process(command);

                    let oldest = -(terminal.grid().scrollback_len() as i32);
                    let mapped = |row: i32| if row < 3 { row - 1 } else { row };
                    let available = !(first < 3 && last >= 3) && mapped(first) >= oldest;
                    let region = terminal
                        .prompt_tracker
                        .completed_region_by_id(before.id)
                        .unwrap();
                    assert_eq!(
                        region.content_available, available,
                        "{first}..{last} {command:?}"
                    );
                    assert_eq!(region.prompt_row, mapped(first));
                    assert_eq!(region.command_row, Some(mapped(first)));
                    assert_eq!(region.output_row, Some(mapped(first)));
                    assert_eq!(region.finish_row, Some(mapped(last)));
                    assert_eq!(region.exit_code, Some(7));
                    if available {
                        assert_eq!(
                            terminal.rows_text(region.prompt_row, region.finish_row.unwrap()),
                            text_before
                        );
                        assert_eq!(
                            terminal
                                .prompt_tracker
                                .command_at(mapped(first))
                                .unwrap()
                                .id,
                            before.id
                        );
                    } else {
                        assert!(terminal.prompt_tracker.command_at(mapped(first)).is_none());
                    }
                    assert_eq!(
                        terminal.prompt_navigation.map(|navigation| navigation.row),
                        (mapped(first) >= oldest).then_some(mapped(first))
                    );
                    assert_eq!(
                        terminal.prompt_tracker.all_prompts().collect::<Vec<_>>(),
                        if mapped(first) >= oldest {
                            vec![mapped(first)]
                        } else {
                            vec![]
                        }
                    );
                    assert_eq!(terminal.grid().row_at_offset(3).unwrap().to_string(), "J");
                }
            }
        }
    }
}

#[test]
fn active_commands_survive_archival_and_complete_with_the_same_identity() {
    for bottom in [3, 4] {
        for command in [b"\x1b[S".as_slice(), b"\x1b[1;1H\x1b[M", b"\n"] {
            let mut terminal = Terminal::new(16, 4);
            terminal.process(b"\x1b]133;A\x07\x1b]133;B\x07echo stable\r\n\x1b]133;C\x07stable");
            let id = terminal.prompt_tracker.active_region().unwrap().id;
            terminal.process(format!("\x1b[1;{bottom}r\x1b[{bottom};1H").as_bytes());
            terminal.process(command);
            let active = terminal.prompt_tracker.active_region().unwrap();
            assert!(active.content_available);
            assert_eq!(active.id, id);
            assert_eq!(active.prompt_row, -1);
            assert_eq!(active.output_row, Some(0));
            assert_eq!(terminal.rows_text(-1, -1), "echo stable");
            assert_eq!(terminal.rows_text(0, 0), "stable");
            terminal.process(format!("\x1b[{bottom};1H\x1b]133;D;9\x07").as_bytes());
            let finished = terminal.prompt_tracker.completed_region_by_id(id).unwrap();
            assert!(finished.content_available);
            assert_eq!(finished.exit_code, Some(9));
            assert_eq!(finished.finish_row, Some(bottom - 1));
        }
    }
}

#[test]
fn active_extent_includes_the_cursor_when_motion_splits_command_output() {
    for (prompt, output, cursor, available) in [(0, 1, 3, false), (3, 3, 2, false), (3, 3, 3, true)]
    {
        let mut terminal = populated(20);
        let tracker = &mut terminal.prompt_tracker;
        tracker.mark_prompt_start(prompt);
        tracker.mark_command_start(prompt);
        tracker.mark_output_start(output);
        let id = tracker.active_region().unwrap().id;
        terminal.grid_mut().set_cursor(cursor, 0);
        terminal.scroll_up(1);
        assert_eq!(
            terminal
                .prompt_tracker
                .active_region()
                .unwrap()
                .content_available,
            available
        );
        terminal.process(b"\x1b]133;D;0\x07");
        let region = terminal.prompt_tracker.completed_region_by_id(id).unwrap();
        assert_eq!(region.content_available, available);
        assert_eq!(region.exit_code, Some(0));
    }
}

#[test]
fn regional_prompt_archival_honors_byte_eviction_without_losing_completions() {
    for command in [b"\x1b[S".as_slice(), b"\x1b[M"] {
        for budget in [0, 1] {
            let mut terminal = populated(20);
            terminal.set_scrollback_limits(20, budget);
            terminal.process(b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07\x1b]133;D;3\x07");
            terminal.reveal_prompt(0);
            terminal.process(command);
            assert_eq!(terminal.grid().scrollback_len(), 0);
            let region = terminal.prompt_tracker.completed_region(0).unwrap();
            assert!(!region.content_available);
            assert_eq!(region.exit_code, Some(3));
            assert!(terminal.prompt_tracker.all_prompts().next().is_none());
            assert!(terminal.prompt_navigation.is_none());
        }

        let mut terminal = populated(20);
        let budget = terminal.grid().memory_usage().scrollback_bytes;
        terminal.set_scrollback_limits(20, budget);
        let tracker = &mut terminal.prompt_tracker;
        for row in [0, 5] {
            tracker.mark_prompt_start(row);
            tracker.mark_command_start(row);
            tracker.mark_output_start(row);
            tracker.mark_command_finished(row, Some(0));
        }
        tracker.adjust_for_scroll(6, -6);
        terminal
            .grid_mut()
            .cell_mut(0, 0)
            .unwrap()
            .set_grapheme(&format!("G{}", "\u{301}".repeat(budget / 4)));
        terminal.reveal_prompt(-1);
        terminal.process(command);
        assert!(terminal.grid().scrollback_len() < 6);
        let tracker = &terminal.prompt_tracker;
        assert_eq!(tracker.completed_count(), 2);
        assert!(!tracker.completed_region(0).unwrap().content_available);
        let recent = tracker.completed_region(1).unwrap();
        assert!(recent.content_available);
        assert_eq!(recent.prompt_row, -2);
        assert_eq!(terminal.rows_text(-2, -2), "F");
        assert_eq!(terminal.prompt_navigation.unwrap().row, -2);
    }
}

#[test]
fn archival_prompt_updates_do_not_move_unrelated_history_or_revive_manual_navigation() {
    for command in [
        b"\x1b[2;3r\x1b[S".as_slice(),
        b"\x1b[2;1H\x1b[M",
        b"\x1b[?69h\x1b[2;6s\x1b[S",
        b"\x1b[?69h\x1b[2;6s\x1b[1;2H\x1b[M",
        b"\x1b[?1049h\x1b[S\x1b[1;1H\x1b[M\x1b[?1049l",
    ] {
        let mut terminal = populated(20);
        let tracker = &mut terminal.prompt_tracker;
        tracker.mark_prompt_start(0);
        tracker.mark_command_start(0);
        tracker.mark_output_start(0);
        tracker.mark_command_finished(0, Some(0));
        tracker.adjust_for_scroll(2, -6);
        terminal.process(command);
        let region = terminal.prompt_tracker.completed_region(0).unwrap();
        assert!(region.content_available);
        assert_eq!(region.prompt_row, -2);
        assert_eq!(terminal.rows_text(-2, -2), "E");
        assert_eq!(
            terminal.prompt_tracker.all_prompts().collect::<Vec<_>>(),
            vec![-2]
        );
    }
    for command in [b"\x1b[S".as_slice(), b"\x1b[M"] {
        let mut terminal = populated(20);
        terminal.prompt_tracker.mark_prompt_start(0);
        terminal.prompt_tracker.adjust_for_scroll(2, -6);
        terminal.reveal_prompt(-2);
        terminal.grid_mut().set_scroll_offset(0);
        terminal.process(command);
        assert!(terminal.prompt_navigation.is_none());
        assert_eq!(terminal.grid().scroll_offset(), 0);
        assert_eq!(
            terminal.prompt_tracker.all_prompts().collect::<Vec<_>>(),
            vec![-3]
        );
    }
}
