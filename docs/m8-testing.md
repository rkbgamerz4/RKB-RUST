# M8 Testing

## Test Classification

Tests are separated into distinct categories as required by the M8 definition of done:

| Category | Scope | Results |
|----------|-------|---------|
| **A) Termux lavapipe offscreen** | GPU tests against Mesa llvmpipe (CPU Vulkan) on Termux Android ARM64 | ✓ 25 pass (15 lib + 7 gpu + 3 shader) |
| **B) Desktop / CI GPU** | Same lavapipe tests on Ubuntu CI runner | ✓ Included in CI `gpu` job |
| **C) Android ARM64 compile** | `cargo check --target aarch64-linux-android` | ✓ CI compiles; local impossible (no rustup target) |
| **D) Emulator presentation** | Android emulator with SwiftShader/Lavapipe | ✓ Smoke test script configured with backend-line assertions |
| **E) Physical Mali** | Hardware GPU on device | ✗ Blocked from Termux (Mali ICD inaccessible); honest fail |

## Rust Unit Tests

**Command**: `cargo test --workspace --all-targets --locked`

**Results**:
- **453 passed / 0 failed** (35 test suites)
- All packages pass: rkb-auth, rkb-gpu, rkb-minecraft, rkb-net, rkb-android, rkb-proto, rkb-runtime, rkb-input
- Includes: format checks, clippy lints, GPU functional tests, shader validation

**GPU-specific tests** (`RKB_GPU_REQUIRED=1 cargo test -p rkb-gpu --locked -- --test-threads=1`):
- 15 library tests
- 7 GPU functional tests (mesh upload, frustum culling, depth buffer, atlas UV, cutout blending)
- 3 shader validation tests (pipeline entry points, uniform layout, world shader parse)
- GPU_EXIT = 0

## Offline Mode Server Testing

**Setup**: Local offline-mode Minecraft Java 1.21.1 server

**Command** (example):
```bash
java -Xms512M -Xmx1024M -jar server.jar nogui
# Server starts on port 25575 (default offline)
```

**What to test**:
1. Client can connect via LAN IP
2. Player spawns at world origin
3. Chunks stream in around the player
4. Player can walk and jump
5. Block breaking and placement (RCON verified)
6. Player disconnect and reconnect

**Note**: The existing `rkb-join-gpu` tool tests this path:
```bash
./target/release/rkb-join-gpu --server 127.0.0.1:25575 \
    --block-report generated/reports/blocks.json \
    --name RKBJoinM7 --seconds 55 --hold 45 \
    --width 640 --height 360 --backend vulkan --stats-every 5 \
    --out artifacts/m7/gpu-lavapipe.ppm
```

**M7 verification**: This was run successfully against an offline server with
flat creative world, view distance 3, 26684 block states. Output confirmed:
- 45 chunks meshed, 19622 tris, 24 draws
- Init 698.6 ms, 213.66 ms/frame, peak RSS 203756 KiB

## Emulator Testing

**Script**: `scripts/emulator-smoke-test.sh`

**What it tests**:
1. APK installation on emulator
2. Native library load (`RKB RUST native version`)
3. Engine creation (`native engine created`)
4. Frame presentation (`first (GPU )?frame presented`)
5. Backend line assertion (`renderer=gpu` or `renderer=software`)
6. No crashes (FATAL EXCEPTION, UnsatisfiedLinkError)

**Backend line assertions** (added for M8):
- `renderer=gpu ... first GPU frame presented` — if wgpu presents successfully
- `renderer=software ... using the M6 software renderer` — if GPU init fails, fallback
- Both outcomes are valid and must be reported honestly

**To run** (requires Android SDK, emulator):
```bash
bash scripts/emulator-smoke-test.sh
```

**Current status**: Script configured and syntax-checked (`bash -n` OK), but
requires Android SDK/emulator which is not available in the Termux environment.
The script will be dispatched via GitHub Actions CI on push to `feature/mc-java-protocol`.

## Physical Android Device Testing

**Status**: Not available in current environment

**Limitations**:
- Mali ICD `vulkan.mt6768.so` not accessible for the default namespace from Termux
- No adb/root access to install CI-built APK on device
- No physical device with accessible Mali GPU available for testing
- All GPU measurements are on llvmpipe (Mesa software rasterizer on CPU)

**Honest reporting** (from docs/m7-limitations.md):
- "Physical Mali GPU rendering: vendor ICD not accessible from Termux namespace;
  no on-device APK screenshots possible from current environment"
