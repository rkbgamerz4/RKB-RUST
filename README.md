# RKB RUST

RKB RUST is an experimental, dependency-light **Rust game foundation for Android
ARM64**, with a headless CLI demo and a native Android host.

The project currently ships a deterministic software renderer, fixed-timestep
runtime, touch/gesture input, a path-safe asset system with a custom `RKBPAK1`
archive format, and a JNI bridge that presents frames through a Kotlin
`SurfaceView`. It also contains the **first playable milestone of a Minecraft:
Java Edition 1.21 client**: user-supplied saves are read through `rkb-mc`,
chunks are streamed and meshed, and opaque block geometry is rendered with a
first-person camera, collision, and touch movement.

This is not a complete Minecraft client. There are no textures, lighting,
block breaking/placing, inventory, mobs, survival, or networking yet. The
project contains no Minecraft assets, jars, or game files, and it does not
bundle a world.

## Status

| Area | State |
| --- | --- |
| Rust workspace | Building on stable Rust; 175 unit/integration tests |
| Renderer | Software RGBA8 rasterizer with primitives, alpha blending, 2D camera, and a depth-tested perspective 3D triangle path |
| Runtime | Fixed timestep, accumulation and drop accounting, lifecycle FSM, resize handling |
| Input | Multi-touch state, tap/drag/pinch detectors, virtual joystick, touch button |
| Resources | Traversal-safe paths, memory/file sources, checksummed `RKBPAK1` archives, LRU cache |
| Minecraft data | Read-only 1.21/1.21.1 saved-world reader: NBT, Anvil regions, chunk palettes, block states, registry reports, `rkb-world` CLI |
| Minecraft client | `rkb-minecraft`: 1.21 save loading, chunk streaming and face meshing, depth-tested flat-colour block rendering, first-person camera, walking, jumping, creative flight, AABB collision, touch look/joystick/buttons |
| Android | `aarch64-linux-android` JNI library, Kotlin `SurfaceView` host that launches the Minecraft client, 0.6x render scaling, status overlay, lifecycle and touch wiring |
| CI | Format, clippy, tests, release build, demo artifact, NDK ARM64 build, APK and ABI verification, emulator world-load/render assertions |

The software renderer is intentional. It keeps the core testable on any host and
gives Android a working end-to-end path without requiring GPU drivers in CI. The
`RenderBackend` and draw-list seam is where a future OpenGL ES/Vulkan backend
belongs.

## Minecraft 1.21 client (first milestone)

`rkb-minecraft` is the client crate. It reads a save you already own, streams
chunks around the player, meshes exposed block faces, and renders them as flat
per-block colours with per-face shading. The Android APK launches this client;
the old ball sandbox is no longer wired into the app and remains only as the
headless stack demo.

Implemented:

- DataVersion-gated loading of 1.21 and 1.21.1 saves (`level.dat` + `region/`).
- Chunk streaming across a 7x7 chunk area, neighbour-aware face culling, and
  frustum culling.
- Depth-tested perspective projection, near-plane clipping, and back-face
  culling in the software renderer.
- Spawn selection from `level.dat`, ground snapping, gravity, jumping,
  creative flight, AABB collision, and fall respawn.
- Touch controls: left joystick, drag-to-look, JUMP, FLY, and DOWN while flying.
- Android host at 0.6x render scale with a status overlay showing the loaded
  save, streamed chunk count, face count, triangle count, and position.

Not implemented yet: textures/resource packs, biome tints, lighting/shadows,
block shapes (slabs, stairs, fences, and other non-cube blocks are treated as
full cubes for both rendering and collision), block breaking or placing,
inventory, crafting, mobs, survival, sound, and networking. Block colours are
approximations, not extracted Mojang textures.

### Run a world on Android

The app looks for a save in its external files directory first, then its private
storage:

- `Android/data/com.rkb.rust/files/minecraft/world` (external)
- `files/minecraft/world` (internal)

For a debuggable build, push a save with ADB:

```bash
adb push /path/to/my-world /data/local/tmp/rkb-world
adb shell "run-as com.rkb.rust sh -c \
  'rm -rf files/minecraft/world && mkdir -p files/minecraft && \
   cp -r /data/local/tmp/rkb-world files/minecraft/world'"
```

If no save is present, the app starts with a status message and instructions
instead of substituting demo content.

### Generate a verification world

The `rkb-fixture` binary writes a synthetic, structurally valid 1.21 save. It
contains no Mojang data and is intended for tests and emulator verification:

```bash
cargo run -p rkb-minecraft --bin rkb-fixture -- /tmp/rkb-demo-world
cargo run -p rkb-mc --bin rkb-world -- info /tmp/rkb-demo-world
```

