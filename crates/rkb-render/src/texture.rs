//! Tightly packed RGBA8 texture storage.

use crate::Color;

/// An RGBA8 image used for sprites and UI elements.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texture {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Texture {
    /// Creates a texture from tightly packed RGBA8 pixels.
    ///
    /// # Panics
    ///
    /// Panics if the dimensions are zero or the buffer length does not match.
    #[must_use]
    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        assert!(
            width > 0 && height > 0,
            "texture dimensions must be non-zero"
        );
        assert_eq!(
            pixels.len(),
            width as usize * height as usize * 4,
            "texture buffer length does not match dimensions"
        );
        Self {
            width,
            height,
            pixels,
        }
    }

    #[must_use]
    pub fn solid(width: u32, height: u32, color: Color) -> Self {
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for _ in 0..width * height {
            pixels.extend_from_slice(&color.to_array());
        }
        Self::from_rgba(width, height, pixels)
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

    /// Samples with nearest-neighbour filtering using normalized coordinates.
    #[must_use]
    pub fn sample_nearest(&self, u: f32, v: f32) -> Color {
        let x = (u.clamp(0.0, 0.999_999) * self.width as f32) as u32;
        let y = (v.clamp(0.0, 0.999_999) * self.height as f32) as u32;
        let offset = (y as usize * self.width as usize + x as usize) * 4;
        Color::from([
            self.pixels[offset],
            self.pixels[offset + 1],
            self.pixels[offset + 2],
            self.pixels[offset + 3],
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_texture_samples_correctly() {
        let texture = Texture::solid(4, 2, Color::BLUE);
        assert_eq!(texture.width(), 4);
        assert_eq!(texture.height(), 2);
        assert_eq!(texture.pixels().len(), 32);
        assert_eq!(texture.sample_nearest(0.9, 0.9), Color::BLUE);
    }
}
