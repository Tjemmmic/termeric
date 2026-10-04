//! Live, filtered, and standalone search over live and synchronized output.

use super::*;

#[test]
fn live_search_discards_completed_results_after_same_shape_text_changes() {
    let mut app = scrollbar_owner_fixture();
    app.active_terminal_mut()
        .unwrap()
        .process(b"\x1bc\x1b[3Jneedle");
    app.search.start();
    app.with_search_and_active_grid(|search, grid| {
        search.set_query("needle", grid);
        search.finish(grid);
    });
    assert_eq!(app.search.match_count(), 1);
    app.active_terminal_mut()
        .unwrap()
        .process(b"\x1b[H\x1b[2Kerased");
    app.reconcile_search_presentation();
    assert_eq!(app.search.match_count(), 0);
    assert!(app.search.is_searching());
    app.with_search_and_active_grid(|search, grid| search.finish(grid));
    assert_eq!(app.search.match_count(), 0);
}

#[test]
fn live_search_refresh_preserves_controls_confirmation_and_frozen_results() {
    for split in [false, true] {
        let mut app = if split {
            divider_owner_fixture()
        } else {
            scrollbar_owner_fixture()
        };
        app.active_terminal_mut()
            .unwrap()
            .process(b"\x1bc\x1b[3JNEEDLE");
        app.search.start();
        app.with_search_and_active_grid(|search, grid| {
            search.toggle_regex(grid);
            search.set_query("N.E+DL.", grid);
            search.finish(grid);
        });
        app.search.toggle_filter();
        app.search.confirm();
        let generation = app.search.filter_generation();
        app.active_terminal_mut()
            .unwrap()
            .process(b"\x1b[H\x1b[?25l");
        app.reconcile_search_presentation();
        assert_eq!(app.search.filter_generation(), generation);
        assert!(!app.search.is_searching());
        app.active_terminal_mut()
            .unwrap()
            .process(b"\x1b[2Kgone\r\nNEEEDLE");
        app.reconcile_search_presentation();
        assert!(app.search.is_searching());
        assert!(!app.search.is_active());
        assert!(app.search.filtered_rows(24).is_empty());
        app.with_search_and_active_grid(|search, grid| search.finish(grid));
        assert_eq!(app.search.query(), "N.E+DL.");
        assert_eq!(app.search.match_count(), 1);
        assert_eq!(app.search.filtered_rows(24), &[1]);

        app.active_terminal_mut().unwrap().process(b"\x1b[?2026h");
        app.with_search_and_active_grid(|search, grid| search.finish(grid));
        let generation = app.search.filter_generation();
        app.active_terminal_mut()
            .unwrap()
            .process(b"\x1b[H\x1b[2Jhidden");
        app.reconcile_search_presentation();
        assert_eq!(app.search.filter_generation(), generation);
        assert_eq!(app.search.match_count(), 1);
        app.active_terminal_mut().unwrap().process(b"\x1b[?2026l");
        app.with_search_and_active_grid(|search, grid| search.finish(grid));
        assert_eq!(app.search.match_count(), 0);
    }
}

#[test]
fn live_command_search_rescans_in_place_but_rejects_moved_scope_rows() {
    let mut app = scrollbar_owner_fixture();
    let terminal = app.active_terminal_mut().unwrap();
    terminal.resize(30, 3);
    terminal.process(b"\x1bc\x1b[3Jneedle\r\nneedle");
    assert!(app.start_command_output_search(0, 0));
    app.with_search_and_active_grid(|search, grid| {
        search.set_query("needle", grid);
        search.finish(grid);
    });
    assert_eq!(app.search.match_count(), 1);
    app.active_terminal_mut()
        .unwrap()
        .process(b"\x1b[H\x1b[2Kerased");
    assert!(!app.reconcile_search_presentation());
    assert!(app.search.is_scoped());
    app.with_search_and_active_grid(|search, grid| search.finish(grid));
    assert_eq!(app.search.match_count(), 0); // Never widens to the other row.
    app.active_terminal_mut().unwrap().process(b"\x1b[3;1H\n");
    assert!(app.reconcile_search_presentation());
    assert!(!app.search.is_scoped());
    assert!(!app.search.is_active());
}

