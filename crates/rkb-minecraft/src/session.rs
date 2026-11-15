//! Client-owned world session state.
//!
//! A Minecraft save stores terrain, but it does not store an RKB seed, the
//! player's exact camera, or blocks edited in a read-only region world. This
//! file carries those values next to `level.dat`:
//!
//! - the seed that reproduces generated terrain,
//! - spawn and player pose,
//! - an overlay of block edits.
//!
//! The file is written atomically. A corrupt or incompatible file is reported
//! as an error so the caller can decide whether to recover, rather than being
//! silently ignored.

use std::fmt;
use std::path::Path;

use rkb_core::Vec3;
use rkb_mc::BlockState;
use serde_json::{json, Value};

use crate::mesh::EditMap;

/// Session filename inside the world directory.
pub const SESSION_FILE: &str = "rkb-session.json";
/// Marker proving the directory was created by the RKB generated-world path.
pub const GENERATED_MARKER: &str = "rkb-generated-world";
/// Current session format version.
pub const FORMAT: u32 = 1;

/// A saved player pose.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerSnapshot {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub flying: bool,
}

/// Session data serialized beside a save.
#[derive(Clone, Debug, Default)]
pub struct Session {
    /// Present for a generated world; absent for a user-supplied save.
    pub seed: Option<u64>,
    pub spawn: Option<Vec3>,
    pub player: Option<PlayerSnapshot>,
    pub edits: EditMap,
}

/// Session read failures.
#[derive(Debug)]
pub struct SessionError(pub String);

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SessionError {}

fn number(value: &Value, key: &str) -> Result<f64, SessionError> {
    value
        .get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| SessionError(format!("session field {key:?} is not a number")))
}

fn vector(value: &Value, key: &str) -> Result<Option<Vec3>, SessionError> {
    let Some(array) = value.get(key) else {
        return Ok(None);
    };
    let array = array
        .as_array()
        .ok_or_else(|| SessionError(format!("session field {key:?} is not an array")))?;
    if array.len() != 3 {
        return Err(SessionError(format!(
            "session field {key:?} must have three numbers"
        )));
    }
    let mut values = [0.0_f32; 3];
    for (index, entry) in array.iter().enumerate() {
        let number = entry
            .as_f64()
            .ok_or_else(|| SessionError(format!("session {key}[{index}] is not a number")))?;
        values[index] = number as f32;
    }
    Ok(Some(Vec3::new(values[0], values[1], values[2])))
}

/// Reads a session file. `Ok(None)` means it does not exist.
pub fn read(dir: &Path) -> Result<Option<Session>, SessionError> {
    let path = dir.join(SESSION_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(SessionError(format!("read {}: {error}", path.display()))),
    };
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| SessionError(format!("parse {}: {error}", path.display())))?;
    let format = value.get("format").and_then(Value::as_u64).unwrap_or(0);
    if format != u64::from(FORMAT) {
        return Err(SessionError(format!(
            "unsupported session format {format}; this build writes {FORMAT}"
        )));
    }
    let seed = value.get("seed").and_then(Value::as_u64);
    let spawn = vector(&value, "spawn")?;
    let player = match value.get("player") {
        None | Some(Value::Null) => None,
        Some(player) => Some(PlayerSnapshot {
            position: vector(player, "position")?.unwrap_or(Vec3::ZERO),
            yaw: number(player, "yaw")? as f32,
            pitch: number(player, "pitch")? as f32,
            flying: player
                .get("flying")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }),
    };

    let mut edits = EditMap::new();
    if let Some(array) = value.get("edits") {
        let array = array
            .as_array()
            .ok_or_else(|| SessionError("session edits is not an array".to_string()))?;
        for (index, entry) in array.iter().enumerate() {
            let entry = entry
                .as_array()
                .ok_or_else(|| SessionError(format!("edit {index} is not an array")))?;
            if entry.len() != 4 && entry.len() != 5 {
                return Err(SessionError(format!(
                    "edit {index} must be [x, y, z, block] or [x, y, z, block, properties]"
                )));
            }
            let coordinate = |slot: usize| -> Result<i32, SessionError> {
                entry[slot]
                    .as_i64()
                    .and_then(|value| i32::try_from(value).ok())
                    .ok_or_else(|| {
                        SessionError(format!("edit {index} coordinate {slot} is out of range"))
                    })
            };
            let name = entry[3]
                .as_str()
                .ok_or_else(|| SessionError(format!("edit {index} block is not a string")))?;
            if !name.contains(':') {
                return Err(SessionError(format!(
                    "edit {index} block {name:?} is not namespaced"
                )));
            }
            let mut state = BlockState::new(name);
            if let Some(properties) = entry.get(4) {
                let properties = properties.as_object().ok_or_else(|| {
                    SessionError(format!("edit {index} properties is not an object"))
                })?;
                for (key, value) in properties {
                    let value = value.as_str().ok_or_else(|| {
                        SessionError(format!("edit {index} property {key:?} is not a string"))
                    })?;
                    state.properties.insert(key.clone(), value.to_owned());
                }
            }
            edits.insert((coordinate(0)?, coordinate(1)?, coordinate(2)?), state);
        }
    }

    Ok(Some(Session {
        seed,
        spawn,
        player,
        edits,
    }))
}

