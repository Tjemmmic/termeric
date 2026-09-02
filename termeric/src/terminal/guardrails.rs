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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardrailAction {
    Confirm { request_id: u64 },
    Cancel { request_id: u64 },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuardedWrite {
    pub(crate) origin: crate::tabs::TerminalEventOrigin,
    pub(crate) data: Vec<u8>,
}

impl GuardedWrite {
    pub(crate) fn new(origin: crate::tabs::TerminalEventOrigin, data: Vec<u8>) -> Self {
        Self { origin, data }
    }
}

#[derive(Debug)]
struct PendingGuardedInput {
    request_id: u64,
    writes: Vec<GuardedWrite>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuardrailRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl GuardrailRect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }
}

/// Guardrail modal geometry shared by rendering, pointer hit testing, and
/// accessibility. Confirm and Cancel are deliberately separate shapes so the
/// safety decision never depends on color alone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuardrailLayout {
    pub card: GuardrailRect,
    pub confirm_button: GuardrailRect,
    pub cancel_button: GuardrailRect,
    pub close_button: GuardrailRect,
    pub padding: f32,
    pub title_height: f32,
    pub command_height: f32,
    pub reason_height: f32,
    pub context_height: f32,
    pub footer_height: f32,
}

impl GuardrailLayout {
    pub fn calculate(
        window_width: f32,
        window_height: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
    ) -> Self {
        let padding = cell_width * 2.0;
        let title_height = cell_height * 2.5;
        let command_height = cell_height * 2.0;
        let reason_height = cell_height * 1.8;
        let context_height = cell_height * 1.5;
        let footer_height = cell_height * 2.6;
        let card_height = padding
            + title_height
            + command_height
            + reason_height
            + context_height
            + footer_height
            + padding;
        let card_width = (cell_width * 50.0).max(400.0).min(window_width * 0.7);
        let card_x = (window_width - card_width) / 2.0;
        let card_y = (window_height - card_height) / 2.0 + (1.0 - animation) * -20.0;
        let button_gap = cell_width;
        let button_height = cell_height * 1.7;
        let available_button_width = card_width - padding * 2.0 - button_gap;
        let button_width = (available_button_width / 2.0).max(1.0);
        let button_y = card_y + card_height - padding - button_height;
        let cancel_button = GuardrailRect {
            x: card_x + padding,
            y: button_y,
            width: button_width,
            height: button_height,
        };
        let confirm_button = GuardrailRect {
            x: cancel_button.x + button_width + button_gap,
            y: button_y,
            width: button_width,
            height: button_height,
        };
        let close_size = cell_height * 1.5;
        Self {
            card: GuardrailRect {
                x: card_x,
                y: card_y,
                width: card_width,
                height: card_height,
            },
            confirm_button,
            cancel_button,
            close_button: GuardrailRect {
                x: card_x + card_width - padding - close_size / 2.0,
                y: card_y + padding - close_size / 2.0,
                width: close_size,
                height: close_size,
            },
            padding,
            title_height,
            command_height,
            reason_height,
            context_height,
            footer_height,
        }
    }

    pub fn hit_test(&self, x: f32, y: f32, request_id: u64) -> GuardrailAction {
        if self.confirm_button.contains(x, y) {
            GuardrailAction::Confirm { request_id }
        } else if self.cancel_button.contains(x, y)
            || self.close_button.contains(x, y)
            || !self.card.contains(x, y)
        {
            GuardrailAction::Cancel { request_id }
        } else {
            GuardrailAction::None
        }
    }
}

/// Known destructive command patterns
static PATTERNS: &[(&str, &str, Severity)] = &[
    // Critical: system-level destructive commands
    (
        "rm -rf /",
        "Recursive force-delete from root",
        Severity::Critical,
    ),
    (
        "rm -rf ~",
        "Recursive force-delete home directory",
        Severity::Critical,
    ),
    (
        "rm -rf /*",
        "Recursive force-delete root contents",
        Severity::Critical,
    ),
    ("> /dev/sd", "Overwrite raw disk device", Severity::Critical),
    ("dd if=", "Raw disk write (dd)", Severity::Critical),
    ("mkfs.", "Format filesystem", Severity::Critical),
    (
        "chmod -R 777 /",
        "Open all permissions from root",
        Severity::Critical,
    ),
    (":(){ :|:& };:", "Fork bomb", Severity::Critical),
    // Warning: potentially dangerous commands
    ("rm -rf", "Recursive force-delete", Severity::Warning),
    ("sudo rm", "Deleting files as root", Severity::Warning),
    (
        "chmod -R 777",
        "Opening all permissions recursively",
        Severity::Warning,
    ),
    (
        "chown -R",
        "Changing ownership recursively",
        Severity::Warning,
    ),
    ("> /dev/null", "Redirecting to /dev/null", Severity::Warning),
    (
        "truncate -s 0",
        "Truncating file to zero",
        Severity::Warning,
    ),
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

    fn fixture_origin() -> crate::tabs::TerminalEventOrigin {
        crate::tabs::TerminalEventOrigin::Tab { tab_id: 42 }
    }

    fn fixture_write(data: &[u8]) -> GuardedWrite {
        GuardedWrite::new(fixture_origin(), data.to_vec())
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
            GuardedWrite::new(
                crate::tabs::TerminalEventOrigin::Pane {
                    tab_id: 9,
                    pane_id: crate::panes::PaneId(2),
                },
                b"first-encoding".to_vec(),
            ),
            GuardedWrite::new(
                crate::tabs::TerminalEventOrigin::Pane {
                    tab_id: 9,
                    pane_id: crate::panes::PaneId(7),
                },
                b"second-encoding".to_vec(),
            ),
        ];
        let request = overlay.show(fixture_match(), writes.clone());
        assert_eq!(overlay.confirm(request), Some(writes));
    }

    #[test]
    fn shared_layout_has_disjoint_explicit_decision_targets() {
        let layout = GuardrailLayout::calculate(1200.0, 800.0, 10.0, 20.0, 1.0);
        assert!(!layout.cancel_button.contains(
            layout.confirm_button.x + layout.confirm_button.width / 2.0,
            layout.confirm_button.y + layout.confirm_button.height / 2.0,
        ));
        assert_eq!(
            layout.hit_test(
                layout.cancel_button.x + layout.cancel_button.width / 2.0,
                layout.cancel_button.y + layout.cancel_button.height / 2.0,
                7,
            ),
            GuardrailAction::Cancel { request_id: 7 }
        );
        assert_eq!(
            layout.hit_test(
                layout.confirm_button.x + layout.confirm_button.width / 2.0,
                layout.confirm_button.y + layout.confirm_button.height / 2.0,
                7,
            ),
            GuardrailAction::Confirm { request_id: 7 }
        );
        assert_eq!(
            layout.hit_test(layout.card.x - 1.0, layout.card.y - 1.0, 7),
            GuardrailAction::Cancel { request_id: 7 }
        );
    }
}
