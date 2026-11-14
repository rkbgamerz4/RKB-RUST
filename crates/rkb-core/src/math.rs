//! Small, allocation-free vector, matrix, and rectangle types.
//!
//! The types are deliberately simple and use column-major matrices, matching
//! the convention used by OpenGL ES and most graphics APIs.

use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// A two-dimensional vector.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Self = Self::new(0.0, 0.0);
    pub const ONE: Self = Self::new(1.0, 1.0);

    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub const fn splat(value: f32) -> Self {
        Self::new(value, value)
    }

    #[must_use]
    pub fn dot(self, rhs: Self) -> f32 {
        self.x.mul_add(rhs.x, self.y * rhs.y)
    }

    /// Squared length. Prefer this over [`Self::length`] for comparisons.
    #[must_use]
    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    #[must_use]
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    #[must_use]
    pub fn distance(self, rhs: Self) -> f32 {
        (self - rhs).length()
    }

    /// Returns `self` scaled to unit length, or [`Vec2::ZERO`] for a zero vector.
    #[must_use]
    pub fn normalize_or_zero(self) -> Self {
        let length = self.length();
        if length > f32::EPSILON {
            self / length
        } else {
            Self::ZERO
        }
    }

    #[must_use]
    pub fn lerp(self, rhs: Self, t: f32) -> Self {
        self + (rhs - self) * t
    }

    #[must_use]
    pub fn min(self, rhs: Self) -> Self {
        Self::new(self.x.min(rhs.x), self.y.min(rhs.y))
    }

    #[must_use]
    pub fn max(self, rhs: Self) -> Self {
        Self::new(self.x.max(rhs.x), self.y.max(rhs.y))
    }

    /// Rotates the vector counter-clockwise by `radians`.
    #[must_use]
    pub fn rotate(self, radians: f32) -> Self {
        let (sin, cos) = radians.sin_cos();
        Self::new(
            self.x.mul_add(cos, -self.y * sin),
            self.x.mul_add(sin, self.y * cos),
        )
    }

    #[must_use]
    pub fn angle(self) -> f32 {
        self.y.atan2(self.x)
    }
}

impl Add for Vec2 {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl AddAssign for Vec2 {
    fn add_assign(&mut self, rhs: Self) {
        self.x += rhs.x;
        self.y += rhs.y;
    }
}

impl Sub for Vec2 {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl SubAssign for Vec2 {
    fn sub_assign(&mut self, rhs: Self) {
        self.x -= rhs.x;
        self.y -= rhs.y;
    }
}

impl Mul<f32> for Vec2 {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self::Output {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

impl Mul<Vec2> for f32 {
    type Output = Vec2;

    fn mul(self, rhs: Vec2) -> Self::Output {
        rhs * self
    }
}

impl Div<f32> for Vec2 {
    type Output = Self;

    fn div(self, rhs: f32) -> Self::Output {
        Self::new(self.x / rhs, self.y / rhs)
    }
}

impl Neg for Vec2 {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self::new(-self.x, -self.y)
    }
}

/// A three-dimensional vector.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);
    pub const ONE: Self = Self::new(1.0, 1.0, 1.0);

    #[must_use]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    #[must_use]
    pub const fn splat(value: f32) -> Self {
        Self::new(value, value, value)
    }

    #[must_use]
    pub fn dot(self, rhs: Self) -> f32 {
        self.x.mul_add(rhs.x, self.y.mul_add(rhs.y, self.z * rhs.z))
    }

    #[must_use]
    pub fn cross(self, rhs: Self) -> Self {
        Self::new(
            self.y.mul_add(rhs.z, -self.z * rhs.y),
            self.z.mul_add(rhs.x, -self.x * rhs.z),
            self.x.mul_add(rhs.y, -self.y * rhs.x),
        )
    }

    #[must_use]
    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    #[must_use]
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    #[must_use]
    pub fn normalize_or_zero(self) -> Self {
        let length = self.length();
        if length > f32::EPSILON {
            self / length
        } else {
            Self::ZERO
        }
    }

    #[must_use]
    pub fn lerp(self, rhs: Self, t: f32) -> Self {
        self + (rhs - self) * t
    }
}

impl Add for Vec3 {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl Sub for Vec3 {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

impl Mul<f32> for Vec3 {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self::Output {
        Self::new(self.x * rhs, self.y * rhs, self.z * rhs)
    }
}

impl Div<f32> for Vec3 {
    type Output = Self;

