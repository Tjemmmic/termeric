//! Tab, overflow, scrollbar, and divider drags, and finishing native gestures.

use super::*;

#[test]
fn direct_tab_drag_stops_when_domain_control_becomes_pending() {
    let mut app = live_tab_drag_fixture();
    assert!(app.apply_tab_drag_target(1));
    let pending = LiveDomainControl::MoveTab {
        domain_id: "drag-domain".into(),
        remote_tab_id: 12,
        index: 0,
    };
    app.pending_live_domain_controls.insert(pending.clone());
    let order: Vec<_> = app
        .tabs
        .as_ref()
        .unwrap()
        .iter()
        .map(|(_, tab)| tab.id)
        .collect();
    assert!(
        !app.apply_tab_drag_target(2),
        "pending authority must block further pointer previews"
    );
    assert_eq!(
        app.tabs
            .as_ref()
            .unwrap()
            .iter()
            .map(|(_, tab)| tab.id)
            .collect::<Vec<_>>(),
        order
    );
    assert_eq!(app.pending_live_domain_controls.len(), 1);
    assert!(app.pending_live_domain_controls.contains(&pending));
}

#[test]
fn direct_tab_drag_refuses_closed_or_closing_targets() {
    for closing in [false, true] {
        let mut app = overflow_drag_fixture();
        let target = app.tabs.as_ref().unwrap().iter().nth(1).unwrap().1.id;
        let tab = app.tabs.as_mut().unwrap().tab_mut_by_id(target).unwrap();
        if closing {
            tab.start_closing();
        } else {
            tab.closed = true;
        }
        assert!(!app.apply_tab_drag_target(1));
        assert_eq!(app.tabs.as_ref().unwrap().active_index(), 0);
    }
}

#[test]
fn direct_tab_drag_accepts_pointer_only_input_but_repeat_requires_keyboard_focus() {
    use crate::renderer::TabBarClick::NextTab;
    let mut app = overflow_drag_fixture();
    let captured = app.tab_dragging.clone();
    app.handle_window_focus_change(false);
    assert!(app.tab_dragging.is_none());
    // A new button-down can arrive without a keyboard seat. It must not be
    // confused with resuming the previous gesture after a lost release.
    app.tab_dragging = captured;
    assert!(app.apply_tab_drag_target(1));
    let now = Instant::now();
    app.update_tab_drag_scroll(Some(NextTab), now);
    assert!(app.tab_drag_scroll.is_none());
    app.handle_window_focus_change(true);
    app.update_tab_drag_scroll(Some(NextTab), now);
    assert!(app.tab_drag_scroll.is_some());
    app.handle_window_focus_change(false);
    assert!(app.tab_drag_scroll.is_none());
    assert!(app.tab_dragging.is_none());
}

#[test]
fn tab_drag_revalidates_captured_domain_route_for_motion_dwell_and_finalization() {
    use crate::renderer::TabBarClick::NextTab;
    for change in ["remote-id", "domain", "detached", "closed", "closing"] {
        let mut app = live_tab_drag_fixture();
        assert!(app.apply_tab_drag_target(1));
        let now = Instant::now();
        app.update_tab_drag_scroll(Some(NextTab), now);
        assert!(app.tab_drag_scroll.is_some());
        let source = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        match change {
            "remote-id" => source.live_domain.as_mut().unwrap().tab_id = 99,
            "domain" => source.live_domain.as_mut().unwrap().domain_id = "replacement".into(),
            "detached" => source.live_domain = None,
            "closed" => source.closed = true,
            "closing" => source.start_closing(),
            _ => unreachable!(),
        }
        assert!(!app.apply_tab_drag_target(2), "{change}");
        app.update_tab_drag_scroll(Some(NextTab), now + Duration::from_secs(1));
        assert!(app.tab_drag_scroll.is_none(), "{change}");
        assert_eq!(app.tabs.as_ref().unwrap().active_index(), 1, "{change}");
        assert!(
            app.take_native_drag_controls().is_empty(),
            "must not address stale remote owner: {change}"
        );
    }
    // A local drag cannot silently acquire remote authority either.
    let mut app = overflow_drag_fixture();
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .live_domain = Some(crate::tabs::LiveDomainTab {
        domain_id: "new-domain".into(),
        tab_id: 99,
        pane_ids: HashMap::new(),
    });
    assert!(!app.apply_tab_drag_target(1));
    app.update_tab_drag_scroll(Some(NextTab), Instant::now());
    assert!(app.tab_drag_scroll.is_none());
}

#[test]
fn direct_tab_drag_preserves_pinned_groups_and_ignores_unrelated_domain_requests() {
    let mut app = live_tab_drag_fixture();
    app.tabs.as_mut().unwrap().all_tabs_mut()[0].pinned = true;
    app.tabs.as_mut().unwrap().all_tabs_mut()[1].pinned = true;
    app.pending_live_domain_controls
        .insert(LiveDomainControl::MoveTab {
            domain_id: "unrelated-domain".into(),
            remote_tab_id: 77,
            index: 0,
        });
    assert!(app.apply_tab_drag_target(1));
    assert!(!app.apply_tab_drag_target(2));
    assert!(app.apply_tab_drag_target(0));
    assert!(!app.apply_tab_drag_target(usize::MAX));
    assert!(
        app.take_native_drag_controls().is_empty(),
        "returned to original domain index"
    );
    assert_eq!(app.pending_live_domain_controls.len(), 1);
}

