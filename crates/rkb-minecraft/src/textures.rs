//! Original, procedurally generated block textures.
//!
//! No Mojang texture, atlas, or resource-pack data is bundled or copied. Every
//! texel here is produced from a deterministic hash, so the atlas is stable
//! across runs and platforms. Tiles are 16x16 and laid out in one 128x64 atlas;
//! the sampler uses nearest-neighbour filtering and half-texel inset UVs so a
//! face never bleeds into its neighbour.
//!
//! This is deliberately a "fast graphics" look: all tiles are opaque, there is
//! no resource-pack loading, and blocks still render as full cubes.

use std::sync::Arc;

use rkb_core::Vec3;
use rkb_render::{Color, Texture};

use crate::blocks::block_color;

/// Edge length of one tile in texels.
pub const TILE_SIZE: u32 = 16;
/// Atlas column count.
pub const ATLAS_COLUMNS: u32 = 8;
/// Atlas row count.
pub const ATLAS_ROWS: u32 = 4;
/// Atlas width in texels.
pub const ATLAS_WIDTH: u32 = TILE_SIZE * ATLAS_COLUMNS;
/// Atlas height in texels.
pub const ATLAS_HEIGHT: u32 = TILE_SIZE * ATLAS_ROWS;

/// Which side of a cube a face belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaceDir {
    PosX,
    NegX,
    PosY,
    NegY,
    PosZ,
    NegZ,
}

impl FaceDir {
    /// Every direction, in the order used by model elements and tests.
    pub const ALL: [Self; 6] = [
        Self::NegX,
        Self::PosX,
        Self::NegY,
        Self::PosY,
        Self::NegZ,
        Self::PosZ,
    ];

    /// Shade multiplier matching Minecraft's directional face lighting
    /// (top brightest, bottom darkest, north/south brighter than east/west).
    #[must_use]
    pub const fn shade(self) -> f32 {
        match self {
            Self::PosY => 1.0,
            Self::NegY => 0.5,
            Self::PosX | Self::NegX => 0.6,
            Self::PosZ | Self::NegZ => 0.8,
        }
    }

    /// Index into a six-face array. The order is `NegX, PosX, NegY, PosY,
    /// NegZ, PosZ`.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::NegX => 0,
            Self::PosX => 1,
            Self::NegY => 2,
            Self::PosY => 3,
            Self::NegZ => 4,
            Self::PosZ => 5,
        }
    }

    /// Outward unit normal.
    #[must_use]
    pub const fn normal(self) -> Vec3 {
        match self {
            Self::NegX => Vec3::new(-1.0, 0.0, 0.0),
            Self::PosX => Vec3::new(1.0, 0.0, 0.0),
            Self::NegY => Vec3::new(0.0, -1.0, 0.0),
            Self::PosY => Vec3::new(0.0, 1.0, 0.0),
            Self::NegZ => Vec3::new(0.0, 0.0, -1.0),
            Self::PosZ => Vec3::new(0.0, 0.0, 1.0),
        }
    }

    /// The block offset a face points into.
    #[must_use]
    pub const fn offset(self) -> (i32, i32, i32) {
        match self {
            Self::NegX => (-1, 0, 0),
            Self::PosX => (1, 0, 0),
            Self::NegY => (0, -1, 0),
            Self::PosY => (0, 1, 0),
            Self::NegZ => (0, 0, -1),
            Self::PosZ => (0, 0, 1),
        }
    }

    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::NegX => Self::PosX,
            Self::PosX => Self::NegX,
            Self::NegY => Self::PosY,
            Self::PosY => Self::NegY,
            Self::NegZ => Self::PosZ,
            Self::PosZ => Self::NegZ,
        }
    }

    /// Parses a model/blockstate face name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "down" => Some(Self::NegY),
            "up" => Some(Self::PosY),
            "north" => Some(Self::NegZ),
            "south" => Some(Self::PosZ),
            "west" => Some(Self::NegX),
            "east" => Some(Self::PosX),
            _ => None,
        }
    }
}

