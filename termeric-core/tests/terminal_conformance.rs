//! Deterministic terminal-core conformance fixtures and invariant smoke tests.

use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;
use termeric_core::term::TerminalMode;
use termeric_core::{CellFlags, Terminal, TerminalEvent};

const SUPPORT_MANIFEST: &str = include_str!("fixtures/vt_support.toml");
const PRODUCT_SUPPORT_MATRIX: &str = include_str!("../../docs/feature-support.md");
const PROJECT_README: &str = include_str!("../../README.md");
const CONFORMANCE_CASES: &str = include_str!("fixtures/conformance_cases.toml");
const PARSER_TEST_SEEDS: &str = include_str!("fixtures/parser_test_seeds.toml");
const EXTERNAL_TRACES: &str = include_str!("fixtures/external_traces.toml");
const FUZZ_SCROLLBACK_LIMIT: usize = 128;
const FUZZ_SCROLLBACK_BYTE_LIMIT: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
struct SupportManifest {
    manifest_version: u32,
    baseline: String,
    sequence: Vec<SequenceClaim>,
}

#[derive(Debug, Deserialize)]
struct SequenceClaim {
    id: String,
    family: String,
    syntax: String,
    layer: String,
    status: String,
    #[serde(default)]
    evidence: Vec<String>,
    tracking_issue: Option<String>,
    notes: String,
}

