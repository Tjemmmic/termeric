//! Reviewing what an agent changed in its Git working tree: a list of changed
//! files, each file's diff, and staging or reverting it.
//!
//! Git work runs on a worker thread. Every job is bound to the agent it was
//! started for — a terminal's, or an agent view session's — and to that
//! agent's baseline, so a result that arrives after the agent was restarted,
//! or its terminal or session closed, is dropped.

use super::*;
use crate::agents::changes::{self, Baseline, ChangeKind, ChangedFile};
use crate::tabs::TerminalIdentity;
use crate::ui::command_palette::palette::{CommandSource, PickKind, RegisteredCommand};
use crate::ui::diff_review::{ChangeOwner, ChangeReviewTarget};
use crate::ui::info_overlay::InfoOverlayChoice;
use std::path::PathBuf;
use std::sync::mpsc;

/// How often a running job is checked for completion.
const JOB_POLL_INTERVAL: Duration = Duration::from_millis(30);
const REVERT_CHOICE: &str = "agent-change-revert";
const KEEP_CHOICE: &str = "agent-change-keep";

enum ChangeTask {
    List,
    Diff(PathBuf),
    Stage(Vec<PathBuf>),
    /// Stage every file that differs from the baseline.
    StageAll,
    Revert(PathBuf),
}

enum ChangeResult {
    List(Vec<ChangedFile>),
    Diff(PathBuf, String),
    Staged(Vec<PathBuf>),
    Reverted(PathBuf),
}

pub(super) struct ChangeJob {
    owner: ChangeOwner,
    baseline: Baseline,
    receiver: mpsc::Receiver<Result<ChangeResult, String>>,
}

/// The terminal whose changes the open picker lists.
pub(super) struct ChangeList {
    owner: ChangeOwner,
    baseline: Baseline,
    /// The listed files; picker rows are indexes into this.
    paths: Vec<PathBuf>,
}

/// The agent change the diff viewer shows, with the baseline its diff was
/// computed from.
pub(super) struct ChangeReview {
    request_id: u64,
    owner: ChangeOwner,
    baseline: Baseline,
    path: PathBuf,
}

/// A revert waiting for the person to confirm it.
pub(super) struct PendingRevert {
    request_id: u64,
    owner: ChangeOwner,
    baseline: Baseline,
    path: PathBuf,
}

impl App {
    /// The focused terminal's baseline, once captured. It survives the
    /// agent, so changes can be reviewed after it exits.
    pub(super) fn focused_change_baseline(&self) -> Option<(TerminalIdentity, Baseline)> {
        let tab = self.tabs.as_ref()?.active_tab()?;
        let identity = tab.focused_terminal_identity();
        let baseline = tab
            .pane_integration(identity.pane_id)?
            .agent_changes
            .baseline()?
            .clone();
        Some((identity, baseline))
    }

    fn baseline_is_current(&self, owner: ChangeOwner, baseline: &Baseline) -> bool {
        match owner {
            ChangeOwner::Terminal(identity) => self
                .tabs
                .as_ref()
                .and_then(|tabs| tabs.tab_by_id(identity.tab_id))
                .and_then(|tab| tab.pane_integration(identity.pane_id))
                .and_then(|session| session.agent_changes.baseline())
                .is_some_and(|current| current == baseline),
            ChangeOwner::AgentView(id) => self
                .agent_view
                .baseline(id)
                .is_some_and(|current| current == baseline),
        }
    }

    /// List the files an agent view session changed since it started.
    pub(super) fn review_agent_view_changes(&mut self, id: crate::agent_view::session::SessionId) {
        match self.agent_view.baseline(id).cloned() {
            Some(baseline) => {
                self.start_change_job(ChangeOwner::AgentView(id), baseline, ChangeTask::List);
            }
            None => {
                let reason = self.agent_view.baseline_unavailable(id);
                self.toasts.info(match reason {
                    Some(reason) => format!("Changes cannot be reviewed: {reason}"),
                    None => "Still taking a snapshot of the folder; try again in a moment".into(),
                });
            }
        }
    }

    /// List the files the focused terminal's agent changed.
    pub(super) fn review_agent_changes(&mut self) {
        self.run_focused_change_task(ChangeTask::List);
    }

    /// Stage everything the focused terminal's agent changed.
    pub(super) fn stage_all_agent_changes(&mut self) {
        self.run_focused_change_task(ChangeTask::StageAll);
    }