#[test]
fn occlusion_ends_native_and_application_gestures_without_focus_loss() {
    use crate::renderer::TabBarClick::NextTab;
    let mut app = overflow_drag_fixture();
    let now = Instant::now();
    app.update_tab_drag_scroll(Some(NextTab), now);
    assert!(app.tab_drag_scroll.is_some());
    let owner = app
        .tabs
        .as_ref()
        .unwrap()
        .active_tab()
        .unwrap()
        .focused_terminal_identity();
    app.mouse_pressed = true;
    app.scrollbar_dragging = Some(owner);
    app.terminal_mouse_capture = Some(TerminalMouseCapture {
        owner,
        button: MouseReportButton::Left,
    });
    app.mouse_state.handle_press(
        MouseButton::Left,
        termeric_core::Point::new(0, 0),
        ModifiersState::empty(),
    );
    app.mouse_state.handle_move(termeric_core::Point::new(0, 5));
    assert!(app.window_focused);
    app.handle_window_occlusion_change(false);
    assert!(app.mouse_pressed);
    assert!(app.tab_dragging.is_some());
    assert!(app.terminal_mouse_capture.is_some());
    app.handle_window_occlusion_change(true);
    assert!(app.window_occluded);
    assert!(
        app.window_focused,
        "occlusion must not fabricate keyboard focus changes"
    );
    assert!(!app.mouse_pressed);
    assert!(app.scrollbar_dragging.is_none());
    assert!(app.terminal_mouse_capture.is_none());
    assert!(app.tab_dragging.is_none());
    assert!(app.tab_drag_scroll.is_none());
    let selection = app.mouse_state.current_selection().unwrap();
    assert!(!selection.active);
    assert_eq!(selection.end, termeric_core::Point::new(0, 5));
    app.handle_window_occlusion_change(true);
    app.handle_window_occlusion_change(false);
    app.update_tab_drag_scroll(Some(NextTab), now + Duration::from_secs(2));
    assert!(!app.apply_tab_drag_target(1));
    assert_eq!(app.tabs.as_ref().unwrap().active_index(), 0);
    assert!(app.take_native_drag_controls().is_empty());
}

#[test]
fn overflow_drag_dwell_repeats_without_motion_or_catchup_and_keeps_owner() {
    use crate::renderer::TabBarClick::NextTab;
    let mut app = overflow_drag_fixture();
    let owner = app.tabs.as_ref().unwrap().active_tab_id();
    let now = Instant::now();
    app.update_tab_drag_scroll(Some(NextTab), now);
    assert_eq!(
        app.tab_drag_scroll.unwrap().deadline,
        now + Duration::from_millis(400)
    );
    app.update_tab_drag_scroll(Some(NextTab), now + Duration::from_millis(399));
    assert_eq!(app.tabs.as_ref().unwrap().active_index(), 0);
    // A stalled event loop must not move every remaining tab in one burst.
    app.update_tab_drag_scroll(Some(NextTab), now + Duration::from_secs(10));
    assert_eq!(app.tabs.as_ref().unwrap().active_index(), 1);
    assert_eq!(
        app.tab_drag_scroll.unwrap().deadline,
        now + Duration::from_millis(10200)
    );
    app.update_tab_drag_scroll(Some(NextTab), now + Duration::from_millis(10199));
    assert_eq!(app.tabs.as_ref().unwrap().active_index(), 1);
    app.update_tab_drag_scroll(Some(NextTab), now + Duration::from_millis(10200));
    assert_eq!(app.tabs.as_ref().unwrap().active_index(), 2);
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), owner);
    assert!(
        app.tab_drag_scroll.is_none(),
        "the boundary must not keep an idle timer alive"
    );
}

#[test]
fn overflow_drag_leaving_or_reversing_controls_starts_a_fresh_dwell() {
    use crate::renderer::TabBarClick::{NextTab, PreviousTab};
    let mut app = overflow_drag_fixture();
    let now = Instant::now();
    app.update_tab_drag_scroll(Some(NextTab), now);
    app.update_tab_drag_scroll(None, now + Duration::from_millis(399));
    assert!(app.tab_drag_scroll.is_none());
    app.update_tab_drag_scroll(Some(NextTab), now + Duration::from_secs(1));
    app.update_tab_drag_scroll(Some(NextTab), now + Duration::from_millis(1400));
    assert_eq!(app.tabs.as_ref().unwrap().active_index(), 1);
    app.update_tab_drag_scroll(Some(PreviousTab), now + Duration::from_millis(1500));
    assert_eq!(
        app.tab_drag_scroll.unwrap().deadline,
        now + Duration::from_millis(1900)
    );
    app.update_tab_drag_scroll(Some(PreviousTab), now + Duration::from_millis(1900));
    assert_eq!(app.tabs.as_ref().unwrap().active_index(), 0);
    app.finish_native_pointer_gesture();
    assert!(app.tab_drag_scroll.is_none());
    app.update_tab_drag_scroll(Some(NextTab), now + Duration::from_secs(10));
    assert!(app.tab_drag_scroll.is_none());
}

