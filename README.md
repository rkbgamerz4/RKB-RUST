# RKB RUST

RKB RUST is an experimental, dependency-light **Rust game foundation for Android
ARM64**, with a headless CLI demo and a native Android host.

The project currently ships a deterministic software renderer, fixed-timestep
runtime, touch/gesture input, a path-safe asset system with a custom `RKBPAK1`
archive format, and a JNI bridge that presents frames through a Kotlin
`SurfaceView`. It also contains a **voxel prototype** that reads a user-supplied
Minecraft: Java Edition 1.21/1.21.1 save (or creates RKB-authored terrain),
streams and meshes chunks, renders textured block geometry with a first-person
camera and collision, supports touch movement plus block breaking/placing, and
persists seed, player pose, and edits.

> **Direction change (2026-11-16).** The voxel prototype is a foundation, not the
> product. The target is a genuine Minecraft: Java Edition client with protocol,
> Microsoft authentication, vanilla-server multiplayer, client worlds, entities,
> inventory, and a GPU renderer. See [`docs/audit-current.md`](docs/audit-current.md),
> [`docs/horizon-comparison.md`](docs/horizon-comparison.md),
> [`docs/migration-plan.md`](docs/migration-plan.md), and
> [`docs/m1-verification.md`](docs/m1-verification.md). New protocol work lives
> on `feature/mc-java-protocol`.

This is not a complete Minecraft client. It speaks the 1.21.1 status, login,
and configuration protocol, but does not yet authenticate with Microsoft, enter
the play state, or join a world. There are no Mojang textures or resource packs,
dynamic lightmaps or shadows, inventory, mobs, or survival. The project contains
no Minecraft assets, jars, or game files; block textures are generated
procedurally by RKB code.

## Status

| Area | State |
| --- | --- |
| Rust workspace | Building on stable Rust; 309 unit/integration tests |
| Renderer | Software RGBA8 rasterizer with primitives, alpha blending, a 2D camera, texture sampling, and a reciprocal-depth perspective 3D path with per-vertex UV/light |
| Runtime | Fixed timestep, accumulation and drop accounting, lifecycle FSM, resize handling |
| Input | Multi-touch state, tap/drag/pinch detectors, virtual joystick, touch button |
| Resources | Traversal-safe paths, memory/file sources, checksummed `RKBPAK1` archives, LRU cache |
| Minecraft data | 1.21/1.21.1 saved-world reader and atomic writer: NBT, Anvil regions, chunk palettes and construction, block states, registry reports, `rkb-world` CLI |
| Minecraft protocol | `rkb-proto` + `rkb-net`: 1.21.1 (protocol 767) codecs, framing, zlib, AES/CFB8, RSA/PKCS#1 v1.5, Java server hash, handshake/status/login/configuration packets, and an offline-mode login through `FinishConfiguration`, verified against the official 1.21.1 server. Microsoft authentication (M2) and the play state are not implemented |
| Minecraft client | `rkb-minecraft`: default seeded terrain, save loading, background chunk streaming and face meshing, textured block rendering from a procedural atlas with face shading and ambient occlusion, first-person camera, walking/jumping/flight, AABB collision, raycast break/place, session persistence and autosave. This is the prototype being migrated |
| Android | `aarch64-linux-android` JNI library, Kotlin `SurfaceView` host that launches the Minecraft client, 0.6x render scaling, status/help overlay, lifecycle and touch wiring |
| CI | Format, clippy, tests, release build, generated and saved world renders, NDK ARM64 build, APK and ABI verification, emulator fresh-install and supplied-save assertions |

The software renderer is intentional. It keeps the core testable on any host and
gives Android a working end-to-end path without requiring GPU drivers in CI. The
`RenderBackend` and draw-list seam is where a future OpenGL ES/Vulkan backend
belongs.

## Minecraft 1.21 client (generated world milestone)

`rkb-minecraft` is the client crate. On a fresh install it creates a
deterministic, seeded overworld from RKB code; if the world directory contains
a user save it opens that instead. It streams chunks around the player, meshes
exposed block faces with edit awareness, and renders them with a procedurally
generated 16x16 block atlas, Minecraft-style directional face shading,
per-vertex ambient occlusion, and distance fog. The Android APK launches this
client; the old ball sandbox is no longer wired into the app and remains only as
the headless stack demo.

Implemented:

- Default generated world: seeded height/climate noise, plains/forest/desert/
  snowy/ocean biomes, water, bedrock, beaches, cacti, and cross-chunk oak trees.
  A 3x3 spawn area is generated eagerly and the rest arrives from one background
  `rkb-chunk-gen` worker.
