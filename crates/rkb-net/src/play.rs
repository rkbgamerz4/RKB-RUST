//! The play state for protocol 767: world join, chunks, and live updates.
//!
//! A [`PlaySession`] continues the verified login/configuration path from
//! [`crate::login_configured`] and keeps the same socket. It decodes the
//! clientbound packets this client models, answers the packets that must be
//! answered (keep-alive, teleport confirmations, ping/pong, and chunk batch
//! reports), and stores the server's world data.
//!
//! Nothing here invents terrain or entities. A chunk is only stored after the
//! server sent it, and [`PlaySession::block_state`] returns `None` for a
//! position the server has not sent yet.
//!
//! Not implemented in M3, and reported explicitly rather than guessed:
//!
//! - item data components inside inventory slots (the whole packet is reported
//!   as unsupported and the slot contents stay unchanged);
//! - completing a reconfiguration request (`StartConfiguration` is
//!   acknowledged, then the caller must stop);
//! - entity metadata, equipment, and player info rendering.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use rkb_mc::{Nbt, Tag};
use rkb_proto::codec::Reader;
use rkb_proto::error::{Error, Result};
use rkb_proto::ids;
use rkb_proto::packets::play::{
    BlockChange, BlockPos, ChunkBatchFinished, ChunkBatchReceived, ChunkColumn, ChunkData,
    ConfigurationAcknowledged, EntityDestroy, EntityLook, EntityMoveLook, EntityTeleport,
    Experience, GameStateChange, HeldItem, JoinGame, KeepAlive, KickDisconnect, Ping, Pong,
    RelativeEntityMove, Respawn, SetSlot, Slot, SpawnEntity, SyncPlayerPosition, SystemChat,
    TeleportConfirm, UnloadChunk, UpdateHealth, UpdateLight, UpdateTime, WindowItems,
};
use rkb_proto::packets::RegistryData;

use crate::chat;
use crate::connection::Connection;
use crate::light::ChunkLight;
use crate::login::{ConfiguredSession, LoginOutcome};

/// Maximum identifier length accepted from a play packet.
const MAX_IDENTIFIER: usize = 32_767;
/// Sanity ceiling for one anonymous NBT value.
const MAX_NBT: usize = 4 * 1024 * 1024;

/// One dimension type from the server's registry, with the fields chunk
/// decoding needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DimensionType {
    /// Registry id, equal to this entry's position in the registry packet.
    pub id: i32,
    /// Resource name such as `minecraft:overworld`.
    pub name: String,
    /// Lowest world Y.
    pub min_y: i32,
    /// Total height in blocks; always a multiple of 16.
    pub height: i32,
    /// Whether this dimension has skylight.
    pub has_skylight: bool,
}

impl DimensionType {
    /// Number of 16-block sections in a chunk column of this dimension.
    #[must_use]
    pub fn section_count(&self) -> usize {
        (self.height / 16) as usize
    }
}

/// A tracked entity. Positions are world coordinates in blocks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntityState {
    pub id: i32,
    pub kind: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: i8,
    pub pitch: i8,
}

/// The local player's server-provided state.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerState {
    pub entity_id: i32,
    pub dimension_type: i32,
    pub dimension_name: String,
    pub game_mode: i8,
    /// Whether a `SynchronizePlayerPosition` has been received.
    pub has_position: bool,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub health: f32,
    pub food: i32,
    pub food_saturation: f32,
    pub experience: Experience,
    pub held_slot: i8,
    /// Window 0 (the player inventory) as last sent by the server.
    ///
    /// Slots with data components are not decoded in M3; such packets are
    /// reported through [`Event::Unsupported`] and this list is left unchanged.
    pub inventory: Vec<Slot>,
    /// The item carried on the cursor.
    pub carried_item: Slot,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            entity_id: -1,
            dimension_type: -1,
            dimension_name: String::new(),
            game_mode: -1,
            has_position: false,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            health: 20.0,
            food: 20,
            food_saturation: 5.0,
            experience: Experience {
                bar: 0.0,
                level: 0,
                total: 0,
            },
            held_slot: 0,
            inventory: Vec::new(),
            carried_item: Slot::Empty,
        }
    }
}

/// Counters for a play session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlayStats {
    pub packets: u64,
    pub unhandled_packets: u64,
    pub unknown_packets: u64,
    pub unsupported_packets: u64,
    pub keep_alives: u64,
    pub block_updates: u64,
    pub chat_messages: u64,
    pub chunk_batches: u64,
}

/// One thing the play state observed.
///
/// Events summarise data; the authoritative copies live on the session
/// (chunks, block updates, entities, and the player).
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The server let the player into a world.
    Joined(JoinGame),
    /// A chunk column was decoded and stored.
    ChunkLoaded {
        x: i32,
        z: i32,
        sections: usize,
        non_air_blocks: i64,
        block_entities: usize,
    },
    /// A chunk column was unloaded by the server.
    ChunkUnloaded { x: i32, z: i32 },
    /// A single block changed.
    BlockChanged(BlockChange),
    /// A light-only update arrived.
    LightUpdated {
        chunk_x: i32,
        chunk_z: i32,
        sky_arrays: usize,
        block_arrays: usize,
    },
    /// The server asked for a keep-alive; the reply was already sent.
    KeepAlive { id: i64 },
    /// The server moved or confirmed the player position; the teleport was
    /// already confirmed.
    Position(SyncPlayerPosition),
    /// Health and food changed.
    Health(UpdateHealth),
    /// A full inventory was received and stored.
    WindowItems { window_id: u8, slots: usize },
    /// One inventory slot changed.
    SlotChanged(SetSlot),
    /// An entity spawned.
    EntitySpawned(SpawnEntity),
    /// An entity moved; `x`, `y`, and `z` are the tracked world position.
    EntityMoved {
        entity_id: i32,
        x: f64,
        y: f64,
        z: f64,
    },
    /// Entities were removed.
    EntitiesRemoved(Vec<i32>),
    /// A game state change such as a game-mode switch.
    GameState(GameStateChange),
    /// The player respawned; stored chunks and entities were cleared.
    Respawned(Respawn),
    /// World age and time of day.
    Time(UpdateTime),
    /// Experience changed.
    Experience(Experience),
    /// The selected hotbar slot changed.
    HeldItem(i8),
    /// A system chat message, reduced to plain text.
    Chat { text: String, action_bar: bool },
    /// A chunk batch finished; the batch report was already sent.
    ChunkBatch { chunks: usize },
    /// The server pinged; the pong reply was already sent.
    Ping { id: i32 },
    /// The server disconnected the client, with a readable reason.
    Disconnected { reason: String },
    /// The server requested reconfiguration. The acknowledgement was sent,
    /// but M3 does not complete a second configuration exchange.
    ReconfigurationRequested,
    /// A modelled packet carried data M3 does not decode. Nothing was guessed.
    Unsupported {
        packet: &'static str,
        reason: String,
    },
    /// A packet id inside the modelled play range that this client skips.
    Unhandled { id: i32 },
    /// A packet id outside the protocol 767 play range.
    Unknown { id: i32, length: usize },
    /// The socket closed or the transport failed.
    Closed { reason: String },
}

