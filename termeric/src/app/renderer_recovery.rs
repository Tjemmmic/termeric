//! Bounded renderer recovery accounting.

use std::collections::VecDeque;
use std::time::{Duration, Instant};
use wgpu::DeviceLostReason;

const RECOVERY_WINDOW: Duration = Duration::from_secs(30);
const MAX_RECOVERIES: usize = 3;
/// Give Winit time to deliver either the resize that safely reconfigures a live
/// surface or the close event for a destroyed native drawable. If neither
/// arrives, one whole-renderer recreation prevents an indefinite blank window.
pub(super) const SURFACE_RECOVERY_DELAY: Duration = Duration::from_millis(100);

pub(super) fn is_current_unexpected_loss(
    callback_generation: u64,
    current_generation: u64,
    reason: DeviceLostReason,
) -> bool {
    callback_generation == current_generation && reason != DeviceLostReason::Destroyed
}

/// Surface loss and an outdated swap chain both require reconfiguration.
/// Timeout is transient, out-of-memory is fatal, and a generic error may be
/// followed by the device-loss callback, so those retain separate policies.
pub(super) fn requires_surface_recovery(error: &wgpu::SurfaceError) -> bool {
    matches!(
        error,
        wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated
    )
}

#[derive(Debug, Default)]
pub(super) struct RendererRecoveryPolicy {
    attempts: VecDeque<Instant>,
}

impl RendererRecoveryPolicy {
    /// Admit at most three recovery attempts in a rolling 30-second window.
    /// Calls only happen after an actual WGPU loss event, never on an idle timer.
    pub(super) fn admit(&mut self, now: Instant) -> bool {
        while self
            .attempts
            .front()
            .is_some_and(|attempt| now.saturating_duration_since(*attempt) >= RECOVERY_WINDOW)
        {
            self.attempts.pop_front();
        }
        if self.attempts.len() >= MAX_RECOVERIES {
            return false;
        }
        self.attempts.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_is_capped_without_spinning_and_reopens_after_real_time() {
        let start = Instant::now();
        let mut policy = RendererRecoveryPolicy::default();
        assert!(policy.admit(start));
        assert!(policy.admit(start + Duration::from_secs(1)));
        assert!(policy.admit(start + Duration::from_secs(2)));
        assert!(!policy.admit(start + Duration::from_secs(3)));
        assert!(policy.admit(start + RECOVERY_WINDOW));
    }

    #[test]
    fn stale_and_intentional_device_callbacks_do_not_recover() {
        assert!(is_current_unexpected_loss(7, 7, DeviceLostReason::Unknown));
        assert!(!is_current_unexpected_loss(6, 7, DeviceLostReason::Unknown));
        assert!(!is_current_unexpected_loss(
            7,
            7,
            DeviceLostReason::Destroyed
        ));
    }

    #[test]
    fn lost_and_outdated_surfaces_enter_the_bounded_recovery_path() {
        assert!(requires_surface_recovery(&wgpu::SurfaceError::Lost));
        assert!(requires_surface_recovery(&wgpu::SurfaceError::Outdated));
        assert!(!requires_surface_recovery(&wgpu::SurfaceError::Timeout));
        assert!(!requires_surface_recovery(&wgpu::SurfaceError::OutOfMemory));
        assert!(!requires_surface_recovery(&wgpu::SurfaceError::Other));
    }
}