- DataVersion-gated loading of 1.21 and 1.21.1 saves (`level.dat` + `region/`).
  A save wins over generated terrain when no RKB marker/session exists.
- Chunk streaming across a 7x7 chunk area, neighbour-aware face culling,
  frustum culling, and mesh refresh after edits.
- Procedural, deterministic block textures (16x16 tiles in a 128x64 atlas) for
  grass top/side, dirt, stone, cobblestone, sand, gravel, snow, ice, logs,
  leaves, planks, water, lava, bedrock, and more. No Mojang assets are copied.
- Per-face tile selection with upright UVs, Minecraft's 1.0/0.8/0.6/0.5
  directional face shading, and per-vertex ambient occlusion baked into the
  chunk mesh. Unknown block names keep a stable hashed tint.
- Depth-tested perspective projection with near-plane clipping, back-face
  culling, reciprocal-depth (`1/w`) rasterization, perspective-correct texture
  sampling, a smooth gradient sky, and full-strength distance fog.
- Spawn selection, ground snapping, gravity, jumping, creative flight, AABB
  collision, and fall respawn.
- Crosshair raycast (5-block reach) with **BREAK** and **PLACE** buttons.
  Breaking removes a block; placing puts oak planks on the targeted face.
- Persistence: `rkb-session.json` stores seed, spawn, player pose, and block
  edits atomically. Generated worlds also write `level.dat` and Anvil region
  files, so `rkb-world` can inspect them; RKB itself reloads from the session
  seed and overlay. Autosave runs every 20 seconds and on pause/stop/destroy.
- Touch controls: left joystick, drag-to-look, JUMP, FLY, DOWN while flying,
  BREAK, and PLACE.
- Android host at 0.6x render scale with a status overlay showing world kind,
  streamed/visible chunk counts, face/triangle/block counts, edit count,
  position, and save status.

Not implemented yet: resource-pack/Mojang textures, biome colour blending,
dynamic lightmaps and shadows, block shapes (slabs, stairs, fences, and other
non-cube blocks are treated as full cubes for both rendering and collision),
inventory, crafting, mobs, survival, sound, and networking. The atlas is an
original procedural approximation, not extracted Mojang art. Generated terrain is
RKB terrain, not Mojang's world generator; it reproduces no vanilla seed.

### Run a world on Android

With no save, the app generates a world in its external files directory, then
private storage:

- `Android/data/com.rkb.rust/files/minecraft/world` (external)
- `files/minecraft/world` (internal)

To play a save you own, push it into either location. The app chooses an
existing `level.dat`; an RKB-generated world is recognized by its
`rkb-generated-world` marker and `rkb-session.json`.

For a debuggable build:

```bash
adb push /path/to/my-world /data/local/tmp/rkb-world
adb shell "run-as com.rkb.rust sh -c \
  'rm -rf files/minecraft/world && mkdir -p files/minecraft && \
   cp -r /data/local/tmp/rkb-world files/minecraft/world'"
```

Deleting the app's world directory (or app data) restores the generated-world
default. If no writable directory is available, the client reports that instead
of substituting demo content.

### Generate a verification world

The `rkb-fixture` binary writes a synthetic, structurally valid 1.21 save. It
contains no Mojang data and is intended for tests and emulator verification:

```bash
cargo run -p rkb-minecraft --bin rkb-fixture -- /tmp/rkb-demo-world
cargo run -p rkb-mc --bin rkb-world -- info /tmp/rkb-demo-world
```

The host render tests load both world sources through `Runtime<MinecraftGame>`:

```bash
RKB_RENDER_OUT=/tmp/rkb-demo.ppm \
  cargo test -p rkb-minecraft --test demo_render -- --nocapture
RKB_RENDER_OUT=/tmp/rkb-generated.ppm \
  cargo test -p rkb-minecraft --test generated_render -- --nocapture
```

## Minecraft 1.21.1 network (M1)

`rkb-proto` and `rkb-net` implement the protocol-767 path from handshake to the
end of configuration:

- AES-128/CFB8 with separate cipher states per direction, RSA/X.509 parsing and
  PKCS#1 v1.5 encryption, and the Java signed-hex server hash.
- Compression activation through `SetCompression`, with threshold enforcement
  and zlib validation.
- Login (`LoginStart`, encryption request/response, plugin and cookie requests,
  login success, login acknowledged) and configuration (client information,
  known packs, registry data, tags, feature flags, keep-alive, ping,
  `FinishConfiguration`).
- Typed disconnect reasons: JSON in login, anonymous network NBT in
  configuration.

