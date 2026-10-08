//! Real PTY output reaching a tab's terminal grid.

use super::{CommandLaunch, Tab};
use std::time::Duration;

/// A tab whose only pane runs `script` under `/bin/sh -c` in a 20×3
/// terminal, once its first row shows `expected`. Fails the test, with
/// what the row shows, if that takes over two seconds.
fn tab_showing(script: &str, expected: &str) -> Tab {
    let mut tab = Tab::new_with_command(
        0,
        20,
        3,
        &CommandLaunch {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            title: Some("real-pty".into()),
            ..CommandLaunch::default()
        },
        100,
        1024 * 1024,
    )
    .expect("the shell starts in a PTY");
    let mut shown = String::new();
    let arrived = crate::test_support::poll_until(Duration::from_secs(2), || {
        tab.poll_events();
        shown = tab
            .focused_terminal()
            .expect("the tab has a terminal")
            .rows_text(0, 0);
        shown.contains(expected)
    });
    assert!(
        arrived,
        "missing {expected:?}; the first row shows {shown:?}"
    );
    tab
}

#[test]
fn real_pty_unicode_clusters_preserve_width_and_content() {
    let expected = "界e\u{301}👩\u{200d}💻";
    let tab = tab_showing(&format!("printf '%s' '{expected}'"), expected);
    let terminal = tab.focused_terminal().unwrap();
    assert_eq!(terminal.rows_text(0, 0), expected);
    assert_eq!(terminal.grid().cursor().col, 5);
    assert!(terminal.grid().cell(0, 0).unwrap().is_wide_char());
    assert!(terminal.grid().cell(0, 1).unwrap().is_wide_char_spacer());
    assert_eq!(
        terminal.grid().cell(0, 2).unwrap().content.to_text(),
        "e\u{301}"
    );
    assert!(terminal.grid().cell(0, 3).unwrap().is_wide_char());
    assert!(terminal.grid().cell(0, 4).unwrap().is_wide_char_spacer());
}

#[test]
fn real_pty_sgr_attributes_reach_the_terminal_grid_without_leaking() {
    let command = "printf '\\033[1mA\\033[22mB\\033[6mC\\033[25mD\\033[8mE\\033[28;9mF\\033[29;4:3;58:2::12:34:56mG\\033[0mH'";
    let tab = tab_showing(command, "ABCDEFGH");
    let terminal = tab.focused_terminal().unwrap();
    assert_eq!(terminal.rows_text(0, 0), "ABCDEFGH");
    let grid = terminal.grid();
    assert!(
        grid.cell(0, 0)
            .unwrap()
            .attrs
            .contains(termeric_core::CellAttrs::BOLD)
    );
    assert!(grid.cell(0, 1).unwrap().attrs.is_empty());
    assert!(
        grid.cell(0, 2)
            .unwrap()
            .attrs
            .contains(termeric_core::CellAttrs::RAPID_BLINK)
    );
    assert!(grid.cell(0, 3).unwrap().attrs.is_empty());
    assert!(
        grid.cell(0, 4)
            .unwrap()
            .attrs
            .contains(termeric_core::CellAttrs::HIDDEN)
    );
    assert!(
        grid.cell(0, 5)
            .unwrap()
            .attrs
            .contains(termeric_core::CellAttrs::STRIKE)
    );
    let underlined = grid.cell(0, 6).unwrap();
    assert!(
        underlined
            .attrs
            .contains(termeric_core::CellAttrs::CURLY_UNDERLINE)
    );
    assert_eq!(
        underlined.underline_color,
        Some(termeric_core::Color::rgb(12, 34, 56))
    );
    assert_eq!(
        underlined.underline_color_spec,
        Some(termeric_core::ColorSpec::TrueColor(
            termeric_core::Color::rgb(12, 34, 56)
        ))
    );
    assert!(grid.cell(0, 7).unwrap().attrs.is_empty());
}
