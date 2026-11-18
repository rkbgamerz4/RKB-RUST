//! Block meshing with model elements, atlas tiles, face shading, world light,
//! and ambient occlusion.
//!
//! Minecraft textures are not bundled. Built-in blocks use the procedural atlas
//! from [`crate::textures`]; resource-pack models supply atlas texture ids
//! through [`crate::model`]. The mesher never assumes that a block is a full
//! cube: it walks resolved elements, so slabs, stairs, snow layers, carpets,
//! fluids, and crossed plants use the same code path as a cube.
//!
//! Face culling is shape-aware. A neighbour only hides a face when it is a full
//! opaque cube (or the same full cutout/translucent block). Leaves, glass,
//! water, and partial shapes therefore stop deleting the faces behind them.
//!
//! Lighting uses real chunk light when the world source provides it. Save and
//! generated worlds return a documented full-sky fallback and the game reports
//! that fallback instead of presenting it as server light.

use std::collections::BTreeMap;
use std::sync::Arc;

use rkb_core::Vec3;
use rkb_mc::{BlockState, Chunk};
use rkb_render::Color;

use crate::blocks::{block_color, is_solid};
use crate::model::{
    face_uv, is_full_cube, material_name, occludes, render_layer, rotate_uv, shade_for_normal,
    BlockModelSource, BlockShape, BuiltinModels, Element, ElementFace, FaceTexture, RenderLayer,
    TintKind, FACE_CORNERS,
};
use crate::textures::{block_tile, FaceDir, Tile};
use crate::world_source::{ChunkSource, LightSample};
use crate::worldgen::{WORLD_MAX_Y, WORLD_MIN_Y};

/// Absolute block edits layered over a world source.
pub type EditMap = BTreeMap<(i32, i32, i32), BlockState>;

/// One outward-facing block face in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Face {
    /// World-space corners, counter-clockwise when viewed from outside.
    pub points: [Vec3; 4],
    pub normal: Vec3,
    /// Active-atlas texture id.
    pub texture: u32,
    /// Multiplied over the tile texels (`white` for tiles that already hold colour).
    pub tint: Color,
    /// Minecraft directional shading for this face direction.
    pub shade: f32,
    /// Tile-local UVs in `[0, 1]`, one per corner.
    pub uv: [[f32; 2]; 4],
    /// Per-corner light: ambient occlusion multiplied by face shade and world
    /// light. Already in display range (0..1.5).
    pub light: [f32; 4],
    /// Flat fallback colour for backends without an atlas.
    pub color: Color,
    /// Compositing layer.
    pub layer: RenderLayer,
    /// Crossed plants are visible from both sides.
    pub double_sided: bool,
}

impl Face {
    #[must_use]
    pub fn center(self) -> Vec3 {
        let sum = self.points[0] + self.points[1] + self.points[2] + self.points[3];
        sum * 0.25
    }

    #[must_use]
    pub fn is_visible_from(self, eye: Vec3) -> bool {
        self.double_sided || self.normal.dot(eye - self.center()) > 0.0
    }

    /// The procedural tile when this face uses the built-in atlas, for tests
    /// and diagnostics.
    #[must_use]
    pub fn builtin_tile(self) -> Option<Tile> {
        Tile::ALL
            .into_iter()
            .find(|tile| tile.index() as u32 == self.texture)
    }
}

/// Meshed geometry for one chunk column, split by render layer.
#[derive(Clone, Debug, Default)]
pub struct ChunkMesh {
    pub chunk: (i32, i32),
    pub opaque: Vec<Face>,
    pub cutout: Vec<Face>,
    pub translucent: Vec<Face>,
    pub min: Vec3,
    pub max: Vec3,
    pub non_air_blocks: usize,
    /// Blocks that had no precise model and used the full-cube fallback.
    pub missing_models: usize,
}

impl ChunkMesh {
    #[must_use]
    pub fn face_count(&self) -> usize {
        self.opaque.len() + self.cutout.len() + self.translucent.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.face_count() == 0
    }

    /// All faces, opaque first, then cutout, then translucent.
    pub fn faces(&self) -> impl Iterator<Item = &Face> {
        self.opaque
            .iter()
            .chain(self.cutout.iter())
            .chain(self.translucent.iter())
    }

