//! `rkb-ping`: a server-list ping for Minecraft: Java Edition 1.21.1.
//!
//! Usage: `rkb-ping <host[:port]> [--json]`

use std::process::ExitCode;
use std::time::Duration;

use rkb_proto::ping;

fn main() -> ExitCode {
    let mut address = None;
    let mut raw_json = false;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--json" => raw_json = true,
            "-h" | "--help" => {
                println!("usage: rkb-ping <host[:port]> [--json]");
                return ExitCode::SUCCESS;
            }
            other if other.starts_with('-') => {
                eprintln!("unknown option {other}");
                eprintln!("usage: rkb-ping <host[:port]> [--json]");
                return ExitCode::FAILURE;
            }
            other => address = Some(other.to_owned()),
        }
    }
    let Some(address) = address else {
        eprintln!("usage: rkb-ping <host[:port]> [--json]");
        return ExitCode::FAILURE;
    };

    match ping(&address, Duration::from_secs(10)) {
        Ok(result) => {
            if raw_json {
                println!("{}", result.raw_json);
                return ExitCode::SUCCESS;
            }
            println!("{}", result.address);
            if let Some(name) = &result.status.version_name {
                match result.status.protocol {
                    Some(protocol) => println!("  version:  {name} (protocol {protocol})"),
                    None => println!("  version:  {name}"),
                }
            }
            if let (Some(online), Some(max)) =
                (result.status.players_online, result.status.players_max)
            {
                println!("  players:  {online}/{max}");
            }
            println!("  latency:  {} ms", result.latency.as_millis());
            if let Some(description) = &result.status.description {
                let description = description.replace('\n', " ");
                println!("  motd:     {description}");
            }
            if !result.status.speaks_this_protocol() {
                eprintln!(
                    "note: server protocol {:?} differs from this build's {}",
                    result.status.protocol,
                    rkb_proto::PROTOCOL_VERSION
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("ping failed: {error}");
            ExitCode::FAILURE
        }
    }
}
