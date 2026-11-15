# Minecraft: Java Edition 1.21 data support

RKB RUST's Minecraft subsystem reads saved Java Edition 1.21 world data, writes
RKB-generated worlds back in the same Anvil layout, and runs a playable client
on that data. The data layer is implemented in `crates/rkb-mc` and exposed
through the `rkb-world` tool; the client is `crates/rkb-minecraft`. It reads and
writes only files supplied by or created for the user; no Minecraft code,
assets, or data are bundled.

## Compatibility matrix

The saved-data table below records what the reader and writer accept. The client
matrix records what has actually been exercised. A version is never marked
supported just because its number is known.

| Minecraft | DataVersion | Save read | Generated default | Edit overlay | Session write | Region write | Evidence |
| --- | ---: | --- | --- | --- | --- | --- | --- |
| 1.21 | 3953 | Verified | Yes (RKB terrain) | Yes | Yes (atomic) | Yes (atomic) | Host tests; emulator fresh-install + synthetic save |
| 1.21.1 | 3955 | Verified | Yes (RKB terrain) | Yes | Yes (atomic) | Yes (atomic) | Host tests only |
| 1.21.2–1.21.8 | 4080–4440 | Rejected | n/a | n/a | n/a | n/a | Explicit unsupported version |
| Older / newer | any other | Rejected | n/a | n/a | n/a | n/a | Explicit unsupported version |

"Generated default" means the client can create terrain from an RKB seed when no
save exists; it is **not** Mojang's world generator and does not reproduce
vanilla seeds, structures, or biomes exactly. Region write applies to
RKB-generated worlds, not to a user save: edits to a user save live in the
`rkb-session.json` overlay.

## Verified save versions

A version is **verified** only when its constants and the saved format this
crate parses are covered by tests. The save's `DataVersion` is authoritative;
the folder name is not trusted.

| Version | DataVersion | Protocol | Resource pack | Data pack | Status |
| --- | ---: | ---: | ---: | ---: | --- |
| 1.21 | 3953 | 767 | 34 | 48 | Verified |
| 1.21.1 | 3955 | 767 | 34 | 48 | Verified |

Constants were cross-checked against Mojang's published version manifest and
the Minecraft Wiki. 1.21 and 1.21.1 share protocol 767 and pack formats 34/48,
but have distinct DataVersions. Files from any other version are rejected with
a message naming the supported DataVersions.

### Recorded but not supported

These 1.21.x DataVersions are known from public version data. They are **not**
marked supported: their chunk/palette/datafix differences have not been tested.

| Version | DataVersion |
| --- | ---: |
| 1.21.2 | 4080 |
| 1.21.3 | 4082 |
| 1.21.4 | 4189 |
| 1.21.5 | 4325 |
| 1.21.6 | 4435 |
| 1.21.7 | 4438 |
| 1.21.8 | 4440 |

Adding a row to this table does not make the version work. Each one needs its
own descriptor and tests before it leaves this list.

## Saved-world layout read by this crate

### `level.dat`

`level.dat` is a gzip-compressed named NBT compound. The reader accepts gzip,
zlib, or raw NBT, detected from the leading bytes.

```text
{
  Data: {
    DataVersion: Int,
    LevelName: String,
    LastPlayed: Long,
    SpawnX/Y/Z: Int,
    GameType: Int,
    ...
  },
  ...
}
```

`World::open` requires `Data.DataVersion`, resolves it in the version table, and
only then opens region files.

### Region files

A save stores chunks in `region/r.X.Z.mca`, where `X` and `Z` are the chunk
coordinates divided by 32 (floor division, so negative coordinates work).

Each file is an 8 KiB header plus 4 KiB sectors:

- 4096 bytes of locations: 3-byte big-endian first-sector index, 1-byte sector
  count, per chunk slot;
- 4096 bytes of timestamps;
- chunk payloads: 4-byte big-endian length (compression byte included), 1-byte
  compression id, then compressed NBT.

Supported compression ids:

| Id | Format | Read | Write |
| ---: | --- | --- | --- |
| 1 | gzip | yes | yes |
| 2 | zlib | yes | yes |
| 3 | uncompressed | yes | yes |
| 4 | LZ4 | no, explicit error | no |

The reader validates that sectors start after the header, fit inside the file,
and that the declared payload length fits the allocated sectors. Decompressed
chunks are capped at 64 MiB to bound compression bombs.

### Chunk NBT

The fields used by this crate:

```text
{
  DataVersion: Int,
  xPos: Int,
  zPos: Int,
  Status: String,          // optional for lookup
  sections: [
    {
      Y: Byte,
      block_states: {
        palette: [
          { Name: String, Properties: { key: String, ... }? },
          ...
        ],
        data: [Long]?      // absent for a one-entry palette
      },
      biomes: { ... }       // ignored for now
    },
    ...
  ],
  block_entities: [ ... ]  // ignored for now
}
```

