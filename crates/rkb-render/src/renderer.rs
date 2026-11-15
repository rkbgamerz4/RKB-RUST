//! Render commands, a 2D camera, and the software backend.

use rkb_core::{Rect, Vec2, Vec3};

use crate::{Color, Framebuffer};

/// A projected triangle vertex for the depth-tested 3D path.
///
/// `x` and `y` are framebuffer pixels, `depth` is normalized device Z in
/// `[0, 1]` (lower is closer), and `inv_w` is the reciprocal clip-space `w`
/// used for perspective-correct depth interpolation. The game is responsible
/// for clipping triangles against the near plane before submitting them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex3D {
    pub x: f32,
    pub y: f32,
    pub depth: f32,
    pub inv_w: f32,
}

impl Vertex3D {
    #[must_use]
    pub const fn new(x: f32, y: f32, depth: f32, inv_w: f32) -> Self {
        Self { x, y, depth, inv_w }
    }
}

/// Builds a projected vertex from clip-space coordinates.
#[must_use]
pub fn project_clip(clip: Vec3, w: f32, viewport: Vec2) -> Vertex3D {
    let inv_w = if w.abs() > f32::EPSILON { 1.0 / w } else { 0.0 };
    let ndc_x = clip.x * inv_w;
    let ndc_y = clip.y * inv_w;
    let ndc_z = clip.z * inv_w;
    Vertex3D {
        x: (ndc_x * 0.5 + 0.5) * viewport.x,
        // Framebuffer rows run top to bottom, clip-space Y runs bottom to top.
        y: (0.5 - ndc_y * 0.5) * viewport.y,
        // Clipping lands exactly on the near plane; rounding can put the
        // value a hair outside the unit range, so clamp it here.
        depth: ((ndc_z + 1.0) * 0.5).clamp(0.0, 1.0),
        inv_w,
    }
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
#[derive(Clone, Debug, Default)]
pub struct DrawList {
    clear: Color,
    commands: Vec<DrawCommand>,
}

impl DrawList {
    /// A list that clears to transparent black.
    #[must_use]
    pub fn new(clear: Color) -> Self {
        Self {
            clear,
            commands: Vec::new(),
        }
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
    depth: Vec<f32>,
}

impl SoftwareBackend {
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        Self {
            framebuffer: Framebuffer::new(width, height),
            depth: vec![1.0; width as usize * height as usize],
        }
    }
}

impl RenderBackend for SoftwareBackend {
    fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        self.framebuffer.resize(width, height);
        self.depth.clear();
        self.depth.resize(width as usize * height as usize, 1.0);
    }

    fn render(&mut self, list: &DrawList) -> Result<RenderStats, RenderError> {
        let framebuffer = &mut self.framebuffer;
        framebuffer.clear(list.clear_color());
        self.depth.fill(1.0);
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

/// Fills a triangle with perspective-correct depth testing.
///
/// The game submits triangles already clipped against the near plane and
/// projected into screen space, so this routine only handles the viewport
/// bounding box and the depth comparison.
fn rasterize_depth_triangle(
    framebuffer: &mut Framebuffer,
    depth: &mut [f32],
    a: Vertex3D,
    b: Vertex3D,
    c: Vertex3D,
    color: Color,
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
            let inv_w = l0 * a.inv_w + l1 * b.inv_w + l2 * c.inv_w;
            let z = if inv_w.abs() > f32::EPSILON {
                (l0 * a.depth * a.inv_w + l1 * b.depth * b.inv_w + l2 * c.depth * c.inv_w) / inv_w
            } else {
                l0 * a.depth + l1 * b.depth + l2 * c.depth
            };
            if !(0.0..=1.0).contains(&z) {
                continue;
            }
            let index = y as usize * width as usize + x as usize;
            if z < depth[index] {
                depth[index] = z;
                let offset = index * 4;
                pixels[offset] = color.r;
                pixels[offset + 1] = color.g;
                pixels[offset + 2] = color.b;
                pixels[offset + 3] = 255;
            }
        }
    }
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

    fn full_screen_triangle(depth: f32) -> (Vertex3D, Vertex3D, Vertex3D) {
        (
            Vertex3D::new(0.0, 0.0, depth, 1.0),
            Vertex3D::new(16.0, 0.0, depth, 1.0),
            Vertex3D::new(0.0, 16.0, depth, 1.0),
        )
    }

    #[test]
    fn depth_test_keeps_nearer_triangle_in_either_order() {
        for order in [true, false] {
            let mut backend = SoftwareBackend::new(16, 16);
            let mut list = DrawList::new(Color::BLACK);
            let (a, b, c) = full_screen_triangle(0.8);
            let far = [a, b, c];
            let (a, b, c) = full_screen_triangle(0.2);
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
        let (a, b, c) = full_screen_triangle(0.5);
        list.triangle_3d(a, b, c, Color::GREEN);
        backend.render(&list).unwrap();
        assert_eq!(backend.framebuffer().pixel(1, 1), Some(Color::GREEN));

        list.clear_commands();
        list.set_clear_color(Color::BLACK);
        let (a, b, c) = full_screen_triangle(0.9);
        list.triangle_3d(a, b, c, Color::BLUE);
        backend.render(&list).unwrap();
        assert_eq!(backend.framebuffer().pixel(1, 1), Some(Color::BLUE));

        backend.resize(4, 4);
        let (a, b, c) = full_screen_triangle(0.9);
        list.clear_commands();
        list.triangle_3d(a, b, c, Color::YELLOW);
        backend.render(&list).unwrap();
        assert_eq!(backend.framebuffer().pixel(1, 1), Some(Color::YELLOW));
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
}
