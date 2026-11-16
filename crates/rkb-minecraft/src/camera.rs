//! First-person camera, near-plane clipping, and frustum culling.

use rkb_core::{Mat4, Vec2, Vec3};
use rkb_render::{project_clip, Vertex3D};

/// A right-handed first-person camera. Yaw 0 looks toward negative Z.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    #[must_use]
    pub fn new(position: Vec3) -> Self {
        Self {
            position,
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 70.0_f32.to_radians(),
            near: 0.05,
            far: 512.0,
        }
    }

    #[must_use]
    pub fn forward(self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        Vec3::new(sin_yaw * cos_pitch, sin_pitch, -cos_yaw * cos_pitch)
    }

    #[must_use]
    pub fn right(self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        Vec3::new(cos_yaw, 0.0, sin_yaw)
    }

    #[must_use]
    pub fn horizontal_forward(self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        Vec3::new(sin_yaw, 0.0, -cos_yaw)
    }

    #[must_use]
    pub fn view(self) -> Mat4 {
        let f = self.forward();
        let r = self.right();
        let u = r.cross(f);
        let eye = self.position;
        let mut view = Mat4::IDENTITY;
        view.cols[0] = [r.x, u.x, -f.x, 0.0];
        view.cols[1] = [r.y, u.y, -f.y, 0.0];
        view.cols[2] = [r.z, u.z, -f.z, 0.0];
        view.cols[3] = [-r.dot(eye), -u.dot(eye), f.dot(eye), 1.0];
        view
    }

    #[must_use]
    pub fn projection(self, aspect: f32) -> Mat4 {
        Mat4::perspective_rh(self.fov_y, aspect.max(0.01), self.near, self.far)
    }

    #[must_use]
    pub fn view_projection(self, viewport: Vec2) -> Mat4 {
        self.projection(viewport.x / viewport.y.max(1.0))
            .multiply(self.view())
    }

    #[must_use]
    pub fn frustum(self, viewport: Vec2) -> Frustum {
        Frustum::from_view_projection(self.view_projection(viewport))
    }

    /// Clips a world-space polygon to the near plane and projects it.
    ///
    /// Returns the number of screen vertices written to `out`; callers fan
    /// triangulate the result. Polygons are at most quads, so an eight-slot
    /// buffer is sufficient after clipping.
    pub fn project_polygon(
        self,
        view_projection: Mat4,
        viewport: Vec2,
        points: &[Vec3],
        out: &mut [Vertex3D; 8],
    ) -> usize {
        let mut clip = [[0.0_f32; 4]; 8];
        let count = points.len().min(8);
        for (index, point) in points.iter().take(count).enumerate() {
            clip[index] = view_projection.transform_vec4(*point);
        }
        let mut clipped = [[0.0_f32; 4]; 8];
        let clipped_count = clip_near(&clip[..count], &mut clipped);
        for index in 0..clipped_count {
            let c = clipped[index];
            out[index] = project_clip(Vec3::new(c[0], c[1], c[2]), c[3], viewport);
        }
        clipped_count
    }

    /// Clips a textured world-space polygon to the near plane and projects it.
    ///
    /// This is [`Self::project_polygon`] with per-corner atlas UVs and light
    /// preserved through clipping. Interpolating them in clip space keeps the
    /// perspective-correct rasterizer exact; `uv` and `light` must have at least
    /// as many entries as `points`.
    #[allow(clippy::too_many_arguments)]
    pub fn project_polygon_textured(
        self,
        view_projection: Mat4,
        viewport: Vec2,
        points: &[Vec3],
        uv: &[[f32; 2]],
        light: &[f32],
        out: &mut [Vertex3D; 8],
    ) -> usize {
        let count = points.len().min(uv.len()).min(light.len()).min(8);
        let mut vertices = [TexturedVertex::default(); 8];
        for index in 0..count {
            vertices[index] = TexturedVertex {
                clip: view_projection.transform_vec4(points[index]),
                u: uv[index][0],
                v: uv[index][1],
                light: light[index],
            };
        }
        let mut clipped = [TexturedVertex::default(); 8];
        let clipped_count = clip_near_textured(&vertices[..count], &mut clipped);
        for index in 0..clipped_count {
            let vertex = clipped[index];
            let projected = project_clip(
                Vec3::new(vertex.clip[0], vertex.clip[1], vertex.clip[2]),
                vertex.clip[3],
                viewport,
            );
            out[index] = Vertex3D::with_attributes(
                projected.x,
                projected.y,
                projected.depth,
                projected.inv_w,
                vertex.u,
                vertex.v,
                vertex.light,
            );
        }
        clipped_count
    }
}

/// A clip-space vertex carrying the attributes that survive near clipping.
#[derive(Clone, Copy, Debug)]
struct TexturedVertex {
    clip: [f32; 4],
    u: f32,
    v: f32,
    light: f32,
}

