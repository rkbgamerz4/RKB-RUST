//! Block render shapes: full cubes, model elements, and built-in partial blocks.
//!
//! Minecraft is not a cube-only world. This module is the seam between a
//! block-state name/properties and the geometry the mesher emits. A resolved
//! shape is either a list of axis-aligned model elements (the same shape a
//! resource-pack model produces) or a crossed plant. Full cubes are one
//! element, so the mesh loop never needs a "full cube" special case.
//!
//! Resource packs replace this module's built-in shapes with JSON models. The
//! built-in shapes cover the common blocks a flat/survival server sends even
//! when no pack is available: slabs, stairs, snow layers, carpets, fluids, and
//! plants. Unsupported shapes fall back to a full cube and are reported, never
//! silently guessed to be something else.

use rkb_core::Vec3;
use rkb_mc::BlockState;

use crate::textures::FaceDir;

/// How a face is composited.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderLayer {
    /// Fully opaque; writes depth and can cull neighbouring faces.
    Opaque,
    /// Alpha-tested; writes depth, does not blend.
    Cutout,
    /// Alpha-blended, sorted back-to-front, does not write depth.
    Translucent,
}

impl RenderLayer {
    /// Alpha threshold used by the rasterizer for this layer.
    #[must_use]
    pub const fn cutout(self) -> f32 {
        match self {
            Self::Opaque => 0.0,
            Self::Cutout => 0.5,
            Self::Translucent => 0.01,
        }
    }

    #[must_use]
    pub const fn blends(self) -> bool {
        matches!(self, Self::Translucent)
    }
}

/// Biome/fixed tint selector. `None` leaves the texture unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TintKind {
    None,
    Grass,
    Foliage,
    Water,
}

/// Where an element face gets its texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaceTexture {
    /// Built-in procedural tile chosen from the block name and face.
    BlockFace(FaceDir),
    /// Texture id in the active atlas (resource-pack models).
    Atlas(u32),
}

/// One face of one model element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ElementFace {
    pub texture: FaceTexture,
    /// Explicit model UV rectangle in 0..16 model units. `None` derives the
    /// rectangle from the element extents, matching vanilla's default.
    pub uv: Option<[f32; 4]>,
    /// Face direction whose neighbour can hide this face.
    pub cullface: Option<FaceDir>,
    /// Face UV rotation in degrees: 0, 90, 180, or 270.
    pub rotation: u8,
    pub tint: TintKind,
}

impl ElementFace {
    #[must_use]
    pub const fn new(texture: FaceTexture) -> Self {
        Self {
            texture,
            uv: None,
            cullface: None,
            rotation: 0,
            tint: TintKind::None,
        }
    }
}

/// One axis-aligned element in 0..16 model units.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub faces: [Option<ElementFace>; 6],
    /// Optional affine transform (model and element rotation) in model space.
    pub transform: Option<ElementTransform>,
    /// Crossed plants are visible from both sides.
    pub double_sided: bool,
}

/// A baked 3x4 affine transform: `p' = linear * p + offset`, in model units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ElementTransform {
    pub linear: [[f32; 3]; 3],
    pub offset: [f32; 3],
}

impl ElementTransform {
    #[must_use]
    pub const fn identity() -> Self {
        Self {
            linear: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            offset: [0.0; 3],
        }
    }

    /// A rotation around `origin` by `angle` degrees about `axis` (0=X, 1=Y,
    /// 2=Z), in model units.
    #[must_use]
    pub fn rotation(axis: u8, angle: f32, origin: [f32; 3]) -> Self {
        let radians = angle.to_radians();
        let (sin, cos) = radians.sin_cos();
        let linear = match axis {
            0 => [[1.0, 0.0, 0.0], [0.0, cos, -sin], [0.0, sin, cos]],
            1 => [[cos, 0.0, sin], [0.0, 1.0, 0.0], [-sin, 0.0, cos]],
            _ => [[cos, -sin, 0.0], [sin, cos, 0.0], [0.0, 0.0, 1.0]],
        };
        Self {
            linear,
            offset: [
                origin[0] - dot(&linear[0], origin),
                origin[1] - dot(&linear[1], origin),
                origin[2] - dot(&linear[2], origin),
            ],
        }
    }

