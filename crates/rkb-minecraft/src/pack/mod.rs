//! User-supplied resource packs in the vanilla layout.
//!
//! RKB does not bundle any Minecraft asset. A pack is whatever the user points
//! at: a directory or `.zip` containing `pack.mcmeta`, `assets/<namespace>/`
//! `blockstates/`, `models/`, and `textures/`. Everything here reads those
//! files and turns them into the renderer's shapes, textures, and atlas ids.
//!
//! Modules:
//! - [`png`] decodes the textures (8-bit, non-interlaced only);
//! - [`zip`] reads pack archives (stored and deflate);
//! - [`model`] resolves blockstate variants and model JSON into geometry;
//! - [`atlas`] packs the decoded textures into one atlas.

pub mod atlas;
pub mod model;
pub mod png;
pub mod zip;

pub use atlas::{Atlas, TextureStore};
pub use model::PackModels;
pub use png::{decode as decode_png, Image};

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

/// A pack could not be opened or an asset is unreadable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackError(pub String);

impl PackError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "resource pack: {}", self.0)
    }
}

impl std::error::Error for PackError {}

#[derive(Debug)]
enum Source {
    Directory(PathBuf),
    Archive(zip::ZipArchive),
}

/// An opened resource pack.
#[derive(Debug)]
pub struct ResourcePack {
    source: Source,
    /// `pack.mcmeta`'s `pack_format`, when the file exists.
    pub pack_format: Option<i64>,
    /// Pack description from `pack.mcmeta`, when present.
    pub description: String,
    /// Where the pack was opened from, for status reporting.
    pub origin: String,
}

impl ResourcePack {
    /// Opens a pack directory or `.zip` archive.
    pub fn open(path: &Path) -> Result<Self, PackError> {
        if path.is_dir() {
            let root = find_pack_root(path)?;
            let mut pack = Self {
                source: Source::Directory(root.clone()),
                pack_format: None,
                description: String::new(),
                origin: path.display().to_string(),
            };
            if let Some(meta) = pack.read_json("pack.mcmeta")? {
                if let Some(pack_meta) = meta.get("pack") {
                    pack.pack_format = pack_meta.get("pack_format").and_then(Value::as_i64);
                    pack.description = pack_meta
                        .get("description")
                        .map(plain_text)
                        .unwrap_or_default();
                }
            }
            Ok(pack)
        } else if path.is_file() {
            let archive = zip::ZipArchive::open(path)?;
            let mut pack = Self {
                source: Source::Archive(archive),
                pack_format: None,
                description: String::new(),
                origin: path.display().to_string(),
            };
            if let Some(meta) = pack.read_json("pack.mcmeta")? {
                if let Some(pack_meta) = meta.get("pack") {
                    pack.pack_format = pack_meta.get("pack_format").and_then(Value::as_i64);
                    pack.description = pack_meta
                        .get("description")
                        .map(plain_text)
                        .unwrap_or_default();
                }
            }
            Ok(pack)
        } else {
            Err(PackError::new(format!(
                "{} is neither a directory nor a .zip archive",
                path.display()
            )))
        }
    }

    /// Reads one file relative to the pack root.
    pub fn read(&self, relative: &str) -> Result<Option<Vec<u8>>, PackError> {
        let relative = relative.trim_start_matches('/');
        if relative.split('/').any(|part| part == ".." || part == ".") {
            return Err(PackError::new(format!("unsafe pack path {relative:?}")));
        }
        match &self.source {
            Source::Directory(root) => {
                let path = root.join(relative);
                match fs::read(&path) {
                    Ok(bytes) => Ok(Some(bytes)),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(error) => Err(PackError::new(format!("{}: {error}", path.display()))),
                }
            }
            Source::Archive(archive) => archive.read_path(relative),
        }
    }

    /// Reads and parses one JSON file.
    pub fn read_json(&self, relative: &str) -> Result<Option<Value>, PackError> {
        let Some(bytes) = self.read(relative)? else {
            return Ok(None);
        };
        let text = String::from_utf8(bytes)
            .map_err(|_| PackError::new(format!("{relative} is not UTF-8")))?;
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| PackError::new(format!("{relative}: invalid JSON: {error}")))
    }

    /// Decodes one texture referenced by a model (`minecraft:block/stone`,
    /// `block/stone`, or `#alias` resolved by the caller).
    pub fn texture(&self, reference: &str) -> Result<Option<Image>, PackError> {
        let Some(bytes) = self.read(&texture_relative(reference))? else {
            return Ok(None);
        };
        decode_png(&bytes)
            .map(Some)
            .map_err(|error| PackError::new(format!("{reference}: {error}")))
    }
}

