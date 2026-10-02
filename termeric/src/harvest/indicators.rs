//! Organic Loading Indicators
//!
//! Plant-growth animations for long-running processes.
//! Displays a small vine/sprout animation while commands are running.

use std::time::Instant;

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
    /// Get the color for this phase [r, g, b, a]
    pub fn color(&self) -> [f32; 4] {
        match self {
            Self::Seed => [0.5, 0.4, 0.3, 0.6],       // Brown seed
            Self::Sprouting => [0.5, 0.6, 0.4, 0.8],  // Light green
            Self::Growing => [0.4, 0.7, 0.3, 1.0],    // Green
            Self::FullGrowth => [0.3, 0.8, 0.3, 1.0], // Bright green
            Self::Withering => [0.6, 0.4, 0.3, 0.8],  // Brown (wilted)
            Self::Blooming => [0.9, 0.8, 0.4, 1.0],   // Golden bloom
            Self::Finished => [0.0, 0.0, 0.0, 0.0],   // Invisible
        }
    }

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
            self.phase = if completion_elapsed > 2.0 {
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

    /// Get the current scale (for pulsing animation)
    pub fn scale(&self) -> f32 {
        let now = Instant::now();
        if let Some(completed_at) = self.completed_at {
            // Shrink when complete
            1.0 - now.duration_since(completed_at).as_secs_f32().min(1.0) * 0.5
        } else {
            // Pulse while running
            1.0 + (now.duration_since(self.started_at).as_secs_f32() * 3.0).sin() * 0.1
        }
    }

    /// Get the current color with animation
    pub fn color(&self) -> [f32; 4] {
        let base = self.phase.color();

        let fade_elapsed = self
            .completed_at
            .map(|completed_at| completed_at.elapsed().as_secs_f32())
            .unwrap_or(0.0);
        if self.completed && fade_elapsed > 1.0 {
            // Fade out
            let fade = 1.0 - (fade_elapsed - 1.0).min(1.0);
            [base[0], base[1], base[2], base[3] * fade]
        } else {
            base
        }
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
}