    #[must_use]
    pub fn layer(&self, layer: RenderLayer) -> &[Face] {
        match layer {
            RenderLayer::Opaque => &self.opaque,
            RenderLayer::Cutout => &self.cutout,
            RenderLayer::Translucent => &self.translucent,
        }
    }

    fn layer_mut(&mut self, layer: RenderLayer) -> &mut Vec<Face> {
        match layer {
            RenderLayer::Opaque => &mut self.opaque,
            RenderLayer::Cutout => &mut self.cutout,
            RenderLayer::Translucent => &mut self.translucent,
        }
    }
}

type Neighborhood = [Option<Arc<Chunk>>; 9];

/// A 3x3x3 view of blocks and light around one block, fetched once per block.
struct LocalView {
    states: [[[Option<BlockState>; 3]; 3]; 3],
    light: [[[LightSample; 3]; 3]; 3],
}

impl LocalView {
    fn state(&self, dx: i32, dy: i32, dz: i32) -> Option<&BlockState> {
        self.states[(dx + 1) as usize][(dy + 1) as usize][(dz + 1) as usize].as_ref()
    }

    fn light(&self, dx: i32, dy: i32, dz: i32) -> LightSample {
        self.light[(dx + 1) as usize][(dy + 1) as usize][(dz + 1) as usize]
    }
}

fn neighbor_index(cx: i32, cz: i32, wx: i32, wz: i32) -> Option<usize> {
    let dx = wx.div_euclid(16) - cx;
    let dz = wz.div_euclid(16) - cz;
    if !(-1..=1).contains(&dx) || !(-1..=1).contains(&dz) {
        return None;
    }
    Some(((dz + 1) * 3 + (dx + 1)) as usize)
}

fn block_at_abs(
    neighbors: &Neighborhood,
    edits: &EditMap,
    cx: i32,
    cz: i32,
    x: i32,
    y: i32,
    z: i32,
) -> Option<BlockState> {
    if let Some(state) = edits.get(&(x, y, z)) {
        return Some(state.clone());
    }
    let index = neighbor_index(cx, cz, x, z)?;
    let chunk = neighbors[index].as_ref()?;
    chunk
        .block_at(x.rem_euclid(16), y, z.rem_euclid(16))
        .cloned()
}

/// Meshes one chunk. `Ok(None)` means the source has no chunk at that slot.
///
/// Cross-chunk boundaries use the surrounding 3x3 chunk neighbourhood so no
/// internal wall is drawn at a chunk edge, and so light is sampled from the
/// neighbouring column when it is loaded.
pub fn build_chunk_mesh(
    source: &mut dyn ChunkSource,
    cx: i32,
    cz: i32,
    edits: &EditMap,
) -> Result<Option<ChunkMesh>, String> {
    build_chunk_mesh_with(source, cx, cz, edits, &BuiltinModels)
}

