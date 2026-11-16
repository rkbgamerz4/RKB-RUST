# Horizon architecture comparison

Status: source-reading report, 2026-11-16. Horizon was **not executed**. No
Minecraft installation, jar, asset bundle, or live server was available. Every
statement below comes from reading the local snapshot's source, manifests, and
documentation. Horizon's own parity claims are not repeated as RKB-verified
facts.

This is a pre-migration baseline. The subsequent M0 slice added `rkb-proto`
(codecs, framing, status ping); it did not change the comparison below.

## Provenance and licence

- Local path: `artifacts/horizon/horizon-android-main` (gitignored).
- Directory name is `horizon-android-main`; the snapshot contains **no `.git`
  metadata**, so the exact commit cannot be proven from the tree itself.
- Its README points at `https://github.com/horizon-rust/horizon` and describes
  the Android port. The snapshot is therefore treated as an Android-port
  snapshot of the upstream project, not as a pinned upstream checkout.
- Any future code reuse must first re-fetch a specific upstream commit and
  record that commit plus the MIT notice. Do not cite "Horizon main" as a
  reproducible source.
- `LICENSE`: MIT, Copyright (c) 2026 Ludovic Marie and the Horizon contributors.
  MIT is compatible with RKB's `MIT OR Apache-2.0` when the copyright and
  permission notice are retained.
- Third-party exclusions that matter:
  - `crates/horizon-client/src/controls/default_layout.json` is Zalith
    Launcher 2's layout under **GPL-3.0**. RKB must not copy it; touch layouts
    must be original.
  - The bundled OpenJDK 25 runtime is **GPL-2.0 with Classpath Exception**.
    RKB does not bundle Java.
  - Microsoft's four-square sign-in mark and branding are Microsoft's. RKB must
    follow Microsoft's branding rules; do not copy Horizon's drawing code as if
    it were an MIT asset.
  - Minecraft code, assets, and data remain Mojang's. Horizon ships none and
    prepares them from a user-owned installation.

## Version constants

| Fact | Horizon snapshot | RKB today |
| --- | --- | --- |
| Game version | 26.3 | 1.21 / 1.21.1 (saves only) |
| Protocol | 777 | 767 declared, **not implemented** |
| DataVersion | 5023 | 3953 / 3955 verified for saves |
| Resource pack format | 97.1 | 34 |
| Data pack format | 121.0 | 48 |
| Java runtime expected | Java 25 | none |
| Workspace Rust | edition 2024, `rust-version = 1.95` | edition 2021, `rust-version = 1.77` |

The edition/MSRV gap matters: Horizon source cannot be dropped into the current
RKB workspace without raising the toolchain and porting idioms. Architecture can
be adopted without that cost; wholesale source reuse cannot.

## Scale and shape

| Measure | Horizon snapshot | RKB |
| --- | ---: | ---: |
| Workspace crates | 49 | 12 |
| Rust files under `crates/` | 2,739 | ~90 |
| Largest client crate | `horizon-client` ≈ 710k lines | `rkb-minecraft` ≈ 5.6k lines |
| World crate | `horizon-world` ≈ 151k lines | `rkb-mc` ≈ 4.8k lines |
| Render crate | `horizon-client-render` ≈ 259k lines | `rkb-render` ≈ 1.5k lines |
| Test files | 104 under `tests/` | unit tests per crate |

Horizon is a full engine reimplementation with an integrated server. RKB is a
small foundation plus a save-format library. Matching Horizon feature-for-feature
is a multi-year effort; the migration must be staged.

## Subsystem comparison

### Protocol and framing

Horizon `crates/horizon-protocol`:

- Primitive codecs (`buf.rs`), VarInt/VarLong, strings, UUIDs, NBT, bitsets.
- Framing with a 21-bit maximum frame and zlib compression above a threshold
  (`frame.rs`).
- AES/CFB8 connection encryption, Java-style server-hash digest, offline UUIDs
  (`crypto.rs`).
- RSA PKCS#1 v1.5 for the encryption exchange and secure-chat signatures
  (`rsa.rs`).
- Typed packet enums for handshake, status, login, configuration, and play
  (`packets/`), with unknown packets preserved as raw id + payload.
- Packet id tables per state in `ids.rs`; data-driven registries in
  `registry.rs`.
- Per-version adapters (`adapter/`) translating older servers to the current
  protocol. This is a large, ongoing compatibility surface.
- Chunk payloads: heightmaps, serialized paletted containers, block entities,
  and light layers (`chunk.rs`).

RKB today has **none** of this. The nearest asset is `rkb-mc`'s **save**
palette parser, which is a different serialization from the wire format.

### Connection and session management

Horizon `crates/horizon-client/src/net.rs` (≈2,390 lines):

- `Session` owns a dedicated `horizon-net` thread and an event channel.
- Address parsing, SRV handling, TCP connect, optional friends tunnel.
- Handshake → login hello → encryption request/response → compression →
  login success → login acknowledged.
- Configuration state: registry data, tags, feature flags, known packs, resource
  packs, finish-configuration acknowledgement.
- Play: keep-alives, teleports, chunk batches, cookies, transfers, reconnects,
  recording/replay hooks.
- Separate read/write halves behind a writer lock; `Event` values are polled by
  the game thread.

RKB has no socket layer, no network thread, and no event model.

### Authentication

Horizon's flow, from `launcher/src/account.rs`,
`horizon-client-render/src/auth.rs`, and `launcher/src/account/session.rs`:

