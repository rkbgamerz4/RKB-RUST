//! Typed play-state packets for Minecraft: Java Edition 1.21.1 (protocol 767).
//!
//! Every `decode` takes a packet body **without** the packet id and rejects
//! trailing bytes. Every `encode` returns the id followed by the body, for mock
//! servers and round-trip tests.
//!
//! The play state is large. This module models what the M3 diagnostic and the
//! renderer actually need: join, keep-alive, position, chunks and light, block
//! updates, health, inventory, entities, time, experience, chat, and
//! disconnects. Unknown ids stay unknown instead of being guessed.

use crate::codec::{Reader, Writer};
use crate::error::{Error, Result};
use crate::ids;

/// Maximum identifier length accepted from a play packet.
pub const MAX_IDENTIFIER: usize = 32_767;
/// Sanity ceiling for one anonymous NBT value in a play packet.
pub const MAX_NBT: usize = 4 * 1024 * 1024;
/// Sanity ceiling for one network chunk-section buffer.
pub const MAX_CHUNK_DATA: usize = 8 * 1024 * 1024;
/// Largest number of sections a dimension may have (height 1024).
pub const MAX_SECTIONS: usize = 64;

fn read_count(reader: &mut Reader<'_>, what: &str, max: usize) -> Result<usize> {
    let count = reader.read_varint()?;
    if count < 0 {
        return Err(Error::protocol(format!("negative {what} count {count}")));
    }
    let count = count as usize;
    if count > max {
        return Err(Error::protocol(format!(
            "{what} count {count} exceeds {max}"
        )));
    }
    Ok(count)
}

fn read_dimension_name(reader: &mut Reader<'_>) -> Result<String> {
    reader.read_string(MAX_IDENTIFIER)
}

fn check_game_mode(game_mode: i8) -> Result<()> {
    if !(0..=3).contains(&game_mode) {
        return Err(Error::protocol(format!(
            "game mode {game_mode} is outside 0..=3"
        )));
    }
    Ok(())
}

/// A block coordinate encoded as vanilla's packed position long.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Encodes x (26 bits), z (26 bits), and y (12 bits) into one `i64`.
    #[must_use]
    pub fn to_packed(self) -> i64 {
        ((i64::from(self.x) & 0x3FF_FFFF) << 38)
            | ((i64::from(self.z) & 0x3FF_FFFF) << 12)
            | (i64::from(self.y) & 0xFFF)
    }

    /// Decodes vanilla's packed position long with sign extension.
    #[must_use]
    pub fn from_packed(value: i64) -> Self {
        Self {
            x: (value >> 38) as i32,
            y: ((value << 52) >> 52) as i32,
            z: ((value << 26) >> 38) as i32,
        }
    }

    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        Ok(Self::from_packed(reader.read_i64()?))
    }

    pub fn encode(self, writer: &mut Writer) {
        writer.write_i64(self.to_packed());
    }
}

/// A VarInt-length-prefixed set of 64-bit words, vanilla's `BitSet`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BitSet {
    words: Vec<u64>,
}

impl BitSet {
    /// Maximum words accepted for a light mask (64 sections).
    const MAX_WORDS: usize = 64;

    #[must_use]
    pub fn from_words(words: Vec<u64>) -> Self {
        Self { words }
    }

    #[must_use]
    pub fn words(&self) -> &[u64] {
        &self.words
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// Whether `bit` is set beyond the stored words.
    #[must_use]
    pub fn get(&self, bit: usize) -> bool {
        let word = bit / 64;
        let offset = bit % 64;
        self.words
            .get(word)
            .is_some_and(|word| word & (1u64 << offset) != 0)
    }

    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        let count = read_count(reader, "BitSet word", Self::MAX_WORDS)?;
        let mut words = Vec::with_capacity(count);
        for _ in 0..count {
            words.push(reader.read_i64()? as u64);
        }
        Ok(Self { words })
    }

    pub fn encode(&self, writer: &mut Writer) -> Result<()> {
        writer.write_varint(
            i32::try_from(self.words.len())
                .map_err(|_| Error::protocol("BitSet has more words than a VarInt can encode"))?,
        );
        for word in &self.words {
            writer.write_i64(*word as i64);
        }
        Ok(())
    }
}

/// Sky and block light masks plus their nibble arrays.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LightData {
    pub sky_light_mask: BitSet,
    pub block_light_mask: BitSet,
    pub empty_sky_light_mask: BitSet,
    pub empty_block_light_mask: BitSet,
    pub sky_light: Vec<Vec<u8>>,
    pub block_light: Vec<Vec<u8>>,
}

impl LightData {
    /// One section's light array is 2048 bytes; allow a little slack.
    const MAX_LIGHT_ARRAY: usize = 4096;
    const MAX_LIGHT_ARRAYS: usize = 1024;

    fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        let sky_light_mask = BitSet::decode(reader)?;
        let block_light_mask = BitSet::decode(reader)?;
        let empty_sky_light_mask = BitSet::decode(reader)?;
        let empty_block_light_mask = BitSet::decode(reader)?;
        let sky_light = decode_light_arrays(reader)?;
        let block_light = decode_light_arrays(reader)?;
        Ok(Self {
            sky_light_mask,
            block_light_mask,
            empty_sky_light_mask,
            empty_block_light_mask,
            sky_light,
            block_light,
        })
    }

    fn encode(&self, writer: &mut Writer) -> Result<()> {
        self.sky_light_mask.encode(writer)?;
        self.block_light_mask.encode(writer)?;
        self.empty_sky_light_mask.encode(writer)?;
        self.empty_block_light_mask.encode(writer)?;
        encode_light_arrays(&self.sky_light, writer)?;
        encode_light_arrays(&self.block_light, writer)?;
        Ok(())
    }
}

fn decode_light_arrays(reader: &mut Reader<'_>) -> Result<Vec<Vec<u8>>> {
    let count = read_count(reader, "light array", LightData::MAX_LIGHT_ARRAYS)?;
    let mut arrays = Vec::with_capacity(count);
    for _ in 0..count {
        let bytes = reader.read_byte_array(LightData::MAX_LIGHT_ARRAY)?;
        arrays.push(bytes.to_vec());
    }
    Ok(arrays)
}

fn encode_light_arrays(arrays: &[Vec<u8>], writer: &mut Writer) -> Result<()> {
    writer.write_varint(
        i32::try_from(arrays.len()).map_err(|_| Error::protocol("too many light arrays"))?,
    );
    for array in arrays {
        writer.write_byte_array(array, LightData::MAX_LIGHT_ARRAY)?;
    }
    Ok(())
}

/// One inventory slot.
///
/// 1.21.1 sends data components with non-default item stacks. Decoding every
/// component is not implemented in M3, so a stack with components is rejected
/// explicitly rather than misread. Empty slots and component-free stacks work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Empty,
    Item { item_id: i32, count: i32 },
}

impl Slot {
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        let count = reader.read_varint()?;
        if count == 0 {
            return Ok(Self::Empty);
        }
        if !(1..=127).contains(&count) {
            return Err(Error::protocol(format!(
                "inventory slot count {count} is outside 1..=127"
            )));
        }
        let item_id = reader.read_varint()?;
        if item_id < 0 {
            return Err(Error::protocol("negative item id"));
        }
        let added = read_count(reader, "added data component", 4096)?;
        let removed = read_count(reader, "removed data component", 4096)?;
        if added != 0 || removed != 0 {
            return Err(Error::protocol(format!(
                "item {item_id} has {added} added and {removed} removed data components; \
                 M3 does not decode item components"
            )));
        }
        Ok(Self::Item { item_id, count })
    }

    pub fn encode(self, writer: &mut Writer) -> Result<()> {
        match self {
            Self::Empty => writer.write_varint(0),
            Self::Item { item_id, count } => {
                if !(1..=127).contains(&count) {
                    return Err(Error::protocol("invalid encoded slot count"));
                }
                writer.write_varint(count);
                writer.write_varint(item_id);
                writer.write_varint(0);
                writer.write_varint(0);
            }
        }
        Ok(())
    }
}

