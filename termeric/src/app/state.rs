//! Application state persistence
//!
//! Stores runtime state separate from user configuration.
//! State is stored in XDG_DATA_HOME (~/.local/share/termeric/).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

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
    /// Stable ID of the last workspace explicitly saved or restored. Startup
    /// recovery uses this reference; it never guesses from file timestamps.
    #[serde(default)]
    pub last_workspace_id: Option<String>,
    /// Complete set of windows to relaunch on automatic multi-window restore.
    #[serde(default)]
    pub last_workspace_ids: Vec<String>,
}

impl AppState {
    /// Load state from file or return default
    pub fn load() -> Self {
        let state_path = Self::state_path();

        if state_path.exists() {
            match std::fs::read_to_string(&state_path) {
                Ok(content) => match serde_json::from_str(&content) {
                    Ok(state) => return state,
                    Err(e) => {
                        log::warn!("Failed to parse state file: {}", e);
                    }
                },
                Err(e) => {
                    log::warn!("Failed to read state file: {}", e);
                }
            }
        }

        Self::default()
    }

    /// Save state to file
    pub fn save(&self) -> Result<()> {
        let state_path = Self::state_path();

        // Ensure state directory exists
        if let Some(parent) = state_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_vec_pretty(self)?;
        let temp_path = state_path.with_extension("json.tmp");
        let mut file = std::fs::File::create(&temp_path)?;
        file.write_all(&content)?;
        file.sync_all()?;
        std::fs::rename(&temp_path, &state_path)?;
        if let Some(parent) = state_path.parent()
            && let Ok(directory) = std::fs::File::open(parent)
        {
            let _ = directory.sync_all();
        }

        log::debug!("Saved state to {}", state_path.display());
        Ok(())
    }

    /// Get the state file path (XDG_DATA_HOME/termeric/state.json)
    pub fn state_path() -> PathBuf {
        #[cfg(test)]
        if let Some(path) = TEST_STATE_PATH.with(|state| state.borrow().clone()) {
            return path;
        }
        if let Some(dirs) = directories::ProjectDirs::from("", "", "termeric") {
            dirs.data_dir().join("state.json")
        } else {
            PathBuf::from("state.json")
        }
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
        latest.last_workspace_id = Some(id.to_string());
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
        if latest.last_workspace_id.as_deref() == Some(id) {
            latest.last_workspace_id = latest.last_workspace_ids.last().cloned();
        }
        latest.save()?;
        *self = latest;
        Ok(())
    }

    pub fn restore_workspace_ids(&self) -> Vec<String> {
        if self.last_workspace_ids.is_empty() {
            self.last_workspace_id.iter().cloned().collect()
        } else {
            self.last_workspace_ids.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                AppState::load().restore_workspace_ids(),
                ["workspace-a", "workspace-b"]
            );
            first.forget_workspace("workspace-a").unwrap();
            let loaded = AppState::load();
            assert_eq!(loaded.restore_workspace_ids(), ["workspace-b"]);
            assert_eq!(loaded.last_workspace_id.as_deref(), Some("workspace-b"));
        });
        AppState::set_test_state_path(None);
        result.unwrap();
    }

    #[test]
    fn legacy_single_workspace_reference_migrates_without_rewrite() {
        let state = AppState {
            last_workspace_id: Some("workspace-legacy".into()),
            ..AppState::default()
        };
        assert_eq!(state.restore_workspace_ids(), ["workspace-legacy"]);
    }
}
