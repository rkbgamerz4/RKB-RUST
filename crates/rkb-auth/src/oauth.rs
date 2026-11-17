//! Microsoft OAuth 2.0 device-code flow and token refresh.
//!
//! The device-code flow is used because a Minecraft client must not ask for a
//! password. The user opens the printed verification URL, enters the short
//! code, and the client polls the token endpoint. No password, client secret,
//! or token is ever logged.

use std::fmt;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::endpoints::{Endpoints, OAUTH_SCOPE};
use crate::error::AuthError;
use crate::http::{form_encode, HttpClient, HttpRequest};
use crate::json::{
    take_optional_secret, take_optional_string, take_required_secret, take_required_string,
    take_u64,
};
use crate::secret::SecretString;

/// One successful Microsoft OAuth token response.
pub struct MicrosoftToken {
    access_token: SecretString,
    refresh_token: Option<SecretString>,
    /// Lifetime in seconds reported by Microsoft.
    pub expires_in: u64,
}

impl MicrosoftToken {
    /// The Microsoft access token for Xbox Live authentication.
    #[must_use]
    pub(crate) fn access_token(&self) -> &str {
        self.access_token.expose()
    }

    /// Moves the refresh token out, if Microsoft returned one.
    pub fn take_refresh_token(&mut self) -> Option<SecretString> {
        self.refresh_token.take()
    }

    /// Borrows the refresh token without exposing its value.
    #[must_use]
    pub fn refresh_token(&self) -> Option<&SecretString> {
        self.refresh_token.as_ref()
    }
}

impl fmt::Debug for MicrosoftToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MicrosoftToken")
            .field("access_token", &self.access_token)
            .field("refresh_token", &self.refresh_token)
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

#[cfg(test)]
impl MicrosoftToken {
    /// A token for unit tests that exercise the flow without HTTP.
    pub(crate) fn for_test(access: &str, refresh: Option<&str>, expires_in: u64) -> Self {
        Self {
            access_token: SecretString::new(access),
            refresh_token: refresh.map(SecretString::new),
            expires_in,
        }
    }
}

/// The first half of a device-code login: what the user must see.
pub struct DeviceCodeStart {
    device_code: SecretString,
    /// Short code the user types in the browser.
    pub user_code: String,
    /// URL the user opens.
    pub verification_uri: String,
    /// Lifetime of the code in seconds.
    pub expires_in: u64,
    /// Minimum polling interval in seconds.
    pub interval: u64,
    /// Human-readable instructions from Microsoft.
    pub message: String,
}

impl DeviceCodeStart {
    /// The opaque device code. Crate-private because only the poller needs it.
    #[must_use]
    pub(crate) fn device_code(&self) -> &str {
        self.device_code.expose()
    }
}

impl fmt::Debug for DeviceCodeStart {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceCodeStart")
            .field("device_code", &self.device_code)
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .field("message", &self.message)
            .finish()
    }
}

/// One device-code poll.
#[derive(Debug)]
pub enum PollOutcome {
    /// Microsoft issued tokens.
    Token(MicrosoftToken),
    /// The user has not finished signing in.
    Pending,
    /// The client must increase its polling interval.
    SlowDown,
    /// The device code expired.
    Expired,
    /// The user declined the sign-in.
    Declined { description: String },
}

/// Starts a device-code login.
pub fn begin_device_code(
    client: &dyn HttpClient,
    endpoints: &Endpoints,
    client_id: &str,
) -> Result<DeviceCodeStart, AuthError> {
    validate_client_id(client_id)?;
    let body = form(&[("client_id", client_id), ("scope", OAUTH_SCOPE)]);
    let mut response = client.send(HttpRequest::post_form(&endpoints.device_code, body))?;
    let status = response.status;
    let mut value = response.json()?;
    if status != 200 {
        return Err(oauth_error(status, &mut value));
    }

    let device_code = take_required_secret(&mut value, "device_code")?;
    let user_code = take_required_string(&mut value, "user_code")?;
    let verification_uri = take_required_string(&mut value, "verification_uri")?;
    let expires_in = take_u64(&mut value, "expires_in")
        .ok_or_else(|| AuthError::json("device-code response has no expires_in"))?;
    let interval = take_u64(&mut value, "interval").unwrap_or(5);
    let message = take_optional_string(&mut value, "message").unwrap_or_default();

    Ok(DeviceCodeStart {
        device_code,
        user_code,
        verification_uri,
        expires_in,
        interval,
        message,
    })
}

