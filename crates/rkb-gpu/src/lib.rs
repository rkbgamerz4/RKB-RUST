//! GPU rendering backend for RKB RUST.
//!
//! The backend is `wgpu 0.19` with the Vulkan and GL backends enabled. It
//! consumes the M6 chunk meshes from [`rkb_minecraft`] and the atlas the game
//! already builds, so models, resource packs, per-vertex light, ambient
//! occlusion, and transparency all come from the existing client rather than
//! being re-derived here.
//!
//! Three consumers share the same [`Scene`]:
//!
//! - [`HeadlessRenderer`] draws into an offscreen texture and reads it back;
//!   this is what the `rkb-join-gpu` verification tool and the CI GPU tests use.
//! - [`SurfaceRenderer`] draws into an Android `ANativeWindow` swapchain.
//! - The game itself keeps [`rkb_render::SoftwareBackend`] as the reference
//!   and fallback path.
//!
//! The architectural decision and the verification tiers are recorded in
//! `docs/m7-gpu-architecture.md`.
//!
//! `unsafe` is denied at the crate root. The single exception is
//! [`surface::create_surface`], which must hand a raw `ANativeWindow` pointer
//! to wgpu; that module re-allows the lint with the contract documented there.

#![deny(unsafe_code)]

pub mod headless;
pub mod mesh;
pub mod scene;
pub mod surface;
pub mod sync;

pub use headless::{save_ppm, HeadlessRenderer};
pub use mesh::{atlas_uv, pack_chunk, tint, GpuVertex, PackedChunk, TranslucentQuad, WorldMesh};
pub use scene::{
    create_instance, BackendInfo, Fog, FrameStats, GpuContext, GpuError, Scene, UploadStats,
    DEPTH_FORMAT, MAX_CHUNKS,
};
pub use surface::{create_surface, SurfaceRenderer};
pub use sync::{SceneSync, SyncStats, SyncTiming, DEFAULT_UPLOAD_BUDGET};

/// Raw window handle types used to create a native swapchain.
pub use wgpu::rwh;

/// Backends the project enables: Vulkan first, OpenGL ES as the same-code
/// fallback for drivers and emulators that expose only GLES.
#[must_use]
pub fn default_backends() -> wgpu::Backends {
    wgpu::Backends::VULKAN | wgpu::Backends::GL
}

/// The WGSL module used by every pipeline, also compiled by the shader test.
pub const WORLD_WGSL: &str = include_str!("world.wgsl");

/// Crate version, reported by diagnostics.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