Section coordinates are world Y divided by 16, rounded down. A standard
Overworld chunk has Y from -4 through 19 (`min_y = -64`, height 384), but the
reader does not hard-code the dimension height: it reads the sections present.

Block cells use the vanilla order:

```text
index = (y << 8) | (z << 4) | x
```

Palette bit width is:

```text
bits = max(4, ceil(log2(palette_length)))
```

Values are packed without spanning 64-bit words:

```text
entries_per_long = 64 / bits
word_index       = cell_index / entries_per_long
shift            = (cell_index % entries_per_long) * bits
value            = (word >> shift) & ((1 << bits) - 1)
```

For a multi-entry palette, `data` must contain exactly
`ceil(4096 / entries_per_long)` longs. Every decoded index must be less than the
palette length. A one-entry palette has no `data` array. These rules are tested
at 4, 5, and larger bit widths, including the 15/16 cell boundary where a naive
spanning decoder fails.

## Generated worlds and the RKB session

When the client starts in an empty world directory it creates generated terrain
instead of showing an empty sky. The terrain is RKB code, produced only from a
seed:

- value-noise height and climate fields at several scales;
- biomes selected per column (plains, forest, desert, snowy, ocean);
- bedrock floor, stone/dirt filler, grass/sand/snow surfaces, water to sea
  level, gravel sea-floor variation, cacti, and oak trees that can cross chunk
  boundaries;
- chunk sections built in memory with real palettes and serialized exactly like
  a 1.21 chunk.

A 3x3 chunk area around spawn is generated eagerly; remaining chunks are
requested from one background `rkb-chunk-gen` thread and meshed only after the
full 3x3 neighbourhood exists, which prevents false walls at seams. Chunks
outside a retention square are dropped and regenerate deterministically if the
player returns.

The seed, spawn, player pose, and edit overlay are stored beside the save in
`rkb-session.json`:

```json
{
  "format": 1,
  "saved_at": 1760000000000,
  "seed": 735000000000000001,
  "spawn": [0.5, 67.0, 0.5],
  "player": {
    "position": [0.5, 67.0, 0.5],
    "yaw": 0.0,
    "pitch": 0.0,
    "flying": false
  },
  "edits": [[4, 66, 5, "minecraft:oak_planks", {}]]
}
```

`rkb-generated-world` is a marker file. A directory with `level.dat` but neither
marker nor session is treated as a user save. Generated worlds additionally
write `level.dat` and `region/r.X.Z.mca` from resident chunks, so the
`rkb-world` reader can reopen the block data. These chunks omit biomes,
heightmaps, and lighting, so the writer is not yet a drop-in vanilla save
exporter. All writes go through a temporary sibling and an atomic rename; a
crash leaves either the old complete file or the new one. A corrupt or
future-format session is reported and recovered by regenerating the default
seed rather than being silently ignored.

## Implemented API

### `rkb_mc::nbt`

- `Nbt::read`, `read_gzip`, `read_zlib`, `read_auto`.
- `Nbt::write`, `write_gzip`, `write_zlib`.
- All NBT tag types, including arrays and nested compounds/lists.
- Java modified UTF-8: NUL (`C0 80`), BMP characters, and supplementary
  characters as surrogate pairs. Invalid/overlong sequences and unpaired
  surrogates are rejected.
- Limits on depth, strings, collection sizes, and decoded bytes.
- Round-trip, truncation, duplicate-key, trailing-data, and malformed-input
  tests.

### `rkb_mc::region`

- `RegionFile::open`, `from_bytes`, `location`, `timestamp`, `chunk_count`,
  `read_chunk`, `read_chunk_nbt`.
- `Compression::{Gzip, Zlib, None, Lz4, Unknown}`.
- `RegionWriter` for synthetic fixtures and future saving; writes deterministic
  headers and sector-aligned payloads.
- Header bounds, zero length, length mismatch, bad offset, bad count, unknown
  compression, and corrupt payload tests.

### `rkb_mc::chunk`

- `BlockState { name, properties }`, `Display`, NBT round-trip, air detection.
- `Section` palette/storage parsing, construction (`new`, `filled`,
  `set_block`, `cells`, `to_nbt`), and local block lookup.
- `Chunk::parse`, `Chunk::new`, `set_block`, `to_nbt`, section iteration,
  world-Y lookup, non-air count.
- Rejection of empty/oversized palettes, missing storage, wrong storage length,
  out-of-range indices, duplicate section Y values, and malformed entries.

### `rkb_mc::registry`

