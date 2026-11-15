//! `rkb-world`: inspect a Minecraft: Java Edition 1.21 save without launching
//! the game.
//!
//! The tool only reads data supplied by the user and never downloads or
//! redistributes Minecraft files.

use std::env;
use std::process::ExitCode;

use rkb_mc::{BlockRegistry, World, VERSION};

const USAGE: &str = "\
rkb-world — inspect Minecraft: Java Edition 1.21 world data

Usage:
  rkb-world info <save>                     Version, name, regions, chunks
  rkb-world block <save> <x> <y> <z>        Block state at world coordinates
  rkb-world chunk <save> <x> <z>            Chunk identity and sections
  rkb-world registry <blocks.json> [block]  Registry summary or one block
  rkb-world version                        Print the tool version

<save> is a world directory containing level.dat and region/.
<blocks.json> is a report generated from the user's own game installation.
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("rkb-world: {message}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        print!("{USAGE}");
        return Ok(());
    };
    if matches!(command.as_str(), "-h" | "--help" | "help") {
        print!("{USAGE}");
        return Ok(());
    }
    match command.as_str() {
        "info" => {
            let save = required(&mut args, "save directory")?;
            let world = World::open(&save).map_err(|error| error.to_string())?;
            let stats = world.stats().map_err(|error| error.to_string())?;
            let level = world.level();
            println!("save: {save}");
            println!(
                "name: {}",
                level.level_name.as_deref().unwrap_or("(unnamed)")
            );
            println!(
                "minecraft: {} (data version {})",
                level.version.id, level.data_version
            );
            println!("protocol: {}", level.version.protocol);
            println!(
                "packs: resource {}, data {}",
                level.version.resource_pack_format, level.version.data_pack_format
            );
            if let Some([x, y, z]) = level.spawn {
                println!("spawn: {x}, {y}, {z}");
            }
            println!("regions: {}", stats.regions);
            println!("chunks: {}", stats.chunks);
        }
        "block" => {
            let save = required(&mut args, "save directory")?;
            let x = integer(&mut args, "x")?;
            let y = integer(&mut args, "y")?;
            let z = integer(&mut args, "z")?;
            let mut world = World::open(&save).map_err(|error| error.to_string())?;
            match world.block_at(x, y, z).map_err(|error| error.to_string())? {
                Some(state) => println!("({x}, {y}, {z}) = {state}"),
                None => println!("({x}, {y}, {z}) = unloaded"),
            }
        }
        "chunk" => {
            let save = required(&mut args, "save directory")?;
            let x = integer(&mut args, "chunk x")?;
            let z = integer(&mut args, "chunk z")?;
            let mut world = World::open(&save).map_err(|error| error.to_string())?;
            match world.chunk(x, z).map_err(|error| error.to_string())? {
                None => println!("chunk ({x}, {z}) = not present"),
                Some(chunk) => {
                    println!(
                        "chunk ({x}, {z}): data version {}{}",
                        chunk.data_version,
                        chunk
                            .status
                            .as_deref()
                            .map(|status| format!(", status {status}"))
                            .unwrap_or_default()
                    );
                    println!("sections: {}", chunk.section_count());
                    println!("non-air blocks: {}", chunk.non_air_blocks());
                    for section in chunk.sections() {
                        println!(
                            "  section {}: {} states, {} non-air",
                            section.y,
                            section.palette().len(),
                            section.non_air_blocks()
                        );
                    }
                }
            }
        }
        "registry" => {
            let path = required(&mut args, "blocks.json path")?;
            let registry = BlockRegistry::from_file(&path).map_err(|error| error.to_string())?;
            if let Some(name) = args.next() {
                let mut found = false;
                for (id, state) in registry.states().enumerate() {
                    if state.name == name {
                        found = true;
                        println!("{id:>6} {state}");
                    }
                }
                if !found {
                    return Err(format!("block {name} is not in the registry"));
                }
            } else {
                println!("states: {}", registry.len());
                println!("blocks: {}", registry.block_count());
                if let Some(state) = registry.default_state("minecraft:stone") {
                    println!("default minecraft:stone: {state}");
                }
            }
        }
        "version" => println!("rkb-world {VERSION}"),
        other => return Err(format!("unknown command {other:?}; try --help")),
    }
    Ok(())
}

fn required(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("missing {name}; try --help"))
}

fn integer(args: &mut impl Iterator<Item = String>, name: &str) -> Result<i32, String> {
    let value = required(args, name)?;
    value
        .parse()
        .map_err(|_| format!("{name} must be an integer, got {value:?}"))
}
