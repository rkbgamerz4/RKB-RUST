//! Loopback mock-server tests for the M1 login/configuration state machine.
//!
//! The mock speaks protocol 767 byte-for-byte, including encryption and zlib
//! frames, so a passing test means the client interoperated with an
//! independent implementation of the wire format. It is not a vanilla-server
//! test; that evidence is recorded separately.

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rkb_mc::Nbt;
use rkb_net::{
    login, LoginOptions, SessionAuthenticator, DATA_VERSION, GAME_VERSION, PROTOCOL_VERSION,
};
use rkb_proto::codec::{Reader, Writer};
use rkb_proto::crypto::{md5, server_hash, RsaPrivateKey};
use rkb_proto::error::Result;
use rkb_proto::ids;
use rkb_proto::packets::{
    ClientInformation, CookieRequest, CookieResponse, Disconnect, EncryptionRequest,
    EncryptionResponse, FeatureFlags, FinishConfiguration, Handshake, KeepAlive, KnownPack,
    KnownPacks, LoginAcknowledged, LoginCookieRequest, LoginCookieResponse, LoginDisconnect,
    LoginPluginRequest, LoginPluginResponse, LoginStart, LoginSuccess, NextState, Ping, Pong,
    RegistryData, SetCompression,
};

const PUBLIC_DER: &[u8] = include_bytes!("fixtures/rsa_test_pub.der");
const AUTHENTICATED_UUID: u128 = 0xb50a_d385_829d_3141_a216_7e7d_7539_ba7f;
const MODULUS: &str = "df37d97634ac5fba3034ec94adf442f898a686c8d9a55505cbd73e4ff55fb2ad3e8e6d30becfb52b6b995f6c3c7be25dd28841d25cb8d39ef0ca0c63b597d2ffbc817972bd7867bfcabfaf95fd847341c6ede5b74f28cc27e945cce5232b9b3723cd6953739a2473a21d8c47d02a8884b8729f2e3189ba29f8c06e9a801061f5";
const PRIVATE_EXPONENT: &str = "30f6e754eea90136ae6c6d990faf8d214b1afc66029829d663a88b9aae9097793947b4dc047f3b9692bbca1acc09c0d623e08fcfbd572dd4a7ad2a9a94b3012d7eef6ba9f9124743f15453034f5fdbdc4e925a056ea81a159ae45327d297783401dd339d39ef167d781b59a5623609b8d9b67a47285d7f5ec8dfa9f4f0ebf801";

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
        .collect()
}

fn private_key() -> RsaPrivateKey {
    RsaPrivateKey::new(&unhex(MODULUS), &unhex(PRIVATE_EXPONENT)).unwrap()
}

type ServerResult = std::result::Result<(), String>;

fn next(
    connection: &mut rkb_net::Connection<TcpStream>,
) -> std::result::Result<(i32, Vec<u8>), String> {
    let body = connection.read_packet().map_err(|e| e.to_string())?;
    let mut reader = Reader::new(&body);
    let id = reader.read_varint().map_err(|e| e.to_string())?;
    Ok((id, reader.read_rest().to_vec()))
}

fn send(connection: &mut rkb_net::Connection<TcpStream>, body: &[u8]) -> ServerResult {
    connection.write_packet(body).map_err(|e| e.to_string())
}

fn accept_with_timeout(listener: &TcpListener) -> TcpStream {
    let (stream, _) = listener.accept().expect("accept");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
}

fn expect_login_start(
    connection: &mut rkb_net::Connection<TcpStream>,
) -> std::result::Result<LoginStart, String> {
    let (id, body) = next(connection)?;
    if id != ids::handshake::SERVERBOUND_INTENTION {
        return Err(format!("expected handshake, got 0x{id:02X}"));
    }
    let handshake = Handshake::decode(&body).map_err(|e| e.to_string())?;
    if handshake.protocol != PROTOCOL_VERSION {
        return Err(format!(
            "protocol {} != {PROTOCOL_VERSION}",
            handshake.protocol
        ));
    }
    if handshake.next_state != NextState::Login {
        return Err("handshake did not request login".to_owned());
    }

    let (id, body) = next(connection)?;
    if id != ids::login::serverbound::HELLO {
        return Err(format!("expected LoginStart, got 0x{id:02X}"));
    }
    LoginStart::decode(&body).map_err(|e| e.to_string())
}

