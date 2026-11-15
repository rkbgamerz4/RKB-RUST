//! A synthetic 1.21 demo world for tests and emulator verification.
//!
//! No Minecraft content is bundled or copied. The save is generated from
//! scratch with the same Anvil layout a 1.21 client writes, so the runtime
//! exercises the real `rkb-mc` reader, chunk mesher, collision, and renderer.
//!
//! The world is a small grassy island: seven by seven chunks of terrain on a
//! bedrock slab. It exists to prove that the client can load and render saved
//! block data, not to stand in for a user's world.

use std::path::Path;

use rkb_mc::chunk::{SECTION_EDGE, SECTION_VOLUME};
use rkb_mc::fixture::{chunk_tag, level_tag, section_tag, write_world};
use rkb_mc::{BlockState, Tag};

/// Chunks are generated for `0..CHUNKS` on both axes (49 chunks).
pub const CHUNKS: i32 = 7;
/// Bottom of the bedrock safety slab.
pub const BEDROCK_Y: i32 = 40;
/// Data version written into every chunk and `level.dat` (1.21).
pub const DATA_VERSION: i32 = 3953;

/// Description of a written demo save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DemoWorld {
    /// Spawn block position: the player stands one block above this.
    pub spawn: [i32; 3],
    pub chunks: usize,
    pub blocks: usize,
}

struct SectionBuilder {
    y: i8,
    palette: Vec<BlockState>,
    cells: Vec<u16>,
}

impl SectionBuilder {
    fn new(y: i8) -> Self {
        Self {
            y,
            // Index zero is always air so untouched cells stay empty.
            palette: vec![BlockState::new("minecraft:air")],
            cells: vec![0; SECTION_VOLUME],
        }
    }

    fn set(&mut self, x: usize, y: usize, z: usize, name: &str) {
        if x >= SECTION_EDGE || y >= SECTION_EDGE || z >= SECTION_EDGE {
            return;
        }
        let index = if name == "minecraft:air" {
            0
        } else if let Some(index) = self.palette.iter().position(|state| state.name == name) {
            index
        } else {
            self.palette.push(BlockState::new(name));
            self.palette.len() - 1
        };
        self.cells[(y << 8) | (z << 4) | x] = index as u16;
    }

    fn tag(self) -> Tag {
        section_tag(self.y, &self.palette, &self.cells)
    }
}

fn hash01(x: i32, z: i32) -> f32 {
    let mut hash = (x as u32)
        .wrapping_mul(0x9E37_79B1)
        .wrapping_mul(0x85EB_CA77)
        ^ (z as u32).wrapping_mul(0xC2B2_AE3D);
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(0x7FEB_352D);
    hash ^= hash >> 15;
    (hash & 0xffff) as f32 / 65535.0
}

/// Deterministic rolling terrain: 64..=69 blocks above the slab.
fn terrain_height(wx: i32, wz: i32) -> i32 {
    let roll = (wx as f32 * 0.17).sin() * 1.8 + (wz as f32 * 0.13).cos() * 1.5;
    let grit = hash01(wx / 3, wz / 3) * 2.0;
    64 + (roll + grit).round().max(0.0) as i32
}

/// Trees sit in a ring around spawn, far enough to leave an open glade and
/// close enough to the chunk center that no canopy spans a chunk boundary.
const TREES: [(i32, i32); 4] = [(50, 50), (61, 50), (50, 61), (61, 61)];

fn add_tree(sections: &mut [SectionBuilder; 3], wx: i32, wz: i32) {
    let base = terrain_height(wx, wz) + 1;
    let trunk = 4 + (hash01(wx, wz) * 2.0) as i32;
    let top = base + trunk - 1;
    for y in base..=top {
        place(sections, wx, y, wz, "minecraft:oak_log");
    }
    for dy in -2..=1 {
        let radius: i32 = if dy <= 0 { 2 } else { 1 };
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                if dx.abs() == radius && dz.abs() == radius {
                    continue;
                }
                if dx == 0 && dz == 0 && dy < 0 {
                    continue;
                }
                place(sections, wx + dx, top + dy, wz + dz, "minecraft:oak_leaves");
            }
        }
    }
}

/// Places a block in the section that contains it. Only sections 2..=4 exist.
fn place(sections: &mut [SectionBuilder; 3], wx: i32, wy: i32, wz: i32, name: &str) {
    if !(32..80).contains(&wy) {
        return;
    }
    let section = (wy / 16 - 2) as usize;
    let local_y = wy.rem_euclid(16) as usize;
    sections[section].set(
        wx.rem_euclid(16) as usize,
        local_y,
        wz.rem_euclid(16) as usize,
        name,
    );
}

