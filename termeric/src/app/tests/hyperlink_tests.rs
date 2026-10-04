//! Hyperlink activation: owner gates, working directories, and refusals.

use super::*;

#[test]
fn stale_hyperlink_activation_is_rejected_before_opener_validation() {
    let mut app = App::new_headless(Config::default()).unwrap();
    // An option-shaped target makes this pre-fix reproduction side-effect-free.
    assert!(!app.activate_hyperlink(
        Some(TerminalEventOrigin {
            tab_id: usize::MAX,
            pane_id: PaneId(0)
        }),
        "--help",
        None,
    ));
    assert!(
        app.toasts
            .iter()
            .last()
            .unwrap()
            .message
            .contains("originating terminal is no longer available")
    );
}

#[test]
fn hyperlink_owner_gate_rejects_closed_routes_but_allows_live_inactive_read_only_owners() {
    for split in [false, true] {
        let mut app = if split {
            divider_owner_fixture()
        } else {
            scrollbar_owner_fixture()
        };
        let origin = app.active_terminal_origin().unwrap();
        let tab_id = app.tabs.as_ref().unwrap().active_tab_id().unwrap();
        assert!(app.validate_hyperlink_origin(Some(origin)).is_ok());
        assert!(
            app.tabs
                .as_mut()
                .unwrap()
                .active_tab_mut()
                .unwrap()
                .toggle_focused_pane_read_only()
        );
        app.tabs.as_mut().unwrap().switch_to(1);
        assert!(
            app.validate_hyperlink_origin(Some(origin)).is_ok(),
            "focus/read-only state is not a launch-owner substitute"
        );
        for invalid in [
            None,
            Some(TerminalEventOrigin {
                tab_id: usize::MAX,
                pane_id: PaneId(0),
            }),
            Some(TerminalEventOrigin {
                tab_id,
                pane_id: PaneId(u32::MAX),
            }),
        ] {
            assert_eq!(
                app.open_hyperlink_for_origin(invalid, "--help", Some("/trusted"))
                    .unwrap_err()
                    .kind(),
                std::io::ErrorKind::NotFound
            );
        }
        if split {
            assert!(
                app.validate_hyperlink_origin(Some(TerminalEventOrigin {
                    tab_id,
                    pane_id: PaneId(0)
                }))
                .is_ok(),
                "an unfocused live pane remains its own route"
            );
        }
        app.tabs
            .as_mut()
            .unwrap()
            .tab_mut_by_id(tab_id)
            .unwrap()
            .closed = true;
        assert_eq!(
            app.open_hyperlink_for_origin(Some(origin), "--help", None)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound
        );
        let tab = app.tabs.as_mut().unwrap().tab_mut_by_id(tab_id).unwrap();
        tab.closed = false;
        tab.start_closing();
        assert_eq!(
            app.open_hyperlink_for_origin(Some(origin), "--help", None)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound
        );
        assert!(!app.activate_hyperlink(Some(origin), "--help", None));
        assert!(
            app.toasts
                .iter()
                .last()
                .unwrap()
                .message
                .contains("originating terminal is no longer available")
        );
    }
}

