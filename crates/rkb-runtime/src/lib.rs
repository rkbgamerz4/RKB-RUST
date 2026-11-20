//! Fixed-step runtime that owns lifecycle, timing, input state, rendering,
//! and frame presentation.

#![forbid(unsafe_code)]

use std::time::Duration;

use rkb_core::{EngineConfig, Lifecycle, LifecycleError, LifecycleEvent, Stage, Vec2};
use rkb_input::{TouchEvent, TouchState};
use rkb_platform::{Surface, SurfaceError};
use rkb_render::{
    Color, DrawList, Framebuffer, RenderBackend, RenderError, RenderStats, SoftwareBackend,
};

/// Per-simulation-step context.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UpdateContext {
    pub frame: u64,
    pub step: u32,
    pub dt: f32,
    pub elapsed: Duration,
    pub viewport: Vec2,
}

/// Application logic implemented by the demo and future games.
pub trait Game {
    fn on_start(&mut self) {}
    fn on_stage_change(&mut self, _stage: Stage) {}
    fn on_resize(&mut self, _viewport: Vec2) {}
    fn on_touch(&mut self, _event: TouchEvent) {}
    fn update(&mut self, _context: UpdateContext) {}
    fn render(&mut self, _list: &mut DrawList, _viewport: Vec2) {}
}

/// Runtime settings.
#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub engine: EngineConfig,
    pub clear_color: Color,
    /// Hash every presented frame.
    ///
    /// Deterministic consumers (tests, the CLI demo, visual regression) keep
    /// this enabled. Hosts that only present pixels, such as Android, disable
    /// it to avoid scanning the whole framebuffer every frame.
    pub checksum_frames: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            engine: EngineConfig::default(),
            clear_color: Color::rgb(14, 18, 28),
            checksum_frames: true,
        }
    }
}

/// Errors surfaced while presenting a frame.
#[derive(Debug)]
pub enum RuntimeError {
    Surface(SurfaceError),
    Render(RenderError),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Surface(error) => error.fmt(f),
            Self::Render(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<SurfaceError> for RuntimeError {
    fn from(error: SurfaceError) -> Self {
        Self::Surface(error)
    }
}

impl From<RenderError> for RuntimeError {
    fn from(error: RenderError) -> Self {
        Self::Render(error)
    }
}

/// Summary of a presented frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameReport {
    pub ran: bool,
    pub frame_index: u64,
    pub steps: u32,
    pub dropped_steps: u32,
    pub alpha: f32,
    pub fps: f32,
    pub elapsed: Duration,
    pub render: RenderStats,
    pub checksum: u64,
}

/// Owns the application loop state.
#[derive(Debug)]
pub struct Runtime<G: Game> {
    config: RuntimeConfig,
    lifecycle: Lifecycle,
    game: G,
    timestep: rkb_core::FixedTimestep,
    backend: SoftwareBackend,
    draw_list: DrawList,
    touches: TouchState,
    rate: rkb_core::RateCounter,
    frame_index: u64,
    simulation_time: Duration,
    elapsed_time: Duration,
    viewport: Vec2,
    last_report: FrameReport,
}

impl<G: Game> Runtime<G> {
    #[must_use]
    pub fn new(config: RuntimeConfig, game: G, width: u32, height: u32) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let timestep = rkb_core::FixedTimestep::from_hz(config.engine.fixed_hz, 8);
        let viewport = Vec2::new(width as f32, height as f32);
        let clear_color = config.clear_color;
        let mut game = game;
        game.on_resize(viewport);
        Self {
            config,
            lifecycle: Lifecycle::new(),
            game,
            timestep,
            backend: SoftwareBackend::new(width, height),
            draw_list: DrawList::new(clear_color),
            touches: TouchState::new(),
            rate: rkb_core::RateCounter::new(),
            frame_index: 0,
            simulation_time: Duration::ZERO,
            elapsed_time: Duration::ZERO,
            viewport,
            last_report: FrameReport::default(),
        }
    }

    #[must_use]
    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    #[must_use]
    pub fn game(&self) -> &G {
        &self.game
    }

    pub fn game_mut(&mut self) -> &mut G {
        &mut self.game
    }

    #[must_use]
    pub fn stage(&self) -> Stage {
        self.lifecycle.stage()
    }

    #[must_use]
    pub fn framebuffer(&self) -> &Framebuffer {
        self.backend.framebuffer()
    }

    #[must_use]
    pub fn touches(&self) -> &TouchState {
        &self.touches
    }

