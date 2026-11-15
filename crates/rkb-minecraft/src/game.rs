//! The Minecraft client: saved world loading, chunk streaming, first-person
//! movement, touch controls, and depth-tested block rendering.
//!
//! Scope of this milestone, stated plainly:
//!
//! - Reads a user-supplied Minecraft: Java Edition 1.21 save through `rkb-mc`.
//! - Renders saved block geometry with flat per-block colours and per-face
//!   shading. There are no textures, biome tints, lighting, or shadows.
//! - Supports walking, jumping, creative flight, collision, and touch look.
//! - Does not implement block breaking/placing, inventory, crafting, mobs,
//!   survival, or networking.
//!
//! The client never bundles a world. When no save is present it shows a status
//! message instead of pretending to have one.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

use rkb_core::{Rect, Vec2, Vec3};
use rkb_input::{TouchButton, TouchEvent, TouchId, TouchPhase, VirtualJoystick};
use rkb_mc::{World, WorldError};
use rkb_render::{Color, DrawList, Vertex3D};
use rkb_runtime::{Game, UpdateContext};

use crate::camera::Camera;
use crate::demo_world;
use crate::mesh::{build_chunk_mesh, is_solid_block, ChunkMesh};
use crate::player::{BlockQuery, MoveInput, Player};

/// Chunks are streamed in a square around the player.
pub const LOAD_RADIUS: i32 = 3;
/// Chunks meshed per simulation step, to keep frames responsive.
const MESH_BUDGET: usize = 2;
/// Chunks beyond this radius are dropped.
const RETAIN_RADIUS: i32 = LOAD_RADIUS + 1;

const SKY_ZENITH: Color = Color::rgb(58, 108, 196);
const SKY_HORIZON: Color = Color::rgb(171, 208, 240);
const HUD_DARK: Color = Color::rgba(18, 24, 36, 150);
const HUD_EDGE: Color = Color::rgba(232, 240, 255, 200);
const HUD_WHITE: Color = Color::rgba(255, 255, 255, 190);
const HUD_JUMP: Color = Color::rgba(104, 196, 128, 200);
const HUD_FLY: Color = Color::rgba(240, 196, 92, 210);
const HUD_DOWN: Color = Color::rgba(104, 156, 226, 210);

/// Snapshot of client state for diagnostics and the Android overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GameStats {
    pub world_loaded: bool,
    pub chunks_loaded: usize,
    pub chunks_queued: usize,
    pub blocks: usize,
    pub faces: usize,
    pub triangles: u32,
    pub frames: u64,
    pub flying: bool,
    pub on_ground: bool,
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Clone, Copy, Debug)]
struct LookDrag {
    id: TouchId,
    last: Vec2,
}

/// The playable client.
#[derive(Debug)]
pub struct MinecraftGame {
    world: Option<World>,
    world_path: Option<PathBuf>,
    message: String,
    spawn: Vec3,
    player: Player,
    meshes: BTreeMap<(i32, i32), ChunkMesh>,
    queue: VecDeque<(i32, i32)>,
    center: Option<(i32, i32)>,
    viewport: Vec2,
    joystick: VirtualJoystick,
    jump: TouchButton,
    fly: TouchButton,
    descend: TouchButton,
    fly_presses: u32,
    look: Option<LookDrag>,
    frames: u64,
    triangles: u32,
    last_error: Option<String>,
}

