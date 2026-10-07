//! Application state persistence
//!
//! Stores runtime state separate from user configuration.
//! State is stored in XDG_DATA_HOME (~/.local/share/termeric/).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// More than the state ever holds; a larger file is not this state.
const MAX_STATE_BYTES: usize = 64 * 1024;

#[cfg(test)]
thread_local! {
    static TEST_STATE_PATH: std::cell::RefCell<Option<PathBuf>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Application runtime state
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppState {
    /// Whether the first boot splash has been shown
    #[serde(default)]
    pub first_boot_complete: bool,
    /// Workspaces to relaunch on automatic restore, one per window, the one
    /// saved or restored last at the end. Startup never guesses from file
    /// timestamps.
    #[serde(default)]
    pub last_workspace_ids: Vec<String>,
}

impl AppState {
    /// Load state from file or return default
    pub fn load() -> Self {
        let Some(state_path) = Self::state_path() else {
            return Self::default();
        };
        let content = match crate::safe_fs::read_text(&state_path, MAX_STATE_BYTES) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                log::warn!("Failed to read state file: {error}");
                return Self::default();
            }
        };
        serde_json::from_str(&content).unwrap_or_else(|error| {
            log::warn!("Failed to parse state file: {error}");
            Self::default()
        })
    }

    /// Save state to file; without a data directory it is not kept.
    pub fn save(&self) -> Result<()> {
        let state_path = Self::state_path().context(crate::paths::NO_HOME_DIRECTORY)?;

        // Ensure state directory exists
        if let Some(parent) = state_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_vec_pretty(self)?;
        crate::safe_fs::atomic_replace(&state_path, |file| file.write_all(&content))?;

        log::debug!("Saved state to {}", state_path.display());
        Ok(())
    }

    /// The state file (XDG_DATA_HOME/termeric/state.json), `None` without a
    /// data directory.
    pub fn state_path() -> Option<PathBuf> {
        #[cfg(test)]
        if let Some(path) = TEST_STATE_PATH.with(|state| state.borrow().clone()) {
            return Some(path);
        }
        crate::paths::data_dir().map(|dir| dir.join("state.json"))
    }

    #[cfg(test)]
    pub(crate) fn set_test_state_path(path: Option<PathBuf>) {
        TEST_STATE_PATH.with(|state| *state.borrow_mut() = path);
    }

    /// Mark first boot as complete and save
    pub fn mark_first_boot_complete(&mut self) -> Result<()> {
        self.first_boot_complete = true;
        self.save()
    }

    pub fn remember_workspace(&mut self, id: &str) -> Result<()> {
        let mut latest = Self::load();
        latest.first_boot_complete |= self.first_boot_complete;
        latest
            .last_workspace_ids
            .retain(|candidate| candidate != id);
        latest.last_workspace_ids.push(id.to_string());
        latest.save()?;
        *self = latest;
        Ok(())
    }

    pub fn forget_workspace(&mut self, id: &str) -> Result<()> {
        let mut latest = Self::load();
        latest.first_boot_complete |= self.first_boot_complete;
        latest
            .last_workspace_ids
            .retain(|candidate| candidate != id);
        latest.save()?;
        *self = latest;
        Ok(())
    }

    /// The workspace saved or restored last.
    pub fn last_workspace_id(&self) -> Option<&str> {
        self.last_workspace_ids.last().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_is_saved_over_a_symlink_and_never_read_unbounded() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.json");
        AppState::set_test_state_path(Some(path.clone()));
        let result = std::panic::catch_unwind(|| {
            let target = temp.path().join("target");
            std::fs::write(&target, "unchanged").unwrap();
            std::os::unix::fs::symlink(&target, &path).unwrap();
            // A symlink is neither followed when loading nor written through.
            assert!(AppState::load().last_workspace_ids.is_empty());
            let state = AppState {
                first_boot_complete: true,
                ..AppState::default()
            };
            state.save().unwrap();
            assert!(std::fs::symlink_metadata(&path).unwrap().is_file());
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "unchanged");
            assert!(AppState::load().first_boot_complete);

            std::fs::write(&path, " ".repeat(MAX_STATE_BYTES + 1)).unwrap();
            assert!(!AppState::load().first_boot_complete);
        });
        AppState::set_test_state_path(None);
        result.unwrap();
    }

    #[test]
    fn concurrent_window_state_merges_and_forgets_workspace_ids() {
        let temp = tempfile::tempdir().unwrap();
        AppState::set_test_state_path(Some(temp.path().join("state/state.json")));
        let result = std::panic::catch_unwind(|| {
            let mut first = AppState::default();
            let mut second = AppState::default();
            first.remember_workspace("workspace-a").unwrap();
            second.remember_workspace("workspace-b").unwrap();
            assert_eq!(
                AppState::load().last_workspace_ids,
                ["workspace-a", "workspace-b"]
            );
            first.forget_workspace("workspace-a").unwrap();
            let loaded = AppState::load();
            assert_eq!(loaded.last_workspace_ids, ["workspace-b"]);
            assert_eq!(loaded.last_workspace_id(), Some("workspace-b"));
        });
        AppState::set_test_state_path(None);
        result.unwrap();
    }

    #[test]
    fn without_a_data_directory_state_is_neither_read_nor_kept() {
        crate::paths::without_home(|| {
            assert!(!AppState::load().first_boot_complete);
            let mut state = AppState::default();
            assert!(state.mark_first_boot_complete().is_err());
            assert!(state.remember_workspace("workspace-a").is_err());
        });
    }
}
