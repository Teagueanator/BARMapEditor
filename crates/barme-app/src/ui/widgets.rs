//! Reusable UI primitives — chips, sections, ramp sliders, pill toggles,
//! split buttons, key-combo glyphs (ADR-035).
//!
//! These are pure rendering helpers. None of them own app state; every
//! interactive widget returns an [`egui::Response`] so the caller can
//! observe `clicked()` / `changed()` and update its own model.
//!
//! Why custom widgets?  The mockup uses a uniform visual language —
//! 4 px corner radii, 1 px borders, 10-px-pill chips, gradient-filled
//! "ramp" sliders — that egui's stock widgets approximate but don't
//! quite match. Centralising the look here means a token change in
//! [`theme.rs`] re-themes the whole editor.

use eframe::egui::{
    self, Color32, CornerRadius, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use crate::ui::help_text::{HelpId, help};
use crate::ui::icons::{self, Icon};
use crate::ui::theme::{ChipTone, Tokens};

/// Render a small uppercase section header inside a panel. `accent`
/// adds the 3 px accent rail to the left of the label that the mockup
/// uses on the first section of each tool's Inspector. `right` runs
/// inside a right-aligned strip on the same row (use it for chips or
/// "+ Add" affordances).
///
/// The body closure renders below the header with a 1-pixel divider
/// underneath the whole section.
pub fn section<R>(
    ui: &mut Ui,
    title: &str,
    accent: bool,
    right: impl FnOnce(&mut Ui),
    body: impl FnOnce(&mut Ui) -> R,
) -> R {
    section_inner(ui, title, accent, None, right, body)
}

/// Sprint 19 / U1 — variant of [`section`] that attaches a hover-text
/// tooltip to the section title. The interaction region covers the
/// title label only (and the accent rail when present), so right-side
/// chips and `+ Add` buttons keep their own per-widget tooltips
/// untouched.
///
/// Callers who don't need a tooltip should keep using [`section`];
/// adding a `_hover` variant rather than changing the signature
/// avoids churning the 50+ existing call sites for Sprint 19's
/// annotative pass.
pub fn section_with_hover<R>(
    ui: &mut Ui,
    title: &str,
    accent: bool,
    header_hover: &str,
    right: impl FnOnce(&mut Ui),
    body: impl FnOnce(&mut Ui) -> R,
) -> R {
    section_inner(ui, title, accent, Some(header_hover), right, body)
}

fn section_inner<R>(
    ui: &mut Ui,
    title: &str,
    accent: bool,
    header_hover: Option<&str>,
    right: impl FnOnce(&mut Ui),
    body: impl FnOnce(&mut Ui) -> R,
) -> R {
    let t = Tokens::DARK;
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 14,
            right: 14,
            top: 10,
            bottom: 12,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if accent {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(3.0, 11.0), Sense::hover());
                    ui.painter().rect_filled(rect, 1.0, t.accent);
                    ui.add_space(4.0);
                }
                let label_resp = ui.add(
                    egui::Label::new(
                        egui::RichText::new(title.to_uppercase())
                            .color(t.text)
                            .size(11.0)
                            .strong(),
                    )
                    .sense(if header_hover.is_some() {
                        Sense::hover()
                    } else {
                        Sense::empty()
                    }),
                );
                if let Some(hover) = header_hover {
                    label_resp.on_hover_text(hover);
                }
                // Right-aligned trailing content.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
            });
            ui.add_space(8.0);
            let result = body(ui);
            ui.add_space(8.0);
            // Draw the bottom divider via a one-pixel-tall painted rect.
            let avail = ui.available_width();
            let (rect, _) = ui.allocate_exact_size(egui::vec2(avail, 1.0), Sense::hover());
            ui.painter().rect_filled(rect, 0.0, t.border);
            result
        })
        .inner
}

/// Pill-shaped status chip. `tone` drives both the foreground colour
/// and the translucent background fill.
pub fn chip(ui: &mut Ui, tone: ChipTone, label: impl Into<String>) -> Response {
    chip_with_icon(ui, tone, None, label)
}

