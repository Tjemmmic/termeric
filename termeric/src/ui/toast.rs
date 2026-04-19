//! Transient status toasts — non-blocking notifications stacked in the
//! bottom-right corner.
//!
//! Used for "something happened, you should know, but don't stop what you're
//! doing" feedback: saves, deletes, font fallbacks, image evictions, etc.
//! Blocking errors that demand acknowledgement still go through
//! `info_overlay` (the full modal card).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Severity tier — drives color and default duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastSeverity {
    Info,
    Success,
    Warning,
    Error,
}

impl ToastSeverity {
    /// Default dwell time before the toast starts fading out.
    pub fn default_duration(self) -> Duration {
        match self {
            // Short-lived confirmations.
            ToastSeverity::Success => Duration::from_millis(2200),
            ToastSeverity::Info => Duration::from_millis(2800),
            // Warnings / errors stay longer; user may want to read them.
            ToastSeverity::Warning => Duration::from_millis(4500),
            ToastSeverity::Error => Duration::from_millis(5500),
        }
    }
}

/// One toast entry.
#[derive(Debug, Clone)]
pub struct Toast {
    pub id: u64,
    pub severity: ToastSeverity,
    pub message: String,
    pub created: Instant,
    pub duration: Duration,
}

impl Toast {
    /// Eased opacity (0.0–1.0) for this frame. Appears with a 150ms fade-in,
    /// holds at 1.0, then fades out over the last 300ms of its life.
    pub fn opacity(&self, now: Instant) -> f32 {
        let elapsed = now.duration_since(self.created);
        let total_ms = self.duration.as_millis() as f32;
        let elapsed_ms = elapsed.as_millis() as f32;
        if elapsed_ms >= total_ms {
            return 0.0;
        }
        const FADE_IN_MS: f32 = 150.0;
        const FADE_OUT_MS: f32 = 300.0;
        let fade_in = (elapsed_ms / FADE_IN_MS).clamp(0.0, 1.0);
        let fade_out = ((total_ms - elapsed_ms) / FADE_OUT_MS).clamp(0.0, 1.0);
        let raw = fade_in.min(fade_out);
        // Ease-out cubic to match the rest of the UI.
        1.0 - (1.0 - raw).powi(3)
    }

    pub fn is_expired(&self, now: Instant) -> bool {
        now.duration_since(self.created) >= self.duration
    }
}

/// Stack of active toasts. Call `update()` once per frame to prune expired
/// entries and learn whether a redraw is needed.
pub struct ToastManager {
    toasts: VecDeque<Toast>,
    next_id: u64,
    max_visible: usize,
}

impl Default for ToastManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ToastManager {
    pub fn new() -> Self {
        Self {
            toasts: VecDeque::new(),
            next_id: 1,
            max_visible: 4,
        }
    }

    fn push_inner(&mut self, severity: ToastSeverity, message: String) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let toast = Toast {
            id,
            severity,
            message,
            created: Instant::now(),
            duration: severity.default_duration(),
        };
        self.toasts.push_back(toast);
        // Evict oldest if we're over the visible cap.
        while self.toasts.len() > self.max_visible {
            self.toasts.pop_front();
        }
        id
    }

    pub fn info(&mut self, message: impl Into<String>) -> u64 {
        self.push_inner(ToastSeverity::Info, message.into())
    }
    pub fn success(&mut self, message: impl Into<String>) -> u64 {
        self.push_inner(ToastSeverity::Success, message.into())
    }
    pub fn warning(&mut self, message: impl Into<String>) -> u64 {
        self.push_inner(ToastSeverity::Warning, message.into())
    }
    pub fn error(&mut self, message: impl Into<String>) -> u64 {
        self.push_inner(ToastSeverity::Error, message.into())
    }

    /// Prune expired toasts. Returns true if anything changed (caller should
    /// request a redraw). Also returns true while any toast is still
    /// animating, so the caller keeps driving frames.
    pub fn update(&mut self) -> bool {
        let now = Instant::now();
        let before = self.toasts.len();
        self.toasts.retain(|t| !t.is_expired(now));
        let changed = before != self.toasts.len();
        let animating = !self.toasts.is_empty();
        changed || animating
    }

    pub fn is_empty(&self) -> bool {
        self.toasts.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Toast> {
        self.toasts.iter()
    }
}

/// Render snapshot for the renderer.
#[derive(Debug, Clone)]
pub struct ToastInfo {
    pub severity: ToastSeverity,
    pub message: String,
    pub opacity: f32,
}

impl ToastManager {
    /// Build the list of renderable toasts (with per-toast opacity).
    pub fn build_info(&self) -> Vec<ToastInfo> {
        let now = Instant::now();
        self.toasts
            .iter()
            .map(|t| ToastInfo {
                severity: t.severity,
                message: t.message.clone(),
                opacity: t.opacity(now),
            })
            .filter(|t| t.opacity > 0.001)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_push_and_expire() {
        let mut tm = ToastManager::new();
        let id = tm.info("hello");
        assert!(id > 0);
        assert_eq!(tm.iter().count(), 1);

        // Override duration to something short, then let it expire.
        if let Some(t) = tm.toasts.back_mut() {
            t.duration = Duration::from_millis(20);
        }
        thread::sleep(Duration::from_millis(30));
        tm.update();
        assert_eq!(tm.iter().count(), 0);
    }

    #[test]
    fn test_eviction_above_max_visible() {
        let mut tm = ToastManager::new();
        tm.max_visible = 2;
        tm.info("a");
        tm.info("b");
        tm.info("c");
        assert_eq!(tm.iter().count(), 2);
        let messages: Vec<_> = tm.iter().map(|t| t.message.as_str()).collect();
        // Oldest ("a") evicted; "b" and "c" remain.
        assert_eq!(messages, vec!["b", "c"]);
    }

    #[test]
    fn test_severity_duration_ordering() {
        // Errors should live at least as long as warnings, and warnings at
        // least as long as info/success.
        assert!(
            ToastSeverity::Error.default_duration() >= ToastSeverity::Warning.default_duration()
        );
        assert!(
            ToastSeverity::Warning.default_duration() >= ToastSeverity::Info.default_duration()
        );
    }
}
