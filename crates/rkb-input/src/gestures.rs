//! Tap, drag, and two-finger pinch recognizers.

use std::time::Duration;

use rkb_core::Vec2;

use crate::event::{TouchEvent, TouchId, TouchPhase, TouchState};

/// A completed tap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tap {
    pub id: TouchId,
    pub position: Vec2,
    pub duration: Duration,
}

/// Recognizes taps with configurable time and movement limits.
#[derive(Clone, Debug)]
pub struct TapDetector {
    max_duration: Duration,
    max_distance: f32,
    tracker: Option<(TouchId, Vec2, Duration)>,
}

impl Default for TapDetector {
    fn default() -> Self {
        Self::new(Duration::from_millis(300), 24.0)
    }
}

impl TapDetector {
    #[must_use]
    pub const fn new(max_duration: Duration, max_distance: f32) -> Self {
        Self {
            max_duration,
            max_distance,
            tracker: None,
        }
    }

    pub fn feed(&mut self, event: TouchEvent) -> Option<Tap> {
        let point = event.point;
        match event.phase {
            TouchPhase::Started => {
                self.tracker = Some((point.id, point.position, point.timestamp));
                None
            }
            TouchPhase::Moved => {
                if let Some((id, start, _)) = self.tracker {
                    if id == point.id && start.distance(point.position) > self.max_distance {
                        self.tracker = None;
                    }
                }
                None
            }
            TouchPhase::Cancelled => {
                self.tracker = None;
                None
            }
            TouchPhase::Ended => {
                let (id, start, started_at) = self.tracker.take()?;
                if id != point.id {
                    return None;
                }
                let duration = point.timestamp.saturating_sub(started_at);
                if duration <= self.max_duration
                    && start.distance(point.position) <= self.max_distance
                {
                    Some(Tap {
                        id,
                        position: point.position,
                        duration,
                    })
                } else {
                    None
                }
            }
        }
    }
}

/// A movement sample from an active drag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragUpdate {
    pub id: TouchId,
    pub position: Vec2,
    /// Movement since the previous update.
    pub delta: Vec2,
    /// Movement since the initial press.
    pub total: Vec2,
}

#[derive(Clone, Copy, Debug)]
struct DragTracker {
    id: TouchId,
    start: Vec2,
    last: Vec2,
    active: bool,
}

/// Recognizes drags after a small movement threshold.
#[derive(Clone, Debug)]
pub struct DragDetector {
    slop: f32,
    tracker: Option<DragTracker>,
}

impl Default for DragDetector {
    fn default() -> Self {
        Self::new(8.0)
    }
}

impl DragDetector {
    #[must_use]
    pub fn new(slop: f32) -> Self {
        Self {
            slop: slop.max(0.0),
            tracker: None,
        }
    }

    #[must_use]
    pub fn is_dragging(&self) -> bool {
        self.tracker.is_some_and(|tracker| tracker.active)
    }

    pub fn feed(&mut self, event: TouchEvent) -> Option<DragUpdate> {
        let point = event.point;
        match event.phase {
            TouchPhase::Started => {
                self.tracker = Some(DragTracker {
                    id: point.id,
                    start: point.position,
                    last: point.position,
                    active: false,
                });
                None
            }
            TouchPhase::Moved => {
                let tracker = self.tracker.as_mut()?;
                if tracker.id != point.id {
                    return None;
                }
                let total = point.position - tracker.start;
                let delta = point.position - tracker.last;
                tracker.last = point.position;
                if !tracker.active && total.length() < self.slop {
                    return None;
                }
                tracker.active = true;
                Some(DragUpdate {
                    id: tracker.id,
                    position: point.position,
                    delta,
                    total,
                })
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                if self.tracker.is_some_and(|tracker| tracker.id == point.id) {
                    self.tracker = None;
                }
                None
            }
        }
    }
}

/// A two-finger pinch sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PinchUpdate {
    /// Scale relative to the gesture's initial distance.
    pub scale: f32,
    /// Scale relative to the previous update.
    pub delta: f32,
    pub center: Vec2,
    pub distance: f32,
}

/// Recognizes a pinch from exactly two active pointers.
#[derive(Clone, Debug, Default)]
pub struct PinchDetector {
    pair: Option<(TouchId, TouchId)>,
    baseline: f32,
    last_distance: f32,
}

