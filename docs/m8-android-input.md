# M8 Android Input System

## Overview
The Android input system receives touch events from the Java UI layer via JNI,
queues them on the render thread, and passes them to the Rust game engine for
processing. The input system supports movement, camera control, jump, sneak,
and flight toggling.

## Architecture

### Java Side (MainActivity.kt)
Touch events from the Android `onTouchListener` are converted to `TouchSample`
objects and queued in a `ConcurrentLinkedQueue<TouchSample>`. The render thread
polls this queue each frame and delivers events to the Rust engine through JNI.

**Key fields in MainActivity**:
- `touchQueue: ConcurrentLinkedQueue<TouchSample>` — UI-thread queue
- `gpuActive: Boolean` — whether the wgpu GPU path is active
- `gpuUnavailable: Boolean` — fallback to software renderer
- `tryAttachGpu()` — attempts GPU initialization on fresh surface
- `renderGpuFrame()` — renders one frame via wgpu swapchain
- `renderSoftwareFrame()` — renders via M6 software path (fallback)

**Touch event flow**:
```
Android onTouch() event
      ↓
convertToTouchSample() → TouchSample
      ↓
touchQueue.offer(TouchSample)
      ↓
render thread poll() → TouchSample
      ↓
RkbNative.nativeTouch() → JNI bridge
      ↓
crates/rkb-android/src/bridge.rs: touch()
      ↓
runtime.touch(TouchEvent)
      ↓
game.handle_touch(TouchEvent)
```

### Rust Side (rkb-input + game.rs)
The input system consists of two main components:

1. **`crates/rkb-input/src/controls.rs`** — Low-level widgets (VirtualJoystick, TouchButton)
2. **`crates/rkb-minecraft/src/game.rs`** — High-level input handling (joystick, buttons, look drag)

The `VirtualJoystick` and `TouchButton` types are pure Rust with no Android
dependencies — they receive `TouchEvent` objects and track their internal state.

### VirtualJoystick

```rust
pub struct VirtualJoystick {
    center: Vec2,       // Screen position of joystick center
    radius: f32,        // Activation radius
    dead_zone: f32,     // Dead zone fraction (0.15)
    active_id: Option<TouchId>, // Currently active touch
    knob: Vec2,         // Current knob position (center when inactive)
    value: Vec2,        // Normalized output [-1, 1] per axis
    touch_radius: f32,  // Activation radius * 1.5
}
```

**Initialization** (game.rs line 290):
```rust
joystick: VirtualJoystick::new(Vec2::new(160.0, 560.0), 96.0)
```
- `center`: (160, 560) — roughly bottom-right quarter of a 1280×720 screen
- `radius`: 96 — touch must be within 96px of center to activate

**`handle(event: TouchEvent) -> bool`**
- `TouchPhase::Started`: If touch is within `touch_radius` of center, activate
  and record the touch ID. Update knob position. Return `true` if consumed.
- `TouchPhase::Moved`: If the touch ID matches the active one, update knob
  position and value. Return `true` if consumed.
- `TouchPhase::Ended/Cancelled`: If the touch ID matches, deactivate (reset
  knob to center, value to zero). Return `true` if consumed.
- Returns `false` if the touch is not for this joystick.

**`value() -> Vec2`**
- Normalized direction and magnitude per axis
- Within dead zone → `(0, 0)`
- Outside radius → clamped to magnitude 1.0
- Formula: `magnitude = (raw - dead_zone) / (1.0 - dead_zone)`, then clamp

### TouchButton (rectangular button)

```rust
pub struct TouchButton {
    bounds: Rect,       // (x_min, y_min, x_max, y_max)
    label: String,      // Debug label
    active_id: Option<TouchId>,
    press_count: u32,   // Number of completed presses
}
```

**`handle(event: TouchEvent) -> bool`**
- `Started`: If touch is within bounds, activate and return `true`
- `Moved`: If active and still within bounds, keep active; else deactivate
- `Ended`: If active and still within bounds, increment `press_count`; deactivate
- `Cancelled`: Deactivate without incrementing press_count

**`is_pressed() -> bool`** and **`press_count() -> u32`** are read-only queries.

### Game Input Handling (game.rs)

The game wraps the low-level widgets and adds game-specific logic:

**Joystick → MoveInput**:
```rust
let value = self.joystick.value();
let input = MoveInput {
    forward: -value.y,    // y-inverted because screen y increases downward
    strafe: value.x,
    jump: self.jump.is_pressed(),
    descend: self.player.flying && self.descend.is_pressed(),
};
```