/// A paletted container from the network chunk format.
///
/// Every variant is `bits per entry` (one byte), a palette, then a
/// length-prefixed long array. Single-valued containers use a zero bit width
/// and still carry a zero-length long array.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PalettedContainer {
    Single(u32),
    Indirect {
        bits_per_entry: u8,
        palette: Vec<u32>,
        data: Vec<u64>,
    },
    Direct {
        bits_per_entry: u8,
        data: Vec<u64>,
    },
}

impl PalettedContainer {
    fn entries_per_word(bits_per_entry: u8) -> usize {
        64 / usize::from(bits_per_entry)
    }

    fn expected_words(entry_count: usize, bits_per_entry: u8) -> usize {
        let per_word = Self::entries_per_word(bits_per_entry);
        entry_count.div_ceil(per_word)
    }

    /// Decodes a paletted container.
    ///
    /// `entry_count` is 4096 for blocks and 64 for biomes. `indirect_max_bits`
    /// is 8 for blocks and 3 for biomes; larger bit widths are direct global
    /// ids. `max_bits` is 15 for blocks and 6 for biomes.
    pub fn decode(
        reader: &mut Reader<'_>,
        entry_count: usize,
        indirect_max_bits: u8,
        max_bits: u8,
    ) -> Result<Self> {
        let bits_per_entry = reader.read_u8()?;
        if bits_per_entry > max_bits {
            return Err(Error::protocol(format!(
                "paletted container uses {bits_per_entry} bits, maximum is {max_bits}"
            )));
        }
        if bits_per_entry == 0 {
            let value = reader.read_varint()?;
            if value < 0 {
                return Err(Error::protocol("negative single palette value"));
            }
            // Vanilla's zero-bit storage still serialises its (empty) long
            // array, so a `VarInt` word count of zero follows the value.
            let words = reader.read_varint()?;
            if words != 0 {
                return Err(Error::protocol(format!(
                    "single-valued paletted container declares {words} data words"
                )));
            }
            return Ok(Self::Single(value as u32));
        }

        let mut palette = Vec::new();
        let mut direct = false;
        if bits_per_entry <= indirect_max_bits {
            let palette_len = read_count(reader, "palette", 1 << bits_per_entry)?;
            if palette_len == 0 {
                return Err(Error::protocol("empty indirect palette"));
            }
            palette.reserve(palette_len);
            for _ in 0..palette_len {
                let value = reader.read_varint()?;
                if value < 0 {
                    return Err(Error::protocol("negative palette entry"));
                }
                palette.push(value as u32);
            }
        } else {
            direct = true;
        }

        let expected = Self::expected_words(entry_count, bits_per_entry);
        let words = read_count(reader, "paletted data word", expected.max(1) * 2)?;
        if words != expected {
            return Err(Error::protocol(format!(
                "paletted container has {words} data words, expected {expected} for \
                 {entry_count} entries at {bits_per_entry} bits"
            )));
        }
        let mut data = Vec::with_capacity(words);
        for _ in 0..words {
            data.push(reader.read_i64()? as u64);
        }

        if direct {
            Ok(Self::Direct {
                bits_per_entry,
                data,
            })
        } else {
            Ok(Self::Indirect {
                bits_per_entry,
                palette,
                data,
            })
        }
    }

    pub fn encode(&self, writer: &mut Writer, entry_count: usize) -> Result<()> {
        match self {
            Self::Single(value) => {
                writer.write_u8(0);
                writer.write_varint(
                    i32::try_from(*value).map_err(|_| {
                        Error::protocol("single palette value does not fit a VarInt")
                    })?,
                );
                // Zero-bit storage writes an empty long array, i.e. a zero
                // word count.
                writer.write_varint(0);
            }
            Self::Indirect {
                bits_per_entry,
                palette,
                data,
            } => {
                if *bits_per_entry == 0 || *bits_per_entry > 8 {
                    return Err(Error::protocol("invalid indirect palette bit width"));
                }
                if palette.is_empty() {
                    return Err(Error::protocol("empty indirect palette"));
                }
                writer.write_u8(*bits_per_entry);
                writer.write_varint(
                    i32::try_from(palette.len())
                        .map_err(|_| Error::protocol("palette is too long"))?,
                );
                for value in palette {
                    writer.write_varint(
                        i32::try_from(*value)
                            .map_err(|_| Error::protocol("palette value does not fit a VarInt"))?,
                    );
                }
                let expected = Self::expected_words(entry_count, *bits_per_entry);
                if data.len() != expected {
                    return Err(Error::protocol("paletted data length mismatch"));
                }
                writer.write_varint(
                    i32::try_from(data.len())
                        .map_err(|_| Error::protocol("paletted data is too long"))?,
                );
                for word in data {
                    writer.write_i64(*word as i64);
                }
            }
            Self::Direct {
                bits_per_entry,
                data,
            } => {
                if *bits_per_entry <= 8 || *bits_per_entry > 15 {
                    return Err(Error::protocol("invalid direct palette bit width"));
                }
                writer.write_u8(*bits_per_entry);
                let expected = Self::expected_words(entry_count, *bits_per_entry);
                if data.len() != expected {
                    return Err(Error::protocol("paletted data length mismatch"));
                }
                writer.write_varint(
                    i32::try_from(data.len())
                        .map_err(|_| Error::protocol("paletted data is too long"))?,
                );
                for word in data {
                    writer.write_i64(*word as i64);
                }
            }
        }
        Ok(())
    }

    /// Resolves one entry, or `None` when the index or palette entry is out of
    /// range.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<u32> {
        match self {
            Self::Single(value) => Some(*value),
            Self::Indirect {
                bits_per_entry,
                palette,
                data,
            } => {
                let raw = unpack(data, *bits_per_entry, index)?;
                palette.get(raw as usize).copied()
            }
            Self::Direct {
                bits_per_entry,
                data,
            } => unpack(data, *bits_per_entry, index),
        }
    }
}

fn unpack(data: &[u64], bits: u8, index: usize) -> Option<u32> {
    let per_word = 64 / usize::from(bits);
    let word = data.get(index / per_word)?;
    let offset = (index % per_word) * usize::from(bits);
    let mask = (1u64 << bits) - 1;
    Some(((*word >> offset) & mask) as u32)
}

/// One 16x16x16 chunk section: block states and biomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkSection {
    pub block_count: i16,
    pub blocks: PalettedContainer,
    pub biomes: PalettedContainer,
}

impl ChunkSection {
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        let block_count = reader.read_i16()?;
        let blocks = PalettedContainer::decode(reader, 4096, 8, 15)?;
        let biomes = PalettedContainer::decode(reader, 64, 3, 6)?;
        Ok(Self {
            block_count,
            blocks,
            biomes,
        })
    }

    pub fn encode(&self, writer: &mut Writer) -> Result<()> {
        writer.write_i16(self.block_count);
        self.blocks.encode(writer, 4096)?;
        self.biomes.encode(writer, 64)?;
        Ok(())
    }

    /// A block state id at section-local coordinates.
    #[must_use]
    pub fn block_state(&self, x: usize, y: usize, z: usize) -> Option<u32> {
        if x >= 16 || y >= 16 || z >= 16 {
            return None;
        }
        self.blocks.get((y << 8) | (z << 4) | x)
    }

    /// A biome id at section-local coordinates, where each biome cell is 4x4x4.
    #[must_use]
    pub fn biome(&self, x: usize, y: usize, z: usize) -> Option<u32> {
        if x >= 16 || y >= 16 || z >= 16 {
            return None;
        }
        self.biomes
            .get(((y >> 2) << 4) | ((z >> 2) << 2) | (x >> 2))
    }
}

/// A decoded network chunk column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkColumn {
    pub x: i32,
    pub z: i32,
    /// Lowest world Y of the dimension this column belongs to.
    pub min_y: i32,
    pub sections: Vec<ChunkSection>,
}

