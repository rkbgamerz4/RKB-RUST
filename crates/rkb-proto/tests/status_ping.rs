//! End-to-end status ping against a loopback mock server.
//!
//! The mock asserts the bytes a 1.21.1 server should receive. No external
//! server or Minecraft installation is involved.

use std::io::Write;
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use rkb_proto::codec::Reader;
use rkb_proto::frame::FrameCodec;
use rkb_proto::ids;
use rkb_proto::packets::{
    Handshake, NextState, PingRequest, PongResponse, StatusRequest, StatusResponse,
};
use rkb_proto::{ping, PROTOCOL_VERSION};

const STATUS_JSON: &str = r#"{
    "version": {"name": "1.21.1", "protocol": 767},
    "players": {"max": 20, "online": 3},
    "description": {"text": "RKB mock server"},
    "enforcesSecureChat": true
}"#;

fn next_packet(
    codec: &FrameCodec,
    stream: &mut std::net::TcpStream,
) -> Result<(i32, Vec<u8>), String> {
    let body = codec
        .read_frame(stream)
        .map_err(|error| error.to_string())?;
    let mut reader = Reader::new(&body);
    let id = reader.read_varint().map_err(|error| error.to_string())?;
    Ok((id, reader.read_rest().to_vec()))
}

#[test]
fn mock_server_receives_the_expected_status_exchange() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("local address").port();
    let server = thread::spawn(move || -> Result<(), String> {
        let (mut stream, _) = listener.accept().map_err(|error| error.to_string())?;
        let codec = FrameCodec::uncompressed();

        let (id, body) = next_packet(&codec, &mut stream)?;
        assert_eq!(id, ids::handshake::SERVERBOUND_INTENTION);
        let handshake = Handshake::decode(&body).map_err(|error| error.to_string())?;
        assert_eq!(handshake.protocol, PROTOCOL_VERSION);
        assert_eq!(handshake.host, "127.0.0.1");
        assert_eq!(handshake.port, port);
        assert_eq!(handshake.next_state, NextState::Status);

        let (id, body) = next_packet(&codec, &mut stream)?;
        assert_eq!(id, ids::status::SERVERBOUND_REQUEST);
        StatusRequest::decode(&body).map_err(|error| error.to_string())?;

        let response = StatusResponse {
            json: STATUS_JSON.to_owned(),
        };
        let response = response.encode().map_err(|error| error.to_string())?;
        codec
            .write_frame(&mut stream, &response)
            .map_err(|error| error.to_string())?;

        let (id, body) = next_packet(&codec, &mut stream)?;
        assert_eq!(id, ids::status::SERVERBOUND_PING);
        let request = PingRequest::decode(&body).map_err(|error| error.to_string())?;

        let pong = PongResponse {
            payload: request.payload,
        };
        codec
            .write_frame(&mut stream, &pong.encode())
            .map_err(|error| error.to_string())?;
        stream.flush().map_err(|error| error.to_string())?;
        Ok(())
    });

    let result = ping(&format!("127.0.0.1:{port}"), Duration::from_secs(5))
        .expect("mock ping should succeed");
    assert_eq!(result.status.version_name.as_deref(), Some("1.21.1"));
    assert_eq!(result.status.protocol, Some(PROTOCOL_VERSION));
    assert_eq!(result.status.players_online, Some(3));
    assert_eq!(result.status.players_max, Some(20));
    assert_eq!(
        result.status.description.as_deref(),
        Some("RKB mock server")
    );
    assert_eq!(result.status.enforces_secure_chat, Some(true));
    assert!(result.status.speaks_this_protocol());
    assert!(result.latency < Duration::from_secs(1));
    assert_eq!(result.raw_json, STATUS_JSON);

    server
        .join()
        .expect("mock server thread")
        .expect("mock server");
}
