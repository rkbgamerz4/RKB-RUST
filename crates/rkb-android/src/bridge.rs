//! Platform-independent Android engine wrapper.
//!
//! Kept free of JNI types so it can be unit-tested on the host.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Duration;

use rkb_core::{Stage, Vec2};
use rkb_gpu::{BackendInfo, Fog, FrameStats, GpuContext, SceneSync, SurfaceRenderer, SyncTiming};
use rkb_input::{TouchEvent, TouchPhase, TouchPoint};
use rkb_mc::BlockRegistry;
use rkb_minecraft::MinecraftGame;
use rkb_net::ServerWorld;
use rkb_platform::{Surface, SurfaceError};
use rkb_runtime::{FrameReport, Runtime, RuntimeConfig};

/// Outcome of a native frame request.
#[derive(Debug)]
pub enum EngineFrame {
    Presented(FrameReport),
    Skipped,
    Failed(String),
}

/// Chunk meshes uploaded per frame on the GPU path, matching the client's
/// meshing budget so uploads stay bounded.
const GPU_UPLOAD_BUDGET: usize = 2;

/// Everything owned by the GPU presentation path.
///
/// Dropped as one unit on detach: the swapchain and its `ANativeWindow` must
/// not outlive the window, so [`AndroidEngine::detach_gpu`] returns the raw
/// window pointer *after* this value is dropped for the caller to release.
#[derive(Debug)]
pub struct GpuState {
    renderer: SurfaceRenderer,
    sync: SceneSync,
    /// Raw `ANativeWindow*` borrowed from the Java `Surface`.
    window: usize,
    width: u32,
    height: u32,
    info: BackendInfo,
    last_stats: FrameStats,
    frames: u64,
    frame_nanos: u128,
    upload_nanos: u128,
    upload_count: u64,
    uploaded_bytes: u64,
    init_nanos: u128,
}

impl GpuState {
    /// Diagnostic line: adapter identity plus cumulative upload counters.
    fn status(&self) -> String {
        format!(
            "{} | adapter={} | init_ms={:.1} frames={} gpu_ms/frame={:.2} uploads={} upload_bytes={} gpu_buffers={} chunks={} draws={} tris={}",
            self.info.detail(),
            self.info.class().label(),
            self.init_nanos as f64 / 1_000_000.0,
            self.frames,
            if self.frames > 0 {
                self.frame_nanos as f64 / 1_000_000.0 / self.frames as f64
            } else {
                0.0
            },
            self.upload_count,
            self.uploaded_bytes,
            self.renderer.scene().gpu_bytes(),
            self.renderer.scene().chunk_count(),
            self.last_stats.draw_calls,
            self.last_stats.triangles,
        )
    }
}

/// Surface that presents directly into a Kotlin-owned direct ByteBuffer.
#[derive(Debug)]
pub struct DirectBufferSurface<'a> {
    pixels: &'a mut [u8],
    width: u32,
    height: u32,
}

impl<'a> DirectBufferSurface<'a> {
    pub fn new(pixels: &'a mut [u8], width: u32, height: u32) -> Result<Self, &'static str> {
        if width == 0 || height == 0 {
            return Err("surface dimensions must be non-zero");
        }
        let expected = frame_bytes(width, height).ok_or("surface dimensions overflow")?;
        if pixels.len() != expected {
            return Err("surface buffer size does not match dimensions");
        }
        Ok(Self {
            pixels,
            width,
            height,
        })
    }
}

impl Surface for DirectBufferSurface<'_> {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn present(&mut self, rgba: &[u8]) -> Result<(), SurfaceError> {
        if rgba.len() != self.pixels.len() {
            return Err(SurfaceError("frame size mismatch".to_owned()));
        }
        self.pixels.copy_from_slice(rgba);
        Ok(())
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }
}