/// Same as [`chip`] but with a leading icon glyph painted to the left
/// of the label.
pub fn chip_with_icon(
    ui: &mut Ui,
    tone: ChipTone,
    icon: Option<Icon>,
    label: impl Into<String>,
) -> Response {
    let t = Tokens::DARK;
    let label: String = label.into();
    let icon_w = if icon.is_some() { 12.0 } else { 0.0 };
    let text_galley = ui.painter().layout_no_wrap(
        label.clone(),
        egui::FontId::proportional(11.0),
        t.chip_fg(tone),
    );
    let inner_pad_x = 8.0;
    let gap = if icon.is_some() { 4.0 } else { 0.0 };
    let total_w = inner_pad_x * 2.0 + icon_w + gap + text_galley.size().x;
    let total_h = 18.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(total_w, total_h), Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(9), t.chip_bg(tone));
    let mut cursor_x = rect.left() + inner_pad_x;
    if let Some(icon) = icon {
        let icon_rect = Rect::from_min_size(
            Pos2::new(cursor_x, rect.top() + (total_h - 11.0) * 0.5),
            egui::vec2(11.0, 11.0),
        );
        icons::paint_icon(painter, icon_rect, icon, t.chip_fg(tone), 1.4);
        cursor_x += icon_w + gap;
    }
    let text_pos = Pos2::new(
        cursor_x,
        rect.top() + (total_h - text_galley.size().y) * 0.5,
    );
    painter.galley(text_pos, text_galley, t.chip_fg(tone));
    response
}

/// Horizontal ramp slider with a gradient fill bar. The fill colour is
/// the caller's choice — pass [`Tokens::accent`] for "value" sliders or
/// a domain-specific colour (red metal density, amber temperature,
/// etc.).
///
/// Returns the response of the underlying drag region; `response.changed()`
/// fires when the user drags the handle.
#[must_use = "drop the result if you don't need the response"]
pub fn ramp_slider(
    ui: &mut Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    color: Color32,
) -> Response {
    let t = Tokens::DARK;
    let height = 14.0;
    let desired = egui::vec2(ui.available_width(), height);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click_and_drag());

    let lo = *range.start();
    let hi = *range.end();
    let span = (hi - lo).max(f32::EPSILON);
    let mut t_val = ((*value - lo) / span).clamp(0.0, 1.0);

    if (response.dragged() || response.clicked())
        && let Some(pos) = response.interact_pointer_pos()
    {
        t_val = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        *value = lo + t_val * span;
    }

    let painter = ui.painter();
    // Frame.
    painter.rect_filled(rect, CornerRadius::same(2), t.panel2);
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0, t.border),
        StrokeKind::Middle,
    );
    // Fill portion.
    let fill_rect = Rect::from_min_max(
        rect.left_top(),
        Pos2::new(rect.left() + rect.width() * t_val, rect.bottom()),
    );
    painter.rect_filled(fill_rect, CornerRadius::same(2), color);
    // Handle indicator.
    let handle_x = rect.left() + rect.width() * t_val;
    let handle_rect = Rect::from_min_max(
        Pos2::new(handle_x - 1.0, rect.top() - 3.0),
        Pos2::new(handle_x + 1.0, rect.bottom() + 3.0),
    );
    painter.rect_filled(handle_rect, CornerRadius::same(1), Color32::WHITE);

    response
}

/// `ramp_slider` plus a labelled header row showing the value next to
/// the slider's caption.
pub fn ramp_slider_labelled(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    color: Color32,
    value_text: impl Into<String>,
) -> Response {
    let t = Tokens::DARK;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(t.muted).size(11.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(value_text.into())
                    .color(t.text)
                    .size(11.0)
                    .monospace(),
            );
        });
    });
    ui.add_space(2.0);
    ramp_slider(ui, value, range, color)
}