fn login_success(uuid: u128, name: &str) -> Vec<u8> {
    LoginSuccess {
        uuid,
        name: name.to_owned(),
        properties: vec![],
        strict_error_handling: true,
    }
    .encode()
    .unwrap()
}

fn anonymous_nbt_text(text: &str) -> Vec<u8> {
    use std::collections::BTreeMap;
    let mut root = BTreeMap::new();
    root.insert("text".to_owned(), rkb_mc::Tag::String(text.to_owned()));
    let named = Nbt::write(&rkb_mc::Tag::Compound(root)).unwrap();
    let mut anonymous = Vec::with_capacity(named.len() - 2);
    anonymous.push(named[0]);
    anonymous.extend_from_slice(&named[3..]);
    anonymous
}

fn configuration(
    connection: &mut rkb_net::Connection<TcpStream>,
    expected_known_packs: bool,
) -> ServerResult {
    // The client sends its settings immediately after LoginAcknowledged.
    let (id, body) = next(connection)?;
    if id != ids::configuration::serverbound::CLIENT_INFORMATION {
        return Err(format!("expected ClientInformation, got 0x{id:02X}"));
    }
    let information = ClientInformation::decode(&body).map_err(|e| e.to_string())?;
    if information.locale != "en_us" || information.view_distance != 8 {
        return Err(format!("unexpected client information {information:?}"));
    }

    if expected_known_packs {
        let packs = KnownPacks {
            packs: vec![KnownPack {
                namespace: "minecraft".to_owned(),
                id: "core".to_owned(),
                version: GAME_VERSION.to_owned(),
            }],
        };
        send(
            connection,
            &packs.encode_clientbound().map_err(|e| e.to_string())?,
        )?;
        let (id, body) = next(connection)?;
        if id != ids::configuration::serverbound::SELECT_KNOWN_PACKS {
            return Err(format!("expected SelectKnownPacks, got 0x{id:02X}"));
        }
        let selected = KnownPacks::decode(&body).map_err(|e| e.to_string())?;
        if !selected.packs.is_empty() {
            return Err("client should select no packs to force full registries".to_owned());
        }
    }

    let registry = RegistryData {
        registry: "minecraft:dimension_type".to_owned(),
        entry_count: 1,
        entries_data: vec![0x0A, 0x00, 0x00],
    };
    send(connection, &registry.encode().map_err(|e| e.to_string())?)?;
    let flags = FeatureFlags {
        flags: vec!["minecraft:vanilla".to_owned()],
    };
    send(connection, &flags.encode().map_err(|e| e.to_string())?)?;

    send(
        connection,
        &KeepAlive { id: 0x1122_3344 }.encode_clientbound(),
    )?;
    let (id, body) = next(connection)?;
    if id != ids::configuration::serverbound::KEEP_ALIVE {
        return Err(format!("expected KeepAlive response, got 0x{id:02X}"));
    }
    if KeepAlive::decode(&body).map_err(|e| e.to_string())?.id != 0x1122_3344 {
        return Err("KeepAlive id did not round-trip".to_owned());
    }

    send(connection, &Ping { id: -5 }.encode())?;
    let (id, body) = next(connection)?;
    if id != ids::configuration::serverbound::PONG {
        return Err(format!("expected Pong, got 0x{id:02X}"));
    }
    if Pong::decode(&body).map_err(|e| e.to_string())?.id != -5 {
        return Err("Pong id did not round-trip".to_owned());
    }

    send(connection, &FinishConfiguration.encode_clientbound())?;
    let (id, body) = next(connection)?;
    if id != ids::configuration::serverbound::FINISH_CONFIGURATION {
        return Err(format!("expected FinishConfiguration ack, got 0x{id:02X}"));
    }
    FinishConfiguration::decode(&body).map_err(|e| e.to_string())?;
    Ok(())
}

fn run(
    listener: TcpListener,
    server: impl FnOnce(&mut rkb_net::Connection<TcpStream>) -> ServerResult + Send + 'static,
) -> (u16, JoinHandle<ServerResult>) {
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let stream = accept_with_timeout(&listener);
        let mut connection = rkb_net::Connection::new(stream);
        server(&mut connection)
    });
    (port, handle)
}

