# RKB RUST — Android host

This module is a minimal host for the Rust native library. The Rust engine does
not use OpenGL yet: it renders RGBA8 frames in software and Kotlin presents them
through a `SurfaceView` bitmap.

## Requirements

- Android SDK 35
- Android NDK (any recent r26/r27)
- JDK 17
- Rust target `aarch64-linux-android`

## Build

From the repository root:

```bash
./scripts/build-android.sh
cd android
./gradlew :app:assembleDebug
```

The script cross-compiles `crates/rkb-android`, copies the resulting
`librkb_android.so` into `app/src/main/jniLibs/arm64-v8a/`, and the Gradle build
packages it into an APK.

Touch events are queued on the UI thread and delivered on the render thread so
the native engine is never accessed concurrently. The APK is a rendering and
touch-input sandbox. It is not a Minecraft client and does not claim
compatibility with Minecraft or any third-party game.

## Host behavior

- The activity is landscape-only (`sensorLandscape`) and requests a
  `PixelFormat.RGBA_8888` surface.
- The render thread owns all native calls. `onPause` stops and joins it before
  `nativePause`; `onResume` calls `nativeResume` and restarts it.
- Frame timing resets on resume and surface recreation, so a pause cannot
  produce one huge simulation delta.
- A surface resize or rotation calls `Runtime::resize`, which reallocates the
  framebuffer and cancels active touches, markers, and gestures.
- `surfaceDestroyed` releases the bitmap and direct buffer after the render
  thread has stopped.
- If `System.loadLibrary("rkb_android")` fails, the activity shows an error
  screen instead of crashing.

## Verification

CI cross-compiles the ARM64 library, builds the debug APK, and checks that the
APK contains exactly `lib/arm64-v8a/librkb_android.so` with package
`com.rkb.rust`, min SDK 24, target SDK 35, and launch activity
`com.rkb.rust.MainActivity`. The host test suite also checks the Kotlin
declarations against the JNI exports.

A physical Android device was not available during the stabilization phase.
Installation, launch, touch, rotation, and background/foreground behavior still
need manual device testing.