impl MinecraftGame {
    /// Creates a client for an optional save directory.
    ///
    /// A missing path, an absent `level.dat`, a rejected DataVersion, or an
    /// I/O error produces an idle client with a readable status message; it
    /// never panics and never substitutes demo content for a real world.
    #[must_use]
    pub fn new(world_path: Option<PathBuf>) -> Self {
        let viewport = Vec2::new(1280.0, 720.0);
        let mut game = Self {
            world: None,
            world_path: None,
            message: String::new(),
            spawn: Vec3::new(0.5, 80.0, 0.5),
            player: Player::new(Vec3::new(0.5, 80.0, 0.5)),
            meshes: BTreeMap::new(),
            queue: VecDeque::new(),
            center: None,
            viewport,
            joystick: VirtualJoystick::new(Vec2::new(160.0, 560.0), 96.0),
            jump: TouchButton::new(Rect::new(1080.0, 560.0, 96.0, 96.0), "JUMP"),
            fly: TouchButton::new(Rect::new(1080.0, 452.0, 96.0, 96.0), "FLY"),
            descend: TouchButton::new(Rect::new(972.0, 560.0, 96.0, 96.0), "DOWN"),
            fly_presses: 0,
            look: None,
            frames: 0,
            triangles: 0,
            last_error: None,
        };
        game.layout(viewport);
        match world_path {
            Some(path) => game.open_world(path),
            None => {
                game.message = "No Minecraft world configured. Copy a 1.21 save to the app's \
                     minecraft/world directory."
                    .to_owned();
            }
        }
        game
    }

    /// Loads a save, or records a readable failure.
    fn open_world(&mut self, path: PathBuf) {
        self.world_path = Some(path.clone());
        match load_world(&path) {
            Ok(loaded) => {
                self.spawn = loaded.spawn;
                self.player = Player::new(loaded.spawn);
                self.world = Some(loaded.world);
                self.message = format!(
                    "Minecraft {} save \"{}\" loaded ({} chunks).",
                    loaded.version, loaded.name, loaded.chunks
                );
            }
            Err(error) => {
                self.message = format!("Minecraft world unavailable: {error}");
            }
        }
    }

    #[must_use]
    pub fn has_world(&self) -> bool {
        self.world.is_some()
    }

