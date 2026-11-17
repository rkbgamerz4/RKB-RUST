//! A tiny blocking HTTP seam.
//!
//! Authentication code depends on [`HttpClient`], not on a particular TLS
//! stack, so every protocol exchange is testable against a loopback server and
//! a mock client. [`UreqClient`] is the production implementation: ureq with
//! rustls and bundled web PKI roots, which works on Android ARM64 without an
//! operating-system certificate store.
//!
//! Request and response bodies are zeroed on drop because they can carry OAuth
//! and Minecraft access tokens. `Debug` deliberately prints neither headers nor
//! bodies.

use std::fmt;
use std::time::Duration;

use zeroize::Zeroize;

use crate::error::AuthError;
use crate::secret::ZeroizingJson;

/// The two methods the authentication endpoints use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// One HTTP request. Constructed by the protocol modules, never by application
/// code.
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    /// A JSON `POST`.
    pub fn post_json(url: impl Into<String>, body: String) -> Self {
        Self {
            method: Method::Post,
            url: url.into(),
            headers: vec![
                ("Content-Type".to_owned(), "application/json".to_owned()),
                ("Accept".to_owned(), "application/json".to_owned()),
            ],
            body: body.into_bytes(),
        }
    }

    /// A form-encoded `POST`.
    pub fn post_form(url: impl Into<String>, body: String) -> Self {
        Self {
            method: Method::Post,
            url: url.into(),
            headers: vec![
                (
                    "Content-Type".to_owned(),
                    "application/x-www-form-urlencoded".to_owned(),
                ),
                ("Accept".to_owned(), "application/json".to_owned()),
            ],
            body: body.into_bytes(),
        }
    }

    /// A `GET` with a JSON `Accept`.
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: Method::Get,
            url: url.into(),
            headers: vec![("Accept".to_owned(), "application/json".to_owned())],
            body: Vec::new(),
        }
    }

    /// Adds an `Authorization: Bearer` header.
    #[must_use]
    pub fn bearer(mut self, token: &str) -> Self {
        self.headers
            .push(("Authorization".to_owned(), format!("Bearer {token}")));
        self
    }

    /// Adds a header.
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

impl Drop for HttpRequest {
    fn drop(&mut self) {
        self.body.zeroize();
    }
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Header values and the body can contain bearer tokens. List names and
        // sizes only.
        let names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        formatter
            .debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("header_names", &names)
            .field("body_len", &self.body.len())
            .finish()
    }
}

/// One HTTP response. The body is kept as bytes so protocol modules can parse
/// and then zero it.
pub struct HttpResponse {
    pub status: u16,
    body: Vec<u8>,
}

impl HttpResponse {
    /// Builds a response, for tests and mock clients.
    #[must_use]
    pub fn new(status: u16, body: Vec<u8>) -> Self {
        Self { status, body }
    }

    /// The body length without exposing the bytes.
    #[must_use]
    pub fn body_len(&self) -> usize {
        self.body.len()
    }

    /// Moves the body out and leaves an empty buffer behind. The caller must
    /// zero it after parsing when it can contain a secret.
    pub fn take_body(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.body)
    }

    /// Parses the body as JSON and zeroes the raw bytes.
    pub(crate) fn json(&mut self) -> Result<ZeroizingJson, AuthError> {
        let mut body = self.take_body();
        ZeroizingJson::parse(&mut body)
            .map_err(|error| AuthError::json(format!("invalid JSON response: {error}")))
    }
}

impl Drop for HttpResponse {
    fn drop(&mut self) {
        self.body.zeroize();
    }
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("body_len", &self.body.len())
            .finish()
    }
}

/// A blocking HTTP client.
pub trait HttpClient {
    /// Sends one request and returns the status and body.
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, AuthError>;
}

/// The production client: blocking ureq with rustls and web PKI roots.
pub struct UreqClient {
    agent: ureq::Agent,
}

impl UreqClient {
    /// Creates a client with one global timeout.
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .user_agent(concat!("RKB-RUST/", env!("CARGO_PKG_VERSION")))
            .build();
        Self {
            agent: config.into(),
        }
    }
}

impl Default for UreqClient {
    fn default() -> Self {
        Self::new(Duration::from_secs(20))
    }
}

impl fmt::Debug for UreqClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("UreqClient").finish_non_exhaustive()
    }
}

impl HttpClient for UreqClient {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, AuthError> {
        let result = match request.method {
            Method::Get => {
                let mut builder = self.agent.get(&request.url);
                for (name, value) in &request.headers {
                    builder = builder.header(name.as_str(), value.as_str());
                }
                builder.call()
            }
            Method::Post => {
                let mut builder = self.agent.post(&request.url);
                for (name, value) in &request.headers {
                    builder = builder.header(name.as_str(), value.as_str());
                }
                builder.send(request.body.as_slice())
            }
        };
        let response = result.map_err(|error| AuthError::transport(error.to_string()))?;
        let status = response.status().as_u16();
        let mut response = response;
        let body = response
            .body_mut()
            .read_to_vec()
            .map_err(|error| AuthError::transport(error.to_string()))?;
        Ok(HttpResponse::new(status, body))
    }
}

/// Percent-encode a form value. The authentication scopes contain a space, so
/// this cannot be skipped.
#[must_use]
pub fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_encoding_escapes_scopes_and_keeps_unreserved_bytes() {
        assert_eq!(
            form_encode("XboxLive.signin offline_access"),
            "XboxLive.signin%20offline_access"
        );
        assert_eq!(form_encode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(form_encode("a+b=&"), "a%2Bb%3D%26");
    }

    #[test]
    fn debug_redacts_header_values_and_body() {
        let request = HttpRequest::get("https://example.invalid")
            .bearer("top-secret")
            .header("X-Note", "also-secret");
        let text = format!("{request:?}");
        assert!(!text.contains("top-secret"));
        assert!(!text.contains("also-secret"));
        assert!(text.contains("Authorization"));
        assert!(text.contains("body_len"));
    }
}