/// Toggle pill — used inside the symmetry cluster ("Symmetry: ON").
/// Returns the clicked() response.
pub fn pill_toggle(ui: &mut Ui, label: &str, on: &mut bool) -> Response {
    let t = Tokens::DARK;
    let text_galley = ui.painter().layout_no_wrap(
        label.to_uppercase(),
        egui::FontId::proportional(10.0),
        if *on { Color32::WHITE } else { t.muted },
    );
    let inner_pad = 8.0;
    let track_w = 20.0;
    let track_h = 12.0;
    let total_w = inner_pad + track_w + 6.0 + text_galley.size().x + inner_pad;
    let total_h = 24.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(total_w, total_h), Sense::click());
    if response.clicked() {
        *on = !*on;
    }
    let painter = ui.painter();
    let bg = if *on { t.accent } else { Color32::TRANSPARENT };
    painter.rect_filled(rect, CornerRadius::same(4), bg);
    // Track.
    let track_rect = Rect::from_min_size(
        Pos2::new(
            rect.left() + inner_pad,
            rect.top() + (total_h - track_h) * 0.5,
        ),
        egui::vec2(track_w, track_h),
    );
    let track_bg = if *on {
        Color32::from_rgba_premultiplied(0xFF, 0xFF, 0xFF, 0x4D)
    } else {
        t.border
    };
    painter.rect_filled(track_rect, CornerRadius::same(8), track_bg);
    // Knob.
    let knob_d = 10.0;
    let knob_x = if *on {
        track_rect.right() - knob_d - 1.0
    } else {
        track_rect.left() + 1.0
    };
    let knob_rect = Rect::from_min_size(
        Pos2::new(knob_x, track_rect.top() + 1.0),
        egui::vec2(knob_d, knob_d),
    );
    painter.rect_filled(knob_rect, CornerRadius::same(5), Color32::WHITE);
    // Label.
    let text_pos = Pos2::new(
        track_rect.right() + 6.0,
        rect.top() + (total_h - text_galley.size().y) * 0.5,
    );
    painter.galley(
        text_pos,
        text_galley,
        if *on { Color32::WHITE } else { t.muted },
    );
    response
}

/// Inline kbd-style key chip — renders a label inside a small bordered
/// pill that reads like a physical key.
pub fn key_combo(ui: &mut Ui, combo: &str) -> Response {
    let t = Tokens::DARK;
    ui.horizontal(|ui| {
        let parts: Vec<&str> = combo.split('+').collect();
        let last = parts.len() - 1;
        for (i, p) in parts.iter().enumerate() {
            let galley =
                ui.painter()
                    .layout_no_wrap(p.to_string(), egui::FontId::monospace(10.0), t.text);
            let pad = egui::vec2(5.0, 1.0);
            let size = galley.size() + pad * 2.0;
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, CornerRadius::same(3), t.bg);
            painter.rect_stroke(
                rect,
                CornerRadius::same(3),
                Stroke::new(1.0, t.border),
                StrokeKind::Middle,
            );
            painter.galley(rect.min + pad, galley, t.text);
            if i < last {
                ui.label(egui::RichText::new("+").color(t.dim).size(11.0));
            }
        }
    })
    .response
}

