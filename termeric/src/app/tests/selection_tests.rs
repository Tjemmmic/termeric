//! Quick select, copy mode, and native selection against live and presented output.

use super::*;

#[test]
fn quick_select_reconciles_live_targets_and_viewport_before_render_or_activation() {
    let patterns = vec![("path".into(), regex::Regex::new(r"/\w+").unwrap())];
    for split in [false, true] {
        let mut app = if split {
            divider_owner_fixture()
        } else {
            scrollbar_owner_fixture()
        };
        app.compiled_copy_patterns = patterns.clone();
        let terminal = app.active_terminal_mut().unwrap();
        terminal.resize(40, 3);
        terminal.process(b"\x1bc\x1b[3J/one /two");
        app.begin_quick_select();
        let label = app.copy_mode.quick_select().unwrap().matches[1]
            .label
            .clone();
        app.active_terminal_mut()
            .unwrap()
            .process(b"\r\x1b[2K/two /new");
        assert!(!app.reconcile_quick_select_presentation());
        let quick = app.copy_mode.quick_select().unwrap();
        assert_eq!(quick.matches.len(), 1);
        assert_eq!(quick.matches[0].value, "/two");
        assert_eq!(quick.matches[0].label, label);
        assert_eq!(app.copy_mode_render_info().unwrap().hints[0].col, 0);
        app.active_terminal_mut().unwrap().process(b"\x1b[2J");
        assert!(app.reconcile_quick_select_presentation());
        assert!(app.copy_mode.quick_select().is_none());
        assert!(
            app.copy_mode
                .push_hint_key(label.chars().next().unwrap())
                .is_none()
        );
        assert!(app.copy_mode.is_active());
    }
}

#[test]
fn copy_mode_follows_live_output_before_navigation_with_or_without_selection() {
    use crate::terminal::copy_mode::CopyMotion;
    use termeric_core::{Point, SelectionType};
    for selected in [false, true] {
        let mut app = scrollbar_owner_fixture();
        let terminal = app.active_terminal_mut().unwrap();
        terminal.resize(8, 3);
        terminal.process(b"\x1bc\x1b[3Jabcdefgh\r\nijklmnop\r\nqrstuvwx");
        app.enter_copy_mode();
        app.move_copy_cursor(CopyMotion::Top);
        if selected {
            app.copy_mode.toggle_selection(SelectionType::Simple);
        }
        app.move_copy_cursor(CopyMotion::Right);
        app.move_copy_cursor(CopyMotion::Right);
        app.active_terminal_mut().unwrap().process(b"\r\nnext");
        app.reconcile_copy_mode_presentation();
        assert_eq!(app.copy_mode.cursor(), Point::new(-1, 2));
        app.move_copy_cursor(CopyMotion::Right);
        assert_eq!(app.copy_mode.cursor(), Point::new(-1, 3));
        assert_eq!(
            app.copy_mode_selection_text().as_deref(),
            selected.then_some("abcd")
        );
    }
}

#[test]
fn copy_mode_output_damage_cannot_resurrect_ranges_and_preserves_intact_cursor() {
    use crate::terminal::copy_mode::CopyMotion;
    use termeric_core::{Point, SelectionType};
    for split in [false, true] {
        let mut app = if split {
            divider_owner_fixture()
        } else {
            scrollbar_owner_fixture()
        };
        for (command, expected_cursor) in [
            (b"\x1b[1;1HX".as_slice(), Point::new(0, 2)),
            (b"\x1b[1;3HX".as_slice(), Point::new(0, 3)),
            (b"\x1b[2J".as_slice(), Point::new(2, 7)),
            (b"\x1b[3S".as_slice(), Point::new(2, 7)),
        ] {
            let terminal = app.active_terminal_mut().unwrap();
            terminal.resize(8, 3);
            terminal.process(b"\x1bc\x1b[3Jabcdefgh\r\nijklmnop\r\nqrstuvwx");
            terminal.set_scrollback_limits(0, 0);
            app.enter_copy_mode();
            app.move_copy_cursor(CopyMotion::Top);
            app.copy_mode.toggle_selection(SelectionType::Simple);
            app.move_copy_cursor(CopyMotion::Right);
            app.move_copy_cursor(CopyMotion::Right);
            app.active_terminal_mut().unwrap().process(command);
            assert!(app.copy_mode_selection_text().is_none());
            assert!(!app.reconcile_copy_mode_presentation());
            assert_eq!(app.copy_mode.cursor(), expected_cursor, "{command:?}");
            assert!(app.copy_mode.selection().is_none());
            app.move_copy_cursor(CopyMotion::Left);
            assert!(app.active_terminal().unwrap().selection().is_none());
            app.leave_copy_mode(false);
            assert!(app.active_terminal().unwrap().navigation_cursor().is_none());
        }
    }
}

