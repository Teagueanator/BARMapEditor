//! Viewport overlays — elmo rulers, the floating "view options" toolbar
//! (grid / lighting / wireframe / view mode), and the empty-state
//! "Create map" CTA (ADR-035).
//!
//! All painters here are stateless. The toolbar interaction state lives
//! on `App` (3 bools); pure rendering happens here.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui,
};

use crate::render::{OrbitCamera, screen_to_world_y0};
use crate::ui::help_text::{HelpId, help};
use crate::ui::icons::{self, Icon};
use crate::ui::theme::Tokens;

/// Paint elmo rulers along the bottom and left edges of the viewport
/// rect. Labels reflect the WORLD coordinates currently visible under
/// each tick, so as the camera zooms / pans / orbits the labels
/// rescale automatically. Step size is picked from a 1-2-5 sequence
/// so ~5-8 ticks fit in the visible range.
///
/// **Sprint 14 follow-up**: the pre-rewrite ruler used a fixed
/// `extent`-linear mapping that ignored the camera projection — at
/// any zoom level the labels stayed at 1024 / 2048 / ... pinned to
/// the same fractional positions in the rect, which made them
/// useless once the user started navigating. The new implementation
/// projects screen positions back to the world XZ plane and labels
/// the world coordinate at each tick.
pub fn paint_rulers(painter: &egui::Painter, rect: Rect, camera: &OrbitCamera) {
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }
    let t = Tokens::DARK;
    let rect_size = glam::Vec2::new(rect.width(), rect.height());

    // Track backgrounds — drawn unconditionally so the user sees a
    // consistent UI shape even when the camera projection isn't
    // resolvable (e.g. mid-orbit at extreme pitches).
    let track_bg = Color32::from_rgba_premultiplied(8, 12, 18, 140);
    painter.rect_filled(
        Rect::from_min_size(
            Pos2::new(rect.left(), rect.bottom() - 18.0),
            egui::vec2(rect.width(), 18.0),
        ),
        CornerRadius::ZERO,
        track_bg,
    );
    painter.rect_filled(
        Rect::from_min_size(rect.left_top(), egui::vec2(18.0, rect.height())),
        CornerRadius::ZERO,
        track_bg,
    );

    // ── Bottom ruler — world X under each screen X along the bottom
    //    edge. Sample 17 evenly-spaced screen positions so the
    //    piecewise interpolation is accurate enough at extreme
    //    obliquities without paying per-pixel cost.
    let sy_bottom = rect.height() - 9.0;
    let bottom_samples: Vec<(f32, f32)> = (0..=16)
        .filter_map(|i| {
            let sx_local = (i as f32 / 16.0) * rect.width();
            let cursor = glam::Vec2::new(sx_local, sy_bottom);
            screen_to_world_y0(cursor, rect_size, camera).map(|w| (sx_local, w.x))
        })
        .collect();
    paint_axis_ticks(painter, &bottom_samples, |sx_local, is_major, label| {
        let x = rect.left() + sx_local;
        let tick_h = if is_major { 8.0 } else { 4.0 };
        painter.line_segment(
            [
                Pos2::new(x, rect.bottom() - 18.0),
                Pos2::new(x, rect.bottom() - 18.0 + tick_h),
            ],
            Stroke::new(1.0, t.dim),
        );
        if let Some(label) = label {
            painter.text(
                Pos2::new(x, rect.bottom() - 3.0),
                Align2::CENTER_BOTTOM,
                label,
                FontId::monospace(9.0),
                t.muted,
            );
        }
    });

    // ── Left ruler — world Z down each screen Y along the left
    //    edge. Same sampling pattern.
    let sx_left = 9.0;
    let left_samples: Vec<(f32, f32)> = (0..=16)
        .filter_map(|i| {
            let sy_local = (i as f32 / 16.0) * rect.height();
            let cursor = glam::Vec2::new(sx_left, sy_local);
            screen_to_world_y0(cursor, rect_size, camera).map(|w| (sy_local, w.z))
        })
        .collect();
    paint_axis_ticks(painter, &left_samples, |sy_local, is_major, label| {
        let y = rect.top() + sy_local;
        let tick_w = if is_major { 8.0 } else { 4.0 };
        painter.line_segment(
            [
                Pos2::new(rect.left() + 18.0, y),
                Pos2::new(rect.left() + 18.0 - tick_w, y),
            ],
            Stroke::new(1.0, t.dim),
        );
        if let Some(label) = label {
            painter.text(
                Pos2::new(rect.left() + 3.0, y),
                Align2::LEFT_CENTER,
                label,
                FontId::monospace(9.0),
                t.muted,
            );
        }
    });
}

