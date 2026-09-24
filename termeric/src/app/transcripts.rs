//! Transcript controls target one local terminal, never tab presentation.
use super::*;
use crate::tabs::TerminalIdentity;

impl App {
    pub(super) fn toggle_terminal_transcript(&mut self, identity: TerminalIdentity) {
        let Some((agent, recording, cwd)) = self.tabs.as_ref().and_then(|tabs| {
            let origin = tabs.origin_for_pane(identity.tab_id, identity.pane_id)?;
            let tab = tabs.tab_by_id(identity.tab_id)?;
            if tab.live_domain.is_some() || tabs.origin_read_only(origin) != Some(false) {
                return None;
            }
            let state = tab.pane_session(identity.pane_id)?;
            // A recording can be stopped after its agent has exited; only a
            // new one needs an agent to record.
            let recording = state.transcript.is_some();
            if !recording && state.agent.is_none() {
                return None;
            }
            Some((
                state.agent.clone(),
                recording,
                tab.pane_working_directory(identity.pane_id),
            ))
        }) else {
            self.toasts
                .warning("Focus a writable local agent terminal to toggle its transcript");
            return;
        };
        if recording {
            let mut recorder = self
                .tabs
                .as_mut()
                .unwrap()
                .tab_mut_by_id(identity.tab_id)
                .unwrap()
                .pane_session_mut(identity.pane_id)
                .unwrap()
                .stop_transcript()
                .unwrap();
            let path = recorder.path().display().to_string();
            match recorder.finish(Duration::from_millis(500)) {
                Ok(()) => {
                    self.info_overlay.show(
                        "Transcript Stopped",
                        vec!["Transcript saved to:".into(), path],
                    );
                }
                Err(error) => {
                    self.info_overlay.show(
                        "Transcript Incomplete",
                        vec![
                            format!("Could not durably flush: {error}"),
                            format!("Partial path: {path}"),
                        ],
                    );
                }
            }
        } else if let Some(agent) = agent {
            let Some(base) = crate::session::sessions_base_dir() else {
                self.info_overlay.show(
                    "Transcript Error",
                    vec!["Session data directory is unavailable".into()],
                );
                return;
            };
            let repo = cwd.as_deref().and_then(|cwd| {
                std::path::Path::new(cwd)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            });
            let branch = self.git_watcher.branch().map(str::to_owned);
            let label = match &agent.profile {
                Some(profile) => format!("{} ({profile})", agent.agent_id),
                None => agent.agent_id.clone(),
            };
            match crate::session::transcript::TranscriptRecorder::new(
                &base.join("transcripts"),
                repo.as_deref(),
                branch.as_deref(),
                "Local",
                &label,
            ) {
                Ok(recorder) => {
                    let path = recorder.path().display().to_string();
                    self.tabs
                        .as_mut()
                        .unwrap()
                        .tab_mut_by_id(identity.tab_id)
                        .unwrap()
                        .pane_session_mut(identity.pane_id)
                        .unwrap()
                        .start_transcript(recorder);
                    self.info_overlay.show(
                        "Transcript Started",
                        vec![
                            "Recording this terminal's output to:".into(),
                            path,
                            String::new(),
                            "Common token prefixes are redacted, but".into(),
                            "this is not a complete secret scanner.".into(),
                        ],
                    );
                }
                Err(error) => {
                    self.info_overlay.show(
                        "Transcript Error",
                        vec![format!("Failed to start recording: {error}")],
                    );
                }
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}