**Sneak handling**:
- `self.sneak.handle(event)` — toggles crouch state
- Message shown: "Sneak toggled."

**Jump handling**:
- `self.jump.handle(event)` — allowed only when `on_ground == true`
- Jump is passed via `MoveInput.jump`

**Look drag** (camera rotation):
- If no widget consumed the touch, track drag start (`LookDrag { id, last }`)
- On each Move: `player.look(delta.x * 0.0032, -delta.y * 0.0032)`
- Sensitivity `0.0032` rad/pixel (configurable later)
- Pitch clamped to `[-89°, +89°]` ( `-1.553` to `1.553` rad)

**Flight mode** (creative/spectator):
- `fly` button toggles `player.flying`
- `descend` button decreases altitude while flying
- `on_ground` is always `true` in flight mode (gravity disabled)

### JNI Bridge (bridge.rs / lib.rs)

The bridge forwards touch events from Java to Rust through `extern "system"` functions.

**`nativeTouch(handle: Long, action: Int, x: Float, y: Float, id: Int, time: Long)`**
- Converts Android `MotionEvent` to `TouchEvent`
- Queues the event on the render thread's touch queue
- The render thread polls this queue each frame

**`nativeCreate(width, height, worldPath) -> Long`** and related JNI functions
- Create the game engine, surface, and GPU renderer
- Track the engine handle for subsequent calls

**`nativeAttachSurface(handle, surface)`** and **`nativeDetachSurface(handle)`**
- Attach/detach the `ANativeWindow` for the GPU swapchain
- Called when the surface is created, changed, or destroyed

**`nativeGpuStatus(handle) -> String`**
- Returns the GPU error string if initialization failed
- The Android overlay reads this to decide whether to show GPU or software path

### Input Priorities and Edge Cases

**Touch priority order** (in `handle_touch()`):
1. Joystick — Always first; if the touch is within the joystick radius, it
   consumes the event and no other widget processes it. This ensures the
   player can always move even if touching other areas.
2. Descend — Only if flying; allows going down in flight mode.
3. Fly toggle — Toggles creative flight mode.
4. **Sneak** — New in M8; consumes the event if the touch is within the
   sneak button bounds.
5. Jump — Only allowed when `on_ground == true`. If the player is in the air,
   the jump press is consumed but has no effect until the player lands.
6. Break — Raycast target block interaction.
7. Place — Raycast target block interaction.
8. Look drag — If no widget consumed the event, the touch drag rotates the
   camera. This is the fallback for free camera control.

**Edge cases**:
- **Multi-touch**: Only one touch ID is tracked per widget. If two fingers
  touch different widgets simultaneously, the first one to `Started` wins.
- **Screen size**: The joystick and button positions are relative to the
  viewport size (`self.viewport`), so they scale with rotation/resize.
- **Surface changes** — The `layout()` method is called on resize, which
  repositions all widgets. The joystick radius is `16%` of the minimum
  viewport dimension, so it scales gracefully.
- **No touch**: If the user is not touching the screen, the joystick value
  is `(0, 0)` and the player stands still.

## M8 Changes

**crates/rkb-minecraft/src/game.rs**:
- Added `sneak: TouchButton` field
- Initialized sneak button at `Rect::new(1080.0, 516.0, 96.0, 96.0)`
- Updated `layout()` to position sneak below jump, adjust descend position
- Updated `handle_touch()` to handle sneak button press (returns early after
  handling, shows message)
- All existing joystick, jump, fly, descend, break, place, and look drag
  handling preserved

**No files deleted or overwritten**. Existing M7 input handling preserved.

## Known Limitations (M8)

- **Sneak server packet not sent**: The sneak button toggles a local state
  message, but the serverbound sneak packet (flags byte in Position/Look) is
  not yet implemented. The server authoritative handles sneaking state.
- **Sprint not wired**: Sprinting (shift + forward) is handled by the server
  when the client moves while holding shift. Not implemented in M8.
- **No D-pad backup**: No keyboard or D-pad input fallback for devices without
  touch capability.
- **Camera sensitivity fixed**: `0.0032` rad/pixel is hardcoded; could be
  configured from settings later.
- **Touch coordinates assume portrait orientation**: Landscape layout not yet
  optimized (would rotate the entire widget grid 90°).