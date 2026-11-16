//! Login state: hello, encryption, compression, and the finished profile.

use crate::codec::{Reader, Writer};
use crate::error::{Error, Result};
use crate::ids;

/// One entry of a game profile's property list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Property {
    pub name: String,
    pub value: String,
    pub signature: Option<String>,
}

impl Property {
    fn write(&self, writer: &mut Writer) -> Result<()> {
        writer.write_string(&self.name, 32_767)?;
        writer.write_string(&self.value, 32_767)?;
        writer.write_optional(self.signature.as_ref(), |writer, signature| {
            writer.write_string(signature, 32_767)
        })
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let name = reader.read_string(32_767)?;
        let value = reader.read_string(32_767)?;
        let signature = reader.read_optional(|reader| reader.read_string(32_767))?;
        Ok(Self {
            name,
            value,
            signature,
        })
    }
}

/// Serverbound `LoginHello`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginStart {
    pub name: String,
    pub uuid: u128,
}

impl LoginStart {
    /// Encodes the packet id and body.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::login::serverbound::HELLO);
        writer.write_string(&self.name, 16)?;
        writer.write_uuid(self.uuid);
        Ok(writer.into_inner())
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let name = reader.read_string(16)?;
        let uuid = reader.read_uuid()?;
        reader.finish()?;
        Ok(Self { name, uuid })
    }
}

/// Serverbound `Key`: the RSA-encrypted shared secret and verify token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptionResponse {
    pub shared_secret: Vec<u8>,
    pub verify_token: Vec<u8>,
}

impl EncryptionResponse {
    /// Encodes the packet id and body.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::login::serverbound::ENCRYPTION_RESPONSE);
        writer.write_byte_array(&self.shared_secret, 256)?;
        writer.write_byte_array(&self.verify_token, 256)?;
        Ok(writer.into_inner())
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let shared_secret = reader.read_byte_array(256)?.to_vec();
        let verify_token = reader.read_byte_array(256)?.to_vec();
        reader.finish()?;
        Ok(Self {
            shared_secret,
            verify_token,
        })
    }
}

/// Serverbound `LoginAcknowledged`: enters the configuration state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LoginAcknowledged;

impl LoginAcknowledged {
    /// Encodes the packet id only.
    pub fn encode(&self) -> Vec<u8> {
        vec![ids::login::serverbound::LOGIN_ACKNOWLEDGED as u8]
    }

    /// Decodes a body that must be empty.
    pub fn decode(payload: &[u8]) -> Result<Self> {
        Reader::new(payload).finish()?;
        Ok(Self)
    }
}

/// Clientbound `Hello`: the server's encryption request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptionRequest {
    pub server_id: String,
    pub public_key: Vec<u8>,
    pub verify_token: Vec<u8>,
    pub should_authenticate: bool,
}

impl EncryptionRequest {
    /// Encodes the packet id and body.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::login::clientbound::ENCRYPTION_REQUEST);
        writer.write_string(&self.server_id, 20)?;
        writer.write_byte_array(&self.public_key, 1024)?;
        writer.write_byte_array(&self.verify_token, 1024)?;
        writer.write_bool(self.should_authenticate);
        Ok(writer.into_inner())
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let server_id = reader.read_string(20)?;
        let public_key = reader.read_byte_array(1024)?.to_vec();
        let verify_token = reader.read_byte_array(1024)?.to_vec();
        let should_authenticate = reader.read_bool()?;
        reader.finish()?;
        Ok(Self {
            server_id,
            public_key,
            verify_token,
            should_authenticate,
        })
    }
}

/// Clientbound `LoginFinished`: the accepted profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginSuccess {
    pub uuid: u128,
    pub name: String,
    pub properties: Vec<Property>,
}

impl LoginSuccess {
    /// Encodes the packet id and body.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.properties.len() > 1024 {
            return Err(Error::protocol("too many profile properties"));
        }
        let mut writer = Writer::new();
        writer.write_varint(ids::login::clientbound::SUCCESS);
        writer.write_uuid(self.uuid);
        writer.write_string(&self.name, 16)?;
        writer.write_varint(self.properties.len() as i32);
        for property in &self.properties {
            property.write(&mut writer)?;
        }
        Ok(writer.into_inner())
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let uuid = reader.read_uuid()?;
        let name = reader.read_string(16)?;
        let count = reader.read_varint()?;
        if !(0..=1024).contains(&count) {
            return Err(Error::protocol(format!(
                "invalid profile property count {count}"
            )));
        }
        let mut properties = Vec::with_capacity(count as usize);
        for _ in 0..count {
            properties.push(Property::read(&mut reader)?);
        }
        reader.finish()?;
        Ok(Self {
            uuid,
            name,
            properties,
        })
    }
}