The host render test loads the same fixture through `Runtime<MinecraftGame>`:

```bash
RKB_RENDER_OUT=/tmp/rkb-demo.ppm \
  cargo test -p rkb-minecraft --test demo_render -- --nocapture
```

## Minecraft 1.21 world inspection

`rkb-mc` reads data from a world the user already owns. It bundles no Mojang
content and does not download anything.

```bash
cargo run -p rkb-mc --bin rkb-world -- info /path/to/world
cargo run -p rkb-mc --bin rkb-world -- block /path/to/world 0 -64 0
cargo run -p rkb-mc --bin rkb-world -- chunk /path/to/world 0 0
cargo run -p rkb-mc --bin rkb-world -- registry /path/to/blocks.json
```

Only `DataVersion` 3953 (1.21) and 3955 (1.21.1) are verified. Other versions
are rejected instead of being guessed. See
[`docs/minecraft-1.21.md`](docs/minecraft-1.21.md) for the format, limits, and
roadmap.

## Quick start (host)

Requirements: stable Rust (MSRV 1.77) and Cargo.

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Run the interactive sandbox logic headlessly and write a frame (the sandbox is
the stack demo, not the Android game):

```bash
cargo run -p rkb-demo --release -- \
  --frames 120 --width 960 --height 540 --out artifacts/demo.ppm
```

The demo prints frame statistics and writes a binary PPM image. Convert it to
PNG with ImageMagick if desired:

```bash
magick artifacts/demo.ppm artifacts/demo.png
```

## Android build

Requirements:

- Android SDK 35 and Android NDK r26/r27
- JDK 17+
- Rust target `aarch64-linux-android`

```bash
# From the repository root
export ANDROID_NDK_HOME=/path/to/android-ndk
./scripts/build-android.sh          # cross-compile + copy librkb_android.so
cd android
./gradlew :app:assembleDebug        # build app/build/outputs/apk/debug/app-debug.apk
```

`scripts/build-android.sh` discovers the NDK from `ANDROID_NDK_HOME`,
`ANDROID_NDK_LATEST_HOME`, or `$ANDROID_HOME/ndk`, cross-compiles
`crates/rkb-android`, and places the library in
`android/app/src/main/jniLibs/arm64-v8a/`. The APK targets `arm64-v8a` only.

CI performs the same NDK cross-compile and Gradle build on Ubuntu and uploads
both the native library and the debug APK. The workflow also inspects the APK
before upload and fails if `lib/arm64-v8a/librkb_android.so` is missing, if
another ABI is present, or if the package, minimum SDK, target SDK, or
launchable activity does not match the expected values.

### Android host behavior

- The product is landscape-only (`sensorLandscape`). `Runtime::resize` rebuilds
  the framebuffer and cancels active touches, markers, and gesture detectors so
  a rotation cannot drag stale coordinates into the new layout.
- The `SurfaceView` requests `PixelFormat.RGBA_8888`; frames are copied from the
  Rust RGBA8 buffer into an `ARGB_8888` bitmap on the render thread.
- The render thread's frame clock resets on resume and surface recreation, so
  the first frame after a pause does not inherit a long delta.
- A missing or ABI-mismatched native library shows an error screen instead of
  crashing during activity startup.
- Touch samples are queued on the UI thread and drained on the render thread;
  the queue is bounded and cleared when rendering stops.

### Measured host performance

These are headless CLI measurements on the development host (Termux, Android
`aarch64`, 8 logical CPUs), not phone FPS. The demo runs 600 frames in release
mode:

| Render size | Full frame checksums | Present-only (`--no-checksum`) |
| --- | --- | --- |
| 1280x720 | ~16.7 ms/frame | ~4.9 ms/frame |
| 960x540 | ~8.9 ms/frame | ~2.5 ms/frame |

The difference is the cost of hashing the entire RGBA frame twice (runtime
checksum plus the headless surface). Android disables the runtime checksum and
still pays one direct-buffer copy plus one bitmap copy per frame, so device
timing must be measured on hardware. No physical device was available during
this phase; installation, launch, touch, rotation, and background/foreground
behavior remain manual checks.

## Controls

Minecraft client (Android):

- Left joystick: walk.
- Drag anywhere outside the controls: look around.
- **JUMP** (bottom-right): jump, or ascend while flying.
- **FLY** (above jump): toggle creative flight.
- **DOWN** (left of jump, visible while flying): descend.

Headless sandbox demo (`rkb-demo` and `rkb-sandbox` tests):

