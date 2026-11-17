//! `rkb-login`: complete a protocol-767 login and configuration exchange.
//!
//! Usage: `rkb-login <host[:port]> [username]`
//!
//! This is an M1/M2 diagnostic. It supports offline-mode servers and fails
//! explicitly when a server requests online-mode authentication, because this
//! binary has no Microsoft authenticator; use `rkb-auth` for that. It never
//! asks for or stores credentials.

use std::env;
use std::process::ExitCode;
use std::time::Duration;

use rkb_net::{login, LoginOptions};

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(address) = args.next() else {
        eprintln!("usage: rkb-login <host[:port]> [username]");
        return ExitCode::from(2);
    };
    let username = args.next().unwrap_or_else(|| "RKB".to_owned());
    if args.next().is_some() {
        eprintln!("usage: rkb-login <host[:port]> [username]");
        return ExitCode::from(2);
    }

    let options = LoginOptions::new(username);
    match login(&address, Duration::from_secs(15), &options, None) {
        Ok(outcome) => {
            println!("address: {}", outcome.address);
            println!(
                "version: {} protocol {} DataVersion {}",
                rkb_net::GAME_VERSION,
                outcome.protocol,
                outcome.data_version
            );
            println!("player: {} ({})", outcome.name, format_uuid(outcome.uuid));
            println!("encrypted: {}", outcome.encrypted);
            println!("session authenticated: {}", outcome.session_authenticated);
            match outcome.compression {
                Some(threshold) => println!("compression: threshold {threshold}"),
                None => println!("compression: disabled"),
            }
            println!("strict error handling: {}", outcome.strict_error_handling);
            println!("registries: {}", outcome.registries.len());
            for registry in &outcome.registries {
                println!("  {registry}");
            }
            println!("feature flags: {}", outcome.feature_flags.len());
            println!("server known packs: {}", outcome.known_packs.len());
            println!("configuration: finished");
            println!("packet trace ({} packets):", outcome.trace.len());
            for entry in &outcome.trace {
                println!(
                    "  {} {:<13} id=0x{:02X} len={}",
                    entry.direction, entry.state, entry.id, entry.len
                );
            }
            println!("note: M1 ends at configuration; no play/world join is claimed");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("login failed: {error}");
            ExitCode::FAILURE
        }
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
