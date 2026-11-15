//! Block-state identities from a version-pinned registry report.
//!
//! Chunk palettes already carry block names and properties, so this crate can
//! read a world without a registry. A registry is still needed for stable
//! numeric state ids, default states, and validation, and it must come from the
//! exact Minecraft version that produced the save.
//!
//! The accepted format is Mojang's generated `blocks.json` report:
//!
//! ```json
//! {
//!   "minecraft:stone": {
//!     "states": [{ "id": 1, "default": true }]
//!   },
//!   "minecraft:oak_log": {
//!     "states": [
//!       { "id": 113, "default": true, "properties": { "axis": "y" } },
//!       { "id": 114, "properties": { "axis": "x" } }
//!     ]
//!   }
//! }
//! ```
//!
//! Reports are not bundled with RKB RUST. Generate them from the user's own
//! game installation; no Mojang data is redistributed here.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::chunk::BlockState;

/// A dense, validated block-state registry.
#[derive(Clone, Debug, Default)]
pub struct BlockRegistry {
    states: Vec<BlockState>,
    ids: HashMap<BlockState, u32>,
    defaults: BTreeMap<String, u32>,
}

impl BlockRegistry {
    /// Parses a `blocks.json`-style report from a string.
    pub fn from_json(json: &str) -> Result<Self, RegistryError> {
        let value: Value =
            serde_json::from_str(json).map_err(|error| RegistryError::Json(error.to_string()))?;
        Self::from_value(&value)
    }

    /// Parses a `blocks.json`-style report from a file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, RegistryError> {
        let path = path.as_ref();
        let json = fs::read_to_string(path).map_err(|error| RegistryError::Io {
            path: path.display().to_string(),
            message: error.to_string(),
        })?;
        Self::from_json(&json)
    }

    /// Parses an already-decoded report.
    pub fn from_value(value: &Value) -> Result<Self, RegistryError> {
        let blocks = value.as_object().ok_or(RegistryError::NotObject)?;
        if blocks.is_empty() {
            return Err(RegistryError::Empty);
        }

        let mut slots: Vec<Option<BlockState>> = Vec::new();
        let mut ids: HashMap<BlockState, u32> = HashMap::new();
        let mut defaults: BTreeMap<String, u32> = BTreeMap::new();

        for (name, block) in blocks {
            if name.is_empty() {
                return Err(RegistryError::EmptyBlockName);
            }
            let block = block
                .as_object()
                .ok_or_else(|| RegistryError::BlockNotObject(name.clone()))?;
            let states = block
                .get("states")
                .ok_or_else(|| RegistryError::MissingStates(name.clone()))?;
            let states = states
                .as_array()
                .ok_or_else(|| RegistryError::StatesNotArray(name.clone()))?;
            if states.is_empty() {
                return Err(RegistryError::NoStates(name.clone()));
            }

            let mut default = None;
            for (index, state) in states.iter().enumerate() {
                let state = state
                    .as_object()
                    .ok_or_else(|| RegistryError::StateNotObject {
                        block: name.clone(),
                        index,
                    })?;
                let id = state
                    .get("id")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| RegistryError::BadId {
                        block: name.clone(),
                        index,
                    })
                    .and_then(|id| {
                        u32::try_from(id).map_err(|_| RegistryError::BadId {
                            block: name.clone(),
                            index,
                        })
                    })?;

                let mut properties = BTreeMap::new();
                if let Some(values) = state.get("properties") {
                    let values =
                        values
                            .as_object()
                            .ok_or_else(|| RegistryError::BadProperties {
                                block: name.clone(),
                                index,
                            })?;
                    for (key, value) in values {
                        if key.is_empty() {
                            return Err(RegistryError::EmptyPropertyName {
                                block: name.clone(),
                                index,
                            });
                        }
                        let Some(value) = value.as_str() else {
                            return Err(RegistryError::PropertyNotString {
                                block: name.clone(),
                                index,
                                property: key.clone(),
                            });
                        };
                        properties.insert(key.clone(), value.to_string());
                    }
                }

                let is_default = match state.get("default") {
                    None => false,
                    Some(Value::Bool(value)) => *value,
                    Some(_) => {
                        return Err(RegistryError::DefaultNotBool {
                            block: name.clone(),
                            index,
                        })
                    }
                };

                let block_state = BlockState {
                    name: name.clone(),
                    properties,
                };
                let slot = usize::try_from(id).map_err(|_| RegistryError::BadId {
                    block: name.clone(),
                    index,
                })?;
                if slot >= slots.len() {
                    slots.resize(slot + 1, None);
                }
                if slots[slot].replace(block_state.clone()).is_some() {
                    return Err(RegistryError::DuplicateId { id });
                }
                if ids.insert(block_state, id).is_some() {
                    return Err(RegistryError::DuplicateState { name: name.clone() });
                }
                if is_default && default.replace(id).is_some() {
                    return Err(RegistryError::MultipleDefaults(name.clone()));
                }
            }

            let default = default.ok_or_else(|| RegistryError::NoDefault(name.clone()))?;
            defaults.insert(name.clone(), default);
        }

        let mut states = Vec::with_capacity(slots.len());
        for (id, slot) in slots.into_iter().enumerate() {
            let Some(state) = slot else {
                return Err(RegistryError::NonContiguous { missing: id as u32 });
            };
            states.push(state);
        }
        Ok(Self {
            states,
            ids,
            defaults,
        })
    }

    /// Number of block states.
    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    /// Number of distinct blocks.
    pub fn block_count(&self) -> usize {
        self.defaults.len()
    }

    /// State by numeric id. Ids are dense by construction.
    pub fn state(&self, id: u32) -> Option<&BlockState> {
        self.states.get(id as usize)
    }

    /// Numeric id of an exact state.
    pub fn id(&self, state: &BlockState) -> Option<u32> {
        self.ids.get(state).copied()
    }

    /// Numeric id of a block's default state.
    pub fn default_id(&self, block: &str) -> Option<u32> {
        self.defaults.get(block).copied()
    }

    /// The default state of a block.
    pub fn default_state(&self, block: &str) -> Option<&BlockState> {
        self.default_id(block).and_then(|id| self.state(id))
    }

    pub fn states(&self) -> impl Iterator<Item = &BlockState> {
        self.states.iter()
    }

    /// Names of all blocks in the registry, sorted.
    pub fn blocks(&self) -> impl Iterator<Item = &str> {
        self.defaults.keys().map(String::as_str)
    }
}