    /// `self ∘ inner`: apply `inner` first, then `self`.
    #[must_use]
    pub fn then(self, inner: Self) -> Self {
        let mut linear = [[0.0_f32; 3]; 3];
        for (row, output_row) in linear.iter_mut().enumerate() {
            for (column, cell) in output_row.iter_mut().enumerate() {
                *cell = (0..3)
                    .map(|k| self.linear[row][k] * inner.linear[k][column])
                    .sum();
            }
        }
        let rotated = [
            dot(&self.linear[0], inner.offset),
            dot(&self.linear[1], inner.offset),
            dot(&self.linear[2], inner.offset),
        ];
        Self {
            linear,
            offset: [
                rotated[0] + self.offset[0],
                rotated[1] + self.offset[1],
                rotated[2] + self.offset[2],
            ],
        }
    }

    #[must_use]
    pub fn point(self, point: [f32; 3]) -> [f32; 3] {
        [
            dot(&self.linear[0], point) + self.offset[0],
            dot(&self.linear[1], point) + self.offset[1],
            dot(&self.linear[2], point) + self.offset[2],
        ]
    }

    #[must_use]
    pub fn direction(self, direction: Vec3) -> Vec3 {
        let point = [direction.x, direction.y, direction.z];
        Vec3::new(
            dot(&self.linear[0], point),
            dot(&self.linear[1], point),
            dot(&self.linear[2], point),
        )
    }
}

fn dot(row: &[f32; 3], point: [f32; 3]) -> f32 {
    row[0] * point[0] + row[1] * point[1] + row[2] * point[2]
}

impl Element {
    /// A box whose faces are chosen from the block's built-in tiles.
    #[must_use]
    pub fn builtin(from: [f32; 3], to: [f32; 3], cull_on_boundary: bool) -> Self {
        let mut faces: [Option<ElementFace>; 6] = [None; 6];
        for face in FaceDir::ALL {
            let on_boundary = match face {
                FaceDir::NegX => from[0] <= 0.0,
                FaceDir::PosX => to[0] >= 16.0,
                FaceDir::NegY => from[1] <= 0.0,
                FaceDir::PosY => to[1] >= 16.0,
                FaceDir::NegZ => from[2] <= 0.0,
                FaceDir::PosZ => to[2] >= 16.0,
            };
            let mut element_face = ElementFace::new(FaceTexture::BlockFace(face));
            if cull_on_boundary && on_boundary {
                element_face.cullface = Some(face);
            }
            faces[face.index()] = Some(element_face);
        }
        Self {
            from,
            to,
            faces,
            transform: None,
            double_sided: false,
        }
    }

    /// The default UV rectangle for one face, derived from the element extents.
    #[must_use]
    pub fn default_uv(&self, face: FaceDir) -> [f32; 4] {
        let [x0, y0, z0] = self.from;
        let [x1, y1, z1] = self.to;
        match face {
            FaceDir::NegZ | FaceDir::PosZ => [x0, 16.0 - y1, x1, 16.0 - y0],
            FaceDir::NegX | FaceDir::PosX => [z0, 16.0 - y1, z1, 16.0 - y0],
            FaceDir::PosY => [x0, z0, x1, z1],
            FaceDir::NegY => [x0, 16.0 - z1, x1, 16.0 - z0],
        }
    }

