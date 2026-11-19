//! GPU scene: device initialization, pipelines, atlas, chunk buffers, and
//! per-frame submission for the M6 chunk meshes.
//!
//! The scene never decodes a chunk or a block. It consumes `ChunkMesh` values
//! produced by `rkb-minecraft` and the atlas rectangles the game reports, so
//! the GPU and software paths share models, light, and textures.

use std::collections::BTreeMap;

use rkb_core::{Rect, Vec2, Vec3};
use rkb_minecraft::{Camera, ChunkMesh, Frustum};
use rkb_render::{Color, DrawCommand, Texture};
use wgpu::util::DeviceExt;

use crate::mesh::{pack_chunk, GpuVertex, TranslucentQuad, WorldMesh};

/// Upper bound on resident chunk meshes; protects GPU memory from a runaway
/// server stream. The M6 client loads a 7×7 area, so this is generous.
pub const MAX_CHUNKS: usize = 4096;

/// Depth format used for all world rendering.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Fog parameters, matching the software renderer's distance fog.
#[derive(Clone, Copy, Debug)]
pub struct Fog {
    pub start: f32,
    pub end: f32,
    pub color: Color,
}

impl Fog {
    #[must_use]
    pub fn new(start: f32, end: f32, color: Color) -> Self {
        Self { start, end, color }
    }
}

/// A GPU error with the failing step named, for diagnostics and fallback
/// reporting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuError(pub String);

impl std::fmt::Display for GpuError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for GpuError {}

impl GpuError {
    fn new(stage: &str, detail: impl std::fmt::Display) -> Self {
        Self(format!("{stage}: {detail}"))
    }
}

/// Adapter identity reported by diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendInfo {
    pub name: String,
    pub backend: String,
    pub device_type: String,
    pub driver: String,
    pub driver_info: String,
}

/// Whether the adapter is silicon or a software implementation.
///
/// This exists so diagnostics can never call a CPU rasterizer a "GPU". The
/// classification combines wgpu's device type with known software adapter names
/// (`llvmpipe`, `lavapipe`, `SwiftShader`, `softpipe`, `Microsoft Basic
/// Render Driver`, `RadeonSI` software paths are not listed because those are
/// real drivers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceClass {
    /// A physical GPU (discrete or integrated).
    Hardware,
    /// A software rasterizer or CPU fallback (llvmpipe, SwiftShader, ...).
    Software,
    /// Reported as a GPU but the name is not recognised; treat as unknown.
    Unknown,
}

impl DeviceClass {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hardware => "hardware GPU",
            Self::Software => "software renderer",
            Self::Unknown => "unknown",
        }
    }

    #[must_use]
    pub const fn is_hardware(self) -> bool {
        matches!(self, Self::Hardware)
    }
}

impl BackendInfo {
    /// `gpu <name> (<backend>, <device type>)`, used in the HUD/CLI.
    #[must_use]
    pub fn label(&self) -> String {
        format!("gpu {} ({}, {})", self.name, self.backend, self.device_type)
    }

    /// Classifies the adapter as hardware or software.
    #[must_use]
    pub fn class(&self) -> DeviceClass {
        let name = self.name.to_ascii_lowercase();
        // Known software rasterizers first: llvmpipe and SwiftShader both
        // report odd device types on some drivers.
        if name.contains("llvmpipe")
            || name.contains("lavapipe")
            || name.contains("swiftshader")
            || name.contains("softpipe")
            || name.contains("software")
            || name.contains("basic render driver")
            || name.contains("microsoft basic")
        {
            return DeviceClass::Software;
        }
        match self.device_type.as_str() {
            "DiscreteGpu" | "IntegratedGpu" => DeviceClass::Hardware,
            "Cpu" | "VirtualGpu" => DeviceClass::Software,
            // "Other" or an unrecognised string.
            _ => DeviceClass::Unknown,
        }
    }