    #[must_use]
    pub const fn last_report(&self) -> FrameReport {
        self.last_report
    }

    #[must_use]
    pub fn viewport(&self) -> Vec2 {
        self.viewport
    }

    /// Starts the runtime. Safe to call once.
    pub fn start(&mut self) -> Result<Stage, LifecycleError> {
        let stage = self.lifecycle.dispatch(LifecycleEvent::Start)?;
        self.game.on_start();
        self.game.on_stage_change(stage);
        Ok(stage)
    }

    pub fn pause(&mut self) -> Result<Stage, LifecycleError> {
        let stage = self.lifecycle.dispatch(LifecycleEvent::Pause)?;
        self.touches.cancel_all();
        self.game.on_stage_change(stage);
        Ok(stage)
    }

    pub fn resume(&mut self) -> Result<Stage, LifecycleError> {
        let stage = self.lifecycle.dispatch(LifecycleEvent::Resume)?;
        self.timestep.reset();
        self.game.on_stage_change(stage);
        Ok(stage)
    }

    pub fn stop(&mut self) -> Result<Stage, LifecycleError> {
        let stage = self.lifecycle.dispatch(LifecycleEvent::Stop)?;
        self.touches.cancel_all();
        self.game.on_stage_change(stage);
        Ok(stage)
    }

    pub fn destroy(&mut self) -> Result<Stage, LifecycleError> {
        let stage = self.lifecycle.dispatch(LifecycleEvent::Destroy)?;
        self.game.on_stage_change(stage);
        Ok(stage)
    }

    /// Forwards a touch event to the game and active-touch tracker.
    pub fn touch(&mut self, event: TouchEvent) -> bool {
        if !self.lifecycle.stage().can_update() {
            return false;
        }
        let changed = self.touches.apply(event);
        self.game.on_touch(event);
        changed
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        self.viewport = Vec2::new(width as f32, height as f32);
        self.backend.resize(width, height);
        // Pointer coordinates are surface-relative, so a resize or rotation
        // invalidates every active touch. Drop them instead of delivering
        // stale positions to the game.
        self.touches.cancel_all();
        self.game.on_resize(self.viewport);
    }

    /// Advances, renders, and presents one frame.
    pub fn frame(
        &mut self,
        delta: Duration,
        surface: &mut dyn Surface,
    ) -> Result<FrameReport, RuntimeError> {
        if !self.lifecycle.stage().can_render() {
            self.last_report.ran = false;
            return Ok(self.last_report);
        }

        let max_delta = Duration::from_millis(u64::from(self.config.engine.max_delta_ms));
        let clamped_delta = delta.min(max_delta);
        let steps = self.timestep.advance(clamped_delta);
        let step_duration = self.timestep.step();
        let step_seconds = step_duration.as_secs_f32();

        for step in 0..steps.steps {
            self.simulation_time += step_duration;
            self.game.update(UpdateContext {
                frame: self.frame_index,
                step,
                dt: step_seconds,
                elapsed: self.simulation_time,
                viewport: self.viewport,
            });
        }

        self.draw_list.set_clear_color(self.config.clear_color);
        self.draw_list.clear_commands();
        self.game.render(&mut self.draw_list, self.viewport);
        let render = self.backend.render(&self.draw_list)?;
        let framebuffer = self.backend.framebuffer();
        surface.present(framebuffer.pixels())?;

        self.elapsed_time += clamped_delta;
        self.rate.tick(clamped_delta);
        self.frame_index += 1;

        self.last_report = FrameReport {
            ran: true,
            frame_index: self.frame_index,
            steps: steps.steps,
            dropped_steps: steps.dropped_steps,
            alpha: steps.alpha(),
            fps: self.rate.fps(),
            elapsed: self.elapsed_time,
            render,
            checksum: if self.config.checksum_frames {
                framebuffer.checksum()
            } else {
                0
            },
        };
        Ok(self.last_report)
    }

