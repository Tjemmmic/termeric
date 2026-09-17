//! Fixed-duration cursor movement, sampled from an absolute clock.

use std::time::{Duration, Instant};

use super::animation::ease_out_cubic;

pub const CURSOR_MOVE_DURATION: Duration = Duration::from_millis(150);

#[derive(Debug, Default)]
pub struct CursorMotion {
    from: (f32, f32),
    target: (f32, f32),
    started: Option<Instant>,
}

impl CursorMotion {
    /// Remains pending until a render samples the final position. Checking the
    /// deadline alone must not suppress the frame that actually draws it.
    pub fn is_active(&self) -> bool {
        self.started.is_some()
    }

    fn position(&self, now: Instant) -> (f32, f32) {
        let Some(started) = self.started else {
            return self.target;
        };
        let elapsed = now.saturating_duration_since(started);
        if elapsed >= CURSOR_MOVE_DURATION {
            return self.target;
        }
        let t = ease_out_cubic(elapsed.as_secs_f32() / CURSOR_MOVE_DURATION.as_secs_f32());
        (
            self.from.0 + (self.target.0 - self.from.0) * t,
            self.from.1 + (self.target.1 - self.from.1) * t,
        )
    }

    pub fn update(&mut self, target: (f32, f32), now: Instant, snap: bool) -> (f32, f32) {
        if snap {
            self.from = target;
            self.target = target;
            self.started = None;
            return target;
        }
        if target != self.target {
            // Evaluate the previous trajectory at the retarget instant, not
            // at the last sampled frame, so sparse and dense frames agree.
            self.from = self.position(now);
            self.target = target;
            self.started = (self.from != target).then_some(now);
        }
        let position = self.position(now);
        if self
            .started
            .is_some_and(|started| now.saturating_duration_since(started) >= CURSOR_MOVE_DURATION)
        {
            self.from = self.target;
            self.started = None;
        }
        position
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_elapsed_time_is_independent_of_frame_schedule() {
        let start = Instant::now();
        for target in [(1.0, 0.0), (0.0, 29.0), (4096.0, 2048.0)] {
            let mut sparse = CursorMotion::default();
            let mut dense = CursorMotion::default();
            sparse.update(target, start, false);
            dense.update(target, start, false);
            for millis in 1..75 {
                dense.update(target, start + Duration::from_millis(millis), false);
            }
            let middle = start + Duration::from_millis(75);
            assert_eq!(
                sparse.update(target, middle, false),
                dense.update(target, middle, false)
            );
            assert_eq!(
                sparse.position(middle),
                (target.0 * 0.875, target.1 * 0.875)
            );
            assert!(sparse.is_active());
            assert_eq!(
                sparse.update(target, start + CURSOR_MOVE_DURATION, false),
                target
            );
            assert_eq!(
                dense.update(target, start + CURSOR_MOVE_DURATION, false),
                target
            );
            assert!(!sparse.is_active());
            assert!(!dense.is_active());
        }
    }

    #[test]
    fn retargeting_uses_the_current_trajectory_without_frame_history() {
        let start = Instant::now();
        let mut sparse = CursorMotion::default();
        let mut dense = CursorMotion::default();
        for motion in [&mut sparse, &mut dense] {
            motion.update((80.0, 24.0), start, false);
        }
        dense.update((80.0, 24.0), start + Duration::from_millis(30), false);
        let retarget = start + Duration::from_millis(75);
        let target = (2.0, 1.0);
        let expected_from = sparse.position(retarget);
        assert_eq!(sparse.update(target, retarget, false), expected_from);
        assert_eq!(dense.update(target, retarget, false), expected_from);
        for millis in [1, 50, 149, 150, 900] {
            let now = retarget + Duration::from_millis(millis);
            let actual = sparse.update(target, now, false);
            assert_eq!(actual, dense.update(target, now, false));
            assert!((target.0..=expected_from.0).contains(&actual.0));
            assert!((target.1..=expected_from.1).contains(&actual.1));
        }
        assert!(!sparse.is_active());
    }

    #[test]
    fn unchanged_targets_finish_exactly_and_keep_the_final_frame_pending() {
        let start = Instant::now();
        let target = (10.0, 29.0);
        let mut motion = CursorMotion::default();
        motion.update(target, start, false);
        motion.update(target, start + Duration::from_millis(149), false);
        assert!(motion.is_active());
        // A delayed/occluded frame does not require replaying missed steps.
        assert!(motion.is_active());
        assert_eq!(
            motion.update(target, start + Duration::from_secs(2), false),
            target
        );
        assert!(!motion.is_active());
        assert_eq!(
            motion.update(target, start + Duration::from_secs(4), false),
            target
        );
        assert!(!motion.is_active());
    }

    #[test]
    fn snapping_cancels_motion_and_sets_the_next_transition_origin() {
        let start = Instant::now();
        let mut motion = CursorMotion::default();
        motion.update((20.0, 30.0), start, false);
        assert_eq!(motion.update((5.0, 6.0), start, true), (5.0, 6.0));
        assert!(!motion.is_active());
        assert_eq!(motion.update((2.0, 1.0), start, false), (5.0, 6.0));
        assert!(motion.is_active());
        assert_eq!(
            motion.update((2.0, 1.0), start + CURSOR_MOVE_DURATION, false),
            (2.0, 1.0)
        );
    }
}
