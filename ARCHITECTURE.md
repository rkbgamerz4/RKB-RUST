# Architecture

RKB RUST is split into a small core with no Android dependency and a thin
platform/JNI shell. The same runtime drives the headless CLI demo and the Android
app, so logic can be tested on a normal host.

## Data flow

```text
Kotlin render thread
  └─ nativeRenderFrame(handle, direct ByteBuffer, w, h, dt)
       └─ AndroidEngine
            └─ Runtime::frame(dt, DirectBufferSurface)
                 ├─ lifecycle drain + touch events
                 ├─ fixed-step Game::update
                 ├─ Game::render -> DrawList
                 ├─ SoftwareBackend::execute -> Framebuffer (RGBA8)
                 └─ Surface::present -> direct ByteBuffer
  └─ bitmap.copyPixelsFromBuffer + Canvas.drawBitmap
```

On the host, `HeadlessSurface` replaces `DirectBufferSurface`; `rkb-demo` uses
the same `Runtime` with the `rkb-sandbox` scene, and `rkb-minecraft`'s render
test drives `Runtime<MinecraftGame>` against a generated save.

`rkb-mc` is the data layer for Minecraft 1.21 saved worlds, and `rkb-minecraft`
is the client built on it: the Android app now runs `Runtime<MinecraftGame>`,
which opens a save, streams and meshes chunks around the player, and submits
depth-tested 3D triangles alongside the 2D HUD.

## Minecraft client data flow

```text
MinecraftGame::new(Some(save_dir))
  └─ rkb_mc::World::open (level.dat, DataVersion gate)
       └─ per-chunk: Anvil section -> block-state palette -> block lookup
MinecraftGame::update
  ├─ Player::step -> WorldQuery -> World::block_at (collision)
  └─ ensure_area + load_one (2 chunks/step)
       └─ build_chunk_mesh: 3x3 neighbourhood, hidden-face culling
MinecraftGame::render
  ├─ Camera::view_projection + Frustum::contains_aabb
  ├─ back-face culling, near-plane clipping -> DrawCommand::Triangle3D
  └─ sky gradient, distance fog, crosshair, joystick/buttons
```

`rkb-mc` remains read-only: the client never writes the save and keeps only the
chunks near the player in memory.

## Crates

### `rkb-core`

No dependencies, no I/O. Contains 2D/3D math (`Vec2`, `Vec3`, `Mat4`, `Rect`),
a fixed-timestep accumulator with drop accounting, the lifecycle finite-state
machine, and the small `key = value` / `[section]` configuration parser.

The lifecycle states are `Created -> Running <-> Paused -> Destroyed`, with
explicit failure transitions. The runtime forwards stage changes to the game so
resources and controls can react deterministically.

### `rkb-platform`

Defines the presentation seam:

- `Surface`: size, `present(&[u8])`, `resize`.
- `FrameClock`: monotonic delta source.
- `HeadlessSurface`: counts frames, stores a copy, checks frame size.
- `SystemClock` and `ManualClock` implementations.
- `PlatformInfo` for diagnostics.

### `rkb-render`

A deterministic software rasterizer:

- `Color` with RGB/RGBA/hex parsing.
- `Framebuffer` with bounds and clipping access.
- Alpha blending and clipped lines, rectangles, circles, triangles.
- A depth-tested perspective path: `Vertex3D` carries clip-space coordinates
  and `1/w`; `project_clip` maps them to viewport pixels; `Triangle3D` is
  rasterized with perspective-correct depth against a resizable depth buffer.
  `Camera::project_polygon` clips polygons crossing the near plane first.
- Texture sampling.
- `DrawList`/`DrawCommand` + `RenderBackend` trait.
- `SoftwareBackend` executes a frame into a framebuffer and reports statistics.
- `Camera2D` maps world coordinates into the viewport.

2D commands keep their painter's-order behaviour; only `Triangle3D` participates
in the depth test. The backend trait is deliberately small so a GPU
implementation can be added without touching game code.

### `rkb-input`

Platform-independent touch model:

- `TouchState` tracks active pointers and produces `TouchEvent`s.
- `TapDetector`, `DragDetector`, `PinchDetector`.
- `VirtualJoystick` and `TouchButton` with hit testing and layout.

