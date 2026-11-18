//! Minimal ZIP reader for resource packs.
//!
//! Supports the two compression methods a resource pack uses: stored (0) and
//! deflate (8). ZIP64, encryption, and multi-disk archives are rejected with a
//! clear error. CRC values are not validated; the reader checks sizes and
//! signature structure instead, because pack integrity is the user's concern
//! and a corrupt entry fails to decode downstream.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use miniz_oxide::inflate::decompress_to_vec;

use super::PackError;

const EOCD_SIGNATURE: u32 = 0x0605_4b50;
const CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
const LOCAL_SIGNATURE: u32 = 0x0403_4b50;
/// Cap for one extracted entry. Resource-pack textures are far smaller.
const MAX_ENTRY: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
struct Entry {
    method: u16,
    compressed_size: usize,
    uncompressed_size: usize,
    local_offset: usize,
}

/// An opened ZIP archive held in memory.
#[derive(Clone, Debug)]
pub struct ZipArchive {
    data: Vec<u8>,
    entries: BTreeMap<String, Entry>,
}

impl ZipArchive {
    /// Reads a ZIP file from disk.
    pub fn open(path: &Path) -> Result<Self, PackError> {
        let data = fs::read(path)
            .map_err(|error| PackError::new(format!("{}: {error}", path.display())))?;
        Self::from_bytes(data)
    }

    /// Parses a ZIP from bytes.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, PackError> {
        let eocd = find_eocd(&data)?;
        let entry_count = read_u16(&data, eocd + 10)? as usize;
        let directory_offset = read_u32(&data, eocd + 16)? as usize;
        let mut entries = BTreeMap::new();
        let mut offset = directory_offset;
        for _ in 0..entry_count {
            if read_u32(&data, offset)? != CENTRAL_SIGNATURE {
                return Err(PackError::new("bad central-directory signature"));
            }
            let method = read_u16(&data, offset + 10)?;
            let compressed_size = read_u32(&data, offset + 20)? as usize;
            let uncompressed_size = read_u32(&data, offset + 24)? as usize;
            let name_len = read_u16(&data, offset + 28)? as usize;
            let extra_len = read_u16(&data, offset + 30)? as usize;
            let comment_len = read_u16(&data, offset + 32)? as usize;
            let local_offset = read_u32(&data, offset + 42)? as usize;
            if compressed_size == u32::MAX as usize || uncompressed_size == u32::MAX as usize {
                return Err(PackError::new(
                    "ZIP64 archives are not supported; re-zip without ZIP64",
                ));
            }
            let name_start = offset + 46;
            let name_end = name_start + name_len;
            if name_end > data.len() {
                return Err(PackError::new("central-directory name is truncated"));
            }
            let name = String::from_utf8_lossy(&data[name_start..name_end]).into_owned();
            let name = normalize(&name);
            if !name.is_empty() {
                entries.insert(
                    name,
                    Entry {
                        method,
                        compressed_size,
                        uncompressed_size,
                        local_offset,
                    },
                );
            }
            offset = name_end + extra_len + comment_len;
        }
        Ok(Self { data, entries })
    }

    /// Entry names in the archive.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Reads one entry by its exact archive path.
    pub fn read(&self, name: &str) -> Result<Option<Vec<u8>>, PackError> {
        if !is_safe(name) {
            return Err(PackError::new(format!(
                "refusing unsafe archive path {name:?}"
            )));
        }
        let Some(entry) = self.entries.get(name) else {
            return Ok(None);
        };
        if entry.local_offset + 30 > self.data.len() {
            return Err(PackError::new(format!("{name}: local header is truncated")));
        }
        if read_u32(&self.data, entry.local_offset)? != LOCAL_SIGNATURE {
            return Err(PackError::new(format!("{name}: bad local header")));
        }
        if entry.uncompressed_size > MAX_ENTRY {
            return Err(PackError::new(format!(
                "{name}: entry exceeds the size cap"
            )));
        }
        let name_len = read_u16(&self.data, entry.local_offset + 26)? as usize;
        let extra_len = read_u16(&self.data, entry.local_offset + 28)? as usize;
        let start = entry.local_offset + 30 + name_len + extra_len;
        let end = start + entry.compressed_size;
        if end > self.data.len() {
            return Err(PackError::new(format!("{name}: payload is truncated")));
        }
        let payload = &self.data[start..end];
        match entry.method {
            0 => {
                if payload.len() != entry.uncompressed_size {
                    return Err(PackError::new(format!("{name}: stored size mismatch")));
                }
                Ok(Some(payload.to_vec()))
            }
            8 => {
                let bytes = decompress_to_vec(payload).map_err(|error| {
                    PackError::new(format!("{name}: inflate failed: {error:?}"))
                })?;
                if bytes.len() != entry.uncompressed_size {
                    return Err(PackError::new(format!(
                        "{name}: inflated {} bytes, expected {}",
                        bytes.len(),
                        entry.uncompressed_size
                    )));
                }
                Ok(Some(bytes))
            }
            method => Err(PackError::new(format!(
                "{name}: unsupported compression method {method}"
            ))),
        }
    }

    /// Finds an entry by exact path or by a `/<path>` suffix.
    ///
    /// A pack is often zipped inside a folder, so `assets/...` may live at
    /// `MyPack/assets/...`. The shortest matching suffix wins to keep the
    /// choice deterministic.
    pub fn find(&self, path: &str) -> Option<&str> {
        if self.entries.contains_key(path) {
            return self
                .entries
                .get_key_value(path)
                .map(|(name, _)| name.as_str());
        }
        let suffix = format!("/{path}");
        self.entries
            .keys()
            .filter(|name| name.ends_with(&suffix))
            .min_by_key(|name| name.len())
            .map(String::as_str)
    }

    /// Reads a path with suffix fallback.
    pub fn read_path(&self, path: &str) -> Result<Option<Vec<u8>>, PackError> {
        match self.find(path) {
            Some(name) => self.read(name),
            None => Ok(None),
        }
    }
}

