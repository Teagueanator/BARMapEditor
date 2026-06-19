//! Grass density bake (Sprint 34 / R6 / ADR-050).
//!
//! BAR grows grass on flat, terrain-type-0 ground and thins it out on
//! slopes. The engine's `CGrassDrawer` rebuilds a per-turf straw count
//! from the type-map mask × a slope term every time the map changes.
//! The editor mirrors that with a CPU bake: one normalised coverage
//! byte per heightmap texel, sampled later by the instance generator
//! (`barme-app::grass`) to decide how many blades to scatter.
//!
//! ## What drives density
//!
//! 1. **Terrain-type mask.** BAR convention: grass only on
//!    `terrain_types[0]`. The type-map editor is F15 (Sprint 36) and
//!    not shipped yet, so for Sprint 34 the mask is uniformly `1.0`
//!    (every texel is type 0). [`bake_grass_density`] takes no type-map
//!    argument today; when F15 lands it grows a `&TypeMap` parameter
//!    and the mask stops being constant. Documented in ADR-050.
//! 2. **Slope falloff.** The primary modulator this sprint. A
//!    sigmoid over the local gradient magnitude: ~full coverage on
//!    flats, fading to nothing past a cliff angle. Matches the engine
//!    intent (no grass on near-vertical rock) without porting its exact
//!    curve.
//! 3. **`maxStrawsPerTurf == 0`** zeroes the whole field — an explicit
//!    "no grass" authoring escape hatch. The non-zero magnitude is
//!    applied later, per-blade, by the instance generator; the texture
//!    itself stays a pure `0..=1` coverage map so it survives a
//!    `maxStrawsPerTurf` edit without a re-bake.
//!
//! The output is square (matches the heightmap), persists to PNG for
//! reuse across runs, and is fully deterministic for a given input.

use std::path::Path;

use anyhow::{Context, Result};

use crate::Heightmap;
use crate::mapinfo_schema::GrassBlock;

/// World elmos spanned by one heightmap texel along each axis. Spring
/// samples the heightmap at 8 elmos/texel (SRS §map-format). Used as
/// the horizontal run when converting height deltas into a slope.
pub const ELMOS_PER_TEXEL: f32 = 8.0;

/// Slope (rise/run, i.e. `tan θ`) at which coverage is exactly 50 %.
/// `0.5` ≈ 27°. Below this grass is dense; above it thins fast.
pub const SLOPE_MIDPOINT: f32 = 0.5;

/// Sigmoid sharpness around [`SLOPE_MIDPOINT`]. Higher = a crisper
/// flat-vs-cliff transition. `8.0` gives ~0.98 coverage on dead-flat
/// ground and ~0.02 by `tan θ = 1.0` (45°).
pub const SLOPE_SHARPNESS: f32 = 8.0;

/// Normalised grass coverage, one byte per heightmap texel.
///
/// `texture[y * dim.0 + x]` is `0..=255` where `255` = full coverage
/// (max blades this turf can hold) and `0` = bare ground. The
/// instance generator multiplies this fraction by
/// `grass.max_straws_per_turf` to get a blade count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrassDensity {
    /// One coverage byte per texel, row-major, top-left origin —
    /// matches [`Heightmap`] storage so index arithmetic is shared.
    pub texture: Vec<u8>,
    /// `(width, height)`, equal to the source heightmap dims
    /// (`64·N + 1` per side).
    pub dim: (u32, u32),
}

impl GrassDensity {
    /// Coverage fraction `0.0..=1.0` at a texel (clamped to bounds).
    pub fn coverage(&self, x: u32, y: u32) -> f32 {
        let x = x.min(self.dim.0.saturating_sub(1));
        let y = y.min(self.dim.1.saturating_sub(1));
        let idx = (y as usize) * (self.dim.0 as usize) + (x as usize);
        self.texture[idx] as f32 / 255.0
    }

    /// Persist as an 8-bit grayscale PNG (one byte = one coverage
    /// sample). Caller chooses the path — the app writes
    /// `<project>/.barme-cache/grass-density.png`. Parent dirs are
    /// created if missing.
    pub fn save_png(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create grass-density cache dir {}", parent.display()))?;
        }
        let img = image::GrayImage::from_raw(self.dim.0, self.dim.1, self.texture.clone())
            .context("grass density buffer length does not match dims")?;
        img.save(path)
            .with_context(|| format!("write grass density png {}", path.display()))?;
        Ok(())
    }

    /// Load a previously-baked density PNG. Returns `Ok(None)` when the
    /// file is absent (cold cache → caller bakes fresh); errors only on
    /// a present-but-unreadable file.
    pub fn load_png(path: impl AsRef<Path>) -> Result<Option<Self>> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(None);
        }
        let img = image::ImageReader::open(path)
            .with_context(|| format!("open grass density png {}", path.display()))?
            .decode()
            .with_context(|| format!("decode grass density png {}", path.display()))?
            .to_luma8();
        let dim = (img.width(), img.height());
        Ok(Some(Self {
            texture: img.into_raw(),
            dim,
        }))
    }
}

/// Map a slope (rise/run) to a coverage fraction via a logistic
/// falloff: ~1.0 on flat ground, dropping through 0.5 at
/// [`SLOPE_MIDPOINT`], approaching 0.0 on cliffs.
fn slope_falloff(slope: f32) -> f32 {
    1.0 / (1.0 + (SLOPE_SHARPNESS * (slope - SLOPE_MIDPOINT)).exp())
}

