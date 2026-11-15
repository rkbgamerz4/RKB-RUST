//! Anvil region files (`region/r.X.Z.mca`).
//!
//! A region file is a fixed 8 KiB header followed by 4 KiB sectors. The header
//! has one four-byte location per chunk (a 24-bit first-sector index and an
//! eight-bit sector count) and one four-byte timestamp. Each allocated chunk
//! starts with a big-endian length that includes the following compression
//! byte. The compressed payload is NBT.
//!
//! Compression 1 (gzip), 2 (zlib), and 3 (uncompressed) are read and written.
//! Compression 4 (LZ4), used by newer files, is reported as unsupported rather
//! than silently guessed.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use flate2::read::{GzDecoder, ZlibDecoder};
use flate2::write::{GzEncoder, ZlibEncoder};
use flate2::Compression as FlateCompression;

use crate::nbt::{Nbt, NbtError, Tag};

/// Size of one region sector.
pub const SECTOR_BYTES: usize = 4096;
/// Locations plus timestamps.
pub const HEADER_BYTES: usize = 2 * SECTOR_BYTES;
/// Chunks along each axis in one region file.
pub const REGION_CHUNKS: u8 = 32;
/// Upper bound on one decompressed chunk, protecting against compression
/// bombs. Vanilla chunks are orders of magnitude smaller.
pub const MAX_CHUNK_BYTES: usize = 64 << 20;

/// Region-file compression ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    Gzip,
    Zlib,
    None,
    /// LZ4, present in some newer region files. Recognized but not decoded.
    Lz4,
    Unknown(u8),
}

impl Compression {
    pub const fn from_id(id: u8) -> Self {
        match id {
            1 => Self::Gzip,
            2 => Self::Zlib,
            3 => Self::None,
            4 => Self::Lz4,
            other => Self::Unknown(other),
        }
    }

    pub const fn id(self) -> u8 {
        match self {
            Self::Gzip => 1,
            Self::Zlib => 2,
            Self::None => 3,
            Self::Lz4 => 4,
            Self::Unknown(id) => id,
        }
    }

    pub const fn is_supported(self) -> bool {
        matches!(self, Self::Gzip | Self::Zlib | Self::None)
    }
}

impl fmt::Display for Compression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gzip => f.write_str("gzip"),
            Self::Zlib => f.write_str("zlib"),
            Self::None => f.write_str("none"),
            Self::Lz4 => f.write_str("lz4 (unsupported)"),
            Self::Unknown(id) => write!(f, "unknown compression {id}"),
        }
    }
}

/// The header entry for one chunk slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkLocation {
    pub offset_sectors: u32,
    pub sector_count: u8,
}

impl ChunkLocation {
    pub const fn is_present(self) -> bool {
        self.offset_sectors != 0 && self.sector_count != 0
    }
}

/// A decompressed chunk payload, still in NBT bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawChunk {
    pub compression: Compression,
    pub data: Vec<u8>,
}

/// Failures while reading or writing a region file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegionError {
    Io {
        context: String,
        message: String,
    },
    ShortFile {
        len: u64,
        needed: usize,
    },
    ChunkOutOfRange {
        x: u8,
        z: u8,
    },
    InvalidLocation {
        x: u8,
        z: u8,
        offset: u32,
        sectors: u8,
    },
    OutOfBounds {
        offset: u64,
        len: u64,
        file_len: u64,
    },
    ZeroLength {
        x: u8,
        z: u8,
    },
    LengthMismatch {
        x: u8,
        z: u8,
        declared: u32,
        allocated: u64,
    },
    UnsupportedCompression {
        id: u8,
    },
    ChunkTooLarge {
        len: usize,
    },
    RegionTooLarge,
    Nbt(NbtError),
}

impl RegionError {
    fn io(context: impl Into<String>, error: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            message: error.to_string(),
        }
    }
}