impl ChunkColumn {
    pub fn decode(x: i32, z: i32, min_y: i32, data: &[u8], section_count: usize) -> Result<Self> {
        if !(1..=MAX_SECTIONS).contains(&section_count) {
            return Err(Error::protocol(format!(
                "chunk section count {section_count} is outside 1..={MAX_SECTIONS}"
            )));
        }
        let mut reader = Reader::new(data);
        let mut sections = Vec::with_capacity(section_count);
        for _ in 0..section_count {
            sections.push(ChunkSection::decode(&mut reader)?);
        }
        reader.finish()?;
        Ok(Self {
            x,
            z,
            min_y,
            sections,
        })
    }

    /// Number of sections.
    #[must_use]
    pub fn section_count(&self) -> usize {
        self.sections.len()
    }

    /// The world Y of the lowest section.
    #[must_use]
    pub fn min_section_y(&self) -> i32 {
        self.min_y >> 4
    }

    /// The block state id at world coordinates, or `None` outside this column.
    #[must_use]
    pub fn block_state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        if !(0..16).contains(&x) || !(0..16).contains(&z) {
            return None;
        }
        let section_index = (y - self.min_y).div_euclid(16);
        if section_index < 0 {
            return None;
        }
        let section = self.sections.get(section_index as usize)?;
        section.block_state(
            x as usize,
            (y - self.min_y).rem_euclid(16) as usize,
            z as usize,
        )
    }

    /// The biome id at world coordinates, or `None` outside this column.
    #[must_use]
    pub fn biome(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        if !(0..16).contains(&x) || !(0..16).contains(&z) {
            return None;
        }
        let section_index = (y - self.min_y).div_euclid(16);
        if section_index < 0 {
            return None;
        }
        let section = self.sections.get(section_index as usize)?;
        section.biome(
            x as usize,
            (y - self.min_y).rem_euclid(16) as usize,
            z as usize,
        )
    }

    /// Count of non-air blocks according to the server's per-section counts.
    #[must_use]
    pub fn non_air_blocks(&self) -> i64 {
        self.sections
            .iter()
            .map(|section| i64::from(section.block_count.max(0)))
            .sum()
    }
}

/// A block entity reference; its NBT is kept raw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockEntity {
    pub packed_xz: u8,
    pub y: i16,
    pub kind: i32,
    pub nbt: Option<Vec<u8>>,
}

impl BlockEntity {
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        let packed_xz = reader.read_u8()?;
        let y = reader.read_i16()?;
        let kind = reader.read_varint()?;
        let nbt = reader.read_optional_nbt_raw(MAX_NBT)?.map(<[u8]>::to_vec);
        Ok(Self {
            packed_xz,
            y,
            kind,
            nbt,
        })
    }

    pub fn encode(&self, writer: &mut Writer) -> Result<()> {
        writer.write_u8(self.packed_xz);
        writer.write_i16(self.y);
        writer.write_varint(self.kind);
        match &self.nbt {
            Some(nbt) => writer.write_bytes(nbt),
            None => writer.write_u8(0),
        }
        Ok(())
    }

    /// Chunk-local X coordinate (`0..=15`).
    #[must_use]
    pub fn x(&self) -> i32 {
        i32::from(self.packed_xz >> 4)
    }

    /// Chunk-local Z coordinate (`0..=15`).
    #[must_use]
    pub fn z(&self) -> i32 {
        i32::from(self.packed_xz & 0x0F)
    }
}

/// Clientbound `Login` (join game).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinGame {
    pub entity_id: i32,
    pub hardcore: bool,
    pub world_names: Vec<String>,
    pub max_players: i32,
    pub view_distance: i32,
    pub simulation_distance: i32,
    pub reduced_debug_info: bool,
    pub enable_respawn_screen: bool,
    pub do_limited_crafting: bool,
    pub spawn: SpawnInfo,
    pub enforces_secure_chat: bool,
}

impl JoinGame {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let packet = Self::read(&mut reader)?;
        reader.finish()?;
        Ok(packet)
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let entity_id = reader.read_i32()?;
        let hardcore = reader.read_bool()?;
        let world_count = read_count(reader, "world name", 1024)?;
        let mut world_names = Vec::with_capacity(world_count);
        for _ in 0..world_count {
            world_names.push(reader.read_string(MAX_IDENTIFIER)?);
        }
        let max_players = reader.read_varint()?;
        let view_distance = reader.read_varint()?;
        let simulation_distance = reader.read_varint()?;
        if max_players < 0 || view_distance < 0 || simulation_distance < 0 {
            return Err(Error::protocol(
                "negative join-game distance or player count",
            ));
        }
        let reduced_debug_info = reader.read_bool()?;
        let enable_respawn_screen = reader.read_bool()?;
        let do_limited_crafting = reader.read_bool()?;
        let spawn = SpawnInfo::read(reader)?;
        let enforces_secure_chat = reader.read_bool()?;
        Ok(Self {
            entity_id,
            hardcore,
            world_names,
            max_players,
            view_distance,
            simulation_distance,
            reduced_debug_info,
            enable_respawn_screen,
            do_limited_crafting,
            spawn,
            enforces_secure_chat,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::LOGIN);
        writer.write_i32(self.entity_id);
        writer.write_bool(self.hardcore);
        writer.write_varint(
            i32::try_from(self.world_names.len())
                .map_err(|_| Error::protocol("too many world names"))?,
        );
        for name in &self.world_names {
            writer.write_string(name, MAX_IDENTIFIER)?;
        }
        writer.write_varint(self.max_players);
        writer.write_varint(self.view_distance);
        writer.write_varint(self.simulation_distance);
        writer.write_bool(self.reduced_debug_info);
        writer.write_bool(self.enable_respawn_screen);
        writer.write_bool(self.do_limited_crafting);
        self.spawn.write(&mut writer)?;
        writer.write_bool(self.enforces_secure_chat);
        Ok(writer.into_inner())
    }
}

/// A respawn location, also used inside `Login` and `Respawn`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnInfo {
    /// Registry id of the dimension type.
    pub dimension_type: i32,
    pub dimension_name: String,
    pub hashed_seed: i64,
    pub game_mode: i8,
    pub previous_game_mode: i8,
    pub is_debug: bool,
    pub is_flat: bool,
    pub death: Option<DeathLocation>,
    pub portal_cooldown: i32,
}

impl SpawnInfo {
    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let dimension_type = reader.read_varint()?;
        if dimension_type < 0 {
            return Err(Error::protocol("negative dimension type id"));
        }
        let dimension_name = read_dimension_name(reader)?;
        let hashed_seed = reader.read_i64()?;
        let game_mode = reader.read_u8()? as i8;
        check_game_mode(game_mode)?;
        let previous_game_mode = reader.read_u8()? as i8;
        let is_debug = reader.read_bool()?;
        let is_flat = reader.read_bool()?;
        let death = reader.read_optional(|reader| {
            Ok(DeathLocation {
                dimension_name: read_dimension_name(reader)?,
                position: BlockPos::decode(reader)?,
            })
        })?;
        let portal_cooldown = reader.read_varint()?;
        if portal_cooldown < 0 {
            return Err(Error::protocol("negative portal cooldown"));
        }
        Ok(Self {
            dimension_type,
            dimension_name,
            hashed_seed,
            game_mode,
            previous_game_mode,
            is_debug,
            is_flat,
            death,
            portal_cooldown,
        })
    }

    fn write(&self, writer: &mut Writer) -> Result<()> {
        check_game_mode(self.game_mode)?;
        writer.write_varint(self.dimension_type);
        writer.write_string(&self.dimension_name, MAX_IDENTIFIER)?;
        writer.write_i64(self.hashed_seed);
        writer.write_i8(self.game_mode);
        writer.write_u8(self.previous_game_mode as u8);
        writer.write_bool(self.is_debug);
        writer.write_bool(self.is_flat);
        writer.write_optional(self.death.as_ref(), |writer, death| {
            writer.write_string(&death.dimension_name, MAX_IDENTIFIER)?;
            death.position.encode(writer);
            Ok(())
        })?;
        writer.write_varint(self.portal_cooldown);
        Ok(())
    }
}

