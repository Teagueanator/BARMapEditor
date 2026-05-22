//! S3O binary parser. Port of
//! `RecoilEngine/rts/Rendering/Models/S3OParser.cpp` (GPL-2.0-or-later)
//! with only the fields the thumbnail render pass consumes.
//!
//! Layout (little-endian throughout; matches the engine reader at
//! `S3OParser.cpp:39-82` + `s3o.h`):
//!
//! ```text
//! S3OHeader (52 B):
//!   [0..12]   magic "Spring unit\0"
//!   [12..16]  u32 version (0 today)
//!   [16..20]  f32 radius
//!   [20..24]  f32 height
//!   [24..28]  f32 midx
//!   [28..32]  f32 midy
//!   [32..36]  f32 midz
//!   [36..40]  u32 rootPiece          — file offset to root Piece
//!   [40..44]  u32 collisionData      — must be 0
//!   [44..48]  u32 texture1           — file offset to NUL-terminated string
//!   [48..52]  u32 texture2           — file offset, may be 0
//!
//! Piece (52 B):
//!   [0..4]    u32 name                — file offset to NUL-terminated string
//!   [4..8]    u32 numchildren
//!   [8..12]   u32 children            — file offset to dword[numchildren]
//!   [12..16]  u32 numVertices
//!   [16..20]  u32 vertices            — file offset to Vertex[numVertices]
//!   [20..24]  u32 vertexType (0)
//!   [24..28]  u32 primitiveType (0=triangles, 1=triangle strips, 2=quads)
//!   [28..32]  u32 vertexTableSize
//!   [32..36]  u32 vertexTable         — file offset to dword[vertexTableSize]
//!   [36..40]  u32 collisionData (0)
//!   [40..44]  f32 xoffset             — relative to parent piece
//!   [44..48]  f32 yoffset
//!   [48..52]  f32 zoffset
//!
//! Vertex (32 B):
//!   [0..12]   f32 pos.xyz
//!   [12..24]  f32 normal.xyz
//!   [24..32]  f32 uv (u, v)
//! ```
//!
//! Engine conventions the port matches:
//!
//! - Endianness: little-endian throughout (the engine swaps on big-
//!   endian machines via `byteorder.h`; we just assume LE since every
//!   target Sprint 29b cares about is LE — x86_64 and aarch64).
//! - `(fp->xxxCount > 0)` guards: the engine skips reading vertex /
//!   index / child arrays when the count is zero (workaround for
//!   pre-2010 S3O tools that wrote stale pointers); we mirror that.
//! - Triangulation: `S3OParser.cpp:197 Trianglize()` collapses strip
//!   and quad primitives into a triangle list. We port the (a, b, c)
//!   emit pattern verbatim — note the engine does NOT flip strip
//!   winding by odd/even (technically incorrect for GL_TRIANGLE_STRIP
//!   semantics but matches what the engine renders in-game, so
//!   thumbnails will look identical to what BAR shows).
//! - End-of-strip markers (`u32::MAX`) are honoured: any triple
//!   containing one is skipped.
//! - Piece offsets accumulate down the tree: a piece at offset
//!   `(0, 5, 0)` whose parent is at `(10, 0, 0)` lands its vertices
//!   at world-relative `(parent_pos + (10, 5, 0) + local_pos)`. The
//!   thumbnail pass renders the resulting flat triangle list with
//!   one draw call — no per-piece transforms.

use thiserror::Error;
use tracing::trace;

const MAGIC: &[u8; 12] = b"Spring unit\0";
const HEADER_SIZE: usize = 52;
const PIECE_SIZE: usize = 52;
const VERTEX_SIZE: usize = 32;
/// Defensive depth cap. Engine doesn't enforce one; we cap at 256 so
/// a malformed cyclic file can't deadlock the parser.
const MAX_DEPTH: u32 = 256;

