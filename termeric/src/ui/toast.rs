//! Transient status toasts — non-blocking notifications stacked in the
//! bottom-right corner.
//!
//! Used for "something happened, you should know, but don't stop what you're
//! doing" feedback: saves, deletes, font fallbacks, image evictions, etc.
//! Blocking errors that demand acknowledgement still go through
//! `info_overlay` (the full modal card).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

const FADE_IN: Duration = Duration::from_millis(150);
const FADE_OUT: Duration = Duration::from_millis(300);
const ANIMATION_FRAME: Duration = Duration::from_millis(16);
pub const MAX_TOAST_MESSAGE_CHARS: usize = 512;

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
    persistent: bool,
}

impl Toast {
    /// Eased opacity (0.0–1.0) for this frame. Appears with a 150ms fade-in,
    /// holds at 1.0, then fades out over the last 300ms of its life.
    pub fn opacity(&self, now: Instant) -> f32 {
        let elapsed = now.duration_since(self.created);
        if self.persistent {
            let fade_in = (elapsed.as_millis() as f32 / FADE_IN.as_millis() as f32).clamp(0.0, 1.0);
            return 1.0 - (1.0 - fade_in).powi(3);
        }
        let total_ms = self.duration.as_millis() as f32;
        let elapsed_ms = elapsed.as_millis() as f32;
        if elapsed_ms >= total_ms {
            return 0.0;
        }
        let fade_in = (elapsed_ms / FADE_IN.as_millis() as f32).clamp(0.0, 1.0);
        let fade_out = ((total_ms - elapsed_ms) / FADE_OUT.as_millis() as f32).clamp(0.0, 1.0);
        let raw = fade_in.min(fade_out);
        // Ease-out cubic to match the rest of the UI.
        1.0 - (1.0 - raw).powi(3)
    }

    pub fn is_expired(&self, now: Instant) -> bool {
        !self.persistent && now.duration_since(self.created) >= self.duration
    }
}

