//! Raise brush: smoothstep falloff, additive delta.
//!
//! `strength = 1.0` corresponds to `+STAMP_MAX_DELTA` u16 units per stamp at
//! center. ~20 stamps at full strength climb from 0 to u16::MAX.

use super::{Brush, BrushStamp, DirtyRect, ELMOS_PER_HEIGHTMAP_PIXEL, pixel_bbox, smoothstep};
use crate::Heightmap;

const STAMP_MAX_DELTA: f32 = 0.05 * u16::MAX as f32;

pub struct Raise;

impl Brush for Raise {
    fn id(&self) -> &'static str {
        "raise"
    }
    fn label(&self) -> &'static str {
        "Raise"
    }

    fn apply(&self, hm: &mut Heightmap, stamp: BrushStamp) -> Option<DirtyRect> {
        if stamp.strength <= 0.0 {
            return None;
        }
        let bbox = pixel_bbox(hm, stamp)?;
        apply_radial_delta(hm, stamp, bbox, stamp.strength * STAMP_MAX_DELTA);
        Some(bbox)
    }
}

/// Shared apply routine for raise/lower. `delta` is the signed u16-space
/// magnitude at falloff=1; scales by falloff per pixel.
//
// TODO(sprint-MT-brushes): per-row parallelism via rayon. Promotion gate
// per `docs/research/multithreading/PROPOSAL.md` §4 = "32-SMU support
// arrives OR user reports stroke lag." The current 0.79 ms / radius-1024
// stamp baseline (ADR-021) has 10× headroom under the 8 ms NFR.
//
// One-line lift when needed:
//
// ```rust
// use rayon::iter::{IndexedParallelIterator, ParallelIterator};
// use rayon::slice::ParallelSliceMut;
// let row_slice = &mut data[(bbox.y * w) as usize
//     ..((bbox.y + bbox.h) * w) as usize];
// row_slice.par_chunks_mut(w as usize).enumerate().for_each(|(lz, row)| {
//     let iz = bbox.y + lz as u32;
//     let dz = iz as f32 - cz_px;
//     for ix in bbox.x..bbox.x + bbox.w {
//         // … existing inner-loop body, indexing row[ix as usize] …
//     }
// });
// ```
//
// Per PROPOSAL §4: symmetric brush replication (ADR-019) writes N stamps
// per stroke and stamps can overlap, so parallelism must be WITHIN a
// stamp (this routine), not ACROSS stamps. The undo-snapshot bitset
// (ADR-033) is shared mutable state — when lifting, either give each
// row its own bitset and merge, or atomic-CAS at the u64 word level.
pub(super) fn apply_radial_delta(
    hm: &mut Heightmap,
    stamp: BrushStamp,
    bbox: DirtyRect,
    delta: f32,
) {
    let (w, _) = hm.dims();
    let r_px = (stamp.radius / ELMOS_PER_HEIGHTMAP_PIXEL).max(f32::EPSILON);
    let cx_px = stamp.world_x / ELMOS_PER_HEIGHTMAP_PIXEL;
    let cz_px = stamp.world_z / ELMOS_PER_HEIGHTMAP_PIXEL;
    let data = hm.data_mut();
    for iz in bbox.y..bbox.y + bbox.h {
        let dz = iz as f32 - cz_px;
        for ix in bbox.x..bbox.x + bbox.w {
            let dx = ix as f32 - cx_px;
            let d = (dx * dx + dz * dz).sqrt();
            if d > r_px {
                continue;
            }
            let t = 1.0 - d / r_px;
            let falloff = smoothstep(t);
            let idx = (iz * w + ix) as usize;
            let current = data[idx] as f32;
            let next = (current + delta * falloff).clamp(0.0, u16::MAX as f32);
            data[idx] = next as u16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MapSize;

    #[test]
    fn raise_strictly_increases_or_keeps_sample_at_center() {
        let mut hm = Heightmap::new(129, 129, vec![0u16; 129 * 129]).unwrap();
        let _ = MapSize::square(2); // sanity for dims
        let stamp = BrushStamp {
            world_x: 64.0 * 8.0, // pixel 64 → center of 129×129
            world_z: 64.0 * 8.0,
            radius: 16.0 * 8.0, // 16 px radius
            strength: 1.0,
        };
        let rect = Raise.apply(&mut hm, stamp).expect("must touch pixels");
        let center = hm.data()[64 * 129 + 64];
        assert!(center > 0, "center sample should rise: got {center}");
        // Outside the brush rect should be untouched.
        assert_eq!(hm.data()[0], 0);
        assert!(rect.w > 0 && rect.h > 0);
    }

    #[test]
    fn raise_with_zero_strength_returns_none() {
        let mut hm = Heightmap::new(129, 129, vec![0u16; 129 * 129]).unwrap();
        let stamp = BrushStamp {
            world_x: 100.0,
            world_z: 100.0,
            radius: 32.0,
            strength: 0.0,
        };
        assert!(Raise.apply(&mut hm, stamp).is_none());
    }
}
