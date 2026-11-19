//! `rkb-join-gpu`: join an offline-mode 1.21.1 server and render its world on
//! the GPU backend.
//!
//! This is the GPU counterpart of `rkb-join`. It drives the same
//! `MinecraftGame` and uploads the same M6 chunk meshes, but draws through
//! `rkb-gpu` instead of the software rasterizer. Server chunks only: if the
//! server sends no geometry the tool fails rather than generating terrain.
//!
//! Usage:
//! `rkb-join-gpu --server <host[:port]> --block-report <blocks.json>
//! [--name <name>] [--seconds <n>] [--out <frame.ppm>] [--width <px>]
//! [--height <px>] [--hold <n>] [--resource-pack <dir|zip>] [--fov <deg>]
//! [--yaw <deg>] [--pitch <deg>] [--backend vulkan|gl|all] [--time-days 1]`

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use rkb_gpu::{save_ppm, Fog, HeadlessRenderer, SceneSync, SyncTiming, MAX_CHUNKS};
use rkb_mc::BlockRegistry;
use rkb_minecraft::MinecraftGame;
use rkb_net::ServerWorld;
use rkb_runtime::{Game, UpdateContext};

const DEFAULT_NAME: &str = "RKBJoinGPU";
const DEFAULT_SECONDS: u64 = 30;
const DEFAULT_WIDTH: u32 = 480;
const DEFAULT_HEIGHT: u32 = 270;
/// Same per-frame meshing budget as the client, so the tool reports the
/// bounded upload pattern the APK will actually see.
const MESH_BUDGET: usize = 2;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("rkb-join-gpu: {message}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum BackendChoice {
    Vulkan,
    Gl,
    All,
}

impl BackendChoice {
    fn backends(self) -> wgpu::Backends {
        match self {
            Self::Vulkan => wgpu::Backends::VULKAN,
            Self::Gl => wgpu::Backends::GL,
            Self::All => wgpu::Backends::VULKAN | wgpu::Backends::GL,
        }
    }
}