#[test]
fn live_search_ignores_inactive_output_and_viewport_navigation() {
    let mut app = scrollbar_owner_fixture();
    app.active_terminal_mut().unwrap().process(b"needle");
    app.search.start();
    app.with_search_and_active_grid(|search, grid| {
        search.set_query("needle", grid);
        search.finish(grid);
    });
    app.search.toggle_filter();
    let generation = app.search.filter_generation();
    assert_eq!(app.search.match_count(), 1);
    app.tabs.as_mut().unwrap().all_tabs_mut()[1]
        .focused_terminal_mut()
        .unwrap()
        .process(b"unrelated output\r\n");
    app.active_terminal_mut()
        .unwrap()
        .grid_mut()
        .set_scroll_offset(5);
    app.reconcile_search_presentation();
    assert_eq!(app.search.filter_generation(), generation);
    assert_eq!(app.search.match_count(), 1);
    assert!(!app.search.is_searching());
}

#[test]
fn live_search_invalidates_filtered_copy_before_the_next_render() {
    use termeric_core::Point;
    let mut app = scrollbar_owner_fixture();
    app.active_terminal_mut()
        .unwrap()
        .process(b"\x1bc\x1b[3Jkeep first\r\nsecret\r\nkeep last");
    app.search.start();
    app.with_search_and_active_grid(|search, grid| {
        search.set_query("keep", grid);
        search.finish(grid);
    });
    app.search.toggle_filter();
    app.begin_native_selection(MouseButton::Left, Point::new(0, 0));
    app.extend_native_selection(Point::new(1, 8));
    app.finish_native_selection();
    assert!(app.native_selection_text().is_some());
    app.active_terminal_mut()
        .unwrap()
        .process(b"\x1b[H\x1b[2Khidden replacement");
    // The headless fixture has no clipboard; the action must fail before any
    // clipboard path, without needing a render to clear the old projection.
    assert!(app.clipboard.is_none());
    assert!(!app.copy_selection());
    assert!(app.active_terminal().unwrap().selection().is_none());
    assert!(app.search.is_searching());
}

#[test]
fn filtered_links_resolve_the_displayed_semantic_target_not_an_intervening_row() {
    let mut app = scrollbar_owner_fixture();
    let terminal = app.active_terminal_mut().unwrap();
    terminal.resize(40, 5);
    terminal.process(b"\x1bc\x1b[3Jkeep https://a.test\r\n\x1b]8;;https://hidden.test\x1b\\secret link\x1b]8;;\x1b\\\r\n\x1b]8;;https://shown.test\x1b\\keep https://label.test\x1b]8;;\x1b\\");
    app.search.start();
    app.with_search_and_active_grid(|search, grid| {
        search.set_query("keep", grid);
        search.finish(grid);
    });
    app.search.toggle_filter();
    assert_eq!(
        app.focused_hyperlink_at(termeric_core::Point::new(1, 6)),
        Some("https://shown.test")
    );
    assert_eq!(
        app.url_cache.url_at(1, 6).unwrap().url,
        "https://label.test"
    );
    assert!(
        app.focused_hyperlink_at(termeric_core::Point::new(2, 6))
            .is_none()
    );
    assert!(app.url_cache.url_at(2, 6).is_none());
    app.search.toggle_filter();
    assert_eq!(
        app.focused_hyperlink_at(termeric_core::Point::new(1, 6)),
        Some("https://hidden.test")
    );
    app.search.toggle_filter();
    assert_eq!(
        app.focused_hyperlink_at(termeric_core::Point::new(1, 6)),
        Some("https://shown.test")
    );
}

