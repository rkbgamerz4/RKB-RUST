//! `rkb-play`: a diagnostic client that joins a 1.21.1 server and reports the
//! play state from real server data.
//!
//! Usage: `rkb-play --server <host[:port]> [--name <name>] [--seconds <n>]
//! [--block-report <blocks.json>] [--timeout <seconds>]`
//!
//! This binary performs an **offline-mode** login. For an online-mode server,
//! sign in first with `rkb-auth --server`, which uses the same play session
//! after an authenticated login.
//!
//! `--block-report` points at a `blocks.json` report generated from the user's
//! own 1.21.1 installation. Without it, block updates are printed as numeric
//! state ids. RKB never bundles Mojang data.

use std::collections::BTreeMap;
use std::env;
use std::net::Shutdown;
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use rkb_mc::BlockRegistry;
use rkb_net::{login_configured, Event, LoginOptions, PlaySession};

const MAX_REPORTED_CHUNKS: usize = 12;

fn main() -> ExitCode {
    match parse_args(env::args().skip(1)) {
        Ok(Action::Help) => {
            print_usage();
            ExitCode::SUCCESS
        }
        Ok(Action::Run(args)) => match run(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("rkb-play: {message}");
                ExitCode::FAILURE
            }
        },
        Err(message) => {
            eprintln!("rkb-play: {message}");
            print_usage();
            ExitCode::from(2)
        }
    }
}

fn print_usage() {
    println!(
        "usage: rkb-play --server <host[:port]> [--name <name>] [--seconds <n>] \
         [--block-report <blocks.json>] [--timeout <seconds>]\n\
         \n\
         Joins a protocol-767 server in offline mode and reports real play-state\n\
         data. Use rkb-auth for online-mode servers. A block report is optional\n\
         and must come from the user's own Minecraft installation."
    );
}

enum Action {
    Help,
    Run(Args),
}

struct Args {
    server: String,
    name: String,
    seconds: Option<u64>,
    block_report: Option<PathBuf>,
    timeout: Duration,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Action, String> {
    let mut server = None;
    let mut name = "RKBPlay".to_owned();
    let mut seconds = None;
    let mut block_report = None;
    let mut timeout = Duration::from_secs(20);

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--help" | "-h" => return Ok(Action::Help),
            "--server" => server = Some(next_value(&mut args, "--server")?),
            "--name" => name = next_value(&mut args, "--name")?,
            "--seconds" => {
                let value = next_value(&mut args, "--seconds")?;
                let parsed: u64 = value
                    .parse()
                    .map_err(|_| format!("--seconds expects a number, got {value:?}"))?;
                if parsed == 0 {
                    return Err("--seconds must be at least one".to_owned());
                }
                seconds = Some(parsed);
            }
            "--block-report" => {
                block_report = Some(PathBuf::from(next_value(&mut args, "--block-report")?));
            }
            "--timeout" => {
                let value = next_value(&mut args, "--timeout")?;
                let parsed: u64 = value
                    .parse()
                    .map_err(|_| format!("--timeout expects seconds, got {value:?}"))?;
                if parsed == 0 {
                    return Err("--timeout must be at least one second".to_owned());
                }
                timeout = Duration::from_secs(parsed);
            }
            other => return Err(format!("unrecognized argument {other:?}")),
        }
    }

    let server = server.ok_or("--server is required")?;
    Ok(Action::Run(Args {
        server,
        name,
        seconds,
        block_report,
        timeout,
    }))
}

fn next_value(args: &mut impl Iterator<Item = String>, option: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("{option} needs a value"))
}

