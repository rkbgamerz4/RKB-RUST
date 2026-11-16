//! Block meshing with atlas tiles, face shading, and ambient occlusion.
//!
//! Minecraft textures are not bundled. Faces point at tiles in the procedural
//! atlas from [`crate::textures`]: the mesher picks the tile and tint for each
//! block name and face direction, computes upright UVs, and bakes Minecraft's
//! directional face shade together with per-vertex ambient occlusion into the
//! face. The mesher itself stays renderer-agnostic, so the world/save paths do
//! not depend on texture details.

use std::collections::BTreeMap;
use std::sync::Arc;

use rkb_core::Vec3;
use rkb_mc::{BlockState, Chunk};
use rkb_render::Color;

use crate::blocks::{block_color, is_solid};
use crate::textures::{block_tile, FaceDir, Tile};
use crate::world_source::ChunkSource;
use crate::worldgen::{WORLD_MAX_Y, WORLD_MIN_Y};

/// Absolute block edits layered over a world source.
pub type EditMap = BTreeMap<(i32, i32, i32), BlockState>;

/// One outward-facing block face in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Face {
    /// World-space corners, counter-clockwise when viewed from outside.
    pub points: [Vec3; 4],
    pub normal: Vec3,
    /// Flat fallback colour (texture tint ignored) for backends without an atlas.
    pub color: Color,
    /// Atlas tile for this block face.
    pub tile: Tile,
    /// Multiplied over the tile texels (`white` for tiles that already hold colour).
    pub tint: Color,
    /// Minecraft directional shading for this face direction.
    pub shade: f32,
    /// Tile-local UVs in `[0, 1]`, one per corner; V grows downwards so side
    /// textures stay upright.
    pub uv: [[f32; 2]; 4],
    /// Per-corner light: ambient occlusion multiplied by the face shade.
    pub light: [f32; 4],
}

impl Face {
    #[must_use]
    pub fn center(self) -> Vec3 {
        let sum = self.points[0] + self.points[1] + self.points[2] + self.points[3];
        sum * 0.25
    }

    #[must_use]
    pub fn is_visible_from(self, eye: Vec3) -> bool {
        self.normal.dot(eye - self.center()) > 0.0
    }
}

/// Meshed geometry for one saved chunk.
#[derive(Clone, Debug, Default)]
pub struct ChunkMesh {
    pub chunk: (i32, i32),
    pub faces: Vec<Face>,
    pub min: Vec3,
    pub max: Vec3,
    pub non_air_blocks: usize,
}

impl ChunkMesh {
    #[must_use]
    pub fn face_count(&self) -> usize {
        self.faces.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }
}

type Neighborhood = [Option<Arc<Chunk>>; 9];

fn neighbor_index(cx: i32, cz: i32, wx: i32, wz: i32) -> Option<usize> {
    let dx = wx.div_euclid(16) - cx;
    let dz = wz.div_euclid(16) - cz;
    if !(-1..=1).contains(&dx) || !(-1..=1).contains(&dz) {
        return None;
    }
    Some(((dz + 1) * 3 + (dx + 1)) as usize)
}

fn is_air_at(
    neighbors: &Neighborhood,
    edits: &EditMap,
    cx: i32,
    cz: i32,
    x: i32,
    y: i32,
    z: i32,
) -> bool {
    if !edits.is_empty() {
        if let Some(state) = edits.get(&(x, y, z)) {
            return state.is_air();
        }
    }
    let Some(index) = neighbor_index(cx, cz, x, z) else {
        return true;
    };
    let Some(chunk) = neighbors[index].as_ref() else {
        return true;
    };
    chunk
        .block_at(x.rem_euclid(16), y, z.rem_euclid(16))
        .map_or(true, BlockState::is_air)
}