1. OAuth 2.0 **device code** flow against
   `https://login.microsoftonline.com/consumers/oauth2/v2.0`
   (`/devicecode`, `/token`), with a public Azure client id.
2. Xbox Live user authentication (`user.auth.xboxlive.com`).
3. XSTS authorization (`xsts.auth.xboxlive.com`) with relying party
   `rp://api.minecraftservices.com/`.
4. Minecraft services login with `XBL3.0 x=<uhs>;<xsts>` to obtain a Minecraft
   access token.
5. Profile/entitlement checks via `api.minecraftservices.com`.
6. Online-mode join: `sessionserver.mojang.com/session/minecraft/join` with the
   server hash.
7. Secure-chat profile keys fetched separately and refreshed.

The Azure client id is configuration (`HORIZON_MSA_CLIENT_ID`, with a public
default in `android/build.sh`). RKB must register its own application; reusing
Horizon's id would misrepresent the app and is not acceptable.

RKB has no auth code and no client id.

### World and chunk storage

Horizon separates:

- `horizon-world`: authoritative/integrated world, level data, chunk status,
  worldgen, block behaviour, fluids, ticks, entities, loot, datafixers,
  structures.
- `horizon-core/src/paletted.rs`: vanilla `PalettedContainer` with single,
  linear, hashmap, and global palettes and non-spanning bit storage.
- `horizon-client-render/src/chunk.rs`: resident client chunk store with
  `Arc` sections, copy-on-write edits, and off-thread meshing/lighting.
- Protocol chunk decoding is in `horizon-protocol/src/chunk.rs`, not in the
  save reader.

RKB's `rkb-mc` already implements the save-side equivalents of NBT, Anvil,
palettes, and block lookup for 1.21/1.21.1. It needs a **client** chunk store,
wire-format section decoding, heightmaps, light, block entities, and registry
identity tables before it can render a server world.

### Rendering and assets

Horizon:

- `wgpu 30.0.1` (Vulkan/Metal/DX12/GLES), `winit 0.30.13`, `glam 0.30`.
- GPU terrain pipeline, GPU culling and occlusion, HDR, shadows, shaders,
  particles, clouds, sky, entities, block entities, fluids, UI.
- `horizon-assets` decodes resource packs: blockstates, models, item models,
  animations, palettes, fonts.
- `horizon-prepare` imports the user's Minecraft installation and runs Mojang
  exporters on a bundled OpenJDK 25, hashing artifacts into a manifest.

RKB:

- Deterministic CPU rasterizer presenting RGBA8.
- Procedural atlas generated from hashes; no resource packs, models, or items.
- No GPU path.

A genuine client cannot keep the current renderer as its final path. The
migration must either build a GPU terrain renderer (wgpu or Vulkan/GLES) and a
resource-pack pipeline, or explicitly scope a reduced visual target. Horizon's
renderer is an architectural reference, not a drop-in dependency.

### Android

Horizon Android (`docs/android.md`, `android/`):

- NativeActivity; Java side limited to clipboard, links, soft keyboard, back.
- `minSdk 29` (Android 10), `targetSdk 35`, `arm64-v8a` only, Vulkan required.
- One winit event loop shared by launcher and game (Android allows one).
- Launcher signs in, downloads 26.3 via Mojang, or accepts a copied
  `Android/data/<pkg>/files/minecraft` folder.
- Preparation exporters run on a bundled OpenJDK 25; the phone compiles
  exporters when no matching ones ship.
- Touch controls follow Zalith layouts, with editing/import/export, gyro, and
  gamepad support (layout JSON is GPL-3.0; do not copy).

RKB Android:

- Kotlin `SurfaceView` host plus a JNI library, `aarch64-linux-android`.
- The engine hard-codes `Runtime<MinecraftGame>`; there is no launcher, account
  UI, server list, or download/prepare flow.
- CI builds an APK and runs an emulator smoke test; no physical-device proof.

## What to adopt, what not to copy

Adopt as architecture:

1. Version-pinned descriptors with separate packet/registry tables.
2. Typed packet enums that preserve unknown packets.
3. A dedicated network thread with an event channel to the game thread.
4. Login → configuration → play as explicit states, with acknowledgements.
5. Paletted containers with vanilla bit-width rules on both save and wire paths.
6. A resident client chunk store with immutable section snapshots.
7. A narrow platform seam for window/surface/input.
8. User-owned asset preparation kept outside the engine binary.

Do **not** copy:

- 26.3 packet ids into a 1.21.1 client (they do not match).
- The Zalith GPL layout, the Microsoft logo drawing, the bundled JRE, or
  Minecraft assets.
- Horizon's product-layer crates (cosmetics, shop, telemetry, official servers,
  friends relay).
- The per-version adapter matrix before a single version works.
- Its 710k-line game crate; RKB needs its own smaller gameplay layer.

## Consequences for RKB

- Pick **one** network version first. RKB's verified save path points to
  **1.21.1 / protocol 767**. Implementing 767 first keeps the save and wire
  worlds consistent and avoids adopting Horizon's 26.3 tables before they can
  be verified.
- Expect the renderer and asset pipeline to dominate the schedule, not the
  handshake.
- Multiplayer cannot be claimed until an actual server join is demonstrated.
  A status ping is not a login; a login is not a playable session.
- Microsoft auth needs RKB's own Azure application and a user-facing device-code
  flow. It cannot be exercised in this environment.