Kotlin queues Android `MotionEvent` samples on the UI thread. The single render
thread drains the queue immediately before `nativeRenderFrame` and calls
`nativeTouch`, so all native state stays confined to one thread. Pointer IDs are
preserved and the queue is bounded.

### `rkb-resource`

- `AssetPath` normalizes slash-separated paths, rejects `..`, absolute paths,
  backslashes, NULs, and control characters.
- `AssetId` is a stable FNV-1a hash of the normalized path.
- `MemorySource` and `FileSource` implement `AssetSource`. `FileSource` resolves
  canonical paths and rejects symlink escapes.
- `RKBPAK1` is a deterministic archive: sorted entries, explicit offsets,
  sizes, and per-entry checksums.
- `ResourceManager` provides a byte-budgeted LRU cache and statistics.

### `rkb-mc`

Minecraft: Java Edition 1.21 saved-world compatibility, with no Android or GPU
dependency:

- `nbt`: Java NBT read/write, modified UTF-8, gzip/zlib, strict limits.
- `region`: Anvil `.mca` headers, chunk locations, gzip/zlib/uncompressed
  chunks, and a deterministic `RegionWriter` for synthetic fixtures.
- `chunk`: 1.21 section parsing, block-state palettes, non-spanning packed
  values, and world-Y block lookup.
- `registry`: block-state identities and defaults from a user-supplied,
  version-pinned Mojang `blocks.json` report.
- `version`: explicit DataVersion descriptors. Only 1.21 (3953) and 1.21.1
  (3955) are marked verified.
- `world`: validates `level.dat`, opens `region/r.X.Z.mca` on demand, validates
  chunk coordinates and versions, and caches loaded chunks.

`rkb-world` is the inspection CLI (`info`, `block`, `chunk`, `registry`). The
crate contains no Minecraft assets, jars, or world files. Tests generate their
own NBT and region bytes. See [`docs/minecraft-1.21.md`](docs/minecraft-1.21.md).

```text
World::open(save)
  ├─ level.dat (gzip NBT) -> DataVersion -> Versions::require
  └─ block_at(x, y, z)
       ├─ chunk coords = floor(x/16), floor(z/16)
       ├─ region/r.floor(chunk/32).mca -> RegionFile
       ├─ decompress -> NBT -> Chunk::parse
       └─ section Y = floor(y/16) -> palette cell
```

### `rkb-runtime`

Owns `EngineConfig`, `Lifecycle`, `TouchState`, `Framebuffer`, and the active
`Game`. `Runtime::frame` clamps delta time, drains lifecycle events, forwards
touch events, runs zero or more fixed update steps, calls `render`, executes the
draw list, and presents. The draw list is allocated once and reused, and
per-frame framebuffer hashing is optional (`RuntimeConfig::checksum_frames`) so
present-only hosts such as Android avoid scanning every pixel.

A viewport resize reallocates the framebuffer and cancels all active touches
before notifying the game. Touch coordinates are surface-relative, so keeping
pointers across a resize or rotation would deliver stale positions.

### `rkb-sandbox`

The executable proof of the original stack, now the headless demo only. It
embeds `config/engine.rkb` and `scenes/sandbox.rkb` with `include_bytes!`, loads
them through the resource manager, parses config, and builds a scene with balls,
a player, a grid, a virtual joystick, a burst button, tap spawning, pinch zoom,
and camera panning. The Android app no longer uses it; `rkb-demo` and its tests
do.

### `rkb-mc`

Minecraft: Java Edition 1.21 save compatibility, with no Minecraft code or data.
`nbt` parses modified-UTF-8 NBT, `region` reads Anvil `.mca` files and their zlib
chunks, `chunk` decodes section palettes and block-state cells, `version` gates
DataVersions (3953 for 1.21, 3955 for 1.21.1), and `world` exposes `level.dat`,
spawn, chunk cache, and `block_at`. `fixture` generates synthetic saves for tests.

### `rkb-minecraft`

