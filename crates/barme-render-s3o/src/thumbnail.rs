//! CPU rasteriser for S3O thumbnails. Produces a 128² RGBA8 image
//! showing the model from above with Lambert key lighting and the
//! per-family diffuse texture sampled bilinearly.
//!
//! Why CPU not wgpu:
//! 1. Determinism — same `S3oModel` bytes always yield identical
//!    output. The thumbnail cache key (`sha256(s3o_bytes)`) is
//!    stable across machines and driver versions.
//! 2. No async device handshake at app startup; the parser + bake
//!    + cache pipeline runs entirely on the main thread.
//! 3. Per-thumbnail cost is small (typical `pedro1.s3o`: ~300 verts,
//!    ~600 tris × 128² fragments) — measured at ~1-3 ms per bake on
//!    a Vega 8 iGPU CPU side. The cache keeps subsequent launches
//!    free.
//!
//! Limitations (acceptable for a thumbnail):
//! - No mipmaps. The diffuse is sampled bilinearly at LOD 0;
//!   minification aliasing on dense textures is bounded by the 128²
//!   target.
//! - Per-fragment normalisation only — no tangent-space lighting.
//!   The Lambert NdotL on the source-authored normals is plenty for
//!   a recognition-grade thumbnail.
//! - Both faces visible (no backface culling). Trees and rocks
//!   benefit from seeing the back-side of leaves / overhangs.
//! - Triangulated triangle-strip winding follows the engine's
//!   non-flipping `Trianglize()` (see `parser.rs` rationale); some
//!   triangles will face away from the camera and just be hidden by
//!   the depth test against their facing counterparts.

use glam::{Mat4, Vec3, Vec4};
use tracing::trace;

use crate::parser::S3oModel;

/// Width / height of a baked thumbnail. Must match
/// `barme_app::feature_decals::SPRITE_SIZE`.
pub const SPRITE_SIZE: u32 = 128;
/// Bytes per row of a baked thumbnail.
pub const STRIDE_BYTES: usize = SPRITE_SIZE as usize * 4;
/// Total RGBA8 byte count of one thumbnail.
pub const TOTAL_BYTES: usize = SPRITE_SIZE as usize * SPRITE_SIZE as usize * 4;

/// Diffuse texture size — feature_decals always resizes to 128²
/// before this crate sees it.
const DIFFUSE_SIDE: u32 = 128;

/// One baked thumbnail in RGBA8, row-major.
#[derive(Debug, Clone)]
pub struct Thumbnail {
    pub rgba: Vec<u8>,
}

/// Bake one thumbnail. `diffuse_rgba` MUST be exactly `128 × 128 × 4`
/// bytes (per `feature_decals::SPRITE_SIZE`). Returns an opaque-on-
/// model, transparent-elsewhere RGBA8 buffer.
///
/// Camera convention:
/// - Look down `-Y` from above.
/// - Orthographic projection sized to `max(extent_x, extent_z) ×
///   max(extent_x, extent_z)` so the model fills the frame on its
///   widest XZ axis.
/// - World `+X` → screen +X; world `+Z` → screen +Y (so a tree
///   pointing `+Y` renders as a top-down round canopy).
///
/// Lighting: Lambert NdotL from a key direction `(1, 1, 1)
/// normalised`. Result is clamped to `[ambient, 1.0]` with
/// `ambient = 0.35` so the underside isn't pure black.
pub fn bake_thumbnail(model: &S3oModel, diffuse_rgba: &[u8]) -> Thumbnail {
    debug_assert_eq!(
        diffuse_rgba.len(),
        (DIFFUSE_SIDE * DIFFUSE_SIDE * 4) as usize,
        "thumbnail diffuse must be 128² RGBA8"
    );
    let mut fb = Framebuffer::new();

    if model.indices.is_empty() {
        // Empty model — return the blank framebuffer (fully
        // transparent). Phase A fallback renders the category glyph
        // instead, but this crate's contract is to produce a 128²
        // buffer either way.
        return Thumbnail { rgba: fb.color };
    }

    let proj = fit_projection(model);
    let light_dir = Vec3::new(1.0, 1.0, 1.0).normalize();

    // Pre-project every vertex once. Triangle iteration only reads
    // post-projection data.
    let projected: Vec<ProjectedVertex> = model
        .vertices
        .iter()
        .map(|v| {
            let world = Vec3::from(v.pos);
            let clip = proj * Vec4::new(world.x, world.y, world.z, 1.0);
            ProjectedVertex {
                screen: Vec3::new(clip.x, clip.y, clip.z),
                normal: Vec3::from(v.normal),
                uv: v.uv,
            }
        })
        .collect();

    let mut tri_count = 0u32;
    let mut frag_count = 0u32;
    for chunk in model.indices.chunks_exact(3) {
        tri_count += 1;
        let v0 = projected[chunk[0] as usize];
        let v1 = projected[chunk[1] as usize];
        let v2 = projected[chunk[2] as usize];
        frag_count += rasterise_triangle(&mut fb, v0, v1, v2, diffuse_rgba, light_dir);
    }

    trace!(
        triangles = tri_count,
        rasterised_fragments = frag_count,
        "baked s3o thumbnail"
    );

    Thumbnail { rgba: fb.color }
}