impl fmt::Display for RegionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { context, message } => write!(f, "{context}: {message}"),
            Self::ShortFile { len, needed } => {
                write!(
                    f,
                    "region file is {len} bytes; at least {needed} are required"
                )
            }
            Self::ChunkOutOfRange { x, z } => {
                write!(f, "chunk slot ({x}, {z}) is outside a region (0..32)")
            }
            Self::InvalidLocation {
                x,
                z,
                offset,
                sectors,
            } => write!(
                f,
                "chunk slot ({x}, {z}) has invalid location {offset} with {sectors} sectors"
            ),
            Self::OutOfBounds {
                offset,
                len,
                file_len,
            } => write!(
                f,
                "region bytes {offset}..{} are past the {file_len}-byte file",
                offset.saturating_add(*len)
            ),
            Self::ZeroLength { x, z } => write!(f, "chunk ({x}, {z}) declares a zero length"),
            Self::LengthMismatch {
                x,
                z,
                declared,
                allocated,
            } => write!(
                f,
                "chunk ({x}, {z}) declares {declared} bytes but only {allocated} are allocated"
            ),
            Self::UnsupportedCompression { id } => {
                write!(f, "chunk compression {id} is not supported")
            }
            Self::ChunkTooLarge { len } => {
                write!(f, "chunk is too large ({len} bytes)")
            }
            Self::RegionTooLarge => f.write_str("region file needs more than 2^24 sectors"),
            Self::Nbt(error) => write!(f, "chunk NBT: {error}"),
        }
    }
}

impl std::error::Error for RegionError {}

impl From<NbtError> for RegionError {
    fn from(error: NbtError) -> Self {
        Self::Nbt(error)
    }
}

enum Source {
    Memory(Arc<[u8]>),
    File { file: File, len: u64 },
}

/// An opened `.mca` file. File-backed regions seek and read one chunk at a
/// time; in-memory regions are used by tests and by callers that already hold
/// the bytes.
pub struct RegionFile {
    path: PathBuf,
    source: Source,
    header: Vec<u8>,
}

impl fmt::Debug for RegionFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegionFile")
            .field("path", &self.path)
            .field("len", &self.len())
            .finish_non_exhaustive()
    }
}