    /// `wgpu / Vulkan / Mali-G52 hardware GPU`, as required by M7 diagnostics.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "wgpu / {} / {} {}",
            self.backend,
            self.name,
            self.class().label()
        )
    }

    /// One-line summary with driver detail for logs.
    #[must_use]
    pub fn detail(&self) -> String {
        format!(
            "{} | device_type={} driver={} info={}",
            self.summary(),
            self.device_type,
            if self.driver.is_empty() {
                "(unknown)"
            } else {
                &self.driver
            },
            if self.driver_info.is_empty() {
                "(unknown)"
            } else {
                &self.driver_info
            }
        )
    }
}

/// wgpu instance, adapter, device, and queue.
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: std::sync::Arc<wgpu::Device>,
    pub queue: std::sync::Arc<wgpu::Queue>,
    pub info: BackendInfo,
}

impl std::fmt::Debug for GpuContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GpuContext")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

/// Creates the wgpu instance used by both the headless and Android paths.
#[must_use]
pub fn create_instance(backends: wgpu::Backends) -> wgpu::Instance {
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        ..Default::default()
    })
}

impl GpuContext {
    /// Headless context: no surface, suitable for offscreen rendering.
    pub fn new(backends: wgpu::Backends) -> Result<Self, GpuError> {
        let instance = create_instance(backends);
        Self::from_instance(instance, None)
    }

    /// Context selected for `surface` when one is supplied.
    pub fn from_instance(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Self, GpuError> {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: surface,
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| {
            GpuError::new(
                "adapter",
                format!(
                    "no adapter for backends {:?} (surface: {})",
                    instance.enumerate_adapters(wgpu::Backends::all()).len(),
                    surface.is_some()
                ),
            )
        })?;
        let raw = adapter.get_info();
        let info = BackendInfo {
            name: raw.name.clone(),
            backend: format!("{:?}", raw.backend),
            device_type: format!("{:?}", raw.device_type),
            driver: raw.driver.clone(),
            driver_info: raw.driver_info.clone(),
        };
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("rkb-gpu"),
                required_features: wgpu::Features::empty(),
                // Downlevel limits keep the same code valid on GLES 3 and on
                // every Vulkan 1.0 Android driver.
                required_limits: wgpu::Limits::downlevel_defaults(),
            },
            None,
        ))
        .map_err(|error| GpuError::new("device", error))?;
        Ok(Self {
            instance,
            adapter,
            device: std::sync::Arc::new(device),
            queue: std::sync::Arc::new(queue),
            info,
        })
    }
}

/// Per-frame counters, printed by the verification tool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameStats {
    pub visible_chunks: u32,
    pub total_chunks: u32,
    pub opaque_draws: u32,
    pub cutout_draws: u32,
    pub translucent_faces: u32,
    pub draw_calls: u32,
    pub triangles: u32,
    pub ui_triangles: u32,
}

/// Result of uploading one chunk mesh.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UploadStats {
    pub opaque_triangles: u32,
    pub cutout_triangles: u32,
    pub translucent_faces: u32,
    pub uploaded_bytes: u64,
}

struct LayerGpu {
    vertex: wgpu::Buffer,
    index: wgpu::Buffer,
    index_count: u32,
    vertex_capacity: u64,
    index_capacity: u64,
}

struct ChunkGpu {
    min: Vec3,
    max: Vec3,
    opaque: Option<LayerGpu>,
    cutout: Option<LayerGpu>,
    translucent: Vec<TranslucentQuad>,
}