#[test]
fn overflow_drag_revalidates_ownership_visibility_and_boundaries_before_each_step() {
    use crate::renderer::TabBarClick::NextTab;
    for change in [
        "selection",
        "closed",
        "closing",
        "target-closed",
        "pinned",
        "live-domain",
        "unfocused",
        "occluded",
        "surface-lost",
        "modal",
    ] {
        let mut app = overflow_drag_fixture();
        let now = Instant::now();
        app.update_tab_drag_scroll(Some(NextTab), now);
        assert!(app.tab_drag_scroll.is_some());
        match change {
            "selection" => app.tabs.as_mut().unwrap().switch_to(1),
            "closed" => app.tabs.as_mut().unwrap().active_tab_mut().unwrap().closed = true,
            "closing" => app
                .tabs
                .as_mut()
                .unwrap()
                .active_tab_mut()
                .unwrap()
                .start_closing(),
            "target-closed" => {
                let id = app.tabs.as_ref().unwrap().iter().nth(1).unwrap().1.id;
                app.tabs.as_mut().unwrap().tab_mut_by_id(id).unwrap().closed = true;
            }
            "pinned" => app.tabs.as_mut().unwrap().active_tab_mut().unwrap().pinned = true,
            "live-domain" => {
                app.tab_dragging.as_mut().unwrap().live = Some(LiveDomainTabDrag {
                    domain_id: "other-domain".into(),
                    remote_tab_id: 1,
                    initial_index: 0,
                })
            }
            "unfocused" => app.window_focused = false,
            "occluded" => app.window_occluded = true,
            "surface-lost" => app.surface_lost = true,
            "modal" => app.settings_panel.toggle(),
            _ => unreachable!(),
        }
        let order: Vec<_> = app
            .tabs
            .as_ref()
            .unwrap()
            .iter()
            .map(|(_, tab)| tab.id)
            .collect();
        app.update_tab_drag_scroll(Some(NextTab), now + Duration::from_secs(1));
        assert!(app.tab_drag_scroll.is_none(), "{change}");
        assert_eq!(
            app.tabs
                .as_ref()
                .unwrap()
                .iter()
                .map(|(_, tab)| tab.id)
                .collect::<Vec<_>>(),
            order,
            "{change}"
        );
    }
}

#[test]
fn overflow_drag_live_preview_finalizes_once_and_respects_pending_control() {
    use crate::renderer::TabBarClick::NextTab;
    let mut app = overflow_drag_fixture();
    for (index, tab) in app
        .tabs
        .as_mut()
        .unwrap()
        .all_tabs_mut()
        .iter_mut()
        .enumerate()
    {
        tab.live_domain = Some(crate::tabs::LiveDomainTab {
            domain_id: "overflow-domain".into(),
            tab_id: index as u32 + 10,
            pane_ids: HashMap::new(),
        });
    }
    let source = app.tabs.as_ref().unwrap().active_tab_id().unwrap();
    app.tab_dragging.as_mut().unwrap().live = Some(LiveDomainTabDrag {
        domain_id: "overflow-domain".into(),
        remote_tab_id: 10,
        initial_index: 0,
    });
    let now = Instant::now();
    app.update_tab_drag_scroll(Some(NextTab), now);
    app.update_tab_drag_scroll(Some(NextTab), now + Duration::from_millis(400));
    assert_eq!(app.tabs.as_ref().unwrap().active_index(), 1);
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(source));
    let control = LiveDomainControl::MoveTab {
        domain_id: "overflow-domain".into(),
        remote_tab_id: 10,
        index: 1,
    };
    app.pending_live_domain_controls.insert(control.clone());
    app.update_tab_drag_scroll(Some(NextTab), now + Duration::from_secs(1));
    assert!(app.tab_drag_scroll.is_none());
    assert_eq!(app.tabs.as_ref().unwrap().active_index(), 1);
    // Common gesture finalization still owns the single authoritative request.
    assert_eq!(app.take_native_drag_controls(), vec![control]);
    assert!(app.take_native_drag_controls().is_empty());
}

#[test]
fn scrollbar_drag_is_canceled_when_keyboard_changes_tabs() {
    let mut app = scrollbar_owner_fixture();
    app.begin_scrollbar_drag();
    app.handle_action(crate::input::Action::NextTab);
    assert!(app.scrollbar_dragging.is_none());
}

#[test]
fn divider_drag_pending_request_interrupts_preview_and_preserves_earlier_authority() {
    for refresh in [false, true] {
        let mut app = domain_divider_pending_fixture();
        app.apply_divider_drag_position(600.0, 200.0);
        assert!(app.divider_dragging.as_ref().unwrap().last_ratio_millis > 500);
        let routes = app
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .live_domain
            .as_ref()
            .unwrap()
            .pane_ids
            .clone();
        let pending = LiveDomainControl::ResizeSplit {
            domain_id: "divider-pending".into(),
            remote_split_id: 44,
            ratio_millis: 600,
        };
        app.pending_live_domain_controls.insert(pending.clone());
        if refresh {
            app.refresh_focused_terminal_surfaces();
        } else {
            app.apply_divider_drag_position(700.0, 200.0);
        }
        assert!(
            app.divider_dragging.is_none(),
            "pending request must stop further previews"
        );
        let source = app.tabs.as_ref().unwrap().active_tab().unwrap();
        assert_eq!(source.panes.get_dividers()[0].ratio_millis, 500);
        assert_eq!(source.live_domain.as_ref().unwrap().pane_ids, routes);
        assert_eq!(app.pending_live_domain_controls.len(), 1);
        assert!(app.pending_live_domain_controls.contains(&pending));
        assert!(app.take_native_drag_controls().is_empty());
        app.pending_live_domain_controls.remove(&pending);
        app.apply_divider_drag_position(700.0, 200.0);
        assert_eq!(
            app.active_panes().get_dividers()[0].ratio_millis,
            500,
            "completing an earlier request must not revive the interrupted drag"
        );
    }
}