struct Args {
    server: String,
    block_report: PathBuf,
    name: String,
    seconds: u64,
    out: Option<PathBuf>,
    width: u32,
    height: u32,
    hold: u64,
    resource_pack: Option<PathBuf>,
    fov: Option<f32>,
    yaw: Option<f32>,
    pitch: Option<f32>,
    backend: BackendChoice,
    stats_every: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut server = None;
    let mut block_report = None;
    let mut name = DEFAULT_NAME.to_owned();
    let mut seconds = DEFAULT_SECONDS;
    let mut out = None;
    let mut width = DEFAULT_WIDTH;
    let mut height = DEFAULT_HEIGHT;
    let mut hold = 0;
    let mut resource_pack = None;
    let mut fov = None;
    let mut yaw = None;
    let mut pitch = None;
    let mut backend = BackendChoice::All;
    let mut stats_every = 60;
    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        let mut value = |flag: &str| args.next().ok_or_else(|| format!("{flag} expects a value"));
        let number = |flag: &str, raw: String| -> Result<f32, String> {
            raw.parse()
                .map_err(|_| format!("{flag} expects a number, got {raw:?}"))
        };
        match argument.as_str() {
            "--server" => server = Some(value("--server")?),
            "--block-report" => block_report = Some(PathBuf::from(value("--block-report")?)),
            "--name" => name = value("--name")?,
            "--seconds" => {
                seconds = value("--seconds")?
                    .parse()
                    .map_err(|_| "--seconds expects a number".to_owned())?;
            }
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--width" => {
                width = value("--width")?
                    .parse()
                    .map_err(|_| "--width expects a number".to_owned())?;
            }
            "--height" => {
                height = value("--height")?
                    .parse()
                    .map_err(|_| "--height expects a number".to_owned())?;
            }
            "--hold" => {
                hold = value("--hold")?
                    .parse()
                    .map_err(|_| "--hold expects seconds".to_owned())?;
            }
            "--resource-pack" => {
                resource_pack = Some(PathBuf::from(value("--resource-pack")?));
            }
            "--fov" => {
                let raw = value("--fov")?;
                fov = Some(number("--fov", raw)?);
            }
            "--yaw" => {
                let raw = value("--yaw")?;
                yaw = Some(number("--yaw", raw)?);
            }
            "--pitch" => {
                let raw = value("--pitch")?;
                pitch = Some(number("--pitch", raw)?);
            }
            "--backend" => {
                backend = match value("--backend")?.as_str() {
                    "vulkan" | "vk" => BackendChoice::Vulkan,
                    "gl" | "gles" | "opengl" => BackendChoice::Gl,
                    "all" => BackendChoice::All,
                    other => {
                        return Err(format!(
                            "--backend expects vulkan, gl, or all, got {other:?}"
                        ))
                    }
                };
            }
            "--stats-every" => {
                stats_every = value("--stats-every")?
                    .parse()
                    .map_err(|_| "--stats-every expects a number".to_owned())?;
            }
            "--help" | "-h" => {
                println!(
                    "usage: rkb-join-gpu --server <host[:port]> --block-report <blocks.json> \
                     [--name <name>] [--seconds <n>] [--out <frame.ppm>] \
                     [--width <px>] [--height <px>] [--hold <n>] \
                     [--resource-pack <dir|zip>] [--fov <deg>] [--yaw <deg>] \
                     [--pitch <deg>] [--backend vulkan|gl|all] [--stats-every <n>]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    if seconds == 0 || width == 0 || height == 0 {
        return Err("--seconds, --width, and --height must be positive".to_owned());
    }
    Ok(Args {
        server: server.ok_or("--server is required")?,
        block_report: block_report
            .ok_or("--block-report is required; RKB bundles no block registry")?,
        name,
        seconds,
        out,
        width,
        height,
        hold,
        resource_pack,
        fov,
        yaw,
        pitch,
        backend,
        stats_every: stats_every.max(1),
    })
}

fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse().ok())
    })
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let registry = BlockRegistry::from_file(&args.block_report)
        .map_err(|error| format!("block report {}: {error}", args.block_report.display()))?;
    println!(
        "block registry: {} states from {}",
        registry.len(),
        args.block_report.display()
    );

    let server = ServerWorld::connect(&args.server, &args.name, Duration::from_secs(20), registry)
        .map_err(|error| format!("join failed: {error}"))?;
    println!(
        "joined: position {}",
        server
            .position()
            .map_or("unknown".to_owned(), |position| format!(
                "({:.2}, {:.2}, {:.2})",
                position.x, position.y, position.z
            ))
    );

    let mut game = MinecraftGame::join_server(server);
    println!(
        "world: {} | {}",
        game.world_kind().unwrap_or("none"),
        game.status()
    );
    match &args.resource_pack {
        Some(path) => {
            let summary = game
                .load_resource_pack(path)
                .map_err(|error| format!("resource pack {}: {error}", path.display()))?;
            println!("resource pack: {summary}");
        }
        None => println!("resource pack: {}", game.resource_pack_status()),
    }
    if let Some(fov) = args.fov {
        game.set_fov_degrees(fov);
    }
    if let Some(yaw) = args.yaw {
        let pitch = game.player().pitch;
        game.set_look(yaw.to_radians(), pitch);
    }
    if let Some(pitch) = args.pitch {
        let yaw = game.player().yaw;
        game.set_look(yaw, pitch.to_radians());
    }

    // GPU initialization is reported with the exact stage and adapter. There is
    // no fallback inside this tool: a verification run must fail, not pretend
    // the software path was the GPU.
    let init_started = Instant::now();
    let mut renderer = HeadlessRenderer::new(
        args.backend.backends(),
        args.width,
        args.height,
        &game.atlas_texture(),
    )
    .map_err(|error| format!("GPU init failed: {error}"))?;
    let init_time = init_started.elapsed();
    println!(
        "gpu: {} | driver {} ({}) | init {:.1} ms | target {:?} {}x{}",
        renderer.info().label(),
        renderer.info().driver,
        renderer.info().driver_info,
        init_time.as_secs_f64() * 1000.0,
        renderer.scene().color_format(),
        renderer.width(),
        renderer.height()
    );

    let viewport = rkb_core::Vec2::new(args.width as f32, args.height as f32);
    Game::on_resize(&mut game, viewport);
    Game::on_start(&mut game);

    let mut sync = SceneSync::new(MESH_BUDGET);
    let mut uploaded_chunks = 0_u64;
    let mut uploaded_bytes = 0_u64;
    let mut upload_nanos = 0_u128;
    let mut atlas_uploads = 0_u64;
    let mut max_chunks = 0_usize;

    let started = Instant::now();
    let deadline = started + Duration::from_secs(args.seconds);
    let mut frame = 0_u64;
    let mut frame_nanos = 0_u128;
    // Only a fallback if the loop body never runs; every iteration overwrites it.
    #[allow(unused_assignments)]
    let mut last_stats = rkb_gpu::FrameStats::default();
    let mut last_game = game.stats();

    loop {
        let context = UpdateContext {
            frame,
            step: 0,
            dt: 1.0 / 60.0,
            elapsed: started.elapsed(),
            viewport,
        };
        game.update(context);

        // `SceneSync` applies atlas changes, new/changed revisions, and removals
        // with a bounded per-frame upload budget.
        let timing = SyncTiming::measure(&mut sync, renderer.scene_mut(), &game)
            .map_err(|error| format!("GPU sync: {error}"))?;
        if timing.stats.atlas_uploads > 0 {
            atlas_uploads += u64::from(timing.stats.atlas_uploads);
            let atlas = game.atlas_texture();
            println!(
                "atlas: re-uploaded {}x{} (generation {})",
                atlas.width(),
                atlas.height(),
                game.atlas_generation()
            );
        }
        let uploads = timing.stats.uploaded;
        upload_nanos += u128::from(timing.nanos);
        uploaded_bytes += timing.stats.uploaded_bytes;
        uploaded_chunks += timing.stats.uploaded as u64;
        if renderer.scene().chunk_count() > MAX_CHUNKS {
            return Err(format!(
                "GPU chunk budget exceeded: {} > {MAX_CHUNKS}",
                renderer.scene().chunk_count()
            ));
        }
        max_chunks = max_chunks.max(renderer.scene().chunk_count());

        let camera = game.camera();
        let (fog_start, fog_end) = game.fog_range();
        let (sky_top, fog_color) = game.sky_colors();
        let ui = game.hud_commands();
        let render_started = Instant::now();
        let stats = renderer.render(
            &camera,
            Fog::new(fog_start, fog_end, fog_color),
            sky_top,
            &ui,
        );
        last_stats = stats;
        // Readback makes the frame real GPU output and forces completion, so the
        // timing below includes GPU work (the APK presents instead of reading).
        let _ = renderer
            .read_rgba()
            .map_err(|error| format!("frame readback: {error}"))?;
        frame_nanos += render_started.elapsed().as_nanos();
        frame += 1;

        if frame % args.stats_every == 0 {
            last_game = game.stats();
            println!(
                "frame {frame}: gpu chunks {} ({} visible), draws {}, tris {}, translucent {}, \
                 ui tris {} | game meshes {} / {} visible | uploads {uploads} in {:.2} ms",
                stats.total_chunks,
                stats.visible_chunks,
                stats.draw_calls,
                stats.triangles,
                stats.translucent_faces,
                stats.ui_triangles,
                last_game.chunks_loaded,
                last_game.chunks_visible,
                timing.nanos as f64 / 1_000_000.0
            );
        }

        if frame >= 120
            && last_game.chunks_loaded >= 9
            && started.elapsed() >= Duration::from_secs(args.hold)
        {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    let elapsed = started.elapsed();
    let game_stats = game.stats();
    println!();
    println!(
        "rendered {frame} GPU frames on {}: {} meshed chunks ({} resident), \
         {} faces, {} triangles, {} visible",
        renderer.info().label(),
        game_stats.chunks_loaded,
        renderer.scene().chunk_count(),
        game_stats.faces,
        game_stats.triangles,
        game_stats.chunks_visible
    );
    let stats = last_stats;
    println!(
        "gpu frame: {} / {} last | {} draws, {} tris, {} translucent, {} UI tris",
        stats.total_chunks,
        stats.visible_chunks,
        stats.draw_calls,
        stats.triangles,
        stats.translucent_faces,
        stats.ui_triangles
    );
    println!(
        "player: ({:.2}, {:.2}, {:.2}) yaw {:.2} pitch {:.2} | {}",
        game_stats.position.x,
        game_stats.position.y,
        game_stats.position.z,
        game_stats.yaw,
        game_stats.pitch,
        game.save_status()
    );
    println!("{}", game.light_status());
    println!("{}", game.resource_pack_status());
    println!(
        "mesh: {} blocks with fallback shapes, {} translucent faces, light fallback {}",
        game_stats.missing_models, game_stats.translucent_faces, game_stats.light_fallback
    );
    println!(
        "server delivery: {} chunk columns, {} unloaded, {} resets, {} block updates | status: {}",
        game_stats.chunks_received,
        game_stats.unloads,
        game_stats.resets,
        game_stats.block_updates,
        game.status()
    );
    let frame_ms = frame_nanos as f64 / 1_000_000.0 / frame.max(1) as f64;
    let fps = if frame_nanos > 0 {
        frame as f64 / (frame_nanos as f64 / 1e9)
    } else {
        0.0
    };
    println!(
        "perf: wall {:.2} s, {:.2} ms/frame GPU submit+readback ({:.1} fps equivalent, \
         readback included), peak RSS {} KiB, GPU buffers {} bytes, atlas {}x{} ({} uploads)",
        elapsed.as_secs_f64(),
        frame_ms,
        fps,
        peak_rss_kib().map_or("unknown".to_owned(), |kib| kib.to_string()),
        renderer.scene().gpu_bytes(),
        renderer.scene().atlas_size().0,
        renderer.scene().atlas_size().1,
        atlas_uploads
    );
    let upload_ms = upload_nanos as f64 / 1_000_000.0;
    println!(
        "uploads: {uploaded_chunks} chunk meshes over {frame} frames, {:.2} ms total ({:.2} ms avg), \
         {} bytes, {max_chunks} chunk slots peak",
        upload_ms,
        if uploaded_chunks > 0 {
            upload_ms / uploaded_chunks as f64
        } else {
            0.0
        },
        uploaded_bytes
    );

    if let Some(out) = &args.out {
        let (width, height) = (renderer.width(), renderer.height());
        let pixels = renderer
            .read_rgba()
            .map_err(|error| format!("capture readback: {error}"))?;
        save_ppm(out, width, height, &pixels)
            .map_err(|error| format!("{}: {error}", out.display()))?;
        // A digest makes the evidence checkable without trusting the image.
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in &pixels {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        println!(
            "wrote GPU frame: {} ({width}x{height}, FNV-1a {hash:016x})",
            out.display()
        );
    }

    if game_stats.chunks_loaded == 0 {
        return Err("no server chunk meshes were uploaded".to_owned());
    }
    if renderer.scene().chunk_count() == 0 {
        return Err("GPU scene has no resident chunks".to_owned());
    }
    if uploaded_chunks == 0 {
        return Err("no chunk mesh reached the GPU".to_owned());
    }
    if stats.draw_calls == 0 || stats.triangles == 0 {
        return Err("GPU frame drew no world geometry".to_owned());
    }
    Ok(())
}
