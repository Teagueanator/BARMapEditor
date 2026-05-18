//! Symmetry enforcement (ADR-019). One brush stamp produces N mirrored /
//! rotated stamps via [`SymmetryAxis::replicate`]; the caller applies the
//! brush at each derived center and unions their `DirtyRect`s for a
//! single GPU upload.
//!
//! All operations work in world-space (elmos). The map occupies the
//! axis-aligned rect `[0, extent_x] × [0, extent_z]`. "Mid" is map center.

use serde::{Deserialize, Serialize};

/// Available symmetry modes for the sculpting UI.
///
/// Mirror modes treat the map's centerlines as the axes of symmetry.
/// `DiagonalMain` mirrors across the line `(x - midX) = (z - midZ)`;
/// `DiagonalAnti` across `(x - midX) = -(z - midZ)`. Diagonals only
/// produce sensible results on square maps — on rectangles the diagonal
/// is *defined* but the reflected point may land off-map (filtered out
/// by `replicate`).
///
/// `Rotational { fold: N }` rotates by `2π / N` around map center. BAR
/// maps overwhelmingly use 2-fold or 4-fold symmetry; 3/6/8 are
/// available for users who want them. `fold == 1` collapses to identity
/// (no replication beyond the original stamp).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SymmetryAxis {
    #[default]
    None,
    Horizontal,
    Vertical,
    Quad,
    DiagonalMain,
    DiagonalAnti,
    Rotational {
        fold: u8,
    },
}