/// Clientbound `LoginCompression`: the threshold for later frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetCompression {
    pub threshold: i32,
}

impl SetCompression {
    /// The enabled threshold, or `None` for a negative "disabled" value.
    #[must_use]
    pub fn enabled_threshold(self) -> Option<usize> {
        usize::try_from(self.threshold).ok()
    }

    /// Encodes the packet id and body.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::login::clientbound::SET_COMPRESSION);
        writer.write_varint(self.threshold);
        writer.into_inner()
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let threshold = reader.read_varint()?;
        reader.finish()?;
        Ok(Self { threshold })
    }
}

/// Clientbound `LoginDisconnect`: a chat-component JSON reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginDisconnect {
    pub reason_json: String,
}

impl LoginDisconnect {
    /// Encodes the packet id and body.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::login::clientbound::DISCONNECT);
        writer.write_string(&self.reason_json, 262_144)?;
        Ok(writer.into_inner())
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let reason_json = reader.read_string(262_144)?;
        reader.finish()?;
        Ok(Self { reason_json })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(packet: &[u8], id: i32) -> &[u8] {
        let mut reader = Reader::new(packet);
        assert_eq!(reader.read_varint().unwrap(), id);
        reader.read_rest()
    }

    #[test]
    fn login_start_round_trips() {
        let value = LoginStart {
            name: "Steve".to_owned(),
            uuid: 0x0123_4567_89AB_CDEF_FEDC_BA98_7654_3210,
        };
        let encoded = value.encode().unwrap();
        assert_eq!(encoded[0], ids::login::serverbound::HELLO as u8);
        assert_eq!(
            LoginStart::decode(payload(&encoded, ids::login::serverbound::HELLO)).unwrap(),
            value
        );
    }

    #[test]
    fn encryption_exchange_round_trips() {
        let request = EncryptionRequest {
            server_id: "".to_owned(),
            public_key: vec![0x30; 162],
            verify_token: vec![0xAB; 4],
            should_authenticate: true,
        };
        let encoded = request.encode().unwrap();
        assert_eq!(
            EncryptionRequest::decode(payload(
                &encoded,
                ids::login::clientbound::ENCRYPTION_REQUEST
            ))
            .unwrap(),
            request
        );

        let response = EncryptionResponse {
            shared_secret: vec![0x11; 128],
            verify_token: vec![0x22; 128],
        };
        let encoded = response.encode().unwrap();
        assert_eq!(
            EncryptionResponse::decode(payload(
                &encoded,
                ids::login::serverbound::ENCRYPTION_RESPONSE
            ))
            .unwrap(),
            response
        );
    }

    #[test]
    fn login_success_round_trips_with_properties() {
        let value = LoginSuccess {
            uuid: 0xDEAD_BEEF_DEAD_BEEF_DEAD_BEEF_DEAD_BEEF,
            name: "Alex".to_owned(),
            properties: vec![Property {
                name: "textures".to_owned(),
                value: "e30=".to_owned(),
                signature: Some("sig".to_owned()),
            }],
        };
        let encoded = value.encode().unwrap();
        assert_eq!(
            LoginSuccess::decode(payload(&encoded, ids::login::clientbound::SUCCESS)).unwrap(),
            value
        );
    }

    #[test]
    fn compression_threshold_maps_to_codec_state() {
        let enabled = SetCompression { threshold: 256 };
        assert_eq!(enabled.enabled_threshold(), Some(256));
        assert_eq!(
            SetCompression::decode(payload(
                &enabled.encode(),
                ids::login::clientbound::SET_COMPRESSION
            ))
            .unwrap(),
            enabled
        );
        assert_eq!(SetCompression { threshold: -1 }.enabled_threshold(), None);
    }

    #[test]
    fn login_acknowledged_is_one_byte() {
        assert_eq!(
            LoginAcknowledged.encode(),
            vec![ids::login::serverbound::LOGIN_ACKNOWLEDGED as u8]
        );
        assert_eq!(LoginAcknowledged::decode(&[]).unwrap(), LoginAcknowledged);
    }

    #[test]
    fn disconnect_reason_round_trips() {
        let value = LoginDisconnect {
            reason_json: r#"{"text":"bye"}"#.to_owned(),
        };
        let encoded = value.encode().unwrap();
        assert_eq!(
            LoginDisconnect::decode(payload(&encoded, ids::login::clientbound::DISCONNECT))
                .unwrap(),
            value
        );
    }

    #[test]
    fn oversized_name_is_rejected() {
        let value = LoginStart {
            name: "x".repeat(17),
            uuid: 0,
        };
        assert!(value.encode().is_err());
    }
}
