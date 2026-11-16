//! Minecraft: Java Edition 1.21 client for RKB RUST.
//!
//! The crate is deliberately honest about scope. It loads user-supplied 1.21
//! saves, streams and meshes saved chunks, renders opaque block geometry with
//! a procedural texture atlas, per-vertex ambient occlusion and face shading,
//! and provides first-person movement with touch controls. Resource-pack
//! loading, dynamic lighting, inventory, mobs, and survival are not implemented
//! yet.
//!
//! No Minecraft assets, code, or world data are bundled. [`demo_world`] writes
//! a synthetic save used only by tests and emulator verification.

#![forbid(unsafe_code)]

pub mod blocks;
pub mod camera;
pub mod demo_world;
pub mod game;
pub mod interaction;
pub mod mesh;
pub mod player;
pub mod session;
pub mod textures;
pub mod world_source;
pub mod worldgen;

pub use blocks::{block_color, is_solid};
pub use camera::{Camera, Frustum};
pub use game::{GameStats, MinecraftGame};
pub use interaction::{raycast, RayHit};
pub use mesh::{build_chunk_mesh, mesh_bounds, ChunkMesh, EditMap, Face};
pub use player::{BlockQuery, MoveInput, Player};
pub use world_source::{ChunkSource, WorldKind, WorldState};
pub use worldgen::{Biome, GeneratedWorld, TerrainGenerator, DEFAULT_SEED};

/// Crate version, reported by the fixture tool and the Android bridge.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