    /// Whether one face lies on the block boundary.
    #[must_use]
    pub fn on_boundary(&self, face: FaceDir) -> bool {
        match face {
            FaceDir::NegX => self.from[0] <= 0.0,
            FaceDir::PosX => self.to[0] >= 16.0,
            FaceDir::NegY => self.from[1] <= 0.0,
            FaceDir::PosY => self.to[1] >= 16.0,
            FaceDir::NegZ => self.from[2] <= 0.0,
            FaceDir::PosZ => self.to[2] >= 16.0,
        }
    }
}

/// Geometry for one block state.
#[derive(Clone, Debug, PartialEq)]
pub enum BlockShape {
    /// Axis-aligned elements. A full cube is one element from 0 to 16.
    Elements(Vec<Element>),
    /// Two crossed quads, rendered double-sided.
    Cross {
        texture: FaceTexture,
        tint: TintKind,
    },
}

impl BlockShape {
    #[must_use]
    pub fn full_cube() -> Self {
        Self::Elements(vec![Element::builtin([0.0; 3], [16.0; 3], true)])
    }
}

/// A resolved shape plus its render layer.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedShape {
    pub shape: BlockShape,
    pub layer: RenderLayer,
}

/// Resolves a block state to geometry. The mesher accepts any implementation,
/// so a resource pack can replace built-in shapes without changing the mesh
/// loop.
pub trait BlockModelSource {
    fn resolve(&self, state: &BlockState) -> ResolvedShape;

    /// Whether `state` is drawn as a full cube because no precise model was
    /// available. The mesher counts these for diagnostics.
    fn approximate(&self, state: &BlockState) -> bool {
        is_approximate_fallback(state)
    }
}

/// The built-in, pack-less resolver.
#[derive(Clone, Copy, Debug, Default)]
pub struct BuiltinModels;

impl BlockModelSource for BuiltinModels {
    fn resolve(&self, state: &BlockState) -> ResolvedShape {
        builtin_shape(state)
    }
}

/// Four corners of each face as min/max selectors. The order matches the
/// historical mesher so existing face-orientation tests stay meaningful.
pub const FACE_CORNERS: [[[u8; 3]; 4]; 6] = [
    // NegX
    [[0, 0, 0], [0, 1, 0], [0, 1, 1], [0, 0, 1]],
    // PosX
    [[1, 0, 0], [1, 1, 0], [1, 1, 1], [1, 0, 1]],
    // NegY
    [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]],
    // PosY
    [[0, 1, 0], [1, 1, 0], [1, 1, 1], [0, 1, 1]],
    // NegZ
    [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0]],
    // PosZ
    [[0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]],
];

/// UV direction mapping for one face. Given normalized element coordinates,
/// returns `(u, v)` in 0..1 with V growing down, so side textures stay upright
/// and positive-X/positive-Z faces are not mirrored.
#[must_use]
pub fn face_uv(face: FaceDir, x: f32, y: f32, z: f32) -> (f32, f32) {
    match face {
        FaceDir::NegZ => (1.0 - x, 1.0 - y),
        FaceDir::PosZ => (x, 1.0 - y),
        FaceDir::NegX => (z, 1.0 - y),
        FaceDir::PosX => (1.0 - z, 1.0 - y),
        FaceDir::PosY => (x, z),
        FaceDir::NegY => (x, 1.0 - z),
    }
}

/// Rotates a normalized face UV clockwise by `rotation` degrees.
#[must_use]
pub fn rotate_uv(rotation: u8, u: f32, v: f32) -> (f32, f32) {
    match (rotation / 90) % 4 {
        1 => (v, 1.0 - u),
        2 => (1.0 - u, 1.0 - v),
        3 => (1.0 - v, u),
        _ => (u, v),
    }
}