#[test]
fn copy_mode_anchor_cleanup_targets_original_pane_and_screen_changes_reset_mode() {
    use crate::terminal::copy_mode::CopyMotion;
    use termeric_core::{Point, Selection, SelectionType};
    let mut app = divider_owner_fixture();
    app.enter_copy_mode();
    let target = app.copy_mode_target.unwrap();
    let tabs = app.tabs.as_mut().unwrap();
    let panes = tabs
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
    let other_term = &mut panes.get_pane_mut(other).unwrap().terminal;
    other_term.set_selection(Some(Selection::new(
        Point::new(0, 0),
        SelectionType::Simple,
    )));
    other_term.set_navigation_cursor(Some(Point::new(0, 0)));
    app.leave_copy_mode(true);
    let tabs = app.tabs.as_ref().unwrap();
    let origin = tabs
        .origin_for_pane(target.tab, target.pane.unwrap())
        .unwrap();
    assert!(
        tabs.terminal_for_origin(origin)
            .unwrap()
            .navigation_cursor()
            .is_none()
    );
    assert!(app.active_terminal().unwrap().navigation_cursor().is_some());
    assert!(app.active_terminal().unwrap().selection().is_some());

    let terminal = app.active_terminal_mut().unwrap();
    terminal.resize(8, 3);
    terminal.process(b"\x1bc\x1b[3Jtext");
    app.enter_copy_mode();
    app.move_copy_cursor(CopyMotion::Top);
    app.copy_mode.toggle_selection(SelectionType::Simple);
    app.move_copy_cursor(CopyMotion::Right);
    app.active_terminal_mut()
        .unwrap()
        .process(b"\x1b[?1049hother");
    assert!(app.copy_mode_selection_text().is_none());
    assert!(app.reconcile_copy_mode_presentation());
    assert!(app.copy_mode.selection().is_none());
    assert!(app.copy_mode.is_active());
    let terminal = app.active_terminal_mut().unwrap();
    *terminal = termeric_core::Terminal::new(8, 3);
    terminal.set_navigation_cursor(Some(Point::new(0, 0)));
    terminal.set_selection(Some(Selection::new(
        Point::new(0, 0),
        SelectionType::Simple,
    )));
    app.leave_copy_mode(true);
    assert!(app.active_terminal().unwrap().navigation_cursor().is_some());
    assert!(app.active_terminal().unwrap().selection().is_some());
}

#[test]
fn copy_mode_resize_does_not_restore_evicted_ranges_over_canonical_selection() {
    use crate::terminal::copy_mode::CopyMotion;
    for split in [false, true] {
        for surviving in [false, true] {
            let mut app = if split {
                divider_owner_fixture()
            } else {
                scrollbar_owner_fixture()
            };
            let terminal = app.active_terminal_mut().unwrap();
            terminal.resize(8, 3);
            terminal.process(b"\x1bc\x1b[3Jabcdefgh\r\nijklmnop\r\nqrstuvwx");
            terminal.grid_mut().set_scrollback_limit(1);
            app.enter_copy_mode();
            app.move_copy_cursor(if surviving {
                CopyMotion::Bottom
            } else {
                CopyMotion::Top
            });
            if surviving {
                for _ in 0..4 {
                    app.move_copy_cursor(CopyMotion::Right);
                }
            }
            app.copy_mode
                .toggle_selection(termeric_core::SelectionType::Simple);
            for _ in 0..3 {
                app.move_copy_cursor(CopyMotion::Right);
            }
            assert_eq!(
                app.copy_mode_selection_text().as_deref(),
                Some(if surviving { "uvwx" } else { "abcd" })
            );
            app.prepare_copy_mode_reflow();
            app.active_terminal_mut().unwrap().resize(2, 2);
            app.finish_copy_mode_reflow();
            assert!(app.copy_mode.is_active());
            assert_eq!(app.copy_mode.selection().is_some(), surviving);
            assert_eq!(
                app.active_terminal().unwrap().selection().is_some(),
                surviving
            );
            assert_eq!(
                app.copy_mode_selection_text().as_deref(),
                surviving.then_some("uvwx")
            );
            if !surviving {
                app.move_copy_cursor(CopyMotion::Left);
                assert!(app.copy_mode_selection_text().is_none());
            }
        }
    }
}

