//! The Minecraft client: default generated terrain or a user save, chunk
//! streaming, first-person movement, touch controls, block interaction, and
//! depth-tested block rendering.
//!
//! Scope of this milestone, stated plainly:
//!
//! - Opens a user-supplied 1.21 save, or creates a deterministic generated
//!   world when no save exists. The generated world is the default game.
//! - Streams chunks from a region save or a background terrain worker, meshes
//!   real block data, culls hidden faces, and renders with a procedural
//!   original texture atlas (no Mojang textures), directional face shading,
//!   per-vertex ambient occlusion, and distance fog.
//! - Supports walking, jumping, collision, creative flight, touch look,
//!   raycast block breaking and placing, and persistent seed/player/edits.
//! - Does not implement inventory, crafting, mobs, survival, dynamic lighting,
//!   or multiplayer.
//!
//! No Minecraft assets, code, or world data are bundled. Generated terrain is
//! produced entirely by RKB code from a seed.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rkb_core::{Rect, Stage, Vec2, Vec3};
use rkb_input::{TouchButton, TouchEvent, TouchId, TouchPhase, VirtualJoystick};
use rkb_mc::{BlockState, World};
use rkb_net::ServerWorld;
use rkb_render::{Color, DrawList, Texture, Vertex3D};
use rkb_runtime::{Game, UpdateContext};

use crate::camera::Camera;
use crate::demo_world;
use crate::interaction::{raycast, RayHit};
use crate::mesh::{build_chunk_mesh_with, is_solid_block, ChunkMesh, EditMap};
use crate::model::{BlockModelSource, BuiltinModels, RenderLayer};
use crate::pack::{self, PackModels, ResourcePack};
use crate::player::{BlockQuery, MoveInput, Player};
use crate::session::{self, PlayerSnapshot, Session};
use crate::textures::BlockAtlas;
use crate::world_source::{ChunkSource, WorldKind, WorldState};
use crate::worldgen::{DEFAULT_SEED, WORLD_MIN_Y};

/// Chunks are streamed in a square around the player.
pub const LOAD_RADIUS: i32 = 3;
/// Chunks meshed per simulation step, to keep frames responsive.
const MESH_BUDGET: usize = 2;
/// Chunks beyond this radius are dropped from memory.
const RETAIN_RADIUS: i32 = LOAD_RADIUS + 1;
/// Blocks the player can reach when breaking or placing.
pub const REACH: f32 = 5.0;
/// Seconds between automatic saves.
const AUTOSAVE_SECONDS: f32 = 20.0;
/// The playable world border used by current Java Edition worlds.
const WORLD_BORDER: f32 = 29_999_984.0;
/// Distance fog blends terrain into the sky between these distances (blocks).
///
/// The far edge is derived from the chunk load radius so terrain always fades
/// into the sky before the outermost meshed ring; the near edge keeps the
/// historical 36/64 ratio as a fraction of it. Hardcoded distances drift out
/// of step with `LOAD_RADIUS` and expose the mesh edge as a hard line.
const FOG_END: f32 = ((LOAD_RADIUS + 1) as f32) * 16.0;
const FOG_START: f32 = FOG_END * 0.5625;

/// Fog amount for a distance in blocks: 0 before [`FOG_START`], 1 at and
/// beyond [`FOG_END`].
fn fog_amount(distance: f32) -> f32 {
    ((distance - FOG_START) / (FOG_END - FOG_START)).clamp(0.0, 1.0)
}

/// Fog colour is the sky at the horizon, so distant terrain dissolves into
/// the band the sky gradient draws there instead of a separate grey. The sky
/// stays fixed until the server's `UpdateTime` is decoded.
fn fog_color() -> Color {
    SKY_HORIZON
}

const SKY_ZENITH: Color = Color::rgb(58, 108, 196);
const SKY_HORIZON: Color = Color::rgb(171, 208, 240);
const HUD_DARK: Color = Color::rgba(18, 24, 36, 150);
const HUD_EDGE: Color = Color::rgba(232, 240, 255, 200);
const HUD_WHITE: Color = Color::rgba(255, 255, 255, 190);
const HUD_JUMP: Color = Color::rgba(104, 196, 128, 200);
const HUD_FLY: Color = Color::rgba(240, 196, 92, 210);
const HUD_DOWN: Color = Color::rgba(104, 156, 226, 210);
const HUD_BREAK: Color = Color::rgba(226, 112, 92, 210);
const HUD_PLACE: Color = Color::rgba(126, 190, 226, 210);

/// Snapshot of client state for diagnostics and the Android overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GameStats {
    pub world_loaded: bool,
    /// Chunks with a resident mesh.
    pub chunks_loaded: usize,
    /// Chunks generated or known in the source.
    pub chunks_generated: usize,
    /// Chunks that passed frustum culling on the last rendered frame.
    pub chunks_visible: usize,
    pub chunks_queued: usize,
    pub blocks: usize,
    pub faces: usize,
    pub triangles: u32,
    pub edits: usize,
    /// Blocks drawn as a full cube because no precise built-in shape exists.
    pub missing_models: usize,
    /// Translucent faces across resident meshes.
    pub translucent_faces: usize,
    /// Server columns with decoded light.
    pub light_columns: usize,
    /// Server block updates applied to resident columns since the join.
    pub block_updates: usize,
    /// Chunk columns the server has delivered since the join.
    pub chunks_received: usize,
    /// Chunk columns the server has unloaded since the join.
    pub unloads: usize,
    /// Respawn/dimension resets received since the join.
    pub resets: usize,
    /// True when the current world uses the documented full-sky light fallback.
    pub light_fallback: bool,
    pub frames: u64,
    pub flying: bool,
    pub on_ground: bool,
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub save_dirty: bool,
}

#[derive(Clone, Copy, Debug)]
struct LookDrag {
    id: TouchId,
    last: Vec2,
}

/// A loaded resource pack: its resolved models plus the atlas built from the
/// pack's textures.
#[derive(Debug)]
struct PackRuntime {
    models: PackModels,
    atlas: pack::Atlas,
}

/// Which shapes and textures the mesher and draw list are currently using.
///
/// The default is [`ActiveTextures::Builtin`]: built-in block shapes and the
/// procedural atlas drawn by this project. [`ActiveTextures::Pack`] replaces
/// both with whatever the user loaded, and every texture in it comes from that
/// pack rather than from any bundled asset.
#[derive(Debug)]
enum ActiveTextures {
    Builtin(BlockAtlas),
    Pack(PackRuntime),
}

impl Default for ActiveTextures {
    fn default() -> Self {
        Self::Builtin(BlockAtlas::new())
    }
}

impl ActiveTextures {
    /// Model source handed to the mesher.
    fn models(&self) -> &dyn BlockModelSource {
        match self {
            Self::Builtin(_) => &BuiltinModels,
            Self::Pack(runtime) => &runtime.models,
        }
    }

    /// `[u0, v0, u1, v1]` for one texture id.
    fn uv_id(&self, id: u32) -> [f32; 4] {
        match self {
            Self::Builtin(atlas) => atlas.uv_id(id),
            Self::Pack(runtime) => runtime.atlas.uv_id(id),
        }
    }

    /// Atlas texture shared with the draw list.
    fn texture(&self) -> Arc<Texture> {
        match self {
            Self::Builtin(atlas) => atlas.texture(),
            Self::Pack(runtime) => runtime.atlas.texture(),
        }
    }

