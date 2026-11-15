# Horizon source analysis

This report is based on the source archive supplied with the task, extracted to
`artifacts/horizon/horizon-android-main` (gitignored). That archive is the
Android port of Horizon (`https://github.com/MocLG/horizon-android`), which is
the same Rust game as the upstream project (`https://github.com/horizon-rust/horizon`).
Paths below are relative to the extracted directory. Horizon was **not** executed for this report, and no
Minecraft installation, game jar, or asset bundle was available. Statements
about behaviour therefore come from reading source, documentation, manifests,
and tests; they are not claims that a particular build was run.

No Horizon source code was copied into RKB RUST. The analysis was used for
architectural ideas only. See [`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md).

## What the project is

Horizon is a native Rust client for Minecraft: Java Edition **26.3** (a version
newer than the 1.21 target of RKB RUST). Its README describes it as a complete
vanilla-like client checked by 281 differential harnesses, with Linux, macOS,
Windows, and Android builds. It is not a launcher around the Java client: the
game loop, world, renderer, protocol, and UI are Rust.

Observed source size at the time of inspection:

| Measure | Count |
| --- | ---: |
| Workspace crates | 49 |
| Rust source files under `crates/` | 2,739 |
| `horizon-client` Rust lines | 710,452 |
| `horizon-world` Rust lines | 150,769 |
| `horizon-client-render` Rust lines | 258,658 |
| Files under `tests/` | 104 |

The repository also contains `launcher/`, `android/`, `benchmarks/`, `mods/`,
`packaging/`, `roadmap/`, `third_party/`, `tools/`, and `xtask/`.

## Architecture

### Layers

`docs/layers.md` describes three layers:

1. a **vanilla engine**;
2. a **platform** that makes the vanilla game easier to port, change, and mod;
3. **Horizon's own layer** (cosmetics, shop, telemetry, official servers,
   launcher).

The repository is being split so that the engine and platform can exist
separately from the product layer. `AGENTS.md`, `docs/architecture.md`, and CI
check dependency direction. For RKB the useful idea is the separation itself:
engine/data code must not depend on Android UI or vendor features.

### Crates

Representative crates, grouped by responsibility:

| Group | Crates |
| --- | --- |
| Core and math | `horizon-core`, `horizon-threads`, `horizon-system` |
| World and vanilla data | `horizon-world`, `horizon-items`, `horizon-command`, `horizon-assets`, `horizon-prepare` |
| Client and gameplay | `horizon-client`, `horizon-client-lib`, `horizon-client-core`, `horizon-game` |
| Rendering and UI | `horizon-client-render`, `horizon-shaders`, `horizon-shader-api`, `horizon-theme`, `horizon-hdr` |
| Network | `horizon-protocol`, `horizon-server`, `horizon-relay`, `horizon-friends` |
| Android/platform | `horizon-platform`, `horizon-hooks`, `horizon-mods`, `android/` |
| Product/extras | `horizon-cosmetics`, `horizon-official`, `horizon-telemetry`, `horizon-addresses` |

### Startup and game loop

- `crates/horizon-client/src/main.rs` is a thin executable: it holds the mod API
  marker and calls `horizon_client::entry::main`.
- `crates/horizon-client/src/entry.rs` performs startup: allocator setup, pack
  formats, mod hooks, language files, crash/telemetry wiring, argument parsing.
- `crates/horizon-client/src/game.rs` is the playable game. `Game` is defined
  around line 2127; `Game::tick` around line 5511 and `Game::render` around
  line 9054.
- `crates/horizon-client/src/clock.rs` ports `DeltaTracker.Timer`: integer
  milliseconds become game ticks, with partial ticks, pause, freeze, step, and
  sprint behaviour. This is the 20 ticks/second simulation.
- `crates/horizon-client/src/game/event_loop.rs` isolates what the game needs
  from winit's event loop (`exit`, control flow), so a headless client can run
  its own loop. `src/game.rs` handles `WindowEvent` and `RedrawRequested`.
- `crates/horizon-client/src/game/` contains the gameplay systems: block
  breaking/placing, inventory screens, recipe book, mobs, boats, redstone,
  advancement UI, commands, autosave, and many parity tests.

### World and chunks

- `crates/horizon-world` is the server-side/authoritative world: level data,
  chunk status, worldgen, block behaviour, ticks, fluids, entities, loot,
  datafixers, and structures.
- `crates/horizon-client-render/src/chunk.rs` is the resident client chunk
  store. `LevelShape::OVERWORLD` is `min_y = -64`, `height = 384`, 24 sections.
  Sections are reference-counted (`Arc`) so worker threads can mesh and light
  immutable snapshots while the game thread edits copy-on-write.
- `crates/horizon-core/src/paletted.rs` implements the 26.3
  `PalettedContainer`: single-value, linear, and hash-map palettes, the vanilla
  bit-width thresholds, the global palette, and `SimpleBitStorage` packing where
  values never span longs. It serves both blocks (4096 entries) and biomes (64).
- `crates/horizon-world/src/chunk.rs` implements saved-section palettes and
  storage-level access. `PalettedContainer::from_nbt` validates palette size,
  bit widths, storage length, and indices.
- `crates/horizon-world/src/registry.rs` loads block-state identities from a
  version-pinned registry export. It supplies names, default properties, and
  numeric ids, not block behaviour.
- `crates/horizon-world/src/worldgen/generator.rs` is the density-function
  terrain generator; `worldgen/` also contains structures, jigsaw assembly,
  feature placement, and decoration.

### Rendering

- `crates/horizon-client-render` depends on **wgpu 30.0.1**, with Vulkan, Metal,
  DX12, and GLES feature sets. The repository vendors `third_party/wgpu-hal`
  with upstream fixes for Android/desktop driver problems.
- `gfx.rs` defines `Frame` and `Renderer`; `Renderer::render` submits a frame
  (around line 7959). `terrain.wgsl` is the terrain pipeline; `cull.wgsl`,
  `gpu_culling.rs`, and `gpu_occlusion.wgsl` implement visibility work;
  `chunk.rs`, `chunk_map.rs`, and `server_terrain.rs` hold terrain data.
- `crates/horizon-client-render/src/camera.rs` ports vanilla camera maths:
  `Camera.tick`, field of view, view/hurt bobbing, and JOML-compatible matrix
  operations. Its header carries the JOML MIT notice.
- The renderer is not a thin software fallback. It is a full GPU renderer with
  terrain, entities, block entities, fluids, clouds, sky, HDR, shader packs, and
  post-processing.

### Entities, player, camera, input

- `crates/horizon-client/src/game/entity.rs` defines an `Entity` trait with
  defaults following vanilla `Entity`, plus `EntityHandle` for list/index
  access. Dispatch is static; entity kinds are generated by `entity_lists!`.
- `crates/horizon-client/src/game/mobs.rs`, projectiles, boats, minecarts, TNT,
  item frames, armor stands, and other modules implement concrete kinds.
- `crates/horizon-client/src/motion.rs` stores movement state for living
  entities and binds air, jump, collision, support, restitution, health, powder
  snow, climbing, and fluid modules from `horizon-world`.
- `crates/horizon-client/src/controls.rs` implements touch controls compatible
  with Zalith Launcher 2 layout files: layers of buttons, joysticks, toggles,
  swipes, and pointer look. `controls/default_layout.json` is Zalith's layout
  under GPL-3.0 (see the license section).
- Camera and input are wired through winit events and Android touch/keyboard
  handling (`crates/horizon-client/src/game/android.rs`, `controls.rs`,
  `horizon-client-render/src/android_runtime.rs`).

### Inventory, items, crafting

- `crates/horizon-items/src/items.rs` defines `ItemStack` and item identities.
- `crates/horizon-client/src/crafting.rs` defines `Recipes` and
  `Recipes::craft`; `recipe_book.rs` handles categories, the recipe index, and
  station mapping.
- `crates/horizon-world/src/player_inventory.rs`, `inventory_use.rs`,
  `inventory_pickup.rs`, and `inventory_tick.rs` implement container movement,
  use, pickup, and ticking.
- `crates/horizon-client/src/game/inventory_input.rs` and
  `inventory_slots.rs` connect screens and slots to the game.

### Resources and data

- `crates/horizon-prepare` imports the player's own Minecraft Java installation:
  client jar, libraries, version JSON, server jar, assets, and data. It checks
  artifacts against `research-lock.json` and records SHA-256 hashes in a
  manifest. Vanilla exporters run on a bundled OpenJDK because they call
  Mojang's code.
- `crates/horizon-assets` decodes resource packs: blockstates, models, item
  models, animations, palettes, OBJ models, and fonts.
- `horizon-client/src/server_packs.rs` and `no_resources.rs` handle server
  resource packs and the no-resource path.
- The repository contains no Minecraft assets, jars, or data. Preparation uses
  files the user already owns.

### Version handling

- `crates/horizon-core/src/version.rs` pins the current version:
  `VERSION_ID`/`VANILLA_VERSION` 26.3, protocol 777, DataVersion 5023,
  resource pack format 97.1, data pack format 121.0, Java 25.
- `crates/horizon-protocol/src/adapter.rs` defines `ServerVersion` and translates
  older servers to the current protocol. Per-version modules exist for
  1.21/1.21.1, 1.21.3, 1.21.4, 1.21.5, 1.21.6, 1.21.8, 1.21.10, 1.21.11,
  26.1, and 26.2, with `*_ids` tables where registries differ.
- `crates/horizon-world/src/datafix` upgrades older saved data.
- The pattern is explicit: one version descriptor, its own packet/registry
  tables, and a translation step. It does not assume every minor release is
  identical.

### Android integration

`docs/android.md` describes the port:

- Android 10+ and Vulkan are required.
- The app is a NativeActivity; the Java side reaches only clipboard, links,
  soft keyboard, and the back gesture.
- The launcher and game share one winit event loop because Android permits one
  per process.
- Touch controls follow Zalith Launcher 2 layouts, including editing, importing,
  exporting, opacity/size, gyroscope look, and gamepad support.
- Minecraft is downloaded by the authenticated launcher from Mojang, or copied
  from the user's own installation. Preparation exporters run on the bundled
  OpenJDK 25 runtime.
- Nothing of Mojang's ships in the APK.

## License findings

Horizon's `LICENSE` is the MIT License:

> Copyright (c) 2026 Ludovic Marie and the Horizon contributors

The full MIT text is reproduced in [`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md).
MIT permits use, modification, and redistribution provided the copyright
notice and permission notice are included in copies or substantial portions.
There is no warranty.

