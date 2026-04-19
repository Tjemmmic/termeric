//! Organic Loading Indicators
//!
//! Plant-growth animations for long-running processes.
//! Displays a small vine/sprout animation while commands are running.

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
    /// Row where the command prompt is
    pub prompt_row: usize,
    /// Current growth phase
    pub phase: GrowthPhase,
    /// Time since command started (seconds)
    pub elapsed: f32,
    /// Animation progress within current phase (0.0 to 1.0)
    pub progress: f32,
    /// Whether the command completed
    pub completed: bool,
    /// Whether the command was successful
    pub success: bool,
    /// Time to fade out after completion
    pub fade_timer: f32,
}

#[allow(dead_code)]
impl GrowthIndicator {
    /// Create a new growth indicator
    pub fn new(prompt_row: usize) -> Self {
        Self {
            prompt_row,
            phase: GrowthPhase::Seed,
            elapsed: 0.0,
            progress: 0.0,
            completed: false,
            success: false,
            fade_timer: 0.0,
        }
    }

    /// Update the indicator
    pub fn update(&mut self, dt: f32) {
        self.elapsed += dt;

        if self.completed {
            // Handle completion animation
            self.fade_timer += dt;
            self.progress = (self.progress + dt * 3.0).min(1.0);

            // Transition through completion phases
            match self.phase {
                GrowthPhase::Blooming | GrowthPhase::Withering => {
                    if self.fade_timer > 2.0 {
                        self.phase = GrowthPhase::Finished;
                    }
                }
                _ => {
                    self.phase = if self.success {
                        GrowthPhase::Blooming
                    } else {
                        GrowthPhase::Withering
                    };
                }
            }
        } else {
            // Update growth phase based on elapsed time
            let new_phase = match self.elapsed {
                t if t < 0.5 => GrowthPhase::Seed,
                t if t < 2.0 => GrowthPhase::Sprouting,
                t if t < 5.0 => GrowthPhase::Growing,
                _ => GrowthPhase::FullGrowth,
            };

            if new_phase != self.phase {
                self.phase = new_phase;
                self.progress = 0.0;
            } else {
                // Animate within phase
                self.progress = (self.progress + dt * 2.0).min(1.0);
            }
        }
    }

    /// Mark the command as completed
    pub fn complete(&mut self, success: bool) {
        self.completed = true;
        self.success = success;
        self.progress = 0.0;
    }

    /// Check if the indicator animation is finished
    pub fn is_finished(&self) -> bool {
        self.phase == GrowthPhase::Finished
    }

    /// Get the current scale (for pulsing animation)
    pub fn scale(&self) -> f32 {
        if self.completed {
            // Shrink when complete
            1.0 - self.fade_timer.min(1.0) * 0.5
        } else {
            // Pulse while running
            1.0 + (self.elapsed * 3.0).sin() * 0.1
        }
    }

    /// Get the current rotation (for growth animation)
    pub fn rotation(&self) -> f32 {
        // Gentle swaying motion
        (self.elapsed * 2.0).sin() * 0.1
    }

    /// Get the current color with animation
    pub fn color(&self) -> [f32; 4] {
        let base = self.phase.color();

        if self.completed && self.fade_timer > 1.0 {
            // Fade out
            let fade = 1.0 - (self.fade_timer - 1.0).min(1.0);
            [base[0], base[1], base[2], base[3] * fade]
        } else {
            base
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_growth_phases() {
        let mut indicator = GrowthIndicator::new(0);

        assert_eq!(indicator.phase, GrowthPhase::Seed);

        // Simulate time passing
        for _ in 0..10 {
            indicator.update(0.5);
        }

        // Should have grown
        assert!(indicator.phase != GrowthPhase::Seed);
        assert!(!indicator.is_finished());
    }

    #[test]
    fn test_completion() {
        let mut indicator = GrowthIndicator::new(0);
        indicator.complete(true);

        assert!(indicator.completed);
        assert!(indicator.success);

        // After enough time, should be finished
        for _ in 0..20 {
            indicator.update(0.2);
        }

        assert!(indicator.is_finished());
    }
}