    /// Rebuilds a pack atlas after new textures were decoded. Built-in
    /// atlases are static, so this is a no-op for them.
    fn refresh(&mut self) {
        if let Self::Pack(runtime) = self {
            let store = runtime.models.store();
            runtime.atlas.rebuild(&store);
        }
    }

    /// Pack path and counters, or a note that the built-in atlas is active.
    fn status(&self) -> String {
        match self {
            Self::Builtin(_) => "textures: built-in procedural atlas".to_owned(),
            Self::Pack(runtime) => {
                let pack = runtime.models.pack();
                let mut text = format!(
                    "textures: pack {} ({} textures, {} unmodelled, {} missing)",
                    pack.origin,
                    runtime.models.texture_count(),
                    runtime.models.unmodelled_blocks(),
                    runtime.models.missing_textures(),
                );
                if let Some(format) = pack.pack_format {
                    text.push_str(&format!(", format {format}"));
                }
                if let Some(error) = runtime.models.first_error() {
                    text.push_str(&format!("; first error: {error}"));
                }
                text
            }
        }
    }
}

/// The playable client.
#[derive(Debug)]
pub struct MinecraftGame {
    world: Option<WorldState>,
    world_path: Option<PathBuf>,
    message: String,
    save_status: String,
    spawn: Vec3,
    player: Player,
    meshes: BTreeMap<(i32, i32), ChunkMesh>,
    /// Monotonic revision per resident mesh, consumed by the GPU backend so it
    /// uploads only meshes that actually changed.
    mesh_revs: BTreeMap<(i32, i32), u64>,
    mesh_serial: u64,
    /// Bumped whenever every mesh is dropped (world load, pack change) so an
    /// external consumer can invalidate its whole cache.
    mesh_epoch: u64,
    /// Bumped whenever the active atlas or its UV rectangles change.
    atlas_generation: u64,
    queue: VecDeque<(i32, i32)>,
    center: Option<(i32, i32)>,
    viewport: Vec2,
    joystick: VirtualJoystick,
    jump: TouchButton,
    sneak: TouchButton,
    sprint: TouchButton,
    fly: TouchButton,
    descend: TouchButton,
    break_button: TouchButton,
    place_button: TouchButton,
    fly_presses: u32,
    break_presses: u32,
    place_presses: u32,
    look: Option<LookDrag>,
    frames: u64,
    triangles: u32,
    visible_chunks: usize,
    edits: EditMap,
    save_clock: f32,
    save_dirty: bool,
    last_error: Option<String>,
    /// Block shapes and the atlas shared with the draw list each frame.
    textures: ActiveTextures,
    /// Vertical field of view in radians (Minecraft's default is 70°).
    camera_fov_y: f32,
}

impl MinecraftGame {
    /// Creates a client for an optional world directory.
    ///
    /// `Some(path)` opens a 1.21 save when one exists, otherwise it creates and
    /// saves a deterministic generated world there. `None` produces an idle
    /// client with an explanatory status; it never substitutes demo content.
    #[must_use]
    pub fn new(world_path: Option<PathBuf>) -> Self {
        let viewport = Vec2::new(1280.0, 720.0);
        let mut game = Self {
            world: None,
            world_path: None,
            message: String::new(),
            save_status: "not saved yet".to_owned(),
            spawn: Vec3::new(0.5, 80.0, 0.5),
            player: Player::new(Vec3::new(0.5, 80.0, 0.5)),
            meshes: BTreeMap::new(),
            mesh_revs: BTreeMap::new(),
            mesh_serial: 0,
            mesh_epoch: 0,
            atlas_generation: 0,
            queue: VecDeque::new(),
            center: None,
            viewport,
            joystick: VirtualJoystick::new(Vec2::new(160.0, 560.0), 96.0),
            jump: TouchButton::new(Rect::new(1080.0, 560.0, 96.0, 96.0), "JUMP"),
            sneak: TouchButton::new(Rect::new(1080.0, 516.0, 96.0, 96.0), "SNEAK"),
            sprint: TouchButton::new(Rect::new(1080.0, 468.0, 96.0, 96.0), "SPRINT"),
            fly: TouchButton::new(Rect::new(1080.0, 452.0, 96.0, 96.0), "FLY"),
            descend: TouchButton::new(Rect::new(972.0, 560.0, 96.0, 96.0), "DOWN"),
            break_button: TouchButton::new(Rect::new(864.0, 560.0, 96.0, 96.0), "BREAK"),
            place_button: TouchButton::new(Rect::new(972.0, 452.0, 96.0, 96.0), "PLACE"),
            fly_presses: 0,
            break_presses: 0,
            place_presses: 0,
            look: None,
            frames: 0,
            triangles: 0,
            visible_chunks: 0,
            edits: EditMap::new(),
            save_clock: 0.0,
            save_dirty: false,
            last_error: None,
            textures: ActiveTextures::default(),
            camera_fov_y: 70.0_f32.to_radians(),
        };
        game.layout(viewport);
        match world_path {
            Some(path) => game.open_or_generate(path),
            None => {
                game.message = "No Minecraft world configured. The Android host must pass a \
                     writable world directory."
                    .to_owned();
            }
        }
        game
    }

    /// Creates a client for a live server world.
    ///
    /// The world is the server's: chunks and block updates arrive over the
    /// network, and no generated terrain is substituted for missing columns.
    #[must_use]
    pub fn join_server(server: ServerWorld) -> Self {
        let mut game = Self::new(None);
        game.load_server(server);
        game
    }

    /// Replaces the current world with a live server world.
    ///
    /// This is the in-place form used by the Android bridge, which keeps one
    /// runtime and swaps its game state after a background login completes.
    pub fn load_server(&mut self, server: ServerWorld) {
        self.world_path = None;
        self.edits.clear();
        self.clear_meshes();
        self.queue.clear();
        self.center = None;
        self.save_dirty = false;
        self.save_status = "multiplayer world; local saves are disabled".to_owned();
        self.last_error = None;
        if let Some(position) = server.position() {
            let spawn = Vec3::new(position.x as f32, position.y as f32, position.z as f32);
            self.spawn = spawn;
            self.player = Player::new(spawn);
            // Minecraft yaw 0 faces +Z; the camera at yaw 0 faces -Z. Pitch is
            // positive downwards in the protocol and positive upwards here.
            self.player.yaw = position.yaw.to_radians() + std::f32::consts::PI;
            self.player.pitch = -position.pitch.to_radians();
        }
        self.message =
            "Joined a protocol-767 server world; only server chunks are rendered.".to_owned();
        self.world = Some(WorldState::Server(server));
    }

    fn open_or_generate(&mut self, path: PathBuf) {
        self.world_path = Some(path.clone());
        if let Err(error) = std::fs::create_dir_all(&path) {
            self.last_error = Some(error.to_string());
            self.message = format!(
                "Could not create world directory {}: {error}",
                path.display()
            );
            return;
        }
        let has_level = path.join("level.dat").is_file();
        let marker = session::is_generated(&path);
        match session::read(&path) {
            Ok(Some(state)) => {
                if let Some(seed) = state.seed {
                    self.start_generated(seed, Some(state), false);
                } else if has_level {
                    self.start_save(&path, Some(state));
                } else {
                    self.start_generated(DEFAULT_SEED, Some(state), false);
                }
            }
            Ok(None) => {
                if has_level && !marker {
                    self.start_save(&path, None);
                } else if marker {
                    // The marker proves this is our generated world. Rebuild
                    // it rather than opening region files as if they were a
                    // complete user save.
                    self.start_generated(DEFAULT_SEED, None, true);
                } else {
                    self.start_generated(DEFAULT_SEED, None, false);
                }
            }
            Err(error) => {
                self.last_error = Some(error.to_string());
                if has_level && !marker {
                    self.start_save(&path, None);
                } else {
                    self.start_generated(DEFAULT_SEED, None, true);
                    self.message = format!(
                        "World session was unreadable ({error}); regenerated seed \
                         {DEFAULT_SEED:#x}. Previous edits may be lost."
                    );
                }
            }
        }
    }

