//! Handshake state: the first packet of every connection.

use crate::codec::{Reader, Writer};
use crate::error::{Error, Result};
use crate::ids;

/// What the connection is for (`ClientIntent`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum NextState {
    /// Server-list ping.
    Status = 1,
    /// Normal login.
    Login = 2,
    /// A transfer from another server.
    Transfer = 3,
}

impl NextState {
    /// Converts the wire value, rejecting anything outside 1..=3.
    pub fn from_varint(value: i32) -> Result<Self> {
        match value {
            1 => Ok(Self::Status),
            2 => Ok(Self::Login),
            3 => Ok(Self::Transfer),
            other => Err(Error::protocol(format!(
                "invalid handshake next state {other}"
            ))),
        }
    }

    /// The wire value.
    #[must_use]
    pub fn as_varint(self) -> i32 {
        self as i32
    }
}

/// Serverbound `Intention`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handshake {
    pub protocol: i32,
    pub host: String,
    pub port: u16,
    pub next_state: NextState,
}

impl Handshake {
    /// Encodes the packet id and body.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::handshake::SERVERBOUND_INTENTION);
        writer.write_varint(self.protocol);
        writer.write_string(&self.host, 255)?;
        writer.write_u16(self.port);
        writer.write_varint(self.next_state.as_varint());
        Ok(writer.into_inner())
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let protocol = reader.read_varint()?;
        let host = reader.read_string(255)?;
        let port = reader.read_u16()?;
        let next_state = NextState::from_varint(reader.read_varint()?)?;
        reader.finish()?;
        Ok(Self {
            protocol,
            host,
            port,
            next_state,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handshake() -> Handshake {
        Handshake {
            protocol: crate::PROTOCOL_VERSION,
            host: "localhost".to_owned(),
            port: 25565,
            next_state: NextState::Login,
        }
    }

    #[test]
    fn golden_bytes_match_protocol_767() {
        let encoded = handshake().encode().unwrap();
        assert_eq!(encoded, b"\x00\xFF\x05\x09localhost\x63\xDD\x02".to_vec());
    }

    #[test]
    fn round_trips() {
        for next_state in [NextState::Status, NextState::Login, NextState::Transfer] {
            let mut value = handshake();
            value.next_state = next_state;
            let encoded = value.encode().unwrap();
            let mut reader = Reader::new(&encoded);
            assert_eq!(
                reader.read_varint().unwrap(),
                ids::handshake::SERVERBOUND_INTENTION
            );
            assert_eq!(Handshake::decode(reader.read_rest()).unwrap(), value);
        }
    }

    #[test]
    fn rejects_unknown_next_state() {
        let mut writer = Writer::new();
        writer.write_varint(767);
        writer.write_string("localhost", 255).unwrap();
        writer.write_u16(25565);
        writer.write_varint(9);
        let error = Handshake::decode(writer.as_slice()).unwrap_err();
        assert!(matches!(error, Error::Protocol { .. }));
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut encoded = handshake().encode().unwrap();
        encoded.push(0);
        let mut reader = Reader::new(&encoded);
        let _ = reader.read_varint().unwrap();
        assert!(matches!(
            Handshake::decode(reader.read_rest()),
            Err(Error::TrailingBytes(1))
        ));
    }
}
