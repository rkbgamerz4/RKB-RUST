# M6 rendering limitations

Status: **recorded 2026-11-18 alongside [`m6-verification.md`](m6-verification.md).**
Everything here is a known, deliberate limit of the current implementation or of
the environment it was verified in. Nothing in this list is presented as
working. When a limit is a *deviation from vanilla*, it says so; when it is an
*environment limit*, it says that too.

## 1. Backend and device

- **Software rendering only.** `crates/rkb-render` is a CPU rasterizer. There
  is no OpenGL ES, Vulkan, or wgpu path, no shaders, and no GPU resource
  management. No GPU claim is made anywhere in M6.
- **No physical device and no emulator were available.** The Android APK ARM64
  build is verified by CI only; the M5 host-tested join still has not been
  observed on a device. Frame rates in the verification doc are headless,
  debug-build, CPU numbers for `rkb-join` — not APK, not GPU, not release.
- **Android presents a 0.6x framebuffer.** `MainActivity.kt` keeps
  `RENDER_SCALE = 0.6f` and lets the compositor upscale. This is a deliberate
  performance choice for a CPU rasterizer; it softens texels, so on-device
  frames look less crisp than the 480x270 headless captures.
- **No Android NDK/SDK in this environment.** `cargo check --target
  aarch64-linux-android` cannot run locally; cross-compilation happens in CI
  (`scripts/build-android.sh`).

## 2. Resource-pack loading

- **Pack work runs on the render thread.** On Android, `poll_join` (called from
  the per-frame engine entry) opens the pack, parses blockstate/model JSON,
  decodes PNGs, and builds the atlas synchronously, then clears and re-queues
  the loaded area. Chunk re-meshing itself is amortized at `MESH_BUDGET = 2`
  chunks per frame, but the initial open/decode is a one-off hitch on the
  render thread rather than a worker. It was not moved because doing so would
  change the M5 join/threading contract.
- **One pack at a time.** Loading replaces the previous pack; there is no pack
  stack, no overlap/priority resolution, and no server resource-pack
  (play-resource-pack) negotiation.
- **No animated textures.** `.png.mcmeta` animation, `frame_time`, `interpolate`
  and `blur` are ignored: an animated texture shows its first frame only.
- **No colormap textures.** `textures/colormap/grass.png` and `foliage.png` are
  not read; see §4.
- **PNG subset.** 8-bit, non-interlaced RGBA/RGB/grayscale/palette PNGs only.
  16-bit, interlaced, and low-bit-depth packed images are rejected with an
  error and fall back to the missing-texture tile.
- **`pack_format` is reported, not enforced.** A pack with a wrong format still
  loads; the number is shown in the status line so a mismatch is visible.
- **Unsupported model constructs fall back.** Weighted `models` arrays,
  `uvlock`, `custom_geometry`, `head`/`wall_hanging` block entities, and entity
  models are not supported; the block keeps its built-in shape. Blockstate
  variant `x`/`y` rotation *is* supported (baked into the element transform).
  Two counters keep this visible: `unmodelled` in the resource-pack status
  counts states the pack had no model for, and `missing_models` counts states
  whose built-in shape is an approximation of a non-cube block. Both were `0`
  in every evidence run (the flat world only contains blocks the verification
  pack models).
- **Verification pack ≠ Minecraft pack.** Evidence used a self-authored pack
  (`scripts/m6-verify-pack.py`) because no Mojang asset may be downloaded or
  bundled. Real-world packs (32x/64x, colormaps, multipart fences) have not been
  exercised.

## 3. Geometry, culling, and UVs

- **`cullface` is only tested for untransformed elements.** When an element has
  a baked `x`/`y` rotation, RKB skips the neighbour cull test instead of
  transforming the test direction, so such elements always draw their faces.
  This costs overdraw but never punches holes.
- **Absent `cullface` means no culling**, matching vanilla semantics (the field
  defaults to "no culling"; vanilla's `cube.json` sets it explicitly on all six
  faces). A pack model that omits `cullface` therefore draws interior faces.
- **AO is only for untransformed full cubes.** Slabs, stairs, rotated elements,
  and cross plants get face shade times one light sample at the block cell —
  no per-corner occlusion, so partial shapes look flatter than vanilla.
- **`cullface` direction is the model face, not the world face after
  transform** (see the first point); texture `rotation` is applied, `uvlock` is
  not.
- **Neighbour culling is per-block-name for cutout blocks**: a cutout block
  hides its own faces against a full cube of the same name (glass-glass,
  leaves-leaves) but never hides an opaque neighbour. This was tested, not
  compared against a vanilla client.
- **No block entities, entities, or item frames.** Chests, signs, banners,
  armour stands, mobs, and dropped items are received by the protocol layer but
  never meshed or drawn.
- **Fluids are built-ins, not pack models.** Water/lava use the built-in
  surface-height shape; flowing animation, `level` per-face heights beyond the
  built-in, and waterlogged block models are not implemented.

