//! Android (and generic) window-surface presentation.
//!
//! Kotlin owns the `Surface`; `rkb-android` turns it into an `ANativeWindow`
//! pointer and passes the raw handles here. This module never touches JNI and
//! never calls `lockCanvas`, so the swapchain path is independent of the M6
//! bitmap fallback.
//!
//! Surface creation from a raw handle is the one place in `rkb-gpu` that needs
//! `unsafe`: the caller guarantees the window outlives the surface. Everything
//! else in the crate is ordinary safe code.
#![allow(unsafe_code)]

use rkb_core::Vec2;
use rkb_minecraft::Camera;
use rkb_render::{Color, DrawCommand};

use crate::scene::{BackendInfo, Fog, FrameStats, GpuContext, GpuError, Scene};

/// Creates a wgpu surface for a native window.
pub fn create_surface(
    instance: &wgpu::Instance,
    window: wgpu::rwh::RawWindowHandle,
    display: wgpu::rwh::RawDisplayHandle,
) -> Result<wgpu::Surface<'static>, GpuError> {
    // SAFETY: the caller keeps the native window (and its display) alive for at
    // least as long as the returned surface, which is the contract documented
    // by `SurfaceTargetUnsafe::RawHandle`.
    unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: display,
            raw_window_handle: window,
        })
    }
    .map_err(|error| GpuError(format!("surface: {error}")))
}

/// Chooses a swapchain format, preferring a non-sRGB one so GPU colours match
/// the M6 software framebuffer byte-for-byte. sRGB is still supported; the
/// shader applies the transfer curve when the chosen format is sRGB.
#[must_use]
pub fn choose_format(capabilities: &wgpu::SurfaceCapabilities) -> wgpu::TextureFormat {
    for preferred in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Bgra8Unorm,
    ] {
        if capabilities.formats.contains(&preferred) {
            return preferred;
        }
    }
    capabilities
        .formats
        .first()
        .copied()
        .unwrap_or(wgpu::TextureFormat::Bgra8Unorm)
}

/// Owns the swapchain and the [`Scene`] that draws into it.
pub struct SurfaceRenderer {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    scene: Scene,
    info: BackendInfo,
    width: u32,
    height: u32,
}

impl std::fmt::Debug for SurfaceRenderer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SurfaceRenderer")
            .field("size", &(self.width, self.height))
            .field("format", &self.config.format)
            .finish_non_exhaustive()
    }
}

impl SurfaceRenderer {
    /// Configures `surface` and builds the pipelines for its format.
    ///
    /// The surface must already be part of the adapter search: create the
    /// [`GpuContext`] with `GpuContext::from_instance(instance, Some(&surface))`
    /// so the selected adapter can present to it.
    pub fn new(
        context: &GpuContext,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
        atlas: &rkb_render::Texture,
    ) -> Result<Self, GpuError> {
        let width = width.max(1);
        let height = height.max(1);
        let capabilities = surface.get_capabilities(&context.adapter);
        if capabilities.formats.is_empty() {
            return Err(GpuError(
                "surface: adapter reports no swapchain formats".to_owned(),
            ));
        }
        let format = choose_format(&capabilities);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: capabilities
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&context.device, &config);
        let mut scene = Scene::new(context, format, atlas)?;
        scene.resize(width, height);
        Ok(Self {
            surface,
            config,
            scene,
            info: context.info.clone(),
            width,
            height,
        })
    }

    /// Adapter identity of the device presenting through this surface.
    #[must_use]
    pub fn info(&self) -> &BackendInfo {
        &self.info
    }

    /// `wgpu / <backend> / <name> <hardware|software>` for diagnostics.
    #[must_use]
    pub fn summary(&self) -> String {
        self.info.summary()
    }

    #[must_use]
    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    #[must_use]
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }

    /// Reconfigures the swapchain and depth target after rotation/resize.
    pub fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if width == self.width && height == self.height {
            return;
        }
        self.width = width;
        self.height = height;
        self.config.width = width;
        self.config.height = height;
        // The device belongs to the context that created this renderer; the
        // surface keeps its own handle so reconfiguration is always valid.
        self.surface.configure(self.scene.device(), &self.config);
        self.scene.resize(width, height);
    }

    /// Replaces the atlas texture; the caller re-uploads resident meshes.
    pub fn set_atlas(&mut self, atlas: &rkb_render::Texture) -> Result<(), GpuError> {
        self.scene.set_atlas(atlas)
    }

    /// Renders and presents one frame.
    ///
    /// `Lost`/`Outdated` swapchains are reconfigured once and retried, which is
    /// the normal path after a surface recreation race.
    pub fn render(
        &mut self,
        camera: &Camera,
        fog: Fog,
        sky_top: Color,
        ui: &[DrawCommand],
    ) -> Result<FrameStats, GpuError> {
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(self.scene.device(), &self.config);
                self.surface
                    .get_current_texture()
                    .map_err(|error| GpuError(format!("swapchain after reconfigure: {error}")))?
            }
            Err(error) => {
                return Err(GpuError(format!("swapchain: {error}")));
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let viewport = Vec2::new(self.width as f32, self.height as f32);
        let mut encoder =
            self.scene
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("rkb-surface"),
                });
        let stats = self
            .scene
            .encode(&mut encoder, &view, camera, viewport, fog, sky_top, ui);
        self.scene.queue().submit(Some(encoder.finish()));
        frame.present();
        Ok(stats)
    }
}
