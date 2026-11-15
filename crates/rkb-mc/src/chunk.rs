//! 1.21 chunk sections, block states, and packed palettes.
//!
//! A chunk's `sections` list holds 16x16x16 sections keyed by `Y`. Each
//! `block_states` container has a palette of state names (with optional
//! properties) and, when the palette has more than one entry, a long array of
//! packed indices. The bit width is `max(4, ceil(log2(palette_length)))` and
//! values never cross a 64-bit word, matching `SimpleBitStorage`.
//!
//! This module reads the saved form; it does not generate terrain, run block
//! behavior, or light the chunk.

use std::collections::BTreeMap;
use std::fmt;

use crate::nbt::Tag;

/// Blocks along one section edge.
pub const SECTION_EDGE: usize = 16;
/// Blocks in one section (16^3).
pub const SECTION_VOLUME: usize = SECTION_EDGE * SECTION_EDGE * SECTION_EDGE;

/// A block state: a namespaced block name plus its properties, exactly as
/// stored in chunk palettes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockState {
    pub name: String,
    pub properties: BTreeMap<String, String>,
}

impl BlockState {
    /// A state with no properties.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            properties: BTreeMap::new(),
        }
    }

    /// Adds one property, returning `self` for chaining.
    pub fn with_property(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.properties.insert(key.into(), value.into());
        self
    }

    pub fn property(&self, key: &str) -> Option<&str> {
        self.properties.get(key).map(String::as_str)
    }

    /// Vanilla's `BlockState.isAir`: air, cave air, and void air.
    pub fn is_air(&self) -> bool {
        matches!(
            self.name.as_str(),
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        )
    }

    /// Converts back to the NBT form used in a chunk palette.
    pub fn to_nbt(&self) -> Tag {
        let mut map = BTreeMap::new();
        map.insert("Name".to_string(), Tag::String(self.name.clone()));
        if !self.properties.is_empty() {
            let properties = self
                .properties
                .iter()
                .map(|(key, value)| (key.clone(), Tag::String(value.clone())))
                .collect();
            map.insert("Properties".to_string(), Tag::Compound(properties));
        }
        Tag::Compound(map)
    }

    /// Parses one palette entry.
    pub fn from_nbt(tag: &Tag) -> Result<Self, BlockStateError> {
        let map = tag.as_compound().ok_or(BlockStateError::NotCompound)?;
        let name = match map.get("Name") {
            Some(Tag::String(name)) => name.clone(),
            Some(_) => return Err(BlockStateError::NameNotString),
            None => return Err(BlockStateError::MissingName),
        };
        if name.is_empty() {
            return Err(BlockStateError::EmptyName);
        }
        let mut properties = BTreeMap::new();
        match map.get("Properties") {
            None => {}
            Some(Tag::Compound(values)) => {
                for (key, value) in values {
                    if key.is_empty() {
                        return Err(BlockStateError::EmptyPropertyName);
                    }
                    let Some(value) = value.as_str() else {
                        return Err(BlockStateError::PropertyNotString { key: key.clone() });
                    };
                    properties.insert(key.clone(), value.to_string());
                }
            }
            Some(_) => return Err(BlockStateError::PropertiesNotCompound),
        }
        Ok(Self { name, properties })
    }
}

impl fmt::Display for BlockState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)?;
        if !self.properties.is_empty() {
            f.write_str("[")?;
            for (index, (key, value)) in self.properties.iter().enumerate() {
                if index > 0 {
                    f.write_str(",")?;
                }
                write!(f, "{key}={value}")?;
            }
            f.write_str("]")?;
        }
        Ok(())
    }
}

/// Why a palette entry is not a valid block state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockStateError {
    NotCompound,
    MissingName,
    NameNotString,
    EmptyName,
    PropertiesNotCompound,
    EmptyPropertyName,
    PropertyNotString { key: String },
}

impl fmt::Display for BlockStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotCompound => f.write_str("block state is not a compound"),
            Self::MissingName => f.write_str("block state has no Name"),
            Self::NameNotString => f.write_str("block state Name is not a string"),
            Self::EmptyName => f.write_str("block state Name is empty"),
            Self::PropertiesNotCompound => f.write_str("block state Properties is not a compound"),
            Self::EmptyPropertyName => f.write_str("block state has an empty property name"),
            Self::PropertyNotString { key } => {
                write!(f, "block state property {key:?} is not a string")
            }
        }
    }
}

impl std::error::Error for BlockStateError {}

