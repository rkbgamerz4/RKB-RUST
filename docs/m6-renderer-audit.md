# M6 renderer audit: why the current frames do not look like Minecraft

Status: **audited 2026-11-17 at commit `0f904b2` on
`feature/mc-java-protocol`.** This document describes the code that actually
exists before any M6 change. It is a diagnosis, not a claim that the current
renderer is close to Minecraft.

The evidence policy is unchanged: "implemented" means code plus tests;
"verified" means observed against the named external system. No screenshot in
this repository is a vanilla-client screenshot, and no Mojang texture, model, or
asset is bundled.

## 1. What the current client is

| Layer | Implementation today |
| --- | --- |
| Backend | Deterministic CPU rasterizer in `crates/rkb-render` (`SoftwareBackend`), RGBA8 framebuffer plus a reciprocal-depth buffer. No GPU path. |
| Runtime | `rkb-runtime` drives `MinecraftGame::update` and `MinecraftGame::render` into a `DrawList`. |
| Android surface | `rkb-android` copies the software framebuffer into a direct `ByteBuffer`; Kotlin draws a `Bitmap` at `RENDER_SCALE = 0.6` and lets the compositor scale it up. |
| World | `WorldState::{Save, Generated, Server}`. The M4/M5 server path is `WorldState::Server(rkb_net::ServerWorld)`. |
| Chunk shape | `rkb_mc::Chunk` (save-shaped, expanded palette per section). Network columns are expanded by `rkb_net::server::chunk_from_column`. |
| Mesher | `crates/rkb-minecraft/src/mesh.rs`: one `Face` list per chunk column, full unit cubes only. |
| Textures | `crates/rkb-minecraft/src/textures.rs`: original procedural 16x16 tiles packed into one 128x64 atlas. |
| Camera | `crates/rkb-minecraft/src/camera.rs`: right-handed first-person perspective, 70 degree vertical FOV, near 0.05, far 512, near-plane clipping, frustum AABB test. |
| Lighting | Fixed face-direction shade multiplied by per-vertex ambient occlusion. No world light is sampled. |
| HUD | Crosshair, virtual joystick, jump/fly/descend/break/place buttons, status overlay. |

The server path itself is genuinely server-driven: `rkb-join` exits non-zero if
no server geometry was rendered, and the M4 evidence shows grass and a live
`/setblock` glowstone from the official 1.21.1 server. M6 does not replace that
data. It changes how that data is turned into geometry, textures, light, and
pixels.

## 2. The exact reasons current frames look unlike Minecraft

These are ordered by visual impact.

### 2.1 Textures are invented, not Minecraft textures

`textures.rs` generates every texel from a deterministic hash and a small set of
colors. The module says so explicitly: "no resource-pack loading", "fast graphics
look". The 24-tile atlas is 128x64 with 16x16 cells.

Consequences:

- Grass, stone, dirt, logs, leaves, water, and every other block are colored
  noise patterns, not the game's textures.
- Only about 40 block names have a dedicated tile
  (`textures.rs::block_tile`). All remaining 1.21.1 blocks use the neutral
  `Tile::Unknown` checker with a flat average-color tint.
- There is no resource-pack path, no `blockstates/*.json`, no `models/*.json`,
  no per-face texture references, no animation metadata, and no reload.
- UVs are a fixed axis mapping, not the model's `uv` rectangle. Any non-cube
  model or rotated texture would be wrong even if it were loaded.

This alone makes a "correct" cube look like a different game.

### 2.2 Every block is a full cube

`mesh.rs::emit_face` unconditionally emits the six unit-cube faces. There is no
shape abstraction. Therefore:

- Slabs, stairs, fences, walls, snow layers, carpets, buttons, torches, plants,
  rails, doors, trapdoors, and fluids all occupy the full 1x1x1 cell.
- Grass plants and flowers are solid cubes, not crossed quads.
- Water renders as an opaque full cube; there is no fluid surface height.
- Fences and walls have no posts or connections.

The renderer has no notion of an element box, so adding slabs or stairs today
would require editing the mesher's six-face loop.

### 2.3 Face culling treats every non-air neighbor as opaque

`mesh.rs::is_air_at` returns `false` for any non-air state. `emit_face` then
skips the face. This is a full-cube opaque rule applied to everything:

- Leaves cull the faces of adjacent logs and terrain, so you can see through
  the world where a leaf block sits.
- Glass, water, ice, and other transparent blocks hide the block behind them.
- A slab or stair adjacent to a full block would cull the full block's face
  even though the partial shape does not cover it.
- Two adjacent leaves cull each other, producing holes.

In Minecraft, culling must consider the neighbor's render shape, its material
(cutout/translucent), and whether the face is actually covered.

### 2.4 Lighting ignores real chunk light

The M3 play session decodes `LightData` (sky and block nibble arrays and masks)
but `PlaySession::store_chunk` stores only the `ChunkColumn`. `ServerWorld`
converts only block states. `ChunkSource` has no light query, so the mesher can
only compute:

```text
vertex light = face_direction_shade * ambient_occlusion
```

with `ambient_occlusion` in `{0.4, 0.6, 0.8, 1.0}`. That means:

- Caves and roofed areas are as bright as open sky.
- Glowstone, lava, torches, and other light sources do not illuminate nearby
  faces.
- Night/day and dimension sky-light factors do not affect terrain.
- Smooth lighting is not interpolated from neighboring blocks; only the AO
  term varies per corner.
- AO treats leaves, glass, water, and plants as full occluders.