/// One S3O vertex (world-relative position, source-authored normal,
/// 2D texture coordinate). `pos` has every piece offset on the path
/// to the root already folded in — the thumbnail render pass takes
/// `vertices` + `indices` as one flat triangle list.
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
    /// Total pieces walked. Useful for fixture assertions.
    pub piece_count: u32,
    /// Flattened triangle list across every piece, in tree-walk
    /// order. Vertices share index space; each piece's emitted
    /// indices have its base-vertex offset already added.
    pub vertices: Vec<S3oVertex>,
    /// Triangle-list indices into `vertices`. `vertices.len() <=
    /// u32::MAX` is assumed (S3O models are tiny — agorm_rock1 is
    /// ~5 KB).
    pub indices: Vec<u32>,
    /// `header.radius` — bounding-sphere radius. The thumbnail
    /// render pass uses this to fit the camera so the mesh fills the
    /// frame.
    pub radius: f32,
    /// `header.height` — overall vertical extent.
    pub height: f32,
    /// First texture path as authored in the .s3o. Engine convention
    /// is a relative name like `"foo.tga"`; the thumbnail pass
    /// resolves this against the family's vendored diffuse.
    pub texture1: Option<String>,
    /// Second texture path. Usually empty for mapfeatures; carried
    /// for completeness.
    pub texture2: Option<String>,
}

impl S3oModel {
    /// Aggregate AABB across every vertex. Returns `(min, max)` with
    /// piece offsets already applied. `([0;3], [0;3])` for empty
    /// models.
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
/// caller can log per-cause counts.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("S3O file too small for header ({0} bytes, need at least {HEADER_SIZE})")]
    HeaderTooShort(usize),
    #[error("S3O magic mismatch — expected \"Spring unit\\0\", got {0:?}")]
    BadMagic(Vec<u8>),
    #[error("S3O version {0} is not supported (only version 0 known)")]
    UnsupportedVersion(u32),
    #[error("S3O piece offset {offset} out of bounds (file size {len})")]
    PieceOob { offset: u64, len: usize },
    #[error("S3O slice offset {offset} out of bounds (file size {len})")]
    SliceOob { offset: u64, len: usize },
    #[error("S3O recursion depth exceeded {max} — piece tree may be cyclic")]
    RecursionLimit { max: u32 },
    #[error("S3O index {index} out of bounds for piece vertex count {count}")]
    IndexOob { index: u32, count: u32 },
}

/// Parse an S3O file from raw bytes. Deterministic — same bytes
/// always yield the same `S3oModel`. Errors are typed so the
/// thumbnail registry can keep per-cause counts.
pub fn parse_s3o(bytes: &[u8]) -> Result<S3oModel, ParseError> {
    if bytes.len() < HEADER_SIZE {
        return Err(ParseError::HeaderTooShort(bytes.len()));
    }
    if &bytes[0..12] != MAGIC {
        return Err(ParseError::BadMagic(bytes[0..12].to_vec()));
    }
    let version = read_u32(bytes, 12)?;
    if version != 0 {
        return Err(ParseError::UnsupportedVersion(version));
    }
    let radius = read_f32(bytes, 16)?;
    let height = read_f32(bytes, 20)?;
    // midx/y/z at 24/28/32 are unused by the thumbnail pass.
    let root_piece = read_u32(bytes, 36)?;
    // collisionData at 40 must be 0; ignored.
    let texture1_off = read_u32(bytes, 44)?;
    let texture2_off = read_u32(bytes, 48)?;

    let texture1 = read_zstring(bytes, texture1_off as usize);
    let texture2 = read_zstring(bytes, texture2_off as usize);

    let mut vertices: Vec<S3oVertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut piece_count: u32 = 0;
    walk_piece(
        bytes,
        root_piece as usize,
        [0.0; 3],
        &mut vertices,
        &mut indices,
        &mut piece_count,
        0,
    )?;

    trace!(
        piece_count,
        verts = vertices.len(),
        tris = indices.len() / 3,
        radius,
        height,
        tex1 = ?texture1,
        "parsed s3o"
    );

    Ok(S3oModel {
        piece_count,
        vertices,
        indices,
        radius,
        height,
        texture1,
        texture2,
    })
}