    fn div(self, rhs: f32) -> Self::Output {
        Self::new(self.x / rhs, self.y / rhs, self.z / rhs)
    }
}

impl Neg for Vec3 {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self::new(-self.x, -self.y, -self.z)
    }
}

/// A column-major 4x4 matrix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4 {
    pub cols: [[f32; 4]; 4],
}

impl Default for Mat4 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mat4 {
    pub const IDENTITY: Self = Self {
        cols: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
    };

    #[must_use]
    pub const fn from_cols(cols: [[f32; 4]; 4]) -> Self {
        Self { cols }
    }

    #[must_use]
    pub fn translation(offset: Vec3) -> Self {
        let mut matrix = Self::IDENTITY;
        matrix.cols[3] = [offset.x, offset.y, offset.z, 1.0];
        matrix
    }

    #[must_use]
    pub fn scale(factor: Vec3) -> Self {
        let mut matrix = Self::IDENTITY;
        matrix.cols[0][0] = factor.x;
        matrix.cols[1][1] = factor.y;
        matrix.cols[2][2] = factor.z;
        matrix
    }

    #[must_use]
    pub fn rotation_z(radians: f32) -> Self {
        let (sin, cos) = radians.sin_cos();
        let mut matrix = Self::IDENTITY;
        matrix.cols[0][0] = cos;
        matrix.cols[0][1] = sin;
        matrix.cols[1][0] = -sin;
        matrix.cols[1][1] = cos;
        matrix
    }

    /// Right-handed perspective projection with a `[-1, 1]` depth range.
    #[must_use]
    pub fn perspective_rh(fov_y_radians: f32, aspect: f32, near: f32, far: f32) -> Self {
        debug_assert!(aspect > 0.0 && near > 0.0 && far > near);
        let f = 1.0 / (fov_y_radians * 0.5).tan();
        let range = near - far;
        let mut matrix = Self::ZERO;
        matrix.cols[0][0] = f / aspect;
        matrix.cols[1][1] = f;
        matrix.cols[2][2] = (far + near) / range;
        matrix.cols[2][3] = -1.0;
        matrix.cols[3][2] = (2.0 * far * near) / range;
        matrix
    }

    /// Orthographic projection with a `[-1, 1]` depth range.
    #[must_use]
    pub fn orthographic(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> Self {
        debug_assert!(right > left && top > bottom && far > near);
        let mut matrix = Self::IDENTITY;
        matrix.cols[0][0] = 2.0 / (right - left);
        matrix.cols[1][1] = 2.0 / (top - bottom);
        matrix.cols[2][2] = -2.0 / (far - near);
        matrix.cols[3][0] = -(right + left) / (right - left);
        matrix.cols[3][1] = -(top + bottom) / (top - bottom);
        matrix.cols[3][2] = -(far + near) / (far - near);
        matrix
    }

    /// Returns the matrix product `self * rhs`.
    #[must_use]
    pub fn multiply(self, rhs: Self) -> Self {
        let mut out = Self::ZERO;
        for column in 0..4 {
            for row in 0..4 {
                out.cols[column][row] = self.cols[0][row].mul_add(
                    rhs.cols[column][0],
                    self.cols[1][row].mul_add(
                        rhs.cols[column][1],
                        self.cols[2][row]
                            .mul_add(rhs.cols[column][2], self.cols[3][row] * rhs.cols[column][3]),
                    ),
                );
            }
        }
        out
    }

    /// Transforms a point, performing the perspective divide.
    #[must_use]
    pub fn transform_point(self, point: Vec3) -> Vec3 {
        let x = self.cols[0][0].mul_add(
            point.x,
            self.cols[1][0].mul_add(point.y, self.cols[2][0].mul_add(point.z, self.cols[3][0])),
        );
        let y = self.cols[0][1].mul_add(
            point.x,
            self.cols[1][1].mul_add(point.y, self.cols[2][1].mul_add(point.z, self.cols[3][1])),
        );
        let z = self.cols[0][2].mul_add(
            point.x,
            self.cols[1][2].mul_add(point.y, self.cols[2][2].mul_add(point.z, self.cols[3][2])),
        );
        let w = self.cols[0][3].mul_add(
            point.x,
            self.cols[1][3].mul_add(point.y, self.cols[2][3].mul_add(point.z, self.cols[3][3])),
        );
        if w.abs() > f32::EPSILON {
            Vec3::new(x / w, y / w, z / w)
        } else {
            Vec3::new(x, y, z)
        }
    }
}

impl Mul for Mat4 {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        Mat4::multiply(self, rhs)
    }
}

impl Mat4 {
    const ZERO: Self = Self {
        cols: [[0.0; 4]; 4],
    };
}

/// An axis-aligned rectangle in screen or world space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    #[must_use]
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    #[must_use]
    pub fn from_center(center: Vec2, size: Vec2) -> Self {
        Self::new(
            center.x - size.x * 0.5,
            center.y - size.y * 0.5,
            size.x,
            size.y,
        )
    }

