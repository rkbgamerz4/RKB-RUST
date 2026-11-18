//! Minimal PNG decoder for user-supplied resource packs.
//!
//! Only what a Minecraft texture needs: 8-bit, non-interlaced images with
//! colour types 0 (grey), 2 (RGB), 3 (palette), 4 (grey+alpha), and 6 (RGBA).
//! 16-bit, interlaced, and low-bit-depth palettes are rejected with an explicit
//! error instead of being guessed. The decoder skips CRCs and non-image chunks;
//! it is a resource reader, not a general-purpose PNG library.
//!
//! No image is bundled: callers pass bytes from a pack the user supplied.

use miniz_oxide::inflate::decompress_to_vec_zlib;

/// A decoded RGBA8 image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8, row-major from the top-left.
    pub rgba: Vec<u8>,
}

impl Image {
    /// A solid-colour image, used for the missing-texture tile.
    #[must_use]
    pub fn solid(width: u32, height: u32, color: [u8; 4]) -> Self {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            rgba.extend_from_slice(&color);
        }
        Self {
            width,
            height,
            rgba,
        }
    }

    /// Nearest-neighbour sample in `0..1` texture coordinates.
    #[must_use]
    pub fn sample(&self, u: f32, v: f32) -> [u8; 4] {
        if self.width == 0 || self.height == 0 {
            return [0, 0, 0, 0];
        }
        let x = ((u * self.width as f32) as u32).min(self.width - 1);
        let y = ((v * self.height as f32) as u32).min(self.height - 1);
        let offset = ((y * self.width + x) * 4) as usize;
        [
            self.rgba[offset],
            self.rgba[offset + 1],
            self.rgba[offset + 2],
            self.rgba[offset + 3],
        ]
    }
}

/// A PNG that this decoder does not support.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PngError(pub String);

impl std::fmt::Display for PngError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "png: {}", self.0)
    }
}

impl std::error::Error for PngError {}

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColourType {
    Grey,
    Rgb,
    Palette,
    GreyAlpha,
    Rgba,
}

impl ColourType {
    fn channels(self) -> usize {
        match self {
            Self::Grey => 1,
            Self::Rgb => 3,
            Self::Palette => 1,
            Self::GreyAlpha => 2,
            Self::Rgba => 4,
        }
    }
}

