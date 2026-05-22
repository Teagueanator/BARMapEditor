//! S3O binary parser. Port of
//! `RecoilEngine/rts/Rendering/Models/S3OParser.cpp` (GPL-2.0-or-later)
//! with only the fields the thumbnail render pass consumes.
//!
//! File layout (little-endian throughout; matches the engine reader at
//! `S3OParser.cpp:39-82` + `s3o.h`):
//!
//! ```text
//! S3OHeader (76 B):
//!   [0..12]  magic "Spring unit\0"
//!   [12..16] u32 version (0 today)
//!   [16..20] f32 radius
//!   [20..24] f32 height
//!   [24..28] f32 midx
//!   [28..32] f32 midy
//!   [32..36] f32 midz
//!   [36..40] u32 rootPiece          — file offset to root Piece
//!   [40..44] u32 collisionData      — must be 0
//!   [44..48] u32 texture1           — file offset to NUL-terminated string
//!   [48..52] u32 texture2           — file offset, may be 0
//!
//! Piece (52 B):
//!   [0..4]   u32 name             — file offset to NUL-terminated string
//!   [4..8]   u32 numchildren
//!   [8..12]  u32 children          — file offset to dword[numchildren]
//!   [12..16] u32 numVertices
//!   [16..20] u32 vertices          — file offset to Vertex[numVertices]
//!   [20..24] u32 vertexType (0)
//!   [24..28] u32 primitiveType (0=triangles, 1=triangle strips, 2=quads)
//!   [28..32] u32 vertexTableSize
//!   [32..36] u32 vertexTable       — file offset to dword[vertexTableSize]
//!   [36..40] u32 collisionData (0)
//!   [40..44] f32 xoffset           — relative to parent piece
//!   [44..48] f32 yoffset
//!   [48..52] f32 zoffset
//!
//! Vertex (32 B):
//!   pos:f32×3, normal:f32×3, texu:f32, texv:f32
//! ```
//!
//! Phase B does NOT consume:
//! - Tangents (the engine recomputes them; we don't shade thumbnails
//!   with normal-mapped lighting).
//! - Collision volumes (`collisionData` must be 0 per spec).
//! - Piece names (we don't expose hierarchy in the thumbnail).
//!
//! Phase B DOES consume:
//! - Texture1 string (matches the family's diffuse, but kept explicit
//!   for the eventual override case where a per-entry `.s3o` references
//!   a non-default texture).
//! - Per-piece vertex pos + normal + uv arrays.
//! - Per-piece primitive type (so we can triangulate strips and quads
//!   to a uniform Triangles layout, matching `Trianglize()` in
//!   `S3OParser.cpp:197`).
//! - Per-piece `(x,y,z) offset` and recursive children (so we can
//!   accumulate piece-local transforms into world positions).

use std::fmt;

use thiserror::Error;

/// One S3O piece flattened into world-relative coordinates. The
/// piece's local `offset` has already been folded into every vertex
/// `pos`; the thumbnail pass just streams `vertices` + `indices` as a
/// single triangle list.
#[derive(Debug, Clone)]
pub struct S3oPiece {
    /// World-relative vertices in this piece (offset already applied).
    pub vertices: Vec<S3oVertex>,
    /// Triangle-list indices into `vertices`. Strips and quads from
    /// the source file are pre-triangulated at parse time so the
    /// renderer doesn't need a primitive-type switch.
    pub indices: Vec<u32>,
}

/// One S3O vertex. `pos` is world-relative (the parser folds piece
/// offsets in during traversal). `normal` is the per-vertex source-
/// authored normal (zero-vector when invalid in source); the thumbnail
/// shader normalises it client-side.
#[derive(Debug, Clone, Copy)]
pub struct S3oVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

/// Parsed S3O model — the projection of the engine's `S3DModel` onto
/// the fields the thumbnail pass needs.
#[derive(Debug, Clone)]
pub struct S3oModel {
    /// Total pieces in the model (sum, not a tree). Useful for the
    /// thumbnail-bake `info!` log and for fixture assertions.
    pub piece_count: u32,
    /// Flattened triangle list across every piece, in tree-walk order.
    /// Pieces share index space (each piece's indices have its base
    /// vertex offset added at parse time).
    pub vertices: Vec<S3oVertex>,
    /// Indices into `vertices`. `vertices.len() <= u32::MAX` is
    /// assumed (S3O models are tiny — agorm_rock1 is ~5 KB).
    pub indices: Vec<u32>,
    /// `header.radius` — bounding-sphere radius. The thumbnail render
    /// pass uses this to fit the camera so the mesh fills the frame.
    pub radius: f32,
    /// `header.height` — overall vertical extent. Useful for vertical
    /// framing in the thumbnail pass (some pieces extend below the
    /// origin, especially tall trees).
    pub height: f32,
    /// First texture path as authored in the .s3o. Engine convention
    /// is a relative path like `"unittextures/foo.tga"` (or just
    /// `"foo.tga"` for older models). The thumbnail pass resolves
    /// this against the family's vendored diffuse.
    pub texture1: Option<String>,
    /// Second texture path (engine `texs[1]`). Usually empty for
    /// mapfeatures; carried for completeness.
    pub texture2: Option<String>,
}