#[test]
fn mixed_case_unicode_detected_targets_use_filtered_and_frozen_display_cells() {
    use termeric_core::Point;
    for split in [false, true] {
        let mut app = if split {
            divider_owner_fixture()
        } else {
            scrollbar_owner_fixture()
        };
        let terminal = app.active_terminal_mut().unwrap();
        terminal.resize(40, 3);
        terminal
            .process("\x1bc\x1b[3Jother https://other.test\r\nhit HtTpS://例.test/界]".as_bytes());
        app.search.start();
        app.with_search_and_active_grid(|search, grid| {
            search.set_query("hit", grid);
            search.finish(grid);
        });
        app.search.toggle_filter();
        for col in [4, 12, 13, 20, 21] {
            assert_eq!(
                app.focused_hyperlink_at(Point::new(0, col)),
                Some("HtTpS://例.test/界")
            );
        }
        assert!(app.focused_hyperlink_at(Point::new(0, 22)).is_none());
        assert!(app.focused_hyperlink_at(Point::new(1, 4)).is_none());
        // The hidden live replacement cannot change a presented Unicode URL.
        app.active_terminal_mut()
            .unwrap()
            .process(b"\x1b[?2026h\x1b[2;1H\x1b[2Khit https://replacement.test");
        // Snapshot transitions rebuild filtered results; wait for the frozen
        // source scan rather than asserting during ordinary-viewport fallback.
        app.with_search_and_active_grid(|search, grid| search.finish(grid));
        assert_eq!(
            app.focused_hyperlink_at(Point::new(0, 21)),
            Some("HtTpS://例.test/界")
        );
        assert!(app.focused_hyperlink_at(Point::new(0, 22)).is_none());
        app.active_terminal_mut().unwrap().process(b"\x1b[?2026l");
        app.with_search_and_active_grid(|search, grid| search.finish(grid));
        assert_eq!(
            app.focused_hyperlink_at(Point::new(0, 4)),
            Some("https://replacement.test")
        );
    }
}

#[test]
fn filtered_detected_links_refresh_at_activation_after_scrolling_or_query_change() {
    use termeric_core::Point;
    for split in [false, true] {
        let mut app = if split {
            divider_owner_fixture()
        } else {
            scrollbar_owner_fixture()
        };
        let terminal = app.active_terminal_mut().unwrap();
        terminal.resize(40, 3);
        terminal.process(b"\x1bc\x1b[3J");
        for index in 0..10 {
            terminal.process(
                format!("hit https://shown{index}.test\r\nsecret https://hidden{index}.test\r\n")
                    .as_bytes(),
            );
        }
        app.search.start();
        app.with_search_and_active_grid(|search, grid| {
            search.set_query("hit", grid);
            search.finish(grid);
        });
        app.search.toggle_filter();
        assert_eq!(
            app.focused_hyperlink_at(Point::new(1, 5)),
            Some("https://shown1.test")
        );
        let index_ptr = app.search.filtered_matching_rows().as_ptr();
        app.search.scroll_filter(-4.0, 3);
        // No render/cache refresh between the scroll and this action.
        assert_eq!(
            app.focused_hyperlink_at(Point::new(1, 5)),
            Some("https://shown5.test")
        );
        assert_eq!(
            app.url_cache.url_at(1, 5).unwrap().url,
            "https://shown5.test"
        );
        assert_eq!(app.search.filtered_matching_rows().as_ptr(), index_ptr);
        app.with_search_and_active_grid(|search, grid| {
            search.set_query("secret", grid);
            search.finish(grid);
        });
        assert_eq!(
            app.focused_hyperlink_at(Point::new(0, 8)),
            Some("https://hidden0.test")
        );
        assert!(app.focused_hyperlink_at(Point::new(3, 8)).is_none());
    }
}

