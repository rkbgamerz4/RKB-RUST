# M7 Limitations

## Physical GPU Accessibility
- The device (NEXG_N10 / mt6768) has vendor Mali ICDs, but `vulkan.mt6768.so`
  is **not accessible for the default namespace** from Termux. No known method
  within the Termux / Android 14 / aarch64 sandbox can make the Mali ICD
  visible to wgpu. Consequently, Tier E (physical Mali) is an honest fail:
  no on-device APK screenshots or physical Mali proof is available from the
  current environment.
- Emulator proof is limited to SwiftShader (`swiftshader_indirect`) or
  lavapipe if the emulator image includes it. A physical Mali GPU on a device
  accessible via CI would be required for a complete Tier E.

## No Online / Microsoft Mode
- The server is offline-mode only. No Microsoft / Xbox Live authentication,
  no Realms, no online-mode join. Resource packs must be self-authored; no
  Mojang/official packs are bundled.

## No Local Android ARM64 Compile (No NDK/SDK/rustup)
- No rustup, no Android NDK, no SDK. The `aarch64-linux-android` target
  can be cross-compiled in CI (GitHub Actions ubuntu-latest with
  `rustup toolchain install stable --target aarch64-linux-android`), but
  locally the target is unavailable. CI job `android.yml` compiles and
  clippy-checks the Android crates; this is a compile-only verification.

## Emulator ≠ Physical GPU
- The emulator (Tier D) runs SwiftShader Indirect or Lavapipe. A successful
  emulator launch with `renderer=gpu` does not prove a physical GPU works.
  Conversely, an emulator fallback to `renderer=software` is expected when
  the emulator's wgpu/Vulkan stack cannot present. Both outcomes are reported
  honestly; neither substitutes for a physical device.

## Lavapipe ≠ Hardware GPU
- All measurements on this device use Mesa llvmpipe (`gpu llvmpipe (LLVM
  21.1.8, 128 bits) (Vulkan, Cpu)`). This is a CPU software rasterizer
  behind Vulkan. It proves the M7 code path is functional, but performance
  and capabilities (e.g. tessellation, geometry shaders, sparse binding,
  dedicated video memory) differ from a discrete or integrated mobile GPU.
- All adapter strings explicitly classify the device as `DeviceClass::Software`
  or `DeviceClass::Unknown`; llvmpipe is never relabeled as hardware.

## Memory and Budget Constraints
- `MAX_CHUNKS = 4096` hard limit; the runtime tracks chunk revisions via a
  `BTreeMap`. On devices with limited RAM, the BTreeMap overhead plus GPU
  buffer residency may exceed available resources.
- `GPU_UPLOAD_BUDGET = 2` meshes/frame on the Android path; if more than 2
  chunks change in a single frame, the remaining revisions queue and upload
  in subsequent frames. This throttling is intentional but may cause visible
  "pop-in" on rapid terrain changes.
- Peak RSS ~204 MB includes the Rust runtime, wgpu structures, Atlas,
  depth buffer, and vertex/index buffers. On 512 MB devices this may be
  tight; on 5.6 GiB (the test device) it is comfortable.

## Live-Edit Scope
- Live-edit only confirms that block edits propagate through the server →
  client → poll → mesh → upload → GPU pipeline → pixel change within one
  session. It does not verify:
  - Client-side edit survival across disconnect/reconnect.
  - Multiplayer consistency (other clients seeing the same edits).
  - Edit persistence to world save (the test server has local saves disabled).
  - Interaction with resource packs or custom models.

## Frustum / Rendering Limits
- Translucent faces are kept on CPU and globally sorted back-to-front into
  one dynamic buffer. Depth write is off; cull mode is `None` for the
  translucent pass. This is a simplified path; real Minecraft uses separate
  translucent bin sorting and order-independent transparency.
- Sky is a fullscreen gradient (62% horizon). Procedural sky texturing is
  not implemented; the horizon color is hardcoded.
- HUD reuses the game `DrawList` 2D commands. No custom HUD geometry
  beyond what the game provides.

## Audio / Network / JNI
- Audio is not captured or rendered in the GPU binary; the tool is purely
  a rasterization pipeline.
- The JNI/Kotlin join contract and networking worker are preserved from M6;
  they are exercised but not profiled for M7 GPU additions.
- The `nativeAttachSurface` / `nativeRenderFrameGpu` / `nativeDestroy`
  lifecycle is instrumented but not timed end-to-end outside the per-frame
  GPU sync measurements.

## CI / GitHub Actions Constraints
- GitHub Actions runners do not have an Android device attached; Tier C
  (compile) and Tier B (headless lavapipe) are the only Android-adjacent
  CI jobs. Emulator Tier D requires the `android-emulator.yml` workflow,
  which launches a headless emulator image — no display, no input, but
  enough to verify the binary launches and logs its backend.
- No local rustup → MSRV 1.77 gate is CI-only (`msrv` job in `ci.yml`).
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets
  --locked -- -D warnings` must pass before a push.

## Known Bugs / To-Do
- Translucent global-sort on CPU may produce visible artifacts at chunk
  boundaries when adjacent meshes have mismatched depths.
- No Mipmap generation for the atlas; minification uses nearest filtering.
  Anisotropic filtering is not supported.
- Depth format is `Depth32Float` with clear 1.0 and `Less` comparison;
  some formats that work on discrete GPUs may differ on lavapipe.
- The `section_blocks_update` (0x49) live-update path is verified but has
  not been tested with mixed-section edits or state ids > 524287.
- Shader validation runs on every `cargo test -p rkb-gpu` invocation; if
  the WGSL / naga API changes (wgpu 0.20+), the tests would need updating.
  The MSRV gate at 0.19.4 + naga pins the API surface.