fn run(args: Args) -> Result<(), String> {
    let registry = match &args.block_report {
        Some(path) => Some(
            BlockRegistry::from_file(path)
                .map_err(|error| format!("could not read {}: {error}", path.display()))?,
        ),
        None => None,
    };

    let options = LoginOptions::new(&args.name);
    let configured = login_configured(&args.server, args.timeout, &options, None)
        .map_err(|error| error.to_string())?;

    let outcome = configured.outcome().clone();
    println!("connected to {}", outcome.address);
    println!("player: {} ({})", outcome.name, format_uuid(outcome.uuid));
    println!(
        "protocol: {} (DataVersion {})",
        outcome.protocol, outcome.data_version
    );
    println!("session authenticated: {}", outcome.session_authenticated);
    println!(
        "configuration: finished ({} registries)",
        outcome.registries.len()
    );

    let mut session =
        PlaySession::from_configured(configured).map_err(|error| error.to_string())?;

    if let Some(seconds) = args.seconds {
        let stream = session
            .try_clone_stream()
            .map_err(|error| error.to_string())?;
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(seconds));
            let _ = stream.shutdown(Shutdown::Both);
        });
    }

    let mut reported_chunks = 0usize;
    let mut chunk_samples: BTreeMap<(i32, i32), i64> = BTreeMap::new();

    loop {
        let event = match session.next_event() {
            Ok(event) => event,
            Err(error) => return Err(format!("protocol error: {error}")),
        };
        match event {
            Event::Joined(packet) => {
                println!(
                    "joined: entity {} in {} (game mode {}{})",
                    packet.entity_id,
                    packet.spawn.dimension_name,
                    packet.spawn.game_mode,
                    if packet.spawn.is_flat {
                        ", flat world"
                    } else {
                        ""
                    }
                );
                println!(
                    "  worlds: {} | view distance {} | simulation distance {} | max players {}",
                    packet.world_names.join(", "),
                    packet.view_distance,
                    packet.simulation_distance,
                    packet.max_players
                );
                println!(
                    "  hardcore: {} | respawn screen: {} | secure chat enforced: {}",
                    packet.hardcore, packet.enable_respawn_screen, packet.enforces_secure_chat
                );
                for dimension in session.dimensions().values() {
                    println!(
                        "  dimension {} (id {}): min_y {} height {}{}",
                        dimension.name,
                        dimension.id,
                        dimension.min_y,
                        dimension.height,
                        if dimension.has_skylight {
                            ", skylight"
                        } else {
                            ""
                        }
                    );
                }
            }
            Event::Position(position) => println!(
                "position: ({:.2}, {:.2}, {:.2}) yaw {:.1} pitch {:.1} teleport {}",
                position.x,
                position.y,
                position.z,
                position.yaw,
                position.pitch,
                position.teleport_id
            ),
            Event::ChunkLoaded {
                x,
                z,
                sections,
                non_air_blocks,
                block_entities,
            } => {
                chunk_samples.insert((x, z), non_air_blocks);
                if reported_chunks < MAX_REPORTED_CHUNKS {
                    println!(
                        "chunk ({x}, {z}): {sections} sections, {non_air_blocks} non-air blocks, \
                         {block_entities} block entities"
                    );
                    reported_chunks += 1;
                    if reported_chunks == MAX_REPORTED_CHUNKS {
                        println!("  (further chunk lines suppressed)");
                    }
                }
            }
            Event::ChunkUnloaded { x, z } => println!("chunk ({x}, {z}) unloaded"),
            Event::BlockChanged(change) => println!(
                "block update at ({}, {}, {}): state {}{}",
                change.position.x,
                change.position.y,
                change.position.z,
                change.state_id,
                block_name(registry.as_ref(), change.state_id)
            ),
            Event::BlocksChanged(changes) => {
                for change in &changes {
                    println!(
                        "block update at ({}, {}, {}): state {}{}",
                        change.position.x,
                        change.position.y,
                        change.position.z,
                        change.state_id,
                        block_name(registry.as_ref(), change.state_id)
                    );
                }
            }
            Event::Health(health) => println!(
                "health: {:.1} food {} saturation {:.1}",
                health.health, health.food, health.food_saturation
            ),
            Event::WindowItems { window_id, slots } => {
                println!("inventory window {window_id}: {slots} slots");
            }
            Event::SlotChanged(slot) => println!(
                "slot {} in window {} now {:?}",
                slot.slot, slot.window_id, slot.item
            ),
            Event::EntitySpawned(entity) => println!(
                "entity {} spawned: type {} at ({:.2}, {:.2}, {:.2})",
                entity.entity_id, entity.kind, entity.x, entity.y, entity.z
            ),
            Event::EntitiesRemoved(ids) => println!("entities removed: {ids:?}"),
            Event::GameState(state) => {
                println!("game state: reason {} value {}", state.reason, state.value)
            }
            Event::Respawned(respawn) => println!(
                "respawned in {} at game mode {}",
                respawn.spawn.dimension_name, respawn.spawn.game_mode
            ),
            Event::Time(time) => println!("time: world age {} time of day {}", time.age, time.time),
            Event::Experience(experience) => println!(
                "experience: bar {:.2} level {} total {}",
                experience.bar, experience.level, experience.total
            ),
            Event::HeldItem(slot) => println!("held hotbar slot {slot}"),
            Event::Chat { text, action_bar } => {
                println!("{}: {text}", if action_bar { "action bar" } else { "chat" })
            }
            Event::Ping { id } => println!("server ping {id} answered"),
            Event::Disconnected { reason } => {
                println!("disconnected by server: {reason}");
                break;
            }
            Event::ReconfigurationRequested => {
                println!(
                    "server requested reconfiguration; M3 acknowledges it but cannot complete it"
                );
                break;
            }
            Event::Unsupported { packet, reason } => {
                println!("unsupported {packet}: {reason}");
            }
            Event::Unknown { id, length } => {
                println!("unknown packet 0x{id:02X} ({length} bytes)");
            }
            Event::Closed { reason } => {
                println!("connection closed: {reason}");
                break;
            }
            Event::EntityMoved { entity_id, x, y, z } => {
                println!("entity {entity_id} moved to ({x:.2}, {y:.2}, {z:.2})")
            }
            Event::KeepAlive { .. } | Event::ChunkBatch { .. } | Event::LightUpdated { .. } => {}
            Event::Unhandled { id } => println!("unhandled packet 0x{id:02X}"),
        }
    }

    print_block_samples(&session, registry.as_ref());

    let stats = session.stats();
    println!("session summary:");
    println!("  packets: {}", stats.packets);
    println!("  chunks received: {}", chunk_samples.len());
    if !chunk_samples.is_empty() {
        let loaded: Vec<String> = chunk_samples
            .iter()
            .take(MAX_REPORTED_CHUNKS)
            .map(|((x, z), blocks)| format!("({x}, {z}) {blocks}"))
            .collect();
        println!("  chunk block counts: {}", loaded.join(", "));
        if chunk_samples.len() > MAX_REPORTED_CHUNKS {
            println!(
                "  ({} more chunks not listed)",
                chunk_samples.len() - MAX_REPORTED_CHUNKS
            );
        }
    }
    println!("  keep-alives answered: {}", stats.keep_alives);
    println!("  chunk batches answered: {}", stats.chunk_batches);
    println!("  block updates: {}", stats.block_updates);
    println!("  chat messages: {}", stats.chat_messages);
    println!("  unsupported packets: {}", stats.unsupported_packets);
    println!("  unhandled packets: {}", stats.unhandled_packets);
    println!("  unknown packets: {}", stats.unknown_packets);
    Ok(())
}

