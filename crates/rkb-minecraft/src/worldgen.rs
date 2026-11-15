//! Deterministic overworld terrain and an in-memory generated world.
//!
//! This is the default world source when the app has no user save. It is not a
//! bundled Minecraft world: every block is produced from a seed by RKB code.
//! Chunks are real [`Chunk`] values with real section palettes, so the mesher,
//! collision, statistics, and save writer all operate on the same data path as
//! a loaded 1.21 save.
//!
//! Terrain is generated lazily on a background worker. A 3x3 spawn area is
//! generated eagerly so the first presented frame already contains ground.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use rkb_core::Vec3;
use rkb_mc::chunk::SECTION_EDGE;
use rkb_mc::{BlockState, Chunk, Section};

/// Data version written into generated chunks (Minecraft 1.21).
pub const DATA_VERSION: i32 = 3953;
/// Lowest generated Y, matching the 1.21 overworld floor.
pub const WORLD_MIN_Y: i32 = -64;
/// Highest generated Y (exclusive). Sections above this are absent and read as air.
pub const WORLD_MAX_Y: i32 = 112;
/// Sea level used by the terrain generator.
pub const SEA_LEVEL: i32 = 62;
/// The default seed for a newly created generated world.
pub const DEFAULT_SEED: u64 = 0x1A21_5EED_5EED_0001;

const SPAWN_RADIUS_CHUNKS: i32 = 1;

/// A deterministic terrain generator. Cloning shares nothing but the seed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainGenerator {
    seed: u64,
}

/// Surface material selected by the low-frequency climate noise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Biome {
    Plains,
    Forest,
    Desert,
    Snowy,
    Ocean,
}

impl Biome {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Plains => "plains",
            Self::Forest => "forest",
            Self::Desert => "desert",
            Self::Snowy => "snowy",
            Self::Ocean => "ocean",
        }
    }
}

