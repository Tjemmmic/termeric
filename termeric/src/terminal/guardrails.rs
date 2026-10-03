//! Guardrails — detect and intercept destructive terminal commands
//!
//! Scans automated terminal input — text sent through the GUI control
//! protocol or by runtime plugins — for dangerous patterns (rm -rf, dd, mkfs,
//! etc.) and asks the person to confirm before it is delivered. Input a person
//! types, pastes, completes, or broadcasts is never intercepted.

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardrailAction {
    Confirm { request_id: u64 },
    Cancel { request_id: u64 },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuardedWrite {
    pub(crate) tab_id: usize,
    pub(crate) pane_id: crate::panes::PaneId,
    pub(crate) data: Vec<u8>,
}

impl GuardedWrite {
    /// Capture identity, never the current single/split ownership mode. A tab
    /// route alone can alias a different survivor by the time consent arrives.
    pub(crate) fn for_pane(tab_id: usize, pane_id: crate::panes::PaneId, data: Vec<u8>) -> Self {
        Self {
            tab_id,
            pane_id,
            data,
        }
    }

    pub(crate) fn resolve_origin(
        &self,
        tabs: &crate::tabs::TabManager,
    ) -> Option<crate::tabs::TerminalEventOrigin> {
        tabs.origin_for_pane(self.tab_id, self.pane_id)
    }
}

#[derive(Debug)]
struct PendingGuardedInput {
    request_id: u64,
    writes: Vec<GuardedWrite>,
}

/// Where a pattern may match.
#[derive(Clone, Copy)]
enum Extent {
    /// Anywhere in the input.
    Anywhere,
    /// Only where its last argument ends: `rm -rf /` matches `rm -rf /` and
    /// `rm -rf /*`, but not `rm -rf /tmp/build`.
    WholeArgument,
}

/// Known destructive command patterns, lowercase because input is lowercased
/// before matching.
static PATTERNS: &[(&str, &str, Severity, Extent)] = &[
    // Critical: system-level destructive commands
    (
        "rm -rf /",
        "Recursive force-delete from root",
        Severity::Critical,
        Extent::WholeArgument,
    ),
    (
        "rm -fr /",
        "Recursive force-delete from root",
        Severity::Critical,
        Extent::WholeArgument,
    ),
    (
        "rm -rf ~",
        "Recursive force-delete home directory",
        Severity::Critical,
        Extent::WholeArgument,
    ),
    (
        "rm -rf ~/",
        "Recursive force-delete home directory",
        Severity::Critical,
        Extent::WholeArgument,
    ),
    (
        "rm -fr ~",
        "Recursive force-delete home directory",
        Severity::Critical,
        Extent::WholeArgument,
    ),
    (
        "rm -fr ~/",
        "Recursive force-delete home directory",
        Severity::Critical,
        Extent::WholeArgument,
    ),
    (
        "> /dev/sd",
        "Overwrite raw disk device",
        Severity::Critical,
        Extent::Anywhere,
    ),
    (
        "dd if=",
        "Raw disk write (dd)",
        Severity::Critical,
        Extent::Anywhere,
    ),
    (
        "mkfs.",
        "Format filesystem",
        Severity::Critical,
        Extent::Anywhere,
    ),
    (
        "chmod -r 777 /",
        "Open all permissions from root",
        Severity::Critical,
        Extent::WholeArgument,
    ),
    (
        ":(){ :|:& };:",
        "Fork bomb",
        Severity::Critical,
        Extent::Anywhere,
    ),
    // Warning: potentially dangerous commands
    (
        "rm -rf",
        "Recursive force-delete",
        Severity::Warning,
        Extent::Anywhere,
    ),
    (
        "rm -fr",
        "Recursive force-delete",
        Severity::Warning,
        Extent::Anywhere,
    ),
    (
        "sudo rm",
        "Deleting files as root",
        Severity::Warning,
        Extent::Anywhere,
    ),
    (
        "chmod -r 777",
        "Opening all permissions recursively",
        Severity::Warning,
        Extent::Anywhere,
    ),
    (
        "chown -r",
        "Changing ownership recursively",
        Severity::Warning,
        Extent::Anywhere,
    ),
    (
        "truncate -s 0",
        "Truncating file to zero",
        Severity::Warning,
        Extent::Anywhere,
    ),
];

/// Whether `pattern` occurs in `input` within its allowed extent.
fn matches_pattern(input: &str, pattern: &str, extent: Extent) -> bool {
    match extent {
        Extent::Anywhere => input.contains(pattern),
        Extent::WholeArgument => input.match_indices(pattern).any(|(start, _)| {
            let rest = &input[start + pattern.len()..];
            let rest = rest.strip_prefix('*').unwrap_or(rest);
            rest.chars()
                .next()
                .is_none_or(|next| next.is_whitespace() || matches!(next, ';' | '&' | '|' | ')'))
        }),
    }
}

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

    for &(pattern, reason, severity, extent) in PATTERNS {
        if matches_pattern(&lower, pattern, extent) {
            let is_better = match (&best_match, severity) {
                (None, _) => true,
                (Some(existing), Severity::Critical) if existing.severity != Severity::Critical => {
                    true
                }
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
    pending: Option<PendingGuardedInput>,
    request_id: u64,
}

impl GuardrailOverlay {
    pub fn new() -> Self {
        Self {
            animation: crate::ui::animation::OverlayAnimation::with_duration(0.25),
            matched: None,
            pending: None,
            request_id: 0,
        }
    }

    /// Show the guardrail confirmation for a matched command
    pub fn show(&mut self, matched: GuardrailMatch, writes: Vec<GuardedWrite>) -> u64 {
        assert!(
            !writes.is_empty(),
            "a guarded request must own at least one terminal write"
        );
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.matched = Some(matched);
        self.pending = Some(PendingGuardedInput {
            request_id: self.request_id,
            writes,
        });
        self.animation.open();
        self.request_id
    }

    /// Dismiss without executing
    pub fn dismiss(&mut self, request_id: u64) -> bool {
        if !self.is_awaiting(request_id) {
            return false;
        }
        // Revoke authority immediately. OverlayAnimation remains active during
        // its exit animation, so retaining bytes here creates an
        // Escape-then-Enter execution race.
        self.pending = None;
        self.animation.close();
        true
    }

    /// Confirm and return the exact pending write batch.
    pub fn confirm(&mut self, request_id: u64) -> Option<Vec<GuardedWrite>> {
        if !self.is_awaiting(request_id) {
            return None;
        }
        let pending = self.pending.take()?;
        self.animation.close();
        Some(pending.writes)
    }

    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    pub fn is_awaiting(&self, request_id: u64) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.request_id == request_id)
    }

    pub fn is_awaiting_current(&self) -> bool {
        self.is_awaiting(self.request_id)
    }

    /// Whether the overlay is active
    pub fn is_active(&self) -> bool {
        self.animation.is_active()
    }

    /// Whether the overlay is visible
    pub fn is_visible(&self) -> bool {
        self.animation.is_visible()
    }

    /// Get eased animation progress
    pub fn animation(&self) -> f32 {
        self.animation.eased()
    }

    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.animation.set_reduced_motion(reduced_motion);
    }

    /// Update animation state
    pub fn update_animation(&mut self) -> bool {
        let still_animating = self.animation.update();
        if !self.animation.is_active() && !self.animation.is_visible() {
            self.matched = None;
            self.pending = None;
        }
        still_animating
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_match() -> GuardrailMatch {
        GuardrailMatch {
            command: "fixture command; never execute".into(),
            reason: "Hermetic guardrail fixture".into(),
            severity: Severity::Critical,
            target: "Fixture terminal".into(),
            user: "tester".into(),
        }
    }

    fn fixture_write(data: &[u8]) -> GuardedWrite {
        GuardedWrite::for_pane(42, crate::panes::PaneId(0), data.to_vec())
    }

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
    fn every_pattern_is_lowercase_like_the_input_it_is_matched_against() {
        for &(pattern, ..) in PATTERNS {
            assert_eq!(pattern, pattern.to_lowercase(), "{pattern}");
        }
    }

    #[test]
    fn recursive_chmod_and_chown_match_in_either_case() {
        let m = check_command("chmod -R 777 /").unwrap();
        assert_eq!(m.severity, Severity::Critical);
        let m = check_command("chmod -R 777 ./site").unwrap();
        assert_eq!(m.severity, Severity::Warning);
        let m = check_command("chown -R user:group /srv").unwrap();
        assert_eq!(m.severity, Severity::Warning);
    }

    #[test]
    fn root_and_home_deletion_match_only_whole_arguments() {
        for command in [
            "rm -rf /",
            "rm -rf /*",
            "rm -fr /",
            "rm -rf / && echo done",
            "rm -rf ~",
            "rm -rf ~/",
            "rm -rf ~/*",
            "rm -rf ~;ls",
            "rm -rf /tmp/build; rm -rf /",
        ] {
            let m = check_command(command).unwrap();
            assert_eq!(m.severity, Severity::Critical, "{command}");
        }
        for command in [
            "rm -rf /tmp/build",
            "rm -rf ~/projects/old",
            "rm -fr ./target",
        ] {
            let m = check_command(command).unwrap();
            assert_eq!(m.severity, Severity::Warning, "{command}");
        }
    }

    #[test]
    fn discarding_output_is_not_flagged() {
        assert!(check_command("make > /dev/null").is_none());
        assert!(check_command("command -v git >/dev/null 2>&1").is_none());
    }

    #[test]
    fn test_fork_bomb() {
        let m = check_command(":(){ :|:& };:").unwrap();
        assert_eq!(m.severity, Severity::Critical);
    }

    #[test]
    fn dismiss_revokes_pending_bytes_before_the_exit_animation_finishes() {
        let mut overlay = GuardrailOverlay::new();
        let request = overlay.show(fixture_match(), vec![fixture_write(b"fixture-enter")]);
        assert!(overlay.dismiss(request));
        assert!(
            overlay.is_active(),
            "the closing animation still owns input"
        );
        assert!(!overlay.is_awaiting(request));
        assert_eq!(overlay.confirm(request), None);
    }

    #[test]
    fn stale_request_cannot_confirm_a_newer_guarded_input() {
        let mut overlay = GuardrailOverlay::new();
        let stale = overlay.show(fixture_match(), vec![fixture_write(b"first")]);
        assert!(overlay.dismiss(stale));
        let current = overlay.show(fixture_match(), vec![fixture_write(b"second")]);
        assert_ne!(stale, current);
        assert_eq!(overlay.confirm(stale), None);
        assert!(overlay.is_awaiting(current));
        assert!(overlay.dismiss(current));
    }

    #[test]
    fn confirmation_returns_the_exact_origin_once_without_writing_anywhere() {
        let mut overlay = GuardrailOverlay::new();
        let write = fixture_write(b"fixture-enter");
        let request = overlay.show(fixture_match(), vec![write.clone()]);
        assert_eq!(overlay.confirm(request), Some(vec![write]));
        assert_eq!(overlay.confirm(request), None);
    }

    #[test]
    fn confirmation_preserves_an_exact_multi_destination_write_batch() {
        let mut overlay = GuardrailOverlay::new();
        let writes = vec![
            GuardedWrite::for_pane(9, crate::panes::PaneId(2), b"first-encoding".to_vec()),
            GuardedWrite::for_pane(9, crate::panes::PaneId(7), b"second-encoding".to_vec()),
        ];
        let request = overlay.show(fixture_match(), writes.clone());
        assert_eq!(overlay.confirm(request), Some(writes));
    }
}