    #[must_use]
    pub fn center(self) -> Vec2 {
        Vec2::new(self.x + self.width * 0.5, self.y + self.height * 0.5)
    }

    #[must_use]
    pub const fn min(self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }

    #[must_use]
    pub const fn max(self) -> Vec2 {
        Vec2::new(self.x + self.width, self.y + self.height)
    }

    #[must_use]
    pub fn contains(self, point: Vec2) -> bool {
        point.x >= self.x
            && point.y >= self.y
            && point.x < self.x + self.width
            && point.y < self.y + self.height
    }

    #[must_use]
    pub fn inset(self, amount: f32) -> Self {
        Self::new(
            self.x + amount,
            self.y + amount,
            (self.width - amount * 2.0).max(0.0),
            (self.height - amount * 2.0).max(0.0),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < 1.0e-4, "{a} != {b}");
    }

    #[test]
    fn vector_operations() {
        let a = Vec2::new(3.0, 4.0);
        approx(a.length(), 5.0);
        approx(a.dot(Vec2::new(1.0, 2.0)), 11.0);
        let unit = a.normalize_or_zero();
        approx(unit.length(), 1.0);
        assert_eq!(Vec2::ZERO.normalize_or_zero(), Vec2::ZERO);
        approx(a.rotate(std::f32::consts::FRAC_PI_2).x, -4.0);
    }

    #[test]
    fn cross_product_is_right_handed() {
        let cross = Vec3::new(1.0, 0.0, 0.0).cross(Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(cross, Vec3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn translation_matrix_moves_points() {
        let matrix = Mat4::translation(Vec3::new(2.0, -3.0, 4.0));
        assert_eq!(
            matrix.transform_point(Vec3::new(1.0, 1.0, 1.0)),
            Vec3::new(3.0, -2.0, 5.0)
        );
    }

    #[test]
    fn matrix_multiplication_is_not_commutative() {
        let translate = Mat4::translation(Vec3::new(1.0, 0.0, 0.0));
        let scale = Mat4::scale(Vec3::splat(2.0));
        let a = translate.multiply(scale).transform_point(Vec3::ONE);
        let b = scale.multiply(translate).transform_point(Vec3::ONE);
        assert_eq!(a, Vec3::new(3.0, 2.0, 2.0));
        assert_eq!(b, Vec3::new(4.0, 2.0, 2.0));
    }

    #[test]
    fn orthographic_maps_corners_to_clip_space() {
        let projection = Mat4::orthographic(0.0, 100.0, 0.0, 50.0, -1.0, 1.0);
        let min = projection.transform_point(Vec3::new(0.0, 0.0, 0.0));
        let max = projection.transform_point(Vec3::new(100.0, 50.0, 0.0));
        approx(min.x, -1.0);
        approx(min.y, -1.0);
        approx(min.z, 0.0);
        approx(max.x, 1.0);
        approx(max.y, 1.0);
        approx(max.z, 0.0);
    }

    #[test]
    fn perspective_projection_divides_by_depth() {
        let projection = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 0.1, 100.0);
        let near = projection.transform_point(Vec3::new(0.0, 0.0, -0.1));
        let far = projection.transform_point(Vec3::new(0.0, 0.0, -100.0));
        approx(near.z, -1.0);
        approx(far.z, 1.0);
    }

    #[test]
    fn rectangle_contains_and_inset() {
        let rect = Rect::new(10.0, 10.0, 20.0, 10.0);
        assert!(rect.contains(Vec2::new(10.0, 10.0)));
        assert!(!rect.contains(Vec2::new(30.0, 20.0)));
        assert_eq!(rect.inset(2.0), Rect::new(12.0, 12.0, 16.0, 6.0));
        assert_eq!(
            Rect::from_center(Vec2::new(5.0, 5.0), Vec2::new(4.0, 2.0)),
            Rect::new(3.0, 4.0, 4.0, 2.0)
        );
    }
}