struct Pipelines {
    opaque: wgpu::RenderPipeline,
    cutout: wgpu::RenderPipeline,
    translucent: wgpu::RenderPipeline,
    sky: wgpu::RenderPipeline,
    ui: wgpu::RenderPipeline,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    fog: [f32; 4],
    fog_end: [f32; 4],
    viewport: [f32; 4],
    sky_top: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UiVertex {
    position: [f32; 2],
    color: [f32; 4],
}

struct Transient {
    vertex: wgpu::Buffer,
    index: wgpu::Buffer,
    vertex_capacity: u64,
    index_capacity: u64,
}

/// The GPU renderer for one viewport. Owns no window; callers supply a color
/// view (headless texture or swapchain image) to [`Scene::encode`].
pub struct Scene {
    device: std::sync::Arc<wgpu::Device>,
    queue: std::sync::Arc<wgpu::Queue>,
    color_format: wgpu::TextureFormat,
    srgb_out: bool,
    world_layout: wgpu::BindGroupLayout,
    camera_buffer: wgpu::Buffer,
    camera_bind: wgpu::BindGroup,
    // Kept alive for the lifetime of the bind group/view that reference it.
    #[allow(dead_code)]
    atlas: wgpu::Texture,
    atlas_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    atlas_size: (u32, u32),
    pipelines: Pipelines,
    // Kept alive for the lifetime of `depth_view`.
    #[allow(dead_code)]
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    width: u32,
    height: u32,
    chunks: BTreeMap<(i32, i32), ChunkGpu>,
    translucent: Option<Transient>,
    ui: Option<Transient>,
    atlas_bytes: u64,
}

const WORLD_ATTRIBUTES: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32, 3 => Float32x3];
const UI_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4];

impl std::fmt::Debug for Scene {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Scene")
            .field("color_format", &self.color_format)
            .field("size", &(self.width, self.height))
            .field("chunks", &self.chunks.len())
            .finish_non_exhaustive()
    }
}

