//! The block sources the client can play: a user save, generated terrain, or a
//! live server world.
//!
//! All return real [`Chunk`] values, so the mesher does not know which source
//! produced them. Local block edits are kept in one overlay owned by the
//! client; server worlds are read-only until movement and block actions are
//! sent to the server.

use std::sync::Arc;

use rkb_mc::{BlockState, Chunk, World};
use rkb_net::{ServerPosition, ServerWorld};

use crate::worldgen::GeneratedWorld;

/// Failure while looking up world data.
pub type SourceResult<T> = Result<T, String>;

/// A source of chunks. Implemented by saved, generated, and server worlds.
pub trait ChunkSource {
    fn chunk(&mut self, cx: i32, cz: i32) -> SourceResult<Option<Arc<Chunk>>>;

    /// Packed sky/block light at absolute block coordinates.
    ///
    /// Sources without decoded light return the documented full-sky fallback;
    /// callers must report that rather than presenting it as server light.
    fn light_at(&mut self, _x: i32, _y: i32, _z: i32) -> SourceResult<LightSample> {
        Ok(LightSample::FULL_SKY)
    }
}

/// Packed light values, each 0..15.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LightSample {
    pub sky: u8,
    pub block: u8,
}

impl LightSample {
    /// Fallback used by worlds whose light has not been decoded.
    pub const FULL_SKY: Self = Self { sky: 15, block: 0 };

    #[must_use]
    pub fn new(sky: u8, block: u8) -> Self {
        Self {
            sky: sky.min(15),
            block: block.min(15),
        }
    }
}

impl ChunkSource for World {
    fn chunk(&mut self, cx: i32, cz: i32) -> SourceResult<Option<Arc<Chunk>>> {
        World::chunk(self, cx, cz).map_err(|error| error.to_string())
    }
}

/// Which kind of world is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldKind {
    Save,
    Generated,
    Server,
}

/// An opened world, either a 1.21 save or seeded generated terrain.
#[derive(Debug)]
pub enum WorldState {
    Save(World, usize),
    Generated(GeneratedWorld),
    Server(ServerWorld),
}

impl WorldState {
    #[must_use]
    pub const fn kind(&self) -> WorldKind {
        match self {
            Self::Save(..) => WorldKind::Save,
            Self::Generated(_) => WorldKind::Generated,
            Self::Server(_) => WorldKind::Server,
        }
    }

