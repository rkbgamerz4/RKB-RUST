//! Conversion of M6 `ChunkMesh` data into GPU-ready vertex and index data.
//!
//! Nothing here decodes chunks or blocks: the mesher in `rkb-minecraft` already
//! produced world-space corners, per-corner light, tint, and atlas texture ids.
//! This module only expands a quad into two triangles and maps the face's
//! tile-local UVs into the active atlas rectangle.

use rkb_core::Vec3;
use rkb_minecraft::mesh::{ChunkMesh, Face};
use rkb_render::Color;

/// One vertex uploaded to the GPU world pipeline.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuVertex {
    /// World-space position in blocks.
    pub position: [f32; 3],
    /// Atlas UV, already mapped from tile-local coordinates.
    pub uv: [f32; 2],
    /// Per-vertex light (AO × face shade × sky/block light) from M6.
    pub light: f32,
    /// Multiplied over the texel colour.
    pub tint: [f32; 3],
}

/// A static layer of one chunk: vertices plus `u32` triangle indices.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldMesh {
    pub vertices: Vec<GpuVertex>,
    pub indices: Vec<u32>,
}

impl WorldMesh {
    #[must_use]
    pub fn triangle_count(&self) -> u32 {
        (self.indices.len() / 3) as u32
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    #[must_use]
    pub fn vertex_bytes(&self) -> u64 {
        (self.vertices.len() * std::mem::size_of::<GpuVertex>()) as u64
    }

    #[must_use]
    pub fn index_bytes(&self) -> u64 {
        (self.indices.len() * std::mem::size_of::<u32>()) as u64
    }
}

/// A translucent face kept on the CPU for per-frame back-to-front sorting.
#[derive(Clone, Debug, PartialEq)]
pub struct TranslucentQuad {
    pub points: [Vec3; 4],
    /// Atlas UVs per corner.
    pub uv: [[f32; 2]; 4],
    pub light: [f32; 4],
    pub tint: [f32; 3],
    pub normal: Vec3,
    pub double_sided: bool,
}

impl TranslucentQuad {
    #[must_use]
    pub fn center(&self) -> Vec3 {
        let sum = self.points[0] + self.points[1] + self.points[2] + self.points[3];
        sum * 0.25
    }

