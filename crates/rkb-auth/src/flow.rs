//! High-level sign-in: device code or refresh token to a Minecraft session.
//!
//! The flow is:
//!
//! ```text
//! device code → Microsoft OAuth token → Xbox Live → XSTS
//!   → Minecraft Services token → profile → entitlements
//! ```
//!
//! A refresh token replaces only the first step. Profile and entitlement
//! verification always run, so a stale or reassigned account cannot reuse an
//! old session without being checked again.
//!
//! Neither function prints anything and no type here implements `Display`;
//! presentation belongs to the caller, which can then keep tokens off logs.

use std::fmt;
use std::time::Duration;

use crate::endpoints::Endpoints;
use crate::error::AuthError;
use crate::http::HttpClient;
use crate::oauth::{self, DeviceCodeStart, MicrosoftToken};
use crate::secret::SecretString;
use crate::session::MinecraftSession;
use crate::store::{StoredSession, TokenStore};

/// The Azure application id and service endpoints used by a sign-in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthConfig {
    /// Azure application (client) id. RKB does not ship one.
    pub client_id: String,
    /// Microsoft, Xbox, Minecraft, and session-server endpoints.
    pub endpoints: Endpoints,
}

impl AuthConfig {
    /// A configuration with the production endpoints.
    #[must_use]
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            endpoints: Endpoints::default(),
        }
    }

    /// Replaces the endpoints, for tests or a controlled proxy.
    #[must_use]
    pub fn with_endpoints(mut self, endpoints: Endpoints) -> Self {
        self.endpoints = endpoints;
        self
    }
}

/// A completed sign-in.
pub struct SignedIn {
    /// The verified Java profile and its Minecraft Services token.
    pub session: MinecraftSession,
    /// The refresh token to persist for the next start. `None` only if
    /// Microsoft did not issue one.
    pub refresh_token: Option<SecretString>,
}

impl SignedIn {
    /// Saves the refresh token and profile identity to `store`.
    ///
    /// This fails rather than silently skipping persistence when Microsoft did
    /// not issue a refresh token.
    pub fn save(&self, store: &dyn TokenStore) -> Result<(), AuthError> {
        let refresh_token = self
            .refresh_token
            .as_ref()
            .ok_or_else(|| AuthError::config("Microsoft did not issue a refresh token to store"))?;
        store.save(&StoredSession::new(
            SecretString::new(refresh_token.expose()),
            self.session.uuid(),
            self.session.name(),
        ))
    }
}