/// Inner helper: given `samples` of `(screen_pos, world_coord)`
/// along one axis of the ruler, pick a "nice" world step (1-2-5
/// sequence) that fits ~6 labels into the visible world range, then
/// invoke `paint(screen_pos, is_major, label)` for each tick.
/// `samples` must be sorted by `screen_pos` ascending (the caller
/// produces them that way by walking `i in 0..=16`).
fn paint_axis_ticks(
    _painter: &egui::Painter,
    samples: &[(f32, f32)],
    mut paint: impl FnMut(f32, bool, Option<String>),
) {
    if samples.len() < 2 {
        return;
    }
    // Compute visible world range. Use min/max rather than first/last
    // so an oblique camera (where world coords aren't monotonic in
    // screen position) doesn't get a tighter-than-real range.
    let mut world_min = f32::INFINITY;
    let mut world_max = f32::NEG_INFINITY;
    for &(_, w) in samples {
        if w < world_min {
            world_min = w;
        }
        if w > world_max {
            world_max = w;
        }
    }
    if !world_min.is_finite() || !world_max.is_finite() || world_max <= world_min {
        return;
    }
    let range = world_max - world_min;
    let major_step = pick_nice_step(range, 6);
    let minor_step = major_step / 2.0;

    // Walk world values in minor-step increments, drawing a tick
    // (and a label every other tick) at the interpolated screen
    // position. Skip ticks whose interpolation fails (rare — only
    // at sample-table edges).
    let start = (world_min / minor_step).ceil() * minor_step;
    let mut w = start;
    // Cap the loop count so a degenerate camera (massive range) can't
    // freeze the UI. 200 ticks across the visible range is far more
    // than the human eye can resolve.
    let mut emitted = 0;
    while w <= world_max && emitted < 200 {
        let Some(screen_pos) = interp_screen_pos(w, samples) else {
            w += minor_step;
            emitted += 1;
            continue;
        };
        let is_major = (w / major_step).round() * major_step;
        let is_major_tick = (w - is_major).abs() < minor_step * 0.5;
        let label = if is_major_tick {
            Some(format_label(w))
        } else {
            None
        };
        paint(screen_pos, is_major_tick, label);
        w += minor_step;
        emitted += 1;
    }
}

/// Find the screen position at which `target_world` falls, by
/// linear interpolation between consecutive `(screen, world)`
/// samples. Returns `None` if `target_world` is outside every
/// sample-pair range.
fn interp_screen_pos(target_world: f32, samples: &[(f32, f32)]) -> Option<f32> {
    for pair in samples.windows(2) {
        let (s0, w0) = pair[0];
        let (s1, w1) = pair[1];
        let (lo, hi) = if w0 < w1 { (w0, w1) } else { (w1, w0) };
        if target_world >= lo && target_world <= hi {
            if (w1 - w0).abs() < 1e-6 {
                return Some(s0);
            }
            let t = (target_world - w0) / (w1 - w0);
            return Some(s0 + t * (s1 - s0));
        }
    }
    None
}

/// Pick a "nice" tick step from the 1-2-5 sequence. `range` is the
/// span of values that should be subdivided into roughly `target`
/// ticks. The result is the smallest 1-2-5×10^k value that produces
/// at most `target` ticks in `range`.
///
/// `range = 5000, target = 6` → step = 1000 (5 ticks).
/// `range = 800, target = 6` → step = 200 (4 ticks).
/// `range = 80, target = 6` → step = 20 (4 ticks).
pub fn pick_nice_step(range: f32, target: u32) -> f32 {
    if range <= 0.0 || target == 0 {
        return 1.0;
    }
    let raw = range / target as f32;
    let pow10 = 10f32.powf(raw.log10().floor());
    let normalized = raw / pow10;
    let step_n = if normalized <= 1.0 {
        1.0
    } else if normalized <= 2.0 {
        2.0
    } else if normalized <= 5.0 {
        5.0
    } else {
        10.0
    };
    step_n * pow10
}

