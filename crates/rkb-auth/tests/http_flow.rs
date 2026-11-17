//! Integration tests for the production HTTP client and the full sign-in flow.
//!
//! A loopback HTTP/1.1 server stands in for Microsoft, Xbox Live, Minecraft
//! Services, and the session server. The test uses the real [`UreqClient`], so
//! request formatting, headers, status handling, and JSON parsing are exercised
//! end to end. TLS is not covered here; that requires the public endpoints and
//! a real account, which is recorded as unverified in `docs/m2-verification.md`.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rkb_auth::{flow, AuthConfig, Endpoints, OnlineAuthenticator, SignedIn, UreqClient};
use rkb_net::SessionAuthenticator;

const PROFILE_UUID: &str = "b50ad385829d3141a2167e7d7539ba7f";
const REFRESH_TOKEN: &str = "refresh-value";
const MICROSOFT_ACCESS: &str = "microsoft-access-value";
const MINECRAFT_ACCESS: &str = "minecraft-access-value";

#[derive(Clone, Debug)]
struct MockRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: String,
}

impl MockRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

struct MockHttpServer {
    port: u16,
    requests: Arc<Mutex<Vec<MockRequest>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MockHttpServer {
    fn start(expected_requests: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("local address").port();
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_requests = Arc::clone(&requests);
        let thread_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut handled = 0;
            while handled < expected_requests
                && !thread_stop.load(Ordering::SeqCst)
                && Instant::now() < deadline
            {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if let Some((request, stream)) = read_request(stream) {
                            let (status, body) = response_for(&request.path);
                            if write_response(stream, status, &body).is_ok() {
                                thread_requests.lock().expect("request log").push(request);
                                handled += 1;
                            }
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            port,
            requests,
            stop,
            handle: Some(handle),
        }
    }

    fn endpoints(&self) -> Endpoints {
        Endpoints::with_base(&format!("http://127.0.0.1:{}", self.port))
    }

    fn finish(mut self) -> Vec<MockRequest> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            handle.join().expect("mock HTTP server thread");
        }
        let requests = self.requests.lock().expect("request log").clone();
        requests
    }
}

impl Drop for MockHttpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn read_request(stream: TcpStream) -> Option<(MockRequest, TcpStream)> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_owned();
    let target = parts.next()?.to_owned();
    let path = target.split('?').next().unwrap_or(&target).to_owned();

    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            break;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let length: usize = headers
        .get("content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body).ok()?;
    let body = String::from_utf8_lossy(&body).into_owned();
    Some((
        MockRequest {
            method,
            path,
            headers,
            body,
        },
        reader.into_inner(),
    ))
}

