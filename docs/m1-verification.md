# M1 verification: login and configuration (protocol 767)

Status: **M1 complete for offline mode**, verified 2026-11-16 on
`feature/mc-java-protocol`. Online-mode join remains an M2 gate.

This document records what was actually observed. "Implemented" means code plus
tests; "verified" means observed against the named external system.

## Scope

M1 adds the network path between a status ping and a world join:

```text
handshake(Login) → LoginStart
  → [EncryptionRequest → RSA/PKCS#1 v1.5 + AES/CFB8 + server hash
     → EncryptionResponse]
  → [SetCompression] → LoginSuccess → LoginAcknowledged
  → configuration: client information, known packs, registry data, tags,
    feature flags, keep-alive, ping
  → FinishConfiguration (clientbound) → FinishConfiguration (serverbound)
```

It stops when configuration finishes. It does not send or decode play packets,
does not claim a world join, and does not implement Microsoft authentication.

## Code map

| Piece | Location |
| --- | --- |
| AES-128, AES/CFB8 | `crates/rkb-proto/src/crypto/aes.rs` |
| RSA DER parse, PKCS#1 v1.5, big-integer modexp | `crates/rkb-proto/src/crypto/rsa.rs` |
| SHA-1, server hash, Java signed-hex rendering | `crates/rkb-proto/src/crypto/{sha1.rs,mod.rs}` |
| MD5 offline UUID | `crates/rkb-proto/src/crypto/md5.rs` |
| Framing, zlib threshold activation/enforcement | `crates/rkb-proto/src/frame.rs` |
| Login packets | `crates/rkb-proto/src/packets/login.rs` |
| Configuration packets | `crates/rkb-proto/src/packets/configuration.rs` |
| Framed encrypted connection | `crates/rkb-net/src/connection.rs` |
| Login/configuration state machine | `crates/rkb-net/src/login.rs` |
| Disconnect text from JSON and network NBT | `crates/rkb-net/src/chat.rs` |
| Anonymous network-NBT reader reuse | `crates/rkb-mc/src/nbt.rs` (`Nbt::read_anonymous`) |
| Diagnostic binary | `crates/rkb-net/src/bin/rkb-login.rs` |

Protocol 767 and DataVersion 3955 are pinned in `rkb-proto`; a test in
`rkb-net` resolves `rkb_mc::Versions::find(DATA_VERSION)` and asserts its
protocol and verified status, so the two claims cannot drift apart.

## Cryptographic verification

- AES-128 block encryption matches the FIPS-197 vector
  `00112233…eeff → 69c4e0d8…c55a`.
- AES/CFB8 matches the NIST SP 800-38A CFB8 vector
  `6bc1bee2…8e51 → 3b79424c…5052`. The expected ciphertext was independently
  produced with `openssl enc -aes-128-cfb8`.
- SHA-1 matches short, incremental, million-`a`, and 55/56/57/63/64/65/119/120
  byte padding-boundary vectors.
- MD5 matches short vectors, and `OfflinePlayer:Notch` produces
  `b50ad385-829d-3141-a216-7e7d7539ba7f`, matching Java's
  `UUID.nameUUIDFromBytes`.
- RSA parses the X.509 `SubjectPublicKeyInfo` fixture, rejects malformed DER,
  and PKCS#1 v1.5 encryption round-trips through the matching private key
  embedded only in tests. A ciphertext produced independently by
  `openssl pkeyutl -encrypt -pkeyopt rsa_padding_mode:pkcs1` decrypts to the
  expected plaintext.
- The server hash `SHA1(server_id || secret || public_key_der)` rendered as
  Java's signed hex was checked with an independent Java program:
  `-4b0bb9f567cb95ca630d64d0e2055b4c3b0f1504` for the test inputs.
  `SHA1("jeb_")` renders as `-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1`.

No real key, credential, token, or session is stored in the repository or
printed by the client.

## Mock-server verification

