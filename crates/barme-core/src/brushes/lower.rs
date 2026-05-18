//! Lower brush — same kernel as raise with sign flipped.

use super::raise::apply_radial_delta;
use super::{Brush, BrushStamp, DirtyRect, pixel_bbox};
use crate::Heightmap;

const STAMP_MAX_DELTA: f32 = 0.05 * u16::MAX as f32;

pub struct Lower;

impl Brush for Lower {
    fn id(&self) -> &'static str {
        "lower"
    }
    fn label(&self) -> &'static str {
        "Lower"
    }

    fn apply(&self, hm: &mut Heightmap, stamp: BrushStamp) -> Option<DirtyRect> {
        if stamp.strength <= 0.0 {
            return None;
        }
        let bbox = pixel_bbox(hm, stamp)?;
        apply_radial_delta(hm, stamp, bbox, -(stamp.strength * STAMP_MAX_DELTA));
        Some(bbox)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_strictly_decreases_sample_at_center() {
        let mut hm = Heightmap::new(129, 129, vec![u16::MAX; 129 * 129]).unwrap();
        let stamp = BrushStamp {
            world_x: 64.0 * 8.0,
            world_z: 64.0 * 8.0,
            radius: 16.0 * 8.0,
            strength: 1.0,
        };
        Lower.apply(&mut hm, stamp).unwrap();
        let center = hm.data()[64 * 129 + 64];
        assert!(center < u16::MAX, "center should fall: got {center}");
        assert_eq!(hm.data()[0], u16::MAX, "far corner untouched");
    }

    #[test]
    fn lower_floors_at_zero() {
        let mut hm = Heightmap::new(129, 129, vec![0u16; 129 * 129]).unwrap();
        let stamp = BrushStamp {
            world_x: 64.0 * 8.0,
            world_z: 64.0 * 8.0,
            radius: 32.0 * 8.0,
            strength: 1.0,
        };
        Lower.apply(&mut hm, stamp).unwrap();
        assert_eq!(
            hm.data()[64 * 129 + 64],
            0,
            "lowering past zero should clamp"
        );
    }
}