/// Recursive piece walk. `parent_world_offset` accumulates down the
/// tree so vertex positions land in a single shared world-relative
/// coordinate space. `depth` is incremented per recursion and capped
/// at [`MAX_DEPTH`] to defend against cyclic files.
#[allow(clippy::too_many_arguments)]
fn walk_piece(
    bytes: &[u8],
    offset: usize,
    parent_world_offset: [f32; 3],
    vertices: &mut Vec<S3oVertex>,
    indices: &mut Vec<u32>,
    piece_count: &mut u32,
    depth: u32,
) -> Result<(), ParseError> {
    if depth > MAX_DEPTH {
        return Err(ParseError::RecursionLimit { max: MAX_DEPTH });
    }
    if offset + PIECE_SIZE > bytes.len() {
        return Err(ParseError::PieceOob {
            offset: offset as u64,
            len: bytes.len(),
        });
    }

    let num_children = read_u32(bytes, offset + 4)?;
    let children_off = read_u32(bytes, offset + 8)?;
    let num_vertices = read_u32(bytes, offset + 12)?;
    let vertices_off = read_u32(bytes, offset + 16)?;
    let prim_type = read_u32(bytes, offset + 24)?;
    let vertex_table_size = read_u32(bytes, offset + 28)?;
    let vertex_table_off = read_u32(bytes, offset + 32)?;
    let xoff = read_f32(bytes, offset + 40)?;
    let yoff = read_f32(bytes, offset + 44)?;
    let zoff = read_f32(bytes, offset + 48)?;

    *piece_count += 1;

    let world_off = [
        parent_world_offset[0] + xoff,
        parent_world_offset[1] + yoff,
        parent_world_offset[2] + zoff,
    ];

    let base_index = vertices.len() as u32;

    if num_vertices > 0 {
        let total = num_vertices as usize * VERTEX_SIZE;
        let start = vertices_off as usize;
        if start.checked_add(total).is_none_or(|end| end > bytes.len()) {
            return Err(ParseError::SliceOob {
                offset: start as u64,
                len: bytes.len(),
            });
        }
        vertices.reserve(num_vertices as usize);
        for i in 0..num_vertices {
            let v_off = start + (i as usize) * VERTEX_SIZE;
            let px = read_f32(bytes, v_off)?;
            let py = read_f32(bytes, v_off + 4)?;
            let pz = read_f32(bytes, v_off + 8)?;
            let nx = read_f32(bytes, v_off + 12)?;
            let ny = read_f32(bytes, v_off + 16)?;
            let nz = read_f32(bytes, v_off + 20)?;
            let u = read_f32(bytes, v_off + 24)?;
            let v = read_f32(bytes, v_off + 28)?;
            vertices.push(S3oVertex {
                pos: [px + world_off[0], py + world_off[1], pz + world_off[2]],
                normal: [nx, ny, nz],
                uv: [u, v],
            });
        }
    }

    let raw_indices: Vec<u32> = if vertex_table_size > 0 {
        let total = vertex_table_size as usize * 4;
        let start = vertex_table_off as usize;
        if start.checked_add(total).is_none_or(|end| end > bytes.len()) {
            return Err(ParseError::SliceOob {
                offset: start as u64,
                len: bytes.len(),
            });
        }
        let mut v = Vec::with_capacity(vertex_table_size as usize);
        for i in 0..vertex_table_size {
            v.push(read_u32(bytes, start + (i as usize) * 4)?);
        }
        v
    } else {
        Vec::new()
    };

    for &idx in &raw_indices {
        if idx != u32::MAX && idx >= num_vertices {
            return Err(ParseError::IndexOob {
                index: idx,
                count: num_vertices,
            });
        }
    }

    match prim_type {
        0 => {
            // Triangles — emit complete triples only.
            for chunk in raw_indices.chunks_exact(3) {
                indices.push(base_index + chunk[0]);
                indices.push(base_index + chunk[1]);
                indices.push(base_index + chunk[2]);
            }
        }
        1 => {
            // Triangle strip — emit (a, b, c) per triple (matches the
            // engine's `Trianglize`, which does NOT flip odd-triangle
            // winding). Skip triples that contain end-of-strip markers.
            for i in 0..raw_indices.len().saturating_sub(2) {
                let a = raw_indices[i];
                let b = raw_indices[i + 1];
                let c = raw_indices[i + 2];
                if a == u32::MAX || b == u32::MAX || c == u32::MAX {
                    continue;
                }
                indices.push(base_index + a);
                indices.push(base_index + b);
                indices.push(base_index + c);
            }
        }
        2 if raw_indices.len().is_multiple_of(4) => {
            // Quads — split into two triangles per 4-vert chunk. A
            // trailing partial chunk drops to the default branch
            // below (engine sets primType = triangles + clears
            // indices on mismatch; we just skip).
            for chunk in raw_indices.chunks_exact(4) {
                indices.push(base_index + chunk[0]);
                indices.push(base_index + chunk[1]);
                indices.push(base_index + chunk[2]);
                indices.push(base_index + chunk[0]);
                indices.push(base_index + chunk[2]);
                indices.push(base_index + chunk[3]);
            }
        }
        _ => {
            // Unknown — engine's default branch is a silent no-op.
        }
    }

    if num_children > 0 {
        let total = num_children as usize * 4;
        let start = children_off as usize;
        if start.checked_add(total).is_none_or(|end| end > bytes.len()) {
            return Err(ParseError::SliceOob {
                offset: start as u64,
                len: bytes.len(),
            });
        }
        for i in 0..num_children {
            let child_off = read_u32(bytes, start + (i as usize) * 4)?;
            walk_piece(
                bytes,
                child_off as usize,
                world_off,
                vertices,
                indices,
                piece_count,
                depth + 1,
            )?;
        }
    }

    Ok(())
}

