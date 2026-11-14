//! Explicit application lifecycle state machine.
//!
//! Android delivers lifecycle callbacks in an order that differs from desktop
//! platforms. Keeping the state machine explicit makes those transitions
//! testable and prevents rendering while the app is paused.

use std::fmt;

/// Lifecycle state of the engine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    #[default]
    Created,
    Running,
    Paused,
    Stopping,
    Destroyed,
}

impl Stage {
    #[must_use]
    pub const fn can_render(self) -> bool {
        matches!(self, Self::Running)
    }

    #[must_use]
    pub const fn can_update(self) -> bool {
        matches!(self, Self::Running)
    }
}

/// Inputs accepted by the lifecycle state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleEvent {
    /// Start the application for the first time.
    Start,
    /// Resume after a pause (Android `onResume`).
    Resume,
    /// Pause (Android `onPause`).
    Pause,
    /// Begin shutdown (Android `onStop`/`onDestroy`).
    Stop,
    /// Release resources (Android `onDestroy`).
    Destroy,
}

/// Invalid transition details.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LifecycleError {
    pub from: Stage,
    pub event: LifecycleEvent,
}

impl fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid lifecycle transition {:?} from {:?}",
            self.event, self.from
        )
    }
}

impl std::error::Error for LifecycleError {}

/// Tracks the application lifecycle.
#[derive(Clone, Debug, Default)]
pub struct Lifecycle {
    stage: Stage,
}

impl Lifecycle {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            stage: Stage::Created,
        }
    }

    #[must_use]
    pub const fn stage(&self) -> Stage {
        self.stage
    }

    #[must_use]
    pub const fn can_render(&self) -> bool {
        self.stage.can_render()
    }

    /// Applies a lifecycle event.
    ///
    /// Duplicate pause/resume events are treated as no-ops, which matches
    /// Android's behaviour when an activity is recreated.
    pub fn dispatch(&mut self, event: LifecycleEvent) -> Result<Stage, LifecycleError> {
        let next = match (self.stage, event) {
            (Stage::Created, LifecycleEvent::Start) => Stage::Running,
            (Stage::Running, LifecycleEvent::Pause) => Stage::Paused,
            (Stage::Paused, LifecycleEvent::Resume) => Stage::Running,
            (Stage::Paused, LifecycleEvent::Pause) | (Stage::Running, LifecycleEvent::Resume) => {
                self.stage
            }
            (Stage::Running | Stage::Paused | Stage::Created, LifecycleEvent::Stop) => {
                Stage::Stopping
            }
            (
                Stage::Created | Stage::Running | Stage::Paused | Stage::Stopping,
                LifecycleEvent::Destroy,
            ) => Stage::Destroyed,
            (Stage::Destroyed, _) => {
                return Err(LifecycleError {
                    from: self.stage,
                    event,
                })
            }
            _ => {
                return Err(LifecycleError {
                    from: self.stage,
                    event,
                })
            }
        };
        self.stage = next;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_android_sequence() {
        let mut lifecycle = Lifecycle::new();
        assert_eq!(
            lifecycle.dispatch(LifecycleEvent::Start),
            Ok(Stage::Running)
        );
        assert_eq!(lifecycle.dispatch(LifecycleEvent::Pause), Ok(Stage::Paused));
        assert!(!lifecycle.can_render());
        assert_eq!(
            lifecycle.dispatch(LifecycleEvent::Resume),
            Ok(Stage::Running)
        );
        assert!(lifecycle.can_render());
        assert_eq!(
            lifecycle.dispatch(LifecycleEvent::Stop),
            Ok(Stage::Stopping)
        );
        assert_eq!(
            lifecycle.dispatch(LifecycleEvent::Destroy),
            Ok(Stage::Destroyed)
        );
    }

    #[test]
    fn duplicate_callbacks_are_idempotent() {
        let mut lifecycle = Lifecycle::new();
        lifecycle.dispatch(LifecycleEvent::Start).unwrap();
        assert_eq!(
            lifecycle.dispatch(LifecycleEvent::Resume),
            Ok(Stage::Running)
        );
        lifecycle.dispatch(LifecycleEvent::Pause).unwrap();
        assert_eq!(lifecycle.dispatch(LifecycleEvent::Pause), Ok(Stage::Paused));
        assert_eq!(
            lifecycle.dispatch(LifecycleEvent::Destroy),
            Ok(Stage::Destroyed)
        );
    }

    #[test]
    fn cannot_resume_before_start() {
        let mut lifecycle = Lifecycle::new();
        let error = lifecycle.dispatch(LifecycleEvent::Resume).unwrap_err();
        assert_eq!(error.from, Stage::Created);
        assert_eq!(error.event, LifecycleEvent::Resume);
    }

    #[test]
    fn destroyed_is_terminal() {
        let mut lifecycle = Lifecycle::new();
        lifecycle.dispatch(LifecycleEvent::Destroy).unwrap();
        assert!(lifecycle.dispatch(LifecycleEvent::Start).is_err());
    }
}
