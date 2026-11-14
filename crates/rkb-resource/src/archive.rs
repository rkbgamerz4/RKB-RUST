//! Deterministic `RKBPAK1` archive format.
//!
//! Layout (little endian):
//!
//! ```text
//! magic[8] = "RKBPAK1\0"
//! version  u32
//! count    u32
//! entries  count * { path_len u16, path bytes, offset u32, size u32, checksum u64 }
//! data     concatenated asset bytes (offsets are relative to this section)
//! ```

use std::collections::BTreeMap;

use crate::path::{AssetId, AssetPath, ResourceError};
use crate::source::AssetSource;

const MAGIC: &[u8; 8] = b"RKBPAK1\0";
const VERSION: u32 = 1;
const HEADER_SIZE: usize = 8 + 4 + 4;
const ENTRY_FIXED_SIZE: usize = 2 + 4 + 4 + 8;

/// Builds an archive from named byte slices.
#[derive(Clone, Debug, Default)]
pub struct PakWriter {
    entries: BTreeMap<String, Vec<u8>>,
}

impl PakWriter {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    pub fn add(&mut self, path: &str, bytes: impl Into<Vec<u8>>) -> Result<(), ResourceError> {
        let path = AssetPath::new(path)?;
        let key = path.as_str().to_owned();
        if self.entries.contains_key(&key) {
            return Err(ResourceError::Archive(format!("duplicate entry '{key}'")));
        }
        if key.len() > usize::from(u16::MAX) {
            return Err(ResourceError::Archive("entry path is too long".to_owned()));
        }
        self.entries.insert(key, bytes.into());
        Ok(())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Builds a complete archive byte buffer.
    pub fn finish(self) -> Result<Vec<u8>, ResourceError> {
        let count = u32::try_from(self.entries.len())
            .map_err(|_| ResourceError::Archive("too many entries".to_owned()))?;

        let header_size = self.entries.keys().try_fold(HEADER_SIZE, |size, path| {
            size.checked_add(ENTRY_FIXED_SIZE + path.len())
                .ok_or_else(|| ResourceError::Archive("archive header overflow".to_owned()))
        })?;
        let data_size = self.entries.values().try_fold(0_usize, |size, bytes| {
            size.checked_add(bytes.len())
                .ok_or_else(|| ResourceError::Archive("archive data overflow".to_owned()))
        })?;
        let mut output = Vec::with_capacity(header_size + data_size);

        output.extend_from_slice(MAGIC);
        output.extend_from_slice(&VERSION.to_le_bytes());
        output.extend_from_slice(&count.to_le_bytes());

        let mut offset = 0_u32;
        for (path, bytes) in &self.entries {
            let path_len = u16::try_from(path.len())
                .map_err(|_| ResourceError::Archive("entry path is too long".to_owned()))?;
            let size = u32::try_from(bytes.len())
                .map_err(|_| ResourceError::Archive("entry is larger than 4 GiB".to_owned()))?;
            output.extend_from_slice(&path_len.to_le_bytes());
            output.extend_from_slice(path.as_bytes());
            output.extend_from_slice(&offset.to_le_bytes());
            output.extend_from_slice(&size.to_le_bytes());
            output.extend_from_slice(&checksum(bytes).to_le_bytes());
            offset = offset
                .checked_add(size)
                .ok_or_else(|| ResourceError::Archive("archive data overflow".to_owned()))?;
        }

        debug_assert_eq!(output.len(), header_size);
        for bytes in self.entries.values() {
            output.extend_from_slice(bytes);
        }
        Ok(output)
    }

