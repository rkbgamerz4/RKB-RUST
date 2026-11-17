//! Small, strict JSON field helpers.
//!
//! Each `take_*` moves the value out of the document so the [`ZeroizingJson`]
//! guard can zero whatever remains. Missing fields are errors rather than
//! silently defaulted, except where the protocol says a field is optional.

use serde_json::Value;

use crate::error::AuthError;
use crate::secret::SecretString;

/// Takes a string field, if present and a string.
pub(crate) fn take_optional_string(value: &mut Value, key: &str) -> Option<String> {
    match value.get_mut(key).map(Value::take) {
        Some(Value::String(text)) => Some(text),
        _ => None,
    }
}

/// Takes a required string field.
pub(crate) fn take_required_string(value: &mut Value, key: &str) -> Result<String, AuthError> {
    take_optional_string(value, key)
        .ok_or_else(|| AuthError::json(format!("response has no {key}")))
}

/// Takes a secret string field, if present.
pub(crate) fn take_optional_secret(value: &mut Value, key: &str) -> Option<SecretString> {
    take_optional_string(value, key).map(SecretString::new)
}

/// Takes a required secret string field.
pub(crate) fn take_required_secret(
    value: &mut Value,
    key: &str,
) -> Result<SecretString, AuthError> {
    take_optional_secret(value, key)
        .ok_or_else(|| AuthError::json(format!("response has no {key}")))
}

/// Takes an unsigned integer field.
pub(crate) fn take_u64(value: &mut Value, key: &str) -> Option<u64> {
    match value.get_mut(key).map(Value::take) {
        Some(Value::Number(number)) => number.as_u64(),
        _ => None,
    }
}

/// Takes a signed integer field.
pub(crate) fn take_i64(value: &mut Value, key: &str) -> Option<i64> {
    match value.get_mut(key).map(Value::take) {
        Some(Value::Number(number)) => number.as_i64(),
        _ => None,
    }
}

/// Takes a string at a mixed object/array path, for example
/// `["DisplayClaims", "xui", "0", "uhs"]`.
pub(crate) fn take_nested_string(value: &mut Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for segment in path {
        current = match segment.parse::<usize>() {
            Ok(index) => current.get_mut(index)?,
            Err(_) => current.get_mut(*segment)?,
        };
    }
    match current.take() {
        Value::String(text) => Some(text),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_lookup_handles_arrays_and_objects() {
        let mut value = serde_json::json!({
            "DisplayClaims": {"xui": [{"uhs": "user-hash"}]}
        });
        assert_eq!(
            take_nested_string(&mut value, &["DisplayClaims", "xui", "0", "uhs"]).as_deref(),
            Some("user-hash")
        );
        assert!(take_nested_string(&mut value, &["DisplayClaims", "xui", "0", "uhs"]).is_none());
    }

    #[test]
    fn missing_required_fields_are_errors() {
        let mut value = serde_json::json!({});
        assert!(take_required_string(&mut value, "name").is_err());
        assert!(take_required_secret(&mut value, "token").is_err());
    }
}
