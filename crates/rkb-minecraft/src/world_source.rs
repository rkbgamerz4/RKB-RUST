//! The two block sources the client can play: a user save or generated terrain.
//!
//! Both return real [`Chunk`] values. Block edits are kept in one overlay owned
//! by the client, so breaking and placing works identically on a read-only
//! saved region world and on the generated world.

use std::sync::Arc;

use rkb_mc::{BlockState, Chunk, World};

use crate::worldgen::GeneratedWorld;

/// Failure while looking up world data.
pub type SourceResult<T> = Result<T, String>;

/// A source of chunks. Implemented by saved and generated worlds.
pub trait ChunkSource {
    fn chunk(&mut self, cx: i32, cz: i32) -> SourceResult<Option<Arc<Chunk>>>;
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
}

/// An opened world, either a 1.21 save or seeded generated terrain.
#[derive(Debug)]
pub enum WorldState {
    Save(World, usize),
    Generated(GeneratedWorld),
}

impl WorldState {
    #[must_use]
    pub const fn kind(&self) -> WorldKind {
        match self {
            Self::Save(..) => WorldKind::Save,
            Self::Generated(_) => WorldKind::Generated,
        }
    }

    #[must_use]
    pub fn kind_name(&self) -> &'static str {
        match self.kind() {
            WorldKind::Save => "save",
            WorldKind::Generated => "generated",
        }
    }

    /// Chunks known to exist in the source (region headers for saves,
    /// generated-this-session for generated worlds).
    #[must_use]
    pub fn known_chunks(&self) -> usize {
        match self {
            Self::Save(_, known) => *known,
            Self::Generated(world) => world.generated_chunks(),
        }
    }

    #[must_use]
    pub fn seed(&self) -> Option<u64> {
        match self {
            Self::Save(..) => None,
            Self::Generated(world) => Some(world.seed()),
        }
    }

    #[must_use]
    pub fn spawn(&self) -> Option<rkb_core::Vec3> {
        match self {
            Self::Save(..) => None,
            Self::Generated(world) => Some(world.spawn()),
        }
    }

    pub fn poll(&mut self) -> Vec<(i32, i32)> {
        match self {
            Self::Save(..) => Vec::new(),
            Self::Generated(world) => world.poll(),
        }
    }

    pub fn retain_chunks(&mut self, center: (i32, i32), radius: i32) {
        match self {
            Self::Save(..) => {}
            Self::Generated(world) => world.retain_chunks(center, radius),
        }
    }

    /// Clones resident generated chunks for the save writer.
    pub fn save_chunks(&self) -> Vec<Chunk> {
        match self {
            Self::Save(..) => Vec::new(),
            Self::Generated(world) => world.chunks().map(|chunk| (**chunk).clone()).collect(),
        }
    }

    /// Absolute block lookup without the edit overlay.
    pub fn base_block_at(&mut self, x: i32, y: i32, z: i32) -> SourceResult<Option<BlockState>> {
        match self {
            Self::Save(world, _) => world.block_at(x, y, z).map_err(|error| error.to_string()),
            Self::Generated(world) => Ok(world.block_at(x, y, z)),
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