    fn run_focused_change_task(&mut self, task: ChangeTask) {
        let Some((identity, baseline)) = self.focused_change_baseline() else {
            let reason = self
                .tabs
                .as_ref()
                .and_then(TabManager::active_tab)
                .and_then(|tab| tab.pane_integration(tab.focused_terminal_identity().pane_id))
                .and_then(|session| session.agent_changes.unavailable_reason())
                .map(str::to_owned);
            self.toasts.info(match reason {
                Some(reason) => format!("Agent changes cannot be reviewed: {reason}"),
                None => "Start an agent in a Git repository to review its changes".into(),
            });
            return;
        };
        self.start_change_job(ChangeOwner::Terminal(identity), baseline, task);
    }

    fn start_change_job(&mut self, owner: ChangeOwner, baseline: Baseline, task: ChangeTask) {
        // One job at a time: a replaced worker would keep running, and two
        // stages could overwrite each other's index.
        if self.agent_change_job.is_some() {
            self.toasts
                .info("Still working on the previous agent change; try again in a moment");
            return;
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker_baseline = baseline.clone();
        let spawned = std::thread::Builder::new()
            .name("termeric-agent-changes".into())
            .spawn(move || {
                let baseline = worker_baseline;
                let result = match task {
                    ChangeTask::List => changes::changes_since(&baseline).map(ChangeResult::List),
                    ChangeTask::Diff(path) => changes::file_diff(&baseline, &path)
                        .map(|text| ChangeResult::Diff(path, text)),
                    ChangeTask::Stage(paths) => {
                        changes::stage(&baseline, &paths).map(|()| ChangeResult::Staged(paths))
                    }
                    ChangeTask::StageAll => changes::changes_since(&baseline).and_then(|files| {
                        let paths: Vec<PathBuf> = files.into_iter().map(|file| file.path).collect();
                        changes::stage(&baseline, &paths).map(|()| ChangeResult::Staged(paths))
                    }),
                    ChangeTask::Revert(path) => {
                        changes::revert(&baseline, &path).map(|()| ChangeResult::Reverted(path))
                    }
                };
                let _ = sender.send(result.map_err(|error| format!("{error:#}")));
            });
        match spawned {
            Ok(_) => {
                self.agent_change_job = Some(ChangeJob {
                    owner,
                    baseline,
                    receiver,
                })
            }
            Err(error) => self
                .toasts
                .error(format!("Could not compare agent changes: {error}")),
        }
    }

    pub(super) fn next_agent_change_deadline(&self, now: Instant) -> Option<Instant> {
        self.agent_change_job
            .as_ref()
            .map(|_| now + JOB_POLL_INTERVAL)
    }

    /// Act on a finished job. Returns whether anything changed on screen.
    pub(super) fn poll_agent_change_job(&mut self) -> bool {
        let Some(job) = &self.agent_change_job else {
            return false;
        };
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("the comparison stopped unexpectedly".to_string())
            }
        };
        let ChangeJob {
            owner, baseline, ..
        } = self
            .agent_change_job
            .take()
            .expect("a job was checked above");
        if !self.baseline_is_current(owner, &baseline) {
            // The agent restarted or its terminal or session closed meanwhile.
            return false;
        }
        match result {
            Err(error) => self.toasts.error(format!("Agent changes: {error}")),
            Ok(ChangeResult::List(files)) => self.show_change_list(owner, baseline, &files),
            Ok(ChangeResult::Diff(path, text)) => {
                let request_id = self.diff_review.show_change(
                    &text,
                    ChangeReviewTarget {
                        owner,
                        path: path.clone(),
                    },
                );
                self.agent_change_review = Some(ChangeReview {
                    request_id,
                    owner,
                    baseline,
                    path,
                });
            }
            Ok(ChangeResult::Staged(paths)) => {
                self.toasts.success(match paths.as_slice() {
                    [] => "The agent has not changed any files since it started".into(),
                    [path] => format!("Staged {}", path.display()),
                    paths => format!("Staged {} files", paths.len()),
                });
                self.start_change_job(owner, baseline, ChangeTask::List);
            }
            Ok(ChangeResult::Reverted(path)) => {
                self.toasts.success(format!("Reverted {}", path.display()));
                self.start_change_job(owner, baseline, ChangeTask::List);
            }
        }
        true
    }

    fn show_change_list(&mut self, owner: ChangeOwner, baseline: Baseline, files: &[ChangedFile]) {
        if files.is_empty() {
            self.toasts
                .info("The agent has not changed any files since it started");
            return;
        }
        let items = files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let change = match (file.kind, file.binary) {
                    (ChangeKind::Deleted, _) => "deleted".to_string(),
                    (_, true) => "binary".to_string(),
                    (ChangeKind::Added, false) => format!("new, +{}", file.added_lines),
                    (ChangeKind::Modified, false) => {
                        format!("+{} −{}", file.added_lines, file.removed_lines)
                    }
                };
                RegisteredCommand {
                    id: index.to_string(),
                    title: file.path.to_string_lossy().into_owned(),
                    category: if file.staged {
                        format!("{change} · staged")
                    } else {
                        change
                    },
                    shortcut: None,
                    source: CommandSource::Core,
                    destructive: false,
                }
            })
            .collect();
        self.agent_change_list = Some(ChangeList {
            owner,
            baseline,
            paths: files.iter().map(|file| file.path.clone()).collect(),
        });
        self.command_palette.begin_pick(
            PickKind::AgentChange,
            items,
            "Review which file…",
            "file",
            None,
        );
    }

    /// Act on a row picked from the change list.
    pub(super) fn pick_agent_change(&mut self, id: &str) {
        let Some(ChangeList {
            owner,
            baseline,
            paths,
        }) = self.agent_change_list.take()
        else {
            return;
        };
        let Some(path) = id.parse::<usize>().ok().and_then(|index| paths.get(index)) else {
            return;
        };
        if !self.baseline_is_current(owner, &baseline) {
            self.toasts.warning("That agent has restarted or closed");
            return;
        }
        self.start_change_job(owner, baseline, ChangeTask::Diff(path.clone()));
    }

    /// The change the diff viewer shows as `request_id`, while its terminal
    /// still has the baseline the diff came from.
    fn reviewed_change(&mut self, request_id: u64) -> Option<ChangeReview> {
        self.diff_review.change_target(request_id)?;
        let review = self
            .agent_change_review
            .take()
            .filter(|review| review.request_id == request_id)?;
        if !self.baseline_is_current(review.owner, &review.baseline) {
            self.toasts
                .warning("The agent has restarted since this diff was shown; review it again");
            return None;
        }
        Some(review)
    }

    /// Stage the file under review.
    pub(super) fn stage_reviewed_change(&mut self, request_id: u64) {
        let Some(review) = self.reviewed_change(request_id) else {
            return;
        };
        self.diff_review.dismiss(request_id);
        self.start_change_job(
            review.owner,
            review.baseline,
            ChangeTask::Stage(vec![review.path]),
        );
    }

    /// Ask before putting the file under review back as it was. The
    /// confirmation has no letter shortcut, so a repeated key cannot revert.
    pub(super) fn confirm_revert_reviewed_change(&mut self, request_id: u64) {
        let Some(review) = self.reviewed_change(request_id) else {
            return;
        };
        self.diff_review.dismiss(request_id);
        let lines = vec![
            format!(
                "Put {} back as it was when the agent started?",
                review.path.display()
            ),
            "The agent's edits to this file are lost. Other files, staged".into(),
            "changes, and commits stay as they are.".into(),
            String::new(),
            "  Revert".into(),
            "  Keep the changes".into(),
        ];
        let revert_request = self.info_overlay.show_choices(
            "Revert file",
            lines,
            vec![
                InfoOverlayChoice::new(REVERT_CHOICE, "Revert", 4, None),
                InfoOverlayChoice::new(KEEP_CHOICE, "Keep the changes", 5, None),
            ],
        );
        self.pending_revert = Some(PendingRevert {
            request_id: revert_request,
            owner: review.owner,
            baseline: review.baseline,
            path: review.path,
        });
    }

    /// Answer the revert confirmation. Returns whether the choice was its.
    pub(super) fn handle_revert_choice(&mut self, request_id: u64, choice_id: &str) -> bool {
        if !matches!(choice_id, REVERT_CHOICE | KEEP_CHOICE)
            || self
                .pending_revert
                .as_ref()
                .is_none_or(|pending| pending.request_id != request_id)
        {
            return false;
        }
        self.info_overlay.dismiss(request_id);
        let Some(pending) = self.pending_revert.take() else {
            return true;
        };
        if choice_id == REVERT_CHOICE {
            if self.baseline_is_current(pending.owner, &pending.baseline) {
                self.start_change_job(
                    pending.owner,
                    pending.baseline,
                    ChangeTask::Revert(pending.path),
                );
            } else {
                self.toasts.warning("That agent has restarted or closed");
            }
        }
        true
    }

    /// Leave change review: the list, the diff's actions, a revert awaiting
    /// confirmation, and a job whose result would open them again.
    pub(super) fn close_agent_change_review(&mut self) {
        if let Some(pending) = self.pending_revert.take() {
            self.info_overlay.dismiss(pending.request_id);
        }
        self.agent_change_review = None;
        self.agent_change_list = None;
        self.agent_change_job = None;
    }

    pub(super) fn cancel_pending_revert(&mut self, request_id: u64) {
        if self
            .pending_revert
            .as_ref()
            .is_some_and(|pending| pending.request_id == request_id)
        {
            self.pending_revert = None;
        }
    }
}