    /// Advances the simulation without rasterizing.
    ///
    /// The GPU backend presents its own swapchain images, so running the
    /// software rasterizer here would only burn CPU on a framebuffer nobody
    /// shows. The returned report carries frame timing but no software
    /// [`RenderStats`]; a GPU host reports its own draw counters instead.
    pub fn advance(&mut self, delta: Duration) -> Result<FrameReport, RuntimeError> {
        if !self.lifecycle.stage().can_render() {
            self.last_report.ran = false;
            return Ok(self.last_report);
        }

        let max_delta = Duration::from_millis(u64::from(self.config.engine.max_delta_ms));
        let clamped_delta = delta.min(max_delta);
        let steps = self.timestep.advance(clamped_delta);
        let step_duration = self.timestep.step();
        let step_seconds = step_duration.as_secs_f32();

        for step in 0..steps.steps {
            self.simulation_time += step_duration;
            self.game.update(UpdateContext {
                frame: self.frame_index,
                step,
                dt: step_seconds,
                elapsed: self.simulation_time,
                viewport: self.viewport,
            });
        }

        self.elapsed_time += clamped_delta;
        self.rate.tick(clamped_delta);
        self.frame_index += 1;

        self.last_report = FrameReport {
            ran: true,
            frame_index: self.frame_index,
            steps: steps.steps,
            dropped_steps: steps.dropped_steps,
            alpha: steps.alpha(),
            fps: self.rate.fps(),
            elapsed: self.elapsed_time,
            render: RenderStats::default(),
            checksum: 0,
        };
        Ok(self.last_report)
    }

