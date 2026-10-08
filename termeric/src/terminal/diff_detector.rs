//! Bounded passive unified-diff capture for agent diff review.
//! Output is framed across PTY reads; only validated, complete file patches
//! reach review/history or the exactly-once completion callback.

mod framing;
#[cfg(test)]
pub(crate) mod performance;
#[cfg(test)]
mod streaming_tests;

use framing::{Framer, MAX_LINE_BYTES};
use std::collections::VecDeque;

const MAX_PATCH_BYTES: usize = 256 * 1024;
const MAX_PATCH_LINES: usize = 4096;
const MAX_HUNKS: usize = 128;
const MAX_OWNED_PATCH_BYTES: usize = 1024 * 1024;
const MAX_HISTORY_BYTES: usize = 4 * 1024 * 1024;
const MAX_HISTORY_COUNT: usize = 64;
const REJECTED: u8 = 1;
const EVICTED: u8 = 2;

#[derive(Debug, Clone, PartialEq)]
pub enum DiffLine {
    Context(String),
    Added(String),
    Removed(String),
}

#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// A validated, complete detected diff for one file.
#[derive(Debug, Clone)]
pub struct DetectedDiff {
    /// Detector-local, monotonically allocated identity, never reused.
    pub id: u64,
    pub filename: String,
    pub hunks: Vec<DiffHunk>,
    /// SGR/OSC-stripped unified diff, normalized to LF with a final newline.
    pub raw: String,
    pub reviewed: bool,
}

impl DetectedDiff {
    /// All owned storage, including duplicated strings and spare capacity.
    fn owned_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.filename.capacity()
            + self.raw.capacity()
            + self.hunks.capacity() * std::mem::size_of::<DiffHunk>()
            + self
                .hunks
                .iter()
                .map(|hunk| {
                    hunk.header.capacity()
                        + hunk.lines.capacity() * std::mem::size_of::<DiffLine>()
                        + hunk
                            .lines
                            .iter()
                            .map(|line| match line {
                                DiffLine::Context(s)
                                | DiffLine::Added(s)
                                | DiffLine::Removed(s) => s.capacity(),
                            })
                            .sum::<usize>()
                })
                .sum::<usize>()
    }
}

/// Pending captures store raw text only. Parsed copies are built once, after
/// validation, and bounded by the same byte/line/hunk limits.
#[derive(Debug, Default)]
struct Capture {
    raw: String,
    lines: usize,
    hunks: usize,
    remaining: Option<(usize, usize)>,
    range_ends: Option<(usize, usize)>,
    last_was_data: bool,
}

impl Capture {
    fn append(&mut self, line: &str) -> bool {
        let required = self.raw.len() + line.len() + 1;
        if required > MAX_PATCH_BYTES || self.lines == MAX_PATCH_LINES {
            return false;
        }
        if required > self.raw.capacity() {
            let target = required
                .max(self.raw.capacity().saturating_mul(2))
                .min(MAX_PATCH_BYTES);
            self.raw.reserve_exact(target - self.raw.len());
        }
        self.raw.push_str(line);
        self.raw.push('\n');
        self.lines += 1;
        true
    }

    fn complete(&self) -> bool {
        self.hunks > 0 && self.remaining == Some((0, 0))
    }

    fn start_hunk(&mut self, line: &str) -> bool {
        if self.hunks == MAX_HUNKS {
            return false;
        }
        let Some(((old_start, old_count), (new_start, new_count))) = hunk_ranges(line) else {
            return false;
        };
        if self
            .range_ends
            .is_some_and(|(old_end, new_end)| old_start < old_end || new_start < new_end)
        {
            return false;
        }
        if !self.append(line) {
            return false;
        }
        self.range_ends = Some((old_start + old_count, new_start + new_count));
        self.remaining = Some((old_count, new_count));
        self.hunks += 1;
        self.last_was_data = false;
        true
    }

    fn data_line(&mut self, line: &str) -> bool {
        let Some((old, new)) = self.remaining else {
            return false;
        };
        let remaining = match line.as_bytes().first() {
            Some(b'-') if old > 0 => (old - 1, new),
            Some(b'+') if new > 0 => (old, new - 1),
            Some(b' ') if old > 0 && new > 0 => (old - 1, new - 1),
            _ => return false,
        };
        if !self.append(line) {
            return false;
        }
        self.remaining = Some(remaining);
        self.last_was_data = true;
        true
    }
}

