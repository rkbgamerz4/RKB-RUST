//! Packet identifiers for Minecraft: Java Edition 1.21.1 (protocol 767).
//!
//! These are version data. A different protocol version must get its own table
//! and its own round-trip tests; reusing these ids for another release is a
//! bug, not a shortcut.

/// The protocol this table describes.
pub const PROTOCOL_VERSION: i32 = 767;

/// The user-facing game version.
pub const GAME_VERSION: &str = "1.21.1";

/// Handshake state.
pub mod handshake {
    /// Serverbound `Intention`.
    pub const SERVERBOUND_INTENTION: i32 = 0x00;
}

/// Status state.
pub mod status {
    /// Serverbound `StatusRequest`.
    pub const SERVERBOUND_REQUEST: i32 = 0x00;
    /// Serverbound `PingRequest`.
    pub const SERVERBOUND_PING: i32 = 0x01;
    /// Clientbound `StatusResponse`.
    pub const CLIENTBOUND_RESPONSE: i32 = 0x00;
    /// Clientbound `PongResponse`.
    pub const CLIENTBOUND_PONG: i32 = 0x01;
}

/// Login state.
pub mod login {
    /// Clientbound packets.
    pub mod clientbound {
        /// `LoginDisconnect`.
        pub const DISCONNECT: i32 = 0x00;
        /// `Hello` (the encryption request).
        pub const ENCRYPTION_REQUEST: i32 = 0x01;
        /// `LoginFinished` (login success).
        pub const SUCCESS: i32 = 0x02;
        /// `LoginCompression`.
        pub const SET_COMPRESSION: i32 = 0x03;
    }

    /// Serverbound packets.
    pub mod serverbound {
        /// `LoginHello` (login start).
        pub const HELLO: i32 = 0x00;
        /// `Key` (the encryption response).
        pub const ENCRYPTION_RESPONSE: i32 = 0x01;
        /// `LoginAcknowledged`.
        pub const LOGIN_ACKNOWLEDGED: i32 = 0x03;
    }
}

/// Configuration state (implemented in a later milestone). The ids are
/// recorded here for planning; no packet code uses them yet and they are not
/// covered by tests until M1 adds the typed packets.
pub mod configuration {
    /// Clientbound packets.
    pub mod clientbound {
        pub const FINISH_CONFIGURATION: i32 = 0x03;
        pub const REGISTRY_DATA: i32 = 0x07;
        pub const FEATURE_FLAGS: i32 = 0x0C;
        pub const UPDATE_TAGS: i32 = 0x0D;
        pub const KNOWN_PACKS: i32 = 0x0E;
    }

    /// Serverbound packets.
    pub mod serverbound {
        pub const FINISH_CONFIGURATION: i32 = 0x03;
        pub const SELECT_KNOWN_PACKS: i32 = 0x07;
    }
}
