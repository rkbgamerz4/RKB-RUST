//! Per-section sky and block light decoded from protocol 767 `LightData`.
//!
//! The protocol stores each section as a 2048-byte nibble array. Mask bit 0
//! refers to the section one below the dimension's minimum section, so a bit
//! index maps to absolute section Y as `min_section_y + bit - 1`. Arrays are
//! sent in ascending mask-bit order for the bits that are set; bits in the
//! "empty" masks mean the section is all zero and carries no array.
//!
//! A missing section is not invented. [`ChunkLight::sample`] returns the
//! documented full-sky fallback for a section the server did not describe, and
//! callers can see [`ChunkLight::has_data`] to report that fallback.

use std::collections::BTreeMap;

use rkb_proto::packets::play::LightData;

/// One section's nibble arrays.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SectionLight {
    sky: Vec<u8>,
    block: Vec<u8>,
}

impl SectionLight {
    fn set_sky(&mut self, array: &[u8]) {
        self.sky.clear();
        self.sky.extend_from_slice(array);
    }

    fn set_block(&mut self, array: &[u8]) {
        self.block.clear();
        self.block.extend_from_slice(array);
    }

    fn sample(&self, x: usize, y: usize, z: usize, sky: bool) -> Option<u8> {
        let array = if sky { &self.sky } else { &self.block };
        let index = (y << 8) | (z << 4) | x;
        let byte = *array.get(index >> 1)?;
        Some(if index & 1 == 0 {
            byte & 0x0F
        } else {
            (byte >> 4) & 0x0F
        })
    }
}

/// Sky/block light for one chunk column, keyed by absolute section Y.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChunkLight {
    sections: BTreeMap<i32, SectionLight>,
    has_data: bool,
}

impl ChunkLight {
    /// Builds light from a chunk or update-light packet.
    #[must_use]
    pub fn decode(data: &LightData, min_section_y: i32) -> Self {
        let mut light = Self::default();
        light.apply(data, min_section_y);
        light
    }

    /// Merges one packet. Sections named by either the data or empty masks are
    /// replaced; sections outside both masks keep their previous value.
    pub fn apply(&mut self, data: &LightData, min_section_y: i32) {
        self.apply_sky(data, min_section_y);
        self.apply_block(data, min_section_y);
    }

    fn apply_sky(&mut self, data: &LightData, min_section_y: i32) {
        let mut array_index = 0_usize;
        for bit in 0..mask_bits(&data.sky_light_mask) {
            if !data.sky_light_mask.get(bit) {
                continue;
            }
            let Some(array) = data.sky_light.get(array_index) else {
                break;
            };
            let section_y = min_section_y + bit as i32 - 1;
            self.sections.entry(section_y).or_default().set_sky(array);
            self.has_data = true;
            array_index += 1;
        }
        for bit in 0..mask_bits(&data.empty_sky_light_mask) {
            if !data.empty_sky_light_mask.get(bit) {
                continue;
            }
            let section_y = min_section_y + bit as i32 - 1;
            let section = self.sections.entry(section_y).or_default();
            section.sky.clear();
            section.sky.resize(2048, 0);
            self.has_data = true;
        }
    }

    fn apply_block(&mut self, data: &LightData, min_section_y: i32) {
        let mut array_index = 0_usize;
        for bit in 0..mask_bits(&data.block_light_mask) {
            if !data.block_light_mask.get(bit) {
                continue;
            }
            let Some(array) = data.block_light.get(array_index) else {
                break;
            };
            let section_y = min_section_y + bit as i32 - 1;
            self.sections.entry(section_y).or_default().set_block(array);
            self.has_data = true;
            array_index += 1;
        }
        for bit in 0..mask_bits(&data.empty_block_light_mask) {
            if !data.empty_block_light_mask.get(bit) {
                continue;
            }
            let section_y = min_section_y + bit as i32 - 1;
            let section = self.sections.entry(section_y).or_default();
            section.block.clear();
            section.block.resize(2048, 0);
            self.has_data = true;
        }
    }

    /// Whether any light array was decoded for this column. When false the
    /// renderer is using the full-sky fallback and must say so.
    #[must_use]
    pub const fn has_data(&self) -> bool {
        self.has_data
    }

