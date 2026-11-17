//! Minecraft profile session and the session-server join.
//!
//! [`MinecraftSession`] is authenticated account data. [`OnlineAuthenticator`]
//! owns the HTTP client and implements `rkb-net`'s `SessionAuthenticator`, so
//! the M1 login state machine can complete an online-mode login without
//! knowing anything about Microsoft authentication.

use std::fmt;

use serde_json::json;

use crate::endpoints::Endpoints;
use crate::error::AuthError;
use crate::http::{HttpClient, HttpRequest, HttpResponse};
use crate::json::take_optional_string;
use crate::minecraft::{self, Entitlements, MinecraftProfile, MinecraftToken};
use crate::secret::{SecretString, ZeroizingJson};
use crate::xbox;

/// A signed-in Java Edition profile with a Minecraft Services token.
pub struct MinecraftSession {
    access_token: SecretString,
    profile: MinecraftProfile,
    entitlements: Entitlements,
}

impl MinecraftSession {
    /// Runs Xbox Live, XSTS, Minecraft login, profile, and entitlement checks.
    pub fn authenticate(
        client: &dyn HttpClient,
        endpoints: &Endpoints,
        microsoft_access_token: &str,
    ) -> Result<Self, AuthError> {
        let xbox = xbox::authenticate(client, endpoints, microsoft_access_token)?;
        let token =
            minecraft::login_with_xbox(client, endpoints, &xbox.user_hash, xbox.xsts_token())?;
        Self::from_token(client, endpoints, &token)
    }

    /// Reuses an existing Minecraft Services token, re-fetching the profile and
    /// entitlements so ownership is always checked for the current account.
    pub fn from_token(
        client: &dyn HttpClient,
        endpoints: &Endpoints,
        token: &MinecraftToken,
    ) -> Result<Self, AuthError> {
        let profile = minecraft::fetch_profile(client, endpoints, token.access_token())?;
        let entitlements = minecraft::fetch_entitlements(client, endpoints, token.access_token())?;
        minecraft::verify_ownership(&entitlements)?;
        Ok(Self {
            access_token: SecretString::new(token.access_token()),
            profile,
            entitlements,
        })
    }

    /// The canonical Java profile UUID.
    #[must_use]
    pub fn uuid(&self) -> u128 {
        self.profile.uuid
    }

    /// The canonical Java profile name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.profile.name
    }

    /// The entitlement items observed for this account.
    #[must_use]
    pub fn entitlements(&self) -> &Entitlements {
        &self.entitlements
    }

    /// Tells the session server that this client is joining `server_hash`.
    ///
    /// `server_hash` is the Minecraft server hash, not a secret. The access
    /// token is placed in the request body and never in a URL.
    pub fn join(
        &self,
        client: &dyn HttpClient,
        endpoints: &Endpoints,
        server_hash: &str,
    ) -> Result<(), AuthError> {
        if server_hash.is_empty() {
            return Err(AuthError::config("the server hash is empty"));
        }
        let value = ZeroizingJson::new(json!({
            "accessToken": self.access_token.expose(),
            "selectedProfile": minecraft::uuid_hex(self.profile.uuid),
            "serverId": server_hash,
        }));
        let body = value
            .serialize()
            .map_err(|error| AuthError::json(format!("could not encode join request: {error}")))?;
        let mut response = client.send(HttpRequest::post_json(&endpoints.session_join, body))?;
        let status = response.status;
        if (200..300).contains(&status) {
            return Ok(());
        }
        Err(join_error(status, &mut response))
    }
}

impl fmt::Debug for MinecraftSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MinecraftSession")
            .field("access_token", &self.access_token)
            .field("profile", &self.profile)
            .field("entitlement_count", &self.entitlements.items.len())
            .finish()
    }
}

/// Owns the HTTP client and session so it can implement the M1 authenticator
/// seam for an online-mode login.
pub struct OnlineAuthenticator {
    client: Box<dyn HttpClient>,
    endpoints: Endpoints,
    session: MinecraftSession,
}

impl OnlineAuthenticator {
    /// Authenticates and keeps the client for the later join call.
    pub fn authenticate(
        client: Box<dyn HttpClient>,
        endpoints: Endpoints,
        microsoft_access_token: &str,
    ) -> Result<Self, AuthError> {
        let session =
            MinecraftSession::authenticate(client.as_ref(), &endpoints, microsoft_access_token)?;
        Ok(Self {
            client,
            endpoints,
            session,
        })
    }

    /// Wraps an already authenticated session.
    pub fn from_session(
        client: Box<dyn HttpClient>,
        endpoints: Endpoints,
        session: MinecraftSession,
    ) -> Self {
        Self {
            client,
            endpoints,
            session,
        }
    }

    /// The authenticated profile.
    #[must_use]
    pub fn session(&self) -> &MinecraftSession {
        &self.session
    }
}

impl fmt::Debug for OnlineAuthenticator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OnlineAuthenticator")
            .field("session", &self.session)
            .field("endpoints", &self.endpoints)
            .finish_non_exhaustive()
    }
}

