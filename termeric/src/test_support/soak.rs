//! Opt-in soaks: repeated work checked against this process's own resources.

use super::*;

/// Opt-in process-level resource exercise: repeats terminal, pane, tab,
/// session, transcript, and diff work, then checks this process's own memory,
/// threads, open files, and children stayed within bounds.
#[test]
#[ignore = "a resource soak: cargo test -p termeric -- --ignored scheduled_resource_soak_scenario"]
fn scheduled_resource_soak_scenario() {
    use crate::panes::{PaneManager, SplitDirection};
    use crate::session::transcript::{TranscriptRecorder, TranscriptWrite};
    use crate::terminal::search::Search;
    use std::fmt::Write as _;
    use termeric_core::graphics::ImageStorage;
    use termeric_core::{ImageId, StoredImage};

    let duration = std::env::var("TERMERIC_SOAK_DURATION_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(5)
        .max(1);
    let deadline = Instant::now() + Duration::from_secs(duration);
    let workspace = tempfile::tempdir().unwrap();
    let closed_tab_dir = workspace.path().join("closed-tab");
    let transcript_dir = workspace.path().join("transcripts");

    let mut terminal = Terminal::new(120, 40);
    terminal.set_scrollback_limits(50_000, 16 * 1024 * 1024);
    let mut images = ImageStorage::new(32, 4 * 1024 * 1024);
    let mut diffs = crate::terminal::diff_detector::DiffDetector::new();
    let mut iteration = 0usize;
    let mut baseline = None;
    let mut most_children = 0;
    let mut largest_rss_kib = 0;

    while Instant::now() < deadline {
        let mut output = String::with_capacity(32 * 1024);
        for line in 0..128 {
            writeln!(
                output,
                "{iteration:06}:{line:03} needle ASCII 界 e\u{301} 👩\u{200d}💻 \x1b[38;2;120;180;90mstyled\x1b[0m\r"
            )
            .unwrap();
        }
        terminal.process(output.as_bytes());
        terminal.resize(if iteration.is_multiple_of(2) { 80 } else { 132 }, 40);
        let mut search = Search::new();
        search.start();
        for character in "needle".chars() {
            search.push_char(character, terminal.grid());
        }
        search.finish(terminal.grid());
        assert!(search.match_count() > 0);

        let pixels = vec![(iteration % 251) as u8; 64 * 64 * 4];
        images
            .store(StoredImage {
                id: ImageId(iteration as u32),
                width: 64,
                height: 64,
                data: pixels.into(),
                revision: iteration as u64,
            })
            .unwrap();
        assert!(images.memory_usage() <= 4 * 1024 * 1024);
        assert!(images.len() <= 32);

        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "printf 'pane-soak\\n'".into()],
            env: Default::default(),
        };
        let mut panes = PaneManager::for_test(
            100,
            36,
            shell,
            1_000,
            8 * 1024 * 1024,
            1_200.0,
            720.0,
            10.0,
            20.0,
        )
        .unwrap();
        panes.split(SplitDirection::Vertical).unwrap();
        panes.split(SplitDirection::Horizontal).unwrap();
        panes.resize(
            crate::terminal::geometry::TerminalArea::from_available_pixels(900.0, 600.0, 9.0, 18.0),
        );
        let pane_deadline = Instant::now() + Duration::from_secs(2);
        while !panes.all_closed() && Instant::now() < pane_deadline {
            panes.poll_events(|_, _| {});
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(panes.all_closed(), "pane child did not exit during soak");
        drop(panes);

        let mut tabs = TabManager::new(100, 36, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
        for tab in 0..4 {
            tabs.new_tab_with_command(
                "/bin/sh",
                &["-c".into(), format!("printf 'tab-{tab}-soak\\n'")],
                None,
                Some("soak"),
                None,
                HashMap::new(),
            )
            .unwrap();
        }
        let tab_deadline = Instant::now() + Duration::from_secs(2);
        while !tabs.is_empty() && Instant::now() < tab_deadline {
            tabs.poll_events();
            tabs.cleanup_closed(false, false);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(tabs.is_empty(), "tab child did not exit during soak");

        let mut closed_tab = crate::workspace::WorkspaceSnapshot::new(
            "workspace-resource-soak".into(),
            format!("soak {iteration}"),
            vec![crate::workspace::WorkspaceTabSnapshot {
                id: 0,
                title: "soak".into(),
                user_title: None,
                pinned: false,
                primary_pane: 0,
                focused_pane: 0,
                layout: crate::workspace::WorkspacePaneLayout::Pane { pane: 0 },
                panes: vec![crate::workspace::WorkspacePaneSnapshot {
                    id: 0,
                    launch: crate::workspace::WorkspaceLaunch::Shell,
                    title: None,
                    working_directory: Some("/tmp".into()),
                    last_command: None,
                    scrollback_file: None,
                    scrollback_truncated: false,
                    scrollback_lines: crate::workspace::restorable_scrollback(terminal.grid()),
                }],
            }],
        );
        closed_tab.closed_tab = true;
        crate::workspace::save_workspace_to_dir(&mut closed_tab, &closed_tab_dir).unwrap();

        let mut transcript =
            TranscriptRecorder::new(&transcript_dir, Some("soak"), Some("main"), "local", "soak")
                .unwrap();
        let mut accepted_entries = 0usize;
        let mut dropped_entries = 0usize;
        for entry in 0..64 {
            let output_result = transcript
                .record_output(format!("iteration={iteration} output={entry} 界\n"))
                .unwrap();
            if output_result == TranscriptWrite::Written {
                accepted_entries += 1;
            } else {
                dropped_entries += 1;
            }
            if entry % 16 == 0 {
                let input_result = transcript.record_input("echo soak\r").unwrap();
                if input_result == TranscriptWrite::Written {
                    accepted_entries += 1;
                } else {
                    dropped_entries += 1;
                }
            }
        }
        // Every entry is either written or counted as dropped.
        assert_eq!(accepted_entries + dropped_entries, 68);
        let queued_bytes = transcript.queued_bytes_for_diagnostics();
        let transcript_path = transcript.path().to_path_buf();
        transcript.finish(Duration::from_secs(2)).unwrap();
        let drained_queue_bytes = transcript.queued_bytes_for_diagnostics();
        assert_eq!(drained_queue_bytes, 0, "transcript queue did not drain");
        std::fs::remove_file(transcript_path).unwrap();

        soak_diff_history(&mut diffs, iteration);
        assert!(
            queued_bytes <= 2 * 1024 * 1024,
            "transcript queue grew to {queued_bytes} bytes"
        );

        iteration += 1;
        let sample = ProcessResources::sample();
        most_children = most_children.max(sample.children);
        largest_rss_kib = largest_rss_kib.max(sample.rss_kib);
        // The first iteration warms caches, pools, and worker threads up.
        baseline.get_or_insert(sample);
        std::thread::sleep(Duration::from_millis(5));
    }

    assert!(iteration > 1, "the soak needs at least two iterations");
    assert!(terminal.memory_usage().scrollback_bytes <= 16 * 1024 * 1024);
    let first = baseline.expect("a sample after the first iteration");
    let last = ProcessResources::sample();
    assert!(
        largest_rss_kib <= 512 * 1024,
        "RSS peaked at {largest_rss_kib} KiB"
    );
    assert!(
        last.rss_kib.saturating_sub(first.rss_kib) <= 128 * 1024,
        "RSS grew from {} to {} KiB",
        first.rss_kib,
        last.rss_kib
    );
    assert!(
        last.threads <= first.threads + 4,
        "threads grew {first:?} -> {last:?}"
    );
    assert!(
        last.fds <= first.fds + 8,
        "file descriptors grew {first:?} -> {last:?}"
    );
    assert!(
        most_children <= 16,
        "{most_children} child processes at once"
    );
}

/// This process's resident memory, threads, open files, and live children.
#[derive(Clone, Copy, Debug)]
struct ProcessResources {
    rss_kib: u64,
    threads: usize,
    fds: usize,
    children: usize,
}

impl ProcessResources {
    fn sample() -> Self {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let rss_kib = status
            .lines()
            .find_map(|line| line.strip_prefix("VmRSS:"))
            .and_then(|value| value.split_whitespace().next()?.parse().ok())
            .expect("VmRSS in /proc/self/status");
        let count = |path: &str| std::fs::read_dir(path).unwrap().count();
        let children = std::fs::read_dir("/proc/self/task")
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|task| std::fs::read_to_string(task.path().join("children")).ok())
            .map(|children| children.split_whitespace().count())
            .sum();
        Self {
            rss_kib,
            threads: count("/proc/self/task"),
            fds: count("/proc/self/fd"),
            children,
        }
    }
}

/// Feed one persistent history past both its count and owned-capacity ceilings.
/// All bytes are synthetic; no provider, process, or working tree is involved.
fn soak_diff_history(
    detector: &mut crate::terminal::diff_detector::DiffDetector,
    iteration: usize,
) -> (usize, usize, usize, usize, usize) {
    let before = detector.history_diagnostics().0;
    let mut completed = 0;
    let mut peak_count = 0;
    let mut peak_bytes = 0;
    for patch in 0..80 {
        let payload = if patch < 64 {
            "small 界".into()
        } else {
            "界".repeat(340)
        };
        let lines = if patch < 64 { 1 } else { 200 };
        let mut data = format!(
            "--- a/soak-{iteration}-{patch}\n+++ b/soak-{iteration}-{patch}\n@@ -0,0 +1,{lines} @@\n"
        );
        for _ in 0..lines {
            data.push('+');
            data.push_str(&payload);
            data.push('\n');
        }
        data.push_str("shell prompt\n");
        // Deliberately split UTF-8 scalars and headers across reads.
        for chunk in data.as_bytes().chunks(257) {
            detector.feed_bytes(chunk, |diff| {
                completed += 1;
                assert_eq!(diff.filename, format!("soak-{iteration}-{patch}"));
                assert_eq!(diff.id, (iteration * 80 + completed) as u64);
            });
        }
        let (count, bytes) = detector.history_diagnostics();
        peak_count = peak_count.max(count);
        peak_bytes = peak_bytes.max(bytes);
        assert!(count <= 64 && bytes <= 4 * 1024 * 1024);
    }
    let retained = detector.history_diagnostics().0;
    let evicted = before + completed - retained;
    assert_eq!(completed, 80);
    assert_eq!(peak_count, 64);
    assert!(peak_bytes >= 3 * 1024 * 1024);
    assert!(evicted > 0);
    assert!(
        detector
            .detected_by_id((iteration * 80 + 1) as u64)
            .is_none()
    );
    assert!(
        detector
            .take_notices()
            .all(|notice| notice.contains("evicted"))
    );
    (completed, evicted, retained, peak_count, peak_bytes)
}

#[test]
fn persistent_diff_history_soak_exercises_count_and_byte_eviction() {
    let mut detector = crate::terminal::diff_detector::DiffDetector::new();
    for iteration in 0..3 {
        soak_diff_history(&mut detector, iteration);
    }
}
