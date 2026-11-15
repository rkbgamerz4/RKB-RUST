//! A saved world: `level.dat`, its version, and block lookup through region
//! files.
//!
//! The world's `DataVersion` is checked against [`crate::version::Versions`]
//! before any chunk is read. Unknown or unverified versions are rejected rather
//! than parsed as if every 1.21.x release had identical data. Chunks are loaded
//! on demand and cached; the cache can be cleared on memory-constrained
//! devices.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::chunk::{BlockState, Chunk, ChunkError};
use crate::nbt::{Nbt, NbtError, Tag};
use crate::region::{RegionError, RegionFile};
use crate::version::{VersionError, VersionSpec, Versions};

/// How a world is opened.
#[derive(Clone, Copy, Debug, Default)]
pub struct OpenOptions {
    /// Accept versions that are listed but not individually verified. Unknown
    /// versions are still rejected.
    pub allow_unverified: bool,
}

/// Version and level information from `level.dat`.
#[derive(Clone, Debug)]
pub struct LevelInfo {
    pub data_version: i32,
    /// The descriptor that accepted this world.
    pub version: &'static VersionSpec,
    pub level_name: Option<String>,
    pub last_played: Option<i64>,
    pub spawn: Option<[i32; 3]>,
    pub game_type: Option<i32>,
}

/// Region and chunk counts, without decompressing chunks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldStats {
    pub regions: usize,
    pub chunks: usize,
    /// Region coordinates in ascending order.
    pub region_files: Vec<(i32, i32)>,
}

/// An opened saved world.
#[derive(Debug)]
pub struct World {
    root: PathBuf,
    level: LevelInfo,
    allow_unverified: bool,
    chunks: HashMap<(i32, i32), Option<Arc<Chunk>>>,
}