#[derive(Debug, Deserialize)]
struct FixtureSuite {
    manifest_version: u32,
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
    expected_keyboard_type: Option<String>,
    expected_keyboard_resource: Option<ExpectedKeyboardResource>,
    expected_cursor_keys: Option<ExpectedCursorKeys>,
    expected_function_keys: Option<ExpectedFunctionKeys>,
    expected_keypad_keys: Option<ExpectedFunctionKeys>,
    expected_string_keys: Option<ExpectedFunctionKeys>,
    expected_modifier_keys: Option<ExpectedFunctionKeys>,
    expected_special_keys: Option<ExpectedFunctionKeys>,
    expected_modify_other_keys: Option<ExpectedModifyOtherKeys>,
    #[serde(default)]
    expected_user_defined_keys: Vec<ExpectedUserDefinedKey>,
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
struct ExpectedModifyOtherKeys {
    level: u8,
    modifier_mask: u8,
    format: u8,
}

#[derive(Debug, Deserialize)]
struct ExpectedKeyboardResource {
    modify: u16,
    format: u16,
}

#[derive(Debug, Deserialize)]
struct ExpectedCursorKeys {
    mode: u16,
    format: u8,
}

#[derive(Debug, Deserialize)]
struct ExpectedFunctionKeys {
    mode: u16,
    format: u8,
}

#[derive(Debug, Deserialize)]
struct ExpectedUserDefinedKey {
    code: u8,
    value_hex: String,
}

#[derive(Debug, Deserialize)]
struct ParserSeedSuite {
    manifest_version: u32,
    seed: Vec<ParserSeed>,
}

#[derive(Debug, Deserialize)]
struct ParserSeed {
    id: String,
    source_test: String,
    input_hex: String,
}

#[derive(Debug, Deserialize)]
struct ExternalTraceSuite {
    manifest_version: u32,
    captured_at: String,
    terminal: String,
    reference: String,
    trace: Vec<ExternalTrace>,
}

#[derive(Debug, Deserialize)]
struct ExternalTrace {
    id: String,
    program: String,
    command: String,
    cols: usize,
    rows: usize,
    input_base64: String,
    reference_base64: Option<String>,
    expected_contains: Option<String>,
    tracking_issue: String,
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    text: String,
    cursor: (usize, usize),
    modes: u32,
    events: Vec<String>,
}

#[test]
fn support_manifest_is_versioned_unique_and_evidenced() {
    let manifest: SupportManifest = toml::from_str(SUPPORT_MANIFEST).unwrap();
    let fixtures: FixtureSuite = toml::from_str(CONFORMANCE_CASES).unwrap();
    assert_eq!(manifest.manifest_version, 1);
    assert!(!manifest.baseline.trim().is_empty());
    assert_eq!(fixtures.manifest_version, 1);

    let fixture_ids: HashSet<&str> = fixtures.case.iter().map(|case| case.id.as_str()).collect();
    let mut claim_ids = HashSet::new();
    let allowed_statuses = [
        "supported",
        "experimental",
        "partial",
        "parsed_only",
        "unsupported",
    ];
    let allowed_layers = ["core", "app", "render"];

    for claim in &manifest.sequence {
        assert!(
            claim_ids.insert(claim.id.as_str()),
            "duplicate claim {}",
            claim.id
        );
        assert!(
            !claim.family.trim().is_empty(),
            "{} has no family",
            claim.id
        );
        assert!(
            !claim.syntax.trim().is_empty(),
            "{} has no syntax",
            claim.id
        );
        assert!(
            !claim.notes.trim().is_empty(),
            "{} has no boundary note",
            claim.id
        );
        assert!(
            allowed_statuses.contains(&claim.status.as_str()),
            "{} has invalid status {}",
            claim.id,
            claim.status
        );
        assert!(
            allowed_layers.contains(&claim.layer.as_str()),
            "{} has invalid layer {}",
            claim.id,
            claim.layer
        );

        if claim.status != "unsupported" {
            assert!(
                !claim.evidence.is_empty(),
                "{} claims {} without evidence",
                claim.id,
                claim.status
            );
        }
        if matches!(
            claim.status.as_str(),
            "partial" | "parsed_only" | "unsupported"
        ) {
            let issue = claim.tracking_issue.as_deref().unwrap_or_default();
            assert!(
                issue.starts_with("https://github.com/Tjemmmic/termeric/issues/"),
                "{} needs a tracking issue",
                claim.id
            );
        }

        for evidence in &claim.evidence {
            if let Some(fixture) = evidence.strip_prefix("fixture:") {
                assert!(
                    fixture_ids.contains(fixture),
                    "{} references missing fixture {}",
                    claim.id,
                    fixture
                );
            } else if let Some(test) = evidence.strip_prefix("test:") {
                validate_test_reference(&claim.id, test);
            } else {
                panic!("{} has invalid evidence reference {}", claim.id, evidence);
            }
        }
    }
}

#[test]
fn product_support_matrix_matches_cross_layer_protocol_contracts() {
    let manifest: SupportManifest = toml::from_str(SUPPORT_MANIFEST).unwrap();

    assert_product_contract(
        &manifest,
        "Application keyboard modes and modifiers",
        "Partial",
        &[
            ("csi-kitty-keyboard-enhancements", "partial"),
            ("csi-xterm-modify-other-keys", "supported"),
            ("csi-xterm-keyboard-resource-zero", "supported"),
            ("dec-xterm-keyboard-types", "supported"),
            ("dcs-dec-user-defined-keys", "supported"),
            ("csi-xterm-cursor-key-resources", "supported"),
            ("csi-xterm-function-key-resources", "supported"),
            ("csi-xterm-keypad-key-resources", "supported"),
            ("csi-xterm-string-key-resources", "supported"),
            ("csi-xterm-modifier-key-resources", "supported"),
            ("csi-xterm-special-key-resources", "supported"),
        ],
    );
    assert_product_contract(
        &manifest,
        "Terminal capability and status queries",
        "Partial",
        &[
            ("csi-xtversion", "supported"),
            ("csi-kitty-keyboard-state-query", "supported"),
            ("csi-kitty-keyboard-enhancements", "partial"),
            ("csi-mode-status", "supported"),
            ("dcs-xtgettcap", "partial"),
            ("dcs-decrqss", "partial"),
        ],
    );
    assert_product_contract(
        &manifest,
        "Synchronized output",
        "Supported",
        &[("csi-synchronized-output", "supported")],
    );
    assert_product_contract(
        &manifest,
        "OSC 8 hyperlinks and detected URLs",
        "Supported",
        &[("osc-hyperlinks", "supported")],
    );
    assert_product_contract(
        &manifest,
        "OSC 133 shell semantics",
        "Supported (Linux)",
        &[("osc-shell-semantics", "supported")],
    );
    assert_product_contract(
        &manifest,
        "Shell-reported working directory",
        "Supported (Linux)",
        &[("osc-current-directory", "supported")],
    );
    assert_product_contract(
        &manifest,
        "Kitty graphics",
        "Partial",
        &[("apc-kitty-graphics", "partial")],
    );
    assert_product_contract(
        &manifest,
        "Sixel and iTerm2 images",
        "Partial",
        &[
            ("dcs-sixel", "partial"),
            ("osc-iterm2-inline-images", "partial"),
        ],
    );

    assert!(
        PROJECT_README.contains("accepted Bash/Zsh/Fish OSC 133 semantics"),
        "README must summarize the supported shell-semantics boundary"
    );
    assert!(
        PROJECT_README.contains("acceptance-pending native Nushell and opt-in Elvish semantics"),
        "README must disclose the pending and manual shell-semantics boundaries"
    );
    assert!(
        !PROJECT_README.contains("experimental OSC 133"),
        "README must not contradict the authoritative support matrix"
    );
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
        if let Some(expected) = &case.expected_keyboard_type {
            let actual = match terminal.xterm_keyboard_type() {
                termeric_core::term::XtermKeyboardType::Normal => "normal",
                termeric_core::term::XtermKeyboardType::Legacy => "legacy",
                termeric_core::term::XtermKeyboardType::Vt220 => "vt220",
            };
            assert_eq!(actual, expected, "fixture {} keyboard type", case.id);
        }
        for expected in &case.expected_user_defined_keys {
            assert_eq!(
                terminal.xterm_user_defined_key(expected.code),
                Some(decode_hex(&expected.value_hex).as_slice()),
                "fixture {} UDK code {}",
                case.id,
                expected.code
            );
        }
        if let Some(expected) = &case.expected_modify_other_keys {
            let actual = terminal.xterm_modify_other_keys();
            assert_eq!(
                actual.level(),
                expected.level,
                "fixture {} MOK level",
                case.id
            );
            assert_eq!(
                actual.modifier_mask(),
                expected.modifier_mask,
                "fixture {} MOK mask",
                case.id
            );
            assert_eq!(
                actual.format().parameter(),
                expected.format,
                "fixture {} MOK format",
                case.id
            );
        }
        if let Some(expected) = &case.expected_keyboard_resource {
            let actual = terminal.xterm_keyboard_resource();
            assert_eq!(
                actual.modify(),
                expected.modify,
                "fixture {} keyboard-resource modify value",
                case.id
            );
            assert_eq!(
                actual.format(),
                expected.format,
                "fixture {} keyboard-resource format value",
                case.id
            );
        }
        if let Some(expected) = &case.expected_cursor_keys {
            let actual = terminal.xterm_cursor_keys();
            assert_eq!(
                actual.mode().parameter(),
                expected.mode,
                "fixture {} cursor-key mode",
                case.id
            );
            assert_eq!(
                actual.format().parameter(),
                expected.format,
                "fixture {} cursor-key format",
                case.id
            );
        }
        if let Some(expected) = &case.expected_function_keys {
            let actual = terminal.xterm_function_keys();
            assert_eq!(
                actual.mode().parameter(),
                expected.mode,
                "fixture {} function-key mode",
                case.id
            );
            assert_eq!(
                actual.format().parameter(),
                expected.format,
                "fixture {} function-key format",
                case.id
            );
        }
        if let Some(expected) = &case.expected_string_keys {
            let actual = terminal.xterm_string_keys();
            assert_eq!(
                actual.mode().parameter(),
                expected.mode,
                "fixture {} string-key mode",
                case.id
            );
            assert_eq!(
                actual.format().parameter(),
                expected.format,
                "fixture {} string-key format",
                case.id
            );
        }
        if let Some(expected) = &case.expected_keypad_keys {
            let actual = terminal.xterm_keypad_keys();
            assert_eq!(
                actual.mode().parameter(),
                expected.mode,
                "fixture {} keypad-key mode",
                case.id
            );
            assert_eq!(
                actual.format().parameter(),
                expected.format,
                "fixture {} keypad-key format",
                case.id
            );
        }
        if let Some(expected) = &case.expected_modifier_keys {
            let actual = terminal.xterm_modifier_keys();
            assert_eq!(
                actual.mode().parameter(),
                expected.mode,
                "fixture {} modifier-key mode",
                case.id
            );
            assert_eq!(
                actual.format().parameter(),
                expected.format,
                "fixture {} modifier-key format",
                case.id
            );
        }
        if let Some(expected) = &case.expected_special_keys {
            let actual = terminal.xterm_special_keys();
            assert_eq!(
                actual.mode().parameter(),
                expected.mode,
                "fixture {} special-key mode",
                case.id
            );
            assert_eq!(
                actual.format().parameter(),
                expected.format,
                "fixture {} special-key format",
                case.id
            );
        }
    }
}

