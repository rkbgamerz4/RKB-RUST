# M8 Player System

## Overview
The player system handles first-person movement, camera control, and interaction
for a Minecraft Java Edition 1.21.1 client on Android ARM64. The core physics
are implemented in `crates/rkb-minecraft/src/player.rs` and wired through the
game loop in `crates/rkb-minecraft/src/game.rs`.

## Player Physics (player.rs)

### Constants
| Constant | Value | Description |
|----------|-------|-------------|
| `PLAYER_WIDTH` | 0.6f32 | Player box width/depth in blocks |
| `PLAYER_HEIGHT` | 1.8f32 | Player box height in blocks |
| `EYE_HEIGHT` | 1.62f32 | Eye offset above feet (first-person view) |
| `WALK_SPEED` | 4.317f32 | Blocks per second when walking |
| `FLY_SPEED` | 10.9f32 | Blocks per second when flying |
| `JUMP_VELOCITY` | 8.2f32 | Initial upward velocity from jump |
| `GRAVITY` | 28.0f32 | Vertical acceleration per second |
| `TERMINAL_VELOCITY` | 60.0f32 | Maximum fall speed |

### Player State
```rust
pub struct Player {
    pub position: Vec3,   // Feet-center position in blocks
    pub velocity: Vec3,   // Current velocity (updates each step)
    pub yaw: f32,         // Yaw in radians (protocol: 0 faces +Z)
    pub pitch: f32,       // Pitch in radians (-π/2 to π/2 clamped)
    pub on_ground: bool,  // True when standing on a solid block
    pub flying: bool,     // True when in creative/spectator flight
}
```

### Movement Integration (`step()`)

The `step()` method advances the player one simulation step. It works as follows:

1. **Clamp dt** to max 0.1s to prevent frame-time explosions
2. **Compute distance** = velocity * dt + (flying ? FLY_SPEED : WALK_SPEED) * dt
3. **Sub-step** the distance into chunks of at most 0.4 blocks (1–8 sub-steps)
4. **Integrate** each sub-step along three axes (X, Z, Y) with collision

### Axis-Integrated Movement (`integrate()`)

For each axis, the movement is resolved one axis at a time, in order: X, Z, Y.

**For X and Z axes** (forward/strafe movement):
- Compute forward vector from yaw: `forward = (sin(yaw), 0, -cos(yaw))`
- Compute right vector: `right = (cos(yaw), 0, sin(yaw))`
- Wish direction = `forward * forward_input + right * strafe_input`
- Clamp wish vector to length 1.0
- Apply speed (WALK_SPEED or FLY_SPEED)
- Move along axis with collision detection — if a block stops us, zero the velocity

**For Y axis** (jumping/gravity):
- If flying: `velocity.y = FLY_SPEED * 0.75` if jump pressed, or `-FLY_SPEED * 0.75` if descend
- If not flying:
  - If jump just pressed AND on_ground: `velocity.y = JUMP_VELOCITY` (8.2), mark `on_ground = false`
  - Apply gravity: `velocity.y -= GRAVITY * dt`, clamped to `[-TERMINAL_VELOCITY, JUMP_VELOCITY]`
- Move along Y axis with collision detection:
  - If velocity.y < 0 (falling) and we land on a block: set `on_ground = true`
  - If velocity.y > -0.01: clear `on_ground`

### Collision Detection (`collides()` / `move_axis()`)

The player is a 0.6 × 1.8 block box. Collision is checked by sampling the
block grid at the box corners:

```
min_x = pos.x - 0.3, max_x = pos.x + 0.3
min_y = pos.y,     max_y = pos.y + 1.8
min_z = pos.z - 0.3, max_z = pos.z + 0.3
```

For each integer cell (x, y, z) in that range, if `blocks.is_solid(x, y, z)`,
the cell is considered solid and movement is blocked.

Movement is resolved by binary search along the axis — find the maximum free
fraction, then apply it with a 0.0005 block "hair" left so the next frame
doesn't start already touching a block.

### Jumping
- Jump only works when `on_ground == true`
- On jump: `velocity.y = JUMP_VELOCITY (8.2)`, `on_ground = false`
- The jump leaves the ground and the player follows a parabolic arc under gravity
- Peak height depends on initial velocity and gravity; ~1 block rise is typical