#[test]
fn offline_login_reaches_finish_configuration() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (port, server) = run(listener, |connection| {
        let start = expect_login_start(connection)?;
        if start.name != "Steve" {
            return Err("wrong player name".to_owned());
        }
        if start.uuid != md5::offline_uuid("Steve") {
            return Err("wrong offline UUID".to_owned());
        }

        // Login plugin request: the client must answer "not understood".
        let plugin = LoginPluginRequest {
            message_id: 3,
            channel: "rkb:test".to_owned(),
            data: b"ask".to_vec(),
        };
        send(connection, &plugin.encode().map_err(|e| e.to_string())?)?;
        let (id, body) = next(connection)?;
        if id != ids::login::serverbound::PLUGIN_RESPONSE {
            return Err(format!("expected LoginPluginResponse, got 0x{id:02X}"));
        }
        let response = LoginPluginResponse::decode(&body).map_err(|e| e.to_string())?;
        if response.message_id != 3 || response.data.is_some() {
            return Err("unexpected plugin response".to_owned());
        }

        // Login cookie request: the client has no cookie to send.
        let cookie = LoginCookieRequest {
            key: "rkb:session".to_owned(),
        };
        send(connection, &cookie.encode().map_err(|e| e.to_string())?)?;
        let (id, body) = next(connection)?;
        if id != ids::login::serverbound::COOKIE_RESPONSE {
            return Err(format!("expected LoginCookieResponse, got 0x{id:02X}"));
        }
        let response = LoginCookieResponse::decode(&body).map_err(|e| e.to_string())?;
        if response.key != "rkb:session" || response.payload.is_some() {
            return Err("unexpected cookie response".to_owned());
        }

        let compression = SetCompression { threshold: 64 };
        send(connection, &compression.encode())?;
        connection.set_compression(Some(64));

        send(
            connection,
            &login_success(md5::offline_uuid("Steve"), "Steve"),
        )?;
        let (id, body) = next(connection)?;
        if id != ids::login::serverbound::LOGIN_ACKNOWLEDGED {
            return Err(format!("expected LoginAcknowledged, got 0x{id:02X}"));
        }
        LoginAcknowledged::decode(&body).map_err(|e| e.to_string())?;

        configuration(connection, true)
    });

    let outcome = login(
        &format!("127.0.0.1:{port}"),
        Duration::from_secs(10),
        &LoginOptions::new("Steve"),
        None,
    )
    .expect("offline login should complete");

    assert_eq!(outcome.name, "Steve");
    assert_eq!(outcome.uuid, md5::offline_uuid("Steve"));
    assert_eq!(outcome.protocol, PROTOCOL_VERSION);
    assert_eq!(outcome.data_version, DATA_VERSION);
    assert!(!outcome.encrypted);
    assert!(!outcome.session_authenticated);
    assert_eq!(outcome.compression, Some(64));
    assert!(outcome.strict_error_handling);
    assert_eq!(outcome.registries, vec!["minecraft:dimension_type"]);
    assert_eq!(outcome.feature_flags, vec!["minecraft:vanilla"]);
    assert_eq!(outcome.known_packs.len(), 1);
    assert_eq!(outcome.known_packs[0].id, "core");

    // The trace must show the state transition and the finish ack.
    assert!(outcome
        .trace
        .iter()
        .any(|entry| entry.state == "configuration" && entry.id == 0x03));
    assert!(outcome
        .trace
        .iter()
        .any(|entry| entry.state == "login"
            && entry.id == ids::login::serverbound::LOGIN_ACKNOWLEDGED));

    server.join().unwrap().expect("mock server");
}

