//! Per-blade grass instance generation (Sprint 34 / R6 / ADR-050).
//!
//! Turns the CPU density bake ([`barme_core::GrassDensity`]) into a flat
//! list of [`GrassInstance`]s the GPU draws as instanced billboard
//! quads. One instance = one blade: a world anchor, a Y rotation, a
//! height scale, and a colour jitter.
//!
//! ## Determinism (pitfall #4)
//!
//! Every blade's jitter is a pure function of its **turf grid cell**
//! (world-anchored integer coords) and its index within that cell —
//! never of the camera. A `fmix32` integer hash drives the PRNG. So a
//! blade keeps the exact same position/rotation/scale frame to frame as
//! the camera moves; the field never shimmers. The whole function is
//! deterministic: same inputs → byte-identical output.
//!
//! ## LOD (pitfall #9)
//!
//! Instances are only emitted for turfs whose centre is within
//! `max_distance` (≈200 elmos) of the camera in the XZ plane. The
//! shader fades blade alpha toward that radius so they don't pop. The
//! total blade count is capped at [`GRASS_MAX_BLADES`]; when a dense
//! field would exceed it, a global scale is applied uniformly (no
//! spatial bias) so the budget holds on a Vega 8 iGPU.
//!
//! ## Turf granularity
//!
//! `maxStrawsPerTurf` is a *per-turf* count in BAR. A turf is a
//! [`TURF_SPACING_ELMOS`]-sided cell (matching the engine's 16-elmo
//! grass-map resolution), NOT a single heightmap texel — scattering
//! `maxStrawsPerTurf` blades per 8-elmo texel would be ~150× over
//! budget. Blade count per turf = `coverage × maxStrawsPerTurf ×
//! density_scale`.

use barme_core::grass::ELMOS_PER_TEXEL;
use barme_core::{GrassBlock, GrassDensity, Heightmap};
use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use tracing::trace;

/// World side length (elmos) of one grass turf — the engine's grass-map
/// texel size. `maxStrawsPerTurf` blades populate one such cell at full
/// coverage.
pub const TURF_SPACING_ELMOS: f32 = 16.0;

/// Hard upper bound on emitted blades. Sized to the Sprint-34 frame
/// budget on a Vega 8 iGPU (100k blades × 6 verts ≈ 600k verts, <4 ms).
/// A field that would exceed this is scaled down uniformly.
pub const GRASS_MAX_BLADES: usize = 100_000;

/// One grass blade, laid out for a wgpu instance buffer.
///
/// Field order is GPU-packing, not the SRS prose order: two `vec4`
/// slots (`position.xyz + orientation`, `color_jitter.xyz +
/// height_scale`) so the 32-byte stride needs no padding. The vertex
/// shader maps the attributes back by offset (see
/// `render::grass_instance_layout`).
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Pod, Zeroable)]
pub struct GrassInstance {
    /// World anchor (XYZ, elmos) — the blade's base sits on the terrain.
    pub position: [f32; 3],
    /// Rotation around +Y (radians). Per-blade jitter so a billboarded
    /// field looks varied from every angle (pitfall #2).
    pub orientation: f32,
    /// Per-channel multiplier on `grass.bladeColor` (≈0.85..1.15).
    pub color_jitter: [f32; 3],
    /// Blade-height multiplier (≈0.8..1.2) on `grass.bladeHeight`.
    pub height_scale: f32,
}

/// murmur3 `fmix32` integer finalizer — good avalanche for a cheap
/// position hash.
#[inline]
fn fmix32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

#[inline]
fn hash2(a: u32, b: u32) -> u32 {
    fmix32(a ^ fmix32(b.wrapping_mul(0x9e37_79b9)))
}

/// Hash a seed to a float in `[0, 1)`. Uses the top 24 bits so the
/// mantissa is exact.
#[inline]
fn rand01(seed: u32) -> f32 {
    (fmix32(seed) >> 8) as f32 / ((1u32 << 24) as f32)
}

/// Bilinear terrain height (world elmos) at fractional texel coords,
/// replicating the terrain shader's `min_h + raw/65535 * (max_h -
/// min_h)` mapping so blades sit exactly on the rendered surface.
fn sample_height(hm: &Heightmap, fx: f32, fz: f32, min_h: f32, max_h: f32) -> f32 {
    let (w, h) = hm.dims();
    let data = hm.data();
    let fx = fx.clamp(0.0, (w - 1) as f32);
    let fz = fz.clamp(0.0, (h - 1) as f32);
    let x0 = fx.floor() as u32;
    let z0 = fz.floor() as u32;
    let x1 = (x0 + 1).min(w - 1);
    let z1 = (z0 + 1).min(h - 1);
    let tx = fx - x0 as f32;
    let tz = fz - z0 as f32;
    let at = |x: u32, z: u32| data[(z as usize) * (w as usize) + (x as usize)] as f32;
    let top = at(x0, z0) * (1.0 - tx) + at(x1, z0) * tx;
    let bot = at(x0, z1) * (1.0 - tx) + at(x1, z1) * tx;
    let raw = top * (1.0 - tz) + bot * tz;
    min_h + (raw / 65535.0) * (max_h - min_h)
}

