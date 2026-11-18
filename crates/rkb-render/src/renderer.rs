//! Render commands, a 2D camera, and the software backend.

use std::sync::Arc;

use rkb_core::{Rect, Vec2, Vec3};

use crate::{Color, Framebuffer, Texture};

/// A projected triangle vertex for the depth-tested 3D path.
///
/// `x` and `y` are framebuffer pixels and `inv_w` is the reciprocal clip-space
/// `w`. Reciprocal depth increases towards the camera, so the backend keeps a
/// `1/w` depth buffer: it is linear in screen space (no `z/w` precision cliff
/// far from the near plane) and is also the weight needed for perspective
/// correct UV/light interpolation. `depth` is the normalized device Z kept for
/// diagnostics and tests; it is not the value compared in the depth buffer.
/// The game clips triangles against the near plane before submitting them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex3D {
    pub x: f32,
    pub y: f32,
    pub depth: f32,
    pub inv_w: f32,
    /// Atlas U coordinate, normalized to the whole atlas.
    pub u: f32,
    /// Atlas V coordinate, normalized to the whole atlas.
    pub v: f32,
    /// Per-vertex light multiplier (ambient occlusion combined with face shade).
    pub light: f32,
}

impl Vertex3D {
    #[must_use]
    pub const fn new(x: f32, y: f32, depth: f32, inv_w: f32) -> Self {
        Self {
            x,
            y,
            depth,
            inv_w,
            u: 0.0,
            v: 0.0,
            light: 1.0,
        }
    }

    #[must_use]
    pub const fn with_attributes(
        x: f32,
        y: f32,
        depth: f32,
        inv_w: f32,
        u: f32,
        v: f32,
        light: f32,
    ) -> Self {
        Self {
            x,
            y,
            depth,
            inv_w,
            u,
            v,
            light,
        }
    }
}

/// Builds a projected vertex from clip-space coordinates.
#[must_use]
pub fn project_clip(clip: Vec3, w: f32, viewport: Vec2) -> Vertex3D {
    let inv_w = if w.abs() > f32::EPSILON { 1.0 / w } else { 0.0 };
    let ndc_x = clip.x * inv_w;
    let ndc_y = clip.y * inv_w;
    let ndc_z = clip.z * inv_w;
    Vertex3D::new(
        (ndc_x * 0.5 + 0.5) * viewport.x,
        // Framebuffer rows run top to bottom, clip-space Y runs bottom to top.
        (0.5 - ndc_y * 0.5) * viewport.y,
        // Clipping lands exactly on the near plane; rounding can put the
        // value a hair outside the unit range, so clamp it here.
        ((ndc_z + 1.0) * 0.5).clamp(0.0, 1.0),
        inv_w,
    )
}

/// A single draw operation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DrawCommand {
    Rect {
        rect: Rect,
        color: Color,
    },
    StrokeRect {
        rect: Rect,
        thickness: f32,
        color: Color,
    },
    Line {
        from: Vec2,
        to: Vec2,
        color: Color,
    },
    Circle {
        center: Vec2,
        radius: f32,
        color: Color,
    },
    Triangle {
        a: Vec2,
        b: Vec2,
        c: Vec2,
        color: Color,
    },
    /// A depth-tested, flat-shaded triangle in screen space.
    Triangle3D {
        a: Vertex3D,
        b: Vertex3D,
        c: Vertex3D,
        color: Color,
    },
    /// A depth-tested triangle sampling the draw list's texture atlas.
    ///
    /// The final pixel is `atlas(u, v) * tint * vertex_light`, blended towards
    /// `fog_color` by `fog`. `alpha` scales the texel alpha, `cutout` discards
    /// pixels at or below that alpha (0 keeps only fully transparent texels
    /// out), `blend` composites source-over into the framebuffer, and
    /// `depth_write` controls whether the triangle updates the depth buffer.
    /// Opaque geometry uses `alpha=1, cutout=0, blend=false, depth_write=true`;
    /// cutout geometry raises `cutout`; translucent geometry sets `blend` and
    /// usually clears `depth_write`.
    TexturedTriangle3D {
        a: Vertex3D,
        b: Vertex3D,
        c: Vertex3D,
        tint: Color,
        fog_color: Color,
        fog: f32,
        alpha: f32,
        cutout: f32,
        blend: bool,
        depth_write: bool,
    },
    GradientTriangle {
        a: Vec2,
        b: Vec2,
        c: Vec2,
        color_a: Color,
        color_b: Color,
        color_c: Color,
    },
}