#[inline]
fn read_u32(bytes: &[u8], off: usize) -> Result<u32, ParseError> {
    if off + 4 > bytes.len() {
        return Err(ParseError::SliceOob {
            offset: off as u64,
            len: bytes.len(),
        });
    }
    Ok(u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()))
}

#[inline]
fn read_f32(bytes: &[u8], off: usize) -> Result<f32, ParseError> {
    if off + 4 > bytes.len() {
        return Err(ParseError::SliceOob {
            offset: off as u64,
            len: bytes.len(),
        });
    }
    Ok(f32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()))
}

/// Read a NUL-terminated string at `off`. Returns `None` when the
/// offset is 0 (engine convention: `texture* == 0` means no texture)
/// or out-of-bounds. Non-UTF-8 contents return `None` rather than
/// erroring — texture names are ASCII in every model I've inspected,
/// and a thumbnail pass with no texture name still renders a useful
/// preview via the per-family diffuse fallback.
fn read_zstring(bytes: &[u8], off: usize) -> Option<String> {
    if off == 0 || off >= bytes.len() {
        return None;
    }
    let end = bytes[off..]
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(bytes.len() - off);
    let slice = &bytes[off..off + end];
    if slice.is_empty() {
        return None;
    }
    String::from_utf8(slice.to_vec()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid S3O blob with one root piece, 3 vertices
    /// (a flat triangle), 3 triangle indices, and a `texture1` of
    /// `"test.tga"`. Total size 221 bytes. Lets us round-trip the
    /// parser without checking any third-party binary into the repo.
    fn synthetic_s3o() -> Vec<u8> {
        let mut out = Vec::with_capacity(256);
        // Header (52 B)
        out.extend_from_slice(b"Spring unit\0"); // 0..12
        out.extend_from_slice(&0u32.to_le_bytes()); // version
        out.extend_from_slice(&5.0_f32.to_le_bytes()); // radius
        out.extend_from_slice(&10.0_f32.to_le_bytes()); // height
        out.extend_from_slice(&0.0_f32.to_le_bytes()); // midx
        out.extend_from_slice(&0.0_f32.to_le_bytes()); // midy
        out.extend_from_slice(&0.0_f32.to_le_bytes()); // midz
        out.extend_from_slice(&52u32.to_le_bytes()); // rootPiece
        out.extend_from_slice(&0u32.to_le_bytes()); // collisionData
        out.extend_from_slice(&212u32.to_le_bytes()); // texture1 -> "test.tga"
        out.extend_from_slice(&0u32.to_le_bytes()); // texture2 absent
        assert_eq!(out.len(), 52);

        // Piece at 52 (52 B)
        out.extend_from_slice(&0u32.to_le_bytes()); // name
        out.extend_from_slice(&0u32.to_le_bytes()); // numchildren
        out.extend_from_slice(&0u32.to_le_bytes()); // children
        out.extend_from_slice(&3u32.to_le_bytes()); // numVertices
        out.extend_from_slice(&104u32.to_le_bytes()); // vertices offset
        out.extend_from_slice(&0u32.to_le_bytes()); // vertexType
        out.extend_from_slice(&0u32.to_le_bytes()); // primitiveType (triangles)
        out.extend_from_slice(&3u32.to_le_bytes()); // vertexTableSize
        out.extend_from_slice(&200u32.to_le_bytes()); // vertexTable
        out.extend_from_slice(&0u32.to_le_bytes()); // collisionData
        out.extend_from_slice(&0.0_f32.to_le_bytes()); // xoff
        out.extend_from_slice(&0.0_f32.to_le_bytes()); // yoff
        out.extend_from_slice(&0.0_f32.to_le_bytes()); // zoff
        assert_eq!(out.len(), 104);

        // 3 vertices (32 B each) at 104, 136, 168
        // v0 = (0,0,0)  normal (0,1,0)  uv (0,0)
        // v1 = (1,0,0)  normal (0,1,0)  uv (1,0)
        // v2 = (0,0,1)  normal (0,1,0)  uv (0,1)
        let push_vertex = |out: &mut Vec<u8>, p: [f32; 3], n: [f32; 3], uv: [f32; 2]| {
            for f in p.iter().chain(n.iter()).chain(uv.iter()) {
                out.extend_from_slice(&f.to_le_bytes());
            }
        };
        push_vertex(&mut out, [0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0]);
        push_vertex(&mut out, [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0]);
        push_vertex(&mut out, [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [0.0, 1.0]);
        assert_eq!(out.len(), 200);

        // 3 indices at 200..212
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&2u32.to_le_bytes());
        assert_eq!(out.len(), 212);

        // texture1 string at 212: "test.tga\0"
        out.extend_from_slice(b"test.tga\0");
        out
    }

    #[test]
    fn parses_synthetic_triangle_model() {
        let bytes = synthetic_s3o();
        let m = parse_s3o(&bytes).expect("synthetic parses");
        assert_eq!(m.piece_count, 1);
        assert_eq!(m.vertices.len(), 3);
        assert_eq!(m.indices, vec![0, 1, 2]);
        assert_eq!(m.texture1.as_deref(), Some("test.tga"));
        assert!(m.texture2.is_none());
        assert!((m.radius - 5.0).abs() < 1e-6);
        assert!((m.height - 10.0).abs() < 1e-6);
        let (min, max) = m.bounds();
        assert_eq!(min, [0.0, 0.0, 0.0]);
        assert_eq!(max, [1.0, 0.0, 1.0]);
    }

    #[test]
    fn short_header_errors() {
        let bytes = vec![0u8; 10];
        match parse_s3o(&bytes) {
            Err(ParseError::HeaderTooShort(10)) => {}
            other => panic!("expected HeaderTooShort(10), got {other:?}"),
        }
    }

    #[test]
    fn bad_magic_errors() {
        let mut bytes = synthetic_s3o();
        bytes[0] = b'X'; // corrupt magic
        match parse_s3o(&bytes) {
            Err(ParseError::BadMagic(_)) => {}
            other => panic!("expected BadMagic, got {other:?}"),
        }
    }

    #[test]
    fn unsupported_version_errors() {
        let mut bytes = synthetic_s3o();
        bytes[12] = 1; // version = 1
        match parse_s3o(&bytes) {
            Err(ParseError::UnsupportedVersion(1)) => {}
            other => panic!("expected UnsupportedVersion(1), got {other:?}"),
        }
    }

    #[test]
    fn out_of_bounds_root_piece_errors() {
        let mut bytes = synthetic_s3o();
        // Overwrite rootPiece offset with 0xFFFFFFFE.
        bytes[36..40].copy_from_slice(&0xFFFF_FFFE_u32.to_le_bytes());
        match parse_s3o(&bytes) {
            Err(ParseError::PieceOob { .. }) => {}
            other => panic!("expected PieceOob, got {other:?}"),
        }
    }

    #[test]
    fn quad_primitive_splits_into_two_triangles() {
        // Build a single-piece model with primType=2 and 4 verts +
        // 4 indices. Expect 6 emitted triangle indices.
        let mut out = Vec::new();
        // Header
        out.extend_from_slice(b"Spring unit\0");
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&1.0_f32.to_le_bytes());
        out.extend_from_slice(&1.0_f32.to_le_bytes());
        out.extend_from_slice(&[0u8; 12]); // midxyz
        out.extend_from_slice(&52u32.to_le_bytes()); // rootPiece
        out.extend_from_slice(&[0u8; 12]); // collisionData + texture1 + texture2
        assert_eq!(out.len(), 52);

        // Piece
        out.extend_from_slice(&0u32.to_le_bytes()); // name
        out.extend_from_slice(&0u32.to_le_bytes()); // numchildren
        out.extend_from_slice(&0u32.to_le_bytes()); // children
        out.extend_from_slice(&4u32.to_le_bytes()); // numVertices
        out.extend_from_slice(&104u32.to_le_bytes()); // vertices
        out.extend_from_slice(&0u32.to_le_bytes()); // vertexType
        out.extend_from_slice(&2u32.to_le_bytes()); // primitiveType = QUADS
        out.extend_from_slice(&4u32.to_le_bytes()); // vertexTableSize
        out.extend_from_slice(&232u32.to_le_bytes()); // vertexTable
        out.extend_from_slice(&[0u8; 16]); // collisionData + offsets
        assert_eq!(out.len(), 104);

        // 4 verts (128 B)
        for _ in 0..4 {
            out.extend_from_slice(&[0u8; 32]);
        }
        assert_eq!(out.len(), 232);

        // 4 indices: 0, 1, 2, 3
        for i in 0u32..4 {
            out.extend_from_slice(&i.to_le_bytes());
        }
        assert_eq!(out.len(), 248);

        let m = parse_s3o(&out).expect("quad model parses");
        assert_eq!(m.indices.len(), 6);
        assert_eq!(m.indices, vec![0, 1, 2, 0, 2, 3]);
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
    fn upstream_pedro1_parses_when_available() {
        // Optional fixture — pedro1.s3o is the smallest upstream
        // model we map (~9 KB) and exercises tree-walk + real
        // texture-string read paths. Skipped when the user hasn't
        // cloned upstream (CI on a clean checkout still passes the
        // synthetic test above).
        let candidate = std::env::var("HOME")
            .ok()
            .map(std::path::PathBuf::from)
            .map(|home| {
                home.join("code")
                    .join("Beyond-All-Reason")
                    .join("mapfeatures")
                    .join("objects3d")
                    .join("pedro1.s3o")
            });
        let Some(path) = candidate else {
            eprintln!("[skipped] no $HOME");
            return;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!(
                "[skipped] upstream pedro1.s3o not present at {}",
                path.display()
            );
            return;
        };
        let m = parse_s3o(&bytes).expect("real upstream s3o parses");
        assert!(m.piece_count >= 1, "model has at least one piece");
        assert!(!m.vertices.is_empty(), "model has vertices");
        assert!(
            m.indices.len().is_multiple_of(3),
            "indices triangulate cleanly"
        );
        assert!(m.texture1.is_some(), "model declares a diffuse texture");
        // Sanity-check the triangle list — every index must be in range.
        let n = m.vertices.len() as u32;
        for &i in &m.indices {
            assert!(i < n, "index {i} out of range for {n} vertices");
        }
    }
}