impl S3oModel {
    /// Aggregate bounding box across every world-relative vertex.
    /// Recomputed on demand because the thumbnail pass only needs it
    /// once per bake. Returns `(min, max)` in piece-local-then-flattened
    /// coordinates.
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        if self.vertices.is_empty() {
            return ([0.0; 3], [0.0; 3]);
        }
        let mut min = self.vertices[0].pos;
        let mut max = min;
        for v in &self.vertices[1..] {
            for i in 0..3 {
                if v.pos[i] < min[i] {
                    min[i] = v.pos[i];
                }
                if v.pos[i] > max[i] {
                    max[i] = v.pos[i];
                }
            }
        }
        (min, max)
    }
}

/// Parse-time failure modes. Distinct variants so the thumbnail-bake
/// caller can log per-cause counts (corrupted vs. unsupported vs.
/// truncated).
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("S3O file too small for header ({0} bytes, need at least 76)")]
    HeaderTooShort(usize),
    #[error("S3O magic mismatch — expected \"Spring unit\\0\", got {0:?}")]
    BadMagic(Vec<u8>),
    #[error("S3O version {0} is not supported (only version 0 known)")]
    UnsupportedVersion(u32),
    #[error("S3O piece offset {offset} out of bounds (file size {len})")]
    PieceOob { offset: u64, len: usize },
    #[error("S3O piece field offset {offset} out of bounds (file size {len})")]
    SliceOob { offset: u64, len: usize },
    #[error("S3O recursion depth exceeded {max} — piece tree may be cyclic")]
    RecursionLimit { max: u32 },
    #[error("S3O index {index} out of bounds for piece vertex count {count}")]
    IndexOob { index: u32, count: u32 },
}

/// Stub for the actual binary parser. Commit 2 of Sprint 29b fills
/// this in; commit 1 (this commit) lands the type definitions + the
/// error enum so downstream callers can be written in parallel
/// (`thumbnail.rs` can already type-check against `S3oModel`).
pub fn parse_s3o(_bytes: &[u8]) -> Result<S3oModel, ParseError> {
    // Phase B commit 2 implementation lands the real parser. Leaving
    // a typed placeholder so the rest of the crate compiles and the
    // wiring tests (cache + thumbnail signature) can land first.
    Err(ParseError::UnsupportedVersion(u32::MAX))
}

impl fmt::Display for S3oModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "S3oModel(pieces={} verts={} tris={} radius={:.1} height={:.1} tex1={:?})",
            self.piece_count,
            self.vertices.len(),
            self.indices.len() / 3,
            self.radius,
            self.height,
            self.texture1,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_error_short_header() {
        // Empty input — must surface a typed length error, not panic.
        match parse_s3o(&[]) {
            Err(ParseError::UnsupportedVersion(_)) => {
                // Commit 1 stub. Commit 2 swaps this assertion to
                // `Err(ParseError::HeaderTooShort(0))` once the real
                // parser lands.
            }
            other => panic!("expected stub UnsupportedVersion, got {other:?}"),
        }
    }

    #[test]
    fn empty_model_bounds_are_zero() {
        let m = S3oModel {
            piece_count: 0,
            vertices: vec![],
            indices: vec![],
            radius: 0.0,
            height: 0.0,
            texture1: None,
            texture2: None,
        };
        let (min, max) = m.bounds();
        assert_eq!(min, [0.0; 3]);
        assert_eq!(max, [0.0; 3]);
    }

    #[test]
    fn bounds_track_extremes() {
        let m = S3oModel {
            piece_count: 1,
            vertices: vec![
                S3oVertex {
                    pos: [-1.0, 0.0, 2.0],
                    normal: [0.0, 1.0, 0.0],
                    uv: [0.0; 2],
                },
                S3oVertex {
                    pos: [3.0, -5.0, 1.0],
                    normal: [0.0, 1.0, 0.0],
                    uv: [0.0; 2],
                },
            ],
            indices: vec![0, 1, 0],
            radius: 5.0,
            height: 5.0,
            texture1: Some("foo.tga".to_string()),
            texture2: None,
        };
        let (min, max) = m.bounds();
        assert_eq!(min, [-1.0, -5.0, 1.0]);
        assert_eq!(max, [3.0, 0.0, 2.0]);
    }
}