/// Bake the grass coverage texture for a heightmap.
///
/// `vertical_scale` is the world height (elmos) that the full 16-bit
/// sample range maps to — i.e. `max_height - min_height` for the
/// project. It converts raw `u16` deltas into world rise so the slope
/// term is in true rise/run units regardless of the project's height
/// band. A flat map (`vertical_scale == 0`) yields uniform full
/// coverage.
///
/// `grass.max_straws_per_turf == 0` zeroes the field (explicit "no
/// grass"); any other value leaves the coverage map untouched — the
/// straw magnitude is applied per-blade downstream.
///
/// The terrain-type-0 mask is uniformly `1.0` for Sprint 34 (F15 not
/// shipped); see the module docs.
pub fn bake_grass_density(
    heightmap: &Heightmap,
    grass: &GrassBlock,
    vertical_scale: f32,
) -> GrassDensity {
    let (w, h) = heightmap.dims();
    let data = heightmap.data();
    let px_count = (w as usize) * (h as usize);

    // Authoring escape hatch — zero straws means a bare map.
    if grass.max_straws_per_turf_or_default() == 0 {
        return GrassDensity {
            texture: vec![0u8; px_count],
            dim: (w, h),
        };
    }

    // World height per raw u16 step. `0` for a flat band → no slope
    // contribution, full coverage everywhere.
    let height_per_step = if vertical_scale > 0.0 {
        vertical_scale / u16::MAX as f32
    } else {
        0.0
    };

    let sample = |x: u32, y: u32| -> f32 {
        let xi = x.min(w - 1) as usize;
        let yi = y.min(h - 1) as usize;
        data[yi * (w as usize) + xi] as f32
    };

    let mut texture = vec![0u8; px_count];
    for y in 0..h {
        for x in 0..w {
            // Central difference on the clamped neighbourhood; the run
            // is two texels (2 × ELMOS_PER_TEXEL) of world distance.
            let xl = sample(x.saturating_sub(1), y);
            let xr = sample(x + 1, y);
            let yt = sample(x, y.saturating_sub(1));
            let yb = sample(x, y + 1);
            let dzdx = (xr - xl) * height_per_step / (2.0 * ELMOS_PER_TEXEL);
            let dzdy = (yb - yt) * height_per_step / (2.0 * ELMOS_PER_TEXEL);
            let slope = (dzdx * dzdx + dzdy * dzdy).sqrt();
            let coverage = slope_falloff(slope).clamp(0.0, 1.0);
            texture[(y as usize) * (w as usize) + (x as usize)] = (coverage * 255.0).round() as u8;
        }
    }

    GrassDensity {
        texture,
        dim: (w, h),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MapSize;

    fn flat(size: MapSize, value: u16) -> Heightmap {
        let (w, h) = size.heightmap_dims();
        Heightmap::new(w, h, vec![value; (w as usize) * (h as usize)]).unwrap()
    }

    #[test]
    fn flat_map_is_fully_grassed() {
        let hm = flat(MapSize::square(2), 30_000);
        let d = bake_grass_density(&hm, &GrassBlock::default(), 500.0);
        assert_eq!(d.dim, hm.dims());
        // Dead-flat ground → slope 0 → sigmoid ~0.98 → 250..=255.
        assert!(
            d.texture.iter().all(|&b| b >= 250),
            "flat terrain should be near-full coverage; min={}",
            d.texture.iter().min().unwrap()
        );
    }

    #[test]
    fn zero_straws_yields_bare_field() {
        let hm = flat(MapSize::square(2), 30_000);
        let grass = GrassBlock {
            max_straws_per_turf: Some(0),
            ..Default::default()
        };
        let d = bake_grass_density(&hm, &grass, 500.0);
        assert!(d.texture.iter().all(|&b| b == 0), "zero straws → bare");
    }

    #[test]
    fn steep_slope_thins_grass() {
        // A hard ramp: height climbs by the full band over the map.
        let size = MapSize::square(2);
        let (w, h) = size.heightmap_dims();
        let mut data = vec![0u16; (w as usize) * (h as usize)];
        for y in 0..h {
            for x in 0..w {
                // Steep diagonal: each texel step adds a big delta.
                let v = ((x + y) as u32 * (u16::MAX as u32) / (w + h)) as u16;
                data[(y as usize) * (w as usize) + (x as usize)] = v;
            }
        }
        let hm = Heightmap::new(w, h, data).unwrap();
        // Large vertical scale exaggerates the slope past the midpoint.
        let d = bake_grass_density(&hm, &GrassBlock::default(), 4000.0);
        let interior = d.coverage(w / 2, h / 2);
        assert!(
            interior < 0.5,
            "steep interior should thin below half coverage; got {interior}"
        );
    }

    #[test]
    fn png_roundtrips() {
        let hm = flat(MapSize::square(2), 12_345);
        let d = bake_grass_density(&hm, &GrassBlock::default(), 250.0);
        let dir = std::env::temp_dir().join("barme-grass-test");
        let path = dir.join("grass-density.png");
        let _ = std::fs::remove_file(&path);
        // Cold cache → None.
        assert!(GrassDensity::load_png(&path).unwrap().is_none());
        d.save_png(&path).unwrap();
        let loaded = GrassDensity::load_png(&path).unwrap().unwrap();
        assert_eq!(loaded, d);
        let _ = std::fs::remove_file(&path);
    }
}
