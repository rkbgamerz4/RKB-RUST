//! Login and configuration session for protocol 767.
//!
//! The implemented flow is the M1 gate:
//!
//! ```text
//! handshake(Login) → LoginStart → [EncryptionRequest → EncryptionResponse]
//!   → SetCompression → LoginSuccess → LoginAcknowledged
//!   → configuration packets → FinishConfiguration (ack)
//! ```
//!
//! This stops at the end of configuration. It does **not** claim to join a
//! world, and online-mode login requires a [`SessionAuthenticator`] supplied by
//! the caller (M2). No credentials are stored or logged here.

use std::fmt;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use rkb_proto::client::ServerAddress;
use rkb_proto::codec::Reader;
use rkb_proto::crypto::{generate_secret, md5, server_hash, RsaPublicKey};
use rkb_proto::error::{Error, Result};
use rkb_proto::ids;
use rkb_proto::packets::configuration::Clientbound;
use rkb_proto::packets::{
    ClientInformation, CookieResponse, FinishConfiguration, Handshake, KeepAlive, KnownPack,
    KnownPacks, LoginAcknowledged, LoginCookieRequest, LoginCookieResponse, LoginDisconnect,
    LoginPluginRequest, LoginPluginResponse, LoginStart, LoginSuccess, NextState, PluginMessage,
    Pong, Property, SetCompression,
};
use rkb_proto::{DATA_VERSION, PROTOCOL_VERSION};

use crate::chat;
use crate::connection::Connection;

/// A conservative ceiling on configuration packets; vanilla sends a few
/// hundred registry/tag packets, not hundreds of thousands.
const MAX_CONFIGURATION_PACKETS: usize = 100_000;

/// The join-server call M2 will implement.
///
/// M1 deliberately has no Microsoft/Xbox implementation. The trait exists so
/// the login state machine can be tested with a server that requests
/// authentication, and so M2 can plug in the real flow without changing this
/// code.
pub trait SessionAuthenticator {
    /// Calls the session server's join endpoint with the server hash.
    ///
    /// The hash is not secret, but it must not be logged together with tokens.
    fn join_server(&mut self, server_hash: &str) -> Result<()>;
}

/// Options for one login attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginOptions {
    /// The player name sent in `LoginStart`.
    pub username: String,
    /// The profile UUID sent in `LoginStart`.
    ///
    /// `None` derives the offline-mode UUID from the name. An authenticated
    /// sign-in supplies the Mojang profile UUID instead.
    pub uuid: Option<u128>,
    /// Whether to record packet ids and sizes.
    pub trace: bool,
}

impl LoginOptions {
    /// Options with tracing enabled and the offline UUID derived from the name.
    #[must_use]
    pub fn new(username: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            uuid: None,
            trace: true,
        }
    }

    /// Uses an explicit profile UUID, as an online-mode sign-in requires.
    #[must_use]
    pub fn with_uuid(mut self, uuid: u128) -> Self {
        self.uuid = Some(uuid);
        self
    }
}

/// Which side sent a traced packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Serverbound,
    Clientbound,
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Serverbound => "C→S",
            Self::Clientbound => "S→C",
        })
    }
}

/// One packet observed during the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceEntry {
    pub direction: Direction,
    pub state: &'static str,
    pub id: i32,
    /// Packet body size before compression.
    pub len: usize,
}

/// What a completed login observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginOutcome {
    pub address: ServerAddress,
    pub protocol: i32,
    pub data_version: i32,
    pub uuid: u128,
    pub name: String,
    pub properties: Vec<Property>,
    pub strict_error_handling: bool,
    /// Whether the key exchange ran.
    pub encrypted: bool,
    /// Whether the server required and the client completed session-server
    /// authentication (`join_server`) for this login.
    pub session_authenticated: bool,
    /// The negotiated compression threshold.
    pub compression: Option<usize>,
    /// Registry names received before `FinishConfiguration`.
    pub registries: Vec<String>,
    pub feature_flags: Vec<String>,
    /// The server's known packs; this client answered with an empty list so the
    /// server sends full registry data.
    pub known_packs: Vec<KnownPack>,
    pub trace: Vec<TraceEntry>,
}

#[derive(Default)]
struct ConfigurationSummary {
    registries: Vec<String>,
    feature_flags: Vec<String>,
    known_packs: Vec<KnownPack>,
}

struct Session<'a, S> {
    connection: Connection<S>,
    trace: Vec<TraceEntry>,
    trace_enabled: bool,
    authenticator: Option<&'a mut dyn SessionAuthenticator>,
}

/// Logs in to `address` and completes the configuration state.
///
/// `authenticator` is required only if the server requests online-mode
/// authentication. With `None`, such a server produces an explicit error
/// instead of a silent failure or a fabricated login.
pub fn login(
    address: &str,
    timeout: Duration,
    options: &LoginOptions,
    authenticator: Option<&mut dyn SessionAuthenticator>,
) -> Result<LoginOutcome> {
    validate_username(&options.username)?;
    let address = ServerAddress::parse(address)?;
    let stream = connect(&address, timeout)?;
    let mut session = Session {
        connection: Connection::new(stream),
        trace: Vec::new(),
        trace_enabled: options.trace,
        authenticator,
    };
    session.run(&address, options)
}

