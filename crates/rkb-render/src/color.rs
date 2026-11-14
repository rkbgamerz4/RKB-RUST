//! Straight-alpha RGBA colors and hex parsing.

use std::fmt;

/// An RGBA8 color with straight (non-premultiplied) alpha.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    pub const WHITE: Self = Self::rgb(255, 255, 255);
    pub const RED: Self = Self::rgb(225, 64, 72);
    pub const GREEN: Self = Self::rgb(70, 190, 120);
    pub const BLUE: Self = Self::rgb(70, 130, 230);
    pub const YELLOW: Self = Self::rgb(245, 196, 75);

    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    #[must_use]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    #[must_use]
    pub const fn to_array(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }

    #[must_use]
    pub const fn with_alpha(self, alpha: u8) -> Self {
        Self { a: alpha, ..self }
    }

    #[must_use]
    pub const fn is_transparent(self) -> bool {
        self.a == 0
    }

    /// Parses `#RRGGBB`, `#RRGGBBAA`, `RRGGBB`, or `RRGGBBAA`.
    pub fn from_hex(value: &str) -> Result<Self, ColorError> {
        let hex = value.strip_prefix('#').unwrap_or(value);
        if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ColorError(value.to_owned()));
        }
        let component =
            |start: usize| -> u8 { u8::from_str_radix(&hex[start..start + 2], 16).unwrap_or(0) };
        Ok(Self::rgba(
            component(0),
            component(2),
            component(4),
            if hex.len() == 8 { component(6) } else { 255 },
        ))
    }

    #[must_use]
    pub fn lerp(self, rhs: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| -> u8 {
            let value = f32::from(a) + (f32::from(b) - f32::from(a)) * t;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let rounded = value.round().clamp(0.0, 255.0) as u8;
            rounded
        };
        Self::rgba(
            mix(self.r, rhs.r),
            mix(self.g, rhs.g),
            mix(self.b, rhs.b),
            mix(self.a, rhs.a),
        )
    }

    /// Multiplies all components by a shader factor in `0.0..=1.0`.
    #[must_use]
    pub fn shaded(self, factor: f32) -> Self {
        self.lerp(Self::rgba(0, 0, 0, self.a), 1.0 - factor.clamp(0.0, 1.0))
    }
}

impl From<[u8; 4]> for Color {
    fn from(value: [u8; 4]) -> Self {
        Self::rgba(value[0], value[1], value[2], value[3])
    }
}

/// Invalid hexadecimal color.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColorError(pub String);

impl fmt::Display for ColorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid color '{}': expected RRGGBB or RRGGBBAA", self.0)
    }
}

impl std::error::Error for ColorError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rgb_and_rgba_hex() {
        assert_eq!(Color::from_hex("#ff8000").unwrap(), Color::rgb(255, 128, 0));
        assert_eq!(
            Color::from_hex("10203040").unwrap(),
            Color::rgba(16, 32, 48, 64)
        );
        assert!(Color::from_hex("#gg0000").is_err());
        assert!(Color::from_hex("#fff").is_err());
    }

    #[test]
    fn lerp_moves_between_colors() {
        assert_eq!(
            Color::BLACK.lerp(Color::WHITE, 0.5),
            Color::rgb(128, 128, 128)
        );
        assert_eq!(Color::BLACK.lerp(Color::WHITE, 0.0), Color::BLACK);
    }
}