/// One 16x16x16 section of a chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// Section coordinate: world Y divided by 16, rounded down.
    pub y: i8,
    palette: Vec<BlockState>,
    /// One palette index per cell, in `(y << 8) | (z << 4) | x` order.
    cells: Vec<u16>,
}

impl Section {
    /// Parses one entry of a chunk's `sections` list.
    pub fn from_nbt(tag: &Tag) -> Result<Self, ChunkError> {
        let map = tag.as_compound().ok_or(ChunkError::WrongType {
            what: "section",
            expected: "compound",
        })?;
        let y = match map.get("Y") {
            Some(Tag::Byte(y)) => *y,
            Some(_) => {
                return Err(ChunkError::WrongType {
                    what: "section.Y",
                    expected: "byte",
                })
            }
            None => return Err(ChunkError::MissingField { what: "section.Y" }),
        };
        let container = map.get("block_states").ok_or(ChunkError::MissingField {
            what: "section.block_states",
        })?;
        let (palette, cells) = parse_block_states(container, y)?;
        Ok(Self { y, palette, cells })
    }

    pub fn palette(&self) -> &[BlockState] {
        &self.palette
    }

    /// Block at a local coordinate. Out-of-range coordinates return `None`.
    pub fn block_at(&self, x: usize, y: usize, z: usize) -> Option<&BlockState> {
        if x >= SECTION_EDGE || y >= SECTION_EDGE || z >= SECTION_EDGE {
            return None;
        }
        let index = (y << 8) | (z << 4) | x;
        self.palette.get(usize::from(self.cells[index]))
    }

    /// Cells whose state is not air, cave air, or void air.
    pub fn non_air_blocks(&self) -> usize {
        self.cells
            .iter()
            .filter(|&&id| {
                self.palette
                    .get(usize::from(id))
                    .map_or(true, |state| !state.is_air())
            })
            .count()
    }

    /// The raw palette index at a local coordinate, for diagnostics.
    pub fn state_index(&self, x: usize, y: usize, z: usize) -> Option<u16> {
        if x >= SECTION_EDGE || y >= SECTION_EDGE || z >= SECTION_EDGE {
            return None;
        }
        Some(self.cells[(y << 8) | (z << 4) | x])
    }
}

fn parse_block_states(tag: &Tag, section: i8) -> Result<(Vec<BlockState>, Vec<u16>), ChunkError> {
    let map = tag.as_compound().ok_or(ChunkError::WrongType {
        what: "section.block_states",
        expected: "compound",
    })?;
    let palette_tags = match map.get("palette") {
        Some(Tag::List(items)) => items,
        Some(_) => {
            return Err(ChunkError::WrongType {
                what: "section.block_states.palette",
                expected: "list",
            })
        }
        None => {
            return Err(ChunkError::MissingField {
                what: "section.block_states.palette",
            })
        }
    };
    if palette_tags.is_empty() {
        return Err(ChunkError::EmptyPalette { section });
    }
    if palette_tags.len() > SECTION_VOLUME {
        return Err(ChunkError::PaletteTooLarge {
            section,
            len: palette_tags.len(),
        });
    }
    let mut palette = Vec::with_capacity(palette_tags.len());
    for (index, entry) in palette_tags.iter().enumerate() {
        let state = BlockState::from_nbt(entry).map_err(|source| ChunkError::BlockState {
            section,
            index,
            source,
        })?;
        palette.push(state);
    }

    let mut cells = vec![0u16; SECTION_VOLUME];
    if palette.len() == 1 {
        // A single-state container has no `data` array.
        return Ok((palette, cells));
    }

    let data = match map.get("data") {
        Some(Tag::LongArray(values)) => values,
        Some(_) => {
            return Err(ChunkError::WrongType {
                what: "section.block_states.data",
                expected: "long array",
            })
        }
        None => return Err(ChunkError::MissingStorage { section }),
    };
    let bits = bits_for_palette(palette.len());
    let per_long = 64 / usize::from(bits);
    let expected = SECTION_VOLUME.div_ceil(per_long);
    if data.len() != expected {
        return Err(ChunkError::BadStorageLength {
            section,
            expected,
            actual: data.len(),
        });
    }
    let mask = (1u64 << bits) - 1;
    for (index, cell) in cells.iter_mut().enumerate() {
        let word = data[index / per_long] as u64;
        let shift = (index % per_long) * usize::from(bits);
        let value = ((word >> shift) & mask) as u16;
        if usize::from(value) >= palette.len() {
            return Err(ChunkError::PaletteIndexOutOfRange {
                section,
                index: value,
                palette: palette.len(),
            });
        }
        *cell = value;
    }
    Ok((palette, cells))
}