- `BlockRegistry::from_json`, `from_file`, `from_value`.
- Parses Mojang's generated `blocks.json` report: per-block `states`, numeric
  ids, `default`, and `properties`.
- Requires contiguous ids from 0 and exactly one default per block.
- Registry data is not bundled; the user generates it from the matching game
  version.

### `rkb_mc::version`

- `VersionSpec` and `Versions::require` / `require_with`.
- Unknown DataVersions are rejected. `Recognized` versions require an explicit
  opt-in; there are none in the current table.

### `rkb_mc::world`

- `World::open` / `open_with`, `level`, `stats`, `chunk`, `block_at`,
  `clear_cache`.
- Validates `level.dat` before reading chunks.
- Requires each chunk's DataVersion to be supported.
- Rejects a chunk whose `xPos`/`zPos` do not match its slot.
- Caches loaded chunks; region headers are reopened per miss, which keeps file
  descriptors bounded and is adequate for inspection. The client clears the
  cache when the player crosses a chunk boundary.

### `rkb_mc::save`

- `write_atomic(path, bytes)` with a same-directory temporary file.
- `level_tag`, `write_level`, `write_world` for `level.dat` + Anvil regions.
- `SaveError` with the failing path in every message.
- Tests reopen a written world with `World::open` and check that no temporary
  files remain after replacement.

### `rkb_minecraft` client modules

- `worldgen`: `TerrainGenerator`, `Biome`, `GeneratedWorld`, `DEFAULT_SEED`,
  `SEA_LEVEL`, `WORLD_MIN_Y`/`WORLD_MAX_Y`; deterministic generation, spawn
  search, background worker, retention, and generated-world save tests.
- `world_source`: `WorldState::{Save, Generated}`, `WorldKind`, `ChunkSource`,
  block lookup, 3x3 neighbourhood readiness, and generated chunk cloning.
- `session`: `rkb-session.json` format 1, atomic write, corruption/format errors,
  generated marker, and round-trip tests.
- `mesh`: `EditMap` overlay, edit-aware face culling/colour/counts, and
  generated blocks above stored sections.
- `interaction`: Amanatides-Woo voxel raycast returning the hit block and the
  placement face, with reach and negative-axis tests.
- `game`: world selection, spawn/player restore, background polling, accurate
  stats, autosave, break/place rules, player-intersection rejection, and HUD
  buttons.

## `rkb-world` CLI

```bash
cargo run -p rkb-mc --bin rkb-world -- info <save>
cargo run -p rkb-mc --bin rkb-world -- block <save> <x> <y> <z>
cargo run -p rkb-mc --bin rkb-world -- chunk <save> <x> <z>
cargo run -p rkb-mc --bin rkb-world -- registry <blocks.json> [block]
cargo run -p rkb-mc --bin rkb-world -- version
```

Examples:

```text
save: /sdcard/games/com.mojang/minecraftWorlds/Example
name: Example
minecraft: 1.21.1 (data version 3955)
protocol: 767
packs: resource 34, data 48
spawn: 0, 64, 0
regions: 1
chunks: 1
```

`block` prints `unloaded` when the chunk or section is absent. `chunk` lists each
section's palette size and non-air count. `registry` prints the state and block
counts, or every state of one block with its numeric id.

## Multi-version plan

The architecture separates four concerns:

1. **Version descriptor** (`version.rs`): DataVersion, protocol, pack formats,
   verification status.
2. **Chunk/data format** (`chunk.rs`, `region.rs`): parsing rules that can be
   version-specific where a release changes them.
3. **Registry pinning** (`registry.rs`): each version's own state ids and
   defaults, supplied as data rather than compiled in.
4. **Compatibility checks** (`world.rs`): reject unknown versions and mismatched
   chunks explicitly.

Adding 1.21.2, for example, is not just a table row. The checklist is:

- obtain the release's `version.json`/manifest and confirm DataVersion, protocol,
  and pack formats;
- add a descriptor marked `Recognized`, not `Verified`;
- add or update chunk/palette fixtures for that release;
- confirm whether the chunk NBT shape, palette rules, or datafixers changed;
- add a version-specific module if they did;
- compile the crate for `aarch64-linux-android`;
- only then mark the descriptor `Verified`.

The same rule applies to resource packs and data packs: a new pack format is not
accepted until its layout is tested.

## Roadmap

### 1. Data and resource compatibility

Done: NBT, region files, chunk sections, block-state palettes, block registry,
saved-world access, CLI, malformed-input tests.

Next: pack metadata (`pack.mcmeta`), resource-pack indexes, data-pack indexes,
blockstate/model JSON, language files, and versioned pack-format checks. These
must read user-supplied packs only.

### 2. World and chunk loading

