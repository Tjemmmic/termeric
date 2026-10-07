//! Harvest capture, search, and reruns, and diff review completion.

use super::*;

#[test]
fn diff_review_done_marks_only_its_original_tab_and_exact_diff() {
    let mut tabs = TabManager::for_test(ShellConfig::default());
    for title in ["first-diff-tab", "active-other-tab"] {
        tabs.new_tab_with_command(
            "/bin/sh",
            &crate::test_support::sleeping_args(),
            None,
            Some(title),
            None,
            HashMap::new(),
        )
        .unwrap();
    }
    for (index, tab) in tabs.all_tabs_mut().iter_mut().enumerate() {
        for line in [
            format!("--- a/fixture-{index}.rs"),
            format!("+++ b/fixture-{index}.rs"),
            "@@ -1,1 +1,1 @@".into(),
            "-old".into(),
            "+new".into(),
            "finished".into(),
        ] {
            tab.focused_integration_mut()
                .unwrap()
                .diff_detector
                .feed_line(&line);
        }
    }
    let first_tab_id = tabs.iter().next().unwrap().1.id;
    let first_diff = tabs
        .iter()
        .next()
        .unwrap()
        .1
        .focused_integration()
        .unwrap()
        .diff_detector
        .latest_unreviewed()
        .unwrap()
        .clone();
    tabs.switch_to(1);

    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    let request_id = app.diff_review.show(&first_diff, first_tab_id, PaneId(0));
    app.apply_diff_review_action(crate::ui::diff_review::DiffReviewAction::Dismiss { request_id });

    let tabs = app.tabs.as_ref().unwrap();
    assert_eq!(tabs.active_index(), 1);
    let mut tabs = tabs.iter();
    assert_eq!(tabs.next().unwrap().1.unreviewed_diff_count(), 0);
    assert_eq!(tabs.next().unwrap().1.unreviewed_diff_count(), 1);
}

#[test]
fn harvest_search_switches_to_the_exact_producing_terminal() {
    let mut tabs = TabManager::for_test(ShellConfig::default());
    let source_tab = tabs
        .new_tab_with_command(
            "/bin/sh",
            &crate::test_support::sleeping_args(),
            None,
            Some("harvest-source"),
            None,
            HashMap::new(),
        )
        .unwrap();
    tabs.active_tab_mut()
        .unwrap()
        .focused_terminal_mut()
        .unwrap()
        .process(
            b"\x1b]133;A\x07\x1b]133;B\x07echo exact\r\n\x1b]133;C\x07exact\r\n\x1b]133;D;0\x07",
        );
    let region = tabs
        .active_tab()
        .unwrap()
        .focused_terminal()
        .unwrap()
        .prompt_tracker()
        .completed_region(0)
        .unwrap()
        .clone();
    for index in 0..30 {
        tabs.active_tab_mut()
            .unwrap()
            .focused_terminal_mut()
            .unwrap()
            .process(format!("history-{index}\r\n").as_bytes());
    }
    let moved_region = tabs
        .active_tab()
        .unwrap()
        .focused_terminal()
        .unwrap()
        .prompt_tracker()
        .completed_region_by_id(region.id)
        .unwrap();
    assert!(moved_region.prompt_row < 0);
    let expected_scroll_offset = moved_region.prompt_row.saturating_neg() as usize;
    let other_tab = tabs
        .new_tab_with_command(
            "/bin/sh",
            &crate::test_support::sleeping_args(),
            None,
            Some("other-active-tab"),
            None,
            HashMap::new(),
        )
        .unwrap();
    assert_eq!(tabs.active_tab_id(), Some(other_tab));

    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    let mut entry = crate::harvest::CommandEntry::new_for_source(
        crate::harvest::HarvestSource {
            tab_id: source_tab,
            pane_id: 0,
        },
        region.prompt_row,
        Some(0),
    );
    entry.region_id = region.id;
    entry.command = "echo exact".into();
    entry.output = "exact".into();
    let availability = app.sprout_availability_for_entry(&entry);
    assert!(availability.live_content_available);
    assert!(availability.action_enabled(crate::harvest::sprouts::SproutAction::SearchOutput));
    app.handle_action(crate::input::Action::SwitchToTab(0));
    app.active_terminal_mut().unwrap().process(b"\x1b[?1049h");
    let alternate = app.sprout_availability_for_entry(&entry);
    assert!(alternate.source_alternate_screen);
    assert!(!alternate.live_content_available);
    assert_eq!(
        alternate.action_label(crate::harvest::sprouts::SproutAction::SearchOutput),
        "Search (alternate screen)"
    );
    app.active_terminal_mut().unwrap().process(b"\x1b[?1049l");
    app.handle_action(crate::input::Action::SwitchToTab(1));
    app.execute_sprout_action(
        crate::harvest::sprouts::SproutAction::SearchOutput,
        Some(entry.clone()),
    );

    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(source_tab));
    assert!(app.search.is_active());
    app.execute_sprout_action(
        crate::harvest::sprouts::SproutAction::ScrollTo,
        Some(entry.clone()),
    );
    assert_eq!(
        app.active_terminal().unwrap().grid().scroll_offset(),
        expected_scroll_offset
    );

    app.active_terminal_mut()
        .unwrap()
        .set_scrollback_limits(1, 1024 * 1024);
    assert!(
        !app.active_terminal()
            .unwrap()
            .prompt_tracker()
            .completed_region_by_id(region.id)
            .unwrap()
            .content_available
    );
    let availability = app.sprout_availability_for_entry(&entry);
    assert!(!availability.live_content_available);
    assert!(!availability.action_enabled(crate::harvest::sprouts::SproutAction::SearchOutput));
    assert!(availability.action_enabled(crate::harvest::sprouts::SproutAction::CopyOutput));
    assert!(availability.action_enabled(crate::harvest::sprouts::SproutAction::Rerun));
    app.handle_action(crate::input::Action::SwitchToTab(1));
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(other_tab));
    app.execute_sprout_action(crate::harvest::sprouts::SproutAction::Rerun, Some(entry));
    assert_eq!(app.tabs.as_ref().unwrap().active_tab_id(), Some(source_tab));
}