/// Meshes one chunk with an explicit model source (a resource pack or the
/// built-in shapes).
pub fn build_chunk_mesh_with(
    source: &mut dyn ChunkSource,
    cx: i32,
    cz: i32,
    edits: &EditMap,
    models: &dyn BlockModelSource,
) -> Result<Option<ChunkMesh>, String> {
    let Some(chunk) = source.chunk(cx, cz)? else {
        return Ok(None);
    };
    let mut neighbors: Neighborhood = Default::default();
    for dz in -1..=1 {
        for dx in -1..=1 {
            let index = ((dz + 1) * 3 + (dx + 1)) as usize;
            neighbors[index] = source.chunk(cx + dx, cz + dz)?;
        }
    }

    let mut mesh = ChunkMesh {
        chunk: (cx, cz),
        ..ChunkMesh::default()
    };

    for section in chunk.sections() {
        if section.palette().iter().all(BlockState::is_air) {
            continue;
        }
        let section_y = i32::from(section.y) * 16;
        for y in 0..16 {
            for z in 0..16 {
                for x in 0..16 {
                    let Some(state) = section.block_at(x, y, z) else {
                        continue;
                    };
                    let wx = cx * 16 + x as i32;
                    let wy = section_y + y as i32;
                    let wz = cz * 16 + z as i32;
                    let effective = if edits.is_empty() {
                        state.clone()
                    } else {
                        edits
                            .get(&(wx, wy, wz))
                            .cloned()
                            .unwrap_or_else(|| state.clone())
                    };
                    if effective.is_air() {
                        continue;
                    }
                    mesh.non_air_blocks += 1;
                    emit_block(
                        &mut mesh, source, models, &neighbors, edits, cx, cz, wx, wy, wz,
                        &effective,
                    )?;
                }
            }
        }
    }

    // Edits can add blocks above the highest saved section. Those cells have
    // no section to iterate, so emit them directly.
    for (&(wx, wy, wz), state) in edits {
        if wx.div_euclid(16) != cx || wz.div_euclid(16) != cz || state.is_air() {
            continue;
        }
        let section_y = i8::try_from(wy.div_euclid(16)).ok();
        if section_y.is_some_and(|y| chunk.section(y).is_some()) {
            continue;
        }
        if !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&wy) {
            continue;
        }
        mesh.non_air_blocks += 1;
        emit_block(
            &mut mesh, source, models, &neighbors, edits, cx, cz, wx, wy, wz, state,
        )?;
    }

    if mesh.is_empty() {
        mesh.min = Vec3::ZERO;
        mesh.max = Vec3::ZERO;
    } else {
        let faces: Vec<Face> = mesh.faces().copied().collect();
        let (min, max) = mesh_bounds(&faces);
        mesh.min = min;
        mesh.max = max;
    }
    Ok(Some(mesh))
}