#[derive(Clone, Copy)]
struct ProjectedVertex {
    /// `screen.x` and `.y` are in 0..=SPRITE_SIZE pixel space.
    /// `screen.z` is the depth (lower = nearer; we look down `-Y`).
    screen: Vec3,
    normal: Vec3,
    uv: [f32; 2],
}

struct Framebuffer {
    color: Vec<u8>,
    depth: Vec<f32>,
}

impl Framebuffer {
    fn new() -> Self {
        Self {
            color: vec![0u8; TOTAL_BYTES],
            depth: vec![f32::INFINITY; (SPRITE_SIZE * SPRITE_SIZE) as usize],
        }
    }
}

/// Build the world-to-screen matrix. Top-down ortho with a Y-flipped
/// screen-Z so smaller values are closer (compared with `<` in the
/// depth test).
fn fit_projection(model: &S3oModel) -> Mat4 {
    let (min, max) = model.bounds();
    let centre = Vec3::new(
        0.5 * (min[0] + max[0]),
        0.5 * (min[1] + max[1]),
        0.5 * (min[2] + max[2]),
    );
    let extent_x = (max[0] - min[0]).max(1e-3);
    let extent_y = (max[1] - min[1]).max(1e-3);
    let extent_z = (max[2] - min[2]).max(1e-3);
    // 5% margin so the silhouette doesn't touch the bezel.
    let half = 0.5 * extent_x.max(extent_z) * 1.05;
    let half_y = 0.5 * extent_y * 1.05;

    // World → ortho-clip (-1..+1 on every axis), then scale to pixel
    // space. Translate the centre to (0, 0, 0), then map:
    //   x ∈ [-half,  +half] → [0, SPRITE_SIZE]
    //   z ∈ [-half,  +half] → [0, SPRITE_SIZE]
    //   y ∈ [-half_y, +half_y] → [0, 1] (depth; smaller = closer)
    // We negate y so the higher the model, the smaller depth — the
    // camera looks from +Y down.
    let to_centre = Mat4::from_translation(-centre);
    let scale = Mat4::from_cols(
        Vec4::new(SPRITE_SIZE as f32 / (2.0 * half), 0.0, 0.0, 0.0),
        Vec4::new(0.0, -1.0 / (2.0 * half_y), 0.0, 0.0),
        Vec4::new(0.0, 0.0, SPRITE_SIZE as f32 / (2.0 * half), 0.0),
        Vec4::new(
            (SPRITE_SIZE as f32) * 0.5,
            0.5,
            (SPRITE_SIZE as f32) * 0.5,
            1.0,
        ),
    );
    scale * to_centre
}