/// A live play-state session over a completed login/configuration.
#[derive(Debug)]
pub struct PlaySession<S> {
    connection: Connection<S>,
    outcome: LoginOutcome,
    dimensions: BTreeMap<i32, DimensionType>,
    player: PlayerState,
    chunks: BTreeMap<(i32, i32), ChunkColumn>,
    block_updates: BTreeMap<BlockPos, u32>,
    entities: BTreeMap<i32, EntityState>,
    /// Decoded sky/block light per chunk column, merged from chunk data and
    /// later `UpdateLight` packets.
    chunk_light: BTreeMap<(i32, i32), ChunkLight>,
    windows: BTreeMap<u8, Vec<Slot>>,
    stats: PlayStats,
    batch_started: Option<Instant>,
    reconfiguration_requested: bool,
}

impl PlaySession<TcpStream> {
    /// Continues a configured session into the play state.
    pub fn from_configured(configured: ConfiguredSession<TcpStream>) -> Result<Self> {
        let (connection, outcome, registries) = configured.into_parts();
        Self::new(connection, outcome, &registries)
    }

    /// Sets the socket read timeout, which bounds [`Self::next_event`].
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> Result<()> {
        self.connection
            .get_ref()
            .set_read_timeout(timeout)
            .map_err(Error::from)
    }

    /// Clones the underlying socket, for example so another thread can shut the
    /// session down after a deadline.
    pub fn try_clone_stream(&self) -> Result<TcpStream> {
        self.connection.get_ref().try_clone().map_err(Error::from)
    }
}

impl<S: Read + Write> PlaySession<S> {
    /// Builds a play session from a connection that has finished
    /// configuration, the login outcome, and the raw registry packets.
    pub fn new(
        connection: Connection<S>,
        outcome: LoginOutcome,
        registries: &[RegistryData],
    ) -> Result<Self> {
        let dimensions = parse_dimension_types(registries)?;
        Ok(Self {
            connection,
            outcome,
            dimensions,
            player: PlayerState::default(),
            chunks: BTreeMap::new(),
            block_updates: BTreeMap::new(),
            entities: BTreeMap::new(),
            chunk_light: BTreeMap::new(),
            windows: BTreeMap::new(),
            stats: PlayStats::default(),
            batch_started: None,
            reconfiguration_requested: false,
        })
    }

    /// What login and configuration observed.
    #[must_use]
    pub fn outcome(&self) -> &LoginOutcome {
        &self.outcome
    }

    /// Dimension types learned from the configuration registry.
    #[must_use]
    pub fn dimensions(&self) -> &BTreeMap<i32, DimensionType> {
        &self.dimensions
    }

    /// The local player's state.
    #[must_use]
    pub fn player(&self) -> &PlayerState {
        &self.player
    }

    /// Stored chunk columns.
    #[must_use]
    pub fn chunks(&self) -> &BTreeMap<(i32, i32), ChunkColumn> {
        &self.chunks
    }

    /// One stored chunk column.
    #[must_use]
    pub fn chunk(&self, x: i32, z: i32) -> Option<&ChunkColumn> {
        self.chunks.get(&(x, z))
    }

    /// Tracked entities.
    #[must_use]
    pub fn entities(&self) -> &BTreeMap<i32, EntityState> {
        &self.entities
    }

    /// Decoded light for one chunk column, if it has arrived.
    ///
    /// The value is merged across the chunk's initial `LightData` and every
    /// later `UpdateLight` packet for that column, so a partial light update
    /// never discards sections the server did not name.
    #[must_use]
    pub fn chunk_light(&self, x: i32, z: i32) -> Option<&ChunkLight> {
        self.chunk_light.get(&(x, z))
    }

    /// Number of chunk columns with decoded light.
    #[must_use]
    pub fn light_columns(&self) -> usize {
        self.chunk_light.len()
    }

    /// Block changes received after the chunk was sent.
    #[must_use]
    pub fn block_updates(&self) -> &BTreeMap<BlockPos, u32> {
        &self.block_updates
    }

    /// Inventory windows other than window 0.
    #[must_use]
    pub fn windows(&self) -> &BTreeMap<u8, Vec<Slot>> {
        &self.windows
    }

    /// Session counters.
    #[must_use]
    pub fn stats(&self) -> PlayStats {
        self.stats
    }

    /// Whether the session asked the server to reconfigure, after which this
    /// client cannot continue.
    #[must_use]
    pub fn reconfiguration_requested(&self) -> bool {
        self.reconfiguration_requested
    }

