# Minecraft: Java Edition 1.21 data support

RKB RUST's first Minecraft subsystem reads saved Java Edition 1.21 world data.
It is implemented in `crates/rkb-mc` and exposed through the `rkb-world` tool.
It reads only files supplied by the user; no Minecraft code, assets, or data are
bundled.

## Verified versions

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
- `Section` palette/storage parsing and local block lookup.
- `Chunk::parse`, section iteration, world-Y lookup, non-air count.
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
  descriptors bounded and is adequate for inspection. A gameplay renderer will
  need a region cache and asynchronous loading.

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

Done: on-demand region/chunk reads, section lookup, chunk identity validation.

Next: async/background loading, a bounded region/chunk cache, biomes,
heightmaps, block entities, entities, structures, and save writing. Datafixers
are required before opening worlds older than the supported DataVersion.

### 3. Block states

Done: names, properties, palette decoding, air detection, registry lookup.

Next: block shapes and collision boxes, default-state resolution per version,
property enums/integers, and block behaviour tables.

### 4. Terrain rendering

Not implemented for Minecraft. The existing RKB software renderer can draw the
sandbox, but there is no chunk mesher, texture atlas, or Minecraft terrain path
yet. The render seam is `DrawList`/`RenderBackend`; a GPU backend must not be
claimed until it is integrated and tested.

Next: section meshing, face culling, texture atlas from a user's resource pack,
lightmap sampling, frustum culling, and memory budgets for low/mid-range phones.

### 5. Player movement and camera

Not implemented. Horizon's movement modules are an architectural reference, not
copied code.

Next: 1.21 movement constants, AABB collision, step-up, swimming, ladders,
sprinting, sneaking, jump cooldown, and a first-person camera with touch look.

### 6. Block breaking and placing

Not implemented.

Next: reach/raycast, block hardness and tool rules, break progress, placement
rules, block updates, drops, and creative/survival differences.

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

## Limitations of the first milestone

- Read-only save inspection. `RegionWriter` exists for synthetic fixtures and
  tools, not a general save writer.
- no LZ4 region compression;
- no datafixers, so only DataVersions 3953 and 3955 are accepted;
- biomes, heightmaps, block entities, entities, structures, and lighting are
  ignored;
- the registry is not bundled and is only used by the `registry` command, not
  yet to validate every palette entry;
- no resource packs, terrain rendering, gameplay, or Android gameplay
  integration;
- no physical Android device was available. The Android evidence for this
  milestone is an ARM64 `cargo check` plus the existing emulator startup smoke
  test, not Minecraft rendering on hardware.

## Tests

`cargo test -p rkb-mc --all-targets --locked` runs:

- 41 unit tests inside the library;
- 6 CLI integration tests that build synthetic saves and run the binary.

Synthetic fixtures contain only generated NBT and region bytes. No Mojang data
is stored. CI also compiles `rkb-mc` for `aarch64-linux-android` in both the Rust
and Android workflows.
