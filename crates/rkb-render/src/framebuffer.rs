//! CPU framebuffer with clipping, alpha blending, and basic primitives.

use std::io::{self, Write};

use rkb_core::{Rect, Vec2};

use crate::{Color, Texture};

/// A tightly packed RGBA8 image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Framebuffer {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Framebuffer {
    /// Creates a transparent framebuffer.
    ///
    /// # Panics
    ///
    /// Panics if either dimension is zero.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        assert!(
            width > 0 && height > 0,
            "framebuffer dimensions must be non-zero"
        );
        Self {
            width,
            height,
            pixels: vec![0; width as usize * height as usize * 4],
        }
    }

    #[must_use]
    pub fn filled(width: u32, height: u32, color: Color) -> Self {
        let mut framebuffer = Self::new(width, height);
        framebuffer.clear(color);
        framebuffer
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    #[must_use]
    pub fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.width == width && self.height == height {
            return;
        }
        self.width = width;
        self.height = height;
        self.pixels.clear();
        self.pixels.resize(width as usize * height as usize * 4, 0);
    }

    pub fn clear(&mut self, color: Color) {
        let rgba = color.to_array();
        for pixel in self.pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&rgba);
        }
    }

    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<Color> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = (y as usize * self.width as usize + x as usize) * 4;
        Some(Color::from([
            self.pixels[offset],
            self.pixels[offset + 1],
            self.pixels[offset + 2],
            self.pixels[offset + 3],
        ]))
    }

    pub fn set_pixel(&mut self, x: i32, y: i32, color: Color) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let offset = (y as usize * self.width as usize + x as usize) * 4;
        self.pixels[offset..offset + 4].copy_from_slice(&color.to_array());
    }

    /// Source-over alpha blending into an existing pixel.
    pub fn blend_pixel(&mut self, x: i32, y: i32, source: Color) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        if source.a == 0 {
            return;
        }
        if source.a == 255 {
            self.set_pixel(x, y, source);
            return;
        }

        let offset = (y as usize * self.width as usize + x as usize) * 4;
        let destination = &mut self.pixels[offset..offset + 4];
        let inverse_alpha = 255 - u32::from(source.a);
        let destination_alpha = u32::from(destination[3]);
        let out_alpha = u32::from(source.a) + destination_alpha * inverse_alpha / 255;
        if out_alpha == 0 {
            destination.fill(0);
            return;
        }

        for (channel, source_channel) in [source.r, source.g, source.b].into_iter().enumerate() {
            let destination_channel = u32::from(destination[channel]);
            let numerator = u32::from(source_channel) * u32::from(source.a)
                + destination_channel * destination_alpha * inverse_alpha / 255;
            destination[channel] = ((numerator + out_alpha / 2) / out_alpha) as u8;
        }
        destination[3] = out_alpha as u8;
    }

    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        let (x0, y0, x1, y1) = clipped_bounds(rect, self.width, self.height);
        for y in y0..y1 {
            for x in x0..x1 {
                self.blend_pixel(x, y, color);
            }
        }
    }

    pub fn stroke_rect(&mut self, rect: Rect, thickness: f32, color: Color) {
        let thickness = thickness.max(0.0).min(rect.width.min(rect.height) * 0.5);
        if thickness <= 0.0 {
            return;
        }
        self.fill_rect(Rect::new(rect.x, rect.y, rect.width, thickness), color);
        self.fill_rect(
            Rect::new(
                rect.x,
                rect.y + rect.height - thickness,
                rect.width,
                thickness,
            ),
            color,
        );
        self.fill_rect(
            Rect::new(
                rect.x,
                rect.y + thickness,
                thickness,
                rect.height - thickness * 2.0,
            ),
            color,
        );
        self.fill_rect(
            Rect::new(
                rect.x + rect.width - thickness,
                rect.y + thickness,
                thickness,
                rect.height - thickness * 2.0,
            ),
            color,
        );
    }

    pub fn draw_line(&mut self, from: Vec2, to: Vec2, color: Color) {
        let mut x0 = from.x.round() as i32;
        let mut y0 = from.y.round() as i32;
        let x1 = to.x.round() as i32;
        let y1 = to.y.round() as i32;
        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut error = dx + dy;

        loop {
            self.blend_pixel(x0, y0, color);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let doubled = error * 2;
            if doubled >= dy {
                error += dy;
                x0 += sx;
            }
            if doubled <= dx {
                error += dx;
                y0 += sy;
            }
        }
    }

    pub fn draw_circle(&mut self, center: Vec2, radius: f32, color: Color) {
        if radius <= 0.0 {
            return;
        }
        let rect = Rect::from_center(center, Vec2::splat(radius * 2.0));
        let (x0, y0, x1, y1) = clipped_bounds(rect, self.width, self.height);
        let radius_squared = radius * radius;
        for y in y0..y1 {
            for x in x0..x1 {
                let dx = x as f32 + 0.5 - center.x;
                let dy = y as f32 + 0.5 - center.y;
                if dx.mul_add(dx, dy * dy) <= radius_squared {
                    self.blend_pixel(x, y, color);
                }
            }
        }
    }

    /// Rasterizes a flat-shaded triangle with a half-space test.
    pub fn draw_triangle(&mut self, a: Vec2, b: Vec2, c: Vec2, color: Color) {
        self.draw_triangle_gradient(a, b, c, color, color, color);
    }

    /// Rasterizes a Gouraud-shaded triangle.
    pub fn draw_triangle_gradient(
        &mut self,
        a: Vec2,
        b: Vec2,
        c: Vec2,
        color_a: Color,
        color_b: Color,
        color_c: Color,
    ) {
        let area = edge(a, b, c);
        if area.abs() < f32::EPSILON {
            return;
        }
        let min_x = a.x.min(b.x).min(c.x).floor();
        let min_y = a.y.min(b.y).min(c.y).floor();
        let max_x = a.x.max(b.x).max(c.x).ceil();
        let max_y = a.y.max(b.y).max(c.y).ceil();
        let rect = Rect::new(min_x, min_y, max_x - min_x, max_y - min_y);
        let (x0, y0, x1, y1) = clipped_bounds(rect, self.width, self.height);

        for y in y0..y1 {
            for x in x0..x1 {
                let point = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let w0 = edge(b, c, point) / area;
                let w1 = edge(c, a, point) / area;
                let w2 = edge(a, b, point) / area;
                if w0 >= -0.000_1 && w1 >= -0.000_1 && w2 >= -0.000_1 {
                    let color = Color::rgba(
                        lerp_channel(color_a.r, color_b.r, color_c.r, w0, w1, w2),
                        lerp_channel(color_a.g, color_b.g, color_c.g, w0, w1, w2),
                        lerp_channel(color_a.b, color_b.b, color_c.b, w0, w1, w2),
                        lerp_channel(color_a.a, color_b.a, color_c.a, w0, w1, w2),
                    );
                    self.blend_pixel(x, y, color);
                }
            }
        }
    }

    /// Draws a texture scaled into `destination` with nearest-neighbour sampling.
    pub fn draw_texture(&mut self, texture: &Texture, destination: Rect, tint: Color) {
        let (x0, y0, x1, y1) = clipped_bounds(destination, self.width, self.height);
        if x1 <= x0 || y1 <= y0 || destination.width <= 0.0 || destination.height <= 0.0 {
            return;
        }
        for y in y0..y1 {
            let v = (y as f32 + 0.5 - destination.y) / destination.height;
            for x in x0..x1 {
                let u = (x as f32 + 0.5 - destination.x) / destination.width;
                let sample = texture.sample_nearest(u, v);
                let source = Color::rgba(
                    (u16::from(sample.r) * u16::from(tint.r) / 255) as u8,
                    (u16::from(sample.g) * u16::from(tint.g) / 255) as u8,
                    (u16::from(sample.b) * u16::from(tint.b) / 255) as u8,
                    (u16::from(sample.a) * u16::from(tint.a) / 255) as u8,
                );
                self.blend_pixel(x, y, source);
            }
        }
    }

    /// FNV-1a checksum used by deterministic visual regression tests.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in &self.pixels {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    /// Writes a binary PPM (RGB) for easy inspection without image dependencies.
    pub fn write_ppm<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        writeln!(writer, "P6\n{} {}\n255", self.width, self.height)?;
        let mut row = Vec::with_capacity(self.width as usize * 3);
        for line in self.pixels.chunks_exact(self.width as usize * 4) {
            row.clear();
            for pixel in line.chunks_exact(4) {
                row.extend_from_slice(&pixel[..3]);
            }
            writer.write_all(&row)?;
        }
        Ok(())
    }
}