    /// A block state at world coordinates.
    ///
    /// Later single-block updates win over the chunk column. `None` means the
    /// server has not sent that chunk (or the Y is outside the dimension).
    #[must_use]
    pub fn block_state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        if let Some(state) = self.block_updates.get(&BlockPos::new(x, y, z)) {
            return Some(*state);
        }
        let chunk = self.chunks.get(&(x.div_euclid(16), z.div_euclid(16)))?;
        chunk.block_state(x.rem_euclid(16), y, z.rem_euclid(16))
    }

    /// Reads and handles the next server packet.
    pub fn next_event(&mut self) -> Result<Event> {
        let body = match self.connection.read_packet() {
            Ok(body) => body,
            Err(Error::Io { message }) => return Ok(Event::Closed { reason: message }),
            Err(error) => return Err(error),
        };
        self.stats.packets += 1;
        let mut reader = Reader::new(&body);
        let id = reader.read_varint()?;
        let payload = reader.read_rest();
        self.dispatch(id, payload)
            .map_err(|error| Error::protocol(format!("play packet 0x{id:02X}: {error}")))
    }

    fn dispatch(&mut self, id: i32, payload: &[u8]) -> Result<Event> {
        use ids::play::clientbound;

        match id {
            clientbound::LOGIN => {
                let packet = JoinGame::decode(payload)?;
                self.apply_join(&packet);
                Ok(Event::Joined(packet))
            }
            clientbound::KEEP_ALIVE => {
                let packet = KeepAlive::decode(payload)?;
                self.send(&packet.encode_serverbound())?;
                self.stats.keep_alives += 1;
                Ok(Event::KeepAlive { id: packet.id })
            }
            clientbound::SYNC_PLAYER_POSITION => {
                let packet = SyncPlayerPosition::decode(payload)?;
                self.apply_position(&packet);
                self.send(
                    &TeleportConfirm {
                        teleport_id: packet.teleport_id,
                    }
                    .encode(),
                )?;
                Ok(Event::Position(packet))
            }
            clientbound::CHUNK_DATA => {
                let packet = ChunkData::decode(payload)?;
                self.store_chunk(packet)
            }
            clientbound::BLOCK_CHANGE => {
                let packet = BlockChange::decode(payload)?;
                self.block_updates
                    .insert(packet.position, packet.state_id as u32);
                self.stats.block_updates += 1;
                Ok(Event::BlockChanged(packet))
            }
            clientbound::UNLOAD_CHUNK => {
                let packet = UnloadChunk::decode(payload)?;
                self.chunks.remove(&(packet.x, packet.z));
                self.chunk_light.remove(&(packet.x, packet.z));
                Ok(Event::ChunkUnloaded {
                    x: packet.x,
                    z: packet.z,
                })
            }
            clientbound::UPDATE_LIGHT => {
                let packet = UpdateLight::decode(payload)?;
                let min_section_y = self
                    .dimensions
                    .get(&self.player.dimension_type)
                    .map_or(0, |dimension| dimension.min_y >> 4);
                self.chunk_light
                    .entry((packet.chunk_x, packet.chunk_z))
                    .or_default()
                    .apply(&packet.light, min_section_y);
                Ok(Event::LightUpdated {
                    chunk_x: packet.chunk_x,
                    chunk_z: packet.chunk_z,
                    sky_arrays: packet.light.sky_light.len(),
                    block_arrays: packet.light.block_light.len(),
                })
            }
            clientbound::UPDATE_HEALTH => {
                let packet = UpdateHealth::decode(payload)?;
                self.player.health = packet.health;
                self.player.food = packet.food;
                self.player.food_saturation = packet.food_saturation;
                Ok(Event::Health(packet))
            }
            clientbound::WINDOW_ITEMS => match WindowItems::decode(payload) {
                Ok(packet) => {
                    if packet.window_id == 0 {
                        self.player.inventory = packet.items.clone();
                        self.player.carried_item = packet.carried_item;
                    } else {
                        self.windows.insert(packet.window_id, packet.items.clone());
                    }
                    Ok(Event::WindowItems {
                        window_id: packet.window_id,
                        slots: packet.items.len(),
                    })
                }
                Err(error) => {
                    self.stats.unsupported_packets += 1;
                    Ok(Event::Unsupported {
                        packet: "SetContainerContent",
                        reason: error.to_string(),
                    })
                }
            },
            clientbound::SET_SLOT => match SetSlot::decode(payload) {
                Ok(packet) => {
                    if packet.window_id == 0 {
                        let slot = packet.slot;
                        if slot >= 0 {
                            let slot = slot as usize;
                            if slot < self.player.inventory.len() {
                                self.player.inventory[slot] = packet.item;
                            }
                        }
                    } else if let Some(window) = self.windows.get_mut(&packet.window_id) {
                        if packet.slot >= 0 {
                            let slot = packet.slot as usize;
                            if slot < window.len() {
                                window[slot] = packet.item;
                            }
                        }
                    }
                    Ok(Event::SlotChanged(packet))
                }
                Err(error) => {
                    self.stats.unsupported_packets += 1;
                    Ok(Event::Unsupported {
                        packet: "SetContainerSlot",
                        reason: error.to_string(),
                    })
                }
            },
            clientbound::HELD_ITEM_SLOT => {
                let packet = HeldItem::decode(payload)?;
                self.player.held_slot = packet.slot;
                Ok(Event::HeldItem(packet.slot))
            }
            clientbound::SPAWN_ENTITY => {
                let packet = SpawnEntity::decode(payload)?;
                let entity = EntityState {
                    id: packet.entity_id,
                    kind: packet.kind,
                    x: packet.x,
                    y: packet.y,
                    z: packet.z,
                    yaw: packet.yaw,
                    pitch: packet.pitch,
                };
                self.entities.insert(packet.entity_id, entity);
                Ok(Event::EntitySpawned(packet))
            }
            clientbound::REL_ENTITY_MOVE => {
                let packet = RelativeEntityMove::decode(payload)?;
                self.move_entity(
                    packet.entity_id,
                    packet.dx,
                    packet.dy,
                    packet.dz,
                    None,
                    None,
                )?;
                self.entity_moved(packet.entity_id)
            }
            clientbound::ENTITY_MOVE_LOOK => {
                let packet = EntityMoveLook::decode(payload)?;
                self.move_entity(
                    packet.entity_id,
                    packet.dx,
                    packet.dy,
                    packet.dz,
                    Some(packet.yaw),
                    Some(packet.pitch),
                )?;
                self.entity_moved(packet.entity_id)
            }
            clientbound::ENTITY_LOOK => {
                let packet = EntityLook::decode(payload)?;
                if let Some(entity) = self.entities.get_mut(&packet.entity_id) {
                    entity.yaw = packet.yaw;
                    entity.pitch = packet.pitch;
                }
                self.entity_moved(packet.entity_id)
            }
            clientbound::ENTITY_TELEPORT => {
                let packet = EntityTeleport::decode(payload)?;
                if let Some(entity) = self.entities.get_mut(&packet.entity_id) {
                    entity.x = packet.x;
                    entity.y = packet.y;
                    entity.z = packet.z;
                    entity.yaw = packet.yaw;
                    entity.pitch = packet.pitch;
                }
                self.entity_moved(packet.entity_id)
            }
            clientbound::ENTITY_DESTROY => {
                let packet = EntityDestroy::decode(payload)?;
                for id in &packet.entity_ids {
                    self.entities.remove(id);
                }
                Ok(Event::EntitiesRemoved(packet.entity_ids))
            }
            clientbound::GAME_STATE_CHANGE => {
                let packet = GameStateChange::decode(payload)?;
                if packet.reason == GameStateChange::REASON_CHANGE_GAME_MODE {
                    self.player.game_mode = packet.value as i8;
                }
                Ok(Event::GameState(packet))
            }
            clientbound::RESPAWN => {
                let packet = Respawn::decode(payload)?;
                self.player.dimension_type = packet.spawn.dimension_type;
                self.player.dimension_name = packet.spawn.dimension_name.clone();
                self.player.game_mode = packet.spawn.game_mode;
                self.player.has_position = false;
                self.chunks.clear();
                self.block_updates.clear();
                self.chunk_light.clear();
                self.entities.clear();
                Ok(Event::Respawned(packet))
            }
            clientbound::UPDATE_TIME => Ok(Event::Time(UpdateTime::decode(payload)?)),
            clientbound::EXPERIENCE => {
                let packet = Experience::decode(payload)?;
                self.player.experience = packet;
                Ok(Event::Experience(packet))
            }
            clientbound::SYSTEM_CHAT => {
                let packet = SystemChat::decode(payload)?;
                self.stats.chat_messages += 1;
                let text = chat::nbt_component_plain_text(&packet.content)
                    .unwrap_or_else(|| "<unreadable chat component>".to_owned());
                Ok(Event::Chat {
                    text,
                    action_bar: packet.action_bar,
                })
            }
            clientbound::CHUNK_BATCH_START => {
                self.batch_started = Some(Instant::now());
                Ok(Event::ChunkBatch { chunks: 0 })
            }
            clientbound::CHUNK_BATCH_FINISHED => {
                let packet = ChunkBatchFinished::decode(payload)?;
                let elapsed = self
                    .batch_started
                    .take()
                    .map_or(5.0, |start| start.elapsed().as_secs_f32());
                let per_tick =
                    (packet.batch_size.max(1) as f32 / elapsed.max(0.005)).clamp(1.0, 64.0);
                self.send(
                    &ChunkBatchReceived {
                        chunks_per_tick: per_tick,
                    }
                    .encode(),
                )?;
                self.stats.chunk_batches += 1;
                Ok(Event::ChunkBatch {
                    chunks: packet.batch_size as usize,
                })
            }
            clientbound::PING => {
                let packet = Ping::decode(payload)?;
                self.send(&Pong { id: packet.id }.encode())?;
                Ok(Event::Ping { id: packet.id })
            }
            clientbound::KICK_DISCONNECT => {
                let packet = KickDisconnect::decode(payload)?;
                let reason = chat::nbt_component_plain_text(&packet.reason_nbt)
                    .unwrap_or_else(|| "<unreadable disconnect reason>".to_owned());
                Ok(Event::Disconnected { reason })
            }
            clientbound::START_CONFIGURATION => {
                self.send(&ConfigurationAcknowledged.encode())?;
                self.reconfiguration_requested = true;
                Ok(Event::ReconfigurationRequested)
            }
            0..=0x7B => {
                self.stats.unhandled_packets += 1;
                Ok(Event::Unhandled { id })
            }
            other => {
                self.stats.unknown_packets += 1;
                Ok(Event::Unknown {
                    id: other,
                    length: payload.len(),
                })
            }
        }
    }

    fn send(&mut self, body: &[u8]) -> Result<()> {
        self.connection.write_packet(body)?;
        self.connection.flush()
    }

    fn apply_join(&mut self, packet: &JoinGame) {
        self.player.entity_id = packet.entity_id;
        self.player.dimension_type = packet.spawn.dimension_type;
        self.player.dimension_name = packet.spawn.dimension_name.clone();
        self.player.game_mode = packet.spawn.game_mode;
        self.player.has_position = false;
        self.chunks.clear();
        self.block_updates.clear();
        self.chunk_light.clear();
        self.entities.clear();
    }

    fn apply_position(&mut self, packet: &SyncPlayerPosition) {
        let player = &mut self.player;
        let relative = packet.flags;
        player.x = if relative & SyncPlayerPosition::RELATIVE_X != 0 {
            player.x + packet.x
        } else {
            packet.x
        };
        player.y = if relative & SyncPlayerPosition::RELATIVE_Y != 0 {
            player.y + packet.y
        } else {
            packet.y
        };
        player.z = if relative & SyncPlayerPosition::RELATIVE_Z != 0 {
            player.z + packet.z
        } else {
            packet.z
        };
        player.yaw = if relative & SyncPlayerPosition::RELATIVE_YAW != 0 {
            player.yaw + packet.yaw
        } else {
            packet.yaw
        };
        player.pitch = if relative & SyncPlayerPosition::RELATIVE_PITCH != 0 {
            player.pitch + packet.pitch
        } else {
            packet.pitch
        };
        player.has_position = true;
    }

    fn store_chunk(&mut self, packet: ChunkData) -> Result<Event> {
        let dimension = self.dimension(self.player.dimension_type)?;
        let sections = dimension.section_count();
        let min_section_y = dimension.min_y >> 4;
        let column =
            ChunkColumn::decode(packet.x, packet.z, dimension.min_y, &packet.data, sections)?;
        let event = Event::ChunkLoaded {
            x: packet.x,
            z: packet.z,
            sections: column.section_count(),
            non_air_blocks: column.non_air_blocks(),
            block_entities: packet.block_entities.len(),
        };
        self.chunks.insert((packet.x, packet.z), column);
        // A chunk packet carries the column's initial light. A later
        // `UpdateLight` merges into this snapshot instead of replacing it.
        self.chunk_light.insert(
            (packet.x, packet.z),
            ChunkLight::decode(&packet.light, min_section_y),
        );
        Ok(event)
    }

    fn dimension(&self, id: i32) -> Result<&DimensionType> {
        self.dimensions.get(&id).ok_or_else(|| {
            Error::protocol(format!(
                "server used dimension type {id}, which is not in the registry"
            ))
        })
    }

    fn move_entity(
        &mut self,
        entity_id: i32,
        dx: i16,
        dy: i16,
        dz: i16,
        yaw: Option<i8>,
        pitch: Option<i8>,
    ) -> Result<()> {
        let entity = self.entities.get_mut(&entity_id).ok_or_else(|| {
            Error::protocol(format!(
                "relative move for entity {entity_id}, which was never spawned"
            ))
        })?;
        entity.x += f64::from(dx) / 4096.0;
        entity.y += f64::from(dy) / 4096.0;
        entity.z += f64::from(dz) / 4096.0;
        if let Some(yaw) = yaw {
            entity.yaw = yaw;
        }
        if let Some(pitch) = pitch {
            entity.pitch = pitch;
        }
        Ok(())
    }

    fn entity_moved(&self, entity_id: i32) -> Result<Event> {
        let entity = self
            .entities
            .get(&entity_id)
            .ok_or_else(|| Error::protocol(format!("missing entity {entity_id}")))?;
        Ok(Event::EntityMoved {
            entity_id,
            x: entity.x,
            y: entity.y,
            z: entity.z,
        })
    }
}