#[test]
fn divider_drag_unrelated_domain_requests_do_not_block_preview() {
    for mut app in [domain_divider_pending_fixture(), divider_owner_fixture()] {
        let pending = LiveDomainControl::ResizeSplit {
            domain_id: "unrelated-domain".into(),
            remote_split_id: 44,
            ratio_millis: 600,
        };
        app.pending_live_domain_controls.insert(pending.clone());
        app.apply_divider_drag_position(600.0, 200.0);
        assert!(app.divider_dragging.as_ref().unwrap().last_ratio_millis > 500);
        assert!(app.pending_live_domain_controls.contains(&pending));
    }
}

#[test]
fn occlusion_finalizes_divider_previews_and_preserves_pending_domain_authority() {
    for remote in [false, true] {
        let mut app = if remote {
            domain_divider_pending_fixture()
        } else {
            divider_owner_fixture()
        };
        app.apply_divider_drag_position(600.0, 200.0);
        let preview_ratio = app.divider_dragging.as_ref().unwrap().last_ratio_millis;
        assert!(preview_ratio > 500);
        let pending = LiveDomainControl::ResizeSplit {
            domain_id: "divider-pending".into(),
            remote_split_id: 44,
            ratio_millis: 600,
        };
        if remote {
            app.pending_live_domain_controls.insert(pending.clone());
        }
        let routes = app
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .live_domain
            .as_ref()
            .map(|live| live.pane_ids.clone());
        app.handle_window_occlusion_change(true);
        assert!(app.divider_dragging.is_none());
        assert!(app.take_native_drag_controls().is_empty());
        let expected_ratio = if remote { 500 } else { preview_ratio };
        let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
        assert_eq!(tab.panes.get_dividers()[0].ratio_millis, expected_ratio);
        assert_eq!(
            tab.live_domain.as_ref().map(|live| live.pane_ids.clone()),
            routes
        );
        if remote {
            assert_eq!(app.pending_live_domain_controls.len(), 1);
            assert!(app.pending_live_domain_controls.remove(&pending));
        }
        app.handle_window_occlusion_change(true);
        app.handle_window_occlusion_change(false);
        app.apply_divider_drag_position(700.0, 200.0);
        assert_eq!(
            app.active_panes().get_dividers()[0].ratio_millis,
            expected_ratio
        );
        assert!(app.pending_live_domain_controls.is_empty());
    }
}

#[test]
fn divider_drag_cannot_follow_a_rebound_remote_owner() {
    for change in ["domain", "remote-tab", "detached"] {
        for release_only in [false, true] {
            let mut app = domain_divider_pending_fixture();
            app.apply_divider_drag_position(600.0, 200.0);
            let ratio = app.divider_dragging.as_ref().unwrap().last_ratio_millis;
            let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
            match change {
                "domain" => {
                    tab.live_domain.as_mut().unwrap().domain_id = "replacement-domain".into()
                }
                "remote-tab" => tab.live_domain.as_mut().unwrap().tab_id = 99,
                "detached" => tab.live_domain = None,
                _ => unreachable!(),
            }
            if !release_only {
                app.apply_divider_drag_position(700.0, 200.0);
                assert!(app.divider_dragging.is_none(), "{change}");
                assert_eq!(app.active_panes().get_dividers()[0].ratio_millis, ratio);
            }
            assert!(
                app.take_native_drag_controls().is_empty(),
                "must not address a rebound remote owner: {change}"
            );
        }
    }
}

#[test]
fn divider_drag_finalization_refuses_closed_remote_sources() {
    for closing in [false, true] {
        let mut app = domain_divider_pending_fixture();
        app.apply_divider_drag_position(600.0, 200.0);
        let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        if closing {
            tab.start_closing();
        } else {
            tab.closed = true;
        }
        assert!(app.take_native_drag_controls().is_empty());
    }
}

#[test]
fn local_divider_drag_cannot_acquire_a_remote_owner() {
    let mut app = divider_owner_fixture();
    app.apply_divider_drag_position(600.0, 200.0);
    let ratio = app.divider_dragging.as_ref().unwrap().last_ratio_millis;
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .live_domain = Some(crate::tabs::LiveDomainTab {
        domain_id: "replacement-domain".into(),
        tab_id: 99,
        pane_ids: HashMap::new(),
    });
    app.apply_divider_drag_position(700.0, 200.0);
    assert!(app.divider_dragging.is_none());
    assert_eq!(app.active_panes().get_dividers()[0].ratio_millis, ratio);
    assert!(app.take_native_drag_controls().is_empty());
    assert!(app.pending_live_domain_controls.is_empty());
}

