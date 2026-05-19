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
use crate::ui::markers::{Marker, MarkerBatch, MarkerShape};

/// Dash on/off lengths in *screen pixels*. Pitfall §B2.1: world-unit
/// dashes shrink with zoom-out and alias to a solid line; screen-pixel
/// spacing is invariant under camera distance.
const DASH_ON_PX: f32 = 8.0;
const DASH_OFF_PX: f32 = 4.0;

/// Below this projected length the dash pattern doesn't read as dashed
/// (each on-segment is shorter than ~3 px). Fall back to a thin solid
/// line so very-edge-on projections stay visible.
const MIN_DASHED_LENGTH_PX: f32 = 32.0;

/// Hard ceiling on the number of dashes emitted per symmetry axis
/// (Sprint 13 hotfix). At extreme zoom-in the projected axis length
/// can span hundreds of thousands of screen pixels, producing 10 000+
/// dashes — well past the line pipeline's pre-allocated vertex buffer.
/// When the predicted dash count crosses this cap we fall back to a
/// single solid segment over the on-screen portion of the axis;
/// individual dashes wouldn't read at that zoom level anyway.
const MAX_DASHES_PER_AXIS: usize = 256;

/// Margin (in screen pixels) added when clipping symmetry-axis
/// endpoints to the viewport rect, so a dash that straddles the edge
/// isn't half-missing. Generously larger than `DASH_ON_PX + DASH_OFF_PX`
/// so even a long edge-of-rect dash survives the clip intact.
const AXIS_CLIP_MARGIN_PX: f32 = 64.0;

/// Premultiplied off-white (alpha ≈ 0.7) so the axis reads against both
/// bright and dark terrain without dominating. The Sprint-13 line
/// pipeline draws at the GPU's native 1-px line width — the previous
/// 1.5-px stroke is gone (most platforms ignore line widths anyway).
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