impl Scene {
    /// Builds pipelines and uploads the initial atlas.
    pub fn new(
        context: &GpuContext,
        color_format: wgpu::TextureFormat,
        atlas: &Texture,
    ) -> Result<Self, GpuError> {
        let device = std::sync::Arc::clone(&context.device);
        let queue = std::sync::Arc::clone(&context.queue);
        let srgb_out = color_format.is_srgb();

        let world_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rkb-world-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rkb-camera"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rkb-atlas-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let (atlas_texture, atlas_view) = upload_atlas(&device, &queue, atlas)?;
        let atlas_size = (atlas.width(), atlas.height());
        let camera_bind = make_bind_group(
            &device,
            &world_layout,
            &camera_buffer,
            &atlas_view,
            &sampler,
        );

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rkb-world.wgsl"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(crate::WORLD_WGSL)),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rkb-pipeline-layout"),
            bind_group_layouts: &[&world_layout],
            push_constant_ranges: &[],
        });
        let pipelines = create_pipelines(&device, &pipeline_layout, &shader, color_format);

        let (depth, depth_view) = create_depth(&device, 1, 1);

        Ok(Self {
            device,
            queue,
            color_format,
            srgb_out,
            world_layout,
            camera_buffer,
            camera_bind,
            atlas: atlas_texture,
            atlas_view,
            sampler,
            atlas_size,
            pipelines,
            depth,
            depth_view,
            width: 1,
            height: 1,
            chunks: BTreeMap::new(),
            translucent: None,
            ui: None,
            atlas_bytes: (atlas_size.0 as u64) * (atlas_size.1 as u64) * 4,
        })
    }

    /// Device handle, for creating per-frame command encoders.
    #[must_use]
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// Queue handle, for submission.
    #[must_use]
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    #[must_use]
    pub fn color_format(&self) -> wgpu::TextureFormat {
        self.color_format
    }

    #[must_use]
    pub fn atlas_size(&self) -> (u32, u32) {
        self.atlas_size
    }

    #[must_use]
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Re-uploads the atlas and rewrites its bind group. Every vertex UV may
    /// have changed, so the caller must re-upload resident meshes afterwards.
    pub fn set_atlas(&mut self, atlas: &Texture) -> Result<(), GpuError> {
        let (texture, view) = upload_atlas(&self.device, &self.queue, atlas)?;
        self.atlas = texture;
        self.atlas_view = view;
        self.atlas_size = (atlas.width(), atlas.height());
        self.atlas_bytes = (atlas.width() as u64) * (atlas.height() as u64) * 4;
        self.camera_bind = make_bind_group(
            &self.device,
            &self.world_layout,
            &self.camera_buffer,
            &self.atlas_view,
            &self.sampler,
        );
        Ok(())
    }

    /// Resizes the depth target; the swapchain is reconfigured by the caller.
    pub fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if width == self.width && height == self.height {
            return;
        }
        self.width = width;
        self.height = height;
        let (depth, view) = create_depth(&self.device, width, height);
        self.depth = depth;
        self.depth_view = view;
    }

    /// Uploads (or re-uploads) one chunk mesh, reusing buffer capacity.
    pub fn upload_chunk(
        &mut self,
        mesh: &ChunkMesh,
        uv_rect: &dyn Fn(u32) -> [f32; 4],
    ) -> Result<UploadStats, GpuError> {
        let key = mesh.chunk;
        if !self.chunks.contains_key(&key) && self.chunks.len() >= MAX_CHUNKS {
            return Err(GpuError(format!(
                "chunk budget exceeded ({MAX_CHUNKS} resident meshes)"
            )));
        }
        let packed = pack_chunk(mesh, uv_rect);
        let mut existing = self.chunks.remove(&key);
        let mut uploaded = 0_u64;
        let old_opaque = existing.as_mut().and_then(|chunk| chunk.opaque.take());
        let old_cutout = existing.as_mut().and_then(|chunk| chunk.cutout.take());

        let opaque = self.upload_layer(packed.opaque, old_opaque, &mut uploaded);
        let cutout = self.upload_layer(packed.cutout, old_cutout, &mut uploaded);

        let stats = UploadStats {
            opaque_triangles: opaque.as_ref().map_or(0, |layer| layer.index_count / 3),
            cutout_triangles: cutout.as_ref().map_or(0, |layer| layer.index_count / 3),
            translucent_faces: packed.translucent.len() as u32,
            uploaded_bytes: uploaded,
        };
        self.chunks.insert(
            key,
            ChunkGpu {
                min: mesh.min,
                max: mesh.max,
                opaque,
                cutout,
                translucent: packed.translucent,
            },
        );
        Ok(stats)
    }

    fn upload_layer(
        &self,
        mesh: WorldMesh,
        existing: Option<LayerGpu>,
        uploaded: &mut u64,
    ) -> Option<LayerGpu> {
        if mesh.is_empty() {
            return None;
        }
        let vertex_bytes = mesh.vertex_bytes();
        let index_bytes = mesh.index_bytes();
        let vertices = bytemuck::cast_slice(&mesh.vertices);
        let indices = bytemuck::cast_slice(&mesh.indices);
        match existing {
            Some(mut layer)
                if layer.vertex_capacity >= vertex_bytes && layer.index_capacity >= index_bytes =>
            {
                self.queue.write_buffer(&layer.vertex, 0, vertices);
                self.queue.write_buffer(&layer.index, 0, indices);
                layer.index_count = mesh.indices.len() as u32;
                *uploaded += vertex_bytes + index_bytes;
                Some(layer)
            }
            _ => {
                let vertex = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("rkb-chunk-vertices"),
                        contents: vertices,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    });
                let index = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("rkb-chunk-indices"),
                        contents: indices,
                        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                    });
                *uploaded += vertex_bytes + index_bytes;
                Some(LayerGpu {
                    vertex,
                    index,
                    index_count: mesh.indices.len() as u32,
                    vertex_capacity: vertex_bytes,
                    index_capacity: index_bytes,
                })
            }
        }
    }

    /// Drops one chunk's buffers. Returns the bytes released.
    pub fn remove_chunk(&mut self, key: (i32, i32)) -> u64 {
        self.chunks.remove(&key).map_or(0, |chunk| {
            let mut bytes = 0_u64;
            for layer in [chunk.opaque, chunk.cutout].into_iter().flatten() {
                bytes += layer.vertex_capacity + layer.index_capacity;
            }
            bytes
        })
    }

    #[must_use]
    pub fn gpu_bytes(&self) -> u64 {
        let mut bytes = self.atlas_bytes;
        for chunk in self.chunks.values() {
            for layer in [chunk.opaque.as_ref(), chunk.cutout.as_ref()]
                .into_iter()
                .flatten()
            {
                bytes += layer.vertex_capacity + layer.index_capacity;
            }
            bytes += (chunk.translucent.len() * std::mem::size_of::<TranslucentQuad>()) as u64;
        }
        bytes
    }

    /// Records one frame into `encoder` targeting `color_view`.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        color_view: &wgpu::TextureView,
        camera: &Camera,
        viewport: Vec2,
        fog: Fog,
        sky_top: Color,
        ui: &[DrawCommand],
    ) -> FrameStats {
        self.resize(viewport.x.max(1.0) as u32, viewport.y.max(1.0) as u32);
        self.write_camera(camera, viewport, fog, sky_top);

        let frustum = Frustum::from_view_projection(camera.view_projection(viewport));
        let eye = camera.position;

        let mut visible = Vec::new();
        for (key, chunk) in &self.chunks {
            if frustum.contains_aabb(chunk.min, chunk.max) {
                visible.push(*key);
            }
        }

        // Gather and globally sort translucent faces, matching M6's ordering.
        let mut quads: Vec<(f32, &TranslucentQuad)> = Vec::new();
        for key in &visible {
            if let Some(chunk) = self.chunks.get(key) {
                for quad in &chunk.translucent {
                    if quad.is_visible_from(eye) {
                        quads.push(((quad.center() - eye).length_squared(), quad));
                    }
                }
            }
        }
        quads.sort_by(|a, b| b.0.total_cmp(&a.0));

        let mut stats = FrameStats {
            visible_chunks: visible.len() as u32,
            total_chunks: self.chunks.len() as u32,
            translucent_faces: quads.len() as u32,
            ..FrameStats::default()
        };

        let translucent_count =
            upload_translucent(&self.device, &self.queue, &mut self.translucent, &quads);
        let ui_count = upload_ui(&self.device, &self.queue, &mut self.ui, ui);

        let color = wgpu::Color {
            r: f64::from(fog.color.r) / 255.0,
            g: f64::from(fog.color.g) / 255.0,
            b: f64::from(fog.color.b) / 255.0,
            a: 1.0,
        };
        let depth_view = &self.depth_view;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("rkb-frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(color),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_bind_group(0, &self.camera_bind, &[]);

        pass.set_pipeline(&self.pipelines.sky);
        pass.draw(0..3, 0..1);
        stats.draw_calls += 1;

        // Opaque and cutout share the bind group and only differ by pipeline.
        pass.set_pipeline(&self.pipelines.opaque);
        for key in &visible {
            if let Some(chunk) = self.chunks.get(key) {
                if let Some(layer) = &chunk.opaque {
                    draw_layer(&mut pass, layer);
                    stats.opaque_draws += 1;
                    stats.draw_calls += 1;
                    stats.triangles += layer.index_count / 3;
                }
            }
        }
        pass.set_pipeline(&self.pipelines.cutout);
        for key in &visible {
            if let Some(chunk) = self.chunks.get(key) {
                if let Some(layer) = &chunk.cutout {
                    draw_layer(&mut pass, layer);
                    stats.cutout_draws += 1;
                    stats.draw_calls += 1;
                    stats.triangles += layer.index_count / 3;
                }
            }
        }

        if translucent_count > 0 {
            if let Some(transient) = &self.translucent {
                pass.set_pipeline(&self.pipelines.translucent);
                pass.set_vertex_buffer(0, transient.vertex.slice(..));
                pass.set_index_buffer(transient.index.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..translucent_count, 0, 0..1);
                stats.draw_calls += 1;
                stats.triangles += translucent_count / 3;
            }
        }

        if ui_count > 0 {
            if let Some(buffers) = &self.ui {
                pass.set_pipeline(&self.pipelines.ui);
                pass.set_vertex_buffer(0, buffers.vertex.slice(..));
                pass.draw(0..ui_count, 0..1);
                stats.draw_calls += 1;
                stats.ui_triangles = ui_count / 3;
            }
        }

        stats
    }

    fn write_camera(&self, camera: &Camera, viewport: Vec2, fog: Fog, sky_top: Color) {
        let matrix = camera.view_projection(viewport);
        let uniform = CameraUniform {
            view_proj: matrix.cols,
            eye: [camera.position.x, camera.position.y, camera.position.z, 1.0],
            fog: [
                f32::from(fog.color.r) / 255.0,
                f32::from(fog.color.g) / 255.0,
                f32::from(fog.color.b) / 255.0,
                fog.start,
            ],
            fog_end: [fog.end, if self.srgb_out { 1.0 } else { 0.0 }, 0.0, 0.0],
            viewport: [viewport.x, viewport.y, 0.0, 0.0],
            sky_top: [
                f32::from(sky_top.r) / 255.0,
                f32::from(sky_top.g) / 255.0,
                f32::from(sky_top.b) / 255.0,
                0.0,
            ],
        };
        self.queue
            .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&uniform));
    }
}

