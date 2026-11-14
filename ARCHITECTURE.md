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
the same `Runtime` and `rkb-sandbox` scene.

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
- Texture sampling.
- `DrawList`/`DrawCommand` + `RenderBackend` trait.
- `SoftwareBackend` executes a frame into a framebuffer and reports statistics.
- `Camera2D` maps world coordinates into the viewport.

The backend trait is deliberately small so a GPU implementation can be added
without touching game code.

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

### `rkb-runtime`

Owns `EngineConfig`, `Lifecycle`, `TouchState`, `Framebuffer`, and the active
`Game`. `Runtime::frame` clamps delta time, drains lifecycle events, forwards
touch events, runs zero or more fixed update steps, calls `render`, executes the
draw list, and presents. It also handles viewport resize without resetting
unrelated game state.

### `rkb-sandbox`

The executable proof of the stack. It embeds `config/engine.rkb` and
`scenes/sandbox.rkb` with `include_bytes!`, loads them through the resource
manager, parses config, and builds a scene with balls, a player, a grid, a
virtual joystick, a burst button, tap spawning, pinch zoom, and camera panning.
It is used by both the demo and Android.

### `rkb-android`

- `bridge.rs` contains host-testable `AndroidEngine`, `DirectBufferSurface`
  (checks the direct buffer capacity and framing), touch conversion, pause/
  resume/destroy, and stats.
- `lib.rs` exports `Java_com_rkb_rust_RkbNative_native*` functions, allocates
  the engine with `Box::into_raw`, and logs through Android's log facility on
  device.
- The JNI surface is intentionally narrow: create, destroy, resize, render,
  touch, pause, resume, stats, version.

Kotlin `MainActivity` owns a single render thread, a direct `ByteBuffer`, an
`ARGB_8888` bitmap, and a `SurfaceView`. It creates the engine on the first
frame, forwards lifecycle callbacks (after stopping the render thread), queues
touch samples for the render thread, and copies the presented frame into the
bitmap.

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
- Android CI: install NDK, cross-compile `aarch64-linux-android`, copy the `.so`
  into `jniLibs`, build the debug APK with Gradle, upload both artifacts.
- All workflows use read-only `contents` permissions and pinned action commits.
