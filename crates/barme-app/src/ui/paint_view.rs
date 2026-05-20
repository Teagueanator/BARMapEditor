//! Top-down 2D paint viewport (D9 / Sprint 16, ADR-040).
//!
//! When `Tool::PaintLayer` is active the central viewport switches
//! from the 3D `TerrainCallback` perspective render to this 2D
//! orthographic view of the GPU composite RT — the live preview of
//! the layered diffuse. The user paints into the active layer's mask
//! by LMB-dragging; the per-frame `App::sync_composite_mask_tiles`
//! picks up the dirty tiles and pushes them to the composite mask
//! array so the next frame's RT reflects the new paint.
//!
//! ## Coordinate spaces
//!
//! - **Viewport pixels**: egui logical pixels relative to `rect`.
//! - **World elmos**: the map's elmo extent. 1 mask pixel = 1 elmo
//!   per the `MapSize::texture_dims == elmo_extents` identity.
//! - **RT pixels**: the composite RT's physical pixels. May be
//!   ≤ world elmos on >8-SMU maps where the RT clamps at 4096².
//!
//! The view computes an auto-fit zoom (`viewport_px / world_elmo`)
//! that letterboxes the empty bands when the viewport's aspect
//! differs from the map's. User scroll-wheel zoom multiplies the
//! auto-fit; user middle-drag pans in world-elmo space. Double-click
//! resets to auto-fit.

use eframe::egui;

/// Per-frame inputs to [`paint_view`]. Everything the renderer
/// needs to project the composite RT into the central viewport +
/// report back the cursor's world-space position.
pub struct PaintViewInput<'a> {
    /// Egui texture id of the composite RT. `None` when
    /// `ensure_composite_rt` hasn't allocated yet (first-frame race);
    /// in that case the viewport renders the background colour and
    /// returns `cursor_elmos = None`.
    pub composite_rt_id: Option<egui::TextureId>,
    /// World extent of the map (`MapSize::elmo_extents`). Drives the
    /// auto-fit zoom factor.
    pub world_extent_elmos: (f32, f32),
    /// Pan / zoom state — mutated in place by the viewport's input
    /// handling. Pan is in world elmos from the map's centre; zoom is
    /// `0.0` = auto-fit or `>0` = explicit screen-px-per-elmo.
    pub view_state: &'a mut crate::PaintViewState,
    /// Active brush radius in elmos. Drives the brush-ring overlay.
    pub brush_radius_elmos: f32,
    /// When set, render only the active layer's mask as grayscale
    /// (red overlay where mask = 0). Sprint 16 leaves this as a
    /// future enhancement — the basic toggle UI is in place but the
    /// per-frame mask preview rendering ships in Sprint 17 when the
    /// Layers panel adds the mask sidecar workflow.
    pub mask_only_preview: bool,
    /// Background colour for the letterbox bands.
    pub background: egui::Color32,
    /// Active layer's mask value at the cursor position, for the
    /// status strip (`None` when the cursor is off-map).
    pub mask_value_at_cursor: Option<u8>,
    /// Active layer's display name for the status strip.
    pub active_layer_name: Option<String>,
    /// Cursor position in world elmo coords (= mask pixel coords),
    /// computed by the caller via the same pan/zoom math the
    /// viewport applies. Drives the brush ring overlay + the status
    /// strip's cursor readout. `None` when the cursor isn't over the
    /// map.
    pub cursor_elmos: Option<glam::Vec2>,
}

/// Per-frame output of [`paint_view`].
pub struct PaintViewOutput {
    /// Egui's hit-test response for the central rect — the app uses
    /// `drag_started_by` / `dragged_by` / `drag_stopped_by` to drive
    /// the brush dispatch.
    pub response: egui::Response,
    /// User requested a mask-only-preview toggle from the in-view
    /// chip.
    pub toggled_mask_preview: bool,
}