#[test]
fn divider_drag_finishes_when_keyboard_changes_tabs() {
    let mut app = divider_owner_fixture();
    app.handle_action(crate::input::Action::NextTab);
    assert!(app.divider_dragging.is_none());
}

#[test]
fn divider_drag_mutation_revalidates_owner_and_cannot_resume() {
    for transition in [
        "tab",
        "closed",
        "closing",
        "missing",
        "wrong-divider",
        "zoom",
    ] {
        let mut app = divider_owner_fixture();
        let tab_id = app.divider_dragging.as_ref().unwrap().tab_id;
        app.apply_divider_drag_position(480.0, 240.0);
        let preview_ratio = app.divider_dragging.as_ref().unwrap().last_ratio_millis;
        assert!(preview_ratio > 500);
        match transition {
            "tab" => app.tabs.as_mut().unwrap().switch_to(1),
            "closed" => app.tabs.as_mut().unwrap().active_tab_mut().unwrap().closed = true,
            "closing" => {
                app.tabs.as_mut().unwrap().close_tab(0);
            }
            "missing" => app.divider_dragging.as_mut().unwrap().tab_id = usize::MAX,
            "wrong-divider" => {
                app.divider_dragging.as_mut().unwrap().target =
                    crate::panes::DividerTarget::Local(u64::MAX)
            }
            "zoom" => {
                app.active_panes_mut().toggle_zoom();
            }
            _ => unreachable!(),
        }
        // Direct focus changes bypass refresh; the mutation boundary still
        // refuses a hidden/stale owner and consumes its gesture.
        app.apply_divider_drag_position(640.0, 240.0);
        assert!(app.divider_dragging.is_none(), "{transition}");
        if transition == "zoom" {
            app.tabs
                .as_mut()
                .unwrap()
                .tab_mut_by_id(tab_id)
                .unwrap()
                .panes
                .toggle_zoom();
        }
        app.tabs.as_mut().unwrap().switch_to(0);
        app.apply_divider_drag_position(320.0, 240.0);
        assert_eq!(
            app.tabs
                .as_ref()
                .unwrap()
                .tab_by_id(tab_id)
                .unwrap()
                .panes
                .get_dividers()[0]
                .ratio_millis,
            preview_ratio,
            "{transition}"
        );
    }
}

#[test]
fn divider_drag_survives_pane_focus_within_its_selected_tab() {
    let mut app = divider_owner_fixture();
    let panes = app.active_panes_mut();
    let current = panes.focused_id();
    let other = panes
        .pane_ids()
        .into_iter()
        .find(|id| *id != current)
        .unwrap();
    panes.focus(other);
    app.refresh_focused_terminal_surfaces();
    app.apply_divider_drag_position(480.0, 240.0);
    let preview_ratio = app.divider_dragging.as_ref().unwrap().last_ratio_millis;
    assert!(preview_ratio > 500);
    app.finish_native_pointer_gesture();
    app.apply_divider_drag_position(640.0, 240.0);
    assert_eq!(
        app.active_panes().get_dividers()[0].ratio_millis,
        preview_ratio
    );
}

#[test]
fn scrollbar_drag_updates_only_its_live_selected_owner() {
    for transition in ["tab", "closed", "closing", "missing", "wrong-pane"] {
        let mut app = scrollbar_owner_fixture();
        app.apply_scrollbar_drag_fraction(1.0);
        assert_eq!(app.active_terminal().unwrap().grid().scroll_offset(), 0);
        app.begin_scrollbar_drag();
        app.refresh_focused_terminal_surfaces();
        assert!(app.scrollbar_dragging.is_some());
        app.apply_scrollbar_drag_fraction(1.0);
        let maximum = app.active_terminal().unwrap().grid().scrollback_len();
        assert!(maximum > 0);
        assert_eq!(
            app.active_terminal().unwrap().grid().scroll_offset(),
            maximum
        );
        app.apply_scrollbar_drag_fraction(0.5);
        let original_offset = app.active_terminal().unwrap().grid().scroll_offset();
        assert_eq!(original_offset, maximum / 2);
        match transition {
            "tab" => app.tabs.as_mut().unwrap().switch_to(1),
            "closed" => app.tabs.as_mut().unwrap().active_tab_mut().unwrap().closed = true,
            "closing" => {
                app.tabs.as_mut().unwrap().close_tab(0);
            }
            "missing" => app.scrollbar_dragging.as_mut().unwrap().tab_id = usize::MAX,
            "wrong-pane" => app.scrollbar_dragging.as_mut().unwrap().pane_id = PaneId(u32::MAX),
            _ => unreachable!(),
        }
        // Deliberately bypass refresh: the write boundary must revalidate too.
        app.apply_scrollbar_drag_fraction(1.0);
        assert!(app.scrollbar_dragging.is_none());
        let offsets: Vec<_> = app
            .tabs
            .as_ref()
            .unwrap()
            .iter()
            .map(|(_, tab)| tab.focused_terminal().unwrap().grid().scroll_offset())
            .collect();
        assert_eq!(offsets, vec![original_offset, 0], "{transition}");
    }
}