`crates/rkb-net/tests/session.rs` implements an independent server side of the
wire protocol over loopback:

- offline login through compression and `FinishConfiguration`;
- encrypted login (`should_authenticate=false`) with RSA-decrypted secret and
  verify token, then AES/CFB8 for every later frame;
- authenticated login where the mock computes the vanilla server hash and
  compares it with what `SessionAuthenticator` received;
- online-mode request without an authenticator fails explicitly;
- login plugin request and login cookie request get responses;
- client information, known packs, registry data, feature flags, keep-alive,
  ping/pong, and the finish acknowledgement are checked byte-for-byte;
- negative paths: declared uncompressed size below the threshold, malformed
  zlib, unknown login packet, JSON login disconnect, and NBT configuration
  disconnect.

The mock decrypts the shared secret with the test private key, so a passing
encrypted test covers RSA parsing, PKCS#1 padding, AES state setup, and frame
ordering. It does not prove interoperability with a Mojang server; the real
server run below does that for the unencrypted/offline path.

## Real vanilla server verification

Server:

- Official Mojang `server.jar` for 1.21.1.
- SHA-1 `59353fb40c36d304f2035d51e7d6e6baa98dc05c`, matching
  `piston-data.mojang.com/v1/objects/59353fb40c36d304f2035d51e7d6e6baa98dc05c/server.jar`.
- Java 21.0.13, `online-mode=false`, port 25575,
  `network-compression-threshold=256`, flat world.

The first run was rejected by the server:

```text
Packet configuration/serverbound/minecraft:client_information (aaa) was larger
than I expected, found 1 bytes extra whilst reading packet
serverbound/minecraft:client_information
```

That identified a real protocol-767 difference: the `particle_status` field in
client information belongs to a later protocol. It was removed for 1.21.1, the
golden byte test was updated, and the run was repeated.

Client command:

```bash
cargo run -p rkb-net --bin rkb-login -- 127.0.0.1:25575 RKBTest
```

Client output (abridged):

```text
version: 1.21.1 protocol 767 DataVersion 3955
player: RKBTest (1e10815c-4278-32aa-bb30-8550a2f231e1)
encrypted: false
compression: threshold 256
registries: 11
feature flags: 1
server known packs: 1
configuration: finished
```

The client observed 11 registry packets, the tag packet, feature flags, known
packs, and `FinishConfiguration`, and sent its acknowledgement.

Server log:

```text
[15:48:12] [Server thread/INFO]: RKBTest[/127.0.0.1:41148] logged in with entity id 1 at (-1.5, -60.0, -8.5)
[15:48:12] [Server thread/INFO]: RKBTest joined the game
[15:48:13] [Server thread/INFO]: RKBTest lost connection: Disconnected
[15:48:13] [Server thread/INFO]: RKBTest left the game
```

The disconnect is intentional: M1 closes after configuration and does not
decode play packets. The server-side "joined the game" line is evidence that
login and configuration succeeded and the server entered play. It is **not**
evidence of a playable world join, chunk streaming, movement, or rendering.

## Reproduce

```bash
# terminal 1 (a server you are allowed to run)
java -Xms512M -Xmx1024M -jar server.jar nogui

# terminal 2
cargo run -p rkb-net --bin rkb-login -- 127.0.0.1:25565 Steve
cargo test -p rkb-proto
cargo test -p rkb-net
```

## Not verified

- **Online-mode join against a real server.** It requires Microsoft/Xbox/XSTS
  authentication (M2). The encryption and server-hash path is covered by the
  mock server and independent crypto vectors, but not by a real Mojang session
  server.
- **Play state.** No Join Game, chunk, entity, inventory, or rendering packet is
  handled yet.
- **Android runtime for networking.** The new crates are host-tested; the
  Android APK path still hosts the old prototype until the product path is
  rewired.
- **Forced resource packs.** The client rejects a forced pack explicitly
  instead of downloading it; vanilla's default server does not send one.
