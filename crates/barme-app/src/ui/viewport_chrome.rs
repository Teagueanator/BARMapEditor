//! Viewport overlays — elmo rulers, the floating "view options" toolbar
//! (grid / lighting / wireframe / view mode), and the empty-state
//! "Create map" CTA (ADR-035).
//!
//! All painters here are stateless. The toolbar interaction state lives
//! on `App` (3 bools); pure rendering happens here.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui,
};

use crate::ui::icons::{self, Icon};
use crate::ui::theme::Tokens;

/// Tick spacing for the bottom + left rulers (in elmos). Ticks are
/// drawn every `MINOR`; labels every `MAJOR`.
const RULER_MINOR_ELMOS: f32 = 512.0;
const RULER_MAJOR_ELMOS: f32 = 1024.0;

/// Paint elmo rulers along the bottom and left edges of the viewport
/// rect. `extents` is the map's world-space size in elmos
/// ((dims.x - 1) * 8, (dims.z - 1) * 8). Caller passes the same rect
/// the terrain is rendered into.
pub fn paint_rulers(painter: &egui::Painter, rect: Rect, extents: (f32, f32)) {
    let t = Tokens::DARK;
    let (ex, ez) = extents;
    if ex <= 0.0 || ez <= 0.0 {
        return;
    }
    // Tracks.
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
    // Bottom ruler.
    let mut e = 0.0_f32;
    while e <= ex + 0.5 {
        let x = rect.left() + (e / ex) * rect.width();
        let is_major = (e % RULER_MAJOR_ELMOS).abs() < 0.5;
        let tick_h = if is_major { 8.0 } else { 4.0 };
        painter.line_segment(
            [
                Pos2::new(x, rect.bottom() - 18.0),
                Pos2::new(x, rect.bottom() - 18.0 + tick_h),
            ],
            Stroke::new(1.0, t.dim),
        );
        if is_major && e > 0.0 {
            painter.text(
                Pos2::new(x, rect.bottom() - 3.0),
                Align2::CENTER_BOTTOM,
                format!("{}", e as i32),
                FontId::monospace(9.0),
                t.muted,
            );
        }
        e += RULER_MINOR_ELMOS;
    }
    // Left ruler.
    let mut e = 0.0_f32;
    while e <= ez + 0.5 {
        let y = rect.top() + (e / ez) * rect.height();
        let is_major = (e % RULER_MAJOR_ELMOS).abs() < 0.5;
        let tick_w = if is_major { 8.0 } else { 4.0 };
        painter.line_segment(
            [
                Pos2::new(rect.left() + 18.0, y),
                Pos2::new(rect.left() + 18.0 - tick_w, y),
            ],
            Stroke::new(1.0, t.dim),
        );
        if is_major && e > 0.0 {
            painter.text(
                Pos2::new(rect.left() + 3.0, y),
                Align2::LEFT_CENTER,
                format!("{}", e as i32),
                FontId::monospace(9.0),
                t.muted,
            );
        }
        e += RULER_MINOR_ELMOS;
    }
}

/// Pure helper: how many major-labelled ticks would the bottom ruler
/// emit for `extent` elmos? Unit-tested so changing the tick
/// constants doesn't silently break the visual.
#[allow(dead_code)]
pub fn ruler_label_count(extent_elmos: f32) -> u32 {
    if extent_elmos <= 0.0 {
        return 0;
    }
    // Inclusive of 0; matches the painter's while-loop bound.
    (extent_elmos / RULER_MAJOR_ELMOS).floor() as u32 + 1
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
                if vp_toggle_btn(ui, Icon::Grid, *grid, "Coordinate grid (G)") {
                    *grid = !*grid;
                    changed = true;
                }
                if vp_toggle_btn(ui, Icon::Light, *lighting, "Lighting (L)") {
                    *lighting = !*lighting;
                    changed = true;
                }
                if vp_toggle_btn(ui, Icon::Wire, *wireframe, "Wireframe (W)") {
                    *wireframe = !*wireframe;
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

    #[test]
    fn ruler_label_count_matches_extent() {
        // 16 × 16 SMU → 8192 elmos. Labels every 1024 elmos including
        // 0 → ⌊8192 / 1024⌋ + 1 = 9.
        assert_eq!(ruler_label_count(8192.0), 9);
    }

    #[test]
    fn ruler_label_count_handles_zero() {
        assert_eq!(ruler_label_count(0.0), 0);
        assert_eq!(ruler_label_count(-100.0), 0);
    }

    #[test]
    fn ruler_label_count_round_extent() {
        // 4096 elmos → labels at 0, 1024, 2048, 3072, 4096 = 5.
        assert_eq!(ruler_label_count(4096.0), 5);
    }
}