/// Polls once. A pending response is not an error.
pub fn poll_once(
    client: &dyn HttpClient,
    endpoints: &Endpoints,
    client_id: &str,
    device_code: &str,
) -> Result<PollOutcome, AuthError> {
    validate_client_id(client_id)?;
    let body = form(&[
        ("client_id", client_id),
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ("device_code", device_code),
    ]);
    let mut response = client.send(HttpRequest::post_form(&endpoints.token, body))?;
    let status = response.status;
    let mut value = response.json()?;

    if status == 200 {
        return Ok(PollOutcome::Token(parse_token(&mut value)?));
    }

    let code = take_optional_string(&mut value, "error").unwrap_or_default();
    let description = take_optional_string(&mut value, "error_description").unwrap_or_default();
    Ok(match code.as_str() {
        "authorization_pending" => PollOutcome::Pending,
        "slow_down" => PollOutcome::SlowDown,
        "expired_token" => PollOutcome::Expired,
        "authorization_declined" => PollOutcome::Declined { description },
        other => {
            return Err(AuthError::OAuth {
                code: if other.is_empty() {
                    format!("http_{status}")
                } else {
                    other.to_owned()
                },
                description,
            })
        }
    })
}

/// Waits for the user to finish the device-code login.
///
/// `sleep` is injected so tests do not block; production passes
/// [`thread_sleep`]. The loop still enforces the reported expiry using a real
/// clock.
pub fn wait_for_token<F>(
    client: &dyn HttpClient,
    endpoints: &Endpoints,
    client_id: &str,
    start: &DeviceCodeStart,
    mut sleep: F,
) -> Result<MicrosoftToken, AuthError>
where
    F: FnMut(Duration),
{
    let started = Instant::now();
    let lifetime = Duration::from_secs(start.expires_in.max(1));
    let mut interval = Duration::from_secs(start.interval.max(1));

    loop {
        if started.elapsed() >= lifetime {
            return Err(AuthError::OAuth {
                code: "expired_token".to_owned(),
                description: "the device code expired before sign-in completed".to_owned(),
            });
        }
        sleep(interval);
        match poll_once(client, endpoints, client_id, start.device_code())? {
            PollOutcome::Token(token) => return Ok(token),
            PollOutcome::Pending => {}
            PollOutcome::SlowDown => {
                interval += Duration::from_secs(5);
            }
            PollOutcome::Expired => {
                return Err(AuthError::OAuth {
                    code: "expired_token".to_owned(),
                    description: "the device code expired".to_owned(),
                })
            }
            PollOutcome::Declined { description } => {
                return Err(AuthError::OAuth {
                    code: "authorization_declined".to_owned(),
                    description,
                })
            }
        }
    }
}

/// Exchanges a refresh token for a new Microsoft access token.
///
/// The returned token may carry a rotated refresh token; the caller should
/// replace its stored value when it does.
pub fn refresh_token(
    client: &dyn HttpClient,
    endpoints: &Endpoints,
    client_id: &str,
    refresh_token: &str,
) -> Result<MicrosoftToken, AuthError> {
    validate_client_id(client_id)?;
    if refresh_token.is_empty() {
        return Err(AuthError::config("refresh token is empty"));
    }
    let body = form(&[
        ("client_id", client_id),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("scope", OAUTH_SCOPE),
    ]);
    let mut response = client.send(HttpRequest::post_form(&endpoints.token, body))?;
    let status = response.status;
    let mut value = response.json()?;
    if status != 200 {
        return Err(oauth_error(status, &mut value));
    }
    parse_token(&mut value)
}

/// Blocks the current thread. Injected into [`wait_for_token`].
pub fn thread_sleep(duration: Duration) {
    std::thread::sleep(duration);
}

fn validate_client_id(client_id: &str) -> Result<(), AuthError> {
    if client_id.is_empty() {
        return Err(AuthError::config(
            "an Azure application (client) id is required; RKB does not ship a borrowed id",
        ));
    }
    if client_id.len() > 128 || client_id.chars().any(char::is_whitespace) {
        return Err(AuthError::config(
            "the client id is not a valid application id",
        ));
    }
    Ok(())
}