    /// Renders without presenting, for embedding backends such as Android.
    pub fn render_only(&mut self) -> Result<FrameReport, RuntimeError> {
        if !self.lifecycle.stage().can_render() {
            self.last_report.ran = false;
            return Ok(self.last_report);
        }
        self.draw_list.set_clear_color(self.config.clear_color);
        self.draw_list.clear_commands();
        self.game.render(&mut self.draw_list, self.viewport);
        let render = self.backend.render(&self.draw_list)?;
        self.last_report.render = render;
        self.last_report.checksum = if self.config.checksum_frames {
            self.backend.framebuffer().checksum()
        } else {
            0
        };
        self.last_report.ran = true;
        self.last_report.frame_index = self.frame_index;
        Ok(self.last_report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rkb_input::{TouchPhase, TouchPoint};
    use rkb_platform::HeadlessSurface;

    #[derive(Debug, Default)]
    struct TestGame {
        updates: u32,
        renders: u32,
        touches: u32,
        resizes: u32,
        last_dt: f32,
        last_viewport: Vec2,
    }

    impl Game for TestGame {
        fn on_touch(&mut self, _event: TouchEvent) {
            self.touches += 1;
        }

        fn on_resize(&mut self, viewport: Vec2) {
            self.resizes += 1;
            self.last_viewport = viewport;
        }

        fn update(&mut self, context: UpdateContext) {
            self.updates += 1;
            self.last_dt = context.dt;
        }

        fn render(&mut self, list: &mut DrawList, viewport: Vec2) {
            self.renders += 1;
            list.rect(
                rkb_core::Rect::new(0.0, 0.0, viewport.x, viewport.y),
                Color::rgba(255, 255, 255, 8),
            );
        }
    }

    fn runtime() -> Runtime<TestGame> {
        Runtime::new(RuntimeConfig::default(), TestGame::default(), 32, 32)
    }

    #[test]
    fn does_not_run_before_start() {
        let mut runtime = runtime();
        let mut surface = HeadlessSurface::new(32, 32);
        let report = runtime
            .frame(Duration::from_millis(16), &mut surface)
            .unwrap();
        assert!(!report.ran);
        assert_eq!(runtime.game().updates, 0);
        assert_eq!(surface.frames_presented(), 0);
    }

    #[test]
    fn advance_updates_without_rasterizing_for_the_gpu_path() {
        let mut runtime = runtime();
        runtime.start().unwrap();
        let report = runtime.advance(Duration::from_millis(32)).unwrap();
        assert!(report.ran);
        assert!(runtime.game().updates > 0, "simulation must still advance");
        assert_eq!(
            runtime.game().renders,
            0,
            "the GPU path must not spend CPU on the software rasterizer"
        );
        assert_eq!(report.checksum, 0);
        assert!(report.fps > 0.0 || report.elapsed > Duration::ZERO);

        // Paused hosts still skip frames exactly like the software path.
        runtime.pause().unwrap();
        let skipped = runtime.advance(Duration::from_millis(16)).unwrap();
        assert!(!skipped.ran);
        let updates = runtime.game().updates;
        runtime.advance(Duration::from_millis(16)).unwrap();
        assert_eq!(runtime.game().updates, updates);
    }

    #[test]
    fn runs_fixed_steps_and_presents() {
        let mut runtime = runtime();
        runtime.start().unwrap();
        let mut surface = HeadlessSurface::new(32, 32);
        let report = runtime
            .frame(Duration::from_millis(34), &mut surface)
            .unwrap();
        assert!(report.ran);
        assert_eq!(report.steps, 2);
        assert_eq!(runtime.game().updates, 2);
        assert_eq!(runtime.game().renders, 1);
        assert_eq!(surface.frames_presented(), 1);
        assert!((runtime.game().last_dt - 1.0 / 60.0).abs() < 0.000_1);
        assert_ne!(report.checksum, 0);
    }

    #[test]
    fn pause_stops_updates_and_cancels_touches() {
        let mut runtime = runtime();
        runtime.start().unwrap();
        runtime.touch(TouchEvent::started(TouchPoint::new(1, Vec2::ONE)));
        runtime.pause().unwrap();
        assert!(runtime.touches().is_empty());
        let mut surface = HeadlessSurface::new(32, 32);
        let report = runtime
            .frame(Duration::from_millis(16), &mut surface)
            .unwrap();
        assert!(!report.ran);
        runtime.resume().unwrap();
        assert!(
            runtime
                .frame(Duration::from_millis(16), &mut surface)
                .unwrap()
                .ran
        );
        assert_eq!(runtime.game().touches, 1);
    }

    #[test]
    fn long_frames_report_dropped_steps() {
        let mut runtime = runtime();
        runtime.start().unwrap();
        let mut surface = HeadlessSurface::new(32, 32);
        let report = runtime.frame(Duration::from_secs(2), &mut surface).unwrap();
        // max_delta_ms is 250, so 8 steps run and the remainder is dropped.
        assert_eq!(report.steps, 8);
        assert!(report.dropped_steps > 0);
    }

    #[test]
    fn resize_updates_viewport_and_framebuffer() {
        let mut runtime = runtime();
        runtime.start().unwrap();
        runtime.resize(100, 50);
        assert_eq!(runtime.viewport(), Vec2::new(100.0, 50.0));
        assert_eq!(runtime.framebuffer().width(), 100);
        assert_eq!(runtime.framebuffer().height(), 50);
    }

    #[test]
    fn resize_cancels_active_touches_and_notifies_game() {
        let mut runtime = runtime();
        runtime.start().unwrap();
        runtime.touch(TouchEvent::started(TouchPoint::new(7, Vec2::new(4.0, 4.0))));
        runtime.touch(TouchEvent::started(TouchPoint::new(8, Vec2::new(5.0, 5.0))));
        assert_eq!(runtime.touches().len(), 2);

        runtime.resize(64, 48);
        assert!(runtime.touches().is_empty());
        assert_eq!(runtime.viewport(), Vec2::new(64.0, 48.0));
        assert_eq!(runtime.game().last_viewport, Vec2::new(64.0, 48.0));
        // One notification during construction, one for the resize.
        assert_eq!(runtime.game().resizes, 2);
    }

    #[test]
    fn frame_checksums_can_be_disabled_for_present_only_hosts() {
        let config = RuntimeConfig {
            checksum_frames: false,
            ..RuntimeConfig::default()
        };
        let mut runtime = Runtime::new(config, TestGame::default(), 16, 16);
        runtime.start().unwrap();
        let mut surface = HeadlessSurface::new(16, 16);
        let report = runtime
            .frame(Duration::from_millis(16), &mut surface)
            .unwrap();
        assert!(report.ran);
        assert_eq!(report.checksum, 0);
        assert_ne!(surface.last_checksum(), 0);
    }

    #[test]
    fn repeated_frames_reuse_the_draw_list() {
        let mut runtime = runtime();
        runtime.start().unwrap();
        let mut surface = HeadlessSurface::new(32, 32);
        for _ in 0..32 {
            let report = runtime
                .frame(Duration::from_millis(16), &mut surface)
                .unwrap();
            assert!(report.ran);
        }
        assert_eq!(runtime.game().renders, 32);
        assert_eq!(surface.frames_presented(), 32);
        // The reused list holds exactly the commands from the latest frame.
        assert_eq!(runtime.draw_list.len(), 1);
    }

    #[test]
    fn invalid_lifecycle_transition_is_reported() {
        let mut runtime = runtime();
        assert!(runtime.resume().is_err());
    }

    #[test]
    fn touch_ignored_while_paused() {
        let mut runtime = runtime();
        runtime.start().unwrap();
        runtime.pause().unwrap();
        let changed = runtime.touch(TouchEvent::new(
            TouchPhase::Started,
            TouchPoint::new(4, Vec2::new(1.0, 1.0)),
        ));
        assert!(!changed);
        assert_eq!(runtime.game().touches, 0);
    }
}