/// The render layer for a block name. This is deliberately name-based so it
/// can be used during culling without resolving full geometry.
#[must_use]
pub fn render_layer(name: &str) -> RenderLayer {
    let short = short_name(name);
    if is_water(short) || short == "ice" || short.contains("stained_glass") {
        return RenderLayer::Translucent;
    }
    if short.contains("leaves")
        || short == "glass"
        || short == "glass_pane"
        || short == "tinted_glass"
        || short == "cobweb"
        || short == "iron_bars"
        || short == "short_grass"
        || short == "grass"
        || short == "fern"
        || short == "large_fern"
        || short == "dead_bush"
        || short == "seagrass"
        || short == "tall_grass"
        || short == "sugar_cane"
        || short == "torch"
        || short == "wall_torch"
        || short == "soul_torch"
        || short == "soul_wall_torch"
        || short == "redstone_torch"
        || short == "redstone_wall_torch"
        || short == "lantern"
        || short == "soul_lantern"
        || short == "campfire"
        || short == "soul_campfire"
        || short.ends_with("_sapling")
        || short.ends_with("_flower")
        || is_flower(short)
    {
        return RenderLayer::Cutout;
    }
    RenderLayer::Opaque
}

/// Whether a state is a full opaque cube that can hide a neighbour face.
#[must_use]
pub fn occludes(state: &BlockState) -> bool {
    !state.is_air() && render_layer(&state.name) == RenderLayer::Opaque && is_full_cube(state)
}

/// Whether a state occupies the full 16x16x16 cell.
#[must_use]
pub fn is_full_cube(state: &BlockState) -> bool {
    if state.is_air() {
        return false;
    }
    let short = short_name(&state.name);
    if is_cross(short) {
        return false;
    }
    if short.ends_with("_slab") {
        return state.property("type") == Some("double");
    }
    if short.ends_with("_stairs")
        || short.ends_with("_fence")
        || short.ends_with("_fence_gate")
        || short.ends_with("_wall")
        || short.ends_with("_carpet")
        || short.ends_with("_button")
        || short.ends_with("_pressure_plate")
        || short.ends_with("_door")
        || short.ends_with("_trapdoor")
        || short.ends_with("_pane")
        || short.ends_with("_bars")
        || short.ends_with("_rail")
        || short.ends_with("_sign")
        || short.ends_with("_banner")
        || short.ends_with("_bed")
        || short.ends_with("_candle")
        || short.ends_with("_chain")
        || short.ends_with("_bud")
        || short.ends_with("_head")
        || short.ends_with("_skull")
        || short.ends_with("_pot")
        || short.ends_with("_composter")
        || short.ends_with("_cauldron")
        || short.ends_with("_hopper")
        || short.ends_with("_chest")
        || short == "snow"
        || short == "water"
        || short == "lava"
        || short == "ladder"
        || short == "vine"
        || short == "farmland"
        || short == "dirt_path"
        || short == "soul_sand"
        || short == "mud"
        || short == "scaffolding"
        || short == "cocoa"
    {
        return false;
    }
    true
}

/// Whether the built-in resolver has precise geometry for a state.
///
/// A `false` result does not mean the block is not drawn; it means the block
/// is drawn as a full cube because no built-in shape exists yet. The mesher
/// counts those so missing model coverage is measurable.
#[must_use]
pub fn is_approximate_fallback(state: &BlockState) -> bool {
    if is_full_cube(state) {
        return false;
    }
    let short = short_name(&state.name);
    !(is_cross(short)
        || short.ends_with("_slab")
        || short.ends_with("_stairs")
        || short == "snow"
        || short.ends_with("_carpet")
        || short == "moss_carpet"
        || short == "water"
        || short == "lava")
}