impl TerrainGenerator {
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { seed }
    }

    #[must_use]
    pub const fn seed(self) -> u64 {
        self.seed
    }

    /// Terrain height at a world column (highest solid ground block).
    #[must_use]
    pub fn height(self, wx: i32, wz: i32) -> i32 {
        let continental = expand(
            noise2(self.seed ^ 0x11, wx as f32 / 176.0, wz as f32 / 176.0),
            2.8,
        );
        let hills = expand(
            noise2(self.seed ^ 0x22, wx as f32 / 52.0, wz as f32 / 52.0),
            2.2,
        );
        let detail = noise2(self.seed ^ 0x33, wx as f32 / 13.0, wz as f32 / 13.0);
        let ridge = (noise2(self.seed ^ 0x44, wx as f32 / 96.0, wz as f32 / 96.0) - 0.5).abs();
        let mut height = 34.0 + continental * 22.0 + hills * 12.0 + detail * 4.0 - ridge * 8.0;
        // Keep a walkable plateau around the origin so the initial spawn is
        // above water and not inside a mountain.
        let distance = (wx as f32).abs().max((wz as f32).abs());
        if distance < 28.0 {
            height += (28.0 - distance) / 28.0 * 8.0;
        }
        height.round().clamp(34.0, 104.0) as i32
    }

    #[must_use]
    pub fn biome(self, wx: i32, wz: i32) -> Biome {
        let height = self.height(wx, wz);
        if height < SEA_LEVEL {
            return Biome::Ocean;
        }
        let temperature = expand(
            noise2(self.seed ^ 0x55, wx as f32 / 190.0, wz as f32 / 190.0),
            2.6,
        );
        let moisture = expand(
            noise2(self.seed ^ 0x66, wx as f32 / 140.0, wz as f32 / 140.0),
            2.4,
        );
        if temperature > 0.64 && moisture < 0.5 {
            Biome::Desert
        } else if temperature < 0.33 && height > 64 {
            Biome::Snowy
        } else if moisture > 0.58 {
            Biome::Forest
        } else {
            Biome::Plains
        }
    }

    /// Picks a deterministic land spawn near the origin.
    #[must_use]
    pub fn find_spawn(self) -> Vec3 {
        for radius in 0..=12i32 {
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    if radius > 0 && dx.abs() != radius && dz.abs() != radius {
                        continue;
                    }
                    let x = dx * 4;
                    let z = dz * 4;
                    let height = self.height(x, z);
                    if height > SEA_LEVEL + 2 && !self.has_tree(x, z) {
                        return Vec3::new(x as f32 + 0.5, (height + 1) as f32, z as f32 + 0.5);
                    }
                }
            }
        }
        Vec3::new(0.5, (self.height(0, 0) + 1) as f32, 0.5)
    }

    fn has_tree(self, wx: i32, wz: i32) -> bool {
        let biome = self.biome(wx, wz);
        if !matches!(biome, Biome::Plains | Biome::Forest) {
            return false;
        }
        let density = if biome == Biome::Forest {
            0.008
        } else {
            0.0025
        };
        let distance = (wx as f32).hypot(wz as f32);
        if distance < 7.0 {
            return false;
        }
        hash01(self.seed ^ 0xAA, wx, wz) < density
    }

    /// Generates one complete chunk. This is a pure function of the seed and
    /// chunk coordinates; two calls always produce identical data.
    #[must_use]
    pub fn generate(self, cx: i32, cz: i32) -> Chunk {
        let mut sections: HashMap<i8, SectionBuilder> = HashMap::new();
        for z in 0..SECTION_EDGE {
            for x in 0..SECTION_EDGE {
                let wx = cx * 16 + x as i32;
                let wz = cz * 16 + z as i32;
                let height = self.height(wx, wz);
                let biome = self.biome(wx, wz);
                fill_column(&mut sections, (cx, cz), wx, wz, height, biome, self.seed);
            }
        }

        // Trees are scanned one chunk outside this one, because leaves can
        // cross a chunk boundary. The same tree therefore appears in every
        // chunk it overlaps without any shared mutable state.
        for origin_cz in cz - 1..=cz + 1 {
            for origin_cx in cx - 1..=cx + 1 {
                for z in 0..SECTION_EDGE {
                    for x in 0..SECTION_EDGE {
                        let wx = origin_cx * 16 + x as i32;
                        let wz = origin_cz * 16 + z as i32;
                        if !self.has_tree(wx, wz) {
                            continue;
                        }
                        let height = self.height(wx, wz);
                        if height <= SEA_LEVEL {
                            continue;
                        }
                        place_tree(&mut sections, self.seed, (cx, cz), wx, height + 1, wz);
                    }
                }
            }
        }

        let mut built: Vec<Section> = sections
            .into_values()
            .filter(|section| section.has_blocks())
            .map(SectionBuilder::build)
            .collect();
        built.sort_by_key(|section| section.y);
        Chunk::new(DATA_VERSION, cx, cz, built).expect("generated sections are unique")
    }
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
            palette: vec![BlockState::air()],
            cells: vec![0; rkb_mc::chunk::SECTION_VOLUME],
        }
    }

    fn set(&mut self, x: i32, y: i32, z: i32, name: &str) {
        let index = match self.palette.iter().position(|state| {
            state.name.strip_prefix("minecraft:") == name.strip_prefix("minecraft:")
        }) {
            Some(index) => index,
            None => {
                self.palette.push(BlockState::new(name));
                self.palette.len() - 1
            }
        };
        let local = (y.rem_euclid(16) << 8 | z << 4 | x) as usize;
        self.cells[local] = index as u16;
    }

    fn has_blocks(&self) -> bool {
        self.cells.iter().any(|&index| index != 0)
    }

    fn build(self) -> Section {
        Section::new(self.y, self.palette, self.cells)
    }
}

