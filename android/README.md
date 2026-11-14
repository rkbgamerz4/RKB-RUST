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
