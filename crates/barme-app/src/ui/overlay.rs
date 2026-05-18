//! Canvas overlays painted on top of the wgpu terrain pass. ADR-031.
//!
//! These functions take an `egui::Painter` plus plain data (camera,
//! symmetry, rect, world cursor) and own no state. They are called from
//! `App::central` after the terrain `Callback::new_paint_callback`,
//! before the start-position marker overlay, so markers stay readable
//! on top of symmetry axes.
//!
//! World→screen projection reuses [`crate::render::world_to_screen`]
//! (ADR-023) — pitfall §B2.5 forbids adding a second projection path.

use barme_core::SymmetryAxis;
use eframe::egui;

use crate::render::{self, OrbitCamera};

/// Dash on/off lengths in *screen pixels*. Pitfall §B2.1: world-unit
/// dashes shrink with zoom-out and alias to a solid line; screen-pixel
/// spacing is invariant under camera distance.
const DASH_ON_PX: f32 = 8.0;
const DASH_OFF_PX: f32 = 4.0;

/// Below this projected length the dash pattern doesn't read as dashed
/// (each on-segment is shorter than ~3 px). Fall back to a thin solid
/// line so very-edge-on projections stay visible.
const MIN_DASHED_LENGTH_PX: f32 = 32.0;

const AXIS_STROKE_WIDTH: f32 = 1.5;
/// Premultiplied off-white (alpha ≈ 0.7) so the axis reads against both
/// bright and dark terrain without dominating.
const AXIS_COLOUR: egui::Color32 = egui::Color32::from_rgba_premultiplied(180, 180, 180, 180);

/// Pure helper: compute the list of world-space axis segments to paint
/// for `symmetry` on a map of `extents`. Mirror modes return 1–2 lines
/// crossing the map through centre; `Rotational` returns N spokes
/// clipped to the map rect. `None` and `Rotational { fold < 2 }`
/// return an empty vec.
///
/// Extracted from `paint_symmetry_overlay` so the geometry is unit-
/// testable without a painter or wgpu context.
pub fn axis_segments_for(
    symmetry: SymmetryAxis,
    extents: (f32, f32),
) -> Vec<((f32, f32), (f32, f32))> {
    let (ex, ez) = extents;
    let (mx, mz) = (ex * 0.5, ez * 0.5);
    match symmetry {
        SymmetryAxis::None => Vec::new(),
        SymmetryAxis::Horizontal => vec![((0.0, mz), (ex, mz))],
        SymmetryAxis::Vertical => vec![((mx, 0.0), (mx, ez))],
        SymmetryAxis::Quad => vec![((0.0, mz), (ex, mz)), ((mx, 0.0), (mx, ez))],
        SymmetryAxis::DiagonalMain => vec![((0.0, 0.0), (ex, ez))],
        SymmetryAxis::DiagonalAnti => vec![((0.0, ez), (ex, 0.0))],
        SymmetryAxis::Rotational { fold } => {
            if fold < 2 {
                Vec::new()
            } else {
                rotational_spoke_segments(mx, mz, ex, ez, fold)
            }
        }
    }
}

/// Paint the persistent symmetry overlay (dashed mirror axes for
/// mirror modes, N spokes for `Rotational`) into `rect`. No-op when
/// `symmetry == SymmetryAxis::None`.
///
/// `extents` is the map's world-space size in elmos
/// (`((dims.x - 1) * 8, (dims.z - 1) * 8)`); axes anchor at
/// `(extent / 2)` because the engine assumes geometric-centre
/// symmetry (ADR-019).
pub fn paint_symmetry_overlay(
    painter: &egui::Painter,
    rect: egui::Rect,
    camera: &OrbitCamera,
    symmetry: SymmetryAxis,
    extents: (f32, f32),
) {
    let segments = axis_segments_for(symmetry, extents);
    if segments.is_empty() {
        return;
    }
    let rect_size = glam::Vec2::new(rect.width(), rect.height());
    for ((ax, az), (bx, bz)) in segments {
        let a_world = glam::Vec3::new(ax, 0.0, az);
        let b_world = glam::Vec3::new(bx, 0.0, bz);
        let (Some(a_screen), Some(b_screen)) = (
            render::world_to_screen(a_world, rect_size, camera),
            render::world_to_screen(b_world, rect_size, camera),
        ) else {
            // One endpoint clipped off-screen — skip rather than guess
            // a clip plane. Pitfall §B2.3 — marginal at sane distances.
            continue;
        };
        let a = egui::Pos2::new(rect.min.x + a_screen.x, rect.min.y + a_screen.y);
        let b = egui::Pos2::new(rect.min.x + b_screen.x, rect.min.y + b_screen.y);
        paint_dashed_segment(painter, a, b);
    }
}

