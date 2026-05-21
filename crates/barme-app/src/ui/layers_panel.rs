//! D10 / Sprint 17 (ADR-041) — Photoshop-style Layers panel.
//!
//! Sprint 16 (ADR-040) shipped the minimal Layers panel inline in
//! `main.rs::inspector_paint_layer` (add / rename / delete / reorder
//! via ↑↓ arrows / opacity / visibility / texture import via picked
//! path). Sprint 17 lifts the layout into its own module and grows it
//! to ADR-041 spec:
//!
//! - Drag-to-reorder via egui's native `dnd_drag_source` /
//!   `dnd_drop_zone` (the up/down arrow buttons stay as a fallback
//!   for keyboard users).
//! - Per-layer thumbnail (32 × 32; slot-sourced layers reuse the
//!   slot-id thumb cache, imported layers cache by layer id).
//! - Lock chip exposing `layer.locked` (data already honoured by
//!   `LayerStack::apply_brush`).
//! - DNTS channel chip (R / G / B / A / ∅) with click-to-cycle + a
//!   right-click picker. At most one layer per channel; conflicts
//!   transfer the binding and surface a one-frame toast.
//! - Active-layer expanded properties: Source / Transform / Color /
//!   Blend / DNTS bindings. Edits flow through
//!   [`barme_core::ProjectDiff::SetLayerProperty`] for undo.
//! - Footer chips: live mask memory, "preview approximate" when the
//!   stack passes the 16-layer GPU cap, and a global toggle for
//!   `Project.dnts_diffuse_in_alpha`.
//!
//! The 64 MB diffuse re-upload (`App::reupload_layer_stack_diffuses`)
//! fires exactly once at drop time — never during the drag — by
//! routing the drop through [`crate::App::reorder_layer`] (which
//! already clears the per-layer mask-version cache + re-pushes the
//! diffuse slot array).

use eframe::egui::{self, CornerRadius, Sense, Stroke, StrokeKind};

use barme_core::undo::LayerPropertyValue;
use barme_core::{
    BlendMode, LayerColor, LayerSource, LayerTransform, ProjectDiff, SplatChannel, TextureLayer,
};

use crate::App;
use crate::ui::theme::{ChipTone, Tokens};
use crate::ui::widgets;

