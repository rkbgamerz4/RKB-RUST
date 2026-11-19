# M7 GPU architecture: a real GPU backend for Android ARM64

Status: **written 2026-11-18 before implementation, on branch
`feature/mc-java-protocol` at `4862113`.** This document records the audited
starting point, the backend comparison, the selected design, and the
verification tiers this environment can actually reach. It is the plan M7 is
implemented against; the results live in
[`m7-verification.md`](m7-verification.md),
[`m7-performance.md`](m7-performance.md), and
[`m7-limitations.md`](m7-limitations.md).

## 1. Audited starting point (M6 at `4862113`)

| Layer | What exists |
| --- | --- |
| Data | `rkb-net::ServerWorld` (protocol 767 chunks, real `LightData`), `rkb-mc::Chunk`. |
| Meshing | `rkb-minecraft::mesh`: element-based block models, pack blockstates, per-corner AO × real sky/block light, layered `opaque`/`cutout`/`translucent` `ChunkMesh` with `min`/`max`, `ChunkMesh::face_count`. |
| Textures | `ActiveTextures::{Builtin, Pack}` → `Arc<rkb_render::Texture>` plus `uv_id(id) -> [u0,v0,u1,v1]` (normalized, half-texel inset). |
| Camera | `rkb_minecraft::Camera` (position/yaw/pitch/fov, `view_projection`, `project_polygon`, `frustum`) and `Frustum::contains_aabb`. |
| Software renderer | `rkb-render::SoftwareBackend` + `DrawList`; `MinecraftGame::render` fills the list (sky gradient, textured triangles, HUD). |
| Android | `SurfaceView` + render thread; `lockCanvas` → `Bitmap.copyPixelsFromBuffer(rgba)` at `RENDER_SCALE = 0.6`; JNI `nativeCreate/nativeRenderFrame/nativeJoinServer/...`; join on a worker thread; `AndroidEngine::poll_join` applies the pack. |
| Verification | Headless `rkb-join` against a real 1.21.1 offline server; no device or GPU proof. |

The M6 mesh already carries everything a GPU needs per vertex: world position,
tile-local UV, `light` (AO × shade × sky/block light), tint, layer, and
double-sided flag. M7 must therefore **consume** `ChunkMesh`, not re-derive
chunks or block states.

## 2. Backend comparison

| Option | Android ARM64 reality | MSRV impact | Shader story | Verdict |
| --- | --- | --- | --- | --- |
| **wgpu 0.19.4 (Vulkan primary, GLES fallback)** | Vulkan 1.0/1.1 and GLES 3.x on every ARM64 Android 7+ device class; wgpu-hal enables both on unix/Android; `ANativeWindow` surface via `RawWindowHandle::AndroidNdk`. | 1.76-era release; no `rust-version` field, dependencies pre-date 1.77. Keeps the workspace MSRV. | WGSL compiled and validated at runtime by naga — no shader toolchain on the device or in CI. | **Selected.** |
| wgpu 22+/24+ | Same APIs, newer features. | 22 targets 1.76 too, but 24+ moves past 1.77 and drags newer `naga`/`bitflags`; no benefit for this scope. | Same. | Rejected: unnecessary MSRV risk. |
| `ash` (raw Vulkan) | Direct Vulkan, no GLES fallback; requires hand-written instance/device/swapchain/descriptor code and precompiled SPIR-V (needs `glslang`/`shaderc` in CI). | Bindings are low-MSRV. | SPIR-V must be built out of band. | Rejected: much more unsafe code, no GLES fallback, extra toolchain. |
| `glow` (GLES only) | Works widely, but GLES 2/3 feature detection, no Vulkan, and a second path if Vulkan is wanted. | Low. | GLSL strings, no validation until runtime. | Rejected: weaker on modern devices and no Vulkan. |
| SDL2 / ANGLE / native C++ | Adds a large C/C++ dependency and NDK build complexity; ANGLE is a GLES-on-Vulkan shim, not a renderer. | n/a. | n/a. | Rejected: heavyweight, no Rust-native benefit. |

**Decision: `wgpu 0.19.4`, `Backends::VULKAN | Backends::GL`, WGSL via naga.**

Why Vulkan first: it is the native Android graphics API, has a swapchain that
maps directly to `ANativeWindow`, and gives explicit depth/attachment control.
GLES is kept as the same-code fallback because some emulators expose only GLES
(SwiftShader) and because a driver-level Vulkan failure must not force the
software path when GLES works.

## 3. Why the MSRV stays at 1.77

`wgpu 0.19.4` was published in December 2023, before Rust 1.77, and its
workspace MSRV was 1.76. No M7 dependency (wgpu, wgpu-core, wgpu-hal, naga,
pollster, bytemuck) declares a `rust-version` above 1.76. The CI MSRV job
(`cargo check --workspace --all-targets --locked` on 1.77.0) is the gate; if it
fails, the change must be documented here **before** the MSRV is raised. As of
this writing, no raise is planned.

