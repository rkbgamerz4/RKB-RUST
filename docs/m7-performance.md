# M7 Performance

All measurements were collected on a Termux Android ARM64 device
(NEXG_N10 / mt6768, 8 cores, 5.6 GiB RAM, Android 14/SDK 34) running
Mesa lavapipe (llvmpipe). The release binary `rkb-join-gpu` was used with
`--backend vulkan` against an offline Minecraft Java 1.21.1 server
(port 25575, flat creative, view distance 3). The server block report
contains 26684 states.

## Init
- GPU init time: **698.6 ms** (from the first log line: `gpu: gpu llvmpipe ... init 698.6 ms`).
- Surface: 640 × 360, `Rgba8Unorm`, non-sRGB.
- Target: `Rgba8Unorm 640x360`.

## Frame Timing (steady state, frames 55–202)
- **Wall time per frame** (GPU submit + readback): **213.66 ms/frame** (4.7 fps equivalent).
  This includes the `wgpu` present + `glReadPixels` readback to CPU for PPM generation.
  GPU-side only (no readback) would be faster; the figure is inclusive of the
  PPM artifact write.
- **Frame budget**: 213.66 ms / frame. No FPS promises are made; the measured
  equivalent is ~4.7–4.8 fps on this CPU-only pipeline.

## Mesh Generation
- 45 total chunks resident after join.
- Peak meshing: frames 15–30 show chunk count rising from 8 → 38 as the
  player spawns and chunks are meshed.
- Mesh budget `MESH_BUDGET = 2` limits per-frame uploads; remaining chunks
  accumulate in the queue.

## GPU Uploads
- **49 chunk meshes** uploaded over 202 frames.
- **34.99 ms total** upload time (0.71 ms average per frame).
- **4 249 224 bytes** total uploaded mesh data across all frames.
- Peak per-frame upload budget `GPU_UPLOAD_BUDGET = 2` meshes/frame on the
  Android path; uploads accumulate when the budget is not exhausted.

## Faces / Trangles / Draws (post live-edit)
| Metric | Baseline | Post-edit (7 blocks) | Delta |
|--------|----------|----------------------|-------|
| Tris | 19574 | 19622 | +48 |
| Faces | 23211 | 23253* | +42 |
| Draws | 23 | 24 | +1 |
| Visible chunks | 19 | 19 | 0 |

* faces = opaque + cutout + translucent across all resident meshes. The +42 faces
correspond to 7 blocks × 6 faces each (each edited block is fresh air → solid,
adding a full-cube mesh per block). The +1 draw is the one additional opaque
face rendered.

## Live-Edit Within-Session Deltas
- Baseline tris: **19574**, faces: **23211**, draws: **23**, uploads: **45**,
  FNV: **c74bc9932fcabf08**.
- After 7 fresh-air block edits (RCON `Changed the block` each):
  tris: **19622** (+48), faces: **23253** (+42), draws: **24** (+1),
  uploads: **49** (34.99 ms total, 0.71 ms avg), FNV: **d7a9b7edcac2d825**.
- The PPM artifact file `artifacts/m7/gpu-live.ppm` has FNV-1a
  `d7a9b7edcac2d825` (changed pixels visible within the frame).
- The live-edit script `m7-live-edit.sh` confirms: all 7 RCON edits returned
  `Changed the block`, all 7 block updates were delivered, and the mesh
  re-upload propagated to the GPU within one session.

## Memory
- **Peak RSS**: **203756 KiB** (~199–204 MB) as reported by the perf summary.
- **GPU buffers** (vertex + index + atlas + depth): **3 932 216 bytes** (~3.75 MiB).
- Atlas: **128 × 64** pixels, built-in procedural, non-sRGB `Rgba8Unorm`.

## Summary
| Metric | Value |
|--------|-------|
| Init | 698.6 ms |
| Wall time / frame | 213.66 ms (4.7 fps equivalent) |
| Peak RSS | 203756 KiB |
| GPU buffers | 3 932 216 B |
| Atlas | 128×64, Rgba8Unorm |
| Chunks resident | 45 |
| Frames rendered | 202 |
| Tris (baseline → post-edit) | 19574 → 19622 (+48) |
| Faces (baseline → post-edit) | 23211 → 23253 (+42) |
| Draws (baseline → post-edit) | 23 → 24 (+1) |
| Uploads total | 49 chunk meshes over 202 frames |
| Upload budget | 0.71 ms avg per frame |
| Live-edit FNV delta | c74bc993… → d7a9b7ed… |