Done: on-demand region/chunk reads, section lookup, chunk identity validation,
a deterministic RKB terrain generator, a 3x3 spawn neighbourhood, one background
generation worker, deterministic regeneration after retention eviction, session
persistence, and writing generated worlds as `level.dat` + Anvil regions.

Next: named seeds, structures, caves/ores, biomes with real climate tables,
entities, and a bounded region/chunk cache. Datafixers are required before
opening worlds older than the supported DataVersion.

### 3. Block states

Done: names, properties, palette decoding, air detection, registry lookup.

Next: block shapes and collision boxes, default-state resolution per version,
property enums/integers, and block behaviour tables.

### 4. Terrain rendering

`rkb-minecraft` streams chunks from either a save or the generated world, meshes
exposed block faces with neighbour culling, frustum-culls chunk bounds, and
renders blocks through the software renderer's depth-tested 3D triangle path.
Every non-air block is treated as a full cube with a flat approximate colour and
per-face shading; there is no texture atlas, lightmap, block model, or shape
support.

Next: texture atlas from a user's resource pack, transparent and non-cube block
models (slabs, stairs, fences), lightmap sampling, and memory/frame budgets for
low/mid-range phones.

### 5. Player movement and camera

Implemented in `rkb-minecraft`: a first-person camera, touch drag look, 0.6x1.8
AABB collision with sub-stepping, gravity, jumping, creative flight, fall
respawn, and a walking/jumping/flying player. Step-up, swimming, ladders,
sprinting, sneaking, and jump cooldown are not implemented yet.

Continue with: 1.21 movement constants, step-up and climbing, swimming,
sprinting/sneaking, and movement on the Android touch layout.

### 6. Block breaking and placing

Implemented: a 5-block DDA raycast, first-hit selection, **BREAK** to remove a
block, **PLACE** to put oak planks on the targeted face, player-intersection
rejection, edit-aware mesh refresh, and persistence in `rkb-session.json`.
Break/place works on generated worlds and on read-only user saves through the
client edit overlay.

Next: block hardness and tool rules, break progress, placement rules beyond the
player box, block updates, drops, and creative/survival differences.

### 7. Inventory and hotbar

Not implemented.

Next: item stacks, slot containers, hotbar selection, pickup, item use, crafting
grids, and the inventory UI.

### 8. Entities and mobs

Not implemented.

Next: entity data from chunks, item entities, mob spawning, pathfinding, combat,
and client interpolation.

### 9. Lighting and environment

Not implemented.

Next: light arrays from chunks, sky/block light propagation, day/night, weather,
fog, and biome colours.

### 10. Survival mechanics

Not implemented.

Next: health, hunger, damage, respawn, experience, and the survival HUD.

## Limitations of the current milestone

- User saves are still treated as read-only terrain: edits live in the
  `rkb-session.json` overlay rather than being written into the user's region
  files. Generated worlds do write `level.dat` and region files.
- no LZ4 region compression;
- no datafixers, so only DataVersions 3953 and 3955 are accepted;
- generated terrain is RKB terrain: it does not reproduce vanilla seeds,
  structures, caves, ores, or exact biome placement;
- biomes, heightmaps, block entities, entities, structures, and lighting in a
  user save are ignored;
- the registry is not bundled and is only used by the `registry` command, not
  yet to validate every palette entry;
- the client treats every block as a full cube with an approximate flat colour:
  no textures, resource packs, biome tints, lighting, shadows, or partial-block
  shapes (slabs, stairs, fences, and similar blocks render and collide as cubes);
- placing always produces oak planks, and block hardness/tools/drops are not
  modeled;
- inventory, crafting, mobs, survival, and networking are not implemented;
- the emulator workflow verifies both a fresh generated-world startup and a
  synthetic supplied-save startup. A physical Android device was not available,
  so real-hardware touch, thermals, and performance remain unverified.

## Tests

`cargo test --workspace` currently runs 199 unit/integration tests, including:

- 41 `rkb-mc` unit tests plus 6 CLI integration tests that build synthetic
  saves and run the binary;
- `rkb-minecraft` tests for generated terrain determinism, surface and spawn
  correctness, background chunk delivery, save/reopen, edit-aware meshing,
  raycast reach, generated startup stats, session round-trip, and break/place;
- `rkb-android` host tests that create an engine with no save and assert
  generated terrain, persisted `level.dat`/`rkb-session.json`, and rendered
  faces;
- end-to-end `Runtime<MinecraftGame>` renders for both a synthetic save and a
  fresh generated world.

Synthetic fixtures contain only generated NBT and region bytes. No Mojang data
is stored. CI also compiles both crates for `aarch64-linux-android` in the Rust
and Android workflows.
