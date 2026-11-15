//! Player physics and axis-aligned collision.
//!
//! The player is a 0.6 x 1.8 block box with the standard 1.62 eye height.
//! Movement is resolved one axis at a time against saved solid blocks, with
//! sub-stepping so a long frame cannot tunnel through a wall.

use rkb_core::Vec3;

/// Box width and depth.
pub const PLAYER_WIDTH: f32 = 0.6;
/// Box height.
pub const PLAYER_HEIGHT: f32 = 1.8;
/// Eye offset from the feet.
pub const EYE_HEIGHT: f32 = 1.62;
/// Walking speed in blocks per second.
pub const WALK_SPEED: f32 = 4.317;
/// Creative flight speed in blocks per second.
pub const FLY_SPEED: f32 = 10.9;
/// Upward velocity from a jump: clears one block with a small margin.
pub const JUMP_VELOCITY: f32 = 8.2;
/// Vertical acceleration, tuned for a roughly 1.25-block jump arc.
pub const GRAVITY: f32 = 28.0;
/// Terminal fall speed.
pub const TERMINAL_VELOCITY: f32 = 60.0;

/// Which blocks stop the player.
pub trait BlockQuery {
    fn is_solid(&mut self, x: i32, y: i32, z: i32) -> bool;
}

/// Player intent for one simulation step.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MoveInput {
    /// Positive looks in the yaw direction.
    pub forward: f32,
    /// Positive strafes right.
    pub strafe: f32,
    pub jump: bool,
    /// Used only while flying.
    pub descend: bool,
}

/// First-person player state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Player {
    /// Feet position at the center of the box.
    pub position: Vec3,
    pub velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub flying: bool,
}

impl Player {
    #[must_use]
    pub fn new(position: Vec3) -> Self {
        Self {
            position,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            flying: false,
        }
    }

    #[must_use]
    pub fn eye(self) -> Vec3 {
        Vec3::new(
            self.position.x,
            self.position.y + EYE_HEIGHT,
            self.position.z,
        )
    }

    /// Applies a look delta in radians, clamping pitch just short of vertical.
    pub fn look(&mut self, yaw_delta: f32, pitch_delta: f32) {
        self.yaw = (self.yaw + yaw_delta).rem_euclid(std::f32::consts::TAU);
        let limit = 89.0_f32.to_radians();
        self.pitch = (self.pitch + pitch_delta).clamp(-limit, limit);
    }

    /// Advances the player. `dt` is clamped so a stalled frame cannot explode
    /// the simulation; movement is split into short sub-steps.
    pub fn step(&mut self, dt: f32, input: MoveInput, blocks: &mut dyn BlockQuery) {
        let dt = dt.clamp(0.0, 0.1);
        if dt <= 0.0 {
            return;
        }
        let distance = self.velocity.length() * dt
            + if self.flying {
                FLY_SPEED * dt
            } else {
                WALK_SPEED * dt
            };
        let sub_steps = ((distance / 0.4).ceil() as u32).clamp(1, 8);
        let sub_dt = dt / sub_steps as f32;
        for _ in 0..sub_steps {
            self.integrate(sub_dt, input, blocks);
        }
    }

    fn integrate(&mut self, dt: f32, input: MoveInput, blocks: &mut dyn BlockQuery) {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let forward = Vec3::new(sin_yaw, 0.0, -cos_yaw);
        let right = Vec3::new(cos_yaw, 0.0, sin_yaw);
        let mut wish = forward * input.forward + right * input.strafe;
        let magnitude = wish.length();
        if magnitude > 1.0 {
            wish = wish * (1.0 / magnitude);
        }
        let speed = if self.flying { FLY_SPEED } else { WALK_SPEED };
        self.velocity.x = wish.x * speed;
        self.velocity.z = wish.z * speed;

        if self.flying {
            self.velocity.y = if input.jump {
                FLY_SPEED * 0.75
            } else if input.descend {
                -FLY_SPEED * 0.75
            } else {
                0.0
            };
        } else {
            if input.jump && self.on_ground {
                self.velocity.y = JUMP_VELOCITY;
                self.on_ground = false;
            }
            self.velocity.y =
                (self.velocity.y - GRAVITY * dt).clamp(-TERMINAL_VELOCITY, JUMP_VELOCITY);
        }

        if !self.move_axis(0, self.velocity.x * dt, blocks) {
            self.velocity.x = 0.0;
        }
        if !self.move_axis(2, self.velocity.z * dt, blocks) {
            self.velocity.z = 0.0;
        }
        let landed = self.velocity.y < 0.0;
        if !self.move_axis(1, self.velocity.y * dt, blocks) {
            if landed {
                self.on_ground = true;
            }
            self.velocity.y = 0.0;
        } else if self.velocity.y < -0.01 {
            self.on_ground = false;
        }
    }