- "Local Android ARM64 compile: no NDK/SDK/rustup; CI-only verification"
- "Online/Microsoft-mode join: offline-mode only"

## Continuous Integration (GitHub Actions)

**CI jobs triggered on push to `feature/mc-java-protocol`**:

1. **Rust CI** (`.github/workflows/ci.yml`):
   - `quality`: `cargo fmt --check` + `cargo clippy -- -D warnings`
   - `test`: `cargo test --workspace --all-targets --locked` (453 pass)
   - `gpu`: `RKB_GPU_REQUIRED=1 cargo test -p rkb-gpu --locked -- --test-threads=1` (25 pass)
   - `msrv`: Installs Rust 1.77.0, runs `cargo test` as MSRV gate

2. **Android ARM64** (`.github/workflows/android.yml`):
   - `cargo check --locked --release --target aarch64-linux-android` for `rkb-android` and `rkb-mc`
   - Verifies Android targets compile and clippy-pass

3. **Android Emulator** (`.github/workflows/android-emulator.yml`):
   - Cross-compiles ARM64 and x86_64 emulator libraries
   - Builds universal debug APK
   - Launches emulator with `swiftshader_indirect` GPU
   - Asserts backend line (`assert_backend_line`) in logcat
   - Checks for crashes (`FATAL EXCEPTION`, `UnsatisfiedLinkError`)

**CI run IDs** (from latest push):
- Rust CI: run triggered on push to `feature/mc-java-protocol`
- Android ARM64: compile check job
- Emulator: smoke test job with backend-line assertions

## Manual Testing Checklist

### Pre-conditions
- [ ] Offline server running on port 25575 (or desired port)
- [ ] Server world loaded (flat creative recommended for join tests)
- [ ] `rkb-join-gpu` binary built (`cargo build --release -p rkb-gpu -p rkb-net`)

### Join Test
- [ ] `./target/release/rkb-join-gpu --server <ip:port> --name <name>`
- [ ] Client logs "joined: position ..."
- [ ] 45+ chunks meshed within first 30 frames
- [ ] Player position matches spawn position from server
- [ ] Camera yaw/pitch matches server-provided orientation

### Movement Test
- [ ] Joystick moves player forward/backward/left/right
- [ ] Jump button jumps when on ground
- [ ] Falling applies gravity (player falls if off ledge)
- [ ] Collision prevents walking through solid blocks
- [ ] Server position corrections applied (player may "rubber-band" back)

### Block Interaction Test (RCON-verified)
- [ ] `rcon setblock <x> <y> <z> minecraft:stone` → "Changed the block"
- [ ] Mesh updates visible after block edit (tris/draws/FNV change)
- [ ] PPM artifact FNV changes between baseline and edited frames

### Disconnect Test
- [ ] Server disconnect → game shows "Disconnected: <reason>"
- [ ] `Event::Disconnected` handled gracefully
- [ ] Client can reconnect to same server

### Output Verification
- [ ] `artifacts/m7/gpu-lavapipe.ppm` generated (baseline)
- [ ] `artifacts/m7/gpu-live.ppm` generated (after live-edit)
- [ ] FNV hashes differ between baseline and edited frames
- [ ] Performance stats in log: chunks, tris, draws, fps, RSS, GPU buffers

## Test Results Summary (as of commit 241c76b)

| Test Category | Result |
|--------------|--------|
| Rust unit tests (453) | ✓ Pass (35 suites) |
| GPU tests (25) | ✓ Pass against lavapipe |
| Offline server join | ✓ Verified (rkb-join-gpu) |
| Emulator smoke test | ⬜ Requires Android SDK (CI only) |
| Physical device | ⬜ Not accessible (Mali blocked) |
| Live-edit within-session | ✓ Verified (7 blocks, FNV delta) |
| Clippy ( -D warnings) | ✓ Exit 0 |
| Formatting | ✓ FMT_OK |

## Known Test Gaps

1. **No automated Android gameplay test**: Touch movement verified in UI unit tests,
   not in actual game join+render flow on device/emulator
2. **No physical Mali GPU proof**: Blocked by Termux linker namespace
3. **No online/Microsoft-mode join**: Offline-mode only; no Mojang assets
4. **No sprint/jump physics automated test**: Manual RCON verification only
5. **No block interaction packet automated test**: RCON-verified, not protocol-test automated

**These gaps are documented as limitations** in docs/m7-limitations.md and
docs/m8-audit.md. Future milestones can address them when physical devices or
online-mode infrastructure becomes available.