#[test]
fn harvest_source_survives_single_split_and_collapse() {
    use crate::harvest::sprouts::SproutAction;
    let (mut app, directory, entry, _) = harvest_rerun_capture_fixture_in(false);
    let source = entry.source;
    app.harvest = Some(crate::harvest::HarvestMode::default());
    app.active_terminal_mut().unwrap().process(
        b"\r\n\x1b]133;A\x07\x1b]133;B\x07echo original\r\n\x1b]133;C\x07original output\r\n\x1b]133;D;0\x07");
    app.process_pty_events();
    let entry = app
        .harvest
        .as_ref()
        .unwrap()
        .cellar
        .latest_for_source(source)
        .unwrap()
        .clone();
    let mut expected_input = Vec::new();
    for cycle in 0..3 {
        let sibling = split_harvest_capture_owner(&mut app, directory.path());
        let captures = current_harvest_capture_routes(&mut app, directory.path());
        assert_ne!(app.active_harvest_source(), Some(source));
        assert!(
            app.terminal_for_harvest_source(source).is_some(),
            "splitting must not orphan a retained source"
        );
        let availability = app.sprout_availability_for_entry(&entry);
        assert!(availability.live_content_available);
        for action in [
            SproutAction::SearchOutput,
            SproutAction::ScrollTo,
            SproutAction::Rerun,
        ] {
            assert!(availability.action_enabled(action));
            app.focus_pane_for(Requester::Control, source.tab_id as u64, sibling.0)
                .unwrap();
            app.execute_sprout_action(action, Some(entry.clone()));
            assert_eq!(app.active_harvest_source(), Some(source));
            if action == SproutAction::SearchOutput {
                assert!(app.search.is_active());
                assert!(app.search.is_scoped());
            }
        }
        // The later intentional sibling focus invalidates the row scope; it
        // must not remain attached to unrelated output when focus returns.
        assert!(!app.search.is_active());
        expected_input.extend_from_slice(b"\x15echo original\r");
        assert_eq!(
            fenced_harvest_capture(&mut app, &captures),
            vec![expected_input.clone(), vec![]]
        );
        expected_input.push(0);
        assert_eq!(
            app.harvest.as_ref().unwrap().cellar.len(),
            cycle + 1,
            "adoption must not duplicate completed regions"
        );
        app.close_pane_for(Requester::Control, source.tab_id as u64, sibling.0)
            .unwrap();
        assert_eq!(app.active_harvest_source(), Some(source));
        assert!(
            app.sprout_availability_for_entry(&entry)
                .live_content_available
        );
        // Later commands from the collapsed owner use the same source key.
        app.active_terminal_mut().unwrap().process(format!(
            "\x1b]133;A\x07\x1b]133;B\x07echo after{cycle}\r\n\x1b]133;C\x07after{cycle}\r\n\x1b]133;D;0\x07"
        ).as_bytes());
        app.process_pty_events();
        let cellar = &app.harvest.as_ref().unwrap().cellar;
        assert_eq!(cellar.len(), cycle + 2);
        assert_eq!(
            cellar.latest_for_source(source).unwrap().command_text(),
            format!("echo after{cycle}")
        );
        assert!(cellar.entries().all(|entry| entry.source == source));
    }
}