impl rkb_net::SessionAuthenticator for OnlineAuthenticator {
    fn join_server(&mut self, server_hash: &str) -> rkb_proto::error::Result<()> {
        self.session
            .join(self.client.as_ref(), &self.endpoints, server_hash)
            .map_err(|error| rkb_proto::error::Error::protocol(error.to_string()))
    }
}

fn join_error(status: u16, response: &mut HttpResponse) -> AuthError {
    if response.body_len() == 0 {
        return AuthError::JoinRejected {
            status,
            message: "the session server returned an empty response".to_owned(),
        };
    }
    let mut value = match response.json() {
        Ok(value) => value,
        Err(_) => {
            return AuthError::JoinRejected {
                status,
                message: "the session server returned an unreadable response".to_owned(),
            }
        }
    };
    let message = take_optional_string(&mut value, "error")
        .or_else(|| take_optional_string(&mut value, "errorMessage"))
        .or_else(|| take_optional_string(&mut value, "path"))
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| format!("HTTP {status}"));
    AuthError::JoinRejected { status, message }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::MockClient;
    use rkb_net::SessionAuthenticator;

    fn response(status: u16, body: &str) -> HttpResponse {
        HttpResponse::new(status, body.as_bytes().to_vec())
    }

    fn account_responses() -> Vec<HttpResponse> {
        vec![
            response(
                200,
                r#"{"Token":"user-token","DisplayClaims":{"xui":[{"uhs":"uhs"}]}}"#,
            ),
            response(
                200,
                r#"{"Token":"xsts-token","DisplayClaims":{"xui":[{"uhs":"uhs"}]}}"#,
            ),
            response(200, r#"{"access_token":"mc-token","expires_in":86400}"#),
            response(
                200,
                r#"{"id":"b50ad385829d3141a2167e7d7539ba7f","name":"Notch"}"#,
            ),
            response(
                200,
                r#"{"items":[{"name":"game_minecraft"},{"name":"product_minecraft"}]}"#,
            ),
        ]
    }

    #[test]
    fn full_account_flow_checks_profile_and_ownership() {
        let client = MockClient::new(account_responses());
        let session =
            MinecraftSession::authenticate(&client, &Endpoints::default(), "ms-token").unwrap();
        assert_eq!(session.name(), "Notch");
        assert_eq!(
            minecraft::uuid_hex(session.uuid()),
            "b50ad385829d3141a2167e7d7539ba7f"
        );
        assert_eq!(session.entitlements().items.len(), 2);
        assert!(!format!("{session:?}").contains("mc-token"));
        assert_eq!(client.take_requests().len(), 5);
    }

    #[test]
    fn join_sends_profile_and_server_hash_without_putting_tokens_in_the_url() {
        let client = MockClient::new(vec![response(204, "")]);
        let session = MinecraftSession {
            access_token: SecretString::new("mc-token"),
            profile: MinecraftProfile {
                uuid: 0xB50A_D385_829D_3141_A216_7E7D_7539_BA7F,
                name: "Notch".to_owned(),
            },
            entitlements: Entitlements {
                items: vec!["game_minecraft".to_owned()],
            },
        };
        let endpoints = Endpoints::default();
        session.join(&client, &endpoints, "-4b0bb9f5").unwrap();

        let requests = client.take_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].url, endpoints.session_join);
        assert!(!requests[0].url.contains("mc-token"));
        let body = requests[0].body_text().unwrap();
        assert!(body.contains(r#""accessToken":"mc-token""#));
        assert!(body.contains(r#""selectedProfile":"b50ad385829d3141a2167e7d7539ba7f""#));
        assert!(body.contains(r#""serverId":"-4b0bb9f5""#));
    }

    #[test]
    fn join_rejection_reports_the_error_but_not_the_token() {
        let client = MockClient::new(vec![response(
            403,
            r#"{"error":"ForbiddenOperationException","path":"/session/minecraft/join","accessToken":"leak"}"#,
        )]);
        let session = MinecraftSession {
            access_token: SecretString::new("mc-token"),
            profile: MinecraftProfile {
                uuid: 1,
                name: "P".to_owned(),
            },
            entitlements: Entitlements::default(),
        };
        let error = session
            .join(&client, &Endpoints::default(), "hash")
            .unwrap_err();
        let text = error.to_string();
        assert!(text.contains("ForbiddenOperationException"));
        assert!(!text.contains("leak"));
        assert!(!text.contains("mc-token"));
    }

    #[test]
    fn online_authenticator_implements_the_m1_seam() {
        let client = MockClient::new({
            let mut responses = account_responses();
            responses.push(response(204, ""));
            responses
        });
        let mut authenticator =
            OnlineAuthenticator::authenticate(Box::new(client), Endpoints::default(), "ms-token")
                .unwrap();
        SessionAuthenticator::join_server(&mut authenticator, "server-hash").unwrap();
        assert_eq!(authenticator.session().name(), "Notch");
    }
}
