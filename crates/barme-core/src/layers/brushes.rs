//! Layer-mask brushes (D9 / Sprint 16, ADR-039).
//!
//! Mirrors the shape of [`crate::splat`] D3 brushes:
//! - [`MaskBrush`] trait, object-safe `Send + Sync + 'static`.
//! - One stamp per [`MaskStamp`]; a stroke is many stamps along a
//!   drag.
//! - Apply returns `Option<DirtyRect>`. The Sprint-16 paint viewport
//!   uses the rect to scope the GPU dirty-tile re-upload.
//! - [`MaskBrushRegistry::default_set`] ships four starter brushes:
//!   `mask-reveal`, `mask-hide`, `mask-smooth`, `mask-fill`. Brush
//!   ids match the kebab-case strings the Sprint-17 Layers panel
//!   dispatches on.
//!
//! Coord convention: stamps live in world space (elmos). The mask is
//! sized at `512 × SMU` per side, the elmo extent is also `512 ×
//! SMU` per side (see [`crate::MapSize::texture_dims`] +
//! [`crate::MapSize::elmo_extents`]), so 1 mask pixel = 1 elmo. The
//! brushes therefore use `world_x` directly as the pixel coordinate
//! (no per-pixel scaling like the heightmap or splat paths).

use crate::brushes::{DirtyRect, smoothstep};

use super::mask::{LayerMask, MaskStamp, flood_fill, mask_pixel_bbox};

/// Plugin surface for layer-mask brushes. The Sprint-17 Layers panel
/// looks up brushes by id at stamp time; the trait is object-safe so
/// a future wasm-plugin runtime could hand back `Box<dyn MaskBrush>`
/// from outside this crate.
pub trait MaskBrush: Send + Sync + 'static {
    /// Stable id used as the serialization key + UI dispatch. Lowercase
    /// kebab ascii. Sprint-16 ids: `mask-reveal`, `mask-hide`,
    /// `mask-smooth`, `mask-fill`.
    fn id(&self) -> &'static str;

    /// Display label for UI dropdowns / chip text.
    fn label(&self) -> &'static str;

    /// Apply one stamp. Returns the pixel bounding box that changed,
    /// or `None` if the stamp was off-map / zero-radius / zero-
    /// strength / a no-op.
    fn apply(&self, mask: &mut LayerMask, stamp: MaskStamp) -> Option<DirtyRect>;
}

/// Vector of `Box<dyn MaskBrush>` — same shape as
/// [`crate::splat::SplatBrushRegistry`]. Built once at App start; the
/// Sprint-16 active-layer strip + the Sprint-17 Layers panel iterate
/// to render brush-mode chips and look up by id at stamp time.
pub struct MaskBrushRegistry {
    brushes: Vec<Box<dyn MaskBrush>>,
}

impl MaskBrushRegistry {
    /// Ships with `mask-reveal` / `mask-hide` / `mask-smooth` /
    /// `mask-fill`. New mask brushes drop in here as a new struct +
    /// `impl MaskBrush` + one line.
    pub fn default_set() -> Self {
        Self {
            brushes: vec![
                Box::new(MaskReveal),
                Box::new(MaskHide),
                Box::new(MaskSmooth),
                Box::new(MaskFill),
            ],
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn MaskBrush> {
        self.brushes.iter().map(|b| b.as_ref())
    }

    pub fn get(&self, id: &str) -> Option<&dyn MaskBrush> {
        self.brushes
            .iter()
            .find(|b| b.id() == id)
            .map(|b| b.as_ref())
    }

    pub fn is_empty(&self) -> bool {
        self.brushes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.brushes.len()
    }
}

impl Default for MaskBrushRegistry {
    fn default() -> Self {
        Self::default_set()
    }
}

/// Smoothstep falloff in mask-pixel space. Mirrors
/// [`crate::splat::Falloff`] but specialised to the mask's 1-elmo-per-
/// pixel coordinate.
struct Falloff {
    cx: f32,
    cz: f32,
    r: f32,
    strength: f32,
}

impl Falloff {
    fn from(stamp: MaskStamp) -> Self {
        Self {
            cx: stamp.world_x,
            cz: stamp.world_z,
            r: stamp.radius.max(f32::EPSILON),
            strength: stamp.strength.clamp(0.0, 1.0),
        }
    }

    /// Returns the falloff-weighted strength at pixel `(x, y)`, or
    /// `None` if outside the circular kernel.
    fn weight_at(&self, x: u32, y: u32) -> Option<f32> {
        let dx = x as f32 - self.cx;
        let dz = y as f32 - self.cz;
        let d = (dx * dx + dz * dz).sqrt();
        if d > self.r {
            return None;
        }
        let w = self.strength * smoothstep(1.0 - d / self.r);
        (w > 0.0).then_some(w)
    }
}

// ─── Reveal / hide ──────────────────────────────────────────────────

/// Push the mask byte toward 255 (visible) with a smoothstep falloff.
/// `new = cur + (255 - cur) · weight`; `weight = strength ·
/// smoothstep(1 - d/r)`.
pub struct MaskReveal;

impl MaskBrush for MaskReveal {
    fn id(&self) -> &'static str {
        "mask-reveal"
    }
    fn label(&self) -> &'static str {
        "Reveal"
    }