#[test]
fn native_width_resize_retains_reflowed_text_without_restoring_old_drag_geometry() {
    use termeric_core::Point;
    for split in [false, true] {
        let mut app = if split {
            divider_owner_fixture()
        } else {
            scrollbar_owner_fixture()
        };
        let terminal = app.active_terminal_mut().unwrap();
        terminal.resize(5, 6);
        terminal.process("\x1bc\x1b[3Jab界Zxy".as_bytes());
        app.begin_native_selection(MouseButton::Left, Point::new(0, 0));
        app.extend_native_selection(Point::new(1, 1));
        app.active_terminal_mut().unwrap().resize(3, 6);
        app.extend_native_selection(Point::new(0, 1));
        app.finish_native_selection();
        assert!(!app.mouse_pressed);
        assert!(app.mouse_state.selection.is_none());
        assert_eq!(
            app.active_terminal()
                .unwrap()
                .get_selection_text()
                .as_deref(),
            Some("ab界Zxy")
        );
    }
}

#[test]
fn native_height_resize_keeps_canonical_text_but_cancels_old_drag_coordinates() {
    use termeric_core::Point;
    for split in [false, true] {
        for retained in [false, true] {
            let mut app = if split {
                divider_owner_fixture()
            } else {
                scrollbar_owner_fixture()
            };
            let terminal = app.active_terminal_mut().unwrap();
            terminal.resize(12, 4);
            terminal.process(b"\x1bc\x1b[3Janchor\r\nsecond\r\nthird\r\nlast");
            app.begin_native_selection(MouseButton::Left, Point::new(0, 0));
            app.extend_native_selection(Point::new(0, 5));
            if !retained {
                app.active_terminal_mut()
                    .unwrap()
                    .grid_mut()
                    .set_scrollback_limit(0);
            }
            app.active_terminal_mut().unwrap().resize(12, 2);
            app.extend_native_selection(Point::new(0, 5));
            app.finish_native_selection();
            assert!(!app.mouse_pressed);
            assert!(app.mouse_state.selection.is_none());
            assert_eq!(
                app.active_terminal()
                    .unwrap()
                    .get_selection_text()
                    .as_deref(),
                retained.then_some("anchor")
            );
        }
    }
}

#[test]
fn native_selection_follows_grapheme_growth_collision_and_skip_policy() {
    use termeric_core::Point;
    for split in [false, true] {
        for skipped in [false, true] {
            let mut app = if split {
                divider_owner_fixture()
            } else {
                scrollbar_owner_fixture()
            };
            let terminal = app.active_terminal_mut().unwrap();
            terminal.resize(12, 4);
            terminal.process(b"\x1bc\x1b[3J\x1b[1;2H\x1b]66;s=2:w=2;old\x07");
            terminal.process(if skipped { b"\x1b[2;1H" } else { b"\x1b[1;1H" });
            terminal.process("❤".as_bytes());
            app.begin_native_selection(MouseButton::Left, Point::new(0, 1));
            app.extend_native_selection(Point::new(0, 4));
            app.active_terminal_mut()
                .unwrap()
                .process("\u{fe0f}".as_bytes());
            app.extend_native_selection(Point::new(0, 4));
            app.finish_native_selection();
            assert_eq!(
                app.native_selection_text().as_deref(),
                skipped.then_some("old")
            );
            assert_eq!(app.mouse_state.selection.is_some(), skipped);
        }
    }
}

#[test]
fn native_selection_tracks_horizontal_motion_and_cannot_restore_deleted_cells() {
    use termeric_core::Point;
    for split in [false, true] {
        for inserting in [false, true] {
            let mut app = if split {
                divider_owner_fixture()
            } else {
                scrollbar_owner_fixture()
            };
            let terminal = app.active_terminal_mut().unwrap();
            terminal.resize(12, 3);
            terminal.process(b"\x1bc\x1b[3J..anchor\x1b[H");
            app.begin_native_selection(MouseButton::Left, Point::new(0, 2));
            app.extend_native_selection(Point::new(0, 7));
            app.active_terminal_mut().unwrap().process(if inserting {
                b"\x1b[1@"
            } else {
                b"\x1b[1P"
            });
            app.finish_native_selection(); // Release without another movement must reconcile too.
            assert_eq!(app.native_selection_text().as_deref(), Some("anchor"));
            assert_eq!(
                app.mouse_state.selection.as_ref().unwrap().start.col,
                if inserting { 3 } else { 1 }
            );

            app.begin_native_selection(MouseButton::Left, Point::new(0, 3));
            app.extend_native_selection(Point::new(0, 5));
            app.active_terminal_mut()
                .unwrap()
                .process(b"\x1b[1;4H\x1b[P");
            app.extend_native_selection(Point::new(0, 5));
            app.finish_native_selection();
            assert!(app.native_selection_text().is_none());
            assert!(app.mouse_state.selection.is_none());
        }
    }
}

