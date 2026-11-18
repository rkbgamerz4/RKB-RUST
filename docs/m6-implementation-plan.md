# M6 implementation plan: Minecraft-correct rendering of server chunks

Status: planned 2026-11-17 at commit `0f904b2`. This plan follows
[`m6-renderer-audit.md`](m6-renderer-audit.md) and stays inside the M1-M5
architecture: `rkb_net::ServerWorld` remains the only server data source, the
Android join stays on its worker thread, and rendering stays on the render
thread.

The plan is staged so each stage can be tested and reverted on its own. A stage
is "done" only when fmt, clippy, workspace tests, and its focused tests pass.

## 0. Ground rules

- Branch: `feature/mc-java-protocol`. Never rewrite `main`; there is no `main`
  to overwrite, but the rule stands for the default branch history.
- No deletion of existing modules. `worldgen`, `demo_world`, and the procedural
  atlas stay as an explicitly labelled fallback/demo path.
- No Mojang assets in the repository, tests, CI artifacts, or docs.
- No invented world data. Missing light, models, or textures are reported as
  fallbacks, never as if they came from the server.
- Software rendering remains the only backend claimed as working. A GPU backend
  is out of scope unless a device can verify it.
- MSRV 1.77 stays buildable; new dependencies must be pure Rust where possible.

## 1. Data model

### 1.1 Block shape and material

Add `crates/rkb-minecraft/src/model.rs` (renderer-side, not `rkb-mc` save
format):

```rust
pub enum RenderLayer { Opaque, Cutout, Translucent }

pub struct Element {
    pub from: [f32; 3],   // 0..16 model units
    pub to: [f32; 3],
    pub faces: [Option<ElementFace>; 6],
}

pub struct ElementFace {
    pub texture: TextureId,
    pub uv: [f32; 4],     // 0..16, model space
    pub cullface: Option<FaceDir>,
    pub rotation: u8,     // 0, 90, 180, 270
    pub tint: TintKind,   // None, Grass, Foliage, Water, Fixed(Color)
}

pub enum BlockShape {
    Cube,                 // full 1x1x1
    Elements(Vec<Element>), // model or built-in partial shape
}
```

`BlockShape` is resolved once per `BlockState` (name + properties) and cached.
The mesher never special-cases "slab" or "stair" in its face loop; it iterates
elements. A full cube is simply one element with six faces. This is the design
seam that lets slabs, stairs, fences, and doors be added without redesigning the
renderer.

Built-in shapes (used when no resource pack model exists) cover:

- full cube (default);
- slab (`type=bottom|top|double`), stairs (`facing`, `half`), snow
  (`layers`), carpet, pressure plate;
- cross plants (`cross` model: two diagonal quads);
- water/lava fluid surface (`level=0` -> 14/16 height, lower levels shorter);
- leaves/glass/ice as full cubes with the correct render layer.

Anything else falls back to a full cube and is counted in diagnostics as a
missing model, so coverage is measurable.

### 1.2 Faces

Extend `mesh::Face`:

```rust
pub struct Face {
    pub points: [Vec3; 4],
    pub normal: Vec3,
    pub texture: TextureId,
    pub uv: [[f32; 2]; 4],   // tile-local 0..1, ordered to the corners
    pub tint: Color,
    pub shade: f32,
    pub light: [f32; 4],     // shade * AO * world light, 0..1-ish
    pub layer: RenderLayer,
    pub flat_color: Color,   // fallback for atlas-less backends
}
```

`TextureId` is a `u32` index into the active atlas. UV resolution to atlas
coordinates happens at draw time, so a rebuilt atlas cannot leave stale UVs in a
cached mesh.

### 1.3 Chunk mesh

```rust
pub struct ChunkMesh {
    pub chunk: (i32, i32),
    pub opaque: Vec<Face>,
    pub cutout: Vec<Face>,
    pub translucent: Vec<Face>,
    pub min: Vec3,
    pub max: Vec3,
    pub non_air_blocks: usize,
    pub missing_models: usize,
}
```

Accessors keep existing call sites readable: `face_count()`, `faces()`,
`is_empty()`, and `mesh_bounds()`. Draw order: opaque, cutout, then globally
sorted translucent.