/// Registry report failures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryError {
    Io {
        path: String,
        message: String,
    },
    Json(String),
    NotObject,
    Empty,
    EmptyBlockName,
    BlockNotObject(String),
    MissingStates(String),
    NoStates(String),
    StatesNotArray(String),
    StateNotObject {
        block: String,
        index: usize,
    },
    BadId {
        block: String,
        index: usize,
    },
    BadProperties {
        block: String,
        index: usize,
    },
    PropertyNotString {
        block: String,
        index: usize,
        property: String,
    },
    EmptyPropertyName {
        block: String,
        index: usize,
    },
    DefaultNotBool {
        block: String,
        index: usize,
    },
    NoDefault(String),
    MultipleDefaults(String),
    DuplicateState {
        name: String,
    },
    DuplicateId {
        id: u32,
    },
    NonContiguous {
        missing: u32,
    },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "read registry {path}: {message}"),
            Self::Json(message) => write!(f, "registry is not valid JSON: {message}"),
            Self::NotObject => f.write_str("registry root is not a JSON object"),
            Self::Empty => f.write_str("registry has no blocks"),
            Self::EmptyBlockName => f.write_str("registry has an empty block name"),
            Self::BlockNotObject(name) => write!(f, "block {name} is not a JSON object"),
            Self::MissingStates(name) => write!(f, "block {name} has no states array"),
            Self::NoStates(name) => write!(f, "block {name} has an empty states array"),
            Self::StatesNotArray(name) => write!(f, "block {name} states is not an array"),
            Self::StateNotObject { block, index } => {
                write!(f, "block {block} state {index} is not an object")
            }
            Self::BadId { block, index } => {
                write!(f, "block {block} state {index} has no valid numeric id")
            }
            Self::BadProperties { block, index } => {
                write!(f, "block {block} state {index} properties is not an object")
            }
            Self::PropertyNotString {
                block,
                index,
                property,
            } => write!(
                f,
                "block {block} state {index} property {property} is not a string"
            ),
            Self::EmptyPropertyName { block, index } => {
                write!(f, "block {block} state {index} has an empty property name")
            }
            Self::DefaultNotBool { block, index } => {
                write!(f, "block {block} state {index} default is not a boolean")
            }
            Self::NoDefault(name) => write!(f, "block {name} has no default state"),
            Self::MultipleDefaults(name) => {
                write!(f, "block {name} has more than one default state")
            }
            Self::DuplicateState { name } => {
                write!(f, "block {name} repeats a state with the same properties")
            }
            Self::DuplicateId { id } => write!(f, "state id {id} is used more than once"),
            Self::NonContiguous { missing } => {
                write!(
                    f,
                    "state id {missing} is missing; ids must be contiguous from 0"
                )
            }
        }
    }
}

impl std::error::Error for RegistryError {}

#[cfg(test)]
mod tests {
    use super::*;