/// Render the Sprint 17 Layers panel. Replaces the inline body the
/// Sprint 16 panel used to live in. The caller (the
/// `Tool::PaintLayer` Inspector dispatch) supplies the `Ui`.
pub fn render(app: &mut App, ui: &mut egui::Ui) {
    // Stash the panel's screen rect so Sprint 17 / Commit 2's
    // drag-drop dispatch can route file drops to the Layers panel vs
    // the central viewport.
    let panel_top = ui.cursor().min;
    app.layers_panel_rect = None; // refreshed at end of render

    // Snapshot the per-row data + active-layer data up front so the
    // borrow checker can split `&mut app` from the closure capture.
    let snapshots: Vec<RowSnapshot> = build_row_snapshots(app);
    let active_snapshot = app
        .paint_active_layer_id
        .clone()
        .and_then(|id| snapshots.iter().find(|r| r.id == id).cloned());
    let layer_count = app.layer_stack.layers.len();
    let mask_mb = app.layer_stack.resident_mask_bytes() / 1_048_576;
    let active_layer_id = app.paint_active_layer_id.clone();
    let has_active = active_layer_id.is_some();
    let diffuse_in_alpha = app.dnts_diffuse_in_alpha;

    // Pre-resolve thumbnail handles per snapshot to avoid threading
    // `&mut App` into the row render closure.
    let ctx = ui.ctx().clone();
    let thumb_handles: std::collections::HashMap<String, egui::TextureHandle> = snapshots
        .iter()
        .filter_map(|s| app.layer_thumbnail(&ctx, &s.id).map(|h| (s.id.clone(), h)))
        .collect();

    // D10 / Sprint 17 (ADR-041) — also pre-resolve stock-slot
    // thumbnails for the Add-layer picker popup + the "Change slot…"
    // popup. The grid widget itself stays free of `&mut App`.
    //
    // Hotfix follow-up: per-frame cost is 16 cache lookups (one HashMap
    // get per slot) after the first frame warms the cache. The first
    // frame pays 16 PNG decodes serially — each decode peaks at ~8 MB
    // transient (1024² × RGBA) but frees before the next slot starts,
    // so sequential peak ≈ one PNG. If the warm-up correlates with
    // OOM pressure on the user's 16-SMU project, the workaround is
    // to scan a smaller texture pack or skip the picker (the "Add
    // empty layer (any unused slot)" fallback in the same popup).
    let slot_registry_view: Vec<(u8, String)> = app
        .slot_registry
        .iter()
        .map(|s| (s.id, s.name.clone()))
        .collect();
    let slot_thumbs: std::collections::HashMap<u8, egui::TextureHandle> = slot_registry_view
        .iter()
        .filter_map(|(id, _)| app.slot_thumbnail(&ctx, *id).map(|h| (*id, h)))
        .collect();
    let slot_picker_entries: Vec<widgets::SlotPickerEntry<'_>> = slot_registry_view
        .iter()
        .map(|(id, name)| widgets::SlotPickerEntry {
            id: *id,
            name: name.as_str(),
            thumbnail: slot_thumbs.get(id),
        })
        .collect();

    // Deferred actions: the UI walks the layer list with `&self`
    // references; mutations get queued and applied after.
    let actions: std::cell::RefCell<Vec<LayerAction>> = std::cell::RefCell::new(Vec::new());
    // Track drag-preview reorder state inside the panel render —
    // applied after.
    let pending_preview_order: std::cell::RefCell<Option<Vec<String>>> =
        std::cell::RefCell::new(None);

    widgets::section(
        ui,
        "Layers",
        true,
        |ui| {
            widgets::chip(
                ui,
                ChipTone::Neutral,
                format!(
                    "{layer_count} layer{}",
                    if layer_count == 1 { "" } else { "s" }
                ),
            );
        },
        |ui| {
            // D10 / Sprint 17 (ADR-041) — Add-layer flow now opens a
            // slot-picker popup on PRIMARY click so the user picks a
            // stock biome instead of "next unused slot" (the
            // Sprint-16 behaviour). The caret keeps the secondary
            // affordances (Import / Duplicate / pick-anything-empty).
            ui.horizontal(|ui| {
                let (primary, caret) = widgets::split_button(ui, None, "Add layer", true);
                let primary = primary
                    .on_hover_text("Open the stock-texture picker. Click a slot to add it as a new layer.");
                let caret = caret.on_hover_text("Secondary add affordances: import a custom texture, duplicate the active layer, or add from a heightmap range.");
                egui::Popup::menu(&primary)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                    .show(|ui| {
                        ui.set_min_width(260.0);
                        ui.label(
                            egui::RichText::new("Pick a stock texture")
                                .color(Tokens::DARK.muted)
                                .size(11.0)
                                .strong(),
                        );
                        ui.add_space(4.0);
                        if let Some(slot_id) = widgets::slot_picker_grid(ui, &slot_picker_entries) {
                            actions
                                .borrow_mut()
                                .push(LayerAction::AddLayerFromSlot(slot_id));
                        }
                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(4.0);
                        if ui
                            .button("Import texture from disk…")
                            .on_hover_text("Load a PNG / JPG / DDS as a new layer. The layer source is the file path (preserved across save/open).")
                            .clicked()
                        {
                            actions.borrow_mut().push(LayerAction::AddLayerFromImport);
                        }
                        if ui
                            .button("Add empty layer (any unused slot)")
                            .on_hover_text("Add a layer bound to the next-unused stock slot. Useful when you'll set the source later via 'Change slot…'.")
                            .clicked()
                        {
                            actions.borrow_mut().push(LayerAction::AddLayer);
                        }
                    });
                egui::Popup::menu(&caret)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                    .show(|ui| {
                        ui.set_min_width(220.0);
                        if ui
                            .button("Import texture from disk…")
                            .on_hover_text("Same as the primary popup's import — quick access from the caret.")
                            .clicked()
                        {
                            actions.borrow_mut().push(LayerAction::AddLayerFromImport);
                        }
                        if ui
                            .add_enabled(has_active, egui::Button::new("Duplicate active"))
                            .on_hover_text("Clone the active layer's source, mask, transform, and colour into a new top-of-stack layer.")
                            .clicked()
                        {
                            actions.borrow_mut().push(LayerAction::DuplicateActive);
                        }
                        ui.separator();
                        ui.add_enabled(false, egui::Button::new("Add from heightmap range…"))
                            .on_disabled_hover_text("Coming in a future sprint — auto-mask a layer by elevation band (e.g. 'snow above 3000 elmos').");
                    });
            });
            ui.add_space(6.0);

            if snapshots.is_empty() {
                ui.label(
                    egui::RichText::new("Empty stack — click 'Add layer' to start.")
                        .small()
                        .weak(),
                );
                // Sprint 22 / U2 — link to the help-center article
                // so users new to the layered painter have a one-
                // click path to the data-model + bake explanation.
                if ui
                    .small_button("How layers work")
                    .on_hover_text(
                        "Open the Layered painter reference article in the help center.",
                    )
                    .clicked()
                {
                    actions.borrow_mut().push(LayerAction::OpenHelpHowLayersWork);
                }
                return;
            }

            // Display order: snapshots already arrive top-of-stack
            // first (display order). Honour an in-flight drag preview.
            let display_ids: Vec<String> = snapshots.iter().map(|s| s.id.clone()).collect();
            for (display_idx, snap) in snapshots.iter().enumerate() {
                let is_active = active_layer_id.as_deref() == Some(snap.id.as_str());
                let thumb = thumb_handles.get(&snap.id);
                render_layer_row(
                    ui,
                    snap,
                    display_idx,
                    &display_ids,
                    is_active,
                    thumb,
                    &actions,
                    &pending_preview_order,
                );
                ui.add_space(3.0);
            }
        },
    );

    // ── Active layer expanded properties ─────────────────────────
    if let Some(snap) = active_snapshot.as_ref() {
        render_active_properties(ui, snap, &slot_picker_entries, &actions);
    }

    // ── Footer chips ─────────────────────────────────────────────
    render_footer(ui, layer_count, mask_mb, diffuse_in_alpha, &actions);

    let panel_bottom = ui.cursor().min;
    let panel_rect =
        egui::Rect::from_min_max(panel_top, egui::pos2(ui.max_rect().right(), panel_bottom.y));
    app.layers_panel_rect = Some(panel_rect);

    // Commit any drag-preview order change.
    if let Some(order) = pending_preview_order.into_inner() {
        app.paint_drag_preview_order = Some(order);
    }

    // Apply collected actions.
    apply_actions(app, actions.into_inner());
}

/// Per-layer UI-only data: enough to render a row + the expanded
/// properties without borrowing the full [`TextureLayer`] (which
/// owns a potentially-heavy [`LayerMask`]).
#[derive(Clone)]
struct RowSnapshot {
    id: String,
    name: String,
    source: LayerSource,
    visible: bool,
    locked: bool,
    opacity: f32,
    dnts_channel: Option<SplatChannel>,
    transform: LayerTransform,
    color: LayerColor,
    blend: BlendMode,
    dnts_tex_scale: f32,
    dnts_tex_mult: f32,
}

fn build_row_snapshots(app: &App) -> Vec<RowSnapshot> {
    let preview_order = display_order(app);
    let preview_to_layer: std::collections::HashMap<&String, &TextureLayer> =
        app.layer_stack.layers.iter().map(|l| (&l.id, l)).collect();
    preview_order
        .iter()
        .filter_map(|id| preview_to_layer.get(id).map(|l| snapshot_from(l)))
        .collect()
}

