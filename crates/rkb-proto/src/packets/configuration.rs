//! Configuration state: the state between login and play.
//!
//! A 1.21.1 server enters configuration after `LoginAcknowledged`. It sends
//! registries, tags, feature flags, and known-pack selection, then
//! `FinishConfiguration`; the client acknowledges with its own
//! `FinishConfiguration`. This module models the control packets a headless
//! login needs. Large registry and tag payloads are kept opaque on purpose:
//! decoding them belongs to the world/registry layer, not to the login state
//! machine.

use crate::codec::{Reader, Writer};
use crate::error::{Error, Result};
use crate::ids;

/// The largest identifier (resource location) string vanilla accepts.
const MAX_IDENTIFIER: usize = 32_767;
/// A cookie's payload is a byte array, not a rest-buffer.
const MAX_COOKIE: usize = 5_120;

/// Serverbound `ClientInformation` (serverbound id `0x00`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientInformation {
    /// Locale such as `en_us`.
    pub locale: String,
    /// Render distance in chunks.
    pub view_distance: i8,
    /// `0` full, `1` commands only, `2` hidden.
    pub chat_visibility: i32,
    pub chat_colors: bool,
    /// Bit flags for cape, jacket, sleeves, pants, and hat.
    pub displayed_skin_parts: u8,
    /// `0` left, `1` right.
    pub main_hand: i32,
    pub text_filtering: bool,
    pub allows_listing: bool,
}

impl Default for ClientInformation {
    fn default() -> Self {
        Self {
            locale: "en_us".to_owned(),
            view_distance: 8,
            chat_visibility: 0,
            chat_colors: true,
            displayed_skin_parts: 0x7F,
            main_hand: 1,
            text_filtering: false,
            allows_listing: false,
        }
    }
}

impl ClientInformation {
    /// Encodes the packet id and body.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::configuration::serverbound::CLIENT_INFORMATION);
        writer.write_string(&self.locale, 16)?;
        if !(0..=32).contains(&self.view_distance) {
            return Err(Error::protocol(format!(
                "view distance {} is outside 0..=32",
                self.view_distance
            )));
        }
        writer.write_u8(self.view_distance as u8);
        if !(0..=2).contains(&self.chat_visibility) {
            return Err(Error::protocol("invalid chat visibility"));
        }
        writer.write_varint(self.chat_visibility);
        writer.write_bool(self.chat_colors);
        writer.write_u8(self.displayed_skin_parts);
        if !(0..=1).contains(&self.main_hand) {
            return Err(Error::protocol("invalid main hand"));
        }
        writer.write_varint(self.main_hand);
        writer.write_bool(self.text_filtering);
        writer.write_bool(self.allows_listing);
        Ok(writer.into_inner())
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let locale = reader.read_string(16)?;
        let view_distance = reader.read_u8()? as i8;
        if !(0..=32).contains(&view_distance) {
            return Err(Error::protocol("view distance is outside 0..=32"));
        }
        let chat_visibility = reader.read_varint()?;
        if !(0..=2).contains(&chat_visibility) {
            return Err(Error::protocol("invalid chat visibility"));
        }
        let chat_colors = reader.read_bool()?;
        let displayed_skin_parts = reader.read_u8()?;
        let main_hand = reader.read_varint()?;
        if !(0..=1).contains(&main_hand) {
            return Err(Error::protocol("invalid main hand"));
        }
        let text_filtering = reader.read_bool()?;
        let allows_listing = reader.read_bool()?;
        reader.finish()?;
        Ok(Self {
            locale,
            view_distance,
            chat_visibility,
            chat_colors,
            displayed_skin_parts,
            main_hand,
            text_filtering,
            allows_listing,
        })
    }
}

/// One entry of the known-packs list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownPack {
    pub namespace: String,
    pub id: String,
    pub version: String,
}

impl KnownPack {
    fn write(&self, writer: &mut Writer) -> Result<()> {
        writer.write_string(&self.namespace, MAX_IDENTIFIER)?;
        writer.write_string(&self.id, MAX_IDENTIFIER)?;
        writer.write_string(&self.version, MAX_IDENTIFIER)
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            namespace: reader.read_string(MAX_IDENTIFIER)?,
            id: reader.read_string(MAX_IDENTIFIER)?,
            version: reader.read_string(MAX_IDENTIFIER)?,
        })
    }
}

/// `SelectKnownPacks` in either direction; the layout is identical and only
/// the packet id differs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KnownPacks {
    pub packs: Vec<KnownPack>,
}