impl Default for TexturedVertex {
    fn default() -> Self {
        Self {
            clip: [0.0; 4],
            u: 0.0,
            v: 0.0,
            light: 1.0,
        }
    }
}

/// Sutherland-Hodgman clip that lerps UV/light alongside the clip position.
fn clip_near_textured(polygon: &[TexturedVertex], out: &mut [TexturedVertex; 8]) -> usize {
    if polygon.is_empty() {
        return 0;
    }
    let mut count = 0;
    for index in 0..polygon.len() {
        let current = polygon[index];
        let previous = polygon[(index + polygon.len() - 1) % polygon.len()];
        let current_in = current.clip[2] + current.clip[3] >= 0.0;
        let previous_in = previous.clip[2] + previous.clip[3] >= 0.0;
        if current_in != previous_in {
            let d_previous = previous.clip[2] + previous.clip[3];
            let d_current = current.clip[2] + current.clip[3];
            let denominator = d_previous - d_current;
            if denominator.abs() > f32::EPSILON && count < out.len() {
                let t = (d_previous / denominator).clamp(0.0, 1.0);
                out[count] = TexturedVertex {
                    clip: [
                        previous.clip[0] + (current.clip[0] - previous.clip[0]) * t,
                        previous.clip[1] + (current.clip[1] - previous.clip[1]) * t,
                        previous.clip[2] + (current.clip[2] - previous.clip[2]) * t,
                        previous.clip[3] + (current.clip[3] - previous.clip[3]) * t,
                    ],
                    u: previous.u + (current.u - previous.u) * t,
                    v: previous.v + (current.v - previous.v) * t,
                    light: previous.light + (current.light - previous.light) * t,
                };
                count += 1;
            }
        }
        if current_in && count < out.len() {
            out[count] = current;
            count += 1;
        }
    }
    count
}

/// Sutherland-Hodgman clip against the near plane (`z + w >= 0`).
fn clip_near(polygon: &[[f32; 4]], out: &mut [[f32; 4]; 8]) -> usize {
    if polygon.is_empty() {
        return 0;
    }
    let mut count = 0;
    for index in 0..polygon.len() {
        let current = polygon[index];
        let previous = polygon[(index + polygon.len() - 1) % polygon.len()];
        let current_in = current[2] + current[3] >= 0.0;
        let previous_in = previous[2] + previous[3] >= 0.0;
        if current_in != previous_in {
            let d_previous = previous[2] + previous[3];
            let d_current = current[2] + current[3];
            let denominator = d_previous - d_current;
            if denominator.abs() > f32::EPSILON && count < out.len() {
                let t = (d_previous / denominator).clamp(0.0, 1.0);
                out[count] = [
                    previous[0] + (current[0] - previous[0]) * t,
                    previous[1] + (current[1] - previous[1]) * t,
                    previous[2] + (current[2] - previous[2]) * t,
                    previous[3] + (current[3] - previous[3]) * t,
                ];
                count += 1;
            }
        }
        if current_in && count < out.len() {
            out[count] = current;
            count += 1;
        }
    }
    count
}

/// Six clip planes for axis-aligned chunk culling.
#[derive(Clone, Copy, Debug)]
pub struct Frustum {
    planes: [[f32; 4]; 6],
}

impl Frustum {
    #[must_use]
    pub fn from_view_projection(matrix: Mat4) -> Self {
        let row = |index: usize| {
            [
                matrix.cols[0][index],
                matrix.cols[1][index],
                matrix.cols[2][index],
                matrix.cols[3][index],
            ]
        };
        let r0 = row(0);
        let r1 = row(1);
        let r2 = row(2);
        let r3 = row(3);
        let combine = |a: [f32; 4], b: [f32; 4], sign: f32| {
            [
                a[0] + b[0] * sign,
                a[1] + b[1] * sign,
                a[2] + b[2] * sign,
                a[3] + b[3] * sign,
            ]
        };
        Self {
            planes: [
                combine(r3, r0, 1.0),
                combine(r3, r0, -1.0),
                combine(r3, r1, 1.0),
                combine(r3, r1, -1.0),
                combine(r3, r2, 1.0),
                combine(r3, r2, -1.0),
            ],
        }
    }