impl fmt::Debug for SignedIn {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignedIn")
            .field("session", &self.session)
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// Starts the device-code flow. Show [`DeviceCodeStart::user_code`] and
/// [`DeviceCodeStart::verification_uri`] to the user, then call
/// [`finish_device_code`].
pub fn begin_device_code(
    client: &dyn HttpClient,
    config: &AuthConfig,
) -> Result<DeviceCodeStart, AuthError> {
    oauth::begin_device_code(client, &config.endpoints, &config.client_id)
}

/// Waits for the user to approve the device code and completes every
/// authentication step.
pub fn finish_device_code<F>(
    client: &dyn HttpClient,
    config: &AuthConfig,
    start: &DeviceCodeStart,
    sleep: F,
) -> Result<SignedIn, AuthError>
where
    F: FnMut(Duration),
{
    let token = oauth::wait_for_token(client, &config.endpoints, &config.client_id, start, sleep)?;
    complete(client, config, token, None)
}

/// Completes a sign-in from an already obtained Microsoft access token.
pub fn complete(
    client: &dyn HttpClient,
    config: &AuthConfig,
    token: MicrosoftToken,
    fallback_refresh_token: Option<&SecretString>,
) -> Result<SignedIn, AuthError> {
    let mut token = token;
    let session = MinecraftSession::authenticate(client, &config.endpoints, token.access_token())?;
    let refresh_token = token
        .take_refresh_token()
        .or_else(|| fallback_refresh_token.map(|token| SecretString::new(token.expose())));
    Ok(SignedIn {
        session,
        refresh_token,
    })
}

/// Renews a session from a stored refresh token.
///
/// Microsoft may rotate the refresh token. When it does, the returned
/// [`SignedIn`] carries the new value; when it does not, the supplied token is
/// returned so the caller can keep using it.
pub fn refresh(
    client: &dyn HttpClient,
    config: &AuthConfig,
    refresh_token: &SecretString,
) -> Result<SignedIn, AuthError> {
    let token = oauth::refresh_token(
        client,
        &config.endpoints,
        &config.client_id,
        refresh_token.expose(),
    )?;
    complete(client, config, token, Some(refresh_token))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::HttpResponse;
    use crate::test_support::MockClient;

    fn response(status: u16, body: &str) -> HttpResponse {
        HttpResponse::new(status, body.as_bytes().to_vec())
    }

    fn session_responses() -> Vec<HttpResponse> {
        vec![
            response(
                200,
                r#"{"Token":"user","DisplayClaims":{"xui":[{"uhs":"uhs"}]}}"#,
            ),
            response(
                200,
                r#"{"Token":"xsts","DisplayClaims":{"xui":[{"uhs":"uhs"}]}}"#,
            ),
            response(200, r#"{"access_token":"mc","expires_in":86400}"#),
            response(
                200,
                r#"{"id":"b50ad385829d3141a2167e7d7539ba7f","name":"Notch"}"#,
            ),
            response(200, r#"{"items":[{"name":"game_minecraft"}]}"#),
        ]
    }

    #[test]
    fn device_code_flow_returns_a_verified_session_and_refresh_token() {
        let client = MockClient::new({
            let mut responses = vec![response(
                200,
                r#"{"device_code":"dev","user_code":"ABCD","verification_uri":"https://example/device","expires_in":900,"interval":1}"#,
            )];
            responses.push(response(
                200,
                r#"{"access_token":"ms","refresh_token":"refresh-token-value","expires_in":3600}"#,
            ));
            responses.extend(session_responses());
            responses
        });
        let config = AuthConfig::new("client-id");
        let start = begin_device_code(&client, &config).unwrap();
        let signed = finish_device_code(&client, &config, &start, |_| {}).unwrap();
        assert_eq!(signed.session.name(), "Notch");
        assert_eq!(
            signed.refresh_token.as_ref().unwrap().expose(),
            "refresh-token-value"
        );
        assert!(!format!("{signed:?}").contains("refresh-token-value"));
        assert!(!format!("{signed:?}").contains("\"mc\""));
    }

    #[test]
    fn refresh_keeps_the_old_token_when_microsoft_does_not_rotate() {
        let client = MockClient::new({
            let mut responses = vec![response(200, r#"{"access_token":"ms","expires_in":3600}"#)];
            responses.extend(session_responses());
            responses
        });
        let config = AuthConfig::new("client-id");
        let old = SecretString::new("old-refresh");
        let signed = refresh(&client, &config, &old).unwrap();
        assert_eq!(
            signed.refresh_token.as_ref().unwrap().expose(),
            "old-refresh"
        );
    }

    #[test]
    fn signed_in_saves_only_the_refresh_token_and_profile() {
        let client = MockClient::new(session_responses());
        let config = AuthConfig::new("client-id");
        let signed = complete(
            &client,
            &config,
            MicrosoftToken::for_test("ms", Some("refresh"), 3600),
            None,
        )
        .unwrap();
        let store = crate::store::MemoryTokenStore::new();
        signed.save(&store).unwrap();
        let stored = store.load().unwrap().unwrap();
        assert_eq!(stored.refresh_token().expose(), "refresh");
        assert_eq!(stored.name(), "Notch");
    }

    #[test]
    fn saving_without_a_refresh_token_is_an_error() {
        let client = MockClient::new(session_responses());
        let config = AuthConfig::new("client-id");
        let signed = complete(
            &client,
            &config,
            MicrosoftToken::for_test("ms", None, 3600),
            None,
        )
        .unwrap();
        let store = crate::store::MemoryTokenStore::new();
        assert!(signed.save(&store).is_err());
    }
}