#[test]
fn native_selection_cannot_return_after_printed_output_replaces_selected_cells() {
    use termeric_core::Point;
    for split in [false, true] {
        for held in [false, true] {
            let mut app = if split {
                divider_owner_fixture()
            } else {
                scrollbar_owner_fixture()
            };
            let terminal = app.active_terminal_mut().unwrap();
            terminal.resize(12, 3);
            terminal.process(b"\x1bc\x1b[3Janchor");
            app.begin_native_selection(MouseButton::Left, Point::new(0, 0));
            app.extend_native_selection(Point::new(0, 5));
            if !held {
                app.finish_native_selection();
            }
            app.active_terminal_mut().unwrap().process(b"\x1b[Hchange");
            assert!(app.active_terminal().unwrap().selection().is_none());
            app.extend_native_selection(Point::new(0, 4));
            app.finish_native_selection();
            assert!(app.native_selection_text().is_none());
            assert!(app.mouse_state.selection.is_none());
            assert!(app.mouse_selection_source.is_none());
        }
    }
}

#[test]
fn native_selection_follows_partial_scroll_and_cannot_revive_a_split_range() {
    use termeric_core::Point;
    for split in [false, true] {
        for survives in [false, true] {
            let mut app = if split {
                divider_owner_fixture()
            } else {
                scrollbar_owner_fixture()
            };
            let terminal = app.active_terminal_mut().unwrap();
            terminal.resize(8, 5);
            terminal
                .process(b"\x1bc\x1b[3J00aaaa00\r\n11bbbb11\r\n22cccc22\r\n33dddd33\r\n44eeee44");
            app.begin_native_selection(
                MouseButton::Left,
                Point::new(2, if survives { 2 } else { 1 }),
            );
            app.extend_native_selection(Point::new(2, 5));
            app.active_terminal_mut()
                .unwrap()
                .process(b"\x1b[2;4r\x1b[?69h\x1b[3;6s\x1b[S");
            app.extend_native_selection(Point::new(1, 5));
            app.finish_native_selection();
            if survives {
                assert_eq!(
                    app.active_terminal().unwrap().selection().unwrap().start,
                    Point::new(1, 2)
                );
                assert_eq!(app.native_selection_text().as_deref(), Some("cccc"));
            } else {
                assert!(app.active_terminal().unwrap().selection().is_none());
                assert!(app.mouse_selection_source.is_none());
                assert!(app.native_selection_text().is_none());
            }
        }
    }
}

#[test]
fn native_drag_keeps_the_terminal_shifted_anchor_after_output_scrolls() {
    use termeric_core::Point;
    let mut app = scrollbar_owner_fixture();
    let terminal = app.active_terminal_mut().unwrap();
    terminal.resize(12, 3);
    terminal.process(b"\x1bc\x1b[3Janchor\r\nmiddle\r\ntail");
    app.begin_native_selection(MouseButton::Left, Point::new(0, 0));
    app.extend_native_selection(Point::new(0, 5));
    app.active_terminal_mut().unwrap().process(b"\r\nnext");
    assert_eq!(
        app.active_terminal().unwrap().selection().unwrap().start,
        Point::new(-1, 0)
    );
    app.extend_native_selection(Point::new(0, 5));
    assert_eq!(
        app.active_terminal().unwrap().selection().unwrap().start,
        Point::new(-1, 0)
    );
    assert_eq!(
        app.native_selection_text().as_deref(),
        Some("anchor      \nmiddle")
    );
}

