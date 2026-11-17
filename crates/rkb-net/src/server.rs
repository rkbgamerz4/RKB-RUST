//! A renderer-facing server world built from a live play session.
//!
//! The protocol layer stores chunks compactly. This module converts them to the
//! expanded [`rkb_mc::Chunk`] shape the client mesher already understands, so a
//! server world uses the same block queries and mesh path as a save or a
//! generated world. Nothing here invents terrain: every block comes from a
//! server chunk or a server block update.

use std::collections::{BTreeMap, HashMap};
use std::net::{Shutdown, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rkb_mc::chunk::SECTION_VOLUME;
use rkb_mc::{BlockRegistry, BlockState, Chunk, Section};
use rkb_proto::error::{Error, Result};
use rkb_proto::packets::play::{ChunkColumn, PalettedContainer};

use crate::login::ConfiguredSession;
use crate::play::{Event, PlaySession};

/// Time allowed for the server to send Join Game and the first position.
const SETUP_TIMEOUT: Duration = Duration::from_secs(30);

/// The server moved the player. Coordinates are absolute.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ServerPosition {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
}

/// A joined server world.
///
/// A background thread owns the socket and answers keep-alives, teleports, and
/// chunk batches. The render thread drains converted chunks with [`poll`]
/// without blocking on the network.
///
/// [`poll`]: ServerWorld::poll
pub struct ServerWorld {
    receiver: Receiver<Update>,
    reader: Option<JoinHandle<()>>,
    shutdown: TcpStream,
    chunks: BTreeMap<(i32, i32), Arc<Chunk>>,
    overrides: BTreeMap<(i32, i32, i32), BlockState>,
    position: Option<ServerPosition>,
    position_dirty: bool,
    message: Option<String>,
    disconnected: Option<String>,
    error: Option<String>,
    chunks_received: usize,
    block_updates: usize,
}

impl std::fmt::Debug for ServerWorld {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServerWorld")
            .field("chunks", &self.chunks.len())
            .field("overrides", &self.overrides.len())
            .field("position", &self.position)
            .field("disconnected", &self.disconnected)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

enum Update {
    Chunk(Chunk),
    Unload {
        x: i32,
        z: i32,
    },
    Block {
        x: i32,
        y: i32,
        z: i32,
        state: BlockState,
    },
    Position(ServerPosition),
    Reset,
    Message(String),
    Disconnected(String),
    Closed(String),
    Error(String),
}

impl ServerWorld {
    /// Logs in to an offline-mode server and starts the play session.
    pub fn connect(
        address: &str,
        username: &str,
        timeout: Duration,
        registry: BlockRegistry,
    ) -> Result<Self> {
        let options = crate::login::LoginOptions::new(username);
        let configured = crate::login::login_configured(address, timeout, &options, None)?;
        Self::from_configured(configured, registry)
    }

    /// Starts from a connection that already finished configuration.
    ///
    /// Waits for Join Game and the first server position so the renderer never
    /// spawns the camera in a guessed place.
    pub fn from_configured(
        configured: ConfiguredSession<TcpStream>,
        registry: BlockRegistry,
    ) -> Result<Self> {
        let registry = Arc::new(registry);
        let mut session = PlaySession::from_configured(configured)?;
        session.set_read_timeout(Some(SETUP_TIMEOUT))?;

        let mut buffered = Vec::new();
        let mut joined = false;
        let mut received = 0_usize;
        let position = loop {
            match session.next_event()? {
                Event::Joined(packet) => {
                    joined = true;
                    buffered.push(Update::Message(format!(
                        "joined {} as entity {} (game mode {}, view distance {})",
                        packet.spawn.dimension_name,
                        packet.entity_id,
                        packet.spawn.game_mode,
                        packet.view_distance
                    )));
                }
                Event::ChunkLoaded { x, z, .. } => {
                    let Some(column) = session.chunk(x, z).cloned() else {
                        continue;
                    };
                    buffered.push(Update::Chunk(chunk_from_column(&column, &registry)?));
                    received += 1;
                }
                Event::BlockChanged(change) => {
                    let state = block_state(&registry, change.state_id)?;
                    buffered.push(Update::Block {
                        x: change.position.x,
                        y: change.position.y,
                        z: change.position.z,
                        state,
                    });
                }
                Event::ChunkUnloaded { x, z } => buffered.push(Update::Unload { x, z }),
                Event::Position(_) => {
                    let player = session.player();
                    break ServerPosition {
                        x: player.x,
                        y: player.y,
                        z: player.z,
                        yaw: player.yaw,
                        pitch: player.pitch,
                    };
                }
                Event::Disconnected { reason } => {
                    return Err(Error::protocol(format!(
                        "server disconnected before the world joined: {reason}"
                    )))
                }
                Event::Closed { reason } => {
                    return Err(Error::io(format!(
                        "connection closed before the world joined: {reason}"
                    )))
                }
                Event::ReconfigurationRequested => {
                    return Err(Error::protocol(
                        "server requested reconfiguration before the world joined",
                    ))
                }
                Event::Unsupported { packet, reason } => {
                    buffered.push(Update::Message(format!("unsupported {packet}: {reason}")));
                }
                _ => {}
            }
        };
        if !joined {
            return Err(Error::protocol("server sent a position before Join Game"));
        }

        session.set_read_timeout(None)?;
        let shutdown = session.try_clone_stream()?;
        let (sender, receiver) = mpsc::channel();
        let reader = thread::spawn(move || reader_loop(session, registry, sender, buffered));
        Ok(Self {
            receiver,
            reader: Some(reader),
            shutdown,
            chunks: BTreeMap::new(),
            overrides: BTreeMap::new(),
            position: Some(position),
            position_dirty: true,
            message: None,
            disconnected: None,
            error: None,
            chunks_received: received,
            block_updates: 0,
        })
    }

