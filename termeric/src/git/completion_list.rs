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
    fn empty_and_cleared_completion_lists_have_no_backing_allocation() {
        for mut list in [BranchCompletions::default(), Vec::<String>::new().into()] {
            assert!(list.0.is_none());
            assert!(list.is_empty());
            assert_eq!(list.max_name_chars(), 0);
            assert!(list.clone().0.is_none());
            list.clear();
            assert!(list.0.is_none());
        }
        let mut list = BranchCompletions::from(vec!["main".into()]);
        let weak = Arc::downgrade(list.0.as_ref().unwrap());
        list.clear();
        assert!(weak.upgrade().is_none());
        assert_eq!(list.max_name_chars(), 0);
    }

    #[test]
    fn completion_metadata_matches_existing_unicode_scalar_sizing_and_moves_storage() {
        let names: Vec<String> = ["é", "界", "e\u{301}", "topic/界"]
            .into_iter()
            .map(String::from)
            .collect();
        let vector = names.as_ptr();
        let string = names[3].as_ptr();
        let list = BranchCompletions::from(names);
        assert_eq!(list.as_ptr(), vector);
        assert_eq!(list[3].as_ptr(), string);
        assert_eq!(list.max_name_chars(), 7);
        assert_eq!(
            list.max_name_chars(),
            list.iter().map(|name| name.chars().count()).max().unwrap()
        );
        assert_eq!(&*list, ["é", "界", "e\u{301}", "topic/界"]);
    }

    #[test]
    fn repeated_render_snapshots_share_names_and_keep_replacement_epochs_separate() {
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
        let weak = Arc::downgrade(list.0.as_ref().unwrap());
        let mut frame = GitCompletionInfo {
            completions: list.clone(),
            selected_index: Some(0),
        };
        for selected in 0..1000 {
            frame.selected_index = Some(selected);
            // Matches the app-to-renderer snapshot, then renderer dispatch clone.
            let draw = frame.clone();
            assert_eq!(draw.selected_index, Some(selected));
            assert_eq!(draw.completions.as_ptr(), list.as_ptr());
            assert_eq!(draw.completions[selected].as_ptr(), list[selected].as_ptr());
            assert_eq!(draw.completions.max_name_chars(), 256);
            assert_eq!(Arc::strong_count(list.0.as_ref().unwrap()), 3);
        }
        list.clear();
        assert!(list.is_empty());
        assert_eq!(frame.completions.len(), super::super::MAX_BRANCH_NAMES);
        list = vec!["new".into()].into();
        assert_eq!(list.max_name_chars(), 3);
        assert_eq!(frame.completions.max_name_chars(), 256);
        assert_ne!(frame.completions.as_ptr(), list.as_ptr());
        drop(frame);
        assert!(weak.upgrade().is_none());
    }
}
