//! Smooth brush: blend toward 3×3 neighbour mean, weighted by smoothstep
//! falloff × strength.
//!
//! Reads from a snapshot of the bounding rect so propagation doesn't bias
//! the result on a single pass.

use super::{Brush, BrushStamp, DirtyRect, ELMOS_PER_HEIGHTMAP_PIXEL, pixel_bbox, smoothstep};
use crate::Heightmap;

pub struct Smooth;

impl Brush for Smooth {
    fn id(&self) -> &'static str {
        "smooth"
    }
    fn label(&self) -> &'static str {
        "Smooth"
    }

    fn apply(&self, hm: &mut Heightmap, stamp: BrushStamp) -> Option<DirtyRect> {
        if stamp.strength <= 0.0 {
            return None;
        }
        let bbox = pixel_bbox(hm, stamp)?;
        let (w, h) = hm.dims();
        let r_px = (stamp.radius / ELMOS_PER_HEIGHTMAP_PIXEL).max(f32::EPSILON);
        let cx_px = stamp.world_x / ELMOS_PER_HEIGHTMAP_PIXEL;
        let cz_px = stamp.world_z / ELMOS_PER_HEIGHTMAP_PIXEL;
        let s = stamp.strength.clamp(0.0, 1.0);

        // Snapshot the area we'll sample from (bbox + 1 pixel margin for
        // the 3×3 kernel). Read from snapshot, write to hm.data_mut().
        let snap_x = bbox.x.saturating_sub(1);
        let snap_y = bbox.y.saturating_sub(1);
        let snap_r = (bbox.x + bbox.w + 1).min(w);
        let snap_b = (bbox.y + bbox.h + 1).min(h);
        let snap_w = snap_r - snap_x;
        let snap_h = snap_b - snap_y;
        let src = hm.data();
        let mut snap = Vec::with_capacity((snap_w * snap_h) as usize);
        for iz in snap_y..snap_b {
            let row_start = (iz * w + snap_x) as usize;
            snap.extend_from_slice(&src[row_start..row_start + snap_w as usize]);
        }
        let neighbour = |ix: u32, iz: u32| -> u32 {
            let lx = (ix - snap_x) as usize;
            let lz = (iz - snap_y) as usize;
            snap[lz * snap_w as usize + lx] as u32
        };

        // TODO(sprint-MT-brushes): per-row parallelism via rayon.
        // Promotion gate per `docs/research/multithreading/PROPOSAL.md`
        // §4 = "32-SMU support arrives OR user reports stroke lag." The
        // snapshot read above is already deferred from the parallel
        // section (immutable `snap` is safe to share via `&` across
        // rayon workers); only the writeback loop below needs lifting:
        //
        // ```rust
        // use rayon::iter::{IndexedParallelIterator, ParallelIterator};
        // use rayon::slice::ParallelSliceMut;
        // let row_slice = &mut data[(bbox.y * w) as usize
        //     ..((bbox.y + bbox.h) * w) as usize];
        // row_slice.par_chunks_mut(w as usize).enumerate().for_each(|(lz, row)| {
        //     let iz = bbox.y + lz as u32;
        //     // … existing inner-loop body, reading `snap` (Sync), …
        //     // … writing row[ix as usize] = next as u16; …
        // });
        // ```
        //
        // Same symmetric-brush / undo-bitset caveats as raise.rs.
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
                // 3×3 sum (clamped to map bounds via the saturating snap).
                let xlo = ix.saturating_sub(1).max(snap_x);
                let xhi = (ix + 1).min(snap_r - 1);
                let zlo = iz.saturating_sub(1).max(snap_y);
                let zhi = (iz + 1).min(snap_b - 1);
                let mut sum = 0u32;
                let mut count = 0u32;
                for nz in zlo..=zhi {
                    for nx in xlo..=xhi {
                        sum += neighbour(nx, nz);
                        count += 1;
                    }
                }
                if count == 0 {
                    continue;
                }
                let avg = sum as f32 / count as f32;
                let idx = (iz * w + ix) as usize;
                let orig = data[idx] as f32;
                let mix = s * falloff;
                let next = (orig * (1.0 - mix) + avg * mix).clamp(0.0, u16::MAX as f32);
                data[idx] = next as u16;
            }
        }
        Some(bbox)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smooth_reduces_local_variance_around_a_spike() {
        // Spike in the center, zero everywhere else.
        let mut hm = Heightmap::new(33, 33, vec![0u16; 33 * 33]).unwrap();
        hm.data_mut()[16 * 33 + 16] = 60000;
        let before_var = local_variance(&hm, 16, 16, 5);
        let stamp = BrushStamp {
            world_x: 16.0 * 8.0,
            world_z: 16.0 * 8.0,
            radius: 8.0 * 8.0,
            strength: 1.0,
        };
        // Apply many stamps to actually move the variance.
        for _ in 0..8 {
            Smooth.apply(&mut hm, stamp).unwrap();
        }
        let after_var = local_variance(&hm, 16, 16, 5);
        assert!(
            after_var < before_var,
            "smoothing should reduce variance: before={before_var:.0}, after={after_var:.0}"
        );
    }

    fn local_variance(hm: &Heightmap, cx: u32, cz: u32, radius: u32) -> f64 {
        let (w, h) = hm.dims();
        let data = hm.data();
        let mut sum = 0f64;
        let mut sq = 0f64;
        let mut n = 0f64;
        for iz in cz.saturating_sub(radius)..=(cz + radius).min(h - 1) {
            for ix in cx.saturating_sub(radius)..=(cx + radius).min(w - 1) {
                let v = data[(iz * w + ix) as usize] as f64;
                sum += v;
                sq += v * v;
                n += 1.0;
            }
        }
        let mean = sum / n;
        sq / n - mean * mean
    }
}