The offline-mode path was verified against the official Mojang 1.21.1 server:
the server logged `RKBTest joined the game` after the client finished
configuration. The client then disconnects intentionally because M1 does not
decode play packets. See [`docs/m1-verification.md`](docs/m1-verification.md)
for the exact evidence and limits.

```bash
# against an offline-mode server you are allowed to run
cargo run -p rkb-net --bin rkb-login -- 127.0.0.1:25565 Steve

cargo test -p rkb-proto
cargo test -p rkb-net
```

Online-mode servers require Microsoft authentication. M1 fails explicitly when
a server requests it; it does not fake a session or store credentials. That
flow is the M2 gate.

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
- **BREAK** (crosshair): remove the targeted block within 5 blocks.
- **PLACE** (crosshair): place oak planks on the targeted face; rejected if the
  destination intersects the player.
- Saves automatically every 20 seconds and when the app pauses or stops; the
  overlay reports the result.

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
| `rkb-render` | Colors, framebuffer, clipping/blending, primitives, draw lists, `Camera2D`, texture sampling, reciprocal-depth 3D path |
| `rkb-input` | Touch points/events, tap/drag/pinch detectors, virtual controls |
| `rkb-resource` | Asset paths and IDs, memory/file sources, `RKBPAK1`, LRU cache |
| `rkb-runtime` | Fixed-step loop, lifecycle, input forwarding, frame presentation |
| `rkb-sandbox` | Embedded config/scene assets and the headless sandbox `Game` (no longer the Android game) |
| `rkb-demo` | Headless CLI that runs the sandbox and writes a PPM frame |
| `rkb-pak` | CLI to pack, list, verify, and unpack `RKBPAK1` archives |
| `rkb-mc` | Minecraft: Java Edition 1.21 NBT, Anvil regions, chunk palettes, block registry, world access, and `rkb-world` CLI |
| `rkb-minecraft` | 1.21 client: chunk streaming/meshing, procedural block atlas, camera, physics, touch controls, textured block renderer, and the `rkb-fixture` generator |
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
release build, the demo render, and both Minecraft render tests (synthetic save
and fresh generated world). GitHub Actions runs the same host checks plus the
Android NDK cross-compile, Gradle APK build, and artifact uploads.

## Security and limitations

- No network access, no scripting runtime, no third-party game code, and no
  credentials in the repository.
- `rkb-mc` covers only 1.21/1.21.1 DataVersions for reading. It ignores biomes,
  heightmaps, block entities, entities, structures, and lighting, and does not
  decode LZ4 region compression. It writes RKB-generated worlds only; a user
  save is never modified in place.
- The Minecraft client renders blocks as opaque full cubes with an original
  procedural texture atlas, directional face shading, per-vertex ambient
  occlusion, and distance fog. It does not load resource packs, apply biome
  tint blending, sample lightmaps, cast shadows, or render transparent blocks.
  Non-cube shapes such as slabs, stairs, and fences are treated as full cubes,
  so geometry and collision boxes for those blocks are approximate.
- Generated terrain is RKB terrain produced from a seed. It does not reproduce
  Mojang's world generator, vanilla seeds, caves, ores, or structures.
- Block breaking/placing works through a client edit overlay (5-block reach,
  oak planks on place). Inventory, crafting, mobs, survival, sound, and
  networking are not implemented.
- Edits to a user save live in `rkb-session.json`, not in the user's region
  files. Generated worlds do write `level.dat` and region files atomically.
  The client keeps at most the chunks around the player in memory.
- No Minecraft assets, jars, data packs, worlds, or registry reports are
  bundled. Users supply their own legally obtained files. The synthetic save
  from `rkb-fixture` is generated by RKB code and used only for verification.
- JNI handles are process-local raw pointers; the Kotlin host must call
  `nativeDestroy` exactly once and must not use a handle after destruction.
- Native code is single-threaded by contract; the Kotlin host confines calls to
  one render thread. Chunk generation uses one dedicated Rust worker.
- The renderer is software-only today. Android renders at 0.6x resolution to
  stay interactive; frame rate on large or dense worlds is not yet characterized
  on real hardware.
- Android installation, launch, touch, rotation, and background/foreground
  behavior are validated in CI on an x86_64 emulator for both a fresh generated
  world and a synthetic save. Physical-device behavior and real user saves are
  not yet verified.
- The project is early-stage; APIs may change before `1.0`.

See [`ARCHITECTURE.md`](ARCHITECTURE.md), [`SECURITY.md`](SECURITY.md),
[`docs/horizon-analysis.md`](docs/horizon-analysis.md), and
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).

## License

Licensed under either of:

- Apache License, Version 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE))
- MIT license ([`LICENSE-MIT`](LICENSE-MIT))

at your option.