/// Meshes one chunk. `Ok(None)` means the save has no chunk at that slot.
///
/// Faces are emitted only where a non-air block touches air. Cross-chunk
/// boundaries are resolved through the surrounding 3x3 chunk neighbourhood so
/// no internal wall is drawn at a chunk edge.
pub fn build_chunk_mesh(
    source: &mut dyn ChunkSource,
    cx: i32,
    cz: i32,
    edits: &EditMap,
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
        faces: Vec::new(),
        min: Vec3::splat(f32::MAX),
        max: Vec3::splat(f32::MIN),
        non_air_blocks: 0,
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
                        state
                    } else {
                        edits.get(&(wx, wy, wz)).unwrap_or(state)
                    };
                    if effective.is_air() {
                        continue;
                    }
                    mesh.non_air_blocks += 1;
                    emit_face(
                        &mut mesh.faces,
                        &neighbors,
                        edits,
                        cx,
                        cz,
                        wx,
                        wy,
                        wz,
                        &effective.name,
                    );
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
        emit_face(
            &mut mesh.faces,
            &neighbors,
            edits,
            cx,
            cz,
            wx,
            wy,
            wz,
            &state.name,
        );
    }

    if mesh.faces.is_empty() {
        mesh.min = Vec3::ZERO;
        mesh.max = Vec3::ZERO;
    } else {
        let (min, max) = mesh_bounds(&mesh.faces);
        mesh.min = min;
        mesh.max = max;
    }
    Ok(Some(mesh))
}

/// One component of a unit-cube corner.
fn corner_axis(corner: Vec3, axis: usize) -> f32 {
    match axis {
        0 => corner.x,
        1 => corner.y,
        _ => corner.z,
    }
}

/// Block-cell offset along one axis.
fn cell_offset(axis: usize, amount: i32) -> (i32, i32, i32) {
    match axis {
        0 => (amount, 0, 0),
        1 => (0, amount, 0),
        _ => (0, 0, amount),
    }
}

fn add_offset(a: (i32, i32, i32), b: (i32, i32, i32)) -> (i32, i32, i32) {
    (a.0 + b.0, a.1 + b.1, a.2 + b.2)
}

/// Neighbour offset, normal, unit-cube corners, and face direction.
type FaceSpec = ((i32, i32, i32), Vec3, [Vec3; 4], FaceDir);

