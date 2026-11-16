//! Typed packets for each state RKB currently models.
//!
//! `encode` returns the packet id followed by its body. `decode` takes the body
//! **after** the id and rejects trailing bytes, so an oversized or mismatched
//! packet cannot be silently truncated.

pub mod configuration;
pub mod handshake;
pub mod login;
pub mod status;

pub use configuration::{
    AddResourcePack, ClientInformation, CookieRequest, CookieResponse, Disconnect, FeatureFlags,
    FinishConfiguration, KeepAlive, KnownPack, KnownPacks, Ping, PluginMessage, Pong, RegistryData,
    RemoveResourcePack, ResourcePackResponse,
};
pub use handshake::{Handshake, NextState};
pub use login::{
    EncryptionRequest, EncryptionResponse, LoginAcknowledged, LoginCookieRequest,
    LoginCookieResponse, LoginDisconnect, LoginPluginRequest, LoginPluginResponse, LoginStart,
    LoginSuccess, Property, SetCompression,
};
pub use status::{PingRequest, PongResponse, StatusRequest, StatusResponse};
