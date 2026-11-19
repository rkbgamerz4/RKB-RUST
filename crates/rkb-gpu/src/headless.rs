//! Offscreen rendering and framebuffer readback.
//!
//! This is the path the GPU verification tool and the GPU tests use. It renders
//! the same [`Scene`] the Android swapchain path renders, into an ordinary
//! texture, so a captured frame is genuine GPU output rather than a software
//! rasterization.

use std::path::Path;

use rkb_core::Vec2;
use rkb_minecraft::Camera;
use rkb_render::DrawCommand;

use crate::scene::{Fog, FrameStats, GpuContext, GpuError, Scene};
use crate::BackendInfo;

/// Rows in `copy_texture_to_buffer` must be a multiple of this many bytes.
const COPY_ALIGN: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

/// A [`Scene`] plus an offscreen color target and readback buffer.
pub struct HeadlessRenderer {
    context: GpuContext,
    scene: Scene,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
    width: u32,
    height: u32,
    padded_row: u32,
}

impl std::fmt::Debug for HeadlessRenderer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HeadlessRenderer")
            .field("info", &self.info())
            .field("size", &(self.width, self.height))
            .finish_non_exhaustive()
    }
}

impl HeadlessRenderer {
    /// Creates a device and an offscreen target of `width` × `height`.
    pub fn new(
        backends: wgpu::Backends,
        width: u32,
        height: u32,
        atlas: &rkb_render::Texture,
    ) -> Result<Self, GpuError> {
        let context = GpuContext::new(backends)?;
        Self::with_context(context, width, height, atlas)
    }

    /// Reuses an existing device, for tests that create one context.
    pub fn with_context(
        context: GpuContext,
        width: u32,
        height: u32,
        atlas: &rkb_render::Texture,
    ) -> Result<Self, GpuError> {
        // A non-sRGB target keeps GPU output byte-comparable with the M6
        // software framebuffer.
        let scene = Scene::new(&context, wgpu::TextureFormat::Rgba8Unorm, atlas)?;
        let (texture, view, readback, padded_row) = create_target(&context.device, width, height)?;
        Ok(Self {
            context,
            scene,
            texture,
            view,
            readback,
            width,
            height,
            padded_row,
        })
    }

    #[must_use]
    pub fn info(&self) -> &BackendInfo {
        &self.context.info
    }

    #[must_use]
    pub fn context(&self) -> &GpuContext {
        &self.context
    }

    #[must_use]
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }

    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Recreates the color target and depth buffer after a size change.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), GpuError> {
        let width = width.max(1);
        let height = height.max(1);
        if width == self.width && height == self.height {
            return Ok(());
        }
        let (texture, view, readback, padded_row) =
            create_target(&self.context.device, width, height)?;
        self.texture = texture;
        self.view = view;
        self.readback = readback;
        self.padded_row = padded_row;
        self.width = width;
        self.height = height;
        self.scene.resize(width, height);
        Ok(())
    }

    /// Renders one frame and returns the scene counters.
    pub fn render(
        &mut self,
        camera: &Camera,
        fog: Fog,
        sky_top: rkb_render::Color,
        ui: &[DrawCommand],
    ) -> FrameStats {
        let viewport = Vec2::new(self.width as f32, self.height as f32);
        let mut encoder =
            self.context
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("rkb-headless"),
                });
        let stats = self
            .scene
            .encode(&mut encoder, &self.view, camera, viewport, fog, sky_top, ui);
        self.context.queue.submit(Some(encoder.finish()));
        stats
    }

    /// Copies the last rendered frame back and returns tightly packed RGBA.
    pub fn read_rgba(&mut self) -> Result<Vec<u8>, GpuError> {
        let mut encoder =
            self.context
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("rkb-readback"),
                });
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &self.readback,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        self.context.queue.submit(Some(encoder.finish()));

        let slice = self.readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            // The receiver outlives the callback; an error is reported by the
            // missing message below.
            let _ = sender.send(result);
        });
        self.context.device.poll(wgpu::Maintain::Wait);
        match receiver.try_recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(GpuError(format!("readback map: {error:?}"))),
            Err(error) => return Err(GpuError(format!("readback callback: {error}"))),
        }

        let data = slice.get_mapped_range();
        let row_bytes = (self.width * 4) as usize;
        let padded = self.padded_row as usize;
        let mut pixels = Vec::with_capacity(row_bytes * self.height as usize);
        for row in 0..self.height as usize {
            let start = row * padded;
            pixels.extend_from_slice(&data[start..start + row_bytes]);
        }
        drop(data);
        self.readback.unmap();
        Ok(pixels)
    }
}

fn create_target(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> Result<(wgpu::Texture, wgpu::TextureView, wgpu::Buffer, u32), GpuError> {
    let width = width.max(1);
    let height = height.max(1);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rkb-headless-color"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let row_bytes = width * 4;
    let padded_row = row_bytes.div_ceil(COPY_ALIGN) * COPY_ALIGN;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rkb-headless-readback"),
        size: u64::from(padded_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    Ok((texture, view, readback, padded_row))
}

/// Writes tightly packed RGBA as a binary PPM (P6).
///
/// PPM keeps the verification artifacts viewable without an image dependency;
/// the tools also report the raw bytes and a pixel digest.
pub fn save_ppm(path: &Path, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let expected = (width as usize) * (height as usize) * 4;
    if rgba.len() != expected {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("expected {expected} RGBA bytes, got {}", rgba.len()),
        ));
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    write!(file, "P6\n{width} {height}\n255\n")?;
    let mut rgb = Vec::with_capacity((width as usize) * (height as usize) * 3);
    for pixel in rgba.chunks_exact(4) {
        rgb.extend_from_slice(&pixel[..3]);
    }
    file.write_all(&rgb)?;
    file.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ppm_rejects_mismatched_buffer() {
        let path = std::env::temp_dir().join("rkb-gpu-bad.ppm");
        assert!(save_ppm(&path, 2, 2, &[0; 4]).is_err());
    }

    #[test]
    fn ppm_writes_header_and_rgb_triplets() {
        let path = std::env::temp_dir().join("rkb-gpu-ok.ppm");
        let rgba = [10, 20, 30, 255, 40, 50, 60, 128];
        save_ppm(&path, 2, 1, &rgba).expect("write ppm");
        let bytes = std::fs::read(&path).expect("read ppm");
        let header = b"P6\n2 1\n255\n";
        assert_eq!(&bytes[..header.len()], header);
        assert_eq!(&bytes[header.len()..], &[10, 20, 30, 40, 50, 60]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn row_alignment_is_a_multiple_of_256() {
        let row = 3u32 * 4;
        let padded = row.div_ceil(COPY_ALIGN) * COPY_ALIGN;
        assert_eq!(padded % COPY_ALIGN, 0);
        assert!(padded >= row);
    }
}
