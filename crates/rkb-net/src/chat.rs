//! Plain text from Minecraft chat components.
//!
//! Login disconnects carry a JSON component; configuration and play
//! disconnects carry anonymous network NBT. Both are reduced to readable text
//! for diagnostics only. No styling, hover events, or click actions are kept.

use rkb_mc::{Nbt, Tag};
use serde_json::Value;

/// Extracts plain text from a JSON chat component.
#[must_use]
pub fn json_component_plain_text(json: &str) -> Option<String> {
    let value: Value = serde_json::from_str(json).ok()?;
    component_from_json(&value)
}

/// Extracts plain text from an anonymous network-NBT chat component.
#[must_use]
pub fn nbt_component_plain_text(bytes: &[u8]) -> Option<String> {
    let tag = Nbt::read_anonymous(bytes).ok()?;
    component_from_nbt(&tag)
}

fn component_from_json(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().filter_map(component_from_json).collect();
            if parts.is_empty() {
                None
            } else {
                Some(parts.concat())
            }
        }
        Value::Object(map) => {
            let mut text = String::new();
            if let Some(value) = map.get("text").and_then(Value::as_str) {
                text.push_str(value);
            } else if let Some(value) = map.get("translate").and_then(Value::as_str) {
                text.push_str(value);
            }
            if let Some(extra) = map.get("extra").and_then(Value::as_array) {
                for child in extra {
                    if let Some(part) = component_from_json(child) {
                        text.push_str(&part);
                    }
                }
            }
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        }
        _ => None,
    }
}

fn component_from_nbt(tag: &Tag) -> Option<String> {
    match tag {
        Tag::String(text) => Some(text.clone()),
        Tag::List(items) => {
            let parts: Vec<String> = items.iter().filter_map(component_from_nbt).collect();
            if parts.is_empty() {
                None
            } else {
                Some(parts.concat())
            }
        }
        Tag::Compound(map) => {
            let mut text = String::new();
            if let Some(value) = map.get("text").and_then(Tag::as_str) {
                text.push_str(value);
            } else if let Some(value) = map.get("translate").and_then(Tag::as_str) {
                text.push_str(value);
            }
            if let Some(extra) = map.get("extra").and_then(Tag::as_list) {
                for child in extra {
                    if let Some(part) = component_from_nbt(child) {
                        text.push_str(&part);
                    }
                }
            }
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn json_text_and_extra_are_combined() {
        assert_eq!(
            json_component_plain_text(r#"{"text":"Hello ","extra":[{"text":"world"}]}"#),
            Some("Hello world".to_owned())
        );
        assert_eq!(
            json_component_plain_text(r#""plain""#),
            Some("plain".to_owned())
        );
        assert_eq!(
            json_component_plain_text(r#"[{"text":"a"},{"text":"b"}]"#),
            Some("ab".to_owned())
        );
        assert_eq!(json_component_plain_text("not json"), None);
    }

    #[test]
    fn anonymous_nbt_text_is_extracted() {
        let mut root = BTreeMap::new();
        root.insert("text".to_owned(), Tag::String("NBT reason".to_owned()));
        let named = Nbt::write(&Tag::Compound(root)).unwrap();
        let mut anonymous = Vec::new();
        anonymous.push(named[0]);
        anonymous.extend_from_slice(&named[3..]);
        assert_eq!(
            nbt_component_plain_text(&anonymous),
            Some("NBT reason".to_owned())
        );
        assert_eq!(nbt_component_plain_text(&[0x00]), None);
        assert_eq!(nbt_component_plain_text(&[]), None);
    }
}