    /// Reads every asset from a source and packages it.
    pub fn from_source(source: &dyn AssetSource) -> Result<Vec<u8>, ResourceError> {
        let mut writer = Self::new();
        for path in source.list() {
            let bytes = source.read(&path)?;
            writer.add(path.as_str(), bytes)?;
        }
        writer.finish()
    }
}

#[derive(Clone, Copy, Debug)]
struct PakEntry {
    data_offset: usize,
    size: usize,
    checksum: u64,
}

/// Parsed, immutable archive implementing [`AssetSource`].
#[derive(Clone, Debug)]
pub struct PakArchive {
    data: Vec<u8>,
    data_start: usize,
    entries: BTreeMap<AssetId, (String, PakEntry)>,
}

impl PakArchive {
    /// Parses and validates an archive.
    pub fn parse(data: Vec<u8>) -> Result<Self, ResourceError> {
        let mut cursor = Cursor::new(&data);
        if cursor.take(MAGIC.len())? != MAGIC {
            return Err(ResourceError::Archive("bad magic".to_owned()));
        }
        let version = cursor.read_u32()?;
        if version != VERSION {
            return Err(ResourceError::Archive(format!(
                "unsupported version {version}"
            )));
        }
        let count = cursor.read_u32()?;
        let mut entries = BTreeMap::new();

        for _ in 0..count {
            let path_len = usize::from(cursor.read_u16()?);
            if path_len == 0 {
                return Err(ResourceError::Archive("empty entry path".to_owned()));
            }
            let path_bytes = cursor.take(path_len)?;
            let path = std::str::from_utf8(path_bytes)
                .map_err(|_| ResourceError::Archive("entry path is not UTF-8".to_owned()))?;
            let path = AssetPath::new(path)
                .map_err(|_| ResourceError::Archive(format!("invalid entry path '{path}'")))?;
            let id = path.id();
            if entries.contains_key(&id) {
                return Err(ResourceError::Archive(format!(
                    "duplicate entry '{}'",
                    path.as_str()
                )));
            }
            let data_offset = cursor.read_u32()? as usize;
            let size = cursor.read_u32()? as usize;
            let checksum = cursor.read_u64()?;
            entries.insert(
                id,
                (
                    path.as_str().to_owned(),
                    PakEntry {
                        data_offset,
                        size,
                        checksum,
                    },
                ),
            );
        }

        let data_start = cursor.position();
        for (path, entry) in entries.values() {
            let end = data_start
                .checked_add(entry.data_offset)
                .and_then(|start| start.checked_add(entry.size))
                .ok_or_else(|| ResourceError::Archive("entry range overflow".to_owned()))?;
            if end > data.len() {
                return Err(ResourceError::Archive(format!(
                    "entry '{path}' extends beyond archive data"
                )));
            }
        }

        Ok(Self {
            data,
            data_start,
            entries,
        })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn entry_bytes(&self, entry: PakEntry) -> &[u8] {
        let start = self.data_start + entry.data_offset;
        &self.data[start..start + entry.size]
    }
}

impl AssetSource for PakArchive {
    fn read(&self, path: &AssetPath) -> Result<Vec<u8>, ResourceError> {
        let (_, entry) = self
            .entries
            .get(&path.id())
            .ok_or_else(|| ResourceError::NotFound(path.to_string()))?;
        let bytes = self.entry_bytes(*entry);
        if checksum(bytes) != entry.checksum {
            return Err(ResourceError::ChecksumMismatch(path.to_string()));
        }
        Ok(bytes.to_vec())
    }

    fn exists(&self, path: &AssetPath) -> bool {
        self.entries.contains_key(&path.id())
    }

    fn list(&self) -> Vec<AssetPath> {
        self.entries
            .values()
            .filter_map(|(path, _)| AssetPath::new(path).ok())
            .collect()
    }
}

fn checksum(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

struct Cursor<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    const fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    const fn position(&self) -> usize {
        self.position
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], ResourceError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(|| ResourceError::Archive("offset overflow".to_owned()))?;
        if end > self.data.len() {
            return Err(ResourceError::Archive(
                "unexpected end of archive".to_owned(),
            ));
        }
        let slice = &self.data[self.position..end];
        self.position = end;
        Ok(slice)
    }

    fn read_u16(&mut self) -> Result<u16, ResourceError> {
        let bytes: [u8; 2] = self
            .take(2)?
            .try_into()
            .map_err(|_| ResourceError::Archive("short u16".to_owned()))?;
        Ok(u16::from_le_bytes(bytes))
    }

    fn read_u32(&mut self) -> Result<u32, ResourceError> {
        let bytes: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| ResourceError::Archive("short u32".to_owned()))?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn read_u64(&mut self) -> Result<u64, ResourceError> {
        let bytes: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| ResourceError::Archive("short u64".to_owned()))?;
        Ok(u64::from_le_bytes(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive() -> PakArchive {
        let mut writer = PakWriter::new();
        writer.add("assets/hello.txt", b"hello".to_vec()).unwrap();
        writer.add("assets/data.bin", vec![0, 1, 2, 255]).unwrap();
        PakArchive::parse(writer.finish().unwrap()).unwrap()
    }

    #[test]
    fn round_trips_entries() {
        let archive = archive();
        assert_eq!(archive.len(), 2);
        assert_eq!(
            archive
                .read(&AssetPath::new("assets/hello.txt").unwrap())
                .unwrap(),
            b"hello"
        );
        assert!(archive.exists(&AssetPath::new("assets/data.bin").unwrap()));
        assert_eq!(archive.list().len(), 2);
    }

    #[test]
    fn empty_archive_is_valid() {
        let archive = PakArchive::parse(PakWriter::new().finish().unwrap()).unwrap();
        assert!(archive.is_empty());
        assert!(archive.list().is_empty());
    }

    #[test]
    fn rejects_bad_magic_and_truncation() {
        assert!(PakArchive::parse(b"NOTAPAK!".to_vec()).is_err());
        let bytes = archive();
        let mut truncated = bytes.data.clone();
        truncated.truncate(20);
        assert!(PakArchive::parse(truncated).is_err());
    }

    #[test]
    fn detects_corrupted_entry_data() {
        let mut archive = archive();
        let position = archive
            .data
            .windows(5)
            .rposition(|window| window == b"hello")
            .unwrap();
        archive.data[position] = b'j';
        let error = archive
            .read(&AssetPath::new("assets/hello.txt").unwrap())
            .unwrap_err();
        assert!(matches!(error, ResourceError::ChecksumMismatch(_)));
    }

    #[test]
    fn rejects_duplicate_writer_entries() {
        let mut writer = PakWriter::new();
        writer.add("a.txt", vec![1]).unwrap();
        assert!(writer.add("a.txt", vec![2]).is_err());
        assert!(writer.add("../b.txt", vec![2]).is_err());
    }
}