/// N spokes from `(mx, mz)` outwards, each clipped to the map rect
/// `[0, ex] × [0, ez]` so they stop at the boundary. High-fold values
/// (10, 12) crowd the centre; per ADR-031 we accept the crowding rather
/// than introduce an inner-circle fall-back this round.
fn rotational_spoke_segments(
    mx: f32,
    mz: f32,
    ex: f32,
    ez: f32,
    fold: u8,
) -> Vec<((f32, f32), (f32, f32))> {
    let n = fold as u32;
    // Longest possible spoke (worst-case corner distance from centre).
    let half_diag = (mx.max(ex - mx).hypot(mz.max(ez - mz))).max(1.0);
    let mut segs = Vec::with_capacity(n as usize);
    for k in 0..n {
        let theta = (k as f32) * std::f32::consts::TAU / (n as f32);
        let (s, c) = theta.sin_cos();
        let bx = mx + c * half_diag;
        let bz = mz + s * half_diag;
        let (cx, cz) = clip_ray_to_rect((mx, mz), (bx, bz), (0.0, 0.0), (ex, ez));
        segs.push(((mx, mz), (cx, cz)));
    }
    segs
}

/// Clip the ray starting at `a` in the direction `b - a` to the
/// axis-aligned rect `[min, max]`. Returns the point where the ray
/// exits the rect, or `b` itself if it doesn't (i.e. `b` is inside).
/// Used to stop rotational spokes at the map boundary.
fn clip_ray_to_rect(a: (f32, f32), b: (f32, f32), min: (f32, f32), max: (f32, f32)) -> (f32, f32) {
    let dx = b.0 - a.0;
    let dz = b.1 - a.1;
    let mut t_exit = 1.0_f32;
    if dx.abs() > 1e-6 {
        let t_a = (min.0 - a.0) / dx;
        let t_b = (max.0 - a.0) / dx;
        let t = t_a.max(t_b);
        if t > 0.0 {
            t_exit = t_exit.min(t);
        }
    }
    if dz.abs() > 1e-6 {
        let t_a = (min.1 - a.1) / dz;
        let t_b = (max.1 - a.1) / dz;
        let t = t_a.max(t_b);
        if t > 0.0 {
            t_exit = t_exit.min(t);
        }
    }
    (a.0 + dx * t_exit, a.1 + dz * t_exit)
}

/// Pure helper: compute the list of on-segments for a dashed line
/// between two screen points. Each entry is a `(start, end)` pair the
/// painter draws solid. Short lines (< `MIN_DASHED_LENGTH_PX`) return
/// a single solid segment `[(a, b)]`; very short lines (< 1e-3 px)
/// return empty so we don't emit zero-length geometry.
///
/// Extracted from `paint_dashed_segment` so the dash cadence math is
/// unit-testable.
pub fn dash_subsegments(a: egui::Pos2, b: egui::Pos2) -> Vec<(egui::Pos2, egui::Pos2)> {
    let dir = b - a;
    let len = dir.length();
    if len < 1e-3 {
        return Vec::new();
    }
    if len < MIN_DASHED_LENGTH_PX {
        return vec![(a, b)];
    }
    let unit = dir / len;
    let mut out = Vec::new();
    let mut t = 0.0_f32;
    while t < len {
        let on_end = (t + DASH_ON_PX).min(len);
        out.push((a + unit * t, a + unit * on_end));
        t = on_end + DASH_OFF_PX;
    }
    out
}

/// Render a single dashed-or-solid segment between two screen points.
/// Dash cadence is in screen pixels (pitfall §B2.1). Below
/// `MIN_DASHED_LENGTH_PX` the on-segments are too short to read as
/// dashes — render solid.
fn paint_dashed_segment(painter: &egui::Painter, a: egui::Pos2, b: egui::Pos2) {
    let stroke = egui::Stroke::new(AXIS_STROKE_WIDTH, AXIS_COLOUR);
    for (p0, p1) in dash_subsegments(a, b) {
        painter.line_segment([p0, p1], stroke);
    }
}