### Falling and Landing
- Gravity continuously accelerates the player downward
- Terminal velocity caps fall speed at 60.0 blocks/s
- When the player's bottom hits a solid block while falling: `on_ground = true`, `velocity.y = 0`
- Fall damage is NOT implemented in M8 (would require health system)

## Game Loop Integration (game.rs)

### `step()` Method

The game's `step(dt)` method is called each frame and orchestrates:

1. **`poll_server()`** — processes server updates (chunk updates, position corrections, disconnects)
2. **Input → MoveInput** — converts Android touch inputs to player movement intent
3. **`player.step(dt, input, &mut query)`** — advances player physics with collision
4. **Border clamping** — prevent player from wandering beyond WORLD_BORDER
5. **Fall detection** — if player falls below WORLD_MIN_Y-4 (non-server worlds), respawn at spawn
6. **Mesh budget** — mesh up to `MESH_BUDGET (2)` chunks per frame
7. **Save clock** — autosave every `AUTOSAVE_SECONDS (20s)`

### Input → MoveInput Pipeline

```
Android touch events
      ↓
VirtualJoystick.handle() → value.x, value.y (normalized [-1,1] with dead zone)
      ↓
Joystick in game.rs
      ↓
MoveInput { forward: -value.y, strafe: value.x, jump: jump_button.is_pressed(), descend: flying && descend_button.is_pressed() }
      ↓
player.step(dt, input, &mut world_query)
      ↓
Player position updated with collision resolution
      ↓
Camera computed from player.yaw + player.pitch
```

### Camera System

The camera is computed each frame from the player state:

```rust
pub fn camera(&self) -> Camera {
    let mut camera = Camera::new(self.player.eye());  // eye = pos + Vec3(0, 1.62, 0)
    camera.yaw = self.player.yaw;
    camera.pitch = self.player.pitch;
    camera.fov_y = self.camera_fov_y;
    camera
}
```

The `forward()` vector is `(sin(yaw) * cos(pitch), sin(pitch), -cos(yaw) * cos(pitch))`.

### Server Position Synchronization

When the server sends a position correction (via `SyncPlayerPosition`), the
`poll_server()` method applies it immediately:

```rust
if let Some(position) = world.take_server_position() {
    let spawn = Vec3::new(position.x as f32, position.y as f32, position.z as f32);
    self.spawn = spawn;
    self.player.position = spawn;
    self.player.velocity = Vec3::ZERO;
    self.player.yaw = position.yaw.to_radians() + std::f32::consts::PI;
    self.player.pitch = -position.pitch.to_radians();
    self.center = None;
}
```

This effectively "teleports" the player to the server's position, which is how
the server corrects cheaters or syncs after network lag. The server typically
sends these corrections every few seconds when the player is moving.

### Look Rotation

The player's look (yaw/pitch) can be updated via:
- **Server position packets** (SyncPlayerPosition) — full position + yaw/pitch
- **Player input** — `player.look(dyaw, dpitch)` from drag input
- **Touch drag** — horizontal drag rotates yaw, vertical drag rotates pitch (clamped at ±89°)

The yaw is stored in radians but the protocol uses degrees (0 = +Z, clockwise).
The conversion `yaw_rad + PI` adjusts from protocol 0=-Z to our internal 0=+Z.

## Android Input Wiring (Phase 3)

### Touch Layout (portrait orientation, right-side controls)

| Position | Control | Function |
|----------|---------|----------|
| Top-right | Joystick (96px radius) | Movement: forward (push forward), strafe (right) |
| Below joystick | JUMP (96×96) | Jump when pressed (only when on_ground) |
| Below JUMP | SNEAK (96×96) | Crouch toggle (shifts player state) |
| Below SNEAK | DESCEND (96×96) | Descend in flight mode (decrease altitude) |
| Further down | FLY (96×96) | Toggle creative/spectator flight |
| Bottom-right | BREAK (96×96) | Raycast target block, send break packet |
| Left of BREAK | PLACE (96×96) | Raycast target block, send place packet |
| Remaining area | Touch drag | Camera look: horizontal → yaw, vertical → pitch (clamped) |