#[test]
fn filtered_links_use_frozen_rows_despite_hidden_replacements_and_live_scroll() {
    use termeric_core::Point;
    for split in [false, true] {
        let mut app = if split {
            divider_owner_fixture()
        } else {
            scrollbar_owner_fixture()
        };
        let terminal = app.active_terminal_mut().unwrap();
        terminal.resize(40, 5);
        terminal.process(b"\x1bc\x1b[3Jkeep https://first.test\r\nsecret\r\n\x1b]8;;https://shown.test\x1b\\keep https://label.test\x1b]8;;\x1b\\\x1b[?2026h\x1b[H\x1b[2Jkeep https://hidden.test\r\n\r\n\r\n\r\n\r\n\r\n\r\n");
        terminal.grid_mut().set_scroll_offset(3);
        app.search.start();
        app.with_search_and_active_grid(|search, grid| {
            search.set_query("keep", grid);
            search.finish(grid);
        });
        app.search.toggle_filter();
        assert_eq!(
            app.focused_hyperlink_at(Point::new(-3, 6)),
            Some("https://first.test")
        );
        assert_eq!(
            app.focused_hyperlink_at(Point::new(-2, 6)),
            Some("https://shown.test")
        );
        assert!(app.focused_hyperlink_at(Point::new(-1, 6)).is_none());
        assert_eq!(
            app.url_cache.url_at(1, 6).unwrap().url,
            "https://label.test"
        );
    }
}

#[test]
fn filtered_selection_points_follow_noncontiguous_displayed_rows() {
    use termeric_core::Point;
    let mut app = scrollbar_owner_fixture();
    let terminal = app.active_terminal_mut().unwrap();
    terminal.resize(16, 5);
    terminal.process(b"\x1bc\x1b[3Jmatch one\r\nSECRET\r\nmatch two");
    app.search.start();
    app.with_search_and_active_grid(|search, grid| {
        search.set_query("match", grid);
        search.finish(grid);
    });
    app.search.toggle_filter();
    assert_eq!(
        app.native_selection_point(Point::new(1, 0)),
        Some(Point::new(2, 0))
    );
    assert!(app.native_selection_point(Point::new(2, 0)).is_none());
    app.begin_native_selection(MouseButton::Left, Point::new(0, 0));
    app.extend_native_selection(Point::new(1, 8));
    app.finish_native_selection();
    assert_eq!(
        app.native_selection_text()
            .unwrap()
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>(),
        ["match one", "match two"]
    );
    for (previous_clicks, expected) in [(1, "match"), (2, "match two")] {
        app.mouse_state.click_count = previous_clicks;
        app.mouse_state.last_click_pos = Some(Point::new(2, 1));
        app.mouse_state.last_click_time = Instant::now();
        app.begin_native_selection(MouseButton::Left, Point::new(1, 1));
        app.finish_native_selection();
        assert_eq!(app.native_selection_text().as_deref(), Some(expected));
    }
    app.modifiers = ModifiersState::ALT;
    app.begin_native_selection(MouseButton::Left, Point::new(1, 8));
    app.extend_native_selection(Point::new(0, 6));
    app.finish_native_selection();
    assert_eq!(app.native_selection_text().as_deref(), Some("one\ntwo"));
    app.enter_copy_mode();
    assert!(app.mouse_selection_source.is_none());
    app.move_copy_cursor(crate::terminal::copy_mode::CopyMotion::Top);
    app.copy_mode
        .toggle_selection(termeric_core::SelectionType::Simple);
    app.move_copy_cursor(crate::terminal::copy_mode::CopyMotion::LineEnd);
    assert!(!app.reconcile_mouse_selection_presentation());
    assert_eq!(app.copy_mode_selection_text().as_deref(), Some("match one"));
}

