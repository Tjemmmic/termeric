//! Deterministic terminal-core conformance fixtures and invariant smoke tests.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use std::collections::HashSet;
use termeric_core::term::TerminalMode;
use termeric_core::{CellFlags, Terminal, TerminalEvent};

const CONFORMANCE_CASES: &str = include_str!("fixtures/conformance_cases.toml");
const EXTERNAL_TRACES: &str = include_str!("fixtures/external_traces.toml");
const FUZZ_SCROLLBACK_LIMIT: usize = 128;
const FUZZ_SCROLLBACK_BYTE_LIMIT: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
struct FixtureSuite {
    case: Vec<FixtureCase>,
}

#[derive(Debug, Deserialize)]
struct FixtureCase {
    id: String,
    cols: usize,
    rows: usize,
    #[serde(default)]
    allow_screen_checksum_reports: bool,
    input_hex: String,
    #[serde(default)]
    chunks: Vec<usize>,
    expected_text: Option<String>,
    expected_cursor: Option<[usize; 2]>,
    #[serde(default)]
    expected_modes_all: Vec<String>,
    #[serde(default)]
    expected_modes_none: Vec<String>,
    #[serde(default)]
    expected_events: Vec<String>,
    #[serde(default)]
    expected_hyperlinks: Vec<ExpectedHyperlink>,
    expected_working_directory: Option<ExpectedWorkingDirectory>,
}

#[derive(Debug, Deserialize)]
struct ExpectedHyperlink {
    row: usize,
    col: usize,
    url: String,
}

#[derive(Debug, Deserialize)]
struct ExpectedWorkingDirectory {
    uri: String,
    host: Option<String>,
    path: String,
}

#[derive(Debug, Deserialize)]
struct ExternalTraceSuite {
    reference: String,
    trace: Vec<ExternalTrace>,
}

#[derive(Debug, Deserialize)]
struct ExternalTrace {
    id: String,
    cols: usize,
    rows: usize,
    input_base64: String,
    reference_base64: Option<String>,
    expected_contains: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    text: String,
    cursor: (usize, usize),
    modes: u32,
    events: Vec<String>,
}

#[test]
fn replayable_conformance_cases_match_expected_state() {
    let suite: FixtureSuite = toml::from_str(CONFORMANCE_CASES).unwrap();
    let mut ids = HashSet::new();

    for case in &suite.case {
        assert!(
            ids.insert(case.id.as_str()),
            "duplicate fixture {}",
            case.id
        );
        let input = decode_hex(&case.input_hex);
        let mut terminal = bounded_terminal(case.cols, case.rows);
        terminal.set_screen_checksum_reporting(case.allow_screen_checksum_reports);
        feed_chunks(&mut terminal, &input, &case.chunks);
        assert_terminal_invariants(&terminal);

        if let Some(expected) = &case.expected_text {
            assert_eq!(
                &terminal.rows_text(0, case.rows as i32 - 1),
                expected,
                "fixture {} text",
                case.id
            );
        }
        if let Some([row, col]) = case.expected_cursor {
            let cursor = terminal.grid().cursor();
            assert_eq!(
                (cursor.row, cursor.col),
                (row, col),
                "fixture {} cursor",
                case.id
            );
        }
        for name in &case.expected_modes_all {
            assert!(
                terminal.mode().contains(mode(name)),
                "fixture {} expected mode {}",
                case.id,
                name
            );
        }
        for name in &case.expected_modes_none {
            assert!(
                !terminal.mode().contains(mode(name)),
                "fixture {} unexpectedly has mode {}",
                case.id,
                name
            );
        }
        assert_eq!(
            event_summaries(terminal.drain_events()),
            case.expected_events,
            "fixture {} events",
            case.id
        );
        for expected in &case.expected_hyperlinks {
            let cell = terminal
                .grid()
                .cell(expected.row, expected.col)
                .unwrap_or_else(|| panic!("fixture {} hyperlink cell is absent", case.id));
            let hyperlink_id = cell
                .hyperlink_id
                .unwrap_or_else(|| panic!("fixture {} hyperlink ID is absent", case.id));
            assert_eq!(
                terminal.get_hyperlink_url(hyperlink_id),
                Some(expected.url.as_str()),
                "fixture {} hyperlink URL",
                case.id
            );
        }
        if let Some(expected) = &case.expected_working_directory {
            let actual = terminal
                .reported_working_directory()
                .unwrap_or_else(|| panic!("fixture {} has no reported working directory", case.id));
            assert_eq!(actual.uri(), expected.uri, "fixture {} cwd URI", case.id);
            assert_eq!(
                actual.host(),
                expected.host.as_deref(),
                "fixture {} cwd host",
                case.id
            );
            assert_eq!(actual.path(), expected.path, "fixture {} cwd path", case.id);
        }
    }
}

