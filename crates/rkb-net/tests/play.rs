//! Loopback test for the play-state handoff.
//!
//! An independent mock server speaks protocol 767 login, configuration, and
//! play. It proves that `login_configured` keeps the connection in the right
//! state, that the client decodes a server-sent chunk, and that it answers the
//! play packets that require an answer. Vanilla-server evidence is recorded in
//! `docs/m3-verification.md`.

use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rkb_mc::{BlockRegistry, Nbt, Tag};
use rkb_net::{login_configured, Event, LoginOptions, PlaySession, ServerWorld};
use rkb_proto::codec::{Reader, Writer};
use rkb_proto::ids;
use rkb_proto::packets::play::{
    BlockChange, BlockPos, ChunkData, JoinGame, KickDisconnect, LightData, PalettedContainer, Slot,
    SpawnInfo, SyncPlayerPosition,
};
use rkb_proto::packets::{
    ClientInformation, FeatureFlags, FinishConfiguration, LoginSuccess, RegistryData,
};

type ServerResult = std::result::Result<(), String>;

fn next(
    connection: &mut rkb_net::Connection<TcpStream>,
) -> std::result::Result<(i32, Vec<u8>), String> {
    let body = connection
        .read_packet()
        .map_err(|error| error.to_string())?;
    let mut reader = Reader::new(&body);
    let id = reader.read_varint().map_err(|error| error.to_string())?;
    Ok((id, reader.read_rest().to_vec()))
}

fn send(connection: &mut rkb_net::Connection<TcpStream>, body: &[u8]) -> ServerResult {
    connection
        .write_packet(body)
        .map_err(|error| error.to_string())
}

fn anonymous_compound(fields: &[(&str, Tag)]) -> Vec<u8> {
    let mut root = std::collections::BTreeMap::new();
    for (key, value) in fields {
        root.insert((*key).to_owned(), value.clone());
    }
    let named = Nbt::write(&Tag::Compound(root)).unwrap();
    let mut anonymous = Vec::with_capacity(named.len() - 2);
    anonymous.push(named[0]);
    anonymous.extend_from_slice(&named[3..]);
    anonymous
}

fn dimension_registry() -> RegistryData {
    let nbt = anonymous_compound(&[
        ("min_y", Tag::Int(0)),
        ("height", Tag::Int(16)),
        ("has_skylight", Tag::Byte(1)),
    ]);
    let mut entries = Vec::new();
    let mut writer = Writer::new();
    writer.write_string("minecraft:overworld", 32_767).unwrap();
    writer.write_bool(true);
    writer.write_bytes(&nbt);
    entries.extend_from_slice(writer.as_slice());
    RegistryData {
        registry: "minecraft:dimension_type".to_owned(),
        entry_count: 1,
        entries_data: entries,
    }
}

fn chunk_packet() -> ChunkData {
    let mut data = Writer::new();
    data.write_i16(4096);
    PalettedContainer::Single(1)
        .encode(&mut data, 4096)
        .unwrap();
    PalettedContainer::Single(0).encode(&mut data, 64).unwrap();
    ChunkData {
        x: 2,
        z: -3,
        heightmaps: anonymous_compound(&[]),
        data: data.into_inner(),
        block_entities: Vec::new(),
        light: LightData::default(),
    }
}

fn join_game() -> JoinGame {
    JoinGame {
        entity_id: 5,
        hardcore: false,
        world_names: vec!["minecraft:overworld".to_owned()],
        max_players: 20,
        view_distance: 10,
        simulation_distance: 8,
        reduced_debug_info: false,
        enable_respawn_screen: true,
        do_limited_crafting: false,
        spawn: SpawnInfo {
            dimension_type: 0,
            dimension_name: "minecraft:overworld".to_owned(),
            hashed_seed: -1,
            game_mode: 0,
            previous_game_mode: -1,
            is_debug: false,
            is_flat: true,
            death: None,
            portal_cooldown: 0,
        },
        enforces_secure_chat: true,
    }
}