/// A frame's worth of interface-independent draw commands.
///
/// The optional atlas is shared by `Arc`, so attaching it to the list every
/// frame does not copy the texture.
#[derive(Clone, Debug, Default)]
pub struct DrawList {
    clear: Color,
    atlas: Option<Arc<Texture>>,
    commands: Vec<DrawCommand>,
}

impl DrawList {
    /// A list that clears to transparent black.
    #[must_use]
    pub fn new(clear: Color) -> Self {
        Self {
            clear,
            atlas: None,
            commands: Vec::new(),
        }
    }

    /// Shares the texture atlas used by `TexturedTriangle3D` commands.
    pub fn set_atlas(&mut self, atlas: Option<Arc<Texture>>) {
        self.atlas = atlas;
    }

    #[must_use]
    pub fn atlas(&self) -> Option<&Texture> {
        self.atlas.as_deref()
    }

    #[must_use]
    pub const fn clear_color(&self) -> Color {
        self.clear
    }

    pub fn set_clear_color(&mut self, color: Color) {
        self.clear = color;
    }

    pub fn push(&mut self, command: DrawCommand) {
        self.commands.push(command);
    }

    pub fn rect(&mut self, rect: Rect, color: Color) {
        self.push(DrawCommand::Rect { rect, color });
    }

    pub fn stroke_rect(&mut self, rect: Rect, thickness: f32, color: Color) {
        self.push(DrawCommand::StrokeRect {
            rect,
            thickness,
            color,
        });
    }

    pub fn line(&mut self, from: Vec2, to: Vec2, color: Color) {
        self.push(DrawCommand::Line { from, to, color });
    }

    pub fn circle(&mut self, center: Vec2, radius: f32, color: Color) {
        self.push(DrawCommand::Circle {
            center,
            radius,
            color,
        });
    }

    pub fn triangle(&mut self, a: Vec2, b: Vec2, c: Vec2, color: Color) {
        self.push(DrawCommand::Triangle { a, b, c, color });
    }

    /// Queues a depth-tested 3D triangle.
    pub fn triangle_3d(&mut self, a: Vertex3D, b: Vertex3D, c: Vertex3D, color: Color) {
        self.push(DrawCommand::Triangle3D { a, b, c, color });
    }

    /// Queues a depth-tested opaque triangle textured from the current atlas.
    pub fn textured_triangle_3d(
        &mut self,
        a: Vertex3D,
        b: Vertex3D,
        c: Vertex3D,
        tint: Color,
        fog_color: Color,
        fog: f32,
    ) {
        self.textured_triangle_3d_ex(a, b, c, tint, fog_color, fog, 1.0, 0.0, false, true);
    }

    /// Queues a textured triangle with an explicit cutout/blend policy.
    #[allow(clippy::too_many_arguments)]
    pub fn textured_triangle_3d_ex(
        &mut self,
        a: Vertex3D,
        b: Vertex3D,
        c: Vertex3D,
        tint: Color,
        fog_color: Color,
        fog: f32,
        alpha: f32,
        cutout: f32,
        blend: bool,
        depth_write: bool,
    ) {
        self.push(DrawCommand::TexturedTriangle3D {
            a,
            b,
            c,
            tint,
            fog_color,
            fog: fog.clamp(0.0, 1.0),
            alpha: alpha.clamp(0.0, 1.0),
            cutout: cutout.clamp(0.0, 0.999),
            blend,
            depth_write,
        });
    }

    /// Queues a screen-space triangle with per-vertex colours.
    pub fn gradient_triangle(
        &mut self,
        a: Vec2,
        b: Vec2,
        c: Vec2,
        color_a: Color,
        color_b: Color,
        color_c: Color,
    ) {
        self.push(DrawCommand::GradientTriangle {
            a,
            b,
            c,
            color_a,
            color_b,
            color_c,
        });
    }

    #[must_use]
    pub fn commands(&self) -> &[DrawCommand] {
        &self.commands
    }