    /// Sky/block light at absolute block coordinates.
    ///
    /// The fallback for a section the server did not describe is full sky and
    /// no block light. This is the same fallback as a world with no light data,
    /// and it is reported through [`Self::has_data`] at the column level.
    #[must_use]
    pub fn sample(&self, x: i32, y: i32, z: i32) -> (u8, u8) {
        let section_y = y.div_euclid(16);
        let Some(section) = self.sections.get(&section_y) else {
            return (15, 0);
        };
        let x = x.rem_euclid(16) as usize;
        let y = y.rem_euclid(16) as usize;
        let z = z.rem_euclid(16) as usize;
        let sky = section.sample(x, y, z, true).unwrap_or(15);
        let block = section.sample(x, y, z, false).unwrap_or(0);
        (sky, block)
    }
}

fn mask_bits(mask: &rkb_proto::packets::play::BitSet) -> usize {
    mask.words().len() * 64
}

#[cfg(test)]
mod tests {
    use super::*;
    use rkb_proto::packets::play::BitSet;

    fn nibble_array(fill: u8) -> Vec<u8> {
        vec![fill | (fill << 4); 2048]
    }

    #[test]
    fn mask_bit_zero_is_the_section_below_the_world() {
        // Bit 1 is the minimum section. Put sky 7 there.
        let data = LightData {
            sky_light_mask: BitSet::from_words(vec![0b10]),
            block_light_mask: BitSet::from_words(vec![0b10]),
            empty_sky_light_mask: BitSet::default(),
            empty_block_light_mask: BitSet::default(),
            sky_light: vec![nibble_array(7)],
            block_light: vec![nibble_array(3)],
        };
        let light = ChunkLight::decode(&data, -4);
        assert!(light.has_data());
        // min_section_y = -4, bit 1 -> section -4, world y -64.
        assert_eq!(light.sample(0, -64, 0), (7, 3));
        assert_eq!(light.sample(15, -49, 15), (7, 3));
        // A section the packet did not describe uses the explicit fallback.
        assert_eq!(light.sample(0, 0, 0), (15, 0));
    }

    #[test]
    fn empty_masks_clear_light_for_that_section() {
        let mut light = ChunkLight::default();
        let data = LightData {
            sky_light_mask: BitSet::from_words(vec![0b10]),
            block_light_mask: BitSet::default(),
            empty_sky_light_mask: BitSet::default(),
            empty_block_light_mask: BitSet::default(),
            sky_light: vec![nibble_array(9)],
            block_light: Vec::new(),
        };
        light.apply(&data, 0);
        assert_eq!(light.sample(1, 1, 1).0, 9);

        let empty = LightData {
            sky_light_mask: BitSet::default(),
            block_light_mask: BitSet::default(),
            empty_sky_light_mask: BitSet::from_words(vec![0b10]),
            empty_block_light_mask: BitSet::default(),
            sky_light: Vec::new(),
            block_light: Vec::new(),
        };
        light.apply(&empty, 0);
        assert_eq!(light.sample(1, 1, 1).0, 0);
    }

    #[test]
    fn nibble_order_is_low_for_even_indices() {
        let mut section = SectionLight::default();
        section.set_sky(&[0x21]);
        assert_eq!(section.sample(0, 0, 0, true), Some(1));
        assert_eq!(section.sample(1, 0, 0, true), Some(2));
        assert_eq!(section.sample(2, 0, 0, true), None);
    }

    #[test]
    fn partial_updates_replace_only_named_sections() {
        let mut light = ChunkLight::decode(
            &LightData {
                sky_light_mask: BitSet::from_words(vec![0b11]),
                block_light_mask: BitSet::default(),
                empty_sky_light_mask: BitSet::default(),
                empty_block_light_mask: BitSet::default(),
                sky_light: vec![nibble_array(4), nibble_array(5)],
                block_light: Vec::new(),
            },
            0,
        );
        // Sections -1 (bit 0) and 0 (bit 1): world y -16 and 0.
        assert_eq!(light.sample(0, -16, 0).0, 4);
        assert_eq!(light.sample(0, 0, 0).0, 5);
        // Replace only the section at world y 0.
        light.apply(
            &LightData {
                sky_light_mask: BitSet::from_words(vec![0b10]),
                block_light_mask: BitSet::default(),
                empty_sky_light_mask: BitSet::default(),
                empty_block_light_mask: BitSet::default(),
                sky_light: vec![nibble_array(12)],
                block_light: Vec::new(),
            },
            0,
        );
        assert_eq!(light.sample(0, -16, 0).0, 4);
        assert_eq!(light.sample(0, 0, 0).0, 12);
    }
}