## 4. Crate and dependency direction

```text
rkb-net ─┐
rkb-mc  ─┼─> rkb-minecraft ──> rkb-gpu ──> wgpu
rkb-render ┘        │              │
                    │              └─> raw-window-handle (via wgpu::rwh)
                    └─> (software path unchanged)

rkb-android ──> rkb-gpu + rkb-minecraft
```

- **New crate `rkb-gpu`** owns wgpu: instance/adapter/device, pipelines,
  atlas texture, camera uniform, per-chunk GPU buffers, translucent sorting,
  sky/HUD batches, offscreen readback, and the Android surface wrapper.
- `rkb-gpu` depends on `rkb-minecraft` for `ChunkMesh`, `Camera`, `Frustum`,
  `Color`, and `Texture`; it never decodes chunks, block states, models, or
  light.
- `rkb-android` gains the surface lifecycle and a `gpu` feature path; it keeps
  the M5 join worker and the M6 pack loading untouched.
- `rkb-render` gains no GPU code, so `#![forbid(unsafe_code)]` stays intact and
  the command list remains backend-independent.
- The software renderer remains compiled into the APK and selectable at
  runtime if GPU initialization fails.

## 5. Rendering pipeline

### 5.1 Geometry

For each `ChunkMesh` the GPU converter emits, per face, four vertices and six
`u32` indices:

| Vertex attribute | Source |
| --- | --- |
| `pos: vec3<f32>` | `Face.points` (world space) |
| `uv: vec2<f32>` | `uv_id(Face.texture)` rect, `mix(rect.xy, rect.zw, Face.uv)` |
| `light: f32` | `Face.light[i]` |
| `tint: vec3<f32>` | `Face.tint` |

Opaque and cutout geometry become static per-chunk vertex/index buffers.
Translucent faces are kept on the CPU (small), frustum-culled and globally
sorted back-to-front by face center each frame (the same rule M6 uses), then
written to one reusable dynamic vertex/index buffer — one translucent draw
call per frame with depth write off and source-over blending.

Buffer reuse: re-uploading a chunk uses `queue.write_buffer` when the new data
fits the existing capacity; otherwise the buffers are recreated. Chunk unload
drops only that chunk's buffers. Atlas or pack changes only invalidate UVs and
the base texture, and trigger a full mesh re-upload because every UV changes.

### 5.2 Shader and state

One WGSL module with three fragment entry points:

- `fs_opaque` — texture × tint × light, fog mix, alpha forced to 1;
- `fs_cutout` — same, `discard` when `alpha < 0.5`;
- `fs_translucent` — same, preserves source alpha for blending.

State matrix:

| Pipeline | Depth write | Compare | Blend | Cull |
| --- | --- | --- | --- | --- |
| opaque | yes | less | none | none |
| cutout | yes | less | none | none |
| translucent | no | less | src-alpha / one-minus-src-alpha | none |
| sky | no | always | none | none |
| HUD | no | always | src-alpha | none |

Backface culling is disabled because M6 geometry can be mirrored by model
rotation and crossed plants are double-sided; M6 already culls block faces.

Color space: the preferred surface format is a non-sRGB `Bgra8Unorm`/
`Rgba8Unorm`, and the atlas is `Rgba8Unorm`, so the values match the M6
software frames one-to-one. If the device exposes only an sRGB surface, the
shader converts its output to linear (`pow(1/2.2)`) via a uniform flag, and the
verification documents the difference.

Fog: the same `FOG_START`/`FOG_END` and sky-horizon color M6 uses, supplied in
the camera uniform; sky is a fullscreen gradient, HUD is a triangle batch built
from the game's existing `DrawList` 2D commands (rect/stroke/line/circle/
triangle), so touch controls look the same without duplicating layout code.

### 5.3 Depth, resize, and camera

`Depth32Float`, cleared to 1.0 every frame. The chunk vertex shader uses
`Camera::view_projection` (column-major `Mat4`), so near/far and FOV are
unchanged. On resize: reconfigure the surface, recreate the depth texture, and
update the HUD/NDC uniform; no mesh re-upload is needed.

### 5.4 Chunk lifecycle

`Scene` keeps `BTreeMap<(i32,i32), ChunkGpu>` with bounds. Per frame:

1. Game reports resident meshes with a monotonically increasing per-mesh
   revision.
2. New/changed revisions are converted and uploaded; missing keys are removed.
3. Frustum culling uses `ChunkMesh.min/max` and `Camera::frustum`.
4. Opaque/cutout draws one range per visible chunk; translucent faces are
   gathered from visible chunks, sorted, and drawn once.

Networking and pack parsing stay where M6 put them (worker thread / one-off
`poll_join`), so the render thread never blocks on a socket or ZIP read.
`MESH_BUDGET = 2` per frame is unchanged; upload work is therefore bounded by
two chunk meshes plus the translucent batch.