fn ceil_log2(value: usize) -> u8 {
    if value <= 1 {
        0
    } else {
        (usize::BITS - (value - 1).leading_zeros()) as u8
    }
}

fn bits_for_palette(len: usize) -> u8 {
    ceil_log2(len).max(4)
}

/// A saved 1.21 chunk: identity plus its sections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub data_version: i32,
    pub x: i32,
    pub z: i32,
    pub status: Option<String>,
    sections: BTreeMap<i8, Section>,
}

impl Chunk {
    /// Parses one chunk NBT root.
    pub fn parse(tag: &Tag) -> Result<Self, ChunkError> {
        let map = tag.as_compound().ok_or(ChunkError::WrongType {
            what: "chunk",
            expected: "compound",
        })?;
        let data_version = match map.get("DataVersion") {
            Some(Tag::Int(value)) => *value,
            Some(_) => {
                return Err(ChunkError::WrongType {
                    what: "chunk.DataVersion",
                    expected: "int",
                })
            }
            None => {
                return Err(ChunkError::MissingField {
                    what: "chunk.DataVersion",
                })
            }
        };
        let x = match map.get("xPos") {
            Some(Tag::Int(value)) => *value,
            Some(_) => {
                return Err(ChunkError::WrongType {
                    what: "chunk.xPos",
                    expected: "int",
                })
            }
            None => return Err(ChunkError::MissingField { what: "chunk.xPos" }),
        };
        let z = match map.get("zPos") {
            Some(Tag::Int(value)) => *value,
            Some(_) => {
                return Err(ChunkError::WrongType {
                    what: "chunk.zPos",
                    expected: "int",
                })
            }
            None => return Err(ChunkError::MissingField { what: "chunk.zPos" }),
        };
        let status = match map.get("Status") {
            Some(Tag::String(status)) => Some(status.clone()),
            Some(_) => {
                return Err(ChunkError::WrongType {
                    what: "chunk.Status",
                    expected: "string",
                })
            }
            None => None,
        };
        let section_tags = match map.get("sections") {
            Some(Tag::List(items)) => items,
            Some(_) => {
                return Err(ChunkError::WrongType {
                    what: "chunk.sections",
                    expected: "list",
                })
            }
            None => {
                return Err(ChunkError::MissingField {
                    what: "chunk.sections",
                })
            }
        };
        let mut sections = BTreeMap::new();
        for section_tag in section_tags {
            let section = Section::from_nbt(section_tag)?;
            if sections.insert(section.y, section).is_some() {
                return Err(ChunkError::DuplicateSection {
                    y: section_tag
                        .get("Y")
                        .and_then(Tag::as_byte)
                        .unwrap_or_default(),
                });
            }
        }
        Ok(Self {
            data_version,
            x,
            z,
            status,
            sections,
        })
    }

    pub fn section(&self, y: i8) -> Option<&Section> {
        self.sections.get(&y)
    }

    pub fn sections(&self) -> impl Iterator<Item = &Section> {
        self.sections.values()
    }

    pub fn section_count(&self) -> usize {
        self.sections.len()
    }

    /// Block at world Y and local chunk X/Z. `None` means the section at that
    /// height was not saved.
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> Option<&BlockState> {
        if !(0..SECTION_EDGE as i32).contains(&x) || !(0..SECTION_EDGE as i32).contains(&z) {
            return None;
        }
        let section_y = i8::try_from(y.div_euclid(SECTION_EDGE as i32)).ok()?;
        let local_y = y.rem_euclid(SECTION_EDGE as i32) as usize;
        self.sections
            .get(&section_y)?
            .block_at(x as usize, local_y, z as usize)
    }

    pub fn non_air_blocks(&self) -> usize {
        self.sections.values().map(Section::non_air_blocks).sum()
    }

    /// Every palette state in the chunk, including duplicates across sections.
    pub fn palette_states(&self) -> impl Iterator<Item = &BlockState> {
        self.sections.values().flat_map(Section::palette)
    }
}

/// Chunk parsing failures. Each variant names the field so a corrupt save can
/// be diagnosed without guessing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChunkError {
    WrongType {
        what: &'static str,
        expected: &'static str,
    },
    MissingField {
        what: &'static str,
    },
    EmptyPalette {
        section: i8,
    },
    PaletteTooLarge {
        section: i8,
        len: usize,
    },
    BlockState {
        section: i8,
        index: usize,
        source: BlockStateError,
    },
    MissingStorage {
        section: i8,
    },
    BadStorageLength {
        section: i8,
        expected: usize,
        actual: usize,
    },
    PaletteIndexOutOfRange {
        section: i8,
        index: u16,
        palette: usize,
    },
    DuplicateSection {
        y: i8,
    },
}