fn snapshot_from(l: &TextureLayer) -> RowSnapshot {
    RowSnapshot {
        id: l.id.clone(),
        name: l.name.clone(),
        source: l.source.clone(),
        visible: l.visible,
        locked: l.locked,
        opacity: l.opacity,
        dnts_channel: l.dnts_channel,
        transform: l.transform,
        color: l.color,
        blend: l.blend,
        dnts_tex_scale: l.dnts_tex_scale,
        dnts_tex_mult: l.dnts_tex_mult,
    }
}

// ─────────────────────────────────────────────────────────────────
// Deferred-action enum + dispatch
// ─────────────────────────────────────────────────────────────────

enum LayerAction {
    SetActive(String),
    ToggleVisible(String),
    ToggleLocked(String),
    Rename(String, String),
    Delete(String),
    Reorder {
        from: usize,
        to: usize,
    },
    SetOpacity(String, f32),
    SetDntsChannel {
        layer_id: String,
        new_channel: Option<SplatChannel>,
    },
    SetDntsTexScale(String, f32),
    SetDntsTexMult(String, f32),
    SetTransform(String, LayerTransform),
    SetColor(String, LayerColor),
    SetBlend(String, BlendMode),
    ImportTexture(String),
    AddLayer,
    /// D10 / Sprint 17 (ADR-041) — explicit-slot variant. Sprint 16's
    /// "Add layer" picked an arbitrary unused stock slot; Sprint 17's
    /// slot-picker popup feeds this with the user's chosen id.
    AddLayerFromSlot(u8),
    /// D10 / Sprint 17 (ADR-041) — re-bind the active layer's `Slot`
    /// source to a different slot id.
    ChangeSlot {
        layer_id: String,
        new_slot_id: u8,
    },
    AddLayerFromImport,
    DuplicateActive,
    SetDntsDiffuseInAlpha(bool),
    /// Sprint 22 / U2 — empty-state help link. Opens the help
    /// center on the layered-painter reference article.
    OpenHelpHowLayersWork,
}

fn apply_actions(app: &mut App, actions: Vec<LayerAction>) {
    for action in actions {
        match action {
            LayerAction::SetActive(id) => app.paint_active_layer_id = Some(id),
            LayerAction::ToggleVisible(id) => {
                let Some(prev) = app.layer_stack.layer_by_id(&id).map(|l| l.visible) else {
                    continue;
                };
                push_layer_property(
                    app,
                    &id,
                    LayerPropertyValue::Visible(prev),
                    LayerPropertyValue::Visible(!prev),
                );
            }
            LayerAction::ToggleLocked(id) => {
                let Some(prev) = app.layer_stack.layer_by_id(&id).map(|l| l.locked) else {
                    continue;
                };
                push_layer_property(
                    app,
                    &id,
                    LayerPropertyValue::Locked(prev),
                    LayerPropertyValue::Locked(!prev),
                );
            }
            LayerAction::Rename(id, new_name) => {
                let Some(prev) = app.layer_stack.layer_by_id(&id).map(|l| l.name.clone()) else {
                    continue;
                };
                if prev == new_name {
                    continue;
                }
                push_layer_property(
                    app,
                    &id,
                    LayerPropertyValue::Name(prev),
                    LayerPropertyValue::Name(new_name),
                );
            }
            LayerAction::Delete(id) => {
                app.delete_layer(&id);
            }
            LayerAction::Reorder { from, to } => {
                if from != to {
                    app.history
                        .push_project_diff(ProjectDiff::ReorderLayer { from, to });
                    app.reorder_layer(from, to);
                }
            }
            LayerAction::SetOpacity(id, v) => {
                let Some(prev) = app.layer_stack.layer_by_id(&id).map(|l| l.opacity) else {
                    continue;
                };
                if (prev - v).abs() < 1e-4 {
                    continue;
                }
                push_layer_property(
                    app,
                    &id,
                    LayerPropertyValue::Opacity(prev),
                    LayerPropertyValue::Opacity(v),
                );
            }
            LayerAction::SetDntsChannel {
                layer_id,
                new_channel,
            } => {
                rebind_dnts_channel(app, &layer_id, new_channel);
            }
            LayerAction::SetDntsTexScale(id, v) => {
                let Some(prev) = app.layer_stack.layer_by_id(&id).map(|l| l.dnts_tex_scale) else {
                    continue;
                };
                if (prev - v).abs() < 1e-6 {
                    continue;
                }
                push_layer_property(
                    app,
                    &id,
                    LayerPropertyValue::DntsTexScale(prev),
                    LayerPropertyValue::DntsTexScale(v),
                );
            }
            LayerAction::SetDntsTexMult(id, v) => {
                let Some(prev) = app.layer_stack.layer_by_id(&id).map(|l| l.dnts_tex_mult) else {
                    continue;
                };
                if (prev - v).abs() < 1e-6 {
                    continue;
                }
                push_layer_property(
                    app,
                    &id,
                    LayerPropertyValue::DntsTexMult(prev),
                    LayerPropertyValue::DntsTexMult(v),
                );
            }
            LayerAction::SetTransform(id, new_t) => {
                let Some(prev) = app.layer_stack.layer_by_id(&id).map(|l| l.transform) else {
                    continue;
                };
                if prev == new_t {
                    continue;
                }
                push_layer_property(
                    app,
                    &id,
                    LayerPropertyValue::Transform(prev),
                    LayerPropertyValue::Transform(new_t),
                );
            }
            LayerAction::SetColor(id, new_c) => {
                let Some(prev) = app.layer_stack.layer_by_id(&id).map(|l| l.color) else {
                    continue;
                };
                if prev == new_c {
                    continue;
                }
                push_layer_property(
                    app,
                    &id,
                    LayerPropertyValue::Color(prev),
                    LayerPropertyValue::Color(new_c),
                );
            }
            LayerAction::SetBlend(id, new_b) => {
                let Some(prev) = app.layer_stack.layer_by_id(&id).map(|l| l.blend) else {
                    continue;
                };
                if prev == new_b {
                    continue;
                }
                push_layer_property(
                    app,
                    &id,
                    LayerPropertyValue::Blend(prev),
                    LayerPropertyValue::Blend(new_b),
                );
            }
            LayerAction::ImportTexture(id) => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Texture image", &["png", "jpg", "jpeg"])
                    .pick_file()
                {
                    app.import_layer_texture(&id, path);
                }
            }
            LayerAction::AddLayerFromSlot(slot_id) => {
                let id = app.add_layer_with_slot(slot_id);
                app.paint_active_layer_id = Some(id);
            }
            LayerAction::ChangeSlot {
                layer_id,
                new_slot_id,
            } => {
                let Some(prev) = app
                    .layer_stack
                    .layer_by_id(&layer_id)
                    .map(|l| l.source.clone())
                else {
                    continue;
                };
                let new_source = LayerSource::Slot { id: new_slot_id };
                if prev == new_source {
                    continue;
                }
                push_layer_property(
                    app,
                    &layer_id,
                    LayerPropertyValue::Source(prev),
                    LayerPropertyValue::Source(new_source),
                );
                app.layer_thumbnails.remove(&layer_id);
                app.composite_layer_last_version.clear();
                app.reupload_layer_stack_diffuses();
            }
            LayerAction::AddLayerFromImport => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Texture image", &["png", "jpg", "jpeg"])
                    .pick_file()
                {
                    let id = app.add_layer_at_top();
                    app.import_layer_texture(&id, path);
                    app.paint_active_layer_id = Some(id);
                }
            }
            LayerAction::AddLayer => {
                // Legacy Sprint-16 path — pick the next unused stock
                // slot. Surfaced behind the dropdown's "Add empty
                // layer (any unused slot)" entry; the primary
                // affordance now opens the slot picker.
                let id = app.add_layer_at_top();
                app.paint_active_layer_id = Some(id);
            }
            LayerAction::DuplicateActive => {
                if let Some(active_id) = app.paint_active_layer_id.clone() {
                    duplicate_layer(app, &active_id);
                }
            }
            LayerAction::SetDntsDiffuseInAlpha(v) => {
                if app.dnts_diffuse_in_alpha != v {
                    app.dnts_diffuse_in_alpha = v;
                    app.mark_dirty();
                }
            }
            LayerAction::OpenHelpHowLayersWork => {
                app.help_center
                    .open_at(crate::ui::help_center::HelpArticleId::LayeredPainter);
            }
        }
    }
}