impl RegionFile {
    /// Opens a region file and reads its 8 KiB header.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, RegionError> {
        let path = path.as_ref().to_path_buf();
        let mut file =
            File::open(&path).map_err(|error| RegionError::io(format!("open {path:?}"), error))?;
        let len = file
            .metadata()
            .map_err(|error| RegionError::io(format!("stat {path:?}"), error))?
            .len();
        if len < HEADER_BYTES as u64 {
            return Err(RegionError::ShortFile {
                len,
                needed: HEADER_BYTES,
            });
        }
        let mut header = vec![0u8; HEADER_BYTES];
        file.read_exact(&mut header)
            .map_err(|error| RegionError::io(format!("read header {path:?}"), error))?;
        Ok(Self {
            path,
            source: Source::File { file, len },
            header,
        })
    }

    /// Uses bytes already in memory. The path is only used in diagnostics.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, RegionError> {
        if bytes.len() < HEADER_BYTES {
            return Err(RegionError::ShortFile {
                len: bytes.len() as u64,
                needed: HEADER_BYTES,
            });
        }
        let header = bytes[..HEADER_BYTES].to_vec();
        Ok(Self {
            path: PathBuf::from("<memory>"),
            source: Source::Memory(bytes.into()),
            header,
        })
    }

    /// Path printed in diagnostics.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Current file length in bytes.
    pub fn len(&self) -> u64 {
        match &self.source {
            Source::Memory(bytes) => bytes.len() as u64,
            Source::File { len, .. } => *len,
        }
    }

    /// Whether the region has no bytes (it never does; headers are required).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn read_at(&mut self, offset: u64, len: usize) -> Result<Vec<u8>, RegionError> {
        match &mut self.source {
            Source::Memory(bytes) => {
                let file_len = bytes.len() as u64;
                let start = usize::try_from(offset).map_err(|_| RegionError::OutOfBounds {
                    offset,
                    len: len as u64,
                    file_len,
                })?;
                let end = start.checked_add(len).ok_or(RegionError::OutOfBounds {
                    offset,
                    len: len as u64,
                    file_len,
                })?;
                let slice = bytes.get(start..end).ok_or(RegionError::OutOfBounds {
                    offset,
                    len: len as u64,
                    file_len,
                })?;
                Ok(slice.to_vec())
            }
            Source::File {
                file,
                len: file_len,
            } => {
                let end = offset
                    .checked_add(len as u64)
                    .ok_or(RegionError::OutOfBounds {
                        offset,
                        len: len as u64,
                        file_len: *file_len,
                    })?;
                if end > *file_len {
                    return Err(RegionError::OutOfBounds {
                        offset,
                        len: len as u64,
                        file_len: *file_len,
                    });
                }
                file.seek(SeekFrom::Start(offset))
                    .map_err(|error| RegionError::io("seek region", error))?;
                let mut buffer = vec![0u8; len];
                file.read_exact(&mut buffer)
                    .map_err(|error| RegionError::io("read region", error))?;
                Ok(buffer)
            }
        }
    }

    fn slot_index(x: u8, z: u8) -> Result<usize, RegionError> {
        if x >= REGION_CHUNKS || z >= REGION_CHUNKS {
            return Err(RegionError::ChunkOutOfRange { x, z });
        }
        Ok(usize::from(x) + usize::from(z) * usize::from(REGION_CHUNKS))
    }

    /// Reads one slot's location, validating that its sectors fit the file.
    pub fn location(&self, x: u8, z: u8) -> Result<ChunkLocation, RegionError> {
        let index = Self::slot_index(x, z)? * 4;
        let bytes = &self.header[index..index + 4];
        let offset_sectors =
            (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
        let sector_count = bytes[3];
        let location = ChunkLocation {
            offset_sectors,
            sector_count,
        };
        if offset_sectors == 0 && sector_count == 0 {
            return Ok(location);
        }
        if offset_sectors < 2 || sector_count == 0 {
            return Err(RegionError::InvalidLocation {
                x,
                z,
                offset: offset_sectors,
                sectors: sector_count,
            });
        }
        let start = u64::from(offset_sectors) * SECTOR_BYTES as u64;
        let size = u64::from(sector_count) * SECTOR_BYTES as u64;
        let end = start.checked_add(size).ok_or(RegionError::OutOfBounds {
            offset: start,
            len: size,
            file_len: self.len(),
        })?;
        if end > self.len() {
            return Err(RegionError::OutOfBounds {
                offset: start,
                len: size,
                file_len: self.len(),
            });
        }
        Ok(location)
    }

    /// Number of slots with a non-zero location field.
    pub fn chunk_count(&self) -> usize {
        (0..1024)
            .filter(|slot| {
                let index = slot * 4;
                self.header[index] != 0
                    || self.header[index + 1] != 0
                    || self.header[index + 2] != 0
            })
            .filter(|slot| self.header[slot * 4 + 3] != 0)
            .count()
    }

    /// Raw timestamp of a slot, as stored by the game.
    pub fn timestamp(&self, x: u8, z: u8) -> Result<u32, RegionError> {
        let index = HEADER_BYTES / 2 + Self::slot_index(x, z)? * 4;
        let bytes = &self.header[index..index + 4];
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Reads and decompresses one chunk. `Ok(None)` means the slot is empty.
    pub fn read_chunk(&mut self, x: u8, z: u8) -> Result<Option<RawChunk>, RegionError> {
        let location = self.location(x, z)?;
        if !location.is_present() {
            return Ok(None);
        }
        let start = u64::from(location.offset_sectors) * SECTOR_BYTES as u64;
        let allocated = u64::from(location.sector_count) * SECTOR_BYTES as u64;
        let prefix = self.read_at(start, 5)?;
        let length = u32::from_be_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]);
        if length == 0 {
            return Err(RegionError::ZeroLength { x, z });
        }
        if 4 + u64::from(length) > allocated {
            return Err(RegionError::LengthMismatch {
                x,
                z,
                declared: length,
                allocated,
            });
        }
        let compression = Compression::from_id(prefix[4]);
        if !compression.is_supported() {
            return Err(RegionError::UnsupportedCompression {
                id: compression.id(),
            });
        }
        let data_len = length as usize - 1;
        if data_len > MAX_CHUNK_BYTES {
            return Err(RegionError::ChunkTooLarge { len: data_len });
        }
        let compressed = self.read_at(start + 5, data_len)?;
        let data = decompress(compression, &compressed)?;
        Ok(Some(RawChunk { compression, data }))
    }

    /// Reads, decompresses, and parses one chunk's NBT.
    pub fn read_chunk_nbt(
        &mut self,
        x: u8,
        z: u8,
    ) -> Result<Option<(Compression, Tag)>, RegionError> {
        let Some(raw) = self.read_chunk(x, z)? else {
            return Ok(None);
        };
        let tag = Nbt::read(&raw.data)?;
        Ok(Some((raw.compression, tag)))
    }
}