/// Where the player died.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeathLocation {
    pub dimension_name: String,
    pub position: BlockPos,
}

/// Clientbound `KeepAlive`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeepAlive {
    pub id: i64,
}

impl KeepAlive {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let id = reader.read_i64()?;
        reader.finish()?;
        Ok(Self { id })
    }

    /// Encodes the serverbound reply.
    #[must_use]
    pub fn encode_serverbound(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::serverbound::KEEP_ALIVE);
        writer.write_i64(self.id);
        writer.into_inner()
    }
}

/// Clientbound `SynchronizePlayerPosition`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyncPlayerPosition {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    /// Bits: x, y, z, yaw, pitch are relative to the current value.
    pub flags: u8,
    pub teleport_id: i32,
}

impl SyncPlayerPosition {
    pub const RELATIVE_X: u8 = 0x01;
    pub const RELATIVE_Y: u8 = 0x02;
    pub const RELATIVE_Z: u8 = 0x04;
    pub const RELATIVE_YAW: u8 = 0x08;
    pub const RELATIVE_PITCH: u8 = 0x10;

    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let packet = Self {
            x: reader.read_f64()?,
            y: reader.read_f64()?,
            z: reader.read_f64()?,
            yaw: reader.read_f32()?,
            pitch: reader.read_f32()?,
            flags: reader.read_u8()?,
            teleport_id: reader.read_varint()?,
        };
        reader.finish()?;
        if packet.flags & !0x1F != 0 {
            return Err(Error::protocol(format!(
                "unknown position-relative flags 0x{:02X}",
                packet.flags
            )));
        }
        if packet.teleport_id < 0 {
            return Err(Error::protocol("negative teleport id"));
        }
        Ok(packet)
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::SYNC_PLAYER_POSITION);
        writer.write_f64(self.x);
        writer.write_f64(self.y);
        writer.write_f64(self.z);
        writer.write_f32(self.yaw);
        writer.write_f32(self.pitch);
        writer.write_u8(self.flags);
        writer.write_varint(self.teleport_id);
        Ok(writer.into_inner())
    }
}

/// Clientbound `SetHealth`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UpdateHealth {
    pub health: f32,
    pub food: i32,
    pub food_saturation: f32,
}

impl UpdateHealth {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let packet = Self {
            health: reader.read_f32()?,
            food: reader.read_varint()?,
            food_saturation: reader.read_f32()?,
        };
        reader.finish()?;
        if !(0..=20).contains(&packet.food) {
            return Err(Error::protocol(format!(
                "food level {} is outside 0..=20",
                packet.food
            )));
        }
        Ok(packet)
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::UPDATE_HEALTH);
        writer.write_f32(self.health);
        writer.write_varint(self.food);
        writer.write_f32(self.food_saturation);
        Ok(writer.into_inner())
    }
}

/// Clientbound `BlockUpdate`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockChange {
    pub position: BlockPos,
    pub state_id: i32,
}

impl BlockChange {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let position = BlockPos::decode(&mut reader)?;
        let state_id = reader.read_varint()?;
        reader.finish()?;
        if state_id < 0 {
            return Err(Error::protocol("negative block state id"));
        }
        Ok(Self { position, state_id })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::BLOCK_CHANGE);
        self.position.encode(&mut writer);
        writer.write_varint(self.state_id);
        Ok(writer.into_inner())
    }
}

/// Clientbound `SectionBlocksUpdate` (multi block change).
///
/// One packet carries every changed block of a single 16³ section. The body
/// is a packed section coordinate written as one big-endian `i64` (`x` in the
/// top 22 bits, `z` in the next 22, `y` in the low 20), a VarInt record
/// count, then one VarInt per record: the new state id in the high bits and
/// the section-relative position (`local x` 4, `local z` 4, `local y` 4) in
/// the low 12. The layout was verified against captured 1.21.1 packets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionBlocksUpdate {
    /// The changed blocks with absolute positions, in wire order.
    pub changes: Vec<BlockChange>,
}

impl SectionBlocksUpdate {
    /// Largest number of records one section can hold (16³).
    pub const MAX_RECORDS: usize = 4096;
    /// Largest state id one record can carry (the low 12 bits hold the
    /// position, and a record must stay a non-negative `i32`).
    pub const MAX_STATE_ID: i32 = 524_287;

    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let section = reader.read_i64()?;
        let count = reader.read_varint()?;
        if !(0..=Self::MAX_RECORDS as i32).contains(&count) {
            return Err(Error::protocol(format!(
                "section block update count {count}"
            )));
        }
        // Section position: x (top 22 bits), z (next 22), y (low 20).
        let section_x = (section >> 42) as i32;
        let section_z = ((section << 22) >> 42) as i32;
        let section_y = ((section << 44) >> 44) as i32;
        let mut changes = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let record = reader.read_varint()?;
            if record < 0 {
                return Err(Error::protocol("negative section block update record"));
            }
            let state_id = record >> 12;
            let local_x = (record >> 8) & 0xF;
            let local_z = (record >> 4) & 0xF;
            let local_y = record & 0xF;
            changes.push(BlockChange {
                position: BlockPos::new(
                    section_x * 16 + local_x,
                    section_y * 16 + local_y,
                    section_z * 16 + local_z,
                ),
                state_id,
            });
        }
        reader.finish()?;
        Ok(Self { changes })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let first = self
            .changes
            .first()
            .ok_or_else(|| Error::protocol("empty section blocks update"))?;
        let section_x = first.position.x.div_euclid(16);
        let section_y = first.position.y.div_euclid(16);
        let section_z = first.position.z.div_euclid(16);
        let count = i32::try_from(self.changes.len())
            .map_err(|_| Error::protocol("too many section block updates"))?;
        if count > Self::MAX_RECORDS as i32 {
            return Err(Error::protocol("too many section block updates"));
        }
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::SECTION_BLOCKS_UPDATE);
        let section = ((i64::from(section_x) & 0x3F_FFFF) << 42)
            | ((i64::from(section_z) & 0x3F_FFFF) << 20)
            | (i64::from(section_y) & 0xF_FFFF);
        writer.write_i64(section);
        writer.write_varint(count);
        for change in &self.changes {
            if change.position.x.div_euclid(16) != section_x
                || change.position.y.div_euclid(16) != section_y
                || change.position.z.div_euclid(16) != section_z
            {
                return Err(Error::protocol(
                    "section blocks update spans more than one section",
                ));
            }
            if !(0..=Self::MAX_STATE_ID).contains(&change.state_id) {
                return Err(Error::protocol("block state id out of range"));
            }
            let relative = ((change.position.x.rem_euclid(16) as u32) << 8)
                | ((change.position.z.rem_euclid(16) as u32) << 4)
                | (change.position.y.rem_euclid(16) as u32);
            let record = ((change.state_id as u32) << 12) | (relative & 0xFFF);
            writer.write_varint(record as i32);
        }
        Ok(writer.into_inner())
    }
}

/// Clientbound `UnloadChunk`.
///
/// The 1.21.1 packet writes Z before X, matching the official packet report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnloadChunk {
    pub x: i32,
    pub z: i32,
}

impl UnloadChunk {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let z = reader.read_i32()?;
        let x = reader.read_i32()?;
        reader.finish()?;
        Ok(Self { x, z })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::UNLOAD_CHUNK);
        writer.write_i32(self.z);
        writer.write_i32(self.x);
        Ok(writer.into_inner())
    }
}

/// Clientbound `GameEvent` (game state change).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GameStateChange {
    pub reason: u8,
    pub value: f32,
}