#[test]
fn filtered_selection_scrolls_across_result_pages_without_copying_hidden_rows() {
    use termeric_core::Point;
    for split in [false, true] {
        let mut app = if split {
            divider_owner_fixture()
        } else {
            scrollbar_owner_fixture()
        };
        let terminal = app.active_terminal_mut().unwrap();
        terminal.resize(12, 3);
        terminal.process(b"\x1bc\x1b[3J");
        for index in 0..10 {
            terminal.process(format!("hit{index}\r\nSECRET{index}\r\n").as_bytes());
        }
        app.search.start();
        app.with_search_and_active_grid(|search, grid| {
            search.set_query("hit", grid);
            search.finish(grid);
        });
        app.search.toggle_filter();
        let index_ptr = app.search.filtered_matching_rows().as_ptr();
        let index_bytes = app.search.result_index_memory_bytes();
        let generation = app.search.filter_generation();
        app.begin_native_selection(MouseButton::Left, Point::new(0, 0));
        app.search.scroll_filter(-4.0, 3);
        assert_eq!(app.search.filter_generation(), generation);
        app.extend_native_selection(Point::new(2, 3));
        app.finish_native_selection();
        let text = app.native_selection_text().unwrap();
        assert_eq!(
            text.lines().map(str::trim_end).collect::<Vec<_>>(),
            ["hit0", "hit1", "hit2", "hit3", "hit4", "hit5", "hit6"]
        );
        assert_eq!(app.search.filtered_matching_rows().as_ptr(), index_ptr);
        assert_eq!(app.search.result_index_memory_bytes(), index_bytes);
        app.search.cancel();
        assert!(app.native_selection_text().is_none());
        assert!(app.reconcile_mouse_selection_presentation());
        assert!(app.active_terminal().unwrap().selection().is_none());
    }
}

#[test]
fn filtered_selection_invalidates_on_query_restart_toggle_or_focus_change() {
    use termeric_core::Point;
    for change in ["query", "toggle", "focus"] {
        let mut app = divider_owner_fixture();
        app.active_terminal_mut()
            .unwrap()
            .process(b"\x1bc\x1b[3Jhit one\r\nSECRET\r\nhit two");
        app.search.start();
        app.with_search_and_active_grid(|search, grid| {
            search.set_query("hit", grid);
            search.finish(grid);
        });
        app.search.toggle_filter();
        let owner = app.focused_pointer_terminal().unwrap();
        app.begin_native_selection(MouseButton::Left, Point::new(0, 0));
        app.extend_native_selection(Point::new(1, 6));
        assert!(app.native_selection_text().is_some());
        match change {
            "query" => {
                app.with_search_and_active_grid(|search, grid| {
                    search.set_query("hit", grid);
                    search.finish(grid);
                });
            }
            "toggle" => {
                app.search.toggle_filter();
                app.search.toggle_filter();
            }
            "focus" => {
                let panes = app
                    .tabs
                    .as_mut()
                    .unwrap()
                    .active_tab_mut()
                    .unwrap()
                    .pane_manager
                    .as_mut()
                    .unwrap();
                let other = panes
                    .pane_ids()
                    .into_iter()
                    .find(|id| *id != panes.focused_id())
                    .unwrap();
                panes.focus(other);
            }
            _ => unreachable!(),
        }
        assert!(app.native_selection_text().is_none(), "{change}");
        assert!(app.reconcile_mouse_selection_presentation(), "{change}");
        assert!(!app.mouse_pressed);
        let tabs = app.tabs.as_ref().unwrap();
        let origin = tabs.origin_for_pane(owner.tab_id, owner.pane_id).unwrap();
        assert!(
            tabs.terminal_for_origin(origin)
                .unwrap()
                .selection()
                .is_none(),
            "{change}"
        );
    }
}

#[test]
fn standalone_search_does_not_find_hidden_synchronized_output() {
    let mut app = scrollbar_owner_fixture();
    app.active_terminal_mut()
        .unwrap()
        .process(b"\x1bc\x1b[3Jvisible\x1b[?2026h\x1b[H\x1b[2Khidden");
    app.search.start();
    app.with_search_and_active_grid(|search, grid| {
        search.set_query("hidden", grid);
        search.finish(grid);
    });
    assert_eq!(app.search.match_count(), 0);
}

