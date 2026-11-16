//! Typed packets for each state RKB currently models.
//!
//! `encode` returns the packet id followed by its body. `decode` takes the body
//! **after** the id and rejects trailing bytes, so an oversized or mismatched
//! packet cannot be silently truncated.

pub mod handshake;
pub mod login;
pub mod status;

pub use handshake::{Handshake, NextState};
pub use login::{
    EncryptionRequest, EncryptionResponse, LoginAcknowledged, LoginDisconnect, LoginStart,
    LoginSuccess, Property, SetCompression,
};
pub use status::{PingRequest, PongResponse, StatusRequest, StatusResponse};
