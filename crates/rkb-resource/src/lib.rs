//! Asset loading with path traversal protection and deterministic archives.

#![forbid(unsafe_code)]

pub mod archive;
pub mod manager;
pub mod path;
pub mod source;

pub use archive::{PakArchive, PakWriter};
pub use manager::{CacheStats, ResourceManager};
pub use path::{AssetId, AssetPath, ResourceError};
pub use source::{AssetSource, FileSource, MemorySource};
