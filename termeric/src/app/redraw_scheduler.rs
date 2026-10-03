//! Central redraw scheduling and diagnostics.
//!
//! Winit already coalesces native redraw requests. This scheduler adds the
//! missing application-level policy: collect why a frame is dirty, keep idle
//! wakeups deadline-driven, and retain lightweight counters for diagnostics.

use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum RedrawReason {
    Startup = 0,
    TerminalOutput,
    KeyboardInput,
    PointerInput,
    Resize,
    Focus,
    ConfigReload,
    GitStatus,
    Animation,
    CursorBlink,
    TextBlink,
    Search,
    SurfaceRecovery,
    BackgroundTask,
    Accessibility,
    WindowEvent,
}

impl RedrawReason {
    const ALL: [Self; 16] = [
        Self::Startup,
        Self::TerminalOutput,
        Self::KeyboardInput,
        Self::PointerInput,
        Self::Resize,
        Self::Focus,
        Self::ConfigReload,
        Self::GitStatus,
        Self::Animation,
        Self::CursorBlink,
        Self::TextBlink,
        Self::Search,
        Self::SurfaceRecovery,
        Self::BackgroundTask,
        Self::Accessibility,
        Self::WindowEvent,
    ];

    const fn bit(self) -> u32 {
        1 << self as u8
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::TerminalOutput => "terminal-output",
            Self::KeyboardInput => "keyboard-input",
            Self::PointerInput => "pointer-input",
            Self::Resize => "resize",
            Self::Focus => "focus",
            Self::ConfigReload => "config-reload",
            Self::GitStatus => "git-status",
            Self::Animation => "animation",
            Self::CursorBlink => "cursor-blink",
            Self::TextBlink => "text-blink",
            Self::Search => "search",
            Self::SurfaceRecovery => "surface-recovery",
            Self::BackgroundTask => "background-task",
            Self::Accessibility => "accessibility",
            Self::WindowEvent => "window-event",
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct RedrawScheduler {
    pending: u32,
    requested_frames: u64,
    rendered_frames: u64,
    coalesced_requests: u64,
}

impl RedrawScheduler {
    pub(super) fn request(&mut self, reason: RedrawReason) {
        let bit = reason.bit();
        if self.pending != 0 && self.pending & bit == 0 {
            self.coalesced_requests += 1;
        }
        self.pending |= bit;
    }

    pub(super) fn has_pending(&self) -> bool {
        self.pending != 0
    }

    pub(super) fn begin_frame(&mut self) -> String {
        self.requested_frames += 1;
        let pending = std::mem::take(&mut self.pending);
        Self::format_reasons(pending)
    }

    pub(super) fn finish_frame(&mut self) {
        self.rendered_frames += 1;
    }

    pub(super) fn next_deadline(
        now: Instant,
        deadlines: impl IntoIterator<Item = Instant>,
    ) -> Option<Instant> {
        deadlines
            .into_iter()
            .min()
            .map(|deadline| deadline.max(now))
    }

    fn format_reasons(bits: u32) -> String {
        let mut labels = Vec::new();
        for reason in RedrawReason::ALL {
            if bits & reason.bit() != 0 {
                labels.push(reason.label());
            }
        }
        if labels.is_empty() {
            "native".to_string()
        } else {
            labels.join(",")
        }
    }

    pub(super) fn diagnostic_summary(&self) -> String {
        format!(
            "requested={} rendered={} coalesced={}",
            self.requested_frames, self.rendered_frames, self.coalesced_requests
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn coalesces_multiple_dirty_sources_into_one_frame() {
        let mut scheduler = RedrawScheduler::default();
        scheduler.request(RedrawReason::TerminalOutput);
        scheduler.request(RedrawReason::CursorBlink);
        scheduler.request(RedrawReason::BackgroundTask);
        scheduler.request(RedrawReason::Accessibility);

        assert!(scheduler.has_pending());
        assert_eq!(
            scheduler.begin_frame(),
            "terminal-output,cursor-blink,background-task,accessibility"
        );
        assert!(!scheduler.has_pending());
        assert!(scheduler.diagnostic_summary().contains("coalesced=3"));
    }

    #[test]
    fn idle_scheduler_does_not_invent_frames() {
        let mut scheduler = RedrawScheduler::default();
        assert!(!scheduler.has_pending());
        assert_eq!(scheduler.begin_frame(), "native");
        scheduler.finish_frame();
        assert!(!scheduler.has_pending());
    }

    #[test]
    fn selects_the_earliest_deadline() {
        let now = Instant::now();
        let next = RedrawScheduler::next_deadline(
            now,
            [
                now + Duration::from_secs(2),
                now + Duration::from_millis(750),
            ],
        );
        assert_eq!(next, Some(now + Duration::from_millis(750)));
    }

    #[test]
    fn elapsed_deadlines_are_due_immediately() {
        let now = Instant::now();
        let next = RedrawScheduler::next_deadline(now, [now - Duration::from_millis(1)]);
        assert_eq!(next, Some(now));
    }

    #[test]
    fn every_diagnostic_reason_has_one_unique_nonzero_bit() {
        let mut observed = 0u32;
        for reason in RedrawReason::ALL {
            let bit = reason.bit();
            assert_ne!(bit, 0);
            assert_eq!(observed & bit, 0, "duplicate bit for {}", reason.label());
            observed |= bit;
        }
        assert_eq!(observed.count_ones() as usize, RedrawReason::ALL.len());
    }
}