fn configuration(connection: &mut rkb_net::Connection<TcpStream>) -> ServerResult {
    let (id, body) = next(connection)?;
    if id != ids::configuration::serverbound::CLIENT_INFORMATION {
        return Err(format!("expected ClientInformation, got 0x{id:02X}"));
    }
    ClientInformation::decode(&body).map_err(|error| error.to_string())?;

    send(
        connection,
        &dimension_registry()
            .encode()
            .map_err(|error| error.to_string())?,
    )?;
    send(
        connection,
        &FeatureFlags {
            flags: vec!["minecraft:vanilla".to_owned()],
        }
        .encode()
        .map_err(|error| error.to_string())?,
    )?;
    send(connection, &FinishConfiguration.encode_clientbound())?;
    let (id, _) = next(connection)?;
    if id != ids::configuration::serverbound::FINISH_CONFIGURATION {
        return Err(format!("expected FinishConfiguration ack, got 0x{id:02X}"));
    }
    Ok(())
}

fn run(
    listener: TcpListener,
    server: impl FnOnce(&mut rkb_net::Connection<TcpStream>) -> ServerResult + Send + 'static,
) -> (u16, JoinHandle<ServerResult>) {
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().map_err(|error| error.to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .map_err(|error| error.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .map_err(|error| error.to_string())?;
        let mut connection = rkb_net::Connection::new(stream);
        server(&mut connection)
    });
    (port, handle)
}

#[test]
fn configured_session_continues_into_the_play_state() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (port, server) = run(listener, |connection| {
        // Login.
        let (id, _) = next(connection)?;
        if id != ids::handshake::SERVERBOUND_INTENTION {
            return Err(format!("expected handshake, got 0x{id:02X}"));
        }
        let (id, body) = next(connection)?;
        if id != ids::login::serverbound::HELLO {
            return Err(format!("expected LoginStart, got 0x{id:02X}"));
        }
        rkb_proto::packets::LoginStart::decode(&body).map_err(|error| error.to_string())?;
        send(
            connection,
            &LoginSuccess {
                uuid: 1,
                name: "PlayTester".to_owned(),
                properties: Vec::new(),
                strict_error_handling: true,
            }
            .encode()
            .map_err(|error| error.to_string())?,
        )?;
        let (id, _) = next(connection)?;
        if id != ids::login::serverbound::LOGIN_ACKNOWLEDGED {
            return Err(format!("expected LoginAcknowledged, got 0x{id:02X}"));
        }

        configuration(connection)?;

        // Play. Join, then a server chunk, then a teleport that must be
        // confirmed, then a keep-alive that must be answered.
        send(
            connection,
            &join_game().encode().map_err(|e| e.to_string())?,
        )?;
        send(
            connection,
            &chunk_packet().encode().map_err(|e| e.to_string())?,
        )?;
        send(
            connection,
            &SyncPlayerPosition {
                x: 32.5,
                y: 4.0,
                z: -40.5,
                yaw: 0.0,
                pitch: 0.0,
                flags: 0,
                teleport_id: 11,
            }
            .encode()
            .map_err(|e| e.to_string())?,
        )?;
        let (id, body) = next(connection)?;
        if id != ids::play::serverbound::TELEPORT_CONFIRM {
            return Err(format!("expected TeleportConfirm, got 0x{id:02X}"));
        }
        if Reader::new(&body)
            .read_varint()
            .map_err(|e| e.to_string())?
            != 11
        {
            return Err("teleport id did not round-trip".to_owned());
        }

        send(connection, &keep_alive(1234))?;
        let (id, body) = next(connection)?;
        if id != ids::play::serverbound::KEEP_ALIVE {
            return Err(format!("expected KeepAlive reply, got 0x{id:02X}"));
        }
        if Reader::new(&body).read_i64().map_err(|e| e.to_string())? != 1234 {
            return Err("keep-alive id did not round-trip".to_owned());
        }

        // Announce the end from the server side, then close.
        let reason = anonymous_compound(&[("text", Tag::String("done".to_owned()))]);
        send(
            connection,
            &rkb_proto::packets::play::KickDisconnect { reason_nbt: reason }
                .encode()
                .map_err(|e| e.to_string())?,
        )?;
        Ok(())
    });

    let configured = login_configured(
        &format!("127.0.0.1:{port}"),
        Duration::from_secs(10),
        &LoginOptions::new("PlayTester"),
        None,
    )
    .expect("login and configuration");
    assert_eq!(configured.outcome().name, "PlayTester");
    assert_eq!(configured.registries().len(), 1);

    let mut session = PlaySession::from_configured(configured).expect("play session");
    assert_eq!(session.dimensions()[&0].height, 16);

    assert!(matches!(
        session.next_event().unwrap(),
        Event::Joined(ref packet) if packet.entity_id == 5
    ));
    assert_eq!(session.block_state(0, 0, 0), None);
    assert!(matches!(
        session.next_event().unwrap(),
        Event::ChunkLoaded {
            x: 2,
            z: -3,
            sections: 1,
            non_air_blocks: 4096,
            ..
        }
    ));
    // World coordinates inside the sent column (chunk 2, -3 spans X 32..47 and
    // Z -48..-33).
    assert_eq!(session.block_state(32, 0, -48), Some(1));
    assert_eq!(session.block_state(47, 15, -33), Some(1));

    assert!(matches!(
        session.next_event().unwrap(),
        Event::Position(position) if position.teleport_id == 11
    ));
    assert_eq!(session.player().x, 32.5);
    assert_eq!(session.player().z, -40.5);
    assert!(session.player().has_position);

    assert_eq!(session.next_event().unwrap(), Event::KeepAlive { id: 1234 });
    assert_eq!(
        session.next_event().unwrap(),
        Event::Disconnected {
            reason: "done".to_owned()
        }
    );
    assert!(matches!(
        session.next_event().unwrap(),
        Event::Closed { .. }
    ));

    let stats = session.stats();
    assert_eq!(stats.keep_alives, 1);
    assert_eq!(stats.packets, 5);
    assert_eq!(stats.unknown_packets, 0);

    server.join().unwrap().unwrap();
}

