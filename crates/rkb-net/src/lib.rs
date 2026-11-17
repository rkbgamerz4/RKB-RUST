//! Minecraft: Java Edition 1.21.1 network sessions.
//!
//! `rkb-proto` owns the byte-level codecs; this crate owns sockets and the
//! login/configuration state machine.
//!
//! - [`connection`]: framed I/O with AES/CFB8 and zlib in the correct order.
//! - [`login`]: handshake, offline and online-mode login, compression, and the
//!   configuration exchange through `FinishConfiguration`.
//! - [`chat`]: readable text from JSON and NBT disconnect reasons.
//!
//! The session stops when configuration finishes. It does not send play
//! packets or claim a world join. Microsoft authentication lives in
//! `rkb-auth`, which implements [`SessionAuthenticator`] here.

#![forbid(unsafe_code)]

pub mod chat;
pub mod connection;
pub mod login;

pub use connection::Connection;
pub use login::{login, Direction, LoginOptions, LoginOutcome, SessionAuthenticator, TraceEntry};
pub use rkb_proto::{DATA_VERSION, GAME_VERSION, PROTOCOL_VERSION};