/// Finds the directory that actually contains `pack.mcmeta`/`assets`.
///
/// Users sometimes point at a parent folder holding one pack, so a single
/// descending step is allowed; anything ambiguous is reported.
fn find_pack_root(path: &Path) -> Result<PathBuf, PackError> {
    let mut root = path.to_path_buf();
    for _ in 0..2 {
        if root.join("pack.mcmeta").is_file() || root.join("assets").is_dir() {
            return Ok(root);
        }
        let mut children: Vec<PathBuf> = fs::read_dir(&root)
            .map_err(|error| PackError::new(format!("{}: {error}", root.display())))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|child| child.is_dir())
            .collect();
        children.sort();
        if children.len() != 1 {
            break;
        }
        root = children.pop().expect("one child");
    }
    Err(PackError::new(format!(
        "{} has no pack.mcmeta or assets directory",
        path.display()
    )))
}

fn plain_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts.iter().map(plain_text).collect::<Vec<_>>().join(""),
        Value::Object(map) => map.get("text").map(plain_text).unwrap_or_default(),
        _ => String::new(),
    }
}

/// Splits `namespace:path` and returns both parts.
pub(crate) fn split_reference(reference: &str) -> (&str, &str) {
    match reference.split_once(':') {
        Some((namespace, path)) => (namespace, path),
        None => ("minecraft", reference),
    }
}

/// `assets/<namespace>/blockstates/<name>.json`.
#[must_use]
pub fn blockstate_relative(name: &str) -> String {
    let (namespace, path) = split_reference(name);
    format!("assets/{namespace}/blockstates/{path}.json")
}

/// `assets/<namespace>/models/<name>.json`.
#[must_use]
pub fn model_relative(name: &str) -> String {
    let (namespace, path) = split_reference(name);
    let path = path.strip_prefix("models/").unwrap_or(path);
    format!("assets/{namespace}/models/{path}.json")
}

/// `assets/<namespace>/textures/<reference>.png`.
#[must_use]
pub fn texture_relative(reference: &str) -> String {
    let (namespace, path) = split_reference(reference);
    let path = path.strip_prefix("textures/").unwrap_or(path);
    let path = if path.ends_with(".png") {
        path.to_owned()
    } else {
        format!("{path}.png")
    };
    format!("assets/{namespace}/textures/{path}")
}

/// Rejects path components that could escape a pack root.
#[must_use]
pub fn is_safe_relative(relative: &str) -> bool {
    Path::new(relative)
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
}

/// Builds a minimal 8-bit RGBA PNG from raw pixels.
///
/// Only used by tests in this module: the file is authored here, not taken
/// from any game, and it exercises the decoder through a real pack layout.
#[cfg(test)]
pub(crate) fn pack_png(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    use miniz_oxide::deflate::compress_to_vec_zlib;

    assert_eq!(rgba.len(), (width * height * 4) as usize);
    let mut raw = Vec::with_capacity(((width * 4 + 1) * height) as usize);
    for row in rgba.chunks_exact((width * 4) as usize) {
        raw.push(0); // filter: none
        raw.extend_from_slice(row);
    }
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let chunk = |kind: &[u8; 4], data: &[u8], out: &mut Vec<u8>| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&[0, 0, 0, 0]);
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    chunk(b"IHDR", &ihdr, &mut out);
    chunk(b"IDAT", &compress_to_vec_zlib(&raw, 6), &mut out);
    chunk(b"IEND", &[], &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_paths_follow_the_vanilla_layout() {
        assert_eq!(
            blockstate_relative("minecraft:stone"),
            "assets/minecraft/blockstates/stone.json"
        );
        assert_eq!(
            model_relative("block/cube_all"),
            "assets/minecraft/models/block/cube_all.json"
        );
        assert_eq!(
            model_relative("minecraft:block/slab"),
            "assets/minecraft/models/block/slab.json"
        );
        assert_eq!(
            texture_relative("block/stone"),
            "assets/minecraft/textures/block/stone.png"
        );
        assert_eq!(
            texture_relative("minecraft:block/stone.png"),
            "assets/minecraft/textures/block/stone.png"
        );
    }

    #[test]
    fn unsafe_paths_are_rejected() {
        assert!(!is_safe_relative("../secrets.txt"));
        assert!(!is_safe_relative("/etc/passwd"));
        assert!(is_safe_relative("assets/minecraft/blockstates/stone.json"));
    }
}