## 2. Resource-pack pipeline

New modules under `crates/rkb-minecraft/src/pack/`:

| File | Responsibility |
| --- | --- |
| `mod.rs` | `ResourcePack` abstraction (directory or `.zip`), path resolution, reload, `pack.mcmeta` validation |
| `png.rs` | Minimal PNG decoder: signature, CRC-skipping chunks, IDAT concat, zlib inflate via `miniz_oxide`, filters 0-4, color types 0/2/3/4/6 at 8-bit, non-interlaced; returns RGBA8 |
| `zip.rs` | End-of-central-directory and local-header reader; stored and deflate entries; no encryption |
| `model.rs` | blockstate variant/multipart selection, model parent resolution, texture aliases, element/face/rotation parsing |
| `atlas.rs` | Dynamic atlas: deduplicated texture paths, 16x16/32x32/64x64 cells, padding, missing-texture tile, rebuild on growth |

Dependency: `miniz_oxide` (MIT OR Zlib OR Apache-2.0, pure Rust, compatible with
MSRV 1.77) for zlib/deflate. No `zip`, `png`, or `image` crate is added, so the
Android cross-build stays dependency-light.

### 2.1 Pack layout

Accepted layout is the vanilla one:

```text
<pack root>/pack.mcmeta
<pack root>/assets/minecraft/blockstates/<block>.json
<pack root>/assets/minecraft/models/<model>.json
<pack root>/assets/minecraft/textures/<texture>.png
```

`pack.mcmeta` is read and its `pack_format` is recorded, but a mismatched format
is a warning, not a silent rejection: users may supply a newer pack for a
1.21.1 server and accept missing assets.

### 2.2 Resolution order

1. Exact variant key match on all block-state properties.
2. Best partial match (most property matches), then first declared variant.
3. `multipart`: evaluate `when` conditions (`OR`, `AND`, `key=value`, `key|v1|v2`)
   and combine applied parts.
4. If the blockstate or model is missing, use the built-in shape/tile and count
   a `missing_model`.
5. If a texture file is missing, use the magenta/black missing-texture tile and
   count `missing_texture`.
6. `#alias` texture references are resolved recursively with a depth limit.
7. Parent models are merged: parent elements first, child elements replace them
   when present; textures merge with child overrides.

### 2.3 Atlas behavior

- Textures are added lazily as block models are first resolved, so a server with
  60 block types does not decode the whole vanilla pack.
- The atlas keeps decoded images, not just packed pixels, so growth rebuilds are
  lossless.
- Tiles are placed in power-of-two cells with a 1-texel transparent border;
  UVs inset by half a texel.
- Animated textures (height > width, square frame width) use frame 0 and are
  documented as non-animated.
- `MinecraftGame::load_resource_pack(path)` replaces the atlas and clears all
  meshes so new UVs apply. Failure keeps the procedural atlas and reports the
  error.

### 2.4 CLI and Android surface

- `rkb-join --resource-pack <path>` loads a pack before joining.
- `MinecraftGame::resource_pack_status()` reports loaded/fallback state for the
  HUD and `rkb-join`.
- Android: add an optional resource-pack path to the existing join dialog and
  pass it through `nativeJoinServer` as a fourth argument. If this cannot be
  done without risking the M5 path, keep the host flag and document the Android
  gap rather than breaking join.

## 3. Lighting

### 3.1 Network plumbing

- `PlaySession` keeps `chunk_light: BTreeMap<(i32, i32), LightData>` in addition
  to `chunk_updates`.
- `store_chunk` stores `packet.light`.
- `UpdateLight` merges by mask: for each mask bit, replace that section's sky or
  block array; empty masks clear the section to zero.
- Expose `PlaySession::chunk_light(x, z)`.
- `ServerWorld` converts `LightData` to a compact `ChunkLight` and stores it per
  column. `Update::Light` updates it without rebuilding geometry unless the
  light actually changed.

### 3.2 Light decode

`LightData` masks use bit 0 = section `min_section_y - 1`. Nibble index is
`(y << 8) | (z << 4) | x`; even indices are the low nibble. `ChunkLight` stores
`BTreeMap<i32, SectionLight>` keyed by absolute section Y:

```rust
pub struct SectionLight { pub sky: Vec<u8>, pub block: Vec<u8> } // 2048 bytes each
```

### 3.3 Sampling

`ChunkSource` gains:

```rust
fn light_at(&mut self, x: i32, y: i32, z: i32) -> SourceResult<LightSample>;
fn block_at(&mut self, x: i32, y: i32, z: i32) -> SourceResult<Option<BlockState>>;
```

`LightSample { sky: u8, block: u8 }` (0..15). Server worlds return real data;
save/generated worlds return a documented fallback (`sky=15, block=0`), and the
game reports that fallback once. A missing light array inside a server chunk is
also reported and treated as `sky=15, block=0` only for the uncovered section.

### 3.4 Smooth vertex light

For each face corner, sample the four cells around that corner just outside the
face (face-adjacent, two edge-adjacent, diagonal). Skip opaque full cubes.
Average the remaining `max(sky, block * 1.0)` values. Apply:

```text
brightness = light_curve(max(sky, block)) * face_shade * ao
light_curve(l) = (l / 15)^1.4 * 0.85 + 0.15 * (l / 15)   // clamped
```

The curve is a documented approximation of Minecraft's lightmap, not a claim of
exact lightmap parity. Day/night sky multiplication is applied when
`UpdateTime` is decoded; if time is not decoded, day factor is 1.0 and the status
says so.

AO rules change: only opaque full cubes occlude. Cutout/translucent blocks do
not cast the current hard AO (documented limitation).

## 4. Meshing and culling

`build_chunk_mesh` becomes:

1. For each non-air block, resolve `BlockShape` and `RenderLayer`.
2. For each element face, transform model coordinates (0..16) to world
   coordinates, apply face rotation, and compute model UVs.
3. Cull only when the face is on a cell boundary and the neighbor covers it:
   - neighbor is an opaque full cube -> cull;
   - neighbor is the same full-cube block and both are cutout/translucent and
     `cull_same` -> cull (glass, water, ice);
   - partial-shape faces are culled only against a full opaque neighbor that
     covers the face plane;
   - never cull cutout plants or cross quads.
4. Sample light/AO at the four corners.
5. Push to the layer's list.

Neighbour lookups use the existing 3x3 `Neighborhood` for blocks and a
`ChunkSource` light query for absolute positions. Cross-chunk boundaries use the
same rule, so a chunk edge never produces a false wall.

Rebuild behavior stays: dirty chunks from `ServerWorld::poll` remove and requeue
their mesh; unloads remove only that chunk; no full-world remesh. Add counters
for `missing_models` and `translucent_faces`.

## 5. Renderer changes

`crates/rkb-render`:

- Add `TexturedTriangle3D { ..., alpha: f32, blend: bool, depth_write: bool }`,
  keeping `textured_triangle_3d` as the opaque/cutout convenience constructor.
- Cutout: discard texels with `alpha < 0.5` (configurable; water/glass use
  `0.1`).
- Translucent: source-over blend against the framebuffer, depth test on, depth
  write off, sorted far-to-near by face center distance.
- Keep existing flat and gradient paths untouched.
- Add tests for alpha blending, cutout discard, depth-write-off ordering, and
  no regression in opaque depth.
- No GPU API is introduced. Backend remains `SoftwareBackend`.

## 6. Camera, sky, and presentation

- Keep the existing first-person camera and Android controls.
- Correct positive-X/positive-Z UV orientation and model rotation.
- Add `--fov`, `--pitch`, `--yaw` to `rkb-join` for reproducible screenshots.
- Replace hardcoded fog distance with a view-distance-derived value and make fog
  color follow the sky. Keep the sky fixed until `UpdateTime` is decoded.
- Apply biome tints when available from pack metadata/block name: grass
  `#91BD59`, foliage `#77AB2F`, water `#3F76E4` as documented approximations.
- Keep crosshair centered from the viewport, not a hardcoded resolution.

## 7. Tests

Focused tests to add (in addition to existing ones):