fn decompress(compression: Compression, data: &[u8]) -> Result<Vec<u8>, RegionError> {
    match compression {
        Compression::Gzip => inflate(GzDecoder::new(data)),
        Compression::Zlib => inflate(ZlibDecoder::new(data)),
        Compression::None => {
            if data.len() > MAX_CHUNK_BYTES {
                Err(RegionError::ChunkTooLarge { len: data.len() })
            } else {
                Ok(data.to_vec())
            }
        }
        Compression::Lz4 | Compression::Unknown(_) => Err(RegionError::UnsupportedCompression {
            id: compression.id(),
        }),
    }
}

fn inflate(mut reader: impl Read) -> Result<Vec<u8>, RegionError> {
    let mut out = Vec::new();
    reader
        .by_ref()
        .take(MAX_CHUNK_BYTES as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|error| RegionError::io("decompress chunk", error))?;
    if out.len() > MAX_CHUNK_BYTES {
        return Err(RegionError::ChunkTooLarge { len: out.len() });
    }
    Ok(out)
}

fn compress(compression: Compression, nbt: &[u8]) -> Result<Vec<u8>, RegionError> {
    if !compression.is_supported() {
        return Err(RegionError::UnsupportedCompression {
            id: compression.id(),
        });
    }
    if nbt.len() > MAX_CHUNK_BYTES {
        return Err(RegionError::ChunkTooLarge { len: nbt.len() });
    }
    match compression {
        Compression::None => Ok(nbt.to_vec()),
        Compression::Gzip => {
            let mut encoder = GzEncoder::new(Vec::new(), FlateCompression::default());
            encoder
                .write_all(nbt)
                .map_err(|error| RegionError::io("compress chunk with gzip", error))?;
            encoder
                .finish()
                .map_err(|error| RegionError::io("finish gzip chunk", error))
        }
        Compression::Zlib => {
            let mut encoder = ZlibEncoder::new(Vec::new(), FlateCompression::default());
            encoder
                .write_all(nbt)
                .map_err(|error| RegionError::io("compress chunk with zlib", error))?;
            encoder
                .finish()
                .map_err(|error| RegionError::io("finish zlib chunk", error))
        }
        Compression::Lz4 | Compression::Unknown(_) => Err(RegionError::UnsupportedCompression {
            id: compression.id(),
        }),
    }
}

/// Builds a region file from uncompressed chunk NBT. Used by this crate's
/// tests, and available for tools that export synthetic worlds.
#[derive(Clone, Debug, Default)]
pub struct RegionWriter {
    chunks: BTreeMap<u16, (Compression, Vec<u8>)>,
}

