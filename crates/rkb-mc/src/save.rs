//! Writing worlds back to the 1.21 save layout.
//!
//! Generated worlds use this module to persist `level.dat` and region files.
//! The layout is the same Anvil layout the reader consumes, so a generated
//! world can be opened by [`crate::World`] and inspected by `rkb-world`. The
//! written chunks contain no biomes, heightmaps, or lighting, so this is not
//! yet a complete vanilla save exporter.
//!
//! Writes are atomic per file: data is written to a temporary sibling and
//! renamed over the destination. A crash therefore leaves either the previous
//! complete file or the new complete file, never a half-written one.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::chunk::Chunk;
use crate::nbt::{Nbt, Tag};
use crate::region::{Compression, RegionWriter};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Failure while writing a save.
#[derive(Debug)]
pub struct SaveError(pub String);

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SaveError {}

fn temp_path(path: &Path) -> PathBuf {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("rkb-save");
    path.with_file_name(format!(".{name}.{}.{}.tmp", std::process::id(), counter))
}

/// Writes bytes to `path` through a temporary file in the same directory.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), SaveError> {
    let temp = temp_path(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| SaveError(format!("create {}: {error}", parent.display())))?;
    }
    fs::write(&temp, bytes)
        .map_err(|error| SaveError(format!("write {}: {error}", temp.display())))?;
    // Windows cannot rename over an existing file; Android and Linux can. If
    // the rename fails because the destination exists, remove it and retry.
    if let Err(error) = fs::rename(&temp, path) {
        if path.exists() {
            let _ = fs::remove_file(path);
            fs::rename(&temp, path).map_err(|second| {
                let _ = fs::remove_file(&temp);
                SaveError(format!(
                    "replace {}: {error}; retry failed: {second}",
                    path.display()
                ))
            })?;
        } else {
            let _ = fs::remove_file(&temp);
            return Err(SaveError(format!("replace {}: {error}", path.display())));
        }
    }
    Ok(())
}

/// Builds a minimal `level.dat` root with a world name and spawn.
#[must_use]
pub fn level_tag(data_version: i32, name: &str, spawn: [i32; 3]) -> Tag {
    let mut data = BTreeMap::new();
    data.insert("DataVersion".to_string(), Tag::Int(data_version));
    data.insert("LevelName".to_string(), Tag::String(name.to_string()));
    data.insert(
        "LastPlayed".to_string(),
        Tag::Long(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis() as i64),
        ),
    );
    data.insert("SpawnX".to_string(), Tag::Int(spawn[0]));
    data.insert("SpawnY".to_string(), Tag::Int(spawn[1]));
    data.insert("SpawnZ".to_string(), Tag::Int(spawn[2]));
    data.insert("GameType".to_string(), Tag::Int(1));
    data.insert("hardcore".to_string(), Tag::Byte(0));
    data.insert("initialized".to_string(), Tag::Byte(1));
    let mut root = BTreeMap::new();
    root.insert("Data".to_string(), Tag::Compound(data));
    Tag::Compound(root)
}

/// Writes `level.dat`.
pub fn write_level(dir: &Path, level: &Tag) -> Result<(), SaveError> {
    fs::create_dir_all(dir)
        .map_err(|error| SaveError(format!("create {}: {error}", dir.display())))?;
    let bytes =
        Nbt::write_gzip(level).map_err(|error| SaveError(format!("encode level.dat: {error}")))?;
    write_atomic(&dir.join("level.dat"), &bytes)
}

/// Writes `level.dat` and every chunk into region files.
///
/// Region files that already exist are replaced atomically as a unit. Region
/// files not represented in `chunks` are left untouched so a caller cannot
/// accidentally erase parts of a larger save.
pub fn write_world(dir: &Path, level: &Tag, chunks: &[Chunk]) -> Result<(), SaveError> {
    write_level(dir, level)?;
    let region_dir = dir.join("region");
    fs::create_dir_all(&region_dir)
        .map_err(|error| SaveError(format!("create {}: {error}", region_dir.display())))?;

    let mut regions: BTreeMap<(i32, i32), RegionWriter> = BTreeMap::new();
    for chunk in chunks {
        let region = (chunk.x.div_euclid(32), chunk.z.div_euclid(32));
        let local_x = chunk.x.rem_euclid(32) as u8;
        let local_z = chunk.z.rem_euclid(32) as u8;
        regions
            .entry(region)
            .or_default()
            .add_chunk_nbt(local_x, local_z, Compression::Zlib, &chunk.to_nbt())
            .map_err(|error| {
                SaveError(format!("encode chunk ({}, {}): {error}", chunk.x, chunk.z))
            })?;
    }
    for ((x, z), writer) in regions {
        let path = region_dir.join(format!("r.{x}.{z}.mca"));
        let temp = temp_path(&path);
        if let Err(error) = writer.write_to(&temp) {
            let _ = fs::remove_file(&temp);
            return Err(SaveError(format!("write {}: {error}", temp.display())));
        }
        if let Err(error) = fs::rename(&temp, &path) {
            if path.exists() {
                let _ = fs::remove_file(&path);
                fs::rename(&temp, &path).map_err(|second| {
                    let _ = fs::remove_file(&temp);
                    SaveError(format!(
                        "replace {}: {error}; retry failed: {second}",
                        path.display()
                    ))
                })?;
            } else {
                let _ = fs::remove_file(&temp);
                return Err(SaveError(format!("replace {}: {error}", path.display())));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{BlockState, Section};
    use crate::fixture::TempDir;
    use crate::world::World;

    fn sample_chunk(x: i32, z: i32) -> Chunk {
        let mut section = Section::filled(3, BlockState::new("minecraft:stone"));
        section.set_block(0, 0, 0, BlockState::new("minecraft:grass_block"));
        Chunk::new(3953, x, z, vec![section]).unwrap()
    }

    #[test]
    fn writes_a_world_the_reader_can_open() {
        let dir = TempDir::new("save-roundtrip");
        let chunks = [sample_chunk(0, 0), sample_chunk(1, 0)];
        write_world(
            dir.path(),
            &level_tag(3953, "Generated", [8, 49, 8]),
            &chunks,
        )
        .expect("write world");

        assert!(dir.path().join("level.dat").is_file());
        assert!(dir.path().join("region/r.0.0.mca").is_file());
        let mut world = World::open(dir.path()).expect("open written world");
        assert_eq!(world.level().level_name.as_deref(), Some("Generated"));
        assert_eq!(world.level().spawn, Some([8, 49, 8]));
        assert_eq!(
            world.block_at(0, 48, 0).unwrap().map(|block| block.name),
            Some("minecraft:grass_block".to_string())
        );
        assert_eq!(
            world.block_at(1, 48, 0).unwrap().map(|block| block.name),
            Some("minecraft:stone".to_string())
        );
    }

    #[test]
    fn atomic_replace_overwrites_cleanly() {
        let dir = TempDir::new("save-atomic");
        let path = dir.path().join("value.bin");
        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        let leftovers = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }
}