## 6. Android presentation and lifecycle

- Kotlin passes `SurfaceHolder.surface` to `nativeAttachSurface` on surface
  creation and after recreation.
- Native (`rkb-android`) calls `ANativeWindow_fromSurface`, builds
  `RawWindowHandle::AndroidNdk`, creates the wgpu surface/config, and renders
  directly to the swapchain from the render thread. `lockCanvas` is not used in
  GPU mode.
- On `surfaceDestroyed`, the render thread is joined first, then
  `nativeDetachSurface` drops the swapchain and releases the window; the device,
  pipelines, and CPU meshes survive, and the next attach re-uploads resident
  meshes (cheap: they are already in memory).
- Pause/resume keeps the engine alive; only the surface is dropped.
- If instance/adapter/surface/config creation fails, `nativeAttachSurface`
  returns false with the exact error, the engine records
  `renderer: software (gpu unavailable: …)`, and Kotlin continues with the M6
  bitmap path. The active backend is always printed in the stats overlay and in
  `nativeStats`.

## 7. Verification tiers (what can and cannot be proven here)

The M7 environment has three distinguishable tiers, and evidence will name the
tier every time:

| Tier | Meaning | Reachable in this environment |
| --- | --- | --- |
| **T1 host software** | M6 `SoftwareBackend`; reference frames. | Yes. |
| **T2 host Vulkan** | `rkb-gpu` + `rkb-join-gpu` offscreen on the *physical device* through Mesa lavapipe (CPU Vulkan ICD in Termux). Real server chunks, real GPU pipeline API, **not** the Mali GPU. | Yes — verified below. |
| **T3 Android surface** | The APK rendering into an `ANativeWindow` with the vendor driver (Mali-G52 on this unit) or an emulator's SwiftShader. | Emulator: CI only. Physical Mali: **blocked** (see below). |

Blocked-path finding (measured before implementation):

- The device is `mt6768` with `/vendor/lib64/hw/vulkan.mt6768.so` present.
- Termux's app linker namespace refuses that vendor library
  (`... is not accessible for the namespace "(default)"`), and the system
  loader cannot load in Termux (`libbinder_ndk.so` unavailable).
- The only Vulkan ICD reachable from Termux is Mesa **lavapipe**
  (`llvmpipe`, device type CPU). A 64×64 wgpu 0.19.4 clear + readback and a
  WGSL compile succeeded through it, which is why T2 is real evidence of the
  pipeline but explicitly **not** evidence of Mali rendering.

Consequences for the definition of done:

- "GPU backend initializes", "M6 meshes upload", "real server chunks render",
  "textures/light/depth/transparency", "live updates/unloads" can all be
  proven at T2 with captured PGM frames and counters.
- "Android surface lifecycle works" is proven by construction plus emulator
  execution; a physical-device Mali screenshot is **not** obtainable without
  installing the CI-built APK through the Android UI or an adb connection,
  neither of which this environment provides. M7 will not claim it.

## 8. Test and CI plan

Host/CI (no GPU needed):

- WGSL parse + validate with `naga` 0.19 in a unit test.
- Mesh packing, atlas UV mapping, translucent sort, frustum visibility, buffer
  growth policy, backend-selection/fallback logic — pure unit tests.
- `cargo fmt`, `clippy -D warnings`, workspace tests on 1.77 and stable.

CI with lavapipe (`mesa-vulkan-drivers` on ubuntu-latest):

- adapter/device init, shader module creation, atlas upload, vertex/index
  buffer creation, depth test, opaque/cutout/translucent readback pixels,
  chunk upload/unload, resize.

Android CI:

- ARM64 + x86_64 cross-compile and APK build (existing workflows).
- Emulator: launch the APK, assert the diagnostics line names the active
  backend, and capture `screencap` screenshots as artifacts. If the emulator
  image exposes no Vulkan/GLES adapter, the run must report the exact fallback
  reason instead of pretending GPU mode is active.

## 9. Performance targets and measurement rules

- No FPS promise. Measure `rkb-join-gpu` wall time, frames, GPU submit+readback
  time, chunk upload time, mesh conversion time, draw calls, triangles, and
  peak RSS; state that T2 includes a full framebuffer readback that the APK
  does not pay.
- Record the APK-tier frame pacing only if the emulator run produces usable
  numbers; emulator GPU numbers are not device numbers.
- Track uploads per frame and assert in debug that only changed revisions are
  uploaded (no full-world re-upload).

## 10. Non-goals

- No new voxel engine, no change to M1-M6 networking, models, packs, lighting,
  or gameplay.
- No Mojang assets, no shader side files downloaded at runtime, no GPU memory
  growth without bounds.
- No removal of the software renderer; it stays the fallback and the reference
  implementation.