#[derive(Debug, Default)]
enum ScanState {
    #[default]
    Idle,
    SeenMinus(String),
    InDiff(Capture),
}

#[derive(Debug)]
pub struct DiffDetector {
    state: ScanState,
    framer: Framer,
    next_id: u64,
    detected: VecDeque<DetectedDiff>,
    history_bytes: usize,
    notices: u8,
}

impl DiffDetector {
    pub fn new() -> Self {
        let detected = VecDeque::with_capacity(MAX_HISTORY_COUNT);
        // Charge every allocated slot, including spare capacity. owned_bytes()
        // also charges live inline values, a small conservative overcount.
        let history_bytes = detected.capacity() * std::mem::size_of::<DetectedDiff>();
        Self {
            state: ScanState::Idle,
            framer: Framer::default(),
            next_id: 1,
            detected,
            history_bytes,
            notices: 0,
        }
    }

    /// Completion callbacks run immediately, before eviction can hide an
    /// earlier completion in the same chunk. They must be bounded/nonblocking.
    pub fn feed_bytes(&mut self, data: &[u8], mut completed: impl FnMut(&DetectedDiff)) {
        let mut framer = std::mem::take(&mut self.framer);
        framer.feed(data, |line| self.accept_line(line, &mut completed));
        self.framer = framer;
    }

    /// Real output EOF only, never the end of one poll or an idle timeout.
    /// Repeated EOF is harmless; incomplete captures are rejected.
    pub fn finish_stream(&mut self, mut completed: impl FnMut(&DetectedDiff)) {
        let mut framer = std::mem::take(&mut self.framer);
        framer.finish(|line| self.accept_line(line, &mut completed));
        self.framer = framer;
        self.settle_capture(&mut completed);
    }

    /// The program whose output was being captured is gone, though the
    /// stream goes on: the terminal's agent changed. A complete patch is kept
    /// and an unfinished one rejected, as at EOF, but an unterminated line is
    /// dropped rather than read, since it may as well be the start of
    /// whatever runs next, such as a shell prompt. Nothing captured from one
    /// program is then resumed against another's output.
    pub fn end_capture(&mut self, mut completed: impl FnMut(&DetectedDiff)) {
        self.framer = Framer::default();
        self.settle_capture(&mut completed);
    }

    fn settle_capture(&mut self, completed: &mut impl FnMut(&DetectedDiff)) {
        match std::mem::take(&mut self.state) {
            ScanState::InDiff(capture) if capture.complete() => self.finalize(capture, completed),
            ScanState::Idle => {}
            _ => self.reject(),
        }
    }

    #[cfg(test)]
    pub fn feed_line(&mut self, line: &str) {
        self.accept_line(Ok(line), &mut |_| {});
    }

    #[cfg(test)]
    pub fn flush(&mut self) {
        self.finish_stream(|_| {});
    }

    fn reject(&mut self) {
        self.state = ScanState::Idle;
        self.notices |= REJECTED;
    }

    fn accept_line(&mut self, line: Result<&str, ()>, completed: &mut impl FnMut(&DetectedDiff)) {
        let Ok(line) = line else {
            self.reject();
            return;
        };
        if line.len() > MAX_LINE_BYTES {
            self.reject();
            return;
        }
        match std::mem::take(&mut self.state) {
            ScanState::Idle => self.start_file(line),
            ScanState::SeenMinus(minus) => {
                if file_header(line, "+++ ").is_some() {
                    let mut capture = Capture::default();
                    if capture.append(&minus) && capture.append(line) {
                        self.state = ScanState::InDiff(capture);
                    } else {
                        self.reject();
                    }
                } else {
                    self.reject();
                    self.start_file(line);
                }
            }
            ScanState::InDiff(mut capture) => {
                if line == "\\ No newline at end of file" && capture.last_was_data {
                    if capture.append(line) {
                        capture.last_was_data = false;
                        self.state = ScanState::InDiff(capture);
                    } else {
                        self.reject();
                    }
                } else if capture
                    .remaining
                    .is_some_and(|remaining| remaining != (0, 0))
                {
                    // Counts disambiguate removed "--- " text from file headers.
                    if capture.data_line(line) {
                        self.state = ScanState::InDiff(capture);
                    } else {
                        self.reject();
                    }
                } else if line.starts_with("@@") {
                    if capture.start_hunk(line) {
                        self.state = ScanState::InDiff(capture);
                    } else {
                        self.reject();
                    }
                } else if file_header(line, "--- ").is_some() {
                    if capture.complete() {
                        self.finalize(capture, completed);
                    } else {
                        self.reject();
                    }
                    self.start_file(line);
                } else if capture.complete() && !line.starts_with(['+', '-', ' ', '\\']) {
                    self.finalize(capture, completed);
                } else {
                    // Never silently truncate extra patch-looking data.
                    self.reject();
                }
            }
        }
    }