    /// Drains network updates without blocking.
    ///
    /// Returns the chunk columns whose geometry changed and should be meshed
    /// again: new chunks, re-sent chunks, and chunks with a block update.
    pub fn poll(&mut self) -> Vec<(i32, i32)> {
        let mut dirty = Vec::new();
        while let Ok(update) = self.receiver.try_recv() {
            match update {
                Update::Chunk(mut chunk) => {
                    let key = (chunk.x, chunk.z);
                    for (&(x, y, z), state) in &self.overrides {
                        if x.div_euclid(16) == key.0 && z.div_euclid(16) == key.1 {
                            chunk.set_block(x.rem_euclid(16), y, z.rem_euclid(16), state.clone());
                        }
                    }
                    self.chunks.insert(key, Arc::new(chunk));
                    self.chunks_received += 1;
                    dirty.push(key);
                }
                Update::Unload { x, z } => {
                    self.chunks.remove(&(x, z));
                }
                Update::Block { x, y, z, state } => {
                    self.overrides.insert((x, y, z), state.clone());
                    let key = (x.div_euclid(16), z.div_euclid(16));
                    if let Some(chunk) = self.chunks.get_mut(&key) {
                        Arc::make_mut(chunk).set_block(
                            x.rem_euclid(16),
                            y,
                            z.rem_euclid(16),
                            state,
                        );
                        dirty.push(key);
                    }
                    self.block_updates += 1;
                }
                Update::Position(position) => {
                    self.position = Some(position);
                    self.position_dirty = true;
                }
                Update::Reset => {
                    self.chunks.clear();
                    self.overrides.clear();
                }
                Update::Message(message) => self.message = Some(message),
                Update::Disconnected(reason) => self.disconnected = Some(reason),
                Update::Closed(reason) => {
                    self.disconnected = Some(format!("connection closed: {reason}"))
                }
                Update::Error(error) => self.error = Some(error),
            }
        }
        dirty.sort_unstable();
        dirty.dedup();
        dirty
    }

    /// A resident server chunk, ready for the existing mesher.
    #[must_use]
    pub fn chunk(&self, cx: i32, cz: i32) -> Option<Arc<Chunk>> {
        self.chunks.get(&(cx, cz)).cloned()
    }

    /// Number of resident chunk columns.
    #[must_use]
    pub fn loaded_chunks(&self) -> usize {
        self.chunks.len()
    }

    /// Number of chunk columns received over the network.
    #[must_use]
    pub fn chunks_received(&self) -> usize {
        self.chunks_received
    }

    /// Number of block updates applied.
    #[must_use]
    pub fn block_updates(&self) -> usize {
        self.block_updates
    }

    /// Block at absolute coordinates from server data only.
    #[must_use]
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> Option<BlockState> {
        self.chunks
            .get(&(x.div_euclid(16), z.div_euclid(16)))
            .and_then(|chunk| chunk.block_at(x.rem_euclid(16), y, z.rem_euclid(16)))
            .cloned()
    }

    /// The latest server position, when the server has sent one.
    #[must_use]
    pub fn position(&self) -> Option<ServerPosition> {
        self.position
    }