The renderer does have a perspective-correct per-vertex light path and a
depth-tested rasterizer, so the plumbing exists; the data is simply not fed in.

### 2.5 There is no transparency or render-layer handling

`rkb-render::renderer::rasterize_triangle` writes alpha 255 and only discards a
texel when `texel.a == 0`. There is no cutout threshold, no alpha blending, no
depth-write policy for translucent geometry, and no back-to-front sorting.

`Mesh` stores one face list. `game.rs::draw_scene` draws faces in map/insertion
order. Even if water and glass had alpha, they would be composited in arbitrary
order and could write depth in front of geometry drawn later.

### 2.6 UV orientation is only approximately right

`mesh.rs` maps side faces with `v = 1 - y` and top/bottom faces with `v = z`.
This keeps side textures upright, but:

- Positive-X and positive-Z faces are mirrored relative to Minecraft's default
  model UV orientation.
- There is no per-face `uv` rectangle, `rotation`, `uvlock`, or model
  `x`/`y` rotation.
- There is no support for `cullface` metadata or `tintindex`.

### 2.7 Atlas filtering is a single nearest-neighbour sample

`BlockAtlas` uses half-texel inset UVs and nearest sampling. That avoids
bleeding but produces strong shimmering and aliasing at distance, and there are
no mipmaps. Minecraft's default uses mipmapped atlas sampling with a cutout
threshold. A real pack at 32x or 64x will look harsher than the game.

### 2.8 Fog and sky are fixed

`game.rs` has `FOG_START = 36`, `FOG_END = 64`, and a fixed two-stop sky
gradient. Fog always blends to `SKY_HORIZON` regardless of dimension, time, or
biome. Because the load radius is small, terrain fades into a flat blue wall
instead of matching the horizon.

There is no time-of-day packet handling in the renderer, no biome tint, and no
underwater or lava fog.

### 2.9 Android renders at 0.6x and upscales

`MainActivity.kt` uses `RENDER_SCALE = 0.6f`, and `RkbView` draws the bitmap
scaled to the surface. This is a reasonable performance choice for a CPU
rasterizer, but it softens texels and makes an already approximate atlas look
further from Minecraft. It is not a correctness bug and is preserved in M6
unless a measured reason to change it appears.

### 2.10 There is no GPU backend

`crates/rkb-render` is a software rasterizer. `rkb-android` presents its pixels.
There is no OpenGL ES, Vulkan, or wgpu surface integration, no shader
compilation, and no GPU resource management. Any claim of GPU rendering would be
false today.

## 3. What already works and must be preserved

- M4 server join, chunk conversion, block updates, unloads, teleport handling,
  and the "missing state id is an error" rule.
- M5 background Android join, render-thread promotion, JNI contract, and
  `INTERNET` permission.
- Perspective projection, near-plane clipping, reciprocal depth, frustum
  culling, and the textured rasterizer's perspective-correct UV and light
  interpolation.
- Chunk neighbourhood culling across chunk boundaries for full cubes.
- Mesh rebuild queue, dirty-chunk handling, and unload removal.
- Touch controls, crosshair, joystick, and status overlay.
- Deterministic procedural atlas as a test/demo fallback.

## 4. Asset and licence constraints

- No Mojang jar, asset bundle, texture, model, blockstate, or font may be
  committed or redistributed by RKB.
- The product path must load a **user-supplied** resource pack directory (or
  pack archive) in the vanilla layout:
  `assets/minecraft/blockstates/`, `assets/minecraft/models/`,
  `assets/minecraft/textures/`, `pack.mcmeta`.
- Tests must use self-authored PNG/JSON fixtures with an explicit licence note.
- The procedural atlas remains an explicitly labelled fallback when no pack is
  supplied. It is never presented as Mojang art.
- Any new dependency must be compatible with `MIT OR Apache-2.0` and MSRV 1.77.

## 5. Verification constraints discovered during the audit

- A local official 1.21.1 server is available on `127.0.0.1:25575` (offline
  mode, flat world, view distance 3). It is the only real-server target.
- No vanilla client resource pack is available locally. Verification must use a
  synthetic test pack whose every byte is authored in this repository, plus the
  procedural fallback.
- No physical Android device is attached. Emulator/CI evidence cannot prove
  device GPU or performance behavior.
- The software renderer can be captured headlessly to PPM. Those frames must be
  labelled "headless software render", not "physical device screenshot".

## 6. Non-goals for M6

- Rewriting authentication, protocol, or JNI code.
- Adding entities, inventory UI, or survival.
- Claiming a GPU backend without device verification.
- Bundling or downloading Mojang assets.
- Full parity with every one of the 26,684 block states. M6 must make the
  pipeline data-driven and correct for common blocks, and document the rest.

## 7. Audit conclusion

The current client already renders real server chunks with correct perspective,
depth, and full-cube culling. It looks unlike Minecraft because the texture
source is invented, every block is a cube, culling assumes everything is opaque,
real chunk light is dropped, and the renderer has no transparency pipeline.
M6's work is therefore concentrated in five places:

1. a user-supplied resource-pack loader (PNG, blockstate/model JSON, atlas);
2. a block-shape/model abstraction with non-cube geometry;
3. shape-aware face culling and render layers;
4. real sky/block light plumbed from `LightData` into smooth per-vertex light;
5. cutout/translucent rasterization with ordering, plus camera/fog corrections.

The implementation plan is in [`m6-implementation-plan.md`](m6-implementation-plan.md).
