//! Brush registry and trait for heightmap sculpting (ADR-018).
//!
//! Three starter brushes ship in `raise`, `lower`, `smooth`. Adding a new
//! brush = one struct + one `Brush` impl + one line in `BrushRegistry::default_set`.
//! The trait is intentionally object-safe so a future wasm-plugin runtime
//! could hand back `Box<dyn Brush>` values from outside this crate.
//!
//! Coord convention: stamps live in world space (elmos, per ADR-008). Each
//! brush converts to heightmap pixel space internally via
//! `ELMOS_PER_HEIGHTMAP_PIXEL` (= 8).
//!
//! Kernel math adapted from Jandodev/bar-editor `src/lib/terrain-edit.ts`
//! (MIT-licensed). Cubic smoothstep falloff for raise/lower; 8-neighbour
//! mean blend for smooth.

use crate::Heightmap;

mod lower;
mod raise;
mod smooth;

pub use lower::Lower;
pub use raise::Raise;
pub use smooth::Smooth;

/// 8 elmos per heightmap pixel — `MapSize::ELMOS_PER_SMU / HEIGHTMAP_PER_SMU`.
pub const ELMOS_PER_HEIGHTMAP_PIXEL: f32 = 8.0;

/// Per-stamp parameters in world space. One of these per *stamp*; a stroke
/// is many stamps along a drag path.
#[derive(Debug, Clone, Copy)]
pub struct BrushStamp {
    /// Stamp center, world X (elmos). 0 = west edge of the map.
    pub world_x: f32,
    /// Stamp center, world Z (elmos). 0 = north edge of the map.
    pub world_z: f32,
    /// Brush radius (elmos).
    pub radius: f32,
    /// Strength 0..=1. Interpretation is brush-specific — see each kernel.
    pub strength: f32,
}

/// Inclusive bounding box of pixels touched by a brush apply. `w`/`h` are
/// pixel counts, not max indices. `Heightmap::sub_rect_slice` consumes this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirtyRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl DirtyRect {
    /// Union of two rects (smallest bounding box containing both). Used to
    /// fold symmetric strokes into a single upload.
    pub fn union(self, other: DirtyRect) -> DirtyRect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let r = (self.x + self.w).max(other.x + other.w);
        let b = (self.y + self.h).max(other.y + other.h);
        DirtyRect {
            x,
            y,
            w: r - x,
            h: b - y,
        }
    }
}

/// The plugin surface. `Brush` is object-safe (`dyn Brush`) and `Send + Sync`
/// so a future plugin runtime can register brushes from outside the crate.
pub trait Brush: Send + Sync + 'static {
    /// Stable id used as the serialization key + UI lookup. Lowercase ascii.
    fn id(&self) -> &'static str;

    /// Display label for UI dropdowns.
    fn label(&self) -> &'static str;

    /// Apply one stamp. Returns the pixel bounding box that changed, or
    /// `None` if the stamp was wholly outside the map / zero-radius / zero
    /// strength (caller can skip the texture upload).
    fn apply(&self, hm: &mut Heightmap, stamp: BrushStamp) -> Option<DirtyRect>;
}

/// Vector of `Box<dyn Brush>`. Built once at app start; iterated by UI to
/// populate the brush dropdown; looked up by id for stroke dispatch.
pub struct BrushRegistry {
    brushes: Vec<Box<dyn Brush>>,
}

impl BrushRegistry {
    /// Ships with raise / lower / smooth. Stage-1.5 brushes
    /// (flatten / erode / noise / terrace / ramp) drop in here.
    pub fn default_set() -> Self {
        Self {
            brushes: vec![Box::new(Raise), Box::new(Lower), Box::new(Smooth)],
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn Brush> {
        self.brushes.iter().map(|b| b.as_ref())
    }

    pub fn get(&self, id: &str) -> Option<&dyn Brush> {
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

impl Default for BrushRegistry {
    fn default() -> Self {
        Self::default_set()
    }
}

/// Convert a world-space stamp into a pixel bounding box clipped to the
/// heightmap. Returns `None` if the rect is empty (off-map or zero radius).
/// Kernel helpers in the per-brush modules call this; exposed here so a
/// new brush can re-use it.
pub(crate) fn pixel_bbox(hm: &Heightmap, stamp: BrushStamp) -> Option<DirtyRect> {
    let (w, h) = hm.dims();
    if w == 0 || h == 0 {
        return None;
    }
    let r_px = (stamp.radius / ELMOS_PER_HEIGHTMAP_PIXEL).max(0.0);
    if r_px <= 0.0 {
        return None;
    }
    let cx_px = stamp.world_x / ELMOS_PER_HEIGHTMAP_PIXEL;
    let cz_px = stamp.world_z / ELMOS_PER_HEIGHTMAP_PIXEL;
    let min_x = ((cx_px - r_px).floor()).max(0.0) as i64;
    let max_x = ((cx_px + r_px).ceil()).min((w - 1) as f32) as i64;
    let min_y = ((cz_px - r_px).floor()).max(0.0) as i64;
    let max_y = ((cz_px + r_px).ceil()).min((h - 1) as f32) as i64;
    if max_x < min_x || max_y < min_y {
        return None;
    }
    Some(DirtyRect {
        x: min_x as u32,
        y: min_y as u32,
        w: (max_x - min_x + 1) as u32,
        h: (max_y - min_y + 1) as u32,
    })
}

/// Cubic smoothstep `t² · (3 - 2t)`, clamped to [0,1].
#[inline]
pub(crate) fn smoothstep(t: f32) -> f32 {
    let x = t.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MapSize;

    #[test]
    fn default_registry_has_three_starter_brushes() {
        let r = BrushRegistry::default_set();
        assert_eq!(r.len(), 3);
        assert!(r.get("raise").is_some());
        assert!(r.get("lower").is_some());
        assert!(r.get("smooth").is_some());
        assert!(r.get("nonexistent").is_none());
    }

    #[test]
    fn pixel_bbox_clips_to_heightmap() {
        let hm = Heightmap::synth_ramp(MapSize::square(2)); // 129×129
        // Stamp centered at corner with radius covering ~2 px.
        let bbox = pixel_bbox(
            &hm,
            BrushStamp {
                world_x: 0.0,
                world_z: 0.0,
                radius: 16.0, // 2 hm pixels
                strength: 0.5,
            },
        )
        .unwrap();
        assert_eq!(bbox.x, 0);
        assert_eq!(bbox.y, 0);
        // 2 pixels each direction + 1 = 3-pixel-wide bbox starting at 0.
        assert!(bbox.w <= 4 && bbox.w >= 2);
    }

    #[test]
    fn pixel_bbox_off_map_returns_none() {
        let hm = Heightmap::synth_ramp(MapSize::square(2));
        let bbox = pixel_bbox(
            &hm,
            BrushStamp {
                world_x: -100.0,
                world_z: -100.0,
                radius: 8.0, // 1 px — too far to touch
                strength: 1.0,
            },
        );
        assert!(bbox.is_none());
    }

    #[test]
    fn dirty_rect_union_grows_correctly() {
        let a = DirtyRect {
            x: 10,
            y: 10,
            w: 5,
            h: 5,
        };
        let b = DirtyRect {
            x: 20,
            y: 20,
            w: 5,
            h: 5,
        };
        let u = a.union(b);
        assert_eq!(u.x, 10);
        assert_eq!(u.y, 10);
        assert_eq!(u.w, 15); // 25 - 10
        assert_eq!(u.h, 15);
    }
}