#[test]
fn hyperlink_owner_gate_rejects_closed_and_removed_panes_but_follows_survivors() {
    let mut app = divider_owner_fixture();
    let old_origin = app.active_terminal_origin().unwrap();
    let TerminalEventOrigin { tab_id, pane_id } = old_origin;
    app.active_panes_mut().get_pane_mut(pane_id).unwrap().closed = true;
    assert_eq!(
        app.open_hyperlink_for_origin(Some(old_origin), "--help", None)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
    let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
    let panes = &mut tab.panes;
    panes.get_pane_mut(pane_id).unwrap().closed = false;
    let other = panes
        .pane_ids()
        .into_iter()
        .find(|id| *id != pane_id)
        .unwrap();
    panes.close(other);
    assert!(app.validate_hyperlink_origin(Some(old_origin)).is_ok());
    assert!(
        app.validate_hyperlink_origin(Some(TerminalEventOrigin {
            tab_id,
            pane_id: other
        }))
        .is_err()
    );
}

#[test]
fn hyperlink_open_context_uses_presented_local_directory_without_hidden_fallback() {
    use termeric_core::Point;
    for split in [false, true] {
        let mut app = if split {
            divider_owner_fixture()
        } else {
            scrollbar_owner_fixture()
        };
        for (report, expected) in [
            (
                b"\x1b]7;file:///tmp/presented\x07".as_slice(),
                Some("/tmp/presented"),
            ),
            (
                b"\x1b]7;file://remote.invalid/tmp/remote\x07".as_slice(),
                None,
            ),
            (b"".as_slice(), None),
        ] {
            let terminal = app.active_terminal_mut().unwrap();
            *terminal = termeric_core::Terminal::new(40, 4);
            terminal.process(report);
            terminal.process(b"\x1b]8;;./relative.txt\x1b\\link\x1b]8;;\x1b\\\x1b[?2026h\x1b]7;file:///tmp/hidden\x07");
            let origin = app.active_terminal_origin().unwrap();
            let (target, directory) = app
                .focused_hyperlink_open_context(Point::new(0, 0))
                .unwrap();
            assert_eq!(target, "./relative.txt");
            assert_eq!(directory.as_deref(), expected);
            assert_eq!(app.hyperlink_open_directory(origin).as_deref(), expected);
            app.active_terminal_mut().unwrap().process(b"\x1b[?2026l");
            assert_eq!(
                app.hyperlink_open_directory(origin).as_deref(),
                Some("/tmp/hidden")
            );
        }
    }
}

#[test]
fn routed_hyperlink_directory_does_not_follow_focus_to_another_pane() {
    use termeric_core::Point;
    let mut app = divider_owner_fixture();
    let original = app.active_terminal_origin().unwrap();
    app.active_terminal_mut()
        .unwrap()
        .process(b"\x1bc\x1b]7;file:///tmp/original\x07");
    let panes = app.active_panes_mut();
    let other = panes
        .pane_ids()
        .into_iter()
        .find(|id| *id != panes.focused_id())
        .unwrap();
    panes.focus(other);
    app.active_terminal_mut().unwrap().process(
        b"\x1bc\x1b]7;file:///tmp/focused\x07\x1b]8;;./relative.txt\x1b\\link\x1b]8;;\x1b\\",
    );
    assert_eq!(
        app.hyperlink_open_directory(original).as_deref(),
        Some("/tmp/original")
    );
    assert_eq!(
        app.focused_hyperlink_open_context(Point::new(0, 0))
            .unwrap()
            .1
            .as_deref(),
        Some("/tmp/focused")
    );
    assert!(
        app.hyperlink_open_directory(TerminalEventOrigin {
            tab_id: usize::MAX,
            pane_id: PaneId(0)
        })
        .is_none()
    );
}

#[test]
fn relative_hyperlink_without_trusted_directory_is_refused_before_launch() {
    let mut app = scrollbar_owner_fixture();
    *app.active_terminal_mut().unwrap() = termeric_core::Terminal::new(8, 3);
    let origin = app.active_terminal_origin().unwrap();
    let directory = app.hyperlink_open_directory(origin);
    assert!(!app.activate_hyperlink(Some(origin), "./file.txt", directory.as_deref()));
    let toast = app.toasts.iter().last().unwrap();
    assert_eq!(toast.severity, crate::ui::toast::ToastSeverity::Error);
    assert!(toast.message.contains("trusted local terminal directory"));
}

#[test]
fn option_shaped_hyperlink_activation_is_refused_with_visible_feedback() {
    let mut app = scrollbar_owner_fixture();
    let origin = app.active_terminal_origin().unwrap();
    assert!(!app.activate_hyperlink(Some(origin), "--help", None));
    let toast = app.toasts.iter().last().unwrap();
    assert_eq!(toast.severity, crate::ui::toast::ToastSeverity::Error);
    assert!(
        toast
            .message
            .contains("opener target must not start with '-'")
    );
    assert!(toast.message.contains("./-name"));
}
