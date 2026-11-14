//! Interactive sandbox scene used as executable proof of the RKB RUST stack.
//!
//! It exercises every foundation crate: configuration and resources are loaded
//! from embedded assets, touch input drives a virtual joystick and button,
//! taps spawn entities, pinches zoom the camera, and the software renderer
//! presents a complete frame.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;

use rkb_core::{Config, ConfigError, EngineConfig, Rect, Vec2};
use rkb_input::{
    DragDetector, DragUpdate, PinchDetector, Tap, TapDetector, TouchButton, TouchEvent, TouchId,
    TouchPhase, TouchState, VirtualJoystick,
};
use rkb_render::{Color, ColorError, DrawList};
use rkb_resource::{AssetPath, CacheStats, MemorySource, ResourceError, ResourceManager};
use rkb_runtime::{Game, UpdateContext};

const ENGINE_ASSET: &str = "config/engine.rkb";
const SCENE_ASSET: &str = "scenes/sandbox.rkb";
const ENGINE_BYTES: &[u8] = include_bytes!("../assets/engine.rkb");
const SCENE_BYTES: &[u8] = include_bytes!("../assets/scene.rkb");
const MAX_BALLS: usize = 96;

/// Failure while bootstrapping the sandbox.
#[derive(Debug)]
pub enum SandboxError {
    Resource(ResourceError),
    Config(ConfigError),
    Color(ColorError),
}

impl fmt::Display for SandboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resource(error) => error.fmt(f),
            Self::Config(error) => error.fmt(f),
            Self::Color(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for SandboxError {}

impl From<ResourceError> for SandboxError {
    fn from(error: ResourceError) -> Self {
        Self::Resource(error)
    }
}

impl From<ConfigError> for SandboxError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<ColorError> for SandboxError {
    fn from(error: ColorError) -> Self {
        Self::Color(error)
    }
}

/// Configuration and scene produced from embedded assets.
#[derive(Debug)]
pub struct SandboxBootstrap {
    pub engine: EngineConfig,
    pub sandbox: Sandbox,
}

/// Builds the sandbox and its engine configuration from embedded assets.
pub fn bootstrap(viewport: Vec2) -> Result<SandboxBootstrap, SandboxError> {
    let memory = MemorySource::from_entries([
        (ENGINE_ASSET, ENGINE_BYTES.to_vec()),
        (SCENE_ASSET, SCENE_BYTES.to_vec()),
    ])?;
    let mut resources = ResourceManager::new(memory, 256 * 1024);
    let engine_path = AssetPath::new(ENGINE_ASSET)?;
    let scene_path = AssetPath::new(SCENE_ASSET)?;
    let engine_text = resources.load_text(&engine_path)?;
    let scene_text = resources.load_text(&scene_path)?;
    let engine = EngineConfig::from_config(&Config::parse(&engine_text)?)?;
    let scene = SceneConfig::parse(&scene_text)?;
    Ok(SandboxBootstrap {
        engine,
        sandbox: Sandbox::new(viewport, scene, resources),
    })
}

#[derive(Clone, Debug)]
struct SceneConfig {
    title: String,
    clear_color: Color,
    grid: bool,
    ball_count: usize,
    world_margin: f32,
    seed: u64,
}

impl SceneConfig {
    fn parse(input: &str) -> Result<Self, SandboxError> {
        let config = Config::parse(input)?;
        let title = config.get("title").unwrap_or("RKB RUST SANDBOX").to_owned();
        let clear_color = Color::from_hex(config.get_or("clear_color", "#101624"))?;
        let grid = config.get_bool("grid")?.unwrap_or(true);
        let ball_count = config
            .get_parsed::<usize>("ball_count")?
            .unwrap_or(10)
            .clamp(1, MAX_BALLS);
        let world_margin = config
            .get_parsed::<f32>("world_margin")?
            .unwrap_or(56.0)
            .clamp(0.0, 200.0);
        let seed = config.get_parsed::<u64>("seed")?.unwrap_or(0x5eed);
        Ok(Self {
            title,
            clear_color,
            grid,
            ball_count,
            world_margin,
            seed,
        })
    }
}

#[derive(Clone, Debug)]
struct Ball {
    position: Vec2,
    velocity: Vec2,
    radius: f32,
    color: Color,
    life: Option<f32>,
    spin: f32,
}

impl Ball {
    fn transient(position: Vec2, velocity: Vec2, color: Color) -> Self {
        Self {
            position,
            velocity,
            radius: 4.0,
            color,
            life: Some(2.2),
            spin: 0.0,
        }
    }
}

#[derive(Clone, Debug)]
struct Player {
    position: Vec2,
    velocity: Vec2,
}

/// Snapshot of sandbox counters, exposed to the CLI and Android bridge.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SandboxStats {
    pub balls: usize,
    pub taps: u32,
    pub bursts: u32,
    pub zoom: f32,
    pub resources: CacheStats,
}