#[test]
fn real_program_traces_replay_against_pinned_reference_snapshots() {
    let suite: ExternalTraceSuite = toml::from_str(EXTERNAL_TRACES).unwrap();
    let mut ids = HashSet::new();
    for trace in &suite.trace {
        assert!(
            ids.insert(trace.id.as_str()),
            "duplicate trace {}",
            trace.id
        );
        let input = decode_base64(&trace.input_base64);
        let mut terminal = bounded_terminal(trace.cols, trace.rows);
        for chunk in input.chunks(17) {
            terminal.process(chunk);
            assert_terminal_invariants(&terminal);
        }

        let actual = terminal.rows_text(0, trace.rows as i32 - 1);
        if let Some(reference) = &trace.reference_base64 {
            let reference = String::from_utf8(decode_base64(reference)).unwrap();
            assert_eq!(
                actual,
                reference.trim_end_matches('\n'),
                "{} differs from {}",
                trace.id,
                suite.reference
            );
        }
        if let Some(needle) = &trace.expected_contains {
            assert!(actual.contains(needle), "{} omitted {needle:?}", trace.id);
        }
    }
}

#[test]
fn parser_streaming_is_independent_of_chunk_boundaries() {
    let suite: FixtureSuite = toml::from_str(CONFORMANCE_CASES).unwrap();
    for case in &suite.case {
        let input = decode_hex(&case.input_hex);
        let expected = run_snapshot(
            case.cols,
            case.rows,
            case.allow_screen_checksum_reports,
            &input,
            &[],
        );
        for split in 0..=input.len() {
            let actual = run_snapshot(
                case.cols,
                case.rows,
                case.allow_screen_checksum_reports,
                &input,
                &[split, input.len() - split],
            );
            assert_eq!(
                actual, expected,
                "fixture {} differs at byte split {}",
                case.id, split
            );
        }
    }
}

#[test]
fn deterministic_parser_fuzz_smoke_preserves_grid_invariants() {
    const STRUCTURED_SEEDS: &[&[u8]] = &[
        b"\x1b[38;2;12;34;56mcolor\x1b[0m",
        b"\x1b]8;;https://example.com\x07link\x1b]8;;\x07",
        b"\x1b]133;A\x07\x1b]133;B\x07echo ok\r\n\x1b]133;D;0\x07",
        b"\x1b_Ga=q,i=1;\x1b\\",
        b"\x1bP1;2;3+qpayload\x1b\\",
        b"\xf0\x9f\x91\xa9\xe2\x80\x8d\xf0\x9f\x92\xbb",
        "combining e\u{301} emoji 👩‍💻 CJK 界".as_bytes(),
    ];

    for seed in 0..512_u64 {
        let mut state = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let cols = (next_u64(&mut state) as usize % 80) + 1;
        let rows = (next_u64(&mut state) as usize % 30) + 1;
        let mut terminal = bounded_terminal(cols, rows);
        let prefix = STRUCTURED_SEEDS[seed as usize % STRUCTURED_SEEDS.len()];
        terminal.process(prefix);

        let mut bytes = vec![0_u8; 384];
        for byte in &mut bytes {
            *byte = next_u64(&mut state) as u8;
        }

        let chunk_size = (next_u64(&mut state) as usize % 31) + 1;
        for (index, chunk) in bytes.chunks(chunk_size).enumerate() {
            terminal.process(chunk);
            if index % 7 == 0 {
                let new_cols = (next_u64(&mut state) as usize % 80) + 1;
                let new_rows = (next_u64(&mut state) as usize % 30) + 1;
                terminal.resize(new_cols, new_rows);
            }
            assert_terminal_invariants(&terminal);
        }
        let _ = terminal.drain_events();
    }
}

