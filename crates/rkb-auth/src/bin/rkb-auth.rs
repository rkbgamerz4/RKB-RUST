//! `rkb-auth`: Microsoft sign-in and an optional online-mode server login.
//!
//! Usage: `rkb-auth --client-id <azure-app-id> [--token-file <path>]
//! [--server <host[:port]>] [--timeout <seconds>]`
//!
//! The command prints the device-code instructions, then the profile name and
//! UUID. It never prints an access token, refresh token, or device code. Token
//! persistence happens only when `--token-file` is given.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use rkb_auth::{
    flow, thread_sleep, AuthConfig, FileTokenStore, OnlineAuthenticator, TokenStore, UreqClient,
};
use rkb_net::login;
use rkb_net::LoginOptions;

fn main() -> ExitCode {
    match parse_args(env::args().skip(1)) {
        Ok(Action::Help) => {
            print_usage();
            ExitCode::SUCCESS
        }
        Ok(Action::Run(args)) => match run(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("rkb-auth: {message}");
                ExitCode::FAILURE
            }
        },
        Err(message) => {
            eprintln!("rkb-auth: {message}");
            print_usage();
            ExitCode::from(2)
        }
    }
}

fn print_usage() {
    println!(
        "usage: rkb-auth --client-id <azure-app-id> [--token-file <path>] \
         [--server <host[:port]>] [--timeout <seconds>] [--sign-out]\n\
         \n\
         The client id may also be set with RKB_MS_CLIENT_ID. RKB does not ship one.\n\
         --token-file opts in to storing a refresh token (owner-only permissions).\n\
         --server attempts a protocol-767 online-mode login after sign-in.\n\
         --sign-out removes a stored token and exits."
    );
}

enum Action {
    Help,
    Run(Args),
}

struct Args {
    client_id: String,
    token_file: Option<PathBuf>,
    server: Option<String>,
    timeout: Duration,
    sign_out: bool,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Action, String> {
    let mut client_id = env::var("RKB_MS_CLIENT_ID").ok();
    let mut token_file = None;
    let mut server = None;
    let mut timeout = Duration::from_secs(20);
    let mut sign_out = false;

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--help" | "-h" => return Ok(Action::Help),
            "--client-id" => {
                client_id = Some(next_value(&mut args, "--client-id")?);
            }
            "--token-file" => {
                token_file = Some(PathBuf::from(next_value(&mut args, "--token-file")?));
            }
            "--server" => {
                server = Some(next_value(&mut args, "--server")?);
            }
            "--timeout" => {
                let value = next_value(&mut args, "--timeout")?;
                let seconds: u64 = value
                    .parse()
                    .map_err(|_| format!("--timeout expects seconds, got {value:?}"))?;
                if seconds == 0 {
                    return Err("--timeout must be at least one second".to_owned());
                }
                timeout = Duration::from_secs(seconds);
            }
            "--sign-out" => sign_out = true,
            other => return Err(format!("unrecognized argument {other:?}")),
        }
    }

    if sign_out && token_file.is_none() {
        return Err("--sign-out requires --token-file".to_owned());
    }
    let client_id = client_id.unwrap_or_default();
    if !sign_out && client_id.is_empty() {
        return Err(
            "--client-id or RKB_MS_CLIENT_ID is required; RKB does not ship a client id".to_owned(),
        );
    }

    Ok(Action::Run(Args {
        client_id,
        token_file,
        server,
        timeout,
        sign_out,
    }))
}

fn next_value(args: &mut impl Iterator<Item = String>, option: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("{option} needs a value"))
}

fn run(args: Args) -> Result<(), String> {
    let store: Option<Box<dyn TokenStore>> = args
        .token_file
        .map(|path| Box::new(FileTokenStore::new(path)) as Box<dyn TokenStore>);

    if args.sign_out {
        let store = store.as_deref().ok_or("--sign-out requires --token-file")?;
        store.clear().map_err(|error| error.to_string())?;
        println!("stored refresh token removed");
        return Ok(());
    }

    let client = UreqClient::new(args.timeout);
    let config = AuthConfig::new(args.client_id);
    let mut signed = try_stored_session(&client, &config, store.as_deref())?;

    if signed.is_none() {
        let start = flow::begin_device_code(&client, &config).map_err(|error| error.to_string())?;
        println!(
            "To sign in, open {} and enter the code {}",
            start.verification_uri, start.user_code
        );
        if !start.message.is_empty() {
            println!("{}", start.message);
        }
        println!(
            "Waiting for approval (the code expires in {} seconds)...",
            start.expires_in
        );
        let completed = flow::finish_device_code(&client, &config, &start, thread_sleep)
            .map_err(|error| error.to_string())?;
        signed = Some(completed);
    }

    let signed = signed.expect("a session was obtained");
    if let Some(store) = store.as_deref() {
        match signed.save(store) {
            Ok(()) => println!("stored a refresh token for the next start"),
            Err(error) => eprintln!("warning: could not store the refresh token: {error}"),
        }
    }

    println!(
        "profile: {} ({})",
        signed.session.name(),
        format_uuid(signed.session.uuid())
    );
    println!(
        "entitlements: {}",
        signed.session.entitlements().items.join(", ")
    );

    let Some(server) = args.server else {
        println!("signed in; no --server was given, so no game login was attempted");
        return Ok(());
    };

    let mut authenticator = OnlineAuthenticator::from_session(
        Box::new(client),
        config.endpoints.clone(),
        signed.session,
    );
    let options =
        LoginOptions::new(authenticator.session().name()).with_uuid(authenticator.session().uuid());
    let outcome = login(&server, args.timeout, &options, Some(&mut authenticator))
        .map_err(|error| error.to_string())?;

    println!("connected to {}", outcome.address);
    println!("player: {} ({})", outcome.name, format_uuid(outcome.uuid));
    println!(
        "protocol: {} (DataVersion {})",
        outcome.protocol, outcome.data_version
    );
    println!("session authenticated: {}", outcome.session_authenticated);
    println!("encrypted: {}", outcome.encrypted);
    match outcome.compression {
        Some(threshold) => println!("compression: threshold {threshold}"),
        None => println!("compression: disabled"),
    }
    println!(
        "configuration: finished ({} registries)",
        outcome.registries.len()
    );
    println!(
        "note: M2 stops at configuration; the play state and a world join are not implemented"
    );
    Ok(())
}

fn try_stored_session(
    client: &UreqClient,
    config: &AuthConfig,
    store: Option<&dyn TokenStore>,
) -> Result<Option<rkb_auth::SignedIn>, String> {
    let Some(store) = store else {
        return Ok(None);
    };
    let stored = store.load().map_err(|error| error.to_string())?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    println!("stored session for {}; refreshing", stored.name());
    match flow::refresh(client, config, stored.refresh_token()) {
        Ok(signed) => Ok(Some(signed)),
        Err(error) => {
            println!("stored session could not be refreshed: {error}");
            Ok(None)
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
