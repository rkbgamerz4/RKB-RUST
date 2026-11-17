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

/// Play state (protocol 767).
///
/// The names follow the official packet report. Only packets this client
/// decodes or sends are listed; unknown ids are reported, never guessed.
pub mod play {
    /// Clientbound packets.
    pub mod clientbound {
        /// `SpawnEntity`.
        pub const SPAWN_ENTITY: i32 = 0x01;
        /// `BlockUpdate` (block change).
        pub const BLOCK_CHANGE: i32 = 0x09;
        /// `ChunkBatchFinished`.
        pub const CHUNK_BATCH_FINISHED: i32 = 0x0C;
        /// `ChunkBatchStart`.
        pub const CHUNK_BATCH_START: i32 = 0x0D;
        /// `SetContainerContent` (window items).
        pub const WINDOW_ITEMS: i32 = 0x13;
        /// `SetContainerSlot`.
        pub const SET_SLOT: i32 = 0x15;
        /// `Disconnect`.
        pub const KICK_DISCONNECT: i32 = 0x1D;
        /// `UnloadChunk`.
        pub const UNLOAD_CHUNK: i32 = 0x21;
        /// `GameEvent` (game state change).
        pub const GAME_STATE_CHANGE: i32 = 0x22;
        /// `KeepAlive`.
        pub const KEEP_ALIVE: i32 = 0x26;
        /// `ChunkDataAndUpdateLight`.
        pub const CHUNK_DATA: i32 = 0x27;
        /// `UpdateLight`.
        pub const UPDATE_LIGHT: i32 = 0x2A;
        /// `Login` (join game).
        pub const LOGIN: i32 = 0x2B;
        /// `UpdateEntityPosition`.
        pub const REL_ENTITY_MOVE: i32 = 0x2E;
        /// `UpdateEntityPositionAndRotation`.
        pub const ENTITY_MOVE_LOOK: i32 = 0x2F;
        /// `UpdateEntityRotation`.
        pub const ENTITY_LOOK: i32 = 0x30;
        /// `Ping`.
        pub const PING: i32 = 0x35;
        /// `PlayerInfoUpdate`.
        pub const PLAYER_INFO: i32 = 0x3E;
        /// `SynchronizePlayerPosition`.
        pub const SYNC_PLAYER_POSITION: i32 = 0x40;
        /// `RemoveEntities`.
        pub const ENTITY_DESTROY: i32 = 0x42;
        /// `Respawn`.
        pub const RESPAWN: i32 = 0x47;
        /// `SetHeldItem`.
        pub const HELD_ITEM_SLOT: i32 = 0x53;
        /// `SetExperience`.
        pub const EXPERIENCE: i32 = 0x5C;
        /// `SetHealth`.
        pub const UPDATE_HEALTH: i32 = 0x5D;
        /// `SetTime`.
        pub const UPDATE_TIME: i32 = 0x64;
        /// `StartConfiguration`.
        pub const START_CONFIGURATION: i32 = 0x69;
        /// `SystemChatMessage`.
        pub const SYSTEM_CHAT: i32 = 0x6C;
        /// `TeleportEntity`.
        pub const ENTITY_TELEPORT: i32 = 0x70;
    }

    /// Serverbound packets.
    pub mod serverbound {
        /// `AcceptTeleportation`.
        pub const TELEPORT_CONFIRM: i32 = 0x00;
        /// `ChunkBatchReceived`.
        pub const CHUNK_BATCH_RECEIVED: i32 = 0x08;
        /// `ClientStatus`.
        pub const CLIENT_COMMAND: i32 = 0x09;
        /// `ClientInformation`.
        pub const SETTINGS: i32 = 0x0A;
        /// `AcknowledgeConfiguration`.
        pub const CONFIGURATION_ACKNOWLEDGED: i32 = 0x0C;
        /// `KeepAlive`.
        pub const KEEP_ALIVE: i32 = 0x18;
        /// `SetPlayerPosition`.
        pub const POSITION: i32 = 0x1A;
        /// `SetPlayerPositionAndRotation`.
        pub const POSITION_LOOK: i32 = 0x1B;
        /// `SetPlayerRotation`.
        pub const LOOK: i32 = 0x1C;
        /// `SetPlayerMovementFlags`.
        pub const FLYING: i32 = 0x1D;
        /// `PingRequest`.
        pub const PING_REQUEST: i32 = 0x21;
        /// `Pong`.
        pub const PONG: i32 = 0x27;
    }
}