/// Resolves built-in geometry for a state.
#[must_use]
pub fn builtin_shape(state: &BlockState) -> ResolvedShape {
    let short = short_name(&state.name);
    let layer = render_layer(short);
    if is_cross(short) {
        return ResolvedShape {
            shape: cross_shape(short),
            layer,
        };
    }
    if short.ends_with("_slab") {
        return ResolvedShape {
            shape: slab_shape(state),
            layer,
        };
    }
    if short.ends_with("_stairs") {
        return ResolvedShape {
            shape: stairs_shape(state),
            layer,
        };
    }
    if short == "snow" {
        return ResolvedShape {
            shape: snow_shape(state),
            layer,
        };
    }
    if short.ends_with("_carpet") || short == "moss_carpet" {
        return ResolvedShape {
            shape: BlockShape::Elements(vec![Element::builtin(
                [0.0, 0.0, 0.0],
                [16.0, 1.0, 16.0],
                true,
            )]),
            layer,
        };
    }
    if short == "water" || short == "lava" {
        return ResolvedShape {
            shape: fluid_shape(state),
            layer,
        };
    }
    ResolvedShape {
        shape: BlockShape::full_cube(),
        layer,
    }
}

/// The material name used for built-in tiles: `oak_stairs` -> `oak_planks`,
/// `stone_slab` -> `stone`, `white_carpet` -> `white_wool`.
#[must_use]
pub fn material_name(name: &str) -> String {
    let short = short_name(name);
    let base = if let Some(base) = short.strip_suffix("_slab") {
        base
    } else if let Some(base) = short.strip_suffix("_stairs") {
        base
    } else if let Some(base) = short.strip_suffix("_carpet") {
        return match base {
            "white" | "orange" | "magenta" | "light_blue" | "yellow" | "lime" | "pink" | "gray"
            | "light_gray" | "cyan" | "purple" | "blue" | "brown" | "green" | "red" | "black" => {
                format!("{base}_wool")
            }
            _ => base.to_owned(),
        };
    } else if let Some(base) = short.strip_suffix("_fence") {
        base
    } else if let Some(base) = short.strip_suffix("_fence_gate") {
        base
    } else if let Some(base) = short.strip_suffix("_wall") {
        base
    } else if let Some(base) = short.strip_suffix("_door") {
        base
    } else if let Some(base) = short.strip_suffix("_trapdoor") {
        base
    } else if let Some(base) = short.strip_suffix("_button") {
        base
    } else if let Some(base) = short.strip_suffix("_pressure_plate") {
        base
    } else {
        short
    };
    if is_wood(base) {
        format!("{base}_planks")
    } else {
        base.to_owned()
    }
}

/// Shade for an arbitrary (rotated) face normal: the dominant axis decides.
#[must_use]
pub fn shade_for_normal(normal: Vec3) -> f32 {
    let x = normal.x.abs();
    let y = normal.y.abs();
    let z = normal.z.abs();
    if y >= x && y >= z {
        if normal.y >= 0.0 {
            1.0
        } else {
            0.5
        }
    } else if x >= z {
        0.6
    } else {
        0.8
    }
}

fn short_name(name: &str) -> &str {
    name.strip_prefix("minecraft:").unwrap_or(name)
}

/// Short block name without the `minecraft:` namespace. Crate-internal so the
/// resource-pack resolver and the mesher agree on names.
#[must_use]
pub fn block_short_name(name: &str) -> &str {
    short_name(name)
}

fn is_wood(name: &str) -> bool {
    matches!(
        name,
        "oak"
            | "spruce"
            | "birch"
            | "jungle"
            | "acacia"
            | "dark_oak"
            | "mangrove"
            | "cherry"
            | "bamboo"
            | "crimson"
            | "warped"
    )
}

fn is_water(short: &str) -> bool {
    short == "water" || short == "bubble_column"
}

fn is_flower(short: &str) -> bool {
    matches!(
        short,
        "dandelion"
            | "poppy"
            | "blue_orchid"
            | "allium"
            | "azure_bluet"
            | "red_tulip"
            | "orange_tulip"
            | "white_tulip"
            | "pink_tulip"
            | "oxeye_daisy"
            | "cornflower"
            | "lily_of_the_valley"
            | "wither_rose"
            | "sunflower"
            | "lilac"
            | "rose_bush"
            | "peony"
            | "torchflower"
    )
}

