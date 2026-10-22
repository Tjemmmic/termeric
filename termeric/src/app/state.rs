//! Application state persistence
//!
//! Stores runtime state separate from user configuration.
//! State is stored in XDG_DATA_HOME (~/.local/share/termeric/).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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

/// The workspaces the sessions before this process left for restoring, read
/// when it started and shared by its windows. The first window the process
/// keeps for restoring takes them and drops them from the list, so the list
/// names only the windows open at the latest exit, never every earlier one.
/// A Termeric running beside this one adds its windows after this process
/// started, so they are not among these and stay.
#[derive(Debug, Clone, Default)]
pub struct EarlierSessions(Arc<Mutex<Option<Vec<String>>>>);

impl EarlierSessions {
    /// What `state`, read at startup, says the earlier sessions left.
    pub fn from_startup(state: &AppState) -> Self {
        Self(Arc::new(Mutex::new(Some(state.last_workspace_ids.clone()))))
    }

    /// The earlier sessions' workspaces, the first time only.
    pub fn take(&self) -> Vec<String> {
        self.0
            .lock()
            .ok()
            .and_then(|mut earlier| earlier.take())
            .unwrap_or_default()
    }
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

    /// Make `id` the workspace restored last, dropping `ended`, the
    /// workspaces of sessions that ended before this one began.
    pub fn remember_workspace(&mut self, id: &str, ended: &[String]) -> Result<()> {
        self.update_workspace_ids(|ids| {
            ids.retain(|candidate| candidate != id && !ended.contains(candidate));
            ids.push(id.to_string());
        })
    }

    pub fn forget_workspace(&mut self, id: &str) -> Result<()> {
        self.update_workspace_ids(|ids| ids.retain(|candidate| candidate != id))
    }

    /// Apply `update` to the workspace list on disk and save it. Other
    /// windows of this and other processes write the same file, so the
    /// change goes onto what the file holds now, not this window's copy.
    fn update_workspace_ids(&mut self, update: impl FnOnce(&mut Vec<String>)) -> Result<()> {
        let mut latest = Self::load();
        latest.first_boot_complete |= self.first_boot_complete;
        update(&mut latest.last_workspace_ids);
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
    fn a_session_replaces_the_restore_list_the_ended_ones_left() {
        let temp = tempfile::tempdir().unwrap();
        AppState::set_test_state_path(Some(temp.path().join("state.json")));
        let result = std::panic::catch_unwind(|| {
            // Two earlier sessions each left their last window.
            let mut earlier = AppState::default();
            earlier.remember_workspace("ended-a", &[]).unwrap();
            earlier.remember_workspace("ended-b", &[]).unwrap();
            // This process starts; a Termeric started after it keeps its
            // own window, which this process must not drop.
            let ended = EarlierSessions::from_startup(&AppState::load());
            AppState::load().remember_workspace("beside", &[]).unwrap();
            // The first window this process keeps drops the ended sessions'
            // windows; its next one keeps the first.
            let mut window = AppState::load();
            window.remember_workspace("this-1", &ended.take()).unwrap();
            window.remember_workspace("this-2", &ended.take()).unwrap();
            assert_eq!(
                AppState::load().last_workspace_ids,
                ["beside", "this-1", "this-2"]
            );
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
            first.remember_workspace("workspace-a", &[]).unwrap();
            second.remember_workspace("workspace-b", &[]).unwrap();
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
            assert!(state.remember_workspace("workspace-a", &[]).is_err());
        });
    }
}