impl fmt::Display for ChunkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongType { what, expected } => {
                write!(f, "{what} must be a {expected}")
            }
            Self::MissingField { what } => write!(f, "missing {what}"),
            Self::EmptyPalette { section } => {
                write!(f, "section {section} has an empty palette")
            }
            Self::PaletteTooLarge { section, len } => {
                write!(f, "section {section} palette has {len} entries (max 4096)")
            }
            Self::BlockState {
                section,
                index,
                source,
            } => write!(f, "section {section} palette entry {index}: {source}"),
            Self::MissingStorage { section } => {
                write!(f, "section {section} has a multi-state palette but no data")
            }
            Self::BadStorageLength {
                section,
                expected,
                actual,
            } => write!(
                f,
                "section {section} data has {actual} longs; expected {expected}"
            ),
            Self::PaletteIndexOutOfRange {
                section,
                index,
                palette,
            } => write!(
                f,
                "section {section} stores palette index {index} with only {palette} entries"
            ),
            Self::DuplicateSection { y } => {
                write!(f, "chunk has more than one section at Y {y}")
            }
        }
    }
}

impl std::error::Error for ChunkError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str) -> BlockState {
        BlockState::new(name.to_string())
    }

    fn palette(values: &[BlockState]) -> Vec<Tag> {
        values.iter().map(BlockState::to_nbt).collect()
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

    fn section_tag(y: i8, values: &[BlockState], data: Option<Vec<i64>>) -> Tag {
        let mut container = BTreeMap::new();
        container.insert("palette".to_string(), Tag::List(palette(values)));
        if let Some(data) = data {
            container.insert("data".to_string(), Tag::LongArray(data));
        }
        let mut section = BTreeMap::new();
        section.insert("Y".to_string(), Tag::Byte(y));
        section.insert("block_states".to_string(), Tag::Compound(container));
        Tag::Compound(section)
    }

    fn chunk_tag(sections: Vec<Tag>) -> Tag {
        let mut chunk = BTreeMap::new();
        chunk.insert("DataVersion".to_string(), Tag::Int(3953));
        chunk.insert("xPos".to_string(), Tag::Int(3));
        chunk.insert("zPos".to_string(), Tag::Int(-2));
        chunk.insert(
            "Status".to_string(),
            Tag::String("minecraft:full".to_string()),
        );
        chunk.insert("sections".to_string(), Tag::List(sections));
        Tag::Compound(chunk)
    }

    #[test]
    fn single_state_section_needs_no_storage() {
        let tag = section_tag(-4, &[state("minecraft:stone")], None);
        let section = Section::from_nbt(&tag).expect("parse");
        assert_eq!(section.y, -4);
        assert_eq!(section.palette().len(), 1);
        assert_eq!(
            section.block_at(0, 0, 0).map(ToString::to_string),
            Some("minecraft:stone".to_string())
        );
        assert_eq!(section.non_air_blocks(), SECTION_VOLUME);
    }

    #[test]
    fn multi_state_palette_packs_without_spanning_longs() {
        let states: Vec<BlockState> = (0..17)
            .map(|index| state(&format!("test:block_{index}")))
            .collect();
        let values: Vec<u16> = (0..SECTION_VOLUME)
            .map(|index| (index % 17) as u16)
            .collect();
        let data = pack(5, &values);
        assert_eq!(data.len(), 342);
        let tag = section_tag(0, &states, Some(data));
        let section = Section::from_nbt(&tag).expect("parse");
        for (index, expected) in values.iter().enumerate() {
            let x = index % 16;
            let y = index / 256;
            let z = (index / 16) % 16;
            assert_eq!(section.state_index(x, y, z), Some(*expected));
            assert_eq!(
                section.block_at(x, y, z),
                Some(&states[usize::from(*expected)])
            );
        }
    }

    #[test]
    fn four_bit_palette_crosses_word_boundaries() {
        let states = [state("minecraft:air"), state("minecraft:stone")];
        let values: Vec<u16> = (0..SECTION_VOLUME)
            .map(|index| ((index / 16) % 2) as u16)
            .collect();
        let data = pack(4, &values);
        assert_eq!(data.len(), 256);
        let tag = section_tag(7, &states, Some(data));
        let section = Section::from_nbt(&tag).expect("parse");
        assert_eq!(section.state_index(15, 0, 0), Some(0));
        assert_eq!(section.state_index(0, 0, 1), Some(1));
        assert_eq!(section.non_air_blocks(), SECTION_VOLUME / 2);
    }

    #[test]
    fn parses_chunk_identity_and_sections() {
        let sections = vec![
            section_tag(-4, &[state("minecraft:bedrock")], None),
            section_tag(0, &[state("minecraft:air")], None),
        ];
        let chunk = Chunk::parse(&chunk_tag(sections)).expect("parse chunk");
        assert_eq!(chunk.data_version, 3953);
        assert_eq!((chunk.x, chunk.z), (3, -2));
        assert_eq!(chunk.status.as_deref(), Some("minecraft:full"));
        assert_eq!(chunk.section_count(), 2);
        assert_eq!(
            chunk.block_at(0, -64, 0).map(ToString::to_string),
            Some("minecraft:bedrock".to_string())
        );
        assert_eq!(
            chunk.block_at(0, 0, 0).map(ToString::to_string),
            Some("minecraft:air".to_string())
        );
        assert!(chunk.block_at(0, 32, 0).is_none());
        assert!(chunk.block_at(-1, 0, 0).is_none());
    }

    #[test]
    fn rejects_bad_palettes_and_storage() {
        let bed = state("minecraft:bedrock");
        let stone = state("minecraft:stone");

        let mut no_data = BTreeMap::new();
        no_data.insert(
            "block_states".to_string(),
            Tag::Compound(BTreeMap::from([(
                "palette".to_string(),
                Tag::List(palette(&[bed.clone(), stone.clone()])),
            )])),
        );
        no_data.insert("Y".to_string(), Tag::Byte(0));
        assert!(matches!(
            Section::from_nbt(&Tag::Compound(no_data)),
            Err(ChunkError::MissingStorage { .. })
        ));

        let wrong_length = section_tag(0, &[bed.clone(), stone.clone()], Some(vec![0; 255]));
        assert!(matches!(
            Section::from_nbt(&wrong_length),
            Err(ChunkError::BadStorageLength { .. })
        ));

        let mut out_of_range = vec![0i64; 256];
        out_of_range[0] = 5;
        let bad_index = section_tag(0, &[bed, stone], Some(out_of_range));
        assert!(matches!(
            Section::from_nbt(&bad_index),
            Err(ChunkError::PaletteIndexOutOfRange { .. })
        ));

        let empty = Tag::Compound(BTreeMap::from([
            ("Y".to_string(), Tag::Byte(0)),
            (
                "block_states".to_string(),
                Tag::Compound(BTreeMap::from([(
                    "palette".to_string(),
                    Tag::List(Vec::new()),
                )])),
            ),
        ]));
        assert!(matches!(
            Section::from_nbt(&empty),
            Err(ChunkError::EmptyPalette { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_sections_and_missing_fields() {
        let duplicate = vec![
            section_tag(0, &[state("minecraft:air")], None),
            section_tag(0, &[state("minecraft:stone")], None),
        ];
        assert!(matches!(
            Chunk::parse(&chunk_tag(duplicate)),
            Err(ChunkError::DuplicateSection { y: 0 })
        ));

        let mut missing = BTreeMap::new();
        missing.insert("DataVersion".to_string(), Tag::Int(3953));
        assert!(matches!(
            Chunk::parse(&Tag::Compound(missing)),
            Err(ChunkError::MissingField { what: "chunk.xPos" })
        ));
    }

    #[test]
    fn block_state_nbt_round_trips_properties() {
        let state = BlockState::new("minecraft:oak_log")
            .with_property("axis", "y")
            .with_property("waterlogged", "false");
        let tag = state.to_nbt();
        assert_eq!(BlockState::from_nbt(&tag).expect("parse"), state);
        assert_eq!(
            state.to_string(),
            "minecraft:oak_log[axis=y,waterlogged=false]"
        );
        assert!(BlockState::new("minecraft:cave_air").is_air());
        assert!(!BlockState::new("minecraft:stone").is_air());
    }

    #[test]
    fn rejects_malformed_block_states() {
        assert_eq!(
            BlockState::from_nbt(&Tag::Int(1)),
            Err(BlockStateError::NotCompound)
        );
        let missing_name = Tag::Compound(BTreeMap::new());
        assert_eq!(
            BlockState::from_nbt(&missing_name),
            Err(BlockStateError::MissingName)
        );
        let wrong_properties = Tag::Compound(BTreeMap::from([
            (
                "Name".to_string(),
                Tag::String("minecraft:stone".to_string()),
            ),
            ("Properties".to_string(), Tag::Int(1)),
        ]));
        assert_eq!(
            BlockState::from_nbt(&wrong_properties),
            Err(BlockStateError::PropertiesNotCompound)
        );
    }
}