fn build_chunk(cx: i32, cz: i32) -> (Tag, usize) {
    let mut sections = [
        SectionBuilder::new(2),
        SectionBuilder::new(3),
        SectionBuilder::new(4),
    ];

    // Section 2: a bedrock safety slab at y=40 so a fall never reaches the void.
    for x in 0..SECTION_EDGE {
        for z in 0..SECTION_EDGE {
            let wx = cx * 16 + x as i32;
            let wz = cz * 16 + z as i32;
            sections[0].set(x, (BEDROCK_Y - 32) as usize, z, "minecraft:bedrock");
            let height = terrain_height(wx, wz);
            for y in 48..=height {
                let name = if y == height {
                    "minecraft:grass_block"
                } else if y >= height - 3 {
                    "minecraft:dirt"
                } else {
                    "minecraft:stone"
                };
                let section = if y < 64 { 1 } else { 2 };
                sections[section].set(x, y.rem_euclid(16) as usize, z, name);
            }
        }
    }

    // Section 3 is solid stone below the terrain surface, so the island reads
    // as ground rather than a floating sheet.
    for x in 0..SECTION_EDGE {
        for y in 0..SECTION_EDGE {
            for z in 0..SECTION_EDGE {
                if sections[1].cells[(y << 8) | (z << 4) | x] == 0 {
                    sections[1].set(x, y, z, "minecraft:stone");
                }
            }
        }
    }

    for (tree_x, tree_z) in TREES {
        if tree_x.div_euclid(16) == cx && tree_z.div_euclid(16) == cz {
            add_tree(&mut sections, tree_x, tree_z);
        }
    }

    let blocks = sections
        .iter()
        .map(|section| section.cells.iter().filter(|&&index| index != 0).count())
        .sum();

    let tag = chunk_tag(
        DATA_VERSION,
        cx,
        cz,
        sections.into_iter().map(SectionBuilder::tag).collect(),
    );
    (tag, blocks)
}

/// Writes a complete demo save into `dir` and returns its description.
///
/// Any existing files in `dir` are left in place; callers that need a clean
/// save should remove the directory first.
pub fn write_demo_world(dir: &Path) -> Result<DemoWorld, String> {
    let mut level = level_tag(DATA_VERSION, "RKB Demo Island");
    // A fixed clear spawn: the center of the generated island.
    if let Some(Tag::Compound(data)) = level.get_mut("Data") {
        let spawn_x = CHUNKS * 16 / 2;
        let spawn_z = CHUNKS * 16 / 2;
        data.insert("SpawnX".to_string(), Tag::Int(spawn_x));
        data.insert(
            "SpawnY".to_string(),
            Tag::Int(terrain_height(spawn_x, spawn_z) + 1),
        );
        data.insert("SpawnZ".to_string(), Tag::Int(spawn_z));
    }

    let mut chunks = Vec::new();
    let mut blocks = 0;
    for cx in 0..CHUNKS {
        for cz in 0..CHUNKS {
            let (tag, chunk_blocks) = build_chunk(cx, cz);
            blocks += chunk_blocks;
            chunks.push((cx, cz, tag));
        }
    }
    write_world(dir, &level, &chunks).map_err(|error| error.to_string())?;

    let spawn_x = CHUNKS * 16 / 2;
    let spawn_z = CHUNKS * 16 / 2;
    Ok(DemoWorld {
        spawn: [spawn_x, terrain_height(spawn_x, spawn_z) + 1, spawn_z],
        chunks: (CHUNKS * CHUNKS) as usize,
        blocks,
    })
}

/// Writes a save with one full-stone chunk, used by mesh and collision tests.
pub fn write_flat_test_world(dir: &Path, data_version: i32) -> Result<(), String> {
    let mut section = SectionBuilder::new(3);
    for x in 0..SECTION_EDGE {
        for y in 0..SECTION_EDGE {
            for z in 0..SECTION_EDGE {
                section.set(x, y, z, "minecraft:stone");
            }
        }
    }
    let level = level_tag(data_version, "Flat Test");
    let chunks = [(0, 0, chunk_tag(data_version, 0, 0, vec![section.tag()]))];
    write_world(dir, &level, &chunks).map_err(|error| error.to_string())
}