fn section_builder(sections: &mut HashMap<i8, SectionBuilder>, wy: i32) -> &mut SectionBuilder {
    let section_y = i8::try_from(wy.div_euclid(16)).expect("generated Y fits a section");
    sections
        .entry(section_y)
        .or_insert_with(|| SectionBuilder::new(section_y))
}

#[allow(clippy::too_many_arguments)]
fn place(
    sections: &mut HashMap<i8, SectionBuilder>,
    chunk: (i32, i32),
    wx: i32,
    wy: i32,
    wz: i32,
    name: &str,
) {
    if !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&wy)
        || wx.div_euclid(16) != chunk.0
        || wz.div_euclid(16) != chunk.1
    {
        return;
    }
    section_builder(sections, wy).set(wx.rem_euclid(16), wy, wz.rem_euclid(16), name);
}

fn fill_column(
    sections: &mut HashMap<i8, SectionBuilder>,
    chunk: (i32, i32),
    wx: i32,
    wz: i32,
    height: i32,
    biome: Biome,
    seed: u64,
) {
    let (top, filler) = match biome {
        Biome::Desert => ("minecraft:sand", "minecraft:sand"),
        Biome::Snowy => ("minecraft:snow_block", "minecraft:dirt"),
        Biome::Ocean => ("minecraft:sand", "minecraft:sand"),
        Biome::Plains | Biome::Forest => ("minecraft:grass_block", "minecraft:dirt"),
    };
    for y in WORLD_MIN_Y..=height {
        let name = if y == WORLD_MIN_Y
            || y == WORLD_MIN_Y + 1
            || (y == WORLD_MIN_Y + 2 && hash01(seed ^ 0x77, wx, wz) < 0.5)
        {
            "minecraft:bedrock"
        } else if y == height {
            top
        } else if y > height - 4 {
            filler
        } else {
            "minecraft:stone"
        };
        section_builder(sections, y).set(wx.rem_euclid(16), y, wz.rem_euclid(16), name);
    }

    if height < SEA_LEVEL {
        for y in height + 1..=SEA_LEVEL {
            section_builder(sections, y).set(
                wx.rem_euclid(16),
                y,
                wz.rem_euclid(16),
                "minecraft:water",
            );
        }
        // A little gravel/sand variation on the sea floor.
        if hash01(seed ^ 0x88, wx, wz) < 0.25 {
            section_builder(sections, height).set(
                wx.rem_euclid(16),
                height,
                wz.rem_euclid(16),
                "minecraft:gravel",
            );
        }
    } else if biome == Biome::Desert && hash01(seed ^ 0x99, wx, wz) < 0.02 {
        // Sparse cacti.
        let height_cactus = 1 + (hash01(seed ^ 0x9A, wx, wz) * 3.0) as i32;
        for y in height + 1..=height + height_cactus {
            place(sections, chunk, wx, y, wz, "minecraft:cactus");
        }
    }
}

fn place_tree(
    sections: &mut HashMap<i8, SectionBuilder>,
    seed: u64,
    chunk: (i32, i32),
    wx: i32,
    base: i32,
    wz: i32,
) {
    let trunk = 4 + (hash01(seed ^ 0xBB, wx, wz) * 3.0) as i32;
    let top = base + trunk - 1;
    for y in base..=top {
        place(sections, chunk, wx, y, wz, "minecraft:oak_log");
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
                place(
                    sections,
                    chunk,
                    wx + dx,
                    top + dy,
                    wz + dz,
                    "minecraft:oak_leaves",
                );
            }
        }
    }
}

fn mix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn hash01(seed: u64, x: i32, z: i32) -> f32 {
    let x = u64::from(x as u32);
    let z = u64::from(z as u32);
    let value = mix(seed ^ x.wrapping_mul(0x9E37_79B1) ^ z.wrapping_mul(0x85EB_CA77));
    (value >> 40) as f32 / (1u32 << 24) as f32
}

fn smooth(value: f32) -> f32 {
    value * value * (3.0 - 2.0 * value)
}

