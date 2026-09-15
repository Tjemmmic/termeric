//! Integration state owned by one terminal, transferred with its PTY.
use super::transcript::{TranscriptNotice, TranscriptRecorder, TranscriptWrite};
use crate::terminal::diff_detector::DiffDetector;
use crate::ui::claude_chrome::AiSessionState;

pub(crate) struct TerminalSession {
    pub ai_session: Option<AiSessionState>,
    pub transcript: Option<TranscriptRecorder>,
    pub diff_detector: DiffDetector,
    failure_reported: bool,
    notices: Vec<String>,
}

impl Default for TerminalSession {
    fn default() -> Self {
        Self::new(false)
    }
}

impl TerminalSession {
    pub fn new(plugin: bool) -> Self {
        Self {
            ai_session: plugin.then(AiSessionState::default),
            transcript: None,
            diff_detector: DiffDetector::new(),
            failure_reported: false,
            notices: Vec::new(),
        }
    }

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
        if let Some(recorder) = &mut self.transcript
            && let Ok(text) = std::str::from_utf8(data)
        {
            let result = recorder.record_output(text);
            self.handle_write(result);
        }
        if plugin {
            self.capture_diffs(Some(data));
        }
    }

    /// Called only once the owning PTY's output stream ends, not per poll.
    pub fn finish_stream(&mut self) {
        self.capture_diffs(None);
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
        if let Some(recorder) = &mut self.transcript
            && let Ok(text) = std::str::from_utf8(data)
        {
            let result = recorder.record_input(text);
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
            }
        }
        if !notices.is_empty()
            && let Some(session) = &mut self.ai_session
        {
            session.recording = false;
        }
        !notices.is_empty()
    }

    pub fn start_transcript(&mut self, recorder: TranscriptRecorder) {
        self.transcript = Some(recorder);
        self.failure_reported = false;
        if let Some(session) = &mut self.ai_session {
            session.recording = true;
        }
    }

    pub fn stop_transcript(&mut self) -> Option<TranscriptRecorder> {
        if let Some(session) = &mut self.ai_session {
            session.recording = false;
        }
        self.transcript.take()
    }

    pub fn drain_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_diffs_are_recorded_once_at_boundaries_and_eof_even_after_eviction() {
        use super::super::transcript::{EntryKind, TranscriptEntry};
        let directory = tempfile::tempdir().unwrap();
        let mut session = TerminalSession::new(true);
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
        let mut plugin = TerminalSession::new(true);
        let mut shell = TerminalSession::new(false);
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
        let mut first = TerminalSession::new(true);
        let second = TerminalSession::new(true);
        first.start_transcript(
            TranscriptRecorder::new(directory.path(), None, None, "Local", "fixture").unwrap(),
        );
        assert!(first.ai_session.as_ref().unwrap().recording);
        assert!(!second.ai_session.as_ref().unwrap().recording);
        first.handle_write(Err(std::io::Error::other("fixture failure")));
        first.handle_write(Err(std::io::Error::other("duplicate failure")));
        assert_eq!(first.notices.len(), 1);
        for _ in 0..100 {
            first.handle_write(Ok(TranscriptWrite::Backpressure));
        }
        assert_eq!(first.notices.len(), 16);
        assert!(second.notices.is_empty());
        let mut recorder = first.stop_transcript().unwrap();
        assert!(!first.ai_session.as_ref().unwrap().recording);
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