`THIRD_PARTY_NOTICES.md` in Horizon lists:

| Component | Where | License |
| --- | --- | --- |
| wgpu-hal 30.0.1 (with one upstream fix) | `third_party/wgpu-hal` | MIT or Apache-2.0 |
| Pixelify Sans | `crates/horizon-theme/fonts/` | SIL OFL 1.1 |
| Pixelarticons | `crates/horizon-theme/icons/pixel/` | MIT |
| Zalith Launcher 2 default layout | `crates/horizon-client/src/controls/default_layout.json` | GPL-3.0 |
| OpenJDK 25 Android runtime | `android/build.sh` | GPL-2.0 with Classpath Exception |

Consequences for RKB RUST:

- Horizon's own Rust code could be reused under MIT with attribution, but RKB
  deliberately implements its own code. Nothing was copied, so no Horizon
  notice is legally required; this report and the notices file still credit the
  project.
- The Zalith layout JSON is GPL-3.0. RKB must not copy it. RKB's own touch
  layout will be original.
- The bundled OpenJDK is GPL-2.0+CE. RKB does not bundle a Java runtime.
- Minecraft code, assets, and data are Mojang's. RKB includes none and reads only
  files the user supplies.
- Horizon's third-party font/icon licenses do not apply to RKB unless those
  assets are copied, which they are not.