#[test]
fn native_release_keeps_scrolled_forward_reverse_block_word_and_line_selections() {
    use termeric_core::{Point, SelectionType};
    for split in [false, true] {
        for kind in ["forward", "reverse", "block", "word", "line"] {
            let mut app = if split {
                divider_owner_fixture()
            } else {
                scrollbar_owner_fixture()
            };
            let terminal = app.active_terminal_mut().unwrap();
            terminal.resize(12, 3);
            terminal.process(b"\x1bc\x1b[3Jalpha beta\r\nmiddle\r\ntail");
            let start = Point::new(
                0,
                if matches!(kind, "reverse" | "block") {
                    4
                } else {
                    0
                },
            );
            if kind == "block" {
                app.modifiers = ModifiersState::ALT;
            }
            if matches!(kind, "word" | "line") {
                app.mouse_state.click_count = if kind == "word" { 1 } else { 2 };
                app.mouse_state.last_click_pos = Some(start);
                app.mouse_state.last_click_time = Instant::now();
            }
            app.begin_native_selection(MouseButton::Left, start);
            if !matches!(kind, "word" | "line") {
                app.extend_native_selection(Point::new(0, if kind == "forward" { 4 } else { 0 }));
            }
            let expected = if kind == "line" {
                "alpha beta"
            } else {
                "alpha"
            };
            assert_eq!(app.native_selection_text().as_deref(), Some(expected));
            let before = app.active_terminal().unwrap().selection().unwrap().clone();
            app.active_terminal_mut().unwrap().process(b"\r\nnext");
            // This same finalizer handles mouse-up, focus loss, and modal takeover.
            app.finish_native_pointer_gesture();
            let selection = app.active_terminal().unwrap().selection().unwrap();
            assert_eq!(
                selection.start,
                Point::new(before.start.row - 1, before.start.col)
            );
            assert_eq!(
                selection.end,
                Point::new(before.end.row - 1, before.end.col)
            );
            assert_eq!(selection.ty, before.ty);
            if kind == "block" {
                assert_eq!(selection.ty, SelectionType::Block);
            }
            assert!(!selection.active);
            assert!(!app.mouse_pressed);
            assert_eq!(
                app.native_selection_text().as_deref(),
                Some(expected),
                "split={split} kind={kind}"
            );
            assert!(app.mouse_state.last_click_pos.is_none());
        }
    }
}

#[test]
fn native_drag_cannot_resurrect_evicted_cleared_or_reset_selection() {
    use termeric_core::Point;
    for split in [false, true] {
        for cause in ["line limit", "byte limit", "clear history", "reset"] {
            for release_only in [false, true] {
                let mut app = if split {
                    divider_owner_fixture()
                } else {
                    scrollbar_owner_fixture()
                };
                let terminal = app.active_terminal_mut().unwrap();
                terminal.resize(12, 3);
                terminal.process(b"\x1bc\x1b[3Janchor\r\nmiddle\r\ntail");
                app.begin_native_selection(MouseButton::Left, Point::new(0, 0));
                app.extend_native_selection(Point::new(0, 5));
                let terminal = app.active_terminal_mut().unwrap();
                terminal.process(b"\r\nnext");
                match cause {
                    "line limit" => terminal.set_scrollback_limits(0, 1024 * 1024),
                    "byte limit" => terminal.set_scrollback_limits(100, 0),
                    "clear history" => terminal.process(b"\x1b[3J"),
                    "reset" => terminal.process(b"\x1bc"),
                    _ => unreachable!(),
                }
                assert!(terminal.selection().is_none());
                if !release_only {
                    app.extend_native_selection(Point::new(0, 3));
                }
                app.finish_native_pointer_gesture();
                assert!(!app.mouse_pressed, "{cause}");
                assert!(app.mouse_selection_source.is_none());
                assert!(app.mouse_state.selection.is_none());
                assert!(app.active_terminal().unwrap().selection().is_none());
                assert!(app.native_selection_text().is_none());
            }
        }
    }
}

#[test]
fn native_drag_tracks_archived_viewport_and_user_scrollback_without_reanchoring() {
    use termeric_core::Point;
    let mut app = scrollbar_owner_fixture();
    let terminal = app.active_terminal_mut().unwrap();
    terminal.resize(12, 3);
    terminal.process(b"\x1bc\x1b[3Janchor\r\nmiddle\r\ntail");
    app.begin_native_selection(MouseButton::Left, Point::new(0, 0));
    app.extend_native_selection(Point::new(0, 5));
    app.active_terminal_mut().unwrap().process(b"\x1b[22J");
    app.active_terminal_mut()
        .unwrap()
        .grid_mut()
        .set_scroll_offset(3);
    app.extend_native_selection(Point::new(-2, 5));
    app.finish_native_selection();
    let selection = app.active_terminal().unwrap().selection().unwrap();
    assert_eq!(selection.start, Point::new(-3, 0));
    assert_eq!(selection.end, Point::new(-2, 5));
    assert_eq!(
        app.native_selection_text().as_deref(),
        Some("anchor      \nmiddle")
    );
}

