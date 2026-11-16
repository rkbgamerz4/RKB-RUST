//! Minecraft: Java Edition protocol primitives.
//!
//! This crate implements the parts of the Java Edition network protocol that
//! RKB needs to speak to a server, independently of any rendering, platform, or
//! world code:
//!
//! - [`codec`]: VarInt/VarLong, strings, UUIDs, and the other primitive types.
//! - [`frame`]: the length-prefixed frame and zlib compression above a
//!   threshold.
//! - [`packets`]: typed handshake, status, and login packets for one pinned
//!   protocol version.
//! - [`status`]: a typed view of the server-list status JSON.
//! - [`client`]: a blocking server-list ping.
//!
//! The pinned version is Minecraft: Java Edition 1.21.1, protocol 767. Packet
//! ids and layouts are version data, not assumptions: another version gets its
//! own descriptor rather than silently sharing these tables.
//!
//! No Mojang code, assets, or data are included. The crate has no sockets in
//! its tests except a loopback mock server.

#![forbid(unsafe_code)]

pub mod client;
pub mod codec;
pub mod error;
pub mod frame;
pub mod ids;
pub mod packets;
pub mod status;

pub use client::{ping, PingResult};
pub use codec::{Reader, Writer};
pub use error::{Error, Result};
pub use frame::FrameCodec;
pub use status::{SamplePlayer, ServerStatus};

/// The Minecraft version this protocol table describes.
pub const GAME_VERSION: &str = "1.21.1";

/// `SharedConstants.getProtocolVersion()` for [`GAME_VERSION`].
pub const PROTOCOL_VERSION: i32 = 767;