#[test]
fn harvest_nonzero_survivor_keeps_live_regions_and_rerun_after_collapse() {
    use crate::harvest::sprouts::SproutAction;
    let (mut app, directory, _, _) = harvest_rerun_capture_fixture();
    let source = app.active_harvest_source().unwrap();
    assert_eq!(source.pane_id, 2);
    app.harvest = Some(crate::harvest::HarvestMode::default());
    app.active_terminal_mut().unwrap().process(
        b"\r\n\x1b]133;A\x07\x1b]133;B\x07echo nonzero\r\n\x1b]133;C\x07nonzero output\r\n\x1b]133;D;0\x07");
    app.process_pty_events();
    let entry = app
        .harvest
        .as_ref()
        .unwrap()
        .cellar
        .latest_for_source(source)
        .unwrap()
        .clone();
    app.close_pane_for(Requester::Control, source.tab_id as u64, 0)
        .unwrap();
    app.close_pane_for(Requester::Control, source.tab_id as u64, 1)
        .unwrap();
    assert_eq!(app.active_harvest_source(), Some(source));
    assert!(
        app.sprout_availability_for_entry(&entry)
            .live_content_available
    );
    assert_eq!(app.harvest_command_text(&entry).0, "echo nonzero");
    for action in [
        SproutAction::SearchOutput,
        SproutAction::ScrollTo,
        SproutAction::Rerun,
    ] {
        app.execute_sprout_action(action, Some(entry.clone()));
        assert_eq!(app.active_harvest_source(), Some(source));
    }
    let captures = current_harvest_capture_routes(&mut app, directory.path());
    assert_eq!(
        fenced_harvest_capture(&mut app, &captures),
        vec![b"\x15echo nonzero\r".to_vec()]
    );
    assert_eq!(app.harvest.as_ref().unwrap().cellar.len(), 1);
}

#[test]
fn harvest_rerun_never_broadcasts_the_source_command() {
    for excluded in [false, true] {
        let (mut app, _directory, entry, captures) = harvest_rerun_capture_fixture();
        app.toggle_broadcast();
        if excluded {
            app.toggle_armed_broadcast_target(PaneId(entry.source.pane_id));
        }
        app.toggle_broadcast();
        let before = app.broadcast_targets().unwrap();
        assert_eq!(before.1.len(), if excluded { 2 } else { 3 });
        app.execute_sprout_action(crate::harvest::sprouts::SproutAction::Rerun, Some(entry));
        let bytes = fenced_harvest_capture(&mut app, &captures);
        assert_eq!(bytes[0], b"\x15echo harvest\r");
        assert!(
            bytes[1..].iter().all(Vec::is_empty),
            "rerun leaked to siblings: {bytes:?}"
        );
        assert_eq!(app.broadcast_targets().unwrap(), before);
    }
}

#[test]
fn harvest_rerun_is_literal_and_reports_only_successful_delivery() {
    let (mut app, _directory, entry, captures) = harvest_rerun_capture_fixture();
    app.config.commands.push(crate::config::CustomCommand {
        name: "echo".into(),
        command: "different".into(),
        alias: None,
    });
    app.input_line_buffer = "old editable input".into();
    app.git_completions_visible = true;
    app.git_completions = vec!["unrelated".into()].into();
    app.git_completion_index = Some(0);
    app.execute_sprout_action(crate::harvest::sprouts::SproutAction::Rerun, Some(entry));
    let bytes = fenced_harvest_capture(&mut app, &captures);
    assert_eq!(bytes[0], b"\x15echo harvest\r");
    assert!(bytes[1..].iter().all(Vec::is_empty));
    assert!(app.input_line_buffer.is_empty());
    assert!(!app.git_completions_visible);
    assert!(
        app.toasts
            .iter()
            .any(|toast| toast.message == "Sent command to its source terminal")
    );
}

