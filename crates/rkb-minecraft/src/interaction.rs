//! Voxel raycasting for block breaking and placement.
//!
//! Uses the standard Amanatides-Woo grid traversal: walk the voxels the view
//! ray crosses, in order, and return the first targetable block plus the empty
//! neighbor the ray entered from (the placement face).

use rkb_core::Vec3;

/// A block hit by a view ray.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RayHit {
    /// The block that was hit.
    pub block: (i32, i32, i32),
    /// The voxel immediately before it along the ray. A block placed against
    /// the hit face goes here.
    pub previous: (i32, i32, i32),
}

/// Casts a ray through a voxel field. `targetable` decides whether a block can
/// be selected; air and liquids should return `false`.
pub fn raycast(
    origin: Vec3,
    direction: Vec3,
    max_distance: f32,
    mut targetable: impl FnMut(i32, i32, i32) -> bool,
) -> Option<RayHit> {
    let length = direction.length();
    if length <= f32::EPSILON || max_distance <= 0.0 {
        return None;
    }
    let dir = direction * (1.0 / length);

    let mut voxel = (
        origin.x.floor() as i32,
        origin.y.floor() as i32,
        origin.z.floor() as i32,
    );
    if targetable(voxel.0, voxel.1, voxel.2) {
        return Some(RayHit {
            block: voxel,
            previous: voxel,
        });
    }

    let step = (
        if dir.x > 0.0 { 1 } else { -1 },
        if dir.y > 0.0 { 1 } else { -1 },
        if dir.z > 0.0 { 1 } else { -1 },
    );
    let axis = |origin: f32, direction: f32, step: i32| -> (f32, f32) {
        if direction.abs() <= f32::EPSILON {
            (f32::INFINITY, f32::INFINITY)
        } else {
            let delta = (1.0 / direction).abs();
            let boundary = if step > 0 {
                origin.floor() + 1.0 - origin
            } else {
                origin - origin.floor()
            };
            (boundary * delta, delta)
        }
    };
    let (mut t_max_x, t_delta_x) = axis(origin.x, dir.x, step.0);
    let (mut t_max_y, t_delta_y) = axis(origin.y, dir.y, step.1);
    let (mut t_max_z, t_delta_z) = axis(origin.z, dir.z, step.2);

    let mut distance = 0.0_f32;
    let mut previous;
    while distance <= max_distance {
        previous = voxel;
        if t_max_x <= t_max_y && t_max_x <= t_max_z {
            voxel.0 += step.0;
            distance = t_max_x;
            t_max_x += t_delta_x;
        } else if t_max_y <= t_max_z {
            voxel.1 += step.1;
            distance = t_max_y;
            t_max_y += t_delta_y;
        } else {
            voxel.2 += step.2;
            distance = t_max_z;
            t_max_z += t_delta_z;
        }
        if distance > max_distance {
            break;
        }
        if targetable(voxel.0, voxel.1, voxel.2) {
            return Some(RayHit {
                block: voxel,
                previous,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(x: i32, y: i32, z: i32) -> bool {
        (x, y, z) == (5, 0, 0)
    }

    #[test]
    fn hits_the_first_block_along_the_view() {
        let hit = raycast(
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(1.0, 0.0, 0.0),
            8.0,
            only,
        )
        .expect("ray should hit");
        assert_eq!(hit.block, (5, 0, 0));
        assert_eq!(hit.previous, (4, 0, 0));
    }

    #[test]
    fn respects_reach_and_negative_axes() {
        assert!(raycast(
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(1.0, 0.0, 0.0),
            4.0,
            only
        )
        .is_none());

        let hit = raycast(
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(0.0, 0.0, -1.0),
            6.0,
            |x, y, z| (x, y, z) == (0, 0, -4),
        )
        .expect("negative z ray");
        assert_eq!(hit.block, (0, 0, -4));
        assert_eq!(hit.previous, (0, 0, -3));
    }

    #[test]
    fn zero_direction_never_hits() {
        assert!(raycast(Vec3::ZERO, Vec3::ZERO, 5.0, |_, _, _| true).is_none());
    }
}