#[test]
fn parser_and_reflow_respect_the_scrollback_memory_budget() {
    const BYTE_LIMIT: usize = 4 * 1024;

    let mut terminal = Terminal::new(24, 4);
    terminal.set_scrollback_limits(10_000, BYTE_LIMIT);
    for line in 0..512 {
        let input = format!(
            "\x1b[38;2;{};{};{}mline-{line:03}-界-e\u{301}-👩‍💻\x1b[0m\r\n",
            line % 251,
            (line * 3) % 251,
            (line * 7) % 251
        );
        for chunk in input.as_bytes().chunks(7) {
            terminal.process(chunk);
        }
        if line % 17 == 0 {
            terminal.resize(31, 5);
            terminal.resize(24, 4);
        }
        assert_terminal_invariants(&terminal);
    }

    let usage = terminal.grid().memory_usage();
    assert_eq!(usage.scrollback_byte_limit, BYTE_LIMIT);
    assert!(usage.scrollback_bytes <= BYTE_LIMIT);
    assert!(
        usage.scrollback_lines < 512,
        "byte cap never evicted history"
    );
}

fn decode_hex(input: &str) -> Vec<u8> {
    input
        .split_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16).unwrap())
        .collect()
}

fn decode_base64(input: &str) -> Vec<u8> {
    let compact: String = input.split_whitespace().collect();
    STANDARD
        .decode(compact)
        .expect("trace fixtures hold valid base64")
}

fn feed_chunks(terminal: &mut Terminal, input: &[u8], chunks: &[usize]) {
    if chunks.is_empty() {
        terminal.process(input);
        return;
    }

    let mut offset: usize = 0;
    for &size in chunks {
        let end = offset.saturating_add(size).min(input.len());
        terminal.process(&input[offset..end]);
        offset = end;
        if offset == input.len() {
            return;
        }
    }
    terminal.process(&input[offset..]);
}

fn run_snapshot(
    cols: usize,
    rows: usize,
    allow_screen_checksum_reports: bool,
    input: &[u8],
    chunks: &[usize],
) -> Snapshot {
    let mut terminal = bounded_terminal(cols, rows);
    terminal.set_screen_checksum_reporting(allow_screen_checksum_reports);
    feed_chunks(&mut terminal, input, chunks);
    assert_terminal_invariants(&terminal);
    let cursor = terminal.grid().cursor();
    Snapshot {
        text: terminal.rows_text(0, rows as i32 - 1),
        cursor: (cursor.row, cursor.col),
        modes: terminal.mode().bits(),
        events: event_summaries(terminal.drain_events()),
    }
}

fn bounded_terminal(cols: usize, rows: usize) -> Terminal {
    let mut terminal = Terminal::new(cols, rows);
    terminal.set_scrollback_limits(FUZZ_SCROLLBACK_LIMIT, FUZZ_SCROLLBACK_BYTE_LIMIT);
    terminal
}

