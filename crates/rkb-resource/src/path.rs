//! Normalized asset paths, stable IDs, and resource errors.

use std::fmt;

/// Error raised by an asset source or archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceError {
    InvalidPath(String),
    NotFound(String),
    Io(String),
    Archive(String),
    ChecksumMismatch(String),
    InvalidUtf8(String),
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath(path) => write!(f, "invalid asset path '{path}'"),
            Self::NotFound(path) => write!(f, "asset not found: '{path}'"),
            Self::Io(message) => write!(f, "asset I/O error: {message}"),
            Self::Archive(message) => write!(f, "invalid PAK archive: {message}"),
            Self::ChecksumMismatch(path) => write!(f, "checksum mismatch for '{path}'"),
            Self::InvalidUtf8(path) => write!(f, "asset is not valid UTF-8: '{path}'"),
        }
    }
}

impl std::error::Error for ResourceError {}

/// A stable 64-bit asset identifier derived from the normalized path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetId(pub u64);

impl AssetId {
    #[must_use]
    pub fn from_path(path: &AssetPath) -> Self {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in path.as_str().as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Self(hash)
    }
}

/// A slash-separated relative asset path with no traversal segments.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetPath(String);

impl AssetPath {
    pub fn new(path: &str) -> Result<Self, ResourceError> {
        if path.is_empty() || path.len() > 1024 || path.contains('\0') || path.contains('\\') {
            return Err(ResourceError::InvalidPath(path.to_owned()));
        }

        let mut parts = Vec::new();
        for part in path.split('/') {
            match part {
                "" | "." => continue,
                ".." => return Err(ResourceError::InvalidPath(path.to_owned())),
                _ => {
                    if part.chars().any(|c| c.is_control()) {
                        return Err(ResourceError::InvalidPath(path.to_owned()));
                    }
                    parts.push(part);
                }
            }
        }
        if parts.is_empty() {
            return Err(ResourceError::InvalidPath(path.to_owned()));
        }
        Ok(Self(parts.join("/")))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn id(&self) -> AssetId {
        AssetId::from_path(self)
    }

    #[must_use]
    pub fn extension(&self) -> Option<&str> {
        self.0.rsplit_once('.').map(|(_, extension)| extension)
    }
}

impl fmt::Display for AssetPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for AssetPath {
    type Err = ResourceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_redundant_separators() {
        let path = AssetPath::new("//textures/./icons//play.png").unwrap();
        assert_eq!(path.as_str(), "textures/icons/play.png");
        assert_eq!(path.extension(), Some("png"));
    }

    #[test]
    fn rejects_traversal_and_absolute_paths() {
        assert!(AssetPath::new("../secret").is_err());
        assert!(AssetPath::new("a/../../b").is_err());
        assert!(AssetPath::new("/").is_err());
        assert!(AssetPath::new("a\\b").is_err());
        assert!(AssetPath::new("").is_err());
    }

    #[test]
    fn ids_are_deterministic_and_distinct() {
        let a = AssetPath::new("a.txt").unwrap();
        let b = AssetPath::new("b.txt").unwrap();
        assert_eq!(a.id(), AssetPath::new("a.txt").unwrap().id());
        assert_ne!(a.id(), b.id());
    }
}
