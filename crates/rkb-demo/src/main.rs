//! Headless RKB RUST demo.
//!
//! Runs the shared sandbox scene for a fixed number of frames and optionally
//! writes the final frame as a binary PPM image.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use rkb_core::Vec2;
use rkb_platform::{HeadlessSurface, PlatformInfo};
use rkb_runtime::{Runtime, RuntimeConfig};

const USAGE: &str = "\
RKB RUST headless demo

Usage: rkb-demo [OPTIONS]

Options:
  --frames N      Frames to simulate (default: 180)
  --width N       Framebuffer width in pixels (default: 960)
  --height N      Framebuffer height in pixels (default: 540)
  --out PATH      Write the final frame as a binary PPM
  --frame-ms N    Frame delta in milliseconds (default: 16.666)
  -h, --help      Show this help
";

#[derive(Clone, Debug)]
struct Options {
    frames: u32,
    width: u32,
    height: u32,
    out: Option<PathBuf>,
    frame_delta: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            frames: 180,
            width: 960,
            height: 540,
            out: None,
            frame_delta: Duration::from_nanos(16_666_667),
        }
    }
}

fn parse_args() -> Result<Option<Options>, String> {
    let mut options = Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--frames" => {
                let value = args.next().ok_or("--frames requires a value")?;
                options.frames = value.parse().map_err(|_| "invalid --frames value")?;
            }
            "--width" => {
                let value = args.next().ok_or("--width requires a value")?;
                options.width = value.parse().map_err(|_| "invalid --width value")?;
            }
            "--height" => {
                let value = args.next().ok_or("--height requires a value")?;
                options.height = value.parse().map_err(|_| "invalid --height value")?;
            }
            "--out" => {
                let value = args.next().ok_or("--out requires a value")?;
                options.out = Some(PathBuf::from(value));
            }
            "--frame-ms" => {
                let value = args.next().ok_or("--frame-ms requires a value")?;
                let millis: f64 = value.parse().map_err(|_| "invalid --frame-ms value")?;
                if !(0.1..=1000.0).contains(&millis) {
                    return Err("--frame-ms must be in 0.1..=1000".to_owned());
                }
                options.frame_delta = Duration::from_secs_f64(millis / 1000.0);
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    if options.width == 0 || options.width > 8192 || options.height == 0 || options.height > 8192 {
        return Err("width and height must be in 1..=8192".to_owned());
    }
    Ok(Some(options))
}

fn run(options: &Options) -> Result<(), String> {
    let viewport = Vec2::new(options.width as f32, options.height as f32);
    let booted = rkb_sandbox::bootstrap(viewport).map_err(|error| error.to_string())?;
    let config = RuntimeConfig {
        engine: booted.engine.clone(),
        clear_color: rkb_render::Color::rgb(14, 18, 28),
    };

    let mut runtime = Runtime::new(config, booted.sandbox, options.width, options.height);
    runtime.start().map_err(|error| error.to_string())?;

    let mut surface = HeadlessSurface::new(options.width, options.height);
    let mut last_report = None;
    for _ in 0..options.frames {
        let report = runtime
            .frame(options.frame_delta, &mut surface)
            .map_err(|error| error.to_string())?;
        last_report = Some(report);
    }

    let report = last_report.unwrap_or_default();
    let stats = runtime.game().stats();
    let platform = PlatformInfo::current();

    println!(
        "RKB RUST {} | {} {} | {}x{}",
        rkb_core::VERSION,
        platform.os,
        platform.arch,
        options.width,
        options.height
    );
    println!(
        "frames={} steps={} dropped={} fps={:.1} checksum={:016x}",
        surface.frames_presented(),
        report.steps,
        report.dropped_steps,
        report.fps,
        report.checksum
    );
    println!(
        "balls={} taps={} bursts={} zoom={:.2} assets_cached={} bytes_cached={}",
        stats.balls,
        stats.taps,
        stats.bursts,
        stats.zoom,
        stats.resources.entries,
        stats.resources.bytes_cached
    );

    if let Some(path) = &options.out {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
            }
        }
        let file = fs::File::create(path)
            .map_err(|error| format!("could not create {}: {error}", path.display()))?;
        let mut writer = io::BufWriter::new(file);
        runtime
            .framebuffer()
            .write_ppm(&mut writer)
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
        println!("wrote {}", path.display());
    }

    Ok(())
}

fn main() -> ExitCode {
    match parse_args() {
        Ok(Some(options)) => match run(&options) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        },
        Ok(None) => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}