/// Apply `to` to the named layer and push a paired `SetLayerProperty`
/// diff so undo / redo can step through the change.
fn push_layer_property(
    app: &mut App,
    layer_id: &str,
    from: LayerPropertyValue,
    to: LayerPropertyValue,
) {
    if let Some(layer) = app.layer_stack.active_layer_mut(layer_id) {
        crate::apply_layer_property(layer, &to);
        app.mark_dirty();
        app.history
            .push_project_diff(ProjectDiff::SetLayerProperty {
                layer_id: layer_id.to_string(),
                from,
                to,
            });
    }
}

/// Bind `new_channel` to `layer_id`. If another layer is already
/// bound to `new_channel`, that layer's binding is transferred to
/// `None` (one-frame toast surfaces the swap). Emits up to two
/// `SetLayerProperty` diffs so undo restores the prior state.
fn rebind_dnts_channel(app: &mut App, layer_id: &str, new_channel: Option<SplatChannel>) {
    let Some(prev) = app
        .layer_stack
        .layer_by_id(layer_id)
        .map(|l| l.dnts_channel)
    else {
        return;
    };
    if prev == new_channel {
        return;
    }
    // If new_channel is Some and another layer already owns it, clear
    // that layer first.
    if let Some(ch) = new_channel
        && let Some(other_id) = app
            .layer_stack
            .layers
            .iter()
            .find(|l| l.id != layer_id && l.dnts_channel == Some(ch))
            .map(|l| l.id.clone())
    {
        push_layer_property(
            app,
            &other_id,
            LayerPropertyValue::DntsChannel(Some(ch)),
            LayerPropertyValue::DntsChannel(None),
        );
    }
    push_layer_property(
        app,
        layer_id,
        LayerPropertyValue::DntsChannel(prev),
        LayerPropertyValue::DntsChannel(new_channel),
    );
}

/// Sprint 17 (ADR-041) — "Duplicate active". Inserts a fresh layer at
/// the top of the stack with the same source / transform / color /
/// blend / opacity / DNTS binding as `source_id`; the mask is reset to
/// `0` (the user duplicates a layer to repaint, not to clone pixels).
/// A future sprint may add a "Duplicate with mask" affordance.
fn duplicate_layer(app: &mut App, source_id: &str) {
    let Some(source) = app.layer_stack.layer_by_id(source_id).cloned() else {
        return;
    };
    let mut copy = TextureLayer::new(source.source.clone(), app.map_size, 0);
    copy.name = format!("{} (copy)", source.name);
    copy.transform = source.transform;
    copy.color = source.color;
    copy.blend = source.blend;
    copy.opacity = source.opacity;
    copy.dnts_tex_scale = source.dnts_tex_scale;
    copy.dnts_tex_mult = source.dnts_tex_mult;
    // Do NOT carry over the channel binding — that would silently
    // collide with the source's binding (which the UI then reassigns).
    let new_idx = app.layer_stack.layers.len();
    let new_id = copy.id.clone();
    app.layer_stack.layers.push(copy.clone());
    app.history.push_project_diff(ProjectDiff::AddLayer {
        index: new_idx,
        layer: Box::new(copy),
    });
    app.mark_dirty();
    app.composite_layer_last_version.clear();
    app.reupload_layer_stack_diffuses();
    app.paint_active_layer_id = Some(new_id);
}

// ─────────────────────────────────────────────────────────────────
// Per-row rendering
// ─────────────────────────────────────────────────────────────────

