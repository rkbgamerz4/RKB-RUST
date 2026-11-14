//! Pointer identity, touch phases, and active-touch bookkeeping.

use std::collections::BTreeMap;
use std::time::Duration;

use rkb_core::Vec2;

/// Stable pointer identifier supplied by the platform.
pub type TouchId = u64;

/// Android-compatible touch phases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

/// A single pointer sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchPoint {
    pub id: TouchId,
    pub position: Vec2,
    pub pressure: f32,
    pub timestamp: Duration,
}

impl TouchPoint {
    #[must_use]
    pub const fn new(id: TouchId, position: Vec2) -> Self {
        Self {
            id,
            position,
            pressure: 1.0,
            timestamp: Duration::ZERO,
        }
    }

    #[must_use]
    pub const fn with_pressure(mut self, pressure: f32) -> Self {
        self.pressure = pressure;
        self
    }

    #[must_use]
    pub const fn at_time(mut self, timestamp: Duration) -> Self {
        self.timestamp = timestamp;
        self
    }
}

/// A phase-tagged touch sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchEvent {
    pub phase: TouchPhase,
    pub point: TouchPoint,
}

impl TouchEvent {
    #[must_use]
    pub const fn new(phase: TouchPhase, point: TouchPoint) -> Self {
        Self { phase, point }
    }

    #[must_use]
    pub fn started(point: TouchPoint) -> Self {
        Self::new(TouchPhase::Started, point)
    }

    #[must_use]
    pub fn moved(point: TouchPoint) -> Self {
        Self::new(TouchPhase::Moved, point)
    }

    #[must_use]
    pub fn ended(point: TouchPoint) -> Self {
        Self::new(TouchPhase::Ended, point)
    }

    #[must_use]
    pub fn cancelled(point: TouchPoint) -> Self {
        Self::new(TouchPhase::Cancelled, point)
    }
}

/// Tracks all currently active pointers.
#[derive(Clone, Debug, Default)]
pub struct TouchState {
    active: BTreeMap<TouchId, TouchPoint>,
}

impl TouchState {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            active: BTreeMap::new(),
        }
    }

    /// Applies an event and returns whether it changed the active set.
    pub fn apply(&mut self, event: TouchEvent) -> bool {
        match event.phase {
            TouchPhase::Started => self.active.insert(event.point.id, event.point).is_none(),
            TouchPhase::Moved => {
                if let Some(point) = self.active.get_mut(&event.point.id) {
                    *point = event.point;
                    true
                } else {
                    false
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                self.active.remove(&event.point.id).is_some()
            }
        }
    }

    #[must_use]
    pub fn get(&self, id: TouchId) -> Option<&TouchPoint> {
        self.active.get(&id)
    }

    #[must_use]
    pub fn contains(&self, id: TouchId) -> bool {
        self.active.contains_key(&id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.active.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &TouchPoint> {
        self.active.values()
    }

    pub fn cancel_all(&mut self) {
        self.active.clear();
    }

    /// Recent pointer positions in ascending identifier order.
    pub fn positions(&self) -> impl Iterator<Item = Vec2> + '_ {
        self.active.values().map(|point| point.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_started_moved_and_ended() {
        let mut state = TouchState::new();
        let point = TouchPoint::new(1, Vec2::new(10.0, 20.0));
        assert!(state.apply(TouchEvent::started(point)));
        assert_eq!(state.len(), 1);

        let moved = TouchPoint::new(1, Vec2::new(30.0, 40.0));
        assert!(state.apply(TouchEvent::moved(moved)));
        assert_eq!(state.get(1).unwrap().position, Vec2::new(30.0, 40.0));

        assert!(state.apply(TouchEvent::ended(moved)));
        assert!(state.is_empty());
        assert!(!state.apply(TouchEvent::moved(moved)));
    }

    #[test]
    fn supports_multiple_pointers() {
        let mut state = TouchState::new();
        state.apply(TouchEvent::started(TouchPoint::new(2, Vec2::ZERO)));
        state.apply(TouchEvent::started(TouchPoint::new(7, Vec2::new(1.0, 1.0))));
        assert_eq!(state.len(), 2);
        assert_eq!(state.positions().count(), 2);
    }

    #[test]
    fn cancel_all_releases_every_pointer() {
        let mut state = TouchState::new();
        state.apply(TouchEvent::started(TouchPoint::new(1, Vec2::ZERO)));
        state.apply(TouchEvent::started(TouchPoint::new(2, Vec2::ONE)));
        state.cancel_all();
        assert!(state.is_empty());
    }
}
