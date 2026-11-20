# Minecraft Java Edition 1.21.1 Compatibility Matrix

**Target**: Minecraft Java Edition 1.21.1 · Protocol 767 · DataVersion 3955
**Client**: RKB-RUST (Rust, Android ARM64 primary)
**Verified baseline commit**: `f23b6d9` on `feature/mc-java-protocol`
**Date**: 2026-11-20

## How to read this matrix

| Mark | Meaning |
|------|---------|
| ✅ **Implemented** | Code exists and has passing tests |
| 🟡 **Partial** | Some sub-features exist; gaps listed |
| ❌ **Not implemented** | No code path |
| ⬜ **Not tested** | Code may exist but has no verification |

A feature is only counted as complete when implemented **and** tested.

---

## 0. Repository reality check (verified 2026-11-20)

17 crates, ~38,000 lines of Rust:

| Crate | LoC | Role |
|-------|-----|------|
| `rkb-minecraft` | 10386 | Game client: mesher, player physics, touch UI, world source |
| `rkb-proto` | 7492 | Protocol 767 codec, packet structs, ids |
| `rkb-net` | 5244 | Login/configuration/play sessions, `ServerWorld`, entities |
| `rkb-auth` | 3671 | Microsoft OAuth device flow, Xbox Live, MC token |
| `rkb-gpu` | 3235 | wgpu renderer (Vulkan + GLES), WGSL, scene sync |
| `rkb-mc` | 4828 | Block registry, chunk/region/save/NBT, world reader |
| `rkb-render` | 1721 | M6 software rasterizer (reference + fallback) |
| `rkb-android` | 1621 | JNI bridge, surface lifecycle, GPU/software loop |
| `rkb-resource` | 966 | Asset archive/manager with path-traversal protection |
| `rkb-core` | 1351 | Vec/Rect/Color/DrawList primitives |
| `rkb-input` | 786 | Touch events, joystick, buttons, gestures |
| `rkb-runtime` | 577 | Fixed-step game loop |
| `rkb-sandbox` | 865 | M1–M5 demo sandbox (preserved, not the product) |
| `rkb-demo` | 186 | Demo binary |
| `rkb-pak` | 336 | Pack archive tool |
| `rkb-platform` | 242 | Platform abstractions |

**Test baseline**: 453 Rust tests pass / 0 fail (35 suites); 25 GPU tests pass
against Mesa lavapipe; clippy `-D warnings` clean; `cargo fmt --check` clean.

---

## 1. Blocks (mission §3)

| Sub-system | Status | Evidence / gap |
|------------|--------|----------------|
| Block-state registry from Mojang `blocks.json` report | ✅ | `rkb-mc/src/registry.rs`; 26684 states parsed in live runs |
| Numeric state id ↔ `BlockState` mapping | ✅ | `BlockRegistry::from_file`, validated |
| Default state per block | ✅ | `defaults: BTreeMap<String,u32>` |
| Block properties (axis, facing, half, …) | 🟡 | Parsed into `BlockState`; only a subset consumed by the mesher |
| Block models (cube, slab, stairs, fence, door, …) | 🟡 | `BuiltinModels` covers common families; resource-pack models via `PackModels` |
| Collision shapes | ❌ | Player collision treats blocks as full cubes (`is_solid_block`) |
| Selection shapes | ❌ | Same as above; raycast uses `!is_air()` |
| Transparent / translucent / cutout render layers | ✅ | `RenderLayer::{Opaque,Cutout,Translucent}` in mesher + GPU pipelines |
| Animated textures | ❌ | Atlas is static; no per-frame UV animation |
| Connected textures | ❌ | Not implemented |
| Block light emission | ❌ | No emission table; lighting comes from server `UpdateLight` |
| Block opacity | ❌ | No opacity table |
| Hardness / resistance / tool requirements | ❌ | No mining-speed model; break is instant |
| Block sounds | ❌ | No audio system |
| Water / lava | 🟡 | Rendered as translucent; no fluid physics or swimming |
| Gravity blocks (sand, gravel, anvils) | ❌ | No falling-block entities |
| Redstone blocks | 🟡 | States arrive and render as blocks; no special models |
| Interactive blocks (doors, chests, furnaces) | 🟡 | Models exist for some; no interaction packets |
| Containers / signs / beds | ❌ | No block-entity decoding (only count in stats) |

**Verdict**: data-driven registry ✅, but only a fraction of vanilla block
behaviour is modelled. Mining, fluids, redstone, block entities are absent.

---

## 2. Items (mission §4)

