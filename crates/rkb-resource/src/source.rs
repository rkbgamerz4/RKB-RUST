//! In-memory and filesystem-backed asset sources.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::path::{AssetPath, ResourceError};

/// A read-only source of named byte assets.
pub trait AssetSource: Send + Sync {
    fn read(&self, path: &AssetPath) -> Result<Vec<u8>, ResourceError>;

    fn exists(&self, path: &AssetPath) -> bool {
        self.read(path).is_ok()
    }

    fn list(&self) -> Vec<AssetPath>;
}

/// Assets embedded in process memory. Useful for Android assets and tests.
#[derive(Clone, Debug, Default)]
pub struct MemorySource {
    entries: BTreeMap<String, Vec<u8>>,
}

impl MemorySource {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    pub fn from_entries<I, P, B>(entries: I) -> Result<Self, ResourceError>
    where
        I: IntoIterator<Item = (P, B)>,
        P: AsRef<str>,
        B: Into<Vec<u8>>,
    {
        let mut source = Self::new();
        for (path, bytes) in entries {
            source.add(path.as_ref(), bytes.into())?;
        }
        Ok(source)
    }

    pub fn add(&mut self, path: &str, bytes: impl Into<Vec<u8>>) -> Result<(), ResourceError> {
        let path = AssetPath::new(path)?;
        self.entries.insert(path.as_str().to_owned(), bytes.into());
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
}

impl AssetSource for MemorySource {
    fn read(&self, path: &AssetPath) -> Result<Vec<u8>, ResourceError> {
        self.entries
            .get(path.as_str())
            .cloned()
            .ok_or_else(|| ResourceError::NotFound(path.to_string()))
    }

    fn exists(&self, path: &AssetPath) -> bool {
        self.entries.contains_key(path.as_str())
    }

    fn list(&self) -> Vec<AssetPath> {
        self.entries
            .keys()
            .filter_map(|path| AssetPath::new(path).ok())
            .collect()
    }
}

/// Assets read from a directory, with traversal and symlink-escape protection.
#[derive(Clone, Debug)]
pub struct FileSource {
    root: PathBuf,
}

impl FileSource {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, ResourceError> {
        let canonical = fs::canonicalize(root.as_ref())
            .map_err(|error| ResourceError::Io(format!("{}: {error}", root.as_ref().display())))?;
        if !canonical.is_dir() {
            return Err(ResourceError::Io(format!(
                "{} is not a directory",
                canonical.display()
            )));
        }
        Ok(Self { root: canonical })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn resolve(&self, path: &AssetPath) -> Result<PathBuf, ResourceError> {
        let candidate = self.root.join(path.as_str());
        let canonical = fs::canonicalize(&candidate).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => ResourceError::NotFound(path.to_string()),
            _ => ResourceError::Io(format!("{}: {error}", candidate.display())),
        })?;
        if !canonical.starts_with(&self.root) {
            return Err(ResourceError::InvalidPath(path.to_string()));
        }
        Ok(canonical)
    }
}

impl AssetSource for FileSource {
    fn read(&self, path: &AssetPath) -> Result<Vec<u8>, ResourceError> {
        let resolved = self.resolve(path)?;
        fs::read(&resolved)
            .map_err(|error| ResourceError::Io(format!("{}: {error}", resolved.display())))
    }

    fn exists(&self, path: &AssetPath) -> bool {
        self.resolve(path).is_ok_and(|path| path.is_file())
    }

    fn list(&self) -> Vec<AssetPath> {
        fn walk(root: &Path, directory: &Path, output: &mut Vec<AssetPath>) {
            let Ok(entries) = fs::read_dir(directory) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_symlink() {
                    continue;
                }
                if file_type.is_dir() {
                    walk(root, &path, output);
                } else if file_type.is_file() {
                    if let Ok(relative) = path.strip_prefix(root) {
                        if let Some(relative) = relative.to_str() {
                            if let Ok(asset) = AssetPath::new(relative) {
                                output.push(asset);
                            }
                        }
                    }
                }
            }
        }

        let mut assets = Vec::new();
        walk(&self.root, &self.root, &mut assets);
        assets.sort();
        assets
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_source_reads_and_lists() {
        let source = MemorySource::from_entries([
            ("config/engine.rkb", b"name = demo".to_vec()),
            ("textures/a.txt", vec![1, 2, 3]),
        ])
        .unwrap();
        assert_eq!(source.len(), 2);
        assert!(source.exists(&AssetPath::new("config/engine.rkb").unwrap()));
        assert_eq!(
            source
                .read(&AssetPath::new("textures/a.txt").unwrap())
                .unwrap(),
            vec![1, 2, 3]
        );
        assert_eq!(source.list().len(), 2);
        assert!(source
            .read(&AssetPath::new("missing").unwrap())
            .unwrap_err()
            .to_string()
            .contains("not found"));
    }

    #[test]
    fn file_source_round_trips_and_blocks_traversal() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("rkb-resource-test-{}-{stamp}", std::process::id()));
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("nested/hello.txt"), b"hello").unwrap();

        let source = FileSource::new(&root).unwrap();
        let path = AssetPath::new("nested/hello.txt").unwrap();
        assert_eq!(source.read(&path).unwrap(), b"hello");
        assert!(source.exists(&path));
        assert_eq!(source.list(), vec![path]);
        assert!(source
            .read(&AssetPath::new("nested/missing.txt").unwrap())
            .is_err());

        let outside = AssetPath::new("../outside.txt").unwrap_err();
        assert!(matches!(outside, ResourceError::InvalidPath(_)));

        fs::remove_dir_all(&root).unwrap();
    }
}