/// Writes the session file atomically.
pub fn write(dir: &Path, session: &Session, saved_at_ms: i64) -> Result<(), SessionError> {
    let edits: Vec<Value> = session
        .edits
        .iter()
        .map(|((x, y, z), state)| json!([x, y, z, state.name, state.properties]))
        .collect();
    let player = session.player.map(|player| {
        json!({
            "position": [player.position.x, player.position.y, player.position.z],
            "yaw": player.yaw,
            "pitch": player.pitch,
            "flying": player.flying,
        })
    });
    let value = json!({
        "format": FORMAT,
        "saved_at": saved_at_ms,
        "seed": session.seed,
        "spawn": session.spawn.map(|spawn| [spawn.x, spawn.y, spawn.z]),
        "player": player,
        "edits": edits,
    });
    let bytes = serde_json::to_vec_pretty(&value)
        .map_err(|error| SessionError(format!("encode session: {error}")))?;
    rkb_mc::save::write_atomic(&dir.join(SESSION_FILE), &bytes)
        .map_err(|error| SessionError(error.to_string()))
}

/// Marks a directory as an RKB generated world.
pub fn mark_generated(dir: &Path) -> Result<(), SessionError> {
    rkb_mc::save::write_atomic(
        &dir.join(GENERATED_MARKER),
        b"RKB RUST generated world. Terrain is reproduced from the seed in \
          rkb-session.json; no Mojang data is stored here.\n",
    )
    .map_err(|error| SessionError(error.to_string()))
}

#[must_use]
pub fn is_generated(dir: &Path) -> bool {
    dir.join(GENERATED_MARKER).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rkb_mc::fixture::TempDir;

    #[test]
    fn session_round_trips_seed_player_and_edits() {
        let dir = TempDir::new("session-roundtrip");
        assert!(read(dir.path()).unwrap().is_none());

        let mut session = Session {
            seed: Some(DEFAULT_TEST_SEED),
            spawn: Some(Vec3::new(1.5, 70.0, 2.5)),
            player: Some(PlayerSnapshot {
                position: Vec3::new(3.5, 71.0, 4.5),
                yaw: 1.25,
                pitch: -0.5,
                flying: true,
            }),
            edits: EditMap::new(),
        };
        session
            .edits
            .insert((4, 70, 5), BlockState::new("minecraft:oak_planks"));
        let mut log = BlockState::new("minecraft:oak_log");
        log.properties.insert("axis".to_string(), "y".to_string());
        session.edits.insert((5, 70, 5), log);
        write(dir.path(), &session, 123).unwrap();

        let read_back = read(dir.path()).unwrap().expect("session exists");
        assert_eq!(read_back.seed, Some(DEFAULT_TEST_SEED));
        assert_eq!(read_back.spawn, Some(Vec3::new(1.5, 70.0, 2.5)));
        let player = read_back.player.expect("player");
        assert!((player.yaw - 1.25).abs() < 0.0001);
        assert!(player.flying);
        assert_eq!(
            read_back.edits.get(&(4, 70, 5)).map(ToString::to_string),
            Some("minecraft:oak_planks".to_string())
        );
        assert_eq!(
            read_back.edits.get(&(5, 70, 5)).map(ToString::to_string),
            Some("minecraft:oak_log[axis=y]".to_string())
        );
    }

    #[test]
    fn corrupt_session_is_reported() {
        let dir = TempDir::new("session-corrupt");
        std::fs::write(dir.path().join(SESSION_FILE), b"{not json").unwrap();
        assert!(read(dir.path()).is_err());

        std::fs::write(
            dir.path().join(SESSION_FILE),
            br#"{"format":99,"edits":[]}"#,
        )
        .unwrap();
        let error = read(dir.path()).expect_err("format must be rejected");
        assert!(error.to_string().contains("unsupported session format"));
    }

    #[test]
    fn generated_marker_is_explicit() {
        let dir = TempDir::new("session-marker");
        assert!(!is_generated(dir.path()));
        mark_generated(dir.path()).unwrap();
        assert!(is_generated(dir.path()));
    }

    const DEFAULT_TEST_SEED: u64 = 0x1A21_5EED_5EED_0001;
}