impl KnownPacks {
    fn encode_body(&self) -> Result<Vec<u8>> {
        if self.packs.len() > 1024 {
            return Err(Error::protocol("too many known packs"));
        }
        let mut writer = Writer::new();
        writer.write_varint(self.packs.len() as i32);
        for pack in &self.packs {
            pack.write(&mut writer)?;
        }
        Ok(writer.into_inner())
    }

    /// Encodes the serverbound packet id and body.
    pub fn encode_serverbound(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::configuration::serverbound::SELECT_KNOWN_PACKS);
        writer.write_bytes(&self.encode_body()?);
        Ok(writer.into_inner())
    }

    /// Encodes the clientbound packet id and body, for mock servers.
    pub fn encode_clientbound(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::configuration::clientbound::SELECT_KNOWN_PACKS);
        writer.write_bytes(&self.encode_body()?);
        Ok(writer.into_inner())
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let count = reader.read_varint()?;
        if !(0..=1024).contains(&count) {
            return Err(Error::protocol(format!("invalid known-pack count {count}")));
        }
        let mut packs = Vec::with_capacity(count as usize);
        for _ in 0..count {
            packs.push(KnownPack::read(&mut reader)?);
        }
        reader.finish()?;
        Ok(Self { packs })
    }
}

/// Serverbound `FinishConfiguration` (id `0x03`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FinishConfiguration;

impl FinishConfiguration {
    /// Encodes the serverbound packet id and empty body.
    #[must_use]
    pub fn encode_serverbound(self) -> Vec<u8> {
        vec![ids::configuration::serverbound::FINISH_CONFIGURATION as u8]
    }

    /// Encodes the clientbound packet id and empty body, for mock servers.
    #[must_use]
    pub fn encode_clientbound(self) -> Vec<u8> {
        vec![ids::configuration::clientbound::FINISH_CONFIGURATION as u8]
    }

    /// Decodes a body that must be empty.
    pub fn decode(payload: &[u8]) -> Result<Self> {
        Reader::new(payload).finish()?;
        Ok(Self)
    }
}

/// `KeepAlive` in either direction; only the packet id differs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeepAlive {
    pub id: i64,
}

impl KeepAlive {
    /// Encodes the serverbound packet id and body.
    #[must_use]
    pub fn encode_serverbound(self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::configuration::serverbound::KEEP_ALIVE);
        writer.write_i64(self.id);
        writer.into_inner()
    }

    /// Encodes the clientbound packet id and body, for mock servers.
    #[must_use]
    pub fn encode_clientbound(self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::configuration::clientbound::KEEP_ALIVE);
        writer.write_i64(self.id);
        writer.into_inner()
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let id = reader.read_i64()?;
        reader.finish()?;
        Ok(Self { id })
    }
}

/// Clientbound `Ping` (id `0x05`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ping {
    pub id: i32,
}

impl Ping {
    /// Encodes the packet id and body.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::configuration::clientbound::PING);
        writer.write_i32(self.id);
        writer.into_inner()
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let id = reader.read_i32()?;
        reader.finish()?;
        Ok(Self { id })
    }
}

/// Serverbound `Pong` (id `0x05`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pong {
    pub id: i32,
}

impl Pong {
    /// Encodes the packet id and body.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::configuration::serverbound::PONG);
        writer.write_i32(self.id);
        writer.into_inner()
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let id = reader.read_i32()?;
        reader.finish()?;
        Ok(Self { id })
    }
}

/// Clientbound configuration-state `CookieRequest` (id `0x00`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CookieRequest {
    pub key: String,
}

impl CookieRequest {
    /// Encodes the packet id and body.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::configuration::clientbound::COOKIE_REQUEST);
        writer.write_string(&self.key, MAX_IDENTIFIER)?;
        Ok(writer.into_inner())
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let key = reader.read_string(MAX_IDENTIFIER)?;
        reader.finish()?;
        Ok(Self { key })
    }
}

/// Serverbound configuration-state `CookieResponse` (id `0x01`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CookieResponse {
    pub key: String,
    pub payload: Option<Vec<u8>>,
}