/// Parses `minecraft:dimension_type` entries from configuration registry
/// packets.
///
/// Registry ids are the entry positions, matching how `Login` and `Respawn`
/// refer to a dimension type.
pub fn parse_dimension_types(registries: &[RegistryData]) -> Result<BTreeMap<i32, DimensionType>> {
    let Some(data) = registries
        .iter()
        .find(|registry| registry.registry == "minecraft:dimension_type")
    else {
        return Ok(BTreeMap::new());
    };
    let mut reader = Reader::new(&data.entries_data);
    let mut dimensions = BTreeMap::new();
    for id in 0..data.entry_count {
        let name = reader.read_string(MAX_IDENTIFIER)?;
        let Some(raw) = reader.read_prefixed_optional_nbt_raw(MAX_NBT)? else {
            continue;
        };
        let tag = Nbt::read_anonymous(raw).map_err(|error| {
            Error::protocol(format!("dimension type {name} has invalid NBT: {error}"))
        })?;
        let min_y = tag.get("min_y").and_then(Tag::as_int).ok_or_else(|| {
            let keys: Vec<&str> = tag
                .as_compound()
                .map(|map| map.keys().map(String::as_str).collect())
                .unwrap_or_default();
            Error::protocol(format!(
                "dimension type {name} has no min_y (keys: {keys:?})"
            ))
        })?;
        let height = tag
            .get("height")
            .and_then(Tag::as_int)
            .ok_or_else(|| Error::protocol(format!("dimension type {name} has no height")))?;
        if height <= 0 || height % 16 != 0 {
            return Err(Error::protocol(format!(
                "dimension type {name} has height {height}, not a positive multiple of 16"
            )));
        }
        let has_skylight = tag
            .get("has_skylight")
            .and_then(Tag::as_byte)
            .is_some_and(|value| value != 0);
        dimensions.insert(
            id,
            DimensionType {
                id,
                name,
                min_y,
                height,
                has_skylight,
            },
        );
    }
    reader.finish()?;
    Ok(dimensions)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap as Map;
    use std::io::{self, Cursor};
    use std::sync::{Arc, Mutex};

    use super::*;
    use rkb_proto::codec::Writer;
    use rkb_proto::packets::play::{BitSet, GameStateChange, LightData, PalettedContainer};

    /// One shared in-memory pipe: bytes written by the test are read by the
    /// session, and the session's replies land in `server`.
    #[derive(Clone, Debug, Default)]
    struct Pipe {
        to_client: Arc<Mutex<Cursor<Vec<u8>>>>,
        from_client: Arc<Mutex<Vec<u8>>>,
    }

    impl Pipe {
        fn with_server_packets(packets: &[Vec<u8>]) -> Self {
            let codec = rkb_proto::frame::FrameCodec::uncompressed();
            let mut bytes = Vec::new();
            for packet in packets {
                bytes.extend_from_slice(&codec.encode(packet).unwrap());
            }
            Self {
                to_client: Arc::new(Mutex::new(Cursor::new(bytes))),
                from_client: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn server_bytes(&self) -> Vec<u8> {
            self.from_client.lock().unwrap().clone()
        }

        /// Reads every framed reply the session wrote.
        fn replies(&self) -> Vec<Vec<u8>> {
            let mut connection = Connection::new(Cursor::new(self.server_bytes()));
            let mut packets = Vec::new();
            while let Ok(body) = connection.read_packet() {
                packets.push(body);
            }
            packets
        }
    }

    struct ClientSide(Pipe);

    impl Read for ClientSide {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.0.to_client.lock().unwrap().read(buffer)
        }
    }

    impl Write for ClientSide {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.from_client.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn anonymous_compound(fields: &[(&str, Tag)]) -> Vec<u8> {
        let mut root = Map::new();
        for (key, value) in fields {
            root.insert((*key).to_owned(), value.clone());
        }
        let named = Nbt::write(&Tag::Compound(root)).unwrap();
        // Strip the empty root name: tag id, two name-length bytes, payload.
        let mut anonymous = Vec::with_capacity(named.len() - 2);
        anonymous.push(named[0]);
        anonymous.extend_from_slice(&named[3..]);
        anonymous
    }

    fn dimension_registry() -> RegistryData {
        let mut entries = Vec::new();
        let mut writer = Writer::new();
        writer
            .write_string("minecraft:overworld", MAX_IDENTIFIER)
            .unwrap();
        let nbt = anonymous_compound(&[
            ("min_y", Tag::Int(-64)),
            ("height", Tag::Int(384)),
            ("has_skylight", Tag::Byte(1)),
        ]);
        writer.write_bool(true);
        writer.write_bytes(&nbt);
        entries.extend_from_slice(writer.as_slice());

        RegistryData {
            registry: "minecraft:dimension_type".to_owned(),
            entry_count: 1,
            entries_data: entries,
        }
    }

    fn outcome() -> LoginOutcome {
        LoginOutcome {
            address: rkb_proto::client::ServerAddress::parse("127.0.0.1:25565").unwrap(),
            protocol: rkb_proto::PROTOCOL_VERSION,
            data_version: rkb_proto::DATA_VERSION,
            uuid: 0,
            name: "Tester".to_owned(),
            properties: Vec::new(),
            strict_error_handling: true,
            encrypted: false,
            session_authenticated: false,
            compression: None,
            registries: vec!["minecraft:dimension_type".to_owned()],
            feature_flags: Vec::new(),
            known_packs: Vec::new(),
            trace: Vec::new(),
        }
    }

    fn session(packets: &[Vec<u8>]) -> (PlaySession<ClientSide>, Pipe) {
        let pipe = Pipe::with_server_packets(packets);
        let connection = Connection::new(ClientSide(pipe.clone()));
        let session = PlaySession::new(connection, outcome(), &[dimension_registry()]).unwrap();
        (session, pipe)
    }

    fn join_game() -> JoinGame {
        JoinGame {
            entity_id: 1,
            hardcore: false,
            world_names: vec!["minecraft:overworld".to_owned()],
            max_players: 20,
            view_distance: 10,
            simulation_distance: 8,
            reduced_debug_info: false,
            enable_respawn_screen: true,
            do_limited_crafting: false,
            spawn: rkb_proto::packets::play::SpawnInfo {
                dimension_type: 0,
                dimension_name: "minecraft:overworld".to_owned(),
                hashed_seed: 0,
                game_mode: 0,
                previous_game_mode: -1,
                is_debug: false,
                is_flat: true,
                death: None,
                portal_cooldown: 0,
            },
            enforces_secure_chat: true,
        }
    }

    fn chunk_packet() -> ChunkData {
        let mut data = Writer::new();
        // A full overworld column: 24 sections. Section 0 (Y -64..-49) is
        // solid state 9; every other section is air.
        for section in 0..24 {
            if section == 0 {
                data.write_i16(4096);
                PalettedContainer::Single(9)
                    .encode(&mut data, 4096)
                    .unwrap();
                PalettedContainer::Single(1).encode(&mut data, 64).unwrap();
            } else {
                data.write_i16(0);
                PalettedContainer::Single(0)
                    .encode(&mut data, 4096)
                    .unwrap();
                PalettedContainer::Single(1).encode(&mut data, 64).unwrap();
            }
        }
        ChunkData {
            x: 0,
            z: 0,
            heightmaps: anonymous_compound(&[]),
            data: data.into_inner(),
            block_entities: Vec::new(),
            light: LightData::default(),
        }
    }

    #[test]
    fn join_sets_the_player_and_dimension() {
        let (mut session, _pipe) = session(&[join_game().encode().unwrap()]);
        assert_eq!(session.next_event().unwrap(), Event::Joined(join_game()));
        assert_eq!(session.player().entity_id, 1);
        assert_eq!(session.player().dimension_name, "minecraft:overworld");
        assert_eq!(session.player().game_mode, 0);
        assert!(!session.player().has_position);
        assert_eq!(session.dimensions().len(), 1);
        assert_eq!(session.dimensions()[&0].section_count(), 24);
    }

    #[test]
    fn chunk_is_stored_only_after_the_server_sends_it() {
        let (mut session, _pipe) = session(&[
            join_game().encode().unwrap(),
            chunk_packet().encode().unwrap(),
        ]);
        assert_eq!(session.next_event().unwrap(), Event::Joined(join_game()));
        assert_eq!(session.block_state(0, 0, 0), None);

        let event = session.next_event().unwrap();
        assert!(matches!(
            event,
            Event::ChunkLoaded {
                x: 0,
                z: 0,
                sections: 24,
                non_air_blocks: 4096,
                block_entities: 0
            }
        ));
        // Section 0 covers Y -64..-49 and is filled with state 9.
        assert_eq!(session.block_state(0, -64, 0), Some(9));
        assert_eq!(session.block_state(15, -49, 15), Some(9));
        // Section 1 is single-valued air (0); the server sent it, so 0 is a
        // real answer rather than an absent chunk.
        assert_eq!(session.block_state(0, -48, 0), Some(0));
        // Below the dimension and outside sent chunks stay unknown.
        assert_eq!(session.block_state(0, -80, 0), None);
        assert_eq!(session.block_state(16, 0, 0), None);
    }

    #[test]
    fn chunk_light_is_decoded_and_merged() {
        let mut packet = chunk_packet();
        // Bit 1 is the dimension's minimum section (-4 -> Y -64) because bit 0
        // is the section below the world.
        packet.light = LightData {
            sky_light_mask: BitSet::from_words(vec![0b10]),
            block_light_mask: BitSet::from_words(vec![0b10]),
            empty_sky_light_mask: BitSet::default(),
            empty_block_light_mask: BitSet::default(),
            sky_light: vec![vec![0x77; 2048]],
            block_light: vec![vec![0x33; 2048]],
        };
        let (mut first, _pipe) =
            session(&[join_game().encode().unwrap(), packet.encode().unwrap()]);
        first.next_event().unwrap();
        first.next_event().unwrap();
        let light = first.chunk_light(0, 0).expect("chunk light");
        assert!(light.has_data());
        assert_eq!(light.sample(0, -64, 0), (7, 3));
        assert_eq!(first.light_columns(), 1);
        let update = UpdateLight {
            chunk_x: 0,
            chunk_z: 0,
            light: LightData {
                sky_light_mask: BitSet::from_words(vec![0b10]),
                block_light_mask: BitSet::default(),
                empty_sky_light_mask: BitSet::default(),
                empty_block_light_mask: BitSet::default(),
                sky_light: vec![vec![0x55; 2048]],
                block_light: Vec::new(),
            },
        };
        let (mut merged, _pipe) = session(&[
            join_game().encode().unwrap(),
            packet.encode().unwrap(),
            update.encode().unwrap(),
        ]);
        merged.next_event().unwrap();
        merged.next_event().unwrap();
        merged.next_event().unwrap();
        let light = merged.chunk_light(0, 0).expect("merged light");
        assert_eq!(light.sample(0, -64, 0), (5, 3));
    }

    #[test]
    fn block_updates_override_chunk_data() {
        let change = BlockChange {
            position: BlockPos::new(1, -63, 2),
            state_id: 5,
        };
        let (mut session, _pipe) = session(&[
            join_game().encode().unwrap(),
            chunk_packet().encode().unwrap(),
            change.encode().unwrap(),
        ]);
        session.next_event().unwrap();
        session.next_event().unwrap();
        assert_eq!(session.block_state(1, -63, 2), Some(9));
        assert_eq!(session.next_event().unwrap(), Event::BlockChanged(change));
        assert_eq!(session.block_state(1, -63, 2), Some(5));
        assert_eq!(session.block_updates().len(), 1);
    }

    #[test]
    fn keep_alive_ping_and_teleport_are_answered() {
        let position = SyncPlayerPosition {
            x: 8.5,
            y: 64.0,
            z: -2.5,
            yaw: 90.0,
            pitch: 10.0,
            flags: 0,
            teleport_id: 42,
        };
        let (mut session, pipe) = session(&[
            join_game().encode().unwrap(),
            keep_alive(77),
            position.encode().unwrap(),
            Ping { id: 9 }.encode(),
        ]);
        session.next_event().unwrap();
        assert_eq!(session.next_event().unwrap(), Event::KeepAlive { id: 77 });

        let event = session.next_event().unwrap();
        assert!(matches!(event, Event::Position(_)));
        assert!(session.player().has_position);
        assert_eq!(session.player().x, 8.5);
        assert_eq!(session.player().z, -2.5);
        assert_eq!(session.player().yaw, 90.0);

        assert_eq!(session.next_event().unwrap(), Event::Ping { id: 9 });

        let sent = pipe.replies();
        assert_eq!(sent.len(), 3);
        let mut reader = Reader::new(&sent[0]);
        assert_eq!(
            reader.read_varint().unwrap(),
            ids::play::serverbound::KEEP_ALIVE
        );
        assert_eq!(reader.read_i64().unwrap(), 77);
        let mut reader = Reader::new(&sent[1]);
        assert_eq!(
            reader.read_varint().unwrap(),
            ids::play::serverbound::TELEPORT_CONFIRM
        );
        assert_eq!(reader.read_varint().unwrap(), 42);
        let mut reader = Reader::new(&sent[2]);
        assert_eq!(reader.read_varint().unwrap(), ids::play::serverbound::PONG);
        assert_eq!(reader.read_i32().unwrap(), 9);
        assert!(reader.is_empty());
    }

    fn keep_alive(id: i64) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::KEEP_ALIVE);
        writer.write_i64(id);
        writer.into_inner()
    }

    #[test]
    fn relative_position_flags_apply_to_the_previous_position() {
        let first = SyncPlayerPosition {
            x: 1.0,
            y: 2.0,
            z: 3.0,
            yaw: 0.0,
            pitch: 0.0,
            flags: 0,
            teleport_id: 1,
        };
        let second = SyncPlayerPosition {
            x: 0.5,
            y: 1.0,
            z: -1.5,
            yaw: 45.0,
            pitch: 0.0,
            flags: SyncPlayerPosition::RELATIVE_X
                | SyncPlayerPosition::RELATIVE_Y
                | SyncPlayerPosition::RELATIVE_Z
                | SyncPlayerPosition::RELATIVE_YAW,
            teleport_id: 2,
        };
        let (mut session, _pipe) = session(&[
            join_game().encode().unwrap(),
            first.encode().unwrap(),
            second.encode().unwrap(),
        ]);
        session.next_event().unwrap();
        session.next_event().unwrap();
        session.next_event().unwrap();
        assert_eq!(session.player().x, 1.5);
        assert_eq!(session.player().y, 3.0);
        assert_eq!(session.player().z, 1.5);
        assert_eq!(session.player().yaw, 45.0);
        assert_eq!(session.player().pitch, 0.0);
    }

    #[test]
    fn chunk_batch_finished_is_reported_back() {
        let (mut session, pipe) = session(&[
            join_game().encode().unwrap(),
            chunk_batch_start(),
            ChunkBatchFinished { batch_size: 4 }.encode().unwrap(),
        ]);
        session.next_event().unwrap();
        assert_eq!(
            session.next_event().unwrap(),
            Event::ChunkBatch { chunks: 0 }
        );
        assert_eq!(
            session.next_event().unwrap(),
            Event::ChunkBatch { chunks: 4 }
        );
        let sent = pipe.replies();
        assert_eq!(sent.len(), 1);
        let mut reader = Reader::new(&sent[0]);
        assert_eq!(
            reader.read_varint().unwrap(),
            ids::play::serverbound::CHUNK_BATCH_RECEIVED
        );
        let per_tick = reader.read_f32().unwrap();
        assert!((1.0..=64.0).contains(&per_tick));
    }

    fn chunk_batch_start() -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::CHUNK_BATCH_START);
        writer.into_inner()
    }

    #[test]
    fn inventory_without_components_is_stored_and_components_are_rejected() {
        let plain = WindowItems {
            window_id: 0,
            state_id: 1,
            items: vec![
                Slot::Empty,
                Slot::Item {
                    item_id: 5,
                    count: 3,
                },
            ],
            carried_item: Slot::Empty,
        };
        let component_bytes = {
            let mut writer = Writer::new();
            writer.write_varint(ids::play::clientbound::SET_SLOT);
            writer.write_u8(0);
            writer.write_varint(2);
            writer.write_i16(0);
            writer.write_varint(1);
            writer.write_varint(1);
            writer.write_varint(1);
            writer.write_varint(0);
            writer.into_inner()
        };
        let (mut session, _pipe) = session(&[
            join_game().encode().unwrap(),
            plain.encode().unwrap(),
            component_bytes,
        ]);
        session.next_event().unwrap();
        assert_eq!(
            session.next_event().unwrap(),
            Event::WindowItems {
                window_id: 0,
                slots: 2
            }
        );
        assert_eq!(session.player().inventory.len(), 2);
        assert_eq!(
            session.player().inventory[1],
            Slot::Item {
                item_id: 5,
                count: 3
            }
        );
        let event = session.next_event().unwrap();
        assert!(matches!(
            event,
            Event::Unsupported {
                packet: "SetContainerSlot",
                ..
            }
        ));
        assert_eq!(session.player().inventory[0], Slot::Empty);
        assert_eq!(session.stats().unsupported_packets, 1);
    }

    #[test]
    fn entities_spawn_move_and_disappear() {
        let spawn = SpawnEntity {
            entity_id: 7,
            uuid: 1,
            kind: 100,
            x: 1.0,
            y: 2.0,
            z: 3.0,
            pitch: 0,
            yaw: 0,
            head_pitch: 0,
            data: 0,
            velocity: [0; 3],
        };
        let moved = RelativeEntityMove {
            entity_id: 7,
            dx: 4096,
            dy: -2048,
            dz: 0,
            on_ground: true,
        };
        let removed = EntityDestroy {
            entity_ids: vec![7],
        };
        let (mut session, _pipe) = session(&[
            join_game().encode().unwrap(),
            spawn.encode().unwrap(),
            moved.encode().unwrap(),
            removed.encode().unwrap(),
        ]);
        session.next_event().unwrap();
        assert_eq!(session.next_event().unwrap(), Event::EntitySpawned(spawn));
        assert_eq!(
            session.next_event().unwrap(),
            Event::EntityMoved {
                entity_id: 7,
                x: 2.0,
                y: 1.5,
                z: 3.0
            }
        );
        assert_eq!(
            session.next_event().unwrap(),
            Event::EntitiesRemoved(vec![7])
        );
        assert!(session.entities().is_empty());
    }

    #[test]
    fn respawn_resets_the_world_and_dimension() {
        let respawn = Respawn {
            spawn: rkb_proto::packets::play::SpawnInfo {
                dimension_type: 0,
                dimension_name: "minecraft:overworld".to_owned(),
                hashed_seed: 0,
                game_mode: 1,
                previous_game_mode: 0,
                is_debug: false,
                is_flat: true,
                death: None,
                portal_cooldown: 0,
            },
            copy_metadata: 0,
        };
        let (mut session, _pipe) = session(&[
            join_game().encode().unwrap(),
            chunk_packet().encode().unwrap(),
            respawn.encode().unwrap(),
        ]);
        session.next_event().unwrap();
        session.next_event().unwrap();
        assert!(!session.chunks().is_empty());
        assert_eq!(session.next_event().unwrap(), Event::Respawned(respawn));
        assert!(session.chunks().is_empty());
        assert_eq!(session.player().game_mode, 1);
        assert!(!session.player().has_position);
    }

    #[test]
    fn disconnect_reason_is_readable() {
        let reason = anonymous_compound(&[("text", Tag::String("Server closed".to_owned()))]);
        let packet = KickDisconnect { reason_nbt: reason };
        let (mut session, _pipe) = session(&[packet.encode().unwrap()]);
        assert_eq!(
            session.next_event().unwrap(),
            Event::Disconnected {
                reason: "Server closed".to_owned()
            }
        );
    }

    #[test]
    fn start_configuration_is_acknowledged_and_reported() {
        let mut writer = Writer::new();
        writer.write_varint(ids::play::clientbound::START_CONFIGURATION);
        let (mut session, pipe) = session(&[writer.into_inner()]);
        assert_eq!(
            session.next_event().unwrap(),
            Event::ReconfigurationRequested
        );
        assert!(session.reconfiguration_requested());
        let sent = pipe.replies();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            Reader::new(&sent[0]).read_varint().unwrap(),
            ids::play::serverbound::CONFIGURATION_ACKNOWLEDGED
        );
    }

    #[test]
    fn unhandled_and_unknown_ids_are_distinguished() {
        let mut writer = Writer::new();
        writer.write_varint(0x0A); // Boss Event, inside the play range.
        let mut known = writer.into_inner();
        known.extend_from_slice(&[0, 0]);
        let mut writer = Writer::new();
        writer.write_varint(0x4000);
        let unknown = writer.into_inner();
        let (mut session, _pipe) = session(&[known, unknown]);
        assert_eq!(session.next_event().unwrap(), Event::Unhandled { id: 0x0A });
        assert_eq!(
            session.next_event().unwrap(),
            Event::Unknown {
                id: 0x4000,
                length: 0
            }
        );
        assert_eq!(session.stats().unhandled_packets, 1);
        assert_eq!(session.stats().unknown_packets, 1);
    }

    #[test]
    fn chat_and_game_state_events() {
        let chat = SystemChat {
            content: anonymous_compound(&[("text", Tag::String("hello".to_owned()))]),
            action_bar: false,
        };
        let state = GameStateChange {
            reason: GameStateChange::REASON_CHANGE_GAME_MODE,
            value: 1.0,
        };
        let (mut session, _pipe) = session(&[
            join_game().encode().unwrap(),
            chat.encode().unwrap(),
            state.encode().unwrap(),
        ]);
        session.next_event().unwrap();
        assert_eq!(
            session.next_event().unwrap(),
            Event::Chat {
                text: "hello".to_owned(),
                action_bar: false
            }
        );
        assert_eq!(session.next_event().unwrap(), Event::GameState(state));
        assert_eq!(session.player().game_mode, 1);
    }

    #[test]
    fn malformed_chunk_for_the_wrong_section_count_is_rejected() {
        let mut packet = chunk_packet();
        packet.data = vec![0, 0, 0, 9]; // one section, far too little data.
        let (mut session, _pipe) =
            session(&[join_game().encode().unwrap(), packet.encode().unwrap()]);
        session.next_event().unwrap();
        assert!(session.next_event().is_err());
        assert!(session.chunks().is_empty());
    }

    #[test]
    fn missing_dimension_registry_fails_chunk_decoding_instead_of_guessing() {
        let connection = Connection::new(ClientSide(Pipe::with_server_packets(&[
            join_game().encode().unwrap(),
            chunk_packet().encode().unwrap(),
        ])));
        let mut session = PlaySession::new(connection, outcome(), &[]).unwrap();
        session.next_event().unwrap();
        let error = session.next_event().unwrap_err().to_string();
        assert!(error.contains("not in the registry"));
    }
}
