//! Test-only HTTP client that records requests and replays responses.

use std::collections::VecDeque;
use std::sync::Mutex;

use zeroize::Zeroize;

use crate::error::AuthError;
use crate::http::{HttpClient, HttpRequest, HttpResponse, Method};

/// A request snapshot safe to inspect in a test. Bodies are zeroed on drop.
pub struct RecordedRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RecordedRequest {
    /// Case-insensitive header lookup.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The body as UTF-8, for test assertions.
    #[must_use]
    pub fn body_text(&self) -> Option<&str> {
        std::str::from_utf8(&self.body).ok()
    }
}

impl Drop for RecordedRequest {
    fn drop(&mut self) {
        self.body.zeroize();
    }
}

impl std::fmt::Debug for RecordedRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RecordedRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("header_names", &self.headers.len())
            .field("body_len", &self.body.len())
            .finish()
    }
}

/// A queued-response HTTP client.
pub struct MockClient {
    responses: Mutex<VecDeque<HttpResponse>>,
    requests: Mutex<Vec<RecordedRequest>>,
}

impl MockClient {
    /// Queues responses in order.
    #[must_use]
    pub fn new(responses: Vec<HttpResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// Takes every request seen so far.
    #[must_use]
    pub fn take_requests(&self) -> Vec<RecordedRequest> {
        std::mem::take(&mut *self.requests.lock().expect("mock request lock"))
    }
}

impl HttpClient for MockClient {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, AuthError> {
        self.requests
            .lock()
            .expect("mock request lock")
            .push(RecordedRequest {
                method: request.method,
                url: request.url.clone(),
                headers: request.headers.clone(),
                body: request.body.clone(),
            });
        self.responses
            .lock()
            .expect("mock response lock")
            .pop_front()
            .ok_or_else(|| AuthError::transport("mock client has no queued response"))
    }
}

impl std::fmt::Debug for MockClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("MockClient").finish_non_exhaustive()
    }
}