## Implemented, documented, and still needed

### Present in the inspected source

These are backed by source files and, in many areas, tests:

- 20 TPS timer, winit event loop, `Game::tick`/`Game::render`.
- Worldgen, chunk status, block behaviour, fluids, redstone, ticks, structures,
  loot, datafixers.
- Paletted block/biome containers with vanilla packing rules.
- Version-pinned block-state registry import.
- wgpu renderer with terrain, entities, block entities, sky, fluids, clouds,
  HDR, shader packs, and GPU culling.
- Trait-based entity system and many concrete entity kinds.
- Player motion, collisions, fluids, climbing, fall damage, attributes.
- Inventory, containers, item use, crafting, recipe book.
- Resource-pack decoding and user-file preparation.
- Protocol implementation with per-version adapters for older servers.
- Android NativeActivity port with touch layouts, lifecycle, and gamepad.

### Claimed by the project, not verified here

- “100% vanilla parity” and “281 differential harnesses” (README badges and
  text).
- Behavioural parity of the worldgen, redstone, mob AI, and rendering against
  vanilla.
- Android device behaviour, performance, and driver compatibility.
- That every per-version adapter is complete for its release.

These claims may be true, but this report did not run Horizon or compare its
output with Minecraft.

### Still needed by RKB RUST

RKB is not a port of Horizon. The following remain RKB work:

- A 1.21-specific data reader (first milestone, now implemented).
- Resource-pack and data-pack loading with versioned pack formats.
- Terrain generation, lighting, and rendering compatible with 1.21.
- Player physics, camera, block interaction, inventory, crafting, mobs, and
  survival.
- A GPU backend integrated behind RKB's `RenderBackend`, with tests. No GPU
  backend is claimed yet.
- Android touch UI wired to Minecraft gameplay rather than the sandbox.
- Per-version registries/datafixers for 1.21.x releases beyond 1.21.1.
- Multiplayer protocol support. RKB currently has no protocol implementation.
- Physical-device testing. RKB's Android evidence is emulator-only.

## Reusable ideas (not code)

1. **Pin a version by DataVersion and protocol.** Version descriptors with
   independent registry/packet tables prevent silent cross-version assumptions.
2. **Validate every saved palette.** Palette size, derived bit width, storage
   length, and index range are all checkable before exposing blocks.
3. **Non-spanning packed values.** Matching `SimpleBitStorage` avoids subtle
   decoding differences at long boundaries.
4. **Immutable section snapshots.** `Arc` sections let meshing/lighting run off
   the game thread while edits copy-on-write.
5. **Keep data preparation outside the engine.** Horizon's exporter model keeps
   Mojang code and user files separate from the shipped binary. RKB should keep
   the same boundary without bundling a Java runtime.
6. **Version-specific folders/modules.** A `v1_21_1` module with its own ids and
   translation tests is clearer than scattered `if version >= ...` checks.
7. **Differential fixtures over hand-written expectations.** Synthetic saves
   plus malformed-input tests catch format regressions cheaply. RKB starts with
   synthetic saves and can add user-supplied fixtures later.
8. **A narrow platform seam.** Horizon's event-loop trait and RKB's
   `Surface`/`RenderBackend` both keep the host replaceable.

## What RKB does differently

- Android ARM64 and low/mid-range devices are the primary target, not desktop
  parity first.
- RKB keeps its tested software renderer until a GPU backend is integrated and
  tested; no unverified GPU path is claimed.
- RKB's first Minecraft subsystem is saved-world data, not a demo scene.
- RKB does not bundle or download Minecraft, a Java runtime, or third-party
  assets.
- RKB marks only 1.21 and 1.21.1 as verified. Other versions must earn their own
  descriptor and tests.