    pub fn clear_commands(&mut self) {
        self.commands.clear();
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

/// Counters from a completed render.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub draw_calls: u32,
    pub triangles: u32,
    pub circles: u32,
}

/// Rendering backend error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderError(pub String);

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "render error: {}", self.0)
    }
}

impl std::error::Error for RenderError {}

/// Abstraction implemented by the software renderer and future GPU backends.
pub trait RenderBackend {
    fn resize(&mut self, width: u32, height: u32);
    fn render(&mut self, list: &DrawList) -> Result<RenderStats, RenderError>;
    fn framebuffer(&self) -> &Framebuffer;
}

/// Deterministic CPU renderer.
#[derive(Debug)]
pub struct SoftwareBackend {
    framebuffer: Framebuffer,
    /// Reciprocal-depth (`1/w`) buffer. Larger is closer; cleared to zero.
    depth: Vec<f32>,
}

impl SoftwareBackend {
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        Self {
            framebuffer: Framebuffer::new(width, height),
            depth: vec![0.0; width as usize * height as usize],
        }
    }
}

impl RenderBackend for SoftwareBackend {
    fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        self.framebuffer.resize(width, height);
        self.depth.clear();
        self.depth.resize(width as usize * height as usize, 0.0);
    }

    fn render(&mut self, list: &DrawList) -> Result<RenderStats, RenderError> {
        let framebuffer = &mut self.framebuffer;
        framebuffer.clear(list.clear_color());
        self.depth.fill(0.0);
        let mut stats = RenderStats::default();

        for command in list.commands() {
            stats.draw_calls += 1;
            match *command {
                DrawCommand::Rect { rect, color } => framebuffer.fill_rect(rect, color),
                DrawCommand::StrokeRect {
                    rect,
                    thickness,
                    color,
                } => framebuffer.stroke_rect(rect, thickness, color),
                DrawCommand::Line { from, to, color } => framebuffer.draw_line(from, to, color),
                DrawCommand::Circle {
                    center,
                    radius,
                    color,
                } => {
                    framebuffer.draw_circle(center, radius, color);
                    stats.circles += 1;
                }
                DrawCommand::Triangle { a, b, c, color } => {
                    framebuffer.draw_triangle(a, b, c, color);
                    stats.triangles += 1;
                }
                DrawCommand::Triangle3D { a, b, c, color } => {
                    rasterize_depth_triangle(framebuffer, &mut self.depth, a, b, c, color);
                    stats.triangles += 1;
                }
                DrawCommand::TexturedTriangle3D {
                    a,
                    b,
                    c,
                    tint,
                    fog_color,
                    fog,
                    alpha,
                    cutout,
                    blend,
                    depth_write,
                } => {
                    rasterize_textured_triangle(
                        framebuffer,
                        &mut self.depth,
                        a,
                        b,
                        c,
                        tint,
                        fog_color,
                        fog,
                        alpha,
                        cutout,
                        blend,
                        depth_write,
                        list.atlas(),
                    );
                    stats.triangles += 1;
                }
                DrawCommand::GradientTriangle {
                    a,
                    b,
                    c,
                    color_a,
                    color_b,
                    color_c,
                } => {
                    framebuffer.draw_triangle_gradient(a, b, c, color_a, color_b, color_c);
                    stats.triangles += 1;
                }
            }
        }
        Ok(stats)
    }

    fn framebuffer(&self) -> &Framebuffer {
        &self.framebuffer
    }
}

fn edge(a: Vertex3D, b: Vertex3D, px: f32, py: f32) -> f32 {
    (px - a.x) * (b.y - a.y) - (py - a.y) * (b.x - a.x)
}

