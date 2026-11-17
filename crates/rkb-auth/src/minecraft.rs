//! Minecraft Services: Xbox login exchange, profile, and entitlements.
//!
//! These calls prove that the signed-in account owns Minecraft: Java Edition
//! before a server join is attempted. A profile alone is not enough; the
//! entitlement store is checked as well.

use std::fmt;

use serde_json::json;
use zeroize::Zeroize;

use crate::endpoints::Endpoints;
use crate::error::AuthError;
#[cfg(test)]
use crate::http::HttpResponse;
use crate::http::{HttpClient, HttpRequest};
use crate::json::{take_optional_string, take_required_secret, take_required_string, take_u64};
use crate::secret::{SecretString, ZeroizingJson};

/// A Minecraft Services access token (not the Microsoft OAuth token).
pub struct MinecraftToken {
    access_token: SecretString,
    /// Lifetime in seconds reported by Minecraft Services.
    pub expires_in: u64,
}

impl MinecraftToken {
    /// The bearer token for profile, entitlements, and session-server calls.
    #[must_use]
    pub(crate) fn access_token(&self) -> &str {
        self.access_token.expose()
    }
}

impl fmt::Debug for MinecraftToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MinecraftToken")
            .field("access_token", &self.access_token)
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

/// The Java Edition profile: canonical UUID and name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MinecraftProfile {
    pub uuid: u128,
    pub name: String,
}

/// The entitlement items returned by Minecraft Services.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entitlements {
    pub items: Vec<String>,
}

/// Exchanges the Xbox identity token for a Minecraft Services token.
pub fn login_with_xbox(
    client: &dyn HttpClient,
    endpoints: &Endpoints,
    user_hash: &str,
    xsts_token: &str,
) -> Result<MinecraftToken, AuthError> {
    if user_hash.is_empty() || xsts_token.is_empty() {
        return Err(AuthError::config(
            "Xbox user hash and XSTS token are both required",
        ));
    }
    let mut identity = format!("XBL3.0 x={user_hash};{xsts_token}");
    let value = ZeroizingJson::new(json!({ "identityToken": identity.as_str() }));
    identity.zeroize();
    let body = value
        .serialize()
        .map_err(|error| AuthError::json(format!("could not encode Minecraft login: {error}")))?;
    let mut response = client.send(HttpRequest::post_json(&endpoints.minecraft_login, body))?;
    let status = response.status;
    let mut value = response.json()?;
    if status != 200 {
        return Err(service_error(status, &mut value));
    }
    let access_token = take_required_secret(&mut value, "access_token")?;
    let expires_in = take_u64(&mut value, "expires_in").unwrap_or(0);
    Ok(MinecraftToken {
        access_token,
        expires_in,
    })
}

/// Fetches the Java profile for an access token.
pub fn fetch_profile(
    client: &dyn HttpClient,
    endpoints: &Endpoints,
    access_token: &str,
) -> Result<MinecraftProfile, AuthError> {
    let mut response =
        client.send(HttpRequest::get(&endpoints.minecraft_profile).bearer(access_token))?;
    let status = response.status;
    if status == 404 {
        return Err(AuthError::NoProfile);
    }
    let mut value = response.json()?;
    if status != 200 {
        return Err(service_error(status, &mut value));
    }
    let id = take_required_string(&mut value, "id")?;
    let uuid = parse_uuid_hex(&id)?;
    let name = take_required_string(&mut value, "name")?;
    if name.is_empty() || name.encode_utf16().count() > 16 {
        return Err(AuthError::json(
            "profile name is empty or longer than 16 units",
        ));
    }
    Ok(MinecraftProfile { uuid, name })
}

/// Fetches the entitlement store for an access token.
pub fn fetch_entitlements(
    client: &dyn HttpClient,
    endpoints: &Endpoints,
    access_token: &str,
) -> Result<Entitlements, AuthError> {
    let mut response =
        client.send(HttpRequest::get(&endpoints.minecraft_entitlements).bearer(access_token))?;
    let status = response.status;
    let mut value = response.json()?;
    if status != 200 {
        return Err(service_error(status, &mut value));
    }
    let items = value
        .get_mut("items")
        .map(serde_json::Value::take)
        .and_then(|items| match items {
            serde_json::Value::Array(items) => Some(items),
            _ => None,
        })
        .ok_or_else(|| AuthError::json("entitlements response has no items array"))?;

    let mut names = Vec::with_capacity(items.len());
    for mut item in items {
        if let Some(name) = take_optional_string(&mut item, "name") {
            names.push(name);
        }
    }
    Ok(Entitlements { items: names })
}

/// Verifies that the entitlement list contains a Java Edition product.
pub fn verify_ownership(entitlements: &Entitlements) -> Result<(), AuthError> {
    let owned = entitlements
        .items
        .iter()
        .any(|item| item == "game_minecraft" || item == "product_minecraft");
    if owned {
        Ok(())
    } else {
        Err(AuthError::NotOwned)
    }
}