    const REPORT: &str = r#"
    {
      "minecraft:air": {
        "states": [{ "id": 0, "default": true }]
      },
      "minecraft:stone": {
        "states": [{ "id": 1, "default": true }]
      },
      "minecraft:oak_log": {
        "properties": { "axis": ["x", "y", "z"] },
        "states": [
          { "id": 2, "properties": { "axis": "x" } },
          { "id": 3, "default": true, "properties": { "axis": "y" } },
          { "id": 4, "properties": { "axis": "z" } }
        ]
      }
    }
    "#;

    #[test]
    fn parses_ids_defaults_and_properties() {
        let registry = BlockRegistry::from_json(REPORT).expect("parse");
        assert_eq!(registry.len(), 5);
        assert_eq!(registry.block_count(), 3);
        assert_eq!(registry.default_id("minecraft:stone"), Some(1));
        assert_eq!(
            registry
                .default_state("minecraft:oak_log")
                .map(ToString::to_string),
            Some("minecraft:oak_log[axis=y]".to_string())
        );
        let state = BlockState::new("minecraft:oak_log").with_property("axis", "z");
        assert_eq!(registry.id(&state), Some(4));
        assert_eq!(registry.state(4), Some(&state));
        assert_eq!(
            registry.id(&BlockState::new("minecraft:oak_log")),
            None,
            "a non-default property set must not match"
        );
        assert_eq!(registry.default_id("minecraft:missing"), None);
        assert_eq!(
            registry.blocks().collect::<Vec<_>>(),
            vec!["minecraft:air", "minecraft:oak_log", "minecraft:stone"]
        );
    }

    #[test]
    fn rejects_duplicate_ids_and_states() {
        let duplicate_id = r#"
        {
          "minecraft:a": { "states": [{ "id": 0, "default": true }] },
          "minecraft:b": { "states": [{ "id": 0, "default": true }] }
        }
        "#;
        assert!(matches!(
            BlockRegistry::from_json(duplicate_id),
            Err(RegistryError::DuplicateId { id: 0 })
        ));

        let duplicate_state = r#"
        {
          "minecraft:a": {
            "states": [
              { "id": 0, "default": true },
              { "id": 1 }
            ]
          }
        }
        "#;
        assert!(matches!(
            BlockRegistry::from_json(duplicate_state),
            Err(RegistryError::DuplicateState { .. })
        ));
    }

    #[test]
    fn rejects_incomplete_reports() {
        let no_default = r#"
        { "minecraft:a": { "states": [{ "id": 0 }] } }
        "#;
        assert!(matches!(
            BlockRegistry::from_json(no_default),
            Err(RegistryError::NoDefault(_))
        ));

        let two_defaults = r#"
        {
          "minecraft:a": {
            "states": [
              { "id": 0, "default": true, "properties": { "variant": "a" } },
              { "id": 1, "default": true, "properties": { "variant": "b" } }
            ]
          }
        }
        "#;
        assert!(matches!(
            BlockRegistry::from_json(two_defaults),
            Err(RegistryError::MultipleDefaults(_))
        ));

        let gap = r#"
        { "minecraft:a": { "states": [{ "id": 2, "default": true }] } }
        "#;
        assert!(matches!(
            BlockRegistry::from_json(gap),
            Err(RegistryError::NonContiguous { missing: 0 })
        ));
    }

    #[test]
    fn rejects_malformed_shapes() {
        assert!(matches!(
            BlockRegistry::from_json("[]"),
            Err(RegistryError::NotObject)
        ));
        assert!(matches!(
            BlockRegistry::from_json("{}"),
            Err(RegistryError::Empty)
        ));
        assert!(matches!(
            BlockRegistry::from_json(
                r#"{"minecraft:a":{"states":[{"id":0,"default":true,"properties":{"axis":1}}]}}"#
            ),
            Err(RegistryError::PropertyNotString { .. })
        ));
        assert!(matches!(
            BlockRegistry::from_json(r#"{"minecraft:a":{"states":[{"id":0,"default":"yes"}]}}"#),
            Err(RegistryError::DefaultNotBool { .. })
        ));
        assert!(matches!(
            BlockRegistry::from_json("not json"),
            Err(RegistryError::Json(_))
        ));
    }

    #[test]
    fn reads_a_report_file() {
        let dir = std::env::temp_dir().join(format!(
            "rkb-mc-registry-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("blocks.json");
        fs::write(&path, REPORT).expect("write report");
        let registry = BlockRegistry::from_file(&path).expect("parse file");
        assert_eq!(registry.len(), 5);
        let error = BlockRegistry::from_file(dir.join("missing.json")).expect_err("missing file");
        assert!(error.to_string().contains("missing.json"));
        let _ = fs::remove_dir_all(&dir);
    }
}