    /// Conservative AABB test: `true` unless the box is fully outside a plane.
    #[must_use]
    pub fn contains_aabb(&self, min: Vec3, max: Vec3) -> bool {
        for plane in self.planes {
            let x = if plane[0] >= 0.0 { max.x } else { min.x };
            let y = if plane[1] >= 0.0 { max.y } else { min.y };
            let z = if plane[2] >= 0.0 { max.z } else { min.z };
            if plane[0] * x + plane[1] * y + plane[2] * z + plane[3] < 0.0 {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_forward_and_right_follow_yaw() {
        let camera = Camera::new(Vec3::ZERO);
        assert_eq!(camera.forward(), Vec3::new(0.0, 0.0, -1.0));
        assert_eq!(camera.right(), Vec3::new(1.0, 0.0, 0.0));

        let turned = Camera {
            yaw: std::f32::consts::FRAC_PI_2,
            ..camera
        };
        let forward = turned.forward();
        assert!(forward.x > 0.99, "{forward:?}");
        let right = turned.right();
        assert!(right.z > 0.99, "{right:?}");
    }

    #[test]
    fn forward_point_projects_to_viewport_center() {
        let camera = Camera::new(Vec3::new(0.0, 0.0, 0.0));
        let viewport = Vec2::new(320.0, 180.0);
        let vp = camera.view_projection(viewport);
        let mut out = [Vertex3D::new(0.0, 0.0, 0.0, 0.0); 8];
        let points = [Vec3::new(0.0, 0.0, -10.0)];
        let count = camera.project_polygon(vp, viewport, &points, &mut out);
        assert_eq!(count, 1);
        assert!((out[0].x - 160.0).abs() < 0.01, "{:?}", out[0]);
        assert!((out[0].y - 90.0).abs() < 0.01, "{:?}", out[0]);
        assert!(out[0].inv_w > 0.0);
    }

    #[test]
    fn textured_projection_keeps_uvs_and_light_through_near_clipping() {
        let camera = Camera::new(Vec3::ZERO);
        let viewport = Vec2::new(320.0, 180.0);
        let vp = camera.view_projection(viewport);
        let mut out = [Vertex3D::new(0.0, 0.0, 0.0, 0.0); 8];
        let points = [
            Vec3::new(-1.0, -1.0, 1.0),
            Vec3::new(1.0, -1.0, 1.0),
            Vec3::new(1.0, 1.0, -5.0),
            Vec3::new(-1.0, 1.0, -5.0),
        ];
        let uv = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        let light = [0.4, 0.6, 0.8, 1.0];
        let count = camera.project_polygon_textured(vp, viewport, &points, &uv, &light, &mut out);
        assert!(count >= 3, "clipped polygon has {count} vertices");
        for vertex in &out[..count] {
            assert!(vertex.u.is_finite() && vertex.v.is_finite());
            assert!((0.0..=1.0).contains(&vertex.u), "{vertex:?}");
            assert!((0.0..=1.0).contains(&vertex.v), "{vertex:?}");
            assert!((0.4..=1.0).contains(&vertex.light), "{vertex:?}");
        }
    }

    #[test]
    fn fully_visible_quad_projects_uvs_unchanged() {
        let camera = Camera::new(Vec3::ZERO);
        let viewport = Vec2::new(320.0, 180.0);
        let vp = camera.view_projection(viewport);
        let mut out = [Vertex3D::new(0.0, 0.0, 0.0, 0.0); 8];
        let points = [
            Vec3::new(-1.0, -1.0, -5.0),
            Vec3::new(1.0, -1.0, -5.0),
            Vec3::new(1.0, 1.0, -5.0),
            Vec3::new(-1.0, 1.0, -5.0),
        ];
        let uv = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        let light = [0.5, 0.5, 1.0, 1.0];
        let count = camera.project_polygon_textured(vp, viewport, &points, &uv, &light, &mut out);
        assert_eq!(count, 4);
        for index in 0..4 {
            assert_eq!([out[index].u, out[index].v], uv[index]);
            assert_eq!(out[index].light, light[index]);
        }
    }

    #[test]
    fn near_plane_clips_polygons_that_cross_the_camera() {
        let camera = Camera::new(Vec3::ZERO);
        let viewport = Vec2::new(320.0, 180.0);
        let vp = camera.view_projection(viewport);
        let mut out = [Vertex3D::new(0.0, 0.0, 0.0, 0.0); 8];
        let points = [
            Vec3::new(-1.0, -1.0, 1.0),
            Vec3::new(1.0, -1.0, 1.0),
            Vec3::new(1.0, 1.0, -5.0),
        ];
        let count = camera.project_polygon(vp, viewport, &points, &mut out);
        assert!(count >= 3, "clipped polygon has {count} vertices");
        for vertex in &out[..count] {
            assert!(vertex.depth.is_finite() && vertex.depth >= 0.0);
        }
    }

    #[test]
    fn frustum_accepts_front_and_rejects_behind() {
        let camera = Camera::new(Vec3::ZERO);
        let frustum = camera.frustum(Vec2::new(320.0, 180.0));
        assert!(frustum.contains_aabb(Vec3::new(-8.0, -8.0, -40.0), Vec3::new(8.0, 8.0, -24.0)));
        assert!(!frustum.contains_aabb(Vec3::new(-8.0, -8.0, 24.0), Vec3::new(8.0, 8.0, 40.0)));
        assert!(!frustum.contains_aabb(Vec3::new(200.0, -8.0, -40.0), Vec3::new(216.0, 8.0, -24.0)));
    }
}