/// The actual touch-driven scene.
#[derive(Debug)]
pub struct Sandbox {
    scene: SceneConfig,
    resources: ResourceManager,
    camera: rkb_render::Camera2D,
    viewport: Vec2,
    player: Player,
    balls: Vec<Ball>,
    joystick: VirtualJoystick,
    button: TouchButton,
    touches: TouchState,
    markers: BTreeMap<TouchId, Vec2>,
    taps: TapDetector,
    drag: DragDetector,
    pinch: PinchDetector,
    rng: Rng,
    tap_count: u32,
    burst_count: u32,
    last_button_presses: u32,
    time: f32,
    started: bool,
}

impl Sandbox {
    fn new(viewport: Vec2, scene: SceneConfig, resources: ResourceManager) -> Self {
        let viewport = Vec2::new(viewport.x.max(1.0), viewport.y.max(1.0));
        let seed = scene.seed;
        let mut sandbox = Self {
            scene,
            resources,
            camera: rkb_render::Camera2D::new(viewport),
            viewport,
            player: Player {
                position: Vec2::ZERO,
                velocity: Vec2::ZERO,
            },
            balls: Vec::new(),
            joystick: VirtualJoystick::new(Vec2::new(110.0, viewport.y - 110.0), 64.0),
            button: TouchButton::new(
                Rect::new(viewport.x - 180.0, viewport.y - 180.0, 110.0, 90.0),
                "BURST",
            ),
            touches: TouchState::new(),
            markers: BTreeMap::new(),
            taps: TapDetector::default(),
            drag: DragDetector::default(),
            pinch: PinchDetector::new(),
            rng: Rng::new(seed),
            tap_count: 0,
            burst_count: 0,
            last_button_presses: 0,
            time: 0.0,
            started: false,
        };
        sandbox.layout(viewport);
        let count = sandbox.scene.ball_count;
        for _ in 0..count {
            sandbox.spawn_ball_random();
        }
        sandbox
    }

    #[must_use]
    pub fn title(&self) -> &str {
        &self.scene.title
    }

    #[must_use]
    pub fn stats(&self) -> SandboxStats {
        SandboxStats {
            balls: self.balls.len(),
            taps: self.tap_count,
            bursts: self.burst_count,
            zoom: self.camera.zoom,
            resources: self.resources.stats(),
        }
    }

    #[must_use]
    pub fn player_position(&self) -> Vec2 {
        self.player.position
    }

    #[must_use]
    pub fn zoom(&self) -> f32 {
        self.camera.zoom
    }

    #[must_use]
    pub fn started(&self) -> bool {
        self.started
    }

    fn layout(&mut self, viewport: Vec2) {
        self.viewport = Vec2::new(viewport.x.max(1.0), viewport.y.max(1.0));
        // Touch coordinates are relative to the old surface. A resize (for
        // example, rotation) invalidates every active pointer and gesture, so
        // start from a clean interaction state instead of dragging stale
        // positions into the new layout.
        self.touches.cancel_all();
        self.markers.clear();
        self.taps = TapDetector::default();
        self.drag = DragDetector::default();
        self.pinch.reset();
        self.camera.set_viewport(self.viewport.x, self.viewport.y);
        let scale = (self.viewport.y / 720.0).clamp(0.7, 1.5);
        let radius = 72.0 * scale;
        self.joystick.set_geometry(
            Vec2::new(
                100.0 * scale + radius,
                self.viewport.y - 100.0 * scale - radius,
            ),
            radius,
        );
        let button_size = 100.0 * scale;
        self.button.set_bounds(Rect::new(
            self.viewport.x - 70.0 * scale - button_size,
            self.viewport.y - 70.0 * scale - button_size,
            button_size,
            button_size,
        ));
    }

