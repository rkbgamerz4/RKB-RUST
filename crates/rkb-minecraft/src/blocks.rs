//! Block-name colour table and collision classification.
//!
//! Colours approximate Minecraft's average texture tint. They are intentionally
//! flat: no textures, biome tints, or lightmaps are applied yet.

use rkb_render::Color;

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::rgb(r, g, b)
}

/// Flat colour for a block name. Unknown blocks get a stable muted colour.
#[must_use]
pub fn block_color(name: &str) -> Color {
    let short = name.strip_prefix("minecraft:").unwrap_or(name);
    match short {
        "grass_block" => rgb(98, 156, 62),
        "dirt" | "coarse_dirt" | "rooted_dirt" => rgb(134, 96, 67),
        "podzol" | "mycelium" => rgb(110, 84, 51),
        "mud" => rgb(88, 74, 70),
        "stone" | "andesite" | "cobblestone" | "mossy_cobblestone" | "stone_bricks"
        | "mossy_stone_bricks" | "smooth_stone" | "bedrock" => rgb(125, 125, 125),
        "granite" | "polished_granite" => rgb(150, 104, 84),
        "diorite" | "polished_diorite" => rgb(190, 190, 190),
        "deepslate" | "cobbled_deepslate" | "deepslate_bricks" | "tuff" => rgb(74, 74, 78),
        "gravel" => rgb(132, 124, 118),
        "sand" | "sandstone" | "smooth_sandstone" | "cut_sandstone" => rgb(219, 207, 163),
        "red_sand" | "red_sandstone" => rgb(188, 103, 41),
        "snow" | "snow_block" | "powder_snow" => rgb(242, 248, 250),
        "ice" | "packed_ice" | "blue_ice" => rgb(150, 190, 220),
        "water" => rgb(52, 112, 196),
        "lava" => rgb(220, 100, 22),
        "oak_log" | "oak_wood" => rgb(118, 92, 52),
        "birch_log" | "birch_wood" => rgb(206, 199, 172),
        "spruce_log" | "spruce_wood" => rgb(74, 52, 30),
        "jungle_log" | "jungle_wood" => rgb(112, 82, 44),
        "acacia_log" | "acacia_wood" => rgb(104, 78, 48),
        "dark_oak_log" | "dark_oak_wood" => rgb(62, 44, 24),
        "oak_planks" => rgb(160, 128, 78),
        "birch_planks" => rgb(196, 178, 123),
        "spruce_planks" => rgb(114, 84, 50),
        "oak_leaves" | "jungle_leaves" | "acacia_leaves" => rgb(62, 122, 42),
        "birch_leaves" => rgb(100, 146, 62),
        "spruce_leaves" => rgb(48, 86, 48),
        "dark_oak_leaves" => rgb(42, 96, 40),
        "short_grass" | "tall_grass" | "fern" | "large_fern" => rgb(88, 148, 54),
        "poppy" | "red_tulip" | "rose_bush" => rgb(178, 46, 46),
        "dandelion" | "sunflower" => rgb(238, 206, 64),
        "dead_bush" => rgb(132, 104, 54),
        "glass" | "glass_pane" => rgb(190, 220, 230),
        "white_wool" => rgb(232, 232, 232),
        "clay" => rgb(160, 166, 178),
        "netherrack" => rgb(112, 44, 44),
        "soul_sand" => rgb(82, 60, 48),
        "obsidian" => rgb(28, 22, 40),
        "end_stone" => rgb(220, 220, 158),
        _ => hashed_color(short),
    }
}

fn hashed_color(name: &str) -> Color {
    // FNV-1a gives stable, well-spread colours without a dependency.
    let mut hash: u32 = 0x811c_9dc5;
    for byte in name.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    rgb(
        80 + (hash % 120) as u8,
        80 + ((hash >> 8) % 120) as u8,
        80 + ((hash >> 16) % 120) as u8,
    )
}

/// Whether a block stops movement. Liquids and non-colliding plants do not.
#[must_use]
pub fn is_solid(name: &str) -> bool {
    let short = name.strip_prefix("minecraft:").unwrap_or(name);
    !matches!(
        short,
        "air"
            | "cave_air"
            | "void_air"
            | "water"
            | "lava"
            | "short_grass"
            | "tall_grass"
            | "fern"
            | "large_fern"
            | "dead_bush"
            | "torch"
            | "wall_torch"
            | "soul_torch"
            | "soul_wall_torch"
            | "redstone_wire"
            | "redstone_torch"
            | "vine"
            | "snow"
            | "poppy"
            | "dandelion"
            | "red_tulip"
            | "orange_tulip"
            | "white_tulip"
            | "pink_tulip"
            | "oxeye_daisy"
            | "cornflower"
            | "sunflower"
            | "lilac"
            | "rose_bush"
            | "peony"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_stable_and_distinct_for_common_blocks() {
        assert_eq!(block_color("minecraft:grass_block"), rgb(98, 156, 62));
        assert_eq!(
            block_color("grass_block"),
            block_color("minecraft:grass_block")
        );
        assert_ne!(
            block_color("minecraft:stone"),
            block_color("minecraft:dirt")
        );
        assert_eq!(
            block_color("mod:unknown_block"),
            block_color("mod:unknown_block")
        );
    }

    #[test]
    fn solids_exclude_air_liquids_and_plants() {
        assert!(is_solid("minecraft:stone"));
        assert!(is_solid("minecraft:oak_planks"));
        assert!(!is_solid("minecraft:air"));
        assert!(!is_solid("minecraft:water"));
        assert!(!is_solid("minecraft:short_grass"));
    }
}