fn event_summaries(events: Vec<TerminalEvent>) -> Vec<String> {
    events
        .into_iter()
        .map(|event| match event {
            TerminalEvent::Bell => "bell".to_string(),
            TerminalEvent::PtyWrite(bytes) => format!("pty:{}", encode_hex(&bytes)),
            TerminalEvent::Notification(request) => {
                let mut policies = Vec::new();
                if request.report_close {
                    policies.push("report-close".to_string());
                }
                if request.report_activation {
                    policies.push("report-activation".to_string());
                }
                if request.focus_on_activation {
                    policies.push("focus-on-activation".to_string());
                }
                match request.occasion {
                    termeric_core::NotificationOccasion::Always => {}
                    termeric_core::NotificationOccasion::Unfocused => {
                        policies.push("occasion=unfocused".to_string());
                    }
                    termeric_core::NotificationOccasion::Invisible => {
                        policies.push("occasion=invisible".to_string());
                    }
                }
                if !request.buttons.is_empty() {
                    policies.push(format!("buttons={}", request.buttons.len()));
                }
                if let Some(cache_id) = request.icon_cache_id {
                    policies.push(format!("icon-cache={cache_id}"));
                }
                if let Some(icon_data) = request.icon_data {
                    policies.push(format!("icon-bytes={}", icon_data.len()));
                }
                let policies = if policies.is_empty() {
                    String::new()
                } else {
                    format!(":{}", policies.join(":"))
                };
                format!(
                    "notification:{}:{}{policies}",
                    request.title.unwrap_or_default(),
                    request.body
                )
            }
            TerminalEvent::NotificationClose { identifier } => {
                format!("notification-close:{identifier}")
            }
            TerminalEvent::NotificationAliveQuery { identifier, .. } => {
                format!("notification-alive:{identifier}")
            }
            TerminalEvent::KittyFileTransfer(command) => {
                format!("kitty-file-transfer:{:?}:{}", command.action, command.id)
            }
            TerminalEvent::TitleChanged(title) => format!("title:{title}"),
            TerminalEvent::ClipboardRequest {
                selection,
                reply_uses_8bit_controls,
            } => format!("clipboard-request:{selection:?}:{reply_uses_8bit_controls}"),
            TerminalEvent::ClipboardWrite { selection, text } => {
                format!("clipboard-write:{selection:?}:{text}")
            }
            TerminalEvent::DynamicColor(operation) => format!("dynamic-color:{operation:?}"),
            TerminalEvent::HyperlinkActivated(url) => format!("hyperlink:{url}"),
            TerminalEvent::CursorVisibilityChanged(visible) => format!("cursor:{visible}"),
            TerminalEvent::PointerShapeChanged => "pointer-shape-changed".to_string(),
            TerminalEvent::Exit => "exit".to_string(),
        })
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;

    bytes.iter().fold(String::new(), |mut output, byte| {
        write!(output, "{byte:02x}").expect("writing to a String cannot fail");
        output
    })
}