/// Split button: primary action on the left, dropdown caret on the
/// right with its own click region. Returns (primary_response, caret_response).
pub fn split_button(
    ui: &mut Ui,
    icon: Option<Icon>,
    label: &str,
    accent: bool,
) -> (Response, Response) {
    let t = Tokens::DARK;
    let height = 30.0;
    let icon_w = if icon.is_some() { 14.0 } else { 0.0 };
    let gap = if icon.is_some() { 6.0 } else { 0.0 };
    let text_color = if accent { Color32::WHITE } else { t.text };
    let text_galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(12.0),
        text_color,
    );
    let inner_pad_x = 12.0;
    let primary_w = inner_pad_x * 2.0 + icon_w + gap + text_galley.size().x;
    let caret_w = 22.0;
    let total = egui::vec2(primary_w + caret_w + 1.0, height);
    let (rect, _outer_resp) = ui.allocate_exact_size(total, Sense::hover());

    let bg = if accent { t.accent } else { t.panel2 };
    let stroke = Stroke::new(1.0, if accent { t.accent } else { t.border });
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(5), bg);
    painter.rect_stroke(rect, CornerRadius::same(5), stroke, StrokeKind::Middle);
    // Internal divider.
    let div_x = rect.left() + primary_w;
    painter.line_segment(
        [
            Pos2::new(div_x, rect.top() + 6.0),
            Pos2::new(div_x, rect.bottom() - 6.0),
        ],
        Stroke::new(
            1.0,
            if accent {
                Color32::from_rgba_premultiplied(0xFF, 0xFF, 0xFF, 0x40)
            } else {
                t.border
            },
        ),
    );

    // Primary interaction region.
    let primary_rect = Rect::from_min_size(rect.left_top(), egui::vec2(primary_w, height));
    let primary_resp = ui.interact(
        primary_rect,
        ui.id().with(("split-primary", label)),
        Sense::click(),
    );
    let mut cursor_x = primary_rect.left() + inner_pad_x;
    if let Some(icon) = icon {
        let icon_rect = Rect::from_min_size(
            Pos2::new(cursor_x, primary_rect.top() + (height - 13.0) * 0.5),
            egui::vec2(13.0, 13.0),
        );
        icons::paint_icon(ui.painter(), icon_rect, icon, text_color, 1.5);
        cursor_x += icon_w + gap;
    }
    let text_pos = Pos2::new(
        cursor_x,
        primary_rect.top() + (height - text_galley.size().y) * 0.5,
    );
    ui.painter().galley(text_pos, text_galley, text_color);

    // Caret region.
    let caret_rect = Rect::from_min_size(
        Pos2::new(div_x + 1.0, rect.top()),
        egui::vec2(caret_w - 1.0, height),
    );
    let caret_resp = ui.interact(
        caret_rect,
        ui.id().with(("split-caret", label)),
        Sense::click(),
    );
    let caret_icon_rect = Rect::from_center_size(caret_rect.center(), egui::vec2(11.0, 11.0));
    icons::paint_icon(
        ui.painter(),
        caret_icon_rect,
        Icon::ChevDown,
        text_color,
        1.5,
    );

    (primary_resp, caret_resp)
}

/// Small drag-value pill — flat appearance, fixed width, monospace
/// numeric. Wraps egui's [`egui::DragValue`] but constrains the visual.
#[allow(dead_code)]
pub fn drag_val_pill<Num: egui::emath::Numeric>(
    ui: &mut Ui,
    value: &mut Num,
    width: f32,
    suffix: &str,
) -> Response {
    ui.scope(|ui| {
        ui.set_width(width);
        let mut dv = egui::DragValue::new(value);
        if !suffix.is_empty() {
            dv = dv.suffix(suffix);
        }
        ui.add(dv)
    })
    .inner
}

/// D10 / Sprint 17 (ADR-041) — one entry the slot-picker grid
/// renders. The caller pre-resolves thumbnails (via the App's
/// `slot_thumbnail` cache) so the widget itself stays free of
/// `&mut App`.
pub struct SlotPickerEntry<'a> {
    pub id: u8,
    pub name: &'a str,
    pub thumbnail: Option<&'a egui::TextureHandle>,
}

