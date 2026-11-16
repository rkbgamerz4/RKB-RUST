//! End-to-end render of the default generated world with no user save.
//!
//! This is the host-side equivalent of a fresh Android install: the game is
//! given an empty world directory, must generate terrain itself, stream chunks
//! from the background worker, and rasterize real block geometry.

use std::fs;
use std::time::Duration;

use rkb_core::EngineConfig;
use rkb_mc::fixture::TempDir;
use rkb_minecraft::MinecraftGame;
use rkb_platform::HeadlessSurface;
use rkb_render::Color;
use rkb_runtime::{Runtime, RuntimeConfig};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;

#[test]
fn generated_world_renders_without_any_save() {
    let dir = TempDir::new("generated-render");
    let game = MinecraftGame::new(Some(dir.path().to_path_buf()));
    assert!(game.has_world(), "{}", game.status());
    assert_eq!(game.world_kind(), Some("generated"));
    assert!(dir.path().join("level.dat").is_file());
    assert!(dir.path().join("rkb-session.json").is_file());

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
    for _ in 0..600 {
        last = Some(
            runtime
                .frame(Duration::from_micros(16_667), &mut surface)
                .unwrap(),
        );
        if runtime.game().stats().chunks_loaded >= 9 {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let report = last.expect("frames rendered");
    assert!(report.ran);

    let stats = runtime.game().stats();
    assert!(stats.chunks_loaded >= 9, "{stats:?}");
    assert!(stats.chunks_generated >= 9, "{stats:?}");
    assert!(stats.faces > 1_000, "{stats:?}");
    assert!(report.render.triangles > 100, "{:?}", report.render);
    assert!(stats.chunks_visible > 0, "{stats:?}");
    assert!(runtime.game().save_status().contains("saved"), "{stats:?}");

    let checksum = runtime.framebuffer().checksum();
    assert_ne!(checksum, 0);
    let unique = runtime
        .framebuffer()
        .pixels()
        .chunks_exact(4)
        .map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        unique.len() > 128,
        "texture detail produced only {} colours",
        unique.len()
    );

    // Snow terrain must cover a meaningful part of the frame, and no pixel
    // should be untextured black.
    let mut snowy = 0_usize;
    let mut black = 0_usize;
    for pixel in runtime.framebuffer().pixels().chunks_exact(4) {
        let (r, g, b) = (
            i32::from(pixel[0]),
            i32::from(pixel[1]),
            i32::from(pixel[2]),
        );
        if r > 200 && g > 210 && b > 210 {
            snowy += 1;
        }
        if r + g + b < 12 {
            black += 1;
        }
    }
    assert!(snowy > 1_000, "only {snowy} snow pixels");
    assert_eq!(black, 0, "{black} black pixels in the frame");

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
