//! Git repository integration
//!
//! Provides git-aware features: branch detection, auto-completion,
//! and status tracking. Uses libgit2 via the `git2` crate.

mod watcher;

pub use watcher::GitWatcher;

use git2::Repository;
use std::path::Path;

/// Wrapper around a git repository for querying branches, status, etc.
pub struct GitRepo {
    repo: Repository,
}

/// Compact git status for display
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitStatus {
    /// Current HEAD branch name (None if detached)
    pub branch: Option<String>,
    /// Working tree has uncommitted changes
    pub dirty: bool,
    /// Number of commits ahead of upstream
    pub ahead: usize,
    /// Number of commits behind upstream
    pub behind: usize,
}

impl GitRepo {
    /// Discover a repository containing the path, distinguishing a normal
    /// non-repository directory from an actionable libgit2 failure.
    pub(crate) fn discover(path: &Path) -> Result<Option<Self>, git2::Error> {
        match Repository::discover(path) {
            Ok(repo) => Ok(Some(Self { repo })),
            Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Get the current branch name (None if HEAD is detached)
    pub fn current_branch(&self) -> Option<String> {
        let head = self.repo.head().ok()?;
        if head.is_branch() {
            head.shorthand().map(|s| s.to_string())
        } else {
            // Detached HEAD — show short hash
            let oid = head.target()?;
            Some(format!("{:.7}", oid))
        }
    }

    /// Check if the working tree has any changes (modified, staged, or untracked)
    fn is_dirty(&self) -> Result<bool, git2::Error> {
        let mut opts = git2::StatusOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(false)
            .exclude_submodules(true);

        self.repo
            .statuses(Some(&mut opts))
            .map(|statuses| !statuses.is_empty())
    }

    /// Get full git status (branch, dirty, ahead/behind)
    pub(crate) fn status(&self) -> Result<GitStatus, git2::Error> {
        let branch = self.current_branch();
        let dirty = self.is_dirty()?;
        let (ahead, behind) = self.ahead_behind().unwrap_or((0, 0));
        Ok(GitStatus {
            branch,
            dirty,
            ahead,
            behind,
        })
    }

    /// Calculate ahead/behind counts relative to upstream
    fn ahead_behind(&self) -> Option<(usize, usize)> {
        let head = self.repo.head().ok()?;
        let local_oid = head.target()?;

        let branch_name = head.shorthand()?;
        let upstream_name = format!("refs/remotes/origin/{}", branch_name);
        let upstream_ref = self.repo.find_reference(&upstream_name).ok()?;
        let upstream_oid = upstream_ref.target()?;

        self.repo.graph_ahead_behind(local_oid, upstream_oid).ok()
    }

    /// List all local branch names
    fn local_branches(&self) -> Result<Vec<String>, git2::Error> {
        let mut branches = Vec::new();
        for entry in self.repo.branches(Some(git2::BranchType::Local))? {
            let (branch, _) = entry?;
            if let Some(name) = branch.name()? {
                branches.push(name.to_string());
            }
        }
        branches.sort();
        Ok(branches)
    }

    /// List all remote branch names (without "origin/" prefix)
    fn remote_branches(&self) -> Result<Vec<String>, git2::Error> {
        let mut branches = Vec::new();
        for entry in self.repo.branches(Some(git2::BranchType::Remote))? {
            let (branch, _) = entry?;
            if let Some(name) = branch.name()? {
                // Strip "origin/" prefix for cleaner display
                let short = name.strip_prefix("origin/").unwrap_or(name);
                // Skip HEAD pointer
                if short != "HEAD" {
                    branches.push(short.to_string());
                }
            }
        }
        branches.sort();
        branches.dedup();
        Ok(branches)
    }

    pub(crate) fn branch_names(&self) -> Result<Vec<String>, git2::Error> {
        let mut names = self.local_branches()?;
        for remote in self.remote_branches()? {
            if !names.contains(&remote) {
                names.push(remote);
            }
        }
        Ok(names)
    }
}

fn filter_branches(branches: &[String], partial: &str) -> Vec<String> {
    if partial.is_empty() {
        return branches.to_vec();
    }

    let partial_lower = partial.to_lowercase();
    let mut exact_prefix = Vec::new();
    let mut contains = Vec::new();
    for name in branches {
        let name_lower = name.to_lowercase();
        if name_lower.starts_with(&partial_lower) {
            exact_prefix.push(name.clone());
        } else if name_lower.contains(&partial_lower) {
            contains.push(name.clone());
        }
    }

    exact_prefix.extend(contains);
    exact_prefix
}

/// Git subcommands that accept branch names as arguments
const BRANCH_SUBCOMMANDS: &[&str] = &[
    "checkout",
    "switch",
    "merge",
    "rebase",
    "branch",
    "diff",
    "log",
    "cherry-pick",
    "reset",
];

/// Parse the input buffer to detect if user is typing a git command
/// that could use branch auto-completion.
///
/// Returns `Some(partial)` if completions should be shown,
/// where `partial` is the partial branch name typed so far.
pub fn detect_git_completion(input: &str) -> Option<&str> {
    let trimmed = input.trim_start();

    // Must start with "git "
    let rest = trimmed.strip_prefix("git ")?;

    // Find the subcommand
    let mut parts = rest.splitn(2, ' ');
    let subcommand = parts.next()?;

    // Check if this subcommand accepts branches
    if !BRANCH_SUBCOMMANDS.contains(&subcommand) {
        return None;
    }

    // Get the remaining text after the subcommand
    match parts.next() {
        Some(args) => {
            // Get the last whitespace-separated token (partial branch name)
            let last_arg = args.rsplit(' ').next().unwrap_or("");
            // Skip flags (starting with -)
            if last_arg.starts_with('-') {
                return None;
            }
            Some(last_arg)
        }
        None => None, // Just "git checkout" with no space after — don't complete yet
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_git_completion() {
        // Should trigger completions
        assert_eq!(detect_git_completion("git checkout "), Some(""));
        assert_eq!(detect_git_completion("git checkout ma"), Some("ma"));
        assert_eq!(detect_git_completion("git switch feat"), Some("feat"));
        assert_eq!(detect_git_completion("git merge dev"), Some("dev"));
        assert_eq!(detect_git_completion("git rebase main"), Some("main"));

        // Should NOT trigger
        assert_eq!(detect_git_completion("git checkout"), None); // no trailing space
        assert_eq!(detect_git_completion("git status"), None); // not a branch subcommand
        assert_eq!(detect_git_completion("echo hello"), None); // not git
        assert_eq!(detect_git_completion("git checkout -b"), None); // flag
    }
}