/// Collect symmetry axes as dashed world-space `LineVertex` pairs for
/// the Sprint-13 line pipeline. Each pair forms one short dashed
/// sub-segment; LineList topology in the pipeline draws them as
/// individual segments. Lifted to [`crate::ui::markers::MARKER_Y_LIFT_ELMOS`]
/// so the axes don't z-fight ground terrain at h=0.
///
/// Dash cadence matches the legacy screen-space pattern (8 px on /
/// 4 px off) by computing the dash boundaries in screen space and
/// then `lerp`-ing world positions back per boundary — preserves the
/// "dashes don't shrink with zoom-out" behaviour from the old
/// `paint_symmetry_overlay` (pitfall §B2.1).
///
/// Sprint 13 / Phase 5 (ADR-037). Replaces `paint_symmetry_overlay`.
pub fn collect_symmetry_segments(
    out: &mut Vec<crate::render::LineVertex>,
    rect_size: glam::Vec2,
    camera: &OrbitCamera,
    symmetry: SymmetryAxis,
    extents: (f32, f32),
) {
    let segments = axis_segments_for(symmetry, extents);
    if segments.is_empty() {
        return;
    }
    let lift = crate::ui::markers::MARKER_Y_LIFT_ELMOS;
    for ((ax, az), (bx, bz)) in segments {
        let a_world = glam::Vec3::new(ax, lift, az);
        let b_world = glam::Vec3::new(bx, lift, bz);
        // Project both endpoints to screen for the screen-space dash
        // cadence. If either is behind the camera (`world_to_screen`
        // returns `None`), skip the segment — cleanly clipping a line
        // that crosses the camera plane is out of scope for Sprint 13.
        let (Some(a_s), Some(b_s)) = (
            render::world_to_screen(a_world, rect_size, camera),
            render::world_to_screen(b_world, rect_size, camera),
        ) else {
            continue;
        };
        let a_pt = egui::Pos2::new(a_s.x, a_s.y);
        let b_pt = egui::Pos2::new(b_s.x, b_s.y);
        let total = (b_pt - a_pt).length();
        if total < 1e-3 {
            continue;
        }
        // Sprint 13 hotfix: clip the projected axis to the visible
        // rect (plus a small margin) before dashing. At extreme
        // zoom-in the off-screen tails dwarf the on-screen middle —
        // dashing those is wasted vertex-buffer pressure.
        let Some((clip_a, clip_b)) =
            clip_segment_to_rect(a_pt, b_pt, rect_size, AXIS_CLIP_MARGIN_PX)
        else {
            continue;
        };
        let clip_len = (clip_b - clip_a).length();
        // One dash "cycle" = on + off pixels; predict the count and
        // fall back to a single solid segment if it exceeds the cap.
        let predicted_dashes = ((clip_len / (DASH_ON_PX + DASH_OFF_PX)) as usize).max(1);
        if predicted_dashes > MAX_DASHES_PER_AXIS {
            // Solid fallback — one segment over the clipped portion.
            let t_start = ((clip_a - a_pt).length() / total).clamp(0.0, 1.0);
            let t_end = ((clip_b - a_pt).length() / total).clamp(0.0, 1.0);
            let w_start = a_world.lerp(b_world, t_start);
            let w_end = a_world.lerp(b_world, t_end);
            out.push(crate::render::LineVertex::new(w_start, AXIS_COLOUR));
            out.push(crate::render::LineVertex::new(w_end, AXIS_COLOUR));
            continue;
        }
        for (s_start, s_end) in dash_subsegments(clip_a, clip_b) {
            // `t` is measured along the ORIGINAL `(a_pt, b_pt)` so the
            // dashes stay locked to the true axis even when only the
            // clipped middle is visible — pan the camera and the
            // pattern moves with the world, not with the rect.
            let t_start = ((s_start - a_pt).length() / total).clamp(0.0, 1.0);
            let t_end = ((s_end - a_pt).length() / total).clamp(0.0, 1.0);
            let w_start = a_world.lerp(b_world, t_start);
            let w_end = a_world.lerp(b_world, t_end);
            out.push(crate::render::LineVertex::new(w_start, AXIS_COLOUR));
            out.push(crate::render::LineVertex::new(w_end, AXIS_COLOUR));
        }
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

/// Liang–Barsky clip of segment `(a, b)` to the axis-aligned screen
/// rect `[0, rect_size.x] × [0, rect_size.y]` expanded by `margin_px`
/// on each side. Returns `Some((a', b'))` with both endpoints inside
/// the expanded rect, or `None` if the entire segment lies outside.
///
/// Sprint 13 hotfix: bounds the dash count in
/// [`collect_symmetry_segments`] when the camera zooms in far enough
/// that the projected axis spans hundreds of thousands of pixels.
/// Off-screen dashes wouldn't render anyway (the GPU rasterizer
/// clips them) — clipping CPU-side just stops us from feeding the
/// line vertex buffer a wave of garbage.
fn clip_segment_to_rect(
    a: egui::Pos2,
    b: egui::Pos2,
    rect_size: glam::Vec2,
    margin_px: f32,
) -> Option<(egui::Pos2, egui::Pos2)> {
    let x_min = -margin_px;
    let y_min = -margin_px;
    let x_max = rect_size.x + margin_px;
    let y_max = rect_size.y + margin_px;
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let mut t_enter = 0.0_f32;
    let mut t_exit = 1.0_f32;
    // `p` = direction sign, `q` = signed distance to the boundary.
    // `t = q / p` is the parameter where the segment crosses the
    // boundary; we tighten `[t_enter, t_exit]` until we have the
    // sub-segment inside the rect, or reject when the interval
    // collapses.
    let clip = |p: f32, q: f32, t_enter: &mut f32, t_exit: &mut f32| -> bool {
        if p.abs() < 1e-6 {
            // Segment parallel to this boundary; accept iff it's
            // inside the half-plane.
            return q >= 0.0;
        }
        let t = q / p;
        if p < 0.0 {
            // Entering through this boundary.
            if t > *t_exit {
                return false;
            }
            if t > *t_enter {
                *t_enter = t;
            }
        } else {
            // Exiting through this boundary.
            if t < *t_enter {
                return false;
            }
            if t < *t_exit {
                *t_exit = t;
            }
        }
        true
    };
    if !clip(-dx, a.x - x_min, &mut t_enter, &mut t_exit) {
        return None;
    }
    if !clip(dx, x_max - a.x, &mut t_enter, &mut t_exit) {
        return None;
    }
    if !clip(-dy, a.y - y_min, &mut t_enter, &mut t_exit) {
        return None;
    }
    if !clip(dy, y_max - a.y, &mut t_enter, &mut t_exit) {
        return None;
    }
    Some((
        egui::Pos2::new(a.x + dx * t_enter, a.y + dy * t_enter),
        egui::Pos2::new(a.x + dx * t_exit, a.y + dy * t_exit),
    ))
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

/// Push faint ghost rings into `batch` at every symmetry-derived
/// centre of `cursor.world` when `symmetry != None`. Skips the primary
/// centre — `collect_primary_brush_ring` owns that — so call both per
/// frame to render the full set.
///
/// Sprint 13 / ADR-037 — replaces the old `paint_brush_ghosts`. The
/// marker pipeline depth-tests against terrain so ghost rings hide
/// behind hills naturally.
pub fn collect_brush_ghosts(
    batch: &mut MarkerBatch,
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
    // Ghosts at ~50 % brightness. egui::Color32 is internally premul.
    let ghost =
        egui::Color32::from_rgba_premultiplied(base.r() / 2, base.g() / 2, base.b() / 2, 128);

    for (cx, cz) in centres {
        let centre_world = glam::Vec3::new(cx, 0.0, cz);
        let radius_px = brush_screen_radius(camera, rect_size, centre_world, cursor.radius_world)
            .unwrap_or(8.0);
        batch.push(Marker {
            world_pos: centre_world,
            radius_px,
            color: ghost,
            shape: MarkerShape::OutlineRing,
        });
        batch.push(Marker {
            world_pos: centre_world,
            radius_px: radius_px * 0.5,
            color: ghost,
            shape: MarkerShape::OutlineRing,
        });
    }
}

/// Pure helper: compute the projected screen-pixel radius of a brush
/// ring at world-space `centre` with `radius_world` elmos. Projects
/// the centre and `centre + (radius_world, 0, 0)` and takes the screen
/// distance between them. Returns `None` if either point projects
/// off-screen (the caller substitutes a fallback fixed radius).
///
/// Used by both [`paint_brush_ghosts`] and [`paint_primary_brush_ring`].
pub fn brush_screen_radius(
    camera: &OrbitCamera,
    rect_size: glam::Vec2,
    centre: glam::Vec3,
    radius_world: f32,
) -> Option<f32> {
    let centre_screen = render::world_to_screen(centre, rect_size, camera)?;
    let tangent = centre + glam::Vec3::new(radius_world, 0.0, 0.0);
    let tangent_screen = render::world_to_screen(tangent, rect_size, camera)?;
    Some((tangent_screen - centre_screen).length().max(2.0))
}

/// Push the primary brush ring (outer + falloff + centre dot) into
/// `batch`. Mirrors are emitted by [`collect_brush_ghosts`].
///
/// Sprint 13 / ADR-037 — replaces the old `paint_primary_brush_ring`.
/// The two centre-dot markers (black halo + coloured fill) rely on
/// `MarkerBatch::sort_back_to_front` being stable so the coloured fill
/// always lands on top of the halo at identical view-Z.
pub fn collect_primary_brush_ring(
    batch: &mut MarkerBatch,
    rect: egui::Rect,
    camera: &OrbitCamera,
    cursor: BrushCursor<'_>,
) {
    let rect_size = glam::Vec2::new(rect.width(), rect.height());
    let radius_px =
        brush_screen_radius(camera, rect_size, cursor.world, cursor.radius_world).unwrap_or(12.0);
    let base = cursor
        .brush_id
        .map(brush_ring_color)
        .unwrap_or(egui::Color32::LIGHT_GRAY);

    batch.push(Marker {
        world_pos: cursor.world,
        radius_px,
        color: base,
        shape: MarkerShape::OutlineRing,
    });
    batch.push(Marker {
        world_pos: cursor.world,
        radius_px: radius_px * 0.5,
        color: base,
        shape: MarkerShape::OutlineRing,
    });
    // Centre dot — black halo first (pushed first → lower in stable
    // sort at identical view-Z → drawn first → behind), then the
    // coloured fill on top.
    batch.push(Marker {
        world_pos: cursor.world,
        radius_px: 3.0,
        color: egui::Color32::BLACK,
        shape: MarkerShape::FilledCircle,
    });
    batch.push(Marker {
        world_pos: cursor.world,
        radius_px: 2.0,
        color: base,
        shape: MarkerShape::FilledCircle,
    });
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

    // ------------------- brush_screen_radius (B3) -------------------

    fn default_camera() -> OrbitCamera {
        OrbitCamera::framing(8192.0, 8192.0)
    }

    #[test]
    fn brush_screen_radius_returns_some_for_centre_world_at_origin() {
        let cam = default_camera();
        let rect_size = glam::Vec2::new(1024.0, 768.0);
        let centre = cam.target;
        let r = brush_screen_radius(&cam, rect_size, centre, 100.0);
        assert!(r.is_some());
        // Must be at least the 2 px floor.
        assert!(r.unwrap() >= 2.0);
    }

    #[test]
    fn brush_screen_radius_floor_is_two_pixels() {
        let cam = default_camera();
        let rect_size = glam::Vec2::new(1024.0, 768.0);
        // Tiny world radius — projected screen distance would be sub-pixel,
        // but the floor clamps to 2 px so the ring is visible.
        let r = brush_screen_radius(&cam, rect_size, cam.target, 0.001);
        assert!(r.unwrap() >= 2.0);
    }

    #[test]
    fn brush_screen_radius_grows_with_world_radius() {
        let cam = default_camera();
        let rect_size = glam::Vec2::new(1024.0, 768.0);
        let r_small = brush_screen_radius(&cam, rect_size, cam.target, 50.0).unwrap();
        let r_large = brush_screen_radius(&cam, rect_size, cam.target, 500.0).unwrap();
        assert!(
            r_large > r_small,
            "expected r_large ({r_large}) > r_small ({r_small})"
        );
    }

    #[test]
    fn brush_screen_radius_shrinks_with_camera_distance() {
        let mut cam = default_camera();
        let rect_size = glam::Vec2::new(1024.0, 768.0);
        let r_near = brush_screen_radius(&cam, rect_size, cam.target, 100.0).unwrap();
        cam.distance *= 4.0;
        let r_far = brush_screen_radius(&cam, rect_size, cam.target, 100.0).unwrap();
        assert!(
            r_far < r_near,
            "expected perspective: r_far ({r_far}) < r_near ({r_near})"
        );
    }

    #[test]
    fn brush_screen_radius_returns_none_when_centre_offscreen() {
        // Push the centre wildly off-screen via a huge world coord —
        // world_to_screen returns None and brush_screen_radius follows.
        let cam = default_camera();
        let rect_size = glam::Vec2::new(1024.0, 768.0);
        let centre = glam::Vec3::new(1.0e10, 0.0, 1.0e10);
        let r = brush_screen_radius(&cam, rect_size, centre, 100.0);
        assert!(r.is_none());
    }

    // ---------------------- collect_symmetry_segments (Phase 5) -------

    #[test]
    fn collect_symmetry_segments_none_emits_zero_vertices() {
        let cam = default_camera();
        let mut out = Vec::new();
        collect_symmetry_segments(
            &mut out,
            glam::Vec2::new(1024.0, 768.0),
            &cam,
            SymmetryAxis::None,
            EXT,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn collect_symmetry_segments_horizontal_emits_paired_vertices() {
        // Horizontal mirror = one dashed axis. We expect a non-empty
        // even-length output (LineList = pairs of vertices) when the
        // axis is on-screen at default camera framing.
        let cam = OrbitCamera::framing(EXT.0, EXT.1);
        let mut out = Vec::new();
        collect_symmetry_segments(
            &mut out,
            glam::Vec2::new(1024.0, 768.0),
            &cam,
            SymmetryAxis::Horizontal,
            EXT,
        );
        assert!(!out.is_empty(), "expected dashed sub-segments");
        assert_eq!(
            out.len() % 2,
            0,
            "LineList topology requires paired vertices, got {}",
            out.len()
        );
    }

    #[test]
    fn collect_symmetry_segments_lifts_vertices_to_marker_y() {
        // Every emitted vertex should sit at MARKER_Y_LIFT_ELMOS so
        // axes don't z-fight ground at h=0.
        let cam = OrbitCamera::framing(EXT.0, EXT.1);
        let mut out = Vec::new();
        collect_symmetry_segments(
            &mut out,
            glam::Vec2::new(1024.0, 768.0),
            &cam,
            SymmetryAxis::Quad,
            EXT,
        );
        assert!(!out.is_empty());
        let lift = crate::ui::markers::MARKER_Y_LIFT_ELMOS;
        for v in &out {
            assert!(
                (v.pos[1] - lift).abs() < 1e-3,
                "vertex Y = {} should be {} (MARKER_Y_LIFT_ELMOS)",
                v.pos[1],
                lift,
            );
        }
    }

    #[test]
    fn collect_symmetry_segments_quad_emits_more_than_horizontal() {
        // Quad (2 axes) should yield more vertices than Horizontal
        // (1 axis) when both axes are on-screen.
        let cam = OrbitCamera::framing(EXT.0, EXT.1);
        let mut quad_out = Vec::new();
        collect_symmetry_segments(
            &mut quad_out,
            glam::Vec2::new(1024.0, 768.0),
            &cam,
            SymmetryAxis::Quad,
            EXT,
        );
        let mut horiz_out = Vec::new();
        collect_symmetry_segments(
            &mut horiz_out,
            glam::Vec2::new(1024.0, 768.0),
            &cam,
            SymmetryAxis::Horizontal,
            EXT,
        );
        assert!(
            quad_out.len() > horiz_out.len(),
            "quad ({}) should yield more vertices than horizontal ({})",
            quad_out.len(),
            horiz_out.len(),
        );
    }

    // ------------------- clip_segment_to_rect (hotfix) -------------------
    //
    // Bug 2: zoomed-in symmetry axes blew past the 5_000-vertex line
    // buffer because the projected axis length spanned hundreds of
    // thousands of pixels. The Liang–Barsky clip caps the dash count
    // by the on-screen portion of the axis.

    const ZERO_MARGIN: f32 = 0.0;
    const RECT_SIZE: glam::Vec2 = glam::Vec2::new(800.0, 600.0);

    #[test]
    fn clip_segment_to_rect_passes_fully_inside() {
        let a = egui::Pos2::new(100.0, 100.0);
        let b = egui::Pos2::new(700.0, 500.0);
        let (ca, cb) = clip_segment_to_rect(a, b, RECT_SIZE, ZERO_MARGIN)
            .expect("segment fully inside should return Some");
        assert!((ca - a).length() < 1e-3, "a unchanged");
        assert!((cb - b).length() < 1e-3, "b unchanged");
    }

    #[test]
    fn clip_segment_to_rect_clips_one_endpoint_outside() {
        // a inside (400, 300); b outside the right edge (1200, 300).
        // Clipped endpoint should land on x = 800 at y = 300.
        let a = egui::Pos2::new(400.0, 300.0);
        let b = egui::Pos2::new(1200.0, 300.0);
        let (ca, cb) = clip_segment_to_rect(a, b, RECT_SIZE, ZERO_MARGIN).expect("partial overlap");
        assert!((ca - a).length() < 1e-3, "a stays inside");
        assert!((cb.x - 800.0).abs() < 1e-3, "b clipped to right edge");
        assert!((cb.y - 300.0).abs() < 1e-3, "y preserved on horizontal seg");
    }

    #[test]
    fn clip_segment_to_rect_rejects_fully_outside() {
        // Segment entirely to the right of the rect.
        let a = egui::Pos2::new(900.0, 300.0);
        let b = egui::Pos2::new(1500.0, 400.0);
        assert!(clip_segment_to_rect(a, b, RECT_SIZE, ZERO_MARGIN).is_none());
        // Entirely above the rect.
        let a2 = egui::Pos2::new(100.0, -200.0);
        let b2 = egui::Pos2::new(500.0, -50.0);
        assert!(clip_segment_to_rect(a2, b2, RECT_SIZE, ZERO_MARGIN).is_none());
    }

    #[test]
    fn clip_segment_to_rect_passes_segment_straddling_two_edges() {
        // Both endpoints outside the rect but the segment line crosses
        // through it (left edge → right edge). Both clipped endpoints
        // should land ON the rect edges.
        let a = egui::Pos2::new(-200.0, 300.0);
        let b = egui::Pos2::new(1000.0, 300.0);
        let (ca, cb) = clip_segment_to_rect(a, b, RECT_SIZE, ZERO_MARGIN)
            .expect("straddling segment should clip");
        assert!((ca.x - 0.0).abs() < 1e-3, "ca on left edge");
        assert!((cb.x - 800.0).abs() < 1e-3, "cb on right edge");
        // Vertical case — segment crosses bottom edge from top.
        let a2 = egui::Pos2::new(400.0, -100.0);
        let b2 = egui::Pos2::new(400.0, 900.0);
        let (ca2, cb2) =
            clip_segment_to_rect(a2, b2, RECT_SIZE, ZERO_MARGIN).expect("vertical clip");
        assert!((ca2.y - 0.0).abs() < 1e-3, "ca2 on top edge");
        assert!((cb2.y - 600.0).abs() < 1e-3, "cb2 on bottom edge");
    }

    #[test]
    fn clip_segment_to_rect_respects_margin() {
        // Endpoint just outside the rect (5 px past the right edge).
        // With margin = 0 it gets clipped; with margin = 10 it passes
        // through unchanged.
        let a = egui::Pos2::new(100.0, 300.0);
        let b = egui::Pos2::new(805.0, 300.0);
        let (_, cb_no_margin) =
            clip_segment_to_rect(a, b, RECT_SIZE, 0.0).expect("partial overlap with margin=0");
        assert!(
            (cb_no_margin.x - 800.0).abs() < 1e-3,
            "expected clip at 800.0 with margin=0, got {}",
            cb_no_margin.x,
        );
        let (_, cb_with_margin) = clip_segment_to_rect(a, b, RECT_SIZE, 10.0)
            .expect("segment inside expanded rect with margin=10");
        assert!(
            (cb_with_margin.x - 805.0).abs() < 1e-3,
            "expected b unchanged at 805.0 with margin=10, got {}",
            cb_with_margin.x,
        );
    }

    #[test]
    fn collect_symmetry_segments_caps_dashes_when_zoomed_in() {
        // Pull the camera in tight so the axis projects across the
        // full rect at a multi-thousand-pixel length. The cap (256
        // dashes per axis → 512 verts) plus the solid-fallback path
        // must keep the output bounded for a single Horizontal axis
        // (1 axis ⇒ at most 2 verts solid, or up to MAX_DASHES_PER_AXIS
        // × 2 = 512 dashed).
        let mut cam = OrbitCamera::framing(EXT.0, EXT.1);
        cam.distance = 50.0; // far inside the framing default
        let mut out = Vec::new();
        collect_symmetry_segments(
            &mut out,
            glam::Vec2::new(1024.0, 768.0),
            &cam,
            SymmetryAxis::Horizontal,
            EXT,
        );
        // One axis → bound is MAX_DASHES_PER_AXIS * 2 verts (when
        // dashed) or 2 verts (when solid-fallback triggers).
        assert!(
            out.len() <= MAX_DASHES_PER_AXIS * 2,
            "expected ≤{} verts, got {}",
            MAX_DASHES_PER_AXIS * 2,
            out.len(),
        );
        // And NOT bigger than the line vertex buffer cap (8 000) —
        // explicit guard against a future regression that re-introduces
        // the pre-hotfix unbounded behaviour.
        assert!(
            out.len() < crate::render::LINE_VERTEX_CAPACITY as usize,
            "exceeded LINE_VERTEX_CAPACITY"
        );
    }
}
