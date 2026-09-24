//! Integration state owned by one terminal, transferred with its PTY.
use super::transcript::{TranscriptNotice, TranscriptRecorder, TranscriptWrite};
use crate::agents::{AgentSession, AgentStatus};
use crate::terminal::diff_detector::DiffDetector;

pub(crate) struct TerminalSession {
    /// The coding agent in this terminal's foreground, if any.
    pub agent: Option<AgentSession>,
    /// What that agent last reported through its hooks.
    pub agent_status: Option<AgentStatus>,
    /// Where the latest agent started, for reviewing what it changed.
    pub agent_changes: crate::agents::changes::ChangeTracking,
    /// When the person last typed or pasted into this terminal.
    pub last_person_input: Option<std::time::Instant>,
    pub transcript: Option<TranscriptRecorder>,
    pub diff_detector: DiffDetector,
    failure_reported: bool,
    notices: Vec<String>,
}

impl Default for TerminalSession {
    fn default() -> Self {
        Self {
            agent: None,
            agent_status: None,
            agent_changes: Default::default(),
            last_person_input: None,
            transcript: None,
            diff_detector: DiffDetector::new(),
            failure_reported: false,
            notices: Vec::new(),
        }
    }
}

impl TerminalSession {
    fn notice(&mut self, text: String) {
        if self.notices.len() < 16 {
            self.notices.push(text);
        }
    }

    fn handle_write(&mut self, result: std::io::Result<TranscriptWrite>) {
        match result {
            Ok(TranscriptWrite::Backpressure) => self.notice("Transcript writer is behind; recording dropped data instead of blocking the terminal".into()),
            Err(error) if !self.failure_reported => {
                self.failure_reported = true;
                self.notice(format!("Transcript recording failed: {error}"));
            }
            _ => {}
        }
    }

    pub fn observe_output(&mut self, data: &[u8], plugin: bool) {
        if let Some(recorder) = &mut self.transcript {
            let result = recorder.record_output(data);
            self.handle_write(result);
        }
        if plugin {
            self.capture_diffs(Some(data));
        }
    }

    /// Called only once the owning PTY's output stream ends, not per poll.
    pub fn finish_stream(&mut self) {
        self.finish_recorded_text();
        self.capture_diffs(None);
    }

    fn finish_recorded_text(&mut self) {
        if let Some(recorder) = &mut self.transcript {
            let result = recorder.finish_text_streams();
            self.handle_write(result);
            self.poll_notices();
        }
    }

    fn capture_diffs(&mut self, data: Option<&[u8]>) {
        let recorder = &mut self.transcript;
        let mut write_problem = None;
        let completed = |diff: &crate::terminal::diff_detector::DetectedDiff| {
            if let Some(recorder) = recorder {
                // One enqueue attempt per completion, including adjacent files
                // and files evicted later in this same PTY chunk. No queue of
                // cloned patches and no inference from unreviewed counts.
                let result = recorder.record_diff(&diff.filename, &diff.raw);
                if !matches!(result, Ok(TranscriptWrite::Written))
                    && (write_problem.is_none() || result.is_err())
                {
                    write_problem = Some(result);
                }
            }
        };
        match data {
            Some(data) => self.diff_detector.feed_bytes(data, completed),
            None => self.diff_detector.finish_stream(completed),
        }
        if let Some(result) = write_problem {
            self.handle_write(result);
        }
        for notice in self.diff_detector.take_notices() {
            self.notice(notice.into());
        }
    }

    pub fn record_input(&mut self, data: &[u8]) {
        if let Some(recorder) = &mut self.transcript {
            let result = recorder.record_input(data);
            self.handle_write(result);
        }
    }