#[test]
fn every_existing_processor_test_seeds_the_fuzz_corpus() {
    let suite: ParserSeedSuite = toml::from_str(PARSER_TEST_SEEDS).unwrap();
    assert_eq!(suite.manifest_version, 1);

    let mut seed_ids = HashSet::new();
    let mut seeded_functions = HashSet::new();
    for seed in &suite.seed {
        assert!(
            seed_ids.insert(seed.id.as_str()),
            "duplicate seed {}",
            seed.id
        );
        validate_test_reference(&seed.id, &seed.source_test);
        let (_, function) = seed.source_test.split_once('#').unwrap();
        assert!(
            seeded_functions.insert(function),
            "duplicate source seed {function}"
        );

        let mut terminal = bounded_terminal(80, 24);
        terminal.process(&decode_hex(&seed.input_hex));
        assert_terminal_invariants(&terminal);
    }

    let source = include_str!("../src/ansi/processor.rs");
    let tests = source
        .split("#[cfg(test)]")
        .nth(1)
        .expect("processor test module");
    let discovered: HashSet<&str> = tests
        .lines()
        .filter_map(|line| line.trim().strip_prefix("fn "))
        .filter_map(|signature| signature.split_once('(').map(|(name, _)| name))
        .collect();
    assert_eq!(seeded_functions, discovered, "parser test corpus is stale");
}

