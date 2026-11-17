# M5 verification: Android networking (implemented, on-device join unverified)

Status: **implemented and host-tested on 2026-11-17; the Android build passes
CI, but no server join has been observed on an emulator or a physical device.**

This milestone is the Android networking slice identified after M3: move the
verified host-side server join into the existing Android ARM64 app without
changing the app/JNI architecture. It is not the gameplay M5 in
[`migration-plan.md`](migration-plan.md) (inventory clicks, containers, chat
sending); those items remain open.

The evidence policy is the same as the other milestones: "implemented" means
code plus tests; "verified" means observed against the named external system.

## Scope

```text
Kotlin overlay tap
  → AlertDialog (server, name, path to user's blocks.json)
  → UI thread queues the request on RkbView
  → render thread calls nativeJoinServer
  → Rust bridge spawns a background login/chunk-buffer thread
  → next render frame swaps in ServerWorld via MinecraftGame::load_server
  → existing chunk mesher renders only server chunks
```

No new app architecture was introduced. Kotlin still owns the render thread and
all native calls stay serialized on it; the socket reader runs on a Rust thread
inside `rkb-net`, as it does on the host.

## Code map

| Piece | Location |
| --- | --- |
| Background join, frame-time promotion, status | `crates/rkb-android/src/bridge.rs` |
| JNI export and string conversion | `crates/rkb-android/src/lib.rs` |
| Kotlin JNI declaration | `android/app/src/main/java/com/rkb/rust/RkbNative.kt` |
| Join dialog, render-thread queue, overlay hint | `android/app/src/main/java/com/rkb/rust/MainActivity.kt` |
| `INTERNET` permission | `android/app/src/main/AndroidManifest.xml` |
| In-place world swap | `crates/rkb-minecraft/src/game.rs` (`load_server`) |
| Server world and chunk conversion | `crates/rkb-net/src/server.rs` |

## Implemented behavior

- **Background connect.** `AndroidEngine::begin_join` loads the user-supplied
  `blocks.json` report and performs the offline-mode login on a worker thread.
  Login, configuration, Join Game, and the first position never run on the
  Android UI or render thread.
- **Render-thread handoff.** Kotlin queues `JoinRequest` values and drains them
  on the render thread, which is the only thread that calls the native engine.
  `frame()` polls the worker; a completed join calls
  `MinecraftGame::load_server`, which clears local meshes, edits, and queued
  chunk work and installs `WorldState::Server`.
- **Honest status.** Pending and failed joins appear in the stats overlay
  (`join=…`) and logcat. A missing or unreadable `blocks.json` produces a
  readable failure instead of a guessed registry.
- **No generated fallback.** The server path still renders only server chunks.
  If nothing arrives, the frame stays empty and `rkb-join` (the host check)
  fails.
- **Read-only server world.** A join dialog message states that movement and
  block edits are not sent to the server yet. The game rejects local break/place
  and disables local saves for server worlds.
- **Permissions.** The manifest adds `android.permission.INTERNET`. The app uses
  raw TCP, so no cleartext-HTTP exception is needed.
- **No credentials.** This path is offline-mode only. Microsoft sign-in is not
  wired into the Android app, and no token or client id is stored or shipped.

The UI is deliberately minimal: a tap on the existing status overlay opens the
join form. A launcher/account/server-list UI is M7 work.

## Host evidence

Command:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
```

Observed 2026-11-17: **385 passed, 0 failed, 0 ignored**, fmt clean, clippy
clean.

New or updated tests:

- `crates/rkb-android/tests/jni_contract.rs` now also covers `nativeJoinServer`,
  so the Kotlin declaration and the JNI export cannot drift silently.
- `crates/rkb-android/src/bridge.rs` has a join-failure test: it starts a join
  pointed at a nonexistent report, renders frames while the worker runs, and
  asserts that the engine stays usable, the join finishes, and the failure text
  reaches both `join_status()` and the stats overlay.
- `crates/rkb-net` still covers the server world end to end with a loopback
  protocol-767 server (chunk, teleport, block update, keep-alive, disconnect).

The host-side multiplayer render check was rerun after the Android changes:

```text
block registry: 26684 states from .../blocks.json
joined: position (-3.50, -60.00, -0.50)
world: server | Joined a protocol-767 server world; only server chunks are rendered.
rendered 373 frames: 45 meshed chunks (77 resident), 23050 faces, 7684 triangles, 15 visible
player: (-3.50, -60.01, -0.50) yaw 3.14 pitch -0.00 | multiplayer world; local saves are disabled
block at (0, -61, 0): minecraft:grass_block[snowy=false]
block at (0, -60, -2): minecraft:glowstone
```

The 77 resident columns and the grass/glowstone samples come from the official
1.21.1 test server described in [`m4-verification.md`](m4-verification.md), not
from generated terrain.

## Android build evidence

`crates/rkb-android` is cross-compiled by `scripts/build-android.sh`, and
`android.yml` / `android-emulator.yml` build the debug APK for `arm64-v8a` (and
`x86_64` for the emulator) with the new dependency on `rkb-net`. CI run links
are recorded below after dispatch.

The emulator smoke test still covers the fresh-install generated world and a
supplied save. It does **not** drive the join dialog, because it has no server
or `blocks.json` to join.

## Not verified

- **A join from the Android app.** No emulator or physical-device join was
  observed. The code path is host-tested, but Android runtime networking,
  permission behavior, and status-overlay updates on a device are unverified.
- **Physical ARM64 device.** No device is available; performance and thermal
  behavior are unmeasured.
- **Pause/destroy during a join.** The worker does not touch native state and
  sends its result through a channel, so a dropped receiver is harmless, but
  this has not been exercised on a device.
- **Online mode and Microsoft sign-in on Android.** Not wired; the M2/M3
  real-account gate remains open.
- **TLS to Microsoft/Xbox/Mojang.** Not reached from the host or Android.
- **Reconnect, server list, and account UI.** M7.
- **Local movement accepted by the server.** Still not sent in any host.

## Reproduce

```bash
# 1. Build the ARM64 library and APK (CI does the same).
./scripts/build-android.sh
cd android && ./gradlew :app:assembleDebug -Prkb.abis=arm64-v8a

# 2. Install and push the user's own version-pinned block report.
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
adb push generated/reports/blocks.json \
  /sdcard/Android/data/com.rkb.rust/files/minecraft/blocks.json

# 3. Start an offline-mode 1.21.1 server reachable from the device, launch the
#    app, and tap the status panel at the top of the screen.
```

The join dialog is offline mode only and states that the server world is
read-only. `blocks.json` must come from the user's own 1.21.1 installation; RKB
bundles no Mojang data.