impl GameStateChange {
    pub const REASON_CHANGE_GAME_MODE: u8 = 3;

    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let reason = reader.read_u8()?;
        let value = reader.read_f32()?;
        reader.finish()?;
        Ok(Self { reason, value })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::GAME_STATE_CHANGE);
        writer.write_u8(self.reason);
        writer.write_f32(self.value);
        Ok(writer.into_inner())
    }
}

/// Clientbound `Respawn`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Respawn {
    pub spawn: SpawnInfo,
    pub copy_metadata: u8,
}

impl Respawn {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let spawn = SpawnInfo::read(&mut reader)?;
        let copy_metadata = reader.read_u8()?;
        reader.finish()?;
        Ok(Self {
            spawn,
            copy_metadata,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::RESPAWN);
        self.spawn.write(&mut writer)?;
        writer.write_u8(self.copy_metadata);
        Ok(writer.into_inner())
    }
}

/// Clientbound `Disconnect` (kick).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KickDisconnect {
    /// Anonymous NBT text component, kept raw for the chat formatter.
    pub reason_nbt: Vec<u8>,
}

impl KickDisconnect {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let reason_nbt = reader.read_anonymous_nbt_raw(MAX_NBT)?.to_vec();
        reader.finish()?;
        Ok(Self { reason_nbt })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::KICK_DISCONNECT);
        writer.write_bytes(&self.reason_nbt);
        Ok(writer.into_inner())
    }
}

/// Clientbound `SetTime`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpdateTime {
    pub age: i64,
    pub time: i64,
}

impl UpdateTime {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let age = reader.read_i64()?;
        let time = reader.read_i64()?;
        reader.finish()?;
        Ok(Self { age, time })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::UPDATE_TIME);
        writer.write_i64(self.age);
        writer.write_i64(self.time);
        Ok(writer.into_inner())
    }
}

/// Clientbound `SetExperience`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Experience {
    pub bar: f32,
    pub level: i32,
    pub total: i32,
}

impl Experience {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let packet = Self {
            bar: reader.read_f32()?,
            level: reader.read_varint()?,
            total: reader.read_varint()?,
        };
        reader.finish()?;
        if packet.level < 0 || packet.total < 0 {
            return Err(Error::protocol("negative experience values"));
        }
        Ok(packet)
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::EXPERIENCE);
        writer.write_f32(self.bar);
        writer.write_varint(self.level);
        writer.write_varint(self.total);
        Ok(writer.into_inner())
    }
}

/// Clientbound `SetHeldItem`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldItem {
    pub slot: i8,
}

impl HeldItem {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let slot = reader.read_u8()? as i8;
        reader.finish()?;
        if !(0..=8).contains(&slot) {
            return Err(Error::protocol(format!(
                "held-item slot {slot} is outside 0..=8"
            )));
        }
        Ok(Self { slot })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::HELD_ITEM_SLOT);
        writer.write_i8(self.slot);
        Ok(writer.into_inner())
    }
}

/// Clientbound `SetContainerContent` (window items).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowItems {
    pub window_id: u8,
    pub state_id: i32,
    pub items: Vec<Slot>,
    pub carried_item: Slot,
}

impl WindowItems {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let window_id = reader.read_u8()?;
        let state_id = reader.read_varint()?;
        if state_id < 0 {
            return Err(Error::protocol("negative window state id"));
        }
        let count = read_count(&mut reader, "window item", 4096)?;
        let mut items = Vec::with_capacity(count);
        for _ in 0..count {
            items.push(Slot::decode(&mut reader)?);
        }
        let carried_item = Slot::decode(&mut reader)?;
        reader.finish()?;
        Ok(Self {
            window_id,
            state_id,
            items,
            carried_item,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::WINDOW_ITEMS);
        writer.write_u8(self.window_id);
        writer.write_varint(self.state_id);
        writer.write_varint(
            i32::try_from(self.items.len())
                .map_err(|_| Error::protocol("too many window items"))?,
        );
        for item in &self.items {
            item.encode(&mut writer)?;
        }
        self.carried_item.encode(&mut writer)?;
        Ok(writer.into_inner())
    }
}

/// Clientbound `SetContainerSlot`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetSlot {
    pub window_id: u8,
    pub state_id: i32,
    pub slot: i16,
    pub item: Slot,
}

impl SetSlot {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let window_id = reader.read_u8()?;
        let state_id = reader.read_varint()?;
        if state_id < 0 {
            return Err(Error::protocol("negative window state id"));
        }
        let slot = reader.read_i16()?;
        let item = Slot::decode(&mut reader)?;
        reader.finish()?;
        Ok(Self {
            window_id,
            state_id,
            slot,
            item,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::SET_SLOT);
        writer.write_u8(self.window_id);
        writer.write_varint(self.state_id);
        writer.write_i16(self.slot);
        self.item.encode(&mut writer)?;
        Ok(writer.into_inner())
    }
}

/// Clientbound `SpawnEntity`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpawnEntity {
    pub entity_id: i32,
    pub uuid: u128,
    pub kind: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub pitch: i8,
    pub yaw: i8,
    pub head_pitch: i8,
    pub data: i32,
    pub velocity: [i16; 3],
}

impl SpawnEntity {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let packet = Self {
            entity_id: reader.read_varint()?,
            uuid: reader.read_uuid()?,
            kind: reader.read_varint()?,
            x: reader.read_f64()?,
            y: reader.read_f64()?,
            z: reader.read_f64()?,
            pitch: reader.read_u8()? as i8,
            yaw: reader.read_u8()? as i8,
            head_pitch: reader.read_u8()? as i8,
            data: reader.read_varint()?,
            velocity: [reader.read_i16()?, reader.read_i16()?, reader.read_i16()?],
        };
        reader.finish()?;
        if packet.entity_id < 0 || packet.kind < 0 {
            return Err(Error::protocol("negative entity id or type"));
        }
        Ok(packet)
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::SPAWN_ENTITY);
        writer.write_varint(self.entity_id);
        writer.write_uuid(self.uuid);
        writer.write_varint(self.kind);
        writer.write_f64(self.x);
        writer.write_f64(self.y);
        writer.write_f64(self.z);
        writer.write_i8(self.pitch);
        writer.write_i8(self.yaw);
        writer.write_i8(self.head_pitch);
        writer.write_varint(self.data);
        for value in self.velocity {
            writer.write_i16(value);
        }
        Ok(writer.into_inner())
    }
}

/// Clientbound relative entity move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelativeEntityMove {
    pub entity_id: i32,
    pub dx: i16,
    pub dy: i16,
    pub dz: i16,
    pub on_ground: bool,
}

impl RelativeEntityMove {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let packet = Self {
            entity_id: reader.read_varint()?,
            dx: reader.read_i16()?,
            dy: reader.read_i16()?,
            dz: reader.read_i16()?,
            on_ground: reader.read_bool()?,
        };
        reader.finish()?;
        if packet.entity_id < 0 {
            return Err(Error::protocol("negative entity id"));
        }
        Ok(packet)
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::REL_ENTITY_MOVE);
        writer.write_varint(self.entity_id);
        writer.write_i16(self.dx);
        writer.write_i16(self.dy);
        writer.write_i16(self.dz);
        writer.write_bool(self.on_ground);
        Ok(writer.into_inner())
    }
}

/// Clientbound relative entity move and rotation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntityMoveLook {
    pub entity_id: i32,
    pub dx: i16,
    pub dy: i16,
    pub dz: i16,
    pub yaw: i8,
    pub pitch: i8,
    pub on_ground: bool,
}

impl EntityMoveLook {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let packet = Self {
            entity_id: reader.read_varint()?,
            dx: reader.read_i16()?,
            dy: reader.read_i16()?,
            dz: reader.read_i16()?,
            yaw: reader.read_u8()? as i8,
            pitch: reader.read_u8()? as i8,
            on_ground: reader.read_bool()?,
        };
        reader.finish()?;
        if packet.entity_id < 0 {
            return Err(Error::protocol("negative entity id"));
        }
        Ok(packet)
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::ENTITY_MOVE_LOOK);
        writer.write_varint(self.entity_id);
        writer.write_i16(self.dx);
        writer.write_i16(self.dy);
        writer.write_i16(self.dz);
        writer.write_i8(self.yaw);
        writer.write_i8(self.pitch);
        writer.write_bool(self.on_ground);
        Ok(writer.into_inner())
    }
}