    fn world_rect(&self) -> Rect {
        let size = self.viewport / self.camera.zoom.max(0.01);
        Rect::from_center(Vec2::ZERO, size * 0.86)
    }

    fn spawn_ball_random(&mut self) {
        let rect = self.world_rect().inset(self.scene.world_margin);
        let position = Vec2::new(
            self.rng.range(rect.x, rect.x + rect.width),
            self.rng.range(rect.y, rect.y + rect.height),
        );
        let speed = self.rng.range(45.0, 150.0);
        let angle = self.rng.range(0.0, std::f32::consts::TAU);
        let velocity = Vec2::new(angle.cos(), angle.sin()) * speed;
        let palette = [
            Color::rgb(92, 200, 255),
            Color::rgb(255, 120, 180),
            Color::rgb(120, 240, 180),
            Color::rgb(255, 205, 100),
        ];
        let color = palette[self.rng.next_u32() as usize % palette.len()];
        let radius = self.rng.range(6.0, 16.0);
        let spin = self.rng.range(-3.0, 3.0);
        self.push_ball(Ball {
            position,
            velocity,
            radius,
            color,
            life: None,
            spin,
        });
    }

    fn push_ball(&mut self, ball: Ball) {
        if self.balls.len() >= MAX_BALLS {
            self.balls.remove(0);
        }
        self.balls.push(ball);
    }

    fn spawn_burst(&mut self) {
        self.burst_count = self.burst_count.saturating_add(1);
        let palette = [
            Color::rgb(92, 200, 255),
            Color::rgb(255, 120, 180),
            Color::rgb(120, 240, 180),
        ];
        for index in 0..10 {
            let angle = std::f32::consts::TAU * index as f32 / 10.0;
            let speed = self.rng.range(90.0, 220.0);
            let color = palette[index % palette.len()];
            self.push_ball(Ball::transient(
                self.player.position,
                Vec2::new(angle.cos(), angle.sin()) * speed,
                color,
            ));
        }
    }

    fn handle_touch(&mut self, event: TouchEvent) {
        self.touches.apply(event);
        if event.phase == TouchPhase::Started {
            self.markers.insert(event.point.id, event.point.position);
        } else if matches!(event.phase, TouchPhase::Ended | TouchPhase::Cancelled) {
            self.markers.remove(&event.point.id);
        } else if let Some(marker) = self.markers.get_mut(&event.point.id) {
            *marker = event.point.position;
        }

        let button_handled = self.button.handle(event);
        let joystick_handled = self.joystick.handle(event);
        if self.button.press_count() != self.last_button_presses {
            self.last_button_presses = self.button.press_count();
            self.spawn_burst();
        }

        if let Some(pinch) = self.pinch.update(&self.touches) {
            if (pinch.delta - 1.0).abs() > f32::EPSILON {
                self.camera.zoom = (self.camera.zoom * pinch.delta).clamp(0.45, 3.0);
                self.clamp_camera();
            }
        }

        if self.touches.len() > 1 {
            self.taps = TapDetector::default();
            self.drag = DragDetector::default();
            return;
        }

        if button_handled || joystick_handled {
            return;
        }

        if let Some(tap) = self.taps.feed(event) {
            self.tap_count = self.tap_count.saturating_add(1);
            self.spawn_tap_ball(tap);
        }
        if let Some(drag) = self.drag.feed(event) {
            self.pan_camera(drag);
        }
    }

