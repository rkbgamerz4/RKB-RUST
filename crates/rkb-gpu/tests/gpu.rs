//! GPU pipeline tests.
//!
//! These run the real wgpu pipeline (adapter, device, shaders, pipelines,
//! textures, buffers, depth, blending, culling) against whatever adapter the
//! machine exposes. On CI that is Mesa lavapipe; on a developer machine with a
//! GPU it is the vendor driver. The tests skip when no adapter exists unless
//! `RKB_GPU_REQUIRED=1`, which CI sets so a missing adapter is a failure rather
//! than a silent skip.
//!
//! The geometry here is deliberately synthetic: this file tests the GPU
//! plumbing. Real server chunks are exercised by the `rkb-join-gpu` tool.

use rkb_core::Vec3;
use rkb_gpu::{Fog, HeadlessRenderer};
use rkb_minecraft::{Camera, ChunkMesh, Face, RenderLayer};
use rkb_render::{Color, Texture};

fn backends() -> wgpu::Backends {
    wgpu::Backends::VULKAN | wgpu::Backends::GL
}

/// 4×1 atlas: red, green, fully transparent, half-transparent white.
fn test_atlas() -> Texture {
    Texture::from_rgba(
        4,
        1,
        vec![
            220, 30, 30, 255, // 0 red
            30, 220, 30, 255, // 1 green
            255, 255, 255, 0, // 2 transparent
            255, 255, 255, 128, // 3 half transparent
        ],
    )
}

fn uv_rect(id: u32) -> [f32; 4] {
    let step = 0.25;
    let start = step * id as f32;
    [start, 0.0, start + step, 1.0]
}

fn renderer(width: u32, height: u32) -> Option<HeadlessRenderer> {
    let atlas = test_atlas();
    match HeadlessRenderer::new(backends(), width, height, &atlas) {
        Ok(renderer) => Some(renderer),
        Err(error) => {
            if std::env::var_os("RKB_GPU_REQUIRED").is_some() {
                panic!("GPU required but initialization failed: {error}");
            }
            eprintln!("skipping GPU test: {error}");
            None
        }
    }
}

fn camera() -> Camera {
    let mut camera = Camera::new(Vec3::new(0.5, 0.5, 3.0));
    camera.fov_y = 70.0_f32.to_radians();
    camera
}

fn fog() -> Fog {
    // Far beyond the geometry, so fog never changes the expected colours.
    Fog::new(1000.0, 2000.0, Color::rgb(0, 0, 0))
}

fn quad(z: f32, texture: u32, tint: Color, layer: RenderLayer) -> Face {
    Face {
        points: [
            Vec3::new(0.0, 0.0, z),
            Vec3::new(1.0, 0.0, z),
            Vec3::new(1.0, 1.0, z),
            Vec3::new(0.0, 1.0, z),
        ],
        normal: Vec3::new(0.0, 0.0, 1.0),
        texture,
        tint,
        shade: 1.0,
        uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        light: [1.0, 1.0, 1.0, 1.0],
        color: tint,
        layer,
        double_sided: false,
    }
}

fn mesh(key: (i32, i32), faces: Vec<Face>) -> ChunkMesh {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for face in &faces {
        for point in face.points {
            min = Vec3::new(min.x.min(point.x), min.y.min(point.y), min.z.min(point.z));
            max = Vec3::new(max.x.max(point.x), max.y.max(point.y), max.z.max(point.z));
        }
    }
    if faces.is_empty() {
        min = Vec3::ZERO;
        max = Vec3::ZERO;
    }
    ChunkMesh {
        chunk: key,
        opaque: faces
            .iter()
            .copied()
            .filter(|face| face.layer == RenderLayer::Opaque)
            .collect(),
        cutout: faces
            .iter()
            .copied()
            .filter(|face| face.layer == RenderLayer::Cutout)
            .collect(),
        translucent: faces
            .iter()
            .copied()
            .filter(|face| face.layer == RenderLayer::Translucent)
            .collect(),
        min,
        max,
        non_air_blocks: 1,
        missing_models: 0,
    }
}

fn upload(renderer: &mut HeadlessRenderer, key: (i32, i32), faces: Vec<Face>) {
    let mesh = mesh(key, faces);
    renderer
        .scene_mut()
        .upload_chunk(&mesh, &uv_rect)
        .expect("upload chunk");
}

fn center(rgba: &[u8], width: u32, height: u32) -> [u8; 4] {
    let x = width / 2;
    let y = height / 2;
    let offset = ((y * width + x) * 4) as usize;
    [
        rgba[offset],
        rgba[offset + 1],
        rgba[offset + 2],
        rgba[offset + 3],
    ]
}