- Drag on the left side: virtual joystick (moves the player).
- Tap elsewhere: spawn a bouncing ball.
- Two-finger pinch: zoom the camera.
- Tap **BURST** in the bottom-right: spawn a burst of balls.
- Drag on empty space: pan the camera.

## Workspace

| Crate | Responsibility |
| --- | --- |
| `rkb-core` | Math, fixed timestep, lifecycle FSM, config parser and `EngineConfig` |
| `rkb-platform` | `Surface`/`FrameClock` traits, headless surface, platform info |
| `rkb-render` | Colors, framebuffer, clipping/blending, primitives, draw lists, `Camera2D` |
| `rkb-input` | Touch points/events, tap/drag/pinch detectors, virtual controls |
| `rkb-resource` | Asset paths and IDs, memory/file sources, `RKBPAK1`, LRU cache |
| `rkb-runtime` | Fixed-step loop, lifecycle, input forwarding, frame presentation |
| `rkb-sandbox` | Embedded config/scene assets and the headless sandbox `Game` (no longer the Android game) |
| `rkb-demo` | Headless CLI that runs the sandbox and writes a PPM frame |
| `rkb-pak` | CLI to pack, list, verify, and unpack `RKBPAK1` archives |
| `rkb-mc` | Minecraft: Java Edition 1.21 NBT, Anvil regions, chunk palettes, block registry, world access, and `rkb-world` CLI |
| `rkb-minecraft` | 1.21 client: chunk streaming/meshing, camera, physics, touch controls, flat-colour block renderer, and the `rkb-fixture` generator |
| `rkb-android` | JNI exports and host-testable Android engine/bridge that launches the Minecraft client |

## Asset archives

The custom format is deterministic and dependency-free:

```text
magic[8] = "RKBPAK1\0"
version  u32
count    u32
entries  count * { path_len u16, path bytes, offset u32, size u32, checksum u64 }
data     concatenated payload bytes
```

```bash
./scripts/pack-assets.sh crates/rkb-sandbox/assets artifacts/assets.pak
cargo run -p rkb-pak -- list artifacts/assets.pak
cargo run -p rkb-pak -- verify artifacts/assets.pak
cargo run -p rkb-pak -- unpack artifacts/assets.pak artifacts/unpacked
```

Paths are normalized, reject `..` and absolute segments, and are validated again
during extraction. Each payload uses an FNV-1a checksum that is verified on read.

## Verification

```bash
./scripts/verify.sh
```

This runs formatting, clippy with warnings denied, the full test suite, a
release build, and the demo render. GitHub Actions runs the same host checks plus
the Android NDK cross-compile, Gradle APK build, and artifact uploads.

## Security and limitations

- No network access, no scripting runtime, no third-party game code, and no
  credentials in the repository.
- `rkb-mc` is read-only and covers only 1.21/1.21.1 DataVersions. It ignores
  biomes, heightmaps, block entities, entities, structures, and lighting, and
  does not decode LZ4 region compression.
- The Minecraft client is a first milestone, not a complete game. It renders
  blocks as opaque full cubes with flat approximate colours: no textures,
  resource packs, biome tints, lighting, shadows, or transparent blocks.
  Non-cube shapes such as slabs, stairs, and fences are treated as full cubes,
  so geometry and collision boxes for those blocks are approximate.
- Block breaking/placing, inventory, crafting, mobs, survival, sound, and
  networking are not implemented. Interaction is limited to moving and looking.
- World edits are never written back; the save is opened read-only and the
  client holds at most the chunks around the player.
- No Minecraft assets, jars, data packs, worlds, or registry reports are
  bundled. Users supply their own legally obtained files. The synthetic save
  from `rkb-fixture` is generated by RKB code and used only for verification.
- JNI handles are process-local raw pointers; the Kotlin host must call
  `nativeDestroy` exactly once and must not use a handle after destruction.
- Native code is single-threaded by contract; the Kotlin host confines calls to
  one render thread.
- The renderer is software-only today. Android renders at 0.6x resolution to
  stay interactive; frame rate on large or dense saves is not yet characterized
  on real hardware.
- Android installation, launch, touch, rotation, and background/foreground
  behavior are validated in CI on an x86_64 emulator against a synthetic save.
  Physical-device behavior and real user saves are not yet verified.
- The project is early-stage; APIs may change before `1.0`.

See [`ARCHITECTURE.md`](ARCHITECTURE.md), [`SECURITY.md`](SECURITY.md),
[`docs/horizon-analysis.md`](docs/horizon-analysis.md), and
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).

## License

Licensed under either of:

- Apache License, Version 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE))
- MIT license ([`LICENSE-MIT`](LICENSE-MIT))

at your option.
