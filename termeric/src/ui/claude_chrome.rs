//! Claude session chrome — status bar UI displayed below the tab bar for Claude Code tabs
//!
//! Shows: repo name · branch · host badge · [Context] button · transcript indicator


/// Host where the Claude Code session is running
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostTarget {
    /// Running on the local machine
    Local,
    /// Running over SSH
    Ssh { host: String, user: Option<String> },
    /// Running inside a container
    Container { name: String, runtime: String },
}

impl HostTarget {
    /// Short display label for the host badge
    pub fn badge_label(&self) -> String {
        match self {
            HostTarget::Local => "Local".to_string(),
            HostTarget::Ssh { host, user } => {
                if let Some(u) = user {
                    format!("{}@{}", u, host)
                } else {
                    format!("SSH: {}", host)
                }
            }
            HostTarget::Container { name, runtime } => format!("{}: {}", runtime, name),
        }
    }
}

/// Persistent state for a Claude Code session tab
#[derive(Debug, Clone)]
pub struct ClaudeSessionState {
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

impl Default for ClaudeSessionState {
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

impl ClaudeSessionState {
    /// Update git-related fields from a branch name, dirty status, and working dir
    pub fn update_git(&mut self, branch: Option<&str>, dirty: bool, cwd: Option<&str>) {
        self.branch = branch.map(|s| s.to_string());
        self.dirty = dirty;

        // Extract repo name from cwd (last path component)
        if self.repo_name.is_none() {
            if let Some(dir) = cwd {
                if let Some(name) = std::path::Path::new(dir).file_name() {
                    self.repo_name = Some(name.to_string_lossy().to_string());
                }
            }
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
    /// Host badge label
    pub host_label: String,
    /// Profile name
    pub profile: String,
    /// Whether transcript is recording
    pub recording: bool,
    /// Height of the chrome bar in pixels (set by renderer)
    pub height: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_target_labels() {
        assert_eq!(HostTarget::Local.badge_label(), "Local");
        assert_eq!(
            HostTarget::Ssh { host: "dev01".into(), user: Some("root".into()) }.badge_label(),
            "root@dev01"
        );
        assert_eq!(
            HostTarget::Ssh { host: "prod".into(), user: None }.badge_label(),
            "SSH: prod"
        );
        assert_eq!(
            HostTarget::Container { name: "devbox".into(), runtime: "docker".into() }.badge_label(),
            "docker: devbox"
        );
    }

    #[test]
    fn test_session_state_defaults() {
        let state = ClaudeSessionState::default();
        assert_eq!(state.profile, "default");
        assert_eq!(state.host, HostTarget::Local);
        assert!(!state.recording);
        assert!(state.repo_name.is_none());
    }

    #[test]
    fn test_update_git() {
        let mut state = ClaudeSessionState::default();
        state.update_git(Some("main"), true, Some("/home/user/projects/termeric"));
        assert_eq!(state.branch.as_deref(), Some("main"));
        assert!(state.dirty);
        assert_eq!(state.repo_name.as_deref(), Some("termeric"));
    }

    #[test]
    fn test_toggle_recording() {
        let mut state = ClaudeSessionState::default();
        assert!(!state.recording);
        state.toggle_recording();
        assert!(state.recording);
        state.toggle_recording();
        assert!(!state.recording);
    }
}