/// Uploads sorted translucent faces; returns the index count.
fn upload_translucent(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    slot: &mut Option<Transient>,
    quads: &[(f32, &TranslucentQuad)],
) -> u32 {
    if quads.is_empty() {
        return 0;
    }
    let mut vertices = Vec::with_capacity(quads.len() * 4);
    let mut indices = Vec::with_capacity(quads.len() * 6);
    for (_, quad) in quads {
        let base = vertices.len() as u32;
        for corner in 0..4 {
            vertices.push(GpuVertex {
                position: [
                    quad.points[corner].x,
                    quad.points[corner].y,
                    quad.points[corner].z,
                ],
                uv: quad.uv[corner],
                light: quad.light[corner],
                tint: quad.tint,
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    let vertex_bytes = bytemuck::cast_slice::<GpuVertex, u8>(&vertices);
    let index_bytes = bytemuck::cast_slice::<u32, u8>(&indices);
    ensure_transient(device, slot, vertex_bytes, index_bytes, "translucent");
    if let Some(buffers) = slot.as_ref() {
        queue.write_buffer(&buffers.vertex, 0, vertex_bytes);
        queue.write_buffer(&buffers.index, 0, index_bytes);
    }
    (quads.len() * 6) as u32
}

/// Converts the game's existing HUD commands into one GPU vertex batch.
fn upload_ui(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    slot: &mut Option<Transient>,
    commands: &[DrawCommand],
) -> u32 {
    let vertices = ui_geometry(commands);
    if vertices.is_empty() {
        return 0;
    }
    let bytes = bytemuck::cast_slice::<UiVertex, u8>(&vertices);
    ensure_transient(device, slot, bytes, &[], "ui");
    if let Some(buffers) = slot.as_ref() {
        queue.write_buffer(&buffers.vertex, 0, bytes);
    }
    vertices.len() as u32
}

fn draw_layer<'a>(pass: &mut wgpu::RenderPass<'a>, layer: &'a LayerGpu) {
    pass.set_vertex_buffer(0, layer.vertex.slice(..));
    pass.set_index_buffer(layer.index.slice(..), wgpu::IndexFormat::Uint32);
    pass.draw_indexed(0..layer.index_count, 0, 0..1);
}

fn ensure_transient(
    device: &wgpu::Device,
    slot: &mut Option<Transient>,
    vertex_bytes: &[u8],
    index_bytes: &[u8],
    label: &str,
) {
    let vertex_need = vertex_bytes.len().max(1) as u64;
    let index_need = index_bytes.len().max(1) as u64;
    let recreate = match slot.as_ref() {
        None => true,
        Some(existing) => {
            existing.vertex_capacity < vertex_need || existing.index_capacity < index_need
        }
    };
    if recreate {
        let vertex_capacity = grow(vertex_need);
        let index_capacity = grow(index_need);
        *slot = Some(Transient {
            vertex: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(&format!("rkb-{label}-vertices")),
                size: vertex_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            index: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(&format!("rkb-{label}-indices")),
                size: index_capacity,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            vertex_capacity,
            index_capacity,
        });
    }
}

fn grow(needed: u64) -> u64 {
    const MIN: u64 = 4096;
    let mut capacity = MIN;
    while capacity < needed {
        capacity = capacity.saturating_mul(2);
    }
    capacity
}

fn create_depth(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rkb-depth"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn upload_atlas(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &Texture,
) -> Result<(wgpu::Texture, wgpu::TextureView), GpuError> {
    let width = atlas.width();
    let height = atlas.height();
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rkb-atlas"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // Raw sRGB bytes, sampled without a transfer conversion. This is what
        // makes the GPU frames comparable with the software renderer's frames.
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        atlas.pixels(),
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Ok((texture, view))
}

fn make_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    camera: &wgpu::Buffer,
    atlas: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("rkb-world-bind"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: camera.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(atlas),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

fn create_pipelines(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> Pipelines {
    let world = [wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<GpuVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &WORLD_ATTRIBUTES,
    }];
    let ui = [wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<UiVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &UI_ATTRIBUTES,
    }];
    let depth = |write: bool, compare: wgpu::CompareFunction| {
        Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: write,
            depth_compare: compare,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        })
    };
    let target = |blend: Option<wgpu::BlendState>| {
        [Some(wgpu::ColorTargetState {
            format,
            blend,
            write_mask: wgpu::ColorWrites::ALL,
        })]
    };
    let primitive = wgpu::PrimitiveState {
        topology: wgpu::PrimitiveTopology::TriangleList,
        front_face: wgpu::FrontFace::Ccw,
        cull_mode: None,
        ..Default::default()
    };
    let make = |label: &str,
                vs: &str,
                fs: &str,
                buffers: &[wgpu::VertexBufferLayout<'_>],
                depth: Option<wgpu::DepthStencilState>,
                targets: &[Option<wgpu::ColorTargetState>]| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: vs,
                buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: fs,
                targets,
            }),
            primitive,
            depth_stencil: depth,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        })
    };

    let opaque = make(
        "rkb-opaque",
        "vs_world",
        "fs_opaque",
        &world,
        depth(true, wgpu::CompareFunction::Less),
        &target(None),
    );
    let cutout = make(
        "rkb-cutout",
        "vs_world",
        "fs_cutout",
        &world,
        depth(true, wgpu::CompareFunction::Less),
        &target(None),
    );
    let translucent = make(
        "rkb-translucent",
        "vs_world",
        "fs_translucent",
        &world,
        depth(false, wgpu::CompareFunction::Less),
        &target(Some(wgpu::BlendState::ALPHA_BLENDING)),
    );
    let sky = make(
        "rkb-sky",
        "vs_sky",
        "fs_sky",
        &[],
        depth(false, wgpu::CompareFunction::Always),
        &target(None),
    );
    let ui = make(
        "rkb-ui",
        "vs_ui",
        "fs_ui",
        &ui,
        depth(false, wgpu::CompareFunction::Always),
        &target(Some(wgpu::BlendState::ALPHA_BLENDING)),
    );
    Pipelines {
        opaque,
        cutout,
        translucent,
        sky,
        ui,
    }
}

