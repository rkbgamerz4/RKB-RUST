# M2 verification: Microsoft authentication (implemented, unverified for real accounts)

Status: **implemented and locally tested on 2026-11-17; the real-account gate is
still open.** No Microsoft account and no Azure application id were available
in this environment, so a real device-code sign-in and a successful online-mode
join against the official server are **not claimed**.

This document separates observed evidence from the remaining gate, following
the policy in [`migration-plan.md`](migration-plan.md).

## Scope

M2 adds a sign-in chain and connects it to the existing login state machine:

```text
device code → Microsoft OAuth token
  → Xbox Live user token → XSTS token
  → Minecraft Services token
  → profile + entitlements (ownership)
  → [online server EncryptionRequest] → session-server join
  → EncryptionResponse → … M1 configuration …
```

It does not add play-state packets, chunk streaming, rendering, or Android
wiring. The client still stops at `FinishConfiguration`.

## Code map

| Piece | Location |
| --- | --- |
| Crate API, re-exports | `crates/rkb-auth/src/lib.rs` |
| Endpoints and relying parties | `crates/rkb-auth/src/endpoints.rs` |
| Error types with length-limited, token-free messages | `crates/rkb-auth/src/error.rs` |
| Blocking HTTP seam and production ureq client | `crates/rkb-auth/src/http.rs` |
| Redacted, zeroizing secrets and JSON guard | `crates/rkb-auth/src/secret.rs` |
| Device code, polling, refresh | `crates/rkb-auth/src/oauth.rs` |
| Xbox Live user auth and XSTS | `crates/rkb-auth/src/xbox.rs` |
| Minecraft login, profile, entitlements, UUIDs | `crates/rkb-auth/src/minecraft.rs` |
| Session, join, `SessionAuthenticator` impl | `crates/rkb-auth/src/session.rs` |
| High-level sign-in and refresh | `crates/rkb-auth/src/flow.rs` |
| Opt-in refresh-token store | `crates/rkb-auth/src/store.rs` |
| Diagnostic CLI | `crates/rkb-auth/src/bin/rkb-auth.rs` |
| Authenticated UUID and outcome flag | `crates/rkb-net/src/login.rs` |
| Mock protocol server assertions | `crates/rkb-net/tests/session.rs` |
| Real-HTTP loopback flow | `crates/rkb-auth/tests/http_flow.rs` |

## Implemented behavior

- **Device-code OAuth.** `POST /consumers/oauth2/v2.0/devicecode`, polling with
  `authorization_pending`, `slow_down`, `expired_token`, and
  `authorization_declined` handling, then the refresh-token grant.
- **Xbox Live and XSTS.** `d=<Microsoft token>` RPS ticket, `RETAIL` sandbox,
  `rp://api.minecraftservices.com/`, `XBL3.0 x=<uhs>;<xsts>` identity token.
- **Minecraft Services.** `login_with_xbox`, `/minecraft/profile`, and
  `/entitlements/mcstore`. Ownership accepts `game_minecraft` or
  `product_minecraft`; a 404 profile is `NoProfile`, a missing entitlement is
  `NotOwned`.
- **Online-mode login.** The M1 seam calls `join_server(hash)` before sending
  `EncryptionResponse`; `LoginOptions::with_uuid` sends the authenticated profile
  UUID in `LoginStart`, and `LoginOutcome::session_authenticated` reports
  whether session-server authentication actually ran.
- **Refresh.** Profile and entitlement checks rerun on every refresh. A rotated
  refresh token replaces the old one; if Microsoft does not rotate, the old
  token is retained.
- **CLI.** Prints only the verification URL, user code, profile name, UUID, and
  entitlements. It never prints access tokens, refresh tokens, device codes, or
  Xbox tokens.

## Security properties and limits

- No password is requested. RKB ships no Azure client id or client secret; the
  caller supplies a registered public application id.
- Tokens are `SecretString` values: `Debug` prints `<redacted>`, the backing
  buffer is zeroized on drop, and JSON request/response bodies are zeroized
  after use. HTTP request/response `Debug` prints header names and body lengths
  only.
- Tokens appear in an `Authorization: Bearer` header or a request body, never
  in a URL. Tests assert this for every recorded request.
- ureq follows redirects without forwarding `Authorization` by default
  (`RedirectAuthHeaders::Never`).
- Token persistence is opt-in. `FileTokenStore` writes only the refresh token,
  profile UUID, and name; creates the file with mode `0600`; refuses symbolic
  links and group/other permissions; and replaces the file atomically.
  `MemoryTokenStore` is the default.
- No operating-system certificate store: ureq 3.2.1 + rustls 0.23 with bundled
  `webpki-roots`, which is what an Android ARM64 build needs.
- Limits: the refresh token is stored in plaintext inside a `0600` file, so the
  same user account (or root) can read it. There is no hardware keystore
  integration yet. TLS is not pinned. The CLI does not display a browser page;
  the user opens the printed URL.

## Test evidence

Command:

```bash
cargo test --workspace --all-targets --locked
```

Observed 2026-11-17: **349 passed, 0 failed, 0 ignored** across the workspace.
`rkb-auth` contributes 38 unit tests, 2 loopback HTTP integration tests, and 1
doc test.

### Unit tests

Covered without the network:

- device-code form encoding, pending/slow-down/expiry/decline classification,
  and polling until a token arrives;