fn display_order(app: &App) -> Vec<String> {
    if let Some(preview) = app.paint_drag_preview_order.as_ref() {
        // Filter out any stale ids (a layer deleted mid-drag, defensive).
        let valid: Vec<String> = preview
            .iter()
            .filter(|id| app.layer_stack.layers.iter().any(|l| &l.id == *id))
            .cloned()
            .collect();
        if valid.len() == app.layer_stack.layers.len() {
            return valid;
        }
    }
    app.layer_stack
        .layers
        .iter()
        .rev()
        .map(|l| l.id.clone())
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn render_layer_row(
    ui: &mut egui::Ui,
    snap: &RowSnapshot,
    display_idx: usize,
    display_ids: &[String],
    is_active: bool,
    thumb: Option<&egui::TextureHandle>,
    actions: &std::cell::RefCell<Vec<LayerAction>>,
    pending_preview_order: &std::cell::RefCell<Option<Vec<String>>>,
) {
    let t = Tokens::DARK;
    let layer_id = snap.id.clone();
    let visible = snap.visible;
    let locked = snap.locked;
    let opacity = snap.opacity;
    let dnts_channel = snap.dnts_channel;
    let name = snap.name.clone();
    let source_label = snap.source.default_label();

    // D10 / Sprint 17 (ADR-041) — active-layer treatment: accent_dim
    // background + a 3-px accent rail along the left edge so the
    // selected row reads at a glance.
    let bg = if is_active { t.accent_dim } else { t.panel2 };
    let stroke = if is_active {
        egui::Stroke::new(1.5, t.accent)
    } else {
        egui::Stroke::new(1.0, t.border)
    };
    let frame = egui::Frame::group(ui.style())
        .fill(bg)
        .stroke(stroke)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(6, 4));

    let payload = layer_id.clone();
    let dnd_id = egui::Id::new(("layer_dnd", &layer_id));

    // D10 / Sprint 17 hotfix — drag-source is JUST the handle, not
    // the whole row. Sprint-17 v1 wrapped the entire row body in
    // `dnd_drag_source` which made every inner button start a drag
    // instead of firing its click. The drop_zone still scopes the
    // whole row so dropping anywhere on a target row reorders.
    let drop_response = ui.dnd_drop_zone::<String, _>(frame, |ui| {
        // Active-layer left-rail accent (paint manually since
        // egui::Frame doesn't surface a per-side stroke).
        if is_active {
            let painter = ui.painter();
            let rect = ui.max_rect();
            let rail = egui::Rect::from_min_size(
                egui::pos2(rect.left() - 4.0, rect.top() + 2.0),
                egui::vec2(3.0, rect.height() - 4.0),
            );
            painter.rect_filled(rail, CornerRadius::same(1), t.accent);
        }
        ui.horizontal(|ui| {
            // ── Drag handle (6 dots) — ONLY this initiates a drag.
            ui.dnd_drag_source(dnd_id, payload.clone(), |ui| {
                let (handle_rect, _) =
                    ui.allocate_exact_size(egui::vec2(10.0, 18.0), Sense::hover());
                paint_drag_handle(ui, handle_rect, t.muted);
            })
            .response
            .on_hover_text("Drag to reorder");

            // ── Eye visibility ──
            let eye = if visible { "👁" } else { "—" };
            if ui
                .add(egui::Button::new(eye).small())
                .on_hover_text("Toggle visibility")
                .clicked()
            {
                actions
                    .borrow_mut()
                    .push(LayerAction::ToggleVisible(layer_id.clone()));
            }

            // ── Lock chip ──
            let lock_label = if locked { "🔒" } else { "🔓" };
            let lock_color = if locked { t.amber } else { t.muted };
            let lock_resp = ui
                .add(egui::Button::new(egui::RichText::new(lock_label).color(lock_color)).small());
            if lock_resp
                .on_hover_text("Lock layer (mask brushes ignore locked layers)")
                .clicked()
            {
                actions
                    .borrow_mut()
                    .push(LayerAction::ToggleLocked(layer_id.clone()));
            }

            // ── Thumbnail ──
            paint_thumbnail(ui, thumb);

            // ── Name (inline rename) ──
            let mut name_mut = name.clone();
            let name_resp = ui
                .add(
                    egui::TextEdit::singleline(&mut name_mut)
                        .desired_width(ui.available_width() - 96.0)
                        .frame(false),
                )
                .on_hover_text("Click to make active. Type to rename — the edit commits on focus loss.");
            if name_resp.lost_focus() && name_mut != name {
                actions
                    .borrow_mut()
                    .push(LayerAction::Rename(layer_id.clone(), name_mut));
            }
            if name_resp.clicked() {
                actions
                    .borrow_mut()
                    .push(LayerAction::SetActive(layer_id.clone()));
            }

            // ── DNTS chip ──
            let chip_label = match dnts_channel {
                Some(SplatChannel::R) => "R",
                Some(SplatChannel::G) => "G",
                Some(SplatChannel::B) => "B",
                Some(SplatChannel::A) => "A",
                None => "∅",
            };
            let chip_color = match dnts_channel {
                Some(SplatChannel::R) => t.red,
                Some(SplatChannel::G) => t.green,
                Some(SplatChannel::B) => t.accent,
                Some(SplatChannel::A) => t.text,
                None => t.muted,
            };
            let chip_resp = ui.add(
                egui::Button::new(
                    egui::RichText::new(chip_label)
                        .color(chip_color)
                        .monospace()
                        .strong(),
                )
                .small(),
            );
            let chip_resp = chip_resp.on_hover_text(
                "DNTS channel — click to cycle R→G→B→A→∅\n\
                     Right-click for picker (at most one layer per channel).",
            );
            if chip_resp.clicked() {
                let next = cycle_channel(dnts_channel);
                actions.borrow_mut().push(LayerAction::SetDntsChannel {
                    layer_id: layer_id.clone(),
                    new_channel: next,
                });
            }
            egui::Popup::context_menu(&chip_resp)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                .show(|ui| {
                    ui.set_min_width(140.0);
                    for ch in [
                        None,
                        Some(SplatChannel::R),
                        Some(SplatChannel::G),
                        Some(SplatChannel::B),
                        Some(SplatChannel::A),
                    ] {
                        let lbl = match ch {
                            None => "None".to_string(),
                            Some(SplatChannel::R) => "R (red)".to_string(),
                            Some(SplatChannel::G) => "G (green)".to_string(),
                            Some(SplatChannel::B) => "B (blue)".to_string(),
                            Some(SplatChannel::A) => "A (alpha)".to_string(),
                        };
                        if ui
                            .add(egui::Button::selectable(dnts_channel == ch, lbl))
                            .clicked()
                        {
                            actions.borrow_mut().push(LayerAction::SetDntsChannel {
                                layer_id: layer_id.clone(),
                                new_channel: ch,
                            });
                        }
                    }
                });

            // ── Delete ──
            if ui
                .add(egui::Button::new("×").small())
                .on_hover_text("Delete layer")
                .clicked()
            {
                actions
                    .borrow_mut()
                    .push(LayerAction::Delete(layer_id.clone()));
            }
        });
        // Second row: source label + opacity slider + import.
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(&source_label)
                        .color(t.muted)
                        .size(10.0)
                        .monospace(),
                )
                .sense(Sense::hover()),
            )
            .on_hover_text("Layer source — either a stock slot id or an imported texture path. Click 'Change slot…' in the active-layer properties to rebind.");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new("Import…").small())
                    .on_hover_text("Replace this layer's source with a PNG/JPG from disk")
                    .clicked()
                {
                    actions
                        .borrow_mut()
                        .push(LayerAction::ImportTexture(layer_id.clone()));
                }
                let mut op = opacity;
                if ui
                    .add(egui::Slider::new(&mut op, 0.0..=1.0).show_value(false))
                    .on_hover_text(format!("Opacity: {:.0}%", op * 100.0))
                    .changed()
                {
                    actions
                        .borrow_mut()
                        .push(LayerAction::SetOpacity(layer_id.clone(), op));
                }
            });
        });

        // Click on the card surface (anywhere not yet
        // interactive) makes the layer active.
        let body_resp = ui.interact(
            ui.min_rect(),
            ui.id().with(("layer_card_body", &layer_id)),
            Sense::click(),
        );
        if body_resp.clicked() {
            actions
                .borrow_mut()
                .push(LayerAction::SetActive(layer_id.clone()));
        }
    });

    // Drop happened ON this row — insert above it (Photoshop
    // convention: dropping a layer onto another puts it above).
    // egui 0.33's `dnd_drop_zone` returns `(InnerResponse<R>,
    // Option<Arc<Payload>>)`; `.1` is the payload (Some only on
    // the frame the drop actually fires).
    let payload_arc = drop_response.1.clone();
    if let Some(payload_id) = payload_arc.map(|arc| (*arc).clone())
        && let Some(src_display_pos) = display_ids.iter().position(|id| id == &payload_id)
        && src_display_pos != display_idx
    {
        // display order is top-of-stack first; vec is
        // bottom-first. Convert to vec indices for the
        // ProjectDiff::ReorderLayer.
        let n = display_ids.len();
        let from = n - 1 - src_display_pos;
        let to = n - 1 - display_idx;
        actions.borrow_mut().push(LayerAction::Reorder { from, to });
        // Clear the drag preview so the canonical order
        // renders on the next frame.
        *pending_preview_order.borrow_mut() = None;
    }
    let _ = payload; // payload moved into dnd_drag_source above
}