| Sub-system | Status | Evidence / gap |
|------------|--------|----------------|
| Item registry | ❌ | No item registry; only block-derived items exist |
| Stack counts / max stack size | ❌ | — |
| Durability | ❌ | — |
| Tools / weapons / armor | ❌ | — |
| Food / potions / buckets / spawn eggs | ❌ | — |
| Item attributes / use behaviour | ❌ | — |
| Slot decoding (`Slot` struct) | 🟡 | `rkb-net` stores slots but **item components are not decoded** (documented limitation) |

**Verdict**: effectively not started. Slots exist as opaque containers.

---

## 3. Inventory & hotbar (mission §5)

| Sub-system | Status | Evidence / gap |
|------------|--------|----------------|
| Hotbar (9 slots) | ❌ | No hotbar UI |
| Main inventory (27 slots) | ❌ | No inventory UI |
| Armor slots | ❌ | — |
| Offhand slot | ❌ | — |
| Held-item selection | 🟡 | Server `HeldItemSlot` decoded into `PlayerState.held_slot`; no client send |
| Window 0 sync | 🟡 | `WindowItems` decoded into `player.inventory` |
| Single slot sync | 🟡 | `SetSlot` applied for window 0 |
| Item movement / drag / shift-click | ❌ | — |
| Container screens (chest, furnace, crafting) | ❌ | Other windows stored as raw `Vec<Slot>` |
| Creative inventory | ❌ | — |
| Protocol packets for inventory actions | ❌ | No click-window / close-window encode |

**Verdict**: server→client slot storage only. No usable inventory.

---

## 4. Crafting (mission §6)

❌ **Not implemented.** No recipe structures, no recipe book packets, no
crafting window handling. Server recipe data is never requested or decoded.

---

## 5. Entities & mobs (mission §7)

| Sub-system | Status | Evidence / gap |
|------------|--------|----------------|
| Entity spawn tracking | 🟡 | `SpawnEntity` decoded to `EntityState {id,kind,x,y,z,yaw,pitch}` |
| Relative move | 🟡 | `RelEntityMove` applied |
| Move+look | 🟡 | `EntityMoveLook` applied |
| Look only | 🟡 | `EntityLook` applied |
| Teleport | 🟡 | `EntityTeleport` applied |
| Destroy | 🟡 | `EntityDestroy` removes |
| Entity metadata | ❌ | Packet not handled |
| Entity animation / equipment | ❌ | — |
| Entity interpolation | ❌ | Positions apply instantly |
| Hitboxes / interaction | ❌ | — |
| Entity rendering | ❌ | Renderer draws chunks only |
| Entity registry (all vanilla mobs) | ❌ | Only a numeric `kind` is stored |
| Passive / hostile / boss behaviour | ❌ | — |
| Projectiles, boats, minecarts, item entities | ❌ | — |
| Paintings / item frames / armor stands | ❌ | — |

**Verdict**: entity *tracking* exists at protocol level; entity *rendering and
behaviour* do not exist.

---

## 6. World & dimensions (mission §8)

| Sub-system | Status | Evidence / gap |
|------------|--------|----------------|
| Overworld chunks | ✅ | Live-verified: 45 chunks, 19 622 tris rendered from a real server |
| Chunk load / unload | ✅ | `ChunkData` + `UnloadChunk` handled |
| Chunk section updates | ✅ | `SectionBlocksUpdate` (0x49) decode/encode, live-edit proven |
| Biome data | 🟡 | Stored in chunk columns; not surfaced to rendering |
| Block light | ✅ | `UpdateLight` decoded into per-column `ChunkLight` |
| Sky light | ✅ | Same packet |
| Height / min-y per dimension | 🟡 | `DimensionType.min_y` decoded and used for light offsets |
| Nether / End | 🟡 | `Respawn` clears state and re-reads dimension; rendering is dimension-agnostic |
| Time / day-night | 🟡 | `UpdateTime` decoded into event; sky colour stays fixed |
| Weather | ❌ | No `GameStateChange` weather application to rendering |
| World border | ✅ | Client clamps position to ±29 999 984 |
| Dimension change / respawn | 🟡 | Handled as a reset; no loading screen |
| Procedural terrain substitution | ✅ **never** | Server worlds only; missing columns are never fabricated |

**Verdict**: real server world streaming works end-to-end. Environmental
presentation (weather, day/night sky) is minimal.

---

## 7. Player & survival (mission §9)

