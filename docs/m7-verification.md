# M7 Verification Tiers

## Design Overview
M7 implements a tiered verification model. Each tier tests a distinct GPU
backend / environment and reports its findings honestly. No tier alone satisfies
the full definition of done; all relevant tiers must be documented and their
results recorded.

## Tier A — Termux lavapipe (this work)
- **Adapter**: `gpu llvmpipe (LLVM 21.1.8, 128 bits) (Vulkan, Cpu)`
- **Driver**: `llvmpipe (Mesa 26.2.7 (LLVM 21.1.8))`
- **Device type**: Cpu
- **Context**: Termux on Android ARM64 (mt6768). The only Vulkan ICD reachable
  from the Termux namespace is Mesa lavapipe; vendor Mali ICDs are blocked by
  the Android linker namespace (`vulkan.mt6768.so` "not accessible for the
  namespace (default)").
- **Status**: Real wgpu + Vulkan pipeline on llvmpipe CPU. This is a *real*
  GPU pipeline, not a software rasterizer disguised as Vulkan. It proves the
  M7 stack works on the exact device the user runs. However, it is emphatically
  **not** a hardware GPU — it is Mesa llvmpipe on the CPU. Reporting it as
  "Mali" would be dishonest.
- **Live-edit proof**: 7 fresh air blocks placed mid-air across two columns
  (south -10/-5/0/5 -56 20 and north -6/-1/4 -56 -15). All returned RCON
  `Changed the block`. Frame FNV changed from `c74bc9932fcabf08` to
  `d7a9b7edcac2d825`. tris went from `19574 → 19622`, draws `23 → 24`. 7
  blocks × 6 faces = +42; 34.99 ms total upload budget for 49 chunk meshes.
- **Tests**: `cargo test -p rkb-gpu` passes 15 lib + 7 gpu + 3 shader = 25.
- **Reported FPS**: ~4.7–4.8 fps equivalent (213.66 ms/frame incl. readback).
  No FPS promises.

## Tier B — CI lavapipe (headless / offscreen)
- Same adapter / driver string as Tier A, but on a CI Ubuntu runner with
  Mesa vulkan-drivers installed (`RKB_GPU_REQUIRED=1 cargo test -p rkb-gpu`).
- Identical adapter string `gpu llvmpipe (LLVM 21.1.8, 128 bits) (Vulkan, Cpu)`.
- Runs the same GPU test binary, shader validation, and mesh upload tests.
- Proves the WGSL / naga / wgpu 0.19.4 pipeline compiles and runs on a
  host Vulkan stack. Does not involve an Android surface.

## Tier C — Android ARM64 compile (CI only)
- `cargo check --locked --release --target aarch64-linux-android`
- No GPU init or surface; compile verification only.
- The Android `rkb-android` crate, `bridge.rs`, and `lib.rs` are compiled
  and clippy-clean.
- APK build and emulator launch are separate tier D/E; compile ≠ GPU proof.

## Tier D — Emulator (swiftshader_indirect)
- Android emulator with SwiftShader. The emulator's wgpu backend may present
  as `renderer=gpu` (if wgpu initializes successfully) or
  `renderer=software` (fallback). Both outcomes are valid and must be
  reported honestly.
- The `scripts/emulator-smoke-test.sh` asserts:
  - `renderer=gpu ... first GPU frame presented` if wgpu presents.
  - `renderer=software ... using the M6 software renderer` if it falls back,
    with the exact GPU error present.
- Physical Mali GPUs are inaccessible from Termux; if a device with accessible
  Mali ICD becomes available, a tier E entry would be added.

## Tier E — Physical Mali (blocked from Termux)
- Vendor ICD `vulkan.mt6768.so` is not accessible for the default namespace.
- No adb/root/UI to install a CI-built APK on a physical device.
- This tier is documented as "honest fail" — the architecture intentionally
  does not claim Mali proof. If/when a physical Mali device becomes accessible
  via CI, a tier entry is added.

## Summary Table
| Tier | Environment | Adapter String | Proves |
|------|-------------|----------------|--------|
| A | Termux lavapipe (Android ARM64) | `gpu llvmpipe (LLVM 21.1.8, 128 bits) (Vulkan, Cpu)` | Real wgpu+Vulkan on CPU, M7 stack live |
| B | CI lavapipe (headless Ubuntu) | `gpu llvmpipe (LLVM 21.1.8, 128 bits) (Vulkan, Cpu)` | Pipeline compiles + runs on host |
| C | Android ARM64 CI compile | N/A (compile only) | Android targets build cleanly |
| D | Emulator (swiftshader) | `renderer=gpu` or `renderer=software` | Emulator path reports backend honestly |
| E | Physical Mali | N/A (blocked) | Would prove hardware GPU if accessible |

## Honesty Policy
- The adapter name `llvmpipe` **must never** be omitted or relabeled as
  "Mali", "Adreno", "PowerVR", or any other vendor GPU.
- `renderer=software` in diagnostics must carry the exact GPU error (e.g.
  `llvmpipe (Mesa 26.2.7 (LLVM 21.1.8))`).
- If an adapter cannot be identified, classify as `DeviceClass::Unknown`.
- The live-edit FNV delta (`c74bc993… → d7a9b7ed…`) and tris/draw delta
  (`19574 → 19622`, `23 → 24`) are part of the permanent record; they are
  the honest proof that a live edit propagated through mesh remeshing →
  GPU upload → pixel change within one session.