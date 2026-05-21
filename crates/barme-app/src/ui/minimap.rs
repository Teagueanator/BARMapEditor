//! Top-down mini-map widget (ADR-035). Replaces the XYZ nav gizmo
//! that lived at the top-right of the viewport pre-overhaul.
//!
//! The mini-map renders a heightfield thumbnail (biome-coloured) with
//! overlay layers for the symmetry guide line, allyteam start pins,
//! metal spots, and the current camera frustum. A tiny N-arrow
//! compass glyph in the top-left corner preserves the "which way is
//! north" cue that the retired XYZ gizmo provided.
//!
//! This module is *pure rendering* — no `App` state. Caller passes the
//! heightmap (or a downsampled version) plus pin/spot data; the
//! widget paints into a fixed rect at the top-right of the viewport.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui,
};

use barme_core::{AllyGroup, Heightmap, SplatDistribution, SymmetryAxis};

use crate::render::OrbitCamera;
use crate::ui::icons::{self, Icon};
use crate::ui::theme::Tokens;

/// Mini-map size in screen pixels. The viewport reserves a
/// (Self::PANEL_W × Self::PANEL_H) rect at its top-right corner.
pub struct Minimap;

impl Minimap {
    pub const BODY: f32 = 168.0;
    pub const PANEL_W: f32 = Self::BODY + 12.0;
    pub const PANEL_H: f32 = Self::BODY + 38.0;
}

/// Paint the mini-map at the top-right of `viewport_rect`. Returns
/// the rect the mini-map occupies (so the caller can hit-test pointer
/// events against it). Heightmap is sampled at most 48 × 48 cells; if
/// the source resolution is lower, the body shows a checker pattern.
///
/// `splat_distribution`, when supplied, is composited over the
/// heightfield thumbnail at 50 % opacity so the user can see at a
/// glance where they've painted (D5 / Sprint 9).
/// One placed feature, ready to paint as a minimap dot. The colour
/// is resolved by the caller via the feature catalog so this module
/// stays pure-rendering (no JSON/serde dependency).
pub struct MinimapFeature {
    pub x_elmo: f32,
    pub z_elmo: f32,
    pub color: Color32,
}

