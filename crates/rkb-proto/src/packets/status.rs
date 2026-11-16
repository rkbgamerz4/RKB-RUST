//! Status state: the server-list ping exchange.

use crate::codec::{Reader, Writer};
use crate::error::Result;
use crate::ids;

/// Serverbound `StatusRequest`: no fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatusRequest;

impl StatusRequest {
    /// Encodes the packet id only.
    pub fn encode(&self) -> Vec<u8> {
        vec![ids::status::SERVERBOUND_REQUEST as u8]
    }

    /// Decodes a body that must be empty.
    pub fn decode(payload: &[u8]) -> Result<Self> {
        Reader::new(payload).finish()?;
        Ok(Self)
    }
}

/// Serverbound `PingRequest`: an opaque payload the server echoes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PingRequest {
    pub payload: i64,
}

impl PingRequest {
    /// Encodes the packet id and payload.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::status::SERVERBOUND_PING);
        writer.write_i64(self.payload);
        writer.into_inner()
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let payload = reader.read_i64()?;
        reader.finish()?;
        Ok(Self { payload })
    }
}

/// Clientbound `StatusResponse`: the server-list JSON document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusResponse {
    pub json: String,
}

impl StatusResponse {
    /// Encodes the packet id and JSON string.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::status::CLIENTBOUND_RESPONSE);
        writer.write_string(&self.json, 32_767)?;
        Ok(writer.into_inner())
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let json = reader.read_string(32_767)?;
        reader.finish()?;
        Ok(Self { json })
    }
}

/// Clientbound `PongResponse`: the payload from `PingRequest`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PongResponse {
    pub payload: i64,
}

impl PongResponse {
    /// Encodes the packet id and payload.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::status::CLIENTBOUND_PONG);
        writer.write_i64(self.payload);
        writer.into_inner()
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let payload = reader.read_i64()?;
        reader.finish()?;
        Ok(Self { payload })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    #[test]
    fn status_request_is_one_zero_byte() {
        assert_eq!(StatusRequest.encode(), vec![0x00]);
        assert_eq!(StatusRequest::decode(&[]).unwrap(), StatusRequest);
    }

    #[test]
    fn ping_and_pong_round_trip() {
        let ping = PingRequest {
            payload: 0x0102_0304_0506_0708,
        };
        let encoded = ping.encode();
        assert_eq!(encoded[0], ids::status::SERVERBOUND_PING as u8);
        let mut reader = Reader::new(&encoded);
        assert_eq!(reader.read_varint().unwrap(), ids::status::SERVERBOUND_PING);
        assert_eq!(PingRequest::decode(reader.read_rest()).unwrap(), ping);

        let pong = PongResponse {
            payload: ping.payload,
        };
        let encoded = pong.encode();
        let mut reader = Reader::new(&encoded);
        assert_eq!(reader.read_varint().unwrap(), ids::status::CLIENTBOUND_PONG);
        assert_eq!(PongResponse::decode(reader.read_rest()).unwrap(), pong);
    }

    #[test]
    fn status_response_round_trips() {
        let response = StatusResponse {
            json: r#"{"version":{"name":"1.21.1","protocol":767}}"#.to_owned(),
        };
        let encoded = response.encode().unwrap();
        let mut reader = Reader::new(&encoded);
        assert_eq!(
            reader.read_varint().unwrap(),
            ids::status::CLIENTBOUND_RESPONSE
        );
        assert_eq!(
            StatusResponse::decode(reader.read_rest()).unwrap(),
            response
        );
    }

    #[test]
    fn empty_status_request_rejects_extra_bytes() {
        assert!(matches!(
            StatusRequest::decode(&[1]),
            Err(Error::TrailingBytes(1))
        ));
    }
}
