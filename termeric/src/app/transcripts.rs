//! Transcript controls target one local terminal, never tab presentation.
use super::*;
use crate::tabs::TerminalIdentity;
use crate::ui::dialog::{DialogBlock, DialogSpec};

impl App {
    pub(super) fn toggle_terminal_transcript(&mut self, identity: TerminalIdentity) {
        let Some((agent, recording, cwd)) = self.tabs.as_ref().and_then(|tabs| {
            let origin = tabs.origin_for_pane(identity.tab_id, identity.pane_id)?;
            let tab = tabs.tab_by_id(identity.tab_id)?;
            if tab.live_domain.is_some() || tabs.origin_read_only(origin) != Some(false) {
                return None;
            }
            let state = tab.pane_integration(identity.pane_id)?;
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
                .pane_integration_mut(identity.pane_id)
                .unwrap()
                .stop_transcript()
                .unwrap();
            let path = recorder.path().display().to_string();
            match recorder.finish(Duration::from_millis(500)) {
                Ok(()) => {
                    self.dialog.show(DialogSpec::message(
                        "Transcript saved",
                        vec![DialogBlock::value(path)],
                    ));
                }
                Err(error) => {
                    self.dialog.show(DialogSpec::error(
                        "Transcript may be incomplete",
                        vec![
                            DialogBlock::text("The end of it couldn't be written to disk."),
                            DialogBlock::value(path),
                            DialogBlock::value(error.to_string()),
                        ],
                    ));
                }
            }
        } else if let Some(agent) = agent {
            let Some(base) = crate::session::sessions_base_dir() else {
                self.dialog.show(DialogSpec::error(
                    "Couldn't start a transcript",
                    vec![DialogBlock::text(
                        "There's no data folder to save it in. Check that HOME is set.",
                    )],
                ));
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
                        .pane_integration_mut(identity.pane_id)
                        .unwrap()
                        .start_transcript(recorder);
                    self.dialog.show(DialogSpec::message(
                        "Recording a transcript",
                        vec![
                            DialogBlock::text("This terminal's output is saved to:"),
                            DialogBlock::value(path),
                            DialogBlock::text(
                                "Common token formats are hidden, but other secrets may still be saved.",
                            ),
                        ],
                    ));
                }
                Err(error) => {
                    self.dialog.show(DialogSpec::error(
                        "Couldn't start a transcript",
                        vec![DialogBlock::value(error.to_string())],
                    ));
                }
            }
        }
        self.request_redraw(RedrawReason::Action);
    }

    /// Toggle transcript recording for the focused terminal.
    pub(super) fn toggle_transcript(&mut self) {
        if let Some(identity) = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.active_tab())
            .map(|tab| tab.focused_terminal_identity())
        {
            self.toggle_terminal_transcript(identity);
        } else {
            self.toasts.warning("No active terminal");
        }
    }
}