#[test]
fn scrollbar_drag_cannot_migrate_between_panes_in_the_same_tab() {
    let mut app = scrollbar_owner_fixture();
    let shell = ShellConfig {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 5".into()],
        env: Default::default(),
    };
    let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
    let panes = tab.panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
    let first = panes.focused_id();
    let second = panes
        .split(
            SplitDirection::Vertical,
            crate::config::PaneCwdMode::Focused,
        )
        .unwrap();
    panes
        .get_pane_mut(second)
        .unwrap()
        .terminal
        .process(b"sibling\r\n".repeat(80).as_slice());
    panes.focus(first);
    app.begin_scrollbar_drag();
    app.apply_scrollbar_drag_fraction(1.0);
    assert!(app.active_terminal().unwrap().grid().scroll_offset() > 0);
    app.active_panes_mut().focus(second);
    app.apply_scrollbar_drag_fraction(1.0);
    assert_eq!(app.active_terminal().unwrap().grid().scroll_offset(), 0);
    assert!(app.scrollbar_dragging.is_none());
    app.active_panes_mut().focus(first);
    app.apply_scrollbar_drag_fraction(0.0);
    assert!(app.active_terminal().unwrap().grid().scroll_offset() > 0);
}

#[test]
fn geometry_changes_finish_all_native_gestures_without_a_button_release() {
    let changes: [fn(&mut App); 3] = [
        |app| app.handle_resize(960, 720),
        |app| app.handle_scale_factor_change(1.5, winit::dpi::PhysicalSize::new(1440, 1080)),
        |app| app.sync_terminal_geometry(960, 720),
    ];
    for change in changes {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.mouse_pressed = true;
        app.scrollbar_dragging = Some(crate::tabs::TerminalIdentity {
            tab_id: 42,
            pane_id: PaneId(7),
        });
        app.tab_dragging = Some(TabDrag {
            tab_id: 42,
            grab: None,
            live: None,
        });
        app.divider_dragging = Some(DividerDrag {
            tab_id: 42,
            domain: None,
            target: crate::panes::DividerTarget::Local(7),
            last_ratio_millis: 500,
        });
        let capture = TerminalMouseCapture {
            owner: crate::tabs::TerminalIdentity {
                tab_id: 42,
                pane_id: PaneId(0),
            },
            button: MouseReportButton::Left,
        };
        app.terminal_mouse_capture = Some(capture);
        app.mouse_state.handle_press(
            MouseButton::Left,
            termeric_core::Point::new(0, 0),
            ModifiersState::empty(),
        );
        app.mouse_state.handle_move(termeric_core::Point::new(0, 5));
        change(&mut app);
        assert!(!app.mouse_pressed);
        assert!(app.scrollbar_dragging.is_none());
        assert!(app.tab_dragging.is_none());
        assert!(app.divider_dragging.is_none());
        let selection = app.mouse_state.current_selection().unwrap();
        assert!(!selection.active);
        assert_eq!(selection.end, termeric_core::Point::new(0, 5));
        assert_eq!(app.terminal_mouse_capture, Some(capture));
        change(&mut app);
        assert!(app.take_native_drag_controls().is_empty());
    }
}

#[test]
fn pointer_panel_ownership_tracks_the_full_open_close_lifecycle() {
    type PanelTransition = fn(&mut App);
    let lifecycles: [(PanelTransition, PanelTransition); 4] = [
        (
            |app| app.settings_panel.toggle(),
            |app| app.settings_panel.close(),
        ),
        (
            |app| app.commands_panel.toggle(),
            |app| app.commands_panel.close(),
        ),
        (
            |app| {
                app.workspace_panel
                    .open(crate::session::listing::WorkspaceListing::workspaces())
            },
            |app| app.workspace_panel.close(),
        ),
        (
            |app| {
                app.workspace_menu = Some(crate::ui::popup_menu::PopupMenu::new(
                    crate::ui::workspace_menu::workspace_menu_rows(&[], None, false),
                    false,
                ))
            },
            |app| app.workspace_menu = None,
        ),
    ];
    for (open, close) in lifecycles {
        let mut app = App::new_headless(Config::default()).unwrap();
        assert!(!app.pointer_panel_owns_input());
        open(&mut app);
        assert!(app.pointer_panel_owns_input());
        close(&mut app);
        assert!(!app.pointer_panel_owns_input());
        open(&mut app);
        assert!(app.pointer_panel_owns_input());
    }
}

#[test]
fn closing_one_panel_does_not_release_another_panels_pointer_ownership() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.commands_panel.toggle();
    app.settings_panel.toggle();
    app.settings_panel.close();
    assert!(app.pointer_panel_owns_input());
    app.commands_panel.close();
    assert!(!app.pointer_panel_owns_input());
}