## 4. Textures and colour

- **Nearest-neighbour sampling, no mipmaps.** The atlas uses half-texel inset
  UVs and point sampling, so distant terrain shimmers and aliases. Vanilla
  mipmaps its atlas with a cutout-aware threshold; RKB does not.
- **Biome tints are fixed constants.** Grass `#91BD59`, foliage `#77AB2F`, and
  water `#3F76E4` are applied from the block's tint kind, not from the biome or
  a colormap. Every biome renders with the same green/blue.
- **`tintindex` maps to one of those three constants.** A pack face that sets
  `tintindex` gets grass/foliage/water chosen from the block name; there is no
  colormap lookup and no red/blue/magenta tint for e.g. `redstone_wire` or
  `vine`.
- **Cutout threshold is fixed at 0.5** (translucent uses 0.01). Vanilla uses a
  configurable `alphaTest`/mip cutoff; a pack that relies on a different
  threshold will differ.

## 5. Lighting

- **Fallback light is labelled, never silent.** Columns without decoded light
  use `(15, 0)` and the HUD/CLI prints `light: full-sky fallback ...` with
  `light fallback true`. It is never reported as server light.
- **Light is sampled once per corner for full cubes, once per block otherwise**
  (see §3), so partial and rotated shapes get flat light instead of vanilla's
  smooth per-corner gradient.
- **No lightmap/block-entity light.** Torches, glowstone, and lava illuminate
  through the server's block-light array only; there is no per-tile lightmap,
  no sky-light angle, and no dimension sky-darkening factor beyond what the
  server sends.
- **Ordering edge case:** `Chunk Data` carries its own light snapshot, which
  *replaces* any entry already stored for that column. An `Update Light`
  received before its `Chunk Data` is therefore superseded by that snapshot
  (vanilla sends the two together, so this has not been observed, but the code
  path exists). Light changes re-mesh only the affected columns; a column that
  was meshed before its light arrived keeps the fallback until its next
  rebuild.

## 6. Sky, fog, and time

- **The sky is a fixed two-stop gradient.** `UpdateTime` is decoded by
  `rkb-net` (the control client prints world age/time of day) but is not applied
  to the renderer: there is no day/night cycle, no moon, no sunset colours, and
  no rain/darkness overlay.
- **Fog is derived from the load radius**, not from the server's view distance:
  `FOG_START = 36`, `FOG_END = 64` blocks for `LOAD_RADIUS = 3`. Increasing the
  server view distance does not change fog.
- **No dimension-specific sky or fog** (nether fog, end sky, underwater fog,
  lava fog are absent), and fog colour always follows the overworld sky.
- **Biome blending, cloud layer, and star field are absent.**

## 7. World and gameplay data

- **No death screen or respawn flow.** RKB does not render a death overlay or
  send a respawn request; a dead player therefore stays dead and — as a vanilla
  server does — receives no world packets. Verification uses a live account.
  This is out of M6 scope, not a rendering defect.
- **Server worlds are read-only.** M4/M6 do not send player movement or block
  actions, so the server never sees the local camera move. Teleports are issued
  from the server console for evidence.
- **No inventory, crafting, chat, or entity UI** beyond what M3 already
  tracked. The HUD is crosshair, joystick, buttons, and status text.
- **Chunk delivery is server-paced.** After a long-distance `/tp` into an area
  the server has not loaded, columns arrive minutes later while the previous
  area is unloaded immediately (reproduced with two independent clients). RKB
  reports `not loaded` instead of inventing terrain; see
  [`m6-verification.md` §9.2](m6-verification.md).

## 8. Assets and licences

- **No Mojang asset is bundled, cached, or redistributed.** The procedural
  atlas is an explicitly labelled fallback and is never presented as Minecraft
  art; the verification pack is generated by a script in this repository; tests
  use self-authored fixtures.
- **Users supply their own resource pack** (directory or `.zip` in the vanilla
  layout). RKB does not download, select, or validate a pack against Mojang's
  catalogue.
- **New dependency:** `miniz_oxide` 0.8.9 (MIT/Apache-2.0, pure Rust) for
  deflate; PNG/ZIP/JSON handling is RKB's own code. `zip`, `png`, and `image`
  crates are not used (MSRV 1.77).

## 9. Honest reporting rules used by M6

- "Verified" is only claimed for what a command printed in
  [`m6-verification.md`](m6-verification.md).
- PPM frames are labelled *headless software render*, never *screenshot* or
  *device capture*, and their SHA-256 prefixes are recorded.
- Frame counts and wall-clock times are the only performance numbers; no FPS is
  attributed to a device, an APK, or a GPU.
- Where the server behaved unusually (dead account, slow post-teleport chunk
  delivery), the diagnosis and the control experiment are published rather than
  the symptom being attributed to the renderer.
