//! Immutable suggestions shared by input state and successive render snapshots.

use std::ops::Deref;
use std::sync::Arc;

#[derive(Debug)]
struct CompletionList {
    names: Vec<String>,
    max_name_chars: usize,
}

/// Cloning a render snapshot shares names instead of copying every string.
/// Empty/cleared lists need no allocation. Only whole-list replacement is
/// allowed, so cached sizing metadata cannot diverge from the names it sizes.
#[derive(Debug, Clone, Default)]
pub struct BranchCompletions(Option<Arc<CompletionList>>);

impl BranchCompletions {
    pub fn clear(&mut self) {
        self.0 = None;
    }

    /// Unicode scalar count, matching the dropdown's existing monospace sizing.
    /// This is not a grapheme count or a terminal display-width calculation.
    pub fn max_name_chars(&self) -> usize {
        self.0.as_ref().map_or(0, |list| list.max_name_chars)
    }
}

impl From<Vec<String>> for BranchCompletions {
    fn from(names: Vec<String>) -> Self {
        if names.is_empty() {
            return Self::default();
        }
        let max_name_chars = names.iter().map(|name| name.chars().count()).max().unwrap();
        Self(Some(Arc::new(CompletionList {
            names,
            max_name_chars,
        })))
    }
}

impl Deref for BranchCompletions {
    type Target = [String];

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().map_or(&[], |list| list.names.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::GitCompletionInfo;

    #[test]
    fn empty_and_cleared_completion_lists_are_empty() {
        for mut list in [BranchCompletions::default(), Vec::<String>::new().into()] {
            assert!(list.is_empty());
            assert_eq!(list.max_name_chars(), 0);
            list.clear();
            assert!(list.is_empty());
        }
        let mut list = BranchCompletions::from(vec!["main".into()]);
        list.clear();
        assert!(list.is_empty());
        assert_eq!(list.max_name_chars(), 0);
    }

    #[test]
    fn completion_metadata_matches_existing_unicode_scalar_sizing() {
        let names: Vec<String> = ["é", "界", "e\u{301}", "topic/界"]
            .into_iter()
            .map(String::from)
            .collect();
        let list = BranchCompletions::from(names);
        assert_eq!(list.max_name_chars(), 7);
        assert_eq!(&*list, ["é", "界", "e\u{301}", "topic/界"]);
    }

    #[test]
    fn render_snapshots_keep_their_list_across_replacement() {
        let mut list = BranchCompletions::from(
            (0..super::super::MAX_BRANCH_NAMES)
                .map(|index| format!("branch-{index:05}-{}", "x".repeat(243)))
                .collect::<Vec<_>>(),
        );
        assert_eq!(list.len(), super::super::MAX_BRANCH_NAMES);
        assert_eq!(
            list.iter().map(String::len).sum::<usize>(),
            super::super::MAX_BRANCH_NAME_BYTES
        );
        let frame = GitCompletionInfo {
            completions: list.clone(),
            selected_index: Some(0),
        };
        assert_eq!(frame.completions.max_name_chars(), 256);
        list.clear();
        assert!(list.is_empty());
        assert_eq!(frame.completions.len(), super::super::MAX_BRANCH_NAMES);
        list = vec!["new".into()].into();
        assert_eq!(list.max_name_chars(), 3);
        assert_eq!(frame.completions.max_name_chars(), 256);
        assert_eq!(
            frame.completions[0],
            "branch-00000-".to_string() + &"x".repeat(243)
        );
    }
}