/// Fills a triangle with reciprocal-depth testing.
///
/// The game submits triangles already clipped against the near plane and
/// projected into screen space, so this routine only handles the viewport
/// bounding box and the depth comparison. `atlas` selects the textured path;
/// when it is `None`, `tint` is used directly.
#[allow(clippy::too_many_arguments)]
fn rasterize_triangle(
    framebuffer: &mut Framebuffer,
    depth: &mut [f32],
    a: Vertex3D,
    b: Vertex3D,
    c: Vertex3D,
    tint: Color,
    fog_color: Color,
    fog: f32,
    alpha: f32,
    cutout: f32,
    blend: bool,
    depth_write: bool,
    atlas: Option<&Texture>,
) {
    let width = framebuffer.width() as i32;
    let height = framebuffer.height() as i32;
    if width == 0 || height == 0 {
        return;
    }
    let area = edge(a, b, c.x, c.y);
    if area.abs() < f32::EPSILON {
        return;
    }
    let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as i32;
    let max_x = a.x.max(b.x).max(c.x).ceil().min(width as f32);
    let min_y = a.y.min(b.y).min(c.y).floor().max(0.0) as i32;
    let max_y = a.y.max(b.y).max(c.y).ceil().min(height as f32);
    if min_x >= max_x as i32 || min_y >= max_y as i32 {
        return;
    }

    let inverse_area = 1.0 / area;
    let fog = fog.clamp(0.0, 1.0);
    let pixels = framebuffer.pixels_mut();
    for y in min_y..max_y as i32 {
        let py = y as f32 + 0.5;
        for x in min_x..max_x as i32 {
            let px = x as f32 + 0.5;
            let w0 = edge(b, c, px, py);
            let w1 = edge(c, a, px, py);
            let w2 = edge(a, b, px, py);
            let inside = if area > 0.0 {
                w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0
            } else {
                w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0
            };
            if !inside {
                continue;
            }
            let l0 = w0 * inverse_area;
            let l1 = w1 * inverse_area;
            let l2 = w2 * inverse_area;
            // Reciprocal depth is linear in screen space, so interpolating it
            // with barycentrics is exact. Larger is closer.
            let inv_w = l0 * a.inv_w + l1 * b.inv_w + l2 * c.inv_w;
            if inv_w <= 0.0 {
                continue;
            }
            let index = y as usize * width as usize + x as usize;
            if inv_w <= depth[index] {
                continue;
            }

            let (red, green, blue, source_alpha) = if let Some(atlas) = atlas {
                let inv = 1.0 / inv_w;
                let u = (l0 * a.u * a.inv_w + l1 * b.u * b.inv_w + l2 * c.u * c.inv_w) * inv;
                let v = (l0 * a.v * a.inv_w + l1 * b.v * b.inv_w + l2 * c.v * c.inv_w) * inv;
                let light =
                    (l0 * a.light * a.inv_w + l1 * b.light * b.inv_w + l2 * c.light * c.inv_w)
                        * inv;
                let texel = atlas.sample_nearest(u, v);
                let source_alpha = f32::from(texel.a) / 255.0 * alpha;
                if source_alpha <= cutout {
                    continue;
                }
                let light = light.clamp(0.0, 1.5);
                let scale = |channel: u8, tint: u8| -> f32 {
                    f32::from(channel) * f32::from(tint) / 255.0 * light
                };
                (
                    scale(texel.r, tint.r),
                    scale(texel.g, tint.g),
                    scale(texel.b, tint.b),
                    source_alpha,
                )
            } else {
                let source_alpha = alpha;
                if source_alpha <= cutout {
                    continue;
                }
                (
                    f32::from(tint.r),
                    f32::from(tint.g),
                    f32::from(tint.b),
                    source_alpha,
                )
            };

            let (red, green, blue) = if fog > 0.0 {
                let mix = |near: f32, far: u8| -> u8 {
                    (near + (f32::from(far) - near) * fog)
                        .round()
                        .clamp(0.0, 255.0) as u8
                };
                (
                    mix(red, fog_color.r),
                    mix(green, fog_color.g),
                    mix(blue, fog_color.b),
                )
            } else {
                (
                    red.round().clamp(0.0, 255.0) as u8,
                    green.round().clamp(0.0, 255.0) as u8,
                    blue.round().clamp(0.0, 255.0) as u8,
                )
            };

            if depth_write {
                depth[index] = inv_w;
            }
            let offset = index * 4;
            if blend {
                let inverse = 1.0 - source_alpha;
                pixels[offset] = (f32::from(red) * source_alpha
                    + f32::from(pixels[offset]) * inverse)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                pixels[offset + 1] = (f32::from(green) * source_alpha
                    + f32::from(pixels[offset + 1]) * inverse)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                pixels[offset + 2] = (f32::from(blue) * source_alpha
                    + f32::from(pixels[offset + 2]) * inverse)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            } else {
                pixels[offset] = red;
                pixels[offset + 1] = green;
                pixels[offset + 2] = blue;
            }
            pixels[offset + 3] = 255;
        }
    }
}