/// Render a world coordinate as a ruler label. Integers under 10 000
/// render as plain integers; >= 10 000 renders in `k` (kilo) units
/// to keep the label short ("12500" → "12.5k").
fn format_label(w: f32) -> String {
    let wi = w.round() as i32;
    if wi.abs() >= 10_000 {
        let k = (w / 1000.0).round() as i32;
        // Whole-thousand values render as bare k; fractions get one decimal.
        if (w / 1000.0 - k as f32).abs() < 0.05 {
            format!("{k}k")
        } else {
            format!("{:.1}k", w / 1000.0)
        }
    } else {
        format!("{wi}")
    }
}

/// Floating viewport-options toolbar — top-left of the viewport (inside
/// the rulers). Renders three toggle pills + a view-mode chip.
///
/// Returns `true` if any toggle changed this frame — caller can use it
/// to log / repaint / flag dirty.
pub fn viewport_options_toolbar(
    ui: &mut Ui,
    grid: &mut bool,
    lighting: &mut bool,
    wireframe: &mut bool,
    buildable: &mut bool,
) -> bool {
    let t = Tokens::DARK;
    let mut changed = false;
    egui::Frame::new()
        .fill(Color32::from_rgba_premultiplied(8, 12, 18, 200))
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::same(3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Sprint 19 / U1 — the legacy `(G)` / `(L)` / `(W)` chord
                // hints in these tooltips were false promises. Those
                // letters are bound to tool accelerators in
                // `handle_keyboard` (Procgen / PaintLayer / Water), not
                // to viewport toggles. Strip the chord; keep the
                // descriptive text.
                if vp_toggle_btn(ui, Icon::Grid, *grid, help(HelpId::ViewportGrid)) {
                    *grid = !*grid;
                    changed = true;
                }
                if vp_toggle_btn(ui, Icon::Light, *lighting, help(HelpId::ViewportLighting)) {
                    *lighting = !*lighting;
                    changed = true;
                }
                if vp_toggle_btn(ui, Icon::Wire, *wireframe, help(HelpId::ViewportWireframe)) {
                    *wireframe = !*wireframe;
                    changed = true;
                }
                // Buildable-area overlay (Sprint 11 hotfix follow-up).
                // Red mask in the viewport over terrain that's too steep
                // for a factory (slope > 10° per the BAR research:
                // `armlab.lua` / `corlab.lua` set `maxslope = 15`, the
                // engine divides by 1.5 → effective 10° cap).
                if vp_toggle_btn(ui, Icon::Build, *buildable, help(HelpId::ViewportBuildable)) {
                    *buildable = !*buildable;
                    changed = true;
                }
            });
        });
    changed
}

fn vp_toggle_btn(ui: &mut Ui, icon: Icon, on: bool, tooltip: &str) -> bool {
    let t = Tokens::DARK;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 24.0), Sense::click());
    let painter = ui.painter();
    if on {
        painter.rect_filled(rect, CornerRadius::same(4), t.accent);
    } else if response.hovered() {
        painter.rect_filled(rect, CornerRadius::same(4), t.hover);
    }
    let color = if on { Color32::WHITE } else { t.muted };
    let icon_rect = Rect::from_center_size(rect.center(), egui::vec2(14.0, 14.0));
    icons::paint_icon(painter, icon_rect, icon, color, 1.5);
    response.on_hover_text(tooltip).clicked()
}