/// Decodes one PNG file into RGBA8.
pub fn decode(bytes: &[u8]) -> Result<Image, PngError> {
    if bytes.len() < 8 || bytes[..8] != SIGNATURE {
        return Err(PngError("missing PNG signature".to_owned()));
    }
    let mut offset = 8_usize;
    let mut width = 0_u32;
    let mut height = 0_u32;
    let mut colour = None;
    let mut palette = Vec::new();
    let mut transparency = Vec::new();
    let mut idat = Vec::new();
    let mut seen_header = false;

    while offset + 8 <= bytes.len() {
        let length = u32::from_be_bytes(
            bytes[offset..offset + 4]
                .try_into()
                .expect("four length bytes"),
        ) as usize;
        let kind = &bytes[offset + 4..offset + 8];
        offset += 8;
        if offset + length + 4 > bytes.len() {
            return Err(PngError("chunk extends past the file".to_owned()));
        }
        let data = &bytes[offset..offset + length];
        offset += length + 4; // payload and CRC
        match kind {
            b"IHDR" => {
                if length != 13 {
                    return Err(PngError("IHDR must be 13 bytes".to_owned()));
                }
                width = u32::from_be_bytes(data[0..4].try_into().expect("width"));
                height = u32::from_be_bytes(data[4..8].try_into().expect("height"));
                let depth = data[8];
                if depth != 8 {
                    return Err(PngError(format!(
                        "unsupported bit depth {depth}; only 8-bit textures are decoded"
                    )));
                }
                colour = Some(match data[9] {
                    0 => ColourType::Grey,
                    2 => ColourType::Rgb,
                    3 => ColourType::Palette,
                    4 => ColourType::GreyAlpha,
                    6 => ColourType::Rgba,
                    other => return Err(PngError(format!("unsupported colour type {other}"))),
                });
                if data[10] != 0 || data[11] != 0 {
                    return Err(PngError(
                        "unsupported compression or filter method".to_owned(),
                    ));
                }
                if data[12] != 0 {
                    return Err(PngError(
                        "interlaced PNGs are not decoded; re-save the texture as non-interlaced"
                            .to_owned(),
                    ));
                }
                seen_header = true;
            }
            b"PLTE" => {
                if length % 3 != 0 {
                    return Err(PngError("PLTE length is not a multiple of 3".to_owned()));
                }
                palette = data.to_vec();
            }
            b"tRNS" => transparency = data.to_vec(),
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
    }

    if !seen_header {
        return Err(PngError("missing IHDR".to_owned()));
    }
    if width == 0 || height == 0 {
        return Err(PngError("zero image dimensions".to_owned()));
    }
    let colour = colour.expect("IHDR sets the colour type");
    if colour == ColourType::Palette && palette.is_empty() {
        return Err(PngError("palette image has no PLTE chunk".to_owned()));
    }
    if idat.is_empty() {
        return Err(PngError("image has no IDAT data".to_owned()));
    }
    let raw = decompress_to_vec_zlib(&idat)
        .map_err(|error| PngError(format!("zlib inflate failed: {error:?}")))?;

    let channels = colour.channels();
    let stride = width as usize * channels;
    let expected = height as usize * (stride + 1);
    if raw.len() < expected {
        return Err(PngError(format!(
            "inflated data is {} bytes, expected {expected}",
            raw.len()
        )));
    }

    let mut previous = vec![0_u8; stride];
    let mut current = vec![0_u8; stride];
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height as usize {
        let start = row * (stride + 1);
        let filter = raw[start];
        current.copy_from_slice(&raw[start + 1..start + 1 + stride]);
        unfilter(filter, channels, &previous, &mut current)?;
        expand_row(colour, &current, &palette, &transparency, &mut rgba)?;
        std::mem::swap(&mut previous, &mut current);
    }

    Ok(Image {
        width,
        height,
        rgba,
    })
}

fn unfilter(
    filter: u8,
    channels: usize,
    previous: &[u8],
    current: &mut [u8],
) -> Result<(), PngError> {
    let bpp = channels;
    match filter {
        0 => {}
        1 => {
            for index in bpp..current.len() {
                current[index] = current[index].wrapping_add(current[index - bpp]);
            }
        }
        2 => {
            for (index, value) in current.iter_mut().enumerate() {
                *value = value.wrapping_add(previous[index]);
            }
        }
        3 => {
            for index in 0..current.len() {
                let left = if index >= bpp {
                    u16::from(current[index - bpp])
                } else {
                    0
                };
                let up = u16::from(previous[index]);
                current[index] = current[index].wrapping_add(((left + up) / 2) as u8);
            }
        }
        4 => {
            for index in 0..current.len() {
                let a = if index >= bpp {
                    i32::from(current[index - bpp])
                } else {
                    0
                };
                let b = i32::from(previous[index]);
                let c = if index >= bpp {
                    i32::from(previous[index - bpp])
                } else {
                    0
                };
                let p = a + b - c;
                let pa = (p - a).abs();
                let pb = (p - b).abs();
                let pc = (p - c).abs();
                let predictor = if pa <= pb && pa <= pc {
                    a
                } else if pb <= pc {
                    b
                } else {
                    c
                };
                current[index] = current[index].wrapping_add(predictor as u8);
            }
        }
        other => return Err(PngError(format!("unknown row filter {other}"))),
    }
    Ok(())
}

fn expand_row(
    colour: ColourType,
    row: &[u8],
    palette: &[u8],
    transparency: &[u8],
    out: &mut Vec<u8>,
) -> Result<(), PngError> {
    match colour {
        ColourType::Grey => {
            for &value in row {
                out.extend_from_slice(&[value, value, value, 255]);
            }
        }
        ColourType::Rgb => {
            for pixel in row.chunks_exact(3) {
                out.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
        }
        ColourType::Palette => {
            for &index in row {
                let index = index as usize;
                let base = index * 3;
                if base + 2 >= palette.len() {
                    return Err(PngError(format!(
                        "palette index {index} is outside the PLTE chunk"
                    )));
                }
                let alpha = transparency.get(index).copied().unwrap_or(255);
                out.extend_from_slice(&[
                    palette[base],
                    palette[base + 1],
                    palette[base + 2],
                    alpha,
                ]);
            }
        }
        ColourType::GreyAlpha => {
            for pixel in row.chunks_exact(2) {
                out.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        ColourType::Rgba => out.extend_from_slice(row),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a PNG with stored (uncompressed) zlib blocks so the fixture does
    /// not depend on an encoder. This is authored test data, not game art.
    fn png(
        width: u32,
        height: u32,
        colour_type: u8,
        scanlines: &[Vec<u8>],
        palette: &[u8],
    ) -> Vec<u8> {
        let mut out = SIGNATURE.to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, colour_type, 0, 0, 0]);
        push_chunk(&mut out, b"IHDR", &ihdr);
        if !palette.is_empty() {
            push_chunk(&mut out, b"PLTE", palette);
        }
        let raw: Vec<u8> = scanlines.iter().flatten().copied().collect();
        push_chunk(&mut out, b"IDAT", &zlib_stored(&raw));
        push_chunk(&mut out, b"IEND", &[]);
        out
    }

    fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&[0, 0, 0, 0]); // CRC is intentionally not checked
    }

    fn adler32(data: &[u8]) -> u32 {
        let mut a = 1_u32;
        let mut b = 0_u32;
        for &byte in data {
            a = (a + u32::from(byte)) % 65_521;
            b = (b + a) % 65_521;
        }
        (b << 16) | a
    }

    fn zlib_stored(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        let mut remaining = data;
        while !remaining.is_empty() {
            let len = remaining.len().min(65_535);
            let last = len == remaining.len();
            out.push(u8::from(last));
            out.extend_from_slice(&(len as u16).to_le_bytes());
            out.extend_from_slice(&(!(len as u16)).to_le_bytes());
            out.extend_from_slice(&remaining[..len]);
            remaining = &remaining[len..];
        }
        out.extend_from_slice(&adler32(data).to_be_bytes());
        out
    }

    #[test]
    fn decodes_rgba_and_rgb_with_filters() {
        let rgba = png(1, 1, 6, &[vec![0, 10, 20, 30, 40]], &[]);
        let image = decode(&rgba).unwrap();
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(image.rgba, vec![10, 20, 30, 40]);

        // Two RGB pixels with a Sub filter: the second adds the first bytewise.
        let rgb = png(2, 1, 2, &[vec![1, 5, 10, 15, 1, 1, 1]], &[]);
        let image = decode(&rgb).unwrap();
        assert_eq!(image.rgba, vec![5, 10, 15, 255, 6, 11, 16, 255]);
    }

    #[test]
    fn decodes_palette_and_grey_alpha() {
        let palette = vec![255, 0, 0, 0, 255, 0];
        let image = decode(&png(2, 1, 3, &[vec![0, 0, 1]], &palette)).unwrap();
        assert_eq!(image.rgba, vec![255, 0, 0, 255, 0, 255, 0, 255]);

        let image = decode(&png(1, 1, 4, &[vec![0, 128, 64]], &[])).unwrap();
        assert_eq!(image.rgba, vec![128, 128, 128, 64]);
    }

    #[test]
    fn rejects_unsupported_and_truncated_data() {
        assert!(decode(b"not a png").is_err());
        let mut image = png(1, 1, 6, &[vec![0, 0, 0, 0, 255]], &[]);
        image.truncate(image.len() / 2);
        assert!(decode(&image).is_err());
        // Interlaced flag.
        let mut interlaced = png(1, 1, 6, &[vec![0, 0, 0, 0, 255]], &[]);
        interlaced[8 + 8 + 12] = 1;
        assert!(decode(&interlaced).is_err());
    }
}