fn print_block_samples(
    session: &PlaySession<std::net::TcpStream>,
    registry: Option<&BlockRegistry>,
) {
    let Some(registry) = registry else {
        println!(
            "block samples need --block-report; run again with a blocks.json report to \
             identify block states"
        );
        return;
    };
    let player = session.player();
    if !player.has_position {
        return;
    }
    let dimension = session.dimensions().get(&player.dimension_type);
    let Some(dimension) = dimension else {
        return;
    };
    let column_x = player.x.floor() as i32;
    let column_z = player.z.floor() as i32;
    let top = dimension.min_y + dimension.height - 1;
    let bottom = dimension.min_y;
    let mut found = false;
    for y in (bottom..=top).rev() {
        let Some(state) = session.block_state(column_x, y, column_z) else {
            continue;
        };
        let Some(text) = describe_state(Some(registry), state) else {
            continue;
        };
        if text == "minecraft:air" {
            continue;
        }
        println!("block sample at ({column_x}, {y}, {column_z}): {text}");
        found = true;
        break;
    }
    if !found {
        println!("block sample at ({column_x}, ?, {column_z}): no non-air block in loaded data");
    }
}

fn describe_state(registry: Option<&BlockRegistry>, state: u32) -> Option<String> {
    let Some(registry) = registry else {
        return Some(format!("state {state}"));
    };
    registry.state(state).map(|block| {
        if block.properties.is_empty() {
            block.name.clone()
        } else {
            let properties: Vec<String> = block
                .properties
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect();
            format!("{}[{}]", block.name, properties.join(","))
        }
    })
}

fn block_name(registry: Option<&BlockRegistry>, state: i32) -> String {
    if state < 0 {
        return String::new();
    }
    match describe_state(registry, state as u32) {
        Some(name) => format!(" ({name})"),
        None => String::new(),
    }
}

fn format_uuid(uuid: u128) -> String {
    let bytes = uuid.to_be_bytes();
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-\
         {:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}