| Sub-system | Status | Evidence / gap |
|------------|--------|----------------|
| Walking / sprinting | ✅ | `WALK_SPEED`, sprint ×1.3, joystick driven |
| Sneaking | 🟡 | Button toggles local state; **no sneak packet sent** |
| Jumping / falling / gravity | ✅ | `JUMP_VELOCITY`, `GRAVITY`, terminal velocity; unit-tested |
| Collision (AABB vs blocks) | ✅ | Axis-separated with sub-stepping; unit-tested (walls stop player) |
| Ground detection | ✅ | `on_ground` tracked |
| Swimming / climbing | ❌ | — |
| Flying (creative) | ✅ | Toggle + ascend/descend |
| Health | 🟡 | `UpdateHealth` decoded into `PlayerState`; no HUD, no damage logic |
| Damage / death / respawn | ❌ | Client-side death not handled; server `Respawn` resets world state |
| Hunger / saturation | 🟡 | Decoded with health; no UI or depletion |
| Experience / levels | 🟡 | `Experience` decoded; no HUD |
| Armor / equipment | ❌ | — |
| Status effects | ❌ | — |
| Food consumption / item use | ❌ | — |
| Fall damage | ❌ | — |
| Server position correction | ✅ | `SyncPlayerPosition` applied in `poll_server()` |
| Teleport confirmation | ✅ | `TeleportConfirm` auto-sent |
| Keepalive reply | ✅ | Auto-sent by `PlaySession` |

**Verdict**: movement and networking are real and tested. Survival mechanics
(health/hunger/death/effects) are decoded but not played.

---

## 8. Redstone & interactive blocks (mission §10)

🟡 **Display-only.** Block states arrive via `BlockChange` /
`SectionBlocksUpdate` and are rendered with their models. There is no
client-side redstone simulation (correct — the server is authoritative), but
also no special rendering for powered states, no piston extension animation,
no note-block/lever/button models beyond the generic set, and no interaction
packets.

---

## 9. Resource & asset pipeline (mission §11)

| Sub-system | Status | Evidence / gap |
|------------|--------|----------------|
| Resource pack open (dir / zip) | ✅ | `rkb-minecraft/src/pack/*` |
| Block model parsing (JSON) | ✅ | `PackModels` |
| Texture atlas build | ✅ | `pack::Atlas`; 128×64 procedural default |
| Path traversal protection | ✅ | `rkb-resource/src/path.rs` |
| Archive read/write | ✅ | `rkb-resource` `PakArchive`/`PakWriter` |
| GUI textures / fonts | ❌ | HUD is drawn with primitive shapes |
| Sounds / particles | ❌ | — |
| Resource-pack selection UI | ❌ | Pack path passed from host |
| Animation metadata | ❌ | — |

**Verdict**: pack *loading* works; pack *presentation* (GUI, sounds, particles)
does not.

---

## 10. Rendering & performance (mission §12)

| Sub-system | Status | Evidence / gap |
|------------|--------|----------------|
| Vulkan backend | ✅ | wgpu `Backends::VULKAN`, live on lavapipe |
| OpenGL ES fallback | ✅ | `Backends::GL` in `default_backends()` |
| Texture atlas | ✅ | Uploaded as one `Rgba8Unorm` texture |
| Chunk mesh caching | ✅ | Revision-based `SceneSync`; only changed meshes upload |
| Frustum culling | ✅ | Per-chunk AABB test, unit-tested |
| Face culling | ✅ | Hidden-face removal in mesher; backface cull in pipelines |
| Opaque / cutout / translucent pipelines | ✅ | Three separate pipelines |
| Alpha blending | ✅ | Translucent pass, depth-write off |
| Depth buffer | ✅ | `Depth32Float` |
| Fog | ✅ | Distance fog in shader |
| Per-vertex light + AO | ✅ | From server light data |
| Entity rendering | ❌ | — |
| Animated textures | ❌ | — |
| Particles | ❌ | — |
| Shadows | ❌ | — |
| First-person hand / held item | ❌ | — |
| Third-person camera | ❌ | — |
| GUI rendering (beyond HUD shapes) | 🟡 | Crosshair, joystick, buttons drawn as 2D shapes |
| **Android hardware GPU** | ❌ | Only Mesa llvmpipe reachable from Termux; vendor Mali ICD blocked |

**Measured performance** (mt6768, Android 14, Termux, llvmpipe, 640×360):
init 698.6 ms · 213.66 ms/frame wall incl. readback (~4.7 fps equivalent) ·
peak RSS 203 756 KiB · GPU buffers 3 932 216 B · 49 uploads / 34.99 ms total.

These are **software-renderer numbers**. No hardware-GPU claim is made.

---

## 11. Sound (mission §13)

❌ **Not implemented.** No audio backend, no sound event handling, no asset
decoding for audio.

---

## 12. Multiplayer & authentication (mission §14)