#[allow(clippy::too_many_arguments)]
fn emit_block(
    mesh: &mut ChunkMesh,
    source: &mut dyn ChunkSource,
    models: &dyn BlockModelSource,
    neighbors: &Neighborhood,
    edits: &EditMap,
    cx: i32,
    cz: i32,
    wx: i32,
    wy: i32,
    wz: i32,
    state: &BlockState,
) -> Result<(), String> {
    let resolved = models.resolve(state);
    if models.approximate(state) {
        mesh.missing_models += 1;
    }
    let view = local_view(source, neighbors, edits, cx, cz, wx, wy, wz)?;
    match &resolved.shape {
        BlockShape::Elements(elements) => {
            for element in elements {
                emit_element(mesh, state, resolved.layer, element, &view, wx, wy, wz);
            }
        }
        BlockShape::Cross { texture, tint } => {
            emit_cross(mesh, state, *texture, *tint, &view, wx, wy, wz);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn local_view(
    source: &mut dyn ChunkSource,
    neighbors: &Neighborhood,
    edits: &EditMap,
    cx: i32,
    cz: i32,
    wx: i32,
    wy: i32,
    wz: i32,
) -> Result<LocalView, String> {
    let mut states: [[[Option<BlockState>; 3]; 3]; 3] = Default::default();
    let mut light: [[[LightSample; 3]; 3]; 3] = Default::default();
    for dy in -1..=1 {
        for dz in -1..=1 {
            for dx in -1..=1 {
                let x = wx + dx;
                let y = wy + dy;
                let z = wz + dz;
                let state = block_at_abs(neighbors, edits, cx, cz, x, y, z);
                let sample = source.light_at(x, y, z)?;
                states[(dx + 1) as usize][(dy + 1) as usize][(dz + 1) as usize] = state;
                light[(dx + 1) as usize][(dy + 1) as usize][(dz + 1) as usize] = sample;
            }
        }
    }
    Ok(LocalView { states, light })
}

#[allow(clippy::too_many_arguments)]
fn emit_element(
    mesh: &mut ChunkMesh,
    state: &BlockState,
    layer: RenderLayer,
    element: &Element,
    view: &LocalView,
    wx: i32,
    wy: i32,
    wz: i32,
) {
    let origin = Vec3::new(wx as f32, wy as f32, wz as f32);
    let full_cube = element.from == [0.0; 3] && element.to == [16.0; 3];
    let same_full_cutout = |neighbor: &BlockState| {
        neighbor.name == state.name
            && layer != RenderLayer::Opaque
            && is_full_cube(state)
            && is_full_cube(neighbor)
    };

    for face in FaceDir::ALL {
        let Some(element_face) = element.faces[face.index()] else {
            continue;
        };
        if element.transform.is_none() {
            if let Some(cullface) = element_face.cullface {
                let (dx, dy, dz) = cullface.offset();
                if let Some(neighbor) = view.state(dx, dy, dz) {
                    if occludes(neighbor) || same_full_cutout(neighbor) {
                        continue;
                    }
                }
            }
        }

        let mut points = [Vec3::ZERO; 4];
        let mut uv = [[0.0_f32; 2]; 4];
        let uv_rect = element_face.uv.unwrap_or_else(|| element.default_uv(face));
        let corner_specs = FACE_CORNERS[face.index()];
        let from = element.from;
        let to = element.to;
        let extent = [
            (to[0] - from[0]).max(f32::EPSILON),
            (to[1] - from[1]).max(f32::EPSILON),
            (to[2] - from[2]).max(f32::EPSILON),
        ];
        for (index, spec) in corner_specs.iter().enumerate() {
            let corner = [
                if spec[0] == 0 { from[0] } else { to[0] },
                if spec[1] == 0 { from[1] } else { to[1] },
                if spec[2] == 0 { from[2] } else { to[2] },
            ];
            let corner = match element.transform {
                Some(transform) => transform.point(corner),
                None => corner,
            };
            points[index] =
                origin + Vec3::new(corner[0] / 16.0, corner[1] / 16.0, corner[2] / 16.0);
            // UVs stay in untransformed model space, matching vanilla models
            // whose texture rotation happens before the element rotation.
            let raw = [
                if spec[0] == 0 { from[0] } else { to[0] },
                if spec[1] == 0 { from[1] } else { to[1] },
                if spec[2] == 0 { from[2] } else { to[2] },
            ];
            let local = [
                (raw[0] - from[0]) / extent[0],
                (raw[1] - from[1]) / extent[1],
                (raw[2] - from[2]) / extent[2],
            ];
            let (u, v) = face_uv(face, local[0], local[1], local[2]);
            let (u, v) = rotate_uv(element_face.rotation, u, v);
            uv[index] = [
                (uv_rect[0] + u * (uv_rect[2] - uv_rect[0])) / 16.0,
                (uv_rect[1] + v * (uv_rect[3] - uv_rect[1])) / 16.0,
            ];
        }

        let normal = match element.transform {
            Some(transform) => transform.direction(face.normal()),
            None => face.normal(),
        };
        let shade = match element.transform {
            Some(_) => shade_for_normal(normal),
            None => face.shade(),
        };
        let (ao, brightness) = if full_cube && element.transform.is_none() {
            sample_ao_light(view, face, corner_specs)
        } else {
            let sample = view.light(0, 0, 0);
            let brightness = light_curve(sample.sky.max(sample.block));
            ([1.0; 4], [brightness; 4])
        };
        let mut light = [0.0_f32; 4];
        for index in 0..4 {
            light[index] = shade * ao[index] * brightness[index];
        }

        let (texture, tint) = face_texture(state, &element_face);
        let face = Face {
            points,
            normal,
            texture,
            tint,
            shade,
            uv,
            light,
            color: block_color(&state.name).shaded(shade),
            layer,
            double_sided: element.double_sided,
        };
        mesh.layer_mut(layer).push(face);
    }
}

fn face_texture(state: &BlockState, element_face: &ElementFace) -> (u32, Color) {
    match element_face.texture {
        FaceTexture::BlockFace(face) => {
            let material = material_name(&state.name);
            let (tile, tint) = block_tile(&material, face);
            (tile.index() as u32, tint)
        }
        FaceTexture::Atlas(id) => (id, tint_color(element_face.tint)),
    }
}

/// Approximated biome tints. These are documented fallbacks until biome data is
/// decoded; they are the values Minecraft uses for the plains biome.
#[must_use]
pub fn tint_color(tint: TintKind) -> Color {
    match tint {
        TintKind::None => Color::WHITE,
        TintKind::Grass => Color::rgb(145, 189, 89),
        TintKind::Foliage => Color::rgb(119, 171, 47),
        TintKind::Water => Color::rgb(63, 118, 228),
    }
}

/// Light-map approximation: 0.03 at level 0, 1.0 at level 15.
#[must_use]
pub fn light_curve(level: u8) -> f32 {
    let normalized = f32::from(level.min(15)) / 15.0;
    0.03 + 0.97 * normalized.powf(1.4)
}

fn sample_ao_light(
    view: &LocalView,
    face: FaceDir,
    corner_specs: [[u8; 3]; 4],
) -> ([f32; 4], [f32; 4]) {
    let (nx, ny, nz) = face.offset();
    let tangent_axes: [usize; 2] = match face {
        FaceDir::NegX | FaceDir::PosX => [1, 2],
        FaceDir::NegY | FaceDir::PosY => [0, 2],
        FaceDir::NegZ | FaceDir::PosZ => [0, 1],
    };
    let mut ao = [1.0_f32; 4];
    let mut brightness = [1.0_f32; 4];
    for (index, spec) in corner_specs.iter().enumerate() {
        let along_a = if spec[tangent_axes[0]] == 0 { -1 } else { 1 };
        let along_b = if spec[tangent_axes[1]] == 0 { -1 } else { 1 };
        let mut a_offset = [0, 0, 0];
        a_offset[tangent_axes[0]] = along_a;
        let mut b_offset = [0, 0, 0];
        b_offset[tangent_axes[1]] = along_b;
        let mut diagonal_offset = [0, 0, 0];
        diagonal_offset[tangent_axes[0]] = along_a;
        diagonal_offset[tangent_axes[1]] = along_b;

        let base = view.state(nx, ny, nz);
        let side_a = view.state(nx + a_offset[0], ny + a_offset[1], nz + a_offset[2]);
        let side_b = view.state(nx + b_offset[0], ny + b_offset[1], nz + b_offset[2]);
        let diagonal = view.state(
            nx + diagonal_offset[0],
            ny + diagonal_offset[1],
            nz + diagonal_offset[2],
        );
        let a = side_a.is_some_and(occludes);
        let b = side_b.is_some_and(occludes);
        let c = diagonal.is_some_and(occludes);
        ao[index] = if a && b {
            0.6
        } else {
            1.0 - 0.2 * u8::from(a) as f32 - 0.2 * u8::from(b) as f32 - 0.2 * u8::from(c) as f32
        };

        let mut sky = 0_u32;
        let mut block = 0_u32;
        let mut count = 0_u32;
        let mut add = |state: Option<&BlockState>, sample: LightSample| {
            if state.is_some_and(occludes) {
                return;
            }
            sky += u32::from(sample.sky);
            block += u32::from(sample.block);
            count += 1;
        };
        add(base, view.light(nx, ny, nz));
        add(
            side_a,
            view.light(nx + a_offset[0], ny + a_offset[1], nz + a_offset[2]),
        );
        add(
            side_b,
            view.light(nx + b_offset[0], ny + b_offset[1], nz + b_offset[2]),
        );
        add(
            diagonal,
            view.light(
                nx + diagonal_offset[0],
                ny + diagonal_offset[1],
                nz + diagonal_offset[2],
            ),
        );
        let count = count.max(1);
        let sky = u8::try_from(sky / count).unwrap_or(0);
        let block = u8::try_from(block / count).unwrap_or(0);
        brightness[index] = light_curve(sky.max(block));
    }
    (ao, brightness)
}

#[allow(clippy::too_many_arguments)]
fn emit_cross(
    mesh: &mut ChunkMesh,
    state: &BlockState,
    texture: FaceTexture,
    tint: TintKind,
    view: &LocalView,
    wx: i32,
    wy: i32,
    wz: i32,
) {
    let origin = Vec3::new(wx as f32, wy as f32, wz as f32);
    let (texture_id, color) = match texture {
        FaceTexture::BlockFace(face) => {
            let (tile, tint) = block_tile(&state.name, face);
            (tile.index() as u32, tint)
        }
        FaceTexture::Atlas(id) => (id, tint_color(tint)),
    };
    let sample = view.light(0, 0, 0);
    let brightness = light_curve(sample.sky.max(sample.block));
    let quads: [([Vec3; 4], Vec3); 2] = [
        (
            [
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 1.0),
                Vec3::new(1.0, 1.0, 1.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
            Vec3::new(-0.707_106_77, 0.0, 0.707_106_77),
        ),
        (
            [
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
                Vec3::new(0.0, 1.0, 1.0),
                Vec3::new(1.0, 1.0, 0.0),
            ],
            Vec3::new(-0.707_106_77, 0.0, -0.707_106_77),
        ),
    ];
    let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    for (points, normal) in quads {
        mesh.layer_mut(RenderLayer::Cutout).push(Face {
            points: points.map(|point| origin + point),
            normal,
            texture: texture_id,
            tint: color,
            shade: 1.0,
            uv,
            light: [brightness; 4],
            color: block_color(&state.name),
            layer: RenderLayer::Cutout,
            double_sided: true,
        });
    }
}

/// World-space bounds used for frustum culling.
#[must_use]
pub fn mesh_bounds(faces: &[Face]) -> (Vec3, Vec3) {
    if faces.is_empty() {
        return (Vec3::ZERO, Vec3::ZERO);
    }
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for face in faces {
        for point in face.points {
            min.x = min.x.min(point.x);
            min.y = min.y.min(point.y);
            min.z = min.z.min(point.z);
            max.x = max.x.max(point.x);
            max.y = max.y.max(point.y);
            max.z = max.z.max(point.z);
        }
    }
    (min, max)
}

/// Whether a block can be stood on; liquids and plants are not solid.
#[must_use]
pub fn is_solid_block(state: &BlockState) -> bool {
    is_solid(&state.name)
}

/// Whether a face should be culled against one neighbour state. Exposed for
/// tests and for resource-pack model baking.
#[must_use]
pub fn cull_against(state: &BlockState, neighbor: &BlockState) -> bool {
    occludes(neighbor)
        || (neighbor.name == state.name
            && render_layer(&state.name) != RenderLayer::Opaque
            && is_full_cube(state)
            && is_full_cube(neighbor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rkb_mc::chunk::SECTION_VOLUME;
    use rkb_mc::fixture::{chunk_tag, level_tag, section_tag, write_world, TempDir};
    use rkb_mc::World;

    fn stone_section() -> rkb_mc::Tag {
        section_tag(
            3,
            &[BlockState::new("minecraft:stone")],
            &vec![0_u16; SECTION_VOLUME],
        )
    }

    #[test]
    fn meshes_a_chunk_at_real_world_coordinates() {
        let dir = TempDir::new("mesh-single");
        let chunks = [(0, 0, chunk_tag(3953, 0, 0, vec![stone_section()]))];
        write_world(dir.path(), &level_tag(3953, "Mesh"), &chunks).unwrap();
        let mut world = World::open(dir.path()).unwrap();
        let mesh = build_chunk_mesh(&mut world, 0, 0, &EditMap::new())
            .unwrap()
            .unwrap();

        // A lone solid chunk exposes all six outer walls: 6 * 16 * 16 quads.
        assert_eq!(mesh.face_count(), 6 * 256);
        assert_eq!(mesh.min, Vec3::new(0.0, 48.0, 0.0));
        assert_eq!(mesh.max, Vec3::new(16.0, 64.0, 16.0));
        assert_eq!(mesh.non_air_blocks, SECTION_VOLUME);
        assert_eq!(mesh.missing_models, 0);
    }

    #[test]
    fn culls_internal_faces_between_loaded_neighbours() {
        let dir = TempDir::new("mesh-neighbours");
        let chunks = [
            (0, 0, chunk_tag(3953, 0, 0, vec![stone_section()])),
            (1, 0, chunk_tag(3953, 1, 0, vec![stone_section()])),
        ];
        write_world(dir.path(), &level_tag(3953, "Mesh"), &chunks).unwrap();
        let mut world = World::open(dir.path()).unwrap();
        let mesh = build_chunk_mesh(&mut world, 0, 0, &EditMap::new())
            .unwrap()
            .unwrap();

        let internal = mesh
            .faces()
            .any(|face| face.normal.x.abs() > 0.9 && (face.center().x - 16.0).abs() < 0.001);
        assert!(!internal, "shared boundary still has a wall");
        assert_eq!(mesh.face_count(), 5 * 256);
        assert_eq!(mesh.max, Vec3::new(16.0, 64.0, 16.0));
    }

    #[test]
    fn missing_neighbour_still_emits_the_chunk_wall() {
        let dir = TempDir::new("mesh-missing");
        let chunks = [(0, 0, chunk_tag(3953, 0, 0, vec![stone_section()]))];
        write_world(dir.path(), &level_tag(3953, "Mesh"), &chunks).unwrap();
        let mut world = World::open(dir.path()).unwrap();
        let mesh = build_chunk_mesh(&mut world, 0, 0, &EditMap::new())
            .unwrap()
            .unwrap();
        let wall = mesh
            .faces()
            .filter(|face| face.normal.x.abs() > 0.9)
            .count();
        assert_eq!(wall, 2 * 256);
    }

    #[test]
    fn faces_carry_upright_atlas_uvs_and_face_shading() {
        let dir = TempDir::new("mesh-uv");
        let chunks = [(0, 0, chunk_tag(3953, 0, 0, vec![stone_section()]))];
        write_world(dir.path(), &level_tag(3953, "Mesh"), &chunks).unwrap();
        let mut world = World::open(dir.path()).unwrap();
        let mesh = build_chunk_mesh(&mut world, 0, 0, &EditMap::new())
            .unwrap()
            .unwrap();

        let top = mesh
            .faces()
            .find(|face| face.normal.y > 0.9)
            .expect("top face");
        assert_eq!(top.builtin_tile(), Some(Tile::Stone));
        assert_eq!(top.shade, 1.0);
        assert_eq!(top.light, [1.0; 4]);

        // Side textures are upright: the highest corner samples the top of the
        // tile (V = 0) and the lowest corner samples the bottom (V = 1).
        for face in mesh.faces().filter(|face| face.normal.y.abs() < 0.5) {
            let expected_shade = if face.normal.x.abs() > 0.5 { 0.6 } else { 0.8 };
            assert_eq!(face.builtin_tile(), Some(Tile::Stone));
            assert_eq!(face.shade, expected_shade);
            let top = face
                .points
                .iter()
                .map(|point| point.y)
                .fold(f32::MIN, f32::max);
            let bottom = face
                .points
                .iter()
                .map(|point| point.y)
                .fold(f32::MAX, f32::min);
            for (index, point) in face.points.iter().enumerate() {
                let expected_v = if (point.y - top).abs() < 0.001 {
                    0.0
                } else {
                    assert!((point.y - bottom).abs() < 0.001);
                    1.0
                };
                assert_eq!(face.uv[index][1], expected_v);
                assert!((0.0..=1.0).contains(&face.uv[index][0]));
            }
        }
    }

    #[test]
    fn positive_x_and_z_faces_are_not_mirrored() {
        let dir = TempDir::new("mesh-uv-orientation");
        let chunks = [(0, 0, chunk_tag(3953, 0, 0, vec![stone_section()]))];
        write_world(dir.path(), &level_tag(3953, "Mesh"), &chunks).unwrap();
        let mut world = World::open(dir.path()).unwrap();
        let mesh = build_chunk_mesh(&mut world, 0, 0, &EditMap::new())
            .unwrap()
            .unwrap();
        for face in mesh.faces() {
            if face.normal.z > 0.9 {
                // South face: the texture's U runs from -X to +X.
                let min_x = face
                    .points
                    .iter()
                    .map(|point| point.x)
                    .fold(f32::MAX, f32::min);
                for (index, point) in face.points.iter().enumerate() {
                    if (point.x - min_x).abs() < 0.001 {
                        assert_eq!(face.uv[index][0], 0.0);
                    }
                }
            }
            if face.normal.x > 0.9 {
                // East face: the texture's U runs from +Z to -Z.
                let max_z = face
                    .points
                    .iter()
                    .map(|point| point.z)
                    .fold(f32::MIN, f32::max);
                for (index, point) in face.points.iter().enumerate() {
                    if (point.z - max_z).abs() < 0.001 {
                        assert_eq!(face.uv[index][0], 0.0);
                    }
                }
            }
        }
    }

    #[test]
    fn ambient_occlusion_darkens_corners_next_to_a_neighbour() {
        let dir = TempDir::new("mesh-ao");
        let chunks = [(0, 0, chunk_tag(3953, 0, 0, vec![stone_section()]))];
        write_world(dir.path(), &level_tag(3953, "Mesh"), &chunks).unwrap();
        let mut world = World::open(dir.path()).unwrap();

        // A block sitting on top of the surface shades the corners of the
        // neighbouring top face that touch it.
        let mut edits = EditMap::new();
        edits.insert((8, 64, 8), BlockState::new("minecraft:oak_planks"));
        let mesh = build_chunk_mesh(&mut world, 0, 0, &edits).unwrap().unwrap();
        let face = mesh
            .faces()
            .find(|face| {
                face.normal.y > 0.9
                    && (face.center().x - 9.5).abs() < 0.01
                    && (face.center().z - 8.5).abs() < 0.01
            })
            .expect("top face beside the placed block");

        let occluded = face
            .points
            .iter()
            .zip(face.light)
            .filter(|(point, _)| (point.x - 9.0).abs() < 0.01)
            .all(|(_, light)| (light - 0.8).abs() < 0.001);
        assert!(occluded, "occluded corners: {:?}", face.light);
        assert!(
            face.light.iter().any(|light| (light - 1.0).abs() < 0.001),
            "unoccluded corners keep full light: {:?}",
            face.light
        );
    }

    #[test]
    fn edits_change_meshed_geometry_and_block_counts() {
        let dir = TempDir::new("mesh-edits");
        let chunks = [(0, 0, chunk_tag(3953, 0, 0, vec![stone_section()]))];
        write_world(dir.path(), &level_tag(3953, "Mesh"), &chunks).unwrap();
        let mut world = World::open(dir.path()).unwrap();
        let base = build_chunk_mesh(&mut world, 0, 0, &EditMap::new())
            .unwrap()
            .unwrap();

        let mut edits = EditMap::new();
        edits.insert((8, 64, 8), BlockState::new("minecraft:oak_planks"));
        let placed = build_chunk_mesh(&mut world, 0, 0, &edits).unwrap().unwrap();
        assert_eq!(placed.face_count(), base.face_count() + 4);
        assert_eq!(placed.non_air_blocks, base.non_air_blocks + 1);
        let base_colour = base.faces().next().unwrap().color;
        assert!(placed.faces().any(|face| face.color != base_colour));

        edits.insert((8, 64, 8), BlockState::air());
        let removed = build_chunk_mesh(&mut world, 0, 0, &edits).unwrap().unwrap();
        assert_eq!(removed.face_count(), base.face_count());
        assert_eq!(removed.non_air_blocks, base.non_air_blocks);
    }

    #[test]
    fn shapes_emit_partial_geometry_and_plants_use_cutout() {
        let dir = TempDir::new("mesh-shapes");
        let chunks = [(0, 0, chunk_tag(3953, 0, 0, vec![stone_section()]))];
        write_world(dir.path(), &level_tag(3953, "Mesh"), &chunks).unwrap();
        let mut world = World::open(dir.path()).unwrap();
        let mut edits = EditMap::new();
        // Put a slab and a plant above the stone surface.
        edits.insert(
            (4, 64, 4),
            BlockState::new("minecraft:oak_slab").with_property("type", "bottom"),
        );
        edits.insert((6, 64, 6), BlockState::new("minecraft:short_grass"));
        edits.insert(
            (8, 64, 8),
            BlockState::new("minecraft:water").with_property("level", "0"),
        );
        let mesh = build_chunk_mesh(&mut world, 0, 0, &edits).unwrap().unwrap();
        assert!(mesh
            .opaque
            .iter()
            .any(|face| (face.max_y() - 64.5).abs() < 0.01));
        assert_eq!(
            mesh.cutout.iter().filter(|face| face.double_sided).count(),
            2
        );
        assert!(!mesh.translucent.is_empty());
    }

    impl Face {
        fn max_y(self) -> f32 {
            self.points
                .iter()
                .map(|point| point.y)
                .fold(f32::MIN, f32::max)
        }
    }
}
