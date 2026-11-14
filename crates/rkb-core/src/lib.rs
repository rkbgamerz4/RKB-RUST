//! Shared, dependency-free foundations for RKB RUST.
//!
//! The crate intentionally has no platform assumptions. It is used by the
//! renderer, input layer, runtime, Android bridge, and the headless demo.

#![forbid(unsafe_code)]

pub mod config;
pub mod lifecycle;
pub mod math;
pub mod time;

pub use config::{Config, ConfigError, EngineConfig, LogLevel};
pub use lifecycle::{Lifecycle, LifecycleError, LifecycleEvent, Stage};
pub use math::{Mat4, Rect, Vec2, Vec3};
pub use time::{FixedTimestep, RateCounter, StepResult};

/// Crate version as reported by the binary and the Android bridge.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