    fn start_file(&mut self, line: &str) {
        if file_header(line, "--- ").is_some() {
            self.state = ScanState::SeenMinus(line.to_owned());
        }
    }

    fn finalize(&mut self, capture: Capture, completed: &mut impl FnMut(&DetectedDiff)) {
        let id = self.next_id;
        let mut lines = capture.raw.lines();
        let minus = file_header(lines.next().unwrap(), "--- ").unwrap();
        let plus = file_header(lines.next().unwrap(), "+++ ").unwrap();
        let path = if plus == "/dev/null" { minus } else { plus };
        let filename = path
            .strip_prefix("b/")
            .or_else(|| path.strip_prefix("a/"))
            .unwrap_or(path)
            .to_owned();
        let mut hunks: Vec<DiffHunk> = Vec::with_capacity(capture.hunks);
        for line in lines {
            if line.starts_with("@@ ") {
                hunks.push(DiffHunk {
                    header: line.to_owned(),
                    lines: Vec::new(),
                });
            } else {
                let parsed = match line.as_bytes().first() {
                    Some(b' ') => DiffLine::Context(line[1..].to_owned()),
                    Some(b'+') => DiffLine::Added(line[1..].to_owned()),
                    Some(b'-') => DiffLine::Removed(line[1..].to_owned()),
                    _ => continue, // Validated "No newline" marker, retained raw.
                };
                hunks.last_mut().unwrap().lines.push(parsed);
            }
        }
        let diff = DetectedDiff {
            id,
            filename,
            hunks,
            raw: capture.raw,
            reviewed: false,
        };
        let bytes = diff.owned_bytes();
        if bytes > MAX_OWNED_PATCH_BYTES {
            self.reject();
            return;
        }
        self.next_id += 1;
        while self.detected.len() == MAX_HISTORY_COUNT
            || self.history_bytes + bytes > MAX_HISTORY_BYTES
        {
            let removed = self.detected.pop_front().unwrap();
            self.history_bytes -= removed.owned_bytes();
            self.notices |= EVICTED;
        }
        completed(&diff);
        self.history_bytes += bytes;
        self.detected.push_back(diff);
    }

    /// At most two coalesced messages, regardless of hostile stream size.
    pub fn take_notices(&mut self) -> impl Iterator<Item = &'static str> + use<> {
        let flags = std::mem::take(&mut self.notices);
        [
            (REJECTED, "Diff capture skipped an incomplete, malformed, edited, or oversized patch/line; terminal output is unchanged"),
            (EVICTED, "Old diff reviews were evicted to keep terminal memory bounded"),
        ].into_iter().filter_map(move |(flag, message)| (flags & flag != 0).then_some(message))
    }

    /// Forget every diff and any capture in progress, as when nothing is
    /// watched any more. Ids keep counting, so a review opened earlier can
    /// never name a later diff. Returns whether an unreviewed diff was
    /// forgotten.
    pub fn clear(&mut self) -> bool {
        let unreviewed = self.unreviewed_count() > 0;
        self.state = ScanState::Idle;
        self.framer = Framer::default();
        self.detected.clear();
        self.history_bytes = self.detected.capacity() * std::mem::size_of::<DetectedDiff>();
        self.notices = 0;
        unreviewed
    }

    pub fn unreviewed_count(&self) -> usize {
        self.detected.iter().filter(|d| !d.reviewed).count()
    }

    #[cfg(test)]
    pub(crate) fn history_diagnostics(&self) -> (usize, usize) {
        // Independently count owned capacities so an accounting regression
        // cannot make the soak pass merely by under-reporting history_bytes.
        let owned = self.detected.capacity() * std::mem::size_of::<DetectedDiff>()
            + self
                .detected
                .iter()
                .map(DetectedDiff::owned_bytes)
                .sum::<usize>();
        assert_eq!(owned, self.history_bytes);
        (self.detected.len(), owned)
    }
    pub fn mark_reviewed(&mut self, id: u64) -> bool {
        let Some(diff) = self.detected.iter_mut().find(|diff| diff.id == id) else {
            return false;
        };
        diff.reviewed = true;
        true
    }
    pub fn latest_unreviewed(&self) -> Option<&DetectedDiff> {
        self.detected.iter().rev().find(|d| !d.reviewed)
    }
    pub fn detected_by_id(&self, id: u64) -> Option<&DetectedDiff> {
        self.detected.iter().find(|diff| diff.id == id)
    }
}