    #[must_use]
    pub fn is_visible_from(&self, eye: Vec3) -> bool {
        self.double_sided || self.normal.dot(eye - self.center()) > 0.0
    }
}

/// The three render-layer inputs the GPU scene needs from one chunk.
#[derive(Clone, Debug, Default)]
pub struct PackedChunk {
    pub opaque: WorldMesh,
    pub cutout: WorldMesh,
    pub translucent: Vec<TranslucentQuad>,
}

impl PackedChunk {
    #[must_use]
    pub fn triangle_count(&self) -> u32 {
        self.opaque.triangle_count()
            + self.cutout.triangle_count()
            + self.translucent.len() as u32 * 2
    }
}

/// Packs one M6 chunk mesh, mapping each face's texture id through `uv_rect`.
///
/// `uv_rect` returns `[u0, v0, u1, v1]` in normalized atlas coordinates for a
/// texture id. The game supplies it from `MinecraftGame::atlas_uv`, so the GPU
/// path and the software path use exactly the same atlas rectangles.
pub fn pack_chunk(mesh: &ChunkMesh, uv_rect: &dyn Fn(u32) -> [f32; 4]) -> PackedChunk {
    PackedChunk {
        opaque: pack_layer(&mesh.opaque, uv_rect),
        cutout: pack_layer(&mesh.cutout, uv_rect),
        translucent: mesh
            .translucent
            .iter()
            .map(|face| translucent_quad(face, uv_rect))
            .collect(),
    }
}

fn pack_layer(faces: &[Face], uv_rect: &dyn Fn(u32) -> [f32; 4]) -> WorldMesh {
    let mut vertices = Vec::with_capacity(faces.len() * 4);
    let mut indices = Vec::with_capacity(faces.len() * 6);
    for face in faces {
        let rect = uv_rect(face.texture);
        let base = vertices.len() as u32;
        for corner in 0..4 {
            vertices.push(GpuVertex {
                position: [
                    face.points[corner].x,
                    face.points[corner].y,
                    face.points[corner].z,
                ],
                uv: atlas_uv(rect, face.uv[corner]),
                light: face.light[corner],
                tint: tint(face.tint),
            });
        }
        // Quad corners are counter-clockwise when seen from outside; the
        // world pipeline disables back-face culling, so this winding is only
        // used for the two triangles' shared edge.
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    WorldMesh { vertices, indices }
}

fn translucent_quad(face: &Face, uv_rect: &dyn Fn(u32) -> [f32; 4]) -> TranslucentQuad {
    let rect = uv_rect(face.texture);
    TranslucentQuad {
        points: face.points,
        uv: [
            atlas_uv(rect, face.uv[0]),
            atlas_uv(rect, face.uv[1]),
            atlas_uv(rect, face.uv[2]),
            atlas_uv(rect, face.uv[3]),
        ],
        light: face.light,
        tint: tint(face.tint),
        normal: face.normal,
        double_sided: face.double_sided,
    }
}

/// `[u0, v0, u1, v1]` + tile-local `[u, v]` → atlas coordinate.
#[must_use]
pub fn atlas_uv(rect: [f32; 4], uv: [f32; 2]) -> [f32; 2] {
    [
        rect[0] + (rect[2] - rect[0]) * uv[0],
        rect[1] + (rect[3] - rect[1]) * uv[1],
    ]
}

#[must_use]
pub fn tint(color: Color) -> [f32; 3] {
    [
        f32::from(color.r) / 255.0,
        f32::from(color.g) / 255.0,
        f32::from(color.b) / 255.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rkb_minecraft::mesh::Face;
    use rkb_minecraft::model::RenderLayer;

    fn face() -> Face {
        Face {
            points: [
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
            normal: Vec3::new(0.0, 0.0, -1.0),
            texture: 7,
            tint: Color::rgb(255, 128, 0),
            shade: 1.0,
            uv: [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
            light: [0.25, 0.5, 0.75, 1.0],
            color: Color::WHITE,
            layer: RenderLayer::Opaque,
            double_sided: false,
        }
    }

    #[test]
    fn packs_one_quad_into_four_vertices_and_six_indices() {
        let mesh = ChunkMesh {
            opaque: vec![face()],
            ..ChunkMesh::default()
        };
        let packed = pack_chunk(&mesh, &|_| [0.0, 0.0, 0.5, 1.0]);
        assert_eq!(packed.opaque.vertices.len(), 4);
        assert_eq!(packed.opaque.indices, vec![0, 1, 2, 0, 2, 3]);
        assert_eq!(packed.opaque.triangle_count(), 2);
    }

    #[test]
    fn maps_tile_uvs_into_the_atlas_rect() {
        let mesh = ChunkMesh {
            opaque: vec![face()],
            ..ChunkMesh::default()
        };
        let packed = pack_chunk(&mesh, &|_| [0.25, 0.5, 0.75, 1.0]);
        let v = &packed.opaque.vertices;
        // tile (0,1) -> atlas (0.25, 1.0); tile (1,0) -> atlas (0.75, 0.5)
        assert!((v[0].uv[0] - 0.25).abs() < 1e-6);
        assert!((v[0].uv[1] - 1.0).abs() < 1e-6);
        assert!((v[2].uv[0] - 0.75).abs() < 1e-6);
        assert!((v[2].uv[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn carries_light_and_tint_unchanged() {
        let mesh = ChunkMesh {
            opaque: vec![face()],
            ..ChunkMesh::default()
        };
        let packed = pack_chunk(&mesh, &|_| [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(packed.opaque.vertices[1].light, 0.5);
        let tint = packed.opaque.vertices[0].tint;
        assert!((tint[0] - 1.0).abs() < 1e-6);
        assert!((tint[1] - 128.0 / 255.0).abs() < 1e-6);
        assert!((tint[2] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn translucent_faces_stay_quads_for_sorting() {
        let mut f = face();
        f.layer = RenderLayer::Translucent;
        let mesh = ChunkMesh {
            translucent: vec![f],
            ..ChunkMesh::default()
        };
        let packed = pack_chunk(&mesh, &|_| [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(packed.translucent.len(), 1);
        assert_eq!(packed.translucent[0].points[2], Vec3::new(1.0, 1.0, 0.0));
        assert_eq!(packed.triangle_count(), 2);
    }

    #[test]
    fn quad_visibility_uses_the_face_normal() {
        let mut f = face();
        f.layer = RenderLayer::Translucent;
        let mesh = ChunkMesh {
            translucent: vec![f],
            ..ChunkMesh::default()
        };
        let packed = pack_chunk(&mesh, &|_| [0.0, 0.0, 1.0, 1.0]);
        let quad = &packed.translucent[0];
        // Normal points to -Z, so an eye at -Z sees it and an eye at +Z does not.
        assert!(quad.is_visible_from(Vec3::new(0.5, 0.5, -5.0)));
        assert!(!quad.is_visible_from(Vec3::new(0.5, 0.5, 5.0)));
    }
}