    /// Returns the server position once after it changes.
    pub fn take_position(&mut self) -> Option<ServerPosition> {
        if std::mem::take(&mut self.position_dirty) {
            self.position
        } else {
            None
        }
    }

    /// Takes the latest status message from the play session, if any.
    pub fn take_message(&mut self) -> Option<String> {
        self.message.take()
    }

    /// Why the server ended the session, if it did.
    #[must_use]
    pub fn disconnect_reason(&self) -> Option<&str> {
        self.disconnected.as_deref()
    }

    /// A protocol or conversion failure reported by the reader thread.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

impl Drop for ServerWorld {
    fn drop(&mut self) {
        let _ = self.shutdown.shutdown(Shutdown::Both);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn reader_loop(
    mut session: PlaySession<TcpStream>,
    registry: Arc<BlockRegistry>,
    sender: Sender<Update>,
    buffered: Vec<Update>,
) {
    for update in buffered {
        if sender.send(update).is_err() {
            return;
        }
    }
    loop {
        let update = match session.next_event() {
            Ok(Event::ChunkLoaded { x, z, .. }) => match session.chunk(x, z) {
                Some(column) => match chunk_from_column(column, &registry) {
                    Ok(chunk) => Update::Chunk(chunk),
                    Err(error) => {
                        Update::Error(format!("chunk ({x}, {z}) could not be converted: {error}"))
                    }
                },
                None => continue,
            },
            Ok(Event::BlockChanged(change)) => match block_state(&registry, change.state_id) {
                Ok(state) => Update::Block {
                    x: change.position.x,
                    y: change.position.y,
                    z: change.position.z,
                    state,
                },
                Err(error) => Update::Error(format!("block update rejected: {error}")),
            },
            Ok(Event::ChunkUnloaded { x, z }) => Update::Unload { x, z },
            Ok(Event::Respawned(_)) => Update::Reset,
            Ok(Event::Position(_)) => {
                let player = session.player();
                Update::Position(ServerPosition {
                    x: player.x,
                    y: player.y,
                    z: player.z,
                    yaw: player.yaw,
                    pitch: player.pitch,
                })
            }
            Ok(Event::Disconnected { reason }) => {
                let _ = sender.send(Update::Disconnected(reason));
                return;
            }
            Ok(Event::Closed { reason }) => {
                let _ = sender.send(Update::Closed(reason));
                return;
            }
            Ok(Event::ReconfigurationRequested) => {
                let _ = sender.send(Update::Error(
                    "server requested reconfiguration; this client cannot complete it".to_owned(),
                ));
                return;
            }
            Ok(Event::Unsupported { packet, reason }) => {
                let _ = sender.send(Update::Message(format!("unsupported {packet}: {reason}")));
                continue;
            }
            Ok(_) => continue,
            Err(error) => {
                let _ = sender.send(Update::Error(error.to_string()));
                return;
            }
        };
        if sender.send(update).is_err() {
            return;
        }
    }
}

fn block_state(registry: &BlockRegistry, state_id: i32) -> Result<BlockState> {
    let state_id = u32::try_from(state_id)
        .map_err(|_| Error::protocol(format!("negative block state id {state_id}")))?;
    registry.state(state_id).cloned().ok_or_else(|| {
        Error::protocol(format!(
            "block state id {state_id} is not in the supplied registry report"
        ))
    })
}

/// Converts one network chunk column to the renderer's chunk shape.
///
/// Every state id must resolve through the user-supplied, version-pinned
/// registry. A missing id is an error: the converter never substitutes air or
/// a guessed block.
pub fn chunk_from_column(column: &ChunkColumn, registry: &BlockRegistry) -> Result<Chunk> {
    let min_section_y = column.min_y >> 4;
    let mut sections = Vec::with_capacity(column.sections.len());
    for (index, section) in column.sections.iter().enumerate() {
        let section_y = i8::try_from(min_section_y + index as i32).map_err(|_| {
            Error::protocol(format!(
                "chunk ({}, {}) section Y {} does not fit the save shape",
                column.x, column.z, index
            ))
        })?;
        let mut palette: Vec<BlockState> = Vec::new();
        let mut palette_ids: HashMap<u32, u16> = HashMap::new();
        let mut cells = Vec::with_capacity(SECTION_VOLUME);
        for cell in 0..SECTION_VOLUME {
            let id = section.blocks.get(cell).ok_or_else(|| {
                Error::protocol(format!(
                    "chunk ({}, {}) section {index} is missing block {cell}",
                    column.x, column.z
                ))
            })?;
            let palette_index = match palette_ids.get(&id) {
                Some(&known) => known,
                None => {
                    let state = registry.state(id).cloned().ok_or_else(|| {
                        Error::protocol(format!(
                            "block state id {id} in chunk ({}, {}) is not in the supplied \
                             registry report",
                            column.x, column.z
                        ))
                    })?;
                    let known = u16::try_from(palette.len()).map_err(|_| {
                        Error::protocol("chunk section palette exceeds 65536 entries")
                    })?;
                    palette.push(state);
                    palette_ids.insert(id, known);
                    known
                }
            };
            cells.push(palette_index);
        }
        if palette.is_empty() {
            palette.push(BlockState::air());
            cells.resize(SECTION_VOLUME, 0);
        }
        sections.push(Section::new(section_y, palette, cells));
    }
    Chunk::new(crate::DATA_VERSION, column.x, column.z, sections)
        .map_err(|error| Error::protocol(format!("chunk could not be built: {error}")))
}

/// Resolves a single palette value. Kept for callers that inspect a container
/// without expanding a whole chunk.
#[must_use]
pub fn palette_value(container: &PalettedContainer, index: usize) -> Option<u32> {
    container.get(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rkb_proto::packets::play::ChunkSection;

    fn registry() -> BlockRegistry {
        BlockRegistry::from_json(
            r#"{
                "minecraft:air": { "states": [{ "id": 0, "default": true }] },
                "minecraft:stone": { "states": [{ "id": 1, "default": true }] },
                "minecraft:grass_block": {
                    "states": [{ "id": 2, "default": true, "properties": { "snowy": "false" } }]
                }
            }"#,
        )
        .unwrap()
    }

    fn column() -> ChunkColumn {
        let mut stone = Vec::with_capacity(SECTION_VOLUME);
        for cell in 0..SECTION_VOLUME {
            stone.push(if cell % 2 == 0 { 1 } else { 0 });
        }
        let mut section = ChunkSection {
            block_count: (SECTION_VOLUME / 2) as i16,
            blocks: PalettedContainer::Single(0),
            biomes: PalettedContainer::Single(0),
        };
        // Build a two-entry indirect palette: air and stone.
        let bits = 4u8;
        let per_word = 64 / usize::from(bits);
        let words = SECTION_VOLUME.div_ceil(per_word);
        let mut data = vec![0u64; words];
        for (cell, &id) in stone.iter().enumerate() {
            if id == 0 {
                continue;
            }
            let word = cell / per_word;
            let offset = (cell % per_word) * usize::from(bits);
            data[word] |= 1u64 << offset;
        }
        section.blocks = PalettedContainer::Indirect {
            bits_per_entry: bits,
            palette: vec![0, 1],
            data,
        };
        ChunkColumn {
            x: 3,
            z: -2,
            min_y: -64,
            sections: vec![section],
        }
    }

    #[test]
    fn network_chunk_converts_to_renderer_chunk() {
        let chunk = chunk_from_column(&column(), &registry()).unwrap();
        assert_eq!((chunk.x, chunk.z), (3, -2));
        assert_eq!(chunk.section_count(), 1);
        assert_eq!(
            chunk.block_at(0, -64, 0),
            Some(&BlockState::new("minecraft:stone"))
        );
        assert_eq!(chunk.block_at(1, -64, 0), Some(&BlockState::air()));
        assert_eq!(chunk.non_air_blocks(), SECTION_VOLUME / 2);
    }

    #[test]
    fn missing_state_id_is_an_error_not_a_guess() {
        let mut column = column();
        column.sections[0].blocks = PalettedContainer::Single(999);
        let error = chunk_from_column(&column, &registry()).unwrap_err();
        assert!(error.to_string().contains("not in the supplied registry"));
    }

    #[test]
    fn block_updates_are_resolved_through_the_registry() {
        assert_eq!(
            block_state(&registry(), 2).unwrap(),
            BlockState::new("minecraft:grass_block").with_property("snowy", "false")
        );
        assert!(block_state(&registry(), -1).is_err());
        assert!(block_state(&registry(), 4242).is_err());
    }

    #[test]
    fn palette_value_reads_single_and_indirect() {
        assert_eq!(palette_value(&PalettedContainer::Single(7), 4095), Some(7));
        let container = &column().sections[0].blocks;
        assert_eq!(palette_value(container, 0), Some(1));
        assert_eq!(palette_value(container, 1), Some(0));
    }
}
