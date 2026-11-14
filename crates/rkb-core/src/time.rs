//! Deterministic fixed-step timing and frame-rate sampling.

use std::time::Duration;

/// Result of advancing a [`FixedTimestep`] by one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StepResult {
    /// Number of simulation steps that should run this frame.
    pub steps: u32,
    /// Fraction (0..step) of the next step already accumulated, in `0.0..1.0`.
    pub alpha_percent: u8,
    /// Steps discarded because the frame exceeded the catch-up limit.
    pub dropped_steps: u32,
}

impl StepResult {
    /// Interpolation factor in `0.0..1.0`.
    #[must_use]
    pub fn alpha(self) -> f32 {
        f32::from(self.alpha_percent) / 100.0
    }
}

/// Accumulates frame time and converts it into a fixed number of simulation steps.
///
/// Long frames are clamped so a stalled device cannot spiral into an unbounded
/// catch-up loop.
#[derive(Clone, Debug)]
pub struct FixedTimestep {
    step: Duration,
    max_steps: u32,
    accumulator: Duration,
}

impl FixedTimestep {
    /// Creates a timestep of `hz` updates per second.
    ///
    /// # Panics
    ///
    /// Panics if `hz` is zero or `max_steps` is zero.
    #[must_use]
    pub fn from_hz(hz: u32, max_steps: u32) -> Self {
        assert!(hz > 0, "fixed timestep rate must be non-zero");
        assert!(max_steps > 0, "max_steps must be non-zero");
        Self {
            step: Duration::from_secs_f64(1.0 / f64::from(hz)),
            max_steps,
            accumulator: Duration::ZERO,
        }
    }

    #[must_use]
    pub fn step(&self) -> Duration {
        self.step
    }

    #[must_use]
    pub fn max_steps(&self) -> u32 {
        self.max_steps
    }

    #[must_use]
    pub fn accumulator(&self) -> Duration {
        self.accumulator
    }

    /// Advances the accumulator and returns how many steps to execute.
    pub fn advance(&mut self, frame_delta: Duration) -> StepResult {
        self.accumulator += frame_delta;

        let catch_up_limit = self.step.saturating_mul(self.max_steps);
        let mut dropped_steps = 0_u32;
        if self.accumulator > catch_up_limit {
            let excess = self.accumulator - catch_up_limit;
            let dropped = excess.as_nanos() / self.step.as_nanos().max(1);
            dropped_steps = u32::try_from(dropped).unwrap_or(u32::MAX);
            self.accumulator = catch_up_limit;
        }

        let steps = (self.accumulator.as_nanos() / self.step.as_nanos().max(1)) as u32;
        self.accumulator = self
            .accumulator
            .saturating_sub(self.step.saturating_mul(steps));

        let alpha = if self.step.is_zero() {
            0.0
        } else {
            self.accumulator.as_secs_f32() / self.step.as_secs_f32()
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let alpha_percent = (alpha.clamp(0.0, 0.999) * 100.0).round() as u8;

        StepResult {
            steps,
            alpha_percent,
            dropped_steps,
        }
    }

    /// Discards accumulated time, for example after resuming from suspension.
    pub fn reset(&mut self) {
        self.accumulator = Duration::ZERO;
    }
}

/// Rolling frame-rate estimator with a one-second window.
#[derive(Clone, Debug)]
pub struct RateCounter {
    frames: u32,
    elapsed: Duration,
    last_fps: f32,
}

impl Default for RateCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl RateCounter {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            frames: 0,
            elapsed: Duration::ZERO,
            last_fps: 0.0,
        }
    }

    pub fn tick(&mut self, delta: Duration) {
        self.frames += 1;
        self.elapsed += delta;
        if self.elapsed >= Duration::from_secs(1) {
            self.last_fps = self.frames as f32 / self.elapsed.as_secs_f32();
            self.frames = 0;
            self.elapsed = Duration::ZERO;
        }
    }

    #[must_use]
    pub fn fps(&self) -> f32 {
        self.last_fps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_partial_steps() {
        let mut timestep = FixedTimestep::from_hz(60, 4);
        let result = timestep.advance(Duration::from_millis(10));
        assert_eq!(result.steps, 0);
        assert!(result.alpha() > 0.5);

        let result = timestep.advance(Duration::from_millis(10));
        assert_eq!(result.steps, 1);
    }

    #[test]
    fn exact_step_produces_one_update() {
        let mut timestep = FixedTimestep::from_hz(50, 4);
        let result = timestep.advance(Duration::from_millis(20));
        assert_eq!(result.steps, 1);
        assert_eq!(result.alpha(), 0.0);
        assert_eq!(timestep.accumulator(), Duration::ZERO);
    }

    #[test]
    fn long_frame_is_clamped_and_reports_drops() {
        let mut timestep = FixedTimestep::from_hz(10, 3);
        let result = timestep.advance(Duration::from_secs(5));
        assert_eq!(result.steps, 3);
        assert_eq!(result.dropped_steps, 47);
        assert_eq!(timestep.accumulator(), Duration::ZERO);
    }

    #[test]
    fn reset_clears_partial_time() {
        let mut timestep = FixedTimestep::from_hz(60, 4);
        let _ = timestep.advance(Duration::from_millis(10));
        timestep.reset();
        assert_eq!(timestep.accumulator(), Duration::ZERO);
    }

    #[test]
    fn rate_counter_reports_after_window() {
        let mut counter = RateCounter::new();
        for _ in 0..120 {
            counter.tick(Duration::from_millis(10));
        }
        assert!((counter.fps() - 100.0).abs() < 0.01);
    }
}