#[test]
fn standalone_search_restarts_partial_and_confirmed_results_at_snapshot_boundaries() {
    for split in [false, true] {
        let mut app = if split {
            divider_owner_fixture()
        } else {
            scrollbar_owner_fixture()
        };
        let terminal = app.active_terminal_mut().unwrap();
        terminal.resize(12, 32);
        terminal.process(b"\x1bc\x1b[3Jneedle");
        app.search.start();
        app.with_search_and_active_grid(|search, grid| {
            search.set_query("needle", grid);
            assert!(search.advance_rows(grid, 1));
        });
        app.search.toggle_filter();
        app.active_terminal_mut()
            .unwrap()
            .process(b"\x1b[?2026h\x1b[H\x1b[2Khidden");
        app.with_search_and_active_grid(|search, grid| search.finish(grid));
        assert_eq!(app.search.match_count(), 1);
        assert_eq!(app.search.filtered_rows(32), &[0]);
        app.search.confirm();
        let terminal = app.active_terminal_mut().unwrap();
        terminal.expire_synchronized_output(terminal.synchronized_output_deadline().unwrap());
        app.reconcile_search_presentation();
        assert!(app.search.is_searching());
        assert!(!app.search.is_active());
        assert_eq!(app.search.query(), "needle");
        assert!(app.search.is_filter_mode());
        assert!(app.search.filtered_rows(32).is_empty());
        app.with_search_and_active_grid(|search, grid| search.finish(grid));
        assert_eq!(app.search.match_count(), 0);
        app.active_terminal_mut()
            .unwrap()
            .process(b"\x1b[H\x1b[2Kneedle\x1b[?2026h");
        app.with_search_and_active_grid(|search, grid| search.finish(grid));
        assert_eq!(app.search.match_count(), 1);
        app.active_terminal_mut()
            .unwrap()
            .process(b"\x1b[?2026l\x1b[H\x1b[2Khidden\x1b[?2026h");
        app.with_search_and_active_grid(|search, grid| search.finish(grid));
        assert_eq!(app.search.match_count(), 0);
    }
}

#[test]
fn standalone_search_navigation_preserves_hidden_live_scroll_and_reflows_results() {
    let mut app = scrollbar_owner_fixture();
    let terminal = app.active_terminal_mut().unwrap();
    terminal.resize(12, 3);
    terminal.process(b"\x1bc\x1b[3Jold\r\nvisible\r\ntail\r\na\r\nb\r\nc");
    terminal.grid_mut().set_scroll_offset(3);
    terminal.process(b"\x1b[?2026h");
    app.search.start();
    app.with_search_and_active_grid(|search, grid| {
        search.set_query("visible", grid);
        search.finish(grid);
    });
    assert_eq!(app.search.current().unwrap().row, 1);
    app.scroll_to_current_match();
    assert_eq!(app.active_terminal().unwrap().grid().scroll_offset(), 3);
    app.active_terminal_mut()
        .unwrap()
        .process(b"\x1b[?2026l\x1bc\x1b[3Jabcdef");
    app.with_search_and_active_grid(|search, grid| {
        search.set_query("def", grid);
        search.finish(grid);
    });
    assert_eq!(app.search.current().unwrap().start_col, 3);
    app.active_terminal_mut().unwrap().resize(3, 5);
    app.with_search_and_active_grid(|search, grid| search.finish(grid));
    let found = app.search.current().unwrap();
    assert_eq!((found.row, found.start_col), (1, 0));
}