/// One 16x16 tile in the atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Tile {
    GrassTop = 0,
    GrassSide = 1,
    Dirt = 2,
    Stone = 3,
    Cobblestone = 4,
    Sand = 5,
    Gravel = 6,
    Snow = 7,
    Ice = 8,
    LogSide = 9,
    LogTop = 10,
    Leaves = 11,
    Planks = 12,
    Bedrock = 13,
    Water = 14,
    Lava = 15,
    CactusSide = 16,
    CactusTop = 17,
    Clay = 18,
    Obsidian = 19,
    Netherrack = 20,
    EndStone = 21,
    Wool = 22,
    Unknown = 23,
    Plant = 24,
    Flower = 25,
    Torch = 26,
    Glass = 27,
}

/// Number of tiles in the atlas.
pub const TILE_COUNT: usize = 28;

impl Tile {
    /// Every tile, in atlas order.
    pub const ALL: [Self; TILE_COUNT] = [
        Self::GrassTop,
        Self::GrassSide,
        Self::Dirt,
        Self::Stone,
        Self::Cobblestone,
        Self::Sand,
        Self::Gravel,
        Self::Snow,
        Self::Ice,
        Self::LogSide,
        Self::LogTop,
        Self::Leaves,
        Self::Planks,
        Self::Bedrock,
        Self::Water,
        Self::Lava,
        Self::CactusSide,
        Self::CactusTop,
        Self::Clay,
        Self::Obsidian,
        Self::Netherrack,
        Self::EndStone,
        Self::Wool,
        Self::Unknown,
        Self::Plant,
        Self::Flower,
        Self::Torch,
        Self::Glass,
    ];

    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// A shared procedural atlas plus its UV rectangles.
#[derive(Clone, Debug)]
pub struct BlockAtlas {
    texture: Arc<Texture>,
    rects: [[f32; 4]; TILE_COUNT],
}

impl BlockAtlas {
    /// Builds the atlas. The result depends only on the code, not on the host.
    #[must_use]
    pub fn new() -> Self {
        let mut pixels = vec![0_u8; (ATLAS_WIDTH * ATLAS_HEIGHT * 4) as usize];
        // Unused padding tiles stay opaque so the whole atlas is alpha-complete.
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        for tile in Tile::ALL {
            let column = tile.index() as u32 % ATLAS_COLUMNS;
            let row = tile.index() as u32 / ATLAS_COLUMNS;
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    let color = paint(tile, x, y);
                    let px = column * TILE_SIZE + x;
                    let py = row * TILE_SIZE + y;
                    let offset = ((py * ATLAS_WIDTH + px) * 4) as usize;
                    pixels[offset..offset + 4].copy_from_slice(&color.to_array());
                }
            }
        }

        let half_u = 0.5 / ATLAS_WIDTH as f32;
        let half_v = 0.5 / ATLAS_HEIGHT as f32;
        let mut rects = [[0.0_f32; 4]; TILE_COUNT];
        for tile in Tile::ALL {
            let column = (tile.index() as u32 % ATLAS_COLUMNS) as f32;
            let row = (tile.index() as u32 / ATLAS_COLUMNS) as f32;
            rects[tile.index()] = [
                (column * TILE_SIZE as f32) / ATLAS_WIDTH as f32 + half_u,
                (row * TILE_SIZE as f32) / ATLAS_HEIGHT as f32 + half_v,
                ((column + 1.0) * TILE_SIZE as f32) / ATLAS_WIDTH as f32 - half_u,
                ((row + 1.0) * TILE_SIZE as f32) / ATLAS_HEIGHT as f32 - half_v,
            ];
        }

        Self {
            texture: Arc::new(Texture::from_rgba(ATLAS_WIDTH, ATLAS_HEIGHT, pixels)),
            rects,
        }
    }

    /// The atlas texture, shared with the renderer's draw list.
    #[must_use]
    pub fn texture(&self) -> Arc<Texture> {
        Arc::clone(&self.texture)
    }

    /// `[u0, v0, u1, v1]` for one tile.
    #[must_use]
    pub const fn uv(&self, tile: Tile) -> [f32; 4] {
        self.rects[tile.index()]
    }

    /// `[u0, v0, u1, v1]` for a texture id. Procedural tile ids are their
    /// `Tile` index, so this is the draw-time lookup used by the mesher.
    #[must_use]
    pub fn uv_id(&self, id: u32) -> [f32; 4] {
        self.rects
            .get(id as usize)
            .copied()
            .unwrap_or(self.rects[Tile::Unknown.index()])
    }
}