fn clipped_bounds(rect: Rect, width: u32, height: u32) -> (i32, i32, i32, i32) {
    let x0 = rect.x.max(0.0).min(width as f32) as i32;
    let y0 = rect.y.max(0.0).min(height as f32) as i32;
    let x1 = (rect.x + rect.width).max(0.0).min(width as f32) as i32;
    let y1 = (rect.y + rect.height).max(0.0).min(height as f32) as i32;
    (x0, y0, x1.max(x0), y1.max(y0))
}

fn edge(a: Vec2, b: Vec2, c: Vec2) -> f32 {
    (c.x - a.x).mul_add(b.y - a.y, -(c.y - a.y) * (b.x - a.x))
}

fn lerp_channel(a: u8, b: u8, c: u8, wa: f32, wb: f32, wc: f32) -> u8 {
    let value = f32::from(a) * wa + f32::from(b) * wb + f32::from(c) * wc;
    value.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_fills_every_pixel() {
        let framebuffer = Framebuffer::filled(3, 2, Color::RED);
        assert_eq!(framebuffer.pixels().len(), 24);
        assert_eq!(framebuffer.pixel(2, 1), Some(Color::RED));
        assert_eq!(framebuffer.pixel(3, 1), None);
    }

    #[test]
    fn rect_is_clipped_to_framebuffer() {
        let mut framebuffer = Framebuffer::filled(4, 4, Color::BLACK);
        framebuffer.fill_rect(Rect::new(-2.0, -2.0, 4.0, 4.0), Color::WHITE);
        assert_eq!(framebuffer.pixel(0, 0), Some(Color::WHITE));
        assert_eq!(framebuffer.pixel(1, 1), Some(Color::WHITE));
        assert_eq!(framebuffer.pixel(2, 2), Some(Color::BLACK));
    }

    #[test]
    fn triangle_covers_center_only() {
        let mut framebuffer = Framebuffer::filled(10, 10, Color::BLACK);
        framebuffer.draw_triangle(
            Vec2::new(1.0, 1.0),
            Vec2::new(9.0, 1.0),
            Vec2::new(1.0, 9.0),
            Color::WHITE,
        );
        assert_eq!(framebuffer.pixel(3, 3), Some(Color::WHITE));
        assert_eq!(framebuffer.pixel(8, 8), Some(Color::BLACK));
    }

    #[test]
    fn alpha_blending_is_source_over() {
        let mut framebuffer = Framebuffer::filled(1, 1, Color::BLACK);
        framebuffer.blend_pixel(0, 0, Color::rgba(255, 255, 255, 128));
        let blended = framebuffer.pixel(0, 0).unwrap();
        assert!((127..=129).contains(&blended.r));
        assert_eq!(blended.a, 255);
    }

    #[test]
    fn transparent_source_keeps_destination() {
        let mut framebuffer = Framebuffer::filled(1, 1, Color::BLUE);
        framebuffer.blend_pixel(0, 0, Color::TRANSPARENT);
        assert_eq!(framebuffer.pixel(0, 0), Some(Color::BLUE));
    }

    #[test]
    fn circle_and_line_draw_pixels() {
        let mut framebuffer = Framebuffer::filled(32, 32, Color::BLACK);
        framebuffer.draw_circle(Vec2::new(16.0, 16.0), 8.0, Color::GREEN);
        assert_eq!(framebuffer.pixel(16, 16), Some(Color::GREEN));
        assert_eq!(framebuffer.pixel(0, 0), Some(Color::BLACK));
        framebuffer.draw_line(Vec2::new(0.0, 0.0), Vec2::new(31.0, 31.0), Color::WHITE);
        assert_eq!(framebuffer.pixel(5, 5), Some(Color::WHITE));
    }

    #[test]
    fn texture_scales_into_destination() {
        let texture = Texture::solid(2, 2, Color::RED);
        let mut framebuffer = Framebuffer::filled(8, 8, Color::BLACK);
        framebuffer.draw_texture(&texture, Rect::new(2.0, 2.0, 4.0, 4.0), Color::WHITE);
        assert_eq!(framebuffer.pixel(3, 3), Some(Color::RED));
        assert_eq!(framebuffer.pixel(0, 0), Some(Color::BLACK));
    }

    #[test]
    fn resize_preserves_dimensions_and_clears() {
        let mut framebuffer = Framebuffer::filled(2, 2, Color::WHITE);
        framebuffer.resize(4, 3);
        assert_eq!((framebuffer.width(), framebuffer.height()), (4, 3));
        assert_eq!(framebuffer.pixels().len(), 48);
        assert_eq!(framebuffer.pixel(0, 0), Some(Color::TRANSPARENT));
    }

    #[test]
    fn checksum_changes_with_content() {
        let a = Framebuffer::filled(2, 2, Color::RED);
        let b = Framebuffer::filled(2, 2, Color::BLUE);
        assert_ne!(a.checksum(), b.checksum());
    }

    #[test]
    fn ppm_header_and_size_are_valid() {
        let framebuffer = Framebuffer::filled(2, 1, Color::WHITE);
        let mut out = Vec::new();
        framebuffer.write_ppm(&mut out).unwrap();
        assert!(out.starts_with(b"P6\n2 1\n255\n"));
        assert_eq!(out.len(), 11 + 6);
    }
}
