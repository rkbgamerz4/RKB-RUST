# RKB RUST

RKB RUST is an experimental, dependency-light **Rust game foundation for Android
ARM64**, with a headless CLI demo and a native Android host.

The project currently ships a deterministic software renderer, fixed-timestep
runtime, touch/gesture input, a path-safe asset system with a custom `RKBPAK1`
archive format, and a JNI bridge that presents frames through a Kotlin
`SurfaceView`. It is a foundation and an interactive sandbox — **not a game
engine with a full editor, and not a Minecraft client**. It does not claim
compatibility with Minecraft or any third-party game.

## Status

| Area | State |
| --- | --- |
| Rust workspace | Building on stable Rust; 90+ unit/integration tests |
| Renderer | Software RGBA8 rasterizer with primitives, clipping, alpha blending, draw lists, and `Camera2D` |
| Runtime | Fixed timestep, accumulation and drop accounting, lifecycle FSM, resize handling |
| Input | Multi-touch state, tap/drag/pinch detectors, virtual joystick, touch button |
| Resources | Traversal-safe paths, memory/file sources, checksummed `RKBPAK1` archives, LRU cache |
| Android | `aarch64-linux-android` JNI library, Kotlin `SurfaceView` host, lifecycle and touch wiring |
| CI | Format, clippy, tests, release build, demo artifact, NDK ARM64 build, APK artifact |

The software renderer is intentional. It keeps the core testable on any host and
gives Android a working end-to-end path without requiring GPU drivers in CI. The
`RenderBackend` and draw-list seam is where a future OpenGL ES/Vulkan backend
belongs.

## Quick start (host)

Requirements: stable Rust (MSRV 1.77) and Cargo.

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Run the interactive sandbox logic headlessly and write a frame:

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
both the native library and the debug APK.

## Controls in the sandbox

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
| `rkb-sandbox` | Embedded config/scene assets and the interactive sandbox `Game` |
| `rkb-demo` | Headless CLI that runs the sandbox and writes a PPM frame |
| `rkb-pak` | CLI to pack, list, verify, and unpack `RKBPAK1` archives |
| `rkb-android` | JNI exports and host-testable Android engine/bridge |

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
- JNI handles are process-local raw pointers; the Kotlin host must call
  `nativeDestroy` exactly once and must not use a handle after destruction.
- Native code is single-threaded by contract; the Kotlin host confines calls to
  one render thread.
- The renderer is software-only today. Performance is adequate for the sandbox,
  not for large 3D scenes.
- The project is early-stage; APIs may change before `1.0`.

See [`ARCHITECTURE.md`](ARCHITECTURE.md) and [`SECURITY.md`](SECURITY.md).

## License

Licensed under either of:

- Apache License, Version 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE))
- MIT license ([`LICENSE-MIT`](LICENSE-MIT))

at your option.