fn paint_drag_handle(ui: &mut egui::Ui, rect: egui::Rect, color: egui::Color32) {
    let painter = ui.painter_at(rect);
    let cx = rect.center().x;
    let dots_y = [rect.top() + 4.0, rect.top() + 8.0, rect.top() + 12.0];
    let cols_x = [cx - 1.5, cx + 1.5];
    for &y in &dots_y {
        for &x in &cols_x {
            painter.circle_filled(egui::pos2(x, y), 1.0, color);
        }
    }
}

fn paint_thumbnail(ui: &mut egui::Ui, handle: Option<&egui::TextureHandle>) {
    let t = Tokens::DARK;
    let size = egui::vec2(32.0, 32.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    if let Some(h) = handle {
        egui::Image::new((h.id(), rect.size()))
            .corner_radius(3.0)
            .paint_at(ui, rect);
    } else {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(3), t.panel);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "?",
            egui::FontId::monospace(11.0),
            t.dim,
        );
    }
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(3),
        Stroke::new(1.0, t.border),
        StrokeKind::Middle,
    );
}

fn cycle_channel(cur: Option<SplatChannel>) -> Option<SplatChannel> {
    match cur {
        None => Some(SplatChannel::R),
        Some(SplatChannel::R) => Some(SplatChannel::G),
        Some(SplatChannel::G) => Some(SplatChannel::B),
        Some(SplatChannel::B) => Some(SplatChannel::A),
        Some(SplatChannel::A) => None,
    }
}

// ─────────────────────────────────────────────────────────────────
// Active layer expanded properties
// ─────────────────────────────────────────────────────────────────

