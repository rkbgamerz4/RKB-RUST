# Migration plan: standalone voxel prototype → Minecraft: Java Edition client

Status: proposed, 2026-11-16. This plan is written **before** any deletion or
rewrite. The old client keeps building and keeps its history. New work lands on
`feature/mc-java-protocol`.

## 1. Target decision

**First network version: Minecraft: Java Edition 1.21.1, protocol 767,
DataVersion 3955.**

Why:

- The only verified Minecraft data support in RKB is 1.21/1.21.1 save reading.
  Targeting 1.21.1 keeps one version descriptor for saves and wire data.
- Protocol 767 is a complete, well-established protocol with the modern
  login → configuration → play states.
- Horizon's 26.3 / protocol 777 tables are a reference, not a shortcut. Copying
  them would create a client whose packet ids, registries, and chunk layout
  disagree with its own save reader.

The version is a data table, not a hard-coded assumption. Adding 1.21.3, 1.21.4,
or 26.3 later means adding a descriptor, ids, and translation tests — exactly
the pattern Horizon uses.

**Non-goals for the first release:** every historical server version, Realms,
LAN discovery, mods, shaders, cosmetics, telemetry, built-in server, world
generation, and a bundled Java runtime.

## 2. Target architecture

```
rkb-core ── rkb-proto ── rkb-net ── rkb-client ── rkb-android
   │           │            │           │
   │           └── rkb-auth ┘           ├── rkb-world-client ── rkb-mc
   │                                    └── rkb-render-gpu ── rkb-assets
   └── rkb-platform / rkb-input / rkb-runtime / rkb-resource
```

| Crate | Responsibility | New? |
| --- | --- | --- |
| `rkb-proto` | VarInt/String/UUID/NBT codecs, framing, zlib, AES/CFB8, RSA, typed packets, 1.21.1 ids, status JSON | New |
| `rkb-auth` | OAuth device code, XBL, XSTS, Minecraft token, profile, join-server, token storage | New |
| `rkb-net` | Connection thread, session events, keep-alive, reconnect, packet dispatch | New |
| `rkb-world-client` | Resident chunk store, light, block entities, entity/player tracking, inventory state | New |
| `rkb-client` | Client state machine, 20 TPS simulation, input → actions, screens, orchestration | New |
| `rkb-assets` | Resource-pack zip/directory decoding: blockstates, models, textures, fonts, language | New |
| `rkb-render-gpu` | GPU terrain/entity/UI renderer behind a backend trait | New |
| `rkb-mc` | Save-format library: NBT, Anvil, palettes, registry, save/world | Reuse |
| `rkb-core`, `rkb-platform`, `rkb-input`, `rkb-resource`, `rkb-runtime` | Generic foundations | Reuse/adapt |
| `rkb-android` | Android host: launcher UI, auth browser, surface, lifecycle, input | Adapt |
| `rkb-minecraft`, `rkb-sandbox`, `rkb-demo`, `rkb-pak` | Legacy prototype | Park, then retire |

Dependency rules:

- `rkb-proto` has no platform, UI, or renderer dependency.
- `rkb-net` owns sockets and threads; the game thread never blocks on a socket.
- `rkb-world-client` has no rendering dependency; it stores blocks, light,
  entities, and inventories.
- `rkb-client` is the only crate that combines world, net, render, and input.
- `rkb-android` depends on `rkb-client`; it does not construct a game from
  inside JNI.

## 3. Reusable code list

### Keep as-is or with small extensions

| Code | Reuse | Work needed |
| --- | --- | --- |
| `rkb-core/src/math.rs` | `Vec2/Vec3/Mat4` foundations | Add `f64` positions/`DVec3`; Minecraft uses double-precision entity coordinates |
| `rkb-core/src/time.rs` | `FixedTimestep`, `RateCounter` | Add a 20 TPS tick accumulator separate from render delta |
| `rkb-core/src/lifecycle.rs` | Android pause/resume FSM | Verify against a networked session (pause ≠ disconnect) |
| `rkb-core/src/config.rs` | Simple key/value config | Add account/server options storage (no secrets in plaintext logs) |
| `rkb-platform` | `Surface`, `FrameClock` seam | Add a GPU surface path; keep the software surface for tests |
| `rkb-input` | Touch points, joystick, gesture detectors | Add keyboard/mouse, gamepad, UI hit-testing, Minecraft control mapping |
| `rkb-resource::path` | Traversal-safe asset paths | Add zip entry paths and namespace rules |
| `rkb-resource::{source,manager}` | File/memory sources and LRU cache | Add zip source; keep RKBPAK out of the Minecraft path |
| `rkb-mc::nbt` | Java NBT, modified UTF-8 | Reuse unchanged for packets and saved data |
| `rkb-mc::region` | Anvil regions | Unchanged for local saves |
| `rkb-mc::chunk` | 1.21 section palettes and block lookup | Add constructors from wire palettes; share bit-storage rules |
| `rkb-mc::registry` | Block-state identities from reports | Extend to items, entities, biomes, damage types, menus |
| `rkb-mc::version` | Version descriptors and strict unknown handling | Add protocol/packet-table identity alongside DataVersion |
| `rkb-mc::world` / `save` | Local world open/write | Keep for single-player/local saves; not used for server play |
| `rkb-runtime` | Lifecycle, timestep, present loop | Replace the `Game` trait with a networked `App` shape |
| `rkb-android` JNI plumbing | `JNIEnv`, direct buffer surface, lifecycle calls | Rework engine construction; add auth/browser/storage/platform calls |