#[test]
fn native_completed_selection_cache_tracks_scroll_and_eviction_without_breaking_double_click() {
    use termeric_core::{Point, SelectionType};
    let mut app = scrollbar_owner_fixture();
    let terminal = app.active_terminal_mut().unwrap();
    terminal.resize(12, 3);
    terminal.process(b"\x1bc\x1b[3Janchor\r\nmiddle\r\ntail");
    app.begin_native_selection(MouseButton::Left, Point::new(0, 0));
    app.finish_native_selection();
    assert!(app.mouse_state.selection.is_none());
    assert_eq!(app.mouse_state.click_count, 1);
    app.mouse_state.last_click_time = Instant::now();
    app.begin_native_selection(MouseButton::Left, Point::new(0, 0));
    assert_eq!(
        app.active_terminal().unwrap().selection().unwrap().ty,
        SelectionType::Semantic
    );
    app.finish_native_selection();
    app.active_terminal_mut().unwrap().process(b"\r\nnext");
    assert!(!app.reconcile_mouse_selection_presentation());
    assert_eq!(
        app.mouse_state.selection.as_ref().unwrap().start,
        Point::new(-1, 0)
    );
    assert_eq!(app.native_selection_text().as_deref(), Some("anchor"));
    app.active_terminal_mut().unwrap().process(b"\x1b[3J");
    assert!(app.reconcile_mouse_selection_presentation());
    assert!(app.mouse_state.selection.is_none());
    assert!(app.mouse_selection_source.is_none());
    assert!(app.native_selection_text().is_none());
}

#[test]
fn native_selection_release_clears_plain_clicks_but_keeps_one_cell_explicit_selections() {
    use termeric_core::Point;
    let mut app = scrollbar_owner_fixture();
    app.active_terminal_mut()
        .unwrap()
        .process(b"\x1bc\x1b[3Jx\x1b[?2026h\x1b[Hhidden");
    for (previous_clicks, modifiers, expected) in [
        (0, ModifiersState::empty(), None),
        (1, ModifiersState::empty(), Some("x")),
        (2, ModifiersState::empty(), Some("x")),
        (0, ModifiersState::ALT, Some("x")),
    ] {
        app.modifiers = modifiers;
        app.mouse_state.click_count = previous_clicks;
        app.mouse_state.last_click_pos = Some(Point::new(0, 0));
        app.mouse_state.last_click_time = Instant::now();
        app.begin_native_selection(MouseButton::Left, Point::new(0, 0));
        app.finish_native_selection();
        assert_eq!(
            app.active_terminal()
                .unwrap()
                .get_selection_text()
                .as_deref(),
            expected
        );
    }
}

#[test]
fn native_selection_uses_scrolled_frozen_text_for_drag_word_line_and_block() {
    use termeric_core::Point;
    for split in [false, true] {
        let mut app = if split {
            divider_owner_fixture()
        } else {
            scrollbar_owner_fixture()
        };
        let terminal = app.active_terminal_mut().unwrap();
        terminal.resize(16, 3);
        terminal.process(b"\x1bc\x1b[3Jolder\r\nvisible word\r\ntail\r\na\r\nb\r\nc");
        terminal.grid_mut().set_scroll_offset(3);
        terminal.process(b"\x1b[?2026h\x1b[H\x1b[2KHIDDEN");
        terminal.grid_mut().set_scroll_offset(3);
        let point = Point::new(-2, 0); // visual row 1 despite hidden live offset 3
        app.begin_native_selection(MouseButton::Left, point);
        app.extend_native_selection(Point::new(-2, 6));
        app.finish_native_selection();
        assert_eq!(
            app.active_terminal()
                .unwrap()
                .get_selection_text()
                .as_deref(),
            Some("visible")
        );
        assert!(!app.active_terminal().unwrap().selection().unwrap().active);
        for (click_count, expected) in [(1, "visible"), (2, "visible word")] {
            app.mouse_state.click_count = click_count;
            app.mouse_state.last_click_pos = Some(Point::new(1, 2));
            app.mouse_state.last_click_time = Instant::now();
            app.begin_native_selection(MouseButton::Left, Point::new(-2, 2));
            app.finish_native_selection();
            assert_eq!(
                app.active_terminal()
                    .unwrap()
                    .get_selection_text()
                    .as_deref(),
                Some(expected)
            );
        }
        app.modifiers = ModifiersState::ALT;
        app.begin_native_selection(MouseButton::Left, point);
        app.extend_native_selection(Point::new(-2, 6));
        assert_eq!(
            app.active_terminal().unwrap().selection().unwrap().ty,
            termeric_core::SelectionType::Block
        );
        assert_eq!(
            app.active_terminal()
                .unwrap()
                .get_selection_text()
                .as_deref(),
            Some("visible")
        );
        app.active_terminal_mut().unwrap().process(b"\x1b[?2026l");
        app.extend_native_selection(Point::new(0, 5));
        assert!(!app.mouse_pressed);
        assert!(app.mouse_state.selection.is_none());
        assert!(app.active_terminal().unwrap().selection().is_none());
    }
}