fn render_active_properties(
    ui: &mut egui::Ui,
    snap: &RowSnapshot,
    slot_picker_entries: &[widgets::SlotPickerEntry<'_>],
    actions: &std::cell::RefCell<Vec<LayerAction>>,
) {
    let t = Tokens::DARK;
    let layer_id = snap.id.clone();
    // Extent isn't exposed on the snapshot; default to a generous
    // ±8192 elmos (16-SMU map ceiling), which the offset sliders
    // can clamp themselves. Tighter per-map bounds is a Sprint 18+
    // polish item.
    let extent_x = 8192.0_f32;

    egui::CollapsingHeader::new(
        egui::RichText::new(format!("'{}' — properties", snap.name))
            .color(t.text)
            .size(11.5)
            .strong(),
    )
    .id_salt(egui::Id::new(("layer_props_section", &layer_id)))
    .default_open(true)
    .show(ui, |ui| {
        // ── SOURCE ──
        widgets::section(
            ui,
            "Source",
            false,
            |_ui| {},
            |ui| match &snap.source {
                LayerSource::Slot { id } => {
                    ui.horizontal(|ui| {
                        let slot_name = slot_picker_entries
                            .iter()
                            .find(|e| e.id == *id)
                            .map(|e| e.name.to_string())
                            .unwrap_or_default();
                        let label = if slot_name.is_empty() {
                            format!("Slot {id:02}")
                        } else {
                            format!("Slot {id:02} · {slot_name}")
                        };
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(label).color(t.muted).monospace(),
                            )
                            .sense(egui::Sense::hover()),
                        )
                        .on_hover_text("Stock slot id (00..15) and friendly name. The slot drives the diffuse + DNTS textures BAR loads at runtime.");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let change_resp = ui
                                .button("Change slot…")
                                .on_hover_text("Rebind this layer to a different stock slot. The mask survives the swap.");
                            egui::Popup::menu(&change_resp)
                                .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                                .show(|ui| {
                                    ui.set_min_width(260.0);
                                    ui.label(
                                        egui::RichText::new("Pick a different stock texture")
                                            .color(t.muted)
                                            .size(11.0)
                                            .strong(),
                                    );
                                    ui.add_space(4.0);
                                    if let Some(slot_id) =
                                        widgets::slot_picker_grid(ui, slot_picker_entries)
                                    {
                                        actions.borrow_mut().push(LayerAction::ChangeSlot {
                                            layer_id: layer_id.clone(),
                                            new_slot_id: slot_id,
                                        });
                                    }
                                });
                        });
                    });
                }
                LayerSource::Imported { path } => {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("imp: {}", path.display()))
                                .color(t.muted)
                                .size(10.0)
                                .monospace(),
                        )
                        .sense(egui::Sense::hover()),
                    )
                    .on_hover_text("Imported texture source path. Saved verbatim in the project file; missing paths produce a pink layer at load.");
                    ui.horizontal(|ui| {
                        if ui
                            .button("Replace…")
                            .on_hover_text("Pick a new PNG / JPG / DDS to replace this layer's source. Mask and transform are preserved.")
                            .clicked()
                        {
                            actions
                                .borrow_mut()
                                .push(LayerAction::ImportTexture(layer_id.clone()));
                        }
                    });
                }
            },
        );

        // ── TRANSFORM ──
        let mut transform_local = snap.transform;
        widgets::section(
            ui,
            "Transform",
            false,
            |_ui| {},
            |ui| {
                let mut changed = false;
                // Offset X
                let label = format!("{:.0} elmos", transform_local.offset_elmos[0]);
                if widgets::ramp_slider_labelled(
                    ui,
                    "Offset X",
                    &mut transform_local.offset_elmos[0],
                    -extent_x..=extent_x,
                    t.accent,
                    label,
                )
                .on_hover_text("Horizontal shift of the layer texture in elmos. Useful for sliding a tileable rock to break repetition.")
                .changed()
                {
                    changed = true;
                }
                ui.add_space(2.0);
                // Offset Y
                let label = format!("{:.0} elmos", transform_local.offset_elmos[1]);
                if widgets::ramp_slider_labelled(
                    ui,
                    "Offset Y",
                    &mut transform_local.offset_elmos[1],
                    -extent_x..=extent_x,
                    t.accent,
                    label,
                )
                .on_hover_text("Vertical shift of the layer texture in elmos.")
                .changed()
                {
                    changed = true;
                }
                ui.add_space(2.0);
                // Scale
                let label = format!("{:.2}×", transform_local.scale);
                if widgets::ramp_slider_labelled(
                    ui,
                    "Scale",
                    &mut transform_local.scale,
                    0.1..=8.0,
                    t.accent,
                    label,
                )
                .on_hover_text("Uniform texture scale multiplier (0.1× – 8×). 1.0 = native sampling rate; smaller = tile finer, larger = stretch the pattern.")
                .changed()
                {
                    changed = true;
                }
                ui.add_space(2.0);
                // Rotation — store radians, display degrees.
                let mut rot_deg = transform_local.rotation_rad.to_degrees();
                let label = format!("{rot_deg:.0}°");
                if widgets::ramp_slider_labelled(
                    ui,
                    "Rotation",
                    &mut rot_deg,
                    -180.0..=180.0,
                    t.accent,
                    label,
                )
                .on_hover_text("Rotate the layer's UV by degrees (-180..180). Stored internally as radians.")
                .changed()
                {
                    transform_local.rotation_rad = rot_deg.to_radians();
                    changed = true;
                }
                ui.add_space(2.0);
                // Mirror toggles.
                ui.horizontal(|ui| {
                    let mut mx = transform_local.mirror_x;
                    if widgets::pill_toggle(ui, "Mirror X", &mut mx)
                        .on_hover_text("Flip the layer texture horizontally.")
                        .clicked()
                    {
                        transform_local.mirror_x = mx;
                        changed = true;
                    }
                    let mut my = transform_local.mirror_y;
                    if widgets::pill_toggle(ui, "Mirror Y", &mut my)
                        .on_hover_text("Flip the layer texture vertically.")
                        .clicked()
                    {
                        transform_local.mirror_y = my;
                        changed = true;
                    }
                });
                if changed {
                    actions
                        .borrow_mut()
                        .push(LayerAction::SetTransform(layer_id.clone(), transform_local));
                }
            },
        );

        // ── COLOR ──
        let mut color_local = snap.color;
        widgets::section(
            ui,
            "Color",
            false,
            |_ui| {},
            |ui| {
                let mut changed = false;
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Tint").color(t.muted).size(11.0));
                    if ui
                        .color_edit_button_rgb(&mut color_local.tint_rgb)
                        .on_hover_text("Multiplicative RGB tint over the layer's diffuse. White = unchanged. Useful for warming a forest to autumn or cooling a beach to dusk.")
                        .changed()
                    {
                        changed = true;
                    }
                });
                ui.add_space(4.0);
                let label = format!("{:+.2}", color_local.brightness);
                if widgets::ramp_slider_labelled(
                    ui,
                    "Brightness",
                    &mut color_local.brightness,
                    -1.0..=1.0,
                    t.accent,
                    label,
                )
                .on_hover_text("Additive brightness offset (-1..+1). 0 = unchanged.")
                .changed()
                {
                    changed = true;
                }
                if changed {
                    actions
                        .borrow_mut()
                        .push(LayerAction::SetColor(layer_id.clone(), color_local));
                }
            },
        );

        // ── BLEND ──
        let blend_local = snap.blend;
        widgets::section(
            ui,
            "Blend",
            false,
            |_ui| {},
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Mode").color(t.muted).size(11.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let combo = egui::ComboBox::from_id_salt(egui::Id::new(("blend_combo", &layer_id)))
                            .selected_text(blend_label(blend_local))
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(
                                        blend_local == BlendMode::Normal,
                                        blend_label(BlendMode::Normal),
                                    )
                                    .clicked()
                                    && blend_local != BlendMode::Normal
                                {
                                    actions.borrow_mut().push(LayerAction::SetBlend(
                                        layer_id.clone(),
                                        BlendMode::Normal,
                                    ));
                                }
                            });
                        combo.response.on_hover_text("Layer blend mode. Today only Normal is wired; Multiply / Add / Screen ship in a future sprint.");
                    });
                });
                ui.label(
                    egui::RichText::new("Only 'Normal' available; more blend modes coming.")
                        .color(t.dim)
                        .size(10.0),
                );
            },
        );

        // ── DNTS BINDINGS ── (only when channel-bound)
        if snap.dnts_channel.is_some() {
            let mut scale_local = snap.dnts_tex_scale;
            let mut mult_local = snap.dnts_tex_mult;
            widgets::section(
                ui,
                "DNTS bindings",
                false,
                |_ui| {},
                |ui| {
                    let label = format!("{scale_local:.4}");
                    if widgets::ramp_slider_labelled(
                        ui,
                        "tex_scale",
                        &mut scale_local,
                        0.0015..=0.05,
                        t.accent,
                        label,
                    )
                    .on_hover_text("DNTS detail-normal texture frequency. Emitted into mapinfo.resources.splatDetailNormalTexScales. Real BAR maps cluster around 0.005-0.02.")
                    .changed()
                    {
                        actions
                            .borrow_mut()
                            .push(LayerAction::SetDntsTexScale(layer_id.clone(), scale_local));
                    }
                    ui.add_space(2.0);
                    let label = format!("{mult_local:.2}");
                    if widgets::ramp_slider_labelled(
                        ui,
                        "tex_mult",
                        &mut mult_local,
                        0.0..=4.0,
                        t.accent,
                        label,
                    )
                    .on_hover_text("DNTS detail-normal intensity. Emitted into mapinfo.resources.splatDetailNormalTexMults. 0 hides the DNTS contribution; 1.0 is the engine default.")
                    .changed()
                    {
                        actions
                            .borrow_mut()
                            .push(LayerAction::SetDntsTexMult(layer_id.clone(), mult_local));
                    }
                },
            );

            // Imported-layer DNTS warning.
            if matches!(snap.source, LayerSource::Imported { .. }) {
                ui.add_space(4.0);
                widgets::chip(
                    ui,
                    ChipTone::Warn,
                    "Imported textures don't contribute runtime normal detail",
                );
            }
        }
    });
}