#[test]
fn real_program_traces_replay_against_pinned_reference_snapshots() {
    let suite: ExternalTraceSuite = toml::from_str(EXTERNAL_TRACES).unwrap();
    assert_eq!(suite.manifest_version, 1);
    assert!(!suite.captured_at.trim().is_empty());
    assert_eq!(suite.terminal, "xterm-256color");
    assert!(suite.reference.contains("tmux 3.7c"));

    let mut ids = HashSet::new();
    let mut programs = HashSet::new();
    for trace in &suite.trace {
        assert!(
            ids.insert(trace.id.as_str()),
            "duplicate trace {}",
            trace.id
        );
        assert!(
            !trace.command.trim().is_empty(),
            "{} has no capture command",
            trace.id
        );
        assert!(
            trace
                .tracking_issue
                .starts_with("https://github.com/Tjemmmic/termeric/issues/")
        );
        programs.insert(trace.program.split_whitespace().next().unwrap_or_default());

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
    assert_eq!(programs, HashSet::from(["Vim", "less", "tmux"]));
}

#[test]
fn real_full_screen_traces_request_application_keyboard_modes() {
    let suite: ExternalTraceSuite = toml::from_str(EXTERNAL_TRACES).unwrap();
    let mut checked = HashSet::new();
    for trace in &suite.trace {
        let program = trace
            .program
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !matches!(program.as_str(), "vim" | "less" | "tmux") {
            continue;
        }
        let input = decode_base64(&trace.input_base64);
        assert!(
            input
                .windows(b"\x1b[?1h".len())
                .any(|window| window == b"\x1b[?1h"),
            "{} never requested DECCKM",
            trace.id
        );
        assert!(
            input
                .windows(b"\x1b=".len())
                .any(|window| window == b"\x1b="),
            "{} never requested DECKPAM",
            trace.id
        );
        checked.insert(program);
    }
    assert_eq!(
        checked,
        HashSet::from(["less".into(), "tmux".into(), "vim".into()])
    );
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

fn validate_test_reference(claim_id: &str, reference: &str) {
    let (relative_path, function) = reference
        .split_once('#')
        .unwrap_or_else(|| panic!("{} has invalid test reference {}", claim_id, reference));
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path);
    let source = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{} references unreadable {}: {}",
            claim_id,
            path.display(),
            error
        )
    });
    let signature = format!("fn {function}(");
    let position = source.find(&signature).unwrap_or_else(|| {
        panic!(
            "{} references missing test {}#{}",
            claim_id, relative_path, function
        )
    });
    let is_test = source[..position]
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .take_while(|line| line.trim().starts_with("#["))
        .any(|line| line.contains("test"));
    assert!(
        is_test,
        "{} references function {}#{} without a test attribute",
        claim_id, relative_path, function
    );
}