    fn spawn_tap_ball(&mut self, tap: Tap) {
        let world = self.camera.screen_to_world(tap.position);
        let palette = [
            Color::rgb(120, 240, 180),
            Color::rgb(255, 120, 180),
            Color::rgb(255, 205, 100),
        ];
        let color = palette[self.rng.next_u32() as usize % palette.len()];
        let speed = self.rng.range(40.0, 110.0);
        let angle = self.rng.range(0.0, std::f32::consts::TAU);
        let ball = Ball {
            position: world,
            velocity: Vec2::new(angle.cos(), angle.sin()) * speed,
            radius: self.rng.range(8.0, 18.0),
            color,
            life: None,
            spin: self.rng.range(-2.0, 2.0),
        };
        self.push_ball(ball);
    }

    fn pan_camera(&mut self, drag: DragUpdate) {
        self.camera.position -= drag.delta / self.camera.zoom;
        self.clamp_camera();
    }

    fn clamp_camera(&mut self) {
        let limit = self.viewport / self.camera.zoom * 0.35;
        self.camera.position.x = self.camera.position.x.clamp(-limit.x, limit.x);
        self.camera.position.y = self.camera.position.y.clamp(-limit.y, limit.y);
    }

    fn step(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.1);
        self.time += dt;
        let world = self.world_rect().inset(self.scene.world_margin);

        let target = self.joystick.value() * 280.0;
        self.player.velocity = self.player.velocity.lerp(target, (dt * 12.0).min(1.0));
        self.player.position += self.player.velocity * dt;
        let player_radius = 16.0;
        self.player.position.x = self.player.position.x.clamp(
            world.x + player_radius,
            world.x + world.width - player_radius,
        );
        self.player.position.y = self.player.position.y.clamp(
            world.y + player_radius,
            world.y + world.height - player_radius,
        );