/// Colour of the brush ring keyed by the brush's stable id. Raise =
/// green, Lower = red, Smooth = blue per the UX research digest
/// (`docs/research/ui/claude-research-findings.md` §5). Unknown ids
/// fall back to light grey rather than panicking, so a future brush
/// id can ship before its colour mapping does.
pub fn brush_ring_color(brush_id: &str) -> egui::Color32 {
    match brush_id {
        "raise" => egui::Color32::from_rgb(80, 200, 100),
        "lower" => egui::Color32::from_rgb(220, 90, 90),
        "smooth" => egui::Color32::from_rgb(100, 160, 230),
        _ => egui::Color32::LIGHT_GRAY,
    }
}

/// Brush-cursor state passed into [`paint_brush_ghosts`]. Bundles the
/// three values that describe "what brush is at what spot" so the
/// function signature stays under the `too_many_arguments` lint cap.
pub struct BrushCursor<'a> {
    pub world: glam::Vec3,
    pub radius_world: f32,
    pub brush_id: Option<&'a str>,
}

/// Pure helper: return the symmetry-derived centres that B2 should
/// paint ghost rings at — i.e. every `replicate` entry except the
/// primary. Empty when `symmetry == None` or `replicate` returned just
/// one centre (degenerate case — stamp at map centre under rotational).
///
/// Extracted from `paint_brush_ghosts` so the "skip primary" contract
/// is unit-testable.
pub fn ghost_centres(
    symmetry: SymmetryAxis,
    world: glam::Vec3,
    extents: (f32, f32),
) -> Vec<(f32, f32)> {
    if matches!(symmetry, SymmetryAxis::None) {
        return Vec::new();
    }
    let mut centres = symmetry.replicate((world.x, world.z), extents);
    if centres.len() <= 1 {
        return Vec::new();
    }
    centres.remove(0);
    centres
}

