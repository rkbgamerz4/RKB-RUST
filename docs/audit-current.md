# Current project audit (pre-migration)

Status: audited on 2026-11-16, at commit `bef6cc1` on
`feature/rkb-rust-foundation`. This document records what exists **now**. It
does not delete, move, or rename anything.

Scope: the whole RKB RUST workspace, its Android host, its CI, and its existing
Minecraft data claims. The point is to separate what can become part of a real
Minecraft: Java Edition client from what is standalone-game scaffolding.

## Repository state

- Git history is intact. Nothing was rewritten or force-pushed.
- Local branches: `feature/rkb-rust-foundation`. `origin/HEAD` points at that
  same branch. **There is no `main` branch in this repository today.** The
  instruction "do not overwrite main" therefore means: do not rewrite the
  existing default/history branch. The migration starts on a separate feature
  branch.
- Working tree was clean at audit time.
- The workspace is `MIT OR Apache-2.0`, so MIT-licensed reference code can be
  used with attribution. No Horizon code has been copied yet.
- `artifacts/horizon/` is a gitignored local reference snapshot. It is not part
  of the shipped tree.

## Workspace inventory

Line counts are Rust lines under each crate's `src/` (including bins). Test
counts are `#[test]` attributes under `src/`; CI additionally runs integration
tests.

| Crate | Lines | Tests | What it actually is | Migration verdict |
| --- | ---: | ---: | --- | --- |
| `rkb-core` | 1,351 | 22 | Math (`Vec2/Vec3/Mat4`), config parser, lifecycle FSM, fixed timestep, frame-rate counter | **Reuse** (generic, dependency-free) |
| `rkb-platform` | 242 | 5 | `Surface`, `FrameClock`, `SystemClock` traits | **Adapt**: keep the host seam; a GPU surface needs a new implementation |
| `rkb-render` | 1,513 | 24 | Deterministic software rasterizer, 2D camera, 3D perspective path, textures, draw lists | **Retain as test oracle**, not as the final renderer |
| `rkb-input` | 786 | 10 | Touch state, tap/drag/pinch, virtual joystick, touch button | **Reuse** the primitives; Minecraft UI/controls are new work |
| `rkb-resource` | 966 | 15 | Traversal-safe asset paths, file/memory sources, `RKBPAK1` archives, LRU cache | **Reuse** path/source/cache; the RKBPAK container is RKB-specific |
| `rkb-runtime` | 506 | 10 | Fixed-step loop, lifecycle, resize, present, frame checksums | **Adapt**: keep timing/lifecycle; the `Game` trait is sandbox-shaped |
| `rkb-sandbox` | 865 | 8 | Ball/joystick sandbox scene and its stats | **Park** (not deleted); not part of the product |
| `rkb-demo` | 186 | 0 | Headless CLI demo of the sandbox stack | **Park**; may remain a CI smoke test |
| `rkb-pak` | 336 | 3 | `RKBPAK1` archive verifier/builder CLI | **Park**; Minecraft resource packs are zip/directory formats |
| `rkb-android` | 908 | 11 | JNI bridge, `AndroidEngine`, direct `ByteBuffer` surface | **Adapt**: keep JNI plumbing, replace the embedded game and add launcher/network/auth surfaces |
| `rkb-mc` | 4,787 | 44 | NBT, Anvil regions, 1.21 chunk sections/palettes, save reader/writer, version descriptors, registry reports, `rkb-world` CLI | **Reuse** — the strongest existing asset |
| `rkb-minecraft` | 5,631 | 50 | Generated-world voxel client, chunk streaming, meshing, procedural atlas, camera/collision/raycast, session persistence | **Partial salvage**; most standalone systems retire |

Total: 12 crates, 18,077 Rust lines under `src/`.

## Capability matrix

| Capability | State | Evidence |
| --- | --- | --- |
| NBT (modified UTF-8) reader/writer | Implemented | `rkb-mc/src/nbt.rs` |
| Anvil `.mca` region reading and compression | Implemented | `rkb-mc/src/region.rs` |
| 1.21/1.21.1 chunk palette decoding | Implemented | `rkb-mc/src/chunk.rs`, DataVersions 3953/3955 |
| Save loading and atomic writing | Implemented | `rkb-mc/src/world.rs`, `save.rs` |
| Block-state registry from a user report | Implemented | `rkb-mc/src/registry.rs` |
| Generated overworld terrain | Implemented, **standalone** | `rkb-minecraft/src/worldgen.rs` |
| Chunk meshing and face culling | Implemented, CPU/software path | `rkb-minecraft/src/mesh.rs` |
| Procedural block textures | Implemented, **not Mojang assets** | `rkb-minecraft/src/textures.rs` |
| First-person camera, AABB collision, raycast | Implemented | `rkb-minecraft/src/{camera,player,interaction}.rs` |
| Touch movement/look and block edit | Implemented | `rkb-minecraft/src/game.rs`, `rkb-android` |
| Android ARM64 APK path | Implemented and CI-verified | `.github/workflows/android.yml` |
| Emulator startup evidence | Implemented and CI-verified | `.github/workflows/android-emulator.yml` |
| Minecraft network protocol | **Absent** | no socket code outside status-less test helpers |
| Microsoft/Xbox authentication | **Absent** | no OAuth/XBL/XSTS code |
| Vanilla server join / configuration / play | **Absent** | — |
| Clientbound chunk and light packets | **Absent** | save format only, not wire format |
| Entities, players, inventory, containers | **Absent** | — |
| Resource-pack decoding (blockstates/models) | **Absent** | procedural atlas only |
| GPU renderer | **Absent** | software rasterizer only |
| Real Minecraft assets | **Absent by design** | none bundled |