#[test]
fn native_selection_drag_rejects_snapshot_resize_and_owner_replacement() {
    use termeric_core::Point;
    for transition in ["begin", "timeout", "restart", "resize", "focus"] {
        let mut app = divider_owner_fixture();
        let terminal = app.active_terminal_mut().unwrap();
        terminal.process(b"\x1bc\x1b[3Jvisible\x1b[H");
        if transition != "begin" {
            terminal.process(b"\x1b[?2026h");
        }
        app.begin_native_selection(MouseButton::Left, Point::new(0, 0));
        app.extend_native_selection(Point::new(0, 6));
        match transition {
            "begin" => app.active_terminal_mut().unwrap().process(b"\x1b[?2026h"),
            "timeout" => {
                let terminal = app.active_terminal_mut().unwrap();
                terminal
                    .expire_synchronized_output(terminal.synchronized_output_deadline().unwrap());
            }
            "restart" => app
                .active_terminal_mut()
                .unwrap()
                .process(b"\x1b[?2026l\x1b[?2026h"),
            "resize" => app.active_terminal_mut().unwrap().resize(9, 4),
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
        app.extend_native_selection(Point::new(0, 2));
        assert!(!app.mouse_pressed, "{transition}");
        assert!(app.mouse_state.selection.is_none(), "{transition}");
        assert!(app.mouse_selection_source.is_none(), "{transition}");
        if transition == "begin" {
            assert_eq!(
                app.active_terminal()
                    .unwrap()
                    .get_selection_text()
                    .as_deref(),
                Some("visible")
            );
            assert!(app.handle_action(crate::input::Action::ClearSelection));
            assert!(app.active_terminal().unwrap().selection().is_none());
        } else {
            assert!(
                app.active_terminal().unwrap().selection().is_none(),
                "{transition}"
            );
        }
    }
}

#[test]
fn copy_mode_enters_at_the_presented_cursor_during_synchronized_output() {
    let mut app = scrollbar_owner_fixture();
    app.active_terminal_mut()
        .unwrap()
        .process(b"\x1bcvisible\x1b[H\x1b[?2026h\x1b[2Khidden\x1b[3;9H");
    app.enter_copy_mode();
    assert_eq!(app.copy_mode.cursor(), termeric_core::Point::new(0, 0));
}

#[test]
fn copy_mode_keeps_live_selection_scroll_tracking_outside_synchronized_output() {
    use crate::terminal::copy_mode::CopyMotion;
    let mut app = scrollbar_owner_fixture();
    let terminal = app.active_terminal_mut().unwrap();
    terminal.resize(12, 3);
    terminal.process(b"\x1bc\x1b[3Jvisible\r\nnext\r\nthird");
    app.enter_copy_mode();
    app.move_copy_cursor(CopyMotion::Top);
    app.copy_mode
        .toggle_selection(termeric_core::SelectionType::Simple);
    app.move_copy_cursor(CopyMotion::LineEnd);
    assert_eq!(app.copy_mode_selection_text().as_deref(), Some("visible"));
    app.active_terminal_mut().unwrap().process(b"\r\nfourth");
    assert_eq!(
        app.active_terminal()
            .unwrap()
            .selection()
            .unwrap()
            .start
            .row,
        -1
    );
    assert_eq!(app.copy_mode_selection_text().as_deref(), Some("visible"));
}

#[test]
fn copy_mode_selection_search_and_reflow_use_the_frozen_view_in_each_pane_layout() {
    use crate::terminal::copy_mode::CopyMotion;
    for split in [false, true] {
        let mut app = if split {
            divider_owner_fixture()
        } else {
            scrollbar_owner_fixture()
        };
        let terminal = app.active_terminal_mut().unwrap();
        terminal.resize(12, 4);
        terminal.process(b"\x1bcvisible\x1b[H\x1b[?2026h\x1b[2Khidden\x1b[3;9H");
        app.enter_copy_mode();
        app.copy_mode
            .toggle_selection(termeric_core::SelectionType::Simple);
        app.move_copy_cursor(CopyMotion::LineEnd);
        assert_eq!(app.copy_mode_selection_text().as_deref(), Some("visible"));
        assert!(app.active_terminal().unwrap().selection().is_none());
        assert_eq!(app.active_terminal().unwrap().grid().cursor().row, 2);
        app.with_search_and_active_grid(|search, grid| {
            search.set_query("hidden", grid);
            search.finish(grid);
        });
        assert!(!app.search.has_results());
        app.with_search_and_active_grid(|search, grid| {
            search.set_query("visible", grid);
            search.finish(grid);
        });
        assert!(app.search.has_results());
        app.prepare_copy_mode_reflow();
        app.active_terminal_mut().unwrap().resize(4, 6);
        app.finish_copy_mode_reflow();
        assert_eq!(app.copy_mode_selection_text().as_deref(), Some("visible"));
        assert!(!app.reconcile_copy_mode_presentation());
        app.active_terminal_mut().unwrap().process(b"\x1b[?2026l");
        // Copy itself refuses stale coordinates even before render/input reconciliation.
        assert!(app.copy_mode_selection_text().is_none());
        assert!(app.reconcile_copy_mode_presentation());
        assert!(app.copy_mode.is_active());
        assert!(app.copy_mode.selection().is_none());
        assert!(!app.search.has_results());
        assert!(!app.reconcile_copy_mode_presentation());
    }
}

#[test]
fn copy_mode_snapshot_boundaries_reset_selection() {
    use crate::terminal::copy_mode::CopyMotion;
    for transition in ["begin", "timeout", "restart", "alternate"] {
        let mut app = scrollbar_owner_fixture();
        let terminal = app.active_terminal_mut().unwrap();
        terminal.process(b"\x1bcvisible\x1b[H");
        if transition != "begin" {
            terminal.process(b"\x1b[?2026h\x1b[2Khidden");
        }
        app.enter_copy_mode();
        app.copy_mode
            .toggle_selection(termeric_core::SelectionType::Lines);
        app.move_copy_cursor(CopyMotion::LineEnd);
        assert_eq!(app.copy_mode_selection_text().as_deref(), Some("visible"));
        let terminal = app.active_terminal_mut().unwrap();
        match transition {
            "begin" => terminal.process(b"\x1b[?2026h"),
            "timeout" => {
                terminal
                    .expire_synchronized_output(terminal.synchronized_output_deadline().unwrap());
            }
            "restart" => terminal.process(b"\x1b[?2026l\x1b[?2026h"),
            "alternate" => {
                terminal.process(b"\x1b[?1049hsecret");
                assert_eq!(app.copy_mode_selection_text().as_deref(), Some("visible"));
                app.active_terminal_mut().unwrap().process(b"\x1b[?2026l");
            }
            _ => unreachable!(),
        }
        assert!(app.copy_mode_selection_text().is_none(), "{transition}");
        assert!(app.reconcile_copy_mode_presentation(), "{transition}");
        assert!(app.copy_mode.selection().is_none(), "{transition}");
        assert!(app.copy_mode.quick_select().is_none(), "{transition}");
    }
}

#[test]
fn quick_select_pins_presented_targets_and_cwd_and_expires_without_activation() {
    let patterns = vec![("path".into(), regex::Regex::new(r"\./[^ ]+").unwrap())];
    for split in [false, true] {
        let mut app = if split {
            divider_owner_fixture()
        } else {
            scrollbar_owner_fixture()
        };
        app.compiled_copy_patterns = patterns.clone();
        app.active_terminal_mut().unwrap().process(b"\x1bc\x1b[3J\x1b]7;file:///tmp/visible\x07\x1b]8;;https://visible.test\x1b\\link\x1b]8;;\x1b\\ ./visible\x1b[?2026h\x1b[H\x1b[2K./hidden\x1b]7;file:///tmp/hidden\x07");
        app.begin_quick_select();
        let quick = app.copy_mode.quick_select().unwrap();
        assert_eq!(
            quick
                .matches
                .iter()
                .map(|item| item.value.as_str())
                .collect::<Vec<_>>(),
            ["https://visible.test", "./visible"]
        );
        assert_eq!(quick.working_directory.as_deref(), Some("/tmp/visible"));
        let point = quick.matches[0].start;
        assert_eq!(app.copy_mode_render_info().unwrap().cursor, point);
        app.copy_mode.toggle_hint_action();
        let label = app.copy_mode.quick_select().unwrap().matches[0]
            .label
            .clone();
        let mut chosen = None;
        for character in label.chars() {
            chosen = app.copy_mode.push_hint_key(character);
        }
        let (selected, action) = chosen.unwrap();
        assert_eq!(selected.value, "https://visible.test");
        assert_eq!(action, crate::terminal::copy_mode::HintAction::Copy);
        assert!(!app.reconcile_quick_select_presentation());
        let terminal = app.active_terminal_mut().unwrap();
        let deadline = terminal.synchronized_output_deadline().unwrap();
        assert!(terminal.expire_synchronized_output(deadline));
        assert!(app.reconcile_quick_select_presentation());
        assert!(app.copy_mode.quick_select().is_none());
        assert!(app.copy_mode.is_active());
        assert!(!app.reconcile_quick_select_presentation());
    }
}