impl CookieResponse {
    /// Encodes the packet id and body.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::configuration::serverbound::COOKIE_RESPONSE);
        writer.write_string(&self.key, MAX_IDENTIFIER)?;
        writer.write_bool(self.payload.is_some());
        if let Some(payload) = &self.payload {
            writer.write_byte_array(payload, MAX_COOKIE)?;
        }
        Ok(writer.into_inner())
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let key = reader.read_string(MAX_IDENTIFIER)?;
        let payload = if reader.read_bool()? {
            Some(reader.read_byte_array(MAX_COOKIE)?.to_vec())
        } else {
            None
        };
        reader.finish()?;
        Ok(Self { key, payload })
    }
}

/// `CustomPayload` (plugin message) in either direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginMessage {
    pub channel: String,
    pub data: Vec<u8>,
}

impl PluginMessage {
    /// Encodes the serverbound packet id and body.
    pub fn encode_serverbound(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::configuration::serverbound::PLUGIN_MESSAGE);
        writer.write_string(&self.channel, MAX_IDENTIFIER)?;
        writer.write_bytes(&self.data);
        Ok(writer.into_inner())
    }

    /// Encodes the clientbound packet id and body, for mock servers.
    pub fn encode_clientbound(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::configuration::clientbound::PLUGIN_MESSAGE);
        writer.write_string(&self.channel, MAX_IDENTIFIER)?;
        writer.write_bytes(&self.data);
        Ok(writer.into_inner())
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let channel = reader.read_string(MAX_IDENTIFIER)?;
        let data = reader.read_rest().to_vec();
        Ok(Self { channel, data })
    }
}

/// Clientbound configuration-state `Disconnect` (id `0x02`).
///
/// Since 1.20.2 the reason is anonymous network NBT, not a JSON string. The
/// login state still uses a JSON string; keeping the raw bytes here avoids
/// pretending the two states share a layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disconnect {
    pub reason_nbt: Vec<u8>,
}

impl Disconnect {
    /// Encodes the packet id and body, for mock servers.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.reason_nbt.is_empty() {
            return Err(Error::protocol("empty disconnect reason"));
        }
        let mut writer = Writer::new();
        writer.write_varint(ids::configuration::clientbound::DISCONNECT);
        writer.write_bytes(&self.reason_nbt);
        Ok(writer.into_inner())
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        if payload.is_empty() {
            return Err(Error::protocol("empty disconnect reason"));
        }
        Ok(Self {
            reason_nbt: payload.to_vec(),
        })
    }
}

/// Clientbound `RegistryData` (id `0x07`).
///
/// The registry entries are network NBT and are intentionally kept opaque.
/// The registry name and declared entry count are decoded so a trace can show
/// what arrived without linking the login path to the world layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryData {
    pub registry: String,
    pub entry_count: i32,
    pub entries_data: Vec<u8>,
}

impl RegistryData {
    /// Encodes the packet id and body, for mock servers.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::configuration::clientbound::REGISTRY_DATA);
        writer.write_string(&self.registry, MAX_IDENTIFIER)?;
        writer.write_varint(self.entry_count);
        writer.write_bytes(&self.entries_data);
        Ok(writer.into_inner())
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let registry = reader.read_string(MAX_IDENTIFIER)?;
        let entry_count = reader.read_varint()?;
        if entry_count < 0 {
            return Err(Error::protocol("negative registry entry count"));
        }
        let entries_data = reader.read_rest().to_vec();
        Ok(Self {
            registry,
            entry_count,
            entries_data,
        })
    }
}

/// Clientbound `AddResourcePack` (id `0x09`).
///
/// Only the fields needed to decide whether the login can continue are
/// decoded; the optional prompt component is ignored. A forced pack cannot be
/// satisfied by this headless M1 client, so the caller rejects it explicitly
/// instead of waiting forever.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddResourcePack {
    pub uuid: u128,
    pub url: String,
    pub hash: String,
    pub forced: bool,
}

impl AddResourcePack {
    /// Encodes the packet id and body, for mock servers. The optional prompt
    /// is never written.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::configuration::clientbound::ADD_RESOURCE_PACK);
        writer.write_uuid(self.uuid);
        writer.write_string(&self.url, MAX_IDENTIFIER)?;
        writer.write_string(&self.hash, 40)?;
        writer.write_bool(self.forced);
        writer.write_bool(false);
        Ok(writer.into_inner())
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let uuid = reader.read_uuid()?;
        let url = reader.read_string(MAX_IDENTIFIER)?;
        let hash = reader.read_string(40)?;
        let forced = reader.read_bool()?;
        // Optional prompt component; ignored, but the trailing-bytes check is
        // intentionally skipped because its layout is a full chat component.
        Ok(Self {
            uuid,
            url,
            hash,
            forced,
        })
    }
}