    #[must_use]
    pub fn kind_name(&self) -> &'static str {
        match self.kind() {
            WorldKind::Save => "save",
            WorldKind::Generated => "generated",
            WorldKind::Server => "server",
        }
    }

    /// Chunks known to exist in the source (region headers for saves,
    /// generated-this-session for generated worlds).
    #[must_use]
    pub fn known_chunks(&self) -> usize {
        match self {
            Self::Save(_, known) => *known,
            Self::Generated(world) => world.generated_chunks(),
            Self::Server(world) => world.loaded_chunks(),
        }
    }

    #[must_use]
    pub fn seed(&self) -> Option<u64> {
        match self {
            Self::Save(..) => None,
            Self::Generated(world) => Some(world.seed()),
            Self::Server(_) => None,
        }
    }

    /// Server columns with decoded light; zero for save/generated worlds.
    #[must_use]
    pub fn light_columns(&self) -> usize {
        match self {
            Self::Server(world) => world.light_columns(),
            _ => 0,
        }
    }

    /// Server block updates applied to resident columns; zero for
    /// save/generated worlds, which have no network block updates.
    #[must_use]
    pub fn block_updates(&self) -> usize {
        match self {
            Self::Server(world) => world.block_updates(),
            _ => 0,
        }
    }

    /// Chunk columns the server has delivered so far, including columns that
    /// were later unloaded; zero for save/generated worlds.
    #[must_use]
    pub fn chunks_received(&self) -> usize {
        match self {
            Self::Server(world) => world.chunks_received(),
            _ => 0,
        }
    }

    /// Chunk columns the server unloaded after the join; zero for
    /// save/generated worlds, which are unloaded locally instead.
    #[must_use]
    pub fn unloads(&self) -> usize {
        match self {
            Self::Server(world) => world.unloads(),
            _ => 0,
        }
    }

    /// Respawn/dimension resets received after the join; zero for
    /// save/generated worlds.
    #[must_use]
    pub fn resets(&self) -> usize {
        match self {
            Self::Server(world) => world.resets(),
            _ => 0,
        }
    }

    /// Whether lighting comes from the documented full-sky fallback rather
    /// than decoded world light.
    #[must_use]
    pub fn uses_light_fallback(&self) -> bool {
        match self {
            Self::Server(world) => world.light_columns() == 0,
            _ => true,
        }
    }

    #[must_use]
    pub fn spawn(&self) -> Option<rkb_core::Vec3> {
        match self {
            Self::Save(..) => None,
            Self::Generated(world) => Some(world.spawn()),
            Self::Server(world) => world.position().map(|position| {
                rkb_core::Vec3::new(position.x as f32, position.y as f32, position.z as f32)
            }),
        }
    }

    pub fn poll(&mut self) -> Vec<(i32, i32)> {
        match self {
            Self::Save(..) => Vec::new(),
            Self::Generated(world) => world.poll(),
            Self::Server(world) => world.poll(),
        }
    }

    pub fn retain_chunks(&mut self, center: (i32, i32), radius: i32) {
        match self {
            Self::Save(..) => {}
            Self::Generated(world) => world.retain_chunks(center, radius),
            // The server decides when a column unloads. Dropping local copies
            // early would leave holes if the player turns around.
            Self::Server(_) => {}
        }
    }

    /// Clones resident generated chunks for the save writer.
    pub fn save_chunks(&self) -> Vec<Chunk> {
        match self {
            Self::Save(..) => Vec::new(),
            Self::Generated(world) => world.chunks().map(|chunk| (**chunk).clone()).collect(),
            Self::Server(_) => Vec::new(),
        }
    }

    /// Absolute block lookup without the edit overlay.
    pub fn base_block_at(&mut self, x: i32, y: i32, z: i32) -> SourceResult<Option<BlockState>> {
        match self {
            Self::Save(world, _) => world.block_at(x, y, z).map_err(|error| error.to_string()),
            Self::Generated(world) => Ok(world.block_at(x, y, z)),
            Self::Server(world) => Ok(world.block_at(x, y, z)),
        }
    }

    /// Checks that all nine chunks around `(cx, cz)` are available. Missing
    /// generated chunks are queued automatically.
    pub fn neighborhood_ready(&mut self, cx: i32, cz: i32) -> bool {
        for dz in -1..=1 {
            for dx in -1..=1 {
                match self {
                    Self::Save(world, _) => {
                        if world
                            .chunk(cx + dx, cz + dz)
                            .map_err(|error| error.to_string())
                            .ok()
                            .flatten()
                            .is_none()
                        {
                            return false;
                        }
                    }
                    Self::Generated(world) => {
                        if world.chunk(cx + dx, cz + dz).is_none() {
                            return false;
                        }
                    }
                    Self::Server(world) => {
                        if world.chunk(cx + dx, cz + dz).is_none() {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }
}

impl ChunkSource for WorldState {
    fn chunk(&mut self, cx: i32, cz: i32) -> SourceResult<Option<Arc<Chunk>>> {
        match self {
            Self::Save(world, _) => World::chunk(world, cx, cz).map_err(|error| error.to_string()),
            Self::Generated(world) => Ok(world.chunk(cx, cz)),
            Self::Server(world) => Ok(world.chunk(cx, cz)),
        }
    }

    fn light_at(&mut self, x: i32, y: i32, z: i32) -> SourceResult<LightSample> {
        match self {
            Self::Server(world) => {
                let (sky, block) = world.light_at(x, y, z);
                Ok(LightSample::new(sky, block))
            }
            // Save and generated worlds do not decode light arrays; the game
            // reports this fallback explicitly.
            _ => Ok(LightSample::FULL_SKY),
        }
    }
}

impl WorldState {
    /// Takes a server position update once, so the camera can follow a join or
    /// a server teleport without fighting local movement.
    pub fn take_server_position(&mut self) -> Option<ServerPosition> {
        match self {
            Self::Server(world) => world.take_position(),
            _ => None,
        }
    }

    /// Takes a status message from the play session.
    pub fn take_server_message(&mut self) -> Option<String> {
        match self {
            Self::Server(world) => world.take_message(),
            _ => None,
        }
    }

    /// Why a server session ended, if it did.
    #[must_use]
    pub fn server_disconnect(&self) -> Option<&str> {
        match self {
            Self::Server(world) => world.disconnect_reason(),
            _ => None,
        }
    }

    /// A protocol or chunk conversion failure reported by the play session.
    #[must_use]
    pub fn server_error(&self) -> Option<&str> {
        match self {
            Self::Server(world) => world.error(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rkb_mc::fixture::TempDir;

    #[test]
    fn save_and_generated_sources_expose_real_chunks() {
        let dir = TempDir::new("world-source");
        let save_dir = dir.path().join("save");
        let generated = WorldState::Generated(GeneratedWorld::new(1));
        assert_eq!(generated.kind(), WorldKind::Generated);
        assert!(generated.known_chunks() >= 9);
        assert!(generated.seed().is_some());

        crate::demo_world::write_flat_test_world(&save_dir, 3953).unwrap();
        let mut save = WorldState::Save(World::open(&save_dir).unwrap(), 1);
        assert_eq!(save.kind_name(), "save");
        assert!(save.chunk(0, 0).unwrap().is_some());
        assert!(save.base_block_at(0, 48, 0).unwrap().is_some());
    }
}