/// Render the 2D paint viewport into `rect`. Returns hit-test
/// information for the app to drive brush dispatch.
///
/// Pan: middle-mouse-button drag.
/// Zoom: scroll wheel (pivot on cursor); range 0.25× – 16× of
/// auto-fit.
/// Double-click: reset to auto-fit + zero pan.
pub fn paint_view(ui: &mut egui::Ui, rect: egui::Rect, input: PaintViewInput<'_>) -> PaintViewOutput {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, input.background);

    // Allocate the rect for input handling.
    let response = ui.interact(
        rect,
        ui.id().with("paint_view"),
        egui::Sense::click_and_drag(),
    );

    let (extent_x, extent_z) = input.world_extent_elmos;
    let auto_fit = auto_fit_factor(rect.size(), extent_x, extent_z);
    let zoom = if input.view_state.zoom > 0.0 {
        input.view_state.zoom.clamp(auto_fit * 0.25, auto_fit * 16.0)
    } else {
        auto_fit
    };

    // Map the world rect (full extent in elmos) into screen-space
    // (px) using zoom + pan. The map's world centre lands at the
    // viewport centre + the pan offset (scaled to px).
    let map_screen_size = egui::vec2(extent_x * zoom, extent_z * zoom);
    let pan_screen = egui::vec2(
        input.view_state.pan_elmos.x * zoom,
        input.view_state.pan_elmos.y * zoom,
    );
    let map_centre_screen = rect.center() + pan_screen;
    let map_origin = map_centre_screen - map_screen_size * 0.5;
    let map_rect = egui::Rect::from_min_size(map_origin, map_screen_size);

    // Composite RT image.
    if let Some(tex_id) = input.composite_rt_id {
        painter.image(
            tex_id,
            map_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        // No composite RT yet — show a hint while ensure_composite_rt
        // catches up on the next frame.
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Allocating composite preview …",
            egui::FontId::proportional(14.0),
            ui.visuals().weak_text_color(),
        );
    }

    // Map outline so the user can see the world bounds even when the
    // composite is mid-grey at the borders.
    painter.rect_stroke(
        map_rect,
        0.0,
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.fg_stroke.color),
        egui::StrokeKind::Inside,
    );

    // Pan handler — middle-mouse-button drag.
    if response.dragged_by(egui::PointerButton::Middle) {
        let delta = ui.input(|i| i.pointer.delta());
        input.view_state.pan_elmos.x += delta.x / zoom;
        input.view_state.pan_elmos.y += delta.y / zoom;
    }

    // Zoom handler — scroll wheel pivoted on cursor.
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.5
            && let Some(cursor) = ui.ctx().pointer_interact_pos()
        {
            let factor = (1.0 + scroll * 0.002).clamp(0.5, 1.5);
            let new_zoom = (zoom * factor).clamp(auto_fit * 0.25, auto_fit * 16.0);
            // Pivot: keep the world point under the cursor stationary.
            let cursor_to_centre = cursor - map_centre_screen;
            let elmo_under_cursor =
                glam::Vec2::new(cursor_to_centre.x / zoom, cursor_to_centre.y / zoom);
            input.view_state.zoom = new_zoom;
            let new_cursor_to_centre = egui::vec2(
                elmo_under_cursor.x * new_zoom,
                elmo_under_cursor.y * new_zoom,
            );
            let new_pan_screen = (cursor - rect.center()) - new_cursor_to_centre;
            input.view_state.pan_elmos.x = new_pan_screen.x / new_zoom;
            input.view_state.pan_elmos.y = new_pan_screen.y / new_zoom;
        }
    }

    // Double-click resets to auto-fit + zero pan.
    if response.double_clicked() {
        input.view_state.zoom = 0.0;
        input.view_state.pan_elmos = glam::Vec2::ZERO;
    }

    // Brush ring overlay at the cursor — the caller already
    // computed `cursor_elmos` via the same math (it needs the value
    // ahead of this call to look up the mask sample for the status
    // strip, which would otherwise borrow-conflict with the
    // `view_state` mut here).
    let cursor_elmos = input.cursor_elmos;
    if let Some(elmos) = cursor_elmos {
        let cursor_screen = egui::pos2(
            map_origin.x + elmos.x * zoom,
            map_origin.y + elmos.y * zoom,
        );
        let radius_px = input.brush_radius_elmos * zoom;
        let accent = ui.visuals().selection.bg_fill;
        painter.circle_stroke(cursor_screen, radius_px, egui::Stroke::new(1.5, accent));
        // Inner pip — Sprint 9 brush-ring style.
        painter.circle_stroke(
            cursor_screen,
            (radius_px * 0.15).max(2.0),
            egui::Stroke::new(1.0, accent),
        );
    }

    // Mask-only preview chip in the top-right of the viewport.
    let mut toggled = false;
    {
        let chip_rect = egui::Rect::from_min_size(
            egui::pos2(rect.right() - 162.0, rect.top() + 8.0),
            egui::vec2(154.0, 22.0),
        );
        let chip_response =
            ui.interact(chip_rect, ui.id().with("paint_mask_only"), egui::Sense::click());
        let bg = if input.mask_only_preview {
            ui.visuals().selection.bg_fill
        } else {
            ui.visuals().widgets.inactive.bg_fill
        };
        painter.rect_filled(chip_rect, 4.0, bg);
        painter.text(
            chip_rect.center(),
            egui::Align2::CENTER_CENTER,
            if input.mask_only_preview {
                "Mask preview · ON"
            } else {
                "Mask preview · off"
            },
            egui::FontId::proportional(11.0),
            ui.visuals().text_color(),
        );
        if chip_response.clicked() {
            toggled = true;
        }
    }

    // Status strip at the bottom of the viewport: cursor coord +
    // active layer's mask value + active layer name.
    {
        let strip_rect = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 8.0, rect.bottom() - 22.0),
            egui::pos2(rect.right() - 8.0, rect.bottom() - 4.0),
        );
        let painter = ui.painter_at(strip_rect);
        let cursor_text = match cursor_elmos {
            Some(p) => format!("({}, {}) elmos", p.x.round() as i32, p.y.round() as i32),
            None => "(off-map)".to_string(),
        };
        let mask_text = match input.mask_value_at_cursor {
            Some(v) => format!("mask = {v}"),
            None => "—".to_string(),
        };
        let layer_text = input
            .active_layer_name
            .as_deref()
            .unwrap_or("no active layer");
        let line = format!("{layer_text}  ·  {cursor_text}  ·  {mask_text}");
        painter.text(
            strip_rect.left_center(),
            egui::Align2::LEFT_CENTER,
            line,
            egui::FontId::monospace(11.0),
            ui.visuals().weak_text_color(),
        );
    }

    PaintViewOutput {
        response,
        toggled_mask_preview: toggled,
    }
}

