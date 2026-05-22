//! Offscreen 128² thumbnail render pass. Commit 1 (this commit)
//! lands the public signature + a synchronous stub that returns a
//! solid-colour placeholder; commit 3 of Sprint 29b wires the actual
//! wgpu pipeline.
//!
//! The signature is fixed by `barme-app::feature_decals` (a thumbnail
//! must produce exactly `SPRITE_SIZE × SPRITE_SIZE × 4` RGBA8 bytes
//! so the existing texture array upload path consumes the output
//! transparently).

use crate::parser::S3oModel;

/// Width / height of a baked thumbnail. Must match
/// `barme_app::feature_decals::SPRITE_SIZE`. Duplicated here so this
/// crate stays leaf-independent of the editor binary.
pub const SPRITE_SIZE: u32 = 128;

/// Bytes per row of a baked thumbnail.
pub const STRIDE_BYTES: usize = SPRITE_SIZE as usize * 4;

/// Total RGBA8 byte count of one thumbnail.
pub const TOTAL_BYTES: usize = SPRITE_SIZE as usize * SPRITE_SIZE as usize * 4;

/// One baked thumbnail in RGBA8, row-major. Same shape as
/// `barme_app::feature_decals::DecalImage::rgba` so the registry
/// upload path consumes either source uniformly.
#[derive(Debug, Clone)]
pub struct Thumbnail {
    pub rgba: Vec<u8>,
}

/// Bake a thumbnail of one S3O model. Phase B / commit 3 implements
/// the real render pass: offscreen 128² `Rgba8UnormSrgb` colour +
/// `Depth32Float` depth, orthographic top-down camera fitted to the
/// model's bounding sphere (`model.radius`), neutral key+fill
/// lighting, the `diffuse_rgba` 128² texture sampled as the surface
/// colour, pre-multiplied alpha out.
///
/// Commit 1 (this commit) returns a solid mid-grey buffer so the
/// caller's wiring + cache pathways can be exercised before the wgpu
/// pipeline lands.
pub fn bake_thumbnail(_model: &S3oModel, _diffuse_rgba: &[u8]) -> Thumbnail {
    Thumbnail {
        rgba: vec![0x80; TOTAL_BYTES],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprite_size_matches_phase_a() {
        // If Phase A bumps SPRITE_SIZE we'll need to mirror it; the
        // pin catches silent drift between barme-app and this crate.
        assert_eq!(SPRITE_SIZE, 128);
        assert_eq!(STRIDE_BYTES, 128 * 4);
        assert_eq!(TOTAL_BYTES, 128 * 128 * 4);
    }

    #[test]
    fn stub_bake_returns_full_size_buffer() {
        let model = S3oModel {
            piece_count: 0,
            vertices: vec![],
            indices: vec![],
            radius: 1.0,
            height: 1.0,
            texture1: None,
            texture2: None,
        };
        let diffuse = vec![0u8; TOTAL_BYTES];
        let thumb = bake_thumbnail(&model, &diffuse);
        assert_eq!(thumb.rgba.len(), TOTAL_BYTES);
    }
}