/// Clientbound `RemoveResourcePack` (id `0x08`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoveResourcePack {
    pub uuid: Option<u128>,
}

impl RemoveResourcePack {
    /// Encodes the packet id and body, for mock servers.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.write_varint(ids::configuration::clientbound::REMOVE_RESOURCE_PACK);
        writer.write_bool(self.uuid.is_some());
        if let Some(uuid) = self.uuid {
            writer.write_uuid(uuid);
        }
        Ok(writer.into_inner())
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let uuid = if reader.read_bool()? {
            Some(reader.read_uuid()?)
        } else {
            None
        };
        reader.finish()?;
        Ok(Self { uuid })
    }
}

/// Clientbound `FeatureFlags` (id `0x0C`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeatureFlags {
    pub flags: Vec<String>,
}

impl FeatureFlags {
    /// Encodes the packet id and body, for mock servers.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.flags.len() > 1024 {
            return Err(Error::protocol("too many feature flags"));
        }
        let mut writer = Writer::new();
        writer.write_varint(ids::configuration::clientbound::FEATURE_FLAGS);
        writer.write_varint(self.flags.len() as i32);
        for flag in &self.flags {
            writer.write_string(flag, MAX_IDENTIFIER)?;
        }
        Ok(writer.into_inner())
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let count = reader.read_varint()?;
        if !(0..=1024).contains(&count) {
            return Err(Error::protocol(format!(
                "invalid feature-flag count {count}"
            )));
        }
        let mut flags = Vec::with_capacity(count as usize);
        for _ in 0..count {
            flags.push(reader.read_string(MAX_IDENTIFIER)?);
        }
        reader.finish()?;
        Ok(Self { flags })
    }
}

/// Serverbound `ResourcePackResponse` (id `0x06`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourcePackResponse {
    pub uuid: u128,
    pub result: i32,
}

impl ResourcePackResponse {
    /// Encodes the packet id and body.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if !(0..=3).contains(&self.result) {
            return Err(Error::protocol("invalid resource-pack result"));
        }
        let mut writer = Writer::new();
        writer.write_varint(ids::configuration::serverbound::RESOURCE_PACK_RESPONSE);
        writer.write_uuid(self.uuid);
        writer.write_varint(self.result);
        Ok(writer.into_inner())
    }

    /// Decodes a body (without the packet id).
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(payload);
        let uuid = reader.read_uuid()?;
        let result = reader.read_varint()?;
        if !(0..=3).contains(&result) {
            return Err(Error::protocol("invalid resource-pack result"));
        }
        reader.finish()?;
        Ok(Self { uuid, result })
    }
}

/// A decoded clientbound configuration packet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clientbound {
    CookieRequest(CookieRequest),
    PluginMessage(PluginMessage),
    Disconnect(Disconnect),
    FinishConfiguration(FinishConfiguration),
    KeepAlive(KeepAlive),
    Ping(Ping),
    ResetChat,
    RegistryData(RegistryData),
    RemoveResourcePack(RemoveResourcePack),
    AddResourcePack(AddResourcePack),
    FeatureFlags(FeatureFlags),
    KnownPacks(KnownPacks),
    /// A packet this client knows is valid but does not decode yet. Its bytes
    /// are preserved so a trace can report the id and size.
    Opaque {
        id: i32,
        payload: Vec<u8>,
    },
}