/// Number of turf-grid cells per axis covering the heightmap.
fn turf_count(hm: &Heightmap) -> (i32, i32) {
    let (w, h) = hm.dims();
    let world_x = (w - 1) as f32 * ELMOS_PER_TEXEL;
    let world_z = (h - 1) as f32 * ELMOS_PER_TEXEL;
    (
        (world_x / TURF_SPACING_ELMOS).ceil() as i32 + 1,
        (world_z / TURF_SPACING_ELMOS).ceil() as i32 + 1,
    )
}

/// Generate grass blade instances for the current camera.
///
/// `height_range` is `(min_height, max_height)` in world elmos — the
/// same band the terrain shader uses for its `u16 → Y` mapping.
/// `density_scale` (`0.0..=1.0`) is the View-menu throttle (pitfall #1);
/// `1.0` = full density. Returns an empty vec when the field is bare,
/// disabled, or off-screen.
pub fn generate_grass_instances(
    density: &GrassDensity,
    heightmap: &Heightmap,
    grass: &GrassBlock,
    height_range: (f32, f32),
    camera_pos: Vec3,
    max_distance: f32,
    density_scale: f32,
) -> Vec<GrassInstance> {
    let max_straws = grass.max_straws_per_turf_or_default();
    let density_scale = density_scale.clamp(0.0, 1.0);
    if max_straws == 0 || density_scale <= 0.0 || max_distance <= 0.0 {
        return Vec::new();
    }

    let (min_h, max_h) = height_range;
    let (tx_count, tz_count) = turf_count(heightmap);
    let (w, h) = heightmap.dims();
    let world_max_x = (w - 1) as f32 * ELMOS_PER_TEXEL;
    let world_max_z = (h - 1) as f32 * ELMOS_PER_TEXEL;
    let max_dist_sq = max_distance * max_distance;

    // Turf-index window around the camera (clamped to the map), so we
    // don't sweep the whole map for a small LOD radius.
    let pad = (max_distance / TURF_SPACING_ELMOS).ceil() as i32 + 1;
    let cam_tx = (camera_pos.x / TURF_SPACING_ELMOS).floor() as i32;
    let cam_tz = (camera_pos.z / TURF_SPACING_ELMOS).floor() as i32;
    let tx_lo = (cam_tx - pad).max(0);
    let tx_hi = (cam_tx + pad).min(tx_count - 1);
    let tz_lo = (cam_tz - pad).max(0);
    let tz_hi = (cam_tz + pad).min(tz_count - 1);

    // Pass 1 — per-turf blade count + total, so an over-budget field is
    // scaled uniformly (no spatial bias from truncation).
    struct Turf {
        ix: i32,
        iz: i32,
        cx: f32,
        cz: f32,
        count: u32,
    }
    let mut turfs: Vec<Turf> = Vec::new();
    let mut total: u64 = 0;
    for iz in tz_lo..=tz_hi {
        for ix in tx_lo..=tx_hi {
            let cx = ix as f32 * TURF_SPACING_ELMOS;
            let cz = iz as f32 * TURF_SPACING_ELMOS;
            let dx = cx - camera_pos.x;
            let dz = cz - camera_pos.z;
            if dx * dx + dz * dz > max_dist_sq {
                continue;
            }
            // Coverage at the turf centre's heightmap texel.
            let tex_x = (cx / ELMOS_PER_TEXEL).round() as u32;
            let tex_z = (cz / ELMOS_PER_TEXEL).round() as u32;
            let coverage = density.coverage(tex_x, tex_z);
            let count = (coverage * max_straws as f32 * density_scale).round() as u32;
            if count == 0 {
                continue;
            }
            total += count as u64;
            turfs.push(Turf {
                ix,
                iz,
                cx,
                cz,
                count,
            });
        }
    }

    // Global scale to honour the blade budget.
    let scale = if total as usize > GRASS_MAX_BLADES {
        GRASS_MAX_BLADES as f32 / total as f32
    } else {
        1.0
    };

    // Pass 2 — scatter blades. Jitter keyed on (turf cell, blade index).
    let mut out: Vec<GrassInstance> =
        Vec::with_capacity((total as usize).min(GRASS_MAX_BLADES) + 64);
    for turf in &turfs {
        let cell = hash2(turf.ix as u32, turf.iz as u32);
        let n = ((turf.count as f32) * scale).round() as u32;
        for b in 0..n {
            let seed = hash2(cell, b);
            let ox = (rand01(seed ^ 0x0000_0001) - 0.5) * TURF_SPACING_ELMOS;
            let oz = (rand01(seed ^ 0x0000_0002) - 0.5) * TURF_SPACING_ELMOS;
            let wx = (turf.cx + ox).clamp(0.0, world_max_x);
            let wz = (turf.cz + oz).clamp(0.0, world_max_z);
            let wy = sample_height(
                heightmap,
                wx / ELMOS_PER_TEXEL,
                wz / ELMOS_PER_TEXEL,
                min_h,
                max_h,
            );
            let orientation = rand01(seed ^ 0x0000_0003) * std::f32::consts::TAU;
            let height_scale = 0.8 + rand01(seed ^ 0x0000_0004) * 0.4;
            let color_jitter = [
                0.85 + rand01(seed ^ 0x0000_0005) * 0.30,
                0.85 + rand01(seed ^ 0x0000_0006) * 0.30,
                0.85 + rand01(seed ^ 0x0000_0007) * 0.30,
            ];
            out.push(GrassInstance {
                position: [wx, wy, wz],
                orientation,
                color_jitter,
                height_scale,
            });
        }
    }

    trace!(
        blades = out.len(),
        turfs = turfs.len(),
        budget_scale = scale,
        "grass instances generated"
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use barme_core::MapSize;
    use barme_core::bake_grass_density;

    fn flat_density() -> (GrassDensity, Heightmap) {
        let size = MapSize::square(4);
        let (w, h) = size.heightmap_dims();
        let hm = Heightmap::new(w, h, vec![20_000; (w as usize) * (h as usize)]).unwrap();
        let d = bake_grass_density(&hm, &GrassBlock::default(), 300.0);
        (d, hm)
    }

    #[test]
    fn deterministic_across_calls() {
        let (d, hm) = flat_density();
        let g = GrassBlock {
            max_straws_per_turf: Some(64),
            ..Default::default()
        };
        let cam = Vec3::new(500.0, 200.0, 500.0);
        let a = generate_grass_instances(&d, &hm, &g, (0.0, 300.0), cam, 200.0, 1.0);
        let b = generate_grass_instances(&d, &hm, &g, (0.0, 300.0), cam, 200.0, 1.0);
        assert_eq!(a, b, "generation must be deterministic");
        assert!(
            !a.is_empty(),
            "flat full-coverage field should spawn blades"
        );
    }

    #[test]
    fn blade_jitter_is_camera_invariant() {
        // A blade in a turf visible from two camera positions must be
        // byte-identical (pitfall #4 — no shimmer on camera move).
        let (d, hm) = flat_density();
        let g = GrassBlock {
            max_straws_per_turf: Some(64),
            ..Default::default()
        };
        let cam_a = Vec3::new(500.0, 200.0, 500.0);
        let cam_b = Vec3::new(508.0, 200.0, 504.0); // small shift
        let a = generate_grass_instances(&d, &hm, &g, (0.0, 300.0), cam_a, 200.0, 1.0);
        let b = generate_grass_instances(&d, &hm, &g, (0.0, 300.0), cam_b, 200.0, 1.0);
        // Both should contain the exact same blade for a turf well
        // inside both radii — match on position, compare full instance.
        let probe = a
            .iter()
            .find(|i| {
                let dx = i.position[0] - 504.0;
                let dz = i.position[2] - 502.0;
                dx * dx + dz * dz < 100.0
            })
            .copied()
            .expect("expected a blade near the shared region");
        assert!(
            b.contains(&probe),
            "the same blade must appear identically from both cameras"
        );
    }

    #[test]
    fn respects_blade_budget() {
        let (d, hm) = flat_density();
        let g = GrassBlock {
            max_straws_per_turf: Some(150),
            ..Default::default()
        };
        let cam = Vec3::new(1000.0, 300.0, 1000.0);
        // A huge radius would blow the budget without the global scale.
        let v = generate_grass_instances(&d, &hm, &g, (0.0, 300.0), cam, 5000.0, 1.0);
        assert!(
            v.len() <= GRASS_MAX_BLADES,
            "blade count {} exceeds budget {}",
            v.len(),
            GRASS_MAX_BLADES
        );
    }

    #[test]
    fn empty_when_disabled() {
        let (d, hm) = flat_density();
        let g = GrassBlock {
            max_straws_per_turf: Some(0),
            ..Default::default()
        };
        let cam = Vec3::new(500.0, 200.0, 500.0);
        assert!(
            generate_grass_instances(&d, &hm, &g, (0.0, 300.0), cam, 200.0, 1.0).is_empty(),
            "zero straws → no blades"
        );
        // density_scale 0 also yields nothing.
        let g2 = GrassBlock {
            max_straws_per_turf: Some(64),
            ..Default::default()
        };
        assert!(
            generate_grass_instances(&d, &hm, &g2, (0.0, 300.0), cam, 200.0, 0.0).is_empty(),
            "zero density scale → no blades"
        );
    }

    #[test]
    fn blades_sit_on_terrain() {
        // On a flat map at raw 20000 with band [0,300], every blade's Y
        // equals the sampled surface height.
        let (d, hm) = flat_density();
        let g = GrassBlock {
            max_straws_per_turf: Some(32),
            ..Default::default()
        };
        let cam = Vec3::new(500.0, 200.0, 500.0);
        let v = generate_grass_instances(&d, &hm, &g, (0.0, 300.0), cam, 150.0, 1.0);
        let expected = (20_000.0 / 65535.0) * 300.0;
        for inst in &v {
            assert!(
                (inst.position[1] - expected).abs() < 0.01,
                "blade Y {} should match flat surface {}",
                inst.position[1],
                expected
            );
        }
    }
}