#[test]
fn modal_interruption_finishes_native_gestures_before_pointer_motion() {
    let openers: [fn(&mut App); 9] = [
        |app| app.command_palette.open(),
        |app| app.settings_panel.toggle(),
        |app| app.shortcuts_panel.open(),
        |app| {
            app.dialog
                .show(crate::ui::dialog::DialogSpec::message("Notice", vec![]));
        },
        |app| app.unicode_picker.open(),
        |app| app.domain_picker.open(&app.config.domains),
        |app| app.commands_panel.toggle(),
        |app| {
            app.workspace_panel
                .open(crate::session::listing::WorkspaceListing::workspaces())
        },
        |app| {
            app.workspace_menu = Some(crate::ui::popup_menu::PopupMenu::new(
                crate::ui::workspace_menu::workspace_menu_rows(&[], None, false),
                false,
            ))
        },
    ];
    for open in openers {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.mouse_pressed = true;
        app.scrollbar_dragging = Some(crate::tabs::TerminalIdentity {
            tab_id: 42,
            pane_id: PaneId(7),
        });
        app.tab_dragging = Some(TabDrag {
            tab_id: 42,
            grab: None,
            live: None,
        });
        app.divider_dragging = Some(DividerDrag {
            tab_id: 42,
            domain: None,
            target: crate::panes::DividerTarget::Local(7),
            last_ratio_millis: 500,
        });
        app.mouse_state.handle_press(
            MouseButton::Left,
            termeric_core::Point::new(0, 0),
            ModifiersState::empty(),
        );
        app.mouse_state.handle_move(termeric_core::Point::new(0, 5));
        open(&mut app);
        app.handle_mouse_move((100.0, 100.0));
        assert!(!app.mouse_pressed);
        assert!(app.scrollbar_dragging.is_none());
        assert!(app.tab_dragging.is_none());
        assert!(app.divider_dragging.is_none());
        assert!(!app.mouse_state.current_selection().unwrap().active);
    }
}

#[test]
fn native_modal_cleanup_preserves_unobscured_drag_and_protocol_button() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.mouse_pressed = true;
    app.tab_dragging = Some(TabDrag {
        tab_id: 42,
        grab: None,
        live: None,
    });
    let capture = TerminalMouseCapture {
        owner: crate::tabs::TerminalIdentity {
            tab_id: 42,
            pane_id: PaneId(0),
        },
        button: MouseReportButton::Left,
    };
    app.terminal_mouse_capture = Some(capture);
    app.finish_obscured_native_pointer_gesture();
    assert!(app.mouse_pressed);
    assert!(app.tab_dragging.is_some());
    app.command_palette.open();
    app.finish_obscured_native_pointer_gesture();
    app.command_palette.close();
    app.finish_obscured_native_pointer_gesture();
    assert!(!app.mouse_pressed);
    assert!(app.tab_dragging.is_none());
    // Application-owned release reporting has a separate owner/lifecycle.
    assert_eq!(app.terminal_mouse_capture, Some(capture));
}

#[test]
fn a_modal_ends_agent_view_drags_whose_release_it_swallows() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.agent_view.state.begin_resize(900.0, 1200.0, 1.0);
    app.agent_view.selecting = true;
    // A modal opens mid-drag, as the guardrail can for a control write.
    app.command_palette.open();
    app.handle_mouse_move((800.0, 300.0));
    assert!(!app.agent_view.state.resizing());
    assert!(!app.agent_view.selecting);
    assert!(app.agent_view.state.hover.is_none());
    // The release went to the modal; once it closes, motion with no button
    // held must not pick the drag back up.
    app.command_palette.close();
    app.handle_mouse_move((700.0, 300.0));
    assert!(!app.agent_view.state.resizing());
    assert!(!app.agent_view.selecting);
}

#[test]
fn closing_modal_still_finishes_native_gesture_before_animation_cleanup() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.shortcuts_panel.open();
    app.shortcuts_panel.close();
    assert!(app.shortcuts_panel.is_active());
    app.mouse_pressed = true;
    app.finish_obscured_native_pointer_gesture();
    assert!(!app.mouse_pressed);
}

#[test]
fn losing_window_focus_finishes_native_pointer_gestures_without_a_release_event() {
    let mut app = App::new_headless(Config::default()).unwrap();
    app.mouse_pressed = true;
    app.scrollbar_dragging = Some(crate::tabs::TerminalIdentity {
        tab_id: 42,
        pane_id: PaneId(7),
    });
    app.divider_dragging = Some(DividerDrag {
        tab_id: 42,
        domain: None,
        target: crate::panes::DividerTarget::Local(7),
        last_ratio_millis: 500,
    });
    app.tab_dragging = Some(TabDrag {
        tab_id: 42,
        grab: None,
        live: None,
    });
    app.mouse_state.handle_press(
        MouseButton::Left,
        termeric_core::Point::new(0, 0),
        ModifiersState::empty(),
    );
    app.mouse_state.handle_move(termeric_core::Point::new(0, 5));
    app.handle_window_focus_change(false);
    assert!(!app.mouse_pressed);
    assert!(app.scrollbar_dragging.is_none());
    assert!(app.divider_dragging.is_none());
    assert!(app.tab_dragging.is_none());
    let selection = app.mouse_state.current_selection().unwrap();
    assert!(!selection.active);
    assert_eq!(selection.end, termeric_core::Point::new(0, 5));
    app.mouse_pressed = true;
    app.tab_dragging = Some(TabDrag {
        tab_id: 42,
        grab: None,
        live: None,
    });
    app.handle_window_focus_change(false);
    assert!(!app.mouse_pressed);
    assert!(app.tab_dragging.is_none());
}

