//! Frame scheduling: synchronized output, cursor motion, deadlines, and occlusion.

use super::*;

#[test]
fn idle_deadline_releases_stalled_synchronized_output_and_requests_the_final_frame() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let mut tabs = TabManager::for_test(ShellConfig::default());
    tabs.new_tab_with_command(
        "/bin/sh",
        &crate::test_support::sleeping_args(),
        None,
        Some("synchronized-output-timeout"),
        None,
        HashMap::new(),
    )
    .unwrap();
    app.tabs = Some(tabs);
    app.active_terminal_mut()
        .unwrap()
        .process(b"before\x1b[?2026h\rafter");
    let deadline = app
        .active_terminal()
        .unwrap()
        .synchronized_output_deadline()
        .unwrap();
    app.window_occluded = true;
    app.surface_lost = true;

    assert_eq!(app.next_synchronized_output_deadline(), Some(deadline));
    // Construction asks for its first frame; this test starts after it.
    app.redraw_scheduler.begin_frame();
    assert!(!app.redraw_scheduler.has_pending());

    app.poll_idle_sources(deadline);

    assert!(!app.active_terminal().unwrap().is_synchronized_output());
    assert!(app.redraw_scheduler.has_pending());
    assert!(
        app.active_terminal()
            .unwrap()
            .presentation_grid()
            .row(0)
            .unwrap()
            .to_string()
            .starts_with("after")
    );
}

#[test]
fn cursor_motion_preserves_new_tab_snaps_and_animates_existing_tab_switches() {
    use crate::ui::cursor_motion::CURSOR_MOVE_DURATION;
    let mut app = App::new_headless(Config::default()).unwrap();
    let now = Instant::now();
    assert_eq!(
        app.update_cursor_animation_at(5, 6, true, cursor_terminal(1, 0), now),
        (5.0, 6.0)
    );
    assert!(!app.cursor_motion.is_active());
    let moved = now + Duration::from_secs(1);
    // An old frame gap no longer accidentally disables the transition.
    assert_eq!(
        app.update_cursor_animation_at(50, 20, true, cursor_terminal(1, 0), moved),
        (5.0, 6.0)
    );
    assert!(app.cursor_motion.is_active());
    assert_eq!(
        app.update_cursor_animation_at(
            50,
            20,
            true,
            cursor_terminal(1, 0),
            moved + CURSOR_MOVE_DURATION
        ),
        (50.0, 20.0)
    );
    assert!(!app.cursor_motion.is_active());
    let switched = moved + Duration::from_secs(1);
    assert_eq!(
        app.update_cursor_animation_at(100, 50, true, cursor_terminal(2, 0), switched),
        (100.0, 50.0)
    );
    assert!(!app.cursor_motion.is_active());
    assert_eq!(
        app.update_cursor_animation_at(5, 6, true, cursor_terminal(1, 0), switched),
        (100.0, 50.0)
    );
    assert!(app.cursor_motion.is_active());
    assert_eq!(
        app.update_cursor_animation_at(
            5,
            6,
            true,
            cursor_terminal(1, 0),
            switched + CURSOR_MOVE_DURATION
        ),
        (5.0, 6.0)
    );
    assert!(!app.cursor_motion.is_active());
}

#[test]
fn cursor_motion_cancels_for_hidden_cursors_reappearance_reduced_motion_and_no_tab() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let now = Instant::now();
    app.update_cursor_animation_at(0, 0, true, cursor_terminal(1, 0), now);
    app.update_cursor_animation_at(20, 29, true, cursor_terminal(1, 0), now);
    assert!(app.cursor_motion.is_active());
    for (step, col, visible) in [(1, 30, false), (2, 40, false), (3, 50, true)] {
        assert_eq!(
            app.update_cursor_animation_at(
                col,
                29,
                visible,
                cursor_terminal(1, 0),
                now + Duration::from_millis(step)
            ),
            (col as f32, 29.0)
        );
        assert!(!app.cursor_motion.is_active());
    }
    app.update_cursor_animation_at(
        60,
        29,
        true,
        cursor_terminal(1, 0),
        now + Duration::from_millis(4),
    );
    assert!(app.cursor_motion.is_active());
    app.config.accessibility.reduce_motion = crate::config::AccessibilityPreference::On;
    assert_eq!(
        app.update_cursor_animation_at(
            70,
            29,
            true,
            cursor_terminal(1, 0),
            now + Duration::from_millis(5)
        ),
        (70.0, 29.0)
    );
    assert!(!app.cursor_motion.is_active());
    app.config.accessibility.reduce_motion = crate::config::AccessibilityPreference::Off;
    app.update_cursor_animation_at(
        80,
        29,
        true,
        cursor_terminal(1, 0),
        now + Duration::from_millis(6),
    );
    assert!(app.cursor_motion.is_active());
    assert_eq!(
        app.update_cursor_animation_at(0, 0, false, None, now + Duration::from_millis(7)),
        (0.0, 0.0)
    );
    assert!(!app.cursor_motion.is_active());
}