/// Owns the runtime and converts JNI calls into engine operations.
#[derive(Debug)]
pub struct AndroidEngine {
    runtime: Runtime<MinecraftGame>,
    width: u32,
    height: u32,
    destroyed: bool,
    join: Option<Receiver<Result<ServerWorld, String>>>,
    join_status: Option<String>,
    /// Pack to apply once the join completes, opened off the network thread.
    join_resource_pack: Option<PathBuf>,
    /// Present when a native surface is attached and the GPU initialized.
    gpu: Option<GpuState>,
    /// Exact GPU initialization or frame failure, reported verbatim.
    gpu_error: Option<String>,
}

impl AndroidEngine {
    pub fn new(width: u32, height: u32, world: Option<PathBuf>) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let config = RuntimeConfig {
            engine: rkb_core::EngineConfig::default(),
            // Sky horizon; the client paints its own gradient over it.
            clear_color: rkb_render::Color::rgb(171, 208, 240),
            // Android copies pixels into a direct buffer; hashing the whole
            // framebuffer every frame would double memory traffic for no host
            // benefit.
            checksum_frames: false,
        };
        let mut runtime = Runtime::new(config, MinecraftGame::new(world), width, height);
        let _ = runtime.start();
        Self {
            runtime,
            width,
            height,
            destroyed: false,
            join: None,
            join_status: None,
            join_resource_pack: None,
            gpu: None,
            gpu_error: None,
        }
    }

    /// Starts an offline-mode server join on a background thread.
    ///
    /// The render thread keeps running; [`Self::frame`] swaps in the world when
    /// the connection is ready. Login and the initial chunk burst never block
    /// the Android UI. `resource_pack` is an optional user-supplied pack
    /// directory or `.zip`; it is opened only after the join succeeds, and a
    /// bad path is reported through [`Self::join_status`] without aborting
    /// the join.
    pub fn begin_join(
        &mut self,
        server: String,
        username: String,
        registry_path: PathBuf,
        resource_pack: Option<PathBuf>,
    ) -> Result<(), String> {
        if self.join.is_some() {
            return Err("a server join is already in progress".to_owned());
        }
        if server.trim().is_empty() {
            return Err("server address is empty".to_owned());
        }
        if username.trim().is_empty() {
            return Err("player name is empty".to_owned());
        }
        let (sender, receiver) = mpsc::channel();
        self.join_status = Some(format!("connecting to {server}"));
        self.join_resource_pack = resource_pack;
        self.join = Some(receiver);
        thread::spawn(move || {
            let result = BlockRegistry::from_file(&registry_path)
                .map_err(|error| format!("block report {}: {error}", registry_path.display()))
                .and_then(|registry| {
                    ServerWorld::connect(&server, &username, Duration::from_secs(20), registry)
                        .map_err(|error| format!("join failed: {error}"))
                });
            let _ = sender.send(result);
        });
        Ok(())
    }

    /// Promotes a completed background join and reports its status.
    fn poll_join(&mut self) {
        let Some(receiver) = self.join.as_ref() else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(server)) => {
                self.join = None;
                self.join_status = None;
                self.runtime.game_mut().load_server(server);
                if let Some(path) = self.join_resource_pack.take() {
                    if let Err(error) = self.apply_resource_pack(&path) {
                        // A bad pack must not undo a successful join.
                        self.join_status = Some(format!("resource pack: {error}"));
                    }
                }
            }
            Ok(Err(error)) => {
                self.join = None;
                self.join_status = Some(error);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.join = None;
                self.join_status = Some("join thread stopped unexpectedly".to_owned());
            }
        }
    }

    /// True while a background server join is still connecting.
    #[must_use]
    pub fn join_in_progress(&self) -> bool {
        self.join.is_some()
    }

    /// Swaps in a user-supplied resource pack and re-meshes the loaded area.
    ///
    /// The pack is whatever the user pointed at; nothing is bundled. Returns
    /// the pack summary for the HUD.
    fn apply_resource_pack(&mut self, path: &Path) -> Result<String, String> {
        self.runtime
            .game_mut()
            .load_resource_pack(path)
            .map_err(|error| format!("{}: {error}", path.display()))
    }

    /// The latest join status, including failures.
    #[must_use]
    pub fn join_status(&self) -> Option<&str> {
        self.join_status.as_deref()
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
        self.runtime.resize(self.width, self.height);
    }

    pub fn touch(
        &mut self,
        phase: TouchPhase,
        pointer_id: u64,
        x: f32,
        y: f32,
        timestamp: Duration,
    ) {
        let point = TouchPoint::new(pointer_id, Vec2::new(x, y)).at_time(timestamp);
        self.runtime.touch(TouchEvent::new(phase, point));
    }

    pub fn frame(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        delta: Duration,
    ) -> EngineFrame {
        if self.destroyed {
            return EngineFrame::Failed("engine already destroyed".to_owned());
        }
        self.poll_join();
        if width != self.width || height != self.height {
            self.resize(width, height);
        }
        let Some(expected) = frame_bytes(width, height) else {
            return EngineFrame::Failed("frame dimensions overflow".to_owned());
        };
        if pixels.len() < expected {
            return EngineFrame::Failed(format!(
                "frame buffer too small: {} < {expected}",
                pixels.len()
            ));
        }
        let mut surface = match DirectBufferSurface::new(&mut pixels[..expected], width, height) {
            Ok(surface) => surface,
            Err(error) => return EngineFrame::Failed(error.to_owned()),
        };
        match self.runtime.frame(delta, &mut surface) {
            Ok(report) if report.ran => EngineFrame::Presented(report),
            Ok(_) => EngineFrame::Skipped,
            Err(error) => EngineFrame::Failed(error.to_string()),
        }
    }

    /// Attaches a native window and initializes the GPU renderer.
    ///
    /// `window` is the raw `ANativeWindow` handle built by the JNI layer from
    /// the Java `Surface`; the caller keeps that window alive until
    /// [`Self::detach_gpu`] returns. On success the returned string is the
    /// adapter summary (for logs and the overlay); on failure the exact reason
    /// is stored and reported by [`Self::renderer_status`].
    pub fn attach_gpu(
        &mut self,
        window: rkb_gpu::rwh::RawWindowHandle,
        window_ptr: usize,
        width: u32,
        height: u32,
    ) -> Result<String, String> {
        if self.destroyed {
            return Err("engine already destroyed".to_owned());
        }
        // Replacing a surface must drop the old swapchain first.
        drop(self.gpu.take());
        let width = width.max(1);
        let height = height.max(1);
        let started = std::time::Instant::now();

        let outcome = (|| -> Result<(SurfaceRenderer, BackendInfo), String> {
            let instance = rkb_gpu::create_instance(rkb_gpu::default_backends());
            let display =
                rkb_gpu::rwh::RawDisplayHandle::Android(rkb_gpu::rwh::AndroidDisplayHandle::new());
            let surface = rkb_gpu::create_surface(&instance, window, display)
                .map_err(|error| error.to_string())?;
            let context = GpuContext::from_instance(instance, Some(&surface))
                .map_err(|error| error.to_string())?;
            let atlas = self.runtime.game().atlas_texture();
            let renderer = SurfaceRenderer::new(&context, surface, width, height, &atlas)
                .map_err(|error| error.to_string())?;
            Ok((renderer, context.info))
        })();

        match outcome {
            Ok((renderer, info)) => {
                let summary = info.summary();
                let format = renderer.format();
                self.gpu_error = None;
                // The HUD, touch, and fog layout must match the swapchain, not
                // the bitmap path's scaled buffer.
                self.runtime.resize(width, height);
                self.gpu = Some(GpuState {
                    renderer,
                    sync: SceneSync::new(GPU_UPLOAD_BUDGET),
                    window: window_ptr,
                    width,
                    height,
                    info: info.clone(),
                    last_stats: FrameStats::default(),
                    frames: 0,
                    frame_nanos: 0,
                    upload_nanos: 0,
                    upload_count: 0,
                    uploaded_bytes: 0,
                    init_nanos: started.elapsed().as_nanos(),
                });
                Ok(format!(
                    "{summary} | init {:.1} ms | {:?} {}x{}",
                    started.elapsed().as_secs_f64() * 1000.0,
                    format,
                    width,
                    height
                ))
            }
            Err(error) => {
                let detail = format!("gpu init failed: {error}");
                self.gpu_error = Some(detail.clone());
                Err(detail)
            }
        }
    }

    /// Detaches the GPU renderer and returns the raw window to release.
    ///
    /// The surface and swapchain are dropped before the pointer is handed back,
    /// so the caller can release its `ANativeWindow` reference safely.
    pub fn detach_gpu(&mut self) -> Option<usize> {
        let state = self.gpu.take()?;
        let window = state.window;
        drop(state);
        Some(window)
    }

    /// True while a native surface is attached to the GPU renderer.
    #[must_use]
    pub fn gpu_attached(&self) -> bool {
        self.gpu.is_some()
    }

    /// The active renderer and, if the GPU failed, the exact reason.
    #[must_use]
    pub fn renderer_status(&self) -> String {
        match (&self.gpu, &self.gpu_error) {
            (Some(state), _) => format!("renderer=gpu {}", state.status()),
            (None, Some(error)) => format!("renderer=software ({error})"),
            (None, None) => "renderer=software (gpu not initialized)".to_owned(),
        }
    }

    /// Adapter summary of the current GPU, if one is attached.
    #[must_use]
    pub fn gpu_adapter(&self) -> Option<&BackendInfo> {
        self.gpu.as_ref().map(|state| &state.info)
    }

    /// Advances the game and renders one frame into the native swapchain.
    ///
    /// Networking and pack work stay where M5/M6 put them: `poll_join` only
    /// checks a channel, and mesh upload is bounded by [`GPU_UPLOAD_BUDGET`].
    pub fn frame_gpu(&mut self, width: u32, height: u32, delta: Duration) -> EngineFrame {
        if self.destroyed {
            return EngineFrame::Failed("engine already destroyed".to_owned());
        }
        let width = width.max(1);
        let height = height.max(1);
        self.poll_join();

        let attached = self.gpu.is_some();
        if !attached {
            return EngineFrame::Failed(
                self.gpu_error
                    .clone()
                    .unwrap_or_else(|| "gpu surface not attached".to_owned()),
            );
        }

        // Rotation and surface recreation resize the swapchain before drawing.
        let resized = self
            .gpu
            .as_ref()
            .is_some_and(|state| state.width != width || state.height != height);
        if resized {
            if let Some(state) = self.gpu.as_mut() {
                state.width = width;
                state.height = height;
                state.renderer.resize(width, height);
            }
            self.runtime.resize(width, height);
        }

        let report = match self.runtime.advance(delta) {
            Ok(report) => report,
            Err(error) => return EngineFrame::Failed(error.to_string()),
        };
        if !report.ran {
            return EngineFrame::Skipped;
        }

        let Some(state) = self.gpu.as_mut() else {
            return EngineFrame::Failed("gpu surface detached mid-frame".to_owned());
        };
        let game = self.runtime.game();
        let timing = match SyncTiming::measure(&mut state.sync, state.renderer.scene_mut(), game) {
            Ok(timing) => timing,
            Err(error) => {
                let detail = format!("gpu upload failed: {error}");
                self.gpu_error = Some(detail.clone());
                return EngineFrame::Failed(detail);
            }
        };
        state.upload_nanos += u128::from(timing.nanos);
        state.upload_count += timing.stats.uploaded as u64;
        state.uploaded_bytes += timing.stats.uploaded_bytes;

        let camera = game.camera();
        let (fog_start, fog_end) = game.fog_range();
        let (sky_top, fog_color) = game.sky_colors();
        let ui = game.hud_commands();
        let frame_started = std::time::Instant::now();
        let rendered = state
            .renderer
            .render(
                &camera,
                Fog::new(fog_start, fog_end, fog_color),
                sky_top,
                &ui,
            )
            .map_err(|error| error.to_string());
        match rendered {
            Ok(stats) => {
                state.last_stats = stats;
                state.frames += 1;
                state.frame_nanos += frame_started.elapsed().as_nanos();
                EngineFrame::Presented(report)
            }
            Err(error) => {
                let detail = format!("gpu frame failed: {error}");
                self.gpu_error = Some(detail.clone());
                EngineFrame::Failed(detail)
            }
        }
    }

    pub fn pause(&mut self) -> Result<Stage, String> {
        self.runtime.pause().map_err(|error| error.to_string())
    }

    pub fn resume(&mut self) -> Result<Stage, String> {
        self.runtime.resume().map_err(|error| error.to_string())
    }

    pub fn destroy(&mut self) {
        if !self.destroyed {
            let _ = self.runtime.destroy();
            self.destroyed = true;
        }
    }

    #[must_use]
    pub fn stats_text(&self) -> String {
        let game = self.runtime.game();
        let stats = game.stats();
        let report = self.runtime.last_report();
        format!(
            "stage={:?} frames={} fps={:.1} world={} kind={} chunks={}/{} visible={} faces={} tris={} blocks={} edits={} pos=({:.1},{:.1},{:.1}) yaw={:.0} pitch={:.0} fly={} save={} join={} status={} pack={} {}",
            self.runtime.stage(),
            report.frame_index,
            report.fps,
            if stats.world_loaded { "loaded" } else { "unavailable" },
            game.world_kind().unwrap_or("none"),
            stats.chunks_loaded,
            stats.chunks_generated,
            stats.chunks_visible,
            stats.faces,
            stats.triangles,
            stats.blocks,
            stats.edits,
            stats.position.x,
            stats.position.y,
            stats.position.z,
            stats.yaw.to_degrees(),
            stats.pitch.to_degrees(),
            stats.flying,
            game.save_status(),
            self.join_status.as_deref().unwrap_or("idle"),
            game.status(),
            game.resource_pack_status(),
            // Active backend: "renderer=gpu wgpu / Vulkan / llvmpipe …
            // software renderer" or "renderer=software (<exact GPU error>)".
            self.renderer_status()
        )
    }

    /// One-line world state, logged at startup so CI can assert the client is
    /// actually running and what world it opened.
    #[must_use]
    pub fn status_text(&self) -> String {
        let game = self.runtime.game();
        format!(
            "world={} save={} status={} save_status={}",
            if game.has_world() {
                "loaded"
            } else {
                "unavailable"
            },
            game.world_path()
                .map_or_else(|| "(none)".to_owned(), |path| path.display().to_string()),
            game.status(),
            game.save_status()
        )
    }
}

