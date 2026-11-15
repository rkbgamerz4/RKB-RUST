//! Rendering foundation.
//!
//! The current backend is a deterministic software rasterizer used for tests,
//! headless rendering, and the Android fallback surface. The command-list and
//! backend traits are the seam for a future OpenGL ES/Vulkan backend.

#![forbid(unsafe_code)]

mod color;
mod framebuffer;
mod renderer;
mod texture;

pub use color::{Color, ColorError};
pub use framebuffer::Framebuffer;
pub use renderer::{
    project_clip, Camera2D, DrawCommand, DrawList, RenderBackend, RenderError, RenderStats,
    SoftwareBackend, Vertex3D,
};
pub use texture::Texture;