- refresh-token fallback when Microsoft does not rotate;
- Xbox user/XSTS claims, XErr codes, and missing `uhs`;
- Minecraft login body, bearer profile request, entitlement parsing, strict
  UUID parsing, and 404 → `NoProfile`;
- full account flow profile + ownership check, join body (`accessToken`,
  `selectedProfile`, `serverId`), and 204 success;
- `OnlineAuthenticator` implementing `rkb_net::SessionAuthenticator`;
- redaction of every token-bearing `Debug` implementation;
- file-store round trip with `0600`, rejection of `0644` and unknown versions,
  and memory-store clear.

### Real-HTTP loopback flow

`crates/rkb-auth/tests/http_flow.rs` starts a loopback HTTP/1.1 server and uses
the production `UreqClient` (not a mock trait object). It records the exact
request sequence:

```text
POST /devicecode
POST /token
POST /xbox/user
POST /xbox/xsts
POST /minecraft/login
GET  /minecraft/profile
GET  /minecraft/entitlements
POST /session/join
```

Assertions include form encoding of `XboxLive.signin offline_access`, the
`d=` RPS ticket, XSTS `UserTokens`, the `XBL3.0` identity token, the bearer
profile request, join body fields, and that no token appears in any URL. A
second test covers refresh with `grant_type=refresh_token`.

This exercises the real ureq request builder, header handling, status handling,
and JSON parsing over plain HTTP. It does **not** exercise TLS certificate
validation against Microsoft, Xbox, or Mojang.

### Mock protocol server

`crates/rkb-net/tests/session.rs` now also asserts:

- `LoginStart` carries the authenticated UUID when
  `LoginOptions::with_uuid` is set;
- `session_authenticated` is `false` for offline logins and `true` only when
  the server sets `should_authenticate` and the authenticator succeeds.

## Real vanilla server regression

Server: official Mojang `server.jar` 1.21.1, SHA-1
`59353fb40c36d304f2035d51e7d6e6baa98dc05c`, Java 21.0.13, port 25575,
`network-compression-threshold=256`, flat world.

### Offline mode (`online-mode=false`)

Client:

```bash
cargo run -p rkb-net --bin rkb-login -- 127.0.0.1:25575 RKBTestM2
```

Observed:

```text
version: 1.21.1 protocol 767 DataVersion 3955
player: RKBTestM2 (9a75ec71-7e05-3abf-a767-02c5e302b78b)
encrypted: false
session authenticated: false
compression: threshold 256
registries: 11
feature flags: 1
server known packs: 1
configuration: finished
```

Server log:

```text
[02:30:42] [Server thread/INFO]: RKBTestM2[/127.0.0.1:54474] logged in with entity id 1 at (-5.5, -60.0, -9.5)
[02:30:42] [Server thread/INFO]: RKBTestM2 joined the game
[02:30:42] [Server thread/INFO]: RKBTestM2 lost connection: Disconnected
[02:30:42] [Server thread/INFO]: RKBTestM2 left the game
```

This confirms M2 did not regress the verified M1 offline path. The intentional
disconnect after configuration is still not a playable world join.

### Online mode without an authenticator (`online-mode=true`)

The same server was restarted with `online-mode=true`, and the client was run
without an authenticator:

```bash
cargo run -p rkb-net --bin rkb-login -- 127.0.0.1:25575 RKBOnlineReject
```

Client (exit status 1):

```text
login failed: protocol error: server requires online-mode authentication and no authenticator is configured
```

Server log:

```text
[02:32:18] [Server thread/INFO]: RKBOnlineReject (/127.0.0.1:44834) lost connection: Disconnected
```

The client did not send a fabricated `EncryptionResponse`, did not invent a
session, and did not log a token. The server setting was restored to
`online-mode=false` afterwards. This test proves the explicit rejection path,
**not** a successful online-mode join.

## Not verified

- **Real Microsoft device-code sign-in.** Requires a user-supplied Microsoft
  account and an Azure public-client application id with device-code flow
  enabled. No credentials were available; none were fabricated.
- **Successful online-mode join to the official server.** Requires the same
  account and a server reachable from this environment. The session join body,
  encryption seam, and authenticated UUID are covered by tests, but the real
  Mojang session server and the server's online-mode verification were not
  exercised.
- **TLS to Microsoft, Xbox, and Mojang.** `UreqClient` is compiled with rustls
  and bundled roots and is used by the loopback tests, but no request reached an
  HTTPS endpoint here.
- **Android ARM64 auth runtime.** `rkb-auth` is a host crate in M2. Wiring it
  into the Android JNI application is M5/product-path work.
- **Play state and a world.** M3–M4. The client still does not decode Join Game,
  chunks, entities, inventory, or health, and does not render a server world.

## Reproduce

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked

# device-code sign-in (prints no tokens); requires your own Azure app id
RKB_MS_CLIENT_ID=<your-azure-app-id> \
  cargo run -p rkb-auth --bin rkb-auth

# optional online-mode login with an opt-in token file
RKB_MS_CLIENT_ID=<your-azure-app-id> \
  cargo run -p rkb-auth --bin rkb-auth -- \
    --token-file ~/.config/rkb/auth.json --server <host[:port]>
```

If you run the online-mode command with a real account, record the exit status,
the CLI's non-secret output, and the server's `joined the game` line. That
observation is what closes the M2 gate.