fn assert_product_contract(
    manifest: &SupportManifest,
    capability: &str,
    expected_product_status: &str,
    expected_protocol_claims: &[(&str, &str)],
) {
    let prefix = format!("| {capability} |");
    let matching_rows: Vec<&str> = PRODUCT_SUPPORT_MATRIX
        .lines()
        .filter(|line| line.starts_with(&prefix))
        .collect();
    assert_eq!(
        matching_rows.len(),
        1,
        "feature matrix must contain exactly one row for {capability}"
    );
    let cells: Vec<&str> = matching_rows[0].split('|').map(str::trim).collect();
    let actual_product_status = cells
        .get(2)
        .unwrap_or_else(|| panic!("feature matrix row for {capability} has no status"))
        .trim_matches('*');
    assert_eq!(
        actual_product_status, expected_product_status,
        "feature matrix status drifted for {capability}"
    );

    for (claim_id, expected_status) in expected_protocol_claims {
        let claim = manifest
            .sequence
            .iter()
            .find(|claim| claim.id == *claim_id)
            .unwrap_or_else(|| panic!("feature matrix contract references missing {claim_id}"));
        assert_eq!(
            claim.status, *expected_status,
            "protocol status drifted for {claim_id}, which backs {capability}"
        );
    }
}

fn decode_hex(input: &str) -> Vec<u8> {
    input
        .split_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16).unwrap())
        .collect()
}

fn decode_base64(input: &str) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut quartet = [0_u8; 4];
    let mut count = 0;

    for byte in input.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        quartet[count] = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 64,
            _ => panic!("invalid base64 byte {byte}"),
        };
        count += 1;
        if count == 4 {
            output.push((quartet[0] << 2) | (quartet[1] >> 4));
            if quartet[2] != 64 {
                output.push((quartet[1] << 4) | (quartet[2] >> 2));
            }
            if quartet[3] != 64 {
                output.push((quartet[2] << 6) | quartet[3]);
            }
            count = 0;
        }
    }
    assert_eq!(count, 0, "base64 input must contain complete quartets");
    output
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
            TerminalEvent::Notification(request) => format!(
                "notification:{}:{}",
                request.title.unwrap_or_default(),
                request.body
            ),
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
        for col in 0..grid.cols() {
            let cell = row.get(col).expect("visible cell must exist");
            if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                assert!(col > 0, "wide spacer cannot start a row");
                assert!(
                    row.get(col - 1)
                        .unwrap()
                        .flags
                        .contains(CellFlags::WIDE_CHAR)
                );
            }
            if cell.flags.contains(CellFlags::WIDE_CHAR) {
                assert!(col + 1 < grid.cols(), "wide cell cannot end a row");
                assert!(
                    row.get(col + 1)
                        .unwrap()
                        .flags
                        .contains(CellFlags::WIDE_CHAR_SPACER)
                );
            }
        }
    }

    for history_index in 1..=grid.scrollback_len() {
        let row = grid
            .row_at_offset(-(history_index as i32))
            .expect("retained scrollback row must exist");
        assert_eq!(row.len(), grid.cols());
        for col in 0..grid.cols() {
            let cell = row.get(col).expect("history cell must exist");
            if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                assert!(col > 0, "wide spacer cannot start a history row");
                assert!(
                    row.get(col - 1)
                        .unwrap()
                        .flags
                        .contains(CellFlags::WIDE_CHAR)
                );
            }
            if cell.flags.contains(CellFlags::WIDE_CHAR) {
                assert!(col + 1 < grid.cols(), "wide cell cannot end a history row");
                assert!(
                    row.get(col + 1)
                        .unwrap()
                        .flags
                        .contains(CellFlags::WIDE_CHAR_SPACER)
                );
            }
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