The playable client. `blocks` maps block names to approximate colours and a
solidity rule, `mesh` builds per-chunk face lists with neighbour culling,
`camera` provides view/projection, frustum planes, and polygon clipping,
`player` implements AABB collision, gravity, jumping, and flight, `demo_world`
writes the synthetic island, and `game` ties them into a `Game`: chunk
streaming, touch input, scene drawing, and status text. The `rkb-fixture` binary
writes that island for CI and emulator verification.

### `rkb-android`

- `bridge.rs` contains host-testable `AndroidEngine`, `DirectBufferSurface`
  (checks the direct buffer capacity and framing), touch conversion, pause/
  resume/destroy, and stats. `AndroidEngine` wraps `Runtime<MinecraftGame>` and
  reports world state, chunk/face/triangle counts, and player position.
- `lib.rs` exports `Java_com_rkb_rust_RkbNative_native*` functions, allocates
  the engine with `Box::into_raw`, converts the optional world path, logs the
  `minecraft client: world=...` startup line, and logs through Android's log
  facility on device.
- The JNI surface is intentionally narrow: create (size plus world path),
  destroy, resize, render, touch, pause, resume, stats, version.

Kotlin `MainActivity` owns a single render thread, a direct `ByteBuffer`, an
`ARGB_8888` bitmap, and a `SurfaceView`. It requests `RGBA_8888` surface format,
resolves the save directory (external files first, then internal), creates the
engine on the first frame with that path, forwards lifecycle callbacks (after
stopping the render thread), queues touch samples for the render thread, and
copies the presented frame into the bitmap. Rendering happens at 0.6x the
surface size and the bitmap is scaled up on draw; touch coordinates are scaled
the same way. A status `TextView` overlays world status, chunk/face/triangle
counts, and position. The frame clock resets on every thread start, and
`surfaceDestroyed` releases the bitmap and direct buffer only after the render
thread has joined. A failed `System.loadLibrary` is captured and rendered as an
error screen rather than aborting the process.

`crates/rkb-android/tests/jni_contract.rs` compares every Kotlin `external fun`
name, parameter type, and return type against the Rust JNI exports, so contract
drift fails on a normal host instead of at runtime on a device.

## Timing

The runtime uses a fixed simulation step from `EngineConfig` (default 60 Hz).
Frame delta is clamped to avoid spiral-of-death behavior; excess time is counted
as dropped steps. Rendering happens once per presented frame. The Kotlin host
paces frames at roughly 60 Hz with `Thread.sleep`; a future backend can replace
this with `Choreographer`/vsync.

## Error handling

- No `unwrap` in production paths; errors are explicit enums with `Display`.
- Config and asset errors include context (path, line, or archive entry).
- Android failures are logged and surfaced as `false`/`0`/empty stats rather
  than aborting the process.
- `panic = "abort"` is set for release builds to keep the native library small.

## Threading and ownership

- The Rust side is not internally synchronized. JNI calls must be confined to the
  Kotlin render thread.
- `nativeCreate` returns an opaque `jlong` pointer; `nativeDestroy` consumes it
  exactly once and zeroes the Kotlin field. Pause and destroy first stop and
  join the render thread; if that join times out, the host skips the native call
  (and may intentionally leak the handle) rather than risk use-after-free.
- `present` writes only into the Kotlin-owned direct buffer for the duration of
  the call.
- No global mutable state is used by the runtime.

## Build and CI

- Host CI: fmt, clippy (`-D warnings`), tests, release build, demo artifact.
- Android CI: install NDK, cross-compile `aarch64-linux-android` (including
  `rkb-mc` and `rkb-minecraft`), copy the `.so` into `jniLibs`, build the debug
  APK with Gradle, verify the packaged `lib/arm64-v8a/librkb_android.so`, inspect
  manifest badging, and upload the APK plus a verification report.
- Emulator CI: build a universal x86_64/ARM64 APK, boot an API 34 emulator,
  generate the synthetic save with `rkb-fixture`, push it into app storage with
  `run-as`, launch the activity, and assert from logcat that the client loaded
  the save, streamed chunks, meshed faces, rendered triangles, presented a
  frame, produced no crash, and is not running the old sandbox (`balls=`).
- All workflows use read-only `contents` permissions and pinned action commits.
