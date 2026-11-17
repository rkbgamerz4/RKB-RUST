# M3 verification: play state and server chunks (implemented, offline-verified)

Status: **implemented and verified against the official 1.21.1 server in
offline mode on 2026-11-17.** The client now completes login and configuration,
enters the play state, receives and stores real server chunks, tracks block
updates and entities, answers keep-alives and teleports, and reports the
server's disconnect reason.

This document follows the evidence policy in
[`migration-plan.md`](migration-plan.md): "implemented" means code plus tests;
"verified" means observed against the named external system.

The M3 gate in the plan (sampled blocks match the server's own state) is met
for an offline-mode server. Microsoft sign-in, a successful online-mode join,
rendering, and Android wiring remain unverified and are out of M3 scope.

## Scope

```text
FinishConfiguration
  → Login (Join Game)
  → chunk batches / light / block updates
  ⇄ keep-alive, teleport confirm, chunk batch received, pong
  → entities, health, inventory, time, game state, chat
  → Disconnect
```

M3 adds the play state and a headless data model. It does **not** add rendering,
local movement or physics, container clicks, secure-chat signing, or Android
networking. The play state is exercised by `rkb-play` and by the `rkb-auth`
`--play` path.

## Code map

| Piece | Location |
| --- | --- |
| Play packet codecs, positions, bitsets, slots, paletted containers, chunks | `crates/rkb-proto/src/packets/play.rs` |
| Protocol 767 play packet ids | `crates/rkb-proto/src/ids.rs` |
| NBT skippers and integer helpers | `crates/rkb-proto/src/codec.rs` |
| `PlaySession`, stores, events, auto-replies | `crates/rkb-net/src/play.rs` |
| `login_configured` / `ConfiguredSession` | `crates/rkb-net/src/login.rs` |
| Diagnostic play client | `crates/rkb-net/src/bin/rkb-play.rs` |
| Authenticated play path | `crates/rkb-auth/src/bin/rkb-auth.rs` |
| Loopback login → configuration → play tests | `crates/rkb-net/tests/play.rs` |
| Real-vanilla chunk regression fixture | `crates/rkb-proto/tests/chunk_fixture.rs` |
| Captured chunk 0,0 payload (7022 bytes) | `crates/rkb-proto/tests/fixtures/chunk_0_0_1_21_1.bin` |

`rkb-net` still does not depend on `rkb-auth`. `rkb-auth` uses
`rkb_net::PlaySession` after an authenticated login.

## Implemented behavior

### Join and world state

- `Login` (Join Game): entity id, hardcore flag, world names, max players, view
  and simulation distance, reduced debug info, respawn screen, secure-chat
  enforcement, and the full dimension-type registry.
- Dimension type parsing: `min_y`, `height`, section count, skylight, ambient
  light, logical height, coordinate scale, bed works, respawn anchor works,
  piglin safe, natural, infiniburn, monster spawn light limits, and monster
  spawn block light limit.
- `Respawn`: updates the dimension and game mode and clears chunks, block
  updates, light, and entities. The client does not guess old-world data.
- `ChangeGameState` reason 13 (game mode) updates the tracked game mode.
- `UpdateTime` and `Experience` are decoded and reported.

### Chunks and blocks

- `LevelChunkWithLight`: x/z, anonymous NBT heightmaps, section buffer, block
  entities, and the four light bitsets plus sky/block light arrays.
- `ChunkColumn` keeps paletted sections compactly: single-valued, indirect, and
  direct containers, with the documented bits-per-entry limits (15 for block
  states, 6 for biomes).
- Blocks are queried at world coordinates through `PlaySession::block_state`,
  which applies stored single-block updates over chunk data.
- `BlockUpdate` and section `BlockUpdate` are stored; `UnloadChunk` removes the
  column and its light; `UpdateLight` stores light-only changes.
- `ChunkBatchStart` / `ChunkBatchFinished` are tracked and
  `ChunkBatchReceived` is sent with the measured chunks-per-tick rate.
- A chunk is only stored after the server sends it. There is no procedural
  substitute terrain anywhere in the client path.

### Player, entities, and inventory

- `SynchronizePlayerPosition` applies the relative/absolute flag bits, marks the
  player as positioned, and answers `ConfirmTeleportation`.
- `SetHealth` tracks health, food, and saturation.
- `SetContainerContent` / `SetContainerSlot` store 1.21.1 slots. Item data
  components are **rejected explicitly**: the packet becomes
  `Event::Unsupported`, stored slots are left unchanged, and the session keeps
  running.
- `SetHeldItem`, `SpawnEntity`, relative and absolute moves, look, teleport, and
  `RemoveEntities` keep an entity table with interpolated tracked positions.
- `SystemChat` is reduced to plain text; action-bar messages are flagged.
- `KeepAlive`, `Ping`, and `StartConfiguration` are acknowledged. The initial
  play join does **not** send `ConfigurationAcknowledged`; only an actual
  `StartConfiguration` does.
- `Disconnect` decodes the NBT reason to readable text.
- Unknown ids outside the protocol 767 play range are reported separately from
  known ids this milestone chooses not to model.

### Unhandled packets in the evidence run

The final 55-second run reported **39 unhandled packets, 0 unsupported, 0
unknown**:

```text
0x00 bundle_delimiter (2)        0x41 recipe
0x0B change_difficulty           0x4B server_data
0x11 commands                    0x54 set_chunk_cache_center (2)
0x1A damage_event                0x56 set_default_spawn_position
0x1F entity_event                0x58 set_entity_data (7)
0x25 initialize_border           0x5A set_entity_motion (6)
0x29 level_particles             0x68 sound (2)
0x38 player_abilities            0x71 ticking_state
0x3E player_info_update (4)      0x72 ticking_step
                                 0x74 update_advancements
                                 0x75 update_attributes (2)
                                 0x77 update_recipes
```

These are counted and surfaced, not parsed. They carry tab-list metadata,
attributes, recipes, sounds, particles, and entity metadata that M3's data model
does not need yet. `disguised_chat` (0x1E), which vanilla `/say` uses, is also
still unhandled; `/tellraw` arrives as `system_chat` (0x6C) and is decoded.

## The paletted-container wire bug

The first real chunk decode failed with trailing bytes after the section buffer.
The cause was a wire detail confirmed from the vanilla 1.21.1 bytecode
(`LevelChunkSection.write` → `PalettedContainer$Data.write`):

- every paletted container is `bits per entry` + palette + length-prefixed long
  array;
- a single-valued container uses `bits = 0`, a `VarInt` palette value, and
  **still writes the long array length**, which is `0`;
- therefore a single-valued container is exactly **three bytes**, not two.

The two-byte assumption left two unconsumed bytes per single-valued (air)
section, so a 24-section column accumulated a trailing-bytes error that scaled
with the number of air sections. Encoding now writes the zero word count too,
and decoding rejects a non-zero word count for a single-valued container.

Regression coverage:

- `crates/rkb-proto/tests/chunk_fixture.rs` decodes a real 7022-byte chunk
  payload captured from the official server and asserts 24 sections, 2245 data
  bytes, 1024 non-air blocks, bedrock/dirt/dirt/grass/air at y = -64…-60,
  `grass_block` at the opposite corner, `None` outside the column, and the light
  array counts.
- The fixture is world data from a local test server, not a Mojang asset.

## Real vanilla server evidence

Server: official Mojang `server.jar` 1.21.1, SHA-1
`59353fb40c36d304f2035d51e7d6e6baa98dc05c`, Java 21.0.13, port 25575,
`online-mode=false`, default flat world, view and simulation distance 3,
`network-compression-threshold=256`.

Client: `rkb-play` (offline login, protocol 767, DataVersion 3955).

### Join

```text
joined: entity 9 in minecraft:overworld (game mode 0, flat world)
  worlds: minecraft:overworld, minecraft:the_end, minecraft:the_nether | view distance 3 | simulation distance 3 | max players 5
  hardcore: false | respawn screen: true | secure chat enforced: false
  dimension minecraft:overworld (id 0): min_y -64 height 384, skylight
  dimension minecraft:overworld_caves (id 1): min_y -64 height 384, skylight
  dimension minecraft:the_end (id 2): min_y 0 height 256
  dimension minecraft:the_nether (id 3): min_y 0 height 256
position: (40.50, -60.00, 40.50) yaw 0.0 pitch 0.0 teleport 1
```

Server log:

```text
[03:32:20] [Server thread/INFO]: RKBTestM3[/127.0.0.1:...] logged in with entity id 9 at (40.5, -60.0, 40.5)
[03:32:20] [Server thread/INFO]: RKBTestM3 joined the game
```

### Chunks and blocks

- 77 chunk columns at join, then 102 after a teleport back to the origin; each
  flat column decoded as 24 sections with 1024 non-air blocks and 0 block
  entities.
- Chunk batches were answered (3 and 16 in the two runs).
- After `setblock 40 -60 40 stone`:

```text
block update at (40, -60, 40): state 1 (minecraft:stone)
```

- After `setblock 2 -61 2 glowstone` + `setblock 2 -60 2 stone`:

```text
block update at (2, -61, 2): state 5863 (minecraft:glowstone)
block update at (2, -60, 2): state 1 (minecraft:stone)
block sample at (2, -60, 2): minecraft:stone
```

- After a teleport back to the origin:

```text
position: (0.50, -60.00, 0.50) yaw 0.0 pitch 0.0 teleport 2
block sample at (0, -61, 0): minecraft:grass_block[snowy=false]
```

This is the M3 gate: the sampled block matches the server's own state, and the
override is visible through the same query.

Block names come from a `blocks.json` report generated from a user-supplied
1.21.1 installation (`--block-report`). Without it, updates print numeric state
ids; RKB bundles no Mojang data.

### Entities, health, inventory, chat, disconnect

Armor stand (`summon minecraft:armor_stand 42 -55 42`):

```text
entity 8 spawned: type 3 at (42.50, -55.00, 42.50)
entity 8 moved to (42.50, -55.00, 42.50)
entity 8 moved to (42.50, -55.23, 42.50)
entity 8 moved to (42.50, -56.15, 42.50)
entity 8 moved to (42.50, -57.69, 42.50)
entity 8 moved to (42.50, -59.84, 42.50)
entity 8 moved to (42.50, -60.00, 42.50)
```

The tracked position follows the server's falling entity to the ground. A pig
spawned as type 77 in another run.

Health and inventory:

```text
inventory window 0: 46 slots
health: 20.0 food 20 saturation 20.0
health: 16.0 food 20 saturation 20.0   # after /damage 4
health: 18.0 food 20 saturation 20.0   # regeneration
health: 19.0 food 20 saturation 18.0
health: 20.0 food 20 saturation 18.0
experience: bar 0.00 level 0 total 0
```

Chat and disconnect:

```text
chat: hello from vanilla tellraw
...
disconnected by server: final m3 evidence
```

A far teleport also produced `chunk (x, z) unloaded` events for the old view
(77 and 54 in two runs), confirming unload handling. `/kick` reasons were
decoded in every run.

### Summary counters (final run)

```text
packets: 276
chunks received: 77
keep-alives answered: 3
chunk batches answered: 3
block updates: 1
chat messages: 1
unsupported packets: 0
unhandled packets: 39
unknown packets: 0
client exit 0
```

## Test evidence

Command:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
```

Observed 2026-11-17: **379 passed, 0 failed, 0 ignored** across the workspace,
fmt clean, clippy clean.

M3-specific tests:

- `rkb-proto`: 89 unit tests plus the real-chunk fixture test. New coverage
  includes packed positions, bitsets, light, slots, paletted single/indirect/
  direct containers, wrong word counts and bit widths, chunk section counts,
  block entities, and NBT skipper truncation.
- `rkb-net`: 23 unit tests. New play tests cover join/dimension setup, chunk
  storage only after a server packet, block-update override, entity
  spawn/move/remove, keep-alive/ping/teleport replies, chunk-batch replies,
  respawn resets, chat and game state, disconnect reasons, relative position
  flags, start-configuration acknowledgement, malformed section counts, missing
  dimension types, and inventory component rejection.
- `rkb-net/tests/play.rs`: 2 loopback tests drive a real socket through login,
  configuration, join, chunk, block update, inventory, and disconnect.
- `rkb-net/tests/session.rs`: 10 login/configuration tests still pass, including
  authenticated UUID and offline/online-mode behavior.

## CI

Runs dispatched for commit `e59a1e9` on `feature/mc-java-protocol`:

| Workflow | Run | Result |
| --- | --- | --- |
| Rust CI | `44011791285` | success |
| Android ARM64 | `44011796359` | success |
| Android emulator startup | `44011801557` | success |

## Not verified

- **Real Microsoft sign-in and a successful online-mode join.** The same gate as
  M2 remains open; no account or Azure application id was available. The
  authenticated CLI path to the play state is implemented but unexercised
  against a real session server.
- **TLS to Microsoft/Xbox/Mojang.** Loopback tests use plain HTTP; no HTTPS
  endpoint was reached.
- **Rendering of server chunks.** M4. `rkb-minecraft` still renders only its own
  saved/generated world; the play-state chunks are not yet connected to the
  renderer.
- **Android networking.** M5. The play state runs on the host only; the JNI
  application is unchanged.
- **Local movement and collision with the server.** M3 tracks server-sent
  positions; it does not send `SetPlayerPosition` or client-side physics.
- **Container clicks, crafting, and dropping.** Only server-to-client slot data
  is stored.
- **Secure chat signing.** The server reported `secure chat enforced: false`;
  signing is not implemented.
- **Forced resource packs.** Still rejected explicitly.
- **Entity metadata and player list.** Entity types and positions are tracked,
  but `set_entity_data` and `player_info_update` are unhandled.

## Reproduce

```bash
# offline-mode server, then:
cargo run -p rkb-net --bin rkb-play -- \
  --server 127.0.0.1:25575 \
  --name RKBTestM3 \
  --seconds 55 \
  --block-report /path/to/1.21.1/generated/reports/blocks.json

# authenticated online-mode path (requires your own Azure app id + account):
RKB_MS_CLIENT_ID=<your-azure-app-id> \
  cargo run -p rkb-auth --bin rkb-auth -- \
    --token-file ~/.config/rkb/auth.json --server <host[:port]> --play --seconds 60
```

To reproduce the block gate, run `/setblock` and `/tp` from the server console
while `rkb-play` is connected and compare the printed `block update` and
`block sample` lines. Record the server's `logged in with entity id` and
`joined the game` lines alongside the client output.