fn keep_alive(id: i64) -> Vec<u8> {
    let mut writer = Writer::new();
    writer.write_varint(ids::play::clientbound::KEEP_ALIVE);
    writer.write_i64(id);
    writer.into_inner()
}

#[test]
fn inventory_components_are_reported_not_guessed() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (port, server) = run(listener, |connection| {
        let (_, _) = next(connection)?;
        let (_, _) = next(connection)?;
        send(
            connection,
            &LoginSuccess {
                uuid: 1,
                name: "PlayTester".to_owned(),
                properties: Vec::new(),
                strict_error_handling: true,
            }
            .encode()
            .map_err(|error| error.to_string())?,
        )?;
        let (_, _) = next(connection)?;
        configuration(connection)?;
        send(
            connection,
            &join_game().encode().map_err(|e| e.to_string())?,
        )?;
        // SetContainerSlot for window 0, slot 0, item 1 with one added data
        // component. M3 must not guess the component.
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::SET_SLOT);
        writer.write_u8(0);
        writer.write_varint(1);
        writer.write_i16(0);
        writer.write_varint(1);
        writer.write_varint(1);
        writer.write_varint(1);
        writer.write_varint(0);
        send(connection, &writer.into_inner())?;
        // Keep the connection open long enough for the client to report.
        thread::sleep(Duration::from_millis(100));
        Ok(())
    });

    let configured = login_configured(
        &format!("127.0.0.1:{port}"),
        Duration::from_secs(10),
        &LoginOptions::new("PlayTester"),
        None,
    )
    .unwrap();
    let mut session = PlaySession::from_configured(configured).unwrap();
    session.next_event().unwrap();
    let event = session.next_event().unwrap();
    assert!(matches!(
        event,
        Event::Unsupported {
            packet: "SetContainerSlot",
            ..
        }
    ));
    assert_eq!(session.player().inventory, Vec::<Slot>::new());
    assert_eq!(session.stats().unsupported_packets, 1);

    server.join().unwrap().unwrap();
}

fn small_registry() -> BlockRegistry {
    BlockRegistry::from_json(
        r#"{
            "minecraft:air": { "states": [{ "id": 0, "default": true }] },
            "minecraft:stone": { "states": [{ "id": 1, "default": true }] },
            "minecraft:glowstone": { "states": [{ "id": 2, "default": true }] }
        }"#,
    )
    .unwrap()
}