    fn start_generated(&mut self, seed: u64, state: Option<Session>, recovered: bool) {
        let generated = crate::worldgen::GeneratedWorld::new(seed);
        let default_spawn = generated.spawn();
        let had_state = state.is_some();
        let session = state.unwrap_or_default();
        self.spawn = session.spawn.unwrap_or(default_spawn);
        self.player = player_from(session.player, self.spawn);
        self.edits = session.edits;
        self.world = Some(WorldState::Generated(generated));
        if !recovered {
            self.message = format!(
                "Generated world seed {seed:#x} ready. Terrain is produced by RKB, \
                 not a bundled save."
            );
        }
        self.save_status = "generated world; save pending".to_owned();
        if !had_state || recovered {
            self.save_quiet();
        }
    }

    fn start_save(&mut self, path: &Path, state: Option<Session>) {
        match load_world(path) {
            Ok(loaded) => {
                self.spawn = loaded.spawn;
                self.player = Player::new(loaded.spawn);
                if let Some(state) = state {
                    if let Some(spawn) = state.spawn {
                        self.spawn = spawn;
                    }
                    if let Some(player) = state.player {
                        self.player = player_from(Some(player), self.spawn);
                    }
                    self.edits = state.edits;
                }
                self.world = Some(WorldState::Save(loaded.world, loaded.chunks));
                self.message = format!(
                    "Minecraft {} save \"{}\" loaded ({} chunks).",
                    loaded.version, loaded.name, loaded.chunks
                );
                self.save_status = if self.edits.is_empty() {
                    "loaded save".to_owned()
                } else {
                    format!("loaded save with {} block edits", self.edits.len())
                };
            }
            Err(error) => {
                self.message = format!("Minecraft world unavailable: {error}");
                self.save_status = "no world".to_owned();
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

    /// `"generated"`, `"save"`, or `"none"`.
    #[must_use]
    pub fn world_kind(&self) -> Option<&'static str> {
        self.world.as_ref().map(WorldState::kind_name)
    }

    /// Last save result, shown next to gameplay status.
    #[must_use]
    pub fn save_status(&self) -> &str {
        &self.save_status
    }

    /// Human-readable lighting source for the HUD and `rkb-join`.
    #[must_use]
    pub fn light_status(&self) -> String {
        match self.world.as_ref() {
            None => "light: no world".to_owned(),
            Some(WorldState::Server(world)) if world.light_columns() > 0 => {
                format!(
                    "light: server LightData ({} columns)",
                    world.light_columns()
                )
            }
            Some(WorldState::Server(_)) => {
                "light: full-sky fallback (server sent no light yet)".to_owned()
            }
            Some(world) => format!(
                "light: full-sky fallback ({} worlds do not decode light)",
                world.kind_name()
            ),
        }
    }

    /// Loads a user-supplied resource pack (a directory or `.zip` archive)
    /// and re-meshes every resident chunk against its blockstates, models,
    /// and textures.
    ///
    /// Only `path` is read: no asset is bundled or cached by RKB, and the
    /// built-in procedural atlas stays available until this succeeds. Loading
    /// again replaces the previous pack, which is how a pack is reloaded.
    /// Returns a human-readable summary for the HUD.
    pub fn load_resource_pack(&mut self, path: &Path) -> Result<String, String> {
        let pack = ResourcePack::open(path).map_err(|error| error.to_string())?;
        let models = PackModels::new(pack);
        let atlas = pack::Atlas::build(&models.store());
        let textures = ActiveTextures::Pack(PackRuntime { models, atlas });
        let summary = textures.status();
        self.textures = textures;
        self.atlas_generation = self.atlas_generation.wrapping_add(1);
        // Every face may now use different geometry and texture ids, so the
        // whole loaded area is queued again rather than kept as-is.
        self.clear_meshes();
        self.center = None;
        self.ensure_area();
        self.message = format!("Resource pack loaded from {}.", path.display());
        Ok(summary)
    }

    /// Drops a loaded pack and returns to the built-in shapes and atlas.
    pub fn clear_resource_pack(&mut self) {
        if matches!(self.textures, ActiveTextures::Pack(_)) {
            self.textures = ActiveTextures::default();
            self.atlas_generation = self.atlas_generation.wrapping_add(1);
            self.clear_meshes();
            self.center = None;
            self.ensure_area();
            self.message = "Resource pack cleared; using built-in textures.".to_owned();
        }
    }

    /// Active texture source: pack path, counters, or the built-in atlas.
    #[must_use]
    pub fn resource_pack_status(&self) -> String {
        self.textures.status()
    }

    #[must_use]
    pub fn stats(&self) -> GameStats {
        GameStats {
            world_loaded: self.world.is_some(),
            chunks_loaded: self.meshes.len(),
            chunks_generated: self.world.as_ref().map_or(0, WorldState::known_chunks),
            chunks_visible: self.visible_chunks,
            chunks_queued: self.queue.len(),
            blocks: self.meshes.values().map(|mesh| mesh.non_air_blocks).sum(),
            faces: self.meshes.values().map(ChunkMesh::face_count).sum(),
            triangles: self.triangles,
            edits: self.edits.len(),
            missing_models: self.meshes.values().map(|mesh| mesh.missing_models).sum(),
            translucent_faces: self
                .meshes
                .values()
                .map(|mesh| mesh.translucent.len())
                .sum(),
            light_columns: self.world.as_ref().map_or(0, WorldState::light_columns),
            block_updates: self.world.as_ref().map_or(0, WorldState::block_updates),
            chunks_received: self.world.as_ref().map_or(0, WorldState::chunks_received),
            unloads: self.world.as_ref().map_or(0, WorldState::unloads),
            resets: self.world.as_ref().map_or(0, WorldState::resets),
            light_fallback: match self.world.as_ref() {
                Some(world) => world.uses_light_fallback(),
                None => true,
            },
            frames: self.frames,
            flying: self.player.flying,
            on_ground: self.player.on_ground,
            position: self.player.position,
            yaw: self.player.yaw,
            pitch: self.player.pitch,
            save_dirty: self.save_dirty,
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

    /// Sets the vertical field of view in degrees, clamped to the same
    /// 30°–110° range as vanilla's FOV slider.
    pub fn set_fov_degrees(&mut self, degrees: f32) {
        self.camera_fov_y = degrees.clamp(30.0, 110.0).to_radians();
    }

    /// Current vertical field of view in degrees.
    #[must_use]
    pub fn fov_degrees(&self) -> f32 {
        self.camera_fov_y.to_degrees()
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
        camera.fov_y = self.camera_fov_y;
        camera
    }

    #[must_use]
    pub fn viewport(&self) -> Vec2 {
        self.viewport
    }

    /// Block at absolute coordinates, including the client edit overlay.
    pub fn block_at(&mut self, x: i32, y: i32, z: i32) -> Result<Option<BlockState>, String> {
        if !self.edits.is_empty() {
            if let Some(state) = self.edits.get(&(x, y, z)) {
                return Ok(Some(state.clone()));
            }
        }
        self.world
            .as_mut()
            .map_or(Ok(None), |world| world.base_block_at(x, y, z))
    }

    /// Replaces a block in the edit overlay and invalidates affected meshes.
    ///
    /// Returns `true` when the world changed. Restoring a cell to its original
    /// saved value removes the overlay entry instead of growing the file.
    pub fn set_block(
        &mut self,
        position: (i32, i32, i32),
        state: BlockState,
    ) -> Result<bool, String> {
        if matches!(
            self.world.as_ref().map(WorldState::kind),
            Some(WorldKind::Server)
        ) {
            return Err(
                "block edits are not sent to a server yet; the server world is read-only"
                    .to_owned(),
            );
        }
        let base = self
            .world
            .as_mut()
            .map_or(Ok(None), |world| {
                world.base_block_at(position.0, position.1, position.2)
            })?
            .unwrap_or_else(BlockState::air);
        let current = self
            .edits
            .get(&position)
            .cloned()
            .unwrap_or_else(|| base.clone());
        if current == state {
            return Ok(false);
        }
        if base == state {
            self.edits.remove(&position);
        } else {
            self.edits.insert(position, state);
        }
        self.save_dirty = true;
        self.save_status = "unsaved block changes".to_owned();
        self.refresh_chunk(position.0.div_euclid(16), position.2.div_euclid(16));
        Ok(true)
    }

    /// Records a new revision for one mesh.
    fn mark_mesh(&mut self, key: (i32, i32)) {
        self.mesh_serial = self.mesh_serial.wrapping_add(1);
        self.mesh_revs.insert(key, self.mesh_serial);
    }

    /// Drops every resident mesh and invalidates external caches.
    fn clear_meshes(&mut self) {
        self.meshes.clear();
        self.mesh_revs.clear();
        self.mesh_epoch = self.mesh_epoch.wrapping_add(1);
    }

    /// Atlas texture the mesher and draw list currently share.
    ///
    /// The GPU backend uploads exactly this texture, so the procedural atlas and
    /// a user pack reach the GPU through the same path.
    #[must_use]
    pub fn atlas_texture(&self) -> Arc<Texture> {
        self.textures.texture()
    }

    /// `[u0, v0, u1, v1]` for one texture id in the active atlas.
    #[must_use]
    pub fn atlas_uv(&self, id: u32) -> [f32; 4] {
        self.textures.uv_id(id)
    }

    /// Bumped when the atlas (or its UV rectangles) changes.
    #[must_use]
    pub fn atlas_generation(&self) -> u64 {
        self.atlas_generation
    }

    /// Bumped when every mesh is dropped, so caches can be cleared wholesale.
    #[must_use]
    pub fn mesh_epoch(&self) -> u64 {
        self.mesh_epoch
    }

    /// Resident meshes with their revision, cheap enough to scan per frame.
    #[must_use]
    pub fn resident_meshes(&self) -> Vec<(&ChunkMesh, u64)> {
        self.meshes
            .iter()
            .map(|(key, mesh)| (mesh, self.mesh_revs.get(key).copied().unwrap_or(0)))
            .collect()
    }

    /// Fog start and end distances in blocks, matching the software renderer.
    #[must_use]
    pub fn fog_range(&self) -> (f32, f32) {
        (FOG_START, FOG_END)
    }

    /// Sky and fog colours used by the software renderer.
    #[must_use]
    pub fn sky_colors(&self) -> (Color, Color) {
        (SKY_ZENITH, SKY_HORIZON)
    }

    /// The 2D HUD commands for one frame, for a GPU UI batch.
    ///
    /// Only 2D commands are produced; the world is drawn by the caller.
    #[must_use]
    pub fn hud_commands(&self) -> Vec<rkb_render::DrawCommand> {
        let mut list = DrawList::new(Color::rgba(0, 0, 0, 0));
        self.draw_hud(&mut list);
        list.commands().to_vec()
    }

    fn refresh_chunk(&mut self, cx: i32, cz: i32) {
        let Some(center) = self.center else {
            return;
        };
        for dz in -1..=1 {
            for dx in -1..=1 {
                let key = (cx + dx, cz + dz);
                if (key.0 - center.0).abs() > LOAD_RADIUS || (key.1 - center.1).abs() > LOAD_RADIUS
                {
                    continue;
                }
                self.meshes.remove(&key);
                self.mesh_revs.remove(&key);
                if !self.queue.contains(&key) {
                    self.queue.push_front(key);
                }
            }
        }
    }

    /// The block under the crosshair, if any.
    pub fn target_block(&mut self) -> Result<Option<RayHit>, String> {
        let eye = self.player.eye();
        let direction = self.camera().forward();
        let mut error = None;
        let hit = raycast(eye, direction, REACH, |x, y, z| {
            match self.block_at(x, y, z) {
                Ok(Some(state)) => {
                    !state.is_air()
                        && !matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava")
                }
                Ok(None) => false,
                Err(cause) => {
                    if error.is_none() {
                        error = Some(cause);
                    }
                    false
                }
            }
        });
        if let Some(error) = error {
            return Err(error);
        }
        Ok(hit)
    }

    /// Breaks the targeted block. Returns the broken position when it changed.
    pub fn break_block(&mut self) -> Result<Option<(i32, i32, i32)>, String> {
        let Some(hit) = self.target_block()? else {
            self.message = "No block within reach.".to_owned();
            return Ok(None);
        };
        if self.set_block(hit.block, BlockState::air())? {
            self.message = format!(
                "Broke block at ({}, {}, {}).",
                hit.block.0, hit.block.1, hit.block.2
            );
            Ok(Some(hit.block))
        } else {
            Ok(None)
        }
    }

    /// Places a plank block against the targeted face.
    pub fn place_block(&mut self) -> Result<Option<(i32, i32, i32)>, String> {
        let Some(hit) = self.target_block()? else {
            self.message = "No block within reach.".to_owned();
            return Ok(None);
        };
        let target = hit.previous;
        if target == hit.block {
            self.message = "Cannot place a block inside terrain.".to_owned();
            return Ok(None);
        }
        if self.intersects_player(target) {
            self.message = "Cannot place a block inside the player.".to_owned();
            return Ok(None);
        }
        let current = self.block_at(target.0, target.1, target.2)?;
        if current.as_ref().is_some_and(|state| !state.is_air()) {
            self.message = "That space is already occupied.".to_owned();
            return Ok(None);
        }
        if self.set_block(target, BlockState::new("minecraft:oak_planks"))? {
            self.message = format!(
                "Placed planks at ({}, {}, {}).",
                target.0, target.1, target.2
            );
            Ok(Some(target))
        } else {
            Ok(None)
        }
    }

    fn intersects_player(&self, block: (i32, i32, i32)) -> bool {
        const EPSILON: f32 = 0.001;
        let player_min_x = self.player.position.x - crate::player::PLAYER_WIDTH * 0.5;
        let player_max_x = self.player.position.x + crate::player::PLAYER_WIDTH * 0.5;
        let player_min_z = self.player.position.z - crate::player::PLAYER_WIDTH * 0.5;
        let player_max_z = self.player.position.z + crate::player::PLAYER_WIDTH * 0.5;
        let player_min_y = self.player.position.y;
        let player_max_y = self.player.position.y + crate::player::PLAYER_HEIGHT;
        let (bx, by, bz) = (block.0 as f32, block.1 as f32, block.2 as f32);
        player_max_x > bx + EPSILON
            && player_min_x < bx + 1.0 - EPSILON
            && player_max_y > by + EPSILON
            && player_min_y < by + 1.0 - EPSILON
            && player_max_z > bz + EPSILON
            && player_min_z < bz + 1.0 - EPSILON
    }

    /// Writes the session and, for generated worlds, region files.
    pub fn save(&mut self) -> Result<(), String> {
        if matches!(
            self.world.as_ref().map(WorldState::kind),
            Some(WorldKind::Server)
        ) {
            self.save_status = "multiplayer world; not written locally".to_owned();
            self.save_dirty = false;
            return Ok(());
        }
        let Some(path) = self.world_path.clone() else {
            return Err("no world directory configured".to_owned());
        };
        let Some(world) = self.world.as_ref() else {
            return Err("no world is loaded".to_owned());
        };
        let state = Session {
            seed: world.seed(),
            spawn: Some(self.spawn),
            player: Some(PlayerSnapshot {
                position: self.player.position,
                yaw: self.player.yaw,
                pitch: self.player.pitch,
                flying: self.player.flying,
            }),
            edits: self.edits.clone(),
        };
        let saved_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as i64);
        session::write(&path, &state, saved_at_ms).map_err(|error| error.to_string())?;

        if let WorldState::Generated(generated) = world {
            let chunks: Vec<rkb_mc::Chunk> =
                generated.chunks().map(|chunk| (**chunk).clone()).collect();
            let level = rkb_mc::save::level_tag(
                crate::worldgen::DATA_VERSION,
                "RKB Generated World",
                [
                    self.spawn.x.floor() as i32,
                    self.spawn.y.floor() as i32,
                    self.spawn.z.floor() as i32,
                ],
            );
            rkb_mc::save::write_world(&path, &level, &chunks).map_err(|error| error.to_string())?;
            session::mark_generated(&path).map_err(|error| error.to_string())?;
            self.save_status = format!(
                "saved {} generated chunks + {} edits",
                chunks.len(),
                self.edits.len()
            );
        } else {
            self.save_status = format!("saved {} block edits", self.edits.len());
        }
        self.save_dirty = false;
        Ok(())
    }

    fn save_quiet(&mut self) {
        if let Err(error) = self.save() {
            self.last_error = Some(error.clone());
            self.save_status = format!("save failed: {error}");
        }
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
        let right = self.viewport.x - margin - side;
        let bottom = self.viewport.y - margin - side;
        self.jump.set_bounds(Rect::new(right, bottom, side, side));
        self.sprint
            .set_bounds(Rect::new(right, bottom - side - gap, side, side));
        self.sneak
            .set_bounds(Rect::new(right, bottom - side - gap * 2.0, side, side));
        self.descend.set_bounds(Rect::new(
            right - side - gap,
            bottom - side - gap * 2.0,
            side,
            side,
        ));
        self.break_button
            .set_bounds(Rect::new(right - (side + gap) * 2.0, bottom, side, side));
        self.place_button
            .set_bounds(Rect::new(right, bottom - side - gap, side, side));
        self.fly
            .set_bounds(Rect::new(right, bottom - (side + gap) * 2.0, side, side));
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
        if self.sneak.handle(event) {
            // Sneak toggles crouching; the server handles the state change.
            // We track the state locally but the server authoritative handles it.
            self.message = if self.player.flying {
                "Sneak toggled (flight mode).".to_owned()
            } else {
                "Sneak toggled.".to_owned()
            };
            return;
        }
        if self.sprint.handle(event) {
            self.message = "Sprint toggled.".to_owned();
            return;
        }
        if self.jump.handle(event) {
            return;
        }
        if self.break_button.handle(event) {
            if self.break_button.press_count() != self.break_presses {
                self.break_presses = self.break_button.press_count();
                if let Err(error) = self.break_block() {
                    self.message = format!("Break failed: {error}");
                }
            }
            return;
        }
        if self.place_button.handle(event) {
            if self.place_button.press_count() != self.place_presses {
                self.place_presses = self.place_button.press_count();
                if let Err(error) = self.place_block() {
                    self.message = format!("Place failed: {error}");
                }
            }
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

    /// Drains server updates, follows server teleports, and re-queues changed
    /// chunk meshes. This never blocks on the socket.
    fn poll_server(&mut self) {
        let Some(world) = self.world.as_mut() else {
            return;
        };
        let updated = world.poll();

        if let Some(position) = world.take_server_position() {
            let spawn = Vec3::new(position.x as f32, position.y as f32, position.z as f32);
            self.spawn = spawn;
            self.player.position = spawn;
            self.player.velocity = Vec3::ZERO;
            self.player.yaw = position.yaw.to_radians() + std::f32::consts::PI;
            self.player.pitch = -position.pitch.to_radians();
            self.center = None;
        }
        if let Some(message) = world.take_server_message() {
            self.message = message;
        }
        if let Some(error) = world.server_error() {
            self.last_error = Some(error.to_owned());
            self.message = format!("Server error: {error}");
        }
        if let Some(reason) = world.server_disconnect() {
            self.message = format!("Disconnected: {reason}");
        }

        if matches!(world.kind(), WorldKind::Server) {
            let unloaded: Vec<(i32, i32)> = self
                .meshes
                .keys()
                .copied()
                .filter(|&(cx, cz)| world.chunk(cx, cz).ok().flatten().is_none())
                .collect();
            for key in unloaded {
                self.meshes.remove(&key);
                self.mesh_revs.remove(&key);
            }
        }

        let center = (
            self.player.position.x.floor().div_euclid(16.0) as i32,
            self.player.position.z.floor().div_euclid(16.0) as i32,
        );
        for key in updated {
            if (key.0 - center.0).abs() > RETAIN_RADIUS || (key.1 - center.1).abs() > RETAIN_RADIUS
            {
                continue;
            }
            self.meshes.remove(&key);
            self.mesh_revs.remove(&key);
            if !self.queue.contains(&key) {
                self.queue.push_front(key);
            }
        }
    }

    fn step(&mut self, dt: f32) {
        self.poll_server();
        let Some(world) = self.world.as_mut() else {
            return;
        };

        let value = self.joystick.value();
        let input = MoveInput {
            forward: -value.y,
            strafe: value.x,
            jump: self.jump.is_pressed(),
            descend: self.player.flying && self.descend.is_pressed(),
            sprint: self.sprint.is_pressed(),
        };
        let mut query = WorldQuery {
            world,
            edits: &self.edits,
            error: None,
        };
        self.player.step(dt, input, &mut query);
        if let Some(error) = query.error {
            self.last_error = Some(error.clone());
            self.message = format!("Block lookup failed: {error}");
        }

        self.player.position.x = self.player.position.x.clamp(-WORLD_BORDER, WORLD_BORDER);
        self.player.position.z = self.player.position.z.clamp(-WORLD_BORDER, WORLD_BORDER);

        if self.player.position.y < (WORLD_MIN_Y + 4) as f32
            && !matches!(world.kind(), WorldKind::Server)
        {
            let before = self.player.position;
            let flying = self.player.flying;
            self.player = Player::new(self.spawn);
            self.player.flying = flying;
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

        self.save_clock += dt;
        if self.save_clock >= AUTOSAVE_SECONDS {
            self.save_clock = 0.0;
            self.save_quiet();
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
            if let WorldState::Save(save, _) = world {
                save.clear_cache();
            }
            world.retain_chunks(center, RETAIN_RADIUS + 1);
        }
        let keep = |&(cx, cz): &(i32, i32)| {
            (cx - center.0).abs() <= RETAIN_RADIUS && (cz - center.1).abs() <= RETAIN_RADIUS
        };
        self.meshes.retain(|&key, _| keep(&key));
        self.mesh_revs.retain(|&key, _| keep(&key));
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
        let ready = match world.kind() {
            // A saved world may legitimately be missing neighboring chunks;
            // mesh it with boundary walls. Generated chunks wait for their
            // full neighbourhood so no false walls appear at seams.
            WorldKind::Save => true,
            WorldKind::Generated | WorldKind::Server => world.neighborhood_ready(cx, cz),
        };
        if !ready {
            self.queue.push_back((cx, cz));
            return true;
        }
        let result = build_chunk_mesh_with(world, cx, cz, &self.edits, self.textures.models());
        match result {
            Ok(Some(mesh)) => {
                self.meshes.insert((cx, cz), mesh);
                self.mark_mesh((cx, cz));
            }
            Ok(None) => {}
            Err(error) => {
                self.last_error = Some(error.clone());
                self.message = format!("Chunk ({cx}, {cz}) failed to load: {error}");
            }
        }
        // Textures decoded while meshing join the atlas before the next frame.
        self.textures.refresh();
        true
    }

    fn draw_sky(&self, list: &mut DrawList) {
        let viewport = self.viewport;
        let horizon_y = viewport.y * 0.62;
        // Smooth zenith-to-horizon gradient. Flat horizontal strips were
        // visible as banding on high-resolution displays.
        list.gradient_triangle(
            Vec2::new(0.0, 0.0),
            Vec2::new(viewport.x, 0.0),
            Vec2::new(viewport.x, horizon_y),
            SKY_ZENITH,
            SKY_ZENITH,
            SKY_HORIZON,
        );
        list.gradient_triangle(
            Vec2::new(0.0, 0.0),
            Vec2::new(viewport.x, horizon_y),
            Vec2::new(0.0, horizon_y),
            SKY_ZENITH,
            SKY_HORIZON,
            SKY_HORIZON,
        );
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
        let mut visible_chunks = 0_usize;
        let atlas = &self.textures;
        let mut translucent = Vec::new();

        for mesh in self.meshes.values() {
            if mesh.is_empty() || !frustum.contains_aabb(mesh.min, mesh.max) {
                continue;
            }
            visible_chunks += 1;
            for face in mesh.faces() {
                if !face.is_visible_from(eye) {
                    continue;
                }
                if face.layer == RenderLayer::Translucent {
                    translucent.push(((face.center() - eye).length_squared(), *face));
                    continue;
                }
                let rect = atlas.uv_id(face.texture);
                let uvs = face.uv.map(|[u, v]| {
                    [
                        rect[0] + u * (rect[2] - rect[0]),
                        rect[1] + v * (rect[3] - rect[1]),
                    ]
                });
                let count = camera.project_polygon_textured(
                    view_projection,
                    viewport,
                    &face.points,
                    &uvs,
                    &face.light,
                    &mut out,
                );
                if count < 3 {
                    continue;
                }
                let distance = (face.center() - eye).length();
                let fog = fog_amount(distance);
                let cutoff = face.layer.cutout();
                for index in 1..count - 1 {
                    list.textured_triangle_3d_ex(
                        out[0],
                        out[index],
                        out[index + 1],
                        face.tint,
                        fog_color(),
                        fog,
                        1.0,
                        cutoff,
                        false,
                        true,
                    );
                    triangles = triangles.saturating_add(1);
                }
            }
        }

        // Translucent faces are composited back-to-front across all visible
        // chunks. They still depth-test against opaque geometry, but do not
        // write depth, so overlapping water/glass does not punch holes.
        translucent.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (distance_squared, face) in translucent {
            let rect = atlas.uv_id(face.texture);
            let uvs = face.uv.map(|[u, v]| {
                [
                    rect[0] + u * (rect[2] - rect[0]),
                    rect[1] + v * (rect[3] - rect[1]),
                ]
            });
            let count = camera.project_polygon_textured(
                view_projection,
                viewport,
                &face.points,
                &uvs,
                &face.light,
                &mut out,
            );
            if count < 3 {
                continue;
            }
            let distance = distance_squared.sqrt();
            let fog = fog_amount(distance);
            for index in 1..count - 1 {
                list.textured_triangle_3d_ex(
                    out[0],
                    out[index],
                    out[index + 1],
                    face.tint,
                    fog_color(),
                    fog,
                    1.0,
                    face.layer.cutout(),
                    true,
                    false,
                );
                triangles = triangles.saturating_add(1);
            }
        }

        self.triangles = triangles;
        self.visible_chunks = visible_chunks;
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
        self.draw_button(list, &self.break_button, HUD_BREAK, ButtonIcon::Break);
        self.draw_button(list, &self.place_button, HUD_PLACE, ButtonIcon::Place);
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
            ButtonIcon::Break => {
                list.rect(
                    Rect::new(center.x - size, center.y - 1.5, size * 2.0, 3.0),
                    HUD_WHITE,
                );
                // A second, rotated stroke is drawn as a thin stepped line.
                let mut step = -size;
                while step < size {
                    list.rect(
                        Rect::new(center.x + step, center.y - step - 1.5, 4.0, 4.0),
                        HUD_WHITE,
                    );
                    step += size * 0.28;
                }
            }
            ButtonIcon::Place => {
                list.stroke_rect(
                    Rect::new(center.x - size, center.y - size, size * 2.0, size * 2.0),
                    2.0,
                    HUD_WHITE,
                );
                list.rect(
                    Rect::new(center.x - size * 0.5, center.y - 1.5, size, 3.0),
                    HUD_WHITE,
                );
                list.rect(
                    Rect::new(center.x - 1.5, center.y - size * 0.5, 3.0, size),
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
    Break,
    Place,
}

impl Game for MinecraftGame {
    fn on_resize(&mut self, viewport: Vec2) {
        self.layout(viewport);
    }

    fn on_stage_change(&mut self, stage: Stage) {
        if matches!(stage, Stage::Paused | Stage::Stopping | Stage::Destroyed)
            && self.world.is_some()
        {
            self.save_quiet();
        }
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
        // Textures decoded during meshing join the atlas before this frame.
        self.textures.refresh();
        list.set_atlas(Some(self.textures.texture()));
        self.draw_sky(list);
        if self.world.is_some() {
            self.draw_scene(list);
        }
        self.draw_hud(list);
    }
}

struct WorldQuery<'a> {
    world: &'a mut WorldState,
    edits: &'a EditMap,
    error: Option<String>,
}

impl BlockQuery for WorldQuery<'_> {
    fn is_solid(&mut self, x: i32, y: i32, z: i32) -> bool {
        if !self.edits.is_empty() {
            if let Some(state) = self.edits.get(&(x, y, z)) {
                return !state.is_air() && is_solid_block(state);
            }
        }
        match self.world.base_block_at(x, y, z) {
            Ok(Some(state)) => !state.is_air() && is_solid_block(&state),
            Ok(None) => false,
            Err(error) => {
                if self.error.is_none() {
                    self.error = Some(error);
                }
                false
            }
        }
    }
}

fn player_from(snapshot: Option<PlayerSnapshot>, spawn: Vec3) -> Player {
    let Some(snapshot) = snapshot else {
        return Player::new(spawn);
    };
    let mut player = Player::new(snapshot.position);
    player.yaw = snapshot.yaw.rem_euclid(std::f32::consts::TAU);
    player.pitch = snapshot.pitch.clamp(-1.553, 1.553);
    player.flying = snapshot.flying;
    player
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

fn describe_world_error(path: &Path, error: &rkb_mc::WorldError) -> String {
    match error {
        rkb_mc::WorldError::Io { .. } => format!(
            "no readable save at {} (expected level.dat and region/)",
            path.display()
        ),
        rkb_mc::WorldError::Version(source) => format!("unsupported save version: {source}"),
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
    fn gpu_accessors_expose_the_same_atlas_and_hud_as_the_software_path() {
        let mut game = MinecraftGame::new(None);
        let atlas = game.atlas_texture();
        assert!(atlas.width() > 0 && atlas.height() > 0);
        let rect = game.atlas_uv(0);
        assert!(rect[2] > rect[0] && rect[3] > rect[1]);
        assert_eq!(game.atlas_generation(), 0);
        assert!(game.resident_meshes().is_empty());

        // The HUD is generated for the laid-out viewport, not a GPU-specific
        // duplicate: the GPU backend batch draws these exact commands.
        game.layout(Vec2::new(640.0, 360.0));
        let commands = game.hud_commands();
        assert!(!commands.is_empty());
        assert!(commands
            .iter()
            .all(|command| !matches!(command, DrawCommand::Triangle3D { .. })));
    }

    #[test]
    fn mesh_revisions_advance_only_for_the_changed_chunk() {
        let mut game = MinecraftGame::new(None);
        let key = (2, -3);
        assert_eq!(game.mesh_epoch(), 0);
        game.meshes.insert(
            key,
            ChunkMesh {
                chunk: key,
                ..ChunkMesh::default()
            },
        );
        game.mark_mesh(key);
        let first = game.resident_meshes();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].1, 1);

        // Re-marking the same mesh advances only its revision.
        game.mark_mesh(key);
        let second = game.resident_meshes();
        assert_eq!(second[0].1, 2);

        // Clearing drops the revision and invalidates external caches.
        game.clear_meshes();
        assert!(game.resident_meshes().is_empty());
        assert_eq!(game.mesh_epoch(), 1);
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
            .any(|command| matches!(command, DrawCommand::TexturedTriangle3D { .. })));
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
        assert!(list.atlas().is_some(), "renderer needs the block atlas");
        assert!(
            list.commands()
                .iter()
                .any(|command| matches!(command, DrawCommand::GradientTriangle { .. })),
            "sky should be a smooth gradient, not flat strips"
        );
        let triangles = list
            .commands()
            .iter()
            .filter(|command| matches!(command, DrawCommand::TexturedTriangle3D { .. }))
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

    #[test]
    fn no_save_startup_generates_terrain_with_real_geometry() {
        let dir = TempDir::new("generated-client");
        let mut game = MinecraftGame::new(Some(dir.path().to_path_buf()));
        assert!(game.has_world(), "{}", game.status());
        assert!(
            game.status().contains("Generated world seed"),
            "{}",
            game.status()
        );
        let viewport = Vec2::new(640.0, 360.0);
        game.on_resize(viewport);
        for _ in 0..600 {
            game.update(context(1.0 / 60.0, viewport));
            if game.stats().chunks_loaded >= 9 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let stats = game.stats();
        assert!(stats.chunks_loaded >= 9, "{stats:?}");
        assert!(stats.chunks_generated >= 9, "{stats:?}");
        assert!(stats.faces > 1_000, "{stats:?}");
        assert!(stats.blocks > 1_000, "{stats:?}");

        game.set_look(0.0, -1.2);
        let mut list = DrawList::new(SKY_HORIZON);
        game.render(&mut list, viewport);
        assert!(game.stats().triangles > 100, "{:?}", game.stats());
        assert!(game.stats().chunks_visible > 0, "{:?}", game.stats());
        assert!(dir.path().join(session::SESSION_FILE).is_file());
        assert!(dir.path().join("level.dat").is_file());
    }

    #[test]
    fn generated_world_saves_and_reloads_seed_player_and_edits() {
        let dir = TempDir::new("generated-reload");
        let mut game = MinecraftGame::new(Some(dir.path().to_path_buf()));
        let seed = game
            .world
            .as_ref()
            .and_then(WorldState::seed)
            .expect("seed");
        let spawn = game.spawn();
        game.set_look(1.0, -0.3);
        game.player.position = spawn + Vec3::new(1.0, 0.0, 1.0);
        let target = (
            spawn.x.floor() as i32,
            spawn.y.floor() as i32 - 1,
            spawn.z.floor() as i32,
        );
        assert!(game.set_block(target, BlockState::air()).unwrap());
        game.save().unwrap();
        let viewport = Vec2::new(640.0, 360.0);
        game.on_resize(viewport);

        let mut reopened = MinecraftGame::new(Some(dir.path().to_path_buf()));
        assert!(reopened.has_world(), "{}", reopened.status());
        assert!(reopened.status().contains("Generated world seed"));
        assert_eq!(
            reopened.world.as_ref().and_then(WorldState::seed),
            Some(seed)
        );
        assert!((reopened.player.yaw - 1.0).abs() < 0.001);
        assert!((reopened.player.position.x - game.player.position.x).abs() < 0.001);
        assert!(reopened.stats().edits >= 1, "{:?}", reopened.stats());
        assert_eq!(
            reopened.block_at(target.0, target.1, target.2).unwrap(),
            Some(BlockState::air())
        );
    }

    #[test]
    fn break_and_place_round_trip_through_the_crosshair() {
        let dir = TempDir::new("interaction-client");
        let mut game = MinecraftGame::new(Some(dir.path().to_path_buf()));
        let viewport = Vec2::new(640.0, 360.0);
        game.on_resize(viewport);
        for _ in 0..40 {
            game.update(context(1.0 / 60.0, viewport));
        }
        let spawn = game.spawn();
        game.teleport(spawn);
        game.set_look(0.0, -1.5);
        for _ in 0..8 {
            game.update(context(1.0 / 60.0, viewport));
        }
        let hit = game.target_block().unwrap().expect("ground underfoot");
        let before = game
            .block_at(hit.block.0, hit.block.1, hit.block.2)
            .unwrap();
        assert!(before.is_some_and(|state| !state.is_air()));
        let broken = game.break_block().unwrap().expect("break target");
        assert_eq!(broken, hit.block);
        assert_eq!(
            game.block_at(broken.0, broken.1, broken.2).unwrap(),
            Some(BlockState::air())
        );
        // Look at the newly exposed neighbour and place the block back.
        game.teleport(spawn);
        game.set_look(0.0, -1.5);
        let placed = game.place_block().unwrap().expect("place a block");
        assert_eq!(
            game.block_at(placed.0, placed.1, placed.2).unwrap(),
            Some(BlockState::new("minecraft:oak_planks"))
        );
        assert!(game.stats().edits >= 1);
    }

    #[test]
    fn generated_world_does_not_overwrite_a_user_save() {
        let dir = TempDir::new("user-save-priority");
        demo_world::write_demo_world(dir.path()).unwrap();
        let game = MinecraftGame::new(Some(dir.path().to_path_buf()));
        assert!(game.has_world(), "{}", game.status());
        assert!(
            game.status().contains("RKB Demo Island"),
            "{}",
            game.status()
        );
        assert_eq!(
            game.world.as_ref().map(WorldState::kind),
            Some(WorldKind::Save)
        );
    }

    /// Writes the smallest vanilla-layout pack that models `minecraft:stone`
    /// with its own 1x1 texture. Everything is authored here; no game asset is
    /// involved.
    fn write_fixture_pack(root: &Path) {
        std::fs::create_dir_all(root.join("assets/minecraft/blockstates")).unwrap();
        std::fs::create_dir_all(root.join("assets/minecraft/models/block")).unwrap();
        std::fs::create_dir_all(root.join("assets/minecraft/textures/block")).unwrap();
        std::fs::write(
            root.join("pack.mcmeta"),
            r#"{"pack":{"pack_format":34,"description":"RKB fixture pack"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("assets/minecraft/blockstates/stone.json"),
            r#"{"variants":{"":{"model":"block/stone"}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("assets/minecraft/models/block/stone.json"),
            r##"{"parent":"block/cube_all","textures":{"all":"block/stone"}}"##,
        )
        .unwrap();
        std::fs::write(
            root.join("assets/minecraft/models/block/cube_all.json"),
            r##"{"elements":[{"from":[0,0,0],"to":[16,16,16],"faces":{
                "north":{"texture":"#all"},"south":{"texture":"#all"},
                "east":{"texture":"#all"},"west":{"texture":"#all"},
                "up":{"texture":"#all"},"down":{"texture":"#all"}}}]}"##,
        )
        .unwrap();
        std::fs::write(
            root.join("assets/minecraft/textures/block/stone.png"),
            crate::pack::pack_png(&[10, 20, 30, 255], 1, 1),
        )
        .unwrap();
    }

    #[test]
    fn resource_pack_replaces_the_builtin_atlas_and_can_be_cleared() {
        let dir = TempDir::new("pack-client");
        demo_world::write_flat_test_world(dir.path(), demo_world::DATA_VERSION).unwrap();
        let root = dir.path().join("pack");
        write_fixture_pack(&root);

        let mut game = MinecraftGame::new(Some(dir.path().to_path_buf()));
        assert!(game.has_world(), "{}", game.status());
        assert!(
            game.resource_pack_status()
                .contains("built-in procedural atlas"),
            "{}",
            game.resource_pack_status()
        );

        let summary = game.load_resource_pack(&root).unwrap();
        assert!(summary.contains("textures: pack"), "{summary}");
        assert!(summary.contains("format 34"), "{summary}");
        // The composed atlas is wider than the procedural one, so the draw
        // list is now sampling pack pixels.
        assert!(
            game.textures.texture().width() > BlockAtlas::new().texture().width(),
            "pack atlas must replace the procedural atlas"
        );

        // Every resident chunk is re-queued and re-meshed against the pack.
        for _ in 0..60 {
            game.update(context(1.0 / 60.0, Vec2::new(640.0, 360.0)));
        }
        assert!(game.stats().faces > 0);
        assert_eq!(game.stats().missing_models, 0);
        let status = game.resource_pack_status();
        assert!(status.contains("2 textures"), "{status}");
        assert!(status.contains("0 unmodelled"), "{status}");
        assert!(status.contains("0 missing"), "{status}");

        // A bad path is reported and leaves the loaded pack untouched.
        let error = game
            .load_resource_pack(&root.join("does-not-exist"))
            .unwrap_err();
        assert!(
            error.contains("neither a directory nor a .zip archive"),
            "{error}"
        );
        assert!(
            game.resource_pack_status().contains("textures: pack"),
            "{}",
            game.resource_pack_status()
        );

        game.clear_resource_pack();
        assert!(
            game.resource_pack_status()
                .contains("built-in procedural atlas"),
            "{}",
            game.resource_pack_status()
        );
        assert!(
            !game.resource_pack_status().contains("textures: pack"),
            "{}",
            game.resource_pack_status()
        );
    }

    #[test]
    fn crosshair_is_centered_from_the_viewport() {
        let dir = TempDir::new("crosshair");
        demo_world::write_flat_test_world(dir.path(), demo_world::DATA_VERSION).unwrap();
        let mut game = MinecraftGame::new(Some(dir.path().to_path_buf()));
        for viewport in [Vec2::new(640.0, 360.0), Vec2::new(1234.0, 777.0)] {
            let mut list = DrawList::new(SKY_HORIZON);
            game.render(&mut list, viewport);
            let center = viewport * 0.5;
            let bar = |width: f32, height: f32| {
                list.commands()
                    .iter()
                    .find_map(|command| match command {
                        DrawCommand::Rect { rect, .. }
                            if rect.width == width && rect.height == height =>
                        {
                            Some(*rect)
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("no {width}x{height} crosshair bar"))
            };
            for rect in [bar(2.0, 16.0), bar(16.0, 2.0)] {
                let actual = Vec2::new(rect.x + rect.width * 0.5, rect.y + rect.height * 0.5);
                assert!(
                    (actual - center).length_squared() < 0.01,
                    "{viewport:?}: crosshair at {actual:?}, expected {center:?}"
                );
            }
        }
    }

    /// Fog far edge must track the chunk load radius, and the ramp must run
    /// from fully clear to fully fogged across it.
    #[test]
    fn fog_range_is_derived_from_the_chunk_load_radius() {
        fn expected_fog_end(radius: i32) -> f32 {
            (radius as f32 + 1.0) * 16.0
        }
        assert!(
            (FOG_END - expected_fog_end(LOAD_RADIUS)).abs() < 0.001,
            "FOG_END {FOG_END} must be one chunk ring past LOAD_RADIUS {LOAD_RADIUS}"
        );
        assert!(fog_amount(0.0).abs() < 0.0001);
        assert!(fog_amount(FOG_START).abs() < 0.0001);
        assert!((fog_amount(FOG_END) - 1.0).abs() < 0.0001);
        assert!((fog_amount(FOG_END * 4.0) - 1.0).abs() < 0.0001);
        let middle = fog_amount((FOG_START + FOG_END) * 0.5);
        assert!(middle > 0.0 && middle < 1.0, "{middle}");
        assert!(fog_amount(FOG_START - 1.0) < middle);
        assert_eq!(fog_color(), SKY_HORIZON);
    }

    #[test]
    fn field_of_view_is_clamped_like_the_vanilla_slider() {
        let mut game = MinecraftGame::new(None);
        assert!((game.fov_degrees() - 70.0).abs() < 0.01);
        game.set_fov_degrees(45.0);
        assert!((game.fov_degrees() - 45.0).abs() < 0.01);
        assert!((game.camera().fov_y.to_degrees() - 45.0).abs() < 0.01);
        game.set_fov_degrees(1000.0);
        assert!((game.fov_degrees() - 110.0).abs() < 0.01);
        game.set_fov_degrees(-5.0);
        assert!((game.fov_degrees() - 30.0).abs() < 0.01);
    }
}