### Input Handling Order (in `handle_touch()`)

1. **Joystick** — Always first; consumes the event if the touch is within the joystick radius
2. **Descend** — Only if player is flying; allows going down in flight mode
3. **Fly toggle** — Toggles creative/spectator flight mode
4. **Sneak** — New in M8; toggles crouching state
5. **Jump** — Only allowed when `on_ground == true`
6. **Break** — Raycast target block, send block break interaction
7. **Place** — Raycast target block, send block place interaction
8. **Look drag** — If no other widget consumed the event, rotate camera

### Joystick Details

The `VirtualJoystick` is initialized with:
- `center: Vec2(160.0, 560.0)` — screen position (x, y)
- `radius: 96.0` — touch activation radius

`value()` returns `Vec2 { x: [-1, 1], y: [-1, 1] }` after dead zone clamping
(`dead_zone: 0.15`). Values within the dead zone are returned as `(0, 0)`.

- `value.x` → `MoveInput.strafe` (positive = right)
- `value.y` → `MoveInput.forward` (negative = forward, since y increases downward on screen)

### Camera Look Drag

When the player starts a touch drag (TouchPhase::Started) and no widget owns it,
a `LookDrag` state is created tracking the touch ID and last position.

On each Move event:
```
delta = current_position - last_position
player.look(delta.x * 0.0032, -delta.y * 0.0032)
```

The sensitivity factor `0.0032` radians per pixel can be configured later.

## Block Interaction Foundation (Phase 5)

### Target Block Selection (`target_block()`)

```rust
pub fn target_block(&mut self) -> Result<Option<RayHit>, String> {
    let eye = self.player.eye();          // pos + (0, 1.62, 0)
    let direction = self.camera().forward(); // forward vector from yaw/pitch
    let hit = raycast(eye, direction, REACH (5.0), ...);
    // Returns the first solid block hit within 5 blocks, or None
}
```

The ray uses the camera's position and forward direction, testing against
all solid blocks within `REACH` blocks. Water and lava blocks are excluded
from being targets (player cannot break/place them).

### Block Breaking (`break_block()`)

1. Call `target_block()` to find the hit block
2. If a block is hit, call `set_block(hit.block, BlockState::air())`
3. Send the block break interaction to the server (protocol 767)
4. Update the message ("Broke block at x, y, z.")

The actual serverbound packet for block breaking (Transaction ID + Location +
Face + Entity ID) is not yet fully implemented in M8 — the RCON test in M7
verified the mesh change, but the protocol packet path needs completion.

### Block Placement (`place_block()`)

1. Call `target_block()` to find the hit block and face
2. Compute the target position = hit.block.previous (the block face adjacent)
3. If target == hit.block, reject ("Cannot place inside terrain")
4. If the target position intersects the player, reject ("Cannot place inside self")
5. If the target space is already occupied, reject
6. Set the target block to oak planks (procedural fallback model)
7. Send the block place interaction to the server (protocol 767)

### Server-Authoritative Block Updates

The `section_blocks_update` (0x49) packet from the server is already handled
and emits `Event::BlocksChanged`. This is the server's way of updating blocks
without requiring full block place/break packets from the client. The client
receives these changes and updates its local mesh cache accordingly.

## Performance Considerations (ARM64)

- Player physics use fixed-point-friendly operations (f32 is fine on ARM64)
- Movement integrates one axis at a time, avoiding unnecessary allocations
- Collision grid sampling is O(1) per sub-step (max 8 sub-steps per frame)
- No heap allocations in the inner loop — all math is stack-allocated Vec3
- The per-frame mesh budget `MESH_BUDGET = 2` prevents frame stalls from mesh generation
- GPU buffer reuse via `SceneSync` revision tracking (from M7) ensures mesh uploads
are only sent when chunks actually change

## M8 Changes Summary

**crates/rkb-minecraft/src/game.rs**:
- Added `sneak: TouchButton` field
- Initialized sneak button in `new()` 
- Updated `layout()` to position sneak below jump
- Updated `handle_touch()` to handle sneak press
- All existing fields and methods preserved

**No files deleted or overwritten**. Existing M7 features (GPU rendering, chunk meshing,
live-edit proof, protocol 0x49) preserved. No `main` branch modified.