| Area | Test |
| --- | --- |
| PNG | decode RGBA/RGB/gray/palette fixtures; reject truncated/CRC-less bad data; filter reconstruction |
| ZIP | stored and deflated entries; path traversal rejected; missing entry |
| Models | variant selection, multipart conditions, parent texture inheritance, texture alias, rotation |
| Atlas | deduplicated paths, correct rects, missing texture, growth rebuild, no bleeding |
| Geometry | slab/stair/snow/carpet/cross element counts and bounds |
| Culling | opaque neighbor culls; glass neighbor does not cull stone; leaves do not cull; partial face not culled by non-covering neighbor |
| UVs | all six faces upright/unmirrored; explicit model UV and rotation |
| Light | nibble decode; mask-to-section mapping; smooth per-corner interpolation; fallback labelled |
| Transparency | cutout discard; translucent blend order; depth write off |
| Chunk boundaries | no wall between two full chunks; wall at unloaded neighbor; light across boundary |
| Rebuild | block update changes only that chunk; unload removes mesh; no full remesh |
| Resource loading | directory and zip pack; missing texture fallback; reload swaps atlas |

## 8. Verification

1. Build and run the existing workspace tests, clippy, fmt.
2. Start the official 1.21.1 offline server; join with `rkb-join`; capture PPM
   frames for:
   - procedural fallback (labelled as fallback);
   - synthetic test resource pack (self-authored textures/models);
   - live `/setblock` update and chunk unload.
3. Query real block coordinates before/after updates.
4. Measure only what is measured: frame count, meshed chunks, faces, triangles,
   wall-clock for the headless run. No FPS claim for an unmeasured device.
5. Android: build ARM64 + emulator APK in CI. If no emulator join is possible,
   state that M5 join remains host-verified only.
6. Write `m6-verification.md` and `m6-rendering-limitations.md` with exact
   commands, outputs, artifact paths, and unverified items.

## 9. Staging and commit plan

| Stage | Contents | Gate |
| --- | --- | --- |
| A | Audit + plan docs | docs review, no code changes |
| B | Renderer alpha/cutout/blend + tests | fmt/clippy/render tests |
| C | Shape abstraction + built-in non-cube geometry + culling + tests | mesh tests, M4 server render still works |
| D | Light plumbing + decode + smooth light + tests | server light evidence, fallback documented |
| E | Pack loader (PNG/ZIP/models/atlas) + reload + tests | synthetic pack renders; missing asset fallback |
| F | Camera/UV/fog/HUD corrections + CLI flags | screenshot orientation checks |
| G | Full verification, docs, CI, commit/push | all gates, honest report |

Each stage is committed separately on `feature/mc-java-protocol` so a failing
stage can be reverted without losing M1-M5.

## 10. Known risks and fallbacks

- **PNG/ZIP edge cases**: unsupported interlaced/16-bit/palette-low-bit images
  are rejected with a clear error and fall back to the missing tile. Tests use
  self-authored fixtures.
- **Vanilla model complexity**: multipart, weighted variants, and model
  rotations are implemented for the common cases; unsupported constructs fall
  back to the built-in shape and are counted.
- **Light merge semantics**: if a server light update is partial and the merge
  is wrong, caves may be too bright. The merge is tested per mask; the fallback
  is documented.
- **Software performance**: transparency sorting adds per-frame work. If it is
  too slow on a device, the sort can be limited to a radius without changing
  correctness claims, and the limit must be documented.
- **Android pack path**: if adding a fourth JNI argument risks M5, the host CLI
  ships first and the Android gap is reported, not hidden.
- **No GPU**: software remains the only verified backend.

## 11. Completion definition

M6 is complete only when:

- real server chunks render with correct full-cube and partial-block geometry;
- opaque faces are culled only by covering opaque neighbors;
- atlas UVs are model-driven and oriented correctly;
- real server sky/block light reaches vertex light, with fallbacks labelled;
- cutout and translucent geometry use the correct layers and ordering;
- chunk updates/unloads/rebuilds work without full-world remeshes;
- camera/projection/first-person presentation is stable;
- tests, fmt, clippy, and CI pass;
- headless screenshots and their environment are documented;
- Android join and GPU status are reported honestly, including "unverified".