#[test]
fn cursor_motion_snaps_when_focus_moves_to_another_pane_of_the_tab() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let now = Instant::now();
    app.update_cursor_animation_at(5, 6, true, cursor_terminal(1, 0), now);
    assert_eq!(
        app.update_cursor_animation_at(30, 2, true, cursor_terminal(1, 1), now),
        (30.0, 2.0)
    );
    assert!(!app.cursor_motion.is_active());
    // Moving within the newly focused pane still glides.
    app.update_cursor_animation_at(31, 2, true, cursor_terminal(1, 1), now);
    assert!(app.cursor_motion.is_active());
}

#[test]
fn cursor_motion_scheduler_follows_presented_state_during_synchronized_output() {
    let mut app = scrollbar_owner_fixture();
    let id = app.tabs.as_ref().unwrap().active_tab_id().unwrap();
    let now = Instant::now();
    app.active_terminal_mut()
        .unwrap()
        .process(b"\x1b[HA\x1b[?2026h\x1b[5;40H");
    let presented = app.active_terminal().unwrap().presentation_grid().cursor();
    let (col, row, visible) = (presented.col, presented.row, presented.visible);
    assert_eq!((col, row), (1, 0));
    assert_ne!(app.active_terminal().unwrap().grid().cursor().col, col);
    app.update_cursor_animation_at(col, row, visible, cursor_terminal(id, 0), now);
    assert!(
        !app.cursor_motion.is_active(),
        "unpresented live-grid movement must not sustain redraws"
    );
    app.active_terminal_mut().unwrap().process(b"\x1b[?2026l");
    let presented = app.active_terminal().unwrap().presentation_grid().cursor();
    let (col, row, visible) = (presented.col, presented.row, presented.visible);
    app.update_cursor_animation_at(col, row, visible, cursor_terminal(id, 0), now);
    assert!(app.cursor_motion.is_active());
    assert_eq!(
        app.update_cursor_animation_at(
            col,
            row,
            visible,
            cursor_terminal(id, 0),
            now + crate::ui::cursor_motion::CURSOR_MOVE_DURATION
        ),
        (39.0, 4.0)
    );
    assert!(!app.cursor_motion.is_active());
}

#[test]
fn occluded_and_surface_lost_windows_suspend_presentation_only_deadlines() {
    let mut app = App::new_headless(Config::default()).unwrap();
    // Consume the intentionally overdue first Git poll so maintenance timing
    // cannot mask the presentation-deadline assertions below.
    app.poll_idle_sources(Instant::now());
    app.redraw_scheduler.begin_frame();

    let now = Instant::now();
    app.last_config_check = now;
    app.last_frame_time = now;
    app.focus_change_time = now;
    assert_eq!(
        app.next_idle_deadline(now),
        Some(now + Duration::from_millis(16))
    );

    app.window_occluded = true;
    assert!(
        app.next_idle_deadline(now).unwrap() >= now + Duration::from_millis(500),
        "occlusion retained a presentation-cadence wake"
    );
    app.poll_idle_sources(now + Duration::from_millis(32));
    assert!(!app.redraw_scheduler.has_pending());

    app.window_occluded = false;
    app.surface_lost = true;
    assert!(
        app.next_idle_deadline(now).unwrap() >= now + Duration::from_millis(500),
        "surface loss retained a presentation-cadence wake"
    );
    app.poll_idle_sources(now + Duration::from_millis(48));
    assert!(!app.redraw_scheduler.has_pending());
}

#[test]
fn unicode_worker_completion_polls_only_while_pending_even_when_occluded() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.poll_idle_sources(Instant::now());
    app.redraw_scheduler.begin_frame();
    app.window_occluded = true;
    let now = Instant::now();
    app.last_config_check = now;
    app.unicode_picker.open();
    app.unicode_picker.set_query("snowman");
    assert_eq!(
        app.next_idle_deadline(now),
        Some(now + Duration::from_millis(16))
    );
    let limit = now + Duration::from_secs(5);
    while app.unicode_picker.search_pending() {
        app.poll_idle_sources(Instant::now());
        assert!(Instant::now() < limit);
        std::thread::yield_now();
    }
    assert!(app.redraw_scheduler.has_pending());
    assert!(!app.unicode_picker.build_info().entries.is_empty());
    let now = Instant::now();
    app.last_config_check = now;
    app.poll_idle_sources(now);
    assert!(app.next_idle_deadline(now).unwrap() >= now + Duration::from_millis(500));
}

#[test]
fn surface_loss_has_one_deferred_recovery_deadline_instead_of_indefinite_pause() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.poll_idle_sources(Instant::now());
    let now = Instant::now();
    app.last_config_check = now;
    app.surface_lost = true;
    app.surface_lost_at = Some(now);

    let recovery_at = now + super::renderer_recovery::SURFACE_RECOVERY_DELAY;
    assert_eq!(app.next_idle_deadline(now), Some(recovery_at));
    app.poll_idle_sources(recovery_at - Duration::from_millis(1));
    assert!(app.surface_lost);
    assert!(!app.should_exit);

    // A headless controller has no native window to recreate. Reaching that
    // branch proves the deadline fired once; production retains its window and
    // reinstalls a renderer while leaving the terminal/PTY state untouched.
    app.poll_idle_sources(recovery_at);
    assert!(!app.surface_lost);
    assert!(app.surface_lost_at.is_none());
    assert!(app.should_exit);
}

