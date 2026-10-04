//! Git repository integration
//!
//! Provides git-aware features: branch detection and branch-name completion.
//! Uses libgit2 via the `git2` crate.

mod completion;
mod completion_list;
mod watcher;

pub(crate) use completion::{BranchQuoting, branch_edit};
pub use completion_list::BranchCompletions;
pub use watcher::GitWatcher;

use git2::Repository;
use std::path::Path;

const MAX_BRANCH_NAMES: usize = 16_384;
const MAX_BRANCH_NAME_BYTES: usize = 4 * 1024 * 1024;

/// Shared across local and remote collection, before sorting/deduplication.
/// Charge before copying so a rejected name never becomes an owned String.
struct BranchBudget {
    names_left: usize,
    bytes_left: usize,
}

impl BranchBudget {
    fn copy_name(&mut self, name: &str) -> Option<String> {
        if self.names_left == 0 || name.len() > self.bytes_left {
            return None;
        }
        self.names_left -= 1;
        self.bytes_left -= name.len();
        Some(name.to_owned())
    }
}

/// Wrapper around a git repository for querying its branches.
pub struct GitRepo {
    repo: Repository,
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

    /// List all local branch names
    fn local_branches(
        &self,
        budget: &mut BranchBudget,
    ) -> Result<Option<Vec<String>>, git2::Error> {
        let mut branches = Vec::new();
        for entry in self.repo.branches(Some(git2::BranchType::Local))? {
            let (branch, _) = entry?;
            if let Some(name) = branch.name()? {
                let Some(name) = budget.copy_name(name) else {
                    return Ok(None);
                };
                branches.push(name);
            }
        }
        branches.sort();
        Ok(Some(branches))
    }

    /// List all remote branch names (without "origin/" prefix)
    fn remote_branches(
        &self,
        budget: &mut BranchBudget,
    ) -> Result<Option<Vec<String>>, git2::Error> {
        let mut branches = Vec::new();
        for entry in self.repo.branches(Some(git2::BranchType::Remote))? {
            let (branch, _) = entry?;
            if let Some(name) = branch.name()? {
                // Strip "origin/" prefix for cleaner display
                let short = name.strip_prefix("origin/").unwrap_or(name);
                // Skip every remote's HEAD pointer
                if !name.ends_with("/HEAD") {
                    let Some(name) = budget.copy_name(short) else {
                        return Ok(None);
                    };
                    branches.push(name);
                }
            }
        }
        branches.sort();
        branches.dedup();
        Ok(Some(branches))
    }

    /// None means completion was disabled by a collection limit, not an empty
    /// repository. Never publish a partial set as if it were complete.
    pub(crate) fn branch_names(&self) -> Result<Option<Vec<String>>, git2::Error> {
        self.branch_names_with_budget(BranchBudget {
            names_left: MAX_BRANCH_NAMES,
            bytes_left: MAX_BRANCH_NAME_BYTES,
        })
    }

    fn branch_names_with_budget(
        &self,
        mut budget: BranchBudget,
    ) -> Result<Option<Vec<String>>, git2::Error> {
        let Some(local) = self.local_branches(&mut budget)? else {
            return Ok(None);
        };
        let Some(remote) = self.remote_branches(&mut budget)? else {
            return Ok(None);
        };
        Ok(Some(merge_sorted_branches(local, remote)))
    }
}