        for ball in &mut self.balls {
            if let Some(life) = &mut ball.life {
                *life -= dt;
            }
            ball.position += ball.velocity * dt;
            let radius = ball.radius;
            if ball.position.x - radius < world.x {
                ball.position.x = world.x + radius;
                ball.velocity.x = ball.velocity.x.abs();
            } else if ball.position.x + radius > world.x + world.width {
                ball.position.x = world.x + world.width - radius;
                ball.velocity.x = -ball.velocity.x.abs();
            }
            if ball.position.y - radius < world.y {
                ball.position.y = world.y + radius;
                ball.velocity.y = ball.velocity.y.abs();
            } else if ball.position.y + radius > world.y + world.height {
                ball.position.y = world.y + world.height - radius;
                ball.velocity.y = -ball.velocity.y.abs();
            }
        }
        self.balls
            .retain(|ball| ball.life.map_or(true, |life| life > 0.0));
    }

    fn draw(&self, list: &mut DrawList) {
        list.set_clear_color(self.scene.clear_color);
        if self.scene.grid {
            self.draw_grid(list);
        }

        for ball in &self.balls {
            let screen = self.camera.world_to_screen(ball.position);
            let radius = (ball.radius * self.camera.zoom).max(1.5);
            let alpha = ball
                .life
                .map_or(255, |life| ((life / 2.2).clamp(0.0, 1.0) * 255.0) as u8);
            let color = ball.color.with_alpha(alpha);
            let tail = self
                .camera
                .world_to_screen(ball.position - ball.velocity * 0.08);
            list.line(tail, screen, color.with_alpha(alpha / 3));
            list.circle(screen, radius, color);
            let spin_phase = self.time * ball.spin;
            list.circle(
                screen + Vec2::new(spin_phase.cos(), spin_phase.sin()) * radius * 0.28,
                radius * 0.32,
                Color::WHITE.with_alpha(alpha / 2),
            );
        }

        self.draw_player(list);
        self.draw_touch_markers(list);
        self.draw_controls(list);
        self.draw_fps_bars(list);
    }

    fn draw_grid(&self, list: &mut DrawList) {
        let visible = self.camera.visible_rect();
        let spacing = 64.0;
        let start_x = (visible.x / spacing).floor() * spacing;
        let end_x = visible.x + visible.width;
        let start_y = (visible.y / spacing).floor() * spacing;
        let end_y = visible.y + visible.height;
        let color = Color::rgba(70, 96, 128, 70);

        let mut x = start_x;
        let mut count = 0;
        while x <= end_x && count < 120 {
            let top = self.camera.world_to_screen(Vec2::new(x, visible.y));
            let bottom = self
                .camera
                .world_to_screen(Vec2::new(x, visible.y + visible.height));
            list.line(top, bottom, color);
            x += spacing;
            count += 1;
        }
        let mut y = start_y;
        count = 0;
        while y <= end_y && count < 120 {
            let left = self.camera.world_to_screen(Vec2::new(visible.x, y));
            let right = self
                .camera
                .world_to_screen(Vec2::new(visible.x + visible.width, y));
            list.line(left, right, color);
            y += spacing;
            count += 1;
        }
    }

    fn draw_player(&self, list: &mut DrawList) {
        let screen = self.camera.world_to_screen(self.player.position);
        let direction = self.joystick.value();
        let angle = if direction.length_squared() > 0.01 {
            direction.angle()
        } else {
            -std::f32::consts::FRAC_PI_2
        };
        list.circle(
            screen,
            24.0 * self.camera.zoom,
            Color::rgba(90, 220, 200, 40),
        );
        let tip = screen + Vec2::new(angle.cos(), angle.sin()) * 20.0;
        let left = screen + Vec2::new(angle.cos(), angle.sin()).rotate(2.5) * 14.0;
        let right = screen + Vec2::new(angle.cos(), angle.sin()).rotate(-2.5) * 14.0;
        list.triangle(tip, left, right, Color::rgb(90, 235, 205));
        list.circle(screen, 4.0, Color::WHITE);
    }

    fn draw_touch_markers(&self, list: &mut DrawList) {
        for position in self.markers.values() {
            list.circle(*position, 18.0, Color::rgba(255, 255, 255, 35));
            list.circle(*position, 5.0, Color::rgba(255, 255, 255, 120));
        }
    }

    fn draw_controls(&self, list: &mut DrawList) {
        let center = self.joystick.center();
        let radius = self.joystick.radius();
        list.circle(center, radius, Color::rgba(26, 36, 54, 190));
        list.circle(center, radius * 0.72, Color::rgba(16, 24, 38, 220));
        list.circle(
            self.joystick.knob(),
            radius * 0.36,
            if self.joystick.is_active() {
                Color::rgb(90, 235, 205)
            } else {
                Color::rgba(120, 150, 180, 180)
            },
        );

        let bounds = self.button.bounds();
        let fill = if self.button.is_pressed() {
            Color::rgb(225, 90, 150)
        } else {
            Color::rgba(40, 54, 78, 220)
        };
        list.rect(bounds, fill);
        list.stroke_rect(bounds, 3.0, Color::rgba(225, 120, 180, 220));
        let center = bounds.center();
        for index in 0..4 {
            let angle = std::f32::consts::TAU * index as f32 / 4.0 + 0.4;
            let inner = center + Vec2::new(angle.cos(), angle.sin()) * 8.0;
            let outer = center + Vec2::new(angle.cos(), angle.sin()) * 26.0;
            list.line(inner, outer, Color::WHITE.with_alpha(210));
        }

        // Blocky "RKB" wordmark, top-left.
        let origin = Vec2::new(24.0, 22.0);
        let accent = Color::rgb(90, 235, 205);
        list.rect(Rect::new(origin.x, origin.y, 6.0, 28.0), accent);
        list.rect(Rect::new(origin.x + 6.0, origin.y, 16.0, 6.0), accent);
        list.rect(
            Rect::new(origin.x + 6.0, origin.y + 11.0, 14.0, 6.0),
            accent,
        );
        list.rect(
            Rect::new(origin.x + 6.0, origin.y + 22.0, 18.0, 6.0),
            accent,
        );
        list.rect(Rect::new(origin.x + 38.0, origin.y, 6.0, 28.0), accent);
        list.rect(Rect::new(origin.x + 58.0, origin.y, 6.0, 28.0), accent);
        list.line(
            Vec2::new(origin.x + 44.0, origin.y + 14.0),
            Vec2::new(origin.x + 58.0, origin.y),
            accent,
        );
        list.line(
            Vec2::new(origin.x + 44.0, origin.y + 14.0),
            Vec2::new(origin.x + 58.0, origin.y + 28.0),
            accent,
        );
        list.rect(Rect::new(origin.x + 76.0, origin.y, 6.0, 28.0), accent);
        list.rect(Rect::new(origin.x + 82.0, origin.y, 12.0, 6.0), accent);
        list.rect(
            Rect::new(origin.x + 82.0, origin.y + 11.0, 12.0, 6.0),
            accent,
        );
        list.rect(
            Rect::new(origin.x + 82.0, origin.y + 22.0, 12.0, 6.0),
            accent,
        );
        list.rect(Rect::new(origin.x + 94.0, origin.y + 6.0, 4.0, 5.0), accent);
        list.rect(
            Rect::new(origin.x + 94.0, origin.y + 17.0, 4.0, 5.0),
            accent,
        );
    }

    fn draw_fps_bars(&self, list: &mut DrawList) {
        let pulse = (self.time * 2.4).sin() * 0.5 + 0.5;
        let base = Vec2::new(self.viewport.x - 36.0, 26.0);
        for index in 0..3 {
            let height = 8.0 + (index as f32 * 5.0) + pulse * 4.0;
            list.rect(
                Rect::new(base.x - index as f32 * 9.0, base.y, 5.0, height),
                Color::rgba(90, 235, 205, 150),
            );
        }
    }
}

