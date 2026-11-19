//! Incremental mesh/atlas synchronisation between the game and the GPU scene.
//!
//! The client reports a revision per resident chunk mesh and a generation for
//! the active atlas. This type remembers what has been uploaded and sends only
//! what changed, with a per-frame budget, so a busy server or a pack reload
//! never turns into a full-world re-upload inside one frame.

use std::collections::BTreeMap;
use std::time::Instant;

use rkb_minecraft::MinecraftGame;

use crate::scene::{GpuError, Scene};

/// Default number of chunk meshes uploaded per frame, matching the client's
/// meshing budget.
pub const DEFAULT_UPLOAD_BUDGET: usize = 2;

/// Result of one synchronisation step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncStats {
    pub uploaded: usize,
    pub removed: usize,
    pub pending: usize,
    pub uploaded_bytes: u64,
    pub atlas_uploads: u32,
}

/// Tracks uploaded revisions and applies changes to a [`Scene`].
#[derive(Debug)]
pub struct SceneSync {
    budget: usize,
    revisions: BTreeMap<(i32, i32), u64>,
    atlas_generation: u64,
    epoch: u64,
    initialized: bool,
}

impl SceneSync {
    #[must_use]
    pub fn new(budget: usize) -> Self {
        Self {
            budget: budget.max(1),
            revisions: BTreeMap::new(),
            atlas_generation: 0,
            epoch: 0,
            initialized: false,
        }
    }

    #[must_use]
    pub fn uploaded_chunks(&self) -> usize {
        self.revisions.len()
    }

    /// Drops the cache so every resident mesh uploads again next sync.
    pub fn invalidate(&mut self) {
        self.revisions.clear();
        self.initialized = false;
    }

    /// Applies changed atlas, new/updated chunks, and removals to `scene`.
    pub fn sync(&mut self, scene: &mut Scene, game: &MinecraftGame) -> Result<SyncStats, GpuError> {
        let mut stats = SyncStats::default();

        let atlas_generation = game.atlas_generation();
        if !self.initialized {
            self.initialized = true;
            self.atlas_generation = atlas_generation;
            self.epoch = game.mesh_epoch();
        } else if atlas_generation != self.atlas_generation {
            self.atlas_generation = atlas_generation;
            // Every mesh UV may reference a different atlas rectangle, so all
            // resident chunks must be converted again.
            scene.set_atlas(&game.atlas_texture())?;
            self.revisions.clear();
            stats.atlas_uploads = 1;
        }
        if game.mesh_epoch() != self.epoch {
            self.epoch = game.mesh_epoch();
            self.revisions.clear();
        }

        let resident = game.resident_meshes();
        let present: std::collections::BTreeSet<(i32, i32)> =
            resident.iter().map(|(mesh, _)| mesh.chunk).collect();
        for key in self.revisions.keys().copied().collect::<Vec<_>>() {
            if !present.contains(&key) {
                scene.remove_chunk(key);
                self.revisions.remove(&key);
                stats.removed += 1;
            }
        }

        for (mesh, revision) in &resident {
            if stats.uploaded >= self.budget {
                stats.pending += 1;
                continue;
            }
            if self.revisions.get(&mesh.chunk) == Some(revision) {
                continue;
            }
            let upload = scene.upload_chunk(mesh, &|id| game.atlas_uv(id))?;
            stats.uploaded += 1;
            stats.uploaded_bytes += upload.uploaded_bytes;
            self.revisions.insert(mesh.chunk, *revision);
        }
        Ok(stats)
    }
}

/// Wall time of one [`SceneSync::sync`] call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncTiming {
    pub stats: SyncStats,
    pub nanos: u64,
}

impl SyncTiming {
    /// Times one sync. Exposed so hosts can report upload cost precisely.
    pub fn measure(
        sync: &mut SceneSync,
        scene: &mut Scene,
        game: &MinecraftGame,
    ) -> Result<Self, GpuError> {
        let started = Instant::now();
        let stats = sync.sync(scene, game)?;
        Ok(Self {
            stats,
            nanos: u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_is_never_zero() {
        let sync = SceneSync::new(0);
        assert_eq!(sync.budget, 1);
        assert!(sync.uploaded_chunks() == 0);
    }

    #[test]
    fn invalidation_clears_the_cache() {
        let mut sync = SceneSync::new(DEFAULT_UPLOAD_BUDGET);
        sync.revisions.insert((1, 2), 7);
        sync.initialized = true;
        sync.invalidate();
        assert_eq!(sync.uploaded_chunks(), 0);
        assert!(!sync.initialized);
    }
}