#[allow(clippy::too_many_arguments)]
fn emit_face(
    faces: &mut Vec<Face>,
    neighbors: &Neighborhood,
    edits: &EditMap,
    cx: i32,
    cz: i32,
    wx: i32,
    wy: i32,
    wz: i32,
    name: &str,
) {
    let origin = Vec3::new(wx as f32, wy as f32, wz as f32);
    // (neighbour offset, normal, four corners as unit-cube offsets, direction)
    let directions: [FaceSpec; 6] = [
        (
            (1, 0, 0),
            Vec3::new(1.0, 0.0, 0.0),
            [
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
                Vec3::new(1.0, 1.0, 1.0),
                Vec3::new(1.0, 0.0, 1.0),
            ],
            FaceDir::PosX,
        ),
        (
            (-1, 0, 0),
            Vec3::new(-1.0, 0.0, 0.0),
            [
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 1.0, 1.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
            FaceDir::NegX,
        ),
        (
            (0, 1, 0),
            Vec3::new(0.0, 1.0, 0.0),
            [
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
                Vec3::new(1.0, 1.0, 1.0),
                Vec3::new(0.0, 1.0, 1.0),
            ],
            FaceDir::PosY,
        ),
        (
            (0, -1, 0),
            Vec3::new(0.0, -1.0, 0.0),
            [
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 1.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
            FaceDir::NegY,
        ),
        (
            (0, 0, 1),
            Vec3::new(0.0, 0.0, 1.0),
            [
                Vec3::new(0.0, 0.0, 1.0),
                Vec3::new(1.0, 0.0, 1.0),
                Vec3::new(1.0, 1.0, 1.0),
                Vec3::new(0.0, 1.0, 1.0),
            ],
            FaceDir::PosZ,
        ),
        (
            (0, 0, -1),
            Vec3::new(0.0, 0.0, -1.0),
            [
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
            FaceDir::NegZ,
        ),
    ];

    for ((dx, dy, dz), normal, corners, face) in directions {
        if !is_air_at(neighbors, edits, cx, cz, wx + dx, wy + dy, wz + dz) {
            continue;
        }
        let (tile, tint) = block_tile(name, face);
        let shade = face.shade();
        let normal_axis = if normal.x.abs() > 0.5 {
            0
        } else if normal.y.abs() > 0.5 {
            1
        } else {
            2
        };
        // Side textures stay upright: V follows +Y (flipped, since texture rows
        // run top to bottom); top/bottom faces map V to Z.
        let (u_axis, v_axis, flip_v) = match normal_axis {
            0 => (2, 1, true),
            1 => (0, 2, false),
            _ => (0, 1, true),
        };
        let tangent_axes: [usize; 2] = match normal_axis {
            0 => [1, 2],
            1 => [0, 2],
            _ => [0, 1],
        };
        let base = (wx + dx, wy + dy, wz + dz);
        let mut uv = [[0.0_f32; 2]; 4];
        let mut light = [0.0_f32; 4];
        for (index, corner) in corners.iter().enumerate() {
            let u = corner_axis(*corner, u_axis);
            let v_axis_value = corner_axis(*corner, v_axis);
            uv[index] = [
                u,
                if flip_v {
                    1.0 - v_axis_value
                } else {
                    v_axis_value
                },
            ];

            // Ambient occlusion: sample the two edge neighbours and the diagonal
            // in the plane one block outside this face.
            let along_a = if corner_axis(*corner, tangent_axes[0]) > 0.5 {
                1
            } else {
                -1
            };
            let along_b = if corner_axis(*corner, tangent_axes[1]) > 0.5 {
                1
            } else {
                -1
            };
            let side_a = add_offset(base, cell_offset(tangent_axes[0], along_a));
            let side_b = add_offset(base, cell_offset(tangent_axes[1], along_b));
            let diagonal = add_offset(
                base,
                add_offset(
                    cell_offset(tangent_axes[0], along_a),
                    cell_offset(tangent_axes[1], along_b),
                ),
            );
            let occludes = |cell: (i32, i32, i32)| {
                !is_air_at(neighbors, edits, cx, cz, cell.0, cell.1, cell.2)
            };
            let a = occludes(side_a);
            let b = occludes(side_b);
            let c = occludes(diagonal);
            let ao = if a && b {
                0.6
            } else {
                1.0 - 0.2 * u8::from(a) as f32 - 0.2 * u8::from(b) as f32 - 0.2 * u8::from(c) as f32
            };
            light[index] = ao * shade;
        }

        faces.push(Face {
            points: corners.map(|corner| corner + origin),
            normal,
            // Shaded flat colour remains a usable fallback without an atlas.
            color: block_color(name).shaded(shade),
            tile,
            tint,
            shade,
            uv,
            light,
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
            .faces
            .iter()
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
            .faces
            .iter()
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
            .faces
            .iter()
            .find(|face| face.normal.y > 0.9)
            .expect("top face");
        assert_eq!(top.tile, Tile::Stone);
        assert_eq!(top.shade, 1.0);
        assert_eq!(top.light, [1.0; 4]);

        // Side textures are upright: the highest corner samples the top of the
        // tile (V = 0) and the lowest corner samples the bottom (V = 1).
        for face in mesh.faces.iter().filter(|face| face.normal.y.abs() < 0.5) {
            let expected_shade = if face.normal.x.abs() > 0.5 { 0.6 } else { 0.8 };
            assert_eq!(face.tile, Tile::Stone);
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
            .faces
            .iter()
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
        assert!(placed
            .faces
            .iter()
            .any(|face| face.color != base.faces[0].color));

        edits.insert((8, 64, 8), BlockState::air());
        let removed = build_chunk_mesh(&mut world, 0, 0, &edits).unwrap().unwrap();
        assert_eq!(removed.face_count(), base.face_count());
        assert_eq!(removed.non_air_blocks, base.non_air_blocks);
    }
}
