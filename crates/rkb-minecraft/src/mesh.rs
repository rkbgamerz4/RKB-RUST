//! Block colours, solidity, and chunk meshing.
//!
//! Minecraft textures are not bundled. Every face gets a flat colour derived
//! from its block name, with per-face shading so the geometry reads as a
//! three-dimensional world. This is a deliberate first step: it renders real
//! saved block data without pretending to have the resource pack pipeline.

use std::sync::Arc;

use rkb_core::Vec3;
use rkb_mc::{BlockState, Chunk, World, WorldError};
use rkb_render::Color;

use crate::blocks::{block_color, is_solid};

/// One outward-facing block face in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Face {
    pub points: [Vec3; 4],
    pub normal: Vec3,
    pub color: Color,
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

fn is_air_at(neighbors: &Neighborhood, cx: i32, cz: i32, x: i32, y: i32, z: i32) -> bool {
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
    world: &mut World,
    cx: i32,
    cz: i32,
) -> Result<Option<ChunkMesh>, WorldError> {
    let Some(chunk) = world.chunk(cx, cz)? else {
        return Ok(None);
    };
    let mut neighbors: Neighborhood = Default::default();
    for dz in -1..=1 {
        for dx in -1..=1 {
            let index = ((dz + 1) * 3 + (dx + 1)) as usize;
            neighbors[index] = world.chunk(cx + dx, cz + dz)?;
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
                    if state.is_air() {
                        continue;
                    }
                    mesh.non_air_blocks += 1;
                    let wx = cx * 16 + x as i32;
                    let wy = section_y + y as i32;
                    let wz = cz * 16 + z as i32;
                    let color = block_color(&state.name);
                    emit_face(&mut mesh.faces, &neighbors, cx, cz, wx, wy, wz, color);
                }
            }
        }
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

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn emit_face(
    faces: &mut Vec<Face>,
    neighbors: &Neighborhood,
    cx: i32,
    cz: i32,
    wx: i32,
    wy: i32,
    wz: i32,
    color: Color,
) {
    let origin = Vec3::new(wx as f32, wy as f32, wz as f32);
    // (neighbour offset, normal, four corners as unit-cube offsets, shade)
    let directions: [((i32, i32, i32), Vec3, [Vec3; 4], f32); 6] = [
        (
            (1, 0, 0),
            Vec3::new(1.0, 0.0, 0.0),
            [
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
                Vec3::new(1.0, 1.0, 1.0),
                Vec3::new(1.0, 0.0, 1.0),
            ],
            0.78,
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
            0.62,
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
            1.0,
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
            0.45,
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
            0.70,
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
            0.55,
        ),
    ];

    for ((dx, dy, dz), normal, corners, shade) in directions {
        if !is_air_at(neighbors, cx, cz, wx + dx, wy + dy, wz + dz) {
            continue;
        }
        faces.push(Face {
            points: corners.map(|corner| corner + origin),
            normal,
            color: color.shaded(shade),
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
        let mesh = build_chunk_mesh(&mut world, 0, 0).unwrap().unwrap();

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
        let mesh = build_chunk_mesh(&mut world, 0, 0).unwrap().unwrap();

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
        let mesh = build_chunk_mesh(&mut world, 0, 0).unwrap().unwrap();
        let wall = mesh
            .faces
            .iter()
            .filter(|face| face.normal.x.abs() > 0.9)
            .count();
        assert_eq!(wall, 2 * 256);
    }
}
