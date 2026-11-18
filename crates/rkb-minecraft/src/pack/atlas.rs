//! Texture store and dynamic atlas for user-supplied resource packs.
//!
//! Decoded pack textures are kept by path with a stable numeric id, and the
//! atlas is rebuilt whenever a new texture is added. Procedural tiles from the
//! built-in atlas occupy ids `0..TILE_COUNT` at the top-left of the composed
//! image, so built-in fallback faces keep working while a pack is loaded.
//! Nothing here is a Mojang asset: every image comes from the pack the user
//! supplied, and the missing-texture tile is drawn by this code.

use std::collections::HashMap;
use std::sync::Arc;

use rkb_render::Texture;

use crate::textures::{
    BlockAtlas, ATLAS_COLUMNS, ATLAS_HEIGHT, ATLAS_WIDTH, TILE_COUNT, TILE_SIZE,
};

use super::png::Image;

/// One decoded texture with its stable id.
///
/// Ids are global: procedural tiles own `0..TILE_COUNT` in the atlas, and
/// pack textures own `TILE_COUNT..`. The store only knows the pack part.
pub struct TextureStore {
    images: Vec<Image>,
    ids: HashMap<String, u32>,
    alphas: Vec<bool>,
    generation: u64,
}

impl Default for TextureStore {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for TextureStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextureStore")
            .field("images", &self.images.len())
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl TextureStore {
    /// A store whose first entry is this project's missing-texture tile.
    ///
    /// Pack ids start after the procedural tiles so both live in one atlas.
    #[must_use]
    pub fn new() -> Self {
        let missing = Image::solid(TILE_SIZE, TILE_SIZE, [255, 0, 255, 255]);
        let mut ids = HashMap::new();
        ids.insert(MISSING_KEY.to_owned(), TILE_COUNT as u32);
        Self {
            images: vec![missing],
            ids,
            alphas: vec![false],
            generation: 1,
        }
    }

    /// The synthetic missing-texture tile id.
    #[must_use]
    pub fn missing_id(&self) -> u32 {
        TILE_COUNT as u32
    }

    /// Number of decoded pack textures (including the missing tile).
    #[must_use]
    pub fn len(&self) -> usize {
        self.images.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    /// Incremented whenever the texture set changes.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Id for `key`, decoding it through `loader` the first time it is seen.
    ///
    /// A failed or absent image maps to the missing-texture tile without
    /// allocating a new slot, so one bad file never breaks the whole pack and
    /// never duplicates the fallback tile.
    pub fn get_or_insert(&mut self, key: &str, loader: impl FnOnce() -> Option<Image>) -> u32 {
        if let Some(&id) = self.ids.get(key) {
            return id;
        }
        let id = match loader() {
            Some(image) => {
                let id = (TILE_COUNT as u32).saturating_add(self.images.len() as u32);
                self.alphas.push(image_has_alpha(&image));
                self.images.push(image);
                self.ids.insert(key.to_owned(), id);
                self.generation = self.generation.wrapping_add(1);
                id
            }
            None => self.missing_id(),
        };
        self.ids.insert(key.to_owned(), id);
        id
    }

    #[must_use]
    pub fn image(&self, id: u32) -> Option<&Image> {
        let slot = id.checked_sub(TILE_COUNT as u32)? as usize;
        self.images.get(slot)
    }

    /// Whether any texel of this texture is not fully opaque.
    #[must_use]
    pub fn has_alpha(&self, id: u32) -> bool {
        let slot = id
            .checked_sub(TILE_COUNT as u32)
            .map_or(usize::MAX, |value| value as usize);
        self.alphas.get(slot).copied().unwrap_or(false)
    }

    /// Pack texture ids in insertion order.
    pub fn ids(&self) -> impl Iterator<Item = u32> {
        let count = self.images.len();
        (0..count).map(|slot| (TILE_COUNT as u32).saturating_add(slot as u32))
    }
}

/// Key used for the missing-texture tile inside the store.
pub const MISSING_KEY: &str = "__rkb_missing_texture__";

fn image_has_alpha(image: &Image) -> bool {
    image.rgba.chunks_exact(4).any(|pixel| pixel[3] != 255)
}

/// A composed atlas: procedural tiles plus every decoded pack texture.
#[derive(Clone, Debug)]
pub struct Atlas {
    texture: Arc<Texture>,
    rects: Vec<[f32; 4]>,
    generation: u64,
}

impl Default for Atlas {
    fn default() -> Self {
        Self::build(&TextureStore::new())
    }
}

impl Atlas {
    /// Builds an atlas from the current texture set.
    #[must_use]
    pub fn build(store: &TextureStore) -> Self {
        let base = BlockAtlas::new();
        let base_width = ATLAS_WIDTH;
        let base_height = ATLAS_HEIGHT;

        // Shelf-pack the pack textures under the procedural region.
        let mut pack_width = base_width;
        for id in store.ids() {
            if let Some(image) = store.image(id) {
                pack_width = pack_width.max(image.width.saturating_add(2));
            }
        }
        pack_width = pack_width.max(512).max(base_width);
        let width = pack_width.max(base_width);
        let mut height = base_height;
        let mut x = 1_u32;
        let mut y = base_height + 1;
        let mut row_height = 0_u32;
        let mut placements: Vec<[u32; 4]> = Vec::with_capacity(TILE_COUNT + store.len());
        // Ids below TILE_COUNT are procedural tiles placed at the top-left.
        for index in 0..TILE_COUNT {
            let column = (index as u32) % ATLAS_COLUMNS;
            let row = (index as u32) / ATLAS_COLUMNS;
            let x0 = column * TILE_SIZE;
            let y0 = row * TILE_SIZE;
            placements.push([x0, y0, x0 + TILE_SIZE, y0 + TILE_SIZE]);
        }
        for id in store.ids() {
            let Some(image) = store.image(id) else {
                placements.push([0, 0, TILE_SIZE, TILE_SIZE]);
                continue;
            };
            let w = image.width;
            let h = image.height;
            if x + w + 1 > width {
                x = 1;
                y += row_height + 1;
                row_height = 0;
            }
            placements.push([x, y, x + w, y + h]);
            x += w + 1;
            row_height = row_height.max(h);
        }
        if row_height > 0 {
            height = height.max(y + row_height + 1);
        }
        height = height.max(base_height + 1);

        let mut pixels = vec![0_u8; (width * height * 4) as usize];
        // Procedural region is copied verbatim, so its tiles keep their
        // original 16x16 texels.
        let base_texture = base.texture();
        let base_pixels = base_texture.pixels();
        for row in 0..base_height {
            let src = (row * base_width * 4) as usize;
            let dst = (row * width * 4) as usize;
            pixels[dst..dst + (base_width * 4) as usize]
                .copy_from_slice(&base_pixels[src..src + (base_width * 4) as usize]);
        }
        for (slot, id) in store.ids().enumerate() {
            let Some(image) = store.image(id) else {
                continue;
            };
            let [x0, y0, x1, _y1] = placements[TILE_COUNT + slot];
            for row in 0..image.height {
                let src = (row * image.width * 4) as usize;
                let dst = (((y0 + row) * width + x0) * 4) as usize;
                let len = ((x1 - x0) * 4) as usize;
                pixels[dst..dst + len].copy_from_slice(&image.rgba[src..src + len]);
            }
        }

        let half = 0.5;
        let rects = placements
            .iter()
            .map(|&[x0, y0, x1, y1]| {
                [
                    (x0 as f32 + half) / width as f32,
                    (y0 as f32 + half) / height as f32,
                    (x1 as f32 - half) / width as f32,
                    (y1 as f32 - half) / height as f32,
                ]
            })
            .collect();

        Self {
            texture: Arc::new(Texture::from_rgba(width, height, pixels)),
            rects,
            generation: store.generation(),
        }
    }

    /// Rebuilds only when the texture set changed.
    pub fn rebuild(&mut self, store: &TextureStore) -> bool {
        if store.generation() == self.generation {
            return false;
        }
        *self = Self::build(store);
        true
    }

    /// The atlas texture, shared with the renderer's draw list.
    #[must_use]
    pub fn texture(&self) -> Arc<Texture> {
        Arc::clone(&self.texture)
    }

    /// `[u0, v0, u1, v1]` for one texture id.
    #[must_use]
    pub fn uv_id(&self, id: u32) -> [f32; 4] {
        self.rects
            .get(id as usize)
            .copied()
            .unwrap_or(self.rects[0])
    }

    /// Texture set version this atlas was built from.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::textures::Tile;

    fn checker() -> Image {
        let mut image = Image::solid(16, 16, [200, 40, 40, 255]);
        for (index, pixel) in image.rgba.chunks_exact_mut(4).enumerate() {
            if index % 2 == 0 {
                pixel[0] = 20;
            }
        }
        image
    }

    #[test]
    fn store_assigns_stable_ids_and_reports_alpha() {
        let mut store = TextureStore::new();
        let missing = store.missing_id();
        assert!(missing >= TILE_COUNT as u32);
        let first = store.get_or_insert("block/stone", || Some(checker()));
        let second = store.get_or_insert("block/stone", || Some(checker()));
        assert_eq!(first, second);
        assert_ne!(first, missing);
        let transparent = store.get_or_insert("block/glass", || {
            Some(Image::solid(16, 16, [255, 255, 255, 10]))
        });
        assert!(store.has_alpha(transparent));
        assert!(!store.has_alpha(first));
        assert_eq!(store.len(), 3);

        // A failed decode maps to the missing tile and does not grow ids.
        let failed = store.get_or_insert("block/broken", || None);
        assert_eq!(failed, store.missing_id());
        assert_eq!(store.ids().count(), 3);
    }

    #[test]
    fn atlas_keeps_procedural_tiles_and_appends_pack_textures() {
        let mut store = TextureStore::new();
        let id = store.get_or_insert("block/stone", || Some(checker()));
        let mut atlas = Atlas::build(&store);
        assert!(atlas.texture().width() >= ATLAS_WIDTH);
        assert!(atlas.texture().height() > ATLAS_HEIGHT);

        // A procedural tile keeps its texel position in the top-left region.
        // The composed atlas is wider than the procedural one, so raw UVs
        // differ; the texel coordinate must not.
        let stone = BlockAtlas::new().uv(Tile::Stone);
        let uv = atlas.uv_id(Tile::Stone.index() as u32);
        let width = atlas.texture().width() as f32;
        let height = atlas.texture().height() as f32;
        let expected = [
            stone[0] * ATLAS_WIDTH as f32,
            stone[1] * ATLAS_HEIGHT as f32,
        ];
        let actual = [uv[0] * width, uv[1] * height];
        assert!(
            (actual[0] - expected[0]).abs() < 1.0 && (actual[1] - expected[1]).abs() < 1.0,
            "{uv:?} -> {actual:?} vs {stone:?} -> {expected:?}"
        );

        // The pack texture lands below the procedural region.
        let pack_uv = atlas.uv_id(id);
        assert!(pack_uv[1] > 0.5, "{pack_uv:?}");
        assert_eq!(atlas.uv_id(9999), atlas.uv_id(0));

        // Rebuilding is a no-op until the set changes.
        assert!(!atlas.rebuild(&store));
        store.get_or_insert("block/bedrock", || Some(checker()));
        assert!(atlas.rebuild(&store));
        assert_eq!(atlas.generation(), store.generation());
    }
}