/// Stack of active toasts. Call `update()` once per frame to prune expired
/// entries and learn whether a redraw is needed.
pub struct ToastManager {
    toasts: VecDeque<Toast>,
    max_visible: usize,
    next_id: u64,
    reduced_motion: bool,
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
            max_visible: 4,
            next_id: 1,
            reduced_motion: false,
        }
    }

    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.reduced_motion = reduced_motion;
    }

    fn push_inner(&mut self, severity: ToastSeverity, message: String) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let toast = Toast {
            id,
            severity,
            message: bound_message(message),
            created: Instant::now(),
            duration: severity.default_duration(),
            persistent: false,
        };
        self.toasts.push_back(toast);
        // Evict oldest if we're over the visible cap.
        while self.toasts.len() > self.max_visible {
            let evict = self
                .toasts
                .iter()
                .position(|toast| !toast.persistent)
                .unwrap_or(0);
            self.toasts.remove(evict);
        }
        id
    }

    pub fn info(&mut self, message: impl Into<String>) {
        self.push_inner(ToastSeverity::Info, message.into());
    }
    pub fn success(&mut self, message: impl Into<String>) {
        self.push_inner(ToastSeverity::Success, message.into());
    }
    pub fn warning(&mut self, message: impl Into<String>) {
        self.push_inner(ToastSeverity::Warning, message.into());
    }
    pub fn error(&mut self, message: impl Into<String>) {
        self.push_inner(ToastSeverity::Error, message.into());
    }

    /// Add an explicitly-dismissed progress/status toast and return its stable
    /// ID so background work can update one bounded UI entry in place.
    pub fn persistent_info(&mut self, message: impl Into<String>) -> u64 {
        let id = self.push_inner(ToastSeverity::Info, message.into());
        if let Some(toast) = self.toasts.iter_mut().find(|toast| toast.id == id) {
            toast.persistent = true;
        }
        id
    }

    pub fn update_persistent(&mut self, id: u64, message: impl Into<String>) -> bool {
        let Some(toast) = self.toasts.iter_mut().find(|toast| toast.id == id) else {
            return false;
        };
        if !toast.persistent {
            return false;
        }
        toast.message = bound_message(message.into());
        true
    }

    pub fn dismiss(&mut self, id: u64) -> bool {
        let before = self.toasts.len();
        self.toasts.retain(|toast| toast.id != id);
        self.toasts.len() != before
    }

    /// Prune expired toasts. Returns true if anything changed (caller should
    /// request a redraw). Also returns true while any toast is still
    /// animating, so the caller keeps driving frames.
    pub fn update(&mut self) -> bool {
        let now = Instant::now();
        let before = self.toasts.len();
        self.toasts.retain(|t| !t.is_expired(now));
        let changed = before != self.toasts.len();
        let animating = self.has_active_animation(now);
        changed || animating
    }

    /// Whether an opacity transition is active at `now`. Toasts in their
    /// fully opaque dwell period do not require frames.
    pub fn has_active_animation(&self, now: Instant) -> bool {
        if self.reduced_motion {
            return false;
        }
        self.toasts.iter().any(|toast| {
            let elapsed = now.saturating_duration_since(toast.created);
            elapsed < FADE_IN
                || (!toast.persistent
                    && elapsed < toast.duration
                    && elapsed >= toast.duration.saturating_sub(FADE_OUT))
        })
    }

    /// Earliest time the toast stack can visually change.
    pub fn next_redraw_deadline(&self, now: Instant) -> Option<Instant> {
        if self.reduced_motion {
            return self
                .toasts
                .iter()
                .filter(|toast| !toast.persistent && !toast.is_expired(now))
                .map(|toast| toast.created + toast.duration)
                .min();
        }
        self.toasts
            .iter()
            .filter_map(|toast| {
                let elapsed = now.saturating_duration_since(toast.created);
                if toast.persistent {
                    return (elapsed < FADE_IN).then_some(toast.created + FADE_IN);
                }
                let fade_out_start = toast.duration.saturating_sub(FADE_OUT);
                if elapsed < FADE_IN || elapsed >= fade_out_start {
                    Some((now + ANIMATION_FRAME).min(toast.created + toast.duration))
                } else {
                    Some(toast.created + fade_out_start)
                }
            })
            .min()
    }

    #[cfg(test)]
    pub fn iter(&self) -> impl Iterator<Item = &Toast> {
        self.toasts.iter()
    }
}

/// Render snapshot for the renderer.
#[derive(Debug, Clone)]
pub struct ToastInfo {
    pub id: u64,
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
                id: t.id,
                severity: t.severity,
                message: t.message.clone(),
                opacity: if self.reduced_motion {
                    1.0
                } else {
                    t.opacity(now)
                },
            })
            .filter(|t| t.opacity > 0.001)
            .collect()
    }

    /// Include newly-created entries even before the first visible fade-in
    /// frame so assistive technology can announce the event immediately.
    pub fn build_accessibility_info(&self) -> Vec<ToastInfo> {
        let now = Instant::now();
        self.toasts
            .iter()
            .filter(|toast| !toast.is_expired(now))
            .map(|toast| ToastInfo {
                id: toast.id,
                severity: toast.severity,
                message: toast.message.clone(),
                opacity: if self.reduced_motion {
                    1.0
                } else {
                    toast.opacity(now)
                },
            })
            .collect()
    }
}