#[test]
fn harvest_retained_rerun_never_uses_the_normalized_or_shortened_caption() {
    for command in [
        "printf '%s' 'two  spaces'".to_string(),
        "printf '%s' 'first\nsecond'".to_string(),
        format!("echo {}", "x".repeat(700)),
    ] {
        let (mut app, _directory, mut entry, captures) = harvest_rerun_capture_fixture();
        entry.command = command.clone();
        let mut cellar = crate::harvest::RootCellar::new();
        cellar.add(entry);
        let retained = cellar.latest().unwrap().clone();
        assert_ne!(retained.command, command);
        app.execute_sprout_action(crate::harvest::sprouts::SproutAction::Rerun, Some(retained));
        let bytes = fenced_harvest_capture(&mut app, &captures);
        assert_eq!(bytes[0], command_keystrokes(&command).concat());
        assert!(bytes[1..].iter().all(Vec::is_empty));
    }
}

#[test]
fn harvest_locally_truncated_snapshot_cannot_be_rerun_or_change_focus() {
    let (mut app, _directory, mut entry, captures) = harvest_rerun_capture_fixture();
    let before = app.active_harvest_source();
    entry.command = "x".repeat(crate::harvest::MAX_HARVEST_COMMAND_BYTES + 1);
    let mut cellar = crate::harvest::RootCellar::new();
    cellar.add(entry);
    let entry = cellar.latest().unwrap().clone();
    assert!(entry.command_truncated);
    assert!(
        !app.sprout_availability_for_entry(&entry)
            .action_enabled(crate::harvest::sprouts::SproutAction::Rerun)
    );
    app.execute_sprout_action(crate::harvest::sprouts::SproutAction::Rerun, Some(entry));
    assert!(
        fenced_harvest_capture(&mut app, &captures)
            .iter()
            .all(Vec::is_empty)
    );
    assert_eq!(app.active_harvest_source(), before);
}

#[test]
fn harvest_incomplete_command_refuses_live_retained_and_stale_menu_reruns() {
    for split in [false, true] {
        let (mut app, _directory, mut entry, captures) = harvest_rerun_capture_fixture_in(split);
        let origin = captures[0].0;
        app.harvest = Some(crate::harvest::HarvestMode::default());
        let command = format!(
            "echo {}",
            "x".repeat(crate::harvest::MAX_HARVEST_COMMAND_BYTES + 100)
        );
        app.tabs.as_mut().unwrap().terminal_for_origin_mut(origin).unwrap().process(
            format!("\r\n\x1b]133;A\x07\x1b]133;B\x07{command}\r\n\x1b]133;C\x07result\r\n\x1b]133;D;0\x07").as_bytes());
        app.process_pty_events();
        let recorded = app
            .harvest
            .as_ref()
            .unwrap()
            .cellar
            .latest()
            .unwrap()
            .clone();
        assert!(
            recorded.command_truncated,
            "app discarded extraction truncation"
        );
        assert_eq!(recorded.source, entry.source);
        assert!(
            !app.sprout_availability_for_entry(&recorded)
                .action_enabled(crate::harvest::sprouts::SproutAction::Rerun)
        );
        app.execute_sprout_action(
            crate::harvest::sprouts::SproutAction::Rerun,
            Some(recorded.clone()),
        );
        assert!(
            fenced_harvest_capture(&mut app, &captures)
                .iter()
                .all(Vec::is_empty)
        );

        // A complete live region can initially enable a menu whose retained
        // snapshot is incomplete. Eviction must revoke that live authority.
        app.tabs.as_mut().unwrap().terminal_for_origin_mut(origin).unwrap().process(
            b"\x1b]133;A\x07\x1b]133;B\x07echo complete\r\n\x1b]133;C\x07ok\r\n\x1b]133;D;0\x07");
        let region_id = app
            .tabs
            .as_ref()
            .unwrap()
            .terminal_for_origin(origin)
            .unwrap()
            .prompt_tracker()
            .completed_regions_after(recorded.region_id)
            .next()
            .unwrap()
            .id;
        entry.region_id = region_id;
        entry.command_truncated = true;
        let availability = app.sprout_availability_for_entry(&entry);
        assert!(availability.action_enabled(crate::harvest::sprouts::SproutAction::Rerun));
        app.harvest
            .as_mut()
            .unwrap()
            .show_sprout_menu(0, 0, Some(entry), availability);
        let request_id = app
            .harvest
            .as_ref()
            .unwrap()
            .sprout_menu
            .as_ref()
            .unwrap()
            .request_id;
        app.tabs
            .as_mut()
            .unwrap()
            .terminal_for_origin_mut(origin)
            .unwrap()
            .process(b"\x1b[2J\x1b[3J");
        app.execute_sprout_request(request_id, crate::harvest::sprouts::SproutAction::Rerun);
        assert!(
            fenced_harvest_capture(&mut app, &captures)
                .iter()
                .all(|bytes| bytes == b"\0")
        );
        assert!(
            app.toasts
                .iter()
                .all(|toast| toast.message != "Sent command to its source terminal")
        );
    }
}