#[test]
fn server_world_feeds_chunks_positions_and_block_updates() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (port, server) = run(listener, |connection| {
        let (id, _) = next(connection)?;
        if id != ids::handshake::SERVERBOUND_INTENTION {
            return Err(format!("expected handshake, got 0x{id:02X}"));
        }
        let (id, body) = next(connection)?;
        if id != ids::login::serverbound::HELLO {
            return Err(format!("expected LoginStart, got 0x{id:02X}"));
        }
        rkb_proto::packets::LoginStart::decode(&body).map_err(|error| error.to_string())?;
        send(
            connection,
            &LoginSuccess {
                uuid: 2,
                name: "WorldTester".to_owned(),
                properties: Vec::new(),
                strict_error_handling: true,
            }
            .encode()
            .map_err(|error| error.to_string())?,
        )?;
        let (id, _) = next(connection)?;
        if id != ids::login::serverbound::LOGIN_ACKNOWLEDGED {
            return Err(format!("expected LoginAcknowledged, got 0x{id:02X}"));
        }
        configuration(connection)?;
        send(
            connection,
            &join_game().encode().map_err(|error| error.to_string())?,
        )?;
        send(
            connection,
            &chunk_packet().encode().map_err(|error| error.to_string())?,
        )?;
        send(
            connection,
            &SyncPlayerPosition {
                x: 32.5,
                y: 4.0,
                z: -40.5,
                yaw: 90.0,
                pitch: -10.0,
                flags: 0,
                teleport_id: 21,
            }
            .encode()
            .map_err(|error| error.to_string())?,
        )?;
        let (id, _) = next(connection)?;
        if id != ids::play::serverbound::TELEPORT_CONFIRM {
            return Err(format!("expected TeleportConfirm, got 0x{id:02X}"));
        }
        send(
            connection,
            &BlockChange {
                position: BlockPos::new(32, 0, -48),
                state_id: 2,
            }
            .encode()
            .map_err(|error| error.to_string())?,
        )?;
        send(connection, &keep_alive(77))?;
        let (id, body) = next(connection)?;
        if id != ids::play::serverbound::KEEP_ALIVE {
            return Err(format!("expected KeepAlive reply, got 0x{id:02X}"));
        }
        if Reader::new(&body).read_i64().map_err(|e| e.to_string())? != 77 {
            return Err("keep-alive id did not round-trip".to_owned());
        }
        let reason = anonymous_compound(&[("text", Tag::String("world done".to_owned()))]);
        send(
            connection,
            &KickDisconnect { reason_nbt: reason }
                .encode()
                .map_err(|error| error.to_string())?,
        )?;
        Ok(())
    });

    let configured = login_configured(
        &format!("127.0.0.1:{port}"),
        Duration::from_secs(10),
        &LoginOptions::new("WorldTester"),
        None,
    )
    .expect("login and configuration");
    let mut world =
        ServerWorld::from_configured(configured, small_registry()).expect("server world");

    let position = world.position().expect("first position");
    assert_eq!((position.x, position.y, position.z), (32.5, 4.0, -40.5));
    assert_eq!((position.yaw, position.pitch), (90.0, -10.0));
    assert!(world.take_position().is_some());
    assert!(world.take_position().is_none());

    let mut converted = false;
    let mut updated = false;
    for _ in 0..200 {
        let dirty = world.poll();
        converted |= dirty.contains(&(2, -3));
        if world.block_at(32, 0, -48) == Some(rkb_mc::BlockState::new("minecraft:glowstone")) {
            updated = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(converted, "chunk (2, -3) was never reported dirty");
    assert!(updated, "block update was not applied");
    assert_eq!(world.loaded_chunks(), 1);
    assert!(world.chunks_received() >= 1);
    assert!(world.block_updates() >= 1);
    assert_eq!(
        world.block_at(47, 15, -33),
        Some(rkb_mc::BlockState::new("minecraft:stone"))
    );
    assert_eq!(world.block_at(0, 0, 0), None);

    for _ in 0..200 {
        world.poll();
        if world.disconnect_reason().is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(world.disconnect_reason(), Some("world done"));
    server.join().unwrap().unwrap();
}
