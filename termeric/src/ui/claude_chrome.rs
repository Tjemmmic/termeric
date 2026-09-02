//! AI session chrome — status bar UI displayed below the tab bar for AI plugin tabs
//!
//! Shows: repo name · branch · host badge · `[Context]` button · transcript indicator

/// Host where the Claude Code session is running
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostTarget {
    /// Running on the local machine
    Local,
}

impl HostTarget {
    /// Short display label for the host badge
    pub fn badge_label(&self) -> String {
        match self {
            HostTarget::Local => "Local".to_string(),
        }
    }
}

/// Persistent state for an AI plugin session tab (Claude Code, Codex, etc.)
#[derive(Debug, Clone)]
pub struct AiSessionState {
    /// Currently active profile name
    pub profile: String,
    /// Where this session is running
    pub host: HostTarget,
    /// Whether transcript recording is active
    pub recording: bool,
    /// Detected repo name (last component of git root)
    pub repo_name: Option<String>,
    /// Current git branch
    pub branch: Option<String>,
    /// Whether the working tree is dirty
    pub dirty: bool,
}

impl Default for AiSessionState {
    fn default() -> Self {
        Self {
            profile: "default".to_string(),
            host: HostTarget::Local,
            recording: false,
            repo_name: None,
            branch: None,
            dirty: false,
        }
    }
}

impl AiSessionState {
    /// Update git-related fields from a branch name, dirty status, and working dir
    pub fn update_git(&mut self, branch: Option<&str>, dirty: bool, cwd: Option<&str>) {
        self.branch = branch.map(|s| s.to_string());
        self.dirty = dirty;

        // Extract repo name from cwd (last path component)
        if self.repo_name.is_none()
            && let Some(dir) = cwd
            && let Some(name) = std::path::Path::new(dir).file_name()
        {
            self.repo_name = Some(name.to_string_lossy().to_string());
        }
    }

    /// Toggle transcript recording
    pub fn toggle_recording(&mut self) {
        self.recording = !self.recording;
    }
}

/// Chrome bar render info (passed to renderer)
#[derive(Debug, Clone)]
pub struct ChromeBarInfo {
    /// Repo name (e.g., "termeric")
    pub repo_name: Option<String>,
    /// Branch name (e.g., "main")
    pub branch: Option<String>,
    /// Whether working tree is dirty
    pub dirty: bool,
    /// Commits ahead of upstream
    pub ahead: usize,
    /// Commits behind upstream
    pub behind: usize,
    /// Host badge label
    pub host_label: String,
    /// Profile name
    pub profile: String,
    /// Whether transcript is recording
    pub recording: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_target_labels() {
        assert_eq!(HostTarget::Local.badge_label(), "Local");
    }

    #[test]
    fn test_session_state_defaults() {
        let state = AiSessionState::default();
        assert_eq!(state.profile, "default");
        assert_eq!(state.host, HostTarget::Local);
        assert!(!state.recording);
        assert!(state.repo_name.is_none());
    }

    #[test]
    fn test_update_git() {
        let mut state = AiSessionState::default();
        state.update_git(Some("main"), true, Some("/home/user/projects/termeric"));
        assert_eq!(state.branch.as_deref(), Some("main"));
        assert!(state.dirty);
        assert_eq!(state.repo_name.as_deref(), Some("termeric"));
    }

    #[test]
    fn test_toggle_recording() {
        let mut state = AiSessionState::default();
        assert!(!state.recording);
        state.toggle_recording();
        assert!(state.recording);
        state.toggle_recording();
        assert!(!state.recording);
    }
}