fn color_array(color: Color) -> [f32; 4] {
    [
        f32::from(color.r) / 255.0,
        f32::from(color.g) / 255.0,
        f32::from(color.b) / 255.0,
        f32::from(color.a) / 255.0,
    ]
}

fn push_rect(vertices: &mut Vec<UiVertex>, rect: Rect, color: Color) {
    let [r, g, b, a] = color_array(color);
    let x0 = rect.x;
    let y0 = rect.y;
    let x1 = rect.x + rect.width;
    let y1 = rect.y + rect.height;
    let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    for index in [0, 1, 2, 0, 2, 3] {
        vertices.push(UiVertex {
            position: corners[index],
            color: [r, g, b, a],
        });
    }
}

/// Converts 2D HUD commands into a triangle batch. 3D and textured commands
/// are not part of the HUD and are ignored.
#[must_use]
pub fn ui_geometry(commands: &[DrawCommand]) -> Vec<UiVertex> {
    let mut vertices = Vec::new();
    for command in commands {
        match *command {
            DrawCommand::Rect { rect, color } => push_rect(&mut vertices, rect, color),
            DrawCommand::StrokeRect {
                rect,
                thickness,
                color,
            } => {
                let t = thickness.max(0.5);
                push_rect(
                    &mut vertices,
                    Rect::new(rect.x, rect.y, rect.width, t),
                    color,
                );
                push_rect(
                    &mut vertices,
                    Rect::new(rect.x, rect.y + rect.height - t, rect.width, t),
                    color,
                );
                push_rect(
                    &mut vertices,
                    Rect::new(rect.x, rect.y, t, rect.height),
                    color,
                );
                push_rect(
                    &mut vertices,
                    Rect::new(rect.x + rect.width - t, rect.y, t, rect.height),
                    color,
                );
            }
            DrawCommand::Line { from, to, color } => {
                let dx = to.x - from.x;
                let dy = to.y - from.y;
                let length = (dx * dx + dy * dy).sqrt();
                if length < 1e-4 {
                    continue;
                }
                let nx = -dy / length * 0.5;
                let ny = dx / length * 0.5;
                let [r, g, b, a] = color_array(color);
                let points = [
                    [from.x + nx, from.y + ny],
                    [to.x + nx, to.y + ny],
                    [to.x - nx, to.y - ny],
                    [from.x - nx, from.y - ny],
                ];
                for index in [0, 1, 2, 0, 2, 3] {
                    vertices.push(UiVertex {
                        position: points[index],
                        color: [r, g, b, a],
                    });
                }
            }
            DrawCommand::Circle {
                center,
                radius,
                color,
            } => {
                const SEGMENTS: usize = 24;
                let [r, g, b, a] = color_array(color);
                for segment in 0..SEGMENTS {
                    let a0 = segment as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
                    let a1 = (segment + 1) as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
                    for point in [
                        [center.x, center.y],
                        [center.x + radius * a0.cos(), center.y + radius * a0.sin()],
                        [center.x + radius * a1.cos(), center.y + radius * a1.sin()],
                    ] {
                        vertices.push(UiVertex {
                            position: point,
                            color: [r, g, b, a],
                        });
                    }
                }
            }
            DrawCommand::Triangle { a, b, c, color } => {
                let [red, green, blue, alpha] = color_array(color);
                for point in [a, b, c] {
                    vertices.push(UiVertex {
                        position: [point.x, point.y],
                        color: [red, green, blue, alpha],
                    });
                }
            }
            DrawCommand::Triangle3D { .. }
            | DrawCommand::TexturedTriangle3D { .. }
            | DrawCommand::GradientTriangle { .. } => {}
        }
    }
    vertices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_becomes_two_triangles() {
        let vertices = ui_geometry(&[DrawCommand::Rect {
            rect: Rect::new(10.0, 20.0, 100.0, 50.0),
            color: Color::rgba(255, 0, 0, 255),
        }]);
        assert_eq!(vertices.len(), 6);
        assert_eq!(vertices[0].position, [10.0, 20.0]);
        assert_eq!(vertices[2].position, [110.0, 70.0]);
        assert_eq!(vertices[0].color, [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn circle_becomes_a_fan() {
        let vertices = ui_geometry(&[DrawCommand::Circle {
            center: Vec2::new(50.0, 50.0),
            radius: 10.0,
            color: Color::WHITE,
        }]);
        assert_eq!(vertices.len(), 24 * 3);
    }

    #[test]
    fn three_d_commands_are_not_part_of_the_hud() {
        let vertices = ui_geometry(&[DrawCommand::Triangle3D {
            a: rkb_render::Vertex3D::new(0.0, 0.0, 0.0, 1.0),
            b: rkb_render::Vertex3D::new(1.0, 0.0, 0.0, 1.0),
            c: rkb_render::Vertex3D::new(0.0, 1.0, 0.0, 1.0),
            color: Color::WHITE,
        }]);
        assert!(vertices.is_empty());
    }

    #[test]
    fn camera_uniform_is_the_size_wgsl_expects() {
        // 3 mat4 rows + 5 vec4s, std140-compatible.
        assert_eq!(std::mem::size_of::<CameraUniform>(), 64 + 5 * 16);
        assert_eq!(std::mem::size_of::<CameraUniform>() % 16, 0);
    }

    #[test]
    fn buffer_growth_is_monotonic_and_bounded() {
        assert_eq!(grow(1), 4096);
        assert_eq!(grow(4096), 4096);
        assert_eq!(grow(4097), 8192);
        assert_eq!(grow(1 << 20), 1 << 20);
    }
}