impl PinchDetector {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            pair: None,
            baseline: 0.0,
            last_distance: 0.0,
        }
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.pair.is_some()
    }

    /// Updates the recognizer from the current active-touch set.
    pub fn update(&mut self, touches: &TouchState) -> Option<PinchUpdate> {
        if touches.len() != 2 {
            self.reset();
            return None;
        }
        let mut points = touches.iter();
        let first = points.next()?;
        let second = points.next()?;
        let pair = (first.id.min(second.id), first.id.max(second.id));
        let distance = first.position.distance(second.position);
        let center = (first.position + second.position) * 0.5;

        if self.pair != Some(pair) {
            self.pair = Some(pair);
            self.baseline = distance.max(f32::EPSILON);
            self.last_distance = self.baseline;
            return Some(PinchUpdate {
                scale: 1.0,
                delta: 1.0,
                center,
                distance,
            });
        }

        let delta = distance / self.last_distance.max(f32::EPSILON);
        self.last_distance = distance;
        Some(PinchUpdate {
            scale: distance / self.baseline.max(f32::EPSILON),
            delta,
            center,
            distance,
        })
    }

    pub fn reset(&mut self) {
        self.pair = None;
        self.baseline = 0.0;
        self.last_distance = 0.0;
    }
}

#[cfg(test)]
fn point(id: TouchId, x: f32, y: f32, millis: u64) -> crate::event::TouchPoint {
    crate::event::TouchPoint::new(id, Vec2::new(x, y)).at_time(Duration::from_millis(millis))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_within_limits_is_recognized() {
        let mut detector = TapDetector::default();
        assert!(detector
            .feed(TouchEvent::started(point(1, 10.0, 10.0, 0)))
            .is_none());
        assert!(detector
            .feed(TouchEvent::moved(point(1, 12.0, 11.0, 100)))
            .is_none());
        let tap = detector
            .feed(TouchEvent::ended(point(1, 12.0, 11.0, 150)))
            .unwrap();
        assert_eq!(tap.id, 1);
        assert_eq!(tap.duration, Duration::from_millis(150));
    }

    #[test]
    fn tap_rejected_when_too_slow_or_far() {
        let mut detector = TapDetector::new(Duration::from_millis(100), 5.0);
        detector.feed(TouchEvent::started(point(1, 0.0, 0.0, 0)));
        assert!(detector
            .feed(TouchEvent::ended(point(1, 0.0, 0.0, 200)))
            .is_none());

        detector.feed(TouchEvent::started(point(2, 0.0, 0.0, 0)));
        assert!(detector
            .feed(TouchEvent::ended(point(2, 20.0, 0.0, 50)))
            .is_none());
    }

    #[test]
    fn drag_starts_after_slop() {
        let mut detector = DragDetector::new(10.0);
        detector.feed(TouchEvent::started(point(1, 0.0, 0.0, 0)));
        assert!(detector
            .feed(TouchEvent::moved(point(1, 4.0, 0.0, 16)))
            .is_none());
        let update = detector
            .feed(TouchEvent::moved(point(1, 15.0, 0.0, 32)))
            .unwrap();
        assert_eq!(update.total, Vec2::new(15.0, 0.0));
        assert!(detector.is_dragging());
        detector.feed(TouchEvent::ended(point(1, 15.0, 0.0, 48)));
        assert!(!detector.is_dragging());
    }

    #[test]
    fn pinch_scales_and_resets() {
        let mut state = TouchState::new();
        state.apply(TouchEvent::started(point(1, 0.0, 0.0, 0)));
        state.apply(TouchEvent::started(point(2, 100.0, 0.0, 0)));
        let mut detector = PinchDetector::new();
        let first = detector.update(&state).unwrap();
        assert_eq!(first.scale, 1.0);
        assert_eq!(first.center, Vec2::new(50.0, 0.0));

        state.apply(TouchEvent::moved(point(2, 200.0, 0.0, 16)));
        let second = detector.update(&state).unwrap();
        assert!((second.scale - 2.0).abs() < 0.001);
        assert!((second.delta - 2.0).abs() < 0.001);

        state.apply(TouchEvent::ended(point(2, 200.0, 0.0, 32)));
        assert!(detector.update(&state).is_none());
        assert!(!detector.is_active());
    }
}