#[allow(clippy::too_many_arguments)] // pure-rendering helper, easier to read than a struct
pub fn paint_minimap(
    ui: &mut Ui,
    viewport_rect: Rect,
    heightmap: Option<&Heightmap>,
    splat_distribution: Option<&SplatDistribution>,
    ally_groups: &[AllyGroup],
    metal_spots: &[(f32, f32, f32)],
    geo_vents: &[(f32, f32)],
    features: &[MinimapFeature],
    extents: (f32, f32),
    camera: &OrbitCamera,
    symmetry: SymmetryAxis,
) -> Rect {
    let t = Tokens::DARK;
    let panel_rect = Rect::from_min_size(
        Pos2::new(
            viewport_rect.right() - Minimap::PANEL_W - 14.0,
            viewport_rect.top() + 14.0,
        ),
        egui::vec2(Minimap::PANEL_W, Minimap::PANEL_H),
    );
    let painter = ui.painter();
    painter.rect_filled(
        panel_rect,
        CornerRadius::same(6),
        Color32::from_rgba_premultiplied(20, 26, 34, 215),
    );
    painter.rect_stroke(
        panel_rect,
        CornerRadius::same(6),
        Stroke::new(1.0, t.border),
        StrokeKind::Middle,
    );

    // Header row: "Top down" label + expand glyph.
    let header_rect = Rect::from_min_size(
        Pos2::new(panel_rect.left() + 6.0, panel_rect.top() + 6.0),
        egui::vec2(Minimap::BODY, 16.0),
    );
    painter.text(
        Pos2::new(header_rect.left() + 16.0, header_rect.center().y),
        Align2::LEFT_CENTER,
        "TOP DOWN",
        FontId::proportional(10.0),
        t.muted,
    );
    let map_icon = Rect::from_min_size(
        Pos2::new(header_rect.left(), header_rect.top() + 2.0),
        egui::vec2(12.0, 12.0),
    );
    icons::paint_icon(painter, map_icon, Icon::Map, t.muted, 1.3);

    // Body rect.
    let body_rect = Rect::from_min_size(
        Pos2::new(panel_rect.left() + 6.0, panel_rect.top() + 24.0),
        egui::vec2(Minimap::BODY, Minimap::BODY),
    );
    let painter = ui.painter();
    painter.rect_filled(body_rect, CornerRadius::same(3), t.bg);

    // Heightfield thumbnail. Sampled coarsely for paint speed.
    if let Some(h) = heightmap {
        paint_heightfield(painter, body_rect, h);
    }

    // D5: splat distribution overlay (50 % opacity over the
    // heightfield) so the user can see at-a-glance where they've
    // painted. Same coarse 48-tap sampling as the heightfield. R/G/B
    // channels drive red/green/blue overlays; alpha is desaturated
    // (white) since "snow"-style A-channel maps want a brightness
    // bump rather than a hue.
    if let Some(d) = splat_distribution {
        paint_splat_overlay(painter, body_rect, d);
    }

    // Sprint 19 / U1 — symmetry guide. Reads the project's active
    // `SymmetryAxis` and draws the corresponding axes / spokes on
    // the minimap body.
    //
    // History: the original mockup-era implementation drew an
    // unconditional vertical bisector because the static mockup
    // shows one for every state. That was visually misleading once
    // the symmetry system grew Quad / Diagonal / Rotational modes —
    // the minimap promised vertical mirroring even when the actual
    // axis was diagonal. The new implementation derives the lines
    // from `Project.symmetry` and the centre point of the body rect.
    paint_minimap_symmetry(painter, body_rect, symmetry);

    // Metal spots (under pins).
    for (x, z, val) in metal_spots {
        let p = world_to_mini(body_rect, (*x, *z), extents);
        let hot = *val >= 1.5;
        let fill = if hot {
            Color32::from_rgb(0xF5, 0x9E, 0x0B)
        } else {
            Color32::from_rgb(0xA3, 0x73, 0x40)
        };
        painter.circle_filled(p, 3.5, fill);
        painter.circle_stroke(
            p,
            3.5,
            Stroke::new(1.0, Color32::from_rgba_premultiplied(0, 0, 0, 160)),
        );
    }

    // Sprint 19 — placed features (under start pins so team colours
    // win on overlap). Catalog-resolved colour by category.
    for f in features {
        let p = world_to_mini(body_rect, (f.x_elmo, f.z_elmo), extents);
        painter.circle_filled(p, 2.5, f.color);
        painter.circle_stroke(
            p,
            2.5,
            Stroke::new(1.0, Color32::from_rgba_premultiplied(0, 0, 0, 140)),
        );
    }

    // Sprint 19 — geo vents (orange triangle, matches the 3D marker
    // colour). Small upward-pointing equilateral so it reads at 3 px.
    let geo_color = Color32::from_rgb(0xF5, 0x9E, 0x0B);
    let geo_outline = Color32::from_rgba_premultiplied(0, 0, 0, 160);
    for (x, z) in geo_vents {
        let c = world_to_mini(body_rect, (*x, *z), extents);
        let pts = [
            Pos2::new(c.x, c.y - 3.5),
            Pos2::new(c.x - 3.0, c.y + 2.5),
            Pos2::new(c.x + 3.0, c.y + 2.5),
        ];
        painter.add(egui::Shape::convex_polygon(
            pts.to_vec(),
            geo_color,
            Stroke::new(1.0, geo_outline),
        ));
    }

    // Start pins.
    for g in ally_groups {
        let color = Color32::from_rgb(g.color[0], g.color[1], g.color[2]);
        for pos in &g.start_positions {
            let p = world_to_mini(body_rect, (pos.x_elmo as f32, pos.z_elmo as f32), extents);
            painter.circle_filled(p, 3.0, color);
            painter.circle_stroke(
                p,
                3.0,
                Stroke::new(1.0, Color32::from_rgba_premultiplied(0, 0, 0, 160)),
            );
        }
    }

    // Camera frustum trapezoid — derived from the camera's yaw, drawn
    // as a translucent accent-blue cone pointing along the look
    // direction.
    paint_frustum(painter, body_rect, camera);

    // N-arrow compass in the top-left corner.
    let compass_rect = Rect::from_min_size(
        Pos2::new(body_rect.left() + 4.0, body_rect.top() + 4.0),
        egui::vec2(14.0, 14.0),
    );
    painter.circle_filled(
        compass_rect.center(),
        7.0,
        Color32::from_rgba_premultiplied(8, 12, 18, 200),
    );
    painter.circle_stroke(compass_rect.center(), 7.0, Stroke::new(1.0, t.border));
    // North needle (red), south needle (grey).
    let c = compass_rect.center();
    painter.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(c.x, c.y - 5.0),
            Pos2::new(c.x + 1.7, c.y),
            Pos2::new(c.x, c.y - 1.5),
            Pos2::new(c.x - 1.7, c.y),
        ],
        Color32::from_rgb(0xEF, 0x44, 0x44),
        Stroke::NONE,
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(c.x, c.y + 5.0),
            Pos2::new(c.x - 1.7, c.y),
            Pos2::new(c.x, c.y + 1.5),
            Pos2::new(c.x + 1.7, c.y),
        ],
        Color32::from_rgb(0x9C, 0xA3, 0xAF),
        Stroke::NONE,
    ));

    // Footer: SMU + scale.
    let footer_y = body_rect.bottom() + 4.0;
    let (ex, _ez) = extents;
    let smu = (ex / 512.0).round() as i32; // 1 SMU = 512 elmos
    painter.text(
        Pos2::new(panel_rect.left() + 8.0, footer_y),
        Align2::LEFT_TOP,
        format!("{smu} × {smu} SMU"),
        FontId::monospace(10.0),
        t.muted,
    );
    let scale = (Minimap::BODY / (ex.max(1.0))) * 64.0; // pixels per 64 elmos
    let _ = scale; // future use; suppressed clippy in stub
    painter.text(
        Pos2::new(panel_rect.right() - 8.0, footer_y),
        Align2::RIGHT_TOP,
        format!("1 : {}", (ex / Minimap::BODY).round() as i32),
        FontId::monospace(10.0),
        t.muted,
    );

    // Interaction hook: click on the body fires a "centre camera here"
    // request via a synthetic Sense::click(). Hit-test only; the
    // App-side handler reads from the response. We don't expose the
    // response from this function in this phase — TODO(Phase-9):
    // wire camera recenter.
    let _ = ui.interact(body_rect, ui.id().with("minimap_body"), Sense::click());

    panel_rect
}

