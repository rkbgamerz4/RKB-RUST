//! On-screen joystick and touch-button widgets.

use rkb_core::{Rect, Vec2};

use crate::event::{TouchEvent, TouchId, TouchPhase};

/// A fixed or floating virtual analog stick.
#[derive(Clone, Debug)]
pub struct VirtualJoystick {
    center: Vec2,
    radius: f32,
    dead_zone: f32,
    active_id: Option<TouchId>,
    knob: Vec2,
    value: Vec2,
    touch_radius: f32,
}

impl VirtualJoystick {
    #[must_use]
    pub fn new(center: Vec2, radius: f32) -> Self {
        let radius = radius.max(1.0);
        Self {
            center,
            radius,
            dead_zone: 0.15,
            active_id: None,
            knob: center,
            value: Vec2::ZERO,
            touch_radius: radius * 1.5,
        }
    }

    #[must_use]
    pub const fn center(&self) -> Vec2 {
        self.center
    }

    #[must_use]
    pub const fn radius(&self) -> f32 {
        self.radius
    }

    #[must_use]
    pub const fn knob(&self) -> Vec2 {
        self.knob
    }

    /// Normalized direction and magnitude in `[-1, 1]` per axis.
    #[must_use]
    pub const fn value(&self) -> Vec2 {
        self.value
    }

    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active_id.is_some()
    }

    #[must_use]
    pub const fn active_id(&self) -> Option<TouchId> {
        self.active_id
    }

    pub fn set_dead_zone(&mut self, dead_zone: f32) {
        self.dead_zone = dead_zone.clamp(0.0, 0.95);
    }

    /// Repositions the joystick, releasing any active touch.
    pub fn set_geometry(&mut self, center: Vec2, radius: f32) {
        self.center = center;
        self.radius = radius.max(1.0);
        self.touch_radius = self.radius * 1.5;
        self.active_id = None;
        self.knob = center;
        self.value = Vec2::ZERO;
    }

    /// Routes a touch event and returns `true` if the joystick owns it.
    pub fn handle(&mut self, event: TouchEvent) -> bool {
        let point = event.point;
        match event.phase {
            TouchPhase::Started => {
                if self.active_id.is_some()
                    || point.position.distance(self.center) > self.touch_radius
                {
                    return false;
                }
                self.active_id = Some(point.id);
                self.update(point.position);
                true
            }
            TouchPhase::Moved => {
                if self.active_id != Some(point.id) {
                    return false;
                }
                self.update(point.position);
                true
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                if self.active_id != Some(point.id) {
                    return false;
                }
                self.active_id = None;
                self.knob = self.center;
                self.value = Vec2::ZERO;
                true
            }
        }
    }

    fn update(&mut self, position: Vec2) {
        let offset = position - self.center;
        let length = offset.length();
        let clamped = if length > self.radius {
            offset * (self.radius / length)
        } else {
            offset
        };
        self.knob = self.center + clamped;

        let raw_magnitude = (clamped.length() / self.radius).clamp(0.0, 1.0);
        if raw_magnitude <= self.dead_zone {
            self.value = Vec2::ZERO;
        } else {
            let normalized = clamped.normalize_or_zero();
            let magnitude = (raw_magnitude - self.dead_zone) / (1.0 - self.dead_zone);
            self.value = normalized * magnitude;
        }
    }
}

/// A rectangular on-screen button.
#[derive(Clone, Debug)]
pub struct TouchButton {
    bounds: Rect,
    label: String,
    active_id: Option<TouchId>,
    press_count: u32,
}

impl TouchButton {
    #[must_use]
    pub fn new(bounds: Rect, label: impl Into<String>) -> Self {
        Self {
            bounds,
            label: label.into(),
            active_id: None,
            press_count: 0,
        }
    }

    #[must_use]
    pub const fn bounds(&self) -> Rect {
        self.bounds
    }

    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Repositions the button, releasing any active press.
    pub fn set_bounds(&mut self, bounds: Rect) {
        self.bounds = bounds;
        self.active_id = None;
    }

    #[must_use]
    pub const fn is_pressed(&self) -> bool {
        self.active_id.is_some()
    }

    /// Number of completed presses, useful for one-shot actions.
    #[must_use]
    pub const fn press_count(&self) -> u32 {
        self.press_count
    }

    pub fn handle(&mut self, event: TouchEvent) -> bool {
        let point = event.point;
        match event.phase {
            TouchPhase::Started => {
                if self.active_id.is_some() || !self.bounds.contains(point.position) {
                    return false;
                }
                self.active_id = Some(point.id);
                true
            }
            TouchPhase::Moved => {
                if self.active_id != Some(point.id) {
                    return false;
                }
                if !self.bounds.contains(point.position) {
                    self.active_id = None;
                }
                true
            }
            TouchPhase::Ended => {
                if self.active_id != Some(point.id) {
                    return false;
                }
                if self.bounds.contains(point.position) {
                    self.press_count = self.press_count.saturating_add(1);
                }
                self.active_id = None;
                true
            }
            TouchPhase::Cancelled => {
                if self.active_id != Some(point.id) {
                    return false;
                }
                self.active_id = None;
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::TouchPoint;

    fn event(phase: TouchPhase, x: f32, y: f32) -> TouchEvent {
        TouchEvent::new(phase, TouchPoint::new(1, Vec2::new(x, y)))
    }

    #[test]
    fn joystick_clamps_to_radius_and_applies_dead_zone() {
        let mut joystick = VirtualJoystick::new(Vec2::new(100.0, 100.0), 50.0);
        assert!(joystick.handle(event(TouchPhase::Started, 105.0, 100.0)));
        assert_eq!(joystick.value(), Vec2::ZERO);

        assert!(joystick.handle(event(TouchPhase::Moved, 200.0, 100.0)));
        assert!((joystick.value().x - 1.0).abs() < 0.001);
        assert!(joystick.value().y.abs() < 0.001);
        assert!((joystick.knob().distance(joystick.center()) - 50.0).abs() < 0.001);

        assert!(joystick.handle(event(TouchPhase::Ended, 200.0, 100.0)));
        assert_eq!(joystick.value(), Vec2::ZERO);
        assert!(!joystick.is_active());
    }

    #[test]
    fn joystick_ignores_touches_outside_activation_radius() {
        let mut joystick = VirtualJoystick::new(Vec2::ZERO, 40.0);
        assert!(!joystick.handle(event(TouchPhase::Started, 200.0, 200.0)));
        assert!(!joystick.is_active());
    }

    #[test]
    fn button_press_counts_only_when_released_inside() {
        let mut button = TouchButton::new(Rect::new(10.0, 10.0, 40.0, 40.0), "JUMP");
        assert!(button.handle(event(TouchPhase::Started, 20.0, 20.0)));
        assert!(button.is_pressed());
        assert!(!button.handle(event(TouchPhase::Started, 20.0, 20.0)));
        assert!(button.handle(event(TouchPhase::Ended, 25.0, 25.0)));
        assert_eq!(button.press_count(), 1);

        button.handle(event(TouchPhase::Started, 20.0, 20.0));
        button.handle(event(TouchPhase::Moved, 100.0, 100.0));
        assert!(!button.is_pressed());
        button.handle(event(TouchPhase::Ended, 100.0, 100.0));
        assert_eq!(button.press_count(), 1);
    }
}