#[test]
fn command_output_search_never_rebinds_a_live_row_scope_to_a_frozen_viewport() {
    let mut app = scrollbar_owner_fixture();
    app.active_terminal_mut()
        .unwrap()
        .process(b"\x1bc\x1b[3Jvisible");
    assert!(app.start_command_output_search(0, 0));
    app.with_search_and_active_grid(|search, grid| {
        search.set_query("visible", grid);
        search.finish(grid);
    });
    assert_eq!(app.search.match_count(), 1);
    app.active_terminal_mut().unwrap().process(b"\x1b[?2026h");
    let mut edited = false;
    assert!(
        app.with_search_and_active_grid(|search, grid| {
            edited = true;
            search.set_query("unscoped", grid);
        })
        .is_none()
    );
    assert!(
        !edited,
        "cancelling a scope must not apply the pending edit globally"
    );
    assert!(!app.search.is_active());
    assert!(!app.search.is_scoped());
    assert!(!app.start_command_output_search(0, 0));
    assert!(!app.search.is_active());
    app.active_terminal_mut().unwrap().process(b"\x1b[?2026l");
    assert!(app.start_command_output_search(0, 0));
    app.active_terminal_mut().unwrap().resize(7, 5);
    assert!(app.reconcile_search_presentation());
    assert!(!app.search.is_scoped());
}

#[test]
fn standalone_search_drops_completed_results_when_the_displayed_screen_switches() {
    let mut app = scrollbar_owner_fixture();
    app.active_terminal_mut()
        .unwrap()
        .process(b"\x1bc\x1b[3Jprimary");
    app.search.start();
    app.with_search_and_active_grid(|search, grid| {
        search.set_query("primary", grid);
        search.finish(grid);
    });
    assert_eq!(app.search.match_count(), 1);
    app.active_terminal_mut()
        .unwrap()
        .process(b"\x1b[?1049halternate");
    app.with_search_and_active_grid(|search, grid| search.finish(grid));
    assert_eq!(app.search.match_count(), 0);
    app.with_search_and_active_grid(|search, grid| {
        search.set_query("alternate", grid);
        search.finish(grid);
    });
    assert_eq!(app.search.match_count(), 1);
    app.active_terminal_mut().unwrap().process(b"\x1b[?1049l");
    app.with_search_and_active_grid(|search, grid| search.finish(grid));
    assert_eq!(app.search.match_count(), 0);
}

#[test]
fn focused_url_refresh_uses_the_synchronized_presentation() {
    for split in [false, true] {
        let mut app = if split {
            divider_owner_fixture()
        } else {
            scrollbar_owner_fixture()
        };
        app.active_terminal_mut().unwrap().process(
            b"\x1bc\x1b[3Jhttps://visible.test\r\n\x1b]8;;https://semantic.test\x1b\\https://label.test\x1b]8;;\x1b\\\x1b[?2026h\x1b[H\x1b[2Jhttps://hidden.test",
        );
        app.refresh_focused_terminal_surfaces();
        assert_eq!(
            app.url_cache.url_at(0, 0).map(|url| url.url.as_str()),
            Some("https://visible.test"),
            "split={split}"
        );
        assert_eq!(
            app.focused_hyperlink_at(termeric_core::Point::new(0, 0)),
            Some("https://visible.test")
        );
        assert_eq!(
            app.focused_hyperlink_at(termeric_core::Point::new(1, 0)),
            Some("https://semantic.test")
        );

        let terminal = app.active_terminal_mut().unwrap();
        terminal.set_scrollback_limits(100, 1024 * 1024);
        let lines = b"\r\n".repeat(terminal.rows() + 4);
        terminal.process(&lines);
        terminal.grid_mut().set_scroll_offset(3);
        app.refresh_focused_terminal_surfaces();
        assert_eq!(
            app.focused_hyperlink_at(termeric_core::Point::new(-3, 0)),
            Some("https://visible.test")
        );
        assert_eq!(
            app.focused_hyperlink_at(termeric_core::Point::new(-2, 0)),
            Some("https://semantic.test")
        );
        assert_eq!(
            app.focused_hyperlink_at(termeric_core::Point::new(-4, 0)),
            None
        );
        assert_eq!(
            app.focused_hyperlink_at(termeric_core::Point::new(-3, usize::MAX)),
            None
        );
        app.active_terminal_mut().unwrap().process(b"\x1b[?2026l");
        app.refresh_focused_terminal_surfaces();
        assert_eq!(
            app.focused_hyperlink_at(termeric_core::Point::new(-3, 0)),
            None
        );
    }
}
