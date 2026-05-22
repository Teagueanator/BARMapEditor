//! Sprint 29 / R5 / ADR-046: decode upstream feature decals to fixed
//! 128² RGBA8 buffers for the [`FeatureDecalRegistry`] texture-array
//! upload.
//!
//! `tools/feature-decals/<family>/diffuse.<ext>` is populated by
//! `scripts/fetch-feature-decals.sh`. Phase A exclusively loads `.tga`
//! diffuses (every verified upstream family in v3 catalog uses TGA).
//! `.png` + `.bmp` are routed through the same `image`-crate path for
//! free. `.dds` is reserved for Sprint 29b (Phase B — S3O texture
//! refs); the dispatcher returns a typed error rather than silently
//! skipping so the registry can log the gap.
//!
//! ## Sizing
//!
//! 128² × 4 B = 64 KB / layer. With 16 families decoded today the
//! texture array is ~1 MB — well inside the iGPU budget noted in
//! PITFALLS §1. The size is `SPRITE_SIZE`; bump only with a
//! corresponding revision to the bind-group resource and a fresh
//! memory-budget audit.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use image::imageops::FilterType;

/// Fixed sprite side length for the texture array. Phase A pin —
/// see module docs.
pub const SPRITE_SIZE: u32 = 128;

/// One decoded feature decal — `SPRITE_SIZE × SPRITE_SIZE` RGBA8 in
/// row-major order. The exact byte count is asserted in tests.
#[derive(Debug, Clone)]
pub struct DecalImage {
    pub rgba: Vec<u8>,
}

impl DecalImage {
    /// `SPRITE_SIZE * 4` bytes per row. Exported so downstream
    /// callers can size scratch buffers without re-deriving from
    /// [`SPRITE_SIZE`]; only used by tests today, kept as part of
    /// the module's stable surface.
    #[allow(dead_code)]
    pub const STRIDE_BYTES: usize = SPRITE_SIZE as usize * 4;
    /// Total RGBA8 byte count for one sprite.
    pub const TOTAL_BYTES: usize = SPRITE_SIZE as usize * SPRITE_SIZE as usize * 4;
}

/// Decode a feature decal diffuse to a 128² RGBA8 buffer.
///
/// Dispatch by file extension. Phase A handles `.tga`, `.png`, and
/// `.bmp` via the `image` crate; `.dds` returns
/// [`DecalError::DdsUnsupported`] (Sprint 29b adds the BC1/BC3
/// decoder via `bcdec_rs`).
pub fn load_decal(path: &Path) -> Result<DecalImage, DecalError> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .ok_or_else(|| {
            DecalError::Other(anyhow!("decal path missing extension: {}", path.display()))
        })?;

    match ext.as_str() {
        "tga" | "png" | "bmp" => load_via_image(path).map_err(DecalError::Other),
        "dds" => Err(DecalError::DdsUnsupported(path.to_path_buf())),
        other => Err(DecalError::Other(anyhow!(
            "unsupported decal format `.{other}`: {}",
            path.display()
        ))),
    }
}

/// Phase A delegates TGA / PNG / BMP straight to the `image` crate;
/// resize to 128² via Lanczos3. The input texture's native size is
/// arbitrary — upstream diffuses range from 512² (peyote) up to
/// 4096² (allpinesb_ad0).
fn load_via_image(path: &Path) -> anyhow::Result<DecalImage> {
    let img =
        image::open(path).with_context(|| format!("image::open failed: {}", path.display()))?;
    let rgba = img.to_rgba8();
    let resized = image::imageops::resize(&rgba, SPRITE_SIZE, SPRITE_SIZE, FilterType::Lanczos3);
    let bytes = resized.into_raw();
    debug_assert_eq!(bytes.len(), DecalImage::TOTAL_BYTES);
    Ok(DecalImage { rgba: bytes })
}

/// Typed failure mode for [`load_decal`]. The registry consults the
/// variant so it can warn loudly on Phase A's DDS gap without
/// conflating it with corrupted files.
#[derive(Debug, thiserror::Error)]
pub enum DecalError {
    #[error("DDS decode not yet implemented (Sprint 29b / Phase B): {0}")]
    DdsUnsupported(std::path::PathBuf),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    /// Encode a synthetic 64² RGBA gradient to a tempfile-backed TGA,
    /// load it back through `load_decal`, and assert the buffer is the
    /// right shape + non-degenerate. Doesn't depend on `tools/`
    /// being populated — the test owns its fixture end-to-end.
    #[test]
    fn loads_tga_to_fixed_128_rgba() {
        let tmp = tempfile::NamedTempFile::with_suffix(".tga").unwrap();
        let mut img = ImageBuffer::<Rgba<u8>, _>::new(64, 64);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = Rgba([x as u8 * 4, y as u8 * 4, 128, 255]);
        }
        img.save(tmp.path()).unwrap();

        let decoded = load_decal(tmp.path()).expect("loads fixture tga");
        assert_eq!(decoded.rgba.len(), DecalImage::TOTAL_BYTES);
        // Spot-check that the image isn't all zeros after resize.
        let nonzero = decoded.rgba.iter().filter(|&&b| b != 0).count();
        assert!(
            nonzero > DecalImage::TOTAL_BYTES / 4,
            "resize produced mostly-zero buffer"
        );
    }

    #[test]
    fn dds_returns_typed_unsupported_error() {
        // Phase A: .dds is an explicit gap signal, not silent skip.
        let p = std::path::PathBuf::from("/tmp/nonexistent.dds");
        match load_decal(&p) {
            Err(DecalError::DdsUnsupported(path)) => assert_eq!(path, p),
            other => panic!("expected DdsUnsupported, got {other:?}"),
        }
    }

    #[test]
    fn unknown_extension_errors() {
        let p = std::path::PathBuf::from("/tmp/x.exr");
        assert!(matches!(load_decal(&p), Err(DecalError::Other(_))));
    }

    #[test]
    fn no_extension_errors() {
        let p = std::path::PathBuf::from("/tmp/no_ext_at_all");
        assert!(matches!(load_decal(&p), Err(DecalError::Other(_))));
    }

    #[test]
    fn sprite_size_is_128() {
        // Phase A pin — bumping requires a memory-budget audit; pin
        // here catches silent drift.
        assert_eq!(SPRITE_SIZE, 128);
        assert_eq!(DecalImage::STRIDE_BYTES, 128 * 4);
        assert_eq!(DecalImage::TOTAL_BYTES, 128 * 128 * 4);
    }
}
