//! Application state persistence
//!
//! Stores runtime state separate from user configuration.
//! State is stored in XDG_DATA_HOME (~/.local/share/termeric/).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Application runtime state
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppState {
    /// Whether the first boot splash has been shown
    #[serde(default)]
    pub first_boot_complete: bool,
}

impl AppState {
    /// Load state from file or return default
    pub fn load() -> Self {
        let state_path = Self::state_path();

        if state_path.exists() {
            match std::fs::read_to_string(&state_path) {
                Ok(content) => {
                    match serde_json::from_str(&content) {
                        Ok(state) => return state,
                        Err(e) => {
                            log::warn!("Failed to parse state file: {}", e);
                        }
                    }
                }
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

        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&state_path, content)?;

        log::debug!("Saved state to {}", state_path.display());
        Ok(())
    }

    /// Get the state file path (XDG_DATA_HOME/termeric/state.json)
    pub fn state_path() -> PathBuf {
        if let Some(dirs) = directories::ProjectDirs::from("", "", "termeric") {
            dirs.data_dir().join("state.json")
        } else {
            PathBuf::from("state.json")
        }
    }

    /// Mark first boot as complete and save
    pub fn mark_first_boot_complete(&mut self) -> Result<()> {
        self.first_boot_complete = true;
        self.save()
    }
}