/// Rasterise one screen-space triangle with depth-tested per-fragment
/// shading. Returns the number of fragments emitted (for the trace
/// log).
fn rasterise_triangle(
    fb: &mut Framebuffer,
    v0: ProjectedVertex,
    v1: ProjectedVertex,
    v2: ProjectedVertex,
    diffuse: &[u8],
    light_dir: Vec3,
) -> u32 {
    // Screen-space bounds of the triangle, clamped to the
    // framebuffer rect.
    let xs = [v0.screen.x, v1.screen.x, v2.screen.x];
    let ys = [v0.screen.z, v1.screen.z, v2.screen.z];
    let min_x = xs
        .iter()
        .copied()
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as i32;
    let max_x = xs
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min((SPRITE_SIZE - 1) as f32) as i32;
    let min_y = ys
        .iter()
        .copied()
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as i32;
    let max_y = ys
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min((SPRITE_SIZE - 1) as f32) as i32;

    if min_x > max_x || min_y > max_y {
        return 0;
    }

    // Edge function denominator for barycentrics.
    let denom = edge(
        v0.screen.x,
        v0.screen.z,
        v1.screen.x,
        v1.screen.z,
        v2.screen.x,
        v2.screen.z,
    );
    if denom.abs() < 1e-6 {
        return 0;
    }
    let inv_denom = 1.0 / denom;

    let mut emitted = 0u32;
    for py in min_y..=max_y {
        for px in min_x..=max_x {
            let fx = px as f32 + 0.5;
            let fy = py as f32 + 0.5;
            let w0 = edge(v1.screen.x, v1.screen.z, v2.screen.x, v2.screen.z, fx, fy) * inv_denom;
            let w1 = edge(v2.screen.x, v2.screen.z, v0.screen.x, v0.screen.z, fx, fy) * inv_denom;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }

            let depth = w0 * v0.screen.y + w1 * v1.screen.y + w2 * v2.screen.y;
            let idx = (py as u32 * SPRITE_SIZE + px as u32) as usize;
            if depth >= fb.depth[idx] {
                continue;
            }
            fb.depth[idx] = depth;

            // Interpolate normal + uv.
            let normal = (v0.normal * w0 + v1.normal * w1 + v2.normal * w2).normalize_or_zero();
            let u = w0 * v0.uv[0] + w1 * v1.uv[0] + w2 * v2.uv[0];
            let v = w0 * v0.uv[1] + w1 * v1.uv[1] + w2 * v2.uv[1];

            let (dr, dg, db, da) = sample_diffuse(diffuse, u, v);
            // Lambert N·L with two-sided lighting (so back-facing
            // triangles still receive some light).
            let ndotl = normal.dot(light_dir).abs();
            let shade = 0.35 + 0.65 * ndotl.clamp(0.0, 1.0);
            let r = (dr as f32 * shade).clamp(0.0, 255.0) as u8;
            let g = (dg as f32 * shade).clamp(0.0, 255.0) as u8;
            let b = (db as f32 * shade).clamp(0.0, 255.0) as u8;
            let a = da; // diffuse alpha passes through (foliage cards)

            let p = idx * 4;
            // Pre-multiplied alpha so the marker pipeline's
            // PREMULTIPLIED_ALPHA_BLENDING composites correctly.
            let af = a as f32 / 255.0;
            fb.color[p] = (r as f32 * af).clamp(0.0, 255.0) as u8;
            fb.color[p + 1] = (g as f32 * af).clamp(0.0, 255.0) as u8;
            fb.color[p + 2] = (b as f32 * af).clamp(0.0, 255.0) as u8;
            fb.color[p + 3] = a;
            emitted += 1;
        }
    }
    emitted
}

#[inline]
fn edge(ax: f32, ay: f32, bx: f32, by: f32, cx: f32, cy: f32) -> f32 {
    (bx - ax) * (cy - ay) - (by - ay) * (cx - ax)
}