/// Whether a short block name uses a crossed-plant model.
#[must_use]
pub fn is_cross_block(short: &str) -> bool {
    is_cross(short)
}

fn is_cross(short: &str) -> bool {
    is_flower(short)
        || matches!(
            short,
            "short_grass"
                | "grass"
                | "fern"
                | "large_fern"
                | "dead_bush"
                | "seagrass"
                | "tall_grass"
                | "sugar_cane"
                | "torch"
                | "wall_torch"
                | "soul_torch"
                | "soul_wall_torch"
                | "redstone_torch"
                | "redstone_wall_torch"
                | "lantern"
                | "soul_lantern"
                | "campfire"
                | "soul_campfire"
        )
        || short.ends_with("_sapling")
        || short.ends_with("_flower")
}

/// Vanilla gives grass-like cross plants `tintindex: 0`, so they take the
/// grass colour; flowers, saplings, torches and lanterns are untinted.
fn is_grass_plant(short: &str) -> bool {
    matches!(
        short,
        "short_grass" | "grass" | "fern" | "large_fern" | "tall_grass" | "sugar_cane" | "seagrass"
    )
}

fn cross_shape(short: &str) -> BlockShape {
    let tint = if is_grass_plant(short) {
        TintKind::Grass
    } else {
        TintKind::None
    };
    BlockShape::Cross {
        texture: FaceTexture::BlockFace(FaceDir::PosY),
        tint,
    }
}

fn slab_shape(state: &BlockState) -> BlockShape {
    match state.property("type") {
        Some("double") => BlockShape::full_cube(),
        Some("top") => BlockShape::Elements(vec![Element::builtin(
            [0.0, 8.0, 0.0],
            [16.0, 16.0, 16.0],
            true,
        )]),
        _ => BlockShape::Elements(vec![Element::builtin(
            [0.0, 0.0, 0.0],
            [16.0, 8.0, 16.0],
            true,
        )]),
    }
}

fn stairs_shape(state: &BlockState) -> BlockShape {
    let top = state.property("half") == Some("top");
    let (base_from, base_to, step_from, step_to) = if top {
        (
            [0.0, 8.0, 0.0],
            [16.0, 16.0, 16.0],
            [0.0, 0.0, 0.0],
            [16.0, 8.0, 16.0],
        )
    } else {
        (
            [0.0, 0.0, 0.0],
            [16.0, 8.0, 16.0],
            [0.0, 8.0, 0.0],
            [16.0, 16.0, 16.0],
        )
    };
    let mut step_from = step_from;
    let mut step_to = step_to;
    match state.property("facing") {
        Some("east") => {
            step_from[0] = 8.0;
        }
        Some("west") => {
            step_to[0] = 8.0;
        }
        Some("south") => {
            step_from[2] = 8.0;
        }
        _ => {
            step_to[2] = 8.0;
        }
    }
    BlockShape::Elements(vec![
        Element::builtin(base_from, base_to, true),
        Element::builtin(step_from, step_to, true),
    ])
}

fn snow_shape(state: &BlockState) -> BlockShape {
    let layers = state
        .property("layers")
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(1)
        .clamp(1, 8);
    if layers >= 8 {
        return BlockShape::full_cube();
    }
    BlockShape::Elements(vec![Element::builtin(
        [0.0, 0.0, 0.0],
        [16.0, f32::from(layers) * 2.0, 16.0],
        true,
    )])
}