### Salvage selected modules from `rkb-minecraft`

| Module | Verdict | Reason |
| --- | --- | --- |
| `camera.rs` | Port | First-person view and frustum logic are useful; move to `f64` and GPU matrices |
| `player.rs` | Adapt | AABB collision/jump skeleton; vanilla physics (friction, sprint, fluids, sneaking) is new work |
| `interaction.rs` | Adapt | Raycast skeleton; block shapes, reach, and placement rules are new work |
| `mesh.rs` | Rewrite | Face culling and AO ideas carry over; output must be GPU buffers with resource-pack models |
| `session.rs` | Replace | Local pose/seed persistence is not account/session management |
| `game.rs` | Reference only | Loop/overlay structure; renderer and world assumptions change |
| `blocks.rs` | Replace | Name→colour table replaced by registry + block models |
| `world_source.rs` | Replace | Network world store replaces generated/save source selection |
| `worldgen.rs` | Retire from product | Not Minecraft world generation |
| `demo_world.rs` | Retire from product | Synthetic fixture writer |
| `textures.rs` | Retire from product | Procedural atlas; replace with resource-pack textures |
| `bin/rkb-fixture.rs` | Park | Useful legacy fixture tool, not shipped |

### Keep but stop shipping

- `rkb-render`: keep as the deterministic software backend and golden-image test
  oracle. It is not the final renderer.
- `rkb-demo`: keep as a headless smoke test for the old foundation until CI is
  retargeted.
- `rkb-sandbox`: park. It exercises the foundation but has no product role.
- `rkb-pak`: park. `RKBPAK1` is not a Minecraft format.

## 4. Replace / retire list

Retire **from the product path**, not from Git:

1. Procedural world generation as the default world.
2. Procedural block atlas and the name→colour block table.
3. Software rasterizer as the final Android renderer.
4. `MinecraftGame` as the object JNI constructs.
5. Generated-world-first-launch behaviour and its world marker/session format.
6. The "1.21 client" claim in the README until a server join is verified.
7. Sandbox/demo code in the app entry point (already partly unwired; finish it
   behind a feature flag, then remove in a dedicated legacy-retirement commit).
8. Any compatibility claim based on the old generated world.

Replace:

| Old | New |
| --- | --- |
| `rkb-minecraft::world_source` (generated/save) | `rkb-world-client` fed by `rkb-net` |
| `rkb-minecraft::mesh` + `rkb-render` final path | `rkb-render-gpu` terrain pipeline |
| `rkb-minecraft::textures` | `rkb-assets` resource-pack textures/models |
| `rkb-runtime::Game` | `rkb-client::App` with network poll + 20 TPS + render |
| `AndroidEngine::new(world)` | `AndroidEngine::new(options)` + launcher/session state |
| Save-only `VersionSpec` | Version descriptor including protocol and packet tables |

Deletion is a separate, final step. Nothing is removed in this migration phase
until the replacement passes its acceptance gate.

## 5. Milestones and acceptance gates

Each gate says what must be **observed**, not merely written.

### M0 — Audit, plan, protocol spine (this branch)

- Documents: this plan, `docs/audit-current.md`, `docs/horizon-comparison.md`.
- New `rkb-proto`: VarInt/VarLong, strings, UUIDs, frame, zlib, typed
  handshake/status/login packets for protocol 767, status ping client.
- Tests: byte-level golden vectors, malformed input, compression on/off,
  mock-server handshake/status exchange.
- Gate: `cargo test -p rkb-proto`, clippy/fmt clean. **No server-compatibility
  claim**; a status ping is not a login.

### M1 — Login and configuration against a test server

- AES/CFB8, RSA PKCS#1 v1.5, server hash, encryption request/response.
- Compression threshold activation and enforcement.
- Login success, login acknowledged, configuration registry data/tags/feature
  flags/known packs, finish configuration.
- Offline-mode login first; online-mode join only after M2.
- Gate: join a user-supplied vanilla 1.21.1 server in offline mode through the
  configuration state; packet trace reviewed; disconnect reasons decoded.

### M2 — Microsoft authentication

- Device-code flow, XBL user auth, XSTS, Minecraft token, profile, join server.
- Own Azure application id; no Horizon id, no client secret in the binary.
- Token store with refresh; never log tokens.
- Gate: sign in with a user-supplied account and complete an online-mode join
  to a user-supplied server. Until then, auth is "implemented, unverified".