#[test]
fn adapter_initializes_and_reports_backend() {
    let Some(mut renderer) = renderer(32, 32) else {
        return;
    };
    let info = renderer.info();
    assert!(!info.name.is_empty(), "adapter name must be reported");
    assert!(
        info.backend == "Vulkan" || info.backend == "Gl",
        "unexpected backend {}",
        info.backend
    );
    assert_eq!(renderer.scene().atlas_size(), (4, 1));
    assert_eq!(renderer.scene().chunk_count(), 0);

    // The sky pipeline covers the whole target: zenith red at the top, fog
    // colour blue below the horizon.
    let mut camera = camera();
    camera.pitch = 0.0;
    let stats = renderer.render(
        &camera,
        Fog::new(1000.0, 2000.0, Color::rgb(0, 0, 255)),
        Color::rgb(255, 0, 0),
        &[],
    );
    assert_eq!(stats.draw_calls, 1, "sky is one fullscreen draw");
    assert_eq!(stats.triangles, 0);
    let pixels = renderer.read_rgba().expect("readback");
    let top = center(&pixels, 32, 2);
    let bottom = center(&pixels, 32, 32);
    assert!(
        top[0] > 200 && top[2] < 60,
        "top should be sky red: {top:?}"
    );
    assert!(
        bottom[2] > 200 && bottom[0] < 60,
        "bottom should be horizon blue: {bottom:?}"
    );
}

#[test]
fn atlas_texture_and_uv_mapping_reach_the_pixel() {
    let Some(mut renderer) = renderer(64, 64) else {
        return;
    };
    upload(
        &mut renderer,
        (0, 0),
        vec![quad(0.0, 0, Color::WHITE, RenderLayer::Opaque)],
    );
    let stats = renderer.render(&camera(), fog(), Color::BLACK, &[]);
    assert_eq!(stats.total_chunks, 1);
    assert_eq!(stats.visible_chunks, 1);
    assert_eq!(stats.opaque_draws, 1);
    assert_eq!(stats.triangles, 2);
    let pixels = renderer.read_rgba().expect("readback");
    let pixel = center(&pixels, 64, 64);
    assert!(
        pixel[0] > 180 && pixel[1] < 80 && pixel[2] < 80,
        "atlas tile 0 must sample red: {pixel:?}"
    );

    // Tile 1 is green: same geometry, different texture id and UV rectangle.
    renderer.scene_mut().remove_chunk((0, 0));
    upload(
        &mut renderer,
        (0, 0),
        vec![quad(0.0, 1, Color::WHITE, RenderLayer::Opaque)],
    );
    renderer.render(&camera(), fog(), Color::BLACK, &[]);
    let pixels = renderer.read_rgba().expect("readback");
    let pixel = center(&pixels, 64, 64);
    assert!(
        pixel[1] > 180 && pixel[0] < 80 && pixel[2] < 80,
        "atlas tile 1 must sample green: {pixel:?}"
    );
}

#[test]
fn depth_buffer_keeps_the_near_opaque_quad() {
    let Some(mut renderer) = renderer(64, 64) else {
        return;
    };
    // Far green first, near red second: the depth buffer must keep red.
    upload(
        &mut renderer,
        (0, 0),
        vec![quad(-2.0, 1, Color::WHITE, RenderLayer::Opaque)],
    );
    upload(
        &mut renderer,
        (1, 0),
        vec![quad(0.0, 0, Color::WHITE, RenderLayer::Opaque)],
    );
    let stats = renderer.render(&camera(), fog(), Color::BLACK, &[]);
    assert_eq!(stats.opaque_draws, 2);
    let pixels = renderer.read_rgba().expect("readback");
    let pixel = center(&pixels, 64, 64);
    assert!(
        pixel[0] > 180 && pixel[1] < 80,
        "near red must win the depth test: {pixel:?}"
    );
}

#[test]
fn cutout_discards_transparent_texels_and_translucent_blends() {
    let Some(mut renderer) = renderer(64, 64) else {
        return;
    };
    upload(
        &mut renderer,
        (0, 0),
        vec![quad(0.0, 0, Color::WHITE, RenderLayer::Opaque)],
    );

    // A cutout quad in front with a fully transparent tile must be discarded,
    // leaving the red block visible.
    upload(
        &mut renderer,
        (1, 0),
        vec![quad(0.5, 2, Color::WHITE, RenderLayer::Cutout)],
    );
    renderer.render(&camera(), fog(), Color::BLACK, &[]);
    let pixels = renderer.read_rgba().expect("readback");
    let pixel = center(&pixels, 64, 64);
    assert!(
        pixel[0] > 180 && pixel[1] < 80,
        "transparent cutout texels must discard: {pixel:?}"
    );

    // A half-transparent white quad over red must produce a pink blend.
    renderer.scene_mut().remove_chunk((1, 0));
    upload(
        &mut renderer,
        (2, 0),
        vec![quad(0.5, 3, Color::WHITE, RenderLayer::Translucent)],
    );
    let stats = renderer.render(&camera(), fog(), Color::BLACK, &[]);
    assert_eq!(stats.translucent_faces, 1);
    let pixels = renderer.read_rgba().expect("readback");
    let pixel = center(&pixels, 64, 64);
    assert!(
        pixel[0] > 200 && pixel[1] > 80 && pixel[1] < 200 && pixel[2] > 80 && pixel[2] < 200,
        "translucent white over red must blend: {pixel:?}"
    );
}

