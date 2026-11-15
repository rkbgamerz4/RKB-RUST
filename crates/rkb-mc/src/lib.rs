//! Minecraft: Java Edition world-data compatibility.
//!
//! This crate reads data produced by Minecraft: Java Edition 1.21 without
//! containing or redistributing any Minecraft code, assets, or data files.
//! The first implemented subsystem is the saved-world data path:
//!
//! - [`nbt`]: Java Edition NBT with modified UTF-8 strings.
//! - [`region`]: Anvil `.mca` region files and chunk compression.
//! - [`chunk`]: 1.21 chunk sections, block-state palettes, and block lookup.
//! - [`version`]: explicit Minecraft version descriptors and compatibility
//!   checks; unknown or unverified versions are rejected instead of guessed.
//! - [`registry`]: block-state identities from a user-supplied, version-pinned
//!   registry report.
//! - [`world`]: a save directory (`level.dat` plus `region/`) and per-block
//!   access.
//!
//! The crate is platform independent: it contains no Android, GPU, or window
//! code and builds for Android ARM64 like every other RKB crate.

#![forbid(unsafe_code)]

pub mod chunk;
pub mod fixture;
pub mod nbt;
pub mod region;
pub mod registry;
pub mod version;
pub mod world;

pub use chunk::{BlockState, Chunk, Section};
pub use nbt::{Nbt, Tag};
pub use region::{Compression, RegionFile};
pub use registry::{BlockRegistry, RegistryError};
pub use version::{VersionError, VersionSpec, VersionStatus, Versions};
pub use world::{LevelInfo, OpenOptions, World, WorldError};

/// Crate version, reported by the `rkb-world` tool.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