#[test]
fn harvest_rerun_refuses_armed_read_only_stale_and_closing_sources_without_writes() {
    for refusal in [
        "armed",
        "read-only",
        "stale",
        "pane-closed",
        "tab-closed",
        "closing",
    ] {
        let (mut app, _directory, mut entry, captures) = harvest_rerun_capture_fixture();
        let before = app.active_harvest_source();
        match refusal {
            "armed" => app.toggle_broadcast(),
            "read-only" => {
                app.active_panes_mut()
                    .get_pane_mut(PaneId(entry.source.pane_id))
                    .unwrap()
                    .read_only = true
            }
            "stale" => entry.source.pane_id = u32::MAX,
            "pane-closed" => {
                app.active_panes_mut()
                    .get_pane_mut(PaneId(entry.source.pane_id))
                    .unwrap()
                    .closed = true
            }
            "tab-closed" => app.tabs.as_mut().unwrap().active_tab_mut().unwrap().closed = true,
            "closing" => {
                app.tabs
                    .as_mut()
                    .unwrap()
                    .active_tab_mut()
                    .unwrap()
                    .closing_at = Some(Instant::now())
            }
            _ => unreachable!(),
        }
        app.execute_sprout_action(crate::harvest::sprouts::SproutAction::Rerun, Some(entry));
        let bytes = fenced_harvest_capture(&mut app, &captures);
        assert!(bytes.iter().all(Vec::is_empty), "{refusal} wrote {bytes:?}");
        assert_eq!(
            app.active_harvest_source(),
            before,
            "{refusal} changed focus"
        );
        assert!(app.toasts.iter().all(|toast| toast.message
            != "Sent command to its source terminal"
            && toast.message != "Re-ran command"));
    }
}

#[test]
fn harvest_rerun_single_tab_origin_uses_the_same_exact_user_write_policy() {
    let (mut app, _directory, entry, captures) = harvest_rerun_capture_fixture_in(false);
    assert_eq!(entry.source.pane_id, 0);
    app.execute_sprout_action(crate::harvest::sprouts::SproutAction::Rerun, Some(entry));
    assert_eq!(
        fenced_harvest_capture(&mut app, &captures),
        vec![b"\x15echo harvest\r".to_vec()]
    );
    app.tabs
        .as_mut()
        .unwrap()
        .active_tab_mut()
        .unwrap()
        .closing_at = Some(Instant::now());
    assert!(
        app.tabs
            .as_mut()
            .unwrap()
            .write_user_to_origin(captures[0].0, b"must not arrive")
            .is_err()
    );
    assert_eq!(
        fenced_harvest_capture(&mut app, &captures),
        vec![b"\x15echo harvest\r\0".to_vec()]
    );
}