| Sub-system | Status | Evidence / gap |
|------------|--------|----------------|
| Server list UI | ❌ | Join via dialog with manual host entry |
| Server status / ping protocol | ✅ | `rkb-proto` status packets + `bin/rkb-ping.rs` |
| Ping wired into app | ❌ | Standalone binary only |
| Offline-mode login | ✅ | Live-verified against local 1.21.1 server |
| Encryption | ✅ | `rkb-proto/src/crypto` |
| Compression | ✅ | `rkb-proto/src/codec` |
| Configuration state | ✅ | `rkb-proto/src/packets/configuration.rs` |
| Play state | ✅ | `rkb-net/src/play.rs` |
| Keepalive / teleport confirm | ✅ | Auto-handled |
| Player movement packets | 🟡 | **Client never sends position/look updates** — see §14 gap below |
| Block interaction packets | ❌ | Break/place verified via RCON, not via client packets |
| Inventory sync | 🟡 | Receive-only |
| Entity sync | 🟡 | Receive-only, positions only |
| Disconnect handling | ✅ | `KickDisconnect`, `Closed`, `Disconnected` events |
| Microsoft auth implementation | ✅ | `rkb-auth`: device-code OAuth, Xbox Live, MC token, entitlements |
| Microsoft auth wired to client | ❌ | Standalone crate; Android join dialog takes a name only |
| Online-mode join verified | ❌ | Never tested against an online-mode server |
| Secure token storage | ✅ | `rkb-auth/src/secret.rs`, `store.rs` (no plaintext passwords) |

**Critical gap**: the client is **receive-only for player movement**. It
applies server positions but never transmits its own. A vanilla server will
therefore see a stationary player that rubber-bands back. This is the single
most important M9/M10 gameplay fix.

---

## 13. Android UX (mission §15)

| Sub-system | Status | Evidence / gap |
|------------|--------|----------------|
| Landscape orientation | ✅ | `android:screenOrientation="sensorLandscape"` |
| Main menu | ❌ | App opens straight into the world view |
| Singleplayer / multiplayer entry | 🟡 | Join dialog only |
| World loading screen | ❌ | — |
| HUD | 🟡 | Crosshair + text overlay |
| Hotbar | ❌ | — |
| Inventory screen | ❌ | — |
| Pause menu | ❌ | — |
| Settings (video/audio/controls) | ❌ | — |
| Touch controls | ✅ | Joystick, jump, sneak, sprint, fly, descend, break, place |
| Customizable control layout | ❌ | Fixed `layout()` geometry |
| Sensitivity settings | ❌ | Hard-coded 0.0032 rad/px |
| Non-blocking UI thread | ✅ | Touch queue drained on render thread |
| JNI safety | ✅ | `native_attach_surface` / `detach` lifecycle, contract tests |

---

## 14. Master milestone plan

| Milestone | Scope | Depends on |
|-----------|-------|-----------|
| **M9** | Android APK + hardware rendering verification | — |
| **M10** | Complete block & item registries | — |
| **M11** | Resource assets, models, textures, animations | M10 |
| **M12** | Inventory, hotbar, containers, crafting | M10 |
| **M13** | Entity & mob rendering | M11 |
| **M14** | Survival gameplay, interactions, effects | M12 |
| **M15** | World, dimensions, lighting, environment | M13 |
| **M16** | Redstone & interactive blocks | M12 |
| **M17** | Multiplayer & online authentication | M12 |
| **M18** | GUI, audio, settings, polish | M14 |
| **M19** | Comprehensive 1.21.1 compatibility testing | all |

### Immediate priorities found by this audit

1. **Send player movement to the server** (protocol gap, blocks real play).
2. **Item registry + slot components** (unblocks inventory and crafting).
3. **Inventory UI + click packets** (unblocks survival).
4. **Hardware GPU verification path** (currently impossible from Termux).

---

## 15. Honest summary

**Fully working and tested**: protocol 767 login/configuration/play against a
real offline 1.21.1 server; chunk streaming, lighting, block updates;
wgpu Vulkan rendering with three pipelines; player movement physics with
collision; touch controls; resource pack loading; Microsoft auth library.

**Partially working**: entities (tracked, not rendered), inventory (received,
not usable), survival stats (decoded, not played), dimensions (reset, not
presented), resource packs (loaded, not fully presented).

**Not working**: items, crafting, entity rendering/behaviour, audio, most
block behaviours, redstone presentation, hotbar/inventory/container UI,
settings UI, server list UI, online-mode join, hardware GPU proof.

**Explicitly preserved**: M1–M8 functionality, existing architecture, all
branches, `main` untouched, no proprietary Minecraft assets or source copied.