#[test]
fn encrypted_login_without_authentication_completes() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (port, server) = run(listener, |connection| {
        let start = expect_login_start(connection)?;
        let request = EncryptionRequest {
            server_id: String::new(),
            public_key: PUBLIC_DER.to_vec(),
            verify_token: vec![0xAA, 0xBB, 0xCC, 0xDD],
            should_authenticate: false,
        };
        send(connection, &request.encode().map_err(|e| e.to_string())?)?;

        let (id, body) = next(connection)?;
        if id != ids::login::serverbound::ENCRYPTION_RESPONSE {
            return Err(format!("expected EncryptionResponse, got 0x{id:02X}"));
        }
        let response = EncryptionResponse::decode(&body).map_err(|e| e.to_string())?;
        let private = private_key();
        let secret = private
            .decrypt(&response.shared_secret)
            .map_err(|e| e.to_string())?;
        let token = private
            .decrypt(&response.verify_token)
            .map_err(|e| e.to_string())?;
        if secret.len() != 16 {
            return Err(format!("shared secret is {} bytes", secret.len()));
        }
        if token != [0xAA, 0xBB, 0xCC, 0xDD] {
            return Err("verify token did not decrypt".to_owned());
        }
        let mut key = [0u8; 16];
        key.copy_from_slice(&secret);
        connection.enable_encryption(&key);

        send(connection, &SetCompression { threshold: 256 }.encode())?;
        connection.set_compression(Some(256));
        send(
            connection,
            &login_success(md5::offline_uuid(&start.name), &start.name),
        )?;
        let (id, body) = next(connection)?;
        if id != ids::login::serverbound::LOGIN_ACKNOWLEDGED {
            return Err(format!("expected LoginAcknowledged, got 0x{id:02X}"));
        }
        LoginAcknowledged::decode(&body).map_err(|e| e.to_string())?;
        configuration(connection, true)
    });

    let outcome = login(
        &format!("127.0.0.1:{port}"),
        Duration::from_secs(10),
        &LoginOptions::new("Alex"),
        None,
    )
    .expect("encrypted offline-mode login should complete");

    assert!(outcome.encrypted);
    assert!(!outcome.session_authenticated);
    assert_eq!(outcome.compression, Some(256));
    assert_eq!(outcome.name, "Alex");
    server.join().unwrap().expect("mock server");
}

#[test]
fn online_mode_without_an_authenticator_fails_explicitly() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (port, server) = run(listener, |connection| {
        expect_login_start(connection)?;
        let request = EncryptionRequest {
            server_id: String::new(),
            public_key: PUBLIC_DER.to_vec(),
            verify_token: vec![1, 2, 3, 4],
            should_authenticate: true,
        };
        send(connection, &request.encode().map_err(|e| e.to_string())?)?;
        // The client must close without sending an EncryptionResponse.
        let _ = next(connection);
        Ok(())
    });

    let error = login(
        &format!("127.0.0.1:{port}"),
        Duration::from_secs(5),
        &LoginOptions::new("Steve"),
        None,
    )
    .expect_err("online mode without auth must fail");
    let message = error.to_string();
    assert!(
        message.contains("online-mode authentication"),
        "unexpected error: {message}"
    );
    server.join().unwrap().expect("mock server");
}

struct RecordingAuthenticator {
    hash: Arc<Mutex<Option<String>>>,
}

impl SessionAuthenticator for RecordingAuthenticator {
    fn join_server(&mut self, server_hash: &str) -> Result<()> {
        *self.hash.lock().unwrap() = Some(server_hash.to_owned());
        Ok(())
    }
}