    fn apply(&self, mask: &mut LayerMask, stamp: MaskStamp) -> Option<DirtyRect> {
        if stamp.strength <= 0.0 || stamp.radius <= 0.0 {
            return None;
        }
        let bbox = mask_pixel_bbox(mask, stamp)?;
        let falloff = Falloff::from(stamp);
        apply_falloff_pass(mask, bbox, |cur, x, y| {
            let Some(w) = falloff.weight_at(x, y) else {
                return cur;
            };
            let cur_f = cur as f32;
            let new = cur_f + (255.0 - cur_f) * w;
            new.round().clamp(0.0, 255.0) as u8
        })
    }
}

/// Push the mask byte toward 0 (transparent) with a smoothstep
/// falloff. `new = cur · (1 - weight)`.
pub struct MaskHide;

impl MaskBrush for MaskHide {
    fn id(&self) -> &'static str {
        "mask-hide"
    }
    fn label(&self) -> &'static str {
        "Hide"
    }

    fn apply(&self, mask: &mut LayerMask, stamp: MaskStamp) -> Option<DirtyRect> {
        if stamp.strength <= 0.0 || stamp.radius <= 0.0 {
            return None;
        }
        let bbox = mask_pixel_bbox(mask, stamp)?;
        let falloff = Falloff::from(stamp);
        apply_falloff_pass(mask, bbox, |cur, x, y| {
            let Some(w) = falloff.weight_at(x, y) else {
                return cur;
            };
            let cur_f = cur as f32;
            let new = cur_f * (1.0 - w);
            new.round().clamp(0.0, 255.0) as u8
        })
    }
}

// ─── Smooth — 3×3 mean blend ────────────────────────────────────────

/// 3×3 mean blend lerped toward by `strength × falloff`. Mirrors
/// [`crate::splat::Smooth`] but on grayscale rather than RGBA.
/// Reads from the mask DIRECTLY without a snapshot — for the typical
/// "fewer than 1024 pixels per stamp" case the propagation bias is
/// negligible and avoiding the snapshot keeps the brush O(bbox).
///
/// If a future regression shows bias building up across a long stroke,
/// switch to the snap-then-write pattern from `splat::Smooth`.
pub struct MaskSmooth;

impl MaskBrush for MaskSmooth {
    fn id(&self) -> &'static str {
        "mask-smooth"
    }
    fn label(&self) -> &'static str {
        "Smooth"
    }

    fn apply(&self, mask: &mut LayerMask, stamp: MaskStamp) -> Option<DirtyRect> {
        if stamp.strength <= 0.0 || stamp.radius <= 0.0 {
            return None;
        }
        let bbox = mask_pixel_bbox(mask, stamp)?;
        let falloff = Falloff::from(stamp);

        // Snapshot the source rect (+1-px margin) so propagation bias
        // doesn't accumulate within the pass. Matches `splat::Smooth`
        // exactly. Memory cost is bbox-sized regardless of mask size.
        let snap_x = bbox.x.saturating_sub(1);
        let snap_y = bbox.y.saturating_sub(1);
        let snap_r = (bbox.x + bbox.w + 1).min(mask.width);
        let snap_b = (bbox.y + bbox.h + 1).min(mask.height);
        let snap_w = (snap_r - snap_x) as usize;
        let snap_h = (snap_b - snap_y) as usize;
        let mut snap: Vec<u8> = Vec::with_capacity(snap_w * snap_h);
        for yy in snap_y..snap_b {
            for xx in snap_x..snap_r {
                snap.push(mask.sample(xx, yy));
            }
        }
        let sample_snap = |x: u32, y: u32| -> u8 {
            let lx = (x - snap_x) as usize;
            let ly = (y - snap_y) as usize;
            snap[ly * snap_w + lx]
        };

        apply_falloff_pass(mask, bbox, |cur, x, y| {
            let Some(w) = falloff.weight_at(x, y) else {
                return cur;
            };
            // 3×3 mean (clipped at edges) from the SNAPSHOT, not from
            // the mask — propagation bias avoided.
            let xlo = x.saturating_sub(1).max(snap_x);
            let xhi = (x + 1).min(snap_r - 1);
            let ylo = y.saturating_sub(1).max(snap_y);
            let yhi = (y + 1).min(snap_b - 1);
            let mut sum: u32 = 0;
            let mut count: u32 = 0;
            for ny in ylo..=yhi {
                for nx in xlo..=xhi {
                    sum += u32::from(sample_snap(nx, ny));
                    count += 1;
                }
            }
            if count == 0 {
                return cur;
            }
            let avg = sum as f32 / count as f32;
            let mix = (cur as f32) * (1.0 - w) + avg * w;
            mix.round().clamp(0.0, 255.0) as u8
        })
    }
}

