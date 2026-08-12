//! Opt-in release performance checks for terminal-core hot paths.

use std::fmt::Write as _;
use std::time::{Duration, Instant};
use termeric_core::Terminal;

fn requested() -> bool {
    std::env::var_os("TERMERIC_PERF_ACCEPTANCE").is_some()
}

#[test]
fn sustained_output_and_large_scrollback_stay_within_beta_budget() {
    if !requested() {
        eprintln!("skipped: set TERMERIC_PERF_ACCEPTANCE=1 for release performance checks");
        return;
    }

    const LINES: usize = 100_000;
    const BUDGET: Duration = Duration::from_secs(3);
    let mut terminal = Terminal::new(120, 40);
    terminal.grid_mut().set_scrollback_limit(LINES);
    let mut output = String::with_capacity(LINES * 48);
    for line in 0..LINES {
        writeln!(
            output,
            "\x1b[3{}m{:06} sustained output αβ界\x1b[0m\r",
            line % 8,
            line
        )
        .unwrap();
    }

    let started = Instant::now();
    terminal.process(output.as_bytes());
    let elapsed = started.elapsed();
    eprintln!(
        "processed {:.1} MiB / {LINES} lines in {:.3}s",
        output.len() as f64 / (1024.0 * 1024.0),
        elapsed.as_secs_f64(),
    );

    assert!(elapsed <= BUDGET, "output budget exceeded: {elapsed:?}");
    assert!(terminal.grid().scrollback_len() <= LINES);
}

#[test]
fn repeated_reflow_stays_within_beta_budget() {
    if !requested() {
        eprintln!("skipped: set TERMERIC_PERF_ACCEPTANCE=1 for release performance checks");
        return;
    }

    const REFLOWS: usize = 200;
    const BUDGET: Duration = Duration::from_secs(3);
    let mut terminal = Terminal::new(120, 40);
    terminal.grid_mut().set_scrollback_limit(20_000);
    let seed = "reflow αβ界 content with enough width to wrap predictably\r\n".repeat(10_000);
    terminal.process(seed.as_bytes());

    let started = Instant::now();
    for iteration in 0..REFLOWS {
        let cols = 80 + iteration % 81;
        terminal.resize(cols, 30 + iteration % 21);
    }
    let elapsed = started.elapsed();
    eprintln!(
        "completed {REFLOWS} reflows in {:.3}s",
        elapsed.as_secs_f64()
    );

    assert!(elapsed <= BUDGET, "reflow budget exceeded: {elapsed:?}");
}