fn normalize(name: &str) -> String {
    name.replace('\\', "/").trim_start_matches('/').to_owned()
}

fn is_safe(name: &str) -> bool {
    !name.split('/').any(|part| part == "..")
}

fn find_eocd(data: &[u8]) -> Result<usize, PackError> {
    if data.len() < 22 {
        return Err(PackError::new("file is too small to be a ZIP archive"));
    }
    let start = data.len().saturating_sub(22 + 65_535);
    for offset in (start..=data.len() - 22).rev() {
        if read_u32(data, offset).ok() == Some(EOCD_SIGNATURE) {
            return Ok(offset);
        }
    }
    Err(PackError::new(
        "ZIP end-of-central-directory record not found",
    ))
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, PackError> {
    let bytes = data
        .get(offset..offset + 2)
        .ok_or_else(|| PackError::new("truncated ZIP field"))?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, PackError> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| PackError::new("truncated ZIP field"))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniz_oxide::deflate::compress_to_vec;

    fn zip_bytes(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data, deflate) in entries {
            let offset = out.len() as u32;
            let payload = if *deflate {
                compress_to_vec(data, 6)
            } else {
                data.to_vec()
            };
            out.extend_from_slice(&LOCAL_SIGNATURE.to_le_bytes());
            out.extend_from_slice(&20_u16.to_le_bytes());
            out.extend_from_slice(&0_u16.to_le_bytes());
            out.extend_from_slice(&(if *deflate { 8_u16 } else { 0_u16 }).to_le_bytes());
            out.extend_from_slice(&0_u32.to_le_bytes()); // time/date
            out.extend_from_slice(&0_u32.to_le_bytes()); // crc
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0_u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&payload);

            central.extend_from_slice(&CENTRAL_SIGNATURE.to_le_bytes());
            central.extend_from_slice(&20_u16.to_le_bytes());
            central.extend_from_slice(&20_u16.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&(if *deflate { 8_u16 } else { 0_u16 }).to_le_bytes());
            central.extend_from_slice(&0_u32.to_le_bytes());
            central.extend_from_slice(&0_u32.to_le_bytes());
            central.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u32.to_le_bytes());
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let directory_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&EOCD_SIGNATURE.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&directory_offset.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out
    }

    #[test]
    fn reads_stored_and_deflated_entries() {
        let bytes = zip_bytes(&[
            (
                "assets/minecraft/textures/block/stone.png",
                b"stored",
                false,
            ),
            ("assets/minecraft/blockstates/stone.json", b"deflated", true),
        ]);
        let zip = ZipArchive::from_bytes(bytes).unwrap();
        assert_eq!(
            zip.read("assets/minecraft/textures/block/stone.png")
                .unwrap()
                .unwrap(),
            b"stored"
        );
        assert_eq!(
            zip.read("assets/minecraft/blockstates/stone.json")
                .unwrap()
                .unwrap(),
            b"deflated"
        );
        assert!(zip.read("missing").unwrap().is_none());
    }

    #[test]
    fn finds_entries_inside_a_top_level_folder() {
        let bytes = zip_bytes(&[("MyPack/assets/minecraft/pack.mcmeta", b"{}", true)]);
        let zip = ZipArchive::from_bytes(bytes).unwrap();
        assert_eq!(
            zip.find("assets/minecraft/pack.mcmeta"),
            Some("MyPack/assets/minecraft/pack.mcmeta")
        );
        assert_eq!(
            zip.read_path("assets/minecraft/pack.mcmeta")
                .unwrap()
                .unwrap(),
            b"{}"
        );
    }

    #[test]
    fn rejects_unsafe_paths_and_truncated_archives() {
        let bytes = zip_bytes(&[("a.txt", b"x", false)]);
        let zip = ZipArchive::from_bytes(bytes).unwrap();
        assert!(zip.read("../a.txt").is_err());
        assert!(ZipArchive::from_bytes(vec![0; 10]).is_err());
        let mut broken = zip_bytes(&[("a.txt", b"x", false)]);
        broken.truncate(30);
        assert!(ZipArchive::from_bytes(broken).is_err());
    }
}