#[test]
fn authenticated_login_uses_the_vanilla_server_hash() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let recorded = Arc::new(Mutex::new(None));
    let server_recorded = Arc::clone(&recorded);

    let (port, server) = run(listener, move |connection| {
        let start = expect_login_start(connection)?;
        if start.uuid != AUTHENTICATED_UUID {
            return Err(
                "LoginStart sent an offline UUID instead of the authenticated profile UUID"
                    .to_owned(),
            );
        }
        let request = EncryptionRequest {
            server_id: String::new(),
            public_key: PUBLIC_DER.to_vec(),
            verify_token: vec![9, 8, 7, 6],
            should_authenticate: true,
        };
        send(connection, &request.encode().map_err(|e| e.to_string())?)?;

        let (id, body) = next(connection)?;
        if id != ids::login::serverbound::ENCRYPTION_RESPONSE {
            return Err(format!("expected EncryptionResponse, got 0x{id:02X}"));
        }
        let response = EncryptionResponse::decode(&body).map_err(|e| e.to_string())?;
        let private = private_key();
        let secret = private
            .decrypt(&response.shared_secret)
            .map_err(|e| e.to_string())?;
        let token = private
            .decrypt(&response.verify_token)
            .map_err(|e| e.to_string())?;
        if secret.len() != 16 || token != [9, 8, 7, 6] {
            return Err("encryption response is malformed".to_owned());
        }

        let expected = server_hash("", &secret, PUBLIC_DER);
        let actual = server_recorded.lock().unwrap().clone();
        if actual.as_deref() != Some(expected.as_str()) {
            return Err(format!(
                "server hash mismatch: authenticator saw {actual:?}, expected {expected}"
            ));
        }

        let mut key = [0u8; 16];
        key.copy_from_slice(&secret);
        connection.enable_encryption(&key);
        send(connection, &SetCompression { threshold: 16 }.encode())?;
        connection.set_compression(Some(16));
        send(connection, &login_success(AUTHENTICATED_UUID, &start.name))?;
        let (id, body) = next(connection)?;
        if id != ids::login::serverbound::LOGIN_ACKNOWLEDGED {
            return Err(format!("expected LoginAcknowledged, got 0x{id:02X}"));
        }
        LoginAcknowledged::decode(&body).map_err(|e| e.to_string())?;
        configuration(connection, true)
    });

    let mut authenticator = RecordingAuthenticator {
        hash: Arc::clone(&recorded),
    };
    let outcome = login(
        &format!("127.0.0.1:{port}"),
        Duration::from_secs(10),
        &LoginOptions::new("Notch").with_uuid(AUTHENTICATED_UUID),
        Some(&mut authenticator),
    )
    .expect("authenticated login should complete");

    assert!(outcome.encrypted);
    assert!(outcome.session_authenticated);
    assert_eq!(outcome.uuid, AUTHENTICATED_UUID);
    assert_eq!(outcome.compression, Some(16));
    assert!(recorded.lock().unwrap().is_some());
    server.join().unwrap().expect("mock server");
}

#[test]
fn declared_size_below_the_threshold_is_rejected() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (port, server) = run(listener, |connection| {
        expect_login_start(connection)?;
        send(connection, &SetCompression { threshold: 64 }.encode())?;
        connection.set_compression(Some(64));
        // Deliberately lie: declared 10 bytes is below the negotiated 64.
        let mut contents = Writer::new();
        contents.write_varint(10);
        contents.write_bytes(&[0u8; 8]);
        let contents = contents.into_inner();
        let mut frame = Writer::new();
        frame.write_varint(contents.len() as i32);
        frame.write_bytes(&contents);
        connection
            .get_mut()
            .write_all(&frame.into_inner())
            .map_err(|e| e.to_string())?;
        Ok(())
    });

    let error = login(
        &format!("127.0.0.1:{port}"),
        Duration::from_secs(5),
        &LoginOptions::new("Steve"),
        None,
    )
    .expect_err("an undersized declared frame must fail");
    let message = error.to_string();
    assert!(
        message.contains("below the threshold"),
        "unexpected error: {message}"
    );
    server.join().unwrap().expect("mock server");
}

#[test]
fn malformed_zlib_payload_is_rejected() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (port, server) = run(listener, |connection| {
        expect_login_start(connection)?;
        send(connection, &SetCompression { threshold: 64 }.encode())?;
        connection.set_compression(Some(64));
        let mut contents = Writer::new();
        contents.write_varint(64); // at the threshold, so it must be zlib
        contents.write_bytes(b"not zlib");
        let contents = contents.into_inner();
        let mut frame = Writer::new();
        frame.write_varint(contents.len() as i32);
        frame.write_bytes(&contents);
        connection
            .get_mut()
            .write_all(&frame.into_inner())
            .map_err(|e| e.to_string())?;
        Ok(())
    });

    let error = login(
        &format!("127.0.0.1:{port}"),
        Duration::from_secs(5),
        &LoginOptions::new("Steve"),
        None,
    )
    .expect_err("a malformed zlib payload must fail");
    assert!(
        error.to_string().to_lowercase().contains("compression"),
        "unexpected error: {error}"
    );
    server.join().unwrap().expect("mock server");
}