/// Compute the "fit map to viewport" zoom factor — the px-per-elmo
/// that makes the world extent fit inside `viewport_size` with 1:1
/// aspect. The shorter axis fully fills; the longer axis letterboxes.
fn auto_fit_factor(viewport_size: egui::Vec2, extent_x: f32, extent_z: f32) -> f32 {
    let pad_px = 32.0;
    let avail_x = (viewport_size.x - pad_px).max(1.0);
    let avail_z = (viewport_size.y - pad_px).max(1.0);
    let zx = avail_x / extent_x.max(1.0);
    let zz = avail_z / extent_z.max(1.0);
    zx.min(zz).max(1e-4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_fit_uses_shorter_axis() {
        // 800×600 viewport, 8192×8192 elmo map (16-SMU): both axes
        // would need ~0.094 px/elmo, but viewport is 800×600 → use
        // the smaller (600 - 32) / 8192 ≈ 0.069.
        let z = auto_fit_factor(egui::vec2(800.0, 600.0), 8192.0, 8192.0);
        let expected = (600.0 - 32.0) / 8192.0;
        assert!((z - expected).abs() < 1e-4, "got {z}");
    }

    #[test]
    fn auto_fit_for_wide_map_uses_width() {
        // 800×600 viewport, 8192×4096 elmo map (16×8 SMU). The longer
        // axis is X; viewport-X/world-X = (800-32)/8192 = 0.0938.
        // viewport-Y/world-Y = (600-32)/4096 = 0.1387. min = 0.0938.
        let z = auto_fit_factor(egui::vec2(800.0, 600.0), 8192.0, 4096.0);
        let expected = (800.0 - 32.0) / 8192.0;
        assert!((z - expected).abs() < 1e-4, "got {z}");
    }

    #[test]
    fn auto_fit_floors_at_epsilon() {
        // Pathological viewport (1 px on the short axis) still
        // returns a positive zoom rather than zero or NaN.
        let z = auto_fit_factor(egui::vec2(1.0, 1.0), 8192.0, 8192.0);
        assert!(z > 0.0);
    }
}