fn form(pairs: &[(&str, &str)]) -> String {
    let mut body = String::new();
    for (index, (name, value)) in pairs.iter().enumerate() {
        if index > 0 {
            body.push('&');
        }
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    body
}

fn parse_token(value: &mut Value) -> Result<MicrosoftToken, AuthError> {
    let access_token = take_required_secret(value, "access_token")?;
    let refresh_token = take_optional_secret(value, "refresh_token");
    let expires_in = take_u64(value, "expires_in").unwrap_or(0);
    Ok(MicrosoftToken {
        access_token,
        refresh_token,
        expires_in,
    })
}

fn oauth_error(status: u16, value: &mut Value) -> AuthError {
    let code = take_optional_string(value, "error")
        .filter(|code| !code.is_empty())
        .unwrap_or_else(|| format!("http_{status}"));
    let description = take_optional_string(value, "error_description").unwrap_or_default();
    AuthError::OAuth { code, description }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::HttpResponse;
    use crate::test_support::MockClient;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn json_response(status: u16, body: &str) -> HttpResponse {
        HttpResponse::new(status, body.as_bytes().to_vec())
    }

    #[test]
    fn device_code_request_uses_form_encoding_and_redacts_debug() {
        let client = MockClient::new(vec![json_response(
            200,
            r#"{"device_code":"device-code-secret","user_code":"ABCD","verification_uri":"https://example/device","expires_in":900,"interval":5,"message":"go"}"#,
        )]);
        let start = begin_device_code(&client, &Endpoints::default(), "client-id").unwrap();
        assert_eq!(start.user_code, "ABCD");
        assert_eq!(start.verification_uri, "https://example/device");
        assert_eq!(start.interval, 5);
        assert!(!format!("{start:?}").contains("device-code-secret"));

        let requests = client.take_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, crate::http::Method::Post);
        let body = String::from_utf8(requests[0].body.clone()).unwrap();
        assert!(body.contains("client_id=client-id"));
        assert!(body.contains("scope=XboxLive.signin%20offline_access"));
        assert!(!body.contains("secret"));
    }

    #[test]
    fn polling_classifies_pending_slow_down_expiry_and_decline() {
        let pending = MockClient::new(vec![json_response(
            400,
            r#"{"error":"authorization_pending","error_description":"wait"}"#,
        )]);
        assert!(matches!(
            poll_once(&pending, &Endpoints::default(), "id", "dev").unwrap(),
            PollOutcome::Pending
        ));

        let slow = MockClient::new(vec![json_response(400, r#"{"error":"slow_down"}"#)]);
        assert!(matches!(
            poll_once(&slow, &Endpoints::default(), "id", "dev").unwrap(),
            PollOutcome::SlowDown
        ));

        let expired = MockClient::new(vec![json_response(400, r#"{"error":"expired_token"}"#)]);
        assert!(matches!(
            poll_once(&expired, &Endpoints::default(), "id", "dev").unwrap(),
            PollOutcome::Expired
        ));

        let declined = MockClient::new(vec![json_response(
            400,
            r#"{"error":"authorization_declined","error_description":"no"}"#,
        )]);
        assert!(matches!(
            poll_once(&declined, &Endpoints::default(), "id", "dev").unwrap(),
            PollOutcome::Declined { .. }
        ));
    }

    #[test]
    fn wait_polls_until_a_token_arrives() {
        let client = MockClient::new(vec![
            json_response(400, r#"{"error":"authorization_pending"}"#),
            json_response(400, r#"{"error":"slow_down"}"#),
            json_response(
                200,
                r#"{"access_token":"access","refresh_token":"refresh","expires_in":3600}"#,
            ),
        ]);
        let start = DeviceCodeStart {
            device_code: SecretString::new("dev"),
            user_code: "ABCD".to_owned(),
            verification_uri: "https://example/device".to_owned(),
            expires_in: 900,
            interval: 1,
            message: String::new(),
        };
        let sleeps = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&sleeps);
        let token = wait_for_token(&client, &Endpoints::default(), "id", &start, move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        assert_eq!(token.expires_in, 3600);
        assert_eq!(token.access_token(), "access");
        assert!(token.refresh_token().is_some());
        assert_eq!(sleeps.load(Ordering::SeqCst), 3);
        assert_eq!(client.take_requests().len(), 3);
    }

    #[test]
    fn token_debug_is_redacted() {
        let mut value = serde_json::json!({
            "access_token": "access-value",
            "refresh_token": "refresh-value",
            "expires_in": 1
        });
        let token = parse_token(&mut value).unwrap();
        let text = format!("{token:?}");
        assert!(!text.contains("access-value"));
        assert!(!text.contains("refresh-value"));
        assert_eq!(token.access_token(), "access-value");
    }
}
