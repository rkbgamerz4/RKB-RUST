# M4 verification: server chunks in the renderer (implemented, offline-verified)

Status: **implemented and verified against the official 1.21.1 server in
offline mode on 2026-11-17.** The renderer now draws chunks, blocks, and live
block updates received from a server through the same `MinecraftGame` runtime
used by the Android host. There is no generated or demo terrain in the server
path.

Real Microsoft sign-in, an online-mode join, and the Android runtime remain
unverified; those are separate gates. See
[`m3-verification.md`](m3-verification.md) for the protocol evidence this builds
on.

## Scope

M4 connects the M3 play session to the existing world and mesh path:

```text
server chunks (compact paletted columns)
  → rkb_net::ServerWorld conversion (registry state ids → block names)
  → rkb_mc::Chunk (the shape the mesher already consumes)
  → MinecraftGame / WorldState::Server
  → existing chunk mesher, atlas, camera, and software renderer
```

It adds no new rendering backend and no terrain generator. A missing server
chunk is an empty hole, never filled with procedural terrain.

## Code map

| Piece | Location |
| --- | --- |
| Server world, background reader, conversion | `crates/rkb-net/src/server.rs` |
| Chunk conversion tests | `crates/rkb-net/src/server.rs` (`mod tests`) |
| Loopback server-world test | `crates/rkb-net/tests/play.rs` |
| `WorldState::Server` and server accessors | `crates/rkb-minecraft/src/world_source.rs` |
| Game integration, server polling, read-only guard | `crates/rkb-minecraft/src/game.rs` |
| Headless multiplayer render check | `crates/rkb-minecraft/src/bin/rkb-join.rs` |

## Implemented behavior

- **Background session.** A reader thread owns the socket and continues to
  answer keep-alives, teleports, and chunk batches. The render thread calls
  `ServerWorld::poll`, which never blocks on the network.
- **Join synchronization.** `ServerWorld::from_configured` waits for Join Game
  and the first server position before returning, so the camera starts where the
  server placed the player instead of at a guessed spawn.
- **Chunk conversion.** Every network section becomes an expanded
  `rkb_mc::Section` through block-state ids resolved by the user-supplied,
  version-pinned `blocks.json` registry. A missing state id is an error, not
  air and not a guessed block.
- **Block updates.** Single-block changes are applied to the resident chunk and
  reported as dirty so the mesher rebuilds it. The game also sees them through
  `block_at`.
- **Unloads and respawns.** Server chunk unloads drop meshes; a respawn clears
  the resident server chunks and overrides.
- **Player sync.** Server teleports move the camera and spawn point. Minecraft
  yaw/pitch are converted to the renderer's convention.
- **Read-only multiplayer.** Local break/place is refused with an explicit
  message because M4 does not send block actions or movement to the server.
  Autosave is disabled for server worlds; nothing is written into a local save.
- **Honest status.** Join details, protocol errors, conversion errors, and the
  server's disconnect reason are shown in the game status.

## Real vanilla server evidence

Server: official Mojang `server.jar` 1.21.1, SHA-1
`59353fb40c36d304f2035d51e7d6e6baa98dc05c`, Java 21.0.13, port 25575,
`online-mode=false`, default flat world, view and simulation distance 3.

Command:

```bash
cargo run -p rkb-minecraft --bin rkb-join -- \
  --server 127.0.0.1:25575 \
  --name RKBJoinM4 \
  --block-report generated/reports/blocks.json \
  --seconds 40 --hold 18 \
  --query 0,-60,-2 --query 1,-60,-2 \
  --out /tmp/rkb-m4-frame.ppm
```

Observed:

```text
block registry: 26684 states from .../blocks.json
joined: position (0.50, -60.00, -5.50)
world: server | Joined a protocol-767 server world; only server chunks are rendered.
rendered 774 frames: 45 meshed chunks (77 resident), 23054 faces,
  8076 triangles, 17 visible
player: (0.50, -60.00, -5.50) yaw 3.14 pitch -0.00 |
  multiplayer world; local saves are disabled
block at (0, -60, -2): minecraft:glowstone
block at (1, -60, -2): minecraft:stone
```

Server log while the client was running:

```text
[03:52:06] RKBJoinM4[/127.0.0.1:...] logged in with entity id 13 at (0.5, -60.0, -5.5)
[03:52:06] RKBJoinM4 joined the game
[03:52:13] Teleported RKBJoinM4 to 0.500000, -60.000000, -5.500000
[03:52:15] Changed the block at 0, -60, -2
[03:52:17] Changed the block at 1, -60, -2
[03:52:24] RKBJoinM4 lost connection: Disconnected
```

The `/setblock` commands ran after the client had joined. Both changes were
visible to the renderer through the block query, and the glowstone and stone
appear in the captured frame. The terrain is the server's flat-world grass,
not RKB generated terrain.

Captured frame (480x270, software renderer, procedural RKB atlas; no Mojang
textures): grass plain, stone block, and glowstone block received from the
server. The frame had 1,912 unique colours, 56,939 green pixels, and zero black
pixels.

## Test evidence

Command:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
```

Observed 2026-11-17: **384 passed, 0 failed, 0 ignored**, fmt clean, clippy
clean.

M4-specific tests:

- `rkb-net` unit tests convert a synthetically built indirect section into a
  renderer `Chunk`, verify air/stone cells, reject an unknown state id instead
  of substituting a block, resolve block updates through the registry, and read
  single/indirect palette values.
- A loopback integration test logs in, joins, sends a chunk, a teleport, a
  block update, and a keep-alive, then wraps the session in `ServerWorld`. It
  asserts the converted chunk is reported dirty, the server position and angles
  arrive, the block update becomes `minecraft:glowstone`, an unrelated
  coordinate stays stone, and the disconnect reason is preserved.

## Not verified

- **Online-mode join and real Microsoft sign-in.** The M2 gate is unchanged.
- **Android networking and rendering of a server world.** M5. The JNI host
  still opens a local save/generated world.
- **Local movement accepted by the server.** M4 renders server data but does
  not send player movement or block actions, so the server does not see the
  local camera move.
- **Entity rendering.** Entities are tracked in the session but not meshed or
  drawn.
- **Server light and biomes.** Chunk light arrays are stored by M3 but the
  renderer still uses its fixed sky/fog and face shading.
- **Resource packs and Mojang textures.** The atlas remains RKB procedural.
- **Long-run multiplayer stability.** The evidence is a short offline run, not
  a timed desync or reconnect test.

## Reproduce

```bash
# offline-mode server you are allowed to run, then:
cargo run -p rkb-minecraft --bin rkb-join -- \
  --server 127.0.0.1:25575 \
  --name RKBJoinM4 \
  --block-report /path/to/1.21.1/generated/reports/blocks.json \
  --seconds 40 --hold 18 \
  --query 0,-60,-2 \
  --out /tmp/rkb-m4-frame.ppm

# while it runs, from the server console:
#   tp RKBJoinM4 0.5 -60 -5.5
#   setblock 0 -60 -2 minecraft:glowstone
```

`rkb-join` exits non-zero when no server chunk geometry was rendered, so an
empty or misconfigured join cannot be mistaken for a successful world.
