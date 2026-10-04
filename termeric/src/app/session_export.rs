//! Window-owned export request/completion routing, independent of later focus.

use super::App;
use crate::session::export_job::{ExportJob, ExportSource, MAX_SCROLLBACK_BYTES};

impl App {
    /// Export one terminal: its transcript when recording, else its
    /// scrollback as Markdown.
    pub(super) fn export_terminal_session(&mut self, identity: crate::tabs::TerminalIdentity) {
        if self.session_export.is_some() {
            self.toasts
                .warning("A session export is already in progress in this window");
            return;
        }
        let Some(tab) = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.tab_by_id(identity.tab_id))
        else {
            self.toasts.warning("The terminal is no longer open");
            return;
        };
        let source = if let Some(recorder) = tab
            .pane_session(identity.pane_id)
            .and_then(|state| state.transcript.as_ref())
        {
            match recorder.flush() {
                Ok(pending) => ExportSource::Transcript(pending),
                Err(error) => {
                    self.toasts.error(format!("Session export failed: {error}"));
                    return;
                }
            }
        } else {
            let Some(terminal) = tab.pane_terminal(identity.pane_id) else {
                self.toasts
                    .warning("No terminal content is available for export");
                return;
            };
            match capture_scrollback(terminal.grid()) {
                Ok(text) if text.trim().is_empty() => {
                    self.toasts.info("No session content to export");
                    return;
                }
                Ok(text) => ExportSource::Scrollback(text),
                Err(error) => {
                    self.toasts.error(error);
                    return;
                }
            }
        };
        let Some(base) = crate::session::sessions_base_dir() else {
            self.toasts.error("Session data directory is unavailable");
            return;
        };
        match ExportJob::start(source, base, super::transcript_event_waker()) {
            Ok(job) => {
                self.session_export = Some(job);
                self.toasts.info("Exporting session…");
            }
            Err(error) => self.toasts.error(format!("Session export failed: {error}")),
        }
    }

    pub(super) fn poll_session_export(&mut self) {
        let Some(result) = self.session_export.as_ref().and_then(ExportJob::poll) else {
            return;
        };
        self.session_export = None;
        match result {
            Ok(export) => {
                let path = export.markdown.display().to_string();
                let copied = self
                    .clipboard
                    .as_mut()
                    .is_some_and(|clipboard| clipboard.set_text(path.clone()).is_ok());
                let format = if export.includes_jsonl {
                    "transcript Markdown + JSONL"
                } else {
                    "scrollback Markdown"
                };
                let suffix = if copied { " (path copied)" } else { "" };
                self.toasts
                    .success(format!("Exported {format} to {path}{suffix}"));
            }
            Err(error) => self.toasts.error(format!("Session export failed: {error}")),
        }
        self.redraw_scheduler
            .request(super::redraw_scheduler::RedrawReason::WindowEvent);
    }
}

fn capture_scrollback(grid: &termeric_core::grid::Grid) -> Result<String, String> {
    let mut text = String::new();
    for row in grid
        .scrollback_rows()
        .chain((0..grid.rows()).filter_map(|row| grid.row_at_offset(row as i32)))
    {
        let line = row.to_ansi_string();
        let needed = text.len().saturating_add(line.len()).saturating_add(1);
        if needed > MAX_SCROLLBACK_BYTES {
            return Err("Scrollback export exceeds 32 MiB; no partial export was written".into());
        }
        if needed > text.capacity() {
            let capacity = needed
                .max(text.capacity().saturating_mul(2))
                .min(MAX_SCROLLBACK_BYTES);
            text.reserve_exact(capacity - text.len());
        }
        text.push_str(&line);
        text.push('\n');
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::session::transcript::TranscriptRecorder;
    use std::time::{Duration, Instant};

    fn focused(app: &App) -> crate::tabs::TerminalIdentity {
        app.tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .focused_terminal_identity()
    }

    fn app() -> App {
        let mut config = Config::default();
        config.shell.program = Some("/bin/cat".into());
        config.shell.args.clear();
        let mut app = App::new_headless(config).unwrap();
        app.allow_auto_restore = false;
        let mut tabs =
            crate::tabs::TabManager::new(80, 24, Default::default(), 1000, 8 * 1024 * 1024);
        tabs.new_tab_with_command(
            "/bin/cat",
            &[],
            None,
            None,
            Some(crate::tabs::TabType::Plugin {
                plugin_id: "export-fixture".into(),
            }),
            Default::default(),
        )
        .unwrap();
        app.tabs = Some(tabs);
        app
    }

    fn poll(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.session_export.is_some() {
            app.poll_idle_sources(Instant::now());
            assert!(Instant::now() < deadline, "export did not complete");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn export_busy_and_completion_stay_with_the_original_window_after_tab_closure() {
        let dir = tempfile::tempdir().unwrap();
        crate::session::set_test_sessions_base_dir(Some(dir.path().join("exports")));
        let mut app = app();
        let other = App::new_headless(Config::default()).unwrap();
        let mut recorder =
            TranscriptRecorder::new(dir.path(), None, None, "Local", "test").unwrap();
        recorder.record_output("captured fixture content").unwrap();
        app.tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .start_transcript(recorder);
        app.export_terminal_session(focused(&app));
        assert!(app.session_export.is_some());
        app.export_terminal_session(focused(&app));
        assert!(
            app.toasts
                .iter()
                .last()
                .unwrap()
                .message
                .contains("already in progress")
        );
        app.tabs = None; // Closes the source; the export owns its pending flush.
        poll(&mut app);
        assert!(
            app.toasts
                .iter()
                .last()
                .unwrap()
                .message
                .contains("Exported transcript")
        );
        assert!(
            !other
                .toasts
                .iter()
                .any(|toast| toast.message.contains("Exported"))
        );
        assert!(other.session_export.is_none());
        let directories: Vec<_> = std::fs::read_dir(dir.path().join("exports"))
            .unwrap()
            .collect();
        assert_eq!(directories.len(), 1);
        crate::session::set_test_sessions_base_dir(None);
    }

    #[test]
    fn unavailable_attached_transcript_reports_failure_instead_of_scrollback_success() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("exports");
        crate::session::set_test_sessions_base_dir(Some(base.clone()));
        let mut app = app();
        let mut recorder =
            TranscriptRecorder::new(dir.path(), None, None, "Local", "test").unwrap();
        recorder.finish(Duration::from_secs(2)).unwrap();
        app.tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .start_transcript(recorder);
        app.export_terminal_session(focused(&app));
        assert!(app.session_export.is_none());
        assert!(
            app.toasts
                .iter()
                .last()
                .unwrap()
                .message
                .contains("Session export failed")
        );
        assert!(!base.exists());
        crate::session::set_test_sessions_base_dir(None);
    }
}
