//! A typed view of the server-list status document.
//!
//! The JSON is deliberately decoded leniently: servers add fields, and RKB
//! should display what it understands without rejecting the rest. The raw
//! document remains available on [`crate::PingResult`].

use serde_json::Value;

use crate::error::{Error, Result};

/// One player entry in `players.sample`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SamplePlayer {
    pub name: String,
    /// The profile UUID as the server sent it (usually dashed text).
    pub id: Option<String>,
}

/// The fields RKB currently uses from a status response.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServerStatus {
    /// `version.name`.
    pub version_name: Option<String>,
    /// `version.protocol`, when it fits in an `i32`.
    pub protocol: Option<i32>,
    /// `players.online`.
    pub players_online: Option<i64>,
    /// `players.max`.
    pub players_max: Option<i64>,
    /// `players.sample`.
    pub sample: Vec<SamplePlayer>,
    /// `description`, flattened to plain text.
    pub description: Option<String>,
    /// `favicon` as a data URI.
    pub favicon: Option<String>,
    /// `enforcesSecureChat`, when the server sends it.
    pub enforces_secure_chat: Option<bool>,
    /// `previewsChat`, when the server sends it.
    pub previews_chat: Option<bool>,
}

impl ServerStatus {
    /// Parses a status document. Unknown fields are ignored.
    pub fn parse(json: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(json)?;
        let object = value.as_object().ok_or_else(|| Error::Json {
            message: "status document is not an object".to_owned(),
        })?;

        let (version_name, protocol) =
            object
                .get("version")
                .and_then(Value::as_object)
                .map_or((None, None), |version| {
                    (
                        version
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        version
                            .get("protocol")
                            .and_then(Value::as_i64)
                            .and_then(|protocol| i32::try_from(protocol).ok()),
                    )
                });

        let (players_online, players_max, sample) = object
            .get("players")
            .and_then(Value::as_object)
            .map_or((None, None, Vec::new()), |players| {
                let sample = players
                    .get("sample")
                    .and_then(Value::as_array)
                    .map(|sample| {
                        sample
                            .iter()
                            .filter_map(Value::as_object)
                            .filter_map(|entry| {
                                let name = entry.get("name")?.as_str()?.to_owned();
                                let id = entry.get("id").and_then(Value::as_str).map(str::to_owned);
                                Some(SamplePlayer { name, id })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                (
                    players.get("online").and_then(Value::as_i64),
                    players.get("max").and_then(Value::as_i64),
                    sample,
                )
            });

        let description = object.get("description").map(|description| {
            let mut text = String::new();
            flatten(description, &mut text);
            text
        });

        Ok(Self {
            version_name,
            protocol,
            players_online,
            players_max,
            sample,
            description,
            favicon: object
                .get("favicon")
                .and_then(Value::as_str)
                .map(str::to_owned),
            enforces_secure_chat: object.get("enforcesSecureChat").and_then(Value::as_bool),
            previews_chat: object.get("previewsChat").and_then(Value::as_bool),
        })
    }

    /// Whether the server's protocol equals RKB's pinned protocol.
    #[must_use]
    pub fn speaks_this_protocol(&self) -> bool {
        self.protocol == Some(crate::PROTOCOL_VERSION)
    }
}

fn flatten(value: &Value, out: &mut String) {
    match value {
        Value::String(text) => out.push_str(text),
        Value::Array(parts) => {
            for part in parts {
                flatten(part, out);
            }
        }
        Value::Object(object) => {
            if let Some(text) = object.get("text").and_then(Value::as_str) {
                out.push_str(text);
            } else if let Some(key) = object.get("translate").and_then(Value::as_str) {
                // A translation key without the language table is still more
                // useful than dropping the component.
                out.push_str(key);
            }
            if let Some(extra) = object.get("extra").and_then(Value::as_array) {
                for part in extra {
                    flatten(part, out);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_typical_document() {
        let json = r#"{
            "version": {"name": "1.21.1", "protocol": 767},
            "players": {
                "max": 20,
                "online": 1,
                "sample": [{"name": "Steve", "id": "00000000-0000-0000-0000-000000000000"}]
            },
            "description": {"text": "Hello ", "extra": [{"text": "world"}, {"color": "red"}]},
            "favicon": "data:image/png;base64,AA==",
            "enforcesSecureChat": true,
            "previewsChat": false
        }"#;
        let status = ServerStatus::parse(json).unwrap();
        assert_eq!(status.version_name.as_deref(), Some("1.21.1"));
        assert_eq!(status.protocol, Some(767));
        assert_eq!(status.players_online, Some(1));
        assert_eq!(status.players_max, Some(20));
        assert_eq!(status.sample.len(), 1);
        assert_eq!(status.sample[0].name, "Steve");
        assert_eq!(status.description.as_deref(), Some("Hello world"));
        assert_eq!(
            status.favicon.as_deref(),
            Some("data:image/png;base64,AA==")
        );
        assert_eq!(status.enforces_secure_chat, Some(true));
        assert_eq!(status.previews_chat, Some(false));
        assert!(status.speaks_this_protocol());
    }

    #[test]
    fn missing_fields_remain_none() {
        let status = ServerStatus::parse("{}").unwrap();
        assert_eq!(status, ServerStatus::default());
        assert!(!status.speaks_this_protocol());
    }

    #[test]
    fn rejects_non_object_documents() {
        assert!(matches!(ServerStatus::parse("[]"), Err(Error::Json { .. })));
        assert!(matches!(
            ServerStatus::parse("not json"),
            Err(Error::Json { .. })
        ));
    }

    #[test]
    fn protocol_outside_i32_is_ignored() {
        let status = ServerStatus::parse(r#"{"version":{"protocol":4294967296}}"#).unwrap();
        assert_eq!(status.protocol, None);
    }
}