fn rasterize_depth_triangle(
    framebuffer: &mut Framebuffer,
    depth: &mut [f32],
    a: Vertex3D,
    b: Vertex3D,
    c: Vertex3D,
    color: Color,
) {
    rasterize_triangle(
        framebuffer,
        depth,
        a,
        b,
        c,
        color,
        color,
        0.0,
        1.0,
        0.0,
        false,
        true,
        None,
    );
}

#[allow(clippy::too_many_arguments)]
fn rasterize_textured_triangle(
    framebuffer: &mut Framebuffer,
    depth: &mut [f32],
    a: Vertex3D,
    b: Vertex3D,
    c: Vertex3D,
    tint: Color,
    fog_color: Color,
    fog: f32,
    alpha: f32,
    cutout: f32,
    blend: bool,
    depth_write: bool,
    atlas: Option<&Texture>,
) {
    // Without an attached atlas the flat tint keeps the command meaningful.
    rasterize_triangle(
        framebuffer,
        depth,
        a,
        b,
        c,
        tint,
        fog_color,
        fog,
        alpha,
        cutout,
        blend,
        depth_write,
        atlas,
    );
}

/// A 2D camera that maps world coordinates to viewport pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera2D {
    pub position: Vec2,
    pub zoom: f32,
    pub viewport: Vec2,
}

impl Camera2D {
    #[must_use]
    pub fn new(viewport: Vec2) -> Self {
        Self {
            position: Vec2::ZERO,
            zoom: 1.0,
            viewport,
        }
    }

    pub fn set_viewport(&mut self, width: f32, height: f32) {
        self.viewport = Vec2::new(width.max(1.0), height.max(1.0));
    }

    #[must_use]
    pub fn world_to_screen(self, world: Vec2) -> Vec2 {
        let zoom = if self.zoom > f32::EPSILON {
            self.zoom
        } else {
            1.0
        };
        (world - self.position) * zoom + self.viewport * 0.5
    }

    #[must_use]
    pub fn screen_to_world(self, screen: Vec2) -> Vec2 {
        let zoom = if self.zoom > f32::EPSILON {
            self.zoom
        } else {
            1.0
        };
        (screen - self.viewport * 0.5) / zoom + self.position
    }