/// Empty-state CTA card — centred inside the viewport when no
/// heightmap is loaded. Returns `true` when "Create map" is clicked.
pub fn empty_state_cta(ui: &mut Ui, rect: Rect) -> EmptyStateClick {
    let t = Tokens::DARK;
    let card_size = egui::vec2(340.0, 180.0);
    let card_rect = Rect::from_center_size(rect.center(), card_size);
    let painter = ui.painter();
    painter.rect_filled(
        card_rect,
        CornerRadius::same(10),
        Color32::from_rgba_premultiplied(20, 22, 28, 190),
    );
    painter.rect_stroke(
        card_rect,
        CornerRadius::same(10),
        Stroke::new(1.0, t.border),
        StrokeKind::Middle,
    );
    // Plus icon hero.
    let icon_rect = Rect::from_center_size(
        Pos2::new(card_rect.center().x, card_rect.top() + 36.0),
        egui::vec2(42.0, 42.0),
    );
    painter.rect_filled(icon_rect, CornerRadius::same(8), t.accent_alpha(0x2E));
    painter.rect_stroke(
        icon_rect,
        CornerRadius::same(8),
        Stroke::new(1.0, t.accent_dim),
        StrokeKind::Middle,
    );
    let inner_icon = Rect::from_center_size(icon_rect.center(), egui::vec2(22.0, 22.0));
    icons::paint_icon(painter, inner_icon, Icon::Plus, t.accent, 1.8);
    // Heading.
    painter.text(
        Pos2::new(card_rect.center().x, card_rect.top() + 90.0),
        Align2::CENTER_CENTER,
        "No project loaded",
        FontId::proportional(17.0),
        t.text,
    );
    painter.text(
        Pos2::new(card_rect.center().x, card_rect.top() + 110.0),
        Align2::CENTER_CENTER,
        "Start with a preset terrain, or open an existing .barmeproj from disk.",
        FontId::proportional(11.5),
        t.muted,
    );
    // Buttons.
    let mut click = EmptyStateClick::None;
    let btn_w = 110.0;
    let btn_h = 32.0;
    let btn_y = card_rect.bottom() - 22.0;
    let create_rect = Rect::from_center_size(
        Pos2::new(card_rect.center().x - 60.0, btn_y),
        egui::vec2(btn_w, btn_h),
    );
    let open_rect = Rect::from_center_size(
        Pos2::new(card_rect.center().x + 60.0, btn_y),
        egui::vec2(btn_w, btn_h),
    );
    let create_resp = ui.interact(create_rect, ui.id().with("empty_create"), Sense::click());
    let open_resp = ui.interact(open_rect, ui.id().with("empty_open"), Sense::click());
    let painter = ui.painter();
    painter.rect_filled(create_rect, CornerRadius::same(5), t.accent);
    painter.text(
        create_rect.center(),
        Align2::CENTER_CENTER,
        "Create map",
        FontId::proportional(13.0),
        Color32::WHITE,
    );
    painter.rect_filled(open_rect, CornerRadius::same(5), t.panel2);
    painter.rect_stroke(
        open_rect,
        CornerRadius::same(5),
        Stroke::new(1.0, t.border),
        StrokeKind::Middle,
    );
    painter.text(
        open_rect.center(),
        Align2::CENTER_CENTER,
        "Open…",
        FontId::proportional(13.0),
        t.text,
    );
    if create_resp.clicked() {
        click = EmptyStateClick::Create;
    } else if open_resp.clicked() {
        click = EmptyStateClick::Open;
    }
    click
}