impl World {
    /// Opens a world directory strictly, accepting only verified versions.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WorldError> {
        Self::open_with(path, OpenOptions::default())
    }

    /// Opens a world with explicit options.
    pub fn open_with(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self, WorldError> {
        let root = path.as_ref().to_path_buf();
        let level_path = root.join("level.dat");
        let bytes = fs::read(&level_path).map_err(|error| WorldError::io(&level_path, error))?;
        let root_tag = Nbt::read_auto(&bytes).map_err(|source| WorldError::LevelDat {
            path: level_path,
            source,
        })?;
        let data = root_tag.get("Data").ok_or(WorldError::MissingData)?;
        let data_version = data
            .get("DataVersion")
            .and_then(Tag::as_int)
            .ok_or(WorldError::MissingDataVersion)?;
        let version = Versions::require_with(data_version, options.allow_unverified)
            .map_err(WorldError::Version)?;
        let level_name = data
            .get("LevelName")
            .and_then(Tag::as_str)
            .map(str::to_owned);
        let last_played = data.get("LastPlayed").and_then(Tag::as_long);
        let spawn = match (
            data.get("SpawnX").and_then(Tag::as_int),
            data.get("SpawnY").and_then(Tag::as_int),
            data.get("SpawnZ").and_then(Tag::as_int),
        ) {
            (Some(x), Some(y), Some(z)) => Some([x, y, z]),
            _ => None,
        };
        let game_type = data.get("GameType").and_then(Tag::as_int);
        Ok(Self {
            root,
            level: LevelInfo {
                data_version,
                version,
                level_name,
                last_played,
                spawn,
                game_type,
            },
            allow_unverified: options.allow_unverified,
            chunks: HashMap::new(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn level(&self) -> &LevelInfo {
        &self.level
    }

    /// Drops every cached chunk. Region headers are not cached, so this is the
    /// only retained world data after the call.
    pub fn clear_cache(&mut self) {
        self.chunks.clear();
    }

    /// Loads a chunk, or returns `None` when its slot is absent.
    pub fn chunk(&mut self, x: i32, z: i32) -> Result<Option<Arc<Chunk>>, WorldError> {
        if let Some(cached) = self.chunks.get(&(x, z)) {
            return Ok(cached.clone());
        }
        let loaded = self.load_chunk(x, z)?;
        self.chunks.insert((x, z), loaded.clone());
        Ok(loaded)
    }

    fn load_chunk(&self, x: i32, z: i32) -> Result<Option<Arc<Chunk>>, WorldError> {
        let region_x = x.div_euclid(32);
        let region_z = z.div_euclid(32);
        let local_x = x.rem_euclid(32) as u8;
        let local_z = z.rem_euclid(32) as u8;
        let path = self
            .root
            .join("region")
            .join(format!("r.{region_x}.{region_z}.mca"));
        if !path.is_file() {
            return Ok(None);
        }
        let mut region = RegionFile::open(&path).map_err(|source| WorldError::Region {
            path: path.clone(),
            source,
        })?;
        let Some((_, tag)) =
            region
                .read_chunk_nbt(local_x, local_z)
                .map_err(|source| WorldError::Region {
                    path: path.clone(),
                    source,
                })?
        else {
            return Ok(None);
        };
        let chunk = Chunk::parse(&tag).map_err(|source| WorldError::Chunk {
            chunk: (x, z),
            source,
        })?;
        if chunk.x != x || chunk.z != z {
            return Err(WorldError::ChunkCoordinateMismatch {
                requested: (x, z),
                found: (chunk.x, chunk.z),
            });
        }
        Versions::require_with(chunk.data_version, self.allow_unverified).map_err(|source| {
            WorldError::ChunkVersion {
                chunk: (x, z),
                source,
            }
        })?;
        Ok(Some(Arc::new(chunk)))
    }

    /// Block at world coordinates. `None` means the chunk or the section at
    /// that height is not present in the save.
    pub fn block_at(&mut self, x: i32, y: i32, z: i32) -> Result<Option<BlockState>, WorldError> {
        let chunk_x = x.div_euclid(16);
        let chunk_z = z.div_euclid(16);
        let Some(chunk) = self.chunk(chunk_x, chunk_z)? else {
            return Ok(None);
        };
        Ok(chunk
            .block_at(x.rem_euclid(16), y, z.rem_euclid(16))
            .cloned())
    }

    /// Counts region files and occupied chunk slots. Headers are read, chunk
    /// payloads are not.
    pub fn stats(&self) -> Result<WorldStats, WorldError> {
        let region_dir = self.root.join("region");
        let mut region_files = Vec::new();
        match fs::read_dir(&region_dir) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|error| WorldError::io(&region_dir, error))?;
                    let path = entry.path();
                    if !path.is_file() {
                        continue;
                    }
                    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                        continue;
                    };
                    if let Some(coords) = parse_region_name(name) {
                        region_files.push(coords);
                    }
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(WorldError::io(&region_dir, error)),
        }
        region_files.sort_unstable();
        region_files.dedup();
        let mut chunks = 0;
        for &(x, z) in &region_files {
            let path = region_dir.join(format!("r.{x}.{z}.mca"));
            let region = RegionFile::open(&path).map_err(|source| WorldError::Region {
                path: path.clone(),
                source,
            })?;
            chunks += region.chunk_count();
        }
        Ok(WorldStats {
            regions: region_files.len(),
            chunks,
            region_files,
        })
    }
}

fn parse_region_name(name: &str) -> Option<(i32, i32)> {
    let rest = name.strip_prefix("r.")?.strip_suffix(".mca")?;
    let mut parts = rest.split('.');
    let x = parts.next()?.parse().ok()?;
    let z = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((x, z))
}

/// World access failures.
#[derive(Debug)]
pub enum WorldError {
    Io {
        path: PathBuf,
        message: String,
    },
    LevelDat {
        path: PathBuf,
        source: NbtError,
    },
    MissingData,
    MissingDataVersion,
    Version(VersionError),
    Region {
        path: PathBuf,
        source: RegionError,
    },
    Chunk {
        chunk: (i32, i32),
        source: ChunkError,
    },
    ChunkVersion {
        chunk: (i32, i32),
        source: VersionError,
    },
    ChunkCoordinateMismatch {
        requested: (i32, i32),
        found: (i32, i32),
    },
}

impl WorldError {
    fn io(path: &Path, error: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    }
}

impl fmt::Display for WorldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "{}: {message}", path.display()),
            Self::LevelDat { path, source } => {
                write!(f, "parse {}: {source}", path.display())
            }
            Self::MissingData => f.write_str("level.dat has no Data compound"),
            Self::MissingDataVersion => f.write_str("level.dat Data has no integer DataVersion"),
            Self::Version(source) => write!(f, "level.dat: {source}"),
            Self::Region { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Chunk { chunk, source } => {
                write!(f, "chunk ({}, {}): {source}", chunk.0, chunk.1)
            }
            Self::ChunkVersion { chunk, source } => {
                write!(f, "chunk ({}, {}): {source}", chunk.0, chunk.1)
            }
            Self::ChunkCoordinateMismatch { requested, found } => write!(
                f,
                "chunk slot ({}, {}) contains chunk ({}, {})",
                requested.0, requested.1, found.0, found.1
            ),
        }
    }
}