    /// Human-readable load/status message shown by the host overlay.
    #[must_use]
    pub fn status(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub fn stats(&self) -> GameStats {
        GameStats {
            world_loaded: self.world.is_some(),
            chunks_loaded: self.meshes.len(),
            chunks_queued: self.queue.len(),
            blocks: self.meshes.values().map(|mesh| mesh.non_air_blocks).sum(),
            faces: self.meshes.values().map(ChunkMesh::face_count).sum(),
            triangles: self.triangles,
            frames: self.frames,
            flying: self.player.flying,
            on_ground: self.player.on_ground,
            position: self.player.position,
            yaw: self.player.yaw,
            pitch: self.player.pitch,
        }
    }

    #[must_use]
    pub fn player(&self) -> &Player {
        &self.player
    }

    /// Moves the player immediately, for tests and the respawn path.
    pub fn teleport(&mut self, position: Vec3) {
        self.player = Player::new(position);
    }

    /// Sets the camera angles in radians, clamped by the player rules.
    pub fn set_look(&mut self, yaw: f32, pitch: f32) {
        self.player.yaw = yaw.rem_euclid(std::f32::consts::TAU);
        self.player.pitch = pitch.clamp(-1.553, 1.553);
    }

    #[must_use]
    pub fn spawn(&self) -> Vec3 {
        self.spawn
    }

    /// Save directory this client was asked to open, if any.
    #[must_use]
    pub fn world_path(&self) -> Option<&Path> {
        self.world_path.as_deref()
    }

    #[must_use]
    pub fn camera(&self) -> Camera {
        let mut camera = Camera::new(self.player.eye());
        camera.yaw = self.player.yaw;
        camera.pitch = self.player.pitch;
        camera
    }

    #[must_use]
    pub fn viewport(&self) -> Vec2 {
        self.viewport
    }

    fn layout(&mut self, viewport: Vec2) {
        self.viewport = Vec2::new(viewport.x.max(1.0), viewport.y.max(1.0));
        let min = self.viewport.x.min(self.viewport.y);
        let radius = (min * 0.16).clamp(44.0, 150.0);
        let margin = (min * 0.05).clamp(18.0, 44.0);
        self.joystick.set_geometry(
            Vec2::new(margin + radius, self.viewport.y - margin - radius),
            radius,
        );
        let side = radius * 0.95;
        let gap = margin * 0.6;
        self.jump.set_bounds(Rect::new(
            self.viewport.x - margin - side,
            self.viewport.y - margin - side,
            side,
            side,
        ));
        self.fly.set_bounds(Rect::new(
            self.viewport.x - margin - side,
            self.viewport.y - margin - side * 2.0 - gap,
            side,
            side,
        ));
        self.descend.set_bounds(Rect::new(
            self.viewport.x - margin - side * 2.0 - gap,
            self.viewport.y - margin - side,
            side,
            side,
        ));
    }

    fn handle_touch(&mut self, event: TouchEvent) {
        if self.joystick.handle(event) {
            return;
        }
        if self.descend.handle(event) && self.player.flying {
            return;
        }
        if self.fly.handle(event) {
            if self.fly.press_count() != self.fly_presses {
                self.fly_presses = self.fly.press_count();
                self.player.flying = !self.player.flying;
                self.player.velocity.y = 0.0;
                self.message = if self.player.flying {
                    "Flight enabled.".to_owned()
                } else {
                    "Flight disabled.".to_owned()
                };
            }
            return;
        }
        if self.jump.handle(event) {
            return;
        }

        match event.phase {
            TouchPhase::Started => {
                if self.look.is_none() {
                    self.look = Some(LookDrag {
                        id: event.point.id,
                        last: event.point.position,
                    });
                }
            }
            TouchPhase::Moved => {
                if let Some(look) = self.look.as_mut() {
                    if look.id == event.point.id {
                        let delta = event.point.position - look.last;
                        look.last = event.point.position;
                        self.player.look(delta.x * 0.0032, -delta.y * 0.0032);
                    }
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                if self.look.is_some_and(|look| look.id == event.point.id) {
                    self.look = None;
                }
            }
        }
    }

    fn step(&mut self, dt: f32) {
        let Some(world) = self.world.as_mut() else {
            return;
        };
        let value = self.joystick.value();
        let input = MoveInput {
            forward: -value.y,
            strafe: value.x,
            jump: self.jump.is_pressed(),
            descend: self.player.flying && self.descend.is_pressed(),
        };
        let mut query = WorldQuery { world, error: None };
        self.player.step(dt, input, &mut query);
        if let Some(error) = query.error {
            self.last_error = Some(error.clone());
            self.message = format!("Block lookup failed: {error}");
        }

        if self.player.position.y < 16.0 {
            let before = self.player.position;
            self.player = Player::new(self.spawn);
            self.message = format!(
                "Fell to y={:.0}; respawned at ({:.0}, {:.0}, {:.0}).",
                before.y, self.spawn.x, self.spawn.y, self.spawn.z
            );
        }

        self.ensure_area();
        for _ in 0..MESH_BUDGET {
            if !self.load_one() {
                break;
            }
        }
    }

    fn ensure_area(&mut self) {
        let center = (
            self.player.position.x.floor().div_euclid(16.0) as i32,
            self.player.position.z.floor().div_euclid(16.0) as i32,
        );
        if self.center == Some(center) {
            return;
        }
        self.center = Some(center);
        if let Some(world) = self.world.as_mut() {
            world.clear_cache();
        }
        self.meshes.retain(|&(cx, cz), _| {
            (cx - center.0).abs() <= RETAIN_RADIUS && (cz - center.1).abs() <= RETAIN_RADIUS
        });
        let mut needed = Vec::new();
        for dz in -LOAD_RADIUS..=LOAD_RADIUS {
            for dx in -LOAD_RADIUS..=LOAD_RADIUS {
                let key = (center.0 + dx, center.1 + dz);
                if !self.meshes.contains_key(&key) {
                    needed.push((dx * dx + dz * dz, key));
                }
            }
        }
        needed.sort_by_key(|&(distance, key)| (distance, key));
        self.queue = needed.into_iter().map(|(_, key)| key).collect();
    }

    /// Meshes one queued chunk. Returns `false` when the queue is empty.
    fn load_one(&mut self) -> bool {
        let Some((cx, cz)) = self.queue.pop_front() else {
            return false;
        };
        let Some(world) = self.world.as_mut() else {
            return false;
        };
        match build_chunk_mesh(world, cx, cz) {
            Ok(Some(mesh)) => {
                self.meshes.insert((cx, cz), mesh);
            }
            Ok(None) => {}
            Err(error) => {
                self.last_error = Some(error.to_string());
                self.message = format!("Chunk ({cx}, {cz}) failed to load: {error}");
            }
        }
        true
    }

    fn draw_sky(&self, list: &mut DrawList) {
        let viewport = self.viewport;
        let horizon_y = viewport.y * 0.62;
        let strips = 8;
        let strip_height = horizon_y / strips as f32;
        for strip in 0..strips {
            let t = strip as f32 / (strips - 1) as f32;
            list.rect(
                Rect::new(
                    0.0,
                    strip as f32 * strip_height,
                    viewport.x,
                    strip_height + 1.0,
                ),
                SKY_ZENITH.lerp(SKY_HORIZON, t),
            );
        }
        list.rect(
            Rect::new(
                0.0,
                horizon_y,
                viewport.x,
                (viewport.y - horizon_y).max(0.0),
            ),
            SKY_HORIZON,
        );
    }

    fn draw_scene(&mut self, list: &mut DrawList) {
        let viewport = self.viewport;
        let camera = self.camera();
        let view_projection = camera.view_projection(viewport);
        let frustum = camera.frustum(viewport);
        let eye = camera.position;
        let mut out = [Vertex3D::new(0.0, 0.0, 1.0, 1.0); 8];
        let mut triangles = 0_u32;

        for mesh in self.meshes.values() {
            if mesh.faces.is_empty() || !frustum.contains_aabb(mesh.min, mesh.max) {
                continue;
            }
            for face in &mesh.faces {
                if !face.is_visible_from(eye) {
                    continue;
                }
                let count =
                    camera.project_polygon(view_projection, viewport, &face.points, &mut out);
                if count < 3 {
                    continue;
                }
                let distance = (face.center() - eye).length();
                let fog = ((distance - 48.0) / 128.0).clamp(0.0, 0.6);
                let color = face.color.lerp(SKY_HORIZON, fog);
                for index in 1..count - 1 {
                    list.triangle_3d(out[0], out[index], out[index + 1], color);
                    triangles = triangles.saturating_add(1);
                }
            }
        }
        self.triangles = triangles;
    }

    fn draw_hud(&self, list: &mut DrawList) {
        let viewport = self.viewport;
        let center = viewport * 0.5;
        list.rect(
            Rect::new(center.x - 1.0, center.y - 8.0, 2.0, 16.0),
            HUD_WHITE,
        );
        list.rect(
            Rect::new(center.x - 8.0, center.y - 1.0, 16.0, 2.0),
            HUD_WHITE,
        );

        let joystick = &self.joystick;
        list.circle(joystick.center(), joystick.radius(), HUD_DARK);
        list.circle(joystick.knob(), joystick.radius() * 0.42, HUD_EDGE);

        self.draw_button(list, &self.jump, HUD_JUMP, ButtonIcon::Up);
        self.draw_button(list, &self.fly, HUD_FLY, ButtonIcon::Level);
        if self.player.flying {
            self.draw_button(list, &self.descend, HUD_DOWN, ButtonIcon::Down);
        }
    }

    fn draw_button(
        &self,
        list: &mut DrawList,
        button: &TouchButton,
        tint: Color,
        icon: ButtonIcon,
    ) {
        let bounds = button.bounds();
        let fill = if button.is_pressed() {
            tint
        } else {
            Color::rgba(tint.r, tint.g, tint.b, 140)
        };
        list.rect(bounds, fill);
        list.stroke_rect(bounds, 2.0, HUD_EDGE);
        let center = Vec2::new(
            bounds.x + bounds.width * 0.5,
            bounds.y + bounds.height * 0.5,
        );
        let size = bounds.width.min(bounds.height) * 0.22;
        match icon {
            ButtonIcon::Up => {
                list.triangle(
                    Vec2::new(center.x, center.y - size),
                    Vec2::new(center.x - size, center.y + size * 0.7),
                    Vec2::new(center.x + size, center.y + size * 0.7),
                    HUD_WHITE,
                );
            }
            ButtonIcon::Down => {
                list.triangle(
                    Vec2::new(center.x, center.y + size),
                    Vec2::new(center.x - size, center.y - size * 0.7),
                    Vec2::new(center.x + size, center.y - size * 0.7),
                    HUD_WHITE,
                );
            }
            ButtonIcon::Level => {
                list.rect(
                    Rect::new(center.x - size, center.y - size * 0.35, size * 2.0, 2.0),
                    HUD_WHITE,
                );
                list.rect(
                    Rect::new(center.x - size, center.y + size * 0.35, size * 2.0, 2.0),
                    HUD_WHITE,
                );
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ButtonIcon {
    Up,
    Down,
    Level,
}

impl Game for MinecraftGame {
    fn on_resize(&mut self, viewport: Vec2) {
        self.layout(viewport);
    }

    fn on_touch(&mut self, event: TouchEvent) {
        self.handle_touch(event);
    }

    fn update(&mut self, context: UpdateContext) {
        self.frames = self.frames.saturating_add(1);
        self.step(context.dt);
    }

    fn render(&mut self, list: &mut DrawList, viewport: Vec2) {
        if (self.viewport - viewport).length_squared() > 0.01 {
            self.layout(viewport);
        }
        self.draw_sky(list);
        if self.world.is_some() {
            self.draw_scene(list);
        }
        self.draw_hud(list);
    }
}

struct WorldQuery<'a> {
    world: &'a mut World,
    error: Option<String>,
}

impl BlockQuery for WorldQuery<'_> {
    fn is_solid(&mut self, x: i32, y: i32, z: i32) -> bool {
        match self.world.block_at(x, y, z) {
            Ok(Some(state)) => !state.is_air() && is_solid_block(&state),
            Ok(None) => false,
            Err(error) => {
                if self.error.is_none() {
                    self.error = Some(error.to_string());
                }
                false
            }
        }
    }
}

struct LoadedWorld {
    world: World,
    spawn: Vec3,
    name: String,
    version: String,
    chunks: usize,
}

fn load_world(path: &Path) -> Result<LoadedWorld, String> {
    let mut world = World::open(path).map_err(|error| describe_world_error(path, &error))?;
    let stats = world
        .stats()
        .map_err(|error| format!("could not read region headers: {error}"))?;
    let level = world.level();
    let preferred = level.spawn.unwrap_or([8, 64, 8]);
    let name = level
        .level_name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "(unnamed)".to_owned());
    let version = level.version.id.to_owned();

    let mut spawn = None;
    let mut candidates = vec![(preferred[0], preferred[2]), (8, 8), (0, 0)];
    for (x, z) in std::mem::take(&mut candidates) {
        if let Some(y) = ground_height(&mut world, x, z, preferred[1].saturating_add(24))? {
            spawn = Some(Vec3::new(x as f32 + 0.5, (y + 1) as f32, z as f32 + 0.5));
            break;
        }
    }
    let spawn = spawn.unwrap_or_else(|| {
        Vec3::new(
            preferred[0] as f32 + 0.5,
            preferred[1] as f32,
            preferred[2] as f32 + 0.5,
        )
    });

    Ok(LoadedWorld {
        world,
        spawn,
        name,
        version,
        chunks: stats.chunks,
    })
}

/// Highest solid block in a column, searched downward.
fn ground_height(world: &mut World, x: i32, z: i32, from: i32) -> Result<Option<i32>, String> {
    let mut y = from.clamp(-64, 319);
    while y >= -64 {
        match world.block_at(x, y, z) {
            Ok(Some(state)) if !state.is_air() && is_solid_block(&state) => {
                return Ok(Some(y));
            }
            Ok(_) => {}
            Err(error) => return Err(error.to_string()),
        }
        y -= 1;
    }
    Ok(None)
}

fn describe_world_error(path: &Path, error: &WorldError) -> String {
    match error {
        WorldError::Io { .. } => format!(
            "no readable save at {} (expected level.dat and region/)",
            path.display()
        ),
        WorldError::Version(source) => format!("unsupported save version: {source}"),
        other => other.to_string(),
    }
}

/// Generates and loads the synthetic demo island. Used by tests and the
/// emulator workflow; it is never shipped as a bundled world.
pub fn load_demo_world(dir: &Path) -> Result<MinecraftGame, String> {
    demo_world::write_demo_world(dir)?;
    let mut game = MinecraftGame::new(Some(dir.to_path_buf()));
    for _ in 0..(LOAD_RADIUS * 2 + 1).pow(2) as usize {
        game.ensure_area();
        if !game.load_one() {
            break;
        }
    }
    Ok(game)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rkb_input::{TouchEvent, TouchPoint};
    use rkb_mc::fixture::TempDir;
    use rkb_render::DrawCommand;

    fn touch(phase: TouchPhase, id: TouchId, x: f32, y: f32) -> TouchEvent {
        TouchEvent::new(phase, TouchPoint::new(id, Vec2::new(x, y)))
    }

    fn context(dt: f32, viewport: Vec2) -> UpdateContext {
        UpdateContext {
            frame: 0,
            step: 0,
            dt,
            elapsed: std::time::Duration::ZERO,
            viewport,
        }
    }

    #[test]
    fn reports_missing_world_instead_of_inventing_one() {
        let mut game = MinecraftGame::new(None);
        assert!(!game.has_world());
        assert!(game.status().contains("No Minecraft world"));
        let mut list = DrawList::new(SKY_HORIZON);
        game.render(&mut list, Vec2::new(640.0, 360.0));
        assert!(!list
            .commands()
            .iter()
            .any(|command| matches!(command, DrawCommand::Triangle3D { .. })));
    }

    #[test]
    fn rejects_a_save_from_an_unverified_version() {
        let dir = TempDir::new("unsupported-client");
        demo_world::write_flat_test_world(dir.path(), 9999).unwrap();
        let game = MinecraftGame::new(Some(dir.path().to_path_buf()));
        assert!(!game.has_world());
        assert!(
            game.status().contains("unsupported save version"),
            "{}",
            game.status()
        );
    }

    #[test]
    fn loads_a_flat_save_and_places_the_player_on_the_surface() {
        let dir = TempDir::new("flat-client");
        demo_world::write_flat_test_world(dir.path(), demo_world::DATA_VERSION).unwrap();
        let game = MinecraftGame::new(Some(dir.path().to_path_buf()));
        assert!(game.has_world(), "{}", game.status());
        assert!((game.spawn().y - 64.0).abs() < 0.001, "{:?}", game.spawn());
        assert!(game.status().contains("1.21"), "{}", game.status());
    }

    #[test]
    fn streams_chunks_and_renders_saved_geometry() {
        let dir = TempDir::new("render-client");
        demo_world::write_flat_test_world(dir.path(), demo_world::DATA_VERSION).unwrap();
        let mut game = MinecraftGame::new(Some(dir.path().to_path_buf()));
        for _ in 0..30 {
            game.update(context(1.0 / 60.0, Vec2::new(640.0, 360.0)));
        }
        let stats = game.stats();
        assert!(stats.chunks_loaded >= 1, "{stats:?}");
        assert!(stats.faces > 0, "{stats:?}");

        // Stand in the middle of the chunk and look down at the stone surface.
        game.teleport(Vec3::new(8.5, 64.0, 8.5));
        game.set_look(0.0, -0.5);
        let mut list = DrawList::new(SKY_HORIZON);
        game.render(&mut list, Vec2::new(640.0, 360.0));
        let triangles = list
            .commands()
            .iter()
            .filter(|command| matches!(command, DrawCommand::Triangle3D { .. }))
            .count();
        assert!(triangles > 10, "only {triangles} triangles submitted");
        assert_eq!(game.stats().triangles as usize, triangles);
    }

    #[test]
    fn joystick_walks_and_drag_turns_the_camera() {
        let dir = TempDir::new("input-client");
        demo_world::write_flat_test_world(dir.path(), demo_world::DATA_VERSION).unwrap();
        let mut game = MinecraftGame::new(Some(dir.path().to_path_buf()));
        let viewport = Vec2::new(640.0, 360.0);
        game.on_resize(viewport);
        let start_z = game.player.position.z;
        let center = game.joystick.center();
        let radius = game.joystick.radius();
        game.on_touch(touch(TouchPhase::Started, 1, center.x, center.y));
        game.on_touch(touch(
            TouchPhase::Moved,
            1,
            center.x,
            center.y - radius * 0.95,
        ));
        for _ in 0..40 {
            game.update(context(1.0 / 60.0, viewport));
        }
        assert!(
            game.player.position.z < start_z - 1.0,
            "walked to {:?}",
            game.player.position
        );

        let yaw = game.player.yaw;
        game.on_touch(touch(TouchPhase::Started, 9, 320.0, 180.0));
        game.on_touch(touch(TouchPhase::Moved, 9, 420.0, 180.0));
        game.on_touch(touch(TouchPhase::Ended, 9, 420.0, 180.0));
        assert!(game.player.yaw > yaw, "yaw {} -> {}", yaw, game.player.yaw);
    }

    #[test]
    fn jump_button_lifts_the_player() {
        let dir = TempDir::new("jump-client");
        demo_world::write_flat_test_world(dir.path(), demo_world::DATA_VERSION).unwrap();
        let mut game = MinecraftGame::new(Some(dir.path().to_path_buf()));
        let viewport = Vec2::new(640.0, 360.0);
        game.on_resize(viewport);
        for _ in 0..10 {
            game.update(context(1.0 / 60.0, viewport));
        }
        assert!(game.player.on_ground, "{:?}", game.player);
        let bounds = game.jump.bounds();
        game.on_touch(touch(
            TouchPhase::Started,
            3,
            bounds.x + bounds.width * 0.5,
            bounds.y + bounds.height * 0.5,
        ));
        for _ in 0..8 {
            game.update(context(1.0 / 60.0, viewport));
        }
        assert!(game.player.position.y > 64.1, "{:?}", game.player);
    }

    #[test]
    fn flight_toggle_disables_gravity() {
        let dir = TempDir::new("fly-client");
        demo_world::write_flat_test_world(dir.path(), demo_world::DATA_VERSION).unwrap();
        let mut game = MinecraftGame::new(Some(dir.path().to_path_buf()));
        let viewport = Vec2::new(640.0, 360.0);
        game.on_resize(viewport);
        let bounds = game.fly.bounds();
        let center = Vec2::new(
            bounds.x + bounds.width * 0.5,
            bounds.y + bounds.height * 0.5,
        );
        game.on_touch(touch(TouchPhase::Started, 4, center.x, center.y));
        game.on_touch(touch(TouchPhase::Ended, 4, center.x, center.y));
        assert!(game.player.flying);
        let start = game.player.position.y;
        for _ in 0..30 {
            game.update(context(1.0 / 60.0, viewport));
        }
        assert!((game.player.position.y - start).abs() < 0.01);
    }
}