    pub fn poll_notices(&mut self) -> bool {
        let Some(recorder) = &mut self.transcript else {
            return false;
        };
        let notices = recorder.poll_notices();
        for notice in &notices {
            match notice {
                TranscriptNotice::CapReached { max_bytes } => self.notice(format!("Transcript reached its {} MiB cap; recording stopped before data was truncated", max_bytes / (1024 * 1024))),
                TranscriptNotice::Failed(error) if !self.failure_reported => {
                    self.failure_reported = true;
                    self.notice(format!("Transcript recording failed; output may be incomplete: {error}"));
                }
                TranscriptNotice::Failed(_) => {}
                TranscriptNotice::TextReplaced { input } => self.notice(format!(
                    "Transcript {} contained invalid or unfinished UTF-8; using replacement characters (�)",
                    if *input { "input" } else { "output" })),
            }
        }
        !notices.is_empty()
    }

    pub fn start_transcript(&mut self, recorder: TranscriptRecorder) {
        // Flush the previous epoch into its own recorder, never its replacement.
        drop(self.stop_transcript());
        self.transcript = Some(recorder);
        self.failure_reported = false;
    }

    pub fn stop_transcript(&mut self) -> Option<TranscriptRecorder> {
        self.finish_recorded_text();
        self.transcript.take()
    }