    /// Moves along one axis, backing off until the box is free.
    ///
    /// Returns `false` when a block stopped the movement.
    fn move_axis(&mut self, axis: usize, delta: f32, blocks: &mut dyn BlockQuery) -> bool {
        if delta.abs() < f32::EPSILON {
            return true;
        }
        let start = self.position;
        if !self.collides(translate(start, axis, delta), blocks) {
            self.position = translate(start, axis, delta);
            return true;
        }
        // Binary search the furthest free fraction along this axis. The start
        // position is assumed free, which holds after spawn and every step.
        let mut free = 0.0_f32;
        let mut blocked = delta;
        for _ in 0..16 {
            let mid = (free + blocked) * 0.5;
            if self.collides(translate(start, axis, mid), blocks) {
                blocked = mid;
            } else {
                free = mid;
            }
        }
        // Leave a hair of space so the next frame does not start touching.
        let applied = if delta > 0.0 {
            (free - 0.0005).max(0.0)
        } else {
            (free + 0.0005).min(0.0)
        };
        self.position = translate(start, axis, applied);
        false
    }

    fn collides(&self, position: Vec3, blocks: &mut dyn BlockQuery) -> bool {
        const EPSILON: f32 = 0.001;
        let min_x = position.x - PLAYER_WIDTH * 0.5;
        let max_x = position.x + PLAYER_WIDTH * 0.5;
        let min_y = position.y;
        let max_y = position.y + PLAYER_HEIGHT;
        let min_z = position.z - PLAYER_WIDTH * 0.5;
        let max_z = position.z + PLAYER_WIDTH * 0.5;

        let x0 = (min_x + EPSILON).floor() as i32;
        let x1 = (max_x - EPSILON).floor() as i32;
        let y0 = (min_y + EPSILON).floor() as i32;
        let y1 = (max_y - EPSILON).floor() as i32;
        let z0 = (min_z + EPSILON).floor() as i32;
        let z1 = (max_z - EPSILON).floor() as i32;

        for y in y0..=y1 {
            for z in z0..=z1 {
                for x in x0..=x1 {
                    if blocks.is_solid(x, y, z) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

fn translate(position: Vec3, axis: usize, delta: f32) -> Vec3 {
    let mut moved = position;
    match axis {
        0 => moved.x += delta,
        1 => moved.y += delta,
        _ => moved.z += delta,
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Floor;

    impl BlockQuery for Floor {
        fn is_solid(&mut self, _x: i32, y: i32, _z: i32) -> bool {
            y == 63
        }
    }

    #[test]
    fn gravity_lands_the_player_on_the_floor() {
        let mut player = Player::new(Vec3::new(0.5, 70.0, 0.5));
        let mut floor = Floor;
        for _ in 0..240 {
            player.step(1.0 / 60.0, MoveInput::default(), &mut floor);
        }
        assert!(player.on_ground);
        assert!(
            (player.position.y - 64.0).abs() < 0.01,
            "{}",
            player.position.y
        );
        assert_eq!(player.velocity.y, 0.0);
    }

    #[test]
    fn jumping_leaves_the_ground_and_returns() {
        let mut player = Player::new(Vec3::new(0.5, 64.0, 0.5));
        let mut floor = Floor;
        player.on_ground = true;
        let mut peak: f32 = 0.0;
        for step in 0..90 {
            let input = MoveInput {
                jump: step == 0,
                ..MoveInput::default()
            };
            player.step(1.0 / 60.0, input, &mut floor);
            peak = peak.max(player.position.y);
        }
        assert!(peak > 65.0, "jump peaked at {peak}");
        assert!(player.on_ground);
    }

    #[test]
    fn walking_forward_moves_along_negative_z() {
        let mut player = Player::new(Vec3::new(0.5, 64.0, 0.5));
        player.on_ground = true;
        let mut floor = Floor;
        for _ in 0..60 {
            player.step(
                1.0 / 60.0,
                MoveInput {
                    forward: 1.0,
                    ..MoveInput::default()
                },
                &mut floor,
            );
        }
        assert!(player.position.z < -2.0, "{}", player.position.z);
        assert!((player.position.x - 0.5).abs() < 0.001);
    }

    struct Wall;

    impl BlockQuery for Wall {
        fn is_solid(&mut self, _x: i32, y: i32, z: i32) -> bool {
            y == 63 || (z == 60 && (64..=66).contains(&y))
        }
    }

    #[test]
    fn walls_stop_the_player_without_tunneling() {
        let mut player = Player::new(Vec3::new(0.5, 64.0, 64.5));
        player.on_ground = true;
        let mut wall = Wall;
        for _ in 0..120 {
            player.step(
                1.0 / 10.0,
                MoveInput {
                    forward: 1.0,
                    ..MoveInput::default()
                },
                &mut wall,
            );
        }
        // The wall occupies z=60, so the box face stops at z=61.
        assert!(player.position.z >= 61.29, "{}", player.position.z);
        assert!(player.position.z < 61.31, "{}", player.position.z);
    }

    #[test]
    fn flying_ignores_gravity_and_descends_on_request() {
        let mut player = Player::new(Vec3::new(0.5, 90.0, 0.5));
        player.flying = true;
        let mut floor = Floor;
        player.step(
            1.0 / 60.0,
            MoveInput {
                descend: true,
                ..MoveInput::default()
            },
            &mut floor,
        );
        assert!(player.position.y < 90.0);
    }
}
