//! Real PTY output reaching the terminal grid.

use super::*;

#[test]
fn real_pty_unicode_clusters_preserve_width_and_content() {
    let expected = "界e\u{301}👩\u{200d}💻";
    let mut tab = Tab::new_with_command(
        0,
        20,
        3,
        &crate::tabs::CommandLaunch {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), format!("printf '%s' '{expected}'")],
            title: Some("unicode-pty".into()),
            ..crate::tabs::CommandLaunch::default()
        },
        100,
        1024 * 1024,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        tab.poll_single(|_, _| {});
        if tab
            .focused_terminal()
            .unwrap()
            .rows_text(0, 0)
            .contains(expected)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
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
    let mut tab = Tab::new_with_command(
        0,
        20,
        3,
        &crate::tabs::CommandLaunch {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), command.into()],
            title: Some("sgr-pty".into()),
            ..crate::tabs::CommandLaunch::default()
        },
        100,
        1024 * 1024,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        tab.poll_single(|_, _| {});
        if tab
            .focused_terminal()
            .unwrap()
            .rows_text(0, 0)
            .contains("ABCDEFGH")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

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

#[test]
fn representative_git_color_output_maps_to_rendered_sgr_state() {
    if !std::path::Path::new("/etc/hostname").exists() {
        return;
    }
    let Ok(output) = std::process::Command::new("git")
        .args([
            "-c",
            "color.ui=always",
            "diff",
            "--no-index",
            "--",
            "/dev/null",
            "/etc/hostname",
        ])
        .output()
    else {
        return;
    };
    assert_eq!(output.status.code(), Some(1));

    let mut terminal = termeric_core::Terminal::new(120, 20);
    terminal.process(&output.stdout);
    let has_bold = (0..terminal.rows()).any(|row| {
        (0..terminal.cols()).any(|col| {
            terminal
                .grid()
                .cell(row, col)
                .is_some_and(|cell| cell.attrs.contains(termeric_core::CellAttrs::BOLD))
        })
    });
    assert!(has_bold);
    assert!(terminal.rows_text(0, 19).contains("diff --git"));
}
