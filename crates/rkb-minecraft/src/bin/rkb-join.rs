//! `rkb-join`: join an offline-mode 1.21.1 server and render its world.
//!
//! This is the headless multiplayer render check. It logs in, waits for server
//! chunks, and drives the same `MinecraftGame` runtime the Android host uses.
//! Nothing is generated: if the server sends no chunks, the frame stays empty
//! and the tool fails instead of drawing substitute terrain.
//!
//! Usage: `rkb-join --server <host[:port]> --block-report <blocks.json>
//! [--name <name>] [--seconds <n>] [--out <frame.ppm>]
//! [--width <px>] [--height <px>] [--hold <n>] [--query <x,y,z>]...
//! [--resource-pack <dir|zip>] [--fov <deg>] [--yaw <deg>] [--pitch <deg>]`

use std::env;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use rkb_core::EngineConfig;
use rkb_mc::BlockRegistry;
use rkb_minecraft::MinecraftGame;
use rkb_net::ServerWorld;
use rkb_platform::HeadlessSurface;
use rkb_render::Color;
use rkb_runtime::{Runtime, RuntimeConfig};

const DEFAULT_NAME: &str = "RKBJoin";
const DEFAULT_SECONDS: u64 = 30;
const DEFAULT_WIDTH: u32 = 480;
const DEFAULT_HEIGHT: u32 = 270;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("rkb-join: {message}");
            ExitCode::FAILURE
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
    queries: Vec<(i32, i32, i32)>,
    resource_pack: Option<PathBuf>,
    fov: Option<f32>,
    yaw: Option<f32>,
    pitch: Option<f32>,
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
    let mut queries = Vec::new();
    let mut resource_pack = None;
    let mut fov = None;
    let mut yaw = None;
    let mut pitch = None;
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
            "--query" => {
                let raw = value("--query")?;
                let parts: Vec<i32> = raw
                    .split(',')
                    .map(|part| {
                        part.parse()
                            .map_err(|_| format!("--query expects x,y,z, got {raw:?}"))
                    })
                    .collect::<Result<_, _>>()?;
                if parts.len() != 3 {
                    return Err(format!("--query expects x,y,z, got {raw:?}"));
                }
                queries.push((parts[0], parts[1], parts[2]));
            }
            "--help" | "-h" => {
                println!(
                    "usage: rkb-join --server <host[:port]> --block-report <blocks.json> \
                     [--name <name>] [--seconds <n>] [--out <frame.ppm>] \
                     [--width <px>] [--height <px>] [--hold <n>] [--query <x,y,z>]... \
                     [--resource-pack <dir|zip>] [--fov <deg>] [--yaw <deg>] \
                     [--pitch <deg>]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    if seconds == 0 || width == 0 || height == 0 {
        return Err("--seconds, --width, and --height must be positive".to_owned());
    }
    if let Some(pitch) = pitch {
        if !(-90.0..=90.0).contains(&pitch) {
            return Err("--pitch must be within -90..90 degrees".to_owned());
        }
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
        queries,
        resource_pack,
        fov,
        yaw,
        pitch,
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
    // Chunk columns stream in after the first position, so count them after
    // rendering instead of claiming a number before any frame has run.
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
    if args.fov.is_some() || args.yaw.is_some() || args.pitch.is_some() {
        println!(
            "camera: fov {:.1}°, yaw {:.1}°, pitch {:.1}°",
            game.fov_degrees(),
            game.player().yaw.to_degrees(),
            game.player().pitch.to_degrees()
        );
    }

    let mut runtime = Runtime::new(
        RuntimeConfig {
            engine: EngineConfig {
                max_delta_ms: 250,
                ..EngineConfig::default()
            },
            clear_color: Color::rgb(171, 208, 240),
            checksum_frames: true,
        },
        game,
        args.width,
        args.height,
    );
    runtime.start().map_err(|error| error.to_string())?;
    let mut surface = HeadlessSurface::new(args.width, args.height);
    let started = Instant::now();
    let deadline = started + Duration::from_secs(args.seconds);
    let mut frames = 0_u64;
    loop {
        runtime
            .frame(Duration::from_micros(16_667), &mut surface)
            .map_err(|error| error.to_string())?;
        frames += 1;
        let stats = runtime.game().stats();
        if frames >= 120
            && stats.chunks_loaded >= 9
            && started.elapsed() >= Duration::from_secs(args.hold)
        {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    let stats = runtime.game().stats();
    println!(
        "rendered {frames} frames: {} meshed chunks ({} resident), {} faces, \
         {} triangles, {} visible",
        stats.chunks_loaded,
        stats.chunks_generated,
        stats.faces,
        stats.triangles,
        stats.chunks_visible
    );
    println!(
        "player: ({:.2}, {:.2}, {:.2}) yaw {:.2} pitch {:.2} | {}",
        stats.position.x,
        stats.position.y,
        stats.position.z,
        stats.yaw,
        stats.pitch,
        runtime.game().save_status()
    );
    println!("{}", runtime.game().light_status());
    println!("{}", runtime.game().resource_pack_status());
    println!(
        "mesh: {} blocks with fallback shapes, {} translucent faces, light fallback {}",
        stats.missing_models, stats.translucent_faces, stats.light_fallback
    );
    println!(
        "server delivery: {} chunk columns, {} unloaded, {} resets, {} block updates | status: {}",
        stats.chunks_received,
        stats.unloads,
        stats.resets,
        stats.block_updates,
        runtime.game().status()
    );
    if let Some(reason) = runtime.game().status().strip_prefix("Disconnected: ") {
        println!("server disconnected: {reason}");
    }
    if let Some(out) = &args.out {
        let file = File::create(out).map_err(|error| format!("{}: {error}", out.display()))?;
        runtime
            .framebuffer()
            .write_ppm(&mut BufWriter::new(file))
            .map_err(|error| format!("{}: {error}", out.display()))?;
        println!("wrote frame: {}", out.display());
    }
    for (x, y, z) in &args.queries {
        match runtime.game_mut().block_at(*x, *y, *z) {
            Ok(Some(state)) => println!("block at ({x}, {y}, {z}): {state}"),
            Ok(None) => println!("block at ({x}, {y}, {z}): not loaded"),
            Err(error) => println!("block at ({x}, {y}, {z}): {error}"),
        }
    }
    if stats.chunks_loaded == 0 || stats.faces == 0 {
        return Err("no server chunk geometry was rendered".to_owned());
    }
    Ok(())
}