#[test]
fn harvest_rerun_records_exact_user_input_once_but_not_protocol_fences_or_refusals() {
    for split in [false, true] {
        let (mut app, directory, entry, captures) = harvest_rerun_capture_fixture_in(split);
        let pane_id = PaneId(entry.source.pane_id);
        let recorder = crate::session::transcript::TranscriptRecorder::new(
            directory.path(),
            None,
            None,
            "fixture",
            "capture",
        )
        .unwrap();
        app.tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .pane_integration_mut(pane_id)
            .unwrap()
            .start_transcript(recorder);
        app.execute_sprout_action(
            crate::harvest::sprouts::SproutAction::Rerun,
            Some(entry.clone()),
        );
        fenced_harvest_capture(&mut app, &captures);
        app.tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .closing_at = Some(Instant::now());
        app.execute_sprout_action(crate::harvest::sprouts::SproutAction::Rerun, Some(entry));
        fenced_harvest_capture(&mut app, &captures);
        let mut recorder = app
            .tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .pane_integration_mut(pane_id)
            .unwrap()
            .stop_transcript()
            .unwrap();
        recorder.finish(std::time::Duration::from_secs(3)).unwrap();
        let entries = std::fs::read_to_string(recorder.path())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        let input = entries
            .iter()
            .filter(|entry| entry["kind"] == "input")
            .collect::<Vec<_>>();
        assert_eq!(input.len(), 1, "{entries:?}");
        assert_eq!(input[0]["content"], "\x15echo harvest\r");
    }
}

#[test]
fn manual_shell_integration_counts_once_the_shell_marks_a_prompt() {
    use crate::terminal::shell_integration::{SUPPORTED_SHELLS, ShellIntegrationSupport};
    let mut tabs = TabManager::for_test(ShellConfig::default());
    tabs.new_tab_with_command(
        "/bin/sh",
        &crate::test_support::sleeping_args(),
        None,
        Some("manual-integration"),
        None,
        HashMap::new(),
    )
    .unwrap();
    tabs.active_tab_mut()
        .unwrap()
        .focused_pane_mut()
        .unwrap()
        .shell_integration = ShellIntegrationSupport::Manual {
        shell: "elvish",
        instructions: "add the module",
    };
    let mut app = App::new_headless(Config::default()).unwrap();
    app.tabs = Some(tabs);
    let toasted =
        |app: &App, message: &str| app.toasts.iter().any(|toast| toast.message == message);

    assert_eq!(app.active_shell_integration_message(), "add the module");
    app.jump_to_failed_command(true);
    assert!(toasted(
        &app,
        "Failed-command jumps need shell integration (Bash, Zsh, Fish, Nushell, or Elvish)"
    ));
    assert_eq!(SUPPORTED_SHELLS, "Bash, Zsh, Fish, Nushell, or Elvish");

    app.active_terminal_mut()
        .unwrap()
        .process(b"\x1b]133;A\x07");
    assert_eq!(
        app.active_shell_integration_message(),
        "No completed command yet; elvish integration is active"
    );
    app.jump_to_failed_command(true);
    assert!(toasted(&app, "No earlier failed command"));
}

#[test]
fn growth_indicators_sit_beside_their_row_of_the_focused_split_pane() {
    let (mut app, _directory, _, _) = harvest_rerun_capture_fixture_in(true);
    let focused = app.active_panes().focused_id();
    let rect = app.active_panes().calculate_layout()[&focused];
    assert!(
        rect.x > 0.0 && rect.y > 0.0,
        "the focused pane is away from the lone pane's place: {rect:?}"
    );
    app.active_panes_mut()
        .get_pane_mut(focused)
        .unwrap()
        .harvest
        .indicators
        .push(crate::harvest::GrowthIndicator::new(7, 2));
    let placement = crate::renderer::PanePlacement {
        left: 0.0,
        top: 30.0,
        right: 800.0,
        bottom: 510.0,
        padding: 8.0,
    };
    let cell = (10.0, 20.0);

    let indicators = app.harvest_indicator_infos(placement, cell, true);
    assert_eq!(indicators.len(), 1);
    let marker = indicators[0].cell;
    assert_eq!(marker, placement.row_marker(rect, false, 2, cell));
    // On the pane's third row, ending inside its left edge.
    let pane = placement.pane_bounds(rect, false);
    assert_eq!(marker.y, pane.y + 2.0 * cell.1);
    assert!(
        marker.x < pane.x && pane.x < marker.right(),
        "{marker:?} {pane:?}"
    );
    assert_eq!(indicators[0].opacity, 1.0);

    app.config.harvest.show_indicators = false;
    assert!(
        app.harvest_indicator_infos(placement, cell, true)
            .is_empty()
    );
}