impl Clientbound {
    /// Decodes a complete clientbound body, including its packet id.
    pub fn decode(body: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(body);
        let id = reader.read_varint()?;
        let payload = reader.read_rest();
        let packet = match id {
            ids::configuration::clientbound::COOKIE_REQUEST => {
                Self::CookieRequest(CookieRequest::decode(payload)?)
            }
            ids::configuration::clientbound::PLUGIN_MESSAGE => {
                Self::PluginMessage(PluginMessage::decode(payload)?)
            }
            ids::configuration::clientbound::DISCONNECT => {
                Self::Disconnect(Disconnect::decode(payload)?)
            }
            ids::configuration::clientbound::FINISH_CONFIGURATION => {
                Self::FinishConfiguration(FinishConfiguration::decode(payload)?)
            }
            ids::configuration::clientbound::KEEP_ALIVE => {
                Self::KeepAlive(KeepAlive::decode(payload)?)
            }
            ids::configuration::clientbound::PING => Self::Ping(Ping::decode(payload)?),
            ids::configuration::clientbound::RESET_CHAT => {
                FinishConfiguration::decode(payload)?;
                Self::ResetChat
            }
            ids::configuration::clientbound::REGISTRY_DATA => {
                Self::RegistryData(RegistryData::decode(payload)?)
            }
            ids::configuration::clientbound::REMOVE_RESOURCE_PACK => {
                Self::RemoveResourcePack(RemoveResourcePack::decode(payload)?)
            }
            ids::configuration::clientbound::ADD_RESOURCE_PACK => {
                Self::AddResourcePack(AddResourcePack::decode(payload)?)
            }
            ids::configuration::clientbound::FEATURE_FLAGS => {
                Self::FeatureFlags(FeatureFlags::decode(payload)?)
            }
            ids::configuration::clientbound::SELECT_KNOWN_PACKS => {
                Self::KnownPacks(KnownPacks::decode(payload)?)
            }
            ids::configuration::clientbound::STORE_COOKIE
            | ids::configuration::clientbound::TRANSFER
            | ids::configuration::clientbound::UPDATE_TAGS
            | ids::configuration::clientbound::CUSTOM_REPORT_DETAILS
            | ids::configuration::clientbound::SERVER_LINKS => Self::Opaque {
                id,
                payload: payload.to_vec(),
            },
            other => {
                return Err(Error::UnknownPacket {
                    state: "configuration",
                    id: other,
                })
            }
        };
        Ok(packet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id_of(packet: &[u8]) -> i32 {
        Reader::new(packet).read_varint().unwrap()
    }

    fn body_of(packet: &[u8]) -> &[u8] {
        let mut reader = Reader::new(packet);
        reader.read_varint().unwrap();
        reader.read_rest()
    }

    #[test]
    fn client_information_round_trips_with_the_expected_layout() {
        let value = ClientInformation::default();
        let encoded = value.encode().unwrap();
        assert_eq!(
            id_of(&encoded),
            ids::configuration::serverbound::CLIENT_INFORMATION
        );
        // locale "en_us", distance 8, chat 0, colors 1, skin 0x7f,
        // main hand 1, filtering 0, listings 0. Protocol 767 has no
        // particle-status field; the real 1.21.1 server rejects an extra byte.
        assert_eq!(body_of(&encoded), b"\x05en_us\x08\x00\x01\x7f\x01\x00\x00");
        assert_eq!(ClientInformation::decode(body_of(&encoded)).unwrap(), value);
    }

    #[test]
    fn client_information_rejects_invalid_enums() {
        for value in [
            ClientInformation {
                chat_visibility: 3,
                ..ClientInformation::default()
            },
            ClientInformation {
                main_hand: 2,
                ..ClientInformation::default()
            },
            ClientInformation {
                view_distance: 33,
                ..ClientInformation::default()
            },
        ] {
            assert!(value.encode().is_err());
        }
    }

    #[test]
    fn known_packs_round_trip_and_empty_list_is_one_zero() {
        let empty = KnownPacks::default();
        let encoded = empty.encode_serverbound().unwrap();
        assert_eq!(
            id_of(&encoded),
            ids::configuration::serverbound::SELECT_KNOWN_PACKS
        );
        assert_eq!(body_of(&encoded), b"\x00");
        assert_eq!(KnownPacks::decode(body_of(&encoded)).unwrap(), empty);

        let value = KnownPacks {
            packs: vec![KnownPack {
                namespace: "minecraft".to_owned(),
                id: "core".to_owned(),
                version: "1.21.1".to_owned(),
            }],
        };
        let encoded = value.encode_clientbound().unwrap();
        assert_eq!(
            id_of(&encoded),
            ids::configuration::clientbound::SELECT_KNOWN_PACKS
        );
        assert_eq!(KnownPacks::decode(body_of(&encoded)).unwrap(), value);
    }

    #[test]
    fn finish_configuration_is_one_byte_in_both_directions() {
        assert_eq!(
            FinishConfiguration.encode_serverbound(),
            vec![ids::configuration::serverbound::FINISH_CONFIGURATION as u8]
        );
        assert_eq!(
            FinishConfiguration.encode_clientbound(),
            vec![ids::configuration::clientbound::FINISH_CONFIGURATION as u8]
        );
        assert_eq!(
            FinishConfiguration::decode(&[]).unwrap(),
            FinishConfiguration
        );
    }

    #[test]
    fn keep_alive_and_ping_round_trip() {
        let keepalive = KeepAlive { id: -9 };
        let encoded = keepalive.encode_clientbound();
        assert_eq!(id_of(&encoded), ids::configuration::clientbound::KEEP_ALIVE);
        assert_eq!(KeepAlive::decode(body_of(&encoded)).unwrap(), keepalive);
        assert_eq!(
            KeepAlive::decode(body_of(&keepalive.encode_serverbound())).unwrap(),
            keepalive
        );

        let ping = Ping { id: 42 };
        assert_eq!(Ping::decode(body_of(&ping.encode())).unwrap(), ping);
        let pong = Pong { id: 42 };
        assert_eq!(Pong::decode(body_of(&pong.encode())).unwrap(), pong);
    }

    #[test]
    fn cookies_round_trip_in_both_directions() {
        let request = CookieRequest {
            key: "rkb:token".to_owned(),
        };
        assert_eq!(
            CookieRequest::decode(body_of(&request.encode().unwrap())).unwrap(),
            request
        );
        for payload in [None, Some(vec![1, 2, 3])] {
            let response = CookieResponse {
                key: "rkb:token".to_owned(),
                payload,
            };
            assert_eq!(
                CookieResponse::decode(body_of(&response.encode().unwrap())).unwrap(),
                response
            );
        }
    }

    #[test]
    fn plugin_messages_round_trip() {
        let value = PluginMessage {
            channel: "minecraft:brand".to_owned(),
            data: b"rkb".to_vec(),
        };
        assert_eq!(
            PluginMessage::decode(body_of(&value.encode_clientbound().unwrap())).unwrap(),
            value
        );
        assert_eq!(
            PluginMessage::decode(body_of(&value.encode_serverbound().unwrap())).unwrap(),
            value
        );
    }

    #[test]
    fn registry_data_keeps_entries_opaque() {
        let value = RegistryData {
            registry: "minecraft:dimension_type".to_owned(),
            entry_count: 1,
            entries_data: vec![0x0A, 0x00, 0x00],
        };
        let encoded = value.encode().unwrap();
        assert_eq!(RegistryData::decode(body_of(&encoded)).unwrap(), value);
    }

    #[test]
    fn resource_packs_decode_enough_to_decide_whether_to_continue() {
        let add = AddResourcePack {
            uuid: 9,
            url: "http://127.0.0.1/pack.zip".to_owned(),
            hash: "0123456789abcdef".to_owned(),
            forced: true,
        };
        let encoded = add.encode().unwrap();
        assert_eq!(
            Clientbound::decode(&encoded).unwrap(),
            Clientbound::AddResourcePack(add)
        );

        let remove = RemoveResourcePack { uuid: Some(9) };
        assert_eq!(
            RemoveResourcePack::decode(body_of(&remove.encode().unwrap())).unwrap(),
            remove
        );
        let remove_all = RemoveResourcePack { uuid: None };
        assert_eq!(
            RemoveResourcePack::decode(body_of(&remove_all.encode().unwrap())).unwrap(),
            remove_all
        );
    }

    #[test]
    fn feature_flags_round_trip() {
        let value = FeatureFlags {
            flags: vec!["minecraft:vanilla".to_owned()],
        };
        assert_eq!(
            FeatureFlags::decode(body_of(&value.encode().unwrap())).unwrap(),
            value
        );
    }

    #[test]
    fn dispatcher_classifies_packets_and_rejects_unknown_ids() {
        let finish = FinishConfiguration.encode_clientbound();
        assert_eq!(
            Clientbound::decode(&finish).unwrap(),
            Clientbound::FinishConfiguration(FinishConfiguration)
        );

        let mut writer = Writer::new();
        writer.write_varint(0x7F);
        let error = Clientbound::decode(writer.as_slice()).unwrap_err();
        assert!(matches!(
            error,
            Error::UnknownPacket {
                state: "configuration",
                id: 0x7F
            }
        ));
    }

    #[test]
    fn disconnect_requires_a_reason() {
        assert!(Disconnect::decode(&[]).is_err());
        let value = Disconnect {
            reason_nbt: vec![0x08, 0x00, 0x04, b'b', b'y', b'e'],
        };
        assert_eq!(
            Disconnect::decode(body_of(&value.encode().unwrap())).unwrap(),
            value
        );
    }

    #[test]
    fn resource_pack_response_round_trips() {
        let value = ResourcePackResponse { uuid: 1, result: 3 };
        assert_eq!(
            ResourcePackResponse::decode(body_of(&value.encode().unwrap())).unwrap(),
            value
        );
    }
}