fn bound_message(message: String) -> String {
    crate::safe_text::truncate_with_ellipsis(&message, MAX_TOAST_MESSAGE_CHARS)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToastRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToastPlacement {
    pub toast_index: usize,
    pub rect: ToastRect,
}

/// Shared toast placement used by rendering and accessibility bounds.
pub fn toast_layout(
    toasts: &[ToastInfo],
    window_width: f32,
    window_height: f32,
    cell_width: f32,
    cell_height: f32,
) -> Vec<ToastPlacement> {
    let padding = cell_width;
    let row_height = cell_height * 2.0;
    let gap = 6.0;
    let max_width = (window_width * 0.5).min(cell_width * 48.0).max(0.0);
    let min_width = (cell_width * 18.0).min(max_width);
    let right_margin = cell_width * 1.5;
    let bottom_margin = cell_height;
    let mut y = window_height - bottom_margin;
    let mut placements = Vec::with_capacity(toasts.len());

    for (toast_index, toast) in toasts.iter().enumerate().rev() {
        y -= row_height;
        let text_width = toast.message.chars().count() as f32 * cell_width;
        let width = (text_width + padding * 2.0 + cell_width * 2.0).clamp(min_width, max_width);
        let x = window_width - width - right_margin + (1.0 - toast.opacity) * (cell_width * 2.0);
        placements.push(ToastPlacement {
            toast_index,
            rect: ToastRect {
                x,
                y,
                width,
                height: row_height,
            },
        });
        y -= gap;
    }
    placements
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_push_and_expire() {
        let mut tm = ToastManager::new();
        tm.info("hello");
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
    fn messages_and_stable_ids_are_bounded() {
        let mut tm = ToastManager::new();
        tm.info("a".repeat(MAX_TOAST_MESSAGE_CHARS + 100));
        tm.info("second");
        let toasts = tm.build_accessibility_info();
        assert_eq!(toasts[0].message.chars().count(), MAX_TOAST_MESSAGE_CHARS);
        assert!(toasts[0].message.ends_with('…'));
        assert_ne!(toasts[0].id, toasts[1].id);
        let first_id = toasts[0].id;
        assert_eq!(tm.build_accessibility_info()[0].id, first_id);
    }

    #[test]
    fn persistent_status_updates_in_place_and_requires_explicit_dismissal() {
        let mut tm = ToastManager::new();
        let id = tm.persistent_info("Uploading: 0%");
        if let Some(toast) = tm.toasts.back_mut() {
            toast.duration = Duration::ZERO;
        }
        tm.update();
        assert_eq!(tm.iter().count(), 1);
        assert!(tm.update_persistent(id, "Uploading: 50%"));
        assert_eq!(tm.iter().next().unwrap().message, "Uploading: 50%");
        tm.toasts.back_mut().unwrap().created = Instant::now() - FADE_IN;
        assert_eq!(tm.next_redraw_deadline(Instant::now()), None);
        assert!(tm.dismiss(id));
        assert_eq!(tm.iter().count(), 0);
    }

    #[test]
    fn transient_churn_does_not_evict_persistent_status() {
        let mut tm = ToastManager::new();
        tm.max_visible = 2;
        let id = tm.persistent_info("Uploading");
        tm.info("first");
        tm.info("second");
        let toasts = tm.build_accessibility_info();
        assert_eq!(toasts.len(), 2);
        assert!(toasts.iter().any(|toast| toast.id == id));
        assert!(toasts.iter().any(|toast| toast.message == "second"));
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

    #[test]
    fn opaque_dwell_uses_a_deadline_instead_of_continuous_frames() {
        let now = Instant::now();
        let mut tm = ToastManager::new();
        tm.info("steady");
        let toast = tm.toasts.back_mut().unwrap();
        toast.created = now - Duration::from_millis(200);
        toast.duration = Duration::from_secs(1);
        let fade_out_at = toast.created + Duration::from_millis(700);

        assert!(!tm.has_active_animation(now));
        assert_eq!(tm.next_redraw_deadline(now), Some(fade_out_at));
    }

    #[test]
    fn reduced_motion_snaps_toasts_and_schedules_only_expiration() {
        let now = Instant::now();
        let mut tm = ToastManager::new();
        tm.info("steady");
        let toast = tm.toasts.back_mut().unwrap();
        toast.created = now;
        toast.duration = Duration::from_secs(1);
        tm.set_reduced_motion(true);

        assert_eq!(tm.build_info()[0].opacity, 1.0);
        assert!(!tm.has_active_animation(now));
        assert_eq!(
            tm.next_redraw_deadline(now),
            Some(now + Duration::from_secs(1))
        );
    }
}