/// Number of RGBA8 bytes for a frame, or `None` on overflow.
#[must_use]
pub fn frame_bytes(width: u32, height: u32) -> Option<usize> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
}

/// Maps Android `MotionEvent` actions to touch phases.
///
/// Pointer index bits are masked off, so `ACTION_POINTER_DOWN | index` works.
#[must_use]
pub const fn touch_phase(action: i32) -> Option<TouchPhase> {
    match action & 0xff {
        0 | 5 => Some(TouchPhase::Started),
        1 | 6 => Some(TouchPhase::Ended),
        2 | 7 => Some(TouchPhase::Moved),
        3 => Some(TouchPhase::Cancelled),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_buffer_surface_copies_frame() {
        let mut pixels = vec![0_u8; 8];
        {
            let mut surface = DirectBufferSurface::new(&mut pixels, 2, 1).unwrap();
            assert_eq!(surface.size(), (2, 1));
            surface.present(&[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
            assert_eq!(&*surface.pixels, &[1, 2, 3, 4, 5, 6, 7, 8]);
            assert!(surface.present(&[0; 4]).is_err());
        }
        assert_eq!(pixels, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn frame_size_checks_overflow() {
        assert_eq!(frame_bytes(2, 3), Some(24));
        assert_eq!(frame_bytes(u32::MAX, u32::MAX), None);
    }

    #[test]
    fn maps_motion_event_actions() {
        assert_eq!(touch_phase(0), Some(TouchPhase::Started));
        assert_eq!(touch_phase(5 << 8), Some(TouchPhase::Started));
        assert_eq!(touch_phase(1), Some(TouchPhase::Ended));
        assert_eq!(touch_phase((6 << 8) | 1), Some(TouchPhase::Ended));
        assert_eq!(touch_phase(2), Some(TouchPhase::Moved));
        assert_eq!(touch_phase(3), Some(TouchPhase::Cancelled));
        assert_eq!(touch_phase(4), None);
    }

    #[test]
    fn failed_server_join_is_reported_without_blocking() {
        let mut engine = AndroidEngine::new(64, 36, None);
        engine
            .begin_join(
                "127.0.0.1:1".to_owned(),
                "Tester".to_owned(),
                std::path::PathBuf::from("/nonexistent/blocks.json"),
                None,
            )
            .unwrap();
        assert!(engine.join_in_progress());
        let mut pixels = vec![0_u8; 64 * 36 * 4];
        for _ in 0..200 {
            engine.frame(&mut pixels, 64, 36, Duration::from_millis(16));
            if !engine.join_in_progress() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!engine.join_in_progress());
        let status = engine.join_status().expect("join failure status");
        assert!(status.contains("blocks.json"), "{status}");
        assert!(
            engine.stats_text().contains("join=block report"),
            "{}",
            engine.stats_text()
        );
    }

    #[test]
    fn engine_reports_minecraft_state_instead_of_sandbox_state() {
        let mut engine = AndroidEngine::new(160, 90, None);
        let mut pixels = vec![0_u8; 160 * 90 * 4];
        engine.frame(&mut pixels, 160, 90, Duration::from_millis(16));
        let stats = engine.stats_text();
        assert!(stats.contains("world=unavailable"), "{stats}");
        assert!(stats.contains("No Minecraft world"), "{stats}");
        assert!(!stats.contains("balls="), "{stats}");
        assert!(engine.status_text().contains("world=unavailable"));
    }

    #[test]
    fn engine_loads_a_supplied_save() {
        let dir = rkb_mc::fixture::TempDir::new("android-world");
        rkb_minecraft::demo_world::write_demo_world(dir.path()).unwrap();
        let mut engine = AndroidEngine::new(160, 90, Some(dir.path().to_path_buf()));
        let mut pixels = vec![0_u8; 160 * 90 * 4];
        assert!(matches!(
            engine.frame(&mut pixels, 160, 90, Duration::from_millis(16)),
            EngineFrame::Presented(_)
        ));
        let stats = engine.stats_text();
        assert!(stats.contains("world=loaded"), "{stats}");
        assert!(stats.contains("RKB Demo Island"), "{stats}");
        assert!(engine.status_text().contains("world=loaded"));
    }

    #[test]
    fn resource_pack_load_is_reported_and_leaves_the_engine_running() {
        let mut engine = AndroidEngine::new(160, 90, None);

        let missing = engine
            .apply_resource_pack(Path::new("/nonexistent/rkb-pack"))
            .unwrap_err();
        assert!(
            missing.contains("neither a directory nor a .zip"),
            "{missing}"
        );

        let dir = rkb_mc::fixture::TempDir::new("android-pack");
        std::fs::write(
            dir.path().join("pack.mcmeta"),
            r#"{"pack":{"pack_format":34,"description":"RKB android fixture"}}"#,
        )
        .unwrap();
        let summary = engine.apply_resource_pack(dir.path()).unwrap();
        assert!(summary.contains("textures: pack"), "{summary}");
        assert!(summary.contains("format 34"), "{summary}");

        let mut pixels = vec![0_u8; 160 * 90 * 4];
        assert!(matches!(
            engine.frame(&mut pixels, 160, 90, Duration::from_millis(16)),
            EngineFrame::Presented(_) | EngineFrame::Skipped
        ));
        let stats = engine.stats_text();
        assert!(stats.contains("textures: pack"), "{stats}");
    }

    #[test]
    fn engine_generates_terrain_when_no_save_exists() {
        let dir = rkb_mc::fixture::TempDir::new("android-generated");
        let mut engine = AndroidEngine::new(160, 90, Some(dir.path().to_path_buf()));
        let mut pixels = vec![0_u8; 160 * 90 * 4];
        for _ in 0..120 {
            assert!(matches!(
                engine.frame(&mut pixels, 160, 90, Duration::from_millis(16)),
                EngineFrame::Presented(_)
            ));
            // Give the background generator real time between updates.
            std::thread::sleep(Duration::from_millis(2));
        }
        let stats = engine.stats_text();
        assert!(stats.contains("world=loaded"), "{stats}");
        assert!(stats.contains("kind=generated"), "{stats}");
        assert!(stats.contains("Generated world seed"), "{stats}");
        let chunks: usize = stats
            .split("chunks=")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        assert!(chunks >= 1, "{stats}");
        let faces: usize = stats
            .split("faces=")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        assert!(faces > 0, "{stats}");
        assert!(engine.status_text().contains("world=loaded"));
        assert!(dir.path().join("level.dat").is_file());
        assert!(dir.path().join("rkb-session.json").is_file());
    }

    #[test]
    fn engine_renders_and_handles_lifecycle() {
        let mut engine = AndroidEngine::new(160, 90, None);
        let mut pixels = vec![0_u8; 160 * 90 * 4];
        let frame = engine.frame(&mut pixels, 160, 90, Duration::from_millis(16));
        assert!(matches!(frame, EngineFrame::Presented(_)));
        assert!(pixels.iter().any(|byte| *byte != 0));
        assert!(engine.stats_text().contains("stage=Running"));

        engine.pause().unwrap();
        assert!(matches!(
            engine.frame(&mut pixels, 160, 90, Duration::from_millis(16)),
            EngineFrame::Skipped
        ));
        engine.resume().unwrap();
        assert!(matches!(
            engine.frame(&mut pixels, 160, 90, Duration::from_millis(16)),
            EngineFrame::Presented(_)
        ));
        engine.destroy();
        assert!(matches!(
            engine.frame(&mut pixels, 160, 90, Duration::from_millis(16)),
            EngineFrame::Failed(_)
        ));
    }

    #[test]
    fn engine_rejects_short_buffers() {
        let mut engine = AndroidEngine::new(64, 64, None);
        let mut pixels = vec![0_u8; 16];
        assert!(matches!(
            engine.frame(&mut pixels, 64, 64, Duration::from_millis(16)),
            EngineFrame::Failed(_)
        ));
    }

    #[test]
    fn engine_resizes_when_frame_dimensions_change() {
        let mut engine = AndroidEngine::new(320, 180, None);
        let mut pixels = vec![0_u8; 640 * 360 * 4];
        let frame = engine.frame(&mut pixels, 640, 360, Duration::from_millis(16));
        assert!(matches!(frame, EngineFrame::Presented(_)));
        let mut again = vec![0_u8; 640 * 360 * 4];
        assert!(matches!(
            engine.frame(&mut again, 640, 360, Duration::from_millis(16)),
            EngineFrame::Presented(_)
        ));
    }

    #[test]
    fn engine_accepts_oversized_direct_buffers() {
        let mut engine = AndroidEngine::new(32, 16, None);
        let mut pixels = vec![0_u8; 32 * 16 * 4 + 64];
        assert!(matches!(
            engine.frame(&mut pixels, 32, 16, Duration::from_millis(16)),
            EngineFrame::Presented(_)
        ));
        assert!(pixels[..32 * 16 * 4].iter().any(|byte| *byte != 0));
    }

    #[test]
    fn engine_destroy_is_idempotent() {
        let mut engine = AndroidEngine::new(16, 16, None);
        engine.destroy();
        engine.destroy();
        let mut pixels = vec![0_u8; 16 * 16 * 4];
        assert!(matches!(
            engine.frame(&mut pixels, 16, 16, Duration::from_millis(16)),
            EngineFrame::Failed(_)
        ));
    }
}