    pub fn drain_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorded_text(path: &std::path::Path, input: bool) -> String {
        use super::super::transcript::{EntryKind, TranscriptEntry};
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<TranscriptEntry>(line).unwrap())
            .filter(|entry| {
                entry.kind
                    == if input {
                        EntryKind::Input
                    } else {
                        EntryKind::Output
                    }
            })
            .map(|entry| entry.content)
            .collect()
    }

    #[test]
    fn terminal_eof_finalizes_pending_text_once_without_stopping_a_healthy_recorder() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = TerminalSession::default();
        session.start_transcript(
            TranscriptRecorder::new(directory.path(), None, None, "Local", "fixture").unwrap(),
        );
        session.observe_output(b"output \xf0\x9f", true);
        session.record_input(b"input \xe2");
        session.finish_stream();
        session.finish_stream();
        assert!(session.transcript.is_some());
        assert_eq!(session.drain_notices().len(), 3); // Two text directions plus strict diff-observer rejection.
        let recorder = session.transcript.as_ref().unwrap();
        let checkpoint = recorder.checkpoint().unwrap();
        checkpoint
            .reply
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(recorded_text(recorder.path(), false), "output �");
        assert_eq!(recorded_text(recorder.path(), true), "input �");
        let mut recorder = session.stop_transcript().unwrap();
        recorder.finish(std::time::Duration::from_secs(2)).unwrap();
        assert_eq!(recorded_text(recorder.path(), false), "output �");
        assert_eq!(recorded_text(recorder.path(), true), "input �");
    }

    #[test]
    fn recording_epochs_never_share_pending_input_or_output() {
        let directory = tempfile::tempdir().unwrap();
        for replace in [false, true] {
            let mut session = TerminalSession::default();
            let first =
                TranscriptRecorder::new(directory.path(), None, None, "Local", "first").unwrap();
            let first_path = first.path().to_owned();
            session.start_transcript(first);
            session.observe_output(b"first \xf0\x9f", false);
            session.record_input(b"input \xe2");
            if !replace {
                let mut first = session.stop_transcript().unwrap();
                first.finish(std::time::Duration::from_secs(2)).unwrap();
                // Unrecorded traffic cannot seed the next recording's carry.
                session.observe_output(b"\xf0\x9f", false);
                session.record_input(b"\xe2");
            }
            let second =
                TranscriptRecorder::new(directory.path(), None, None, "Local", "second").unwrap();
            session.start_transcript(second);
            assert_eq!(recorded_text(&first_path, false), "first �");
            assert_eq!(recorded_text(&first_path, true), "input �");
            session.observe_output(b"\x8c\xb1 second", false);
            session.record_input(b"\x82\xac second");
            session.poll_notices();
            assert!(
                session.transcript.is_some(),
                "replacement is a warning, not a stopped recorder"
            );
            session.finish_stream();
            session.finish_stream();
            let mut second = session.stop_transcript().unwrap();
            second.finish(std::time::Duration::from_secs(2)).unwrap();
            assert_eq!(recorded_text(second.path(), false), "�� second");
            assert_eq!(recorded_text(second.path(), true), "�� second");
        }
    }

    #[test]
    fn input_and_output_decoders_are_separate_and_replacement_notices_stay_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = TerminalSession::default();
        session.start_transcript(
            TranscriptRecorder::new(directory.path(), None, None, "Local", "fixture").unwrap(),
        );
        session.record_input(b"\xf0\x9f");
        session.observe_output(b"\x8c\xb1", false);
        session.record_input(b"\x8c\xb1");
        for _ in 0..100 {
            session.observe_output(b"\xff", false);
            session.record_input(b"\xff");
            session.poll_notices();
        }
        assert!(session.transcript.is_some());
        assert_eq!(session.drain_notices().len(), 2);
        let mut recorder = session.stop_transcript().unwrap();
        recorder.finish(std::time::Duration::from_secs(2)).unwrap();
        assert_eq!(
            recorded_text(recorder.path(), true),
            format!("🌱{}", "�".repeat(100))
        );
        assert_eq!(recorded_text(recorder.path(), false), "�".repeat(102));
    }

    #[test]
    fn diff_entries_use_completion_epoch_without_replaying_pre_recording_raw_output() {
        use super::super::transcript::{EntryKind, TranscriptEntry};
        let directory = tempfile::tempdir().unwrap();
        let mut session = TerminalSession::default();
        let patch = b"--- a/file\n+++ b/file\n@@ -1 +1 @@\n-old\n+new\n";
        session.observe_output(patch, true);
        session.start_transcript(
            TranscriptRecorder::new(directory.path(), None, None, "Local", "fixture").unwrap(),
        );
        session.observe_output(b"finished\n", true);
        let mut recorder = session.stop_transcript().unwrap();
        recorder.finish(std::time::Duration::from_secs(2)).unwrap();
        assert_eq!(recorded_text(recorder.path(), false), "finished\n");
        let diffs: Vec<_> = std::fs::read_to_string(recorder.path())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<TranscriptEntry>(line).unwrap())
            .filter(|entry| entry.kind == EntryKind::Diff)
            .collect();
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].content.as_bytes(), patch);
    }

    #[test]
    fn raw_transcript_preserves_split_utf8_in_both_directions() {
        use super::super::transcript::{EntryKind, TranscriptEntry};
        let directory = tempfile::tempdir().unwrap();
        let mut session = TerminalSession::default();
        session.start_transcript(
            TranscriptRecorder::new(directory.path(), None, None, "Local", "fixture").unwrap(),
        );
        for part in [b"before \xf0\x9f".as_slice(), b"\x8c\xb1 after"] {
            session.observe_output(part, false);
            session.record_input(part);
        }
        let mut recorder = session.stop_transcript().unwrap();
        recorder.finish(std::time::Duration::from_secs(2)).unwrap();
        let entries: Vec<TranscriptEntry> = std::fs::read_to_string(recorder.path())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        for input in [false, true] {
            let text: String = entries
                .iter()
                .filter(|entry| {
                    if input {
                        matches!(entry.kind, EntryKind::Input)
                    } else {
                        matches!(entry.kind, EntryKind::Output)
                    }
                })
                .map(|entry| entry.content.as_str())
                .collect();
            assert_eq!(text, "before 🌱 after");
        }
    }

    #[test]
    fn completed_diffs_are_recorded_once_at_boundaries_and_eof_even_after_eviction() {
        use super::super::transcript::{EntryKind, TranscriptEntry};
        let directory = tempfile::tempdir().unwrap();
        let mut session = TerminalSession::default();
        session.start_transcript(
            TranscriptRecorder::new(directory.path(), None, None, "Local", "fixture").unwrap(),
        );
        let patches: Vec<_> = (0..80)
            .map(|id| format!("--- a/file-{id}\n+++ b/file-{id}\n@@ -1 +1 @@\n-old\n+new\n"))
            .collect();
        let stream = patches.join("");
        session.observe_output(stream.as_bytes(), true);
        // 79 adjacent-file completions, with the final file still pending.
        assert_eq!(session.diff_detector.latest_unreviewed().unwrap().id, 79);
        session.observe_output(b"finished\n", true);
        let unicode = "--- a/根\n+++ b/根\n@@ -1 +1 @@\n-old\n+🌱";
        for byte in unicode.as_bytes().chunks(1) {
            session.observe_output(byte, true);
        }
        session.finish_stream();
        session.finish_stream();
        let mut recorder = session.stop_transcript().unwrap();
        recorder.finish(std::time::Duration::from_secs(2)).unwrap();
        let entries: Vec<TranscriptEntry> = std::fs::read_to_string(recorder.path())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let diffs: Vec<_> = entries
            .iter()
            .filter(|entry| matches!(entry.kind, EntryKind::Diff))
            .map(|entry| entry.content.as_str())
            .collect();
        let mut expected: Vec<_> = patches.iter().map(String::as_str).collect();
        let unicode = format!("{unicode}\n");
        expected.push(&unicode);
        assert_eq!(diffs, expected);
        assert!(session.diff_detector.detected_by_id(1).is_none());
        assert_eq!(session.diff_detector.latest_unreviewed().unwrap().id, 81);
        let notices = session.drain_notices();
        assert_eq!(notices.len(), 3); // One coalesced eviction notice per completion batch.
        assert!(notices.iter().all(|notice| notice.contains("evicted")));
    }

    #[test]
    fn rejected_capture_notices_are_bounded_and_shell_output_is_not_captured() {
        let patch = b"--- a/file\n+++ b/file\n@@ -1,2 +1,2 @@\n-old\n+new\nfinished\n";
        let mut plugin = TerminalSession::default();
        let mut shell = TerminalSession::default();
        for _ in 0..100 {
            plugin.observe_output(patch, true);
            shell.observe_output(patch, false);
        }
        assert_eq!(plugin.diff_detector.unreviewed_count(), 0);
        assert_eq!(plugin.drain_notices().len(), 16);
        assert_eq!(shell.diff_detector.unreviewed_count(), 0);
        assert!(shell.drain_notices().is_empty());
    }

    #[test]
    fn recorder_flags_and_bounded_failure_notices_belong_to_one_owner() {
        let directory = tempfile::tempdir().unwrap();
        let mut first = TerminalSession::default();
        let second = TerminalSession::default();
        first.start_transcript(
            TranscriptRecorder::new(directory.path(), None, None, "Local", "fixture").unwrap(),
        );
        assert!(first.transcript.is_some());
        assert!(!second.transcript.is_some());
        first.handle_write(Err(std::io::Error::other("fixture failure")));
        first.handle_write(Err(std::io::Error::other("duplicate failure")));
        assert_eq!(first.notices.len(), 1);
        for _ in 0..100 {
            first.handle_write(Ok(TranscriptWrite::Backpressure));
        }
        assert_eq!(first.notices.len(), 16);
        assert!(second.notices.is_empty());
        let mut recorder = first.stop_transcript().unwrap();
        assert!(!first.transcript.is_some());
        recorder.finish(std::time::Duration::from_secs(2)).unwrap();
        assert_eq!(first.drain_notices().len(), 16);
        assert!(first.drain_notices().is_empty());
        first.start_transcript(
            TranscriptRecorder::new(directory.path(), None, None, "Local", "fresh").unwrap(),
        );
        assert!(!first.failure_reported);
        first.handle_write(Err(std::io::Error::other("fresh failure")));
        assert_eq!(first.notices.len(), 1);
    }
}