impl RegionWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces one chunk slot. Coordinates are local to the region.
    pub fn add_chunk(
        &mut self,
        x: u8,
        z: u8,
        compression: Compression,
        nbt: Vec<u8>,
    ) -> Result<(), RegionError> {
        let slot = RegionFile::slot_index(x, z)?;
        if nbt.is_empty() {
            return Err(RegionError::ZeroLength { x, z });
        }
        self.chunks.insert(slot as u16, (compression, nbt));
        Ok(())
    }

    /// Serializes and adds one chunk from its NBT tree.
    pub fn add_chunk_nbt(
        &mut self,
        x: u8,
        z: u8,
        compression: Compression,
        tag: &Tag,
    ) -> Result<(), RegionError> {
        self.add_chunk(x, z, compression, Nbt::write(tag)?)
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Serializes the region. Empty slots stay zero in the header.
    pub fn to_bytes(&self) -> Result<Vec<u8>, RegionError> {
        let mut header = vec![0u8; HEADER_BYTES];
        let mut body = Vec::new();
        let mut next_sector: u32 = 2;
        for (slot, (compression, nbt)) in &self.chunks {
            let compressed = compress(*compression, nbt)?;
            let payload_len = 1 + compressed.len();
            let total = 4 + payload_len;
            let sectors = total.div_ceil(SECTOR_BYTES);
            if sectors > u8::MAX as usize {
                return Err(RegionError::ChunkTooLarge { len: total });
            }
            if next_sector > 0x00FF_FFFF {
                return Err(RegionError::RegionTooLarge);
            }
            let index = *slot as usize * 4;
            header[index] = (next_sector >> 16) as u8;
            header[index + 1] = (next_sector >> 8) as u8;
            header[index + 2] = next_sector as u8;
            header[index + 3] = sectors as u8;

            body.extend_from_slice(&(payload_len as u32).to_be_bytes());
            body.push(compression.id());
            body.extend_from_slice(&compressed);
            body.resize(body.len() + sectors * SECTOR_BYTES - total, 0);
            next_sector += sectors as u32;
        }
        let mut bytes = header;
        bytes.extend_from_slice(&body);
        Ok(bytes)
    }

    /// Serializes the region and writes it to disk.
    pub fn write_to(&self, path: impl AsRef<Path>) -> Result<(), RegionError> {
        let path = path.as_ref();
        let bytes = self.to_bytes()?;
        fs::write(path, bytes).map_err(|error| RegionError::io(format!("write {path:?}"), error))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn chunk_tag(value: i32) -> Tag {
        let mut root = BTreeMap::new();
        root.insert("Value".to_string(), Tag::Int(value));
        Tag::Compound(root)
    }

    fn sample_region(compression: Compression) -> Vec<u8> {
        let mut writer = RegionWriter::new();
        writer
            .add_chunk_nbt(0, 0, compression, &chunk_tag(1))
            .expect("add first");
        writer
            .add_chunk_nbt(31, 31, compression, &chunk_tag(2))
            .expect("add second");
        writer.to_bytes().expect("serialize")
    }

    #[test]
    fn round_trips_supported_compressions() {
        for compression in [Compression::Gzip, Compression::Zlib, Compression::None] {
            let bytes = sample_region(compression);
            let mut region = RegionFile::from_bytes(bytes).expect("open");
            assert_eq!(region.chunk_count(), 2);
            let (kind, tag) = region
                .read_chunk_nbt(0, 0)
                .expect("read first")
                .expect("present");
            assert_eq!(kind, compression);
            assert_eq!(tag.get("Value").and_then(Tag::as_int), Some(1));
            let (_, tag) = region
                .read_chunk_nbt(31, 31)
                .expect("read second")
                .expect("present");
            assert_eq!(tag.get("Value").and_then(Tag::as_int), Some(2));
            assert!(region.read_chunk_nbt(1, 1).expect("read absent").is_none());
        }
    }

    #[test]
    fn writes_a_valid_header() {
        let bytes = sample_region(Compression::Zlib);
        let region = RegionFile::from_bytes(bytes).expect("open");
        let first = region.location(0, 0).expect("location");
        assert_eq!(first.offset_sectors, 2);
        assert!(first.sector_count >= 1);
        let second = region.location(31, 31).expect("location");
        assert!(second.offset_sectors >= first.offset_sectors);
        assert_eq!(region.timestamp(0, 0).expect("timestamp"), 0);
    }

    #[test]
    fn rejects_short_files_and_out_of_range_slots() {
        assert!(matches!(
            RegionFile::from_bytes(vec![0; 100]),
            Err(RegionError::ShortFile { .. })
        ));
        let region = RegionFile::from_bytes(vec![0; HEADER_BYTES]).expect("open");
        assert!(matches!(
            region.location(32, 0),
            Err(RegionError::ChunkOutOfRange { .. })
        ));
        assert!(!region.is_empty());
    }

    #[test]
    fn rejects_invalid_locations() {
        let mut bytes = vec![0u8; HEADER_BYTES];
        // Offset zero with a non-zero sector count.
        bytes[3] = 1;
        let region = RegionFile::from_bytes(bytes).expect("open");
        assert!(matches!(
            region.location(0, 0),
            Err(RegionError::InvalidLocation { .. })
        ));

        let mut bytes = vec![0u8; HEADER_BYTES];
        // A non-zero offset with zero sectors.
        bytes[2] = 2;
        let region = RegionFile::from_bytes(bytes).expect("open");
        assert!(matches!(
            region.location(0, 0),
            Err(RegionError::InvalidLocation { .. })
        ));

        let mut bytes = vec![0u8; HEADER_BYTES];
        bytes[2] = 2;
        bytes[3] = 1;
        let region = RegionFile::from_bytes(bytes).expect("open");
        assert!(matches!(
            region.location(0, 0),
            Err(RegionError::OutOfBounds { .. })
        ));
    }

    fn raw_region(length: u32, compression: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0u8; HEADER_BYTES];
        bytes[2] = 2;
        bytes[3] = 1;
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.push(compression);
        bytes.extend_from_slice(payload);
        bytes.resize(HEADER_BYTES + SECTOR_BYTES, 0);
        bytes
    }

    #[test]
    fn rejects_bad_lengths_and_compressions() {
        let mut region = RegionFile::from_bytes(raw_region(0, 2, &[])).expect("open");
        assert!(matches!(
            region.read_chunk(0, 0),
            Err(RegionError::ZeroLength { .. })
        ));

        let mut region = RegionFile::from_bytes(raw_region(5000, 2, &[])).expect("open");
        assert!(matches!(
            region.read_chunk(0, 0),
            Err(RegionError::LengthMismatch { .. })
        ));

        let mut region = RegionFile::from_bytes(raw_region(1, 4, &[])).expect("open");
        assert!(matches!(
            region.read_chunk(0, 0),
            Err(RegionError::UnsupportedCompression { id: 4 })
        ));

        let mut writer = RegionWriter::new();
        writer
            .add_chunk(0, 0, Compression::Lz4, vec![1, 2, 3])
            .expect("add");
        assert!(matches!(
            writer.to_bytes(),
            Err(RegionError::UnsupportedCompression { id: 4 })
        ));
    }

    #[test]
    fn rejects_corrupt_compressed_payloads() {
        let mut region = RegionFile::from_bytes(raw_region(6, 2, &[1, 2, 3, 4, 5])).expect("open");
        assert!(matches!(
            region.read_chunk(0, 0),
            Err(RegionError::Io { .. })
        ));
    }

    #[test]
    fn writer_rejects_empty_and_out_of_range_chunks() {
        let mut writer = RegionWriter::new();
        assert!(matches!(
            writer.add_chunk(0, 0, Compression::None, Vec::new()),
            Err(RegionError::ZeroLength { .. })
        ));
        assert!(matches!(
            writer.add_chunk(32, 0, Compression::None, vec![1]),
            Err(RegionError::ChunkOutOfRange { .. })
        ));
        assert_eq!(writer.chunk_count(), 0);
    }

    #[test]
    fn file_backed_region_reads_the_same_chunk() {
        let dir = std::env::temp_dir().join(format!(
            "rkb-mc-region-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("r.0.0.mca");
        let bytes = sample_region(Compression::Gzip);
        fs::write(&path, &bytes).expect("write region");

        let mut region = RegionFile::open(&path).expect("open");
        assert_eq!(region.path(), path);
        assert_eq!(region.len(), bytes.len() as u64);
        let (_, tag) = region
            .read_chunk_nbt(31, 31)
            .expect("read")
            .expect("present");
        assert_eq!(tag.get("Value").and_then(Tag::as_int), Some(2));

        let _ = fs::remove_dir_all(&dir);
    }
}