### M3 — World join and chunk streaming

- Join game, dimension/registry data, chunk batch and light packets.
- Client chunk store with paletted sections, heightmaps, block entities.
- Headless block queries and chunk statistics.
- Gate: for a test server, blocks at sampled coordinates match the server's
  own state (`/setblock`, `/data`, or a vanilla-client screenshot).

### M4 — Players, entities, and movement

- Spawn/remove, relative/absolute moves, metadata, rotation, teleport
  confirmation, keep-alives.
- Local player physics and collision against server chunks.
- Gate: two clients (RKB + vanilla) see each other move; positions agree within
  tolerance; no desync over a timed run.

### M5 — Inventory, containers, and basic gameplay

- Window items, clicks, hotbar, held item, drop, basic crafting.
- Chat send/receive and command feedback; secure-chat status handled honestly.
- Gate: item moves survive a reconnect; container contents match the server.

### M6 — GPU renderer and resource packs

- GPU backend (wgpu or Vulkan/GLES) with terrain, entities, sky, fog, UI.
- Resource-pack zip/directory loading: blockstates, models, textures, fonts,
  language; user-supplied pack only.
- Gate: frames from RKB and the vanilla client on the same server/area are
  compared; documented differences remain.

### M7 — Android product path

- Launcher/account/server-list UI (Kotlin or Rust, decided before starting).
- Auth browser/device-code UX, storage permissions, network lifecycle.
- Touch controls adapted to Minecraft actions; no GPL layout copied.
- Gate: emulator install + join; physical ARM64 device run with logs and
  performance numbers.

### M8 — Hardening

- Reconnect, timeout, malformed packet, oversized frame, decompression bomb,
  unknown packet, and disconnect-reason tests.
- Fuzzing for codecs; bounded memory; thread shutdown on pause/destroy.
- Gate: no panics on malformed input; documented resource ceilings.

Renderer work (M6) can run in parallel with M1–M5 by a separate track, but the
world store interface must land first so it is not designed around the old
mesher.

## 6. Verification policy

- **Unit/golden tests** for every codec and frame path.
- **Mock server** tests for state machines (deterministic, offline).
- **User-supplied vanilla server** for real joins. This environment has none;
  the gate stays open until one is used.
- **User-supplied account** for Microsoft auth. No credentials in the repo.
- **User-supplied Minecraft files** for resource packs. No Mojang assets are
  committed or redistributed.
- **Physical device** for Android performance and Vulkan claims.
- Wording rule: "implemented" means code + tests; "verified" means observed
  against the named external system. Never conflate them.

## 7. Blockers and decisions needed

| Blocker | Impact | Proposed handling |
| --- | --- | --- |
| No Android NDK/SDK/Kotlin compiler locally | Android changes verified only in CI | Keep host-only tests; use CI for APK; do not claim local device results |
| No physical Android device | No Vulkan/thermal/performance proof | Gate M7; mark unverified |
| No Minecraft 1.21.1 files locally | No resource-pack or vanilla-server test here | User supplies files/server; keep tests synthetic |
| No Microsoft account/client id here | Auth cannot be end-to-end tested | M2 gate; user supplies; never commit tokens |
| Renderer gap (software → GPU) | Largest schedule risk | Decide wgpu vs direct Vulkan/GLES before M6; prototype early |
| Rust 1.77 / edition 2021 | Blocks direct Horizon source reuse | Raise MSRV only if a dependency requires it; adopt patterns, not files |
| Version choice | 26.3 vs 1.21.1 splits the data path | Default to 1.21.1; treat 26.3 as a later descriptor |
| Android host shape | Current JNI hard-codes the old game | Decide Kotlin launcher + GPU surface vs NativeActivity before M7 |
| Secure chat (1.19+) | Chat signing affects online servers | Implement profile keys and signing in M2/M5; document fallback |

## 8. Branching and rollback

1. `feature/mc-java-protocol` branches from `bef6cc1` (current default head).
2. Old crates remain present and tested on that branch.
3. Each milestone is a reviewable commit series; no force-push.
4. The Android entry point switches only after M1+M3 produce a joinable,
   inspectable session. Until then the old app remains the default build.
5. Legacy retirement is its own commit/PR: delete `rkb-sandbox`/`rkb-demo`/
   generated-world paths only after the replacement gate passes.
6. Rollback is `git checkout feature/rkb-rust-foundation`; no history is lost.

## 9. Definition of done for "a Minecraft Java client"

The product may be described as a Minecraft: Java Edition-compatible client only
when all of the following are demonstrated:

- Microsoft sign-in and online-mode join to a vanilla server.
- Server world chunks, blocks, light, and entities rendered.
- Local movement and collision accepted by the server.
- Inventory transfer and chat work.
- The Android ARM64 build runs on a physical device.
- The version, protocol, and verification environment are stated with the build.

Until then the accurate description is: "a Rust Minecraft protocol client in
development, currently at milestone Mx."