fn fluid_shape(state: &BlockState) -> BlockShape {
    let level = state
        .property("level")
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(0)
        .min(15);
    let height = f32::from(14_u8.saturating_sub(level.min(13))).max(1.0);
    BlockShape::Elements(vec![Element::builtin(
        [0.0, 0.0, 0.0],
        [16.0, height, 16.0],
        true,
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str) -> BlockState {
        BlockState::new(name)
    }

    #[test]
    fn full_cube_is_one_element_with_boundary_cullfaces() {
        let resolved = builtin_shape(&state("minecraft:stone"));
        let BlockShape::Elements(elements) = resolved.shape else {
            panic!("stone must be elements");
        };
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].from, [0.0; 3]);
        assert_eq!(elements[0].to, [16.0; 3]);
        for face in FaceDir::ALL {
            assert_eq!(
                elements[0].faces[face.index()].unwrap().cullface,
                Some(face)
            );
        }
    }

    #[test]
    fn slabs_and_stairs_are_partial_and_top_slabs_crop_uvs() {
        let bottom = builtin_shape(&BlockState::new("minecraft:oak_slab"));
        let BlockShape::Elements(elements) = bottom.shape else {
            panic!("slab must be elements");
        };
        assert_eq!(elements[0].to[1], 8.0);
        assert_eq!(
            elements[0].default_uv(FaceDir::NegZ),
            [0.0, 8.0, 16.0, 16.0]
        );
        assert_eq!(
            elements[0].default_uv(FaceDir::PosY),
            [0.0, 0.0, 16.0, 16.0]
        );

        let top =
            builtin_shape(&BlockState::new("minecraft:oak_slab").with_property("type", "top"));
        let BlockShape::Elements(elements) = top.shape else {
            panic!("top slab must be elements");
        };
        assert_eq!(elements[0].from[1], 8.0);
        assert_eq!(elements[0].default_uv(FaceDir::NegZ), [0.0, 0.0, 16.0, 8.0]);
        assert!(!elements[0].on_boundary(FaceDir::NegY));

        let stairs = builtin_shape(&BlockState::new("minecraft:oak_stairs"));
        let BlockShape::Elements(elements) = stairs.shape else {
            panic!("stairs must be elements");
        };
        assert_eq!(elements.len(), 2);
        assert!(!is_full_cube(&state("minecraft:oak_stairs")));
    }

    #[test]
    fn snow_carpet_and_fluid_have_real_heights() {
        let snow = builtin_shape(&BlockState::new("minecraft:snow").with_property("layers", "2"));
        let BlockShape::Elements(elements) = snow.shape else {
            panic!("snow must be elements");
        };
        assert_eq!(elements[0].to[1], 4.0);
        let carpet = builtin_shape(&state("minecraft:white_carpet"));
        let BlockShape::Elements(elements) = carpet.shape else {
            panic!("carpet must be elements");
        };
        assert_eq!(elements[0].to[1], 1.0);
        let water = builtin_shape(&state("minecraft:water"));
        let BlockShape::Elements(elements) = water.shape else {
            panic!("water must be elements");
        };
        assert!(elements[0].to[1] < 16.0);
        assert_eq!(render_layer("minecraft:water"), RenderLayer::Translucent);
    }

    #[test]
    fn plants_use_crossed_quads_and_cutout() {
        let resolved = builtin_shape(&state("minecraft:short_grass"));
        assert!(matches!(resolved.shape, BlockShape::Cross { .. }));
        assert_eq!(resolved.layer, RenderLayer::Cutout);
        assert!(!is_full_cube(&state("minecraft:poppy")));
    }

    #[test]
    fn culling_only_treats_full_opaque_cubes_as_occluders() {
        assert!(occludes(&state("minecraft:stone")));
        assert!(!occludes(&state("minecraft:glass")));
        assert!(!occludes(&state("minecraft:oak_leaves")));
        assert!(!occludes(&state("minecraft:oak_slab")));
        assert!(!occludes(&state("minecraft:water")));
    }

    #[test]
    fn material_names_map_shapes_back_to_textures() {
        assert_eq!(material_name("minecraft:oak_stairs"), "oak_planks");
        assert_eq!(material_name("minecraft:stone_slab"), "stone");
        assert_eq!(material_name("minecraft:white_carpet"), "white_wool");
        assert_eq!(material_name("minecraft:cobblestone_stairs"), "cobblestone");
    }
}