#[test]
fn finishing_native_drags_extracts_exact_live_domain_controls_once() {
    let mut app = App::new_headless(Config::default()).unwrap();
    let mut tabs = TabManager::for_test(crate::config::ShellConfig::default());
    for (domain, remote) in [("a", 10), ("b", 99), ("a", 20)] {
        tabs.new_tab_with_command(
            "/bin/sh",
            &crate::test_support::sleeping_args(),
            None,
            None,
            None,
            HashMap::new(),
        )
        .unwrap();
        tabs.active_tab_mut().unwrap().live_domain = Some(crate::tabs::LiveDomainTab {
            domain_id: domain.into(),
            tab_id: remote,
            pane_ids: HashMap::new(),
        });
    }
    let ids: Vec<_> = tabs.iter().map(|(_, tab)| tab.id).collect();
    app.tabs = Some(tabs);
    app.divider_dragging = Some(DividerDrag {
        tab_id: ids[1],
        domain: Some(("b".into(), 99)),
        target: crate::panes::DividerTarget::Domain(88),
        last_ratio_millis: 650,
    });
    app.tab_dragging = Some(TabDrag {
        tab_id: ids[2],
        grab: None,
        live: Some(LiveDomainTabDrag {
            domain_id: "a".into(),
            remote_tab_id: 20,
            initial_index: 0,
        }),
    });
    assert_eq!(
        app.take_native_drag_controls(),
        vec![
            LiveDomainControl::ResizeSplit {
                domain_id: "b".into(),
                remote_split_id: 88,
                ratio_millis: 650
            },
            LiveDomainControl::MoveTab {
                domain_id: "a".into(),
                remote_tab_id: 20,
                index: 1
            },
        ]
    );
    assert!(app.take_native_drag_controls().is_empty());
    app.tab_dragging = Some(TabDrag {
        tab_id: ids[2],
        grab: None,
        live: Some(LiveDomainTabDrag {
            domain_id: "a".into(),
            remote_tab_id: 20,
            initial_index: 1,
        }),
    });
    assert!(app.take_native_drag_controls().is_empty());
    app.tab_dragging = Some(TabDrag {
        tab_id: usize::MAX,
        grab: None,
        live: Some(LiveDomainTabDrag {
            domain_id: "a".into(),
            remote_tab_id: 20,
            initial_index: 0,
        }),
    });
    assert!(app.take_native_drag_controls().is_empty());
}

#[test]
fn a_press_within_a_dividers_margin_drags_it_unless_the_tab_is_zoomed() {
    use crate::panes::PANE_DIVIDER_HIT_MARGIN;
    let mut app = divider_owner_fixture();
    app.divider_dragging = None;
    let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
    let tab_id = tab.id;
    let divider = tab.split_panes().unwrap().get_dividers()[0];
    assert!(divider.vertical);
    let middle = (divider.start + divider.end) / 2.0;

    // At the margin the press belongs to the pane beside the divider.
    assert!(!app.press_pane_divider(divider.position + PANE_DIVIDER_HIT_MARGIN, middle));
    assert!(app.divider_dragging.is_none());

    // Within it, the press drags exactly that divider.
    let inside = PANE_DIVIDER_HIT_MARGIN - 1.0;
    assert!(app.press_pane_divider(divider.position - inside, middle));
    let drag = app.divider_dragging.take().unwrap();
    assert_eq!(drag.tab_id, tab_id);
    assert_eq!(drag.target, divider.target);
    assert_eq!(drag.last_ratio_millis, divider.ratio_millis);
    assert!(drag.domain.is_none());

    // A zoomed tab shows one pane and no dividers to drag.
    let panes = app
        .tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .split_panes_mut()
        .unwrap();
    assert!(panes.toggle_zoom());
    assert!(!app.press_pane_divider(divider.position, middle));
    assert!(app.divider_dragging.is_none());
}

#[test]
fn a_double_click_on_a_tab_renames_it_and_a_single_press_still_drags_it() {
    let mut app = scrollbar_owner_fixture();
    let second = app.tabs.as_ref().unwrap().all_tabs()[1].id;

    // Presses on two different tabs are no double-click.
    app.press_tab(0);
    app.finish_native_pointer_gesture();
    app.press_tab(1);
    assert!(app.tab_rename.is_none());
    assert_eq!(
        app.tab_dragging.as_ref().map(|drag| drag.tab_id),
        Some(second)
    );
    app.finish_native_pointer_gesture();

    app.press_tab(1);
    assert!(app.tab_dragging.is_none());
    assert_eq!(
        app.tab_rename.as_ref().map(|rename| rename.tab_id),
        Some(second)
    );

    // A third press starts a new pair and leaves the rename alone.
    app.press_tab(1);
    assert!(app.tab_dragging.is_some());
    assert_eq!(
        app.tab_rename.as_ref().map(|rename| rename.tab_id),
        Some(second)
    );
}

#[test]
fn ctrl_alt_shift_t_renames_the_active_tab() {
    let mut app = scrollbar_owner_fixture();
    app.modifiers = ModifiersState::CONTROL | ModifiersState::ALT | ModifiersState::SHIFT;
    assert!(app.handle_bound_key(&winit::keyboard::Key::Character("T".into())));
    let active = app.tabs.as_ref().unwrap().active_tab_id();
    assert_eq!(app.tab_rename.as_ref().map(|rename| rename.tab_id), active);
}