/// Both inputs are sorted and individually unique. Keep locals first, followed
/// by remote-only names in their existing order. The local cursor only advances:
/// duplicate checking takes O(local + remote) comparisons, without a second set
/// or cloning any branch strings. Never compare against the appended suffix,
/// since the combined local-first result is not necessarily globally sorted.
fn merge_sorted_branches(mut local: Vec<String>, remote: Vec<String>) -> Vec<String> {
    let local_count = local.len();
    let mut cursor = 0;
    for name in remote {
        while cursor < local_count && local[cursor] < name {
            cursor += 1;
        }
        if cursor == local_count || local[cursor] != name {
            local.push(name);
        }
    }
    local
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
    if !input.chars().all(completion::simple_command_character) {
        return None;
    }
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
    fn branch_merge_matches_local_first_reference_for_all_small_subsets() {
        let candidates = ["A", "a", "a/branch", "feature", "main", "é", "界"];
        for local_mask in 0..1usize << candidates.len() {
            for remote_mask in 0..1usize << candidates.len() {
                let subset = |mask: usize| {
                    candidates
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| mask & (1 << index) != 0)
                        .map(|(_, name)| name.to_string())
                        .collect::<Vec<_>>()
                };
                let local = subset(local_mask);
                let remote = subset(remote_mask);
                let mut expected = local.clone();
                for name in &remote {
                    if !expected.contains(name) {
                        expected.push(name.clone());
                    }
                }
                assert_eq!(merge_sorted_branches(local, remote), expected);
            }
        }
    }

    #[test]
    fn branch_merge_preserves_large_overlapping_lists_and_completion_priority() {
        let local = (20_000..40_000).map(|i| format!("branch-{i:05}")).collect();
        let remote = (0..60_000).map(|i| format!("branch-{i:05}")).collect();
        let result = merge_sorted_branches(local, remote);
        let expected: Vec<_> = (20_000..40_000)
            .chain(0..20_000)
            .chain(40_000..60_000)
            .map(|i| format!("branch-{i:05}"))
            .collect();
        assert_eq!(result, expected);
        assert_eq!(filter_branches(&result, "BRANCH-20000"), ["branch-20000"]);
        assert_eq!(filter_branches(&result, "59999"), ["branch-59999"]);
        assert_eq!(filter_branches(&result, ""), expected);
    }

    #[test]
    fn repository_branch_snapshot_preserves_remote_normalization_and_order() {
        let directory = tempfile::tempdir().unwrap();
        let repo = Repository::init_bare(directory.path()).unwrap();
        let tree_id = repo.treebuilder(None).unwrap().write().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let signature =
            git2::Signature::new("Fixture", "fixture@example.invalid", &git2::Time::new(1, 0))
                .unwrap();
        let commit = repo
            .commit(None, &signature, &signature, "fixture", &tree, &[])
            .unwrap();
        for reference in [
            "refs/heads/z-local",
            "refs/heads/main",
            "refs/heads/upstream/shared",
            "refs/remotes/origin/main",
            "refs/remotes/origin/a-remote",
            "refs/remotes/origin/upstream/shared",
            "refs/remotes/upstream/shared",
            "refs/remotes/upstream/other",
        ] {
            repo.reference(reference, commit, false, "fixture").unwrap();
        }
        for (head, target) in [
            ("refs/remotes/origin/HEAD", "refs/remotes/origin/main"),
            ("refs/remotes/upstream/HEAD", "refs/remotes/upstream/other"),
        ] {
            repo.reference_symbolic(head, target, false, "fixture")
                .unwrap();
        }
        drop(tree);
        let git = GitRepo { repo };
        let names = git.branch_names().unwrap().unwrap();
        assert_eq!(
            names,
            [
                "main",
                "upstream/shared",
                "z-local",
                "a-remote",
                "upstream/other"
            ]
        );
        // Eight collected names, including normalized/cross-list duplicates.
        // origin/HEAD is excluded before charging. Charge normalized names
        // before sorting and deduplication, across both local and remote lists.
        let bytes = [
            "z-local",
            "main",
            "upstream/shared",
            "main",
            "a-remote",
            "upstream/shared",
            "upstream/shared",
            "upstream/other",
        ]
        .iter()
        .map(|name| name.len())
        .sum::<usize>();
        for (names_left, bytes_left, accepted) in [
            (8, bytes, true),
            (7, bytes, false),
            (8, bytes - 1, false),
            (0, bytes, false),
            (8, 0, false),
        ] {
            let result = git
                .branch_names_with_budget(BranchBudget {
                    names_left,
                    bytes_left,
                })
                .unwrap();
            assert_eq!(result.is_some(), accepted);
            if let Some(result) = result {
                assert_eq!(result, names);
            }
        }
    }

    #[test]
    fn branch_budget_counts_utf8_bytes_and_rejects_without_charging() {
        let mut budget = BranchBudget {
            names_left: 2,
            bytes_left: 5,
        };
        assert_eq!(budget.copy_name("é"), Some("é".into()));
        assert_eq!(budget.copy_name("long"), None);
        assert_eq!((budget.names_left, budget.bytes_left), (1, 3));
        assert_eq!(budget.copy_name("界"), Some("界".into()));
        assert_eq!(budget.copy_name("x"), None);
        assert_eq!((budget.names_left, budget.bytes_left), (0, 0));
    }

    #[test]
    fn production_branch_budget_accepts_exact_limits_then_refuses() {
        let mut budget = BranchBudget {
            names_left: MAX_BRANCH_NAMES,
            bytes_left: MAX_BRANCH_NAME_BYTES,
        };
        assert_eq!(
            budget.copy_name(&"x".repeat(MAX_BRANCH_NAME_BYTES + 1)),
            None
        );
        assert_eq!(budget.bytes_left, MAX_BRANCH_NAME_BYTES);
        assert_eq!(budget.names_left, MAX_BRANCH_NAMES);
        let name = "x".repeat(MAX_BRANCH_NAME_BYTES / MAX_BRANCH_NAMES);
        let names: Vec<_> = (0..MAX_BRANCH_NAMES)
            .map(|_| budget.copy_name(&name).unwrap())
            .collect();
        assert_eq!(names.len(), MAX_BRANCH_NAMES);
        assert_eq!(
            names.iter().map(String::len).sum::<usize>(),
            MAX_BRANCH_NAME_BYTES
        );
        assert_eq!(budget.copy_name("x"), None);
        assert_eq!((budget.names_left, budget.bytes_left), (0, 0));
    }

    #[test]
    fn empty_repository_needs_no_branch_budget() {
        let directory = tempfile::tempdir().unwrap();
        let git = GitRepo {
            repo: Repository::init_bare(directory.path()).unwrap(),
        };
        assert_eq!(
            git.branch_names_with_budget(BranchBudget {
                names_left: 0,
                bytes_left: 0,
            })
            .unwrap(),
            Some(Vec::new())
        );
    }

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