/// Widens the narrow range of bilinearly interpolated value noise. Without
/// this, a large-scale noise field stays near 0.5 and terrain never forms
/// distinct lowlands and highlands.
fn expand(value: f32, factor: f32) -> f32 {
    ((value - 0.5) * factor + 0.5).clamp(0.0, 1.0)
}

fn noise2(seed: u64, x: f32, z: f32) -> f32 {
    let x0 = x.floor();
    let z0 = z.floor();
    let fx = smooth(x - x0);
    let fz = smooth(z - z0);
    let x0 = x0 as i32;
    let z0 = z0 as i32;
    let a = hash01(seed, x0, z0);
    let b = hash01(seed, x0 + 1, z0);
    let c = hash01(seed, x0, z0 + 1);
    let d = hash01(seed, x0 + 1, z0 + 1);
    let top = a + (b - a) * fx;
    let bottom = c + (d - c) * fx;
    top + (bottom - top) * fz
}

struct GenerationWorker {
    requests: Sender<(i32, i32)>,
    results: Receiver<((i32, i32), Arc<Chunk>)>,
    join: Option<thread::JoinHandle<()>>,
}

impl GenerationWorker {
    fn spawn(generator: TerrainGenerator) -> Option<Self> {
        let (requests, request_rx) = mpsc::channel::<(i32, i32)>();
        let (result_tx, results) = mpsc::channel::<((i32, i32), Arc<Chunk>)>();
        let join = thread::Builder::new()
            .name("rkb-chunk-gen".to_string())
            .spawn(move || {
                for (cx, cz) in request_rx {
                    let chunk = Arc::new(generator.generate(cx, cz));
                    if result_tx.send(((cx, cz), chunk)).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Self {
            requests,
            results,
            join: Some(join),
        })
    }

    fn shutdown(mut self) {
        // Dropping the sender ends the worker loop. The receiver is dropped
        // first so a result already in flight does not block the join.
        drop(self.requests);
        let results = self.results;
        drop(results);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// An in-memory world whose chunks are generated from a seed.
pub struct GeneratedWorld {
    generator: TerrainGenerator,
    spawn: Vec3,
    chunks: HashMap<(i32, i32), Arc<Chunk>>,
    pending: HashSet<(i32, i32)>,
    generated_total: usize,
    worker: Option<GenerationWorker>,
}

impl fmt::Debug for GeneratedWorld {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GeneratedWorld")
            .field("seed", &self.generator.seed())
            .field("spawn", &self.spawn)
            .field("chunks", &self.chunks.len())
            .field("pending", &self.pending.len())
            .field("generated_total", &self.generated_total)
            .finish_non_exhaustive()
    }
}

impl GeneratedWorld {
    /// Creates a world, eagerly generating the spawn neighbourhood and then
    /// starting one background generation worker.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        let generator = TerrainGenerator::new(seed);
        let spawn = generator.find_spawn();
        let center = (
            spawn.x.floor().div_euclid(16.0) as i32,
            spawn.z.floor().div_euclid(16.0) as i32,
        );
        let mut world = Self {
            generator,
            spawn,
            chunks: HashMap::new(),
            pending: HashSet::new(),
            generated_total: 0,
            worker: GenerationWorker::spawn(generator),
        };
        for dz in -SPAWN_RADIUS_CHUNKS..=SPAWN_RADIUS_CHUNKS {
            for dx in -SPAWN_RADIUS_CHUNKS..=SPAWN_RADIUS_CHUNKS {
                world.generate_sync(center.0 + dx, center.1 + dz);
            }
        }
        world
    }

    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.generator.seed()
    }

    #[must_use]
    pub const fn spawn(&self) -> Vec3 {
        self.spawn
    }

    #[must_use]
    pub fn generator(&self) -> TerrainGenerator {
        self.generator
    }

    /// Chunks currently resident in memory.
    #[must_use]
    pub fn resident_chunks(&self) -> usize {
        self.chunks.len()
    }

    /// Chunks generated since the world was created (including unloaded ones).
    #[must_use]
    pub const fn generated_chunks(&self) -> usize {
        self.generated_total
    }

    #[must_use]
    pub fn pending_chunks(&self) -> usize {
        self.pending.len()
    }

    fn generate_sync(&mut self, cx: i32, cz: i32) {
        if self.chunks.contains_key(&(cx, cz)) {
            return;
        }
        let chunk = Arc::new(self.generator.generate(cx, cz));
        self.chunks.insert((cx, cz), chunk);
        self.generated_total += 1;
        self.pending.remove(&(cx, cz));
    }

    /// Requests generation and returns the chunk when it is already resident.
    ///
    /// The first call for a coordinate queues background work and returns
    /// `None`; the caller should try again on a later frame.
    pub fn chunk(&mut self, cx: i32, cz: i32) -> Option<Arc<Chunk>> {
        self.poll();
        if let Some(chunk) = self.chunks.get(&(cx, cz)) {
            return Some(Arc::clone(chunk));
        }
        if self.worker.is_none() {
            self.generate_sync(cx, cz);
            return self.chunks.get(&(cx, cz)).cloned();
        }
        self.request(cx, cz);
        None
    }

    /// Generates a chunk immediately on this thread. Used by tests and by the
    /// save path.
    pub fn chunk_now(&mut self, cx: i32, cz: i32) -> Arc<Chunk> {
        self.generate_sync(cx, cz);
        Arc::clone(&self.chunks[&(cx, cz)])
    }

    fn request(&mut self, cx: i32, cz: i32) {
        if self.chunks.contains_key(&(cx, cz)) || !self.pending.insert((cx, cz)) {
            return;
        }
        let Some(worker) = self.worker.as_ref() else {
            self.pending.remove(&(cx, cz));
            self.generate_sync(cx, cz);
            return;
        };
        if worker.requests.send((cx, cz)).is_err() {
            // Worker died: keep the world usable by generating inline.
            self.pending.remove(&(cx, cz));
            self.generate_sync(cx, cz);
        }
    }

    /// Drains finished background chunks and returns their coordinates.
    pub fn poll(&mut self) -> Vec<(i32, i32)> {
        let mut delivered = Vec::new();
        if let Some(worker) = self.worker.as_ref() {
            while let Ok(((cx, cz), chunk)) = worker.results.try_recv() {
                delivered.push(((cx, cz), chunk));
            }
        }
        let mut finished = Vec::with_capacity(delivered.len());
        for ((cx, cz), chunk) in delivered {
            self.pending.remove(&(cx, cz));
            if self.chunks.insert((cx, cz), chunk).is_none() {
                self.generated_total += 1;
                finished.push((cx, cz));
            }
        }
        finished
    }

    /// Block at absolute world coordinates. Missing or ungenerated chunks
    /// read as air so callers can distinguish "not loaded" from a saved block.
    pub fn block_at(&mut self, x: i32, y: i32, z: i32) -> Option<BlockState> {
        let chunk = self.chunk(x.div_euclid(16), z.div_euclid(16))?;
        chunk
            .block_at(x.rem_euclid(16), y, z.rem_euclid(16))
            .cloned()
    }

    /// Drops chunks outside the retention square. Terrain is regenerated from
    /// the seed if the player returns, so this bounds memory without losing
    /// world state.
    pub fn retain_chunks(&mut self, center: (i32, i32), radius: i32) {
        self.chunks.retain(|&(cx, cz), _| {
            (cx - center.0).abs() <= radius && (cz - center.1).abs() <= radius
        });
    }

    /// Chunks resident in memory, for saving.
    pub fn chunks(&self) -> impl Iterator<Item = &Arc<Chunk>> {
        self.chunks.values()
    }
}

impl Drop for GeneratedWorld {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rkb_mc::fixture::TempDir;
    use rkb_mc::World;