fn validate_username(username: &str) -> Result<()> {
    let units = username.encode_utf16().count();
    if units == 0 || units > 16 {
        return Err(Error::protocol(format!(
            "player name must be 1..=16 UTF-16 code units, got {units}"
        )));
    }
    Ok(())
}

fn connect(address: &ServerAddress, timeout: Duration) -> Result<TcpStream> {
    let timeout = timeout.max(Duration::from_millis(1));
    let addresses = (address.host.as_str(), address.port)
        .to_socket_addrs()
        .map_err(|error| Error::io(format!("{}: {error}", address.host)))?;
    let mut last_error = None;
    for candidate in addresses {
        match TcpStream::connect_timeout(&candidate, timeout) {
            Ok(stream) => {
                stream.set_read_timeout(Some(timeout))?;
                stream.set_write_timeout(Some(timeout))?;
                stream.set_nodelay(true)?;
                return Ok(stream);
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(Error::io(match last_error {
        Some(error) => format!("{}: {error}", address.host),
        None => format!("{} did not resolve to any address", address.host),
    }))
}

impl<S: Read + Write> Session<'_, S> {
    fn run(&mut self, address: &ServerAddress, options: &LoginOptions) -> Result<LoginOutcome> {
        let handshake = Handshake {
            protocol: PROTOCOL_VERSION,
            host: address.host.clone(),
            port: address.port,
            next_state: NextState::Login,
        };
        self.send("handshake", &handshake.encode()?)?;

        let username = options.username.as_str();
        let uuid = options.uuid.unwrap_or_else(|| md5::offline_uuid(username));
        self.send(
            "login",
            &LoginStart {
                name: username.to_owned(),
                uuid,
            }
            .encode()?,
        )?;

        let mut encrypted = false;
        let mut session_authenticated = false;
        let mut compression = None;
        loop {
            let body = self.recv("login")?;
            let id = packet_id(&body)?;
            match id {
                ids::login::clientbound::DISCONNECT => {
                    let disconnect = LoginDisconnect::decode(packet_payload(&body))?;
                    let reason = chat::json_component_plain_text(&disconnect.reason_json)
                        .unwrap_or(disconnect.reason_json);
                    return Err(Error::protocol(format!(
                        "server disconnected during login: {reason}"
                    )));
                }
                ids::login::clientbound::ENCRYPTION_REQUEST => {
                    if encrypted {
                        return Err(Error::protocol("server sent a second encryption request"));
                    }
                    let request =
                        rkb_proto::packets::EncryptionRequest::decode(packet_payload(&body))?;
                    let key = RsaPublicKey::from_der(&request.public_key)?;
                    let secret = generate_secret()?;
                    let hash = server_hash(&request.server_id, &secret, &request.public_key);

                    if request.should_authenticate {
                        match self.authenticator.as_deref_mut() {
                            Some(authenticator) => {
                                authenticator.join_server(&hash)?;
                                session_authenticated = true;
                            }
                            None => {
                                return Err(Error::protocol(
                                    "server requires online-mode authentication and no \
                                     authenticator is configured",
                                ))
                            }
                        }
                    }

                    let response = rkb_proto::packets::EncryptionResponse {
                        shared_secret: key.encrypt(&secret)?,
                        verify_token: key.encrypt(&request.verify_token)?,
                    };
                    self.send("login", &response.encode()?)?;
                    self.connection.enable_encryption(&secret);
                    encrypted = true;
                }
                ids::login::clientbound::SET_COMPRESSION => {
                    let set = SetCompression::decode(packet_payload(&body))?;
                    let threshold = set.enabled_threshold();
                    self.connection.set_compression(threshold);
                    compression = threshold;
                }
                ids::login::clientbound::SUCCESS => {
                    let success = LoginSuccess::decode(packet_payload(&body))?;
                    self.send("login", &LoginAcknowledged.encode())?;
                    let summary = self.configuration()?;
                    return Ok(LoginOutcome {
                        address: address.clone(),
                        protocol: PROTOCOL_VERSION,
                        data_version: DATA_VERSION,
                        uuid: success.uuid,
                        name: success.name,
                        properties: success.properties,
                        strict_error_handling: success.strict_error_handling,
                        encrypted,
                        session_authenticated,
                        compression,
                        registries: summary.registries,
                        feature_flags: summary.feature_flags,
                        known_packs: summary.known_packs,
                        trace: std::mem::take(&mut self.trace),
                    });
                }
                ids::login::clientbound::PLUGIN_REQUEST => {
                    let request = LoginPluginRequest::decode(packet_payload(&body))?;
                    self.send(
                        "login",
                        &LoginPluginResponse {
                            message_id: request.message_id,
                            data: None,
                        }
                        .encode()?,
                    )?;
                }
                ids::login::clientbound::COOKIE_REQUEST => {
                    let request = LoginCookieRequest::decode(packet_payload(&body))?;
                    self.send(
                        "login",
                        &LoginCookieResponse {
                            key: request.key,
                            payload: None,
                        }
                        .encode()?,
                    )?;
                }
                other => {
                    return Err(Error::UnknownPacket {
                        state: "login",
                        id: other,
                    })
                }
            }
        }
    }

    fn configuration(&mut self) -> Result<ConfigurationSummary> {
        // Vanilla sends its settings immediately after LoginAcknowledged.
        self.send("configuration", &ClientInformation::default().encode()?)?;

        let mut summary = ConfigurationSummary::default();
        for _ in 0..MAX_CONFIGURATION_PACKETS {
            let body = self.recv("configuration")?;
            match Clientbound::decode(&body)? {
                Clientbound::CookieRequest(request) => {
                    self.send(
                        "configuration",
                        &CookieResponse {
                            key: request.key,
                            payload: None,
                        }
                        .encode()?,
                    )?;
                }
                Clientbound::PluginMessage(PluginMessage { .. }) => {
                    // No configuration plugin channels are handled in M1.
                }
                Clientbound::Disconnect(disconnect) => {
                    let reason = chat::nbt_component_plain_text(&disconnect.reason_nbt)
                        .unwrap_or_else(|| {
                            format!("unreadable NBT ({} bytes)", disconnect.reason_nbt.len())
                        });
                    return Err(Error::protocol(format!(
                        "server disconnected during configuration: {reason}"
                    )));
                }
                Clientbound::FinishConfiguration(_) => {
                    self.send("configuration", &FinishConfiguration.encode_serverbound())?;
                    return Ok(summary);
                }
                Clientbound::KeepAlive(keepalive) => {
                    self.send(
                        "configuration",
                        &KeepAlive { id: keepalive.id }.encode_serverbound(),
                    )?;
                }
                Clientbound::Ping(ping) => {
                    self.send("configuration", &Pong { id: ping.id }.encode())?;
                }
                Clientbound::ResetChat => {}
                Clientbound::RegistryData(data) => summary.registries.push(data.registry),
                Clientbound::RemoveResourcePack(_) => {}
                Clientbound::AddResourcePack(pack) => {
                    if pack.forced {
                        return Err(Error::protocol(format!(
                            "server requires resource pack {} which this M1 client cannot download",
                            format_uuid(pack.uuid)
                        )));
                    }
                }
                Clientbound::FeatureFlags(flags) => {
                    summary.feature_flags.extend(flags.flags);
                }
                Clientbound::KnownPacks(packs) => {
                    summary.known_packs = packs.packs;
                    // Answering with no packs makes the server send complete
                    // registry data instead of a known-pack delta.
                    self.send(
                        "configuration",
                        &KnownPacks::default().encode_serverbound()?,
                    )?;
                }
                Clientbound::Opaque { .. } => {}
            }
        }
        Err(Error::protocol(
            "configuration did not finish within the packet limit",
        ))
    }

    fn send(&mut self, state: &'static str, body: &[u8]) -> Result<()> {
        let id = packet_id(body)?;
        self.connection.write_packet(body)?;
        if self.trace_enabled {
            self.trace.push(TraceEntry {
                direction: Direction::Serverbound,
                state,
                id,
                len: body.len(),
            });
        }
        Ok(())
    }

    fn recv(&mut self, state: &'static str) -> Result<Vec<u8>> {
        let body = self.connection.read_packet()?;
        if self.trace_enabled {
            self.trace.push(TraceEntry {
                direction: Direction::Clientbound,
                state,
                id: packet_id(&body)?,
                len: body.len(),
            });
        }
        Ok(body)
    }
}

fn packet_id(body: &[u8]) -> Result<i32> {
    Reader::new(body).read_varint()
}

fn packet_payload(body: &[u8]) -> &[u8] {
    let mut reader = Reader::new(body);
    // `packet_id` already succeeded for this body.
    let _ = reader.read_varint();
    reader.read_rest()
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

#[cfg(test)]
mod tests {
    use super::*;
    use rkb_proto::GAME_VERSION;

    #[test]
    fn username_length_is_checked_in_utf16_units() {
        assert!(validate_username("Steve").is_ok());
        assert!(validate_username("").is_err());
        assert!(validate_username(&"x".repeat(16)).is_ok());
        assert!(validate_username(&"x".repeat(17)).is_err());
        // Four UTF-16 units, not four Unicode scalars.
        assert!(validate_username("😀😀").is_ok());
        assert!(validate_username(&"😀".repeat(9)).is_err());
    }

    #[test]
    fn uuid_formatting_is_hyphenated() {
        assert_eq!(
            format_uuid(0xB50A_D385_829D_3141_A216_7E7D_7539_BA7F),
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
    }

    #[test]
    fn protocol_and_data_version_match_the_verified_release() {
        let spec = rkb_mc::Versions::find(DATA_VERSION).expect("DataVersion 3955 is known");
        assert_eq!(spec.id, GAME_VERSION);
        assert_eq!(spec.protocol, PROTOCOL_VERSION);
        assert!(spec.is_verified());
    }
}