/// D10 / Sprint 17 (ADR-041) — render a 3-column grid of slot
/// thumbnails. Each entry is a clickable 64 × 64 tile + a tiny
/// `"NN · name"` label. Returns `Some(slot_id)` the frame the user
/// clicks a tile; `None` otherwise.
///
/// Extracted from the legacy `inspector_splat`'s inline picker. Used
/// by Sprint 17's Layers panel for both the Add-layer flow (choose a
/// stock biome instead of "next-unused slot") and the active-layer
/// "Change slot…" affordance.
pub fn slot_picker_grid(ui: &mut Ui, entries: &[SlotPickerEntry<'_>]) -> Option<u8> {
    let t = Tokens::DARK;
    let mut clicked: Option<u8> = None;
    if entries.is_empty() {
        ui.label(
            egui::RichText::new(
                "No slots found under tools/textures/.\n\
                 Run scripts/fetch-textures.sh.",
            )
            .color(t.muted)
            .size(11.0),
        );
        return None;
    }
    egui::Grid::new(ui.id().with("slot_picker_grid"))
        .num_columns(3)
        .spacing([6.0, 6.0])
        .show(ui, |ui| {
            for (i, entry) in entries.iter().enumerate() {
                ui.vertical(|ui| {
                    let (thumb_rect, thumb_resp) =
                        ui.allocate_exact_size(egui::vec2(64.0, 64.0), Sense::click());
                    if let Some(handle) = entry.thumbnail {
                        egui::Image::new((handle.id(), thumb_rect.size()))
                            .corner_radius(4.0)
                            .paint_at(ui, thumb_rect);
                    } else {
                        ui.painter()
                            .rect_filled(thumb_rect, CornerRadius::same(4), t.bg);
                    }
                    ui.painter().rect_stroke(
                        thumb_rect,
                        CornerRadius::same(4),
                        Stroke::new(1.0, t.border),
                        StrokeKind::Middle,
                    );
                    ui.label(
                        egui::RichText::new(format!("{:02} · {}", entry.id, entry.name))
                            .color(t.muted)
                            .size(9.5),
                    );
                    if thumb_resp.clicked() {
                        clicked = Some(entry.id);
                    }
                });
                if (i + 1) % 3 == 0 {
                    ui.end_row();
                }
            }
        });
    clicked
}

/// Compact icon-only button with hover background. Returns the
/// underlying response so callers can wire `clicked()`.
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, tooltip: &str) -> Response {
    let t = Tokens::DARK;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, CornerRadius::same(3), t.hover);
    }
    let color = if response.hovered() { t.text } else { t.muted };
    let inset = (size - 14.0).max(0.0) * 0.5;
    let icon_rect = rect.shrink(inset);
    icons::paint_icon(painter, icon_rect, icon, color, 1.5);
    response.on_hover_text(tooltip)
}

/// Sprint 27 / U5 — descriptor for [`brush_card`]. The struct exists
/// so the call-site reads top-down (`label … active … hover_help`)
/// instead of guessing positional arguments.
///
/// `icon` is `Option<Icon>` for future Stage-2 polish (an icon centred
/// in the ring). For Sprint 27 callers pass `None`, preserving the
/// translucent-fill swatch the legacy `App::brush_card` produced.
pub struct BrushCard<'a> {
    pub label: &'a str,
    pub icon: Option<Icon>,
    pub ring_color: Color32,
    pub active: bool,
    pub hover_help: HelpId,
}

