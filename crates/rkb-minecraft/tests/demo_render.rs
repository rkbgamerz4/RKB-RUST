//! End-to-end render of the synthetic demo island.
//!
//! This is the closest host-side equivalent of what Android presents: a real
//! `Runtime<MinecraftGame>` loads a generated 1.21 save, streams chunks, and
//! rasterizes frames through the same software backend.

use std::fs;
use std::time::Duration;

use rkb_core::{EngineConfig, Vec2};
use rkb_mc::fixture::TempDir;
use rkb_minecraft::demo_world::write_demo_world;
use rkb_minecraft::MinecraftGame;
use rkb_platform::HeadlessSurface;
use rkb_render::Color;
use rkb_runtime::{Runtime, RuntimeConfig};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;

#[test]
fn demo_world_renders_real_block_geometry() {
    let dir = TempDir::new("demo-render");
    let demo = write_demo_world(dir.path()).expect("write demo save");
    assert_eq!(demo.chunks, 49);

    let game = MinecraftGame::new(Some(dir.path().to_path_buf()));
    assert!(game.has_world(), "{}", game.status());

    let config = RuntimeConfig {
        engine: EngineConfig {
            max_delta_ms: 250,
            ..EngineConfig::default()
        },
        clear_color: Color::rgb(171, 208, 240),
        checksum_frames: true,
    };
    let mut runtime = Runtime::new(config, game, WIDTH, HEIGHT);
    runtime.start().unwrap();
    let mut surface = HeadlessSurface::new(WIDTH, HEIGHT);
    let mut last = None;
    for _ in 0..48 {
        last = Some(
            runtime
                .frame(Duration::from_micros(16_667), &mut surface)
                .unwrap(),
        );
    }
    let report = last.expect("frames rendered");
    assert!(report.ran);
    assert_eq!(report.frame_index, 48);

    let stats = runtime.game().stats();
    assert!(stats.chunks_loaded >= 40, "{stats:?}");
    assert!(stats.faces > 1_000, "{stats:?}");
    assert!(report.render.triangles > 100, "{:?}", report.render);

    // The sky and terrain must produce a non-trivial frame.
    let checksum = runtime.framebuffer().checksum();
    assert_ne!(checksum, 0);
    let pixels = runtime.framebuffer().pixels();
    let unique = pixels
        .chunks_exact(4)
        .map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .collect::<std::collections::BTreeSet<_>>();
    assert!(unique.len() > 16, "only {} colours in frame", unique.len());
    assert_eq!(
        runtime.game().viewport(),
        Vec2::new(WIDTH as f32, HEIGHT as f32)
    );

    if let Ok(path) = std::env::var("RKB_RENDER_OUT") {
        let file = fs::File::create(&path).expect("create PPM");
        let mut writer = std::io::BufWriter::new(file);
        runtime
            .framebuffer()
            .write_ppm(&mut writer)
            .expect("write PPM");
        println!("wrote {path}");
    }
}
