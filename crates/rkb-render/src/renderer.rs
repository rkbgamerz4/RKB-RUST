//! Render commands, a 2D camera, and the software backend.

use rkb_core::{Rect, Vec2};

use crate::{Color, Framebuffer};

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
}

impl SoftwareBackend {
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            framebuffer: Framebuffer::new(width, height),
        }
    }
}

impl RenderBackend for SoftwareBackend {
    fn resize(&mut self, width: u32, height: u32) {
        self.framebuffer.resize(width, height);
    }

    fn render(&mut self, list: &DrawList) -> Result<RenderStats, RenderError> {
        let framebuffer = &mut self.framebuffer;
        framebuffer.clear(list.clear_color());
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
}