fn write_response(mut stream: TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Response",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

fn response_for(path: &str) -> (u16, String) {
    match path {
        "/devicecode" => (
            200,
            r#"{"device_code":"device-code-value","user_code":"ABCD",
                "verification_uri":"http://127.0.0.1/device","expires_in":900,
                "interval":5,"message":"go"}"#
                .to_owned(),
        ),
        "/token" => (
            200,
            format!(
                r#"{{"access_token":"{MICROSOFT_ACCESS}","refresh_token":"{REFRESH_TOKEN}","expires_in":3600}}"#
            ),
        ),
        "/xbox/user" => (
            200,
            r#"{"Token":"user-token-value","DisplayClaims":{"xui":[{"uhs":"user-hash"}]}}"#
                .to_owned(),
        ),
        "/xbox/xsts" => (
            200,
            r#"{"Token":"xsts-token-value","DisplayClaims":{"xui":[{"uhs":"user-hash"}]}}"#
                .to_owned(),
        ),
        "/minecraft/login" => (
            200,
            format!(r#"{{"access_token":"{MINECRAFT_ACCESS}","expires_in":86400}}"#),
        ),
        "/minecraft/profile" => (200, format!(r#"{{"id":"{PROFILE_UUID}","name":"Notch"}}"#)),
        "/minecraft/entitlements" => (
            200,
            r#"{"items":[{"name":"game_minecraft"},{"name":"product_minecraft"}]}"#.to_owned(),
        ),
        "/session/join" => (204, String::new()),
        _ => (404, r#"{"error":"not found"}"#.to_owned()),
    }
}

fn config(server: &MockHttpServer) -> AuthConfig {
    AuthConfig::new("test-client-id").with_endpoints(server.endpoints())
}

fn assert_no_token_in_urls(requests: &[MockRequest]) {
    for request in requests {
        for secret in [
            MICROSOFT_ACCESS,
            MINECRAFT_ACCESS,
            REFRESH_TOKEN,
            "user-token-value",
            "xsts-token-value",
            "device-code-value",
        ] {
            assert!(
                !request.path.contains(secret),
                "a token appeared in the request URL {}",
                request.path
            );
        }
    }
}

#[test]
fn real_http_client_completes_device_sign_in_and_session_join() {
    let server = MockHttpServer::start(8);
    let client = UreqClient::new(Duration::from_secs(5));
    let config = config(&server);

    let start = flow::begin_device_code(&client, &config).expect("device code");
    assert_eq!(start.user_code, "ABCD");
    assert_eq!(start.verification_uri, "http://127.0.0.1/device");
    let signed: SignedIn =
        flow::finish_device_code(&client, &config, &start, |_| {}).expect("complete");

    assert_eq!(signed.session.name(), "Notch");
    assert_eq!(
        rkb_auth::minecraft::uuid_hex(signed.session.uuid()),
        PROFILE_UUID
    );
    assert_eq!(
        signed.refresh_token.as_ref().map(|token| token.len()),
        Some(REFRESH_TOKEN.len())
    );
    assert_eq!(signed.session.entitlements().items.len(), 2);

    let mut authenticator = OnlineAuthenticator::from_session(
        Box::new(client),
        config.endpoints.clone(),
        signed.session,
    );
    authenticator
        .join_server("server-hash-value")
        .expect("session join");
    assert_eq!(authenticator.session().name(), "Notch");

    let requests = server.finish();
    let paths: Vec<&str> = requests
        .iter()
        .map(|request| request.path.as_str())
        .collect();
    assert_eq!(
        paths,
        vec![
            "/devicecode",
            "/token",
            "/xbox/user",
            "/xbox/xsts",
            "/minecraft/login",
            "/minecraft/profile",
            "/minecraft/entitlements",
            "/session/join",
        ]
    );
    let methods: Vec<&str> = requests
        .iter()
        .map(|request| request.method.as_str())
        .collect();
    assert_eq!(
        methods,
        vec!["POST", "POST", "POST", "POST", "POST", "GET", "GET", "POST"]
    );

    let device_code_request = &requests[0].body;
    assert!(device_code_request.contains("scope=XboxLive.signin%20offline_access"));

    assert!(requests[2].body.contains(&format!("d={MICROSOFT_ACCESS}")));
    assert!(requests[3]
        .body
        .contains(r#""UserTokens":["user-token-value"]"#));
    assert!(requests[4]
        .body
        .contains(r#""identityToken":"XBL3.0 x=user-hash;xsts-token-value""#));
    let expected_authorization = format!("Bearer {MINECRAFT_ACCESS}");
    assert_eq!(
        requests[5].header("authorization"),
        Some(expected_authorization.as_str())
    );
    assert!(requests[7]
        .body
        .contains(r#""serverId":"server-hash-value""#));
    assert!(requests[7]
        .body
        .contains(&format!(r#""selectedProfile":"{PROFILE_UUID}""#)));
    assert_no_token_in_urls(&requests);
}

#[test]
fn real_http_client_refreshes_a_session_without_rotating_the_token() {
    let server = MockHttpServer::start(6);
    let client = UreqClient::new(Duration::from_secs(5));
    let config = config(&server);
    let stored = rkb_auth::SecretString::new("old-refresh-value");

    let signed = flow::refresh(&client, &config, &stored).expect("refresh");
    assert_eq!(signed.session.name(), "Notch");
    assert_eq!(
        signed.refresh_token.as_ref().map(|token| token.len()),
        Some(REFRESH_TOKEN.len())
    );

    let requests = server.finish();
    assert_eq!(requests[0].path, "/token");
    assert!(requests[0].body.contains("grant_type=refresh_token"));
    assert!(requests[0].body.contains("refresh_token=old-refresh-value"));
    assert_no_token_in_urls(&requests);
}