impl Default for BlockAtlas {
    fn default() -> Self {
        Self::new()
    }
}

/// Picks the tile and multiply tint for a block face.
///
/// Known blocks return `Color::WHITE` as the tint because the tile already holds
/// their colour. Unknown block names reuse the neutral `Unknown` tile and keep
/// the stable hashed colour from [`crate::blocks::block_color`] as a tint.
#[must_use]
pub fn block_tile(name: &str, face: FaceDir) -> (Tile, Color) {
    let short = name.strip_prefix("minecraft:").unwrap_or(name);
    let white = Color::WHITE;
    let top = matches!(face, FaceDir::PosY);
    let bottom = matches!(face, FaceDir::NegY);
    match short {
        "grass_block" => (
            if top {
                Tile::GrassTop
            } else if bottom {
                Tile::Dirt
            } else {
                Tile::GrassSide
            },
            white,
        ),
        "short_grass" | "grass" | "fern" | "large_fern" | "dead_bush" | "seagrass"
        | "tall_grass" | "sugar_cane" => (Tile::Plant, white),
        "dandelion" | "poppy" | "blue_orchid" | "allium" | "azure_bluet" | "red_tulip"
        | "orange_tulip" | "white_tulip" | "pink_tulip" | "oxeye_daisy" | "cornflower"
        | "lily_of_the_valley" | "wither_rose" | "sunflower" | "lilac" | "rose_bush" | "peony"
        | "torchflower" => (Tile::Flower, white),
        "oak_sapling" | "spruce_sapling" | "birch_sapling" | "jungle_sapling"
        | "acacia_sapling" | "dark_oak_sapling" | "mangrove_propagule" | "azalea"
        | "flowering_azalea" => (Tile::Plant, white),
        "torch"
        | "wall_torch"
        | "soul_torch"
        | "soul_wall_torch"
        | "redstone_torch"
        | "redstone_wall_torch"
        | "lantern"
        | "soul_lantern"
        | "campfire"
        | "soul_campfire" => (Tile::Torch, white),
        "glass" | "glass_pane" | "tinted_glass" => (Tile::Glass, white),
        "dirt" | "coarse_dirt" | "rooted_dirt" => (Tile::Dirt, white),
        "podzol" | "mycelium" => (Tile::Dirt, Color::rgb(110, 84, 51)),
        "mud" | "muddy_mangrove_roots" => (Tile::Dirt, Color::rgb(96, 82, 78)),
        "stone" | "smooth_stone" | "stone_bricks" => (Tile::Stone, white),
        "mossy_stone_bricks" => (Tile::Stone, Color::rgb(150, 176, 132)),
        "andesite" => (Tile::Stone, Color::rgb(205, 205, 205)),
        "granite" | "polished_granite" => (Tile::Stone, Color::rgb(235, 170, 140)),
        "diorite" | "polished_diorite" => (Tile::Stone, Color::rgb(245, 245, 245)),
        "deepslate" | "cobbled_deepslate" | "deepslate_bricks" | "tuff" => {
            (Tile::Stone, Color::rgb(120, 120, 130))
        }
        "cobblestone" => (Tile::Cobblestone, white),
        "mossy_cobblestone" => (Tile::Cobblestone, Color::rgb(170, 200, 150)),
        "sand" | "sandstone" | "smooth_sandstone" | "cut_sandstone" => (Tile::Sand, white),
        "red_sand" | "red_sandstone" => (Tile::Sand, Color::rgb(235, 140, 80)),
        "gravel" => (Tile::Gravel, white),
        "snow" | "snow_block" | "powder_snow" => (Tile::Snow, white),
        "ice" | "packed_ice" | "blue_ice" => (Tile::Ice, white),
        "water" => (Tile::Water, white),
        "lava" => (Tile::Lava, white),
        "bedrock" => (Tile::Bedrock, white),
        "clay" => (Tile::Clay, white),
        "obsidian" => (Tile::Obsidian, white),
        "netherrack" => (Tile::Netherrack, white),
        "end_stone" => (Tile::EndStone, white),
        "white_wool" => (Tile::Wool, white),
        "oak_log" | "oak_wood" => (
            if top || bottom {
                Tile::LogTop
            } else {
                Tile::LogSide
            },
            white,
        ),
        "birch_log" | "birch_wood" => (
            if top || bottom {
                Tile::LogTop
            } else {
                Tile::LogSide
            },
            Color::rgb(250, 244, 226),
        ),
        "spruce_log" | "spruce_wood" => (
            if top || bottom {
                Tile::LogTop
            } else {
                Tile::LogSide
            },
            Color::rgb(120, 100, 78),
        ),
        "jungle_log" | "jungle_wood" => (
            if top || bottom {
                Tile::LogTop
            } else {
                Tile::LogSide
            },
            Color::rgb(190, 155, 100),
        ),
        "acacia_log" | "acacia_wood" => (
            if top || bottom {
                Tile::LogTop
            } else {
                Tile::LogSide
            },
            Color::rgb(235, 145, 90),
        ),
        "dark_oak_log" | "dark_oak_wood" => (
            if top || bottom {
                Tile::LogTop
            } else {
                Tile::LogSide
            },
            Color::rgb(95, 78, 55),
        ),
        "oak_planks" => (Tile::Planks, white),
        "birch_planks" => (Tile::Planks, Color::rgb(245, 230, 190)),
        "spruce_planks" => (Tile::Planks, Color::rgb(150, 120, 85)),
        "jungle_planks" => (Tile::Planks, Color::rgb(205, 170, 115)),
        "acacia_planks" => (Tile::Planks, Color::rgb(235, 165, 100)),
        "dark_oak_planks" => (Tile::Planks, Color::rgb(120, 95, 65)),
        "oak_leaves" | "jungle_leaves" | "acacia_leaves" => (Tile::Leaves, white),
        "birch_leaves" => (Tile::Leaves, Color::rgb(225, 240, 190)),
        "spruce_leaves" => (Tile::Leaves, Color::rgb(150, 185, 150)),
        "dark_oak_leaves" => (Tile::Leaves, Color::rgb(150, 190, 150)),
        "cactus" => (
            if top || bottom {
                Tile::CactusTop
            } else {
                Tile::CactusSide
            },
            white,
        ),
        // Everything else (modded names included) keeps a stable colour.
        _ => (Tile::Unknown, block_color(name)),
    }
}