fn blend_label(b: BlendMode) -> &'static str {
    match b {
        BlendMode::Normal => "Normal",
    }
}

// ─────────────────────────────────────────────────────────────────
// Footer
// ─────────────────────────────────────────────────────────────────

fn render_footer(
    ui: &mut egui::Ui,
    n: usize,
    mb: usize,
    diffuse_in_alpha_in: bool,
    actions: &std::cell::RefCell<Vec<LayerAction>>,
) {
    let t = Tokens::DARK;
    let mem_chip_tone = if mb > 256 {
        ChipTone::Warn
    } else {
        ChipTone::Neutral
    };
    widgets::section(
        ui,
        "Stack",
        false,
        |_ui| {},
        |ui| {
            ui.horizontal_wrapped(|ui| {
                widgets::chip(ui, ChipTone::Neutral, format!("{n} layers"))
                    .on_hover_text("Total layers in the stack. Order top-to-bottom in the panel = composite order (top wins).");
                widgets::chip(ui, mem_chip_tone, format!("{mb} MB masks"))
                    .on_hover_text("Resident mask memory across all layers. Warns above 256 MB; each unique 4096² mask is ~16 MB.");
                if n > 16 {
                    widgets::chip(ui, ChipTone::Warn, "Preview approximate · 16-layer cap")
                        .on_hover_text("The GPU composite samples at most 16 layers per pass. Layers beyond 16 still bake into the SMT diffuse, but the preview shows only the top 16.");
                }
            });
            ui.add_space(6.0);
            let mut diffuse_in_alpha = diffuse_in_alpha_in;
            if widgets::pill_toggle(ui, "Diffuse in DNTS alpha", &mut diffuse_in_alpha)
                .on_hover_text("Mirrors mapinfo.resources.splatDetailNormalDiffuseAlpha. On = pack diffuse into the DNTS alpha channel; off = leave alpha empty.")
                .clicked()
            {
                actions
                    .borrow_mut()
                    .push(LayerAction::SetDntsDiffuseInAlpha(diffuse_in_alpha));
            }
            ui.label(
                egui::RichText::new("Mirrors mapinfo.resources.splatDetailNormalDiffuseAlpha.")
                    .color(t.dim)
                    .size(10.0),
            );
        },
    );
}