/// Clientbound entity rotation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntityLook {
    pub entity_id: i32,
    pub yaw: i8,
    pub pitch: i8,
    pub on_ground: bool,
}

impl EntityLook {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let packet = Self {
            entity_id: reader.read_varint()?,
            yaw: reader.read_u8()? as i8,
            pitch: reader.read_u8()? as i8,
            on_ground: reader.read_bool()?,
        };
        reader.finish()?;
        if packet.entity_id < 0 {
            return Err(Error::protocol("negative entity id"));
        }
        Ok(packet)
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::ENTITY_LOOK);
        writer.write_varint(self.entity_id);
        writer.write_i8(self.yaw);
        writer.write_i8(self.pitch);
        writer.write_bool(self.on_ground);
        Ok(writer.into_inner())
    }
}

/// Clientbound absolute entity teleport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntityTeleport {
    pub entity_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: i8,
    pub pitch: i8,
    pub on_ground: bool,
}

impl EntityTeleport {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let packet = Self {
            entity_id: reader.read_varint()?,
            x: reader.read_f64()?,
            y: reader.read_f64()?,
            z: reader.read_f64()?,
            yaw: reader.read_u8()? as i8,
            pitch: reader.read_u8()? as i8,
            on_ground: reader.read_bool()?,
        };
        reader.finish()?;
        if packet.entity_id < 0 {
            return Err(Error::protocol("negative entity id"));
        }
        Ok(packet)
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::ENTITY_TELEPORT);
        writer.write_varint(self.entity_id);
        writer.write_f64(self.x);
        writer.write_f64(self.y);
        writer.write_f64(self.z);
        writer.write_i8(self.yaw);
        writer.write_i8(self.pitch);
        writer.write_bool(self.on_ground);
        Ok(writer.into_inner())
    }
}

/// Clientbound `RemoveEntities`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityDestroy {
    pub entity_ids: Vec<i32>,
}

impl EntityDestroy {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let count = read_count(&mut reader, "entity id", 4096)?;
        let mut entity_ids = Vec::with_capacity(count);
        for _ in 0..count {
            let id = reader.read_varint()?;
            if id < 0 {
                return Err(Error::protocol("negative entity id"));
            }
            entity_ids.push(id);
        }
        reader.finish()?;
        Ok(Self { entity_ids })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::ENTITY_DESTROY);
        writer.write_varint(
            i32::try_from(self.entity_ids.len())
                .map_err(|_| Error::protocol("too many entity ids"))?,
        );
        for id in &self.entity_ids {
            writer.write_varint(*id);
        }
        Ok(writer.into_inner())
    }
}

/// Clientbound chunk data with light.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkData {
    pub x: i32,
    pub z: i32,
    /// Anonymous NBT heightmaps, kept raw.
    pub heightmaps: Vec<u8>,
    /// The section buffer decoded by [`ChunkColumn::decode`].
    pub data: Vec<u8>,
    pub block_entities: Vec<BlockEntity>,
    pub light: LightData,
}

impl ChunkData {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let x = reader.read_i32()?;
        let z = reader.read_i32()?;
        let heightmaps = reader.read_anonymous_nbt_raw(MAX_NBT)?.to_vec();
        let data = reader.read_byte_array(MAX_CHUNK_DATA)?.to_vec();
        let entity_count = read_count(&mut reader, "block entity", 65_536)?;
        let mut block_entities = Vec::with_capacity(entity_count);
        for _ in 0..entity_count {
            block_entities.push(BlockEntity::decode(&mut reader)?);
        }
        let light = LightData::decode(&mut reader)?;
        reader.finish()?;
        Ok(Self {
            x,
            z,
            heightmaps,
            data,
            block_entities,
            light,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::CHUNK_DATA);
        writer.write_i32(self.x);
        writer.write_i32(self.z);
        writer.write_bytes(&self.heightmaps);
        writer.write_byte_array(&self.data, MAX_CHUNK_DATA)?;
        writer.write_varint(
            i32::try_from(self.block_entities.len())
                .map_err(|_| Error::protocol("too many block entities"))?,
        );
        for entity in &self.block_entities {
            entity.encode(&mut writer)?;
        }
        self.light.encode(&mut writer)?;
        Ok(writer.into_inner())
    }
}

/// Clientbound light-only update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateLight {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub light: LightData,
}

impl UpdateLight {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let chunk_x = reader.read_varint()?;
        let chunk_z = reader.read_varint()?;
        let light = LightData::decode(&mut reader)?;
        reader.finish()?;
        Ok(Self {
            chunk_x,
            chunk_z,
            light,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::UPDATE_LIGHT);
        writer.write_varint(self.chunk_x);
        writer.write_varint(self.chunk_z);
        self.light.encode(&mut writer)?;
        Ok(writer.into_inner())
    }
}

/// Clientbound `ChunkBatchStart`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChunkBatchStart;

impl ChunkBatchStart {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        Reader::new(payload).finish()?;
        Ok(Self)
    }

    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::CHUNK_BATCH_START);
        writer.into_inner()
    }
}

/// Clientbound `ChunkBatchFinished`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkBatchFinished {
    pub batch_size: i32,
}

impl ChunkBatchFinished {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let batch_size = reader.read_varint()?;
        reader.finish()?;
        if batch_size < 0 {
            return Err(Error::protocol("negative chunk batch size"));
        }
        Ok(Self { batch_size })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::CHUNK_BATCH_FINISHED);
        writer.write_varint(self.batch_size);
        Ok(writer.into_inner())
    }
}

/// Clientbound `SystemChatMessage`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemChat {
    /// Anonymous NBT text component, kept raw.
    pub content: Vec<u8>,
    pub action_bar: bool,
}

impl SystemChat {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let content = reader.read_anonymous_nbt_raw(MAX_NBT)?.to_vec();
        let action_bar = reader.read_bool()?;
        reader.finish()?;
        Ok(Self {
            content,
            action_bar,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::SYSTEM_CHAT);
        writer.write_bytes(&self.content);
        writer.write_bool(self.action_bar);
        Ok(writer.into_inner())
    }
}

/// Clientbound `Ping`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ping {
    pub id: i32,
}

impl Ping {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let id = reader.read_i32()?;
        reader.finish()?;
        Ok(Self { id })
    }

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::PING);
        writer.write_i32(self.id);
        writer.into_inner()
    }
}

/// Serverbound `Pong`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pong {
    pub id: i32,
}

impl Pong {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::serverbound::PONG);
        writer.write_i32(self.id);
        writer.into_inner()
    }
}

/// Serverbound `AcceptTeleportation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TeleportConfirm {
    pub teleport_id: i32,
}

impl TeleportConfirm {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::serverbound::TELEPORT_CONFIRM);
        writer.write_varint(self.teleport_id);
        writer.into_inner()
    }
}

/// Serverbound `SetPlayerPosition`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerPosition {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub on_ground: bool,
}

impl PlayerPosition {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::serverbound::POSITION);
        writer.write_f64(self.x);
        writer.write_f64(self.y);
        writer.write_f64(self.z);
        writer.write_bool(self.on_ground);
        writer.into_inner()
    }
}

/// Serverbound `SetPlayerPositionAndRotation`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerPositionLook {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

impl PlayerPositionLook {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::serverbound::POSITION_LOOK);
        writer.write_f64(self.x);
        writer.write_f64(self.y);
        writer.write_f64(self.z);
        writer.write_f32(self.yaw);
        writer.write_f32(self.pitch);
        writer.write_bool(self.on_ground);
        writer.into_inner()
    }
}

