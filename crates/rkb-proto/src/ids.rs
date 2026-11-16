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
        /// `LoginPluginRequest`.
        pub const PLUGIN_REQUEST: i32 = 0x04;
        /// `CookieRequest`.
        pub const COOKIE_REQUEST: i32 = 0x05;
    }

    /// Serverbound packets.
    pub mod serverbound {
        /// `LoginHello` (login start).
        pub const HELLO: i32 = 0x00;
        /// `Key` (the encryption response).
        pub const ENCRYPTION_RESPONSE: i32 = 0x01;
        /// `LoginPluginResponse`.
        pub const PLUGIN_RESPONSE: i32 = 0x02;
        /// `LoginAcknowledged`.
        pub const LOGIN_ACKNOWLEDGED: i32 = 0x03;
        /// `CookieResponse`.
        pub const COOKIE_RESPONSE: i32 = 0x04;
    }
}

/// Configuration state.
pub mod configuration {
    /// Clientbound packets.
    pub mod clientbound {
        /// `CookieRequest`.
        pub const COOKIE_REQUEST: i32 = 0x00;
        /// `CustomPayload` (plugin message).
        pub const PLUGIN_MESSAGE: i32 = 0x01;
        /// `Disconnect`.
        pub const DISCONNECT: i32 = 0x02;
        /// `FinishConfiguration`.
        pub const FINISH_CONFIGURATION: i32 = 0x03;
        /// `KeepAlive`.
        pub const KEEP_ALIVE: i32 = 0x04;
        /// `Ping`.
        pub const PING: i32 = 0x05;
        /// `ResetChat`.
        pub const RESET_CHAT: i32 = 0x06;
        /// `RegistryData`.
        pub const REGISTRY_DATA: i32 = 0x07;
        /// `RemoveResourcePack`.
        pub const REMOVE_RESOURCE_PACK: i32 = 0x08;
        /// `AddResourcePack`.
        pub const ADD_RESOURCE_PACK: i32 = 0x09;
        /// `StoreCookie`.
        pub const STORE_COOKIE: i32 = 0x0A;
        /// `Transfer`.
        pub const TRANSFER: i32 = 0x0B;
        /// `FeatureFlags`.
        pub const FEATURE_FLAGS: i32 = 0x0C;
        /// `UpdateTags`.
        pub const UPDATE_TAGS: i32 = 0x0D;
        /// `SelectKnownPacks`.
        pub const SELECT_KNOWN_PACKS: i32 = 0x0E;
        /// `CustomReportDetails`.
        pub const CUSTOM_REPORT_DETAILS: i32 = 0x0F;
        /// `ServerLinks`.
        pub const SERVER_LINKS: i32 = 0x10;
    }

    /// Serverbound packets.
    pub mod serverbound {
        /// `ClientInformation`.
        pub const CLIENT_INFORMATION: i32 = 0x00;
        /// `CookieResponse`.
        pub const COOKIE_RESPONSE: i32 = 0x01;
        /// `CustomPayload` (plugin message).
        pub const PLUGIN_MESSAGE: i32 = 0x02;
        /// `FinishConfiguration`.
        pub const FINISH_CONFIGURATION: i32 = 0x03;
        /// `KeepAlive`.
        pub const KEEP_ALIVE: i32 = 0x04;
        /// `Pong`.
        pub const PONG: i32 = 0x05;
        /// `ResourcePackResponse`.
        pub const RESOURCE_PACK_RESPONSE: i32 = 0x06;
        /// `SelectKnownPacks`.
        pub const SELECT_KNOWN_PACKS: i32 = 0x07;
        /// `CustomReportDetails`.
        pub const CUSTOM_REPORT_DETAILS: i32 = 0x08;
        /// `ServerLinks`.
        pub const SERVER_LINKS: i32 = 0x09;
    }
}