// ─── Fill — 4-connected flood ───────────────────────────────────────

/// Bucket fill from the stamp's centre. Visits every 4-connected
/// neighbour whose pre-fill value lies in `seed_value ± 5`; sets each
/// visited pixel to either `255` (`target_visible = true`) or `0`.
///
/// Ignores `strength` and `radius`. The ±5 threshold mirrors
/// Photoshop's "Contiguous" magic-wand at tolerance 5.
pub struct MaskFill;

impl MaskBrush for MaskFill {
    fn id(&self) -> &'static str {
        "mask-fill"
    }
    fn label(&self) -> &'static str {
        "Fill"
    }

    fn apply(&self, mask: &mut LayerMask, stamp: MaskStamp) -> Option<DirtyRect> {
        let seed_x = stamp.world_x.round();
        let seed_y = stamp.world_z.round();
        if seed_x < 0.0 || seed_y < 0.0 {
            return None;
        }
        let sx = seed_x as u32;
        let sy = seed_y as u32;
        let target = if stamp.target_visible { 255u8 } else { 0u8 };
        flood_fill(mask, sx, sy, target, 5)
    }
}

// ─── Internal helpers ───────────────────────────────────────────────

/// Common `write_rect_with` invocation for falloff brushes. Reads the
/// current byte at each pixel via `mask.sample` (cheap on `Uniform`
/// tiles) and writes the per-pixel update returned by `f`.
fn apply_falloff_pass<F: FnMut(u8, u32, u32) -> u8>(
    mask: &mut LayerMask,
    bbox: DirtyRect,
    mut f: F,
) -> Option<DirtyRect> {
    // Pre-read the current bytes inside the bbox so `f` sees the
    // pre-stamp state at every pixel even after tile promotion.
    let rect_w = bbox.w;
    let rect_h = bbox.h;
    let mut current: Vec<u8> = Vec::with_capacity((rect_w as usize) * (rect_h as usize));
    for dy in 0..rect_h {
        for dx in 0..rect_w {
            current.push(mask.sample(bbox.x + dx, bbox.y + dy));
        }
    }
    let stride = rect_w as usize;
    mask.write_rect_with(bbox.x, bbox.y, rect_w, rect_h, |dx, dy| {
        let cur = current[(dy as usize) * stride + (dx as usize)];
        f(cur, bbox.x + dx, bbox.y + dy)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MapSize;

    fn two_smu() -> MapSize {
        MapSize::square(2)
    }

    fn stamp(world_x: f32, world_z: f32, radius: f32, strength: f32) -> MaskStamp {
        MaskStamp {
            world_x,
            world_z,
            radius,
            strength,
            target_visible: true,
        }
    }

    #[test]
    fn registry_ships_four_brushes_with_expected_ids() {
        let r = MaskBrushRegistry::default_set();
        assert_eq!(r.len(), 4);
        for id in ["mask-reveal", "mask-hide", "mask-smooth", "mask-fill"] {
            assert!(r.get(id).is_some(), "missing brush id `{id}`");
        }
        assert!(r.get("nonexistent").is_none());
        assert!(!r.is_empty());
    }

    #[test]
    fn registry_iter_returns_brushes_in_declared_order() {
        let r = MaskBrushRegistry::default_set();
        let ids: Vec<_> = r.iter().map(|b| b.id()).collect();
        assert_eq!(
            ids,
            vec!["mask-reveal", "mask-hide", "mask-smooth", "mask-fill"]
        );
    }

    // ─── reveal ──────────────────────────────────────────────────

    #[test]
    fn reveal_raises_center_pixel_toward_255() {
        let mut m = LayerMask::filled(two_smu(), 0);
        let bbox = MaskReveal
            .apply(&mut m, stamp(512.0, 512.0, 64.0, 1.0))
            .expect("touches pixels");
        assert!(bbox.w > 0 && bbox.h > 0);
        // Centre pixel should saturate at 255.
        assert_eq!(m.sample(512, 512), 255);
        // Edge of the kernel: smoothstep falloff → very small write,
        // probably 0 after rounding. Just check < 50.
        assert!(m.sample(512 + 60, 512) < 50);
    }

    #[test]
    fn reveal_returns_dirty_rect_clipped_to_mask() {
        let mut m = LayerMask::filled(two_smu(), 0);
        // Stamp near the corner — rect must clip to (0, 0, ...).
        let bbox = MaskReveal
            .apply(&mut m, stamp(0.0, 0.0, 100.0, 1.0))
            .expect("touches pixels");
        assert_eq!(bbox.x, 0);
        assert_eq!(bbox.y, 0);
        assert!(bbox.x + bbox.w <= m.width);
        assert!(bbox.y + bbox.h <= m.height);
        assert_eq!(m.sample(0, 0), 255);
    }

    // ─── hide ────────────────────────────────────────────────────

    #[test]
    fn hide_drops_center_pixel_toward_0() {
        let mut m = LayerMask::filled(two_smu(), 255);
        MaskHide
            .apply(&mut m, stamp(512.0, 512.0, 64.0, 1.0))
            .unwrap();
        assert_eq!(m.sample(512, 512), 0);
        assert!(m.sample(512 + 60, 512) > 200, "edge barely touched");
    }

    #[test]
    fn hide_then_reveal_round_trips_to_full_visibility_within_tolerance() {
        let mut m = LayerMask::filled(two_smu(), 255);
        for _ in 0..4 {
            MaskHide
                .apply(&mut m, stamp(512.0, 512.0, 64.0, 1.0))
                .unwrap();
        }
        // Mid-region should be near 0 now.
        assert!(m.sample(512, 512) < 10);
        for _ in 0..8 {
            MaskReveal
                .apply(&mut m, stamp(512.0, 512.0, 64.0, 1.0))
                .unwrap();
        }
        // And reveal pushes it back to ~255.
        assert!(m.sample(512, 512) > 250);
    }

    // ─── smooth ──────────────────────────────────────────────────

    #[test]
    fn smooth_reduces_local_variance_around_a_spike() {
        let mut m = LayerMask::filled(two_smu(), 0);
        // Spike: a single pixel at 255 surrounded by 0s.
        m.set_pixel(512, 512, 255);
        let before = local_variance(&m, 512, 512, 4);
        for _ in 0..6 {
            MaskSmooth
                .apply(&mut m, stamp(512.0, 512.0, 64.0, 1.0))
                .unwrap();
        }
        let after = local_variance(&m, 512, 512, 4);
        assert!(
            after < before,
            "smoothing reduces variance: before = {before:.1}, after = {after:.1}"
        );
    }

    fn local_variance(m: &LayerMask, cx: u32, cy: u32, r: u32) -> f64 {
        let mut sum = 0.0f64;
        let mut sq = 0.0f64;
        let mut n = 0.0f64;
        for y in cy.saturating_sub(r)..=(cy + r).min(m.height - 1) {
            for x in cx.saturating_sub(r)..=(cx + r).min(m.width - 1) {
                let v = f64::from(m.sample(x, y));
                sum += v;
                sq += v * v;
                n += 1.0;
            }
        }
        let mean = sum / n;
        sq / n - mean * mean
    }

    // ─── fill ────────────────────────────────────────────────────

    #[test]
    fn fill_flood_paints_connected_region() {
        let mut m = LayerMask::filled(two_smu(), 100);
        // Plant a 5×5 patch at value 200 around (500, 500).
        for y in 498..=502 {
            for x in 498..=502 {
                m.set_pixel(x, y, 200);
            }
        }
        let mut s = stamp(500.0, 500.0, 0.0, 0.0);
        s.target_visible = true;
        let bbox = MaskFill.apply(&mut m, s).expect("filled pixels");
        // Patch becomes 255 (target_visible = true).
        for y in 498..=502 {
            for x in 498..=502 {
                assert_eq!(m.sample(x, y), 255);
            }
        }
        // Surrounding 100s untouched.
        assert_eq!(m.sample(497, 500), 100);
        assert_eq!(m.sample(503, 500), 100);
        assert_eq!(bbox.w, 5);
        assert_eq!(bbox.h, 5);
    }

    #[test]
    fn fill_target_invisible_writes_zero() {
        let mut m = LayerMask::filled(two_smu(), 255);
        // Plant a patch with a slightly different value so the flood
        // bounds itself.
        for y in 498..=502 {
            for x in 498..=502 {
                m.set_pixel(x, y, 240);
            }
        }
        let mut s = stamp(500.0, 500.0, 0.0, 0.0);
        s.target_visible = false;
        MaskFill.apply(&mut m, s).unwrap();
        assert_eq!(m.sample(500, 500), 0);
        // Surrounding 255s untouched.
        assert_eq!(m.sample(497, 500), 255);
    }

    #[test]
    fn fill_off_map_returns_none() {
        let mut m = LayerMask::filled(two_smu(), 0);
        let mut s = stamp(2000.0, 2000.0, 0.0, 0.0);
        s.target_visible = true;
        assert!(MaskFill.apply(&mut m, s).is_none());
    }

    // ─── invariants every brush respects ─────────────────────────

    #[test]
    fn zero_strength_returns_none_for_falloff_brushes() {
        let mut m = LayerMask::filled(two_smu(), 128);
        for brush_id in ["mask-reveal", "mask-hide", "mask-smooth"] {
            let r = MaskBrushRegistry::default_set();
            let brush = r.get(brush_id).unwrap();
            let s = stamp(512.0, 512.0, 64.0, 0.0);
            assert!(
                brush.apply(&mut m, s).is_none(),
                "{brush_id} must no-op at strength 0"
            );
        }
    }

    #[test]
    fn zero_radius_returns_none_for_falloff_brushes() {
        let mut m = LayerMask::filled(two_smu(), 128);
        for brush_id in ["mask-reveal", "mask-hide", "mask-smooth"] {
            let r = MaskBrushRegistry::default_set();
            let brush = r.get(brush_id).unwrap();
            let s = stamp(512.0, 512.0, 0.0, 1.0);
            assert!(
                brush.apply(&mut m, s).is_none(),
                "{brush_id} must no-op at radius 0"
            );
        }
    }

    #[test]
    fn off_map_returns_none_for_falloff_brushes() {
        let mut m = LayerMask::filled(two_smu(), 0);
        for brush_id in ["mask-reveal", "mask-hide", "mask-smooth"] {
            let r = MaskBrushRegistry::default_set();
            let brush = r.get(brush_id).unwrap();
            // Centre 200 elmos off the map's right edge, radius 50
            // (still off-map). The mask is 1024 px wide; centre at
            // 1300 with radius 50 → rect would land at [1250, 1350)
            // which is entirely past width = 1024.
            let s = stamp(1300.0, 1300.0, 50.0, 1.0);
            assert!(
                brush.apply(&mut m, s).is_none(),
                "{brush_id} must no-op off-map"
            );
        }
    }

    /// Repeated reveal at strength 1.0 converges to all-255 in the
    /// kernel after enough stamps. The Sprint-17 paint UI repeats
    /// stamps along a drag — convergence is what makes a long
    /// brush hold "feel" like a flat fill.
    #[test]
    fn repeated_reveal_at_strength_one_converges_to_255() {
        let mut m = LayerMask::filled(two_smu(), 0);
        for _ in 0..16 {
            MaskReveal
                .apply(&mut m, stamp(512.0, 512.0, 64.0, 1.0))
                .unwrap();
        }
        // The peak of the falloff (centre pixel) gets the strongest
        // push; check that every pixel within r/2 of centre is at
        // 255 ± rounding.
        for y in 500..=524 {
            for x in 500..=524 {
                let v = m.sample(x, y);
                assert!(v >= 250, "({x}, {y}) = {v} below convergence threshold");
            }
        }
    }
}