Anything not listed above is not implemented, regardless of how the current
README phrases the generated-world milestone.

## Existing claims and their limits

- The current client reads **saved worlds** of Minecraft 1.21 and 1.21.1 only.
  It does not speak their protocol and has never joined a server.
- The generated world is RKB-authored terrain written into real 1.21 chunk
  structures. It is not a Minecraft world and not Minecraft world generation.
- Textures are deterministic RKB-generated 16x16 tiles. They are not Mojang
  assets and do not come from a resource pack.
- Rendering is a CPU software rasterizer presenting RGBA8 into a Kotlin buffer.
  There is no OpenGL ES or Vulkan path.
- Android evidence is emulator-only. No physical ARM64 device, Vulkan driver,
  thermal, or sustained-performance test exists.
- There is no Microsoft account flow, no session server call, no server-list
  ping, and no multiplayer code.

These limits are why the old client cannot be presented as a Minecraft client.

## CI and packaging coupling

| Workflow | What it enforces |
| --- | --- |
| `.github/workflows/ci.yml` | `fmt`, clippy `-D warnings`, 212 workspace tests, release build, demo render, asset-pack verify, Android `cargo check` for `rkb-android`/`rkb-mc` |
| `.github/workflows/android.yml` | NDK ARM64 Gradle build, APK assembly, ABI/version checks, artifact upload |
| `.github/workflows/android-emulator.yml` | API 34 x86_64 emulator, fresh-install and supplied-save smoke tests |

The CI currently builds the **old client**. A migration must keep this green
while new crates are added, then switch the app entry point in a controlled
step. The emulator workflow's world-stat parsing is tied to `rkb-minecraft`
log lines; it will need a parallel smoke path rather than silent breakage.

## Structural risks

1. **Renderer mismatch.** A real client needs GPU terrain rendering at scale.
   The existing software renderer cannot carry that, and Horizon's renderer is
   wgpu/Vulkan with a large asset pipeline. This is the largest single gap.
2. **Version mismatch.** RKB's verified data path is 1.21/1.21.1 (protocol 767,
   DataVersion 3953/3955). Horizon's snapshot is 26.3 (protocol 777,
   DataVersion 5023). Protocol tables, registries, and packet layouts differ.
3. **The `Game` trait is sandbox-shaped.** It assumes draw-list rendering and
   synchronous update/render. A networked client needs a separate network
   thread, a 20 TPS simulation clock, and a render clock.
4. **Android is coupled to `MinecraftGame`.** `rkb-android/src/bridge.rs`
   constructs `Runtime<MinecraftGame>` directly. Launcher, auth, server list,
   and connection screens have no place to live yet.
5. **No real assets pipeline.** Without resource-pack decoding, block models,
   item models, fonts, and sounds cannot match Minecraft. Procedural textures
   are a placeholder.
6. **No verification path for multiplayer.** There is no local vanilla server,
   no legally supplied client files, no Microsoft client ID, and no physical
   device in this environment. Claims must stay explicit about that.

## Preservation rules adopted

- Do not delete, rewrite, or squash existing history.
- Do not modify the existing default branch while the migration is in progress.
- Keep `rkb-minecraft`, `rkb-sandbox`, and `rkb-demo` building until the
  replacement path is verified; then retire them in a separate, reviewable
  commit.
- New work goes on `feature/mc-java-protocol` (or later feature branches).
- No Mojang code, jars, assets, or data are committed.
- No Horizon source is copied without the MIT notice and a recorded provenance
  commit.

## Bottom line

`rkb-mc` is a genuine, tested Minecraft **save-format** library and should be
kept. `rkb-core`, `rkb-platform`, `rkb-input`, `rkb-resource`, and the JNI
plumbing are useful engineering foundations. The visible "game" — generated
terrain, procedural textures, software voxel rendering, sandbox/demo — is a
prototype and must not define the product. The missing product is the network
client: protocol, authentication, server join, client world, entities,
inventory, and a GPU renderer.