#[test]
fn chunk_upload_update_remove_and_resize() {
    let Some(mut renderer) = renderer(64, 64) else {
        return;
    };
    upload(
        &mut renderer,
        (3, 4),
        vec![quad(0.0, 0, Color::WHITE, RenderLayer::Opaque)],
    );
    assert_eq!(renderer.scene().chunk_count(), 1);
    let used = renderer.scene().gpu_bytes();
    assert!(used > 0, "chunk buffers count toward GPU bytes");

    let stats = renderer.render(&camera(), fog(), Color::BLACK, &[]);
    assert_eq!(stats.total_chunks, 1);
    assert_eq!(stats.triangles, 2);

    // Re-upload the same chunk with more geometry: capacity is reused and only
    // this chunk changes.
    let mesh = mesh(
        (3, 4),
        vec![
            quad(0.0, 0, Color::WHITE, RenderLayer::Opaque),
            quad(-0.5, 0, Color::WHITE, RenderLayer::Opaque),
        ],
    );
    let upload = renderer
        .scene_mut()
        .upload_chunk(&mesh, &uv_rect)
        .expect("re-upload");
    assert_eq!(upload.opaque_triangles, 4);
    assert!(upload.uploaded_bytes > 0);
    assert_eq!(renderer.scene().chunk_count(), 1);
    let stats = renderer.render(&camera(), fog(), Color::BLACK, &[]);
    assert_eq!(stats.triangles, 4);

    // Resize recreates the target and depth buffer without touching meshes.
    renderer.resize(100, 50).expect("resize");
    assert_eq!((renderer.width(), renderer.height()), (100, 50));
    renderer.render(&camera(), fog(), Color::BLACK, &[]);
    let pixels = renderer.read_rgba().expect("readback after resize");
    assert_eq!(pixels.len(), 100 * 50 * 4);
    assert_eq!(renderer.scene().chunk_count(), 1);

    let released = renderer.scene_mut().remove_chunk((3, 4));
    assert!(released > 0, "unload reports released bytes");
    assert_eq!(renderer.scene().chunk_count(), 0);
    assert!(renderer.scene().gpu_bytes() > 0, "atlas stays resident");
}

#[test]
fn frustum_culls_chunks_outside_the_view_and_camera_moves() {
    let Some(mut renderer) = renderer(64, 64) else {
        return;
    };
    upload(
        &mut renderer,
        (0, 0),
        vec![quad(0.0, 0, Color::WHITE, RenderLayer::Opaque)],
    );
    // Bounds far away from the camera: only the near chunk is in view.
    let mut far = mesh(
        (200, 0),
        vec![quad(0.0, 1, Color::WHITE, RenderLayer::Opaque)],
    );
    far.min = Vec3::new(2000.0, 0.0, 0.0);
    far.max = Vec3::new(2016.0, 16.0, 16.0);
    renderer
        .scene_mut()
        .upload_chunk(&far, &uv_rect)
        .expect("upload far chunk");

    let stats = renderer.render(&camera(), fog(), Color::BLACK, &[]);
    assert_eq!(stats.total_chunks, 2);
    assert_eq!(stats.visible_chunks, 1, "far chunk must be frustum-culled");
    assert_eq!(stats.opaque_draws, 1);

    // Turn around: the near chunk leaves the frustum too, so only sky remains.
    let mut behind = camera();
    behind.yaw = std::f32::consts::PI;
    let stats = renderer.render(&behind, fog(), Color::BLACK, &[]);
    assert_eq!(stats.visible_chunks, 0);
    assert_eq!(stats.triangles, 0);
    assert_eq!(stats.draw_calls, 1);
}

#[test]
fn atlas_reupload_keeps_the_scene_usable() {
    let Some(mut renderer) = renderer(32, 32) else {
        return;
    };
    upload(
        &mut renderer,
        (0, 0),
        vec![quad(0.0, 0, Color::WHITE, RenderLayer::Opaque)],
    );
    let replacement = Texture::from_rgba(1, 1, vec![10, 20, 200, 255]);
    renderer
        .scene_mut()
        .set_atlas(&replacement)
        .expect("atlas re-upload");
    assert_eq!(renderer.scene().atlas_size(), (1, 1));
    let stats = renderer.render(&camera(), fog(), Color::BLACK, &[]);
    assert_eq!(stats.total_chunks, 1);
    assert!(stats.triangles > 0);
}