fn file_header<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let path = line.strip_prefix(prefix)?.split('\t').next()?;
    (!path.is_empty()).then_some(path)
}

type Range = (usize, usize);

fn hunk_ranges(line: &str) -> Option<(Range, Range)> {
    let rest = line.strip_prefix("@@ ")?;
    let (ranges, suffix) = rest.split_once(" @@")?;
    if !suffix.is_empty() && !suffix.starts_with(' ') {
        return None;
    }
    let (old, new) = ranges.split_once(' ')?;
    let parse = |text: &str, prefix| -> Option<Range> {
        let text = text.strip_prefix(prefix)?;
        let (start, count) = text.split_once(',').unwrap_or((text, "1"));
        if start.is_empty()
            || count.is_empty()
            || !start.bytes().all(|b| b.is_ascii_digit())
            || !count.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let start: usize = start.parse().ok()?;
        let count: usize = count.parse().ok()?;
        // Empty ranges name the line BEFORE the insertion/deletion point.
        // Normalize to a half-open position before comparing adjacent hunks.
        let start = if count == 0 {
            start.checked_add(1)?
        } else {
            start
        };
        start.checked_add(count)?;
        (count <= MAX_PATCH_LINES && (count == 0 || start > 0)).then_some((start, count))
    };
    let old = parse(old, '-')?;
    let new = parse(new, '+')?;
    (old.1 != 0 || new.1 != 0).then_some((old, new))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn truncated_hunk_is_not_a_complete_patch() {
        let mut detector = DiffDetector::new();
        for line in [
            "--- a/file",
            "+++ b/file",
            "@@ -1,2 +1,2 @@",
            "-old",
            "+new",
        ] {
            detector.feed_line(line);
        }
        detector.flush();
        assert_eq!(detector.unreviewed_count(), 0);
    }

    #[test]
    fn test_simple_diff() {
        let mut detector = DiffDetector::new();
        let lines = [
            "--- a/src/main.rs",
            "+++ b/src/main.rs",
            "@@ -1,3 +1,4 @@",
            " fn main() {",
            "-    println!(\"hello\");",
            "+    println!(\"hello, world!\");",
            "+    println!(\"goodbye\");",
            " }",
        ];
        for line in &lines {
            detector.feed_line(line);
        }
        detector.flush();

        assert_eq!(detector.detected.len(), 1);
        let diff = &detector.detected[0];
        assert_eq!(diff.filename, "src/main.rs");
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(diff.hunks[0].lines.len(), 5);
        assert_eq!(
            diff.hunks[0].lines[0],
            DiffLine::Context("fn main() {".to_string())
        );
        assert_eq!(
            diff.hunks[0].lines[1],
            DiffLine::Removed("    println!(\"hello\");".to_string())
        );
        assert_eq!(
            diff.hunks[0].lines[2],
            DiffLine::Added("    println!(\"hello, world!\");".to_string())
        );
        assert_eq!(
            diff.hunks[0].lines[3],
            DiffLine::Added("    println!(\"goodbye\");".to_string())
        );
        assert_eq!(diff.hunks[0].lines[4], DiffLine::Context("}".to_string()));
    }

    #[test]
    fn test_multiple_hunks() {
        let mut detector = DiffDetector::new();
        let lines = [
            "--- a/file.txt",
            "+++ b/file.txt",
            "@@ -1,1 +1,1 @@",
            "-old line",
            "+new line",
            "@@ -10,1 +10,1 @@",
            "-old line 10",
            "+new line 10",
        ];
        for line in &lines {
            detector.feed_line(line);
        }
        detector.flush();

        assert_eq!(detector.detected.len(), 1);
        assert_eq!(detector.detected[0].hunks.len(), 2);
    }

    #[test]
    fn test_multiple_files() {
        let mut detector = DiffDetector::new();
        let lines = [
            "--- a/file1.rs",
            "+++ b/file1.rs",
            "@@ -1,1 +1,1 @@",
            "-old",
            "+new",
            "--- a/file2.rs",
            "+++ b/file2.rs",
            "@@ -1,1 +1,1 @@",
            "-old2",
            "+new2",
        ];
        for line in &lines {
            detector.feed_line(line);
        }
        detector.flush();

        assert_eq!(detector.detected.len(), 2);
        assert_eq!(detector.detected[0].filename, "file1.rs");
        assert_eq!(detector.detected[1].filename, "file2.rs");
    }

    #[test]
    fn test_no_diff() {
        let mut detector = DiffDetector::new();
        detector.feed_line("just a normal line");
        detector.feed_line("nothing to see here");
        detector.flush();

        assert_eq!(detector.detected.len(), 0);
    }

    #[test]
    fn test_incomplete_diff() {
        let mut detector = DiffDetector::new();
        detector.feed_line("--- a/file.rs");
        detector.feed_line("some other line"); // Not a +++ line
        detector.flush();

        assert_eq!(detector.detected.len(), 0);
    }

    #[test]
    fn test_unreviewed_count() {
        let mut detector = DiffDetector::new();
        let lines = [
            "--- a/file.rs",
            "+++ b/file.rs",
            "@@ -1,1 +1,1 @@",
            "-old",
            "+new",
        ];
        for line in &lines {
            detector.feed_line(line);
        }
        detector.flush();

        assert_eq!(detector.unreviewed_count(), 1);
        let id = detector.detected[0].id;
        assert!(detector.mark_reviewed(id));
        assert_eq!(detector.unreviewed_count(), 0);
    }

    #[test]
    fn stable_ids_mark_only_the_exact_reviewed_diff() {
        let mut detector = DiffDetector::new();
        for filename in ["first.rs", "second.rs"] {
            for line in [
                format!("--- a/{filename}"),
                format!("+++ b/{filename}"),
                "@@ -1,1 +1,1 @@".into(),
                "-old".into(),
                "+new".into(),
                "finished".into(),
            ] {
                detector.feed_line(&line);
            }
        }
        assert_eq!(detector.detected.len(), 2);
        let first_id = detector.detected[0].id;
        let second_id = detector.detected[1].id;
        assert_ne!(first_id, second_id);
        assert!(
            detector
                .detected_by_id(second_id)
                .unwrap()
                .raw
                .contains("+++ b/second.rs")
        );
        assert!(detector.mark_reviewed(first_id));
        assert!(detector.detected[0].reviewed);
        assert!(!detector.detected[1].reviewed);
        assert!(!detector.mark_reviewed(u64::MAX));
    }

    #[test]
    fn test_git_diff_with_noise() {
        let mut detector = DiffDetector::new();
        let lines = [
            "diff --git a/src/main.rs b/src/main.rs",
            "index abc123..def456 100644",
            "--- a/src/main.rs",
            "+++ b/src/main.rs",
            "@@ -5,3 +5,4 @@",
            " use std::io;",
            "+use std::fs;",
            " ",
            " fn main() {",
            "some random output after the diff",
        ];
        for line in &lines {
            detector.feed_line(line);
        }
        detector.flush();

        assert_eq!(detector.detected.len(), 1);
        assert_eq!(detector.detected[0].filename, "src/main.rs");
    }

    #[test]
    fn test_raw_text_preserved() {
        let mut detector = DiffDetector::new();
        let lines = [
            "--- a/file.rs",
            "+++ b/file.rs",
            "@@ -1,1 +1,1 @@",
            "-old",
            "+new",
        ];
        for line in &lines {
            detector.feed_line(line);
        }
        detector.flush();

        let raw = &detector.detected[0].raw;
        assert!(raw.contains("--- a/file.rs"));
        assert!(raw.contains("+new"));
    }

    /// Feed one persistent history past both its count and owned-capacity ceilings.
    /// All bytes are synthetic; no provider, process, or working tree is involved.
    /// The resource soak repeats this with one detector across its iterations.
    pub(crate) fn feed_past_history_limits(detector: &mut DiffDetector, iteration: usize) {
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
    }

    #[test]
    fn a_persistent_history_evicts_by_count_and_by_bytes() {
        let mut detector = DiffDetector::new();
        for iteration in 0..3 {
            feed_past_history_limits(&mut detector, iteration);
        }
    }
}