/// Sprint 19 / U1 — return the screen-space line segments the
/// minimap should draw for the active symmetry mode. Mirror modes
/// return one or two full-body crosshairs; diagonal modes return
/// a corner-to-corner cross; rotational modes return `fold` spokes
/// originating at the centre.
///
/// Returning `Vec<(Pos2, Pos2)>` rather than painting directly keeps
/// the geometry unit-testable without an `egui::Painter`.
pub fn minimap_symmetry_segments(body: Rect, symmetry: SymmetryAxis) -> Vec<(Pos2, Pos2)> {
    let c = body.center();
    match symmetry {
        SymmetryAxis::None => Vec::new(),
        SymmetryAxis::Horizontal => {
            vec![(Pos2::new(body.left(), c.y), Pos2::new(body.right(), c.y))]
        }
        SymmetryAxis::Vertical => vec![(Pos2::new(c.x, body.top()), Pos2::new(c.x, body.bottom()))],
        SymmetryAxis::Quad => vec![
            (Pos2::new(body.left(), c.y), Pos2::new(body.right(), c.y)),
            (Pos2::new(c.x, body.top()), Pos2::new(c.x, body.bottom())),
        ],
        SymmetryAxis::DiagonalMain => vec![(body.left_top(), body.right_bottom())],
        SymmetryAxis::DiagonalAnti => vec![(body.left_bottom(), body.right_top())],
        SymmetryAxis::Rotational { fold } => {
            if fold < 2 {
                Vec::new()
            } else {
                let r = (body.width().min(body.height())) * 0.5;
                let n = fold as u32;
                let mut out = Vec::with_capacity(n as usize);
                for k in 0..n {
                    let theta = (k as f32) * std::f32::consts::TAU / (n as f32);
                    let (s, co) = theta.sin_cos();
                    out.push((c, Pos2::new(c.x + r * co, c.y + r * s)));
                }
                out
            }
        }
    }
}