impl SymmetryAxis {
    /// Stable string id for serialization / UI lookup.
    pub fn id(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
            Self::Quad => "quad",
            Self::DiagonalMain => "diagonal-main",
            Self::DiagonalAnti => "diagonal-anti",
            Self::Rotational { .. } => "rotational",
        }
    }

    /// UI label.
    pub fn label(&self) -> String {
        match self {
            Self::None => "None".into(),
            Self::Horizontal => "Mirror — horizontal".into(),
            Self::Vertical => "Mirror — vertical".into(),
            Self::Quad => "Quad (H + V)".into(),
            Self::DiagonalMain => "Diagonal \\".into(),
            Self::DiagonalAnti => "Diagonal /".into(),
            Self::Rotational { fold } => format!("Rotational ×{fold}"),
        }
    }

    /// Given a brush stamp's world-space center and the map's
    /// `(extent_x, extent_z)` in elmos, return the original center
    /// plus all symmetry-derived centers. Length always ≥ 1.
    /// Centers that round to the same pixel as the original (degenerate
    /// — e.g. stamp at map center under rotational) are deduplicated
    /// to one entry within an `EPS` tolerance.
    pub fn replicate(&self, center: (f32, f32), extents: (f32, f32)) -> Vec<(f32, f32)> {
        const EPS: f32 = 0.5; // < 1 elmo = sub-pixel; rounding-class duplicates
        let (cx, cz) = center;
        let (ex, ez) = extents;
        let (mx, mz) = (ex * 0.5, ez * 0.5);

        let mut out: Vec<(f32, f32)> = Vec::with_capacity(8);
        let push = |p: (f32, f32), buf: &mut Vec<(f32, f32)>| {
            // Filter off-map (post-mirror) and dedup.
            if p.0 < 0.0 || p.0 > ex || p.1 < 0.0 || p.1 > ez {
                return;
            }
            for existing in buf.iter() {
                if (existing.0 - p.0).abs() < EPS && (existing.1 - p.1).abs() < EPS {
                    return;
                }
            }
            buf.push(p);
        };
        push((cx, cz), &mut out);

        match *self {
            Self::None => {}
            Self::Horizontal => {
                push((2.0 * mx - cx, cz), &mut out);
            }
            Self::Vertical => {
                push((cx, 2.0 * mz - cz), &mut out);
            }
            Self::Quad => {
                push((2.0 * mx - cx, cz), &mut out);
                push((cx, 2.0 * mz - cz), &mut out);
                push((2.0 * mx - cx, 2.0 * mz - cz), &mut out);
            }
            Self::DiagonalMain => {
                // Reflect (cx, cz) across line (x-mx) = (z-mz):
                //   x' = mx + (cz - mz),  z' = mz + (cx - mx)
                push((mx + (cz - mz), mz + (cx - mx)), &mut out);
            }
            Self::DiagonalAnti => {
                // Reflect across (x-mx) = -(z-mz):
                //   x' = mx - (cz - mz),  z' = mz - (cx - mx)
                push((mx - (cz - mz), mz - (cx - mx)), &mut out);
            }
            Self::Rotational { fold } => {
                let n = fold.max(1) as f32;
                if fold > 1 {
                    let dx = cx - mx;
                    let dz = cz - mz;
                    for k in 1..fold as u32 {
                        let theta = (k as f32) * std::f32::consts::TAU / n;
                        let (s, c) = theta.sin_cos();
                        let rx = c * dx - s * dz;
                        let rz = s * dx + c * dz;
                        push((mx + rx, mz + rz), &mut out);
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXT: (f32, f32) = (1024.0, 1024.0); // 2 SMU square test map

    #[test]
    fn none_returns_only_original() {
        let pts = SymmetryAxis::None.replicate((100.0, 200.0), EXT);
        assert_eq!(pts.len(), 1);
        assert!((pts[0].0 - 100.0).abs() < 1e-3);
        assert!((pts[0].1 - 200.0).abs() < 1e-3);
    }

    #[test]
    fn horizontal_mirrors_x_around_mid() {
        let pts = SymmetryAxis::Horizontal.replicate((100.0, 256.0), EXT);
        assert_eq!(pts.len(), 2);
        assert!((pts[1].0 - 924.0).abs() < 1e-3);
        assert!((pts[1].1 - 256.0).abs() < 1e-3);
    }

    #[test]
    fn vertical_mirrors_z_around_mid() {
        let pts = SymmetryAxis::Vertical.replicate((256.0, 100.0), EXT);
        assert_eq!(pts.len(), 2);
        assert!((pts[1].0 - 256.0).abs() < 1e-3);
        assert!((pts[1].1 - 924.0).abs() < 1e-3);
    }

    #[test]
    fn quad_produces_four_points() {
        let pts = SymmetryAxis::Quad.replicate((100.0, 200.0), EXT);
        assert_eq!(pts.len(), 4);
    }

    #[test]
    fn diagonal_main_swaps_offsets() {
        let pts = SymmetryAxis::DiagonalMain.replicate((100.0, 700.0), EXT);
        assert_eq!(pts.len(), 2);
        // (100, 700) - (512, 512) = (-412, 188); swap → (188, -412)
        //   → mirror is (512 + 188, 512 + -412) = (700, 100).
        assert!((pts[1].0 - 700.0).abs() < 1e-3);
        assert!((pts[1].1 - 100.0).abs() < 1e-3);
    }

    #[test]
    fn rotational_fold4_produces_four_equispaced_points() {
        let pts = SymmetryAxis::Rotational { fold: 4 }.replicate((100.0, 512.0), EXT);
        assert_eq!(pts.len(), 4);
        // After 90° rotations around (512, 512), (100, 512) goes to
        // (512, 100), (924, 512), (512, 924). Order not guaranteed; check set.
        let expected = [
            (100.0, 512.0),
            (512.0, 100.0),
            (924.0, 512.0),
            (512.0, 924.0),
        ];
        for e in expected.iter() {
            assert!(
                pts.iter()
                    .any(|p| (p.0 - e.0).abs() < 1.0 && (p.1 - e.1).abs() < 1.0),
                "missing expected point {:?} in {:?}",
                e,
                pts
            );
        }
    }

    #[test]
    fn rotational_at_map_center_collapses_to_one_point() {
        let pts = SymmetryAxis::Rotational { fold: 4 }.replicate((512.0, 512.0), EXT);
        assert_eq!(pts.len(), 1, "all images collapse to center: {:?}", pts);
    }

    #[test]
    fn replicate_filters_offmap_centers() {
        // A stamp center near a corner, mirrored by a rotational that
        // would land outside the map, drops that point.
        let pts = SymmetryAxis::Rotational { fold: 4 }.replicate((0.0, 0.0), EXT);
        // From (0,0), 90° rot around (512,512): (0,1024), (1024,1024),
        // (1024,0). All on-map. So 4 points.
        assert_eq!(pts.len(), 4);
    }
}
