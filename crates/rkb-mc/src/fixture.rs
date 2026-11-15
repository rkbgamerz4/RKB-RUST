//! Synthetic saves for tests and verification.
//!
//! Everything here is generated: no Minecraft assets, game files, or Mojang
//! data are included. The helpers write the same NBT and region layout that a
//! 1.21 save uses so the reader is exercised end to end.
//!
//! These builders are deliberately generic. Terrain layout belongs to
//! `rkb-minecraft`'s demo-world generator, which composes them into a
//! structurally valid save for tests and emulator verification.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::chunk::BlockState;
use crate::nbt::{Nbt, Tag};
use crate::region::{Compression, RegionWriter};

/// Failure while writing a synthetic save.
#[derive(Debug)]
pub struct FixtureError(pub String);

impl fmt::Display for FixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FixtureError {}

/// A unique temporary directory removed when the test ends.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("rkb-mc-{label}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).expect("create temporary directory");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub fn block(name: &str) -> BlockState {
    BlockState::new(name.to_string())
}

/// A 4096-cell section array with selected indices set.
pub fn cells(entries: &[(usize, u16)]) -> Vec<u16> {
    let mut cells = vec![0u16; crate::chunk::SECTION_VOLUME];
    for &(index, value) in entries {
        assert!(index < cells.len(), "cell index out of range");
        cells[index] = value;
    }
    cells
}

fn bits_for_palette(len: usize) -> u8 {
    let required = if len <= 1 {
        0
    } else {
        (usize::BITS - (len - 1).leading_zeros()) as u8
    };
    required.max(4)
}

fn pack(bits: u8, values: &[u16]) -> Vec<i64> {
    let per_long = 64 / usize::from(bits);
    let mut words = vec![0i64; values.len().div_ceil(per_long)];
    for (index, value) in values.iter().enumerate() {
        let word = index / per_long;
        let shift = (index % per_long) * usize::from(bits);
        words[word] |= i64::from(*value) << shift;
    }
    words
}

/// Builds a chunk section with a valid palette and non-spanning storage.
pub fn section_tag(y: i8, palette: &[BlockState], cells: &[u16]) -> Tag {
    assert_eq!(cells.len(), crate::chunk::SECTION_VOLUME);
    assert!(!palette.is_empty(), "palette must not be empty");
    let mut container = BTreeMap::new();
    container.insert(
        "palette".to_string(),
        Tag::List(palette.iter().map(BlockState::to_nbt).collect()),
    );
    if palette.len() > 1 {
        let bits = bits_for_palette(palette.len());
        container.insert("data".to_string(), Tag::LongArray(pack(bits, cells)));
    }
    let mut section = BTreeMap::new();
    section.insert("Y".to_string(), Tag::Byte(y));
    section.insert("block_states".to_string(), Tag::Compound(container));
    Tag::Compound(section)
}

pub fn chunk_tag(data_version: i32, x: i32, z: i32, sections: Vec<Tag>) -> Tag {
    let mut chunk = BTreeMap::new();
    chunk.insert("DataVersion".to_string(), Tag::Int(data_version));
    chunk.insert("xPos".to_string(), Tag::Int(x));
    chunk.insert("zPos".to_string(), Tag::Int(z));
    chunk.insert("yPos".to_string(), Tag::Int(-4));
    chunk.insert(
        "Status".to_string(),
        Tag::String("minecraft:full".to_string()),
    );
    chunk.insert("sections".to_string(), Tag::List(sections));
    Tag::Compound(chunk)
}

pub fn level_tag(data_version: i32, name: &str) -> Tag {
    let mut data = BTreeMap::new();
    data.insert("DataVersion".to_string(), Tag::Int(data_version));
    data.insert("LevelName".to_string(), Tag::String(name.to_string()));
    data.insert("LastPlayed".to_string(), Tag::Long(1_700_000_000_000));
    data.insert("SpawnX".to_string(), Tag::Int(0));
    data.insert("SpawnY".to_string(), Tag::Int(64));
    data.insert("SpawnZ".to_string(), Tag::Int(0));
    data.insert("GameType".to_string(), Tag::Int(1));
    let mut root = BTreeMap::new();
    root.insert("Data".to_string(), Tag::Compound(data));
    Tag::Compound(root)
}

/// Writes `level.dat` and one zlib-compressed region file per affected region.
pub fn write_world(
    dir: &Path,
    level: &Tag,
    chunks: &[(i32, i32, Tag)],
) -> Result<(), FixtureError> {
    let region_dir = dir.join("region");
    fs::create_dir_all(&region_dir)
        .map_err(|error| FixtureError(format!("create {}: {error}", region_dir.display())))?;
    let level_bytes = Nbt::write_gzip(level)
        .map_err(|error| FixtureError(format!("encode level.dat: {error}")))?;
    fs::write(dir.join("level.dat"), level_bytes)
        .map_err(|error| FixtureError(format!("write level.dat: {error}")))?;

    let mut regions: BTreeMap<(i32, i32), RegionWriter> = BTreeMap::new();
    for (x, z, tag) in chunks {
        let region = (x.div_euclid(32), z.div_euclid(32));
        let local_x = x.rem_euclid(32) as u8;
        let local_z = z.rem_euclid(32) as u8;
        regions
            .entry(region)
            .or_default()
            .add_chunk_nbt(local_x, local_z, Compression::Zlib, tag)
            .map_err(|error| FixtureError(format!("add chunk ({x}, {z}): {error}")))?;
    }
    for ((x, z), writer) in regions {
        let path = region_dir.join(format!("r.{x}.{z}.mca"));
        writer
            .write_to(&path)
            .map_err(|error| FixtureError(format!("write {}: {error}", path.display())))?;
    }
    Ok(())
}