impl Game for Sandbox {
    fn on_start(&mut self) {
        self.started = true;
    }

    fn on_resize(&mut self, viewport: Vec2) {
        self.layout(viewport);
    }

    fn on_touch(&mut self, event: TouchEvent) {
        self.handle_touch(event);
    }

    fn update(&mut self, context: UpdateContext) {
        self.step(context.dt);
    }

    fn render(&mut self, list: &mut DrawList, _viewport: Vec2) {
        self.draw(list);
    }
}

/// Small deterministic xorshift generator; avoids a random-number dependency.
#[derive(Clone, Debug)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            seed
        })
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 32) as u32
    }

    fn next_f32(&mut self) -> f32 {
        self.next_u32() as f32 / u32::MAX as f32
    }

    fn range(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.next_f32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rkb_render::DrawList;
    use rkb_render::RenderBackend;
    use rkb_runtime::{Runtime, RuntimeConfig};

    use rkb_input::TouchPoint;

    fn boot() -> SandboxBootstrap {
        bootstrap(Vec2::new(1280.0, 720.0)).unwrap()
    }

    fn touch(phase: TouchPhase, id: TouchId, x: f32, y: f32, millis: u64) -> TouchEvent {
        TouchEvent::new(
            phase,
            TouchPoint::new(id, Vec2::new(x, y)).at_time(std::time::Duration::from_millis(millis)),
        )
    }

    #[test]
    fn bootstrap_loads_embedded_configuration_and_scene() {
        let booted = boot();
        assert_eq!(booted.engine.app_name, "RKB RUST");
        assert_eq!(booted.engine.fixed_hz, 60);
        assert_eq!(booted.sandbox.title(), "RKB RUST SANDBOX");
        assert_eq!(booted.sandbox.balls.len(), 10);
        assert!(booted.sandbox.stats().resources.entries >= 2);
    }

    #[test]
    fn joystick_moves_player() {
        let mut booted = boot();
        let joystick = &booted.sandbox.joystick;
        let center = joystick.center();
        let radius = joystick.radius();
        booted
            .sandbox
            .handle_touch(touch(TouchPhase::Started, 1, center.x, center.y, 0));
        booted.sandbox.handle_touch(touch(
            TouchPhase::Moved,
            1,
            center.x + radius * 0.9,
            center.y,
            16,
        ));
        let context = UpdateContext {
            frame: 0,
            step: 0,
            dt: 1.0 / 60.0,
            elapsed: std::time::Duration::ZERO,
            viewport: Vec2::new(1280.0, 720.0),
        };
        for _ in 0..30 {
            booted.sandbox.update(context);
        }
        assert!(booted.sandbox.player_position().x > 1.0);
        assert!(booted.sandbox.player_position().x < 400.0);
    }

    #[test]
    fn tap_on_canvas_spawns_ball() {
        let mut booted = boot();
        let before = booted.sandbox.balls.len();
        booted
            .sandbox
            .handle_touch(touch(TouchPhase::Started, 7, 640.0, 120.0, 0));
        booted
            .sandbox
            .handle_touch(touch(TouchPhase::Ended, 7, 640.0, 120.0, 90));
        assert_eq!(booted.sandbox.balls.len(), before + 1);
        assert_eq!(booted.sandbox.stats().taps, 1);
    }

    #[test]
    fn button_emits_burst() {
        let mut booted = boot();
        let bounds = booted.sandbox.button.bounds();
        let center = bounds.center();
        booted
            .sandbox
            .handle_touch(touch(TouchPhase::Started, 4, center.x, center.y, 0));
        booted
            .sandbox
            .handle_touch(touch(TouchPhase::Ended, 4, center.x, center.y, 30));
        assert_eq!(booted.sandbox.stats().bursts, 1);
        assert!(booted.sandbox.balls.len() >= 10 + 10);
    }

    #[test]
    fn pinch_changes_camera_zoom() {
        let mut booted = boot();
        booted
            .sandbox
            .handle_touch(touch(TouchPhase::Started, 10, 500.0, 300.0, 0));
        booted
            .sandbox
            .handle_touch(touch(TouchPhase::Started, 11, 700.0, 300.0, 0));
        booted
            .sandbox
            .handle_touch(touch(TouchPhase::Moved, 11, 900.0, 300.0, 20));
        assert!(booted.sandbox.zoom() > 1.0);
    }

    #[test]
    fn resize_cancels_touches_and_gesture_state() {
        let mut booted = boot();
        let center = booted.sandbox.joystick.center();
        booted
            .sandbox
            .handle_touch(touch(TouchPhase::Started, 1, center.x, center.y, 0));
        booted
            .sandbox
            .handle_touch(touch(TouchPhase::Started, 2, 500.0, 300.0, 0));

        assert_eq!(booted.sandbox.touches.len(), 2);
        assert_eq!(booted.sandbox.markers.len(), 2);
        assert!(booted.sandbox.joystick.is_active());
        assert!(booted.sandbox.pinch.is_active());

        booted.sandbox.on_resize(Vec2::new(720.0, 1280.0));

        assert!(booted.sandbox.touches.is_empty());
        assert!(booted.sandbox.markers.is_empty());
        assert!(!booted.sandbox.joystick.is_active());
        assert!(!booted.sandbox.button.is_pressed());
        assert!(!booted.sandbox.pinch.is_active());
        assert!(!booted.sandbox.drag.is_dragging());
        assert_eq!(booted.sandbox.viewport, Vec2::new(720.0, 1280.0));
    }

    #[test]
    fn rendering_produces_commands_and_pixels() {
        let mut list = DrawList::new(Color::BLACK);
        let booted = boot();
        booted.sandbox.draw(&mut list);
        assert!(list.len() > 10);
        let mut backend = rkb_render::SoftwareBackend::new(1280, 720);
        let stats = backend.render(&list).unwrap();
        assert!(stats.draw_calls > 10);
        assert_ne!(backend.framebuffer().checksum(), 0);
    }

    #[test]
    fn runtime_drives_sandbox_frames() {
        let booted = boot();
        let mut runtime = Runtime::new(
            RuntimeConfig {
                engine: booted.engine.clone(),
                clear_color: booted.sandbox.scene.clear_color,
                checksum_frames: true,
            },
            booted.sandbox,
            1280,
            720,
        );
        runtime.start().unwrap();
        let mut surface = rkb_platform::HeadlessSurface::new(1280, 720);
        let report = runtime
            .frame(std::time::Duration::from_millis(17), &mut surface)
            .unwrap();
        assert!(report.ran);
        assert_eq!(report.steps, 1);
        assert_eq!(surface.frames_presented(), 1);
        assert!(runtime.game().started());
    }
}