fn assert_terminal_invariants(terminal: &Terminal) {
    let grid = terminal.grid();
    assert_eq!(grid.cols(), terminal.cols());
    assert_eq!(grid.rows(), terminal.rows());
    assert!(grid.cols() > 0 && grid.rows() > 0);
    assert!(grid.scroll_offset() <= grid.scrollback_len());
    let active_usage = grid.memory_usage();
    assert!(active_usage.scrollback_lines <= active_usage.scrollback_line_limit);
    assert!(active_usage.scrollback_bytes <= active_usage.scrollback_byte_limit);
    let total_usage = terminal.memory_usage();
    assert!(total_usage.scrollback_lines <= total_usage.scrollback_line_limit);
    assert!(total_usage.scrollback_bytes <= total_usage.scrollback_byte_limit);

    let scroll_region = terminal.scroll_region();
    assert!(scroll_region.start < scroll_region.end);
    assert!(scroll_region.end <= terminal.rows());
    let horizontal_region = terminal.horizontal_scroll_region();
    assert!(horizontal_region.start < horizontal_region.end);
    assert!(horizontal_region.end <= terminal.cols());

    let cursor = grid.cursor();
    assert!(cursor.row < grid.rows());
    assert!(cursor.col < grid.cols());

    for row_index in 0..grid.rows() {
        let row = grid.row(row_index).expect("visible row must exist");
        assert_eq!(row.len(), grid.cols());
        let flags: Vec<CellFlags> = row.iter().map(|cell| cell.flags).collect();
        assert_wide_pairs_are_whole(&flags, &format!("visible row {row_index}"));
        for (col, cell) in row.iter().enumerate() {
            if cell.multicell.is_present() {
                assert!(
                    grid.multicell_region_at(row_index, col).is_some(),
                    "multicell descriptor at {row_index}:{col} must resolve to one complete block"
                );
            }
        }
    }

    for history_index in 1..=grid.scrollback_len() {
        let row = grid
            .row_at_offset(-(history_index as i32))
            .expect("retained scrollback row must exist");
        assert_eq!(row.len(), grid.cols());
        let flags: Vec<CellFlags> = row.iter().map(|cell| cell.flags).collect();
        assert_wide_pairs_are_whole(&flags, &format!("history row {history_index}"));
    }
}

/// Every wide cell must be followed by its spacer and every spacer preceded by
/// its wide cell, so neither half of a pair can start or end a row.
fn assert_wide_pairs_are_whole(flags: &[CellFlags], row: &str) {
    for (col, cell) in flags.iter().enumerate() {
        if cell.contains(CellFlags::WIDE_CHAR) {
            assert!(
                flags
                    .get(col + 1)
                    .is_some_and(|next| next.contains(CellFlags::WIDE_CHAR_SPACER)),
                "{row}: wide cell at column {col} has no spacer after it"
            );
        }
        if cell.contains(CellFlags::WIDE_CHAR_SPACER) {
            assert!(
                col > 0 && flags[col - 1].contains(CellFlags::WIDE_CHAR),
                "{row}: spacer at column {col} has no wide cell before it"
            );
        }
    }
}

fn mode(name: &str) -> TerminalMode {
    match name {
        "INSERT" => TerminalMode::INSERT,
        "CURSOR_KEYS_APPLICATION" => TerminalMode::CURSOR_KEYS_APPLICATION,
        "ORIGIN" => TerminalMode::ORIGIN,
        "AUTOWRAP" => TerminalMode::AUTOWRAP,
        "CURSOR_VISIBLE" => TerminalMode::CURSOR_VISIBLE,
        "LEFT_RIGHT_MARGIN" => TerminalMode::LEFT_RIGHT_MARGIN,
        "MOUSE_X10" => TerminalMode::MOUSE_X10,
        "MOUSE_TRACKING" => TerminalMode::MOUSE_TRACKING,
        "MOUSE_BUTTON_EVENT" => TerminalMode::MOUSE_BUTTON_EVENT,
        "MOUSE_ANY_EVENT" => TerminalMode::MOUSE_ANY_EVENT,
        "MOUSE_UTF8" => TerminalMode::MOUSE_UTF8,
        "MOUSE_SGR" => TerminalMode::MOUSE_SGR,
        "FOCUS_EVENTS" => TerminalMode::FOCUS_EVENTS,
        "ALT_SCREEN" => TerminalMode::ALT_SCREEN,
        "ALTERNATE_SCROLL" => TerminalMode::ALTERNATE_SCROLL,
        "BRACKETED_PASTE" => TerminalMode::BRACKETED_PASTE,
        "KEYPAD_APPLICATION" => TerminalMode::KEYPAD_APPLICATION,
        "LINEFEED_NEWLINE" => TerminalMode::LINEFEED_NEWLINE,
        "SYNCHRONIZED_OUTPUT" => TerminalMode::SYNCHRONIZED_OUTPUT,
        _ => panic!("unknown fixture mode {name}"),
    }
}

fn next_u64(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1);
    *state
}