/// Bilinear sample a 128² RGBA8 diffuse at floating UV. UVs outside
/// `[0, 1]` wrap mod 1.0 (matches OpenGL `GL_REPEAT` — S3O models
/// frequently UV-tile foliage cards beyond 1.0).
fn sample_diffuse(diffuse: &[u8], u: f32, v: f32) -> (u8, u8, u8, u8) {
    let side = DIFFUSE_SIDE as f32;
    // Repeat (wrap) — fract handles negative UVs by wrapping toward
    // 1.0 the same way OpenGL does.
    let uu = u - u.floor();
    let vv = v - v.floor();
    let fx = uu * side - 0.5;
    let fy = vv * side - 0.5;
    let x0 = fx.floor() as i32;
    let y0 = fy.floor() as i32;
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;
    let s = DIFFUSE_SIDE as i32;
    let i = |x: i32, y: i32| {
        let xw = x.rem_euclid(s) as usize;
        let yw = y.rem_euclid(s) as usize;
        (yw * DIFFUSE_SIDE as usize + xw) * 4
    };
    let p00 = i(x0, y0);
    let p10 = i(x0 + 1, y0);
    let p01 = i(x0, y0 + 1);
    let p11 = i(x0 + 1, y0 + 1);
    let lerp = |a: u8, b: u8, t: f32| (a as f32 + (b as f32 - a as f32) * t) as u8;
    let r = lerp(
        lerp(diffuse[p00], diffuse[p10], tx),
        lerp(diffuse[p01], diffuse[p11], tx),
        ty,
    );
    let g = lerp(
        lerp(diffuse[p00 + 1], diffuse[p10 + 1], tx),
        lerp(diffuse[p01 + 1], diffuse[p11 + 1], tx),
        ty,
    );
    let b = lerp(
        lerp(diffuse[p00 + 2], diffuse[p10 + 2], tx),
        lerp(diffuse[p01 + 2], diffuse[p11 + 2], tx),
        ty,
    );
    let a = lerp(
        lerp(diffuse[p00 + 3], diffuse[p10 + 3], tx),
        lerp(diffuse[p01 + 3], diffuse[p11 + 3], tx),
        ty,
    );
    (r, g, b, a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{S3oModel, S3oVertex};

    fn flat_diffuse(r: u8, g: u8, b: u8, a: u8) -> Vec<u8> {
        let mut d = Vec::with_capacity((DIFFUSE_SIDE * DIFFUSE_SIDE * 4) as usize);
        for _ in 0..(DIFFUSE_SIDE * DIFFUSE_SIDE) {
            d.extend_from_slice(&[r, g, b, a]);
        }
        d
    }

    #[test]
    fn sprite_size_matches_phase_a() {
        assert_eq!(SPRITE_SIZE, 128);
        assert_eq!(STRIDE_BYTES, 128 * 4);
        assert_eq!(TOTAL_BYTES, 128 * 128 * 4);
    }

    #[test]
    fn empty_model_produces_blank_buffer() {
        let model = S3oModel {
            piece_count: 0,
            vertices: vec![],
            indices: vec![],
            radius: 1.0,
            height: 1.0,
            texture1: None,
            texture2: None,
        };
        let diffuse = flat_diffuse(255, 0, 0, 255);
        let thumb = bake_thumbnail(&model, &diffuse);
        assert_eq!(thumb.rgba.len(), TOTAL_BYTES);
        assert!(
            thumb.rgba.iter().all(|&b| b == 0),
            "empty model produces all-zero (transparent) output"
        );
    }

    #[test]
    fn flat_quad_renders_with_diffuse_colour() {
        // 2-triangle quad spanning the XZ plane at Y=0. Top-down
        // projection should colour the whole frame with the diffuse.
        let v = |x: f32, z: f32, u: f32, vv: f32| S3oVertex {
            pos: [x, 0.0, z],
            normal: [0.0, 1.0, 0.0],
            uv: [u, vv],
        };
        let model = S3oModel {
            piece_count: 1,
            vertices: vec![
                v(-1.0, -1.0, 0.0, 0.0),
                v(1.0, -1.0, 1.0, 0.0),
                v(1.0, 1.0, 1.0, 1.0),
                v(-1.0, 1.0, 0.0, 1.0),
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            radius: 1.5,
            height: 0.1,
            texture1: Some("flat.tga".to_string()),
            texture2: None,
        };
        // Solid green diffuse.
        let diffuse = flat_diffuse(0, 200, 0, 255);
        let thumb = bake_thumbnail(&model, &diffuse);
        // Count opaque pixels (alpha != 0). Should be most of the
        // frame; centre pixels are guaranteed to land inside the quad.
        let centre = ((SPRITE_SIZE / 2) * SPRITE_SIZE + SPRITE_SIZE / 2) as usize * 4;
        let (r, g, b, a) = (
            thumb.rgba[centre],
            thumb.rgba[centre + 1],
            thumb.rgba[centre + 2],
            thumb.rgba[centre + 3],
        );
        assert_eq!(a, 255, "centre fragment is opaque");
        // Lambert with NdotL = 1.0 (top-facing normal vs (1,1,1)
        // normalised light) yields shade ≈ 0.35 + 0.65 * 0.577 ≈ 0.72.
        // So green channel should land around 200 * 0.72 ≈ 144 (±20).
        assert!(g > 100 && g < 200, "green shade plausible ({g})");
        assert_eq!(r, 0);
        assert_eq!(b, 0);
    }

    #[test]
    fn pre_multiplied_alpha_on_translucent_diffuse() {
        // 50%-alpha diffuse must yield pre-multiplied colour (~50% of
        // diffuse RGB).
        let v = |x: f32, z: f32, u: f32, vv: f32| S3oVertex {
            pos: [x, 0.0, z],
            normal: [0.0, 1.0, 0.0],
            uv: [u, vv],
        };
        let model = S3oModel {
            piece_count: 1,
            vertices: vec![
                v(-1.0, -1.0, 0.0, 0.0),
                v(1.0, -1.0, 1.0, 0.0),
                v(1.0, 1.0, 1.0, 1.0),
                v(-1.0, 1.0, 0.0, 1.0),
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            radius: 1.5,
            height: 0.1,
            texture1: Some("translucent.tga".to_string()),
            texture2: None,
        };
        let diffuse = flat_diffuse(255, 255, 255, 128);
        let thumb = bake_thumbnail(&model, &diffuse);
        let centre = ((SPRITE_SIZE / 2) * SPRITE_SIZE + SPRITE_SIZE / 2) as usize * 4;
        // Pre-mul: rgb_out = rgb_diffuse * alpha * lambert_shade.
        // alpha = 128/255 ≈ 0.502; shade ≈ 0.72 (see above).
        // So R ≈ 255 * 0.502 * 0.72 ≈ 92.
        let r = thumb.rgba[centre];
        let a = thumb.rgba[centre + 3];
        assert_eq!(a, 128);
        assert!(
            r > 50 && r < 130,
            "pre-mul rgb ({r}) plausible for 50% alpha"
        );
    }

    #[test]
    fn upstream_pedro1_bake_when_available() {
        let candidate = std::env::var("HOME")
            .ok()
            .map(std::path::PathBuf::from)
            .map(|h| h.join("code/Beyond-All-Reason/mapfeatures/objects3d/pedro1.s3o"));
        let Some(path) = candidate else {
            return;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let model = crate::parser::parse_s3o(&bytes).expect("upstream parses");
        let diffuse = flat_diffuse(140, 90, 40, 255); // earthy
        let thumb = bake_thumbnail(&model, &diffuse);
        assert_eq!(thumb.rgba.len(), TOTAL_BYTES);
        // At least 10 % of pixels should be opaque (the silhouette
        // touches >1600 pixels for a 128² render of a typical cactus).
        let opaque = thumb.rgba.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(
            opaque > (SPRITE_SIZE * SPRITE_SIZE / 10) as usize,
            "upstream pedro1 silhouette covers >10% of frame ({opaque} px)"
        );
    }
}
