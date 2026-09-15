//! Integration state owned by one terminal, transferred with its PTY.
use super::transcript::{TranscriptNotice, TranscriptRecorder, TranscriptWrite};
use crate::terminal::diff_detector::{DiffDetector, strip_ansi};
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
        let Ok(text) = std::str::from_utf8(data) else {
            return;
        };
        if plugin {
            for line in text.lines() {
                self.diff_detector.feed_line(&strip_ansi(line));
            }
        }
        if let Some(recorder) = &mut self.transcript {
            let result = recorder.record_output(text);
            self.handle_write(result);
        }
    }

    pub fn finish_output(&mut self) {
        let before = self.diff_detector.unreviewed_count();
        self.diff_detector.flush();
        if self.diff_detector.unreviewed_count() > before
            && let Some(recorder) = &mut self.transcript
            && let Some(diff) = self.diff_detector.latest_unreviewed()
        {
            let result = recorder.record_diff(&diff.filename, &diff.raw);
            self.handle_write(result);
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