impl std::error::Error for WorldError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::LevelDat { source, .. } => Some(source),
            Self::Version(source) => Some(source),
            Self::Region { source, .. } => Some(source),
            Self::Chunk { source, .. } => Some(source),
            Self::ChunkVersion { source, .. } => Some(source),
            Self::Io { .. } | Self::MissingData | Self::MissingDataVersion => None,
            Self::ChunkCoordinateMismatch { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{block, cells, chunk_tag, level_tag, section_tag, write_world, TempDir};

    fn one_block_section(y: i8, name: &str) -> crate::nbt::Tag {
        section_tag(y, &[block(name)], &cells(&[]))
    }

    #[test]
    fn reads_level_info_and_blocks_across_sections() {
        let dir = TempDir::new("world-basic");
        let stone_at_origin = section_tag(
            0,
            &[block("minecraft:air"), block("minecraft:stone")],
            &cells(&[(0, 1)]),
        );
        let chunks = vec![
            (
                0,
                0,
                chunk_tag(
                    3955,
                    0,
                    0,
                    vec![one_block_section(-4, "minecraft:bedrock"), stone_at_origin],
                ),
            ),
            (
                -1,
                -1,
                chunk_tag(3955, -1, -1, vec![one_block_section(0, "minecraft:dirt")]),
            ),
            (
                32,
                0,
                chunk_tag(3955, 32, 0, vec![one_block_section(0, "minecraft:sand")]),
            ),
        ];
        write_world(dir.path(), &level_tag(3955, "Synthetic"), &chunks)
            .expect("write synthetic world");

        let mut world = World::open(dir.path()).expect("open world");
        assert_eq!(world.level().data_version, 3955);
        assert_eq!(world.level().version.id, "1.21.1");
        assert_eq!(world.level().level_name.as_deref(), Some("Synthetic"));
        assert_eq!(world.level().spawn, Some([0, 64, 0]));
        assert_eq!(world.level().game_type, Some(1));

        assert_eq!(
            world
                .block_at(0, -64, 0)
                .expect("block")
                .map(|b| b.to_string()),
            Some("minecraft:bedrock".to_string())
        );
        assert_eq!(
            world
                .block_at(0, 0, 0)
                .expect("block")
                .map(|b| b.to_string()),
            Some("minecraft:stone".to_string())
        );
        assert_eq!(
            world
                .block_at(1, 0, 0)
                .expect("block")
                .map(|b| b.to_string()),
            Some("minecraft:air".to_string())
        );
        assert_eq!(
            world
                .block_at(-1, 0, -1)
                .expect("block")
                .map(|b| b.to_string()),
            Some("minecraft:dirt".to_string())
        );
        assert_eq!(
            world
                .block_at(32 * 16, 0, 0)
                .expect("block")
                .map(|b| b.to_string()),
            Some("minecraft:sand".to_string())
        );
    }

    #[test]
    fn returns_none_for_unloaded_areas_and_sections() {
        let dir = TempDir::new("world-unloaded");
        let chunks = vec![(
            0,
            0,
            chunk_tag(3953, 0, 0, vec![one_block_section(0, "minecraft:stone")]),
        )];
        write_world(dir.path(), &level_tag(3953, "Unloaded"), &chunks)
            .expect("write synthetic world");
        let mut world = World::open(dir.path()).expect("open world");
        assert!(world.block_at(1000, 0, 0).expect("query").is_none());
        assert!(world.block_at(0, 1000, 0).expect("query").is_none());
        world.clear_cache();
        assert!(world.block_at(0, 0, 0).expect("query").is_some());
    }

    #[test]
    fn counts_regions_and_chunk_slots() {
        let dir = TempDir::new("world-stats");
        let chunks = vec![
            (
                0,
                0,
                chunk_tag(3955, 0, 0, vec![one_block_section(0, "minecraft:stone")]),
            ),
            (
                1,
                0,
                chunk_tag(3955, 1, 0, vec![one_block_section(0, "minecraft:stone")]),
            ),
            (
                32,
                0,
                chunk_tag(3955, 32, 0, vec![one_block_section(0, "minecraft:stone")]),
            ),
        ];
        write_world(dir.path(), &level_tag(3955, "Stats"), &chunks).expect("write synthetic world");
        let world = World::open(dir.path()).expect("open world");
        let stats = world.stats().expect("stats");
        assert_eq!(stats.regions, 2);
        assert_eq!(stats.chunks, 3);
        assert_eq!(stats.region_files, vec![(0, 0), (1, 0)]);
    }

    #[test]
    fn rejects_unknown_versions_and_reports_the_supported_set() {
        let dir = TempDir::new("world-version");
        write_world(dir.path(), &level_tag(4080, "Future"), &[]).expect("write synthetic world");
        let error = World::open(dir.path()).expect_err("1.21.2 is not verified");
        assert!(error
            .to_string()
            .contains("unsupported Minecraft data version 4080"));
        assert!(matches!(
            error,
            WorldError::Version(VersionError::Unknown { data_version: 4080 })
        ));
    }

    #[test]
    fn rejects_unsupported_chunk_versions() {
        let dir = TempDir::new("world-chunk-version");
        let chunks = vec![(
            0,
            0,
            chunk_tag(3837, 0, 0, vec![one_block_section(0, "minecraft:stone")]),
        )];
        write_world(dir.path(), &level_tag(3955, "Chunk version"), &chunks)
            .expect("write synthetic world");
        let mut world = World::open(dir.path()).expect("open world");
        let error = world.block_at(0, 0, 0).expect_err("old chunk");
        assert!(matches!(error, WorldError::ChunkVersion { .. }));
    }

    #[test]
    fn detects_chunks_in_the_wrong_slot() {
        let dir = TempDir::new("world-slot");
        let chunks = vec![(
            0,
            0,
            chunk_tag(3955, 7, 0, vec![one_block_section(0, "minecraft:stone")]),
        )];
        write_world(dir.path(), &level_tag(3955, "Wrong slot"), &chunks)
            .expect("write synthetic world");
        let mut world = World::open(dir.path()).expect("open world");
        assert!(matches!(
            world.block_at(0, 0, 0),
            Err(WorldError::ChunkCoordinateMismatch { .. })
        ));
    }

    #[test]
    fn reports_missing_level_dat() {
        let dir = TempDir::new("world-missing");
        let error = World::open(dir.path()).expect_err("no level.dat");
        assert!(matches!(error, WorldError::Io { .. }));
        assert!(error.to_string().contains("level.dat"));
    }

    #[test]
    fn region_name_parsing_is_strict() {
        assert_eq!(parse_region_name("r.0.0.mca"), Some((0, 0)));
        assert_eq!(parse_region_name("r.-1.2.mca"), Some((-1, 2)));
        assert_eq!(parse_region_name("r.0.mca"), None);
        assert_eq!(parse_region_name("r.0.0.0.mca"), None);
        assert_eq!(parse_region_name("level.dat"), None);
    }
}
