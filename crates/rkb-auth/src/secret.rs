//! Secret values with redacted formatting and zeroization on drop.
//!
//! This is deliberately small. It prevents the two most common leaks in a
//! client: a `Debug`/`Display` of a struct that happens to contain a token, and
//! a token lingering in a heap buffer after logout. It cannot stop a caller
//! from deliberately copying the value; `SecretString::expose` is therefore
//! crate-private.

use std::fmt;
use std::ops::{Deref, DerefMut};

use serde_json::Value;
use zeroize::Zeroize;

/// An owned UTF-8 secret. `Debug` prints `<redacted>` and the backing buffer is
/// zeroed when the value is dropped.
#[derive(Clone, Default)]
pub struct SecretString(String);

impl SecretString {
    /// Wraps a token or other secret.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The secret for an HTTP header or request body.
    ///
    /// Crate-private so application code cannot accidentally log, serialize,
    /// or store the raw value.
    #[must_use]
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }

    /// Whether the secret is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The length in bytes, for diagnostics that must not reveal content.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// A JSON document that zeroes every string it still owns when dropped.
///
/// Authentication responses and requests carry tokens inside JSON. Moving a
/// token into [`SecretString`] removes the main copy; this guard covers any
/// duplicate left in the tree.
pub(crate) struct ZeroizingJson(Value);

impl ZeroizingJson {
    /// Wraps a parsed document.
    pub(crate) fn new(value: Value) -> Self {
        Self(value)
    }

    /// Parses bytes and zeroes the source buffer afterwards.
    pub(crate) fn parse(bytes: &mut [u8]) -> Result<Self, serde_json::Error> {
        let parsed = serde_json::from_slice(bytes);
        bytes.zeroize();
        parsed.map(Self)
    }

    /// Serializes the document into a new string. The caller must move it into
    /// an [`crate::http::HttpRequest`] or zero it explicitly.
    pub(crate) fn serialize(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&self.0)
    }
}

impl Deref for ZeroizingJson {
    type Target = Value;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ZeroizingJson {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for ZeroizingJson {
    fn drop(&mut self) {
        zeroize_value(&mut self.0);
    }
}

fn zeroize_value(value: &mut Value) {
    match value {
        Value::String(text) => text.zeroize(),
        Value::Array(items) => {
            for item in items {
                zeroize_value(item);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                zeroize_value(item);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_the_value() {
        let secret = SecretString::new("super-secret-token");
        assert_eq!(format!("{secret:?}"), "<redacted>");
        assert_eq!(secret.expose(), "super-secret-token");
        assert_eq!(secret.len(), 18);
        assert!(!secret.is_empty());
    }

    #[test]
    fn clone_is_independent_but_still_redacted() {
        let first = SecretString::new("a");
        let second = first.clone();
        assert_eq!(first.expose(), second.expose());
        assert_eq!(format!("{second:?}"), "<redacted>");
    }
}
