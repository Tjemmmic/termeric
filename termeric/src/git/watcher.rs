//! Background git state watcher
//!
//! Polls the git repository state on a timer to avoid blocking
//! the render loop. Updates branch name and dirty status.

use super::{GitRepo, GitStatus};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Background git state tracker for a terminal tab
pub struct GitWatcher {
    /// Last known working directory
    last_dir: Option<PathBuf>,
    /// Cached git status
    status: Option<GitStatus>,
    /// Cached branch completions (refreshed when branch list may change)
    cached_completions: Vec<String>,
    /// Last time we polled
    last_poll: Instant,
    /// Poll interval
    poll_interval: Duration,
}

impl GitWatcher {
    pub fn new() -> Self {
        Self {
            last_dir: None,
            status: None,
            cached_completions: Vec::new(),
            last_poll: Instant::now() - Duration::from_secs(10), // ensure first poll fires
            poll_interval: Duration::from_millis(2000),
        }
    }

    /// Update git state if enough time has passed or directory changed.
    /// Call this on each frame — it will skip if the poll interval hasn't elapsed.
    pub fn update(&mut self, working_dir: Option<&str>) {
        let dir = working_dir.map(PathBuf::from);

        // If directory changed, force immediate refresh
        let dir_changed = self.last_dir != dir;

        if !dir_changed && self.last_poll.elapsed() < self.poll_interval {
            return;
        }

        self.last_poll = Instant::now();
        self.last_dir = dir.clone();

        if let Some(ref path) = dir {
            if let Some(repo) = GitRepo::open(path) {
                self.status = Some(repo.status());
                // Only refresh completions when directory changes (branch list is relatively static)
                if dir_changed || self.cached_completions.is_empty() {
                    self.cached_completions = repo.local_branches();
                }
            } else {
                self.status = None;
                self.cached_completions.clear();
            }
        } else {
            self.status = None;
            self.cached_completions.clear();
        }
    }

    /// Get the current git status (if in a git repo)
    pub fn status(&self) -> Option<&GitStatus> {
        self.status.as_ref()
    }

    /// Get branch name for display
    pub fn branch(&self) -> Option<&str> {
        self.status.as_ref()?.branch.as_deref()
    }

    /// Check if working tree is dirty
    pub fn is_dirty(&self) -> bool {
        self.status.as_ref().map_or(false, |s| s.dirty)
    }

    /// Get branch completions for the given partial input.
    /// Uses the currently detected working directory.
    pub fn complete_branch(&self, partial: &str) -> Vec<String> {
        if let Some(ref path) = self.last_dir {
            if let Some(repo) = GitRepo::open(path) {
                return repo.complete_branch(partial);
            }
        }
        Vec::new()
    }

    /// Force a refresh on next update (e.g., after the user runs a git command)
    pub fn invalidate(&mut self) {
        self.last_poll = Instant::now() - Duration::from_secs(10);
    }
}
