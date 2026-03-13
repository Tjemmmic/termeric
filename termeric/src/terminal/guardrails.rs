//! Guardrails — detect and intercept destructive commands in plugin tabs
//!
//! Scans PTY input for dangerous patterns (rm -rf, sudo, dd, mkfs, etc.)
//! and triggers a confirmation prompt before allowing the command through.

/// Get the current username
fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// A matched destructive command
#[derive(Debug, Clone)]
pub struct GuardrailMatch {
    /// The command that was matched
    pub command: String,
    /// Human-readable description of the danger
    pub reason: String,
    /// Severity level
    pub severity: Severity,
    /// Where this will execute (e.g., "Local", "SSH: dev01", "docker: devbox")
    pub target: String,
    /// As which user (e.g., "root", "deploy")
    pub user: String,
}

/// Threat severity
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Potentially dangerous (e.g., rm -rf in a specific directory)
    Warning,
    /// Extremely dangerous (e.g., rm -rf /, dd to disk)
    Critical,
}

/// Known destructive command patterns
static PATTERNS: &[(&str, &str, Severity)] = &[
    // Critical: system-level destructive commands
    ("rm -rf /", "Recursive force-delete from root", Severity::Critical),
    ("rm -rf ~", "Recursive force-delete home directory", Severity::Critical),
    ("rm -rf /*", "Recursive force-delete root contents", Severity::Critical),
    ("> /dev/sd", "Overwrite raw disk device", Severity::Critical),
    ("dd if=", "Raw disk write (dd)", Severity::Critical),
    ("mkfs.", "Format filesystem", Severity::Critical),
    ("chmod -R 777 /", "Open all permissions from root", Severity::Critical),
    (":(){ :|:& };:", "Fork bomb", Severity::Critical),

    // Warning: potentially dangerous commands
    ("rm -rf", "Recursive force-delete", Severity::Warning),
    ("sudo rm", "Deleting files as root", Severity::Warning),
    ("chmod -R 777", "Opening all permissions recursively", Severity::Warning),
    ("chown -R", "Changing ownership recursively", Severity::Warning),
    ("> /dev/null", "Redirecting to /dev/null", Severity::Warning),
    ("truncate -s 0", "Truncating file to zero", Severity::Warning),
];

/// Check an input line against destructive patterns.
///
/// Returns the first (most severe) match, or None if the command is safe.
pub fn check_command(input: &str) -> Option<GuardrailMatch> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_lowercase();

    // Check critical patterns first, then warnings
    let mut best_match: Option<GuardrailMatch> = None;

    for &(pattern, reason, severity) in PATTERNS {
        if lower.contains(pattern) {
            let is_better = match (&best_match, severity) {
                (None, _) => true,
                (Some(existing), Severity::Critical) if existing.severity != Severity::Critical => true,
                _ => false,
            };
            if is_better {
                best_match = Some(GuardrailMatch {
                    command: trimmed.to_string(),
                    reason: reason.to_string(),
                    severity,
                    target: "Local".to_string(),
                    user: whoami(),
                });
            }
        }
    }

    best_match
}

/// Guardrail confirmation overlay state
pub struct GuardrailOverlay {
    /// Animation state
    animation: crate::ui::animation::OverlayAnimation,
    /// The matched command that triggered the guardrail
    pub matched: Option<GuardrailMatch>,
    /// The raw bytes to send to PTY if confirmed
    pub pending_data: Vec<u8>,
}

impl GuardrailOverlay {
    pub fn new() -> Self {
        Self {
            animation: crate::ui::animation::OverlayAnimation::new(),
            matched: None,
            pending_data: Vec::new(),
        }
    }

    /// Show the guardrail confirmation for a matched command
    pub fn show(&mut self, matched: GuardrailMatch, pending_data: Vec<u8>) {
        self.matched = Some(matched);
        self.pending_data = pending_data;
        self.animation.open();
    }

    /// Dismiss without executing
    pub fn dismiss(&mut self) {
        self.animation.close();
    }

    /// Confirm and return the pending data to send
    pub fn confirm(&mut self) -> Vec<u8> {
        let data = std::mem::take(&mut self.pending_data);
        self.animation.close();
        data
    }

    /// Whether the overlay is active
    pub fn is_active(&self) -> bool {
        self.animation.is_active()
    }

    /// Whether the overlay is visible
    pub fn is_visible(&self) -> bool {
        self.animation.is_visible()
    }

    /// Get animation progress
    pub fn animation(&self) -> f32 {
        self.animation.progress()
    }

    /// Update animation state
    pub fn update_animation(&mut self) -> bool {
        let still_animating = self.animation.update();
        if !self.animation.is_active() && !self.animation.is_visible() {
            self.matched = None;
        }
        still_animating
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_match() {
        assert!(check_command("ls -la").is_none());
        assert!(check_command("cargo build").is_none());
        assert!(check_command("git commit -m 'test'").is_none());
    }

    #[test]
    fn test_critical_rm_rf_root() {
        let m = check_command("sudo rm -rf /").unwrap();
        assert_eq!(m.severity, Severity::Critical);
        assert!(m.reason.contains("root"));
    }

    #[test]
    fn test_critical_rm_rf_home() {
        let m = check_command("rm -rf ~").unwrap();
        assert_eq!(m.severity, Severity::Critical);
    }

    #[test]
    fn test_warning_rm_rf() {
        let m = check_command("rm -rf ./build").unwrap();
        assert_eq!(m.severity, Severity::Warning);
    }

    #[test]
    fn test_critical_dd() {
        let m = check_command("dd if=/dev/zero of=/dev/sda").unwrap();
        assert_eq!(m.severity, Severity::Critical);
    }

    #[test]
    fn test_critical_mkfs() {
        let m = check_command("mkfs.ext4 /dev/sda1").unwrap();
        assert_eq!(m.severity, Severity::Critical);
    }

    #[test]
    fn test_warning_sudo_rm() {
        let m = check_command("sudo rm -r /tmp/stuff").unwrap();
        assert_eq!(m.severity, Severity::Warning);
    }

    #[test]
    fn test_empty_input() {
        assert!(check_command("").is_none());
        assert!(check_command("   ").is_none());
    }

    #[test]
    fn test_case_insensitive() {
        // Patterns are checked lowercase
        let m = check_command("RM -RF /").unwrap();
        assert_eq!(m.severity, Severity::Critical);
    }

    #[test]
    fn test_fork_bomb() {
        let m = check_command(":(){ :|:& };:").unwrap();
        assert_eq!(m.severity, Severity::Critical);
    }
}