/// Serverbound `SetPlayerRotation`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerLook {
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

impl PlayerLook {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::serverbound::LOOK);
        writer.write_f32(self.yaw);
        writer.write_f32(self.pitch);
        writer.write_bool(self.on_ground);
        writer.into_inner()
    }
}

/// Serverbound `SetPlayerMovementFlags`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerFlying {
    pub on_ground: bool,
}

impl PlayerFlying {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::serverbound::FLYING);
        writer.write_bool(self.on_ground);
        writer.into_inner()
    }
}

/// Serverbound `ClientStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientCommand {
    /// `0` respawn, `1` request statistics.
    pub action: i32,
}

impl ClientCommand {
    pub const PERFORM_RESPAWN: i32 = 0;
    pub const REQUEST_STATS: i32 = 1;

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::serverbound::CLIENT_COMMAND);
        writer.write_varint(self.action);
        writer.into_inner()
    }
}

/// Serverbound `ChunkBatchReceived`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChunkBatchReceived {
    pub chunks_per_tick: f32,
}

impl ChunkBatchReceived {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::serverbound::CHUNK_BATCH_RECEIVED);
        writer.write_f32(self.chunks_per_tick);
        writer.into_inner()
    }
}

/// Serverbound `AcknowledgeConfiguration`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConfigurationAcknowledged;