fn clamp_channel(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

fn rgb(r: f32, g: f32, b: f32) -> Color {
    Color::rgb(clamp_channel(r), clamp_channel(g), clamp_channel(b))
}

fn hash01(seed: u32, x: u32, y: u32) -> f32 {
    let mut hash = seed
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(x.wrapping_mul(0x85EB_CA6B))
        .wrapping_add(y.wrapping_mul(0xC2B2_AE35));
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(0x7FEB_352D);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x846C_A68B);
    hash ^= hash >> 16;
    (hash & 0xFFFF) as f32 / 65_535.0
}

/// Base tile colour with deterministic per-texel brightness noise.
fn speckled(seed: u32, x: u32, y: u32, base: (u8, u8, u8), spread: f32) -> Color {
    let noise = hash01(seed, x, y);
    let delta = (noise * 2.0 - 1.0) * spread;
    rgb(
        f32::from(base.0) + delta,
        f32::from(base.1) + delta,
        f32::from(base.2) + delta,
    )
}

fn paint(tile: Tile, x: u32, y: u32) -> Color {
    let seed = 0x5EED_0000_u32.wrapping_add(tile.index() as u32 * 7919);
    match tile {
        Tile::GrassTop => {
            let color = speckled(seed, x, y, (96, 158, 64), 14.0);
            if hash01(seed ^ 0xA5, x, y) > 0.86 {
                rgb(72.0, 124.0, 48.0)
            } else {
                color
            }
        }
        Tile::GrassSide => {
            // Dirt sides with a ragged grass fringe along the top edge.
            let fringe = 2.0 + hash01(seed, x, 0) * 3.0;
            if (y as f32) < fringe {
                speckled(seed ^ 0x11, x, y, (96, 158, 64), 12.0)
            } else {
                speckled(seed ^ 0x22, x, y, (134, 96, 67), 12.0)
            }
        }
        Tile::Dirt => speckled(seed, x, y, (134, 96, 67), 12.0),
        Tile::Stone => {
            let color = speckled(seed, x, y, (128, 128, 128), 10.0);
            if hash01(seed ^ 0x33, x, y) > 0.9 {
                rgb(104.0, 104.0, 104.0)
            } else {
                color
            }
        }
        Tile::Cobblestone => {
            // 4x4 stones separated by darker mortar.
            let mortar = x % 4 == 0 || y % 4 == 0;
            if mortar {
                rgb(86.0, 86.0, 86.0)
            } else {
                speckled(seed ^ 0x44, x / 4, y / 4, (150, 150, 150), 16.0)
            }
        }
        Tile::Sand => speckled(seed, x, y, (219, 207, 163), 9.0),
        Tile::Gravel => {
            let cell = hash01(seed ^ 0x55, x / 2, y / 2);
            let base = if cell > 0.66 {
                (150, 144, 138)
            } else if cell > 0.33 {
                (132, 124, 118)
            } else {
                (110, 104, 100)
            };
            speckled(seed, x, y, base, 8.0)
        }
        Tile::Snow => {
            let color = speckled(seed, x, y, (242, 248, 250), 5.0);
            if hash01(seed ^ 0x66, x, y) > 0.9 {
                rgb(222.0, 234.0, 242.0)
            } else {
                color
            }
        }
        Tile::Ice => {
            let color = speckled(seed, x, y, (150, 190, 220), 8.0);
            if (x + y) % 11 == 0 {
                rgb(120.0, 162.0, 200.0)
            } else {
                color
            }
        }
        Tile::LogSide => {
            // Vertical bark streaks with occasional knots.
            let streak = hash01(seed ^ 0x77, x, 0);
            let base = if x % 5 == 0 || streak > 0.82 {
                (92.0, 72.0, 44.0)
            } else {
                (122.0, 96.0, 60.0)
            };
            let knot = hash01(seed, x / 3, y / 3) > 0.94;
            if knot {
                rgb(base.0 - 24.0, base.1 - 20.0, base.2 - 12.0)
            } else {
                rgb(base.0, base.1, base.2)
            }
        }
        Tile::LogTop => {
            // Concentric growth rings around the tile centre.
            let dx = x as f32 - 7.5;
            let dy = y as f32 - 7.5;
            let distance = (dx * dx + dy * dy).sqrt();
            let ring = (distance * 1.6) as u32 % 2 == 0;
            let base = if ring {
                (160.0, 130.0, 82.0)
            } else {
                (128.0, 100.0, 60.0)
            };
            if distance < 1.5 {
                rgb(104.0, 80.0, 46.0)
            } else {
                rgb(base.0, base.1, base.2)
            }
        }
        Tile::Leaves => {
            // Opaque mottled leaves (fast-graphics style).
            let noise = hash01(seed, x, y);
            if noise > 0.82 {
                rgb(44.0, 88.0, 34.0)
            } else if noise < 0.18 {
                rgb(88.0, 148.0, 58.0)
            } else {
                speckled(seed ^ 0x88, x, y, (62, 122, 42), 16.0)
            }
        }
        Tile::Planks => {
            // Horizontal boards with staggered vertical joints.
            let band = y / 4;
            let joint = (band * 5 + 3) % 16;
            if y % 4 == 3 || x == joint {
                rgb(116.0, 90.0, 52.0)
            } else {
                speckled(seed ^ 0x99, x, y, (176, 142, 88), 10.0)
            }
        }
        Tile::Bedrock => {
            let cell = hash01(seed ^ 0xAA, x / 2, y / 2);
            let level = 46.0 + cell * 54.0;
            rgb(level, level, level)
        }
        Tile::Water => {
            let wave = hash01(seed ^ 0xBB, x / 2, y);
            let base = if y % 5 == 0 {
                (70.0, 132.0, 210.0)
            } else {
                (52.0, 112.0, 196.0)
            };
            rgb(
                base.0 + wave * 12.0,
                base.1 + wave * 12.0,
                base.2 + wave * 8.0,
            )
        }
        Tile::Lava => {
            let noise = hash01(seed, x, y);
            if noise > 0.72 {
                rgb(255.0, 180.0, 48.0)
            } else if noise < 0.2 {
                rgb(150.0, 52.0, 12.0)
            } else {
                speckled(seed ^ 0xCC, x, y, (220, 100, 22), 14.0)
            }
        }
        Tile::CactusSide => {
            if x % 4 == 0 {
                rgb(42.0, 88.0, 38.0)
            } else if y % 3 == 0 && x % 4 == 2 {
                rgb(214.0, 226.0, 190.0)
            } else {
                speckled(seed, x, y, (58, 110, 50), 10.0)
            }
        }
        Tile::CactusTop => {
            let dx = x as f32 - 7.5;
            let dy = y as f32 - 7.5;
            if (dx * dx + dy * dy).sqrt() < 3.0 {
                rgb(44.0, 92.0, 42.0)
            } else {
                speckled(seed, x, y, (70, 120, 60), 9.0)
            }
        }
        Tile::Clay => speckled(seed, x, y, (160, 166, 178), 8.0),
        Tile::Obsidian => {
            let noise = hash01(seed, x, y);
            if noise > 0.9 {
                rgb(58.0, 48.0, 82.0)
            } else {
                speckled(seed ^ 0xDD, x, y, (30, 24, 44), 9.0)
            }
        }
        Tile::Netherrack => {
            let noise = hash01(seed, x, y);
            if noise > 0.7 {
                rgb(132.0, 54.0, 54.0)
            } else {
                speckled(seed ^ 0xEE, x, y, (112, 44, 44), 12.0)
            }
        }
        Tile::EndStone => {
            let noise = hash01(seed, x, y);
            if noise > 0.85 {
                rgb(198.0, 198.0, 136.0)
            } else {
                speckled(seed ^ 0xFF, x, y, (220, 220, 158), 8.0)
            }
        }
        Tile::Wool => speckled(seed, x, y, (232, 232, 232), 6.0),
        Tile::Plant => {
            // Sparse upright blades on a transparent background.
            let blade = x % 4 == 1 || (y > 8 && x % 5 == 0);
            if blade {
                speckled(seed, x, y, (72, 132, 52), 12.0)
            } else {
                Color::TRANSPARENT
            }
        }
        Tile::Flower => {
            let stem = x == 8 && y > 6;
            let dx = x as f32 - 7.5;
            let dy = y as f32 - 5.5;
            if stem || (dx * dx + dy * dy).sqrt() < 3.4 {
                if y < 6 {
                    rgb(232.0, 196.0, 72.0)
                } else {
                    speckled(seed, x, y, (86, 150, 60), 10.0)
                }
            } else {
                Color::TRANSPARENT
            }
        }
        Tile::Torch => {
            if (6..=9).contains(&x) && y >= 6 {
                speckled(seed, x, y, (128, 96, 54), 8.0)
            } else if (6..=9).contains(&x) && (3..=5).contains(&y) {
                rgb(255.0, 196.0, 64.0)
            } else {
                Color::TRANSPARENT
            }
        }
        Tile::Glass => {
            // Clear centre with a light frame so cutout rendering is obvious.
            if x == 0 || y == 0 || x == TILE_SIZE - 1 || y == TILE_SIZE - 1 {
                Color::rgba(214, 232, 244, 210)
            } else if x % 7 == 0 && y % 5 == 0 {
                Color::rgba(240, 248, 255, 120)
            } else {
                Color::TRANSPARENT
            }
        }
        Tile::Unknown => {
            // Neutral checker; the block's hashed colour is applied as a tint.
            if (x / 8 + y / 8) % 2 == 0 {
                rgb(180.0, 180.0, 180.0)
            } else {
                rgb(110.0, 110.0, 110.0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_has_expected_dimensions_and_padding_is_opaque() {
        let atlas = BlockAtlas::new();
        assert_eq!(atlas.texture.width(), ATLAS_WIDTH);
        assert_eq!(atlas.texture.height(), ATLAS_HEIGHT);
        assert_eq!(ATLAS_WIDTH, 128);
        assert_eq!(ATLAS_HEIGHT, 64);
        // Padding and ordinary tiles are opaque; glass/plants intentionally
        // carry alpha so the cutout path has real transparent texels.
        let glass = atlas.uv(Tile::Glass);
        let x = (glass[0] * ATLAS_WIDTH as f32) as u32;
        let y = (glass[1] * ATLAS_HEIGHT as f32) as u32;
        assert!(
            atlas
                .texture
                .sample_nearest(
                    (x as f32 + 4.0) / ATLAS_WIDTH as f32,
                    (y as f32 + 4.0) / ATLAS_HEIGHT as f32
                )
                .a
                < 255
        );
        let stone = atlas.uv(Tile::Stone);
        let x = (stone[0] * ATLAS_WIDTH as f32) as u32;
        let y = (stone[1] * ATLAS_HEIGHT as f32) as u32;
        assert_eq!(
            atlas
                .texture
                .sample_nearest(
                    (x as f32 + 4.0) / ATLAS_WIDTH as f32,
                    (y as f32 + 4.0) / ATLAS_HEIGHT as f32
                )
                .a,
            255
        );
    }

    #[test]
    fn tiles_are_unique_and_uvs_stay_inside_the_atlas() {
        let atlas = BlockAtlas::new();
        let mut rects = std::collections::BTreeSet::new();
        for tile in Tile::ALL {
            let uv = atlas.uv(tile);
            assert!(
                uv[0] > 0.0 && uv[0] < uv[2] && uv[2] < 1.0,
                "{tile:?}: {uv:?}"
            );
            assert!(
                uv[1] > 0.0 && uv[1] < uv[3] && uv[3] < 1.0,
                "{tile:?}: {uv:?}"
            );
            rects.insert(uv.map(f32::to_bits));
        }
        assert_eq!(rects.len(), TILE_COUNT);
    }

    #[test]
    fn atlas_is_deterministic() {
        let first = BlockAtlas::new();
        let second = BlockAtlas::new();
        assert_eq!(first.texture.pixels(), second.texture.pixels());
    }

    #[test]
    fn grass_uses_face_specific_tiles() {
        assert_eq!(
            block_tile("minecraft:grass_block", FaceDir::PosY).0,
            Tile::GrassTop
        );
        assert_eq!(
            block_tile("minecraft:grass_block", FaceDir::NegY).0,
            Tile::Dirt
        );
        assert_eq!(
            block_tile("minecraft:grass_block", FaceDir::PosX).0,
            Tile::GrassSide
        );
        assert_eq!(
            block_tile("minecraft:oak_log", FaceDir::PosY).0,
            Tile::LogTop
        );
        assert_eq!(
            block_tile("minecraft:oak_log", FaceDir::NegZ).0,
            Tile::LogSide
        );
    }

    #[test]
    fn unknown_blocks_keep_a_stable_tint_and_neutral_tile() {
        let (tile, tint) = block_tile("mod:weird_block", FaceDir::PosY);
        assert_eq!(tile, Tile::Unknown);
        assert_eq!(tint, block_color("mod:weird_block"));
        assert_eq!(
            block_tile("mod:weird_block", FaceDir::NegZ),
            (Tile::Unknown, tint)
        );
    }

    #[test]
    fn face_shading_matches_the_minecraft_convention() {
        assert_eq!(FaceDir::PosY.shade(), 1.0);
        assert_eq!(FaceDir::NegY.shade(), 0.5);
        assert_eq!(FaceDir::PosX.shade(), 0.6);
        assert_eq!(FaceDir::PosZ.shade(), 0.8);
    }
}