    #[test]
    fn terrain_is_deterministic_for_a_seed() {
        let one = TerrainGenerator::new(12345);
        let two = TerrainGenerator::new(12345);
        let a = one.generate(3, -2);
        let b = two.generate(3, -2);
        assert_eq!(a, b);
        assert!(a.non_air_blocks() > 1_000, "{}", a.non_air_blocks());

        let other = TerrainGenerator::new(54321).generate(3, -2);
        assert_ne!(a, other);
    }

    #[test]
    fn tree_canopies_stay_inside_the_chunk_that_requests_them() {
        use std::collections::BTreeSet;

        let generator = TerrainGenerator::new(DEFAULT_SEED);
        let target = (-3..=3)
            .flat_map(|cx| (-3..=3).map(move |cz| (cx, cz)))
            .find(|&(cx, cz)| {
                generator.generate(cx, cz).sections().any(|section| {
                    section
                        .palette()
                        .iter()
                        .any(|state| state.name == "minecraft:oak_log")
                })
            })
            .expect("a nearby chunk contains a tree");
        let chunk = generator.generate(target.0, target.1);

        let mut actual = BTreeSet::new();
        for section in chunk.sections() {
            let base_y = i32::from(section.y) * 16;
            for y in 0..16 {
                for z in 0..16 {
                    for x in 0..16 {
                        let Some(state) = section.block_at(x, y, z) else {
                            continue;
                        };
                        if state.name == "minecraft:oak_log" || state.name == "minecraft:oak_leaves"
                        {
                            actual.insert((
                                target.0 * 16 + x as i32,
                                base_y + y as i32,
                                target.1 * 16 + z as i32,
                            ));
                        }
                    }
                }
            }
        }

        // Independently place every tree from the surrounding chunks and keep
        // only the blocks that belong to the target chunk. A coordinates wrap
        // bug would put leaves on the wrong side of the chunk and fail here.
        let mut expected = BTreeSet::new();
        for origin_cz in target.1 - 1..=target.1 + 1 {
            for origin_cx in target.0 - 1..=target.0 + 1 {
                for z in 0..16 {
                    for x in 0..16 {
                        let wx = origin_cx * 16 + x;
                        let wz = origin_cz * 16 + z;
                        if !generator.has_tree(wx, wz) {
                            continue;
                        }
                        let height = generator.height(wx, wz);
                        if height <= SEA_LEVEL {
                            continue;
                        }
                        let base = height + 1;
                        let trunk = 4 + (hash01(generator.seed ^ 0xBB, wx, wz) * 3.0) as i32;
                        let top = base + trunk - 1;
                        let mut keep = |bx: i32, by: i32, bz: i32| {
                            if bx.div_euclid(16) == target.0
                                && bz.div_euclid(16) == target.1
                                && (WORLD_MIN_Y..WORLD_MAX_Y).contains(&by)
                            {
                                expected.insert((bx, by, bz));
                            }
                        };
                        for y in base..=top {
                            keep(wx, y, wz);
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
                                    keep(wx + dx, top + dy, wz + dz);
                                }
                            }
                        }
                    }
                }
            }
        }