#[test]
fn broadcast_arming_expiry_is_an_exact_idle_deadline_and_requests_cleanup_frame() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.poll_idle_sources(Instant::now());
    app.redraw_scheduler.begin_frame();
    let now = Instant::now();
    let expires = now + Duration::from_millis(100);
    app.last_config_check = now;
    app.window_occluded = true;
    app.broadcast = BroadcastState::Armed {
        tab_id: 1,
        expires,
        targets: vec![PaneId(1), PaneId(2)],
    };

    assert_eq!(app.next_idle_deadline(now), Some(expires));
    app.poll_idle_sources(expires - Duration::from_millis(1));
    assert!(matches!(app.broadcast, BroadcastState::Armed { .. }));
    app.poll_idle_sources(expires);
    assert!(matches!(app.broadcast, BroadcastState::Inactive));
    assert!(app.redraw_scheduler.has_pending());
}

#[test]
fn kitty_animation_uses_its_exact_deadline_and_requests_one_frame() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let mut tabs = TabManager::new(8, 4, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
    tabs.new_tab_with_command(
        "/bin/sh",
        &crate::test_support::sleeping_args(),
        None,
        Some("kitty-animation-deadline"),
        None,
        HashMap::new(),
    )
    .unwrap();
    app.tabs = Some(tabs);
    app.active_terminal_mut().unwrap().process(
        b"\x1b_Ga=T,f=32,s=1,v=1,i=73,c=1,r=1,C=1;AQIDBA==\x1b\\\x1b_Ga=f,f=32,s=1,v=1,i=73,z=10;BQYHCA==\x1b\\\x1b_Ga=a,i=73,r=1,z=10,s=3\x1b\\",
    );
    let deadline = app.next_graphics_animation_deadline().unwrap();
    assert_eq!(
        app.active_terminal().unwrap().presentation_images()[0]
            .image
            .data
            .as_ref(),
        &[1, 2, 3, 4]
    );
    app.redraw_scheduler.begin_frame();
    assert!(!app.redraw_scheduler.has_pending());

    app.poll_idle_sources(deadline);

    assert!(app.redraw_scheduler.has_pending());
    assert_eq!(
        app.active_terminal().unwrap().presentation_images()[0]
            .image
            .data
            .as_ref(),
        &[5, 6, 7, 8]
    );
}

#[test]
fn synchronized_output_timeout_is_terminal_local_even_when_its_pane_is_unfocused() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let shell = crate::test_support::sleeping_shell();
    let mut panes = PaneManager::for_test(
        80,
        24,
        shell,
        1_000,
        8 * 1024 * 1024,
        800.0,
        480.0,
        10.0,
        20.0,
    )
    .unwrap();
    let right = panes
        .split(
            SplitDirection::Vertical,
            crate::config::PaneCwdMode::Focused,
        )
        .unwrap();
    panes
        .get_pane_mut(right)
        .unwrap()
        .terminal
        .process(b"\x1b[?2026hhidden-update");
    let deadline = panes
        .get_pane(right)
        .unwrap()
        .terminal
        .synchronized_output_deadline()
        .unwrap();
    panes.focus(PaneId(0));

    let mut tabs = TabManager::for_test(ShellConfig::default());
    tabs.new_tab_with_command(
        "/bin/sh",
        &crate::test_support::sleeping_args(),
        None,
        Some("pane-synchronized-output-timeout"),
        None,
        HashMap::new(),
    )
    .unwrap();
    tabs.active_tab_mut().unwrap().panes = panes;
    app.tabs = Some(tabs);

    assert!(!app.active_terminal().unwrap().is_synchronized_output());
    assert_eq!(app.next_synchronized_output_deadline(), Some(deadline));

    assert_eq!(app.expire_synchronized_outputs(deadline), 1);

    let panes = app.active_panes();
    assert!(
        !panes
            .get_pane(right)
            .unwrap()
            .terminal
            .is_synchronized_output()
    );
}

#[test]
fn tab_bar_hover_fades_in_by_elapsed_time_not_frame_count() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.config.accessibility.reduce_motion = crate::config::AccessibilityPreference::Off;
    assert_eq!(app.tab_bar_hover_progress(), 0.0);

    app.tab_bar_hover = Some(crate::renderer::TabBarClick::NewTab);
    app.tab_bar_hover_since = Instant::now() - Duration::from_millis(50);
    let halfway = app.tab_bar_hover_progress();
    assert!((0.4..0.9).contains(&halfway), "{halfway}");
    app.tab_bar_hover_since = Instant::now() - Duration::from_secs(1);
    assert_eq!(app.tab_bar_hover_progress(), 1.0);

    app.tab_bar_hover_since = Instant::now();
    app.config.accessibility.reduce_motion = crate::config::AccessibilityPreference::On;
    assert_eq!(app.tab_bar_hover_progress(), 1.0);

    app.tab_bar_hover = None;
    assert_eq!(app.tab_bar_hover_progress(), 0.0);
}
