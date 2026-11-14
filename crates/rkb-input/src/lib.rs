//! Touch and gesture input without platform dependencies.

#![forbid(unsafe_code)]

pub mod controls;
pub mod event;
pub mod gestures;

pub use controls::{TouchButton, VirtualJoystick};
pub use event::{TouchEvent, TouchId, TouchPhase, TouchPoint, TouchState};
pub use gestures::{DragDetector, DragUpdate, PinchDetector, PinchUpdate, Tap, TapDetector};