#[test]
fn login_disconnect_reason_is_decoded() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (port, server) = run(listener, |connection| {
        expect_login_start(connection)?;
        let disconnect = LoginDisconnect {
            reason_json: r#"{"text":"server is in maintenance"}"#.to_owned(),
        };
        send(connection, &disconnect.encode().map_err(|e| e.to_string())?)
    });

    let error = login(
        &format!("127.0.0.1:{port}"),
        Duration::from_secs(5),
        &LoginOptions::new("Steve"),
        None,
    )
    .expect_err("a login disconnect must fail");
    let message = error.to_string();
    assert!(
        message.contains("server is in maintenance"),
        "unexpected error: {message}"
    );
    server.join().unwrap().expect("mock server");
}

#[test]
fn configuration_disconnect_nbt_reason_is_decoded() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (port, server) = run(listener, |connection| {
        expect_login_start(connection)?;
        send(
            connection,
            &login_success(md5::offline_uuid("Steve"), "Steve"),
        )?;
        let (id, body) = next(connection)?;
        if id != ids::login::serverbound::LOGIN_ACKNOWLEDGED {
            return Err(format!("expected LoginAcknowledged, got 0x{id:02X}"));
        }
        LoginAcknowledged::decode(&body).map_err(|e| e.to_string())?;

        let (id, _) = next(connection)?;
        if id != ids::configuration::serverbound::CLIENT_INFORMATION {
            return Err(format!("expected ClientInformation, got 0x{id:02X}"));
        }
        let disconnect = Disconnect {
            reason_nbt: anonymous_nbt_text("configuration closed"),
        };
        send(connection, &disconnect.encode().map_err(|e| e.to_string())?)
    });

    let error = login(
        &format!("127.0.0.1:{port}"),
        Duration::from_secs(5),
        &LoginOptions::new("Steve"),
        None,
    )
    .expect_err("a configuration disconnect must fail");
    let message = error.to_string();
    assert!(
        message.contains("configuration closed"),
        "unexpected error: {message}"
    );
    server.join().unwrap().expect("mock server");
}

#[test]
fn unknown_login_packet_is_rejected() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (port, server) = run(listener, |connection| {
        expect_login_start(connection)?;
        let mut writer = Writer::new();
        writer.write_varint(0x7F);
        send(connection, writer.as_slice())
    });

    let error = login(
        &format!("127.0.0.1:{port}"),
        Duration::from_secs(5),
        &LoginOptions::new("Steve"),
        None,
    )
    .expect_err("an unknown login packet must fail");
    assert!(
        matches!(
            error,
            rkb_proto::Error::UnknownPacket {
                state: "login",
                id: 0x7F
            }
        ),
        "unexpected error: {error}"
    );
    server.join().unwrap().expect("mock server");
}

#[test]
fn cookie_and_plugin_packet_shapes_are_exercised() {
    // Packet-level round trips for the two login extras, independent of a
    // socket. This keeps the negative-path tests above from being the only
    // coverage.
    let plugin = LoginPluginRequest {
        message_id: 1,
        channel: "minecraft:brand".to_owned(),
        data: b"x".to_vec(),
    };
    let encoded = plugin.encode().unwrap();
    let mut reader = Reader::new(&encoded);
    assert_eq!(
        reader.read_varint().unwrap(),
        ids::login::clientbound::PLUGIN_REQUEST
    );
    let request = LoginPluginRequest::decode(reader.read_rest()).unwrap();
    assert_eq!(request.data, b"x");

    let cookie = CookieRequest {
        key: "rkb:x".to_owned(),
    };
    let encoded = cookie.encode().unwrap();
    let mut reader = Reader::new(&encoded);
    assert_eq!(
        reader.read_varint().unwrap(),
        ids::configuration::clientbound::COOKIE_REQUEST
    );
    assert_eq!(CookieRequest::decode(reader.read_rest()).unwrap(), cookie);

    let response = CookieResponse {
        key: "rkb:x".to_owned(),
        payload: None,
    };
    let encoded = response.encode().unwrap();
    let mut reader = Reader::new(&encoded);
    assert_eq!(
        reader.read_varint().unwrap(),
        ids::configuration::serverbound::COOKIE_RESPONSE
    );
    assert_eq!(
        CookieResponse::decode(reader.read_rest()).unwrap(),
        response
    );
}
