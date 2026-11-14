//! Platform boundary for RKB RUST.
//!
//! Rendering, input, and lifecycle are kept behind small traits so the same
//! runtime can execute on Android, in a headless test, or in the CLI demo.

#![forbid(unsafe_code)]

use std::fmt;
use std::time::{Duration, Instant};

/// Error returned by a presentation surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceError(pub String);

impl fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "surface error: {}", self.0)
    }
}

impl std::error::Error for SurfaceError {}

/// Receives completed RGBA8 frames.
pub trait Surface {
    /// Width and height in physical pixels.
    fn size(&self) -> (u32, u32);

    /// Presents a tightly packed RGBA8 frame.
    fn present(&mut self, rgba: &[u8]) -> Result<(), SurfaceError>;

    /// Called when the host surface dimensions change.
    fn resize(&mut self, width: u32, height: u32);
}

/// Monotonic frame clock.
pub trait FrameClock {
    /// Returns the time since the previous call.
    fn tick(&mut self) -> Duration;
}

/// Wall-clock frame timer with a configurable maximum delta.
#[derive(Debug)]
pub struct SystemClock {
    last: Instant,
    max_delta: Duration,
}

impl SystemClock {
    #[must_use]
    pub fn new(max_delta: Duration) -> Self {
        Self {
            last: Instant::now(),
            max_delta,
        }
    }
}

impl FrameClock for SystemClock {
    fn tick(&mut self) -> Duration {
        let now = Instant::now();
        let delta = now.saturating_duration_since(self.last).min(self.max_delta);
        self.last = now;
        delta
    }
}

/// Deterministic clock for tests and headless runs.
#[derive(Clone, Debug)]
pub struct ManualClock {
    delta: Duration,
    ticks: u64,
}

impl ManualClock {
    #[must_use]
    pub const fn new(delta: Duration) -> Self {
        Self { delta, ticks: 0 }
    }

    #[must_use]
    pub const fn ticks(&self) -> u64 {
        self.ticks
    }
}

impl FrameClock for ManualClock {
    fn tick(&mut self) -> Duration {
        self.ticks += 1;
        self.delta
    }
}

/// In-memory surface used by tests, CI, and the CLI demo.
#[derive(Clone, Debug)]
pub struct HeadlessSurface {
    width: u32,
    height: u32,
    frames_presented: u64,
    last_checksum: u64,
    hash_frames: bool,
}

impl HeadlessSurface {
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            frames_presented: 0,
            last_checksum: 0,
            hash_frames: true,
        }
    }

    /// Skips the per-frame checksum, matching hosts that only present pixels.
    #[must_use]
    pub const fn without_hashing(mut self) -> Self {
        self.hash_frames = false;
        self
    }

    #[must_use]
    pub const fn frames_presented(&self) -> u64 {
        self.frames_presented
    }

    #[must_use]
    pub const fn last_checksum(&self) -> u64 {
        self.last_checksum
    }
}

impl Surface for HeadlessSurface {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn present(&mut self, rgba: &[u8]) -> Result<(), SurfaceError> {
        let expected = self.width as usize * self.height as usize * 4;
        if rgba.len() != expected {
            return Err(SurfaceError(format!(
                "expected {expected} bytes, received {}",
                rgba.len()
            )));
        }
        if self.hash_frames {
            let mut hash = 0xcbf2_9ce4_8422_2325_u64;
            for byte in rgba {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            self.last_checksum = hash;
        }
        self.frames_presented += 1;
        Ok(())
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
    }
}

/// Information about the executing host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlatformInfo {
    pub os: &'static str,
    pub arch: &'static str,
    pub family: &'static str,
    pub android_api: Option<u32>,
}

impl PlatformInfo {
    #[must_use]
    pub const fn current() -> Self {
        Self {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            family: std::env::consts::FAMILY,
            android_api: None,
        }
    }

    #[must_use]
    pub const fn with_android_api(mut self, api: u32) -> Self {
        self.android_api = Some(api);
        self
    }

    #[must_use]
    pub fn is_android(&self) -> bool {
        self.os == "android"
    }

    #[must_use]
    pub fn is_arm64(&self) -> bool {
        self.arch == "aarch64"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_surface_accepts_exact_frame() {
        let mut surface = HeadlessSurface::new(2, 2);
        assert_eq!(surface.size(), (2, 2));
        surface.present(&[0_u8; 16]).unwrap();
        assert_eq!(surface.frames_presented(), 1);
        assert_ne!(surface.last_checksum(), 0);
    }

    #[test]
    fn headless_surface_rejects_wrong_frame_size() {
        let mut surface = HeadlessSurface::new(2, 2);
        assert!(surface.present(&[0_u8; 15]).is_err());
    }

    #[test]
    fn headless_surface_can_skip_hashing() {
        let mut surface = HeadlessSurface::new(2, 2).without_hashing();
        surface.present(&[7_u8; 16]).unwrap();
        assert_eq!(surface.frames_presented(), 1);
        assert_eq!(surface.last_checksum(), 0);
    }

    #[test]
    fn manual_clock_is_deterministic() {
        let mut clock = ManualClock::new(Duration::from_millis(16));
        assert_eq!(clock.tick(), Duration::from_millis(16));
        assert_eq!(clock.tick(), Duration::from_millis(16));
        assert_eq!(clock.ticks(), 2);
    }

    #[test]
    fn platform_info_reports_android_details() {
        let info = PlatformInfo::current().with_android_api(34);
        assert_eq!(info.android_api, Some(34));
        assert!(info.is_android() == cfg!(target_os = "android"));
    }
}
