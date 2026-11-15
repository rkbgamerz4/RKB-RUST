//! Platform-independent Android engine wrapper.
//!
//! Kept free of JNI types so it can be unit-tested on the host.

use std::path::PathBuf;
use std::time::Duration;

use rkb_core::{Stage, Vec2};
use rkb_input::{TouchEvent, TouchPhase, TouchPoint};
use rkb_minecraft::MinecraftGame;
use rkb_platform::{Surface, SurfaceError};
use rkb_runtime::{FrameReport, Runtime, RuntimeConfig};

/// Outcome of a native frame request.
#[derive(Debug)]
pub enum EngineFrame {
    Presented(FrameReport),
    Skipped,
    Failed(String),
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
        }
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
            "stage={:?} frames={} fps={:.1} world={} chunks={}/{} faces={} tris={} pos=({:.1},{:.1},{:.1}) yaw={:.0} pitch={:.0} fly={} status={}",
            self.runtime.stage(),
            report.frame_index,
            report.fps,
            if stats.world_loaded { "loaded" } else { "unavailable" },
            stats.chunks_loaded,
            stats.chunks_loaded + stats.chunks_queued,
            stats.faces,
            stats.triangles,
            stats.position.x,
            stats.position.y,
            stats.position.z,
            stats.yaw.to_degrees(),
            stats.pitch.to_degrees(),
            stats.flying,
            game.status()
        )
    }

    /// One-line world state, logged at startup so CI can assert the client is
    /// actually running and what save it opened.
    #[must_use]
    pub fn status_text(&self) -> String {
        let game = self.runtime.game();
        format!(
            "world={} save={} status={}",
            if game.has_world() {
                "loaded"
            } else {
                "unavailable"
            },
            game.world_path()
                .map_or_else(|| "(none)".to_owned(), |path| path.display().to_string()),
            game.status()
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