/// Parses a 32-hex-digit UUID, accepting hyphens for convenience.
pub fn parse_uuid_hex(text: &str) -> Result<u128, AuthError> {
    let cleaned: String = text.chars().filter(|character| *character != '-').collect();
    if cleaned.len() != 32
        || !cleaned
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(AuthError::json("profile id is not a 32-hex-digit UUID"));
    }
    u128::from_str_radix(&cleaned, 16)
        .map_err(|error| AuthError::json(format!("invalid profile UUID: {error}")))
}

/// Formats a UUID the way the session server expects: 32 lowercase hex digits.
#[must_use]
pub fn uuid_hex(uuid: u128) -> String {
    format!("{uuid:032x}")
}

fn service_error(status: u16, value: &mut serde_json::Value) -> AuthError {
    let message = take_optional_string(value, "errorMessage")
        .or_else(|| take_optional_string(value, "error"))
        .or_else(|| take_optional_string(value, "Message"))
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| format!("HTTP {status}"));
    AuthError::http(status, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::MockClient;

    fn response(status: u16, body: &str) -> HttpResponse {
        HttpResponse::new(status, body.as_bytes().to_vec())
    }

    #[test]
    fn login_request_uses_the_xbox_identity_token() {
        let client = MockClient::new(vec![response(
            200,
            r#"{"username":"id","access_token":"mc-token","expires_in":86400}"#,
        )]);
        let token = login_with_xbox(&client, &Endpoints::default(), "uhs", "xsts").unwrap();
        assert_eq!(token.access_token(), "mc-token");
        assert_eq!(token.expires_in, 86400);
        assert!(!format!("{token:?}").contains("mc-token"));

        let requests = client.take_requests();
        assert_eq!(requests.len(), 1);
        let body = requests[0].body_text().unwrap();
        assert!(body.contains(r#""identityToken":"XBL3.0 x=uhs;xsts""#));
    }

    #[test]
    fn profile_request_uses_bearer_and_parses_uuid() {
        let client = MockClient::new(vec![response(
            200,
            r#"{"id":"b50ad385829d3141a2167e7d7539ba7f","name":"Notch"}"#,
        )]);
        let profile = fetch_profile(&client, &Endpoints::default(), "mc-token").unwrap();
        assert_eq!(uuid_hex(profile.uuid), "b50ad385829d3141a2167e7d7539ba7f");
        assert_eq!(profile.name, "Notch");

        let requests = client.take_requests();
        assert_eq!(requests[0].header("Authorization"), Some("Bearer mc-token"));
    }

    #[test]
    fn missing_profile_is_reported_as_not_owned() {
        let client = MockClient::new(vec![response(404, r#"{"error":"NOT_FOUND"}"#)]);
        assert_eq!(
            fetch_profile(&client, &Endpoints::default(), "token").unwrap_err(),
            AuthError::NoProfile
        );
    }

    #[test]
    fn entitlements_are_parsed_and_checked() {
        let client = MockClient::new(vec![response(
            200,
            r#"{"items":[{"name":"product_minecraft"},{"name":"game_minecraft"}]}"#,
        )]);
        let entitlements = fetch_entitlements(&client, &Endpoints::default(), "mc-token").unwrap();
        assert_eq!(
            entitlements.items,
            vec!["product_minecraft", "game_minecraft"]
        );
        assert!(verify_ownership(&entitlements).is_ok());
    }

    #[test]
    fn an_account_without_java_is_rejected() {
        let entitlements = Entitlements {
            items: vec!["product_other".to_owned()],
        };
        assert_eq!(verify_ownership(&entitlements), Err(AuthError::NotOwned));
    }

    #[test]
    fn uuid_parsing_is_strict() {
        assert!(parse_uuid_hex("b50ad385-829d-3141-a216-7e7d7539ba7f").is_ok());
        assert!(parse_uuid_hex("b50ad385829d3141a2167e7d7539ba7").is_err());
        assert!(parse_uuid_hex("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz").is_err());
        assert_eq!(
            uuid_hex(0xB50A_D385_829D_3141_A216_7E7D_7539_BA7F),
            "b50ad385829d3141a2167e7d7539ba7f"
        );
    }

    #[test]
    fn service_errors_use_the_message_field_not_the_body() {
        let client = MockClient::new(vec![response(
            401,
            r#"{"path":"/minecraft/profile","errorMessage":"Invalid token","access_token":"leak-me"}"#,
        )]);
        let error = fetch_profile(&client, &Endpoints::default(), "token").unwrap_err();
        let text = error.to_string();
        assert!(text.contains("Invalid token"));
        assert!(!text.contains("leak-me"));
    }
}