    /// World-space rectangle visible through the camera.
    #[must_use]
    pub fn visible_rect(self) -> Rect {
        let zoom = if self.zoom > f32::EPSILON {
            self.zoom
        } else {
            1.0
        };
        let size = self.viewport / zoom;
        Rect::from_center(self.position, size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn software_backend_executes_commands() {
        let mut backend = SoftwareBackend::new(16, 16);
        let mut list = DrawList::new(Color::BLACK);
        list.rect(Rect::new(2.0, 2.0, 4.0, 4.0), Color::RED);
        list.circle(Vec2::new(10.0, 10.0), 2.0, Color::GREEN);
        let stats = backend.render(&list).unwrap();
        assert_eq!(stats.draw_calls, 2);
        assert_eq!(stats.circles, 1);
        assert_eq!(backend.framebuffer().pixel(3, 3), Some(Color::RED));
        assert_eq!(backend.framebuffer().pixel(10, 10), Some(Color::GREEN));
    }

    #[test]
    fn resize_changes_output() {
        let mut backend = SoftwareBackend::new(8, 8);
        backend.resize(20, 10);
        assert_eq!(backend.framebuffer().width(), 20);
        assert_eq!(backend.framebuffer().height(), 10);
    }

    #[test]
    fn camera_round_trips_coordinates() {
        let camera = Camera2D {
            position: Vec2::new(10.0, -4.0),
            zoom: 2.0,
            viewport: Vec2::new(100.0, 50.0),
        };
        let screen = camera.world_to_screen(Vec2::new(20.0, 6.0));
        assert_eq!(screen, Vec2::new(70.0, 45.0));
        assert_eq!(camera.screen_to_world(screen), Vec2::new(20.0, 6.0));
    }

    #[test]
    fn visible_rect_uses_zoom() {
        let camera = Camera2D {
            position: Vec2::new(5.0, 5.0),
            zoom: 2.0,
            viewport: Vec2::new(100.0, 50.0),
        };
        assert_eq!(camera.visible_rect(), Rect::new(-20.0, -7.5, 50.0, 25.0));
    }

    #[test]
    fn draw_list_counts_commands() {
        let mut list = DrawList::new(Color::BLACK);
        assert!(list.is_empty());
        list.line(Vec2::ZERO, Vec2::ONE, Color::WHITE);
        assert_eq!(list.len(), 1);
        list.clear_commands();
        assert!(list.is_empty());
    }

    fn full_screen_triangle(inv_w: f32) -> (Vertex3D, Vertex3D, Vertex3D) {
        // NDC depth is reported alongside reciprocal depth; the backend tests
        // the reciprocal value, which is larger for nearer geometry.
        let depth = 1.0 - inv_w * 0.5;
        (
            Vertex3D::new(0.0, 0.0, depth, inv_w),
            Vertex3D::new(16.0, 0.0, depth, inv_w),
            Vertex3D::new(0.0, 16.0, depth, inv_w),
        )
    }

    #[test]
    fn depth_test_keeps_nearer_triangle_in_either_order() {
        for order in [true, false] {
            let mut backend = SoftwareBackend::new(16, 16);
            let mut list = DrawList::new(Color::BLACK);
            let (a, b, c) = full_screen_triangle(0.5);
            let far = [a, b, c];
            let (a, b, c) = full_screen_triangle(2.0);
            let near = [a, b, c];
            let (first, first_color, second, second_color) = if order {
                (far, Color::BLUE, near, Color::RED)
            } else {
                (near, Color::RED, far, Color::BLUE)
            };
            list.triangle_3d(first[0], first[1], first[2], first_color);
            list.triangle_3d(second[0], second[1], second[2], second_color);
            let stats = backend.render(&list).unwrap();
            assert_eq!(stats.triangles, 2);
            assert_eq!(
                backend.framebuffer().pixel(2, 2),
                Some(Color::RED),
                "nearer triangle must win (near first: {order})"
            );
        }
    }

    #[test]
    fn depth_buffer_is_cleared_each_frame_and_on_resize() {
        let mut backend = SoftwareBackend::new(8, 8);
        let mut list = DrawList::new(Color::BLACK);
        let (a, b, c) = full_screen_triangle(1.0);
        list.triangle_3d(a, b, c, Color::GREEN);
        backend.render(&list).unwrap();
        assert_eq!(backend.framebuffer().pixel(1, 1), Some(Color::GREEN));

        list.clear_commands();
        list.set_clear_color(Color::BLACK);
        let (a, b, c) = full_screen_triangle(0.1);
        list.triangle_3d(a, b, c, Color::BLUE);
        backend.render(&list).unwrap();
        assert_eq!(backend.framebuffer().pixel(1, 1), Some(Color::BLUE));

        backend.resize(4, 4);
        let (a, b, c) = full_screen_triangle(0.1);
        list.clear_commands();
        list.triangle_3d(a, b, c, Color::YELLOW);
        backend.render(&list).unwrap();
        assert_eq!(backend.framebuffer().pixel(1, 1), Some(Color::YELLOW));
    }

    #[test]
    fn textured_triangles_sample_the_attached_atlas() {
        // Two texels: red on the left, blue on the right.
        let mut texels = Vec::new();
        texels.extend_from_slice(&[255, 0, 0, 255]);
        texels.extend_from_slice(&[0, 0, 255, 255]);
        let atlas = Arc::new(Texture::from_rgba(2, 1, texels));

        let mut backend = SoftwareBackend::new(16, 8);
        let mut list = DrawList::new(Color::BLACK);
        list.set_atlas(Some(Arc::clone(&atlas)));
        let a = Vertex3D::with_attributes(0.0, 0.0, 0.5, 1.0, 0.01, 0.5, 1.0);
        let b = Vertex3D::with_attributes(16.0, 0.0, 0.5, 1.0, 0.99, 0.5, 1.0);
        let c = Vertex3D::with_attributes(0.0, 8.0, 0.5, 1.0, 0.01, 0.5, 1.0);
        list.textured_triangle_3d(a, b, c, Color::WHITE, Color::BLACK, 0.0);
        backend.render(&list).unwrap();

        assert_eq!(
            backend.framebuffer().pixel(1, 1),
            Some(Color::rgb(255, 0, 0))
        );
        assert_eq!(
            backend.framebuffer().pixel(12, 1),
            Some(Color::rgb(0, 0, 255))
        );
    }

    #[test]
    fn textured_triangles_apply_light_and_fog() {
        let mut texels = Vec::new();
        texels.extend_from_slice(&[255, 255, 255, 255]);
        let atlas = Arc::new(Texture::from_rgba(1, 1, texels));
        let mut backend = SoftwareBackend::new(8, 8);
        let mut list = DrawList::new(Color::BLACK);
        list.set_atlas(Some(atlas));
        let a = Vertex3D::with_attributes(0.0, 0.0, 0.5, 1.0, 0.5, 0.5, 0.5);
        let b = Vertex3D::with_attributes(8.0, 0.0, 0.5, 1.0, 0.5, 0.5, 0.5);
        let c = Vertex3D::with_attributes(0.0, 8.0, 0.5, 1.0, 0.5, 0.5, 0.5);
        list.textured_triangle_3d(a, b, c, Color::WHITE, Color::BLACK, 0.0);
        backend.render(&list).unwrap();
        // White * 0.5 light = 128.
        assert_eq!(
            backend.framebuffer().pixel(1, 1),
            Some(Color::rgb(128, 128, 128))
        );

        list.clear_commands();
        let a = Vertex3D::with_attributes(0.0, 0.0, 0.5, 1.0, 0.5, 0.5, 1.0);
        let b = Vertex3D::with_attributes(8.0, 0.0, 0.5, 1.0, 0.5, 0.5, 1.0);
        let c = Vertex3D::with_attributes(0.0, 8.0, 0.5, 1.0, 0.5, 0.5, 1.0);
        list.textured_triangle_3d(a, b, c, Color::WHITE, Color::BLACK, 1.0);
        backend.render(&list).unwrap();
        assert_eq!(backend.framebuffer().pixel(1, 1), Some(Color::BLACK));
    }

    #[test]
    fn coplanar_quads_leave_no_holes_at_their_shared_edge() {
        // A top face and a side face meeting along one edge, as in a terrain
        // step. Reciprocal depth must not let the seam punch background
        // through the geometry.
        let mut backend = SoftwareBackend::new(32, 32);
        let mut list = DrawList::new(Color::rgb(255, 0, 255));
        let top = (
            Vertex3D::new(0.0, 8.0, 0.5, 0.25),
            Vertex3D::new(32.0, 8.0, 0.5, 0.25),
            Vertex3D::new(0.0, 16.0, 0.5, 0.5),
        );
        let side = (
            Vertex3D::new(0.0, 16.0, 0.5, 0.5),
            Vertex3D::new(32.0, 16.0, 0.5, 0.5),
            Vertex3D::new(0.0, 24.0, 0.5, 1.0),
        );
        list.triangle_3d(top.0, top.1, top.2, Color::WHITE);
        list.triangle_3d(side.0, side.1, side.2, Color::GREEN);
        // The other halves of the two quads.
        list.triangle_3d(
            top.1,
            top.2,
            Vertex3D::new(32.0, 16.0, 0.5, 0.5),
            Color::WHITE,
        );
        list.triangle_3d(
            side.1,
            side.2,
            Vertex3D::new(32.0, 24.0, 0.5, 1.0),
            Color::GREEN,
        );
        backend.render(&list).unwrap();
        let magenta = Color::rgb(255, 0, 255);
        for y in 9..24 {
            for x in 1..31 {
                assert_ne!(
                    backend.framebuffer().pixel(x, y),
                    Some(magenta),
                    "hole at ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn project_clip_maps_center_and_edges() {
        let viewport = Vec2::new(100.0, 50.0);
        let center = project_clip(Vec3::new(0.0, 0.0, 0.0), 1.0, viewport);
        assert_eq!((center.x, center.y), (50.0, 25.0));
        assert!((center.depth - 0.5).abs() < 1.0e-6);

        let corner = project_clip(Vec3::new(-1.0, 1.0, 1.0), 1.0, viewport);
        assert_eq!((corner.x, corner.y), (0.0, 0.0));
        assert!((corner.depth - 1.0).abs() < 1.0e-6);
    }

    #[test]
    fn cutout_discards_transparent_texels() {
        // Left texel is transparent, right texel is red.
        let mut texels = Vec::new();
        texels.extend_from_slice(&[0, 0, 0, 0]);
        texels.extend_from_slice(&[255, 0, 0, 255]);
        let atlas = Arc::new(Texture::from_rgba(2, 1, texels));
        let mut backend = SoftwareBackend::new(16, 8);
        let mut list = DrawList::new(Color::BLACK);
        list.set_atlas(Some(atlas));
        let a = Vertex3D::with_attributes(0.0, 0.0, 0.5, 1.0, 0.01, 0.5, 1.0);
        let b = Vertex3D::with_attributes(16.0, 0.0, 0.5, 1.0, 0.99, 0.5, 1.0);
        let c = Vertex3D::with_attributes(0.0, 8.0, 0.5, 1.0, 0.01, 0.5, 1.0);
        list.textured_triangle_3d_ex(
            a,
            b,
            c,
            Color::WHITE,
            Color::BLACK,
            0.0,
            1.0,
            0.5,
            false,
            true,
        );
        backend.render(&list).unwrap();
        assert_eq!(backend.framebuffer().pixel(1, 1), Some(Color::BLACK));
        assert_eq!(
            backend.framebuffer().pixel(12, 1),
            Some(Color::rgb(255, 0, 0))
        );
    }

    #[test]
    fn translucent_triangles_blend_source_over() {
        let atlas = Arc::new(Texture::from_rgba(1, 1, vec![255, 255, 255, 255]));
        let mut backend = SoftwareBackend::new(8, 8);
        let mut list = DrawList::new(Color::rgb(0, 0, 255));
        list.set_atlas(Some(atlas));
        let a = Vertex3D::with_attributes(0.0, 0.0, 0.5, 1.0, 0.5, 0.5, 1.0);
        let b = Vertex3D::with_attributes(8.0, 0.0, 0.5, 1.0, 0.5, 0.5, 1.0);
        let c = Vertex3D::with_attributes(0.0, 8.0, 0.5, 1.0, 0.5, 0.5, 1.0);
        list.textured_triangle_3d_ex(
            a,
            b,
            c,
            Color::WHITE,
            Color::BLACK,
            0.0,
            0.5,
            0.0,
            true,
            false,
        );
        backend.render(&list).unwrap();
        let pixel = backend.framebuffer().pixel(1, 1).unwrap();
        assert!((127..=128).contains(&pixel.r), "{pixel:?}");
        assert!((127..=128).contains(&pixel.g), "{pixel:?}");
        assert_eq!(pixel.b, 255, "{pixel:?}");
    }

    #[test]
    fn translucent_triangles_do_not_write_depth() {
        let atlas = Arc::new(Texture::from_rgba(1, 1, vec![255, 255, 255, 255]));
        let mut backend = SoftwareBackend::new(8, 8);
        let mut list = DrawList::new(Color::BLACK);
        list.set_atlas(Some(atlas));
        let near = || {
            let a = Vertex3D::with_attributes(0.0, 0.0, 0.5, 0.25, 0.5, 0.5, 1.0);
            let b = Vertex3D::with_attributes(8.0, 0.0, 0.5, 0.25, 0.5, 0.5, 1.0);
            let c = Vertex3D::with_attributes(0.0, 8.0, 0.5, 0.25, 0.5, 0.5, 1.0);
            (a, b, c)
        };
        // Opaque backdrop at mid depth.
        let (a, b, c) = near();
        let a = Vertex3D { inv_w: 0.2, ..a };
        let b = Vertex3D { inv_w: 0.2, ..b };
        let c = Vertex3D { inv_w: 0.2, ..c };
        list.triangle_3d(a, b, c, Color::BLUE);
        // Nearer translucent face that must not block the next opaque face.
        let (a, b, c) = near();
        list.textured_triangle_3d_ex(
            a,
            b,
            c,
            Color::GREEN,
            Color::BLACK,
            0.0,
            1.0,
            0.0,
            true,
            false,
        );
        // Opaque face between the translucent face and the backdrop.
        let (a, b, c) = near();
        let a = Vertex3D { inv_w: 0.3, ..a };
        let b = Vertex3D { inv_w: 0.3, ..b };
        let c = Vertex3D { inv_w: 0.3, ..c };
        list.triangle_3d(a, b, c, Color::RED);
        backend.render(&list).unwrap();
        assert_eq!(backend.framebuffer().pixel(1, 1), Some(Color::RED));
    }
}