/// Paint the symmetry guide lines for `symmetry` inside `body`. Pure
/// renderer — geometry comes from [`minimap_symmetry_segments`].
fn paint_minimap_symmetry(painter: &egui::Painter, body: Rect, symmetry: SymmetryAxis) {
    let segs = minimap_symmetry_segments(body, symmetry);
    let stroke = Stroke::new(1.0, Color32::from_rgba_premultiplied(255, 255, 255, 90));
    for (a, b) in segs {
        painter.line_segment([a, b], stroke);
    }
}

/// Project a world-space (x, z) onto the mini-map body rect.
pub fn world_to_mini(body: Rect, world: (f32, f32), extents: (f32, f32)) -> Pos2 {
    let (x, z) = world;
    let (ex, ez) = extents;
    let u = if ex > 0.0 {
        (x / ex).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let v = if ez > 0.0 {
        (z / ez).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Pos2::new(
        body.left() + u * body.width(),
        body.top() + v * body.height(),
    )
}

/// Sample the splat distribution at a coarse grid and paint each cell
/// as a translucent RGBA overlay on top of the heightfield thumbnail.
/// D5 overlay rule (see module docs): R/G/B channels drive R/G/B
/// colour bumps; alpha drives a desaturated brightness bump so "snow"
/// reads as a white wash rather than transparent.
fn paint_splat_overlay(painter: &egui::Painter, body: Rect, d: &SplatDistribution) {
    let n = 48u32.min(d.width.min(d.height));
    if n == 0 {
        return;
    }
    let cell_w = body.width() / n as f32;
    let cell_h = body.height() / n as f32;
    for y in 0..n {
        for x in 0..n {
            let sx = (x as f32 / n as f32 * d.width as f32) as u32;
            let sy = (y as f32 / n as f32 * d.height as f32) as u32;
            let Some(px) = d.get(sx, sy) else { continue };
            // Skip empty pixels — overdraw of zero RGBA does nothing
            // visually but burns paint commands.
            if px == [0; 4] {
                continue;
            }
            // R/G/B as direct channels; A as desaturated bright wash.
            // Each channel weighted 0..=128 alpha so layered overlays
            // don't oversaturate the heightfield.
            let r = px[0] as u32;
            let g = px[1] as u32;
            let b = px[2] as u32;
            let a = px[3] as u32;
            let max = r.max(g).max(b).max(a) as f32 / 255.0;
            let alpha = (max * 128.0).clamp(0.0, 128.0) as u8;
            let mix_r = ((r + a) / 2).min(255) as u8;
            let mix_g = ((g + a) / 2).min(255) as u8;
            let mix_b = ((b + a) / 2).min(255) as u8;
            let color = Color32::from_rgba_premultiplied(mix_r, mix_g, mix_b, alpha);
            let cell_rect = Rect::from_min_size(
                Pos2::new(
                    body.left() + x as f32 * cell_w,
                    body.top() + y as f32 * cell_h,
                ),
                egui::vec2(cell_w + 0.5, cell_h + 0.5),
            );
            painter.rect_filled(cell_rect, CornerRadius::ZERO, color);
        }
    }
}

/// Sample the heightmap at a coarse grid and paint each cell as a
/// biome-coloured square. Cells are 1×1 in viewBox space; egui will
/// rasterise them at the body's pixel resolution.
fn paint_heightfield(painter: &egui::Painter, body: Rect, h: &Heightmap) {
    let n = 48u32.min(h.dims().0.min(h.dims().1));
    if n == 0 {
        return;
    }
    let cell_w = body.width() / n as f32;
    let cell_h = body.height() / n as f32;
    let data = h.data();
    let (hw, _hh) = h.dims();
    let max_sample = data.iter().copied().max().unwrap_or(1).max(1) as f32;
    for y in 0..n {
        for x in 0..n {
            // Nearest-neighbour sample.
            let sx = (x as f32 / n as f32 * h.dims().0 as f32) as u32;
            let sy = (y as f32 / n as f32 * h.dims().1 as f32) as u32;
            let idx = (sy * hw + sx) as usize;
            let v = (*data.get(idx).unwrap_or(&0)) as f32 / max_sample;
            let color = biome_ramp(v);
            let cell_rect = Rect::from_min_size(
                Pos2::new(
                    body.left() + x as f32 * cell_w,
                    body.top() + y as f32 * cell_h,
                ),
                egui::vec2(cell_w + 0.5, cell_h + 0.5),
            );
            painter.rect_filled(cell_rect, CornerRadius::ZERO, color);
        }
    }
}

/// Elevation → biome RGB ramp. Mirrors the mockup's ramp:
/// water (deep blue) → grass → mid stone → snow.
pub fn biome_ramp(t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.30 {
        Color32::from_rgb(40, 60, 86)
    } else if t < 0.45 {
        Color32::from_rgb(62, 100, 78)
    } else if t < 0.65 {
        Color32::from_rgb(104, 122, 92)
    } else if t < 0.82 {
        Color32::from_rgb(128, 124, 112)
    } else {
        Color32::from_rgb(220, 224, 230)
    }
}

/// Draw the camera frustum as a translucent trapezoid centred on the
/// body. We use the camera's `yaw` to point the wide end away from
/// the camera's facing direction.
fn paint_frustum(painter: &egui::Painter, body: Rect, camera: &OrbitCamera) {
    let t = Tokens::DARK;
    let c = body.center();
    let yaw = camera.yaw;
    let cos = yaw.cos();
    let sin = yaw.sin();
    // Local frustum points: apex behind camera, two splay points
    // forward. Transform by rotation around `c`.
    let pts_local = [(0.0, -25.0), (-25.0, 12.0), (25.0, 12.0)];
    let pts: Vec<Pos2> = pts_local
        .iter()
        .map(|(lx, ly)| {
            let rx = lx * cos - ly * sin;
            let ry = lx * sin + ly * cos;
            Pos2::new(c.x + rx, c.y + ry)
        })
        .collect();
    let fill = Color32::from_rgba_premultiplied(0x0E, 0x20, 0x3C, 0x40);
    painter.add(egui::Shape::convex_polygon(
        pts.clone(),
        fill,
        Stroke::new(1.0, t.accent),
    ));
    // Camera position dot at the apex.
    painter.circle_filled(pts[0], 2.6, t.accent);
    painter.circle_stroke(pts[0], 2.6, Stroke::new(0.6, Color32::WHITE));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_to_mini_maps_origin_to_top_left() {
        let body = Rect::from_min_size(Pos2::new(0.0, 0.0), egui::vec2(100.0, 100.0));
        let p = world_to_mini(body, (0.0, 0.0), (1000.0, 1000.0));
        assert!((p.x - 0.0).abs() < 0.001);
        assert!((p.y - 0.0).abs() < 0.001);
    }

    #[test]
    fn world_to_mini_maps_max_to_bottom_right() {
        let body = Rect::from_min_size(Pos2::new(0.0, 0.0), egui::vec2(100.0, 100.0));
        let p = world_to_mini(body, (1000.0, 1000.0), (1000.0, 1000.0));
        assert!((p.x - 100.0).abs() < 0.001);
        assert!((p.y - 100.0).abs() < 0.001);
    }

    #[test]
    fn world_to_mini_clamps_out_of_bounds() {
        let body = Rect::from_min_size(Pos2::new(0.0, 0.0), egui::vec2(100.0, 100.0));
        let p = world_to_mini(body, (5000.0, -200.0), (1000.0, 1000.0));
        // Clamps both axes to body.
        assert_eq!(p.x, 100.0);
        assert_eq!(p.y, 0.0);
    }

    #[test]
    fn biome_ramp_monotonic_in_brightness() {
        // Each band gets brighter than the last (luma).
        let luma = |c: Color32| (c.r() as u32 + c.g() as u32 + c.b() as u32) / 3;
        let bands = [0.1, 0.35, 0.55, 0.75, 0.95];
        let lumas: Vec<u32> = bands.iter().map(|&t| luma(biome_ramp(t))).collect();
        for w in lumas.windows(2) {
            assert!(w[1] >= w[0], "biome ramp not monotonic: {:?}", lumas);
        }
    }

    #[test]
    fn biome_ramp_clamps_extremes() {
        // Anything below 0 → water; anything above 1 → snow.
        let c0 = biome_ramp(-5.0);
        let c1 = biome_ramp(99.0);
        assert_eq!(c0, biome_ramp(0.0));
        assert_eq!(c1, biome_ramp(1.0));
    }

    // D5 / Sprint 9: splat overlay tests — exercise the pure mixing
    // math from `paint_splat_overlay` indirectly via small helpers.
    // The actual paint commands go through `egui::Painter`, which
    // needs a `Context`; the math we care about (per-channel mix +
    // alpha) is small enough to pin directly.

    /// Mirror of the per-pixel mix in `paint_splat_overlay` so we can
    /// unit-test the colour blend without an `egui::Painter`.
    fn overlay_mix(px: [u8; 4]) -> Option<(u8, u8, u8, u8)> {
        if px == [0; 4] {
            return None;
        }
        let r = px[0] as u32;
        let g = px[1] as u32;
        let b = px[2] as u32;
        let a = px[3] as u32;
        let max = r.max(g).max(b).max(a) as f32 / 255.0;
        let alpha = (max * 128.0).clamp(0.0, 128.0) as u8;
        let mix_r = ((r + a) / 2).min(255) as u8;
        let mix_g = ((g + a) / 2).min(255) as u8;
        let mix_b = ((b + a) / 2).min(255) as u8;
        Some((mix_r, mix_g, mix_b, alpha))
    }

    #[test]
    fn overlay_mix_zero_pixel_returns_none() {
        // Zero RGBA pixels are skipped to avoid burning paint
        // commands on invisible squares.
        assert!(overlay_mix([0, 0, 0, 0]).is_none());
    }

    #[test]
    fn overlay_mix_full_red_drives_red_alpha_128() {
        // Pure-R channel: red component dominates, alpha caps at 128.
        let m = overlay_mix([255, 0, 0, 0]).unwrap();
        assert_eq!(m.0, 127); // (255 + 0) / 2 = 127
        assert_eq!(m.1, 0);
        assert_eq!(m.2, 0);
        assert_eq!(m.3, 128);
    }

    #[test]
    fn overlay_mix_full_alpha_white_washes_evenly() {
        // Pure-A channel: alpha desaturates to (a/2) on all three
        // colour outs — reads as white wash on the heightfield.
        let m = overlay_mix([0, 0, 0, 255]).unwrap();
        assert_eq!(m.0, 127);
        assert_eq!(m.1, 127);
        assert_eq!(m.2, 127);
        assert_eq!(m.3, 128);
    }

    #[test]
    fn overlay_mix_alpha_scales_with_channel_intensity() {
        // 50 %-green pixel: alpha ≈ (128/255) * 128 = 64.
        let m = overlay_mix([0, 128, 0, 0]).unwrap();
        assert!(m.3 >= 60 && m.3 <= 70, "got alpha {}", m.3);
    }

    // ---------------------- minimap_symmetry_segments (Sprint 19 / U1) ----------------------

    fn unit_body() -> Rect {
        Rect::from_min_size(Pos2::new(0.0, 0.0), egui::vec2(100.0, 100.0))
    }

    #[test]
    fn minimap_symmetry_segments_none_returns_empty() {
        assert!(minimap_symmetry_segments(unit_body(), SymmetryAxis::None).is_empty());
    }

    #[test]
    fn minimap_symmetry_segments_horizontal_runs_left_to_right_through_centre() {
        let segs = minimap_symmetry_segments(unit_body(), SymmetryAxis::Horizontal);
        assert_eq!(segs.len(), 1);
        let (a, b) = segs[0];
        assert!((a.x - 0.0).abs() < 1e-3 && (b.x - 100.0).abs() < 1e-3);
        assert!((a.y - 50.0).abs() < 1e-3 && (b.y - 50.0).abs() < 1e-3);
    }

    #[test]
    fn minimap_symmetry_segments_vertical_runs_top_to_bottom_through_centre() {
        let segs = minimap_symmetry_segments(unit_body(), SymmetryAxis::Vertical);
        assert_eq!(segs.len(), 1);
        let (a, b) = segs[0];
        assert!((a.x - 50.0).abs() < 1e-3 && (b.x - 50.0).abs() < 1e-3);
        assert!((a.y - 0.0).abs() < 1e-3 && (b.y - 100.0).abs() < 1e-3);
    }

    #[test]
    fn minimap_symmetry_segments_quad_returns_both_mirror_axes() {
        assert_eq!(
            minimap_symmetry_segments(unit_body(), SymmetryAxis::Quad).len(),
            2
        );
    }

    #[test]
    fn minimap_symmetry_segments_diagonal_main_runs_corner_to_corner() {
        let segs = minimap_symmetry_segments(unit_body(), SymmetryAxis::DiagonalMain);
        assert_eq!(segs.len(), 1);
        let (a, b) = segs[0];
        assert_eq!(a, Pos2::new(0.0, 0.0));
        assert_eq!(b, Pos2::new(100.0, 100.0));
    }

    #[test]
    fn minimap_symmetry_segments_diagonal_anti_runs_opposite_corners() {
        let segs = minimap_symmetry_segments(unit_body(), SymmetryAxis::DiagonalAnti);
        assert_eq!(segs.len(), 1);
        let (a, b) = segs[0];
        assert_eq!(a, Pos2::new(0.0, 100.0));
        assert_eq!(b, Pos2::new(100.0, 0.0));
    }

    #[test]
    fn minimap_symmetry_segments_rotational_returns_fold_spokes() {
        for &fold in &[2u8, 3, 4, 6, 8, 12] {
            let segs = minimap_symmetry_segments(unit_body(), SymmetryAxis::Rotational { fold });
            assert_eq!(segs.len(), fold as usize, "fold={fold}");
        }
    }

    #[test]
    fn minimap_symmetry_segments_rotational_fold_lt_2_returns_empty() {
        for fold in [0u8, 1] {
            let segs = minimap_symmetry_segments(unit_body(), SymmetryAxis::Rotational { fold });
            assert!(segs.is_empty(), "fold={fold} should be empty");
        }
    }
}