        assert!(!actual.is_empty(), "sample chunk has no trees");
        assert_eq!(actual, expected);
    }

    #[test]
    fn generated_chunks_have_real_sections_and_surface_blocks() {
        let generator = TerrainGenerator::new(DEFAULT_SEED);
        let chunk = generator.generate(0, 0);
        assert!(chunk.section_count() >= 4, "{}", chunk.section_count());
        assert!(chunk
            .sections()
            .all(|section| (WORLD_MIN_Y / 16..=WORLD_MAX_Y / 16).contains(&i32::from(section.y))));

        // Find a column and confirm the surface is grass/dirt/sand/water.
        let mut surfaces = 0;
        for x in 0..16 {
            for z in 0..16 {
                let wx = x;
                let wz = z;
                let height = generator.height(wx, wz);
                let block = chunk.block_at(x, height, z).expect("surface block");
                if block.name.contains("grass")
                    || block.name.contains("sand")
                    || block.name.contains("snow")
                    || block.name.contains("water")
                {
                    surfaces += 1;
                }
                assert!(
                    chunk.block_at(x, WORLD_MIN_Y, z).is_some(),
                    "bedrock missing at ({x},{z})"
                );
            }
        }
        assert_eq!(surfaces, 256);
    }

    #[test]
    fn spawn_is_above_solid_ground() {
        let mut world = GeneratedWorld::new(DEFAULT_SEED);
        let spawn = world.spawn();
        let x = spawn.x.floor() as i32;
        let y = spawn.y.floor() as i32;
        let z = spawn.z.floor() as i32;
        assert!(spawn.y > SEA_LEVEL as f32, "{spawn:?}");
        let below = world.block_at(x, y - 1, z).expect("spawn column generated");
        assert!(!below.is_air(), "spawn block is {below}");
        let feet = world.block_at(x, y, z).expect("spawn feet");
        assert!(feet.is_air() || feet.name.contains("water"), "{feet}");
        assert!(world.resident_chunks() >= 9);
    }

    #[test]
    fn generated_chunks_can_be_saved_and_reopened() {
        let dir = TempDir::new("generated-save");
        let generated = GeneratedWorld::new(7);
        let chunks: Vec<Chunk> = generated.chunks().map(|chunk| (**chunk).clone()).collect();
        let spawn = generated.spawn();
        let level = rkb_mc::save::level_tag(
            DATA_VERSION,
            "Generated test",
            [
                spawn.x.floor() as i32,
                spawn.y.floor() as i32,
                spawn.z.floor() as i32,
            ],
        );
        rkb_mc::save::write_world(dir.path(), &level, &chunks).expect("save generated");
        let mut loaded = World::open(dir.path()).expect("reopen generated save");
        assert_eq!(loaded.level().level_name.as_deref(), Some("Generated test"));
        let below = loaded
            .block_at(
                spawn.x.floor() as i32,
                spawn.y.floor() as i32 - 1,
                spawn.z.floor() as i32,
            )
            .expect("query saved block")
            .expect("saved block");
        assert!(!below.is_air());
    }

    #[test]
    fn background_chunks_arrive_after_requests() {
        let mut world = GeneratedWorld::new(DEFAULT_SEED);
        let target = (world.spawn().x.floor().div_euclid(16.0) as i32 + 3, 0);
        assert!(world.chunk(target.0, target.1).is_none());
        let mut arrived = false;
        for _ in 0..400 {
            if !world.poll().is_empty() {
                arrived = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(arrived, "worker did not deliver a chunk");
        assert!(world.chunk(target.0, target.1).is_some());
    }

    #[test]
    fn terrain_contains_water_and_trees_somewhere() {
        let generator = TerrainGenerator::new(DEFAULT_SEED);
        let mut low = i32::MAX;
        let mut high = i32::MIN;
        let mut below = 0;
        let mut total = 0;
        for wx in -80..=80 {
            for wz in -80..=80 {
                let h = generator.height(wx, wz);
                low = low.min(h);
                high = high.max(h);
                total += 1;
                if h < SEA_LEVEL {
                    below += 1;
                }
            }
        }
        eprintln!("height range {low}..={high}, below sea {below}/{total}");
        let mut water = 0;
        let mut logs = 0;
        for cx in -2..=2 {
            for cz in -2..=2 {
                let chunk = generator.generate(cx, cz);
                for section in chunk.sections() {
                    for state in section.palette() {
                        if state.name == "minecraft:water" {
                            water += 1;
                        }
                        if state.name == "minecraft:oak_log" {
                            logs += 1;
                        }
                    }
                }
            }
        }
        assert!(water > 0, "no water in a 5x5 chunk sample");
        assert!(logs > 0, "no trees in a 5x5 chunk sample");
    }
}