impl ConfigurationAcknowledged {
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::serverbound::CONFIGURATION_ACKNOWLEDGED);
        writer.into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nbt_compound() -> Vec<u8> {
        // Anonymous TAG_Compound (no root name) with TAG_String "text" = "hi".
        let mut bytes = vec![10];
        bytes.push(8);
        bytes.extend_from_slice(&[0, 4]);
        bytes.extend_from_slice(b"text");
        bytes.extend_from_slice(&[0, 2]);
        bytes.extend_from_slice(b"hi");
        bytes.push(0);
        bytes
    }

    #[test]
    fn packed_position_round_trips_with_negative_values() {
        for pos in [
            BlockPos::new(0, 0, 0),
            BlockPos::new(1, 2, 3),
            BlockPos::new(-1, -64, -30_000_000),
            BlockPos::new(33_554_431, 2047, 33_554_431),
            BlockPos::new(-33_554_432, -2048, -33_554_432),
        ] {
            assert_eq!(BlockPos::from_packed(pos.to_packed()), pos);
        }
    }

    #[test]
    fn bitset_bits_are_little_endian_within_words() {
        let bits = BitSet::from_words(vec![0b1010, 1 << 3]);
        assert!(!bits.get(0));
        assert!(bits.get(1));
        assert!(!bits.get(2));
        assert!(bits.get(3));
        assert!(bits.get(64 + 3));
        assert!(!bits.get(64 + 4));
        assert!(!bits.get(1000));
    }

    #[test]
    fn nbt_skipper_handles_compounds_lists_and_arrays() {
        let bytes = nbt_compound();
        let mut reader = Reader::new(&bytes);
        let raw = reader.read_anonymous_nbt_raw(1024).unwrap();
        assert_eq!(raw, bytes.as_slice());
        assert!(reader.is_empty());

        // TAG_Compound { "l": TAG_List<TAG_Int>[1,2], "a": TAG_LongArray[7] }
        let mut bytes = vec![10];
        bytes.push(9);
        bytes.extend_from_slice(&[0, 1]);
        bytes.push(b'l');
        bytes.push(3);
        bytes.extend_from_slice(&2i32.to_be_bytes());
        bytes.extend_from_slice(&1i32.to_be_bytes());
        bytes.extend_from_slice(&2i32.to_be_bytes());
        bytes.push(12);
        bytes.extend_from_slice(&[0, 1]);
        bytes.push(b'a');
        bytes.extend_from_slice(&1i32.to_be_bytes());
        bytes.extend_from_slice(&7i64.to_be_bytes());
        bytes.push(0);
        let mut reader = Reader::new(&bytes);
        assert_eq!(
            reader.read_anonymous_nbt_raw(1024).unwrap(),
            bytes.as_slice()
        );
        assert!(reader.is_empty());
    }

    #[test]
    fn nbt_skipper_rejects_truncation_and_bad_lengths() {
        // TAG_Compound, child TAG_String whose name claims 4 bytes but has 1.
        let mut reader = Reader::new(&[10, 8, 0, 4, b't']);
        assert!(reader.read_anonymous_nbt_raw(1024).is_err());

        let negative = [7u8, 0xFF, 0xFF, 0xFF, 0xFF];
        assert!(Reader::new(&negative).read_anonymous_nbt_raw(1024).is_err());

        let mut reader = Reader::new(&[0]);
        assert!(reader.read_anonymous_nbt_raw(1024).is_err());
    }

    #[test]
    fn palette_single_indirect_and_direct_decode() {
        // Single: bits 0, value 42, zero-length long array.
        let mut reader = Reader::new(&[0, 42, 0]);
        let container = PalettedContainer::decode(&mut reader, 4096, 8, 15).unwrap();
        assert_eq!(container, PalettedContainer::Single(42));
        assert_eq!(container.get(0), Some(42));
        assert_eq!(container.get(4095), Some(42));

        // Indirect 4 bits, palette [5, 9], 256 words.
        let mut bytes = vec![4, 2, 5, 9];
        bytes.push(128); // VarInt 256 = 0x80 0x02
        bytes.push(2);
        let mut words = vec![0u8; 256 * 8];
        // Words are big-endian on the wire, so entry 0 is the last byte of the
        // first word. Set it to palette index 1; every other entry is 0.
        words[7] = 0x01;
        bytes.extend_from_slice(&words);
        let mut reader = Reader::new(&bytes);
        let container = PalettedContainer::decode(&mut reader, 4096, 8, 15).unwrap();
        assert_eq!(container.get(0), Some(9));
        assert_eq!(container.get(1), Some(5));
        assert!(reader.is_empty());

        // Direct 9 bits: floor(64/9) = 7 entries per word, ceil(4096/7) = 586
        // words. First entry value 7.
        let mut bytes = vec![9];
        bytes.push(0xCA);
        bytes.push(0x04); // VarInt 586
        let mut words = vec![0u8; 586 * 8];
        // Entry 0 occupies the least-significant bits of the first word.
        words[7] = 7;
        bytes.extend_from_slice(&words);
        let mut reader = Reader::new(&bytes);
        let container = PalettedContainer::decode(&mut reader, 4096, 8, 15).unwrap();
        assert_eq!(container.get(0), Some(7));
    }

    #[test]
    fn palette_rejects_wrong_word_count_and_bit_width() {
        assert!(PalettedContainer::decode(&mut Reader::new(&[16]), 4096, 8, 15).is_err());
        assert!(
            PalettedContainer::decode(&mut Reader::new(&[4, 1, 1, 1, 0, 0]), 4096, 8, 15).is_err()
        );
        assert!(PalettedContainer::decode(&mut Reader::new(&[0, 0]), 4096, 8, 15).is_err());
        assert!(PalettedContainer::decode(&mut Reader::new(&[0, 0, 1]), 4096, 8, 15).is_err());
        assert!(PalettedContainer::decode(&mut Reader::new(&[0, 0, 0]), 4096, 8, 15).is_ok());
    }

    #[test]
    fn chunk_column_resolves_blocks_and_biomes() {
        let mut bytes = Vec::new();
        let mut writer = Writer::new();
        writer.write_i16(2);
        PalettedContainer::Single(9)
            .encode(&mut writer, 4096)
            .unwrap();
        PalettedContainer::Single(3)
            .encode(&mut writer, 64)
            .unwrap();
        writer.write_i16(0);
        PalettedContainer::Single(0)
            .encode(&mut writer, 4096)
            .unwrap();
        PalettedContainer::Single(1)
            .encode(&mut writer, 64)
            .unwrap();
        bytes.extend_from_slice(writer.as_slice());

        let column = ChunkColumn::decode(2, -3, -64, &bytes, 2).unwrap();
        assert_eq!(column.block_state(0, -64, 0), Some(9));
        assert_eq!(column.block_state(15, -49, 15), Some(9));
        assert_eq!(column.block_state(0, -48, 0), Some(0));
        assert_eq!(column.block_state(0, -80, 0), None);
        assert_eq!(column.biome(0, -64, 0), Some(3));
        assert_eq!(column.biome(15, -48, 15), Some(1));
        assert_eq!(column.non_air_blocks(), 2);
    }

    #[test]
    fn slot_rejects_components_instead_of_guessing() {
        // count 1, item 5, one added component, none removed.
        assert!(Slot::decode(&mut Reader::new(&[1, 5, 1, 0])).is_err());
        assert_eq!(Slot::decode(&mut Reader::new(&[0])).unwrap(), Slot::Empty);
        assert_eq!(
            Slot::decode(&mut Reader::new(&[2, 5, 0, 0])).unwrap(),
            Slot::Item {
                item_id: 5,
                count: 2
            }
        );
    }

    #[test]
    fn join_game_round_trips() {
        let packet = JoinGame {
            entity_id: 7,
            hardcore: false,
            world_names: vec!["minecraft:overworld".to_owned()],
            max_players: 20,
            view_distance: 10,
            simulation_distance: 8,
            reduced_debug_info: false,
            enable_respawn_screen: true,
            do_limited_crafting: false,
            spawn: SpawnInfo {
                dimension_type: 0,
                dimension_name: "minecraft:overworld".to_owned(),
                hashed_seed: -42,
                game_mode: 0,
                previous_game_mode: -1,
                is_debug: false,
                is_flat: true,
                death: Some(DeathLocation {
                    dimension_name: "minecraft:overworld".to_owned(),
                    position: BlockPos::new(1, 64, -2),
                }),
                portal_cooldown: 0,
            },
            enforces_secure_chat: true,
        };
        let encoded = packet.encode().unwrap();
        assert_eq!(encoded[0] as i32, ids::play::clientbound::LOGIN);
        assert_eq!(JoinGame::decode(&encoded[1..]).unwrap(), packet);
    }

    #[test]
    fn position_and_health_round_trip() {
        let position = SyncPlayerPosition {
            x: 1.5,
            y: 64.0,
            z: -3.25,
            yaw: 90.0,
            pitch: -10.0,
            flags: SyncPlayerPosition::RELATIVE_X | SyncPlayerPosition::RELATIVE_YAW,
            teleport_id: 5,
        };
        let encoded = position.encode().unwrap();
        assert_eq!(SyncPlayerPosition::decode(&encoded[1..]).unwrap(), position);

        let health = UpdateHealth {
            health: 20.0,
            food: 20,
            food_saturation: 5.0,
        };
        let encoded = health.encode().unwrap();
        assert_eq!(UpdateHealth::decode(&encoded[1..]).unwrap(), health);
        assert!(UpdateHealth::decode(&[0, 0, 0, 0, 0, 0, 0, 0, 21, 0, 0, 0, 0]).is_err());
    }

    #[test]
    fn chunk_data_round_trips_with_light_and_block_entities() {
        let packet = ChunkData {
            x: -1,
            z: 2,
            heightmaps: nbt_compound(),
            data: vec![0, 0, 0, 42],
            block_entities: vec![BlockEntity {
                packed_xz: 0x21,
                y: 70,
                kind: 1,
                nbt: Some(nbt_compound()),
            }],
            light: LightData {
                sky_light_mask: BitSet::from_words(vec![0b11]),
                block_light_mask: BitSet::default(),
                empty_sky_light_mask: BitSet::default(),
                empty_block_light_mask: BitSet::from_words(vec![0b100]),
                sky_light: vec![vec![0xFF; 2048], vec![0x00; 2048]],
                block_light: vec![],
            },
        };
        let encoded = packet.encode().unwrap();
        assert_eq!(ChunkData::decode(&encoded[1..]).unwrap(), packet);
    }

    #[test]
    fn unload_chunk_writes_z_then_x() {
        let packet = UnloadChunk { x: 3, z: -4 };
        let encoded = packet.encode().unwrap();
        assert_eq!(&encoded[1..5], &(-4i32).to_be_bytes());
        assert_eq!(&encoded[5..9], &3i32.to_be_bytes());
        assert_eq!(UnloadChunk::decode(&encoded[1..]).unwrap(), packet);
    }

    /// A captured 1.21.1 `SectionBlocksUpdate`: three setblocks (stone,
    /// gold, glass) at y=-50 inside section (3, -4, -4).
    #[test]
    fn section_blocks_update_decodes_captured_packet() {
        let payload = [
            0x00, 0x00, 0x0f, 0xff, 0xff, 0xcf, 0xff, 0xfc, // section (3, -4, -4)
            0x03, // count
            0xee, 0x25, // state 1 at (50, -50, -50)
            0xee, 0xed, 0x8a, 0x04, // state 2091 at (54, -50, -50)
            0xee, 0xe9, 0x81, 0x01, // state 519 at (52, -50, -50)
        ];
        let packet = SectionBlocksUpdate::decode(&payload).unwrap();
        let expected = [(50, -50, -50, 1), (54, -50, -50, 2091), (52, -50, -50, 519)];
        assert_eq!(packet.changes.len(), 3);
        for (change, (x, y, z, state)) in packet.changes.iter().zip(expected) {
            assert_eq!(
                (
                    change.position.x,
                    change.position.y,
                    change.position.z,
                    change.state_id
                ),
                (x, y, z, state)
            );
        }
        // Round-trip through the encoder.
        assert_eq!(
            SectionBlocksUpdate::decode(&packet.encode().unwrap()[1..]).unwrap(),
            packet
        );
    }

    /// The second captured packet: glass and stone at z=-60, plus a record
    /// whose relative z and y differ, proving the 4/4/4-bit field order.
    #[test]
    fn section_blocks_update_decodes_second_captured_packet() {
        let payload = [
            0x00, 0x00, 0x0f, 0xff, 0xff, 0xcf, 0xff, 0xfc, 0x02, 0xce, 0xfc, 0x81,
            0x01, // state 519 at (62, -50, -60)
            0xce, 0x38, // state 1 at (60, -50, -60)
        ];
        let packet = SectionBlocksUpdate::decode(&payload).unwrap();
        assert_eq!(
            packet
                .changes
                .iter()
                .map(|c| (c.position.x, c.position.y, c.position.z, c.state_id))
                .collect::<Vec<_>>(),
            vec![(62, -50, -60, 519), (60, -50, -60, 1)]
        );
    }

    #[test]
    fn section_blocks_update_rejects_bad_records() {
        let mut payload = vec![0u8; 8]; // section (0, 0, 0)
        payload.push(1); // count
        payload.push(0x80); // varint without a terminator
        assert!(SectionBlocksUpdate::decode(&payload).is_err());

        // Count above one section's capacity.
        let mut payload = vec![0u8; 8];
        payload.extend_from_slice(&[0x80, 0x20]); // varint 4097
        assert!(SectionBlocksUpdate::decode(&payload).is_err());

        // Trailing bytes after the declared records.
        let mut payload = vec![0u8; 8];
        payload.extend_from_slice(&[0x00, 0x01, 0x00]);
        assert!(SectionBlocksUpdate::decode(&payload).is_err());
    }

    #[test]
    fn section_blocks_update_encode_rejects_mixed_sections() {
        let mixed = SectionBlocksUpdate {
            changes: vec![
                BlockChange {
                    position: BlockPos::new(1, 0, 0),
                    state_id: 1,
                },
                BlockChange {
                    position: BlockPos::new(16, 0, 0),
                    state_id: 1,
                },
            ],
        };
        assert!(mixed.encode().is_err());
        assert!(SectionBlocksUpdate {
            changes: Vec::new()
        }
        .encode()
        .is_err());
    }
}
