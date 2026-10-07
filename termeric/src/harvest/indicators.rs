//! Organic Loading Indicators
//!
//! Plant-growth animations for long-running processes.
//! Displays a small vine/sprout animation while commands are running.

use std::time::Instant;

use crate::ui::animation::ease_out_cubic;

/// How long a finished command's bloom or wither stays before it is gone.
const COMPLETED_SECONDS: f32 = 2.0;
/// The fade that ends that time, within the 250 ms every transition keeps to.
const FADE_SECONDS: f32 = 0.25;

/// Growth phase of the indicator
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrowthPhase {
    /// Initial seed phase (just started)
    Seed,
    /// Sprouting (short time running)
    Sprouting,
    /// Growing (medium time)
    Growing,
    /// Full growth (long running)
    FullGrowth,
    /// Withering (completed with failure)
    Withering,
    /// Blooming (completed with success)
    Blooming,
    /// Finished (animation complete)
    Finished,
}

impl GrowthPhase {
    /// Get a character representation for text-based display
    pub fn character(&self) -> char {
        match self {
            Self::Seed => '\u{25CF}',       // Filled circle
            Self::Sprouting => '\u{2740}',  // Flower
            Self::Growing => '\u{2618}',    // Shamrock
            Self::FullGrowth => '\u{2766}', // Floral heart
            Self::Withering => '\u{2767}',  // Rotated floral heart
            Self::Blooming => '\u{2055}',   // Flower
            Self::Finished => ' ',
        }
    }
}

/// Growth indicator for a running command
#[derive(Debug, Clone)]
pub struct GrowthIndicator {
    /// Stable shell-region identity for this exact command.
    pub region_id: u64,
    /// Row where the command prompt is
    pub prompt_row: usize,
    /// Current growth phase
    pub phase: GrowthPhase,
    /// Monotonic command start time.
    started_at: Instant,
    /// Whether the command completed
    pub completed: bool,
    /// Whether the command was successful
    pub success: bool,
    /// Monotonic completion time.
    completed_at: Option<Instant>,
}

impl GrowthIndicator {
    /// Create a new growth indicator
    pub fn new(region_id: u64, prompt_row: usize) -> Self {
        Self {
            region_id,
            prompt_row,
            phase: GrowthPhase::Seed,
            started_at: Instant::now(),
            completed: false,
            success: false,
            completed_at: None,
        }
    }

    /// Update the indicator
    pub fn update(&mut self) {
        self.update_at(Instant::now());
    }

    fn update_at(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.started_at).as_secs_f32();
        if let Some(completed_at) = self.completed_at {
            let completion_elapsed = now.duration_since(completed_at).as_secs_f32();
            self.phase = if completion_elapsed > COMPLETED_SECONDS {
                GrowthPhase::Finished
            } else if self.success {
                GrowthPhase::Blooming
            } else {
                GrowthPhase::Withering
            };
        } else {
            let new_phase = match elapsed {
                t if t < 0.5 => GrowthPhase::Seed,
                t if t < 2.0 => GrowthPhase::Sprouting,
                t if t < 5.0 => GrowthPhase::Growing,
                _ => GrowthPhase::FullGrowth,
            };
            self.phase = new_phase;
        }
    }

    /// Mark the command as completed
    pub fn complete(&mut self, success: bool) {
        self.completed = true;
        self.success = success;
        self.completed_at = Some(Instant::now());
    }

    /// Check if the indicator animation is finished
    pub fn is_finished(&self) -> bool {
        self.phase == GrowthPhase::Finished
    }

    /// How visible the indicator is: fully, until it fades out at the end
    /// of its finished command's bloom or wither.
    pub fn opacity(&self) -> f32 {
        self.opacity_at(Instant::now())
    }

    fn opacity_at(&self, now: Instant) -> f32 {
        let Some(completed_at) = self.completed_at else {
            return 1.0;
        };
        let fade_start = COMPLETED_SECONDS - FADE_SECONDS;
        let elapsed = now.duration_since(completed_at).as_secs_f32();
        let t = ((elapsed - fade_start) / FADE_SECONDS).clamp(0.0, 1.0);
        1.0 - ease_out_cubic(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_growth_phases() {
        let mut indicator = GrowthIndicator::new(1, 0);

        assert_eq!(indicator.phase, GrowthPhase::Seed);

        indicator.update_at(indicator.started_at + Duration::from_secs(5));

        // Should have grown
        assert!(indicator.phase != GrowthPhase::Seed);
        assert!(!indicator.is_finished());
    }

    #[test]
    fn test_completion() {
        let mut indicator = GrowthIndicator::new(1, 0);
        indicator.complete(true);

        assert!(indicator.completed);
        assert!(indicator.success);

        let completed_at = indicator.completed_at.unwrap();
        indicator.update_at(completed_at + Duration::from_secs(3));

        assert!(indicator.is_finished());
    }

    #[test]
    fn a_finished_indicator_fades_out_within_250_ms_before_it_is_gone() {
        let mut indicator = GrowthIndicator::new(1, 0);
        assert_eq!(indicator.opacity_at(indicator.started_at), 1.0);
        indicator.complete(false);
        let completed_at = indicator.completed_at.unwrap();
        let at = |millis| completed_at + Duration::from_millis(millis);

        assert_eq!(indicator.opacity_at(at(1_750)), 1.0);
        let fading = indicator.opacity_at(at(1_875));
        assert!(0.0 < fading && fading < 1.0, "{fading}");
        assert_eq!(indicator.opacity_at(at(2_000)), 0.0);
        indicator.update_at(at(1_999));
        assert_eq!(indicator.phase, GrowthPhase::Withering);
        indicator.update_at(at(2_001));
        assert!(indicator.is_finished());
    }
}