/// Bottom-centre translucent hint card listing the camera gestures.
/// Caller wires `*show = false` when the user clicks the X.
pub fn hint_card(ui: &mut Ui, rect: Rect, show: &mut bool) {
    if !*show {
        return;
    }
    let t = Tokens::DARK;
    let card_size = egui::vec2(420.0, 38.0);
    let card_rect = Rect::from_min_size(
        Pos2::new(
            rect.center().x - card_size.x * 0.5,
            rect.bottom() - card_size.y - 24.0,
        ),
        card_size,
    );
    let painter = ui.painter();
    painter.rect_filled(
        card_rect,
        CornerRadius::same(6),
        Color32::from_rgba_premultiplied(20, 26, 34, 200),
    );
    painter.rect_stroke(
        card_rect,
        CornerRadius::same(6),
        Stroke::new(1.0, t.border),
        StrokeKind::Middle,
    );
    let mut x = card_rect.left() + 12.0;
    for hint in ["RMB · orbit", "MMB · pan", "Scroll · zoom", "?  shortcuts"] {
        let galley = painter.layout_no_wrap(hint.to_string(), FontId::proportional(11.0), t.muted);
        painter.galley(
            Pos2::new(
                x,
                card_rect.top() + (card_rect.height() - galley.size().y) * 0.5,
            ),
            galley.clone(),
            t.muted,
        );
        x += galley.size().x + 14.0;
    }
    // Dismiss button on the right.
    let x_rect = Rect::from_center_size(
        Pos2::new(card_rect.right() - 16.0, card_rect.center().y),
        egui::vec2(16.0, 16.0),
    );
    let close_resp = ui.interact(x_rect, ui.id().with("hint_close"), Sense::click());
    icons::paint_icon(ui.painter(), x_rect, Icon::X, t.muted, 1.6);
    if close_resp.clicked() {
        *show = false;
    }
}

/// Empty-state CTA click outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmptyStateClick {
    None,
    Create,
    Open,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sprint 14 follow-up: the dynamic ruler picks tick spacing
    /// from a 1-2-5 sequence so labels stay round at every zoom
    /// level. Verify the math on a few representative ranges.
    #[test]
    fn nice_step_picks_clean_values_for_typical_ranges() {
        // 16-SMU map fully framed: visible range ≈ 8000 elmos →
        // step 1000 (8 ticks fits in 6 target).
        assert_eq!(pick_nice_step(8000.0, 6), 2000.0);
        // 4-SMU map: ≈ 2000 elmos → step 500.
        assert_eq!(pick_nice_step(2000.0, 6), 500.0);
        // Zoomed-in: ≈ 800 elmos → step 200.
        assert_eq!(pick_nice_step(800.0, 6), 200.0);
        // Heavy zoom: ≈ 80 elmos → step 20.
        assert_eq!(pick_nice_step(80.0, 6), 20.0);
        // Zoomed way out: ≈ 50_000 elmos → step 10000.
        assert_eq!(pick_nice_step(50_000.0, 6), 10_000.0);
    }

    #[test]
    fn nice_step_rejects_degenerate_inputs() {
        assert_eq!(pick_nice_step(0.0, 6), 1.0);
        assert_eq!(pick_nice_step(-100.0, 6), 1.0);
        assert_eq!(pick_nice_step(1000.0, 0), 1.0);
    }

    /// At an oblique camera angle the world coordinate isn't a
    /// strictly-linear function of screen position; the piecewise
    /// interpolator should still resolve a screen position for any
    /// world value contained in at least one segment. First match
    /// wins by design.
    #[test]
    fn interp_screen_pos_handles_non_monotone_samples() {
        // Samples where world coord increases (0→50: 100→200) then
        // decreases (50→100: 200→150).
        let samples = [(0.0, 100.0), (50.0, 200.0), (100.0, 150.0)];
        // 175 falls on the first ascending segment (75% of the way
        // through 100→200) — returns screen 37.5.
        let p = interp_screen_pos(175.0, &samples).unwrap();
        assert!((p - 37.5).abs() < 1e-3, "got {p}");
        // 150 is reachable on both segments — first match wins.
        let _ = interp_screen_pos(150.0, &samples).unwrap();
        // Descending segment alone: 175 falls halfway from 200 → 150
        // → screen at 50 % of (0..50) = 25.
        let descending = [(0.0, 200.0), (50.0, 150.0)];
        let q = interp_screen_pos(175.0, &descending).unwrap();
        assert!((q - 25.0).abs() < 1e-3, "got {q}");
    }

    #[test]
    fn interp_screen_pos_returns_none_for_out_of_range() {
        let samples = [(0.0, 100.0), (50.0, 200.0)];
        assert!(interp_screen_pos(50.0, &samples).is_none()); // below range
        assert!(interp_screen_pos(250.0, &samples).is_none()); // above range
    }
}