/// Sprint 27 / U5 — the brush-selector card lifted from
/// `App::brush_card`. Sculpt's `Off / Raise / Lower / Smooth` strip
/// and PaintLayer's `Reveal / Hide / Smooth / Fill` strip now share
/// this widget, so a change to the ring + label layout reaches both
/// tools.
///
/// Visual contract:
/// - Allocates the column's full width and 42 px tall.
/// - Active state lifts the background to `t.hover` and the border to
///   `t.border_hi`; inactive uses `t.bg` / `t.border`.
/// - Ring (~14 px diameter) sits 14 px down from the top, stroked in
///   `card.ring_color`. A translucent fill of the same tone fills the
///   ring when the label is anything other than "Off" — the "Off"
///   variant intentionally reads as empty.
/// - Optional icon (`card.icon`) paints inside the ring instead of
///   the translucent fill.
/// - Label sits at the bottom, in `t.text` when active and `t.muted`
///   otherwise.
///
/// Tooltip is sourced from `card.hover_help` so the help_text.rs
/// catalogue stays the single source of truth.
pub fn brush_card(ui: &mut Ui, card: BrushCard<'_>) -> Response {
    let t = Tokens::DARK;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 42.0), Sense::click());
    let painter = ui.painter();
    let bg = if card.active { t.hover } else { t.bg };
    let stroke = Stroke::new(1.0, if card.active { t.border_hi } else { t.border });
    painter.rect_filled(rect, CornerRadius::same(4), bg);
    painter.rect_stroke(rect, CornerRadius::same(4), stroke, StrokeKind::Middle);
    let cx = rect.center().x;
    let swatch_y = rect.top() + 14.0;
    let r = 7.0;
    let is_off = card.label == "Off";
    let fill = if is_off || card.icon.is_some() {
        Color32::TRANSPARENT
    } else {
        Color32::from_rgba_premultiplied(
            card.ring_color.r() / 5,
            card.ring_color.g() / 5,
            card.ring_color.b() / 5,
            80,
        )
    };
    painter.circle(
        Pos2::new(cx, swatch_y),
        r,
        fill,
        Stroke::new(1.5, card.ring_color),
    );
    if let Some(icon) = card.icon {
        let icon_rect = Rect::from_center_size(Pos2::new(cx, swatch_y), egui::vec2(10.0, 10.0));
        icons::paint_icon(painter, icon_rect, icon, card.ring_color, 1.5);
    }
    painter.text(
        Pos2::new(cx, rect.bottom() - 12.0),
        egui::Align2::CENTER_CENTER,
        card.label,
        egui::FontId::proportional(11.0),
        if card.active { t.text } else { t.muted },
    );
    response.on_hover_text(help(card.hover_help))
}

/// Sprint 27 / U5 — descriptor for the chips in [`sticky_chip_strip`].
/// `hover_help` is a [`HelpId`] so each chip's tooltip routes through
/// the central catalogue.
pub struct ChipDesc<'a> {
    pub tone: ChipTone,
    pub label: &'a str,
    pub hover_help: HelpId,
}

/// Sprint 27 / U5 — sticky symmetry + map-size chip strip rendered
/// at the top of every tool's Inspector body, between the persistent
/// header and the first section. Replaces the per-tool inline
/// `App::inspector_sticky_chips` so the band reads identically
/// across all 9 inspectors.
///
/// Layout: 14 px horizontal padding, 6 / 4 px top / bottom, with a
/// 1-pixel divider below the chips so the row reads as a band even
/// against the section that follows.
pub fn sticky_chip_strip(ui: &mut Ui, chips: &[ChipDesc<'_>]) {
    let t = Tokens::DARK;
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 14,
            right: 14,
            top: 6,
            bottom: 4,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for c in chips {
                    chip(ui, c.tone, c.label).on_hover_text(help(c.hover_help));
                }
            });
            ui.add_space(2.0);
            let avail = ui.available_width();
            let (rect, _) = ui.allocate_exact_size(egui::vec2(avail, 1.0), Sense::hover());
            ui.painter().rect_filled(rect, 0.0, t.border);
        });
}

#[cfg(test)]
mod tests {
    /// Pure helper: given a value+range, compute the normalised t in [0,1].
    /// Mirrors the math inside `ramp_slider` so we can unit-test it.
    fn ramp_t(value: f32, range: std::ops::RangeInclusive<f32>) -> f32 {
        let lo = *range.start();
        let hi = *range.end();
        let span = (hi - lo).max(f32::EPSILON);
        ((value - lo) / span).clamp(0.0, 1.0)
    }

    #[test]
    fn ramp_t_clamps_below_range() {
        assert_eq!(ramp_t(-5.0, 0.0..=10.0), 0.0);
    }

    #[test]
    fn ramp_t_clamps_above_range() {
        assert_eq!(ramp_t(99.0, 0.0..=10.0), 1.0);
    }

    #[test]
    fn ramp_t_midpoint() {
        assert!((ramp_t(5.0, 0.0..=10.0) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn ramp_t_handles_zero_range() {
        // Degenerate but should not divide-by-zero or produce NaN.
        let r = ramp_t(0.0, 5.0..=5.0);
        assert!(r.is_finite());
    }
}