/// Paint faint ghost rings at every symmetry-derived centre of
/// `cursor.world` when `symmetry != None`. Skips the primary centre —
/// B3 owns the primary brush ring; B2 only adds the mirrors so the
/// two land in independent commits.
///
/// `cursor.radius_world` is the brush radius in elmos; we project a
/// tangent world point `(cx + radius, 0, cz)` per ghost to compute the
/// screen-space ring radius (pitfall §B2.4).
pub fn paint_brush_ghosts(
    painter: &egui::Painter,
    rect: egui::Rect,
    camera: &OrbitCamera,
    symmetry: SymmetryAxis,
    cursor: BrushCursor<'_>,
    extents: (f32, f32),
) {
    let centres = ghost_centres(symmetry, cursor.world, extents);
    if centres.is_empty() {
        return;
    }
    let rect_size = glam::Vec2::new(rect.width(), rect.height());
    let base = cursor
        .brush_id
        .map(brush_ring_color)
        .unwrap_or(egui::Color32::LIGHT_GRAY);
    // Ghosts at ~50 % brightness. Premultiplied alpha so blending
    // against either bright or dark terrain produces the same hue.
    let ghost =
        egui::Color32::from_rgba_premultiplied(base.r() / 2, base.g() / 2, base.b() / 2, 128);

    for (cx, cz) in centres {
        let centre_world = glam::Vec3::new(cx, 0.0, cz);
        let Some(centre_screen) = render::world_to_screen(centre_world, rect_size, camera) else {
            continue;
        };
        let tangent_world = glam::Vec3::new(cx + cursor.radius_world, 0.0, cz);
        let radius_px = match render::world_to_screen(tangent_world, rect_size, camera) {
            Some(t) => (t - centre_screen).length().max(2.0),
            None => 8.0,
        };
        let p = egui::Pos2::new(rect.min.x + centre_screen.x, rect.min.y + centre_screen.y);
        painter.circle_stroke(p, radius_px, egui::Stroke::new(1.5, ghost));
        // Inner falloff ring at radius × 0.5 — matches B3's primary
        // visual so when both ship the rings look like a family.
        painter.circle_stroke(p, radius_px * 0.5, egui::Stroke::new(1.0, ghost));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXT: (f32, f32) = (1024.0, 1024.0); // 2 SMU square test extents
    const RECT: (f32, f32, f32, f32) = (0.0, 0.0, 1024.0, 1024.0);

    // ---------------------- clip_ray_to_rect ----------------------

    #[test]
    fn clip_ray_to_rect_stops_at_right_edge() {
        let p = clip_ray_to_rect(
            (512.0, 512.0),
            (2000.0, 512.0),
            (RECT.0, RECT.1),
            (RECT.2, RECT.3),
        );
        assert!((p.0 - 1024.0).abs() < 1e-3);
        assert!((p.1 - 512.0).abs() < 1e-3);
    }

    #[test]
    fn clip_ray_to_rect_stops_at_top_edge() {
        let p = clip_ray_to_rect(
            (512.0, 512.0),
            (512.0, -200.0),
            (RECT.0, RECT.1),
            (RECT.2, RECT.3),
        );
        assert!((p.0 - 512.0).abs() < 1e-3);
        assert!((p.1 - 0.0).abs() < 1e-3);
    }

    #[test]
    fn clip_ray_to_rect_stops_at_left_edge() {
        let p = clip_ray_to_rect(
            (512.0, 512.0),
            (-500.0, 512.0),
            (RECT.0, RECT.1),
            (RECT.2, RECT.3),
        );
        assert!((p.0 - 0.0).abs() < 1e-3);
        assert!((p.1 - 512.0).abs() < 1e-3);
    }

    #[test]
    fn clip_ray_to_rect_stops_at_bottom_edge() {
        let p = clip_ray_to_rect(
            (512.0, 512.0),
            (512.0, 2000.0),
            (RECT.0, RECT.1),
            (RECT.2, RECT.3),
        );
        assert!((p.0 - 512.0).abs() < 1e-3);
        assert!((p.1 - 1024.0).abs() < 1e-3);
    }

    #[test]
    fn clip_ray_to_rect_diagonal_exits_at_corner() {
        // Ray from centre toward equal +x/+z hits the (1024, 1024) corner.
        let p = clip_ray_to_rect(
            (512.0, 512.0),
            (1536.0, 1536.0),
            (RECT.0, RECT.1),
            (RECT.2, RECT.3),
        );
        assert!((p.0 - 1024.0).abs() < 1e-3);
        assert!((p.1 - 1024.0).abs() < 1e-3);
    }

    #[test]
    fn clip_ray_to_rect_endpoint_inside_returns_endpoint() {
        // b is inside the rect — no clipping; t_exit stays at 1.0.
        let p = clip_ray_to_rect(
            (100.0, 100.0),
            (500.0, 500.0),
            (RECT.0, RECT.1),
            (RECT.2, RECT.3),
        );
        assert!((p.0 - 500.0).abs() < 1e-3);
        assert!((p.1 - 500.0).abs() < 1e-3);
    }

    // ---------------------- rotational_spoke_segments ----------------------

    #[test]
    fn rotational_spoke_count_matches_fold() {
        for &fold in &[2u8, 3, 4, 6, 8, 12] {
            let segs = rotational_spoke_segments(512.0, 512.0, 1024.0, 1024.0, fold);
            assert_eq!(segs.len(), fold as usize, "fold={fold}");
        }
    }

    #[test]
    fn rotational_spokes_all_originate_at_centre() {
        let segs = rotational_spoke_segments(512.0, 512.0, 1024.0, 1024.0, 6);
        for ((ax, az), _) in segs {
            assert!((ax - 512.0).abs() < 1e-3);
            assert!((az - 512.0).abs() < 1e-3);
        }
    }

    #[test]
    fn rotational_spoke_endpoints_inside_rect() {
        for &fold in &[2u8, 3, 4, 6, 8, 12] {
            let segs = rotational_spoke_segments(512.0, 512.0, 1024.0, 1024.0, fold);
            for ((ax, az), (bx, bz)) in segs {
                assert!((0.0..=1024.0).contains(&ax));
                assert!((0.0..=1024.0).contains(&az));
                assert!((0.0..=1024.0).contains(&bx));
                assert!((0.0..=1024.0).contains(&bz));
            }
        }
    }

    #[test]
    fn rotational_fold_4_endpoints_land_on_map_edge() {
        // Fold=4 spokes point along ±x/±z; each endpoint must touch
        // a map edge at the axis midpoint.
        let segs = rotational_spoke_segments(512.0, 512.0, 1024.0, 1024.0, 4);
        for (_, (bx, bz)) in segs {
            let on_edge = (bx - 0.0).abs() < 1e-3
                || (bx - 1024.0).abs() < 1e-3
                || (bz - 0.0).abs() < 1e-3
                || (bz - 1024.0).abs() < 1e-3;
            assert!(on_edge, "fold=4 spoke endpoint ({bx}, {bz}) not on edge");
        }
    }

    #[test]
    fn rotational_spokes_on_rectangular_map_clip_to_rect_bounds() {
        // 2:1 rect map. Endpoints stay inside [0, ex] × [0, ez].
        let segs = rotational_spoke_segments(512.0, 1024.0, 1024.0, 2048.0, 8);
        for (_, (bx, bz)) in segs {
            assert!((0.0..=1024.0).contains(&bx), "bx={bx} out of [0,1024]");
            assert!((0.0..=2048.0).contains(&bz), "bz={bz} out of [0,2048]");
        }
    }

    // ---------------------- axis_segments_for ----------------------

    #[test]
    fn axis_segments_for_none_returns_empty() {
        assert!(axis_segments_for(SymmetryAxis::None, EXT).is_empty());
    }

    #[test]
    fn axis_segments_for_horizontal_one_segment_through_mid_z() {
        let segs = axis_segments_for(SymmetryAxis::Horizontal, EXT);
        assert_eq!(segs.len(), 1);
        let ((ax, az), (bx, bz)) = segs[0];
        assert!((ax - 0.0).abs() < 1e-3);
        assert!((bx - 1024.0).abs() < 1e-3);
        assert!((az - 512.0).abs() < 1e-3);
        assert!((bz - 512.0).abs() < 1e-3);
    }

    #[test]
    fn axis_segments_for_vertical_one_segment_through_mid_x() {
        let segs = axis_segments_for(SymmetryAxis::Vertical, EXT);
        assert_eq!(segs.len(), 1);
        let ((ax, _), (bx, _)) = segs[0];
        assert!((ax - 512.0).abs() < 1e-3);
        assert!((bx - 512.0).abs() < 1e-3);
    }

    #[test]
    fn axis_segments_for_quad_returns_both_mirror_axes() {
        assert_eq!(axis_segments_for(SymmetryAxis::Quad, EXT).len(), 2);
    }

    #[test]
    fn axis_segments_for_diagonal_main_runs_corner_to_corner() {
        let segs = axis_segments_for(SymmetryAxis::DiagonalMain, EXT);
        assert_eq!(segs.len(), 1);
        let ((ax, az), (bx, bz)) = segs[0];
        assert!((ax - 0.0).abs() < 1e-3 && (az - 0.0).abs() < 1e-3);
        assert!((bx - 1024.0).abs() < 1e-3 && (bz - 1024.0).abs() < 1e-3);
    }

    #[test]
    fn axis_segments_for_diagonal_anti_runs_opposite_corners() {
        let segs = axis_segments_for(SymmetryAxis::DiagonalAnti, EXT);
        assert_eq!(segs.len(), 1);
        let ((ax, az), (bx, bz)) = segs[0];
        assert!((ax - 0.0).abs() < 1e-3 && (az - 1024.0).abs() < 1e-3);
        assert!((bx - 1024.0).abs() < 1e-3 && (bz - 0.0).abs() < 1e-3);
    }

    #[test]
    fn axis_segments_for_rotational_fold_lt_2_returns_empty() {
        // ADR-019: fold == 1 is identity. Overlay should paint nothing.
        for fold in [0u8, 1] {
            let segs = axis_segments_for(SymmetryAxis::Rotational { fold }, EXT);
            assert!(segs.is_empty(), "fold={fold} should be empty");
        }
    }

    #[test]
    fn axis_segments_for_rotational_fold_n_returns_n_spokes() {
        for &fold in &[2u8, 3, 4, 6, 8, 12] {
            let segs = axis_segments_for(SymmetryAxis::Rotational { fold }, EXT);
            assert_eq!(segs.len(), fold as usize, "fold={fold}");
        }
    }

    // ---------------------- dash_subsegments ----------------------

    #[test]
    fn dash_subsegments_zero_length_returns_empty() {
        let p = egui::Pos2::new(100.0, 100.0);
        assert!(dash_subsegments(p, p).is_empty());
    }

    #[test]
    fn dash_subsegments_short_line_returns_single_solid() {
        // 20 px line — below MIN_DASHED_LENGTH_PX (32). Single solid.
        let a = egui::Pos2::new(0.0, 0.0);
        let b = egui::Pos2::new(20.0, 0.0);
        let out = dash_subsegments(a, b);
        assert_eq!(out.len(), 1);
        let (p0, p1) = out[0];
        assert_eq!(p0, a);
        assert_eq!(p1, b);
    }

    #[test]
    fn dash_subsegments_long_line_yields_multiple_on_segments() {
        // 100 px line. Pattern cycle is DASH_ON_PX + DASH_OFF_PX = 12.
        let a = egui::Pos2::new(0.0, 0.0);
        let b = egui::Pos2::new(100.0, 0.0);
        let out = dash_subsegments(a, b);
        assert!(out.len() >= 8, "expected ≥8 dashes, got {}", out.len());
        // Every dash on-segment must be ≤ DASH_ON_PX long and stay
        // inside [a, b].
        for (p0, p1) in &out {
            let len = (*p1 - *p0).length();
            assert!(
                len <= DASH_ON_PX + 1e-3,
                "dash on-segment length {} > DASH_ON_PX {}",
                len,
                DASH_ON_PX
            );
            assert!(p1.x <= b.x + 1e-3, "dash endpoint past b");
        }
    }

    #[test]
    fn dash_subsegments_cadence_at_threshold_boundary() {
        // Just past MIN_DASHED_LENGTH_PX (32 px): expect the first
        // on-segment exactly DASH_ON_PX long, second starting
        // DASH_OFF_PX later.
        let a = egui::Pos2::new(0.0, 0.0);
        let b = egui::Pos2::new(40.0, 0.0);
        let out = dash_subsegments(a, b);
        assert!(
            out.len() >= 2,
            "expected dashed (≥2 segments), got {}",
            out.len()
        );
        let (p0, p1) = out[0];
        assert!((p0 - a).length() < 1e-3);
        assert!(((p1.x - p0.x) - DASH_ON_PX).abs() < 1e-3);
        let (q0, _) = out[1];
        assert!((q0.x - (p1.x + DASH_OFF_PX)).abs() < 1e-3);
    }

    #[test]
    fn dash_subsegments_diagonal_line_aligns_with_direction() {
        // Diagonal 50 px line. On-segments should follow the unit
        // direction so their endpoints stay on the original line.
        let a = egui::Pos2::new(0.0, 0.0);
        let b = egui::Pos2::new(35.355, 35.355); // length 50 px
        let out = dash_subsegments(a, b);
        assert!(out.len() >= 4);
        for (p0, p1) in out {
            // Both endpoints lie on the y=x line (within float epsilon).
            assert!((p0.x - p0.y).abs() < 1e-2);
            assert!((p1.x - p1.y).abs() < 1e-2);
        }
    }

    // ---------------------- ghost_centres ----------------------

    #[test]
    fn ghost_centres_for_none_returns_empty() {
        let centres = ghost_centres(SymmetryAxis::None, glam::Vec3::new(200.0, 0.0, 300.0), EXT);
        assert!(centres.is_empty());
    }

    #[test]
    fn ghost_centres_skips_primary_for_horizontal_mirror() {
        // H mirror around centre x=512: stamp at x=100 has one mirror
        // at x=924. ghost_centres returns only the mirror (skips primary).
        let centres = ghost_centres(
            SymmetryAxis::Horizontal,
            glam::Vec3::new(100.0, 0.0, 256.0),
            EXT,
        );
        assert_eq!(centres.len(), 1);
        let (gx, gz) = centres[0];
        assert!((gx - 924.0).abs() < 1e-3);
        assert!((gz - 256.0).abs() < 1e-3);
    }

    #[test]
    fn ghost_centres_for_quad_returns_three_mirrors() {
        assert_eq!(
            ghost_centres(SymmetryAxis::Quad, glam::Vec3::new(100.0, 0.0, 200.0), EXT).len(),
            3
        );
    }

    #[test]
    fn ghost_centres_for_rotational_returns_fold_minus_one() {
        for &fold in &[2u8, 3, 4, 6, 8] {
            let centres = ghost_centres(
                SymmetryAxis::Rotational { fold },
                glam::Vec3::new(200.0, 0.0, 300.0),
                EXT,
            );
            assert_eq!(centres.len(), (fold - 1) as usize, "fold={fold}");
        }
    }

    #[test]
    fn ghost_centres_at_map_centre_under_rotational_returns_empty() {
        // Stamp at exact centre under rotational collapses (every
        // rotation maps back to itself within EPS).
        let centres = ghost_centres(
            SymmetryAxis::Rotational { fold: 4 },
            glam::Vec3::new(512.0, 0.0, 512.0),
            EXT,
        );
        assert!(centres.is_empty(), "expected empty, got {centres:?}");
    }

    #[test]
    fn ghost_centres_offmap_originating_stamp_keeps_inrange_mirrors() {
        // replicate filters off-map points. If the primary is off-map
        // and mirrors land in-map, ghost_centres should return them.
        let centres = ghost_centres(SymmetryAxis::Quad, glam::Vec3::new(-50.0, 0.0, 200.0), EXT);
        for (gx, gz) in &centres {
            assert!((0.0..=1024.0).contains(gx), "gx={gx} out of bounds");
            assert!((0.0..=1024.0).contains(gz), "gz={gz} out of bounds");
        }
    }

    #[test]
    fn ghost_centres_for_vertical_mirror() {
        let centres = ghost_centres(
            SymmetryAxis::Vertical,
            glam::Vec3::new(256.0, 0.0, 100.0),
            EXT,
        );
        assert_eq!(centres.len(), 1);
        let (gx, gz) = centres[0];
        assert!((gx - 256.0).abs() < 1e-3);
        assert!((gz - 924.0).abs() < 1e-3);
    }

    #[test]
    fn ghost_centres_for_diagonal_main_returns_reflection() {
        let centres = ghost_centres(
            SymmetryAxis::DiagonalMain,
            glam::Vec3::new(100.0, 0.0, 300.0),
            EXT,
        );
        assert_eq!(centres.len(), 1);
        // Reflection of (100, 300) across `(x - 512) = (z - 512)`:
        //   x' = 512 + (300 - 512) = 300
        //   z' = 512 + (100 - 512) = 100
        let (gx, gz) = centres[0];
        assert!((gx - 300.0).abs() < 1e-3);
        assert!((gz - 100.0).abs() < 1e-3);
    }

    // ---------------------- brush_ring_color ----------------------

    #[test]
    fn brush_ring_color_known_ids_match_research_digest() {
        // Pin Raise=green / Lower=red / Smooth=blue dominance per the
        // UX research digest §5. Future colour-balance edits stay green-
        // / red- / blue-dominant; a regression flips one of these.
        let r = brush_ring_color("raise");
        let l = brush_ring_color("lower");
        let s = brush_ring_color("smooth");
        assert!(
            r.g() > r.r() && r.g() > r.b(),
            "raise should be green-dominant"
        );
        assert!(
            l.r() > l.g() && l.r() > l.b(),
            "lower should be red-dominant"
        );
        assert!(
            s.b() > s.r() && s.b() > s.g(),
            "smooth should be blue-dominant"
        );
    }

    #[test]
    fn brush_ring_color_distinct_per_known_id() {
        let r = brush_ring_color("raise");
        let l = brush_ring_color("lower");
        let s = brush_ring_color("smooth");
        assert_ne!(r, l);
        assert_ne!(r, s);
        assert_ne!(l, s);
    }

    #[test]
    fn brush_ring_color_unknown_id_returns_neutral_fallback() {
        let unknown = brush_ring_color("nonsense_brush_id");
        assert_eq!(unknown, egui::Color32::LIGHT_GRAY);
    }

    // ------------------- BrushCursor sanity -------------------

    #[test]
    fn brush_cursor_struct_round_trips_fields() {
        let world = glam::Vec3::new(123.0, 0.0, 456.0);
        let cursor = BrushCursor {
            world,
            radius_world: 99.0,
            brush_id: Some("raise"),
        };
        assert_eq!(cursor.world, world);
        assert!((cursor.radius_world - 99.0).abs() < 1e-3);
        assert_eq!(cursor.brush_id, Some("raise"));
    }
}
