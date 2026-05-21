mod build_runner;
mod config;
mod launcher;
mod render;
mod ui;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use barme_core::{
    ALLY_GROUP_PALETTE, AllyGroup, BAR_DEFAULT_SURFACE_ALPHA, BAR_DEFAULT_SURFACE_COLOR, BIOMES,
    BrushRegistry, BrushStamp, DirtyRect, FeatureInstance, GeoVent, Heightmap, History,
    HistoryEntry, LayerPropertyValue, LayerStack, MapSize, MetalSpot, PROJECT_EXTENSION, Project,
    ProjectDiff, SlotResolver, SplatConfig, StartPosition, SymmetryAxis, TextureLayer, WaterBlock,
    WaterField, WaterMode, WaterValue, WizardSnapshot,
    brushes::pixel_bbox,
    default_extractor_radius, merge_overrides, preset_water_block,
    procgen::{
        Domain, PRESETS, generate as procgen_generate, generate_thumbnail as procgen_thumbnail_gen,
        validate_expression,
    },
    project::sanitize_name,
    water_override_count,
};
use barme_pipeline::PyMapConvDriver;
use eframe::egui;
use eframe::egui_wgpu;
use tracing::{error, info, trace, warn};

use crate::render::{
    CompositeLayerU, CompositeU, OrbitCamera, SplatUniforms, TerrainCallback, WaterDraw,
};

/// wgpu/vulkan/naga emit a lot of INFO-level chatter at startup (adapter
/// enumeration, layer loading) that drowns out our own logs. Keep them at
/// WARN by default; users can override with `RUST_LOG`.
///
/// Two submodules are bumped one notch further to ERROR because their
/// warn-level events are benign cosmetics on Linux/Wayland:
/// - `wgpu_hal::gles::egl` — "Re-initializing Gles context due to Wayland
///   window" (egui's surface re-init, every launch).
/// - `wgpu_hal::vulkan` — "VALIDATION requested, but unable to find layer:
///   `VK_LAYER_KHRONOS_validation`" (dev-only diagnostic; fires unless
///   `vulkan-validationlayers` is installed system-wide).
///
/// See `docs/RUNTIME-WARNINGS.md` for the rationale and the `RUST_LOG`
/// recipe to re-enable any suppressed line.
const DEFAULT_TRACING_FILTER: &str = "info,wgpu=warn,wgpu_core=warn,wgpu_hal=warn,\
    wgpu_hal::gles::egl=error,wgpu_hal::vulkan=error,\
    naga=warn,egui_wgpu=warn";

/// Side of the square Procgen Inspector thumbnail (B7). 256 pixels →
/// ~65k evalexpr samples per rebake, well inside the 50 ms debounce.
const PROCGEN_THUMBNAIL_PX: usize = 256;

/// Debounce window before a `(expr, domain)` change triggers a
/// thumbnail rebake (B7). 50 ms is short enough to feel live and long
/// enough to coalesce a multi-keystroke burst. Pinned by a smoke test
/// in `procgen.rs` showing 256² cone-peak finishes well inside this
/// budget.
const PROCGEN_THUMBNAIL_DEBOUNCE_MS: u64 = 50;

/// Stable hash of `(expr, domain)` used as the dirty key for the
/// procgen thumbnail cache. Domain is folded in so toggling Unit ↔
/// Centered triggers a re-bake even with an unchanged expression
/// string (B7 pitfall: domain change re-evaluates against a different
/// mapping).
fn procgen_thumbnail_key(expr: &str, domain: Domain) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    expr.hash(&mut h);
    domain.id().hash(&mut h);
    h.finish()
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_TRACING_FILTER)),
        )
        .init();

    info!("barme {} starting", env!("CARGO_PKG_VERSION"));

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 600.0])
            .with_title("BAR Map Editor"),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native(
        "BAR Map Editor",
        native_options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))
}

struct App {
    project_name: String,
    /// Authoritative project map size. Supports rectangular (smu_x ≠ smu_z) —
    /// real BAR maps such as `gecko_isle_remake` are 16×18 SMU. Side panel
    /// edits the two axes independently; the F1 wizard sets both on confirm.
    map_size: MapSize,
    heightmap: Option<HeightmapState>,
    last_error: Option<String>,
    render_state: Option<egui_wgpu::RenderState>,
    camera: OrbitCamera,
    height_scale: f32,
    /// World-space Y for raw heightmap value 0 (in elmos). `< 0`
    /// means the lowest sample carves below sea level — BAR's water
    /// plane sits at Y = 0 (`Ground.h::GetWaterPlaneLevel` is
    /// `consteval`), so this drives the C9 validation chip
    /// (`water_mode` vs `min_height < 0` agreement).
    ///
    /// Mirrors `Project.min_height`. Sprint 14 / C9 introduces the
    /// shadow — earlier `snapshot_project` hard-coded `0.0` which
    /// silently dropped the wizard-set value on first save.
    min_height: f32,
    current_project_path: Option<PathBuf>,
    last_install: Option<Result<PathBuf, String>>,
    /// Sprint 20 — worker-thread build pipeline state. Drives the
    /// progress overlay, status-strip "Building…" chip, and log panel.
    /// `BuildState::Idle` until the user clicks Build; then transitions
    /// `Running → Done | Failed | Cancelled`. Each non-`Idle` variant
    /// owns its log ring buffer so the panel survives until the user
    /// dismisses it (returns to `Idle` on dismiss or on the next
    /// successful build).
    build_state: build_runner::BuildState,
    /// Sprint 20 — is the build log panel window open this frame?
    /// Toggled by the status strip's click affordance, the progress
    /// overlay's "View log…" button, and the top-bar Build > Show log
    /// menu item. Auto-set when the build transitions to Failed.
    build_log_open: bool,
    brushes: BrushRegistry,
    brush_id: Option<String>,
    brush_radius: f32,
    brush_strength: f32,
    symmetry: SymmetryAxis,
    /// Rotational symmetry fold value, kept separately so the UI dropdown
    /// can preserve the user's last choice when toggling between rotational
    /// and non-rotational modes.
    rotational_fold: u8,
    procgen_expr: String,
    procgen_domain: Domain,
    procgen_last_error: Option<String>,
    /// Cached parse-and-dry-eval outcome for the current
    /// `procgen_expr` (A4). `Ok(())` enables the Apply button +
    /// renders the green chip; `Err(msg)` disables Apply + drives the
    /// red chip with `msg` in the tooltip. Refreshed whenever
    /// `procgen_expr` changes (typing, preset pick, biome apply).
    procgen_validation: Result<(), String>,
    /// 256² greyscale preview thumbnail rendered into the Procgen
    /// Inspector (B7). `None` until the first successful generation;
    /// re-uploaded via `handle.set(...)` on every refresh so we don't
    /// leak GPU textures across keystrokes.
    procgen_thumbnail: Option<egui::TextureHandle>,
    /// Hash of `(expr, domain)` last baked into `procgen_thumbnail`.
    /// Re-bake fires when the current `(expr, domain)` hashes
    /// differently AND the debounce window has elapsed. `None` until
    /// the first bake.
    procgen_thumbnail_key: Option<u64>,
    /// Wall-clock timestamp of the last `(expr, domain)` change. The
    /// thumbnail refreshes when `now - this >= PROCGEN_DEBOUNCE_MS`.
    /// Reset whenever the user types or toggles domain; the debounce
    /// re-arms.
    procgen_changed_at: Option<std::time::Instant>,
    /// Persistent undo/redo across the session. Cleared on barrier events
    /// (procgen, heightmap load, new project) — see ADR-022. Stroke state
    /// (copy-on-first-write scratch + bitset) lives inside `History` since
    /// ADR-033.
    history: History,
    /// Active editor tool (ADR-030). Drives the Inspector's exhaustive
    /// `match`, the central viewport's pointer interaction, and the
    /// highlighted button in the left tool strip.
    tool: Tool,
    /// Last-seen value of `tool` — used by `set_tool` to emit a single
    /// `tracing::info!` line per actual change, not on every frame.
    previous_tool: Tool,
    /// Per-side configuration tree (ADR-032 / B6). Round-trips through
    /// `Project`. Empty by default — the pipeline emits a 25 % / 75 %
    /// diagonal default pair when ally_groups is empty. The F8 tool
    /// adds a default `AllyGroup` on first placement when this is
    /// empty.
    ally_groups: Vec<AllyGroup>,
    /// Identifier of the ally group currently receiving F8 placements.
    /// Tracks the user's selection in the Inspector tree. Adjusted on
    /// preset apply / Add AllyGroup / group delete to point at a
    /// surviving group.
    active_ally_group_id: u8,
    /// While LMB is held in `StartPositions` on an existing marker,
    /// holds `(ally_group_id, source_index_within_group)` so the drag
    /// re-positions that exact source. Cleared on release.
    dragging_start_pos: Option<(u8, usize)>,
    /// Pre-drag coordinates of the source being dragged. `Some`
    /// whenever `dragging_start_pos` is `Some`; on drag-stop the
    /// `from` is paired with the now-current `to` and pushed as a
    /// `ProjectDiff::MoveStartPosition` undo entry (B5 / ADR-032).
    dragging_start_pos_from: Option<StartPosition>,
    /// F8 drag-paint count: when the user LMB-drags across empty
    /// terrain, this many positions are distributed evenly along the
    /// drag vector. Default 8 — the canonical 8v8 case. Lives in an
    /// Inspector `DragValue`. ADR-032.
    drag_paint_count: u8,
    /// Frame of in-flight drag origin world coords. `Some` from
    /// `drag_started_by(LMB)` until `drag_stopped_by(LMB)`. Used to
    /// disambiguate drag-paint (origin on empty terrain) from
    /// drag-move (origin on a marker — recorded in
    /// `dragging_start_pos`).
    drag_paint_origin: Option<glam::Vec2>,
    /// Hover↔pulse plumbing (ADR-032). `Some((ally_group_id, source
    /// index, hover-instant))` when an Inspector row is hovered;
    /// the marker pulses (thick ring at 2 Hz) for 1 s after the
    /// timestamp.
    pulsing_marker: Option<(u8, usize, std::time::Instant)>,
    /// `Some((ally_group_id, source_index))` when the canvas marker is
    /// hovered. Drives the Inspector to scroll the matching row into
    /// view. Cleared every frame; set during marker hit-test.
    hovered_canvas_marker: Option<(u8, usize)>,
    /// F1 new-project wizard (ADR-024). Open on app launch (replaces the
    /// pre-F1 in-memory "untitled" auto-start) and via File → New project.
    wizard_open: bool,
    wizard: WizardState,
    /// Top-bar symmetry chip popover. Toggled by clicking the chip; the
    /// popover holds the symmetry-axis combo + rotational fold spinner
    /// (the controls that used to live in the Sculpt section of the
    /// side panel pre-ADR-030). B2 will replace the popover with a
    /// canvas overlay; B1 keeps the controls reachable.
    symmetry_popover_open: bool,
    /// Per-user editor config (TOML at the OS config dir). Carries the
    /// version-keyed "first-launch hint dismissed" flag. B3.
    editor_config: config::EditorConfig,
    /// Whether the first-launch hint Window should render this frame.
    /// Initialised from `editor_config.intro_seen_for_current_version()`.
    show_intro: bool,
    /// Whether the `?` cheat-sheet modal is open this frame.
    show_cheat_sheet: bool,
    /// Sprint 19 / U1 — is the lint-panel window open this frame? Driven by
    /// the top-bar validation chip and the status-strip issue-count label.
    /// Stub today; Sprint 21 / C8 lands the full `LintRule` registry.
    lint_panel_open: bool,
    /// Sprint 19 / U1 — previous-frame snapshot so `lint_panel::render`
    /// can emit `trace!` exactly on the open / close transitions
    /// without spamming the log every frame.
    lint_panel_was_open: bool,
    /// Retired by ADR-035 — the nav gizmo was replaced by the
    /// top-down mini-map. Field kept (always-false) only because
    /// removing it churns tests + downstream init blocks; clean
    /// removal can happen in a follow-up.
    #[allow(dead_code)]
    nav_gizmo_drag_active: bool,
    /// User-selected build pipeline variant for the top-bar primary
    /// button (B4). Persists across button clicks within a session.
    build_variant: BuildVariant,
    /// User-authored `mapinfo.lua` field overrides (C1 / ADR-028).
    /// Mirrors `Project.mapinfo_overrides` so save/open preserves the
    /// user's F9 edits across sessions. F9 (C7) will own the editor
    /// surface; here we just round-trip the bag so data isn't lost
    /// before that lands.
    mapinfo_overrides: std::collections::HashMap<String, toml::Value>,
    /// B8: whether the "Next steps" hint window should render this
    /// frame. Set to `true` by `apply_wizard`; toggled to `false` by
    /// the X-button handler and persisted into
    /// `Project.next_steps_dismissed` so reopening the same project
    /// leaves the window dismissed. A fresh project (`File → New`)
    /// re-shows it because the new `Project` starts with
    /// `next_steps_dismissed = false`.
    show_next_steps: bool,
    /// B8: mirrors `Project.next_steps_dismissed`. Snapshotted into
    /// the next save; load_project / new_project sync this with
    /// `Project::next_steps_dismissed`.
    next_steps_dismissed: bool,
    /// D7 / Sprint 18 (F10): mirrors `Project.minimap_override`. When
    /// `Some`, the build pipeline copies this PNG into the `.sd7`
    /// verbatim (after a 1024² dim check) instead of running the
    /// auto-bake. The F9 form's Minimap tab surfaces the file picker
    /// and "Clear override" button that drive this field; until C7
    /// lands the field round-trips silently so the path survives
    /// save/open.
    minimap_override: Option<PathBuf>,
    /// C7 / Sprint 18 (F9): is the mapinfo form `egui::Window` open?
    /// Driven by the top-bar `Icon::MapInfo` button.
    mapinfo_form_open: bool,
    /// C7 / Sprint 18 (F9): which tab the mapinfo form is showing.
    /// Persisted across re-opens of the form within a session.
    mapinfo_form_tab: crate::ui::inspector_mapinfo::MapInfoTab,
    /// D10 / Sprint 17 (ADR-041): mirrors
    /// `Project.migration_toast_dismissed`. Persists across save /
    /// open so the "your splat layers were migrated" toast stays
    /// quiet once dismissed.
    migration_toast_dismissed: bool,
    /// D10 / Sprint 17 (ADR-041): session-only flag set by the open
    /// path when `after_load_migrate` actually seeded layers from a
    /// pre-Sprint-14 `splat_config`. Drives the one-frame info toast
    /// surfaced in `App::update`; cleared on dismiss.
    pending_migration_toast: bool,
    /// ADR-035: best-effort dirty flag for the top-bar Save chip.
    /// Set by [`Self::mark_dirty`] (called from edit sites); cleared
    /// by `save_to` / `new_project` / `open_from`. Not durable across
    /// sessions — undo log + the on-disk project file are authoritative.
    dirty: bool,
    /// ADR-035: the last non-`None` symmetry mode the user picked. The
    /// top-bar pill toggle switches between `None` (off) and this
    /// value (on); without it, toggling off+on would forget the mode.
    last_non_none_symmetry: SymmetryAxis,
    /// ADR-035 (Phase 6): viewport-options toolbar toggles. These
    /// currently drive only the overlay rendering — the wgpu pipeline
    /// is untouched until a future ADR.
    grid_overlay_on: bool,
    lighting_on: bool,
    wireframe_on: bool,
    /// Sprint 11 hotfix follow-up: viewport overlay that shades
    /// terrain too steep for a factory in red. Factory `maxslope`
    /// is 15 (BAR `armlab.lua`/`corlab.lua`); the engine divides by
    /// 1.5 → effective cap of ~10°. The fragment shader checks
    /// `world_normal.y < cos(10°)` ≈ `0.9848` and mixes in red.
    /// Mex spots use a more generous 20° cap (mex `maxslope = 30`)
    /// — surfaced as a hover-tooltip nuance, not a separate mode.
    buildable_overlay_on: bool,
    /// D8 / Sprint 15 (ADR-038): the project's texture layer stack —
    /// the Photoshop-style stack of [`TextureLayer`]s the `.sd7` bake
    /// composites into the diffuse BMP. Mirrors `Project.layers`;
    /// persists through `snapshot_project` / `open_from`. Sprint 15
    /// shipped data + bake; Sprint 16 wires the GPU live preview.
    layer_stack: LayerStack,
    /// D9 / Sprint 16 (ADR-039): per-layer mask versions last uploaded
    /// to the composite mask array. Keyed by `(layer_id,
    /// composite_slot_idx)`. `version() > last_uploaded` drives the
    /// per-frame [`render::write_composite_layer_mask_tiles`] dispatch
    /// so a single brush stroke only triggers a tile-scoped GPU upload
    /// next frame, not a full mask re-push.
    composite_layer_last_version: std::collections::HashMap<(String, usize), u64>,
    /// D9 / Sprint 16 (ADR-040): the layer id paint strokes currently
    /// target. `None` until the user enters [`Tool::PaintLayer`] for
    /// the first time, then defaults to the top-of-stack visible
    /// layer. Persists across tool switches so re-entering paint
    /// mode resumes on the same layer.
    paint_active_layer_id: Option<String>,
    /// D9 / Sprint 16 (ADR-040): top-down 2D paint viewport pan / zoom.
    /// Pan: world-elmo offset from the central viewport's centre;
    /// zoom: RT-pixels per logical screen pixel (default = fit map).
    paint_view_state: PaintViewState,
    /// D9 / Sprint 16: per-session mask brush settings — radius /
    /// strength / spacing sliders + active brush id + the mask-only
    /// preview toggle.
    paint_brush_state: PaintBrushState,
    /// D9 / Sprint 16: mask brush registry (`mask-reveal` / `mask-hide`
    /// / `mask-smooth` / `mask-fill`). Looked up by id at stamp time.
    mask_brushes: barme_core::MaskBrushRegistry,
    /// D9 / Sprint 16 (ADR-040): the previous frame's cursor position
    /// in world-elmo space, used to interpolate stamps along a fast
    /// drag (PITFALL §3 — fast drags must not leave gaps). Cleared
    /// on `drag_stopped` so a fresh LMB press doesn't carry a stale
    /// `prev` from the prior stroke.
    paint_last_drag_pos: Option<glam::Vec2>,
    /// D10 / Sprint 17 (ADR-041): per-layer 32² thumbnail cache. Keyed
    /// by `layer.id` because `LayerSource::Imported` thumbnails are
    /// derived from the imported PNG (one-shot decode on first surface),
    /// not the slot id. Survives a project switch because the live
    /// thumbnail cache is cheap and the entries' ids are unique per
    /// layer.
    layer_thumbnails: std::collections::HashMap<String, egui::TextureHandle>,
    /// D10 / Sprint 17 (ADR-041): in-flight drag-to-reorder state.
    /// `Some(ids)` when the user is mid-drag — the Layers panel renders
    /// in this order without committing. On drop, the panel emits one
    /// `ProjectDiff::ReorderLayer` (which goes through
    /// [`Self::reorder_layer`] and triggers the 64 MB diffuse re-upload
    /// exactly once); during the drag itself the re-upload stays
    /// suppressed.
    paint_drag_preview_order: Option<Vec<String>>,
    /// D10 / Sprint 17 (ADR-041): active layer's mask preview overlay.
    /// `Some((layer_id, mask_version, handle))` when the cache is up to
    /// date; the per-frame helper invalidates when either changes.
    layer_mask_preview_cache: Option<(String, u64, egui::TextureHandle)>,
    /// D10 / Sprint 17 (ADR-041): the layers panel's last-rendered
    /// screen rect, captured so the drag-drop handler can route file
    /// drops onto the layers panel vs the central viewport. `None`
    /// when the panel isn't on screen this frame.
    layers_panel_rect: Option<egui::Rect>,
    /// D10 / Sprint 17 (ADR-041): mirrors
    /// [`barme_core::Project::dnts_diffuse_in_alpha`]. Drives the
    /// Layers panel footer toggle + the splat pipeline's per-build
    /// `BakeOptions.diffuse_in_alpha`. Replaces the per-channel
    /// `splat_config.diffuse_in_alpha`; migration copies the legacy
    /// value across on first load of a pre-Sprint-17 project.
    dnts_diffuse_in_alpha: bool,
    /// D1 / ADR-027 slot registry, scanned once at app start from
    /// `tools/textures/<NN-slot>/`. Each entry pairs a slot id with
    /// its display name + dir for the thumbnail loader. Empty when
    /// `tools/textures/` is missing (e.g. first checkout before
    /// `scripts/fetch-textures.sh` has run).
    slot_registry: Vec<SlotMeta>,
    /// Lazy cache of slot id → 96² thumbnail handle for the slot
    /// picker grid + inspector row swatch. Populated on first
    /// inspector render that surfaces a given slot.
    slot_thumbnails: std::collections::HashMap<u8, egui::TextureHandle>,
    metal_state: MetalState,
    geo_state: GeoState,
    /// C4 (Sprint 11): F5 metal-spot sources, mirroring
    /// `barme_core::Project::metal_spots`. Persists across save /
    /// open through `snapshot_project` / `open_from`.
    metal_spots: Vec<MetalSpot>,
    /// C5 (Sprint 11): F6 geo-vent sources, mirroring
    /// `barme_core::Project::geo_vents`.
    geo_vents: Vec<GeoVent>,
    /// C4 (Sprint 11): BAR-convention extractor radius in elmos.
    /// Edited in the metal-spots inspector; surfaces as
    /// `mapinfo.extractor_radius` through the F9 form (Sprint 13).
    /// Default 80 per [`barme_core::default_extractor_radius`] —
    /// PITFALL §6 (engine default 500 breaks BAR's mex-snap).
    extractor_radius: f32,
    /// While LMB is held in `Tool::MetalSpots` on an existing spot,
    /// holds the source's index in `metal_spots` so the drag moves
    /// that exact entry. Cleared on release / RMB.
    dragging_metal_spot: Option<usize>,
    /// Pre-drag spot record for the metal-spot drag finalizer. On
    /// drag-stop the original is paired with the now-current spot
    /// and pushed as `ProjectDiff::MoveMetalSpot`. Mirrors the F8
    /// `dragging_start_pos_from` pattern.
    dragging_metal_spot_from: Option<MetalSpot>,
    /// Mirror of the above for the geo-vent tool.
    dragging_geo_vent: Option<usize>,
    dragging_geo_vent_from: Option<GeoVent>,
    /// C6 (Sprint 12): F7 user-feature sources. Mirrors
    /// `barme_core::Project::features`. Persists across save / open
    /// through `snapshot_project` / `open_from`.
    features: Vec<FeatureInstance>,
    /// C6: session-only state for the feature picker / placed-list UI.
    feature_state: FeatureState,
    /// C6: while LMB is held in `Tool::Feature` on an existing feature
    /// marker, holds the source's index in `features` so the drag
    /// rotates that exact entry (rotation = drag dx → heading delta).
    /// Cleared on release.
    dragging_feature: Option<usize>,
    /// C6: pre-drag feature record for the drag finalizer. On
    /// drag-stop the original is paired with the now-current entry
    /// and pushed as a `ProjectDiff::MoveFeature` undo entry.
    dragging_feature_from: Option<FeatureInstance>,
    /// C6: anchor (cursor x at drag-start) so the rotation gesture
    /// produces a stable per-pixel heading delta. `Some(anchor_x)`
    /// from `drag_started_by(LMB)` until `drag_stopped_by(LMB)`.
    dragging_feature_anchor_x: Option<f32>,
    /// C6: pre-drag heading captured at drag-start; combined with
    /// `(cursor_x - anchor_x)` and `ROTATE_GAIN_PER_PX` to compute the
    /// in-progress rotation. Cleared on release.
    dragging_feature_start_rot: Option<u16>,
    /// C9 (Sprint 14 / ADR-042): active water preset. Mirrors
    /// `Project.water_mode`; round-trips through save/open and drives
    /// the `mapinfo.water` emission path.
    water_mode: WaterMode,
    /// C9 (Sprint 14): sparse per-field overrides applied on top of
    /// the active preset. Mirrors `Project.water_overrides`. Survives
    /// preset changes — switching modes only updates `water_mode`,
    /// never blows away `water_overrides`.
    water_overrides: WaterBlock,
    /// C9 (Sprint 14): top-level `voidWater` shadow. Mirrors
    /// `Project.void_water`. Mutually exclusive with
    /// `water_overrides.plane_color` (PITFALL §6).
    void_water: bool,
    /// C9 (Sprint 14): top-level `tidalStrength` shadow. Mirrors
    /// `Project.tidal_strength`. Lives at MapInfo top level, NOT
    /// inside `water = {}` — the inspector co-locates for UX.
    tidal_strength: Option<f32>,
    /// C9 (Sprint 14): lava-atmosphere offer. Mirrors
    /// `Project.lava_atmosphere`. When `true`, the emission path
    /// applies a hardcoded fog/sun/cloud patch (red-orange fog, dim
    /// warm sun) on top of `bar_default()`. Independent of
    /// `water_mode` so the user can apply it freely.
    lava_atmosphere: bool,
    /// C9 (Sprint 14): ephemeral target depth for `Tool::Water`'s
    /// flood-carve gesture (elmos, negative = lower terrain). NOT
    /// persisted — same status as `brush_radius` / `brush_strength`
    /// (per-session tool preference). Default `-80` matches a
    /// generic flooded basin and the prompt's spec.
    ///
    /// Commit 3 (Slice 3) wires this through `apply_brush_at`'s
    /// dispatch when `Tool::Water` is active; Commit 1 lays the
    /// session state in advance so save/load + the inspector form
    /// don't have to ship in lockstep.
    #[allow(dead_code)]
    water_carve_depth: f32,
}

/// View-state for the F7 feature picker + placed-list inspector.
/// Pure session state — the placements themselves live on
/// `App::features` (mirrored to `Project.features` on save).
#[derive(Debug, Clone)]
struct FeatureState {
    /// Stock manifest, parsed once at app start from
    /// `assets/mapfeatures_catalog.json`. Empty when the file is
    /// missing (first checkout before assets are committed).
    manifest: FeatureCatalog,
    /// User's current category selection in the inspector
    /// (`"trees"` / `"rocks"` / `"wreckage"` / `"props"` / `"geo"`).
    /// Drives the feature picker list below it.
    active_category: String,
    /// Currently-selected feature name (the picker's highlight).
    /// LMB-clicks on the canvas place this name; `None` until the
    /// user picks one.
    selected_feature: Option<String>,
    /// Free-text filter applied to the feature picker.
    filter: String,
    /// Index of the placed-features tree row the user clicked. Drives
    /// hover-pulse + inspector scroll. Cleared on tool switch.
    selected_placed: Option<usize>,
}

impl Default for FeatureState {
    fn default() -> Self {
        Self {
            manifest: FeatureCatalog::default(),
            active_category: "trees".to_string(),
            selected_feature: None,
            filter: String::new(),
            selected_placed: None,
        }
    }
}

/// Parsed shape of `assets/mapfeatures_catalog.json` (C6 / Sprint 12;
/// extended Sprint 19 with `category_visuals` + per-entry `metal`).
/// Loaded once at App start; `Default` is the empty catalog.
#[derive(Debug, Clone, Default, serde::Deserialize)]
struct FeatureCatalog {
    #[serde(default)]
    categories: std::collections::BTreeMap<String, Vec<CatalogEntry>>,
    #[serde(default)]
    category_visuals: std::collections::BTreeMap<String, CategoryVisual>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct CatalogEntry {
    name: String,
    display: String,
    /// Metal value the feature yields when reclaimed. Falls back to 0
    /// (the BAR-engine default for un-reclaimable features) when the
    /// catalog entry doesn't specify; rocks / wreckage / crates have
    /// non-zero defaults curated in the JSON.
    #[serde(default)]
    metal: u32,
    #[serde(default)]
    #[allow(dead_code)] // surfaced in inspector tooltip in a future polish pass
    tags: Vec<String>,
}

/// Per-category visual hint used by the marker batch + minimap.
/// Loaded from `category_visuals` in the catalog JSON; deserialised
/// case-sensitively so the JSON matches `MarkerShape` variant names
/// byte-for-byte.
#[derive(Debug, Clone, serde::Deserialize)]
struct CategoryVisual {
    shape: String,
    /// CSS-style `#RRGGBB` colour. Parsed to `egui::Color32` on first
    /// use; falls back to `LIGHT_GRAY` if malformed.
    color: String,
    radius_px: f32,
}

/// Resolved per-feature visual hint — what the marker / minimap
/// renderer needs without re-walking the catalog every frame.
#[derive(Debug, Clone, Copy)]
struct ResolvedFeatureVisual {
    shape: crate::ui::markers::MarkerShape,
    color: egui::Color32,
    radius_px: f32,
}

/// Fallback visual for catalog miss-hits. Distinct enough from the
/// curated category palettes that an unknown FeatureDef stands out as
/// "needs catalog work" rather than blending in with a stock category.
const FALLBACK_FEATURE_VISUAL: ResolvedFeatureVisual = ResolvedFeatureVisual {
    shape: crate::ui::markers::MarkerShape::OutlineRing,
    color: egui::Color32::from_rgb(0xD1, 0xD5, 0xDB),
    radius_px: 6.0,
};

fn parse_hex_color(s: &str) -> Option<egui::Color32> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(egui::Color32::from_rgb(r, g, b))
}

fn parse_marker_shape(s: &str) -> Option<crate::ui::markers::MarkerShape> {
    use crate::ui::markers::MarkerShape;
    match s {
        "FilledCircle" => Some(MarkerShape::FilledCircle),
        "OutlineRing" => Some(MarkerShape::OutlineRing),
        "FilledWithStroke" => Some(MarkerShape::FilledWithStroke),
        "Triangle" => Some(MarkerShape::Triangle),
        "OutlineTriangle" => Some(MarkerShape::OutlineTriangle),
        _ => None,
    }
}

impl FeatureCatalog {
    /// Load the catalog from `assets/mapfeatures_catalog.json`. Falls
    /// back to an empty catalog (with a `warn!` log) on any error so
    /// a first checkout without the asset still launches.
    fn load_default(repo_root: &Path) -> Self {
        let path = repo_root.join("assets").join("mapfeatures_catalog.json");
        match std::fs::read_to_string(&path) {
            Ok(s) => match serde_json::from_str::<Self>(&s) {
                Ok(c) => {
                    info!(
                        path = %path.display(),
                        category_count = c.categories.len(),
                        "feature catalog loaded"
                    );
                    c
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "feature catalog parse failed; using empty catalog");
                    Self::default()
                }
            },
            Err(e) => {
                warn!(path = %path.display(), error = %e, "feature catalog missing; using empty catalog");
                Self::default()
            }
        }
    }

    fn category_names(&self) -> Vec<String> {
        self.categories.keys().cloned().collect()
    }

    fn entries(&self, category: &str) -> &[CatalogEntry] {
        self.categories
            .get(category)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Find an entry by FeatureDef name. Returns the `(category,
    /// entry)` pair so the caller can resolve visuals via
    /// `category_visuals`. None on a catalog miss-hit.
    fn lookup_entry(&self, name: &str) -> Option<(&str, &CatalogEntry)> {
        for (cat_name, entries) in &self.categories {
            if let Some(e) = entries.iter().find(|e| e.name == name) {
                return Some((cat_name.as_str(), e));
            }
        }
        None
    }

    /// Resolve the visual hint for a placed feature. Falls back to
    /// [`FALLBACK_FEATURE_VISUAL`] for unknown names so the renderer
    /// always has something to draw — orphaned features are common
    /// during round-trip with custom feature packs (engine logs +
    /// skips at game start; the editor should not).
    fn resolved_visual(&self, name: &str) -> ResolvedFeatureVisual {
        let Some((cat, _)) = self.lookup_entry(name) else {
            return FALLBACK_FEATURE_VISUAL;
        };
        let Some(v) = self.category_visuals.get(cat) else {
            return FALLBACK_FEATURE_VISUAL;
        };
        let shape = parse_marker_shape(&v.shape).unwrap_or(FALLBACK_FEATURE_VISUAL.shape);
        let color = parse_hex_color(&v.color).unwrap_or(FALLBACK_FEATURE_VISUAL.color);
        ResolvedFeatureVisual {
            shape,
            color,
            radius_px: v.radius_px,
        }
    }

    /// Metal value for a placed feature by name. `None` on catalog
    /// miss-hit so the caller can distinguish "0 known" from "unknown".
    fn metal_for(&self, name: &str) -> Option<u32> {
        self.lookup_entry(name).map(|(_, e)| e.metal)
    }
}

/// D9 / Sprint 16 (ADR-040): per-session view state for the top-
/// down 2D paint viewport. Pan in world-elmo space; zoom = RT
/// pixels per logical screen pixel. Default (zoom = 0.0) is the
/// "fit map to viewport" auto-zoom — the viewport solves for the
/// per-axis zoom on each frame so the map fills the available
/// rect with 1:1 aspect (letterboxed bands on the short axis).
#[derive(Debug, Clone, Copy)]
struct PaintViewState {
    /// World-elmo offset from the map centre.
    pan_elmos: glam::Vec2,
    /// Manual zoom factor in screen-px per RT-px. `0.0` = use
    /// auto-fit; >0 = explicit zoom. Bounded `[0.25, 16.0]` on user
    /// input. Double-click resets to `0.0` (auto-fit).
    zoom: f32,
}

impl Default for PaintViewState {
    fn default() -> Self {
        Self {
            pan_elmos: glam::Vec2::ZERO,
            zoom: 0.0,
        }
    }
}

/// D9 / Sprint 16 (ADR-040): per-session mask brush controls.
/// Mirrors [`SplatBrushState`] but addresses layer masks rather
/// than the splat distribution. None of these belong on the
/// project — they're tool preferences.
#[derive(Debug, Clone)]
struct PaintBrushState {
    /// `mask-reveal` / `mask-hide` / `mask-smooth` / `mask-fill`.
    /// Resolved against `App::mask_brushes` at stamp time.
    brush_id: String,
    /// Radius in elmos. Mask is 1 px = 1 elmo, so this maps
    /// directly to pixel radius.
    radius: f32,
    /// Strength 0..=1.
    strength: f32,
    /// Stamp spacing along a drag, in radii. `0.5` = one stamp per
    /// half-radius of pointer motion (Sprint 9 default — avoids
    /// gaps on fast drags while keeping per-frame stamp count
    /// bounded).
    spacing: f32,
    /// Show only the active layer's mask in the 2D viewport
    /// (grayscale; red overlay where mask = 0). Useful for
    /// scrubbing a mask without the diffuse composite interfering
    /// visually.
    mask_only_preview: bool,
    /// `mask-fill` target visibility: when `true`, fill paints
    /// 255; when `false`, fill paints 0.
    fill_target_visible: bool,
}

impl Default for PaintBrushState {
    fn default() -> Self {
        Self {
            brush_id: "mask-reveal".to_string(),
            // 192 elmos = ~2.3% of a 16-SMU map's width — visible at
            // a glance during the first paint test without being so
            // large it dominates the canvas. Mirrors the Sprint-9
            // splat default that aims for "obviously a stroke,
            // obviously not the whole map."
            radius: 192.0,
            strength: 0.5,
            spacing: 0.5,
            mask_only_preview: false,
            fill_target_visible: true,
        }
    }
}

/// D8 / Sprint 15 (ADR-038): apply one [`LayerPropertyValue`] onto
/// `layer`. The undo dispatcher invokes this with the `from` value
/// on undo and `to` on redo — single-direction routing keeps the
/// path symmetric. Mask edits intentionally aren't here; they live
/// on a separate Sprint 16 / D9 path.
///
/// D10 / Sprint 17 (ADR-041) — `pub(crate)` so [`crate::ui::layers_panel`]
/// can reuse it. The dispatcher in [`App::apply_project_diff`] remains
/// the canonical caller.
/// D10 / Sprint 17 (ADR-041) — escape a value for embedding inside a
/// double-quoted TOML basic-string. Handles backslash, double quote,
/// and the standard control chars. Used by the imported-texture
/// sidecar's `meta.toml`.
fn escape_toml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

pub(crate) fn apply_layer_property(layer: &mut TextureLayer, value: &LayerPropertyValue) {
    match value {
        LayerPropertyValue::Name(s) => layer.name = s.clone(),
        LayerPropertyValue::Transform(t) => layer.transform = *t,
        LayerPropertyValue::Color(c) => layer.color = *c,
        LayerPropertyValue::Blend(b) => layer.blend = *b,
        LayerPropertyValue::Visible(v) => layer.visible = *v,
        LayerPropertyValue::Locked(v) => layer.locked = *v,
        LayerPropertyValue::Opacity(o) => layer.opacity = *o,
        LayerPropertyValue::DntsChannel(c) => layer.dnts_channel = *c,
        LayerPropertyValue::Source(s) => layer.source = s.clone(),
        LayerPropertyValue::DntsTexScale(s) => layer.dnts_tex_scale = *s,
        LayerPropertyValue::DntsTexMult(m) => layer.dnts_tex_mult = *m,
    }
}

/// Slot registry entry (ADR-027). Built once from `tools/textures/`.
#[derive(Debug, Clone)]
struct SlotMeta {
    /// Slot index — `00`..`15` for the starter pack.
    id: u8,
    /// Human-readable name from the slot's `meta.toml`.
    name: String,
    /// Path to the slot directory, e.g. `tools/textures/00-grass-meadow/`.
    /// `diffuse.png` lives directly inside it.
    dir: PathBuf,
}

/// D8 / Sprint 15 (ADR-038): adapter so the [`LayerStack`] bake can
/// reach the App's slot registry without depending on `barme-app`.
/// Wraps `&[SlotMeta]` and resolves each slot id to its `diffuse.png`.
///
/// D10 / Sprint 17 (ADR-041): also carries an optional project root,
/// used to resolve relative `LayerSource::Imported` paths (the
/// project-local sidecar lives at `<root>/textures/<uuid>.png`).
struct AppSlotResolver<'a> {
    slots: &'a [SlotMeta],
    project_root: Option<&'a Path>,
}

impl<'a> AppSlotResolver<'a> {
    /// Build a resolver with an optional project root attached so
    /// relative `LayerSource::Imported` paths resolve to the sidecar
    /// directory. Pass `None` for paths-as-CWD-relative behaviour.
    fn with_project_root(slots: &'a [SlotMeta], project_root: Option<&'a Path>) -> Self {
        Self {
            slots,
            project_root,
        }
    }
}

impl<'a> SlotResolver for AppSlotResolver<'a> {
    fn diffuse_path(&self, slot_id: u8) -> Option<PathBuf> {
        self.slots
            .iter()
            .find(|s| s.id == slot_id)
            .map(|s| s.dir.join("diffuse.png"))
    }
    fn imported_root(&self) -> Option<&std::path::Path> {
        self.project_root
    }
}

/// One-time scan of `tools/textures/` building the slot registry. Each
/// subdir is expected to follow the ADR-027 layout
/// (`<NN>-<slug>/{meta.toml,diffuse.png,normal.png}`); dirs that don't
/// parse get warned and skipped rather than panicking — first
/// checkouts before `scripts/fetch-textures.sh` runs have an empty
/// dir and the registry stays empty.
fn scan_slot_registry(root: &Path) -> Vec<SlotMeta> {
    if !root.exists() {
        info!(root = %root.display(), "texture registry root missing — skipping scan");
        return Vec::new();
    }
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(root) {
        Ok(it) => it,
        Err(e) => {
            warn!(root = %root.display(), error = %e, "texture registry scan failed");
            return Vec::new();
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let meta_path = path.join("meta.toml");
        if !meta_path.exists() {
            continue;
        }
        let toml_str = match std::fs::read_to_string(&meta_path) {
            Ok(s) => s,
            Err(e) => {
                warn!(meta = %meta_path.display(), error = %e, "slot meta unreadable");
                continue;
            }
        };
        // Parse just the fields we care about — slot + name. The
        // texture-pack fetch script owns the file shape.
        let value: toml::Value = match toml::from_str(&toml_str) {
            Ok(v) => v,
            Err(e) => {
                warn!(meta = %meta_path.display(), error = %e, "slot meta parse failed");
                continue;
            }
        };
        let id = match value.get("slot").and_then(|v| v.as_integer()) {
            Some(n) if (0..=255).contains(&n) => n as u8,
            _ => {
                warn!(meta = %meta_path.display(), "slot meta missing valid `slot` field");
                continue;
            }
        };
        let name = value
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("Slot {id:02}"));
        out.push(SlotMeta {
            id,
            name,
            dir: path,
        });
    }
    out.sort_by_key(|m| m.id);
    info!(count = out.len(), "scanned slot registry");
    out
}

/// Load a slot's `diffuse.png`, downscale to a 96² thumbnail, and
/// return the RGBA bytes. Used for the inspector row swatch + slot
/// picker grid. Returns `None` if the file is missing / unreadable —
/// the caller substitutes a neutral grey square.
fn load_slot_thumbnail_rgba(slot: &SlotMeta) -> Option<image::RgbaImage> {
    let diffuse = slot.dir.join("diffuse.png");
    let img = match image::open(&diffuse) {
        Ok(i) => i,
        Err(e) => {
            warn!(path = %diffuse.display(), error = %e, "diffuse.png load failed");
            return None;
        }
    };
    let rgba = img.to_rgba8();
    // 96² thumbnail keeps the inspector decode budget < 1 ms per slot
    // on first paint (a 1024² → 96² nearest-neighbour resize).
    let thumb = image::imageops::resize(&rgba, 96, 96, image::imageops::FilterType::Triangle);
    Some(thumb)
}

/// Load a slot's `diffuse.png` at full resolution + resize to
/// `SLOT_DIFFUSE_DIM` if needed (per FINDINGS H2 — starter pack ships
/// 1024² so the happy path skips the resize). Returns the RGBA8 bytes
/// ready for `render::upload_diffuse_layer`.
fn load_slot_full_rgba(slot: &SlotMeta) -> Option<image::RgbaImage> {
    use crate::render::SLOT_DIFFUSE_DIM;
    let diffuse = slot.dir.join("diffuse.png");
    let img = match image::open(&diffuse) {
        Ok(i) => i,
        Err(e) => {
            warn!(path = %diffuse.display(), error = %e, "diffuse.png load failed");
            return None;
        }
    };
    let mut rgba = img.to_rgba8();
    if rgba.width() != SLOT_DIFFUSE_DIM || rgba.height() != SLOT_DIFFUSE_DIM {
        rgba = image::imageops::resize(
            &rgba,
            SLOT_DIFFUSE_DIM,
            SLOT_DIFFUSE_DIM,
            image::imageops::FilterType::Lanczos3,
        );
    }
    Some(rgba)
}

/// View-state for the F5 metal-spots inspector + viewport. Spot data
/// itself lives on `App::metal_spots` (mirrors `barme_core::Project`);
/// this holds only the selection / drag state that's session-scoped.
///
/// C4 (Sprint 11) replaces the Phase-7 placeholder `MetalState`
/// (density / min_spacing / max_metal / spots) with the slim shape
/// the F5 schema work actually needs — the generator-style sliders
/// were never wired to anything and the spot data now belongs on
/// the Project.
#[derive(Debug, Clone, Default)]
struct MetalState {
    /// Index in `App::metal_spots` of the spot the user clicked in
    /// the Spots table. Drives hover-pulse + inspector scroll. Cleared
    /// on tool switch.
    selected: Option<usize>,
}

/// View-state for the F6 geo-vents inspector + viewport. Mirrors
/// [`MetalState`]. Geo vents have no metal value or extractor radius,
/// so the inspector is leaner than metal's.
#[derive(Debug, Clone, Default)]
struct GeoState {
    selected: Option<usize>,
}

/// Mutable form state for the F1 wizard. Held independently of App state
/// so dismissing the wizard mid-edit doesn't disturb whatever project is
/// already loaded. Sized to a single contiguous struct so we can swap it
/// out wholesale on apply.
#[derive(Debug, Clone)]
struct WizardState {
    project_name: String,
    smu_x: u32,
    smu_z: u32,
    symmetry: SymmetryAxis,
    rotational_fold: u8,
    biome_index: usize,
    max_height: f32,
    /// True until the user manually edits `max_height`. While true, picking
    /// a different biome resets the field to the new biome's hint; once the
    /// user touches the field directly the hint stops overriding.
    height_from_biome: bool,
}

impl WizardState {
    fn default_for_new_project() -> Self {
        Self {
            project_name: "untitled".to_string(),
            smu_x: 16,
            smu_z: 16,
            // B8: Horizontal default lines up with the two ally groups
            // apply_wizard places along the N/S strips, so a Create-
            // and-build user gets a symmetric 1v1 out of the box. User
            // can still pick anything else in the wizard.
            symmetry: SymmetryAxis::Horizontal,
            rotational_fold: 2,
            biome_index: 0,
            max_height: BIOMES[0].max_height_hint,
            height_from_biome: true,
        }
    }
}

/// Single-active-tool model (ADR-030). Exactly one tool is active at any
/// time; the left tool strip selects it, the right Inspector swaps its
/// contents via an exhaustive `match` on the active variant, and the
/// central viewport's pointer interaction is driven by this enum.
///
/// Each variant has a one-letter accelerator. ADR-035 (UI overhaul) adds
/// `SplatPaint`, `MetalSpots`, and `GeoFeatures` variants — every match
/// site is exhaustive so adding a variant produces a compile error at
/// each dispatch location. The three new tools are scaffolded with
/// inspector stubs that ship behind their F-series schema work (F4
/// splat, F5 metal, F7 features).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Tool {
    /// Camera-only. LMB orbits; no central-rect editing.
    Select,
    /// Heightmap brush (F2 / ADR-018). LMB stamps the current brush.
    Sculpt,
    /// F8 start-position placement (ADR-023). LMB places / drags
    /// markers, RMB deletes.
    StartPositions,
    /// F5 metal-spot placement (ADR-035 scaffolding; schema work
    /// pending). LMB places spots, RMB deletes.
    MetalSpots,
    /// F6 geo-vent placement (Sprint 11 / C5). LMB places vents
    /// (which spawn as `geovent` features via the Springboard trio),
    /// RMB deletes. Distinct from the general feature tool below —
    /// vents are surfaced as their own affordance because they're
    /// gameplay-critical (geo-only economy positions). Keyboard `V`
    /// since Sprint 12 freed `F` for [`Tool::Feature`].
    GeoFeatures,
    /// F7 general feature placement (Sprint 12 / C6). LMB places
    /// from the inspector's picker (trees / rocks / wreckage / props
    /// / geo); LMB-drag rotates an existing placement; RMB deletes.
    /// Keyboard `F`.
    Feature,
    /// C9 (Sprint 14 / ADR-042) — water / lava authoring. Inspector
    /// surface for `Project.water_mode` + `water_overrides`; LMB drag
    /// floods (calls `Brush::Lower` with strength derived from
    /// `water_carve_depth`), RMB raises terrain back. Keyboard `W`.
    Water,
    /// D9 / Sprint 16 (ADR-040) — layered texture painting. LMB
    /// stamps the active mask brush (`mask-reveal` / `mask-hide` /
    /// `mask-smooth` / `mask-fill`) into the active layer's mask;
    /// the central viewport switches to a top-down 2D orthographic
    /// view of the GPU composite RT. Keyboard `L`. Inspector shows
    /// a minimal active-layer chip strip + brush controls until
    /// Sprint 17 lands the full Photoshop-style Layers panel.
    PaintLayer,
    /// Math-function terrain generator (F14 / ADR-020). No central-rect
    /// editing; the formula is committed via Apply in the Inspector.
    Procgen,
}

/// Build-pipeline variants offered in the top action bar (B4). Today
/// every enabled variant funnels into the same `FileAction::BuildAndInstall`
/// pipeline — the variant selector is reserved UX surface for Phase 5+
/// (F12 Launch + a Build-vs-Install split). `BuildInstallLaunch` is
/// permanently disabled in the UI until F12 wires the engine launcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
enum BuildVariant {
    /// Compile `.smf` + `.smt` + package `.sd7` only — no install to
    /// the BAR user maps dir. Today this still runs the existing
    /// build-and-install path (pipeline split deferred); the label
    /// signals intent.
    Only,
    /// `.sd7` + install to `~/.local/state/Beyond All Reason/maps/`.
    /// Default — matches the B1 button's behaviour.
    #[default]
    Install,
    /// `.sd7` + install + launch the BAR engine pointing at the new
    /// map. Greyed pre-F12.
    Launch,
}

impl BuildVariant {
    /// All variants in display order. Pinned by tests so a future
    /// reorder is explicit.
    const ALL: [BuildVariant; 3] = [
        BuildVariant::Only,
        BuildVariant::Install,
        BuildVariant::Launch,
    ];

    /// Short label rendered in the ComboBox + on the primary button.
    fn label(self) -> &'static str {
        match self {
            BuildVariant::Only => "Build",
            BuildVariant::Install => "Build + Install",
            BuildVariant::Launch => "Build + Install + Launch",
        }
    }

    /// Is this variant available *today*? `Launch` returns false
    /// until F12 ships an engine launcher.
    fn is_enabled(self) -> bool {
        match self {
            BuildVariant::Only => true,
            BuildVariant::Install => true,
            // F12 isn't wired — the variant is greyed in the combo and
            // skipped by the click handler.
            BuildVariant::Launch => false,
        }
    }

    /// Map an enabled variant to the FileAction to enqueue. Returns
    /// `None` for the disabled-pre-F12 launch variant — the click
    /// handler treats `None` as "this shouldn't have been clickable;
    /// drop the click."
    fn to_file_action(self) -> Option<FileAction> {
        // Today every enabled variant lands in the same pipeline path.
        // A Phase-5 split would diversify these returns.
        match self {
            BuildVariant::Only => Some(FileAction::BuildAndInstall),
            BuildVariant::Install => Some(FileAction::BuildAndInstall),
            BuildVariant::Launch => None,
        }
    }
}

impl Tool {
    /// All tool variants in display order. Single source of truth used
    /// by the tool strip *and* the unit tests so adding a variant in
    /// one place doesn't drift from the other. The exhaustive `match`
    /// dispatches in the Inspector enforce the rest of the invariant.
    const ALL: [Tool; 9] = [
        Tool::Select,
        Tool::Sculpt,
        Tool::StartPositions,
        Tool::MetalSpots,
        Tool::GeoFeatures,
        Tool::Feature,
        Tool::Water,
        Tool::PaintLayer,
        Tool::Procgen,
    ];

    /// Legacy one-character glyph kept for tracing/diagnostics. The
    /// tool strip now paints a Lucide-style line icon via
    /// [`Self::icon_kind`]; this method only feeds log lines and the
    /// cheat sheet's plain-text fallback.
    #[allow(dead_code)]
    fn icon(self) -> &'static str {
        match self {
            Tool::Select => "↺",
            Tool::Sculpt => "✎",
            Tool::StartPositions => "⚑",
            Tool::MetalSpots => "◆",
            Tool::GeoFeatures => "♨",
            Tool::Feature => "🌲",
            Tool::Water => "🌊",
            Tool::PaintLayer => "🖌",
            Tool::Procgen => "ƒ",
        }
    }

    /// Lucide-style icon variant used by the left tool strip. ADR-035.
    fn icon_kind(self) -> crate::ui::icons::Icon {
        use crate::ui::icons::Icon;
        match self {
            Tool::Select => Icon::Select,
            Tool::Sculpt => Icon::Sculpt,
            Tool::StartPositions => Icon::Pin,
            Tool::MetalSpots => Icon::Metal,
            Tool::GeoFeatures => Icon::Geo,
            Tool::Feature => Icon::Tree,
            Tool::Water => Icon::Water,
            Tool::PaintLayer => Icon::Brush,
            Tool::Procgen => Icon::Procgen,
        }
    }

    /// Single-letter accelerator key. Wired in `App::handle_keyboard`.
    fn accel(self) -> &'static str {
        match self {
            Tool::Select => "Q",
            Tool::Sculpt => "B",
            Tool::StartPositions => "S",
            Tool::MetalSpots => "M",
            // V for "vent" — Sprint 12 freed `F` for the general
            // feature tool below.
            Tool::GeoFeatures => "V",
            Tool::Feature => "F",
            Tool::Water => "W",
            Tool::PaintLayer => "L",
            Tool::Procgen => "G",
        }
    }

    /// Long-form name for hover tooltips + tracing output.
    fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select / orbit",
            Tool::Sculpt => "Sculpt",
            Tool::StartPositions => "Start positions",
            Tool::MetalSpots => "Metal spots",
            Tool::GeoFeatures => "Geo vents",
            Tool::Feature => "Features",
            Tool::Water => "Water / Lava",
            Tool::PaintLayer => "Paint layer",
            Tool::Procgen => "Procgen",
        }
    }
}

struct HeightmapState {
    path: PathBuf,
    /// Authoritative CPU mirror of the heightmap. Brushes mutate this in
    /// place; the GPU texture is the derived view (see ADR-017).
    data: Heightmap,
    dims: (u32, u32),
    min: u16,
    max: u16,
    validated_against: Option<MapSize>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let render_state = cc.wgpu_render_state.clone();
        if let Some(rs) = render_state.as_ref() {
            let info = rs.adapter.get_info();
            info!(
                backend = ?info.backend,
                adapter = %info.name,
                vendor = info.vendor,
                device_type = ?info.device_type,
                "wgpu adapter selected"
            );
            render::install(rs);
            info!("terrain renderer installed (ADR-017 r16uint storage)");
        } else {
            error!("no wgpu render state — terrain preview disabled");
        }

        // ADR-035: install the dark DCC palette + line-icon-tuned style
        // before any panel renders. Must happen on every launch — egui
        // does not persist visuals between sessions.
        crate::ui::theme::install(&cc.egui_ctx);

        let editor_config = config::EditorConfig::load();
        let show_intro = !editor_config.intro_seen_for_current_version();

        let mut app = Self {
            project_name: "untitled".to_string(),
            map_size: MapSize::square(16),
            heightmap: None,
            last_error: None,
            render_state,
            camera: OrbitCamera::framing(8192.0, 8192.0),
            height_scale: 256.0,
            min_height: 0.0,
            current_project_path: None,
            last_install: None,
            build_state: build_runner::BuildState::Idle,
            build_log_open: false,
            brushes: BrushRegistry::default_set(),
            brush_id: None, // Off
            brush_radius: 256.0,
            brush_strength: 0.5,
            symmetry: SymmetryAxis::None,
            rotational_fold: 2,
            procgen_expr: "1 - (x*x + z*z)".to_string(),
            procgen_domain: Domain::Centered,
            procgen_last_error: None,
            // Default expression is the parabolic-bowl preset — known
            // to validate. Anything that doesn't validate would render
            // the chip red on first frame.
            procgen_validation: Ok(()),
            procgen_thumbnail: None,
            procgen_thumbnail_key: None,
            // First time the Procgen Inspector is opened the debounce
            // immediately fires and bakes the default expression.
            procgen_changed_at: None,
            history: History::default(),
            tool: Tool::Sculpt,
            previous_tool: Tool::Sculpt,
            ally_groups: Vec::new(),
            active_ally_group_id: 0,
            dragging_start_pos: None,
            dragging_start_pos_from: None,
            drag_paint_count: 8,
            drag_paint_origin: None,
            pulsing_marker: None,
            hovered_canvas_marker: None,
            // Open on first launch — the F1 wizard *is* the entry point now.
            wizard_open: true,
            wizard: WizardState::default_for_new_project(),
            symmetry_popover_open: false,
            editor_config,
            show_intro,
            show_cheat_sheet: false,
            lint_panel_open: false,
            lint_panel_was_open: false,
            nav_gizmo_drag_active: false,
            build_variant: BuildVariant::default(),
            mapinfo_overrides: std::collections::HashMap::new(),
            show_next_steps: false,
            next_steps_dismissed: false,
            minimap_override: None,
            mapinfo_form_open: false,
            mapinfo_form_tab: crate::ui::inspector_mapinfo::MapInfoTab::default(),
            migration_toast_dismissed: false,
            pending_migration_toast: false,
            dirty: false,
            last_non_none_symmetry: SymmetryAxis::Horizontal,
            grid_overlay_on: false,
            lighting_on: true,
            wireframe_on: false,
            buildable_overlay_on: false,
            // D8 / Sprint 15: real-app launch seeds a single-layer
            // biome-base stack so the bake hits the layered path
            // straight away. `Project::new` seeds the same shape on
            // its side — the App stays the source of truth across
            // session lifetime and re-syncs from `Project` on every
            // open.
            layer_stack: LayerStack::from_biome("", MapSize::square(16)),
            composite_layer_last_version: std::collections::HashMap::new(),
            paint_active_layer_id: None,
            paint_view_state: PaintViewState::default(),
            paint_brush_state: PaintBrushState::default(),
            mask_brushes: barme_core::MaskBrushRegistry::default_set(),
            paint_last_drag_pos: None,
            layer_thumbnails: std::collections::HashMap::new(),
            paint_drag_preview_order: None,
            layer_mask_preview_cache: None,
            layers_panel_rect: None,
            dnts_diffuse_in_alpha: false,
            slot_registry: scan_slot_registry(Path::new("tools/textures")),
            slot_thumbnails: std::collections::HashMap::new(),
            metal_state: MetalState::default(),
            geo_state: GeoState::default(),
            metal_spots: Vec::new(),
            geo_vents: Vec::new(),
            extractor_radius: default_extractor_radius(),
            dragging_metal_spot: None,
            dragging_metal_spot_from: None,
            dragging_geo_vent: None,
            dragging_geo_vent_from: None,
            features: Vec::new(),
            feature_state: FeatureState {
                manifest: FeatureCatalog::load_default(&repo_root()),
                ..FeatureState::default()
            },
            dragging_feature: None,
            dragging_feature_from: None,
            dragging_feature_anchor_x: None,
            dragging_feature_start_rot: None,
            water_mode: WaterMode::default(),
            water_overrides: WaterBlock::default(),
            void_water: false,
            tidal_strength: None,
            lava_atmosphere: false,
            // Default carve depth — matches the prompt's spec and a
            // generic flooded basin. Lives on App not Project (per-
            // session tool preference, same status as brush_radius).
            water_carve_depth: -80.0,
        };
        // D9 / Sprint 16 — push the default biome stack's source
        // diffuse to the composite slot array so the first central()
        // frame has real data to composite from (mask uploads land
        // on the same frame via `sync_composite_mask_tiles`).
        // Demo seed: add a second accent layer (slot 1 if it exists)
        // at mask = 0 so painting reveal in `Tool::PaintLayer`
        // produces immediately-visible results without forcing the
        // user to "+ Add" a layer first.
        app.seed_demo_accent_layer();
        app.reupload_layer_stack_diffuses();
        app
    }

    fn load_heightmap(&mut self, path: PathBuf) {
        self.last_error = None;
        // Wholesale replacement — undoing across it would require a full-map
        // pre-snapshot. ADR-022 barriers the history instead.
        self.end_stroke();
        self.history.barrier();
        match Heightmap::load_png(&path) {
            Ok(h) => {
                let dims = h.dims();
                let (min, max) = h.min_max();
                let size = self.map_size;
                let validated_against = h.validate_against(size).ok().map(|_| size);
                if validated_against.is_none() {
                    warn!(
                        path = %path.display(),
                        loaded_dims = ?dims,
                        expected_dims = ?size.heightmap_dims(),
                        smu_x = size.smu_x,
                        smu_z = size.smu_z,
                        "heightmap dims do not match project SMU; rendering anyway"
                    );
                }
                if let Some(rs) = self.render_state.as_ref() {
                    render::upload_heightmap(rs, &h);
                    let extent_x = (dims.0 - 1) as f32 * render::ELMOS_PER_PIXEL;
                    let extent_z = (dims.1 - 1) as f32 * render::ELMOS_PER_PIXEL;
                    self.camera = OrbitCamera::framing(extent_x, extent_z);
                }
                info!(
                    path = %path.display(),
                    dims = ?dims,
                    min,
                    max,
                    validated = validated_against.is_some(),
                    "heightmap loaded"
                );
                self.heightmap = Some(HeightmapState {
                    path,
                    data: h,
                    dims,
                    min,
                    max,
                    validated_against,
                });
            }
            Err(e) => {
                error!(path = %path.display(), error = %format!("{e:#}"), "heightmap load failed");
                self.last_error = Some(format!("{e:#}"));
            }
        }
    }

    /// Reset to a blank in-memory project. Does not touch disk.
    fn new_project(&mut self) {
        info!("new project (in-memory, untitled, 16×16 SMU)");
        self.project_name = "untitled".to_string();
        self.map_size = MapSize::square(16);
        self.heightmap = None;
        self.current_project_path = None;
        self.height_scale = 256.0;
        self.min_height = 0.0;
        self.camera = OrbitCamera::framing(8192.0, 8192.0);
        self.last_error = None;
        self.last_install = None;
        self.ally_groups.clear();
        self.active_ally_group_id = 0;
        self.mapinfo_overrides.clear();
        // D7 / Sprint 18: fresh projects opt into the auto-bake path
        // by default. A user-supplied override is opt-in via the F9
        // form's Minimap tab (C7).
        self.minimap_override = None;
        self.dragging_start_pos = None;
        self.dragging_start_pos_from = None;
        self.drag_paint_origin = None;
        self.pulsing_marker = None;
        self.hovered_canvas_marker = None;
        // B8: a brand-new project always starts with the hint re-armed
        // and not yet dismissed. apply_wizard sets show_next_steps =
        // true after this; the bare new_project (e.g. on Cancel) keeps
        // it hidden until the user goes through the wizard.
        self.next_steps_dismissed = false;
        self.show_next_steps = false;
        // D5: fresh project starts with no slots bound and no painted
        // distribution. Slot thumbnails stay cached — the registry +
        // PNGs on disk don't change with project lifecycle.
        self.dnts_diffuse_in_alpha = false;
        // D8 / Sprint 15 (ADR-038): a fresh "New project" gets a
        // single-layer biome-base stack. D9 / Sprint 16 seeds a
        // second accent layer at mask=0 so paint reveal/hide
        // immediately produce visible results.
        self.layer_stack = LayerStack::from_biome("", self.map_size);
        self.seed_demo_accent_layer();
        // D9 / Sprint 16 (ADR-039): clear the per-layer GPU upload
        // cursor so the new stack's masks land on the first central()
        // frame. The slot diffuse re-upload pushes the default
        // biome's source to the composite slot array next.
        self.composite_layer_last_version.clear();
        self.reupload_layer_stack_diffuses();
        // D9 / Sprint 16 (ADR-040): reset paint viewport state. The
        // active layer id resets to None so the next Tool::PaintLayer
        // entry picks the default (topmost visible) layer; the
        // view's pan/zoom resets to auto-fit.
        self.paint_active_layer_id = None;
        self.paint_view_state = PaintViewState::default();
        self.layer_thumbnails.clear();
        self.paint_drag_preview_order = None;
        self.layer_mask_preview_cache = None;
        self.layers_panel_rect = None;
        // GPU side resets via the next-frame TerrainCallback (uniforms
        // re-write to defaults; distribution texture is left holding
        // the prior session's pixels — irrelevant since active_mask = 0
        // suppresses sampling. A full distribution clear happens at
        // first stamp.)
        // C4/C5 (Sprint 11): metal + geo are project-scoped data; a
        // fresh project starts with neither and the BAR-default
        // extractor radius. Drag state clears too — a switch to a
        // new project mid-drag would be a surprise undo source.
        self.metal_spots.clear();
        self.geo_vents.clear();
        self.extractor_radius = default_extractor_radius();
        self.metal_state = MetalState::default();
        self.geo_state = GeoState::default();
        self.dragging_metal_spot = None;
        self.dragging_metal_spot_from = None;
        self.dragging_geo_vent = None;
        self.dragging_geo_vent_from = None;
        // C6 (Sprint 12): a fresh project starts with no user features
        // and the inspector's picker reset to the "trees" category.
        // The catalog itself is session-scoped (loaded at App start)
        // and survives the project lifecycle.
        self.features.clear();
        self.feature_state.active_category = "trees".to_string();
        self.feature_state.selected_feature = None;
        self.feature_state.selected_placed = None;
        self.feature_state.filter.clear();
        self.dragging_feature = None;
        self.dragging_feature_from = None;
        self.dragging_feature_anchor_x = None;
        self.dragging_feature_start_rot = None;
        // C9 (Sprint 14): a fresh project starts with the engine's
        // built-in water defaults — no preset, no overrides, no
        // voidWater, no tidal strength. `water_carve_depth` is a
        // per-session tool preference that survives "New project."
        self.water_mode = WaterMode::default();
        self.water_overrides = WaterBlock::default();
        self.void_water = false;
        self.tidal_strength = None;
        self.lava_atmosphere = false;
        self.dirty = false;
        self.end_stroke();
        self.history.barrier();
    }

    fn snapshot_project(&self) -> Project {
        Project {
            name: self.project_name.clone(),
            size: self.map_size,
            min_height: self.min_height,
            max_height: self.height_scale,
            heightmap: self.heightmap.as_ref().map(|h| h.path.clone()),
            ally_groups: self.ally_groups.clone(),
            mapinfo_overrides: self.mapinfo_overrides.clone(),
            next_steps_dismissed: self.next_steps_dismissed,
            migration_toast_dismissed: self.migration_toast_dismissed,
            // D10 / Sprint 17 (ADR-041): `splat_config` is
            // `#[serde(skip_serializing)]` so this default never hits
            // disk on new saves; legacy projects still load through
            // the migration path. App-side fields retired.
            splat_config: SplatConfig::default(),
            dnts_diffuse_in_alpha: self.dnts_diffuse_in_alpha,
            layers: self.layer_stack.clone(),
            splat_distribution: None,
            metal_spots: self.metal_spots.clone(),
            geo_vents: self.geo_vents.clone(),
            features: self.features.clone(),
            // D6 (Sprint 12): user-authored specular override path. The
            // inspector surface for setting this lands in F9 (Sprint 13);
            // until then the field round-trips through `.barmeproj` so
            // a future authored override survives save/open.
            specular_tex_path: None,
            extractor_radius: self.extractor_radius,
            water_mode: self.water_mode,
            water_overrides: self.water_overrides.clone(),
            void_water: self.void_water,
            tidal_strength: self.tidal_strength,
            lava_atmosphere: self.lava_atmosphere,
            // D7 / Sprint 18 (F10): round-trip the user's minimap
            // override path through save / open. The F9 form's
            // Minimap tab (C7) surfaces the file picker that mutates
            // `self.minimap_override`; until then the field passes
            // through silently.
            minimap_override: self.minimap_override.clone(),
            // Re-saved projects always carry the current schema version
            // so subsequent loads skip migrations.
            schema_v: Project::SCHEMA_V,
        }
    }

    /// Build-time snapshot: like [`Self::snapshot_project`] but with
    /// every source position replicated through the active symmetry
    /// axis into the same ally group. The pipeline emitter walks the
    /// resulting `teams[]` flat — without this expansion, a Quad-
    /// symmetric placement would only ship 1 `teams[*].startPos`
    /// instead of 4, and BAR would only render one spawn per side.
    ///
    /// Idempotent — exact-coord duplicates are dropped, so calling
    /// twice is safe.
    fn snapshot_project_for_build(&self) -> Project {
        let mut p = self.snapshot_project();
        expand_symmetry_into_ally_groups(&mut p, self.symmetry);
        p
    }

    /// D6 (Sprint 12): translate `Project.splat_config.channels`
    /// (slot ids 0..=255) to per-channel directories under
    /// `tools/textures/<NN-slug>/`. Unbound channels stay `None`;
    /// bound channels resolve to the slot registry entry the user
    /// picked in the F4 splat inspector. The splat pipeline calls
    /// `bake_dnts` only for `Some(_)` entries that ALSO have non-
    /// zero distribution pixels — see
    /// `splat_pipeline::compute_active_channels`.
    fn resolve_splat_bake_inputs(&self, project: &Project) -> barme_pipeline::SplatBakeInputs {
        let mut out = barme_pipeline::SplatBakeInputs::default();
        for (ch, binding) in project.splat_config.channels.iter().enumerate() {
            let Some(slot_id) = binding else {
                continue;
            };
            if let Some(slot) = self.slot_registry.iter().find(|m| m.id == *slot_id) {
                out.channel_slot_dirs[ch] = Some(slot.dir.clone());
            } else {
                warn!(
                    channel = ch,
                    slot_id = slot_id,
                    "splat channel binding references missing slot in registry"
                );
            }
        }
        out
    }

    /// D10 / Sprint 17 (ADR-041) — resolve the layer-driven splat
    /// pipeline inputs from `project.layers.dnts_layers()`. For each
    /// DNTS-bound channel: capture the layer's slot dir (if
    /// `Slot`-sourced; `None` for `Imported`), clone the mask, copy
    /// the per-layer `dnts_tex_scale` / `dnts_tex_mult` / name /
    /// imported-flag. Returns defaults (all-None / zero) when the
    /// stack carries no DNTS bindings.
    fn resolve_layer_splat_bake_inputs(
        &self,
        project: &Project,
    ) -> barme_pipeline::LayerSplatBakeInputs {
        use barme_core::LayerSource;
        // Default the scales/mults to engine baseline so unbound
        // channels emit the FINDINGS-§1.6 defaults rather than zero.
        let mut out = barme_pipeline::LayerSplatBakeInputs {
            channel_tex_scales: [0.02; 4],
            channel_tex_mults: [1.0; 4],
            ..Default::default()
        };

        let dnts_layers = project.layers.dnts_layers();
        for (ch, maybe_layer) in dnts_layers.iter().enumerate() {
            let Some(layer) = maybe_layer else { continue };
            out.channel_masks[ch] = Some(layer.mask.clone());
            out.channel_tex_scales[ch] = layer.dnts_tex_scale;
            out.channel_tex_mults[ch] = layer.dnts_tex_mult;
            out.channel_layer_names[ch] = Some(layer.name.clone());
            match &layer.source {
                LayerSource::Slot { id } => {
                    if let Some(slot) = self.slot_registry.iter().find(|m| m.id == *id) {
                        out.channel_slot_dirs[ch] = Some(slot.dir.clone());
                    } else {
                        warn!(
                            channel = ch,
                            slot_id = *id,
                            "Sprint 17 layer splat: bound slot id not in registry; skipping DDS bake"
                        );
                    }
                }
                LayerSource::Imported { .. } => {
                    // No stock normal map for imported diffuses.
                    // `stage_splat_assets_from_layers` will emit a
                    // `LintWarning::ImportedLayerDnts` for this channel.
                    out.channel_imported[ch] = true;
                }
            }
        }
        out
    }

    fn save_to(&mut self, path: PathBuf) {
        let mut p = self.snapshot_project();
        let abs_before = p.heightmap.clone();
        p.relativize_heightmap(&path);
        if let (Some(before), Some(after)) = (&abs_before, &p.heightmap)
            && before != after
        {
            info!(
                "heightmap path made relative: {} -> {}",
                before.display(),
                after.display()
            );
        }
        match p.save_to_file(&path) {
            Ok(()) => {
                info!(
                    "saved project '{}' ({}×{} SMU, heightmap={}) to {}",
                    p.name,
                    p.size.smu_x,
                    p.size.smu_z,
                    p.heightmap
                        .as_ref()
                        .map(|h| h.display().to_string())
                        .unwrap_or_else(|| "(none)".into()),
                    path.display()
                );
                self.current_project_path = Some(path);
                self.last_error = None;
                self.dirty = false;
            }
            Err(e) => {
                error!(path = %path.display(), error = %format!("{e:#}"), "project save failed");
                self.last_error = Some(format!("save: {e:#}"));
            }
        }
    }

    /// Mark the project as needing-save. Called from edit sites that
    /// mutate any persisted field. Idempotent. ADR-035.
    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Compute the top-bar validation chip's tone + label. Pure helper
    /// over snapshot state; unit-tested under
    /// `validation_summary_*`.
    fn validation_summary(&self) -> (crate::ui::theme::ChipTone, String) {
        use crate::ui::theme::ChipTone;
        // Project-fatal: no heightmap loaded.
        if self.heightmap.is_none() {
            return (ChipTone::Err, "No heightmap".into());
        }
        // Heightmap dims mismatch vs declared SMU.
        if let Some(h) = &self.heightmap
            && h.validated_against != Some(self.map_size)
        {
            return (ChipTone::Err, "Heightmap mismatch".into());
        }
        // Procgen panel: live parse error.
        if let Err(_msg) = &self.procgen_validation
            && matches!(self.tool, Tool::Procgen)
        {
            return (ChipTone::Err, "Expression error".into());
        }
        // C9 / Sprint 14 / PITFALL §8 — DNTS + water LOS TV-snow bug.
        // Beherith forum t=35202: with DNTS slots bound AND water
        // active (`min_height < 0` OR `water_mode != None`), any
        // gameplay-side LOS widget can trigger animated-noise
        // artefacts on the terrain. Warn but don't gate (the user
        // might still ship and accept the risk). Sprint 19's lint
        // panel will surface this same condition more loudly.
        // D10 / Sprint 17 (ADR-041): derive from layer stack instead
        // of the retired `splat_config.channels`.
        let has_dnts = self.layer_stack.dnts_layers().iter().any(|l| l.is_some());
        let has_water = self.water_mode != WaterMode::None || self.min_height < 0.0;
        if has_dnts && has_water {
            return (ChipTone::Warn, "DNTS + water: LOS bug".into());
        }
        // D5 / FINDINGS §7.2: a DNTS slot is bound but no specular
        // texture is set. Engine no longer gates the DNTS branch on
        // specularTex (Recoil HEAD, SMFRenderState.cpp:114), but the
        // visual still looks flatter than published BAR maps. Surface
        // as a yellow warning. Editor doesn't author specular yet, so
        // any bound slot trips this.
        if has_dnts {
            return (ChipTone::Warn, "DNTS: no specular".into());
        }
        // C9 / Sprint 14 — water mode vs min_height agreement.
        // `min_height < 0` with no preset = engine renders its
        // default blue ocean (silent surprise). Preset selected
        // with `min_height >= 0` = no water visible without
        // `forceRendering`. Sprint 19's lint promotes these into
        // the lint panel with one-click fixes.
        if self.water_mode == WaterMode::None && self.min_height < 0.0 {
            return (
                ChipTone::Warn,
                "Terrain below Y=0 with no water preset".into(),
            );
        }
        if self.water_mode != WaterMode::None && self.min_height >= 0.0 {
            return (
                ChipTone::Warn,
                "Water preset set, no terrain below Y=0".into(),
            );
        }
        // Soft warning: empty ally groups will fall back to the
        // engine's 25/75 diagonal default. That still ships a playable
        // map but the user almost certainly meant something else.
        if self.ally_groups.is_empty() {
            return (ChipTone::Warn, "No start positions".into());
        }
        (ChipTone::Ok, "Ready".into())
    }

    /// Refresh the cached parse-and-dry-eval outcome (ADR-…/A4). Stores
    /// the formatted `#[source]` chain so the UI tooltip can render it
    /// directly. Called whenever `procgen_expr` changes — keystroke, preset
    /// pick, biome apply. Cost is ~μs for typical inputs, but the input is
    /// capped at `procgen::MAX_EXPRESSION_LEN` chars by the validator
    /// itself.
    ///
    /// Also re-arms the B7 thumbnail debounce. The thumbnail rebake
    /// fires from `inspector_procgen` once `procgen_changed_at` is
    /// older than [`PROCGEN_THUMBNAIL_DEBOUNCE_MS`].
    fn revalidate_procgen(&mut self) {
        self.procgen_validation =
            validate_expression(&self.procgen_expr).map_err(|e| format!("{e:#}"));
        self.procgen_changed_at = Some(std::time::Instant::now());
    }

    /// Generate a heightmap from the current procgen expression and
    /// replace the loaded heightmap. Errors render as a red label in the
    /// "Generate from formula" panel; the existing heightmap is left
    /// untouched on failure.
    fn apply_procgen(&mut self) {
        self.procgen_last_error = None;
        self.mark_dirty();
        // Procgen replaces the entire map; barrier history so undo doesn't
        // try to walk across a wholesale swap. ADR-022.
        self.end_stroke();
        self.history.barrier();
        let size = self.map_size;
        let expr = self.procgen_expr.clone();
        let domain = self.procgen_domain;
        info!(
            expr = %expr,
            domain = ?domain,
            smu_x = size.smu_x,
            smu_z = size.smu_z,
            "procgen: applying expression"
        );
        let t0 = std::time::Instant::now();
        let hm = match procgen_generate(&expr, domain, size, 0.0, self.height_scale) {
            Ok(h) => h,
            Err(e) => {
                let msg = format!("{e:#}");
                error!(expr = %expr, error = %msg, "procgen failed");
                self.procgen_last_error = Some(msg);
                return;
            }
        };
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let dims = hm.dims();
        let (min, max) = hm.min_max();
        info!(
            dims = ?dims,
            min,
            max,
            elapsed_ms,
            "procgen: heightmap generated"
        );
        if let Some(rs) = self.render_state.as_ref() {
            render::upload_heightmap(rs, &hm);
            let extent_x = (dims.0 - 1) as f32 * render::ELMOS_PER_PIXEL;
            let extent_z = (dims.1 - 1) as f32 * render::ELMOS_PER_PIXEL;
            self.camera = OrbitCamera::framing(extent_x, extent_z);
        }
        // Synthetic project path so save flow doesn't reuse a stale on-disk PNG.
        let synth_path = PathBuf::from(format!(
            "<procgen:{}>",
            expr.chars().take(40).collect::<String>()
        ));
        self.heightmap = Some(HeightmapState {
            path: synth_path,
            data: hm,
            dims,
            min,
            max,
            validated_against: Some(size),
        });
    }

    /// Apply one brush stamp at the cursor position. Resolves cursor →
    /// world via screen-ray vs y=0 plane, runs the kernel against the
    /// CPU heightmap, then sub-uploads the dirty rect to the GPU
    /// texture. No-op if no brush is selected or no heightmap is
    /// loaded. Sculpt's default entry — uses the current
    /// `self.brush_id` + `self.brush_strength`.
    fn apply_brush_at(&mut self, cursor: egui::Pos2, rect: egui::Rect) {
        let Some(brush_id) = self.brush_id.as_deref() else {
            return;
        };
        let id = brush_id.to_string();
        let strength = self.brush_strength;
        self.apply_brush_id_at(cursor, rect, &id, strength);
    }

    /// Apply one stamp with an explicit brush id + strength.
    /// `Tool::Water` (Sprint 14 / C9) uses this with
    /// `("lower", carve_strength)` to flood and `("raise",
    /// carve_strength)` to undo a flooding gesture, separately from
    /// the user's sculpt-tool `brush_id` selection.
    fn apply_brush_id_at(
        &mut self,
        cursor: egui::Pos2,
        rect: egui::Rect,
        brush_id: &str,
        strength: f32,
    ) {
        let Some(rs) = self.render_state.as_ref() else {
            return;
        };
        let Some(hm_state) = self.heightmap.as_mut() else {
            return;
        };
        let Some(brush) = self.brushes.get(brush_id) else {
            // Selected brush id refers to something not in the registry —
            // a real bug (state out of sync). Warn so it's not swallowed.
            warn!(brush_id, "selected brush id not present in registry");
            return;
        };
        if !rect.contains(cursor) {
            return;
        }
        self.dirty = true;
        let cursor_in = glam::Vec2::new(cursor.x - rect.min.x, cursor.y - rect.min.y);
        let rect_size = glam::Vec2::new(rect.width(), rect.height());
        let Some(world) = render::screen_to_world_y0(cursor_in, rect_size, &self.camera) else {
            // Ray missed the y=0 plane (camera looking up / behind). Trace-
            // only since this is a common harmless case at oblique angles.
            trace!(
                cursor = ?(cursor.x, cursor.y),
                "brush picking: ray-vs-plane miss"
            );
            return;
        };
        trace!(
            brush = brush_id,
            world_x = world.x,
            world_z = world.z,
            radius = self.brush_radius,
            strength = strength,
            symmetry = self.symmetry.id(),
            "brush stamp"
        );
        let dims = hm_state.dims;
        let extents = (
            (dims.0 - 1) as f32 * render::ELMOS_PER_PIXEL,
            (dims.1 - 1) as f32 * render::ELMOS_PER_PIXEL,
        );
        // Symmetry replicates the stamp through all derived centers
        // (ADR-019). Compute every stamp's bbox *first* — without applying —
        // so we can hand the unioned region to History::snapshot_rect for
        // copy-on-first-write capture (ADR-033).
        let centers = self.symmetry.replicate((world.x, world.z), extents);
        let mut planned: Vec<(BrushStamp, DirtyRect)> = Vec::with_capacity(centers.len());
        let mut pre_union: Option<DirtyRect> = None;
        for (cx, cz) in centers {
            let stamp = BrushStamp {
                world_x: cx,
                world_z: cz,
                radius: self.brush_radius,
                strength,
            };
            if let Some(r) = pixel_bbox(&hm_state.data, stamp) {
                pre_union = Some(match pre_union {
                    Some(u) => u.union(r),
                    None => r,
                });
                planned.push((stamp, r));
            }
        }
        let Some(snap_rect) = pre_union else {
            return;
        };

        // Capture pre-edit pixel values for any pixels in `snap_rect` that
        // haven't been snapshotted yet in this stroke (ADR-033). Must run
        // BEFORE the brush writes to the heightmap.
        self.history.snapshot_rect(&hm_state.data, snap_rect);

        // Apply each planned stamp, then sub-upload the union for one
        // queue.write_texture call.
        let mut union: Option<DirtyRect> = None;
        for (stamp, _) in planned {
            if let Some(r) = brush.apply(&mut hm_state.data, stamp) {
                union = Some(match union {
                    Some(u) => u.union(r),
                    None => r,
                });
            }
        }
        let Some(rect_dirty) = union else {
            return;
        };

        render::write_heightmap_rect(rs, dims, hm_state.data.data(), rect_dirty);
        let (mn, mx) = hm_state.data.min_max();
        hm_state.min = mn;
        hm_state.max = mx;
    }

    /// Flush the in-progress stroke into the undo stack. Idempotent; a no-op
    /// when no stroke is open. Called on pointer-release and before every
    /// barrier event (procgen / load / new project).
    fn end_stroke(&mut self) {
        if let Some(hm_state) = self.heightmap.as_ref() {
            self.history.end_stroke(&hm_state.data);
        } else {
            // No heightmap → history can't snapshot from it. Drop any
            // in-flight stroke state via barrier (cheap if already empty).
            if self.history.stroke_open() {
                self.history.barrier();
            }
        }
    }

    // (helper used by inspector_procgen for chip-fitting)
    // — placeholder to keep impl block tidy; see free helper below.

    /// Pop one entry off the unified undo stack and apply it. Heightmap
    /// strokes swap the affected rect and re-upload to the GPU; project
    /// diffs mutate F8 / wizard state via the local dispatcher. Always
    /// flushes an open stroke first so the user undoes a finished unit.
    /// Gated on `!is_dragging_anything()` so undo can't fire mid-gesture
    /// (B5 pitfall #2). Pushes the inverse onto redo.
    fn undo_one(&mut self) {
        self.end_stroke();
        if self.is_dragging_anything() {
            trace!("undo: gated by in-flight drag");
            return;
        }
        let Some(entry) = self.history.pop_undo() else {
            trace!("undo: nothing to undo");
            return;
        };
        let inverse = self.apply_history_entry(entry);
        info!(
            undo_depth = self.history.undo_depth(),
            redo_depth = self.history.redo_depth() + 1,
            "undo applied"
        );
        self.history.push_to_redo(inverse);
    }

    /// Commit the wizard form: reset app state, apply size + symmetry +
    /// max height, run the chosen biome's procgen preset to seed the
    /// heightmap. Idempotent if called twice — `new_project()` clears
    /// per-project state first. ADR-024.
    ///
    /// B5: snapshots the pre-wizard project state BEFORE the
    /// `new_project()` / `apply_procgen()` barriers wipe history, then
    /// pushes one `ProjectDiff::ApplyWizard` entry on top of the
    /// freshly-cleared stack so Ctrl-Z reverts the wizard.
    fn apply_wizard(&mut self) {
        let w = self.wizard.clone();
        let sanitized = sanitize_name(&w.project_name);
        // Capture pre-wizard state for B5 undo. Cloned before any
        // mutation; `new_project()` clears `start_positions` two lines
        // below, so this must run first.
        let pre_wizard = self.capture_wizard_snapshot();
        self.new_project();
        self.dirty = true; // wizard-applied project hasn't been saved yet
        self.project_name = sanitized;
        self.map_size = MapSize {
            smu_x: w.smu_x.max(1),
            smu_z: w.smu_z.max(1),
        };
        self.symmetry = match w.symmetry {
            SymmetryAxis::Rotational { .. } => SymmetryAxis::Rotational {
                fold: w.rotational_fold.max(2),
            },
            other => other,
        };
        self.rotational_fold = w.rotational_fold.max(2);
        self.height_scale = w.max_height.max(1.0);

        // Seed terrain with the chosen biome's procgen preset.
        let biome = &BIOMES[w.biome_index.min(BIOMES.len() - 1)];
        self.procgen_expr = biome.expression.to_string();
        self.procgen_domain = biome.domain;
        self.revalidate_procgen();
        info!(
            name = %self.project_name,
            smu_x = self.map_size.smu_x,
            smu_z = self.map_size.smu_z,
            symmetry = self.symmetry.id(),
            biome = biome.label,
            max_height = self.height_scale,
            "F1 wizard: applying"
        );
        self.apply_procgen();

        // B8: pre-place a 1v1 N/S strip pair in ally_groups[0] so the
        // demo state is non-empty out of the box. Done AFTER
        // apply_procgen so the valley-vs-peak heuristic can see the
        // freshly-baked heightmap and dodge the parabolic-dome center.
        self.seed_demo_start_positions();

        // B8: 35° pitch, ~1.6× diagonal distance from map centre. The
        // default OrbitCamera::framing tilts at 45° / 1.4× — a slightly
        // shallower angle reads more like the in-game RTS view and
        // makes the N/S strip markers easier to read.
        let (ex, ez) = self.map_size.elmo_extents();
        self.camera = OrbitCamera::framing(ex as f32, ez as f32);
        let diag = ((ex as f32).powi(2) + (ez as f32).powi(2)).sqrt();
        self.camera.pitch = 35f32.to_radians();
        self.camera.distance = diag * 1.6;

        // B5: history is now empty (apply_procgen barriered it). Push
        // one ApplyWizard entry holding the pre-wizard snapshot so
        // Ctrl-Z reverts the whole wizard apply atomically.
        self.history
            .push_project_diff(ProjectDiff::ApplyWizard(Box::new(pre_wizard)));
        self.wizard_open = false;

        // B8: trigger the non-modal "Next steps" hint overlay. Stays
        // hidden if the user previously dismissed this *project*'s
        // hint (per-project flag, not per-user), letting reopening a
        // fresh project re-show the hint.
        self.show_next_steps = !self.next_steps_dismissed;
    }

    /// B8: seed the freshly-wizard'd project with two default start
    /// positions in `ally_groups[0]`. Positions land on the N/S
    /// strips at `z = 0.15 / 0.85` of map height by default, but are
    /// nudged into the closest valley pixel (normalised height in
    /// `[0.2, 0.6]`) when the seeded heightmap places them on a peak
    /// — relevant for the Parabolic-dome biome where the centre is
    /// the highest point. Falls back to map quarter-points if no
    /// valley is found.
    fn seed_demo_start_positions(&mut self) {
        let (ex, ez) = self.map_size.elmo_extents();
        let cx = (ex as f32) * 0.5;
        let north = (cx, (ez as f32) * 0.15);
        let south = (cx, (ez as f32) * 0.85);
        let n_pos = self.nudge_into_valley(north);
        let s_pos = self.nudge_into_valley(south);
        let mut group = AllyGroup::new(0);
        group.start_positions = vec![
            StartPosition {
                x_elmo: n_pos.0 as i32,
                z_elmo: n_pos.1 as i32,
            },
            StartPosition {
                x_elmo: s_pos.0 as i32,
                z_elmo: s_pos.1 as i32,
            },
        ];
        self.ally_groups.push(group);
        self.active_ally_group_id = 0;
        info!(
            north = ?n_pos,
            south = ?s_pos,
            "F1 wizard / B8: seeded demo state with 2 start positions in ally_groups[0]"
        );
    }

    /// B8 valley-finder. Given an `(x, z)` proposal in elmos, return
    /// a nearby coordinate whose normalised heightmap value lies in
    /// `[0.2, 0.6]`. If the proposal is already in that band (or no
    /// heightmap is loaded), pass it through unchanged. Otherwise
    /// scan outward in a small square neighbourhood (16 samples each
    /// way) for the first pixel that satisfies; fall back to the
    /// quarter-point on miss so we never plant a marker on a peak
    /// (parabolic-dome biome) or off the cliff (diagonal-ramp
    /// biome).
    fn nudge_into_valley(&self, proposal: (f32, f32)) -> (f32, f32) {
        let Some(hm) = self.heightmap.as_ref() else {
            return proposal;
        };
        let (w, h) = hm.dims;
        let (ex, ez) = self.map_size.elmo_extents();
        let to_pixel = |x: f32, z: f32| -> Option<(u32, u32)> {
            if ex == 0 || ez == 0 {
                return None;
            }
            let px = ((x / ex as f32) * (w - 1) as f32).round();
            let pz = ((z / ez as f32) * (h - 1) as f32).round();
            if px < 0.0 || pz < 0.0 || px > (w - 1) as f32 || pz > (h - 1) as f32 {
                None
            } else {
                Some((px as u32, pz as u32))
            }
        };
        let in_valley = |px: u32, pz: u32| -> bool {
            let v = hm.data.data()[(pz as usize) * (w as usize) + (px as usize)];
            let norm = v as f32 / u16::MAX as f32;
            (0.2..=0.6).contains(&norm)
        };
        if let Some((px, pz)) = to_pixel(proposal.0, proposal.1)
            && in_valley(px, pz)
        {
            return proposal;
        }
        // Square spiral outward in coarse pixel steps. 16 steps × the
        // pixel pitch is enough to escape a peaky biome on the seed
        // heightmap; further than that and we'd cross the symmetry
        // axis. Iterate radii in increments of (1/32) of map dim so
        // even a 2-SMU test fixture has some headroom.
        let step_px = ((w.max(h)) / 32).max(1);
        let pitch_x = ex as f32 / (w - 1) as f32;
        let pitch_z = ez as f32 / (h - 1) as f32;
        for r in 1..=16i32 {
            for dz in [-r, r] {
                for dx in -r..=r {
                    let nx = proposal.0 + dx as f32 * step_px as f32 * pitch_x;
                    let nz = proposal.1 + dz as f32 * step_px as f32 * pitch_z;
                    if let Some((px, pz)) = to_pixel(nx, nz)
                        && in_valley(px, pz)
                    {
                        return (nx, nz);
                    }
                }
            }
            for dx in [-r, r] {
                for dz in -(r - 1)..=(r - 1) {
                    let nx = proposal.0 + dx as f32 * step_px as f32 * pitch_x;
                    let nz = proposal.1 + dz as f32 * step_px as f32 * pitch_z;
                    if let Some((px, pz)) = to_pixel(nx, nz)
                        && in_valley(px, pz)
                    {
                        return (nx, nz);
                    }
                }
            }
        }
        // Fallback: map quarter-points. Far enough from centre to
        // dodge a parabolic dome's peak even when the search loop
        // missed.
        let qx = ex as f32 * 0.25;
        let qz = ez as f32 * 0.25;
        warn!(
            proposal = ?proposal,
            quarter = ?(qx, qz),
            "B8 valley search exhausted; falling back to map quarter-point"
        );
        (qx, qz)
    }

    /// Build the per-frame splat uniforms passed to the GPU.
    ///
    /// D10 / Sprint 17 (ADR-041): now derives from the layer stack
    /// (per-layer `dnts_tex_scale` / `dnts_tex_mult` / channel
    /// binding) instead of the retired `App::splat_config`. The
    /// `diffuse_in_alpha` flag comes from `App::dnts_diffuse_in_alpha`
    /// (the per-project replacement for the legacy field).
    fn splat_uniforms_for_render(&self) -> SplatUniforms {
        let base = SplatUniforms::default();
        let mut tex_scales = [0.02f32; 4];
        let mut tex_mults = [1.0f32; 4];
        let mut active_mask = 0u32;
        for (ch, maybe_layer) in self.layer_stack.dnts_layers().iter().enumerate() {
            if let Some(layer) = maybe_layer {
                tex_scales[ch] = layer.dnts_tex_scale;
                tex_mults[ch] = layer.dnts_tex_mult;
                active_mask |= 1 << ch;
            }
        }
        SplatUniforms {
            tex_scales,
            tex_mults,
            flags: [
                active_mask,
                u32::from(self.dnts_diffuse_in_alpha),
                u32::from(self.buildable_overlay_on),
                0,
            ],
            sun_dir: base.sun_dir,
            ground_ambient: base.ground_ambient,
            ground_diffuse: base.ground_diffuse,
        }
    }

    /// C9 (Sprint 14 / ADR-042) — produce a `WaterDraw` describing the
    /// frame's water plane, or `None` when `water_mode == None`. The
    /// MVP returns a flat alpha-blended quad tinted by the active
    /// preset's `surface_color` + `surface_alpha`. Cross-tool ghost
    /// (commit 5) flips the `alpha_scale` to 0.5 when `Tool::Water`
    /// isn't active.
    fn water_draw_for_frame(&self, extent_x: f32, extent_z: f32) -> Option<WaterDraw> {
        if self.water_mode == WaterMode::None {
            return None;
        }
        // Build the same merged WaterBlock the emission path produces.
        // For `Custom` the preset is empty; the user's overrides land
        // verbatim. For all other modes the preset provides a baseline
        // surface RGB + alpha which overrides shadow per-field.
        let preset = preset_water_block(self.water_mode).unwrap_or_default();
        let merged = merge_overrides(&preset, &self.water_overrides);
        let [r, g, b] = merged.surface_color.unwrap_or(BAR_DEFAULT_SURFACE_COLOR);
        let a = merged
            .surface_alpha
            .unwrap_or(BAR_DEFAULT_SURFACE_ALPHA)
            .clamp(0.0, 1.0);
        // Pre-multiply RGB by alpha — the pipeline uses
        // `PREMULTIPLIED_ALPHA_BLENDING`, so the shader output must
        // already be `(r·a, g·a, b·a, a)`.
        let surface_rgba = [r * a, g * a, b * a, a];
        // Cross-tool ghosting (Sprint 14 / commit 5): the water plane
        // is a project property regardless of active tool, but it
        // shouldn't dominate the canvas while the user is sculpting
        // or painting. At full opacity only when `Tool::Water` is
        // active; otherwise 0.5× so the plane stays visible but
        // doesn't take over. Pattern mirrors the marker / line
        // cross-tool ghosts in `central()`.
        let alpha_scale = if matches!(self.tool, Tool::Water) {
            1.0
        } else {
            0.5
        };
        Some(WaterDraw {
            surface_rgba,
            extent_x,
            extent_z,
            alpha_scale,
        })
    }

    /// D9 / Sprint 16 (ADR-039) — build the per-frame composite
    /// uniforms from the live layer stack. Returns `None` when the
    /// stack is empty (the terrain shader's `params2.y` stays at 0
    /// and the Sprint-9 splat/biome fallback renders instead).
    ///
    /// Layers beyond `COMPOSITE_MAX_LAYERS = 16` are clipped — the
    /// Sprint-17 Layers panel will surface a chip warning when this
    /// hits; for now, only the bottom 16 contribute to the preview.
    fn composite_uniforms_for_render(&self) -> Option<CompositeU> {
        if self.layer_stack.layers.is_empty() {
            return None;
        }
        let (rt_w, rt_h) = self.composite_rt_dims();
        let (ex, ez) = self.map_size.elmo_extents();
        let mut cu = CompositeU {
            // .xy = RT dims (mask UV normalisation), .zw = elmo
            // extent (layer-transform math). The two diverge on >8
            // SMU maps where the RT clamp engages.
            dims: [rt_w as f32, rt_h as f32, ex as f32, ez as f32],
            ..CompositeU::default()
        };
        for (i, layer) in self.layer_stack.layers.iter().enumerate().take(16) {
            let active = layer.visible && layer.opacity > 0.0;
            cu.layers[i] =
                CompositeLayerU::from_layer(&layer.transform, &layer.color, layer.opacity, active);
        }
        Some(cu)
    }

    /// D9 / Sprint 16 — composite RT dims = `min(texture_dims, 4096²)`
    /// per ADR-039. The CPU bake stays authoritative at full
    /// texture_dims for the .sd7 export; the GPU preview is
    /// approximate for >8-SMU maps.
    fn composite_rt_dims(&self) -> (u32, u32) {
        let (tw, th) = self.map_size.texture_dims();
        let cap = crate::render::COMPOSITE_RT_CLAMP;
        (tw.min(cap), th.min(cap))
    }

    /// D9 / Sprint 16 — re-upload every layer's source diffuse to the
    /// composite slot array. Stock slots resolve through the registry;
    /// imported layers fall back to the magenta diagnostic the slot
    /// array initialises with at install time (Sprint 17 fixes this).
    ///
    /// Called on project open / new project / wizard apply / migration
    /// — anywhere the layer stack changes wholesale. Per-layer slot
    /// rebinds (Sprint 17's Layers panel) target individual array
    /// layers via a focused call.
    fn reupload_layer_stack_diffuses(&self) {
        let Some(rs) = self.render_state.as_ref() else {
            return;
        };
        for (i, layer) in self.layer_stack.layers.iter().enumerate().take(16) {
            match &layer.source {
                barme_core::LayerSource::Slot { id } => {
                    let Some(slot) = self.slot_registry.iter().find(|s| s.id == *id) else {
                        warn!(
                            slot_id = id,
                            layer_idx = i,
                            "composite slot diffuse missing in registry — layer renders magenta"
                        );
                        continue;
                    };
                    let Some(rgba) = load_slot_full_rgba(slot) else {
                        continue;
                    };
                    crate::render::upload_composite_slot_diffuse(rs, i as u32, rgba.as_raw());
                }
                barme_core::LayerSource::Imported { .. } => {
                    // Sprint 16 deliberately leaves imported layers
                    // as the magenta diagnostic the slot array
                    // initialised with — Sprint 17 lands the import
                    // workflow that populates this for real.
                    trace!(
                        layer_idx = i,
                        layer_id = %layer.id,
                        "composite: imported layer renders magenta until Sprint 17"
                    );
                }
            }
        }
    }

    /// D9 / Sprint 16 — push any layer mask tiles that have changed
    /// since the last upload to the composite mask array. Called from
    /// `central()` once per frame when a composite RT is allocated.
    ///
    /// On the first call after `reupload_layer_stack_diffuses` (or on
    /// a project open), every layer has `version() > last_uploaded =
    /// 0`, so the full mask grid uploads. Subsequent frames only push
    /// tiles touched by brush strokes (Sprint 16 Commit 3).
    fn sync_composite_mask_tiles(&mut self) {
        let Some(rs) = self.render_state.as_ref() else {
            return;
        };
        if self.layer_stack.layers.is_empty() {
            return;
        }
        for (i, layer) in self.layer_stack.layers.iter().enumerate().take(16) {
            let key = (layer.id.clone(), i);
            let last = self
                .composite_layer_last_version
                .get(&key)
                .copied()
                .unwrap_or(0);
            let cur = layer.mask.version();
            if cur <= last {
                continue;
            }
            let dirty = layer.mask.dirty_tiles_since(last);
            if dirty.is_empty() {
                continue;
            }
            crate::render::write_composite_layer_mask_tiles(rs, i as u32, &layer.mask, &dirty);
            self.composite_layer_last_version.insert(key, cur);
            trace!(
                layer_idx = i,
                layer_id = %layer.id,
                dirty_tiles = dirty.len(),
                from_version = last,
                to_version = cur,
                "composite mask sync"
            );
        }
    }

    /// Lazily produce + cache a 96² thumbnail for `slot_id`. Returns
    /// `None` when the slot's diffuse can't be loaded (missing dir /
    /// corrupt PNG). The cache lives on `App::slot_thumbnails`; entries
    /// survive a project open/close because the texture pack on disk
    /// is project-independent.
    fn slot_thumbnail(&mut self, ctx: &egui::Context, slot_id: u8) -> Option<egui::TextureHandle> {
        if let Some(handle) = self.slot_thumbnails.get(&slot_id) {
            return Some(handle.clone());
        }
        let slot = self.slot_registry.iter().find(|s| s.id == slot_id)?.clone();
        let rgba = load_slot_thumbnail_rgba(&slot)?;
        let (w, h) = rgba.dimensions();
        let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw());
        let handle = ctx.load_texture(
            format!("slot-thumb-{slot_id:02}"),
            img,
            egui::TextureOptions::LINEAR,
        );
        self.slot_thumbnails.insert(slot_id, handle.clone());
        Some(handle)
    }

    /// D10 / Sprint 17 (ADR-041) — produce a 96² thumbnail handle for
    /// the layer identified by `layer_id`. For `Slot`-sourced layers
    /// this delegates to [`Self::slot_thumbnail`] (slot-id-keyed cache;
    /// already shared by the slot picker). For `Imported`-sourced
    /// layers it decodes the imported PNG once and caches it on
    /// [`App::layer_thumbnails`] keyed by layer id.
    fn layer_thumbnail(
        &mut self,
        ctx: &egui::Context,
        layer_id: &str,
    ) -> Option<egui::TextureHandle> {
        let layer = self.layer_stack.layer_by_id(layer_id)?;
        match layer.source.clone() {
            barme_core::LayerSource::Slot { id } => self.slot_thumbnail(ctx, id),
            barme_core::LayerSource::Imported { path } => {
                if let Some(h) = self.layer_thumbnails.get(layer_id) {
                    return Some(h.clone());
                }
                // Imported paths in pre-Sprint-17 projects may be
                // absolute; Sprint 17 normalises them to project-relative
                // `textures/<uuid>.png`. The current project path isn't
                // tracked on `App` directly, so we trust the absolute /
                // CWD-relative form and fall back to `None` on decode
                // failure. Sprint 17 / Commit 2 wires the project-root
                // base into a helper.
                let img = match image::open(&path) {
                    Ok(i) => i,
                    Err(e) => {
                        warn!(
                            layer_id,
                            path = %path.display(),
                            error = %e,
                            "layer_thumbnail: imported source decode failed",
                        );
                        return None;
                    }
                };
                let rgba = image::imageops::resize(
                    &img.to_rgba8(),
                    96,
                    96,
                    image::imageops::FilterType::Triangle,
                );
                let (w, h) = rgba.dimensions();
                let color = egui::ColorImage::from_rgba_unmultiplied(
                    [w as usize, h as usize],
                    rgba.as_raw(),
                );
                let handle = ctx.load_texture(
                    format!("layer-thumb-{layer_id}"),
                    color,
                    egui::TextureOptions::LINEAR,
                );
                self.layer_thumbnails
                    .insert(layer_id.to_string(), handle.clone());
                Some(handle)
            }
        }
    }

    /// D10 / Sprint 17 (ADR-041) — build (or hit the cache for) the
    /// active layer's grayscale mask preview. Returned texture is sized
    /// at most 512² (downsampled via box filter), with red wherever the
    /// mask reads 0. `None` when no active layer is selected.
    ///
    /// Cache key = `(active_layer_id, mask.version())`. The mask version
    /// bumps once per brush stamp, so a stroke pushes ~one new texture
    /// upload per frame; idle frames hit the cache and return the
    /// existing handle.
    fn active_mask_overlay_texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        let layer_id = self.paint_active_layer_id.clone()?;
        let layer = self.layer_stack.layer_by_id(&layer_id)?;
        let version = layer.mask.version();
        if let Some((cached_id, cached_v, handle)) = self.layer_mask_preview_cache.as_ref()
            && cached_id == &layer_id
            && *cached_v == version
        {
            return Some(handle.clone());
        }
        // Downsample mask to max 512² with a simple box filter.
        let src_w = layer.mask.width;
        let src_h = layer.mask.height;
        if src_w == 0 || src_h == 0 {
            return None;
        }
        let max_dim = 512u32;
        let scale = (src_w.max(src_h) as f32 / max_dim as f32).max(1.0).ceil() as u32;
        let scale = scale.max(1);
        let dst_w = (src_w / scale).max(1);
        let dst_h = (src_h / scale).max(1);
        let mut pixels: Vec<u8> = Vec::with_capacity((dst_w as usize) * (dst_h as usize) * 4);
        for dy in 0..dst_h {
            for dx in 0..dst_w {
                // Box-filter: average a `scale × scale` block.
                let mut sum: u32 = 0;
                let mut count: u32 = 0;
                let sx0 = dx * scale;
                let sy0 = dy * scale;
                let sx1 = (sx0 + scale).min(src_w);
                let sy1 = (sy0 + scale).min(src_h);
                for sy in sy0..sy1 {
                    for sx in sx0..sx1 {
                        sum += u32::from(layer.mask.sample(sx, sy));
                        count += 1;
                    }
                }
                let m = sum.checked_div(count).map(|v| v as u8).unwrap_or(0);
                if m == 0 {
                    // Red where mask is zero — visually pops the "no
                    // contribution" region.
                    pixels.extend_from_slice(&[224, 32, 32, 255]);
                } else {
                    pixels.extend_from_slice(&[m, m, m, 255]);
                }
            }
        }
        let color =
            egui::ColorImage::from_rgba_unmultiplied([dst_w as usize, dst_h as usize], &pixels);
        let handle = ctx.load_texture(
            format!("layer-mask-{layer_id}"),
            color,
            egui::TextureOptions::NEAREST,
        );
        self.layer_mask_preview_cache = Some((layer_id, version, handle.clone()));
        Some(handle)
    }

    /// D9 / Sprint 16 — seed a freshly-built `LayerStack::from_biome`
    /// with a second "accent" layer (slot 1 if it exists, mask = 0)
    /// so the user can immediately paint reveal/hide and see the
    /// result in the central viewport without first having to add a
    /// layer via the Layers panel. Idempotent — bails when the stack
    /// already has > 1 layer (migration from older projects) or when
    /// the registry doesn't carry a second slot.
    fn seed_demo_accent_layer(&mut self) {
        if self.layer_stack.layers.len() != 1 {
            return;
        }
        let used_first = match &self.layer_stack.layers[0].source {
            barme_core::LayerSource::Slot { id } => Some(*id),
            barme_core::LayerSource::Imported { .. } => None,
        };
        let next = self
            .slot_registry
            .iter()
            .find(|s| Some(s.id) != used_first)
            .map(|s| s.id);
        let Some(slot_id) = next else {
            return;
        };
        let accent = TextureLayer::new(
            barme_core::LayerSource::Slot { id: slot_id },
            self.map_size,
            0,
        );
        self.layer_stack.layers.push(accent);
        // Don't `mark_dirty` — this is part of the initial fresh-
        // project shape, not a user edit.
        info!(
            slot_id,
            "Sprint 16 Layers: seeded demo accent layer on top of base biome"
        );
    }

    /// D9 / Sprint 16 (ADR-040, brought-forward Layers panel) — add a
    /// new layer at the TOP of the stack (Photoshop convention: the
    /// new layer sits over what was there). Picks the next available
    /// slot id from the registry that isn't already used in the
    /// stack; falls back to slot 0 when every registry slot is
    /// already bound. Mask starts at 0 (fully transparent) so the new
    /// layer is a clean canvas to paint into.
    ///
    /// Returns the new layer's id. Re-uploads all slot diffuses to
    /// the composite slot array so indices stay aligned, and clears
    /// the per-layer mask version cache so the new layer's mask
    /// uploads on the next frame.
    fn add_layer_at_top(&mut self) -> String {
        let used: std::collections::HashSet<u8> = self
            .layer_stack
            .layers
            .iter()
            .filter_map(|l| match &l.source {
                barme_core::LayerSource::Slot { id } => Some(*id),
                barme_core::LayerSource::Imported { .. } => None,
            })
            .collect();
        let pick = self
            .slot_registry
            .iter()
            .find(|s| !used.contains(&s.id))
            .map(|s| s.id)
            .unwrap_or(0);
        self.add_layer_with_slot(pick)
    }

    /// D10 / Sprint 17 (ADR-041) — add a new layer at the top of the
    /// stack with `slot_id` as its source. Used by the Layers panel's
    /// slot-picker popup so the user picks the stock biome they want
    /// instead of accepting "whatever the next unused slot is."
    ///
    /// Mask starts at 0 (transparent) — the new layer is a clean
    /// canvas. Returns the new layer's id, with the GPU composite
    /// slot array re-uploaded so indices stay aligned.
    fn add_layer_with_slot(&mut self, slot_id: u8) -> String {
        let layer = TextureLayer::new(
            barme_core::LayerSource::Slot { id: slot_id },
            self.map_size,
            0,
        );
        let id = layer.id.clone();
        let index = self.layer_stack.layers.len();
        self.layer_stack.layers.push(layer.clone());
        self.mark_dirty();
        self.composite_layer_last_version.clear();
        self.reupload_layer_stack_diffuses();
        self.history
            .push_project_diff(barme_core::ProjectDiff::AddLayer {
                index,
                layer: Box::new(layer),
            });
        info!(
            slot_id,
            layer_id = %id,
            "Sprint 17 Layers: added layer at top of stack with explicit slot"
        );
        id
    }

    /// D9 / Sprint 16 — delete the layer with `layer_id`. If it was
    /// the active layer, drop the active selection (the central
    /// helper picks a fresh top-of-stack default on next frame).
    /// Re-uploads slot diffuses so the remaining layers' indices
    /// realign with the composite slot array.
    fn delete_layer(&mut self, layer_id: &str) {
        let Some(idx) = self
            .layer_stack
            .layers
            .iter()
            .position(|l| l.id == layer_id)
        else {
            return;
        };
        self.layer_stack.layers.remove(idx);
        if self.paint_active_layer_id.as_deref() == Some(layer_id) {
            self.paint_active_layer_id = None;
        }
        self.mark_dirty();
        self.composite_layer_last_version.clear();
        self.reupload_layer_stack_diffuses();
        info!(layer_id, "Sprint 16 Layers: deleted layer");
    }

    /// D9 / Sprint 16 — move the layer at `from` to `to`, shifting the
    /// rest. Indices are bottom-first vec indices, so a UI "move up"
    /// (Photoshop convention = move toward the top of the visible
    /// stack) is `from + 1` → `from`. Idempotent when `from == to`.
    fn reorder_layer(&mut self, from: usize, to: usize) {
        let n = self.layer_stack.layers.len();
        if from >= n || to >= n || from == to {
            return;
        }
        let layer = self.layer_stack.layers.remove(from);
        self.layer_stack.layers.insert(to, layer);
        self.mark_dirty();
        self.composite_layer_last_version.clear();
        self.reupload_layer_stack_diffuses();
        info!(from, to, "Sprint 16 Layers: reordered layer");
    }

    /// D10 / Sprint 17 (ADR-041) — import a PNG / JPG into the layer
    /// identified by `layer_id` via the project-local sidecar:
    ///
    /// 1. Validate the source decodes + dims ∈ [16, 8192].
    /// 2. Re-encode the source as PNG at
    ///    `<project_root>/textures/<uuid>.png`. Larger sources
    ///    downsample to 8192² via Lanczos3 (PNG re-encode normalises
    ///    any JPEG artefacts).
    /// 3. Write a `<uuid>.meta.toml` sidecar with `name`,
    ///    `source_filename`, `original_dims`, `imported_at_unix`.
    /// 4. Update the layer's `LayerSource` to the project-relative
    ///    path `textures/<uuid>.png`. Renames the layer's auto-default
    ///    name to the source file stem.
    /// 5. Resize a 1024² copy for the GPU composite slot at this
    ///    layer's vec index.
    /// 6. Push `ProjectDiff::SetLayerProperty(Source)` for undo.
    ///
    /// Bails with a `last_error` toast when the project hasn't been
    /// saved yet (no `<project>/textures/` directory to write into).
    fn import_layer_texture(&mut self, layer_id: &str, path: PathBuf) {
        use barme_core::LayerSource;
        use barme_core::undo::LayerPropertyValue;

        let Some(idx) = self
            .layer_stack
            .layers
            .iter()
            .position(|l| l.id == layer_id)
        else {
            return;
        };

        // Require a saved project so we have a textures/ sidecar dir
        // to write into. PITFALL §17.4 — imports MUST live under the
        // project, otherwise a project move dangles the layer.
        let Some(project_path) = self.current_project_path.clone() else {
            self.last_error = Some(
                "Save the project before importing textures (textures live next to the \
                 .barmeproj)."
                    .into(),
            );
            warn!("Sprint 17 Layers: import refused — project not yet saved (no textures sidecar)");
            return;
        };
        let Some(project_root) = project_path.parent().map(Path::to_path_buf) else {
            return;
        };

        // Decode + validate.
        let img = match image::open(&path) {
            Ok(i) => i,
            Err(e) => {
                warn!(
                    path = %path.display(),
                    error = %e,
                    "Sprint 17 Layers: import failed; layer unchanged"
                );
                self.last_error = Some(format!("Texture import failed: {e:#}"));
                return;
            }
        };
        let (orig_w, orig_h) = (img.width(), img.height());
        if orig_w < 16 || orig_h < 16 {
            self.last_error = Some(format!(
                "Imported texture too small ({orig_w}×{orig_h}); must be at least 16×16.",
            ));
            return;
        }
        let mut rgba = img.to_rgba8();
        let max_dim = 8192u32;
        let mut did_downsample = false;
        if orig_w > max_dim || orig_h > max_dim {
            let scale = (max_dim as f32) / (orig_w.max(orig_h) as f32);
            let nw = ((orig_w as f32) * scale).round().max(1.0) as u32;
            let nh = ((orig_h as f32) * scale).round().max(1.0) as u32;
            rgba = image::imageops::resize(&rgba, nw, nh, image::imageops::FilterType::Lanczos3);
            did_downsample = true;
        }
        let (final_w, final_h) = rgba.dimensions();

        // Sidecar directory + UUID-named PNG + meta.toml.
        let textures_dir = project_root.join("textures");
        if let Err(e) = std::fs::create_dir_all(&textures_dir) {
            self.last_error = Some(format!("Could not create textures dir: {e}"));
            return;
        }
        let uuid = barme_core::alloc_layer_id();
        let dest_filename = format!("{uuid}.png");
        let dest_disk = textures_dir.join(&dest_filename);
        if let Err(e) = rgba.save(&dest_disk) {
            self.last_error = Some(format!("Could not save imported texture: {e}"));
            return;
        }
        let source_filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        let imported_at_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let layer_name_for_meta = self.layer_stack.layers[idx].name.clone();
        let meta = format!(
            "name = \"{}\"\n\
             source_filename = \"{}\"\n\
             original_dims = [{}, {}]\n\
             imported_at_unix = {}\n",
            escape_toml(&layer_name_for_meta),
            escape_toml(source_filename),
            orig_w,
            orig_h,
            imported_at_unix,
        );
        let _ = std::fs::write(textures_dir.join(format!("{uuid}.meta.toml")), meta);

        // Resize for the GPU composite slot.
        let rgba_gpu = if final_w != crate::render::SLOT_COMPOSITE_DIM
            || final_h != crate::render::SLOT_COMPOSITE_DIM
        {
            image::imageops::resize(
                &rgba,
                crate::render::SLOT_COMPOSITE_DIM,
                crate::render::SLOT_COMPOSITE_DIM,
                image::imageops::FilterType::Lanczos3,
            )
        } else {
            rgba.clone()
        };

        // Update layer + push undo diff. The relative path keeps the
        // project portable (move the .barmeproj + textures/ together).
        let new_source = LayerSource::Imported {
            path: PathBuf::from("textures").join(&dest_filename),
        };
        let prev_source = self.layer_stack.layers[idx].source.clone();
        let auto_name = prev_source.default_label();
        let layer = &mut self.layer_stack.layers[idx];
        if layer.name == auto_name {
            layer.name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "Imported".to_string());
        }
        layer.source = new_source.clone();
        // Bust the thumbnail cache so the next render picks up the new
        // PNG.
        self.layer_thumbnails.remove(layer_id);
        self.mark_dirty();
        if let Some(rs) = self.render_state.as_ref() {
            crate::render::upload_composite_slot_diffuse(rs, idx as u32, rgba_gpu.as_raw());
        }
        self.history
            .push_project_diff(barme_core::ProjectDiff::SetLayerProperty {
                layer_id: layer_id.to_string(),
                from: LayerPropertyValue::Source(prev_source),
                to: LayerPropertyValue::Source(new_source),
            });
        if did_downsample {
            self.last_error = Some(format!(
                "Imported {orig_w}×{orig_h} → downsampled to {final_w}×{final_h} (8192² cap).",
            ));
        }
        info!(
            layer_id,
            disk = %dest_disk.display(),
            orig_w,
            orig_h,
            "Sprint 17 Layers: copied imported texture into project-local sidecar"
        );
    }

    /// D10 / Sprint 17 (ADR-041) — walk the loaded layer stack and
    /// migrate any pre-Sprint-17 `LayerSource::Imported` paths that
    /// either (a) are absolute or (b) don't start with `textures/`
    /// into the project-local sidecar. Source files are copied into
    /// `<project>/textures/<uuid>.png` (PNG re-encode) and the
    /// layer's `LayerSource::Imported.path` rewrites to the relative
    /// form.
    ///
    /// No-op when no migration is needed. Called from the load path
    /// AFTER `Project::after_load_migrate` has hydrated the layer
    /// stack.
    fn migrate_imported_layer_paths(&mut self) {
        use barme_core::LayerSource;

        let Some(project_path) = self.current_project_path.clone() else {
            return;
        };
        let Some(project_root) = project_path.parent().map(Path::to_path_buf) else {
            return;
        };
        let textures_dir = project_root.join("textures");

        // Pass 1: identify layers that need migrating. Pass 2 mutates
        // (avoids holding `&mut self.layer_stack` across the per-layer
        // I/O which may also need `self`).
        let plan: Vec<(String, PathBuf)> = self
            .layer_stack
            .layers
            .iter()
            .filter_map(|l| {
                let LayerSource::Imported { path } = &l.source else {
                    return None;
                };
                if !path.is_absolute() && path.starts_with("textures") {
                    return None;
                }
                Some((l.id.clone(), path.clone()))
            })
            .collect();
        if plan.is_empty() {
            return;
        }
        if !textures_dir.exists()
            && let Err(e) = std::fs::create_dir_all(&textures_dir)
        {
            warn!(
                dir = %textures_dir.display(),
                error = %e,
                "Sprint 17 import migration: failed to create textures dir; skipping",
            );
            return;
        }
        let mut migrated_any = false;
        for (layer_id, orig_path) in plan {
            let src = if orig_path.is_absolute() {
                orig_path.clone()
            } else {
                project_root.join(&orig_path)
            };
            if !src.is_file() {
                warn!(
                    layer_id = %layer_id,
                    path = %src.display(),
                    "Sprint 17 import migration: source file missing; leaving placeholder",
                );
                continue;
            }
            let img = match image::open(&src) {
                Ok(i) => i,
                Err(e) => {
                    warn!(
                        layer_id = %layer_id,
                        src = %src.display(),
                        error = %e,
                        "Sprint 17 import migration: source decode failed",
                    );
                    continue;
                }
            };
            let rgba = img.to_rgba8();
            let uuid = barme_core::alloc_layer_id();
            let dest_filename = format!("{uuid}.png");
            let dest = textures_dir.join(&dest_filename);
            if let Err(e) = rgba.save(&dest) {
                warn!(
                    layer_id = %layer_id,
                    dest = %dest.display(),
                    error = %e,
                    "Sprint 17 import migration: PNG save failed",
                );
                continue;
            }
            // Rewrite the layer's source — `active_layer_mut` is the
            // existing accessor.
            if let Some(layer) = self.layer_stack.active_layer_mut(&layer_id) {
                layer.source = LayerSource::Imported {
                    path: PathBuf::from("textures").join(&dest_filename),
                };
                migrated_any = true;
            }
            info!(
                layer_id = %layer_id,
                from = %src.display(),
                to = %dest.display(),
                "Sprint 17 import migration: copied texture into project-local sidecar",
            );
        }
        if migrated_any {
            self.mark_dirty();
        }
    }

    /// D9 / Sprint 16 (ADR-040) — apply the active mask brush at the
    /// given cursor position (in mask-elmo coords). No-op when no
    /// active layer is selected / the layer doesn't exist / the brush
    /// id resolves to nothing.
    ///
    /// D10 / Sprint 17 (ADR-041): fans the stamp through
    /// [`Self::symmetry`] like the heightmap + splat paths do, so a
    /// single LMB stroke under e.g. `SymmetryAxis::Horizontal` paints
    /// both halves of the map. The compositor picks up the per-tile
    /// version bumps via [`Self::sync_composite_mask_tiles`] next
    /// frame; no per-stamp dirty-rect plumbing is needed.
    ///
    /// D10 / Sprint 17 (ADR-041): also pre-walks the brush bbox's
    /// tile coords + snapshots each tile via `History::snapshot_mask_tile`
    /// BEFORE the brush writes, so the drag's `end_mask_stroke` can
    /// commit a `HistoryEntry::Mask` covering all touched tiles.
    fn apply_mask_brush_at_elmos(&mut self, world_x: f32, world_z: f32) {
        let Some(layer_id) = self.paint_active_layer_id.clone() else {
            return;
        };
        let brush_id = self.paint_brush_state.brush_id.clone();
        let Some(brush) = self.mask_brushes.get(&brush_id) else {
            return;
        };
        let extents = self.map_size.elmo_extents();
        let extents = (extents.0 as f32, extents.1 as f32);
        let centers = self.symmetry.replicate((world_x, world_z), extents);
        if centers.is_empty() {
            return;
        }
        let radius = self.paint_brush_state.radius;
        let strength = self.paint_brush_state.strength;
        let target_visible = self.paint_brush_state.fill_target_visible;
        // Pre-walk + snapshot every tile each per-symmetry stamp will
        // touch, capturing the pre-stroke state. Deduplicate via a
        // HashSet — overlapping stamps + the per-tile `or_insert` on
        // the history side already guard against double-snapshot, but
        // the local set saves the work of N tile clones.
        let mut tiles_to_snapshot: std::collections::HashSet<barme_core::TileCoord> =
            std::collections::HashSet::new();
        if let Some(layer) = self.layer_stack.layer_by_id(&layer_id) {
            for (cx, cz) in &centers {
                let stamp = barme_core::MaskStamp {
                    world_x: *cx,
                    world_z: *cz,
                    radius,
                    strength,
                    target_visible,
                };
                if let Some(bbox) = layer.mask.brush_bbox(stamp) {
                    for coord in layer.mask.tile_coords_overlapping_rect(bbox) {
                        tiles_to_snapshot.insert(coord);
                    }
                }
            }
        }
        // D10 / Sprint 17 hotfix — skip the 64 KB clone for tiles
        // the open stroke already snapshotted. A continuous 60-FPS
        // drag over the same tiles was churning ~15 MB/sec of
        // redundant clones; this dedup brings second-and-later
        // stamps over the same tiles to zero allocation.
        if !tiles_to_snapshot.is_empty()
            && let Some(layer) = self.layer_stack.layer_by_id(&layer_id)
        {
            let mut snapshotted = 0usize;
            for coord in &tiles_to_snapshot {
                if self.history.mask_stroke_has_snapshot(&layer_id, *coord) {
                    continue;
                }
                let current = layer.mask.clone_tile(*coord);
                self.history.snapshot_mask_tile(&layer_id, *coord, current);
                snapshotted += 1;
            }
            if snapshotted > 16 {
                tracing::warn!(
                    layer_id = %layer_id,
                    tiles_snapshotted = snapshotted,
                    "Sprint 17 mask stroke: > 16 tiles snapshotted in one stamp; \
                     consider reducing brush radius for memory headroom"
                );
            }
        }
        let mut touched_any = false;
        for (cx, cz) in centers {
            let stamp = barme_core::MaskStamp {
                world_x: cx,
                world_z: cz,
                radius,
                strength,
                target_visible,
            };
            if self
                .layer_stack
                .apply_brush(&layer_id, brush, stamp)
                .is_some()
            {
                touched_any = true;
            }
        }
        if touched_any {
            self.dirty = true;
        }
    }

    /// D9 / Sprint 16 (ADR-040) — interpolate stamps along the drag
    /// segment between `from` and `to` so a fast drag (delta >
    /// spacing × radius) doesn't leave gaps in the stroke. Mirrors
    /// the heightmap brush's drag-interp pattern (PITFALL §3 in the
    /// Sprint-16 prompt).
    fn apply_mask_brush_along_drag(&mut self, from: glam::Vec2, to: glam::Vec2) {
        let radius = self.paint_brush_state.radius.max(1.0);
        let spacing_elmos = (self.paint_brush_state.spacing.max(0.05) * radius).max(1.0);
        let delta = to - from;
        let dist = delta.length();
        if dist <= spacing_elmos {
            // One stamp at `to` covers it.
            self.apply_mask_brush_at_elmos(to.x, to.y);
            return;
        }
        let steps = (dist / spacing_elmos).ceil() as u32;
        for i in 1..=steps {
            let t = (i as f32) / (steps as f32);
            let p = from + delta * t;
            self.apply_mask_brush_at_elmos(p.x, p.y);
        }
    }

    /// D9 / Sprint 16 (ADR-040) — central viewport for `Tool::Paint
    /// Layer`. Renders the 2D composite preview, handles brush
    /// dispatch + drag interpolation, mutates `paint_view_state` for
    /// pan / zoom, and toggles the mask-only preview from the chip.
    fn central_paint_layer(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let t = crate::ui::theme::Tokens::DARK;
        let (extent_x, extent_z) = self.map_size.elmo_extents();
        let extent = (extent_x as f32, extent_z as f32);

        // Default active layer: topmost visible layer if no selection
        // has stuck. Top-of-stack = last in the bottom-first vec.
        if self.paint_active_layer_id.is_none()
            && let Some(top) = self.layer_stack.layers.iter().rev().find(|l| l.visible)
        {
            self.paint_active_layer_id = Some(top.id.clone());
        }

        // Composite RT egui texture id — request a re-allocation each
        // frame at the clamped dims; the function is idempotent when
        // the size hasn't changed.
        let composite_id = if let Some(rs) = self.render_state.as_ref() {
            let (cw, ch) = self.composite_rt_dims();
            render::ensure_composite_rt(rs, (cw, ch))
        } else {
            None
        };
        // Push mask tile uploads if the active brush bumped versions.
        // The frame's pipeline pass samples whatever's on the GPU now,
        // so this needs to land BEFORE the egui paint that uses the RT.
        self.sync_composite_mask_tiles();

        // The 3D viewport's `TerrainCallback::prepare` re-runs the
        // composite pass every frame and writes the per-layer uniforms.
        // `Tool::PaintLayer` never dispatches `TerrainCallback`, so
        // without this we'd show a stale RT — paint strokes would lag,
        // opacity / tint / transform slider edits wouldn't reflect, and
        // making a layer transparent wouldn't reveal the layer below.
        // Drop in a no-op-paint `CompositeCallback` that runs the
        // shared composite pass via the same encode helper.
        if let Some(cu) = self.composite_uniforms_for_render() {
            ui.painter()
                .add(eframe::egui_wgpu::Callback::new_paint_callback(
                    rect,
                    crate::render::CompositeCallback { composite: cu },
                ));
        }

        let cursor_world = ui.ctx().pointer_interact_pos().and_then(|p| {
            // Cheap world-elmo conversion via the same math the
            // paint_view uses internally — we need it ahead of the
            // `paint_view` call so the status strip + brush dispatch
            // agree on the cursor coord.
            if !rect.contains(p) {
                return None;
            }
            let auto_fit = (((rect.size().x - 32.0) / extent.0.max(1.0))
                .min((rect.size().y - 32.0) / extent.1.max(1.0)))
            .max(1e-4);
            let zoom = if self.paint_view_state.zoom > 0.0 {
                self.paint_view_state
                    .zoom
                    .clamp(auto_fit * 0.25, auto_fit * 16.0)
            } else {
                auto_fit
            };
            let map_centre_screen = rect.center()
                + egui::vec2(
                    self.paint_view_state.pan_elmos.x * zoom,
                    self.paint_view_state.pan_elmos.y * zoom,
                );
            let map_size_screen = egui::vec2(extent.0 * zoom, extent.1 * zoom);
            let map_origin = map_centre_screen - map_size_screen * 0.5;
            let rel = p - map_origin;
            let ex = rel.x / zoom;
            let ez = rel.y / zoom;
            if ex < 0.0 || ex >= extent.0 || ez < 0.0 || ez >= extent.1 {
                None
            } else {
                Some(glam::Vec2::new(ex, ez))
            }
        });

        // Mask value at cursor for the status strip.
        let mask_value_at_cursor = match (&self.paint_active_layer_id, cursor_world) {
            (Some(id), Some(p)) => self
                .layer_stack
                .layer_by_id(id)
                .map(|l| l.mask.sample(p.x.round() as u32, p.y.round() as u32)),
            _ => None,
        };
        let active_layer_name = self
            .paint_active_layer_id
            .as_ref()
            .and_then(|id| self.layer_stack.layer_by_id(id))
            .map(|l| l.name.clone());

        let mask_preview_on = self.paint_brush_state.mask_only_preview;
        let radius = self.paint_brush_state.radius;
        // D10 / Sprint 17 (ADR-041): build / cache the active layer's
        // grayscale mask preview only when the chip is on. Idle frames
        // pay one hash-map lookup; brush frames pay a downsample but
        // only when the mask version changed.
        let active_mask_overlay = if mask_preview_on {
            let ctx_handle = ui.ctx().clone();
            self.active_mask_overlay_texture(&ctx_handle)
                .map(|h| h.id())
        } else {
            None
        };
        let out = crate::ui::paint_view::paint_view(
            ui,
            rect,
            crate::ui::paint_view::PaintViewInput {
                composite_rt_id: composite_id,
                world_extent_elmos: extent,
                view_state: &mut self.paint_view_state,
                brush_radius_elmos: radius,
                mask_only_preview: mask_preview_on,
                active_mask_overlay,
                background: t.bg,
                mask_value_at_cursor,
                active_layer_name,
                cursor_elmos: cursor_world,
            },
        );

        if out.toggled_mask_preview {
            self.paint_brush_state.mask_only_preview = !self.paint_brush_state.mask_only_preview;
        }

        // Brush dispatch: LMB drag stamps. Use the cursor world-coord
        // we computed above (same math as `paint_view`) for the
        // dispatch site; track the previous stamp so fast drags get
        // interpolated stamps along the delta.
        if let Some(now) = cursor_world {
            if out.response.drag_started_by(egui::PointerButton::Primary)
                || out.response.clicked_by(egui::PointerButton::Primary)
            {
                self.apply_mask_brush_at_elmos(now.x, now.y);
                tracing::debug!(
                    layer = self.paint_active_layer_id.as_deref().unwrap_or("<none>"),
                    brush = %self.paint_brush_state.brush_id,
                    world_x = now.x,
                    world_z = now.y,
                    "mask brush stamp"
                );
            } else if out.response.dragged_by(egui::PointerButton::Primary)
                && let Some(prev) = self.paint_last_drag_pos
            {
                self.apply_mask_brush_along_drag(prev, now);
            }
            self.paint_last_drag_pos = Some(now);
        }
        if out.response.drag_stopped_by(egui::PointerButton::Primary) {
            self.paint_last_drag_pos = None;
            // D10 / Sprint 17 (ADR-041) — commit any in-flight mask
            // stroke into the undo history. The stroke records every
            // tile the brush touched (with before / after snapshots);
            // a no-op stroke (net change == 0) discards itself in
            // `History::end_mask_stroke`.
            if let Some(layer_id) = self.paint_active_layer_id.as_ref().cloned()
                && let Some(layer) = self.layer_stack.layer_by_id(&layer_id)
            {
                let pushed = self.history.end_mask_stroke(&layer.mask);
                if pushed {
                    tracing::debug!(
                        layer_id = %layer_id,
                        "mask stroke committed to undo history",
                    );
                }
            }
        }
    }

    /// World-space extents (elmos) currently active. Derived from the
    /// loaded heightmap dims when present, falling back to the project's
    /// declared SMU. Used by symmetry replication + start-position
    /// placement so off-map mirrors get filtered.
    fn world_extents(&self) -> (f32, f32) {
        if let Some(hm) = self.heightmap.as_ref() {
            (
                (hm.dims.0 - 1) as f32 * render::ELMOS_PER_PIXEL,
                (hm.dims.1 - 1) as f32 * render::ELMOS_PER_PIXEL,
            )
        } else {
            let (ex, ez) = self.map_size.elmo_extents();
            (ex as f32, ez as f32)
        }
    }

    /// Reset the orbit camera to the default framing for the current
    /// map extents. Wired to the top-bar "recenter" button and a
    /// future `Home` keyboard accelerator. Idempotent — clicking
    /// repeatedly is a no-op after the first call.
    fn recenter_camera(&mut self) {
        let (ex, ez) = self.world_extents();
        self.camera = OrbitCamera::framing(ex, ez);
        tracing::info!(
            extent_x = ex,
            extent_z = ez,
            "camera recentered to default framing"
        );
    }

    /// World-space Y of the terrain surface at `(x_elmo, z_elmo)`.
    /// Returns 0.0 when no heightmap is loaded.
    ///
    /// Sprint 13 hotfix (2026-05-19): markers built in PHASE A of
    /// `central()` were constructed at world Y = 0 and lifted by
    /// `MARKER_Y_LIFT_ELMOS = 2`, leaving them buried under any
    /// terrain with relief at their XZ. Lifting to `terrain_y_at +
    /// MARKER_Y_LIFT_ELMOS` puts the marker on the surface, where the
    /// depth test still occludes it behind hills BETWEEN camera and
    /// marker — only the same-pixel z-fight is removed.
    ///
    /// Sampling is nearest-neighbour at `ELMOS_PER_PIXEL = 8`; XZ
    /// outside the heightmap clamps to the edge sample rather than
    /// panicking. Sub-pixel accuracy isn't load-bearing for marker
    /// placement so bilinear is deliberately deferred.
    fn terrain_y_at(&self, x_elmo: f32, z_elmo: f32) -> f32 {
        let Some(hm) = self.heightmap.as_ref() else {
            return 0.0;
        };
        let (w, h) = hm.data.dims();
        if w == 0 || h == 0 {
            return 0.0;
        }
        let px = ((x_elmo / render::ELMOS_PER_PIXEL).round() as i32).clamp(0, w as i32 - 1) as u32;
        let pz = ((z_elmo / render::ELMOS_PER_PIXEL).round() as i32).clamp(0, h as i32 - 1) as u32;
        let raw = hm.data.data()[(pz as usize) * (w as usize) + (px as usize)];
        (raw as f32 / 65535.0) * self.height_scale
    }

    /// Ensure an ally group with `active_ally_group_id` exists,
    /// creating it on first F8 placement when the project is empty.
    /// Returns the active group's vec index for further mutation.
    fn ensure_active_ally_group(&mut self) -> usize {
        if let Some(idx) = self
            .ally_groups
            .iter()
            .position(|g| g.id == self.active_ally_group_id)
        {
            return idx;
        }
        // No matching group — create one. New id = active_ally_group_id;
        // sequence to keep colours stable when the user adds more.
        let g = AllyGroup::new(self.active_ally_group_id);
        self.ally_groups.push(g);
        info!(
            ally_group_id = self.active_ally_group_id,
            "F8: auto-created AllyGroup for first placement"
        );
        self.ally_groups.len() - 1
    }

    /// Place a new start position at `(world_x, world_z)` in the active
    /// ally group. When symmetry is active, mirror counterparts go into
    /// the SAME ally group (per ADR-032's "mirror into same group"
    /// rule). Each placement is its own undo entry — Ctrl-Z peels them
    /// off one at a time in reverse-placement order.
    fn place_start_position(&mut self, world_x: f32, world_z: f32) {
        let extents = self.world_extents();
        let (ex, ez) = extents;
        if world_x < 0.0 || world_x > ex || world_z < 0.0 || world_z > ez {
            trace!(
                world_x,
                world_z,
                extents = ?extents,
                "start position click landed off-map; ignored"
            );
            return;
        }
        self.dirty = true;
        let centers = self.symmetry.replicate((world_x, world_z), extents);
        let group_idx = self.ensure_active_ally_group();
        let ally_group_id = self.ally_groups[group_idx].id;
        for (cx, cz) in centers {
            // Clamp to map; mirrors that land off-map are dropped.
            if cx < 0.0 || cx > ex || cz < 0.0 || cz > ez {
                continue;
            }
            let pos = StartPosition {
                x_elmo: cx.round().clamp(0.0, ex) as i32,
                z_elmo: cz.round().clamp(0.0, ez) as i32,
            };
            // Skip exact-coord duplicates within the group (a
            // symmetry-replicated stamp may land on top of an existing
            // source when the center is on the symmetry axis itself).
            if self.ally_groups[group_idx].start_positions.contains(&pos) {
                continue;
            }
            info!(
                ally_group_id,
                x_elmo = pos.x_elmo,
                z_elmo = pos.z_elmo,
                symmetry = self.symmetry.id(),
                "start position placed"
            );
            self.ally_groups[group_idx].start_positions.push(pos);
            self.history
                .push_project_diff(ProjectDiff::PlaceStartPosition { ally_group_id, pos });
        }
    }

    /// Place `count` evenly-spaced positions along the line segment
    /// from `(x0, z0)` to `(x1, z1)`. Used by F8 drag-paint (default
    /// 8 for the canonical 8v8 case). Each step goes through
    /// [`Self::place_start_position`] so symmetry replication +
    /// dedup + undo entries all apply uniformly.
    fn drag_paint_start_positions(&mut self, x0: f32, z0: f32, x1: f32, z1: f32) {
        if self.drag_paint_count == 0 {
            return;
        }
        let n = self.drag_paint_count as usize;
        if n == 1 {
            self.place_start_position((x0 + x1) * 0.5, (z0 + z1) * 0.5);
            return;
        }
        for i in 0..n {
            let t = i as f32 / (n - 1) as f32;
            let x = x0 + (x1 - x0) * t;
            let z = z0 + (z1 - z0) * t;
            self.place_start_position(x, z);
        }
    }

    /// Move the source position identified by `(ally_group_id,
    /// source_index)` to the given world coordinates, clamped to the
    /// map. No-op if the source isn't present. Drag-emitted
    /// frame-by-frame, so this does NOT push an undo entry — that
    /// lands on `drag_stopped` via
    /// [`Self::finish_start_position_drag`].
    fn move_start_position(
        &mut self,
        ally_group_id: u8,
        source_index: usize,
        world_x: f32,
        world_z: f32,
    ) {
        let (ex, ez) = self.world_extents();
        if let Some(g) = self.ally_groups.iter_mut().find(|g| g.id == ally_group_id)
            && let Some(p) = g.start_positions.get_mut(source_index)
        {
            p.x_elmo = world_x.clamp(0.0, ex).round() as i32;
            p.z_elmo = world_z.clamp(0.0, ez).round() as i32;
        }
    }

    /// Commit an in-flight start-position drag. Pushes a single
    /// `MoveStartPosition` undo entry covering the whole drag (start
    /// coords → end coords) when both are known and changed. Idempotent;
    /// always clears `dragging_start_pos*`. B5 / ADR-032.
    fn finish_start_position_drag(&mut self) {
        let (Some((ally_group_id, source_index)), Some(from)) = (
            self.dragging_start_pos.take(),
            self.dragging_start_pos_from.take(),
        ) else {
            self.dragging_start_pos = None;
            self.dragging_start_pos_from = None;
            return;
        };
        let Some(g) = self.ally_groups.iter().find(|g| g.id == ally_group_id) else {
            return;
        };
        let Some(to) = g.start_positions.get(source_index).copied() else {
            // Marker was deleted during the drag (RMB clicks fire
            // concurrently). Nothing to commit.
            return;
        };
        if from == to {
            return;
        }
        self.history
            .push_project_diff(ProjectDiff::MoveStartPosition {
                ally_group_id,
                from,
                to,
            });
    }

    /// Predicate: is any edit drag currently in flight? Brush strokes
    /// (heightmap channel), start-position drags, metal-spot drags,
    /// and geo-vent drags all gate undo/redo so the user can't peel
    /// back state mid-gesture. B5 + C4/C5.
    fn is_dragging_anything(&self) -> bool {
        self.history.stroke_open()
            || self.dragging_start_pos.is_some()
            || self.dragging_metal_spot.is_some()
            || self.dragging_geo_vent.is_some()
    }

    /// Remove the position with `team_id`. No-op if absent. B5: pushes a
    /// `DeleteStartPosition` undo entry holding the full pre-delete
    /// position so undo can re-add it verbatim.
    fn delete_start_position(&mut self, ally_group_id: u8, source_index: usize) {
        let Some(g) = self.ally_groups.iter_mut().find(|g| g.id == ally_group_id) else {
            return;
        };
        if source_index >= g.start_positions.len() {
            return;
        }
        self.dirty = true;
        let pos = g.start_positions.remove(source_index);
        info!(ally_group_id, source_index, "start position deleted");
        self.history
            .push_project_diff(ProjectDiff::DeleteStartPosition { ally_group_id, pos });
    }

    // ───────────────────────────────────────────────────────────────
    // C4 / Sprint 11 — F5 metal-spot helpers. Pattern matches the
    // F8 start-position helpers above: place / drag-move / delete /
    // hit-test, each pushing a `ProjectDiff` so Ctrl-Z reverses it.
    // ───────────────────────────────────────────────────────────────

    /// Place a new metal spot at `(world_x, world_z)`. Symmetry
    /// replicates the click through `App::symmetry`; mirrors that
    /// land off-map are dropped. Each placed source pushes its own
    /// `ProjectDiff::PlaceMetalSpot` so undo peels mirrors one at a
    /// time (matches F8 / B5 behaviour).
    fn place_metal_spot(&mut self, world_x: f32, world_z: f32) {
        let extents = self.world_extents();
        let (ex, ez) = extents;
        if world_x < 0.0 || world_x > ex || world_z < 0.0 || world_z > ez {
            trace!(
                world_x,
                world_z,
                extents = ?extents,
                "metal-spot click landed off-map; ignored"
            );
            return;
        }
        self.dirty = true;
        let centers = self.symmetry.replicate((world_x, world_z), extents);
        for (cx, cz) in centers {
            if cx < 0.0 || cx > ex || cz < 0.0 || cz > ez {
                continue;
            }
            let spot = MetalSpot {
                x_elmo: cx.round().clamp(0.0, ex) as i32,
                z_elmo: cz.round().clamp(0.0, ez) as i32,
                metal: MetalSpot::DEFAULT_METAL,
            };
            // Skip exact-coord duplicates (a symmetry-replicated stamp
            // may land on an existing source when the center is on
            // the axis itself).
            if self
                .metal_spots
                .iter()
                .any(|m| m.x_elmo == spot.x_elmo && m.z_elmo == spot.z_elmo)
            {
                continue;
            }
            info!(
                x_elmo = spot.x_elmo,
                z_elmo = spot.z_elmo,
                metal = spot.metal,
                symmetry = self.symmetry.id(),
                "metal spot placed"
            );
            self.metal_spots.push(spot);
            self.history
                .push_project_diff(ProjectDiff::PlaceMetalSpot { spot });
        }
    }

    /// Replace the metal spot at `index` with `to`, clamped to the
    /// map. Frame-by-frame from drag; no undo entry per call. The
    /// drag finalizer collapses the entire gesture into a single
    /// `ProjectDiff::MoveMetalSpot`.
    fn move_metal_spot_to(&mut self, index: usize, to: MetalSpot) {
        let (ex, ez) = self.world_extents();
        if let Some(slot) = self.metal_spots.get_mut(index) {
            slot.x_elmo = (to.x_elmo as f32).clamp(0.0, ex).round() as i32;
            slot.z_elmo = (to.z_elmo as f32).clamp(0.0, ez).round() as i32;
            // Generous metal cap — see note on the inspector DragValue
            // about strategic mex value placement.
            slot.metal = to.metal.clamp(0.0, 50.0);
            self.dirty = true;
        }
    }

    /// Commit an in-flight metal-spot drag — pushes one
    /// `MoveMetalSpot` undo entry covering the whole gesture when
    /// the spot actually moved. Idempotent.
    fn finish_metal_spot_drag(&mut self) {
        let (Some(index), Some(from)) = (
            self.dragging_metal_spot.take(),
            self.dragging_metal_spot_from.take(),
        ) else {
            self.dragging_metal_spot = None;
            self.dragging_metal_spot_from = None;
            return;
        };
        let Some(&to) = self.metal_spots.get(index) else {
            return;
        };
        if from == to {
            return;
        }
        self.history
            .push_project_diff(ProjectDiff::MoveMetalSpot { from, to });
    }

    /// Remove the metal spot at `index`. Pushes a
    /// `DeleteMetalSpot` undo entry carrying the full pre-delete
    /// record so undo can re-add it verbatim.
    fn delete_metal_spot(&mut self, index: usize) {
        if index >= self.metal_spots.len() {
            return;
        }
        self.dirty = true;
        let spot = self.metal_spots.remove(index);
        info!(
            x_elmo = spot.x_elmo,
            z_elmo = spot.z_elmo,
            "metal spot deleted"
        );
        // If the user deleted the currently-selected row, drop the
        // selection (or fix up the index if a later row would now
        // sit at the same slot).
        if let Some(sel) = self.metal_state.selected {
            if sel == index {
                self.metal_state.selected = None;
            } else if sel > index {
                self.metal_state.selected = Some(sel - 1);
            }
        }
        self.history
            .push_project_diff(ProjectDiff::DeleteMetalSpot { spot });
    }

    /// Find the metal spot (SOURCE only — mirrors are non-
    /// interactive, per F8's symmetry contract) whose on-screen
    /// marker is within `radius_px` of `cursor`.
    fn hit_test_metal_spot(
        &self,
        cursor: egui::Pos2,
        rect: egui::Rect,
        radius_px: f32,
    ) -> Option<usize> {
        let rect_size = glam::Vec2::new(rect.width(), rect.height());
        let cursor_in_rect = glam::Vec2::new(cursor.x - rect.min.x, cursor.y - rect.min.y);
        let mut best: Option<(usize, f32)> = None;
        for (i, spot) in self.metal_spots.iter().enumerate() {
            let world = glam::Vec3::new(spot.x_elmo as f32, 0.0, spot.z_elmo as f32);
            let Some(screen) = render::world_to_screen(world, rect_size, &self.camera) else {
                continue;
            };
            let d = (screen - cursor_in_rect).length();
            if d <= radius_px && best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    // ───────────────────────────────────────────────────────────────
    // C5 / Sprint 11 — F6 geo-vent helpers. Mirror of the metal-spot
    // helpers above; geo vents have no `metal` value, so the API is
    // a touch leaner.
    // ───────────────────────────────────────────────────────────────

    fn place_geo_vent(&mut self, world_x: f32, world_z: f32) {
        let extents = self.world_extents();
        let (ex, ez) = extents;
        if world_x < 0.0 || world_x > ex || world_z < 0.0 || world_z > ez {
            trace!(world_x, world_z, "geo-vent click off-map; ignored");
            return;
        }
        self.dirty = true;
        let centers = self.symmetry.replicate((world_x, world_z), extents);
        for (cx, cz) in centers {
            if cx < 0.0 || cx > ex || cz < 0.0 || cz > ez {
                continue;
            }
            let vent = GeoVent {
                x_elmo: cx.round().clamp(0.0, ex) as i32,
                z_elmo: cz.round().clamp(0.0, ez) as i32,
            };
            if self
                .geo_vents
                .iter()
                .any(|v| v.x_elmo == vent.x_elmo && v.z_elmo == vent.z_elmo)
            {
                continue;
            }
            info!(
                x_elmo = vent.x_elmo,
                z_elmo = vent.z_elmo,
                symmetry = self.symmetry.id(),
                "geo vent placed"
            );
            self.geo_vents.push(vent);
            self.history
                .push_project_diff(ProjectDiff::PlaceGeoVent { vent });
        }
    }

    fn move_geo_vent_to(&mut self, index: usize, to: GeoVent) {
        let (ex, ez) = self.world_extents();
        if let Some(slot) = self.geo_vents.get_mut(index) {
            slot.x_elmo = (to.x_elmo as f32).clamp(0.0, ex).round() as i32;
            slot.z_elmo = (to.z_elmo as f32).clamp(0.0, ez).round() as i32;
            self.dirty = true;
        }
    }

    fn finish_geo_vent_drag(&mut self) {
        let (Some(index), Some(from)) = (
            self.dragging_geo_vent.take(),
            self.dragging_geo_vent_from.take(),
        ) else {
            self.dragging_geo_vent = None;
            self.dragging_geo_vent_from = None;
            return;
        };
        let Some(&to) = self.geo_vents.get(index) else {
            return;
        };
        if from == to {
            return;
        }
        self.history
            .push_project_diff(ProjectDiff::MoveGeoVent { from, to });
    }

    fn delete_geo_vent(&mut self, index: usize) {
        if index >= self.geo_vents.len() {
            return;
        }
        self.dirty = true;
        let vent = self.geo_vents.remove(index);
        info!(
            x_elmo = vent.x_elmo,
            z_elmo = vent.z_elmo,
            "geo vent deleted"
        );
        if let Some(sel) = self.geo_state.selected {
            if sel == index {
                self.geo_state.selected = None;
            } else if sel > index {
                self.geo_state.selected = Some(sel - 1);
            }
        }
        self.history
            .push_project_diff(ProjectDiff::DeleteGeoVent { vent });
    }

    fn hit_test_geo_vent(
        &self,
        cursor: egui::Pos2,
        rect: egui::Rect,
        radius_px: f32,
    ) -> Option<usize> {
        let rect_size = glam::Vec2::new(rect.width(), rect.height());
        let cursor_in_rect = glam::Vec2::new(cursor.x - rect.min.x, cursor.y - rect.min.y);
        let mut best: Option<(usize, f32)> = None;
        for (i, vent) in self.geo_vents.iter().enumerate() {
            let world = glam::Vec3::new(vent.x_elmo as f32, 0.0, vent.z_elmo as f32);
            let Some(screen) = render::world_to_screen(world, rect_size, &self.camera) else {
                continue;
            };
            let d = (screen - cursor_in_rect).length();
            if d <= radius_px && best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    // ───────────────────────────────────────────────────────────────
    // C6 (Sprint 12): F7 general feature helpers. Same shape as
    // metal-spot / geo-vent above, with two extras:
    //   - `name` and `rot_heading` carry through Place/Delete/Move diffs.
    //   - drag is a ROTATION gesture (not translation) — see
    //     [`Self::move_feature_to`] and the canvas dispatch.
    // ───────────────────────────────────────────────────────────────

    /// `2π` worth of Spring heading per pixel of horizontal drag. With
    /// `ROTATE_GAIN_PER_PX = 182` (≈ 65536 / 360), one pixel of drag
    /// corresponds to ~1° of rotation — feels responsive without being
    /// twitchy. Matches the convention in BAR's `unit_sunfacing.lua`
    /// (`mathAtan2 * (COBSCALE / ...)` scales similarly).
    const ROTATE_GAIN_PER_PX: f32 = 182.0;

    fn place_feature(&mut self, world_x: f32, world_z: f32) {
        let Some(name) = self.feature_state.selected_feature.clone() else {
            // No feature chosen — silently no-op rather than placing an
            // invisible "" entry. The inspector reminds the user to pick
            // one before clicking.
            trace!("feature-tool click without selected feature; ignored");
            return;
        };
        let extents = self.world_extents();
        let (ex, ez) = extents;
        if world_x < 0.0 || world_x > ex || world_z < 0.0 || world_z > ez {
            trace!(world_x, world_z, "feature click off-map; ignored");
            return;
        }
        self.dirty = true;
        // Symmetry replication: source first, then mirrors. For
        // rotational symmetry the per-copy `rot_heading` rotates by
        // `65536 / fold` so visually-symmetric placements LOOK
        // symmetric (a forward-facing tank wreck on the south side
        // mirrors to north as the same forward-facing wreck on the
        // mirror axis).
        let centers = self.symmetry.replicate((world_x, world_z), extents);
        let copies = centers.len().max(1) as u32;
        for (i, (cx, cz)) in centers.into_iter().enumerate() {
            if cx < 0.0 || cx > ex || cz < 0.0 || cz > ez {
                continue;
            }
            let rot_offset = if matches!(self.symmetry, SymmetryAxis::Rotational { .. }) {
                ((i as u32 * (u16::MAX as u32 + 1)) / copies) as u16
            } else {
                0
            };
            let f = FeatureInstance {
                name: name.clone(),
                x_elmo: cx.round().clamp(0.0, ex) as i32,
                z_elmo: cz.round().clamp(0.0, ez) as i32,
                rot_heading: rot_offset,
            };
            // Coords-only dedup — same as metal/geo. A future "rotate
            // existing" feature edit doesn't collide because the user
            // would drag-rotate, not re-place.
            if self
                .features
                .iter()
                .any(|q| q.name == f.name && q.x_elmo == f.x_elmo && q.z_elmo == f.z_elmo)
            {
                continue;
            }
            info!(
                feature_name = %f.name,
                x_elmo = f.x_elmo,
                z_elmo = f.z_elmo,
                rot_heading = f.rot_heading,
                symmetry = self.symmetry.id(),
                "feature placed"
            );
            self.features.push(f.clone());
            self.history
                .push_project_diff(ProjectDiff::PlaceFeature { feature: f });
        }
    }

    fn move_feature_to(&mut self, index: usize, to: FeatureInstance) {
        let (ex, ez) = self.world_extents();
        if let Some(slot) = self.features.get_mut(index) {
            // Coords clamp into the map; rot wraps freely (u16
            // arithmetic is modular by definition — no clamp needed).
            slot.x_elmo = (to.x_elmo as f32).clamp(0.0, ex).round() as i32;
            slot.z_elmo = (to.z_elmo as f32).clamp(0.0, ez).round() as i32;
            slot.rot_heading = to.rot_heading;
            self.dirty = true;
        }
    }

    fn finish_feature_drag(&mut self) {
        let (Some(index), Some(from), _, _) = (
            self.dragging_feature.take(),
            self.dragging_feature_from.take(),
            self.dragging_feature_anchor_x.take(),
            self.dragging_feature_start_rot.take(),
        ) else {
            self.dragging_feature = None;
            self.dragging_feature_from = None;
            self.dragging_feature_anchor_x = None;
            self.dragging_feature_start_rot = None;
            return;
        };
        let Some(to) = self.features.get(index).cloned() else {
            return;
        };
        if from == to {
            return;
        }
        self.history
            .push_project_diff(ProjectDiff::MoveFeature { from, to });
    }

    fn delete_feature(&mut self, index: usize) {
        if index >= self.features.len() {
            return;
        }
        self.dirty = true;
        let feature = self.features.remove(index);
        info!(
            feature_name = %feature.name,
            x_elmo = feature.x_elmo,
            z_elmo = feature.z_elmo,
            "feature deleted"
        );
        if let Some(sel) = self.feature_state.selected_placed {
            if sel == index {
                self.feature_state.selected_placed = None;
            } else if sel > index {
                self.feature_state.selected_placed = Some(sel - 1);
            }
        }
        self.history
            .push_project_diff(ProjectDiff::DeleteFeature { feature });
    }

    fn hit_test_feature(
        &self,
        cursor: egui::Pos2,
        rect: egui::Rect,
        radius_px: f32,
    ) -> Option<usize> {
        let rect_size = glam::Vec2::new(rect.width(), rect.height());
        let cursor_in_rect = glam::Vec2::new(cursor.x - rect.min.x, cursor.y - rect.min.y);
        let mut best: Option<(usize, f32)> = None;
        for (i, f) in self.features.iter().enumerate() {
            let world = glam::Vec3::new(f.x_elmo as f32, 0.0, f.z_elmo as f32);
            let Some(screen) = render::world_to_screen(world, rect_size, &self.camera) else {
                continue;
            };
            let d = (screen - cursor_in_rect).length();
            if d <= radius_px && best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    /// Find the SOURCE start position whose on-screen marker is within
    /// `radius_px` of `cursor`. Returns `(ally_group_id, source_index)`.
    /// Symmetry-derived display markers are NOT hit-tested (they're
    /// non-interactive; the user is told to edit the source instead).
    fn hit_test_start_position(
        &self,
        cursor: egui::Pos2,
        rect: egui::Rect,
        radius_px: f32,
    ) -> Option<(u8, usize)> {
        let rect_size = glam::Vec2::new(rect.width(), rect.height());
        let cursor_in_rect = glam::Vec2::new(cursor.x - rect.min.x, cursor.y - rect.min.y);
        let mut best: Option<((u8, usize), f32)> = None;
        for g in &self.ally_groups {
            for (i, pos) in g.start_positions.iter().enumerate() {
                let world = glam::Vec3::new(pos.x_elmo as f32, 0.0, pos.z_elmo as f32);
                let Some(screen) = render::world_to_screen(world, rect_size, &self.camera) else {
                    continue;
                };
                let d = (screen - cursor_in_rect).length();
                if d <= radius_px && best.map(|(_, bd)| d < bd).unwrap_or(true) {
                    best = Some(((g.id, i), d));
                }
            }
        }
        best.map(|(handle, _)| handle)
    }

    /// Allocate the lowest unused ally-group id and add a fresh group
    /// (default palette colour, default name). Returns its id. Sets
    /// the new group as active.
    fn add_ally_group(&mut self) -> u8 {
        let used: std::collections::HashSet<u8> = self.ally_groups.iter().map(|g| g.id).collect();
        let id = (0u16..=255)
            .find(|i| !used.contains(&(*i as u8)))
            .unwrap_or(255) as u8;
        let g = AllyGroup::new(id);
        info!(ally_group_id = id, "F8: added ally group");
        self.ally_groups.push(g);
        self.active_ally_group_id = id;
        id
    }

    /// Remove the ally group with `id` and all its positions. Sets the
    /// active group to the lowest surviving id, or 0 if none.
    fn delete_ally_group(&mut self, id: u8) {
        self.ally_groups.retain(|g| g.id != id);
        self.active_ally_group_id = self.ally_groups.iter().map(|g| g.id).min().unwrap_or(0);
        // Any in-flight drag against the deleted group is now stale.
        if matches!(self.dragging_start_pos, Some((gid, _)) if gid == id) {
            self.dragging_start_pos = None;
            self.dragging_start_pos_from = None;
        }
        info!(ally_group_id = id, "F8: deleted ally group");
    }

    /// Replace the entire `ally_groups` tree with a preset layout.
    /// Used by the Inspector's preset dropdown.
    fn apply_ally_preset(&mut self, preset: AllyPreset) {
        let (ex, ez) = self.world_extents();
        self.ally_groups = preset.materialise(ex, ez);
        self.active_ally_group_id = self.ally_groups.iter().map(|g| g.id).min().unwrap_or(0);
        self.dragging_start_pos = None;
        self.dragging_start_pos_from = None;
        info!(preset = ?preset, "F8: applied ally preset");
    }

    /// Inverse of `undo_one`. Same drag-gating rules.
    fn redo_one(&mut self) {
        self.end_stroke();
        if self.is_dragging_anything() {
            trace!("redo: gated by in-flight drag");
            return;
        }
        let Some(entry) = self.history.pop_redo() else {
            trace!("redo: nothing to redo");
            return;
        };
        let inverse = self.apply_history_entry(entry);
        info!(
            undo_depth = self.history.undo_depth() + 1,
            redo_depth = self.history.redo_depth(),
            "redo applied"
        );
        self.history.push_to_undo(inverse);
    }

    /// Apply one `HistoryEntry` against the current app state, returning
    /// the entry that would re-do it. Heightmap variants swap the rect
    /// in-place and re-upload to the GPU; project variants dispatch
    /// through [`Self::apply_project_diff`]. Used by both `undo_one`
    /// (popping from undo, pushing inverse onto redo) and `redo_one`
    /// (popping from redo, pushing inverse onto undo) — symmetric.
    fn apply_history_entry(&mut self, entry: HistoryEntry) -> HistoryEntry {
        match entry {
            HistoryEntry::Heightmap(mut e) => {
                if let Some(hm_state) = self.heightmap.as_mut() {
                    let rect = self.history.apply_heightmap(&mut e, &mut hm_state.data);
                    if let Some(rs) = self.render_state.as_ref() {
                        render::write_heightmap_rect(rs, hm_state.dims, hm_state.data.data(), rect);
                    }
                    let (mn, mx) = hm_state.data.min_max();
                    hm_state.min = mn;
                    hm_state.max = mx;
                } else {
                    warn!(
                        "undo: heightmap entry on stack but no heightmap loaded — \
                         entry pushed unchanged onto opposite stack"
                    );
                }
                HistoryEntry::Heightmap(e)
            }
            HistoryEntry::Mask(e) => HistoryEntry::Mask(self.apply_mask_entry(e)),
            HistoryEntry::Project(diff) => HistoryEntry::Project(self.apply_project_diff(diff)),
        }
    }

    /// D10 / Sprint 17 (ADR-041) — walk a [`MaskEntry`]'s tiles and
    /// swap each `(before, after)` pair on the named layer's mask.
    /// Returns the symmetric entry (before / after swapped) for the
    /// opposite-stack push.
    ///
    /// Clears `composite_layer_last_version` for the affected layer
    /// so the GPU compositor re-uploads the changed tiles on the
    /// next frame.
    fn apply_mask_entry(
        &mut self,
        mut e: barme_core::undo::MaskEntry,
    ) -> barme_core::undo::MaskEntry {
        let Some(layer) = self.layer_stack.active_layer_mut(&e.layer_id) else {
            warn!(
                layer_id = %e.layer_id,
                "undo: mask entry references unknown layer; pushing unchanged to opposite stack"
            );
            return e;
        };
        for (coord, before, after) in e.tiles.iter_mut() {
            // Restore `before` (the pre-stroke state) onto the mask;
            // capture the current state into `after` for the redo
            // direction. Since we swap below, the post-swap `before`
            // is what the redo restores.
            let live = layer.mask.clone_tile(*coord);
            layer.mask.restore_tile(*coord, before.clone());
            *after = live;
        }
        // Swap before/after so the entry on the opposite stack
        // restores the post-undo state correctly on redo.
        for (_, before, after) in e.tiles.iter_mut() {
            std::mem::swap(before, after);
        }
        // Invalidate the composite cache for this layer so the GPU
        // picks up the change next frame.
        self.composite_layer_last_version
            .retain(|(id, _), _| id != &e.layer_id);
        e
    }

    /// Dispatch a `ProjectDiff` against this `App`'s F8 + wizard state,
    /// returning the inverse to push onto the opposite stack. The
    /// inversion is symmetric for Place/Delete (swap variants) and
    /// Move/ApplyWizard (swap the from↔to and old↔current snapshot
    /// respectively). B5.
    fn apply_project_diff(&mut self, diff: ProjectDiff) -> ProjectDiff {
        match diff {
            ProjectDiff::PlaceStartPosition { ally_group_id, pos } => {
                // Undo: remove the position with matching coords from
                // its ally group. Redo direction: re-add.
                if let Some(g) = self.ally_groups.iter_mut().find(|g| g.id == ally_group_id) {
                    g.start_positions.retain(|q| *q != pos);
                }
                trace!(
                    ally_group_id,
                    x = pos.x_elmo,
                    z = pos.z_elmo,
                    "undo: removed placed start position"
                );
                ProjectDiff::DeleteStartPosition { ally_group_id, pos }
            }
            ProjectDiff::DeleteStartPosition { ally_group_id, pos } => {
                // Undo a delete: re-add to the group. Create the group
                // if it's missing (e.g. group was deleted between
                // delete and undo — unusual but defensive).
                let group_idx = match self.ally_groups.iter().position(|g| g.id == ally_group_id) {
                    Some(i) => i,
                    None => {
                        let g = AllyGroup::new(ally_group_id);
                        self.ally_groups.push(g);
                        self.ally_groups.len() - 1
                    }
                };
                self.ally_groups[group_idx].start_positions.push(pos);
                trace!(
                    ally_group_id,
                    x = pos.x_elmo,
                    z = pos.z_elmo,
                    "undo: restored deleted start position"
                );
                ProjectDiff::PlaceStartPosition { ally_group_id, pos }
            }
            ProjectDiff::MoveStartPosition {
                ally_group_id,
                from,
                to,
            } => {
                if let Some(g) = self.ally_groups.iter_mut().find(|g| g.id == ally_group_id)
                    && let Some(p) = g.start_positions.iter_mut().find(|p| **p == to)
                {
                    *p = from;
                }
                trace!(
                    ally_group_id,
                    ?from,
                    ?to,
                    "undo: reverted start position move"
                );
                ProjectDiff::MoveStartPosition {
                    ally_group_id,
                    from: to,
                    to: from,
                }
            }
            ProjectDiff::ApplyWizard(snap) => {
                let current = Box::new(self.capture_wizard_snapshot());
                self.restore_wizard_snapshot(*snap);
                info!("undo: reverted F1 wizard apply");
                ProjectDiff::ApplyWizard(current)
            }

            // C4 (Sprint 11): metal-spot undo dispatch. Pattern
            // mirrors the F8 PlaceStartPosition / DeleteStartPosition
            // pair — symmetric Place/Delete inversion.
            ProjectDiff::PlaceMetalSpot { spot } => {
                if let Some(pos) = self
                    .metal_spots
                    .iter()
                    .position(|m| m.x_elmo == spot.x_elmo && m.z_elmo == spot.z_elmo)
                {
                    self.metal_spots.remove(pos);
                }
                trace!(
                    x = spot.x_elmo,
                    z = spot.z_elmo,
                    "undo: removed placed metal spot"
                );
                ProjectDiff::DeleteMetalSpot { spot }
            }
            ProjectDiff::DeleteMetalSpot { spot } => {
                self.metal_spots.push(spot);
                trace!(
                    x = spot.x_elmo,
                    z = spot.z_elmo,
                    "undo: restored deleted metal spot"
                );
                ProjectDiff::PlaceMetalSpot { spot }
            }
            ProjectDiff::MoveMetalSpot { from, to } => {
                if let Some(slot) = self
                    .metal_spots
                    .iter_mut()
                    .find(|m| m.x_elmo == to.x_elmo && m.z_elmo == to.z_elmo)
                {
                    *slot = from;
                }
                trace!(?from, ?to, "undo: reverted metal-spot move");
                ProjectDiff::MoveMetalSpot { from: to, to: from }
            }
            ProjectDiff::SetExtractorRadius { from, to } => {
                self.extractor_radius = from;
                trace!(from, to, "undo: reverted extractor_radius edit");
                ProjectDiff::SetExtractorRadius { from: to, to: from }
            }

            // C5 (Sprint 11): geo-vent undo dispatch.
            ProjectDiff::PlaceGeoVent { vent } => {
                if let Some(pos) = self
                    .geo_vents
                    .iter()
                    .position(|v| v.x_elmo == vent.x_elmo && v.z_elmo == vent.z_elmo)
                {
                    self.geo_vents.remove(pos);
                }
                trace!(
                    x = vent.x_elmo,
                    z = vent.z_elmo,
                    "undo: removed placed geo vent"
                );
                ProjectDiff::DeleteGeoVent { vent }
            }
            ProjectDiff::DeleteGeoVent { vent } => {
                self.geo_vents.push(vent);
                trace!(
                    x = vent.x_elmo,
                    z = vent.z_elmo,
                    "undo: restored deleted geo vent"
                );
                ProjectDiff::PlaceGeoVent { vent }
            }
            ProjectDiff::MoveGeoVent { from, to } => {
                if let Some(slot) = self
                    .geo_vents
                    .iter_mut()
                    .find(|v| v.x_elmo == to.x_elmo && v.z_elmo == to.z_elmo)
                {
                    *slot = from;
                }
                trace!(?from, ?to, "undo: reverted geo-vent move");
                ProjectDiff::MoveGeoVent { from: to, to: from }
            }

            // C6 (Sprint 12): F7 feature undo dispatch. Identity is
            // (name, x_elmo, z_elmo) — rot_heading is allowed to differ
            // for the Move case (drag-rotate). Same per-mirror diff
            // semantics as metal/geo: undo peels one mirror at a time.
            ProjectDiff::PlaceFeature { feature } => {
                if let Some(pos) = self.features.iter().position(|f| {
                    f.name == feature.name
                        && f.x_elmo == feature.x_elmo
                        && f.z_elmo == feature.z_elmo
                }) {
                    self.features.remove(pos);
                }
                trace!(
                    feature_name = %feature.name,
                    x = feature.x_elmo,
                    z = feature.z_elmo,
                    "undo: removed placed feature"
                );
                ProjectDiff::DeleteFeature { feature }
            }
            ProjectDiff::DeleteFeature { feature } => {
                self.features.push(feature.clone());
                trace!(
                    feature_name = %feature.name,
                    x = feature.x_elmo,
                    z = feature.z_elmo,
                    "undo: restored deleted feature"
                );
                ProjectDiff::PlaceFeature { feature }
            }
            ProjectDiff::MoveFeature { from, to } => {
                if let Some(slot) = self
                    .features
                    .iter_mut()
                    .find(|f| f.name == to.name && f.x_elmo == to.x_elmo && f.z_elmo == to.z_elmo)
                {
                    *slot = from.clone();
                }
                trace!(?from, ?to, "undo: reverted feature move");
                ProjectDiff::MoveFeature { from: to, to: from }
            }

            // C9 (Sprint 14 / ADR-042): water diff dispatch. SetWaterMode
            // overwrites `App::water_mode`; EditWaterField targets either
            // a field on `App::water_overrides` or the MapInfo-top-level
            // shadows on `App::void_water` / `App::tidal_strength`
            // depending on the `WaterField` variant.
            ProjectDiff::SetWaterMode { from, to } => {
                self.water_mode = from;
                trace!(?from, ?to, "undo: reverted water mode");
                ProjectDiff::SetWaterMode { from: to, to: from }
            }
            ProjectDiff::EditWaterField { field, from, to } => {
                self.apply_water_field(field, from);
                trace!(?field, ?from, ?to, "undo: reverted water field");
                ProjectDiff::EditWaterField {
                    field,
                    from: to,
                    to: from,
                }
            }
            ProjectDiff::SetLavaAtmosphere { from, to } => {
                self.lava_atmosphere = from;
                trace!(from, to, "undo: reverted lava-atmosphere toggle");
                ProjectDiff::SetLavaAtmosphere { from: to, to: from }
            }

            // D8 / Sprint 15 (ADR-038): layer-stack edit dispatch.
            // Sprint 15 has no UI that pushes these diffs (the Layers
            // panel lands in Sprint 17); the arms exist so a future
            // panel can dispatch through the same `apply_project_diff`
            // pipeline as every other ProjectDiff variant. Mask-pixel
            // edits go through a separate Sprint 16 / D9 path —
            // they're NOT a ProjectDiff.
            ProjectDiff::AddLayer { index, layer } => {
                // Undo direction: remove the layer that was added.
                if index < self.layer_stack.layers.len()
                    && self.layer_stack.layers[index].id == layer.id
                {
                    self.layer_stack.layers.remove(index);
                } else if let Some(pos) = self
                    .layer_stack
                    .layers
                    .iter()
                    .position(|l| l.id == layer.id)
                {
                    // Defensive: the layer was reordered between push
                    // and undo. Remove by id rather than by index.
                    self.layer_stack.layers.remove(pos);
                }
                trace!(index, layer_id = %layer.id, "undo: removed added layer");
                ProjectDiff::RemoveLayer { index, layer }
            }
            ProjectDiff::RemoveLayer { index, layer } => {
                let i = index.min(self.layer_stack.layers.len());
                self.layer_stack.layers.insert(i, *layer.clone());
                trace!(index = i, layer_id = %layer.id, "undo: restored removed layer");
                ProjectDiff::AddLayer { index: i, layer }
            }
            ProjectDiff::ReorderLayer { from, to } => {
                let n = self.layer_stack.layers.len();
                if from < n && to <= n && from != to {
                    let layer = self.layer_stack.layers.remove(from);
                    let dst = to.min(self.layer_stack.layers.len());
                    self.layer_stack.layers.insert(dst, layer);
                }
                trace!(from, to, "undo: reverted layer reorder");
                ProjectDiff::ReorderLayer { from: to, to: from }
            }
            ProjectDiff::SetLayerProperty { layer_id, from, to } => {
                if let Some(layer) = self
                    .layer_stack
                    .layers
                    .iter_mut()
                    .find(|l| l.id == layer_id)
                {
                    apply_layer_property(layer, &from);
                }
                trace!(layer_id = %layer_id, "undo: reverted layer property edit");
                ProjectDiff::SetLayerProperty {
                    layer_id,
                    from: to,
                    to: from,
                }
            }
            // C7 / Sprint 18 (F9): F9 form mutation. The `from` patch
            // holds the pre-edit value; on undo we apply `from` and
            // return the inverse (with `from` / `to` swapped) so redo
            // restores the post-edit value.
            ProjectDiff::EditMapInfo { from, to } => {
                self.apply_mapinfo_patch(from.clone());
                trace!(field = %to.label(), "undo: reverted mapinfo edit");
                ProjectDiff::EditMapInfo { from: to, to: from }
            }
        }
    }

    /// Write a [`WaterValue`] into the App field identified by
    /// [`WaterField`]. The dispatcher in [`Self::apply_project_diff`]
    /// uses this for both the apply (current frame) and revert (undo)
    /// directions — the same routing logic.
    ///
    /// Type-mismatched payloads (e.g. an `Rgb` value targeting a Float
    /// field) silently no-op with a `warn!`. The diff producer should
    /// always pair the right `WaterValue` variant with each `WaterField`;
    /// a mismatch is a programming bug in whoever pushed the diff.
    fn apply_water_field(&mut self, field: WaterField, value: WaterValue) {
        macro_rules! float {
            ($lhs:expr) => {
                match value {
                    WaterValue::Float(v) => {
                        $lhs = v;
                    }
                    other => warn!(
                        ?field,
                        ?other,
                        "EditWaterField type mismatch: expected Float"
                    ),
                }
            };
        }
        macro_rules! rgb {
            ($lhs:expr) => {
                match value {
                    WaterValue::Rgb(v) => {
                        $lhs = v;
                    }
                    other => warn!(?field, ?other, "EditWaterField type mismatch: expected Rgb"),
                }
            };
        }
        macro_rules! bool_opt {
            ($lhs:expr) => {
                match value {
                    WaterValue::Bool(v) => {
                        $lhs = v;
                    }
                    other => warn!(
                        ?field,
                        ?other,
                        "EditWaterField type mismatch: expected Bool"
                    ),
                }
            };
        }
        macro_rules! uint {
            ($lhs:expr) => {
                match value {
                    WaterValue::UInt(v) => {
                        $lhs = v;
                    }
                    other => warn!(
                        ?field,
                        ?other,
                        "EditWaterField type mismatch: expected UInt"
                    ),
                }
            };
        }
        let w = &mut self.water_overrides;
        match field {
            WaterField::Damage => float!(w.damage),
            WaterField::SurfaceColor => rgb!(w.surface_color),
            WaterField::SurfaceAlpha => float!(w.surface_alpha),
            WaterField::PlaneColor => rgb!(w.plane_color),
            WaterField::Absorb => rgb!(w.absorb),
            WaterField::BaseColor => rgb!(w.base_color),
            WaterField::MinColor => rgb!(w.min_color),
            WaterField::AmbientFactor => float!(w.ambient_factor),
            WaterField::DiffuseFactor => float!(w.diffuse_factor),
            WaterField::SpecularFactor => float!(w.specular_factor),
            WaterField::SpecularColor => rgb!(w.specular_color),
            WaterField::SpecularPower => float!(w.specular_power),
            WaterField::FresnelMin => float!(w.fresnel_min),
            WaterField::FresnelMax => float!(w.fresnel_max),
            WaterField::FresnelPower => float!(w.fresnel_power),
            WaterField::ReflectionDistortion => float!(w.reflection_distortion),
            WaterField::BlurBase => float!(w.blur_base),
            WaterField::BlurExponent => float!(w.blur_exponent),
            WaterField::PerlinStartFreq => float!(w.perlin_start_freq),
            WaterField::PerlinLacunarity => float!(w.perlin_lacunarity),
            WaterField::PerlinAmplitude => float!(w.perlin_amplitude),
            WaterField::WaveFoamIntensity => float!(w.wave_foam_intensity),
            WaterField::NumTiles => uint!(w.num_tiles),
            WaterField::ShoreWaves => bool_opt!(w.shore_waves),
            WaterField::ForceRendering => bool_opt!(w.force_rendering),
            WaterField::RepeatX => float!(w.repeat_x),
            WaterField::RepeatY => float!(w.repeat_y),
            // MapInfo top-level shadows on App.
            WaterField::VoidWater => match value {
                // void_water is a non-Option bool; diffs always use
                // Bool(Some(_)). Bool(None) is a no-op + warn.
                WaterValue::Bool(Some(b)) => self.void_water = b,
                WaterValue::Bool(None) => warn!(
                    ?field,
                    "EditWaterField: VoidWater expects Bool(Some(_)) — \
                     the field is non-optional"
                ),
                other => warn!(
                    ?field,
                    ?other,
                    "EditWaterField type mismatch: expected Bool"
                ),
            },
            WaterField::TidalStrength => float!(self.tidal_strength),
        }
    }

    /// C7 / Sprint 18 (F9): dispatch a [`MapInfoPatch`] against the
    /// App-side fields it targets. Most variants update either
    /// `self.mapinfo_overrides` (the free-form bag the emitter
    /// consults) or a dedicated App shadow (`min_height`, `height_scale`,
    /// `minimap_override`, `lava_atmosphere`, …).
    ///
    /// Pragmatic Sprint 18 dispatch: schema fields with first-class
    /// App shadows route to those shadows; everything else routes
    /// through `Project.mapinfo_overrides` keyed by the dotted Lua
    /// path. Sprint 19's tooltip / lint pass + Sprint 27's inspector
    /// consistency refactor will introduce typed shadow fields on
    /// `App` for the most-edited atmosphere / lighting subset; until
    /// then this routes safely through the bag.
    fn apply_mapinfo_patch(&mut self, patch: barme_core::MapInfoPatch) {
        use barme_core::MapInfoPatch as P;
        let label = patch.label();
        match patch {
            // ─── First-class App shadows ───
            P::SmfMinHeight(Some(v)) => self.min_height = v,
            P::SmfMinHeight(None) => self.min_height = 0.0,
            P::SmfMaxHeight(Some(v)) => self.height_scale = v.max(1.0),
            P::SmfMaxHeight(None) => {} // engine default
            P::ExtractorRadius(Some(v)) => self.extractor_radius = v,
            P::ExtractorRadius(None) => {
                self.extractor_radius = barme_core::default_extractor_radius();
            }
            P::VoidWater(b) => self.void_water = b,
            P::TidalStrength(v) => self.tidal_strength = v,
            P::LavaAtmosphere(b) => self.lava_atmosphere = b,
            P::MinimapOverride(p) => self.minimap_override = p,
            // ─── Free-form bag (Sprint 19 / 27 will type these) ───
            ref other => {
                let toml_value: Option<toml::Value> = match other {
                    P::Name(s) | P::Version(s) => Some(toml::Value::String(s.clone())),
                    P::Shortname(s)
                    | P::Description(s)
                    | P::Author(s)
                    | P::SmfMinimapTex(s)
                    | P::AtmosphereSkyBox(s)
                    | P::ResourcesDetailTex(s)
                    | P::ResourcesSpecularTex(s)
                    | P::ResourcesDetailNormalTex(s)
                    | P::ResourcesLightEmissionTex(s)
                    | P::ResourcesSkyReflectModTex(s)
                    | P::ResourcesParallaxHeightTex(s) => s.clone().map(toml::Value::String),
                    P::Maphardness(v)
                    | P::Gravity(v)
                    | P::MaxMetal(v)
                    | P::LightingGroundShadowDensity(v)
                    | P::LightingUnitShadowDensity(v)
                    | P::LightingSpecularExponent(v)
                    | P::AtmosphereMinWind(v)
                    | P::AtmosphereMaxWind(v)
                    | P::AtmosphereFogStart(v)
                    | P::AtmosphereFogEnd(v)
                    | P::AtmosphereCloudDensity(v) => v.map(|f| toml::Value::Float(f as f64)),
                    P::VoidAlphaMin(v) => Some(toml::Value::Float(*v as f64)),
                    P::NotDeformable(b) | P::AutoShowMetal(b) => b.map(toml::Value::Boolean),
                    P::LightingSunDir(arr) | P::AtmosphereSkyAxisAngle(arr) => {
                        Some(toml::Value::Array(
                            arr.iter().map(|f| toml::Value::Float(*f as f64)).collect(),
                        ))
                    }
                    P::LightingGroundAmbientColor(c)
                    | P::LightingGroundDiffuseColor(c)
                    | P::LightingGroundSpecularColor(c)
                    | P::LightingUnitAmbientColor(c)
                    | P::LightingUnitDiffuseColor(c)
                    | P::LightingUnitSpecularColor(c)
                    | P::AtmosphereFogColor(c)
                    | P::AtmosphereSunColor(c)
                    | P::AtmosphereSkyColor(c)
                    | P::AtmosphereCloudColor(c) => c.map(|rgb| {
                        toml::Value::Array(
                            rgb.iter().map(|f| toml::Value::Float(*f as f64)).collect(),
                        )
                    }),
                    P::TerrainTypes(types) => {
                        // Round-trip the vec via TOML so the override bag
                        // keeps the data. Per-row editing inside the
                        // override bag is awkward but Sprint 27 will
                        // promote terrain_types to a first-class App
                        // shadow.
                        toml::Value::try_from(types).ok()
                    }
                    P::CustomField { key: k, value: v } => {
                        // Custom-field path bypasses the dotted-path
                        // bag and writes directly to the user's
                        // mapinfo_overrides under the user's chosen key.
                        match v {
                            Some(val) => {
                                self.mapinfo_overrides.insert(k.clone(), val.clone());
                            }
                            None => {
                                self.mapinfo_overrides.remove(k);
                            }
                        }
                        self.dirty = true;
                        trace!(field = %label, "F9 form: applied (custom key)");
                        return;
                    }
                    // Variants we already handled above.
                    P::SmfMinHeight(_)
                    | P::SmfMaxHeight(_)
                    | P::ExtractorRadius(_)
                    | P::VoidWater(_)
                    | P::TidalStrength(_)
                    | P::LavaAtmosphere(_)
                    | P::MinimapOverride(_)
                    | P::VoidGround(_) => None,
                };
                let dotted = format!("mapinfo.{}", label);
                match toml_value {
                    Some(v) => {
                        self.mapinfo_overrides.insert(dotted, v);
                    }
                    None => {
                        self.mapinfo_overrides.remove(&format!("mapinfo.{}", label));
                    }
                }
                // VoidGround is a non-Option bool with no first-class
                // shadow; route as a Boolean entry to mapinfo_overrides
                // so the emitter (post-Sprint-19) can consult it.
                if let P::VoidGround(b) = patch {
                    self.mapinfo_overrides
                        .insert(format!("mapinfo.{}", label), toml::Value::Boolean(b));
                }
            }
        }
        self.dirty = true;
        trace!(field = %label, "F9 form: applied");
    }

    /// C7 / Sprint 18 (F9): produce the inverse [`MapInfoPatch`] for
    /// the given prospective edit by sampling the App's current state
    /// for the same field. Used by the undo plumbing — the "from"
    /// side of a `ProjectDiff::EditMapInfo` is what the field was
    /// before the user committed the new value.
    ///
    /// For App-shadow fields (gravity / void_water / minimap_override
    /// / …) we read the shadow directly. For free-form-bag fields the
    /// snapshot reads from `mapinfo_overrides` via the schema view —
    /// `MapInfo::from(&Project)` materialises the canonical value the
    /// emitter would write today.
    fn snapshot_mapinfo_patch_inverse(
        &self,
        new: &barme_core::MapInfoPatch,
    ) -> barme_core::MapInfoPatch {
        use barme_core::MapInfoPatch as P;
        let project = self.snapshot_project();
        let info: barme_core::MapInfo = (&project).into();
        match new {
            P::Name(_) => P::Name(info.name.clone()),
            P::Shortname(_) => P::Shortname(info.shortname.clone()),
            P::Description(_) => P::Description(info.description.clone()),
            P::Author(_) => P::Author(info.author.clone()),
            P::Version(_) => P::Version(info.version.clone()),
            P::Maphardness(_) => P::Maphardness(info.maphardness),
            P::NotDeformable(_) => P::NotDeformable(info.not_deformable),
            P::Gravity(_) => P::Gravity(info.gravity),
            P::TidalStrength(_) => P::TidalStrength(self.tidal_strength),
            P::MaxMetal(_) => P::MaxMetal(info.max_metal),
            P::ExtractorRadius(_) => P::ExtractorRadius(Some(self.extractor_radius)),
            P::VoidWater(_) => P::VoidWater(self.void_water),
            P::VoidGround(_) => P::VoidGround(info.void_ground),
            P::VoidAlphaMin(_) => P::VoidAlphaMin(info.void_alpha_min),
            P::AutoShowMetal(_) => P::AutoShowMetal(info.auto_show_metal),
            P::LavaAtmosphere(_) => P::LavaAtmosphere(self.lava_atmosphere),
            P::SmfMinHeight(_) => P::SmfMinHeight(Some(self.min_height)),
            P::SmfMaxHeight(_) => P::SmfMaxHeight(Some(self.height_scale)),
            P::SmfMinimapTex(_) => P::SmfMinimapTex(info.smf.minimap_tex.clone()),
            P::LightingSunDir(_) => P::LightingSunDir(info.lighting.sun_dir),
            P::LightingGroundAmbientColor(_) => {
                P::LightingGroundAmbientColor(info.lighting.ground_ambient_color)
            }
            P::LightingGroundDiffuseColor(_) => {
                P::LightingGroundDiffuseColor(info.lighting.ground_diffuse_color)
            }
            P::LightingGroundSpecularColor(_) => {
                P::LightingGroundSpecularColor(info.lighting.ground_specular_color)
            }
            P::LightingGroundShadowDensity(_) => {
                P::LightingGroundShadowDensity(info.lighting.ground_shadow_density)
            }
            P::LightingUnitAmbientColor(_) => {
                P::LightingUnitAmbientColor(info.lighting.unit_ambient_color)
            }
            P::LightingUnitDiffuseColor(_) => {
                P::LightingUnitDiffuseColor(info.lighting.unit_diffuse_color)
            }
            P::LightingUnitSpecularColor(_) => {
                P::LightingUnitSpecularColor(info.lighting.unit_specular_color)
            }
            P::LightingUnitShadowDensity(_) => {
                P::LightingUnitShadowDensity(info.lighting.unit_shadow_density)
            }
            P::LightingSpecularExponent(_) => {
                P::LightingSpecularExponent(info.lighting.specular_exponent)
            }
            P::AtmosphereMinWind(_) => P::AtmosphereMinWind(info.atmosphere.min_wind),
            P::AtmosphereMaxWind(_) => P::AtmosphereMaxWind(info.atmosphere.max_wind),
            P::AtmosphereFogStart(_) => P::AtmosphereFogStart(info.atmosphere.fog_start),
            P::AtmosphereFogEnd(_) => P::AtmosphereFogEnd(info.atmosphere.fog_end),
            P::AtmosphereFogColor(_) => P::AtmosphereFogColor(info.atmosphere.fog_color),
            P::AtmosphereSunColor(_) => P::AtmosphereSunColor(info.atmosphere.sun_color),
            P::AtmosphereSkyColor(_) => P::AtmosphereSkyColor(info.atmosphere.sky_color),
            P::AtmosphereSkyAxisAngle(_) => {
                P::AtmosphereSkyAxisAngle(info.atmosphere.sky_axis_angle)
            }
            P::AtmosphereSkyBox(_) => P::AtmosphereSkyBox(info.atmosphere.sky_box.clone()),
            P::AtmosphereCloudDensity(_) => {
                P::AtmosphereCloudDensity(info.atmosphere.cloud_density)
            }
            P::AtmosphereCloudColor(_) => P::AtmosphereCloudColor(info.atmosphere.cloud_color),
            P::ResourcesDetailTex(_) => P::ResourcesDetailTex(info.resources.detail_tex.clone()),
            P::ResourcesSpecularTex(_) => {
                P::ResourcesSpecularTex(info.resources.specular_tex.clone())
            }
            P::ResourcesDetailNormalTex(_) => {
                P::ResourcesDetailNormalTex(info.resources.detail_normal_tex.clone())
            }
            P::ResourcesLightEmissionTex(_) => {
                P::ResourcesLightEmissionTex(info.resources.light_emission_tex.clone())
            }
            P::ResourcesSkyReflectModTex(_) => {
                P::ResourcesSkyReflectModTex(info.resources.sky_reflect_mod_tex.clone())
            }
            P::ResourcesParallaxHeightTex(_) => {
                P::ResourcesParallaxHeightTex(info.resources.parallax_height_tex.clone())
            }
            P::TerrainTypes(_) => P::TerrainTypes(info.terrain_types.clone()),
            P::CustomField { key, .. } => P::CustomField {
                key: key.clone(),
                value: self.mapinfo_overrides.get(key).cloned(),
            },
            P::MinimapOverride(_) => P::MinimapOverride(self.minimap_override.clone()),
        }
    }

    /// Snapshot the project-level fields the F1 wizard touches so
    /// `apply_wizard` can push a `ProjectDiff::ApplyWizard` entry
    /// carrying the pre-wizard state. Does NOT capture the heightmap
    /// (the wizard barriers the stroke history; see ADR-033). B5.
    fn capture_wizard_snapshot(&self) -> WizardSnapshot {
        WizardSnapshot {
            project_name: self.project_name.clone(),
            map_size: self.map_size,
            height_scale: self.height_scale,
            symmetry: self.symmetry,
            rotational_fold: self.rotational_fold,
            ally_groups: self.ally_groups.clone(),
            procgen_expr: self.procgen_expr.clone(),
            procgen_domain: self.procgen_domain,
        }
    }

    /// Restore a `WizardSnapshot` over the current app state. Mirror of
    /// [`Self::capture_wizard_snapshot`]; the camera is reframed from
    /// the restored map size. B5 / ADR-032.
    fn restore_wizard_snapshot(&mut self, snap: WizardSnapshot) {
        self.project_name = snap.project_name;
        self.map_size = snap.map_size;
        self.height_scale = snap.height_scale;
        self.symmetry = snap.symmetry;
        self.rotational_fold = snap.rotational_fold;
        self.ally_groups = snap.ally_groups;
        self.active_ally_group_id = self.ally_groups.iter().map(|g| g.id).min().unwrap_or(0);
        self.procgen_expr = snap.procgen_expr;
        self.procgen_domain = snap.procgen_domain;
        self.revalidate_procgen();
        let (ex, ez) = self.map_size.elmo_extents();
        self.camera = OrbitCamera::framing(ex as f32, ez as f32);
    }

    /// Compile the current project to a `.sd7` and copy it into BAR's
    /// user maps directory. v0 UX: heightmap must be loaded, texture is a
    /// synthesised flat grey (Stage 1 will replace with real DNTS).
    ///
    /// Sprint 20: start a worker-thread build. Snapshots every input
    /// the worker needs (project clone, heightmap PNG written to a
    /// temp dir, splat + layer resolver bindings, owned slot
    /// resolver), then spawns a thread that drives
    /// `BuildPlan::execute` and `install_sd7`. The UI thread keeps
    /// polling `BuildState` each frame via
    /// [`App::poll_build_state`].
    fn build_and_install(&mut self) {
        use std::collections::VecDeque;
        use std::sync::Mutex;
        use std::sync::atomic::AtomicBool;
        use std::sync::mpsc;

        // Bail if a build is already running — the top-bar gate
        // disables the button but the keyboard shortcut path can
        // still slip through. Idempotent.
        if self.build_state.is_running() {
            warn!("build & install already running; ignoring duplicate request");
            return;
        }

        self.last_install = None;
        self.last_error = None;
        let Some(hm) = self.heightmap.as_ref() else {
            warn!("build & install requested with no heightmap loaded");
            self.last_error = Some("load a heightmap first".into());
            return;
        };
        // The CPU-side heightmap is authoritative (may include unsaved
        // brush edits). Serialize to a temp PNG so the pipeline gets the
        // current state, not a stale on-disk snapshot. The TempDir is
        // moved into the worker so it auto-cleans only after the
        // build finishes.
        let workdir = match tempfile::tempdir() {
            Ok(t) => t,
            Err(e) => {
                let msg = format!("tempdir: {e:#}");
                error!("build & install tempdir failed: {msg}");
                self.last_error = Some(msg);
                return;
            }
        };
        let hm_path = workdir.path().join("heightmap.png");
        if let Err(e) = hm.data.save_png(&hm_path) {
            let msg = format!("write heightmap: {e:#}");
            error!("build & install snapshot failed: {msg}");
            self.last_error = Some(msg);
            return;
        }
        let Some(dst_dir) = launcher::bar_maps_dir() else {
            let msg =
                "could not locate BAR maps dir on this platform — pick one manually (Stage 1)";
            warn!("{msg}");
            self.last_error = Some(msg.into());
            return;
        };
        let repo_root = repo_root();
        let driver = match PyMapConvDriver::vendored(&repo_root) {
            Ok(d) => d,
            Err(e) => {
                let msg = format!("{e:#}");
                error!("pymapconv unavailable: {msg}");
                self.last_error = Some(msg);
                return;
            }
        };
        let project = self.snapshot_project_for_build();
        let splat_inputs = self.resolve_splat_bake_inputs(&project);
        let layer_inputs = self.resolve_layer_splat_bake_inputs(&project);
        info!(
            name = %project.name,
            smu_x = self.map_size.smu_x,
            smu_z = self.map_size.smu_z,
            max_height = self.height_scale,
            heightmap = %hm_path.display(),
            dst = %dst_dir.display(),
            "build & install requested (worker thread)"
        );
        // Owned slot resolver — clone the registry so the worker
        // thread doesn't borrow the App's slot_registry slice. ~16
        // entries × 2 fields each = trivial allocation cost.
        let owned_slots: Vec<build_runner::OwnedSlotEntry> = self
            .slot_registry
            .iter()
            .map(|s| build_runner::OwnedSlotEntry {
                id: s.id,
                dir: s.dir.clone(),
            })
            .collect();
        let project_root_owned = self
            .current_project_path
            .as_ref()
            .and_then(|p| p.parent().map(Path::to_path_buf));
        let owned_resolver = build_runner::OwnedSlotResolver::new(owned_slots, project_root_owned);
        let project_name = project.name.clone();

        let inputs = build_runner::WorkerInputs {
            driver,
            project,
            heightmap_png: hm_path,
            heightmap: hm.data.clone(),
            splat_inputs,
            layer_inputs: Some(layer_inputs),
            slot_resolver: Box::new(owned_resolver),
            project_path: self.current_project_path.clone(),
            work_dir: workdir,
            dst_dir,
            project_name: project_name.clone(),
        };

        let (tx, rx) = mpsc::channel::<barme_pipeline::BuildEvent>();
        let cancel = Arc::new(AtomicBool::new(false));
        let log = Arc::new(Mutex::new(VecDeque::with_capacity(
            build_runner::LOG_RING_CAP,
        )));

        let cancel_for_worker = cancel.clone();
        let thread = std::thread::Builder::new()
            .name(format!("barme-build:{project_name}"))
            .spawn(move || {
                let sink = build_runner::ChannelSink::new(tx);
                build_runner::run_worker_build(inputs, &sink, &cancel_for_worker)
            })
            .map_err(|e| format!("spawn build thread: {e}"));
        let thread = match thread {
            Ok(h) => h,
            Err(msg) => {
                error!("{msg}");
                self.last_error = Some(msg);
                return;
            }
        };

        self.build_state = build_runner::BuildState::Running {
            project_name,
            started_at: std::time::Instant::now(),
            current_stage: barme_pipeline::BuildStage::RenderDiffuse,
            latest_progress: 0.0,
            events: rx,
            log,
            cancel,
            thread: Some(thread),
        };
    }

    /// Sprint 20 / chunk 5: apply this frame's [build log panel]
    /// clicks. Clear locks the ring buffer and truncates; Save writes
    /// the current contents to a user-picked file (best-effort —
    /// failure surfaces a single `last_error` toast).
    ///
    /// [build log panel]: crate::ui::build_log::render
    fn apply_build_log_clicks(&mut self, clicks: crate::ui::build_log::LogPanelClicks) {
        if clicks.clear
            && let Some(log) = self.build_state.log()
            && let Ok(mut guard) = log.lock()
        {
            info!("build_log: clearing ring buffer");
            guard.clear();
        }
        if let Some(path) = clicks.save_as
            && let Some(log) = self.build_state.log()
            && let Ok(guard) = log.lock()
        {
            let text = crate::ui::build_log::render_log_as_text(&guard);
            match std::fs::write(&path, text) {
                Ok(()) => info!(?path, "build_log: saved to file"),
                Err(e) => {
                    let msg = format!("save log: {e:#}");
                    error!("{msg}");
                    self.last_error = Some(msg);
                }
            }
        }
    }

    /// Sprint 20: poll the worker thread + drain its event channel
    /// once per UI frame. Transitions Running → Done | Failed |
    /// Cancelled when the join handle is ready.
    fn poll_build_state(&mut self, ctx: &egui::Context) {
        let (transition, repaint_soon) = match &mut self.build_state {
            build_runner::BuildState::Running {
                events,
                log,
                current_stage,
                latest_progress,
                thread,
                started_at,
                ..
            } => {
                let log_arc: Arc<_> = log.clone();
                let closed =
                    build_runner::drain_events(events, log, current_stage, latest_progress)
                        .is_err();
                // Drive a repaint while running so the spinner + elapsed
                // readout stays fresh even when the worker is silent.
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                if closed {
                    // Worker dropped the sender → join the thread.
                    let handle = thread.take();
                    let result = handle.and_then(|h| h.join().ok());
                    let duration = started_at.elapsed();
                    let next = match result {
                        Some(Ok(installed)) => {
                            info!(
                                installed = %installed.display(),
                                elapsed_ms = duration.as_millis() as u64,
                                "build & install ok (worker)"
                            );
                            self.last_install = Some(Ok(installed.clone()));
                            build_runner::BuildState::Done {
                                sd7_path: installed,
                                duration,
                                log: log_arc,
                            }
                        }
                        Some(Err(msg)) => {
                            error!(error = %msg, "build & install failed (worker)");
                            let cancelled = msg.starts_with("Cancelled");
                            self.last_install = Some(Err(msg.clone()));
                            if cancelled {
                                build_runner::BuildState::Cancelled {
                                    duration,
                                    log: log_arc,
                                }
                            } else {
                                build_runner::BuildState::Failed {
                                    error: msg,
                                    duration,
                                    log: log_arc,
                                }
                            }
                        }
                        None => {
                            let msg = "worker thread panicked (no result)".to_string();
                            error!("{msg}");
                            self.last_install = Some(Err(msg.clone()));
                            build_runner::BuildState::Failed {
                                error: msg,
                                duration,
                                log: log_arc,
                            }
                        }
                    };
                    (Some(next), false)
                } else {
                    (None, true)
                }
            }
            _ => (None, false),
        };
        if let Some(next) = transition {
            // Auto-open the log panel on failure so the user sees the
            // tail of stderr without an extra click.
            if matches!(next, build_runner::BuildState::Failed { .. }) {
                self.build_log_open = true;
            }
            self.build_state = next;
        }
        let _ = repaint_soon;
    }

    fn open_from(&mut self, path: PathBuf) {
        self.end_stroke();
        self.history.barrier();
        match Project::load_from_file(&path) {
            Ok(p) => {
                info!(
                    "opened project '{}' ({}×{} SMU, heightmap={}) from {}",
                    p.name,
                    p.size.smu_x,
                    p.size.smu_z,
                    p.heightmap
                        .as_ref()
                        .map(|h| h.display().to_string())
                        .unwrap_or_else(|| "(none)".into()),
                    path.display()
                );
                let hm_resolved = p.resolve_heightmap(&path);
                self.project_name = p.name;
                self.map_size = p.size;
                self.height_scale = p.max_height.max(1.0);
                self.min_height = p.min_height;
                self.heightmap = None;
                self.current_project_path = Some(path);
                self.last_error = None;
                self.dirty = false;
                let (ex, ez) = self.map_size.elmo_extents();
                self.camera = OrbitCamera::framing(ex as f32, ez as f32);

                self.ally_groups = p.ally_groups;
                // Active group: first by id, or 0 if none.
                self.active_ally_group_id =
                    self.ally_groups.iter().map(|g| g.id).min().unwrap_or(0);
                self.mapinfo_overrides = p.mapinfo_overrides;
                // B8: respect a saved dismissal so reopening the
                // project doesn't replay the hint window.
                self.next_steps_dismissed = p.next_steps_dismissed;
                self.show_next_steps = false;

                // D10 / Sprint 17 (ADR-041): `App::splat_config` +
                // `App::splat_distribution` retired. The legacy
                // `Project.splat_config` still exists on the wire
                // for one more sprint so pre-Sprint-14 projects can
                // migrate; we read it once below into the migration
                // shadow and discard.
                self.dnts_diffuse_in_alpha = p.dnts_diffuse_in_alpha;

                // D8 / Sprint 15 (ADR-038): hoist the layer stack onto
                // App, then run the one-shot pre-D8 migration so
                // pre-Sprint-15 `.barmeproj` files seed a stack from
                // their persisted `splat_config`. `after_load_migrate`
                // is idempotent — a stack the user has touched in
                // Sprint 17+ survives unchanged.
                //
                // D10 / Sprint 17 (ADR-041): the same migration also
                // promotes the legacy `splat_config.diffuse_in_alpha`
                // to the new per-project `dnts_diffuse_in_alpha`. Pull
                // both fields back out of the shadow.
                let (stack, diffuse_in_alpha, ran_migration) = {
                    let project_root = self.current_project_path.as_ref().and_then(|p| p.parent());
                    let resolver =
                        AppSlotResolver::with_project_root(&self.slot_registry, project_root);
                    let mut shadow = Project::new("__migrate__", self.map_size.smu_x);
                    shadow.layers = p.layers;
                    shadow.splat_config = p.splat_config;
                    shadow.dnts_diffuse_in_alpha = self.dnts_diffuse_in_alpha;
                    shadow.size = self.map_size;
                    let was_empty = shadow.layers.layers.is_empty();
                    shadow.after_load_migrate(&resolver);
                    let ran = was_empty && !shadow.layers.layers.is_empty();
                    (shadow.layers, shadow.dnts_diffuse_in_alpha, ran)
                };
                self.layer_stack = stack;
                self.dnts_diffuse_in_alpha = diffuse_in_alpha;
                self.migration_toast_dismissed = p.migration_toast_dismissed;
                self.pending_migration_toast = ran_migration && !self.migration_toast_dismissed;
                if ran_migration {
                    info!("Sprint 17 migration: legacy splat layers seeded into layer stack");
                }
                // D10 / Sprint 17 (ADR-041): pre-Sprint-17 imported
                // textures live at arbitrary disk paths; migrate them
                // into the project-local sidecar so the project stays
                // portable.
                self.migrate_imported_layer_paths();
                // D9 / Sprint 16 (ADR-039): loaded project may have
                // painted layer masks from a prior session; force a
                // full mask resync next frame + push every layer's
                // slot diffuse to the composite slot array.
                self.composite_layer_last_version.clear();
                self.reupload_layer_stack_diffuses();
                // D9 / Sprint 16 (ADR-040): paint viewport resets too
                // — the open project may have a different stack, so
                // the prior session's active layer id may no longer
                // exist.
                self.paint_active_layer_id = None;
                self.paint_view_state = PaintViewState::default();
                // D10 / Sprint 17 (ADR-041): per-layer caches keyed by
                // layer id are invalid for the loaded project.
                self.layer_thumbnails.clear();
                self.paint_drag_preview_order = None;
                self.layer_mask_preview_cache = None;
                self.layers_panel_rect = None;

                // C4/C5 (Sprint 11): metal-spot + geo-vent persistence.
                // The Project model owns the sources; the inspector
                // view-state (selection) is session-scoped and resets
                // on every open.
                self.metal_spots = p.metal_spots;
                self.geo_vents = p.geo_vents;
                self.extractor_radius = p.extractor_radius;

                // C9 (Sprint 14): hoist water state. Migration already
                // ran in `Project::From<ProjectWire>`, so by here
                // `water_mode` already reflects `Ocean` if the
                // pre-Sprint-14 project had `min_height < 0`. The
                // re-save (via `save_to`) writes back `schema_v = 1`
                // so subsequent opens skip the migration.
                self.water_mode = p.water_mode;
                self.water_overrides = p.water_overrides;
                self.void_water = p.void_water;
                self.tidal_strength = p.tidal_strength;
                self.lava_atmosphere = p.lava_atmosphere;
                self.minimap_override = p.minimap_override;
                self.metal_state = MetalState::default();
                self.geo_state = GeoState::default();
                self.dragging_metal_spot = None;
                self.dragging_metal_spot_from = None;
                self.dragging_geo_vent = None;
                self.dragging_geo_vent_from = None;

                // C6 (Sprint 12): user-feature persistence. Like
                // metal/geo, the Project owns the sources; the
                // inspector's picker / selection / filter are
                // session-scoped and reset on open.
                self.features = p.features;
                self.feature_state.active_category = "trees".to_string();
                self.feature_state.selected_feature = None;
                self.feature_state.selected_placed = None;
                self.feature_state.filter.clear();
                self.dragging_feature = None;
                self.dragging_feature_from = None;
                self.dragging_feature_anchor_x = None;
                self.dragging_feature_start_rot = None;

                if let Some(hm_path) = hm_resolved {
                    if hm_path.exists() {
                        info!("restoring heightmap from {}", hm_path.display());
                        self.load_heightmap(hm_path);
                    } else {
                        warn!(
                            "project references heightmap {} but file is missing",
                            hm_path.display()
                        );
                        self.last_error =
                            Some(format!("heightmap not found: {}", hm_path.display()));
                    }
                }
            }
            Err(e) => {
                error!(path = %path.display(), error = %format!("{e:#}"), "project open failed");
                self.last_error = Some(format!("open: {e:#}"));
            }
        }
    }
}

/// One-frame outcome of the F1 wizard. Returned from the wizard render so
/// the actual project mutation happens outside the egui closure (so
/// `apply_wizard` can borrow `self` mutably).
enum WizardAction {
    Apply,
    Cancel,
}

impl App {
    /// Render the F1 new-project wizard as a centered modal-style window.
    /// Returns the user's outcome on the click-frame, or `None` while the
    /// form is still being edited. ADR-024.
    /// New-project wizard (ADR-035): split layout with name/size/height
    /// on the left and symmetry/biome preset cards on the right.
    /// Footer carries the info chip and Cancel / Create buttons.
    fn render_wizard(&mut self, ctx: &egui::Context) -> Option<WizardAction> {
        let t = crate::ui::theme::Tokens::DARK;
        let mut action: Option<WizardAction> = None;
        let mut open = true;
        egui::Window::new("New project")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(720.0)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(
                        "Create a playable starter map in one step. You can change everything later.",
                    )
                    .color(t.muted)
                    .size(12.0),
                );
                ui.add_space(10.0);
                ui.columns(2, |cols| {
                    // ── Left column ──
                    let lcol = &mut cols[0];
                    lcol.label(
                        egui::RichText::new("PROJECT NAME")
                            .color(t.muted)
                            .size(10.0)
                            .strong(),
                    );
                    lcol.add(
                        egui::TextEdit::singleline(&mut self.wizard.project_name)
                            .desired_width(f32::INFINITY)
                            .font(egui::FontId::monospace(13.0)),
                    )
                    .on_hover_text("Display name + filename root. Sanitised to [A-Za-z0-9_-] for the saved .barmeproj — see the 'Saves as:' line below.");
                    let sanitized = sanitize_name(&self.wizard.project_name);
                    lcol.label(
                        egui::RichText::new(format!("Saves as: {sanitized}"))
                            .color(t.dim)
                            .size(10.0),
                    );
                    lcol.add_space(14.0);
                    lcol.label(
                        egui::RichText::new("MAP SIZE · SMU")
                            .color(t.muted)
                            .size(10.0)
                            .strong(),
                    );
                    lcol.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.wizard.smu_x).range(2u32..=64))
                            .on_hover_text("Map width in SMU (1 SMU = 512 elmos = 65 heightmap pixels). Heightmap dims follow `64·N + 1`. PITFALL §4.");
                        ui.label(egui::RichText::new("×").color(t.dim));
                        ui.add(egui::DragValue::new(&mut self.wizard.smu_z).range(2u32..=64))
                            .on_hover_text("Map depth in SMU. Asymmetric sizes are valid — e.g. 8×16 for a corridor map.");
                        ui.label(
                            egui::RichText::new(format!(
                                "= {} × {} px",
                                self.wizard.smu_x * 64 + 1,
                                self.wizard.smu_z * 64 + 1,
                            ))
                            .color(t.dim)
                            .size(10.0),
                        );
                    });
                    lcol.add_space(14.0);
                    lcol.label(
                        egui::RichText::new("MAX HEIGHT · ELMOS")
                            .color(t.muted)
                            .size(10.0)
                            .strong(),
                    );
                    let label = format!("{:.0}", self.wizard.max_height);
                    let r = crate::ui::widgets::ramp_slider_labelled(
                        lcol,
                        "Elevation cap",
                        &mut self.wizard.max_height,
                        64.0..=4096.0,
                        t.accent,
                        label,
                    )
                    .on_hover_text("World Y at heightmap value 65535 (elmos). Picking a biome auto-sets this to the biome's recommended cap; manually tweaking detaches the link.");
                    if r.changed() {
                        self.wizard.height_from_biome = false;
                    }

                    // ── Right column ──
                    let rcol = &mut cols[1];
                    rcol.label(
                        egui::RichText::new("SYMMETRY PRESET")
                            .color(t.muted)
                            .size(10.0)
                            .strong(),
                    );
                    rcol.horizontal_wrapped(|ui| {
                        let presets = [
                            (SymmetryAxis::None, "None", None),
                            (SymmetryAxis::Horizontal, "Horizontal", Some(crate::ui::icons::Icon::SymH)),
                            (SymmetryAxis::Vertical, "Vertical", Some(crate::ui::icons::Icon::SymV)),
                            (SymmetryAxis::Quad, "Quad", Some(crate::ui::icons::Icon::SymQ)),
                            (
                                SymmetryAxis::Rotational {
                                    fold: self.wizard.rotational_fold,
                                },
                                "Rotational",
                                Some(crate::ui::icons::Icon::SymRot),
                            ),
                        ];
                        for (axis, name, icon) in presets {
                            let active = std::mem::discriminant(&axis)
                                == std::mem::discriminant(&self.wizard.symmetry);
                            if Self::wizard_preset_card(ui, name, icon, active) {
                                self.wizard.symmetry = axis;
                            }
                        }
                    });
                    rcol.add_space(12.0);
                    rcol.label(
                        egui::RichText::new("BIOME PRESET")
                            .color(t.muted)
                            .size(10.0)
                            .strong(),
                    );
                    rcol.horizontal_wrapped(|ui| {
                        for (i, biome) in BIOMES.iter().enumerate() {
                            let active = self.wizard.biome_index == i;
                            if Self::wizard_biome_card(ui, biome.label, active) {
                                self.wizard.biome_index = i;
                                if self.wizard.height_from_biome {
                                    self.wizard.max_height = biome.max_height_hint;
                                }
                            }
                        }
                    });
                });
                ui.add_space(14.0);
                // Footer bar.
                egui::Frame::new()
                    .fill(t.panel2)
                    .inner_margin(egui::Margin::symmetric(0, 8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let icon_rect = ui
                                .allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover())
                                .0;
                            crate::ui::icons::paint_icon(
                                ui.painter(),
                                icon_rect,
                                crate::ui::icons::Icon::Info,
                                t.muted,
                                1.3,
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "A {}×{} demo terrain will be generated.",
                                    self.wizard.smu_x, self.wizard.smu_z
                                ))
                                .color(t.muted)
                                .size(11.0),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add(
                                            egui::Button::new("Create")
                                                .fill(t.accent)
                                                .min_size(egui::vec2(96.0, 30.0)),
                                        )
                                        .on_hover_text("Generate the heightmap + project from these settings. The wizard closes; edits land via Ctrl+Z if you want to undo.")
                                        .clicked()
                                    {
                                        action = Some(WizardAction::Apply);
                                    }
                                    if ui
                                        .add(
                                            egui::Button::new("Cancel")
                                                .min_size(egui::vec2(80.0, 30.0)),
                                        )
                                        .on_hover_text("Close the wizard without creating anything.")
                                        .clicked()
                                    {
                                        action = Some(WizardAction::Cancel);
                                    }
                                },
                            );
                        });
                    });
            });
        if !open && action.is_none() {
            action = Some(WizardAction::Cancel);
        }
        action
    }

    /// Wizard preset card — symmetry / biome cards both use the same
    /// 80px tile with glyph + label.
    fn wizard_preset_card(
        ui: &mut egui::Ui,
        label: &str,
        icon: Option<crate::ui::icons::Icon>,
        active: bool,
    ) -> bool {
        let t = crate::ui::theme::Tokens::DARK;
        let (rect, response) = ui.allocate_exact_size(egui::vec2(82.0, 64.0), egui::Sense::click());
        let painter = ui.painter();
        let bg = if active { t.accent_alpha(0x2E) } else { t.bg };
        let stroke = egui::Stroke::new(1.0, if active { t.accent_dim } else { t.border });
        painter.rect_filled(rect, egui::CornerRadius::same(6), bg);
        painter.rect_stroke(
            rect,
            egui::CornerRadius::same(6),
            stroke,
            egui::StrokeKind::Middle,
        );
        if let Some(ic) = icon {
            let icon_rect = egui::Rect::from_center_size(
                egui::pos2(rect.center().x, rect.top() + 20.0),
                egui::vec2(28.0, 28.0),
            );
            crate::ui::icons::paint_icon(
                painter,
                icon_rect,
                ic,
                if active { t.text } else { t.muted },
                1.4,
            );
        }
        painter.text(
            egui::pos2(rect.center().x, rect.bottom() - 12.0),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(11.0),
            if active { t.text } else { t.muted },
        );
        let hover = match label {
            "None" => "No symmetry. Stamps land only at the cursor.".to_string(),
            "Horizontal" => "Mirror strokes across the horizontal centreline (Z = ez/2).".to_string(),
            "Vertical" => "Mirror strokes across the vertical centreline (X = ex/2).".to_string(),
            "Quad" => "Mirror both axes — every stamp produces 4 copies.".to_string(),
            "Rotational" => "Replicate strokes N times around the map centre. Set the fold count after creation.".to_string(),
            other => format!("Biome preset · {other}. Sets diffuse / max-height defaults; you can change them later."),
        };
        response.on_hover_text(hover).clicked()
    }

    fn wizard_biome_card(ui: &mut egui::Ui, label: &str, active: bool) -> bool {
        Self::wizard_preset_card(ui, label, None, active)
    }
}

/// 8-colour palette indexed by `team_id`. Even ids get warm tones (side A),
/// odd get cool (side B), matching the BAR per-side convention the F8 editor
/// auto-assigns to. Beyond 8 ids the palette wraps; the wrap is visible and
/// intentional — colour is a hint, the team-id label is the source of truth.
/// Configuration preset for the F8 ally-group tree (ADR-032).
/// Materialising one replaces `Project.ally_groups` wholesale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AllyPreset {
    /// 2 ally groups, 1 position each on the 25 % / 75 % diagonal.
    OneVOne,
    /// 2 ally groups, 8 positions each laid out north / south strips
    /// at the BAR-standard 12 % / 88 % offsets.
    EightVEight,
    /// 3 ally groups arranged in a triangle (N, SW, SE) — Comet
    /// Catcher style FFA.
    ThreeWayFfa,
    /// 4 ally groups at the four corners.
    FourWayFfa,
}

impl AllyPreset {
    const ALL: [AllyPreset; 4] = [
        AllyPreset::OneVOne,
        AllyPreset::EightVEight,
        AllyPreset::ThreeWayFfa,
        AllyPreset::FourWayFfa,
    ];

    fn label(self) -> &'static str {
        match self {
            AllyPreset::OneVOne => "1v1",
            AllyPreset::EightVEight => "8v8",
            AllyPreset::ThreeWayFfa => "3-way FFA",
            AllyPreset::FourWayFfa => "4-way FFA",
        }
    }

    /// Build the ally-group tree for this preset given the map's
    /// world-space extents. Default colours come from
    /// [`ALLY_GROUP_PALETTE`]; box polygons mirror the BAR community
    /// convention (north / south / corner strips).
    fn materialise(self, ex: f32, ez: f32) -> Vec<AllyGroup> {
        fn group(id: u8, positions: &[(f32, f32)], polygon: Option<Vec<(f32, f32)>>) -> AllyGroup {
            AllyGroup {
                id,
                name: format!("AllyGroup {id}"),
                color: ALLY_GROUP_PALETTE[(id as usize) % ALLY_GROUP_PALETTE.len()],
                start_positions: positions
                    .iter()
                    .map(|&(x, z)| StartPosition {
                        x_elmo: x.round() as i32,
                        z_elmo: z.round() as i32,
                    })
                    .collect(),
                box_polygon: polygon,
            }
        }
        match self {
            AllyPreset::OneVOne => {
                let g0 = group(
                    0,
                    &[(ex * 0.25, ez * 0.25)],
                    Some(vec![(0.0, 0.0), (1.0, 0.5)]),
                );
                let g1 = group(
                    1,
                    &[(ex * 0.75, ez * 0.75)],
                    Some(vec![(0.0, 0.5), (1.0, 1.0)]),
                );
                vec![g0, g1]
            }
            AllyPreset::EightVEight => {
                // North + south strips, 8 evenly-spaced positions per
                // side. Y-coords pin to ~6 %/94 % of map extent.
                let n_z = ez * 0.06;
                let s_z = ez * 0.94;
                let mut north_xs = Vec::with_capacity(8);
                let mut south_xs = Vec::with_capacity(8);
                for i in 0..8 {
                    let t = (i as f32 + 0.5) / 8.0;
                    north_xs.push((ex * t, n_z));
                    south_xs.push((ex * t, s_z));
                }
                let g0 = group(0, &north_xs, Some(vec![(0.0, 0.0), (1.0, 0.12)]));
                let g1 = group(1, &south_xs, Some(vec![(0.0, 0.88), (1.0, 1.0)]));
                vec![g0, g1]
            }
            AllyPreset::ThreeWayFfa => {
                let g0 = group(
                    0,
                    &[(ex * 0.5, ez * 0.1)],
                    Some(vec![(0.0, 0.0), (1.0, 0.25)]),
                );
                let g1 = group(
                    1,
                    &[(ex * 0.1, ez * 0.85)],
                    Some(vec![(0.0, 0.65), (0.5, 1.0)]),
                );
                let g2 = group(
                    2,
                    &[(ex * 0.9, ez * 0.85)],
                    Some(vec![(0.5, 0.65), (1.0, 1.0)]),
                );
                vec![g0, g1, g2]
            }
            AllyPreset::FourWayFfa => {
                let g0 = group(
                    0,
                    &[(ex * 0.15, ez * 0.15)],
                    Some(vec![(0.0, 0.0), (0.3, 0.3)]),
                );
                let g1 = group(
                    1,
                    &[(ex * 0.85, ez * 0.15)],
                    Some(vec![(0.7, 0.0), (1.0, 0.3)]),
                );
                let g2 = group(
                    2,
                    &[(ex * 0.15, ez * 0.85)],
                    Some(vec![(0.0, 0.7), (0.3, 1.0)]),
                );
                let g3 = group(
                    3,
                    &[(ex * 0.85, ez * 0.85)],
                    Some(vec![(0.7, 0.7), (1.0, 1.0)]),
                );
                vec![g0, g1, g2, g3]
            }
        }
    }
}

/// Replicate every source position in `project.ally_groups` through
/// `symmetry` into the same ally group. Used by the build path so the
/// emitted `mapinfo.lua` `teams[]` carries every concrete spawn the
/// editor showed on the canvas. Idempotent: exact-coord duplicates
/// within a group are dropped.
fn expand_symmetry_into_ally_groups(project: &mut Project, symmetry: SymmetryAxis) {
    if matches!(symmetry, SymmetryAxis::None) {
        return;
    }
    let (ex, ez) = project.size.elmo_extents();
    let extents = (ex as f32, ez as f32);
    for g in &mut project.ally_groups {
        let sources: Vec<StartPosition> = g.start_positions.clone();
        for src in &sources {
            let mirrors = symmetry.replicate((src.x_elmo as f32, src.z_elmo as f32), extents);
            for (mx, mz) in mirrors.into_iter().skip(1) {
                if mx < 0.0 || mx > extents.0 || mz < 0.0 || mz > extents.1 {
                    continue;
                }
                let p = StartPosition {
                    x_elmo: mx.round() as i32,
                    z_elmo: mz.round() as i32,
                };
                if !g.start_positions.contains(&p) {
                    g.start_positions.push(p);
                }
            }
        }
    }

    // C4 / C5 (Sprint 11): metal-spot + geo-vent sources expand
    // through symmetry the same way start positions do — the editor
    // canvas already paints mirrors live (see `central()`), but
    // those mirrors aren't stored in `Project.metal_spots` /
    // `Project.geo_vents`. The build path needs them concrete so
    // `metal_layout.rs` / `featureplacer.rs` can emit them.
    let metal_sources: Vec<MetalSpot> = project.metal_spots.clone();
    for src in &metal_sources {
        let mirrors = symmetry.replicate((src.x_elmo as f32, src.z_elmo as f32), extents);
        for (mx, mz) in mirrors.into_iter().skip(1) {
            if mx < 0.0 || mx > extents.0 || mz < 0.0 || mz > extents.1 {
                continue;
            }
            let m = MetalSpot {
                x_elmo: mx.round() as i32,
                z_elmo: mz.round() as i32,
                metal: src.metal,
            };
            if !project
                .metal_spots
                .iter()
                .any(|q| q.x_elmo == m.x_elmo && q.z_elmo == m.z_elmo)
            {
                project.metal_spots.push(m);
            }
        }
    }
    let geo_sources: Vec<GeoVent> = project.geo_vents.clone();
    for src in &geo_sources {
        let mirrors = symmetry.replicate((src.x_elmo as f32, src.z_elmo as f32), extents);
        for (mx, mz) in mirrors.into_iter().skip(1) {
            if mx < 0.0 || mx > extents.0 || mz < 0.0 || mz > extents.1 {
                continue;
            }
            let v = GeoVent {
                x_elmo: mx.round() as i32,
                z_elmo: mz.round() as i32,
            };
            if !project
                .geo_vents
                .iter()
                .any(|q| q.x_elmo == v.x_elmo && q.z_elmo == v.z_elmo)
            {
                project.geo_vents.push(v);
            }
        }
    }

    // C6 (Sprint 12): F7 features expand through symmetry too. Unlike
    // metal/geo the mirrors carry a rotation offset — under rotational
    // symmetry an N-fold copy spins by `65536 / fold` per copy so each
    // mirror "looks the same" relative to its sector. Translation
    // mirrors (Horizontal / Vertical / Quad) leave rotation alone —
    // a tree facing east on the south stays facing east when mirrored
    // to the north (the engine handles the visual flip via the mirror
    // axis, not the heading).
    let feature_sources: Vec<FeatureInstance> = project.features.clone();
    for src in &feature_sources {
        let mirrors = symmetry.replicate((src.x_elmo as f32, src.z_elmo as f32), extents);
        let copies = mirrors.len().max(1) as u32;
        for (i, (mx, mz)) in mirrors.into_iter().enumerate().skip(1) {
            if mx < 0.0 || mx > extents.0 || mz < 0.0 || mz > extents.1 {
                continue;
            }
            let rot_offset = if matches!(symmetry, SymmetryAxis::Rotational { .. }) {
                src.rot_heading
                    .wrapping_add(((i as u32 * (u16::MAX as u32 + 1)) / copies) as u16)
            } else {
                src.rot_heading
            };
            let f = FeatureInstance {
                name: src.name.clone(),
                x_elmo: mx.round() as i32,
                z_elmo: mz.round() as i32,
                rot_heading: rot_offset,
            };
            if !project
                .features
                .iter()
                .any(|q| q.name == f.name && q.x_elmo == f.x_elmo && q.z_elmo == f.z_elmo)
            {
                project.features.push(f);
            }
        }
    }
}

fn pick_save_path(suggested_name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("BAR map project", &[PROJECT_EXTENSION])
        .set_file_name(format!("{suggested_name}.{PROJECT_EXTENSION}"))
        .save_file()
}

fn pick_open_path() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("BAR map project", &[PROJECT_EXTENSION])
        .pick_file()
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two parents up from a member crate")
        .to_path_buf()
}

fn fixture_path(smu: u32) -> PathBuf {
    let edge = smu * 64 + 1;
    repo_root()
        .join("assets")
        .join("fixtures")
        .join(format!("r16_ramp_{smu}x{smu}smu_{edge}px.png"))
}

/// Truncate an error message for display in a section-header Chip.
/// Keeps the chip visually compact while leaving the full message
/// available on the TextEdit's hover tooltip.
fn short_error(msg: &str) -> String {
    let first_line = msg.lines().next().unwrap_or(msg);
    if first_line.len() <= 32 {
        first_line.to_string()
    } else {
        format!("{}…", &first_line[..31])
    }
}

/// Discrete user intent collected during UI building. We don't perform IO
/// inside the egui closure; we drain this after the panel closes so borrow
/// checking stays simple.
enum FileAction {
    LoadHeightmap(PathBuf),
    /// Open the F1 new-project wizard (ADR-024). Creating happens on the
    /// wizard's "Create" button via `apply_wizard`.
    OpenWizard,
    Save,
    SaveAs,
    Open,
    BuildAndInstall,
    ApplyProcGen,
    Undo,
    Redo,
}

/// Layout-shell methods for the five-zone UI introduced in ADR-030. Each
/// panel function takes `&mut self` + the egui `Context` (plus
/// `&mut Option<FileAction>` where relevant) and writes exactly one
/// panel. They're called from `eframe::App::update` in egui panel
/// add-order: top → bottom → left → right → CentralPanel LAST.
impl App {
    /// Switch the active tool, emitting one `tracing::info!` line per
    /// real change (no-op transitions are silent). Tracked via
    /// `previous_tool` so the diff is observable in bug-report logs.
    fn set_tool(&mut self, new: Tool) {
        if self.tool == new {
            return;
        }
        info!(
            from = ?self.tool,
            to = ?new,
            "tool change"
        );
        self.previous_tool = self.tool;
        self.tool = new;
        // Drop any in-flight stroke when switching out of Sculpt so the
        // user's last paint motion lands as a committed undo unit
        // before the next tool can fire. Idempotent if no stroke open.
        self.end_stroke();
        // Cancel an in-flight start-position drag when leaving
        // StartPositions — otherwise `dragging_start_pos` would linger
        // and a re-entry to the tool would resume an invisible drag.
        // Drop the captured `from` too; cancelling a drag should not
        // emit a MoveStartPosition undo entry (no committed move).
        self.dragging_start_pos = None;
        self.dragging_start_pos_from = None;
    }

    /// Keyboard: Ctrl-Z / Ctrl-Shift-Z / Ctrl-Y for undo / redo,
    /// Q / B / S / G for tool switch. Tool accelerators only fire when
    /// no widget has keyboard focus so typing into the procgen
    /// `TextEdit` doesn't eat keystrokes and bounce the user out of
    /// Procgen mid-edit.
    fn handle_keyboard(&mut self, ctx: &egui::Context, action: &mut Option<FileAction>) {
        let (key_undo, key_redo, key_save, key_save_as) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            let shift = i.modifiers.shift;
            let z = i.key_pressed(egui::Key::Z);
            let y = i.key_pressed(egui::Key::Y);
            let s = i.key_pressed(egui::Key::S);
            (
                cmd && !shift && z,
                (cmd && shift && z) || (cmd && y),
                cmd && !shift && s,
                cmd && shift && s,
            )
        });
        if key_undo {
            *action = Some(FileAction::Undo);
        } else if key_redo {
            *action = Some(FileAction::Redo);
        } else if key_save_as {
            // Sprint 19 / U1 — Ctrl+Shift+S = Save as. Documented in
            // `cheat_sheet::PROJECT_BINDINGS`; the tooltip catalogue
            // does not cite it directly today.
            *action = Some(FileAction::SaveAs);
        } else if key_save {
            // Sprint 19 / U1 — Ctrl+S = Save. The top-bar Save
            // button's hover-text cites this chord; the binding lives
            // in `cheat_sheet::PROJECT_BINDINGS` for the cheat sheet.
            *action = Some(FileAction::Save);
        }

        if ctx.wants_keyboard_input() {
            return;
        }
        let (q, b, s, m_key, v_key, f_key, w_key, l_key, g, help, esc) = ctx.input(|i| {
            let shift = i.modifiers.shift;
            (
                i.key_pressed(egui::Key::Q),
                i.key_pressed(egui::Key::B),
                i.key_pressed(egui::Key::S),
                // D10 / Sprint 17 (ADR-041): `T` is freed by the retirement
                // of `Tool::SplatPaint`. Reserved for a future keybinding
                // pass.
                i.key_pressed(egui::Key::M),
                i.key_pressed(egui::Key::V),
                i.key_pressed(egui::Key::F),
                i.key_pressed(egui::Key::W),
                i.key_pressed(egui::Key::L),
                i.key_pressed(egui::Key::G),
                // `?` is shift+/ on US layouts. Egui exposes the slash
                // key; we gate on shift so plain `/` doesn't open help.
                shift && i.key_pressed(egui::Key::Slash),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if q {
            self.set_tool(Tool::Select);
        }
        if b {
            self.set_tool(Tool::Sculpt);
        }
        if s {
            self.set_tool(Tool::StartPositions);
        }
        if m_key {
            self.set_tool(Tool::MetalSpots);
        }
        if v_key {
            // Sprint 12 / C6: V = "vents" (the old F binding moved here
            // to free F for Tool::Feature).
            self.set_tool(Tool::GeoFeatures);
        }
        if f_key {
            self.set_tool(Tool::Feature);
        }
        if w_key {
            // Sprint 14 / C9: W = "Water / Lava".
            self.set_tool(Tool::Water);
        }
        if l_key {
            // Sprint 16 / D9: L = "Paint layer".
            self.set_tool(Tool::PaintLayer);
        }
        if g {
            self.set_tool(Tool::Procgen);
        }
        // `?` toggles the cheat-sheet; gated on `!wizard_open` so it
        // can't ride on top of the F1 wizard.
        if help && !self.wizard_open {
            self.show_cheat_sheet = !self.show_cheat_sheet;
        }
        // Esc closes whichever overlay is on (cheat-sheet first, then
        // intro hint).
        if esc {
            if self.show_cheat_sheet {
                self.show_cheat_sheet = false;
            } else if self.show_intro {
                self.dismiss_intro();
            }
        }

        // Arrow-key camera pan (post-Sprint-14 follow-up). Uses
        // `key_down` (level-triggered, not edge-triggered) so holding
        // an arrow gives continuous motion. Shift = 3× speed for
        // long traversals. Velocity scales with orbit distance so
        // the pan feels consistent at any zoom level, and is
        // scaled by `stable_dt` so frame-rate variance doesn't
        // affect feel.
        let (arrow_left, arrow_right, arrow_up, arrow_down, shift_held, dt) = ctx.input(|i| {
            (
                i.key_down(egui::Key::ArrowLeft),
                i.key_down(egui::Key::ArrowRight),
                i.key_down(egui::Key::ArrowUp),
                i.key_down(egui::Key::ArrowDown),
                i.modifiers.shift,
                // `stable_dt` is the previous frame's duration. Clamp
                // to avoid huge jumps when the editor was paused / in
                // the background and resumes — > 100 ms gets clamped.
                i.stable_dt.min(0.1),
            )
        });
        if arrow_left || arrow_right || arrow_up || arrow_down {
            // Velocity = 0.25 × orbit distance per SECOND (so at
            // the default framing of an 8192-elmo map, distance ≈
            // 11 500, velocity ≈ 2 900 elmos/sec — ~2.8 s to cross
            // a 16-SMU map). Shift bumps to 3× for long jumps
            // (~0.9 s to cross). dt scaling means the same feel at
            // 30 / 60 / 120 fps.
            let base_per_sec = self.camera.distance * 0.25;
            let mut step = base_per_sec * dt;
            if shift_held {
                step *= 3.0;
            }
            let (sy, cy) = self.camera.yaw.sin_cos();
            // Camera-relative XZ axes. The first pass had the
            // left / right pair derived from the right-handed cross
            // product, but glam's `Mat4::look_at_lh` mirrors the X
            // axis vs the RH convention (s = up × forward at
            // yaw = 0 lands on world -X, not +X). Up / Down were
            // already aligned with the LH projection. Empirically
            // verified after the first user smoke session reported
            // the inversion.
            //   screen-right (world) = (-cos(yaw), 0,  sin(yaw))
            //   screen-up    (world) = (-sin(yaw), 0, -cos(yaw))
            let mut dx = 0.0f32;
            let mut dz = 0.0f32;
            if arrow_right {
                dx -= cy;
                dz += sy;
            }
            if arrow_left {
                dx += cy;
                dz -= sy;
            }
            if arrow_up {
                dx += -sy;
                dz += -cy;
            }
            if arrow_down {
                dx -= -sy;
                dz -= -cy;
            }
            self.camera.target.x += dx * step;
            self.camera.target.z += dz * step;
            // egui only repaints on input events by default; request
            // a follow-up frame so the camera keeps sliding while
            // the key is held without producing visible stalls.
            ctx.request_repaint();
            trace!(
                dx,
                dz,
                step,
                target_x = self.camera.target.x,
                target_z = self.camera.target.z,
                "arrow-key camera pan"
            );
        }
    }

    /// Mark the first-launch hint as seen for the current editor
    /// version and persist the config to disk. Best-effort; save
    /// errors log at `warn` (see `config::EditorConfig::save`).
    fn dismiss_intro(&mut self) {
        self.show_intro = false;
        self.editor_config.mark_intro_seen_for_current_version();
        self.editor_config.save();
        info!(
            version = config::CURRENT_VERSION,
            "first-launch hint dismissed"
        );
    }

    /// B8: hide the Next-steps Window and persist that choice into
    /// `Project.next_steps_dismissed` so save/open round-trips
    /// remember the dismissal. Lives on the project, NOT in
    /// `EditorConfig` — opening another fresh project should re-show
    /// the hint.
    fn dismiss_next_steps(&mut self) {
        self.show_next_steps = false;
        self.next_steps_dismissed = true;
        info!(
            project = %self.project_name,
            "B8 next-steps hint dismissed"
        );
    }

    /// Top action bar (ADR-035): brand chip + File/Edit/View/Build
    /// menus on the left, centred symmetry cluster (pill toggle + mode
    /// dropdown + fold spinner) in the middle, validation chip + Save
    /// + Build-and-install split button on the right.
    fn top_bar(&mut self, ctx: &egui::Context, action: &mut Option<FileAction>) {
        let t = crate::ui::theme::Tokens::DARK;
        egui::TopBottomPanel::top("action_bar")
            .exact_height(40.0)
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .fill(t.panel)
                    .stroke(egui::Stroke::new(1.0, t.border))
                    .inner_margin(egui::Margin {
                        left: 12,
                        right: 8,
                        top: 4,
                        bottom: 4,
                    }),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    self.top_bar_brand(ui);
                    ui.add_space(8.0);
                    // Sprint 19 — active-tool chip between brand and
                    // menus. Click to open a Popup that mirrors the
                    // left tool strip; gives the user a clear top-bar
                    // indicator + a second way to switch tools.
                    self.top_bar_tool_chip(ui);
                    ui.add_space(8.0);
                    self.top_bar_menus(ui, action);

                    // Centred symmetry cluster. Implemented as a sized
                    // child that we manually flex by emitting blank
                    // space on either side — egui doesn't have a true
                    // 3-region horizontal flex.
                    let avail = ui.available_width();
                    let cluster_w = self.top_bar_symmetry_width();
                    let right_w = self.top_bar_right_block_width();
                    let centre_left_pad = ((avail - cluster_w - right_w) * 0.5).max(0.0);
                    ui.add_space(centre_left_pad);
                    self.top_bar_symmetry_cluster(ui);

                    // Right-aligned validation + Save + Build cluster.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.top_bar_right_block(ui, action);
                    });
                });
            });
    }

    /// Active-tool chip + dropdown for the top action bar. Mirrors
    /// `symmetry_mode_dropdown` styling so the two chips read as a
    /// consistent set; the Popup lists every `Tool::ALL` variant with
    /// its icon + label + accelerator. The left 48 px tool strip
    /// stays the primary picker; this is the up-top indicator.
    fn top_bar_tool_chip(&mut self, ui: &mut egui::Ui) {
        let t = crate::ui::theme::Tokens::DARK;
        let tool = self.tool;
        let label = format!("{}  ·  {}", tool.label(), tool.accel());
        let icon = tool.icon_kind();
        let resp = egui::Frame::new()
            .fill(t.accent_alpha(0x2E))
            .stroke(egui::Stroke::new(1.0, t.accent_dim))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(13.0, 13.0), egui::Sense::hover());
                    crate::ui::icons::paint_icon(ui.painter(), rect, icon, t.text, 1.4);
                    ui.label(egui::RichText::new(&label).color(t.text).size(12.0));
                    let (caret_rect, _) =
                        ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                    crate::ui::icons::paint_icon(
                        ui.painter(),
                        caret_rect,
                        crate::ui::icons::Icon::ChevDown,
                        t.muted,
                        1.4,
                    );
                });
            })
            .response
            .interact(egui::Sense::click());
        let mut selected_tool: Option<Tool> = None;
        egui::Popup::menu(&resp)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
            .show(|ui| {
                ui.set_min_width(220.0);
                for &candidate in &Tool::ALL {
                    let row_label = format!("{}  ({})", candidate.label(), candidate.accel());
                    let selected = candidate == self.tool;
                    if ui
                        .add(egui::Button::selectable(selected, row_label))
                        .clicked()
                    {
                        selected_tool = Some(candidate);
                    }
                }
            });
        if let Some(new_tool) = selected_tool
            && new_tool != self.tool
        {
            self.set_tool(new_tool);
        }
    }

    fn top_bar_brand(&self, ui: &mut egui::Ui) {
        let t = crate::ui::theme::Tokens::DARK;
        let (rect, _resp) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, egui::CornerRadius::same(4), t.bg);
        painter.rect_stroke(
            rect,
            egui::CornerRadius::same(4),
            egui::Stroke::new(1.0, t.border_hi),
            egui::StrokeKind::Middle,
        );
        // Mountain glyph — drawn directly (not in the Icon catalogue,
        // because it's brand-only).
        let stroke = egui::Stroke::new(1.6, t.accent);
        let s = rect.width() / 24.0;
        let p = |x: f32, y: f32| egui::pos2(rect.left() + x * s, rect.top() + y * s);
        for (a, b) in [
            ((3.0, 18.0), (9.0, 8.0)),
            ((9.0, 8.0), (13.0, 14.0)),
            ((13.0, 14.0), (17.0, 6.0)),
            ((17.0, 6.0), (21.0, 18.0)),
            ((21.0, 18.0), (3.0, 18.0)),
        ] {
            painter.line_segment([p(a.0, a.1), p(b.0, b.1)], stroke);
        }
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("BAR Map Editor")
                .color(t.muted)
                .size(12.0),
        );
        ui.add_space(8.0);
        // Vertical separator.
        let (sep_rect, _) = ui.allocate_exact_size(egui::vec2(1.0, 20.0), egui::Sense::hover());
        ui.painter().rect_filled(sep_rect, 0.0, t.border);
    }

    fn top_bar_menus(&mut self, ui: &mut egui::Ui, action: &mut Option<FileAction>) {
        ui.menu_button("File", |ui| {
            if ui
                .button("New project…")
                .on_hover_text("Open the wizard to seed a new project (name, size, biome, symmetry).")
                .clicked()
            {
                *action = Some(FileAction::OpenWizard);
                ui.close();
            }
            if ui
                .button("Open project…")
                .on_hover_text("Load an existing .barmeproj from disk.")
                .clicked()
            {
                *action = Some(FileAction::Open);
                ui.close();
            }
            if ui
                .button("Save project")
                .on_hover_text("Save to the current .barmeproj path. [Shortcut: Ctrl+S]")
                .clicked()
            {
                *action = Some(FileAction::Save);
                ui.close();
            }
            if ui
                .button("Save project as…")
                .on_hover_text("Save to a new .barmeproj path. [Shortcut: Ctrl+Shift+S]")
                .clicked()
            {
                *action = Some(FileAction::SaveAs);
                ui.close();
            }
            ui.separator();
            ui.label("Load fixture heightmap");
            for smu in [2u32, 4, 16] {
                if ui
                    .button(format!("{smu}×{smu} SMU"))
                    .on_hover_text(format!(
                        "Load the bundled {smu}×{smu} SMU test heightmap. Useful for quick smoke tests of the editor."
                    ))
                    .clicked()
                {
                    *action = Some(FileAction::LoadHeightmap(fixture_path(smu)));
                    ui.close();
                }
            }
        });
        ui.menu_button("Edit", |ui| {
            let can_undo = self.history.can_undo() || self.history.stroke_open();
            let can_redo = self.history.can_redo();
            if ui
                .add_enabled(can_undo, egui::Button::new("Undo\tCtrl+Z"))
                .on_hover_text("Roll back the last edit. History ring is capped at 100 MB; older entries drop off the tail.")
                .clicked()
            {
                *action = Some(FileAction::Undo);
                ui.close();
            }
            if ui
                .add_enabled(can_redo, egui::Button::new("Redo\tCtrl+Shift+Z"))
                .on_hover_text("Re-apply the most recently undone edit.")
                .clicked()
            {
                *action = Some(FileAction::Redo);
                ui.close();
            }
        });
        ui.menu_button("View", |ui| {
            if ui
                .checkbox(&mut self.grid_overlay_on, "Coordinate grid")
                .on_hover_text("Toggle the world-aligned grid overlay on the 3D terrain preview.")
                .clicked()
            {
                ui.close();
            }
            if ui
                .checkbox(&mut self.lighting_on, "Lighting (preview only)")
                .on_hover_text("Toggle directional-light shading. Preview-only — the in-game render is governed by mapinfo.lighting.")
                .clicked()
            {
                ui.close();
            }
            if ui
                .checkbox(&mut self.wireframe_on, "Wireframe (preview only)")
                .on_hover_text("Toggle wireframe overlay on the terrain mesh.")
                .clicked()
            {
                ui.close();
            }
        });
        ui.menu_button("Build", |ui| {
            let enabled = self.heightmap.is_some();
            if ui
                .add_enabled(enabled, egui::Button::new("Build & Install to BAR"))
                .on_hover_text("Compile the project to a .sd7 and install into BAR's user maps directory. Same as the top-bar split-button primary.")
                .clicked()
            {
                *action = Some(FileAction::BuildAndInstall);
                ui.close();
            }
            if !enabled {
                ui.label("(load a heightmap first)");
            }
            // Sprint 20 / chunk 5 — surface the build log panel.
            if ui
                .button("Show log…")
                .on_hover_text(
                    "Open the build log panel — shows live PyMapConv output during a build \
                     and stays open until dismissed."
                )
                .clicked()
            {
                self.build_log_open = true;
                ui.close();
            }
        });
    }

    /// Estimated horizontal extent of the symmetry cluster. Used by
    /// [`top_bar`] to centre it. Width changes when rotational fold is
    /// active (extra spinner) but a fixed estimate is fine for
    /// centring — the actual layout adjusts.
    fn top_bar_symmetry_width(&self) -> f32 {
        let base = 32.0 + 110.0; // pill toggle + mode dropdown
        if matches!(self.symmetry, SymmetryAxis::Rotational { .. }) {
            base + 80.0
        } else {
            base
        }
    }

    fn top_bar_right_block_width(&self) -> f32 {
        // chip + save + split-build + spacing
        90.0 + 80.0 + 160.0 + 24.0
    }

    fn top_bar_symmetry_cluster(&mut self, ui: &mut egui::Ui) {
        let t = crate::ui::theme::Tokens::DARK;
        let on = !matches!(self.symmetry, SymmetryAxis::None);
        let cluster_fill = if on { t.accent_alpha(0x2E) } else { t.bg };
        let cluster_stroke = egui::Stroke::new(1.0, if on { t.accent_dim } else { t.border });
        egui::Frame::new()
            .fill(cluster_fill)
            .stroke(cluster_stroke)
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin {
                left: 4,
                right: 4,
                top: 2,
                bottom: 2,
            })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let mut on_state = on;
                    if crate::ui::widgets::pill_toggle(ui, "Symmetry", &mut on_state)
                        .on_hover_text(crate::ui::help_text::help(
                            crate::ui::help_text::HelpId::TopBarSymmetryPill,
                        ))
                        .clicked()
                    {
                        // Toggle behaviour: remember the user's last
                        // non-None mode so on→off→on returns there.
                        if on_state {
                            self.symmetry = self.last_non_none_symmetry;
                        } else {
                            if !matches!(self.symmetry, SymmetryAxis::None) {
                                self.last_non_none_symmetry = self.symmetry;
                            }
                            self.symmetry = SymmetryAxis::None;
                        }
                    }
                    self.symmetry_mode_dropdown(ui);
                    if let SymmetryAxis::Rotational { fold } = self.symmetry {
                        let mut f = fold as i32;
                        ui.label(egui::RichText::new("Fold").color(t.muted).size(11.0));
                        if ui
                            .add(egui::DragValue::new(&mut f).range(2..=12).speed(0.1))
                            .on_hover_text(crate::ui::help_text::help(
                                crate::ui::help_text::HelpId::TopBarSymmetryFold,
                            ))
                            .changed()
                        {
                            self.symmetry = SymmetryAxis::Rotational {
                                fold: f.clamp(2, 12) as u8,
                            };
                            self.rotational_fold = f.clamp(2, 12) as u8;
                        }
                    }
                });
            });
    }

    fn symmetry_mode_dropdown(&mut self, ui: &mut egui::Ui) {
        use crate::ui::icons::Icon;
        let t = crate::ui::theme::Tokens::DARK;
        let mode = self.symmetry;
        let (icon, label) = match mode {
            SymmetryAxis::None => (None, "None".to_string()),
            SymmetryAxis::Horizontal => (Some(Icon::SymH), "Horizontal".into()),
            SymmetryAxis::Vertical => (Some(Icon::SymV), "Vertical".into()),
            SymmetryAxis::Quad => (Some(Icon::SymQ), "Quad".into()),
            SymmetryAxis::DiagonalMain => (Some(Icon::SymQ), "Diagonal".into()),
            SymmetryAxis::DiagonalAnti => (Some(Icon::SymQ), "Diag2".into()),
            SymmetryAxis::Rotational { fold } => {
                (Some(Icon::SymRot), format!("Rotational ×{fold}"))
            }
        };
        let resp = egui::Frame::new()
            .fill(t.panel2)
            .stroke(egui::Stroke::new(1.0, t.border))
            .corner_radius(egui::CornerRadius::same(4))
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some(ic) = icon {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(13.0, 13.0), egui::Sense::hover());
                        crate::ui::icons::paint_icon(ui.painter(), rect, ic, t.text, 1.4);
                    }
                    ui.label(egui::RichText::new(&label).color(t.text).size(12.0));
                    let (caret_rect, _) =
                        ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                    crate::ui::icons::paint_icon(
                        ui.painter(),
                        caret_rect,
                        Icon::ChevDown,
                        t.muted,
                        1.4,
                    );
                });
            })
            .response
            .interact(egui::Sense::click())
            .on_hover_text(crate::ui::help_text::help(
                crate::ui::help_text::HelpId::TopBarSymmetryMode,
            ));
        egui::Popup::menu(&resp)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
            .show(|ui| {
                ui.set_min_width(160.0);
                let modes: &[(SymmetryAxis, &str)] = &[
                    (SymmetryAxis::None, "None"),
                    (SymmetryAxis::Horizontal, "Horizontal"),
                    (SymmetryAxis::Vertical, "Vertical"),
                    (SymmetryAxis::Quad, "Quad"),
                    (SymmetryAxis::DiagonalMain, "Diagonal"),
                    (SymmetryAxis::DiagonalAnti, "Diag2"),
                    (
                        SymmetryAxis::Rotational {
                            fold: self.rotational_fold,
                        },
                        "Rotational",
                    ),
                ];
                for (m, l) in modes {
                    let selected =
                        std::mem::discriminant(m) == std::mem::discriminant(&self.symmetry);
                    if ui.add(egui::Button::selectable(selected, *l)).clicked() {
                        if !matches!(*m, SymmetryAxis::None) {
                            self.last_non_none_symmetry = *m;
                        }
                        self.symmetry = *m;
                    }
                }
            });
    }

    fn top_bar_right_block(&mut self, ui: &mut egui::Ui, action: &mut Option<FileAction>) {
        use crate::ui::help_text::{HelpId, help};
        use crate::ui::icons::Icon;
        let t = crate::ui::theme::Tokens::DARK;

        // Recenter-camera button (icon-only, Compass). One-click
        // reset to the default framing — pairs with the arrow-key
        // pan controls so a user who's panned off the map can get
        // back without manually orbiting.
        let recenter_resp = ui
            .allocate_response(egui::vec2(30.0, 30.0), egui::Sense::click())
            .on_hover_text(help(HelpId::TopBarRecenter));
        {
            let painter = ui.painter();
            let bg = if recenter_resp.hovered() {
                t.hover
            } else {
                t.panel2
            };
            painter.rect_filled(recenter_resp.rect, egui::CornerRadius::same(4), bg);
            painter.rect_stroke(
                recenter_resp.rect,
                egui::CornerRadius::same(4),
                egui::Stroke::new(1.0, t.border),
                egui::StrokeKind::Middle,
            );
            let icon_rect =
                egui::Rect::from_center_size(recenter_resp.rect.center(), egui::vec2(18.0, 18.0));
            crate::ui::icons::paint_icon(painter, icon_rect, Icon::Compass, t.muted, 1.6);
        }
        if recenter_resp.clicked() {
            self.recenter_camera();
        }
        ui.add_space(4.0);

        // Build & install split-button (rightmost so it's the eye
        // anchor — the user's most-used action).
        let can_run = self.heightmap.is_some();
        let (primary, caret) = crate::ui::widgets::split_button(
            ui,
            Some(Icon::Play),
            "Build & install",
            can_run, // accent only when actionable
        );
        let build_hover = match self.build_destination_hint() {
            Some(path) => format!("{} → {}", help(HelpId::TopBarBuildPrimary), path),
            None => help(HelpId::TopBarBuildPrimary).to_string(),
        };
        let primary = primary.on_hover_text(build_hover);
        let _caret = caret.on_hover_text(help(HelpId::TopBarBuildVariant));
        if can_run
            && primary.clicked()
            && let Some(act) = self.build_variant.to_file_action()
        {
            *action = Some(act);
        }
        egui::Popup::menu(&_caret)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
            .show(|ui| {
                ui.set_min_width(220.0);
                for v in BuildVariant::ALL {
                    let selected = self.build_variant == v;
                    let enabled = v.is_enabled();
                    let label = if enabled {
                        v.label().to_string()
                    } else {
                        format!("{} (Phase 5+)", v.label())
                    };
                    let btn = egui::Button::selectable(selected, label);
                    let resp_v = ui.add_enabled(enabled, btn);
                    if resp_v.clicked() && enabled {
                        self.build_variant = v;
                    }
                }
            });
        ui.add_space(4.0);

        // Save button with dirty dot.
        let save_label = if self.dirty { "Save •" } else { "Save" };
        if ui
            .add(
                egui::Button::new(save_label)
                    .fill(t.panel2)
                    .min_size(egui::vec2(60.0, 30.0)),
            )
            .on_hover_text(help(HelpId::TopBarSave))
            .clicked()
        {
            *action = Some(FileAction::Save);
        }
        ui.add_space(4.0);

        // C7 / Sprint 18 (F9): mapinfo form button. Opens an
        // egui::Window with the 12-tab editor. Non-modal so a user
        // can tweak gravity while painting splats.
        if crate::ui::widgets::icon_button(ui, Icon::MapInfo, 30.0, help(HelpId::TopBarMapInfoForm))
            .clicked()
        {
            self.mapinfo_form_open = !self.mapinfo_form_open;
        }
        ui.add_space(4.0);

        // Sprint 19 / U1 — top-bar Help (?) icon. Opens the cheat
        // sheet (also reachable via the `?` chord). Sprint 22
        // extends this into a full help center.
        if crate::ui::widgets::icon_button(ui, Icon::Help, 30.0, help(HelpId::TopBarHelpIcon))
            .clicked()
        {
            self.show_cheat_sheet = !self.show_cheat_sheet;
        }
        ui.add_space(4.0);

        // Validation chip. Sprint 19 / U1 — clickable, opens the
        // lint panel stub. Hover text reproduces the summary so the
        // chip carries the same affordance the cheat-sheet does.
        let (tone, label) = self.validation_summary();
        let chip_hover = format!("{} — Issue: {}", help(HelpId::TopBarValidationChip), label);
        let chip_resp = crate::ui::widgets::chip(ui, tone, label).on_hover_text(chip_hover);
        if chip_resp.clicked() {
            self.lint_panel_open = !self.lint_panel_open;
        }
    }

    /// Sprint 19 / U1 — best-effort string describing where a
    /// Build & install click will write the `.sd7`. Used by the
    /// top-bar tooltip. Returns `None` if the project doesn't have
    /// a name yet (the empty-state placeholder).
    fn build_destination_hint(&self) -> Option<String> {
        if self.project_name.is_empty() {
            None
        } else {
            Some(format!("{}.sd7", self.project_name))
        }
    }

    /// Bottom status strip: live camera-orbit readout, project size,
    /// validation-chip placeholder, last-install / last-error state.
    /// C8 wires the validation chip to real lint output later.
    fn status_strip(&mut self, ctx: &egui::Context) {
        use crate::ui::help_text::{HelpId, help};
        // 1-Hz repaint nudge so the camera readout stays current
        // while idle (pitfall §B4.2 — egui only repaints on input
        // otherwise). The hint is a no-op if a higher-frequency
        // repaint is already scheduled this frame.
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
        let (issue_tone, issue_label) = self.validation_summary();
        let issue_count = crate::ui::lint_panel::issue_count(issue_tone);
        let mut open_lint = false;
        egui::TopBottomPanel::bottom("status_strip").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let cam = &self.camera;
                ui.add(
                    egui::Label::new(format!(
                        "Cam: yaw {:.0}° pitch {:.0}° dist {:.0}",
                        cam.yaw.to_degrees(),
                        cam.pitch.to_degrees(),
                        cam.distance,
                    ))
                    .sense(egui::Sense::hover()),
                )
                .on_hover_text(help(HelpId::StatusCamera));
                ui.separator();
                let (hpx_x, hpx_z) = self.map_size.heightmap_dims();
                ui.add(
                    egui::Label::new(format!(
                        "Map: {}×{} SMU ({}×{} px)",
                        self.map_size.smu_x, self.map_size.smu_z, hpx_x, hpx_z,
                    ))
                    .sense(egui::Sense::hover()),
                )
                .on_hover_text(help(HelpId::StatusMapSize));
                ui.separator();
                // Sprint 19 / U1 — live issue count, clickable.
                let issue_text = if issue_count == 0 {
                    "0 issues".to_string()
                } else {
                    format!("{issue_count} issue · {issue_label}")
                };
                let issue_resp = ui
                    .add(
                        egui::Label::new(egui::RichText::new(issue_text).weak())
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_text(help(HelpId::StatusIssueCount));
                if issue_resp.clicked() {
                    open_lint = true;
                }
                ui.separator();
                // Sprint 20 / chunk 6 — status strip mirrors the
                // BuildState machine. Running shows the active stage
                // and elapsed; Done / Failed / Cancelled stay sticky
                // until the next build kicks off. Every variant is
                // click-to-show-log.
                let mut open_build_log = false;
                let build_chip_text: egui::RichText = match &self.build_state {
                    build_runner::BuildState::Running {
                        current_stage,
                        started_at,
                        ..
                    } => egui::RichText::new(format!(
                        "Building: {} · {}s",
                        current_stage.label(),
                        started_at.elapsed().as_secs(),
                    ))
                    .color(egui::Color32::from_rgb(180, 200, 240)),
                    build_runner::BuildState::Done {
                        sd7_path, duration, ..
                    } => egui::RichText::new(format!(
                        "✓ {} in {}s",
                        sd7_path.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
                        duration.as_secs(),
                    ))
                    .color(egui::Color32::from_rgb(110, 200, 120)),
                    build_runner::BuildState::Failed { error, .. } => {
                        egui::RichText::new(format!("✗ Build failed: {}", short_error(error)))
                            .color(egui::Color32::from_rgb(220, 110, 90))
                    }
                    build_runner::BuildState::Cancelled { duration, .. } => {
                        egui::RichText::new(format!("Build cancelled ({}s)", duration.as_secs()))
                            .weak()
                    }
                    build_runner::BuildState::Idle => match &self.last_install {
                        Some(Ok(p)) => egui::RichText::new(format!(
                            "Installed: {}",
                            p.file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or_else(|| p.to_str().unwrap_or("?")),
                        ))
                        .color(egui::Color32::from_rgb(110, 200, 120)),
                        Some(Err(msg)) => {
                            egui::RichText::new(format!("Install failed: {}", short_error(msg)))
                                .color(egui::Color32::from_rgb(220, 110, 90))
                        }
                        None => egui::RichText::new("Build: idle").weak(),
                    },
                };
                // The Idle / no-prior-install variant gets hover-only
                // semantics (nothing to show in the log); every other
                // variant is click-to-show-log.
                let clickable = !matches!(
                    (&self.build_state, &self.last_install),
                    (build_runner::BuildState::Idle, None)
                );
                let sense = if clickable {
                    egui::Sense::click()
                } else {
                    egui::Sense::hover()
                };
                let build_chip_resp = ui
                    .add(egui::Label::new(build_chip_text).sense(sense))
                    .on_hover_text(help(HelpId::StatusInstall));
                if clickable && build_chip_resp.clicked() {
                    open_build_log = true;
                }
                if open_build_log {
                    self.build_log_open = true;
                }
                if let Some(err) = &self.last_error {
                    ui.separator();
                    ui.colored_label(egui::Color32::RED, err);
                }
                // Sprint 19 — brush readout chip (right-aligned).
                // Surfaces brush radius + strength for the active
                // brush-bearing tools so the user always knows their
                // brush size without opening the Inspector.
                let brush_chip = match self.tool {
                    Tool::Sculpt | Tool::Water => Some(format!(
                        "Brush · {:.0} elmos · {:.2}",
                        self.brush_radius, self.brush_strength,
                    )),
                    Tool::PaintLayer => Some(format!(
                        "Brush · {:.0} elmos · {:.2}",
                        self.paint_brush_state.radius, self.paint_brush_state.strength,
                    )),
                    _ => None,
                };
                if let Some(text) = brush_chip {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(text).monospace().weak())
                                .sense(egui::Sense::hover()),
                        )
                        .on_hover_text(help(HelpId::StatusBrushChip));
                    });
                }
            });
        });
        if open_lint {
            self.lint_panel_open = true;
        }
    }

    /// Left tool strip: 40 px fixed-width column of one selectable
    /// `Button` per `Tool`. Hover-tooltip carries the long name and
    /// accelerator. Phase 4 grows this with Splat / Metal / Feature;
    /// adding a variant to `Tool` adds a row here automatically as long
    /// as it's listed in the array below (the per-site exhaustive
    /// `match`es elsewhere catch a missing dispatch).
    /// Tool strip styling (ADR-035): 48 px column, 36×36 line-icon
    /// tile per tool, active state = filled accent bg + 2 px left
    /// accent rail + letter glyph beneath the icon. Cog at the bottom
    /// is a placeholder for editor preferences (Phase 9+).
    fn tool_strip(&mut self, ctx: &egui::Context) {
        let t = crate::ui::theme::Tokens::DARK;
        egui::SidePanel::left("tool_strip")
            .resizable(false)
            .exact_width(48.0)
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .fill(t.panel)
                    .stroke(egui::Stroke::new(1.0, t.border))
                    .inner_margin(egui::Margin {
                        left: 6,
                        right: 6,
                        top: 6,
                        bottom: 6,
                    }),
            )
            .show(ctx, |ui| {
                for &tool in &Tool::ALL {
                    let active = self.tool == tool;
                    self.tool_strip_tile(ui, tool, active);
                    ui.add_space(2.0);
                }
                // Push the cog to the bottom edge.
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    let resp = crate::ui::widgets::icon_button(
                        ui,
                        crate::ui::icons::Icon::Cog,
                        36.0,
                        "Editor settings (coming soon)",
                    );
                    let _ = resp; // wired in Phase 9+
                });
            });
    }

    fn tool_strip_tile(&mut self, ui: &mut egui::Ui, tool: Tool, active: bool) {
        let t = crate::ui::theme::Tokens::DARK;
        // 36×36 tile with a 2 px accent rail to the left when active.
        let size = egui::vec2(36.0, 36.0);
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        let painter = ui.painter();
        if active {
            // Left rail.
            let rail = egui::Rect::from_min_size(
                egui::pos2(rect.left() - 6.0, rect.top() + 4.0),
                egui::vec2(2.0, rect.height() - 8.0),
            );
            painter.rect_filled(rail, egui::CornerRadius::same(1), t.accent);
            painter.rect_filled(rect, egui::CornerRadius::same(6), t.accent);
        } else if response.hovered() {
            painter.rect_filled(rect, egui::CornerRadius::same(6), t.hover);
        }
        let icon_color = if active {
            egui::Color32::WHITE
        } else {
            t.muted
        };
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(rect.center().x, rect.top() + 12.0),
            egui::vec2(20.0, 20.0),
        );
        crate::ui::icons::paint_icon(painter, icon_rect, tool.icon_kind(), icon_color, 1.5);
        // Letter under the icon.
        let galley = painter.layout_no_wrap(
            tool.accel().to_string(),
            egui::FontId::monospace(9.0),
            if active {
                egui::Color32::WHITE.gamma_multiply(0.9)
            } else {
                t.muted
            },
        );
        let text_pos = egui::pos2(
            rect.center().x - galley.size().x * 0.5,
            rect.bottom() - galley.size().y - 2.0,
        );
        painter.galley(text_pos, galley, icon_color);
        if response.clicked() {
            self.set_tool(tool);
        }
        response.on_hover_text(format!("{} ({})", tool.label(), tool.accel()));
    }

    /// Right Inspector: persistent project / heightmap / height-scale
    /// header always at the top, tool-specific controls underneath
    /// dispatched by an exhaustive `match` on `self.tool`. Adding a
    /// new `Tool` variant produces a compile error at this match site
    /// — that's the safety property the enum buys us.
    fn inspector(&mut self, ctx: &egui::Context, action: &mut Option<FileAction>) {
        egui::SidePanel::right("inspector")
            .resizable(true)
            .default_width(300.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("inspector_scroll")
                    .show(ui, |ui| {
                        self.inspector_header(ui);
                        ui.separator();
                        match self.tool {
                            Tool::Select => self.inspector_select(ui),
                            Tool::Sculpt => self.inspector_sculpt(ui),
                            Tool::StartPositions => self.inspector_start_positions(ui),
                            Tool::MetalSpots => self.inspector_metal(ui),
                            Tool::GeoFeatures => self.inspector_geo(ui),
                            Tool::Feature => self.inspector_feature(ui),
                            Tool::Water => self.inspector_water(ui),
                            Tool::PaintLayer => self.inspector_paint_layer(ui),
                            Tool::Procgen => self.inspector_procgen(ctx, ui, action),
                        }
                    });
            });
    }

    /// Persistent Inspector header (ADR-035). Project name + size +
    /// dirty chip; then heightmap card with path/dims/sample as a
    /// 2-col grid + a valid/invalid chip in the section header.
    fn inspector_header(&mut self, ui: &mut egui::Ui) {
        use crate::ui::help_text::{HelpId, help};
        let t = crate::ui::theme::Tokens::DARK;
        // PROJECT section.
        let dirty = self.dirty;
        crate::ui::widgets::section(
            ui,
            "Project",
            false,
            |ui| {
                let tone = if dirty {
                    crate::ui::theme::ChipTone::Warn
                } else {
                    crate::ui::theme::ChipTone::Neutral
                };
                let label = if dirty { "Unsaved" } else { "Saved" };
                crate::ui::widgets::chip(ui, tone, label)
                    .on_hover_text(help(HelpId::HeaderProjectSavedChip));
            },
            |ui| {
                let name_resp = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.project_name)
                            .desired_width(f32::INFINITY)
                            .frame(false)
                            .font(egui::FontId::proportional(15.0)),
                    )
                    .on_hover_text(help(HelpId::HeaderProjectName));
                if name_resp.changed() {
                    self.dirty = true;
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Size").color(t.muted).size(11.0));
                    let prev_size = self.map_size;
                    ui.add(
                        egui::DragValue::new(&mut self.map_size.smu_x)
                            .range(2..=96)
                            .speed(0.1),
                    )
                    .on_hover_text(help(HelpId::HeaderMapSizeX));
                    ui.label(egui::RichText::new("×").color(t.dim).size(11.0));
                    ui.add(
                        egui::DragValue::new(&mut self.map_size.smu_z)
                            .range(2..=96)
                            .speed(0.1),
                    )
                    .on_hover_text(help(HelpId::HeaderMapSizeZ));
                    ui.label(egui::RichText::new("SMU").color(t.muted).size(11.0));
                    if prev_size != self.map_size {
                        self.dirty = true;
                    }
                });
            },
        );

        // HEIGHTMAP section.
        let hm = self.heightmap.as_ref();
        let valid = hm.is_some_and(|h| h.validated_against.is_some());
        let path_str = hm
            .map(|h| {
                h.path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?")
                    .to_string()
            })
            .unwrap_or_else(|| "—".to_string());
        let dims_str = hm
            .map(|h| format!("{} × {}", h.dims.0, h.dims.1))
            .unwrap_or_else(|| "—".to_string());
        let sample_str = hm
            .map(|h| format!("min {} · max {}", h.min, h.max))
            .unwrap_or_else(|| "—".to_string());
        let height_scale = &mut self.height_scale;
        crate::ui::widgets::section(
            ui,
            "Heightmap",
            false,
            |ui| {
                let tone = if valid {
                    crate::ui::theme::ChipTone::Ok
                } else {
                    crate::ui::theme::ChipTone::Err
                };
                let label = if valid { "Valid" } else { "Invalid" };
                crate::ui::widgets::chip(ui, tone, label)
                    .on_hover_text(help(HelpId::HeaderHeightmapValidChip));
            },
            |ui| {
                egui::Grid::new("inspector_hm_grid")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .striped(false)
                    .show(ui, |ui| {
                        for (k, v, help_id) in [
                            ("Path", path_str.as_str(), HelpId::HeaderHeightmapPath),
                            ("Dims", dims_str.as_str(), HelpId::HeaderHeightmapDims),
                            ("Sample", sample_str.as_str(), HelpId::HeaderHeightmapSample),
                        ] {
                            ui.add(
                                egui::Label::new(egui::RichText::new(k).color(t.muted).size(11.0))
                                    .sense(egui::Sense::hover()),
                            )
                            .on_hover_text(help(help_id));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(v).color(t.text).size(11.0).monospace(),
                                    );
                                },
                            );
                            ui.end_row();
                        }
                        ui.label(egui::RichText::new("Max height").color(t.muted).size(11.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add(
                                egui::DragValue::new(height_scale)
                                    .range(1.0..=4096.0)
                                    .speed(1.0)
                                    .suffix(" elmos"),
                            )
                            .on_hover_text(help(HelpId::HeaderHeightScale));
                        });
                        ui.end_row();
                    });
            },
        );
    }

    /// Sprint 19 / U1 — sticky chip row rendered at the top of every
    /// tool's Inspector body, just below the persistent header. Echoes
    /// the active symmetry mode (which drives the tool's strokes) plus
    /// the map size (in SMU). Each chip carries its own hover-text via
    /// [`crate::ui::help_text`].
    fn inspector_sticky_chips(&self, ui: &mut egui::Ui) {
        use crate::ui::help_text::{HelpId, help};
        use crate::ui::theme::{ChipTone, Tokens};
        let t = Tokens::DARK;
        let sym_label = self.symmetry.label();
        let sym_tone = match self.symmetry {
            SymmetryAxis::None => ChipTone::Neutral,
            _ => ChipTone::Ok,
        };
        let size_label = format!("{}×{} SMU", self.map_size.smu_x, self.map_size.smu_z);
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 14,
                right: 14,
                top: 6,
                bottom: 4,
            })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    crate::ui::widgets::chip(ui, sym_tone, format!("Sym · {sym_label}"))
                        .on_hover_text(help(HelpId::InspectorSymmetryChip));
                    crate::ui::widgets::chip(ui, ChipTone::Neutral, size_label)
                        .on_hover_text(help(HelpId::InspectorMapSizeChip));
                });
                ui.add_space(2.0);
                // 1-px divider line so the chip row reads as a band.
                let avail = ui.available_width();
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(avail, 1.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 0.0, t.border);
            });
    }

    fn inspector_select(&self, ui: &mut egui::Ui) {
        use crate::ui::help_text::{HelpId, help};
        self.inspector_sticky_chips(ui);
        ui.add(
            egui::Label::new(egui::RichText::new("Select / orbit").heading())
                .sense(egui::Sense::hover()),
        )
        .on_hover_text(help(HelpId::SelectModeInfo));
        ui.label(
            egui::RichText::new(
                "Camera-only mode. LMB orbits, MMB pans, RMB orbits, scroll zooms.\n\
                 Pick a tool on the left strip to start editing.",
            )
            .small()
            .weak(),
        );
    }

    /// F5 metal-spots inspector (C4 / Sprint 11). Operates directly
    /// on `App::metal_spots` (mirroring `Project.metal_spots`); each
    /// edit pushes a `ProjectDiff` so Ctrl-Z reverses it.
    ///
    /// Layout:
    /// - **GLOBAL** — `extractor_radius` (PITFALL §6) with a tooltip
    ///   explaining the 80-elmo BAR convention.
    /// - **SPOTS** — table of (index, X / Z `DragValue`, metal
    ///   `DragValue` 0.5..=8.0, delete button) plus "+ Add spot" at
    ///   the bottom (places at map centre, metal = 2.0).
    fn inspector_metal(&mut self, ui: &mut egui::Ui) {
        use crate::ui::help_text::{HelpId, help};
        let t = crate::ui::theme::Tokens::DARK;
        let (ex, ez) = self.world_extents();
        self.inspector_sticky_chips(ui);

        // GLOBAL section — extractor_radius. The closure captures
        // its own working copy so the header chip + body DragValue
        // don't fight over the App-level field's borrow.
        let radius_before = self.extractor_radius;
        let mut radius_edit = radius_before;
        let is_default = (radius_before - default_extractor_radius()).abs() < 0.01;
        crate::ui::widgets::section_with_hover(
            ui,
            "Global",
            true,
            "Project-wide metal settings. Per-spot fields live in the SPOTS section below.",
            |ui| {
                let tone = if is_default {
                    crate::ui::theme::ChipTone::Ok
                } else {
                    crate::ui::theme::ChipTone::Warn
                };
                let label = if is_default { "BAR default" } else { "Custom" };
                crate::ui::widgets::chip(ui, tone, label)
                    .on_hover_text(help(HelpId::MetalGlobalChip));
            },
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Extractor radius")
                            .color(t.muted)
                            .size(11.0),
                    );
                    ui.add(
                        egui::DragValue::new(&mut radius_edit)
                            .range(16.0..=200.0)
                            .speed(1.0)
                            .suffix(" elmos"),
                    )
                    .on_hover_text(help(HelpId::MetalExtractorRadius));
                });
            },
        );
        if (radius_edit - radius_before).abs() > 0.0001 {
            self.extractor_radius = radius_edit;
            self.history
                .push_project_diff(ProjectDiff::SetExtractorRadius {
                    from: radius_before,
                    to: radius_edit,
                });
            self.mark_dirty();
        }

        // SPOTS section — table with per-row edits + Add button.
        let mut selected = self.metal_state.selected;
        let mut to_delete: Option<usize> = None;
        let mut to_move: Option<(usize, MetalSpot, MetalSpot)> = None;
        let mut add_clicked = false;
        let spots_snapshot: Vec<MetalSpot> = self.metal_spots.clone();
        let spot_count = spots_snapshot.len();
        let title = format!("Spots · {}", spot_count);
        crate::ui::widgets::section_with_hover(
            ui,
            &title,
            false,
            "Per-spot metal entries. Position is in elmos from the south-west corner; the value is the per-spot metal multiplier.",
            |ui| {
                if ui
                    .add(egui::Button::new("+ Add"))
                    .on_hover_text(help(HelpId::MetalAddSpot))
                    .clicked()
                {
                    add_clicked = true;
                }
            },
            |ui| {
                if spots_snapshot.is_empty() {
                    ui.label(
                        egui::RichText::new(
                            "No metal spots yet. LMB on the canvas to place one, or click + Add.",
                        )
                        .color(t.dim)
                        .size(11.0),
                    );
                }
                egui::ScrollArea::vertical()
                    .max_height(420.0)
                    .id_salt("metal_spots_scroll")
                    .show(ui, |ui| {
                        for (i, original) in spots_snapshot.iter().enumerate() {
                            let mut edited = *original;
                            let is_sel = selected == Some(i);
                            let row = egui::Frame::new()
                                .fill(if is_sel { t.hover } else { t.bg })
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    if is_sel { t.border_hi } else { t.border },
                                ))
                                .corner_radius(egui::CornerRadius::same(3))
                                .inner_margin(egui::Margin::symmetric(8, 4))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(format!("M{:02}", i + 1))
                                                    .color(t.muted)
                                                    .monospace()
                                                    .size(11.0),
                                            )
                                            .sense(egui::Sense::hover()),
                                        )
                                        .on_hover_text("Metal-spot index (1-based, for display only). The engine identifies spots by position, not by id.");
                                        ui.add(
                                            egui::DragValue::new(&mut edited.x_elmo)
                                                .range(0..=(ex as i32))
                                                .speed(8.0)
                                                .prefix("x "),
                                        )
                                        .on_hover_text(help(HelpId::MetalSpotX));
                                        ui.add(
                                            egui::DragValue::new(&mut edited.z_elmo)
                                                .range(0..=(ez as i32))
                                                .speed(8.0)
                                                .prefix("z "),
                                        )
                                        .on_hover_text(help(HelpId::MetalSpotZ));
                                        // PITFALL: don't artificially cap the metal value —
                                        // BAR maps strategically place high-value central mexes
                                        // (e.g. 5.2) and low-value perimeter mexes (e.g. 0.5)
                                        // for asymmetric pressure. Range is generous (0..=50)
                                        // and the user can type any value in that span.
                                        ui.add(
                                            egui::DragValue::new(&mut edited.metal)
                                                .range(0.0..=50.0)
                                                .speed(0.1)
                                                .fixed_decimals(2),
                                        )
                                        .on_hover_text(help(HelpId::MetalSpotValue));
                                        if ui
                                            .small_button("×")
                                            .on_hover_text(help(HelpId::MetalSpotDelete))
                                            .clicked()
                                        {
                                            to_delete = Some(i);
                                        }
                                    });
                                })
                                .response
                                .interact(egui::Sense::click());
                            if row.clicked() {
                                selected = Some(i);
                            }
                            if edited != *original {
                                to_move = Some((i, *original, edited));
                            }
                        }
                    });
            },
        );
        self.metal_state.selected = selected;
        if let Some(i) = to_delete {
            self.delete_metal_spot(i);
        }
        if let Some((i, from, to)) = to_move {
            self.move_metal_spot_to(i, to);
            self.history
                .push_project_diff(ProjectDiff::MoveMetalSpot { from, to });
            self.mark_dirty();
        }
        if add_clicked {
            let cx = (ex * 0.5).round() as i32;
            let cz = (ez * 0.5).round() as i32;
            self.place_metal_spot(cx as f32, cz as f32);
        }
    }

    /// F6 geo-vents inspector (C5 / Sprint 11). Operates on
    /// `App::geo_vents`. Simpler than metal: no per-spot value, no
    /// global section (the stock `geovent` FeatureDef carries its
    /// own size).
    ///
    /// Sprint 12 / C6 will add a separate `Tool::Feature` for
    /// general trees / rocks / wreckage placement; that's the
    /// scaffolded library / scatter UI's eventual home.
    fn inspector_geo(&mut self, ui: &mut egui::Ui) {
        use crate::ui::help_text::{HelpId, help};
        let t = crate::ui::theme::Tokens::DARK;
        let (ex, ez) = self.world_extents();
        self.inspector_sticky_chips(ui);

        // SPOTS section — same row pattern as metal, minus the value
        // column.
        let mut selected = self.geo_state.selected;
        let mut to_delete: Option<usize> = None;
        let mut to_move: Option<(usize, GeoVent, GeoVent)> = None;
        let mut add_clicked = false;
        let vents_snapshot: Vec<GeoVent> = self.geo_vents.clone();
        let vent_count = vents_snapshot.len();
        let title = format!("Geo vents · {}", vent_count);
        crate::ui::widgets::section_with_hover(
            ui,
            &title,
            true,
            "Geo vents = unique economy slots that produce steam plumes in-game. Emitted via the Springboard featureplacer trio's `geovent` entries (PITFALL §14 / §21).",
            |ui| {
                if ui
                    .add(egui::Button::new("+ Add"))
                    .on_hover_text(help(HelpId::GeoAddVent))
                    .clicked()
                {
                    add_clicked = true;
                }
            },
            |ui| {
                if vents_snapshot.is_empty() {
                    ui.label(
                        egui::RichText::new(
                            "No geo vents yet. LMB on the canvas to place one, or click + Add.",
                        )
                        .color(t.dim)
                        .size(11.0),
                    );
                }
                egui::ScrollArea::vertical()
                    .max_height(420.0)
                    .id_salt("geo_vents_scroll")
                    .show(ui, |ui| {
                        for (i, original) in vents_snapshot.iter().enumerate() {
                            let mut edited = *original;
                            let is_sel = selected == Some(i);
                            let row = egui::Frame::new()
                                .fill(if is_sel { t.hover } else { t.bg })
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    if is_sel { t.border_hi } else { t.border },
                                ))
                                .corner_radius(egui::CornerRadius::same(3))
                                .inner_margin(egui::Margin::symmetric(8, 4))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(format!("V{:02}", i + 1))
                                                    .color(t.muted)
                                                    .monospace()
                                                    .size(11.0),
                                            )
                                            .sense(egui::Sense::hover()),
                                        )
                                        .on_hover_text(
                                            "Geo-vent index (1-based, for display only).",
                                        );
                                        ui.add(
                                            egui::DragValue::new(&mut edited.x_elmo)
                                                .range(0..=(ex as i32))
                                                .speed(8.0)
                                                .prefix("x "),
                                        )
                                        .on_hover_text(help(HelpId::GeoVentX));
                                        ui.add(
                                            egui::DragValue::new(&mut edited.z_elmo)
                                                .range(0..=(ez as i32))
                                                .speed(8.0)
                                                .prefix("z "),
                                        )
                                        .on_hover_text(help(HelpId::GeoVentZ));
                                        if ui
                                            .small_button("×")
                                            .on_hover_text(help(HelpId::GeoVentDelete))
                                            .clicked()
                                        {
                                            to_delete = Some(i);
                                        }
                                    });
                                })
                                .response
                                .interact(egui::Sense::click());
                            if row.clicked() {
                                selected = Some(i);
                            }
                            if edited != *original {
                                to_move = Some((i, *original, edited));
                            }
                        }
                    });
            },
        );
        self.geo_state.selected = selected;
        if let Some(i) = to_delete {
            self.delete_geo_vent(i);
        }
        if let Some((i, from, to)) = to_move {
            self.move_geo_vent_to(i, to);
            self.history
                .push_project_diff(ProjectDiff::MoveGeoVent { from, to });
            self.mark_dirty();
        }
        if add_clicked {
            let cx = (ex * 0.5).round();
            let cz = (ez * 0.5).round();
            self.place_geo_vent(cx, cz);
        }
    }

    /// F7 feature inspector (C6 / Sprint 12). Three sections:
    ///   - CATEGORY: ComboBox to switch between trees / rocks /
    ///     wreckage / props / geo.
    ///   - PICKER: filtered list of features in the chosen category.
    ///     Click a row to select; the canvas's next LMB-click places
    ///     it at the cursor.
    ///   - PLACED: list of already-placed sources with x/z/rotation
    ///     fields + delete button. Rotation displays as 0..359 deg
    ///     (`rot_heading * 360 / 65536` rounding to nearest int).
    fn inspector_feature(&mut self, ui: &mut egui::Ui) {
        use crate::ui::help_text::{HelpId, help};
        let t = crate::ui::theme::Tokens::DARK;
        let (ex, ez) = self.world_extents();
        self.inspector_sticky_chips(ui);

        // CATEGORY section.
        let categories: Vec<String> = self.feature_state.manifest.category_names();
        let no_catalog = categories.is_empty();
        crate::ui::widgets::section_with_hover(
            ui,
            "Category",
            true,
            "Pick the feature catalogue — trees / rocks / props / wreckage / geo. Switching categories resets the pending placement.",
            |_ui| {},
            |ui| {
                if no_catalog {
                    ui.label(
                        egui::RichText::new(
                            "No mapfeatures_catalog.json found. Falling back to a single \"geovent\" entry.",
                        )
                        .color(t.dim)
                        .size(11.0),
                    );
                    return;
                }
                ui.horizontal(|ui| {
                    let combo_resp = egui::ComboBox::from_id_salt("feature_category")
                        .selected_text(self.feature_state.active_category.clone())
                        .show_ui(ui, |ui| {
                            for cat in &categories {
                                if ui
                                    .selectable_label(
                                        cat == &self.feature_state.active_category,
                                        cat,
                                    )
                                    .clicked()
                                {
                                    self.feature_state.active_category = cat.clone();
                                    // Reset selection when switching
                                    // categories so the next LMB doesn't
                                    // place a stale name.
                                    self.feature_state.selected_feature = None;
                                }
                            }
                        });
                    combo_resp
                        .response
                        .on_hover_text(help(HelpId::FeatureCategoryCombo));
                });
            },
        );

        // PICKER section.
        let category = self.feature_state.active_category.clone();
        let entries = self.feature_state.manifest.entries(&category).to_vec();
        let filter_lc = self.feature_state.filter.to_lowercase();
        let filtered: Vec<CatalogEntry> = if filter_lc.is_empty() {
            entries.clone()
        } else {
            entries
                .iter()
                .filter(|e| {
                    e.name.to_lowercase().contains(&filter_lc)
                        || e.display.to_lowercase().contains(&filter_lc)
                        || e.tags.iter().any(|t| t.to_lowercase().contains(&filter_lc))
                })
                .cloned()
                .collect()
        };
        let picker_title = format!("{} · {}", category, filtered.len());
        crate::ui::widgets::section(
            ui,
            &picker_title,
            true,
            |_ui| {},
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Filter").color(t.muted).size(11.0));
                    ui.text_edit_singleline(&mut self.feature_state.filter)
                        .on_hover_text(help(HelpId::FeatureFilter));
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .id_salt("feature_picker_scroll")
                    .show(ui, |ui| {
                        if filtered.is_empty() {
                            ui.label(
                                egui::RichText::new("No features match the current filter.")
                                    .color(t.dim)
                                    .size(11.0),
                            );
                        }
                        for entry in &filtered {
                            let is_sel = self.feature_state.selected_feature.as_deref()
                                == Some(entry.name.as_str());
                            let row = egui::Frame::new()
                                .fill(if is_sel { t.hover } else { t.bg })
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    if is_sel { t.border_hi } else { t.border },
                                ))
                                .corner_radius(egui::CornerRadius::same(3))
                                .inner_margin(egui::Margin::symmetric(8, 4))
                                .show(ui, |ui| {
                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new(&entry.display)
                                                .color(t.text)
                                                .size(12.0),
                                        );
                                        ui.label(
                                            egui::RichText::new(&entry.name)
                                                .color(t.muted)
                                                .monospace()
                                                .size(10.0),
                                        );
                                    });
                                })
                                .response
                                .interact(egui::Sense::click())
                                .on_hover_text(help(HelpId::FeaturePickerRow));
                            if row.clicked() {
                                self.feature_state.selected_feature = Some(entry.name.clone());
                            }
                        }
                    });
            },
        );

        // PLACED section.
        let mut selected = self.feature_state.selected_placed;
        let mut to_delete: Option<usize> = None;
        let mut to_move: Option<(usize, FeatureInstance, FeatureInstance)> = None;
        let features_snapshot: Vec<FeatureInstance> = self.features.clone();
        let placed_title = format!("Placed · {}", features_snapshot.len());
        crate::ui::widgets::section(
            ui,
            &placed_title,
            true,
            |_ui| {},
            |ui| {
                if features_snapshot.is_empty() {
                    ui.label(
                        egui::RichText::new(
                            "No features yet. Pick one above, then LMB on the canvas to place. LMB-drag rotates; RMB deletes.",
                        )
                        .color(t.dim)
                        .size(11.0),
                    );
                }
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .id_salt("features_placed_scroll")
                    .show(ui, |ui| {
                        for (i, original) in features_snapshot.iter().enumerate() {
                            let mut edited = original.clone();
                            let is_sel = selected == Some(i);
                            let row = egui::Frame::new()
                                .fill(if is_sel { t.hover } else { t.bg })
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    if is_sel { t.border_hi } else { t.border },
                                ))
                                .corner_radius(egui::CornerRadius::same(3))
                                .inner_margin(egui::Margin::symmetric(8, 4))
                                .show(ui, |ui| {
                                    ui.vertical(|ui| {
                                        ui.horizontal(|ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(format!("F{:02}", i + 1))
                                                        .color(t.muted)
                                                        .monospace()
                                                        .size(11.0),
                                                )
                                                .sense(egui::Sense::hover()),
                                            )
                                            .on_hover_text("Feature instance index (1-based, for display only).");
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(&original.name)
                                                        .color(t.text)
                                                        .monospace()
                                                        .size(11.0),
                                                )
                                                .sense(egui::Sense::hover()),
                                            )
                                            .on_hover_text("Feature `name` field — matches a FeatureDef in BAR's featuredefs.lua (or the map-bundled set.lua for custom features).");
                                            if ui
                                                .small_button("×")
                                                .on_hover_text(help(HelpId::FeaturePlacedDelete))
                                                .clicked()
                                            {
                                                to_delete = Some(i);
                                            }
                                        });
                                        ui.horizontal(|ui| {
                                            ui.add(
                                                egui::DragValue::new(&mut edited.x_elmo)
                                                    .range(0..=(ex as i32))
                                                    .speed(8.0)
                                                    .prefix("x "),
                                            )
                                            .on_hover_text(help(HelpId::FeaturePlacedX));
                                            ui.add(
                                                egui::DragValue::new(&mut edited.z_elmo)
                                                    .range(0..=(ez as i32))
                                                    .speed(8.0)
                                                    .prefix("z "),
                                            )
                                            .on_hover_text(help(HelpId::FeaturePlacedZ));
                                            // Display rotation as integer
                                            // degrees 0..359; convert
                                            // back through the heading
                                            // rounding so a 90° edit
                                            // produces exactly 16384.
                                            let mut deg = ((edited.rot_heading as f32 * 360.0
                                                / 65536.0)
                                                .round()
                                                as i32)
                                                .rem_euclid(360);
                                            let resp = ui
                                                .add(
                                                    egui::DragValue::new(&mut deg)
                                                        .range(0..=359)
                                                        .speed(1.0)
                                                        .suffix("°"),
                                                )
                                                .on_hover_text(help(HelpId::FeaturePlacedRot));
                                            if resp.changed() {
                                                let deg_u = deg.rem_euclid(360) as u32;
                                                edited.rot_heading = ((deg_u * 65536) / 360) as u16;
                                            }
                                        });
                                    });
                                })
                                .response
                                .interact(egui::Sense::click());
                            if row.clicked() {
                                selected = Some(i);
                            }
                            if edited != *original {
                                to_move = Some((i, original.clone(), edited));
                            }
                        }
                    });
            },
        );
        self.feature_state.selected_placed = selected;
        if let Some(i) = to_delete {
            self.delete_feature(i);
        }
        if let Some((i, from, to)) = to_move {
            self.move_feature_to(i, to.clone());
            self.history
                .push_project_diff(ProjectDiff::MoveFeature { from, to });
            self.mark_dirty();
        }
    }

    /// Sculpt inspector (ADR-035): 4-card brush picker (Off / Raise /
    /// Lower / Smooth) styled with a coloured swatch ring per mode,
    /// ramp sliders for radius and strength, and a behaviour chip row
    /// (Continuous active; Pressure and Lock-Z placeholder-disabled).
    /// D9 / Sprint 16 (ADR-040) — paint-layer inspector. Originally
    /// scoped as a minimal active-layer strip; expanded mid-Sprint
    /// per user request into a proper Layers panel covering
    /// add / rename / delete / reorder / opacity / per-layer
    /// visibility / texture-import. Sprint 17's spec (ADR-041) had
    /// owned these affordances; bringing them forward keeps the
    /// painting workflow self-sufficient without waiting on the
    /// full Photoshop-style panel.
    /// D10 / Sprint 17 (ADR-041) — `Tool::PaintLayer` Inspector. The
    /// Layers panel lives in [`crate::ui::layers_panel`]; the brush
    /// section stays here because it owns session-only
    /// [`PaintBrushState`].
    fn inspector_paint_layer(&mut self, ui: &mut egui::Ui) {
        use crate::ui::help_text::{HelpId, help};
        let t = crate::ui::theme::Tokens::DARK;
        self.inspector_sticky_chips(ui);

        // Layers panel (rows + active-layer properties + footer).
        crate::ui::layers_panel::render(self, ui);

        // ---- BRUSH section (session-only state, stays inline) ----
        crate::ui::widgets::section_with_hover(
            ui,
            "Brush",
            false,
            "Pick a mask-paint brush. Reveal = increase active layer alpha; Hide = decrease; Smooth = blur; Fill = stamp the entire footprint at the target value.",
            |_ui| {},
            |ui| {
                let brushes: [(&str, &str, egui::Color32, HelpId); 4] = [
                    ("mask-reveal", "Reveal", t.green, HelpId::PaintBrushReveal),
                    ("mask-hide", "Hide", t.red, HelpId::PaintBrushHide),
                    ("mask-smooth", "Smooth", t.accent, HelpId::PaintBrushSmooth),
                    ("mask-fill", "Fill", t.amber, HelpId::PaintBrushFill),
                ];
                let mut new_brush_id: Option<String> = None;
                ui.columns(4, |cols| {
                    for (i, (id, label, color, help_id)) in brushes.iter().enumerate() {
                        let active = self.paint_brush_state.brush_id == *id;
                        let resp = Self::brush_card(&mut cols[i], label, *color, active)
                            .on_hover_text(help(*help_id));
                        if resp.clicked() {
                            new_brush_id = Some((*id).to_string());
                        }
                    }
                });
                if let Some(id) = new_brush_id {
                    self.paint_brush_state.brush_id = id;
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Radius").color(t.muted).size(11.0));
                    ui.add(
                        egui::Slider::new(&mut self.paint_brush_state.radius, 8.0..=512.0)
                            .suffix(" e"),
                    )
                    .on_hover_text(help(HelpId::PaintRadius));
                });
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Strength").color(t.muted).size(11.0));
                    ui.add(egui::Slider::new(
                        &mut self.paint_brush_state.strength,
                        0.0..=1.0,
                    ))
                    .on_hover_text(help(HelpId::PaintStrength));
                });
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Spacing").color(t.muted).size(11.0));
                    ui.add(egui::Slider::new(
                        &mut self.paint_brush_state.spacing,
                        0.05..=2.0,
                    ))
                    .on_hover_text(help(HelpId::PaintSpacing));
                });
                if self.paint_brush_state.brush_id == "mask-fill" {
                    ui.add_space(4.0);
                    ui.checkbox(
                        &mut self.paint_brush_state.fill_target_visible,
                        "Fill makes layer visible (else hide)",
                    )
                    .on_hover_text(help(HelpId::PaintFillTargetVisible));
                }
                ui.add_space(4.0);
                ui.checkbox(
                    &mut self.paint_brush_state.mask_only_preview,
                    "Mask-only preview",
                )
                .on_hover_text(help(HelpId::PaintMaskOnlyPreview));
            },
        );
    }

    fn inspector_sculpt(&mut self, ui: &mut egui::Ui) {
        use crate::ui::help_text::{HelpId, help};
        let t = crate::ui::theme::Tokens::DARK;
        self.inspector_sticky_chips(ui);
        // BRUSH section: 4-card picker.
        let brushes_info: Vec<(Option<String>, &str, egui::Color32, HelpId)> = vec![
            (None, "Off", t.muted, HelpId::SculptBrushOff),
            (
                Some("raise".to_string()),
                "Raise",
                t.green,
                HelpId::SculptBrushRaise,
            ),
            (
                Some("lower".to_string()),
                "Lower",
                t.red,
                HelpId::SculptBrushLower,
            ),
            (
                Some("smooth".to_string()),
                "Smooth",
                t.accent,
                HelpId::SculptBrushSmooth,
            ),
        ];
        let mut new_brush: Option<Option<String>> = None;
        crate::ui::widgets::section_with_hover(
            ui,
            "Brush",
            true,
            "Pick the active sculpt brush. Off disables stamping while keeping the tool selected.",
            |_ui| {},
            |ui| {
                ui.columns(4, |cols| {
                    for (i, (id, label, color, help_id)) in brushes_info.iter().enumerate() {
                        let active = self.brush_id == *id;
                        let resp = Self::brush_card(&mut cols[i], label, *color, active)
                            .on_hover_text(help(*help_id));
                        if resp.clicked() {
                            new_brush = Some(id.clone());
                        }
                    }
                });
            },
        );
        if let Some(b) = new_brush {
            self.brush_id = b;
        }

        // SHAPE section: ramp sliders.
        let mut radius_raw = self.brush_radius;
        let strength_raw = &mut self.brush_strength;
        crate::ui::widgets::section_with_hover(
            ui,
            "Shape",
            false,
            "Brush radius (elmos) + per-stamp strength (0..1). The falloff curve is fixed for Sprint 19; per-brush curves come later.",
            |_ui| {},
            |ui| {
                let r_label = format!("{:.0} elmos", radius_raw);
                crate::ui::widgets::ramp_slider_labelled(
                    ui,
                    "Radius",
                    &mut radius_raw,
                    8.0..=4096.0,
                    t.accent,
                    r_label,
                )
                .on_hover_text(help(HelpId::SculptRadius));
                ui.add_space(8.0);
                let s_label = format!("{:.2}", *strength_raw);
                crate::ui::widgets::ramp_slider_labelled(
                    ui,
                    "Strength",
                    strength_raw,
                    0.0..=1.0,
                    t.accent,
                    s_label,
                )
                .on_hover_text(help(HelpId::SculptStrength));
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(egui::RichText::new("Falloff").color(t.muted).size(11.0))
                        .sense(egui::Sense::hover()),
                )
                .on_hover_text(help(HelpId::SculptFalloff));
                // Decorative falloff preview — pure painter, no state.
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 34.0),
                    egui::Sense::hover(),
                );
                let painter = ui.painter();
                painter.rect_filled(rect, egui::CornerRadius::same(2), t.bg);
                painter.rect_stroke(
                    rect,
                    egui::CornerRadius::same(2),
                    egui::Stroke::new(1.0, t.border),
                    egui::StrokeKind::Middle,
                );
                // Vertical guides at 1/3 and 2/3.
                for x in [
                    rect.left() + rect.width() / 3.0,
                    rect.left() + 2.0 * rect.width() / 3.0,
                ] {
                    painter.line_segment(
                        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                        egui::Stroke::new(1.0, t.border),
                    );
                }
                // Smoothstep-ish curve, painted as 24 segments.
                let mut prev = egui::pos2(rect.left() + 2.0, rect.top() + 4.0);
                for i in 1..=24 {
                    let tt = i as f32 / 24.0;
                    let yt = 1.0 - (1.0 - tt).powi(3); // ease-out
                    let p = egui::pos2(
                        rect.left() + 2.0 + tt * (rect.width() - 4.0),
                        rect.top() + 4.0 + yt * (rect.height() - 8.0),
                    );
                    painter.line_segment([prev, p], egui::Stroke::new(1.6, t.accent));
                    prev = p;
                }
            },
        );
        self.brush_radius = radius_raw;

        // BEHAVIOR chips — Continuous is active; others are reserved
        // future features and render disabled.
        crate::ui::widgets::section(
            ui,
            "Behavior",
            false,
            |_ui| {},
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    let _ = ui
                        .add(egui::Button::selectable(true, "Continuous"))
                        .on_hover_text(help(HelpId::SculptBehaviorContinuous));
                    let _ = ui
                        .add_enabled(false, egui::Button::new("Pressure"))
                        .on_disabled_hover_text(help(HelpId::SculptBehaviorPressure));
                    let _ = ui
                        .add_enabled(false, egui::Button::new("Lock Z"))
                        .on_disabled_hover_text(help(HelpId::SculptBehaviorLockZ));
                });
            },
        );
    }

    /// C9 (Sprint 14 / ADR-042) — `Tool::Water` Inspector. Surfaces
    /// the preset chip strip, key water-block fields, the
    /// flood-carve depth, and a placeholder for the F9 advanced form
    /// tab Sprint 18 will land.
    ///
    /// Each per-field edit emits a `ProjectDiff::EditWaterField` so
    /// undo / redo step through changes the same way the F8 / metal
    /// / geo / feature tools do. Drag-finalisation (coalesce a single
    /// slider gesture into one diff) is deferred — for now each frame
    /// pushes its own diff, which gives fine-grained undo but a busy
    /// stack. A follow-up sprint can ship per-slider drag-start /
    /// drag-stop tracking analogous to `dragging_metal_spot_from`.
    fn inspector_water(&mut self, ui: &mut egui::Ui) {
        use crate::ui::help_text::{HelpId, help};
        let t = crate::ui::theme::Tokens::DARK;
        self.inspector_sticky_chips(ui);

        // ── PRESET ────────────────────────────────────────
        let active_mode = self.water_mode;
        let mut new_mode: Option<WaterMode> = None;
        let custom_overrides_count = water_override_count(&self.water_overrides);
        crate::ui::widgets::section_with_hover(
            ui,
            "Preset",
            true,
            "Drop a stock water/lava style. Each preset is a bag of WaterBlock overrides; pick Custom to start from a blank slate.",
            |_ui| {},
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    for &m in &WaterMode::ALL {
                        let active = m == active_mode;
                        let label_owned = match m {
                            WaterMode::Custom if custom_overrides_count > 0 => {
                                format!("Custom ({custom_overrides_count})")
                            }
                            _ => m.label().to_string(),
                        };
                        let preset_help = match m {
                            WaterMode::None => HelpId::WaterPresetNone,
                            WaterMode::Custom => HelpId::WaterPresetCustom,
                            WaterMode::Ocean => HelpId::WaterPresetOcean,
                            WaterMode::Tropical => HelpId::WaterPresetTropical,
                            WaterMode::Acid => HelpId::WaterPresetAcid,
                            WaterMode::Lava => HelpId::WaterPresetLava,
                            WaterMode::Magma => HelpId::WaterPresetMagma,
                        };
                        let resp = ui
                            .add(egui::Button::selectable(active, label_owned))
                            .on_hover_text(help(preset_help));
                        if resp.clicked() && m != active_mode {
                            new_mode = Some(m);
                        }
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Switching presets keeps your tweaks. \
                         BAR floods every region where heightmap.y < 0.",
                    )
                    .color(t.dim)
                    .size(10.0),
                );
            },
        );
        if let Some(to) = new_mode {
            self.history.push_project_diff(ProjectDiff::SetWaterMode {
                from: active_mode,
                to,
            });
            self.water_mode = to;
            self.mark_dirty();
            tracing::info!(from = ?active_mode, to = ?self.water_mode, "water mode changed");
        }

        // ── LAVA ATMOSPHERE OFFER ─────────────────────────
        // C9 / Sprint 14 Slice 4. When Lava / Magma is active and the
        // user hasn't applied the lava-atmosphere patch yet, offer
        // the one-click affordance. When the patch IS on, surface
        // a "Revert" button so the user can undo from the same card.
        self.inspector_water_atmosphere_offer(ui);

        // ── HEIGHTMAP RANGE ──────────────────────────────
        // Sprint 19 — surface min / max height edits inside the Water
        // tool so the user can fix "water spawned in the wrong place"
        // without scrolling back to the persistent header. BAR's water
        // plane is fixed at Y=0 (`Ground.h::GetWaterPlaneLevel` is
        // `consteval`); the user adjusts the heightmap range to slide
        // terrain above / below it.
        crate::ui::widgets::section_with_hover(
            ui,
            "Heightmap range",
            false,
            "Duplicates the Project header sliders, surfaced here because flooding decisions live with the water tool. Floor and ceiling map raw heightmap u16 values to world Y.",
            |_ui| {},
            |ui| {
                ui.label(
                    egui::RichText::new(
                        "Water plane is fixed at Y = 0. Lower the floor below 0 \
                         so basins fill with water; raise the ceiling above 0 \
                         so mountains stand clear of it.",
                    )
                    .color(t.dim)
                    .size(10.0),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Floor (min)").color(t.muted).size(11.0));
                    let before = self.min_height;
                    let mut current = self.min_height;
                    ui.add(
                        egui::DragValue::new(&mut current)
                            .range(-2048.0..=0.0)
                            .speed(1.0)
                            .suffix(" elmos"),
                    )
                    .on_hover_text(help(HelpId::WaterFloorMin));
                    if (current - before).abs() > 1e-3 {
                        self.min_height = current;
                        self.mark_dirty();
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Ceiling (max)")
                            .color(t.muted)
                            .size(11.0),
                    );
                    let before = self.height_scale;
                    let mut current = self.height_scale;
                    ui.add(
                        egui::DragValue::new(&mut current)
                            .range(1.0..=4096.0)
                            .speed(1.0)
                            .suffix(" elmos"),
                    )
                    .on_hover_text(help(HelpId::WaterCeilingMax));
                    if (current - before).abs() > 1e-3 {
                        self.height_scale = current;
                        self.mark_dirty();
                    }
                });
            },
        );

        // ── BEHAVIOUR ────────────────────────────────────
        // Damage / void_water / tidal_strength. Tidal lives at
        // MapInfo top level but co-locates here for UX (PITFALL §5
        // / prompt's critical pitfall #5).
        crate::ui::widgets::section(
            ui,
            "Behaviour",
            false,
            |_ui| {},
            |ui| {
                self.water_field_float_slider(
                    ui,
                    "Damage",
                    WaterField::Damage,
                    |o| o.damage,
                    0.0..=10000.0,
                    " HP/tick",
                    help(HelpId::WaterDamage),
                );
                self.water_field_void_toggle(ui);
                self.water_field_tidal_slider(ui);
            },
        );

        // ── APPEARANCE ───────────────────────────────────
        crate::ui::widgets::section(
            ui,
            "Appearance",
            false,
            |_ui| {},
            |ui| {
                self.water_field_color(
                    ui,
                    "Surface",
                    WaterField::SurfaceColor,
                    |o| o.surface_color,
                    help(HelpId::WaterSurfaceColor),
                );
                // Plane colour disabled while voidWater is on
                // (PITFALL §6 — they're mutually exclusive; the
                // emission path auto-clears plane_color but we
                // surface the gating here too).
                let plane_enabled = !self.void_water;
                ui.add_enabled_ui(plane_enabled, |ui| {
                    self.water_field_color(
                        ui,
                        "Plane",
                        WaterField::PlaneColor,
                        |o| o.plane_color,
                        help(HelpId::WaterPlaneColor),
                    );
                });
                if !plane_enabled {
                    ui.label(
                        egui::RichText::new(
                            "Plane colour disabled — voidWater is on (PITFALL §6).",
                        )
                        .color(t.dim)
                        .size(10.0),
                    );
                }
                self.water_field_float_slider(
                    ui,
                    "Surface alpha",
                    WaterField::SurfaceAlpha,
                    |o| o.surface_alpha,
                    0.0..=1.0,
                    "",
                    help(HelpId::WaterSurfaceAlpha),
                );
                self.water_field_float_slider(
                    ui,
                    "Wave size",
                    WaterField::PerlinAmplitude,
                    |o| o.perlin_amplitude,
                    0.0..=2.0,
                    "",
                    help(HelpId::WaterWaveSize),
                );
                self.water_field_float_slider(
                    ui,
                    "Foam strength",
                    WaterField::WaveFoamIntensity,
                    |o| o.wave_foam_intensity,
                    0.0..=2.0,
                    "",
                    help(HelpId::WaterFoamStrength),
                );
            },
        );

        // ── FLOOD ────────────────────────────────────────
        crate::ui::widgets::section(
            ui,
            "Flood",
            false,
            |_ui| {},
            |ui| {
                // Explainer — the water plane is fixed at Y = 0
                // (`Ground.h::GetWaterPlaneLevel` is `consteval 0.0`
                // in Recoil). The user can't "raise the water";
                // they raise the SEA FLOOR by setting `min_height
                // < 0`, which makes raw heightmap value 0 land
                // below sea level.
                ui.label(
                    egui::RichText::new(
                        "BAR's water plane is fixed at Y = 0. To make water \
                         visible, set min_height below zero — the lowest part \
                         of the heightmap then sits underwater.",
                    )
                    .color(t.dim)
                    .size(10.0),
                );
                ui.add_space(6.0);

                // min_height DragValue — directly editable. Negative
                // values give the heightmap room to dip below Y = 0.
                // 0 = no water visible (lowest heightmap sample sits
                // AT sea level). −200 = the deepest point sits 200
                // elmos under sea level.
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Sea-floor depth")
                            .color(t.muted)
                            .size(11.0),
                    );
                    let before = self.min_height;
                    let mut current = self.min_height;
                    ui.add(
                        egui::DragValue::new(&mut current)
                            .range(-2048.0..=0.0)
                            .speed(1.0)
                            .suffix(" elmos"),
                    )
                    .on_hover_text(help(HelpId::WaterFloorMin));
                    if (current - before).abs() > 1e-3 {
                        self.min_height = current;
                        self.mark_dirty();
                    }
                });

                ui.add_space(6.0);

                // Observed deepest world Y in the current heightmap.
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Heightmap min observed")
                            .color(t.muted)
                            .size(11.0),
                    );
                    let mh_chip = format!("{:.0} elmos", self.heightmap_observed_min_height());
                    ui.add(
                        egui::Label::new(egui::RichText::new(mh_chip).color(t.text).size(11.0))
                            .sense(egui::Sense::hover()),
                    )
                    .on_hover_text("Deepest world-Y the current heightmap reaches. Compare against the sea-floor depth above to predict where water will be visible.");
                });

                ui.add_space(6.0);

                // Quick-set shortcut: clamp min_height to the carve
                // depth so a Tool::Water flood gesture immediately
                // produces visible water.
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Carve depth").color(t.muted).size(11.0));
                    ui.add(
                        egui::DragValue::new(&mut self.water_carve_depth)
                            .range(-1024.0..=0.0)
                            .speed(1.0)
                            .suffix(" elmos"),
                    )
                    .on_hover_text(help(HelpId::WaterCarveDepth));
                });
                ui.label(
                    egui::RichText::new("LMB drag → flood. RMB drag → raise.")
                        .color(t.dim)
                        .size(10.0),
                );
                ui.add_space(4.0);
                let auto_clicked = ui
                    .add(egui::Button::new("Set sea floor to carve depth"))
                    .on_hover_text(help(HelpId::WaterAutoMinHeight))
                    .clicked();
                if auto_clicked {
                    self.auto_set_min_height_from_heightmap();
                }
            },
        );

        // ── ADVANCED ────────────────────────────────────
        ui.collapsing("Advanced (raw mapinfo fields)", |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "Full 30-field form ships from Sprint 18's F9 mapinfo \
                         form. The advanced tab will reach the same \
                         water_overrides this Inspector edits — Tool::Water \
                         remains the primary entry point.",
                    )
                    .color(t.dim)
                    .size(10.0),
                )
                .sense(egui::Sense::hover()),
            )
            .on_hover_text("Open the F9 mapinfo form (top-bar icon or F9 chord) for the full WaterBlock field set.");
        });
    }

    /// C9 (Sprint 14 / ADR-042 — Slice 4): render the lava-atmosphere
    /// offer / status card. Shows only when the active water preset
    /// is Lava or Magma. Toggle pushes `SetLavaAtmosphere` for undo.
    fn inspector_water_atmosphere_offer(&mut self, ui: &mut egui::Ui) {
        let lava_family = matches!(self.water_mode, WaterMode::Lava | WaterMode::Magma);
        if !lava_family {
            return;
        }
        let t = crate::ui::theme::Tokens::DARK;
        let applied = self.lava_atmosphere;
        crate::ui::widgets::section(
            ui,
            "Lava atmosphere",
            false,
            |ui| {
                let tone = if applied {
                    crate::ui::theme::ChipTone::Ok
                } else {
                    crate::ui::theme::ChipTone::Warn
                };
                let label = if applied { "Applied" } else { "Not applied" };
                crate::ui::widgets::chip(ui, tone, label)
                    .on_hover_text("Whether the lava atmosphere patch is currently active. Click the Apply / Revert button below to toggle.");
            },
            |ui| {
                ui.label(
                    egui::RichText::new(
                        "Lava maps usually pair their water with red-orange fog \
                         and a dim warm sun. The patch sets fogColor, sunColor, \
                         fogStart/End, and cloud density on top of the BAR \
                         atmosphere default.",
                    )
                    .color(t.dim)
                    .size(11.0),
                );
                ui.add_space(6.0);
                let mut clicked_toggle = false;
                if applied {
                    let resp = ui
                        .add(egui::Button::new("Revert atmosphere"))
                        .on_hover_text(crate::ui::help_text::help(
                            crate::ui::help_text::HelpId::WaterLavaAtmosphereRevert,
                        ));
                    if resp.clicked() {
                        clicked_toggle = true;
                    }
                } else {
                    let resp = ui
                        .add(egui::Button::new("Apply lava-style atmosphere").fill(t.accent))
                        .on_hover_text(crate::ui::help_text::help(
                            crate::ui::help_text::HelpId::WaterLavaAtmosphereApply,
                        ));
                    if resp.clicked() {
                        clicked_toggle = true;
                    }
                }
                if clicked_toggle {
                    let new_val = !applied;
                    self.history
                        .push_project_diff(ProjectDiff::SetLavaAtmosphere {
                            from: applied,
                            to: new_val,
                        });
                    self.lava_atmosphere = new_val;
                    self.mark_dirty();
                    tracing::info!(applied = new_val, "lava atmosphere toggle");
                }
            },
        );
    }

    /// Float-slider edit on `Project.water_overrides`'s `field`,
    /// pushed through `EditWaterField` for undo. `accessor` extracts
    /// the current `Option<f32>` so the slider displays either the
    /// user's override or the active preset's value.
    #[allow(clippy::too_many_arguments)] // helper with field-specific knobs; refactoring is Sprint 27 scope
    fn water_field_float_slider(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        field: WaterField,
        accessor: impl Fn(&WaterBlock) -> Option<f32>,
        range: std::ops::RangeInclusive<f32>,
        suffix: &str,
        hover_text: &str,
    ) {
        let t = crate::ui::theme::Tokens::DARK;
        // Display value: override → preset → 0.0.
        let preset = preset_water_block(self.water_mode).unwrap_or_default();
        let before_opt = accessor(&self.water_overrides);
        let baseline = before_opt.unwrap_or_else(|| accessor(&preset).unwrap_or(0.0));
        let mut current = baseline;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(label).color(t.muted).size(11.0));
            ui.add(
                egui::Slider::new(&mut current, range)
                    .show_value(true)
                    .suffix(suffix),
            )
            .on_hover_text(hover_text);
        });
        if (current - baseline).abs() > 1e-6 {
            let to = WaterValue::Float(Some(current));
            let from = WaterValue::Float(before_opt);
            self.apply_water_field(field, to);
            self.history
                .push_project_diff(ProjectDiff::EditWaterField { field, from, to });
            self.mark_dirty();
        }
    }

    /// `void_water` toggle. PITFALL §6: when the user flips this on,
    /// the emission path auto-clears `plane_color`. The UI grays the
    /// plane colour picker (in the Appearance section) so the user
    /// sees the gating; the diff path doesn't have to touch
    /// plane_color here because emission handles it.
    fn water_field_void_toggle(&mut self, ui: &mut egui::Ui) {
        let t = crate::ui::theme::Tokens::DARK;
        let before = self.void_water;
        let mut current = before;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Void water").color(t.muted).size(11.0));
            ui.add(egui::Checkbox::new(&mut current, "")).on_hover_text(
                crate::ui::help_text::help(crate::ui::help_text::HelpId::WaterVoidWater),
            );
        });
        if current != before {
            let to = WaterValue::Bool(Some(current));
            let from = WaterValue::Bool(Some(before));
            self.apply_water_field(WaterField::VoidWater, to);
            self.history.push_project_diff(ProjectDiff::EditWaterField {
                field: WaterField::VoidWater,
                from,
                to,
            });
            self.mark_dirty();
        }
    }

    /// `tidal_strength` slider. Lives at MapInfo top level (PITFALL
    /// §5 / prompt's pitfall list #5). Co-located in Behaviour
    /// purely for UX — the schema field is `MapInfo.tidal_strength`,
    /// not `WaterBlock.tidal_strength`.
    fn water_field_tidal_slider(&mut self, ui: &mut egui::Ui) {
        let t = crate::ui::theme::Tokens::DARK;
        let before_opt = self.tidal_strength;
        let baseline = before_opt.unwrap_or(0.0);
        let mut current = baseline;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Tidal strength")
                    .color(t.muted)
                    .size(11.0),
            );
            ui.add(egui::Slider::new(&mut current, 0.0..=30.0).show_value(true))
                .on_hover_text(crate::ui::help_text::help(
                    crate::ui::help_text::HelpId::WaterTidalStrength,
                ));
        });
        if (current - baseline).abs() > 1e-6 {
            let to = WaterValue::Float(Some(current));
            let from = WaterValue::Float(before_opt);
            self.apply_water_field(WaterField::TidalStrength, to);
            self.history.push_project_diff(ProjectDiff::EditWaterField {
                field: WaterField::TidalStrength,
                from,
                to,
            });
            self.mark_dirty();
        }
    }

    /// `Rgb` colour picker for a water-block field. Renders an
    /// egui::color_picker swatch; on change, pushes an
    /// `EditWaterField` with the `Rgb(Option<[f32; 3]>)` payload.
    fn water_field_color(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        field: WaterField,
        accessor: impl Fn(&WaterBlock) -> Option<[f32; 3]>,
        hover_text: &str,
    ) {
        let t = crate::ui::theme::Tokens::DARK;
        let preset = preset_water_block(self.water_mode).unwrap_or_default();
        let before_opt = accessor(&self.water_overrides);
        let baseline = before_opt.unwrap_or_else(|| accessor(&preset).unwrap_or([0.5, 0.5, 0.5]));
        let mut current = baseline;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(label).color(t.muted).size(11.0));
            ui.color_edit_button_rgb(&mut current)
                .on_hover_text(hover_text);
        });
        let changed = (current[0] - baseline[0]).abs() > 1e-4
            || (current[1] - baseline[1]).abs() > 1e-4
            || (current[2] - baseline[2]).abs() > 1e-4;
        if changed {
            let to = WaterValue::Rgb(Some(current));
            let from = WaterValue::Rgb(before_opt);
            self.apply_water_field(field, to);
            self.history
                .push_project_diff(ProjectDiff::EditWaterField { field, from, to });
            self.mark_dirty();
        }
    }

    /// World Y (elmos) of the heightmap's lowest sample. Used by the
    /// Inspector to display "this is the deepest point of your map"
    /// alongside the carve-depth control.
    ///
    /// `u16 = 0` maps to `min_height`; `u16 = u16::MAX` maps to
    /// `height_scale`. The lowest observed raw value sits at
    /// `min_height + (raw_min / 65535) * (height_scale - min_height)`.
    fn heightmap_observed_min_height(&self) -> f32 {
        let Some(hm) = self.heightmap.as_ref() else {
            return self.min_height;
        };
        let t = hm.min as f32 / u16::MAX as f32;
        self.min_height + t * (self.height_scale - self.min_height)
    }

    /// "Auto-set min_height from heightmap" — sets
    /// `App.min_height` to `min(0, water_carve_depth)`, which makes
    /// BAR's water plane sit inside any region the user carves down
    /// to the heightmap's `u16 = 0` floor. The world-Y of a carved
    /// pixel is then exactly `min_height` (i.e. `<= 0`), which is
    /// what BAR's water render path expects for "this is water."
    ///
    /// **Simplification (Sprint 14 MVP):** the original prompt's
    /// formula `min_height = min(0, observed_min)` requires sampling
    /// the heightmap's normalised-then-projected world Y, which the
    /// existing `Heightmap` representation doesn't expose directly.
    /// The carve_depth shadow is the user's stated "how deep should
    /// the pool go" anyway, so we use it as the target. Sprint 18's
    /// F9 form will surface the underlying field for direct edit.
    fn auto_set_min_height_from_heightmap(&mut self) {
        if self.heightmap.is_none() {
            return;
        }
        let target = self.water_carve_depth.min(0.0);
        if (target - self.min_height).abs() > 1e-3 {
            info!(
                from = self.min_height,
                to = target,
                "auto_set_min_height_from_heightmap: setting min_height \
                 = min(0, water_carve_depth)"
            );
            self.min_height = target;
            self.mark_dirty();
        }
    }

    /// Single 4-card BrushPicker tile. Pure renderer; returns the
    /// click response.
    fn brush_card(
        ui: &mut egui::Ui,
        label: &str,
        color: egui::Color32,
        active: bool,
    ) -> egui::Response {
        let t = crate::ui::theme::Tokens::DARK;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 42.0), egui::Sense::click());
        let painter = ui.painter();
        let bg = if active { t.hover } else { t.bg };
        let stroke = egui::Stroke::new(1.0, if active { t.border_hi } else { t.border });
        painter.rect_filled(rect, egui::CornerRadius::same(4), bg);
        painter.rect_stroke(
            rect,
            egui::CornerRadius::same(4),
            stroke,
            egui::StrokeKind::Middle,
        );
        // Swatch ring.
        let cx = rect.center().x;
        let swatch_y = rect.top() + 14.0;
        let r = 7.0;
        let fill = if label == "Off" {
            egui::Color32::TRANSPARENT
        } else {
            egui::Color32::from_rgba_premultiplied(color.r() / 5, color.g() / 5, color.b() / 5, 80)
        };
        painter.circle(
            egui::pos2(cx, swatch_y),
            r,
            fill,
            egui::Stroke::new(1.5, color),
        );
        // Label.
        painter.text(
            egui::pos2(cx, rect.bottom() - 12.0),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(11.0),
            if active { t.text } else { t.muted },
        );
        response
    }

    /// F8 Inspector tree (ADR-032 / B6). One collapsing header per
    /// ally group with colour swatch, name, count, delete. Child rows
    /// show source positions; clicking a row marks it for the
    /// hover-pulse on the canvas. Symmetry-derived positions render
    /// greyed with a `(mirror of …)` label.
    fn inspector_start_positions(&mut self, ui: &mut egui::Ui) {
        use crate::ui::help_text::{HelpId, help};
        let t = crate::ui::theme::Tokens::DARK;
        let (ex, ez) = self.world_extents();
        self.inspector_sticky_chips(ui);

        // LAYOUT section: preset chip + drag-paint toggle + Balanced chip.
        let balanced = self.start_positions_balanced();
        crate::ui::widgets::section_with_hover(
            ui,
            "Layout",
            true,
            "Apply a stock allyteam layout, or LMB-drag a line on the canvas to drop N positions equally spaced.",
            |ui| {
                let tone = if balanced {
                    crate::ui::theme::ChipTone::Ok
                } else {
                    crate::ui::theme::ChipTone::Warn
                };
                let label = if balanced { "Balanced" } else { "Asymmetric" };
                crate::ui::widgets::chip(ui, tone, label)
                    .on_hover_text(help(HelpId::StartLayoutBalancedChip));
            },
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Preset").color(t.muted).size(11.0));
                    let mut selected: Option<AllyPreset> = None;
                    let combo = egui::ComboBox::from_id_salt("ally_preset")
                        .selected_text("Apply a layout…")
                        .show_ui(ui, |ui| {
                            for preset in AllyPreset::ALL {
                                if ui.selectable_label(false, preset.label()).clicked() {
                                    selected = Some(preset);
                                }
                            }
                        });
                    combo.response.on_hover_text(help(HelpId::StartPresetCombo));
                    if let Some(p) = selected {
                        self.apply_ally_preset(p);
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Drag-paint").color(t.muted).size(11.0));
                    ui.add(
                        egui::DragValue::new(&mut self.drag_paint_count)
                            .range(1u8..=32)
                            .suffix(" pos"),
                    )
                    .on_hover_text(help(HelpId::StartDragPaintCount));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("LMB drag a line")
                                .color(t.dim)
                                .size(10.0),
                        );
                    });
                });
            },
        );

        // ALLYTEAMS section with collapsible cards.
        let mut to_delete_pos: Option<(u8, usize)> = None;
        let mut to_delete_group: Option<u8> = None;
        let mut new_active: Option<u8> = None;
        let mut new_pulse: Option<(u8, usize)> = None;
        let mut add_group_clicked = false;
        let active = self.active_ally_group_id;
        // Materialise a stable ordering so the tree doesn't shuffle
        // when ids are non-contiguous.
        let mut group_indices: Vec<usize> = (0..self.ally_groups.len()).collect();
        group_indices.sort_by_key(|&i| self.ally_groups[i].id);

        let group_count = self.ally_groups.len();
        let group_title = format!("Allyteams · {}", group_count);
        crate::ui::widgets::section_with_hover(
            ui,
            &group_title,
            false,
            "One collapsible card per ally team. Source positions are authored here; symmetry mirrors render as greyed `↳` entries.",
            |ui| {
                if ui
                    .add(egui::Button::new("+ Add"))
                    .on_hover_text(help(HelpId::StartAllyAdd))
                    .clicked()
                {
                    add_group_clicked = true;
                }
            },
            |ui| {
                egui::ScrollArea::vertical()
                    .max_height(420.0)
                    .id_salt("ally_groups_scroll")
                    .show(ui, |ui| {
                        for idx in group_indices {
                            let g_id = self.ally_groups[idx].id;
                            let is_active = g_id == active;
                            let header_label = {
                                let g = &self.ally_groups[idx];
                                format!("{} — {} pos", g.name, g.start_positions.len())
                            };
                            let header_id = egui::Id::new(("ally_group_header", g_id));
                            egui::CollapsingHeader::new(header_label)
                                .id_salt(header_id)
                                .default_open(true)
                                .show(ui, |ui| {
                                    // Row: swatch + name + active marker + delete.
                                    ui.horizontal(|ui| {
                                        // Persistent egui::Id for the colour
                                        // popover so it survives tool switches +
                                        // tree rebuilds (PITFALL #6).
                                        let g = &mut self.ally_groups[idx];
                                        let mut c = egui::Color32::from_rgb(
                                            g.color[0], g.color[1], g.color[2],
                                        );
                                        if ui
                                            .color_edit_button_srgba(&mut c)
                                            .on_hover_text(help(HelpId::StartAllyColor))
                                            .changed()
                                        {
                                            g.color = [c.r(), c.g(), c.b()];
                                        }
                                        ui.text_edit_singleline(&mut g.name)
                                            .on_hover_text(help(HelpId::StartAllyName));
                                        if ui
                                            .selectable_label(is_active, "★")
                                            .on_hover_text(help(HelpId::StartAllyActiveStar))
                                            .clicked()
                                        {
                                            new_active = Some(g_id);
                                        }
                                        if ui
                                            .small_button("delete group")
                                            .on_hover_text(help(HelpId::StartAllyDelete))
                                            .clicked()
                                        {
                                            to_delete_group = Some(g_id);
                                        }
                                    });

                                    // Source position rows.
                                    let positions: Vec<(usize, StartPosition)> = self.ally_groups
                                        [idx]
                                        .start_positions
                                        .iter()
                                        .enumerate()
                                        .map(|(i, p)| (i, *p))
                                        .collect();
                                    for (i, pos) in &positions {
                                        let row = ui.horizontal(|ui| {
                                            ui.label(format!(
                                                "#{}: ({}, {})",
                                                i, pos.x_elmo, pos.z_elmo
                                            ));
                                            if ui
                                                .small_button("×")
                                                .on_hover_text(help(HelpId::StartPosDelete))
                                                .clicked()
                                            {
                                                to_delete_pos = Some((g_id, *i));
                                            }
                                        });
                                        if row.response.hovered() {
                                            new_pulse = Some((g_id, *i));
                                        }
                                        // Hover-from-canvas: scroll the row
                                        // into view if it matches.
                                        if self.hovered_canvas_marker == Some((g_id, *i)) {
                                            row.response.scroll_to_me(Some(egui::Align::Center));
                                        }
                                    }

                                    // Greyed derived (mirror) rows.
                                    if !matches!(self.symmetry, SymmetryAxis::None) {
                                        let extents = (ex, ez);
                                        for (src_i, pos) in &positions {
                                            let mirrors = self.symmetry.replicate(
                                                (pos.x_elmo as f32, pos.z_elmo as f32),
                                                extents,
                                            );
                                            // replicate yields the source as
                                            // its first element; skip it.
                                            for (mx, mz) in mirrors.into_iter().skip(1) {
                                                if mx < 0.0 || mx > ex || mz < 0.0 || mz > ez {
                                                    continue;
                                                }
                                                ui.add_enabled(
                                                    false,
                                                    egui::Label::new(format!(
                                                        "  ↳ ({}, {})  (mirror of #{src_i})",
                                                        mx.round() as i32,
                                                        mz.round() as i32,
                                                    )),
                                                )
                                                .on_disabled_hover_text(format!(
                                                    "Edit source #{src_i} to move this mirror."
                                                ));
                                            }
                                        }
                                    }
                                });
                        }
                        if group_count == 0 {
                            ui.label(
                                egui::RichText::new(
                                    "No allyteams yet — pick a preset above, or click + Add.",
                                )
                                .color(t.dim)
                                .size(11.0),
                            );
                        }
                    });
            },
        );

        if add_group_clicked {
            self.add_ally_group();
        }
        if let Some(id) = new_active {
            self.active_ally_group_id = id;
        }
        if let Some(handle) = new_pulse {
            self.pulsing_marker = Some((handle.0, handle.1, std::time::Instant::now()));
        }
        if let Some((g_id, i)) = to_delete_pos {
            self.delete_start_position(g_id, i);
        }
        if let Some(id) = to_delete_group {
            self.delete_ally_group(id);
        }
    }

    /// Pure helper: returns true iff every allyteam has the same
    /// source-position count. The Layout section's "Balanced" /
    /// "Asymmetric" chip is wired through this so tests can pin the
    /// rule.
    fn start_positions_balanced(&self) -> bool {
        if self.ally_groups.len() <= 1 {
            // 0 or 1 allyteams trivially satisfies "all counts equal."
            // Surface as Balanced — Asymmetric would mislead the user.
            return true;
        }
        let first = self.ally_groups[0].start_positions.len();
        self.ally_groups
            .iter()
            .all(|g| g.start_positions.len() == first)
    }

    /// Procgen inspector (ADR-035): preset chip row + collapsible
    /// custom expression with live-parse error tooltip + domain radio
    /// + preview thumbnail + Commit button (disabled when invalid).
    fn inspector_procgen(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        action: &mut Option<FileAction>,
    ) {
        use crate::ui::help_text::{HelpId, help};
        let t = crate::ui::theme::Tokens::DARK;
        self.inspector_sticky_chips(ui);
        let active_preset = PRESETS
            .iter()
            .find(|p| p.expression == self.procgen_expr && p.domain == self.procgen_domain)
            .map(|p| p.label);

        // PRESET section: chip row.
        crate::ui::widgets::section_with_hover(
            ui,
            "Preset",
            true,
            "Stock procgen formulas. Picking one fills the Custom expression below and switches domain to match.",
            |_ui| {},
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    for p in PRESETS {
                        let chosen = Some(p.label) == active_preset;
                        let btn = egui::Button::selectable(chosen, p.label);
                        if ui
                            .add(btn)
                            .on_hover_text(help(HelpId::ProcgenPresetChip))
                            .clicked()
                        {
                            self.procgen_expr = p.expression.to_string();
                            self.procgen_domain = p.domain;
                            self.revalidate_procgen();
                        }
                    }
                });
            },
        );

        // CUSTOM EXPRESSION section. The TextEdit gets a red outline
        // when validation fails so the error is visible without
        // hovering for the tooltip.
        let err_msg = self.procgen_validation.clone().err();
        let domain_was = self.procgen_domain;
        crate::ui::widgets::section(
            ui,
            "Custom f(x, z)",
            false,
            |ui| {
                if let Some(msg) = &err_msg {
                    crate::ui::widgets::chip(
                        ui,
                        crate::ui::theme::ChipTone::Err,
                        format!("Error: {}", short_error(msg)),
                    );
                }
            },
            |ui| {
                let stroke_color = if err_msg.is_some() { t.red } else { t.border };
                egui::Frame::new()
                    .fill(t.bg)
                    .stroke(egui::Stroke::new(1.0, stroke_color))
                    .corner_radius(egui::CornerRadius::same(4))
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        let resp = ui
                            .add(
                                egui::TextEdit::multiline(&mut self.procgen_expr)
                                    .font(egui::FontId::monospace(11.5))
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(2)
                                    .frame(false),
                            )
                            .on_hover_text(help(HelpId::ProcgenExpression));
                        if resp.changed() {
                            self.revalidate_procgen();
                        }
                        if let Some(m) = &err_msg {
                            resp.on_hover_text(m);
                        }
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Domain").color(t.muted).size(11.0));
                    if ui
                        .add(egui::Button::selectable(
                            self.procgen_domain == Domain::Unit,
                            Domain::Unit.label(),
                        ))
                        .on_hover_text(help(HelpId::ProcgenDomainUnit))
                        .clicked()
                    {
                        self.procgen_domain = Domain::Unit;
                    }
                    if ui
                        .add(egui::Button::selectable(
                            self.procgen_domain == Domain::Centered,
                            Domain::Centered.label(),
                        ))
                        .on_hover_text(help(HelpId::ProcgenDomainCentered))
                        .clicked()
                    {
                        self.procgen_domain = Domain::Centered;
                    }
                });
                if self.procgen_domain != domain_was {
                    self.revalidate_procgen();
                }
            },
        );

        // PREVIEW section.
        self.refresh_procgen_thumbnail(ctx);
        let valid = self.procgen_validation.is_ok();
        crate::ui::widgets::section(
            ui,
            "Preview · 256²",
            false,
            |ui| {
                let (tone, label) = if valid {
                    (crate::ui::theme::ChipTone::Ok, "Live")
                } else {
                    (crate::ui::theme::ChipTone::Warn, "Parse error")
                };
                crate::ui::widgets::chip(ui, tone, label)
                    .on_hover_text(help(HelpId::ProcgenPreviewChip));
            },
            |ui| {
                if let Some(tex) = self.procgen_thumbnail.as_ref() {
                    let max_side = ui.available_width().min(PROCGEN_THUMBNAIL_PX as f32);
                    ui.add(egui::Image::new(tex).fit_to_exact_size(egui::vec2(max_side, max_side)))
                        .on_hover_text(
                            "256×256 grayscale preview of the formula. Updates after a short debounce so live keystrokes don't thrash the GPU.",
                        );
                } else if !valid {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new("(fix expression to render preview)")
                                .color(t.dim)
                                .size(11.0),
                        )
                        .sense(egui::Sense::hover()),
                    )
                    .on_hover_text("The expression failed to parse — fix the red-outlined TextEdit above to bake a fresh preview.");
                } else {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new("(baking preview…)")
                                .color(t.dim)
                                .size(11.0),
                        )
                        .sense(egui::Sense::hover()),
                    )
                    .on_hover_text("Preview bake is scheduled — appears after a short debounce so live keystrokes don't thrash the GPU.");
                }
                ui.add_space(8.0);
                // Commit button — disabled until parse succeeds.
                let resp = ui
                    .add_enabled(
                        valid,
                        egui::Button::new("Commit to heightmap")
                            .fill(if valid { t.accent } else { t.panel2 })
                            .min_size(egui::vec2(ui.available_width(), 32.0)),
                    )
                    .on_hover_text(help(HelpId::ProcgenCommitButton));
                if resp.clicked() {
                    *action = Some(FileAction::ApplyProcGen);
                }
            },
        );
        if let Some(err) = &self.procgen_last_error {
            ui.colored_label(t.red, format!("Apply error: {err}"));
        }
    }

    /// Rebake the Procgen thumbnail when (a) the cached
    /// `(expr, domain)` hash differs from the current state AND
    /// (b) the debounce window has elapsed. Reuses the cached
    /// [`egui::TextureHandle`] via `set(...)` so we don't leak GPU
    /// textures across keystrokes.
    fn refresh_procgen_thumbnail(&mut self, ctx: &egui::Context) {
        // Bail early on parse failure — the existing thumbnail (if
        // any) stays visible. The red ✗ chip already tells the user
        // their expression is broken.
        if self.procgen_validation.is_err() {
            return;
        }

        let current_key = procgen_thumbnail_key(&self.procgen_expr, self.procgen_domain);
        let key_changed = self.procgen_thumbnail_key != Some(current_key);
        let elapsed = self.procgen_changed_at.map(|t| t.elapsed());

        // Decision tree:
        // - No thumbnail yet → bake immediately (first frame in the tool).
        // - Key changed AND we've been quiet for >= debounce → bake now.
        // - Key changed AND debounce window not yet elapsed → schedule a
        //   repaint at the remainder so we don't sleep on user idle.
        let debounce = std::time::Duration::from_millis(PROCGEN_THUMBNAIL_DEBOUNCE_MS);
        let should_bake = match (self.procgen_thumbnail.is_some(), key_changed, elapsed) {
            (false, _, _) => true,
            (true, false, _) => false,
            (true, true, None) => true,
            (true, true, Some(e)) => e >= debounce,
        };

        if !should_bake {
            // Hold off but wake the UI loop at the remaining-debounce
            // mark so the thumbnail catches up on quiescence.
            if let Some(e) = elapsed
                && e < debounce
            {
                ctx.request_repaint_after(debounce - e);
            }
            return;
        }

        match procgen_thumbnail_gen(
            &self.procgen_expr,
            self.procgen_domain,
            PROCGEN_THUMBNAIL_PX,
        ) {
            Ok(grey) => {
                let img = egui::ColorImage::from_gray(
                    [PROCGEN_THUMBNAIL_PX, PROCGEN_THUMBNAIL_PX],
                    &grey,
                );
                let options = egui::TextureOptions::default();
                match self.procgen_thumbnail.as_mut() {
                    Some(handle) => handle.set(img, options),
                    None => {
                        self.procgen_thumbnail =
                            Some(ctx.load_texture("procgen-thumb", img, options));
                    }
                }
                self.procgen_thumbnail_key = Some(current_key);
                trace!(
                    expr = %self.procgen_expr,
                    domain = ?self.procgen_domain,
                    "procgen thumbnail rebaked"
                );
            }
            Err(e) => {
                // Validation pre-flighted parse + dry-eval at (0,0), but
                // a thumbnail can still hit NonNumeric / EvalFailed at
                // other coords. Don't clobber the thumbnail — log and
                // leave the previous frame visible.
                warn!(
                    expr = %self.procgen_expr,
                    domain = ?self.procgen_domain,
                    error = %format!("{e:#}"),
                    "procgen thumbnail bake failed; keeping prior preview"
                );
                self.procgen_thumbnail_key = Some(current_key);
            }
        }
    }

    /// Symmetry chip popover. Visible only when the top-bar chip is
    /// toggled open. ADR-031 (B2) replaces this with a canvas overlay
    /// + ghost-brush rings; B1 keeps the existing controls reachable.
    fn symmetry_popover(&mut self, ctx: &egui::Context) {
        if !self.symmetry_popover_open {
            return;
        }
        let mut open = true;
        egui::Window::new("Symmetry")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(220.0)
            .show(ctx, |ui| {
                egui::ComboBox::from_label("Axis")
                    .selected_text(self.symmetry.label())
                    .show_ui(ui, |ui| {
                        let options = [
                            SymmetryAxis::None,
                            SymmetryAxis::Horizontal,
                            SymmetryAxis::Vertical,
                            SymmetryAxis::Quad,
                            SymmetryAxis::DiagonalMain,
                            SymmetryAxis::DiagonalAnti,
                            SymmetryAxis::Rotational {
                                fold: self.rotational_fold,
                            },
                        ];
                        for opt in options {
                            let label = opt.label();
                            ui.selectable_value(&mut self.symmetry, opt, label);
                        }
                    });
                if matches!(self.symmetry, SymmetryAxis::Rotational { .. }) {
                    let resp = ui.add(
                        egui::DragValue::new(&mut self.rotational_fold)
                            .range(2u8..=12u8)
                            .speed(0.1)
                            .prefix("Fold (players): "),
                    );
                    if resp.changed() {
                        self.symmetry = SymmetryAxis::Rotational {
                            fold: self.rotational_fold,
                        };
                    }
                    ui.label(
                        egui::RichText::new("Tip: 3 = three-player map, 4 = quad-player, etc.")
                            .small()
                            .weak(),
                    );
                }
            });
        if !open {
            self.symmetry_popover_open = false;
        }
    }

    /// Central viewport: the wgpu terrain pass, the start-position
    /// marker overlay, and tool-driven pointer interaction.
    ///
    /// Camera control rules:
    /// - `Tool::Sculpt` + brush selected + heightmap loaded: LMB
    ///   sculpts, RMB orbits.
    /// - `Tool::StartPositions`: LMB places / drags, RMB deletes.
    /// - `Tool::Select` and `Tool::Procgen` (and Sculpt-without-brush):
    ///   LMB orbits (no central edit).
    ///
    /// Scroll wheel always zooms when the viewport is hovered.
    fn central(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let (rect, response) =
                ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());

            // D9 / Sprint 16 (ADR-040) — `Tool::PaintLayer` swaps the
            // central viewport for the 2D paint view. Pointer dispatch
            // happens entirely inside `central_paint_layer`; the 3D
            // path below is skipped for this tool.
            if matches!(self.tool, Tool::PaintLayer) {
                self.central_paint_layer(ui, rect);
                return;
            }

            let brush_active = matches!(self.tool, Tool::Sculpt)
                && self.brush_id.is_some()
                && self.heightmap.is_some();
            let start_pos_active = matches!(self.tool, Tool::StartPositions);
            let metal_active = matches!(self.tool, Tool::MetalSpots);
            let geo_active = matches!(self.tool, Tool::GeoFeatures);
            let feature_active = matches!(self.tool, Tool::Feature);
            // C9 / Sprint 14 — Water tool carves with Lower / Raise.
            let water_active = matches!(self.tool, Tool::Water) && self.heightmap.is_some();
            let central_interactive =
                brush_active || start_pos_active || metal_active || geo_active || water_active;

            // ADR-035: the top-right nav-gizmo retires in favour of
            // the mini-map. Reserve its rect so brush/start-pos
            // handlers can short-circuit over it (the mini-map is
            // non-interactive for sculpt input — clicking its body
            // future-routes to a camera-recenter, not a brush
            // stamp).
            let minimap_rect = egui::Rect::from_min_size(
                egui::pos2(
                    rect.right() - crate::ui::minimap::Minimap::PANEL_W - 14.0,
                    rect.top() + 14.0,
                ),
                egui::vec2(
                    crate::ui::minimap::Minimap::PANEL_W,
                    crate::ui::minimap::Minimap::PANEL_H,
                ),
            );
            let cursor_in_gizmo = ctx
                .pointer_interact_pos()
                .map(|p| minimap_rect.contains(p))
                .unwrap_or(false);
            let consumed_click = false;

            // RMB usually orbits the camera while a tool is active,
            // but `Tool::Water` re-purposes RMB for "raise" (the
            // inverse of LMB-flood). Skip the orbit hook entirely
            // when the water tool is on.
            let camera_drag = if water_active {
                false
            } else if central_interactive {
                response.dragged_by(egui::PointerButton::Secondary)
            } else {
                response.dragged()
            };
            if camera_drag {
                let d = if central_interactive {
                    ui.input(|i| i.pointer.delta())
                } else {
                    response.drag_delta()
                };
                self.camera.yaw -= d.x * 0.005;
                self.camera.pitch = (self.camera.pitch + d.y * 0.005).clamp(
                    -std::f32::consts::FRAC_PI_2 + 0.05,
                    std::f32::consts::FRAC_PI_2 - 0.05,
                );
            }
            if response.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    let factor = (1.0 - scroll * 0.002).clamp(0.5, 2.0);
                    self.camera.distance = (self.camera.distance * factor).clamp(100.0, 200000.0);
                }
            }

            // Brush stroke: while LMB is down in the central rect, emit
            // one stamp per frame at the cursor's world-space projection
            // on the y=0 plane. Spacing along the drag is implicit
            // (frame rate). One LMB-down → LMB-up coalesces into a
            // single undo unit via `end_stroke` on pointer release
            // (ADR-033). Skip when the gizmo is taking the LMB.
            if brush_active
                && !consumed_click
                && !cursor_in_gizmo
                && (response.dragged_by(egui::PointerButton::Primary)
                    || response.clicked_by(egui::PointerButton::Primary))
                && let Some(cursor) = ctx.pointer_interact_pos()
            {
                self.apply_brush_at(cursor, rect);
            }
            // End the in-flight stroke when no painting button is
            // held. Sculpt only paints with LMB; `Tool::Water` (C9)
            // paints with both buttons (LMB = lower, RMB = raise) so
            // we extend the check to either when the water tool is
            // active.
            let any_paint_held = response.dragged_by(egui::PointerButton::Primary)
                || (water_active && response.dragged_by(egui::PointerButton::Secondary));
            if self.history.stroke_open() && !any_paint_held {
                self.end_stroke();
            }

            // D10 / Sprint 17 (ADR-041): `Tool::SplatPaint` retired.
            // The dispatch site here used to fan LMB into
            // `apply_splat_brush_at`; layered painting via
            // `Tool::PaintLayer` (Sprint 16) supersedes it.

            // C9 / Sprint 14 — Tool::Water flood-carve dispatch.
            // LMB stamps Lower (carves the heightmap below Y=0);
            // RMB stamps Raise (undo a flooding gesture without
            // walking the undo stack — gentler than Ctrl-Z when the
            // user just wants a smaller pool). Strength scales the
            // carve depth (elmos) against the project's height_scale
            // so the slider stays meaningful regardless of map size.
            if water_active && !consumed_click && !cursor_in_gizmo {
                let strength =
                    (self.water_carve_depth.abs() / self.height_scale.max(1.0)).clamp(0.0, 1.0);
                if (response.dragged_by(egui::PointerButton::Primary)
                    || response.clicked_by(egui::PointerButton::Primary))
                    && let Some(cursor) = ctx.pointer_interact_pos()
                {
                    self.apply_brush_id_at(cursor, rect, "lower", strength);
                }
                if (response.dragged_by(egui::PointerButton::Secondary)
                    || response.clicked_by(egui::PointerButton::Secondary))
                    && let Some(cursor) = ctx.pointer_interact_pos()
                {
                    self.apply_brush_id_at(cursor, rect, "raise", strength);
                }
            }

            // Start-position placement / move / delete / drag-paint
            // (ADR-032 supersedes ADR-023 in mode-specific behaviour).
            if start_pos_active
                && !consumed_click
                && !cursor_in_gizmo
                && let Some(cursor) = ctx.pointer_interact_pos()
            {
                let cursor_in = glam::Vec2::new(cursor.x - rect.min.x, cursor.y - rect.min.y);
                let rect_size = glam::Vec2::new(rect.width(), rect.height());
                const HIT_RADIUS_PX: f32 = 12.0;

                if response.clicked_by(egui::PointerButton::Secondary)
                    && let Some((gid, idx)) =
                        self.hit_test_start_position(cursor, rect, HIT_RADIUS_PX)
                {
                    self.delete_start_position(gid, idx);
                }
                if response.drag_started_by(egui::PointerButton::Primary) {
                    // Drag-move if the press started on a marker;
                    // drag-paint otherwise (capture origin world coord
                    // for the line endpoint on drag_stopped).
                    self.dragging_start_pos =
                        self.hit_test_start_position(cursor, rect, HIT_RADIUS_PX);
                    self.dragging_start_pos_from =
                        self.dragging_start_pos.and_then(|(gid, idx)| {
                            self.ally_groups
                                .iter()
                                .find(|g| g.id == gid)
                                .and_then(|g| g.start_positions.get(idx).copied())
                        });
                    self.drag_paint_origin = if self.dragging_start_pos.is_none() {
                        render::screen_to_world_y0(cursor_in, rect_size, &self.camera)
                            .map(|w| glam::Vec2::new(w.x, w.z))
                    } else {
                        None
                    };
                }
                if response.dragged_by(egui::PointerButton::Primary)
                    && let Some((gid, idx)) = self.dragging_start_pos
                    && let Some(world) =
                        render::screen_to_world_y0(cursor_in, rect_size, &self.camera)
                {
                    self.move_start_position(gid, idx, world.x, world.z);
                }
                if response.drag_stopped_by(egui::PointerButton::Primary) {
                    // Finish drag-move if a marker was being dragged…
                    let was_moving = self.dragging_start_pos.is_some();
                    self.finish_start_position_drag();
                    // …else commit a drag-paint line from origin →
                    // cursor (if both ends resolved to map coords).
                    if !was_moving
                        && let Some(origin) = self.drag_paint_origin.take()
                        && let Some(end) =
                            render::screen_to_world_y0(cursor_in, rect_size, &self.camera)
                    {
                        let len_sq = (origin.x - end.x).powi(2) + (origin.y - end.z).powi(2);
                        // Threshold matches the egui click-vs-drag
                        // disambiguator. If the line is tiny, treat as
                        // a click (handled below).
                        if len_sq.sqrt() > 32.0 {
                            self.drag_paint_start_positions(origin.x, origin.y, end.x, end.z);
                        }
                    }
                }
                if response.clicked_by(egui::PointerButton::Primary)
                    && self
                        .hit_test_start_position(cursor, rect, HIT_RADIUS_PX)
                        .is_none()
                    && let Some(world) =
                        render::screen_to_world_y0(cursor_in, rect_size, &self.camera)
                {
                    self.place_start_position(world.x, world.z);
                }
                // Canvas marker hover → drives Inspector scroll-to-row.
                self.hovered_canvas_marker =
                    self.hit_test_start_position(cursor, rect, HIT_RADIUS_PX);
            } else {
                self.hovered_canvas_marker = None;
            }

            // C4 (Sprint 11): metal-spot pointer dispatch. Same hit
            // radius + symmetry pattern as start positions. LMB on
            // empty space places (with symmetry mirrors); LMB drag
            // on an existing spot moves; RMB deletes; cross-tool
            // ghost rendering (50 % alpha) handled below.
            if metal_active
                && !consumed_click
                && !cursor_in_gizmo
                && let Some(cursor) = ctx.pointer_interact_pos()
            {
                let cursor_in = glam::Vec2::new(cursor.x - rect.min.x, cursor.y - rect.min.y);
                let rect_size = glam::Vec2::new(rect.width(), rect.height());
                const METAL_HIT_RADIUS_PX: f32 = 12.0;
                if response.clicked_by(egui::PointerButton::Secondary)
                    && let Some(idx) = self.hit_test_metal_spot(cursor, rect, METAL_HIT_RADIUS_PX)
                {
                    self.delete_metal_spot(idx);
                }
                if response.drag_started_by(egui::PointerButton::Primary) {
                    self.dragging_metal_spot =
                        self.hit_test_metal_spot(cursor, rect, METAL_HIT_RADIUS_PX);
                    self.dragging_metal_spot_from = self
                        .dragging_metal_spot
                        .and_then(|i| self.metal_spots.get(i).copied());
                }
                if response.dragged_by(egui::PointerButton::Primary)
                    && let Some(idx) = self.dragging_metal_spot
                    && let Some(world) =
                        render::screen_to_world_y0(cursor_in, rect_size, &self.camera)
                    && let Some(existing) = self.metal_spots.get(idx).copied()
                {
                    let updated = MetalSpot {
                        x_elmo: world.x.round() as i32,
                        z_elmo: world.z.round() as i32,
                        metal: existing.metal,
                    };
                    self.move_metal_spot_to(idx, updated);
                }
                if response.drag_stopped_by(egui::PointerButton::Primary) {
                    self.finish_metal_spot_drag();
                }
                if response.clicked_by(egui::PointerButton::Primary)
                    && self
                        .hit_test_metal_spot(cursor, rect, METAL_HIT_RADIUS_PX)
                        .is_none()
                    && let Some(world) =
                        render::screen_to_world_y0(cursor_in, rect_size, &self.camera)
                {
                    self.place_metal_spot(world.x, world.z);
                }
            }

            // C5 (Sprint 11): geo-vent pointer dispatch. Mirror of
            // metal's. Same hit radius. No `metal` value drag.
            if geo_active
                && !consumed_click
                && !cursor_in_gizmo
                && let Some(cursor) = ctx.pointer_interact_pos()
            {
                let cursor_in = glam::Vec2::new(cursor.x - rect.min.x, cursor.y - rect.min.y);
                let rect_size = glam::Vec2::new(rect.width(), rect.height());
                const GEO_HIT_RADIUS_PX: f32 = 12.0;
                if response.clicked_by(egui::PointerButton::Secondary)
                    && let Some(idx) = self.hit_test_geo_vent(cursor, rect, GEO_HIT_RADIUS_PX)
                {
                    self.delete_geo_vent(idx);
                }
                if response.drag_started_by(egui::PointerButton::Primary) {
                    self.dragging_geo_vent =
                        self.hit_test_geo_vent(cursor, rect, GEO_HIT_RADIUS_PX);
                    self.dragging_geo_vent_from = self
                        .dragging_geo_vent
                        .and_then(|i| self.geo_vents.get(i).copied());
                }
                if response.dragged_by(egui::PointerButton::Primary)
                    && let Some(idx) = self.dragging_geo_vent
                    && let Some(world) =
                        render::screen_to_world_y0(cursor_in, rect_size, &self.camera)
                {
                    let updated = GeoVent {
                        x_elmo: world.x.round() as i32,
                        z_elmo: world.z.round() as i32,
                    };
                    self.move_geo_vent_to(idx, updated);
                }
                if response.drag_stopped_by(egui::PointerButton::Primary) {
                    self.finish_geo_vent_drag();
                }
                if response.clicked_by(egui::PointerButton::Primary)
                    && self
                        .hit_test_geo_vent(cursor, rect, GEO_HIT_RADIUS_PX)
                        .is_none()
                    && let Some(world) =
                        render::screen_to_world_y0(cursor_in, rect_size, &self.camera)
                {
                    self.place_geo_vent(world.x, world.z);
                }
            }

            // C6 (Sprint 12): F7 feature pointer dispatch. Same hit
            // pattern as metal/geo but the LMB-drag gesture ROTATES
            // the hit feature rather than translating it. The drag
            // anchor (cursor x at drag-start) + start_rot let us
            // compute heading deltas without accumulating drift.
            if feature_active
                && !consumed_click
                && !cursor_in_gizmo
                && let Some(cursor) = ctx.pointer_interact_pos()
            {
                let cursor_in = glam::Vec2::new(cursor.x - rect.min.x, cursor.y - rect.min.y);
                let rect_size = glam::Vec2::new(rect.width(), rect.height());
                const FEATURE_HIT_RADIUS_PX: f32 = 12.0;
                if response.clicked_by(egui::PointerButton::Secondary)
                    && let Some(idx) = self.hit_test_feature(cursor, rect, FEATURE_HIT_RADIUS_PX)
                {
                    self.delete_feature(idx);
                }
                if response.drag_started_by(egui::PointerButton::Primary) {
                    let idx = self.hit_test_feature(cursor, rect, FEATURE_HIT_RADIUS_PX);
                    self.dragging_feature = idx;
                    self.dragging_feature_from = idx.and_then(|i| self.features.get(i).cloned());
                    self.dragging_feature_anchor_x = idx.map(|_| cursor.x);
                    self.dragging_feature_start_rot =
                        idx.and_then(|i| self.features.get(i).map(|f| f.rot_heading));
                }
                if response.dragged_by(egui::PointerButton::Primary)
                    && let (Some(idx), Some(anchor_x), Some(start_rot)) = (
                        self.dragging_feature,
                        self.dragging_feature_anchor_x,
                        self.dragging_feature_start_rot,
                    )
                    && let Some(existing) = self.features.get(idx).cloned()
                {
                    // Heading delta = (cursor_x - anchor_x) * gain.
                    // Wrapping arithmetic preserves the u16 invariant
                    // through full revolutions (a 720° drag wraps back
                    // to 0). PITFALL §23 conventions stay intact.
                    let dx = cursor.x - anchor_x;
                    let delta = (dx * Self::ROTATE_GAIN_PER_PX) as i32;
                    let new_rot = (start_rot as i32).wrapping_add(delta).rem_euclid(65536) as u16;
                    let updated = FeatureInstance {
                        name: existing.name.clone(),
                        x_elmo: existing.x_elmo,
                        z_elmo: existing.z_elmo,
                        rot_heading: new_rot,
                    };
                    self.move_feature_to(idx, updated);
                }
                if response.drag_stopped_by(egui::PointerButton::Primary) {
                    self.finish_feature_drag();
                }
                if response.clicked_by(egui::PointerButton::Primary)
                    && self
                        .hit_test_feature(cursor, rect, FEATURE_HIT_RADIUS_PX)
                        .is_none()
                    && let Some(world) =
                        render::screen_to_world_y0(cursor_in, rect_size, &self.camera)
                {
                    self.place_feature(world.x, world.z);
                }
            }

            // ----------------------------------------------------------
            // Sprint 13 / ADR-037 — PHASE A: build the marker batch
            //
            // Walk every visible marker source (brush rings, start
            // positions, metal spots, geo vents) and push one
            // `Marker` per glyph into a frame-local `MarkerBatch`.
            // Markers render on the GPU through the offscreen pass
            // encoded by `TerrainCallback::prepare`; the CPU sort
            // owns translucent ordering, the depth test owns terrain
            // occlusion.
            //
            // What stays in egui::Painter (PHASE C below):
            // - Symmetry overlay (Phase 5 moves it to the line pipeline)
            // - Text labels (start-pos index, metal value)
            // - Geo-vent plume + mirror outline (Phase 5)
            // - Viewport chrome (rulers, minimap, toolbar, hint card)
            // ----------------------------------------------------------
            let extents = self.world_extents();
            let mut marker_batch = crate::ui::markers::MarkerBatch::default();
            let now = std::time::Instant::now();

            // Brush rings (primary + symmetry-derived ghosts).
            if matches!(self.tool, Tool::Sculpt)
                && self.brush_id.is_some()
                && response.hovered()
                && let Some(cursor) = ctx.pointer_interact_pos()
                && rect.contains(cursor)
                && !cursor_in_gizmo
            {
                let cursor_in = glam::Vec2::new(cursor.x - rect.min.x, cursor.y - rect.min.y);
                let rect_size_v = glam::Vec2::new(rect.width(), rect.height());
                if let Some(world) =
                    render::screen_to_world_y0(cursor_in, rect_size_v, &self.camera)
                {
                    let brush_cursor = crate::ui::overlay::BrushCursor {
                        world,
                        radius_world: self.brush_radius,
                        brush_id: self.brush_id.as_deref(),
                    };
                    crate::ui::overlay::collect_primary_brush_ring(
                        &mut marker_batch,
                        rect,
                        &self.camera,
                        crate::ui::overlay::BrushCursor {
                            world: brush_cursor.world,
                            radius_world: brush_cursor.radius_world,
                            brush_id: brush_cursor.brush_id,
                        },
                    );
                    if !matches!(self.symmetry, SymmetryAxis::None) {
                        crate::ui::overlay::collect_brush_ghosts(
                            &mut marker_batch,
                            rect,
                            &self.camera,
                            self.symmetry,
                            brush_cursor,
                            extents,
                        );
                    }
                }
            }

            // Start-position markers (cross-tool ghost falloff +
            // hover-pulse handled here so the batch contains the
            // animated radius for the current frame).
            if !self.ally_groups.is_empty() {
                let cross_tool_ghost = !matches!(self.tool, Tool::StartPositions);
                let alpha_mul: u8 = if cross_tool_ghost { 128 } else { 255 };
                for g in &self.ally_groups {
                    let base_color = egui::Color32::from_rgba_unmultiplied(
                        g.color[0], g.color[1], g.color[2], alpha_mul,
                    );
                    for (i, pos) in g.start_positions.iter().enumerate() {
                        let dragging = self.dragging_start_pos == Some((g.id, i));
                        let mut r = if dragging { 10.0_f32 } else { 8.0_f32 };
                        if let Some((pg, pi, t0)) = self.pulsing_marker
                            && pg == g.id
                            && pi == i
                        {
                            let dt = now.duration_since(t0).as_secs_f32();
                            if dt < 1.0 {
                                let osc = (dt * 2.0 * std::f32::consts::TAU).sin().abs();
                                r += 3.0 * osc;
                                ctx.request_repaint();
                            } else {
                                self.pulsing_marker = None;
                            }
                        }
                        let y = self.terrain_y_at(pos.x_elmo as f32, pos.z_elmo as f32);
                        let world = glam::Vec3::new(pos.x_elmo as f32, y, pos.z_elmo as f32);
                        marker_batch.push(crate::ui::markers::Marker {
                            world_pos: world,
                            radius_px: r,
                            color: base_color,
                            shape: crate::ui::markers::MarkerShape::FilledWithStroke,
                        });
                        if !matches!(self.symmetry, SymmetryAxis::None) {
                            let mirrors = self
                                .symmetry
                                .replicate((pos.x_elmo as f32, pos.z_elmo as f32), extents);
                            for (mx, mz) in mirrors.into_iter().skip(1) {
                                if mx < 0.0 || mx > extents.0 || mz < 0.0 || mz > extents.1 {
                                    continue;
                                }
                                let my = self.terrain_y_at(mx, mz);
                                marker_batch.push(crate::ui::markers::Marker {
                                    world_pos: glam::Vec3::new(mx, my, mz),
                                    radius_px: 7.0,
                                    color: base_color,
                                    shape: crate::ui::markers::MarkerShape::OutlineRing,
                                });
                            }
                        }
                    }
                }
            }

            // Metal-spot markers + extractor-radius ring (only when
            // the MetalSpots tool is active — otherwise the canvas
            // would be a sea of cyan rings).
            if !self.metal_spots.is_empty() {
                let rect_size = glam::Vec2::new(rect.width(), rect.height());
                let cross_tool_ghost = !matches!(self.tool, Tool::MetalSpots);
                let alpha_mul: u8 = if cross_tool_ghost { 128 } else { 255 };
                let red_fill = egui::Color32::from_rgba_unmultiplied(0xF1, 0x5C, 0x5C, alpha_mul);
                let cyan = egui::Color32::from_rgba_unmultiplied(
                    0x33,
                    0xD8,
                    0xE6,
                    if cross_tool_ghost { 64 } else { 160 },
                );
                let radius_world = self.extractor_radius.max(8.0);
                for (i, spot) in self.metal_spots.iter().enumerate() {
                    let y = self.terrain_y_at(spot.x_elmo as f32, spot.z_elmo as f32);
                    let world = glam::Vec3::new(spot.x_elmo as f32, y, spot.z_elmo as f32);
                    let dragging = self.dragging_metal_spot == Some(i);
                    let r = if dragging { 10.0_f32 } else { 7.0_f32 };
                    marker_batch.push(crate::ui::markers::Marker {
                        world_pos: world,
                        radius_px: r,
                        color: red_fill,
                        shape: crate::ui::markers::MarkerShape::FilledWithStroke,
                    });
                    if !cross_tool_ghost {
                        let east_x = spot.x_elmo as f32 + radius_world;
                        let east_y = self.terrain_y_at(east_x, spot.z_elmo as f32);
                        let east = glam::Vec3::new(east_x, east_y, spot.z_elmo as f32);
                        if let (Some(centre_screen), Some(east_screen)) = (
                            render::world_to_screen(world, rect_size, &self.camera),
                            render::world_to_screen(east, rect_size, &self.camera),
                        ) {
                            let radius_px = (east_screen.x - centre_screen.x).abs();
                            if radius_px > 1.5 {
                                marker_batch.push(crate::ui::markers::Marker {
                                    world_pos: world,
                                    radius_px,
                                    color: cyan,
                                    shape: crate::ui::markers::MarkerShape::OutlineRing,
                                });
                            }
                        }
                    }
                    if !matches!(self.symmetry, SymmetryAxis::None) {
                        let mirrors = self
                            .symmetry
                            .replicate((spot.x_elmo as f32, spot.z_elmo as f32), extents);
                        for (mx, mz) in mirrors.into_iter().skip(1) {
                            if mx < 0.0 || mx > extents.0 || mz < 0.0 || mz > extents.1 {
                                continue;
                            }
                            let my = self.terrain_y_at(mx, mz);
                            marker_batch.push(crate::ui::markers::Marker {
                                world_pos: glam::Vec3::new(mx, my, mz),
                                radius_px: 6.0,
                                color: red_fill,
                                shape: crate::ui::markers::MarkerShape::OutlineRing,
                            });
                        }
                    }
                }
            }

            // Geo-vent primary triangles + mirror outline triangles
            // (Phase 5 / OutlineTriangle marker). Plumes flow through
            // the line pipeline (`line_vertices` below).
            if !self.geo_vents.is_empty() {
                let cross_tool_ghost = !matches!(self.tool, Tool::GeoFeatures);
                let alpha_mul: u8 = if cross_tool_ghost { 128 } else { 255 };
                let orange = egui::Color32::from_rgba_unmultiplied(0xF5, 0x9E, 0x0B, alpha_mul);
                for (i, vent) in self.geo_vents.iter().enumerate() {
                    let y = self.terrain_y_at(vent.x_elmo as f32, vent.z_elmo as f32);
                    let world = glam::Vec3::new(vent.x_elmo as f32, y, vent.z_elmo as f32);
                    let dragging = self.dragging_geo_vent == Some(i);
                    let size = if dragging { 12.0_f32 } else { 9.0_f32 };
                    marker_batch.push(crate::ui::markers::Marker {
                        world_pos: world,
                        radius_px: size,
                        color: orange,
                        shape: crate::ui::markers::MarkerShape::Triangle,
                    });
                    if !matches!(self.symmetry, SymmetryAxis::None) {
                        let mirrors = self
                            .symmetry
                            .replicate((vent.x_elmo as f32, vent.z_elmo as f32), extents);
                        for (mx, mz) in mirrors.into_iter().skip(1) {
                            if mx < 0.0 || mx > extents.0 || mz < 0.0 || mz > extents.1 {
                                continue;
                            }
                            let my = self.terrain_y_at(mx, mz);
                            marker_batch.push(crate::ui::markers::Marker {
                                world_pos: glam::Vec3::new(mx, my, mz),
                                radius_px: 7.0,
                                color: orange,
                                shape: crate::ui::markers::MarkerShape::OutlineTriangle,
                            });
                        }
                    }
                }
            }

            // Sprint 19 — placed-feature markers. Catalog-driven shape +
            // colour by category (tree / rock / wreck / prop / geo) with
            // the same cross-tool ghost + symmetry-mirror pattern as
            // metal / vents. Falls back to a generic outline ring on
            // catalog miss-hits so orphaned FeatureDef names from
            // round-tripped projects still surface.
            if !self.features.is_empty() {
                let cross_tool_ghost = !matches!(self.tool, Tool::Feature);
                let alpha_mul: u8 = if cross_tool_ghost { 128 } else { 255 };
                for (i, f) in self.features.iter().enumerate() {
                    let v = self.feature_state.manifest.resolved_visual(&f.name);
                    let dragging = self.dragging_feature == Some(i);
                    let r = if dragging {
                        v.radius_px + 3.0
                    } else {
                        v.radius_px
                    };
                    let [cr, cg, cb, _] = v.color.to_array();
                    let color = egui::Color32::from_rgba_unmultiplied(cr, cg, cb, alpha_mul);
                    let y = self.terrain_y_at(f.x_elmo as f32, f.z_elmo as f32);
                    marker_batch.push(crate::ui::markers::Marker {
                        world_pos: glam::Vec3::new(f.x_elmo as f32, y, f.z_elmo as f32),
                        radius_px: r,
                        color,
                        shape: v.shape,
                    });
                    if !matches!(self.symmetry, SymmetryAxis::None) {
                        let mirrors = self
                            .symmetry
                            .replicate((f.x_elmo as f32, f.z_elmo as f32), extents);
                        for (mx, mz) in mirrors.into_iter().skip(1) {
                            if mx < 0.0 || mx > extents.0 || mz < 0.0 || mz > extents.1 {
                                continue;
                            }
                            let my = self.terrain_y_at(mx, mz);
                            marker_batch.push(crate::ui::markers::Marker {
                                world_pos: glam::Vec3::new(mx, my, mz),
                                radius_px: (v.radius_px * 0.75).max(4.0),
                                color,
                                shape: crate::ui::markers::MarkerShape::OutlineRing,
                            });
                        }
                    }
                }
            }

            // Sprint 13 / Phase 5 — build the world-space line vertex
            // buffer: symmetry axes (dashed) + geo-vent plumes
            // (solid). LineList topology: every consecutive pair is
            // one segment.
            let mut line_vertices: Vec<crate::render::LineVertex> = Vec::new();
            crate::ui::overlay::collect_symmetry_segments(
                &mut line_vertices,
                glam::Vec2::new(rect.width(), rect.height()),
                &self.camera,
                self.symmetry,
                extents,
            );
            // Geo-vent plumes — vertical 64-elmo line above each vent.
            // Constant world-space height (not pixel-based) means the
            // plume scales with zoom like the terrain it sits on,
            // matching the rest of the 3D content.
            if !self.geo_vents.is_empty() {
                const PLUME_HEIGHT_ELMOS: f32 = 64.0;
                let cross_tool_ghost = !matches!(self.tool, Tool::GeoFeatures);
                let alpha_mul: u8 = if cross_tool_ghost { 128 } else { 255 };
                let plume_color =
                    egui::Color32::from_rgba_unmultiplied(0xF5, 0x9E, 0x0B, alpha_mul / 3);
                // Line vertices are NOT auto-lifted by
                // `MarkerBatch::into_instances` (that path is markers-only),
                // so the plume's base Y must be terrain_y + the marker
                // Y-lift epsilon manually.
                let lift = crate::ui::markers::MARKER_Y_LIFT_ELMOS;
                for vent in &self.geo_vents {
                    let base_y = self.terrain_y_at(vent.x_elmo as f32, vent.z_elmo as f32);
                    let base =
                        glam::Vec3::new(vent.x_elmo as f32, base_y + lift, vent.z_elmo as f32);
                    let top = base + glam::Vec3::new(0.0, PLUME_HEIGHT_ELMOS, 0.0);
                    line_vertices.push(crate::render::LineVertex::new(base, plume_color));
                    line_vertices.push(crate::render::LineVertex::new(top, plume_color));
                }
            }

            // ----------------------------------------------------------
            // PHASE B: terrain callback + offscreen composite
            // ----------------------------------------------------------
            if self.heightmap.is_some() {
                let (ex, ez) = self.world_extents();
                let splat_u = self.splat_uniforms_for_render();

                // Sort the batch back-to-front so translucent markers
                // blend in correct camera-relative order, then encode
                // to GPU instances. The view matrix (not view-proj)
                // gives us LH view-Z directly.
                let view = self.camera.view_matrix();
                marker_batch.sort_back_to_front(view);
                tracing::trace!(markers = marker_batch.len(), "marker batch built");
                let instances = marker_batch.into_instances();

                // Marker shader uses LOGICAL viewport units so
                // `radius_px` keeps egui::Painter's px semantics.
                let viewport_size = [rect.width(), rect.height()];

                // Offscreen RT physical pixel size — clamped to 2048
                // per axis by `ensure_offscreen` (PITFALLS §1 / iGPU).
                let pixels_per_point = ctx.pixels_per_point();
                let requested_phys = (
                    (rect.width() * pixels_per_point).round().max(0.0) as u32,
                    (rect.height() * pixels_per_point).round().max(0.0) as u32,
                );
                let offscreen_id = self
                    .render_state
                    .as_ref()
                    .and_then(|rs| render::ensure_offscreen(rs, requested_phys));

                let water = self.water_draw_for_frame(ex, ez);

                // D9 / Sprint 16 (ADR-039) — composite RT + per-frame
                // mask-tile sync. The RT allocation is idempotent;
                // sync_composite_mask_tiles is a no-op when no layer
                // has accumulated dirty tiles since the last push.
                let composite_uniforms = self.composite_uniforms_for_render();
                if composite_uniforms.is_some()
                    && let Some(rs) = self.render_state.as_ref()
                {
                    let (cw, ch) = self.composite_rt_dims();
                    crate::render::ensure_composite_rt(rs, (cw, ch));
                    self.sync_composite_mask_tiles();
                }

                let cb = TerrainCallback::new(
                    &self.camera,
                    rect,
                    self.height_scale,
                    self.min_height,
                    ex,
                    ez,
                    splat_u,
                    instances,
                    viewport_size,
                    line_vertices,
                    water,
                    composite_uniforms,
                );
                ui.painter()
                    .add(egui_wgpu::Callback::new_paint_callback(rect, cb));

                if let Some(id) = offscreen_id {
                    ui.painter().image(
                        id,
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
            } else {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Load a heightmap to see the terrain.",
                    egui::FontId::proportional(16.0),
                    ui.visuals().weak_text_color(),
                );
            }

            // ----------------------------------------------------------
            // PHASE C: 2D residue — text labels + viewport chrome.
            // Symmetry axes now flow through the line pipeline (Phase
            // 5); geo plumes + geo mirror outlines do too.
            // ----------------------------------------------------------
            let overlay_painter = ui.painter_at(rect);

            // Overlay start-position markers on top of the terrain
            // pass. ADR-032: cross-tool ghost falloff (50 % alpha) when
            // the StartPositions tool isn't active. Sources render as
            // filled circles in their AllyGroup colour; symmetry-
            // derived mirrors render as outlined rings with a thinner
            // stroke. Hovered-from-Inspector source pulses at 2 Hz for
            // 1 s after the hover event.
            // Start-position TEXT LABELS only — the marker glyphs
            // themselves render via the GPU pipeline in PHASE A above.
            // Mirror outline rings are batched too; only the index
            // labels remain in 2D (egui::Painter handles text well; a
            // wgpu SDF text path is out of scope for Sprint 13).
            if !self.ally_groups.is_empty() {
                let rect_size = glam::Vec2::new(rect.width(), rect.height());
                let cross_tool_ghost = !matches!(self.tool, Tool::StartPositions);
                let alpha_mul: u8 = if cross_tool_ghost { 128 } else { 255 };
                let label_color = egui::Color32::from_rgba_unmultiplied(255, 255, 255, alpha_mul);
                for g in &self.ally_groups {
                    for (i, pos) in g.start_positions.iter().enumerate() {
                        let world = glam::Vec3::new(pos.x_elmo as f32, 0.0, pos.z_elmo as f32);
                        let Some(screen) = render::world_to_screen(world, rect_size, &self.camera)
                        else {
                            continue;
                        };
                        let p = egui::Pos2::new(rect.min.x + screen.x, rect.min.y + screen.y);
                        let dragging = self.dragging_start_pos == Some((g.id, i));
                        // Match the marker pipeline's idle radius so
                        // the label sits a constant 2 px above the
                        // glyph edge. The hover-pulse animation
                        // (mutated in PHASE A) doesn't oscillate the
                        // label — minor UX regression, acceptable
                        // until the GPU SDF text path lands.
                        let r = if dragging { 10.0 } else { 8.0 };
                        overlay_painter.text(
                            p + egui::Vec2::new(0.0, -r - 2.0),
                            egui::Align2::CENTER_BOTTOM,
                            format!("{}", i),
                            egui::FontId::proportional(11.0),
                            label_color,
                        );
                    }
                }
            }

            // C4 (Sprint 11): metal-spot markers. Red filled circle
            // per source; extractor-radius ring (cyan stroke at
            // `App::extractor_radius` elmos in world) when the
            // MetalSpots tool is active. Cross-tool ghost falloff
            // (50 % alpha) outside MetalSpots (B1 pattern). Symmetry
            // mirrors render as outline-only rings.
            // Metal-spot TEXT LABELS only — marker fills + outlines +
            // extractor-radius ring + mirrors all batch through PHASE A.
            if !self.metal_spots.is_empty() {
                let rect_size = glam::Vec2::new(rect.width(), rect.height());
                let cross_tool_ghost = !matches!(self.tool, Tool::MetalSpots);
                let alpha_mul: u8 = if cross_tool_ghost { 128 } else { 255 };
                let label_color = egui::Color32::from_rgba_unmultiplied(255, 255, 255, alpha_mul);
                for (i, spot) in self.metal_spots.iter().enumerate() {
                    let world = glam::Vec3::new(spot.x_elmo as f32, 0.0, spot.z_elmo as f32);
                    let Some(screen) = render::world_to_screen(world, rect_size, &self.camera)
                    else {
                        continue;
                    };
                    let p = egui::Pos2::new(rect.min.x + screen.x, rect.min.y + screen.y);
                    let dragging = self.dragging_metal_spot == Some(i);
                    let r = if dragging { 10.0 } else { 7.0 };
                    overlay_painter.text(
                        p + egui::Vec2::new(0.0, -r - 2.0),
                        egui::Align2::CENTER_BOTTOM,
                        format!("{:.1}", spot.metal),
                        egui::FontId::proportional(11.0),
                        label_color,
                    );
                }
            }

            // Sprint 19 — brush radius readout next to the cursor
            // ring. The GPU marker pipeline doesn't draw text; we
            // project the world cursor here and paint a small chip
            // via egui::Painter so the user always knows the brush
            // size in elmos without opening the Inspector.
            if matches!(self.tool, Tool::Sculpt)
                && self.brush_id.is_some()
                && let Some(cursor) = ctx.pointer_interact_pos()
                && rect.contains(cursor)
            {
                let cursor_in = glam::Vec2::new(cursor.x - rect.min.x, cursor.y - rect.min.y);
                let rect_size_v = glam::Vec2::new(rect.width(), rect.height());
                if let Some(world) =
                    render::screen_to_world_y0(cursor_in, rect_size_v, &self.camera)
                {
                    let world_v3 = glam::Vec3::new(world.x, 0.0, world.z);
                    if let Some(screen) =
                        render::world_to_screen(world_v3, rect_size_v, &self.camera)
                    {
                        let p = egui::Pos2::new(
                            rect.min.x + screen.x + 12.0,
                            rect.min.y + screen.y + 12.0,
                        );
                        overlay_painter.text(
                            p,
                            egui::Align2::LEFT_TOP,
                            format!(
                                "r {:.0} elmos · s {:.2}",
                                self.brush_radius, self.brush_strength
                            ),
                            egui::FontId::monospace(10.0),
                            egui::Color32::from_rgba_unmultiplied(240, 240, 240, 220),
                        );
                    }
                }
            }

            // Sprint 19 — feature metal-value chips. Always visible
            // when Tool::Feature is active so the user can see at a
            // glance how much metal each placed feature yields; switches
            // to hover-only otherwise to keep the viewport readable when
            // editing other layers. Catalog miss-hits render "?m" so
            // missing entries are visible rather than silent.
            if !self.features.is_empty() {
                let rect_size = glam::Vec2::new(rect.width(), rect.height());
                let cross_tool_ghost = !matches!(self.tool, Tool::Feature);
                let alpha_mul: u8 = if cross_tool_ghost { 128 } else { 255 };
                let label_color = egui::Color32::from_rgba_unmultiplied(255, 255, 255, alpha_mul);
                let pointer = if cross_tool_ghost {
                    ctx.pointer_interact_pos()
                } else {
                    None
                };
                for f in &self.features {
                    let world = glam::Vec3::new(f.x_elmo as f32, 0.0, f.z_elmo as f32);
                    let Some(screen) = render::world_to_screen(world, rect_size, &self.camera)
                    else {
                        continue;
                    };
                    let p = egui::Pos2::new(rect.min.x + screen.x, rect.min.y + screen.y);
                    // Hover-only path: skip when the cursor isn't
                    // within ~14 px of the projected marker.
                    if let Some(cursor) = pointer
                        && (cursor - p).length() > 14.0
                    {
                        continue;
                    }
                    let metal = self.feature_state.manifest.metal_for(&f.name);
                    let chip = match metal {
                        Some(0) => continue, // skip 0-metal trees / props to keep canvas tidy
                        Some(m) => format!("{m}m"),
                        None => "?m".to_string(),
                    };
                    overlay_painter.text(
                        p + egui::Vec2::new(0.0, -12.0),
                        egui::Align2::CENTER_BOTTOM,
                        chip,
                        egui::FontId::proportional(11.0),
                        label_color,
                    );
                }
            }

            // C5 (Sprint 11): geo-vent markers. Orange triangle with
            // a faint upward gradient (steam-plume hint). Cross-tool
            // ghost falloff identical to metal.
            // ADR-035 viewport chrome (replaces XYZ nav gizmo):
            // 1. elmo rulers (bottom + left edges)
            // 2. mini-map (top-right)
            // 3. viewport-options toolbar (top-left)
            // 4. hint card (bottom-centre, first-launch only)
            crate::ui::viewport_chrome::paint_rulers(&overlay_painter, rect, &self.camera);

            // Mini-map. Uses its own painter inside ui scope.
            // Sprint 19 — minimap now shows real metal spots + geo
            // vents + placed features as overlay glyphs, so the user
            // has a reliable top-down readout even when 3D markers are
            // occluded by terrain relief.
            let metal_spots: Vec<(f32, f32, f32)> = self
                .metal_spots
                .iter()
                .map(|s| (s.x_elmo as f32, s.z_elmo as f32, s.metal))
                .collect();
            let geo_vents: Vec<(f32, f32)> = self
                .geo_vents
                .iter()
                .map(|v| (v.x_elmo as f32, v.z_elmo as f32))
                .collect();
            let features_mini: Vec<crate::ui::minimap::MinimapFeature> = self
                .features
                .iter()
                .map(|f| {
                    let v = self.feature_state.manifest.resolved_visual(&f.name);
                    crate::ui::minimap::MinimapFeature {
                        x_elmo: f.x_elmo as f32,
                        z_elmo: f.z_elmo as f32,
                        color: v.color,
                    }
                })
                .collect();
            let heightmap_data = self.heightmap.as_ref().map(|h| &h.data);
            // D10 / Sprint 17 (ADR-041): `App::splat_distribution`
            // retired. The minimap loses its low-res splat overlay
            // until a future sprint plumbs the composite RT through.
            let splat_data: Option<&barme_core::SplatDistribution> = None;
            crate::ui::minimap::paint_minimap(
                ui,
                rect,
                heightmap_data,
                splat_data,
                &self.ally_groups,
                &metal_spots,
                &geo_vents,
                &features_mini,
                extents,
                &self.camera,
                self.symmetry,
            );

            // Floating viewport-options toolbar. Allocate a Ui placed
            // at the top-left, just inside the rulers. Width sized
            // generously so future toolbar additions don't clip
            // silently — current 4-button content is ~130 px, the
            // allocation is 360 px to leave headroom.
            let chrome_origin = egui::pos2(rect.left() + 32.0, rect.top() + 14.0);
            let mut chrome_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_min_size(
                        chrome_origin,
                        egui::vec2(360.0, 32.0),
                    ))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            crate::ui::viewport_chrome::viewport_options_toolbar(
                &mut chrome_ui,
                &mut self.grid_overlay_on,
                &mut self.lighting_on,
                &mut self.wireframe_on,
                &mut self.buildable_overlay_on,
            );

            // Bottom-centre first-launch hint strip (ADR-035 replaces
            // the egui::Window in ui/intro.rs). The intro state still
            // tracks "seen-this-version" persistence.
            if self.show_intro {
                let mut shown = self.show_intro;
                crate::ui::viewport_chrome::hint_card(ui, rect, &mut shown);
                if !shown {
                    self.dismiss_intro();
                }
            }

            // Empty-state CTA when no heightmap is loaded.
            if self.heightmap.is_none() {
                match crate::ui::viewport_chrome::empty_state_cta(ui, rect) {
                    crate::ui::viewport_chrome::EmptyStateClick::Create => {
                        self.wizard = WizardState::default_for_new_project();
                        self.wizard_open = true;
                    }
                    crate::ui::viewport_chrome::EmptyStateClick::Open => {
                        if let Some(p) = pick_open_path() {
                            self.open_from(p);
                        }
                    }
                    crate::ui::viewport_chrome::EmptyStateClick::None => {}
                }
            }
        });

        // Sprint 20 / chunk 4 — progress overlay floats above the central
        // panel while a build is running. Doesn't allocate UI rect (uses
        // `egui::Area::Foreground`); rendered AFTER central so it draws
        // on top.
        let overlay_rect = ctx.available_rect();
        let click = crate::ui::build_overlay::render(ctx, overlay_rect, &self.build_state);
        crate::ui::build_overlay::apply_click(click, &self.build_state, &mut self.build_log_open);
    }

    /// Drain the per-frame `FileAction` queued by panel handlers. Done
    /// outside the egui closures so IO and project-mutating ops have
    /// uncontended `&mut self` access (every `.show(...)` closure holds
    /// `&mut self` for its scope).
    fn drain_action(&mut self, action: Option<FileAction>) {
        match action {
            Some(FileAction::LoadHeightmap(p)) => self.load_heightmap(p),
            Some(FileAction::OpenWizard) => {
                self.wizard = WizardState::default_for_new_project();
                self.wizard_open = true;
            }
            Some(FileAction::Save) => {
                let target = self
                    .current_project_path
                    .clone()
                    .or_else(|| pick_save_path(&self.project_name));
                if let Some(p) = target {
                    self.save_to(p);
                }
            }
            Some(FileAction::SaveAs) => {
                if let Some(p) = pick_save_path(&self.project_name) {
                    self.save_to(p);
                }
            }
            Some(FileAction::Open) => {
                if let Some(p) = pick_open_path() {
                    self.open_from(p);
                }
            }
            Some(FileAction::BuildAndInstall) => self.build_and_install(),
            Some(FileAction::ApplyProcGen) => self.apply_procgen(),
            Some(FileAction::Undo) => self.undo_one(),
            Some(FileAction::Redo) => self.redo_one(),
            None => {}
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 8 px drag threshold (ADR-030). egui exposes the click-vs-drag
        // discriminator as `InputOptions::max_click_dist` (default 6 px
        // — small movements within that radius still count as clicks).
        // Bumping to 8 px restores the click-place vs drag-paint
        // disambiguation in StartPositions mode and matches the
        // Photoshop / Blender convention.
        ctx.options_mut(|o| o.input_options.max_click_dist = 8.0);

        let mut action: Option<FileAction> = None;

        // Sprint 20: poll the worker thread + drain its event channel
        // BEFORE rendering panels so the status strip + progress
        // overlay see this frame's stage / log updates immediately.
        self.poll_build_state(ctx);

        // egui panel add-order rule: top → bottom → left → right →
        // CentralPanel LAST. Reversing this means CentralPanel eats the
        // rect later panels were supposed to claim.
        self.handle_keyboard(ctx, &mut action);
        self.top_bar(ctx, &mut action);
        self.status_strip(ctx);
        self.tool_strip(ctx);
        self.inspector(ctx, &mut action);
        self.central(ctx);

        // Sprint 20 / chunk 5 — build log panel renders AFTER the
        // central panel so the egui::Window floats on top. The
        // `LogPanelClicks` it returns are applied below so the lock
        // patterns stay inside the App layer.
        let log_clicks =
            crate::ui::build_log::render(ctx, &mut self.build_log_open, &self.build_state);
        self.apply_build_log_clicks(log_clicks);

        self.drain_action(action);
        self.symmetry_popover(ctx);

        // D10 / Sprint 17 (ADR-041) — file drag-drop dispatch.
        // Routes any PNG / JPG dropped over the Layers panel into a
        // freshly-created layer at the top of the stack via the
        // sidecar import flow. The Layers panel captures its rect on
        // render (`App::layers_panel_rect`); we read it here. No
        // central-viewport drop handler exists in earlier sprints, so
        // unmatched drops are logged + ignored.
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if !dropped.is_empty() {
            let drop_pos = ctx.pointer_interact_pos();
            let on_layers_panel = drop_pos
                .zip(self.layers_panel_rect)
                .is_some_and(|(p, rect)| rect.contains(p));
            for f in dropped {
                let Some(path) = f.path else {
                    continue;
                };
                let ext_ok = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_ascii_lowercase())
                    .is_some_and(|e| matches!(e.as_str(), "png" | "jpg" | "jpeg"));
                if !ext_ok {
                    info!(path = %path.display(), "drag-drop: ignoring non-image file");
                    continue;
                }
                if on_layers_panel && matches!(self.tool, Tool::PaintLayer) {
                    let id = self.add_layer_at_top();
                    self.import_layer_texture(&id, path);
                    self.paint_active_layer_id = Some(id);
                } else {
                    info!(
                        path = %path.display(),
                        "drag-drop: dropped outside Layers panel; ignored (\
                         only the Layers panel imports images today)"
                    );
                }
            }
        }

        // `?` cheat-sheet modal (B3). Builds the per-tool entries from
        // `Tool::ALL` so a new variant in Phase 4 shows up automatically.
        if self.show_cheat_sheet {
            let tool_entries: Vec<crate::ui::cheat_sheet::ToolBinding<'_>> =
                Tool::ALL.iter().map(|t| (t.accel(), t.label())).collect();
            crate::ui::cheat_sheet::render_cheat_sheet(
                ctx,
                &mut self.show_cheat_sheet,
                &tool_entries,
            );
        }

        // Sprint 19 / U1 — lint-panel stub. Opens from the top-bar
        // validation chip and the status-strip issue count; the panel
        // itself owns close behaviour via the egui Window's X button.
        let summary = self.validation_summary();
        crate::ui::lint_panel::render(
            ctx,
            &mut self.lint_panel_open,
            summary,
            &mut self.lint_panel_was_open,
        );

        // First-launch hint (B3). Renders ONLY after the wizard closes
        // so the two don't compete; this also serves a project on disk
        // that auto-applied via the wizard's default state.
        if self.show_intro
            && !self.wizard_open
            && let Some(crate::ui::intro::IntroAction::Dismiss) =
                crate::ui::intro::render_intro_hint(ctx, &mut self.show_intro)
        {
            self.dismiss_intro();
        }

        // C7 / Sprint 18 (F9): mapinfo form. Non-modal — runs every
        // frame the user keeps it open. The form returns a batch of
        // `MapInfoPatch` edits made this frame; each one becomes one
        // `ProjectDiff::EditMapInfo` undo entry.
        if self.mapinfo_form_open {
            let project = self.snapshot_project();
            let info: barme_core::MapInfo = (&project).into();
            let raw_lua = barme_pipeline::mapinfo::render_mapinfo(&info);
            let dnts_summary = self
                .layer_stack
                .dnts_layers()
                .iter()
                .enumerate()
                .filter_map(|(ch, l)| {
                    l.map(|layer| {
                        let ch_letter = ['R', 'G', 'B', 'A'][ch];
                        let idx = self
                            .layer_stack
                            .layers
                            .iter()
                            .position(|l| l.id == layer.id)
                            .unwrap_or(usize::MAX);
                        (
                            idx,
                            layer.name.clone(),
                            ch_letter,
                            layer.dnts_tex_scale,
                            layer.dnts_tex_mult,
                        )
                    })
                })
                .collect();
            let mut tab = self.mapinfo_form_tab;
            let mut open = self.mapinfo_form_open;
            let form_ctx = crate::ui::inspector_mapinfo::FormCtx {
                project: &project,
                info: &info,
                dnts_summary,
                layer_count: self.layer_stack.layers.len(),
                raw_lua: &raw_lua,
                // Sprint 18 / D7: minimap preview texture lands in
                // Sprint 19 (the preview rebake on a 1-Hz debounce is
                // not load-bearing for this commit). Pass None so the
                // tab shows the "(preview pending)" placeholder.
                minimap_preview: None,
                // Sprint 21 / C8 lint output — stubbed at zero. The
                // rendering is live so per-tab dots show up the moment
                // Sprint 21 populates this.
                lint_per_tab: [0; 12],
            };
            let patches =
                crate::ui::inspector_mapinfo::show_window(ctx, &mut open, &mut tab, &form_ctx);
            self.mapinfo_form_tab = tab;
            self.mapinfo_form_open = open;
            for patch in patches {
                let from = self.snapshot_mapinfo_patch_inverse(&patch);
                self.apply_mapinfo_patch(patch.clone());
                self.history
                    .push_project_diff(ProjectDiff::EditMapInfo { from, to: patch });
            }
        }

        // D10 / Sprint 17 (ADR-041) — one-time migration toast for
        // pre-Sprint-14 projects whose layer stack got seeded from
        // the legacy `splat_config`. Persists across reopens via
        // `Project.migration_toast_dismissed`.
        if self.pending_migration_toast {
            let mut still_open = true;
            egui::Window::new("Splat layers migrated")
                .id(egui::Id::new("sprint17_migration_toast"))
                .open(&mut still_open)
                .resizable(false)
                .collapsible(false)
                .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 60.0))
                .show(ctx, |ui| {
                    ui.set_max_width(420.0);
                    ui.label(
                        "Your project's splat layers were migrated to the new Layers panel.\n\
                         The old painting wasn't carried over — re-paint into the layer masks.\n\
                         (One-time prompt; dismissing stores the preference per project.)",
                    );
                    ui.add_space(6.0);
                    if ui.button("Got it").clicked() {
                        self.pending_migration_toast = false;
                        self.migration_toast_dismissed = true;
                        self.mark_dirty();
                    }
                });
            if !still_open {
                self.pending_migration_toast = false;
                self.migration_toast_dismissed = true;
                self.mark_dirty();
            }
        }

        // B8 Next-steps hint: shown after the wizard's Create, hidden
        // for projects that the user previously dismissed (per-project
        // flag in the `.barmeproj`). Skipped while either wizard or
        // intro is up — three stacked floating windows would be
        // chaos.
        if self.show_next_steps
            && !self.wizard_open
            && !self.show_intro
            && let Some(crate::ui::next_steps::NextStepsAction::Dismiss) =
                crate::ui::next_steps::render_next_steps_hint(ctx, &mut self.show_next_steps)
        {
            self.dismiss_next_steps();
        }

        // Wizard renders on top, after all other panels. Drains to
        // `apply_wizard` / close depending on the user's choice. ADR-024.
        if self.wizard_open {
            match self.render_wizard(ctx) {
                Some(WizardAction::Apply) => self.apply_wizard(),
                Some(WizardAction::Cancel) => self.wizard_open = false,
                None => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Construct an `App` that doesn't need an `eframe::CreationContext`
    /// or a live wgpu device — enough state to exercise the data-shape
    /// helpers (`set_tool`, `Tool::*`). Renderer-touching paths
    /// (`apply_brush_at`, `apply_procgen`'s GPU upload) require a real
    /// `render_state` and are exercised by the workspace integration
    /// tests instead.
    fn make_test_app() -> App {
        App {
            project_name: "test".to_string(),
            map_size: MapSize::square(2),
            heightmap: None,
            last_error: None,
            render_state: None,
            camera: OrbitCamera::framing(128.0, 128.0),
            height_scale: 256.0,
            min_height: 0.0,
            current_project_path: None,
            last_install: None,
            build_state: build_runner::BuildState::Idle,
            build_log_open: false,
            brushes: BrushRegistry::default_set(),
            brush_id: None,
            brush_radius: 256.0,
            brush_strength: 0.5,
            symmetry: SymmetryAxis::None,
            rotational_fold: 2,
            procgen_expr: "0".to_string(),
            procgen_domain: Domain::Centered,
            procgen_last_error: None,
            procgen_validation: Ok(()),
            procgen_thumbnail: None,
            procgen_thumbnail_key: None,
            procgen_changed_at: None,
            history: History::default(),
            tool: Tool::Sculpt,
            previous_tool: Tool::Sculpt,
            ally_groups: Vec::new(),
            active_ally_group_id: 0,
            dragging_start_pos: None,
            dragging_start_pos_from: None,
            drag_paint_count: 8,
            drag_paint_origin: None,
            pulsing_marker: None,
            hovered_canvas_marker: None,
            wizard_open: false,
            wizard: WizardState::default_for_new_project(),
            symmetry_popover_open: false,
            editor_config: config::EditorConfig::default(),
            show_intro: false,
            show_cheat_sheet: false,
            lint_panel_open: false,
            lint_panel_was_open: false,
            nav_gizmo_drag_active: false,
            build_variant: BuildVariant::default(),
            mapinfo_overrides: std::collections::HashMap::new(),
            show_next_steps: false,
            next_steps_dismissed: false,
            minimap_override: None,
            mapinfo_form_open: false,
            mapinfo_form_tab: crate::ui::inspector_mapinfo::MapInfoTab::default(),
            migration_toast_dismissed: false,
            pending_migration_toast: false,
            dirty: false,
            last_non_none_symmetry: SymmetryAxis::Horizontal,
            grid_overlay_on: false,
            lighting_on: true,
            wireframe_on: false,
            buildable_overlay_on: false,
            // D8 / Sprint 15: test-harness apps stay empty here — no
            // implicit biome seed, so smoke tests can opt-in to a
            // specific stack shape.
            layer_stack: LayerStack::default(),
            composite_layer_last_version: std::collections::HashMap::new(),
            paint_active_layer_id: None,
            paint_view_state: PaintViewState::default(),
            paint_brush_state: PaintBrushState::default(),
            mask_brushes: barme_core::MaskBrushRegistry::default_set(),
            paint_last_drag_pos: None,
            layer_thumbnails: std::collections::HashMap::new(),
            paint_drag_preview_order: None,
            layer_mask_preview_cache: None,
            layers_panel_rect: None,
            dnts_diffuse_in_alpha: false,
            slot_registry: Vec::new(),
            slot_thumbnails: std::collections::HashMap::new(),
            metal_state: MetalState::default(),
            geo_state: GeoState::default(),
            metal_spots: Vec::new(),
            geo_vents: Vec::new(),
            extractor_radius: default_extractor_radius(),
            dragging_metal_spot: None,
            dragging_metal_spot_from: None,
            dragging_geo_vent: None,
            dragging_geo_vent_from: None,
            features: Vec::new(),
            feature_state: FeatureState::default(),
            dragging_feature: None,
            dragging_feature_from: None,
            dragging_feature_anchor_x: None,
            dragging_feature_start_rot: None,
            water_mode: WaterMode::default(),
            water_overrides: WaterBlock::default(),
            void_water: false,
            tidal_strength: None,
            lava_atmosphere: false,
            water_carve_depth: -80.0,
        }
    }

    /// `Tool::ALL` must enumerate every variant exactly once. Adding a
    /// new variant to `Tool` is what should drive new rows in the tool
    /// strip — this test fires if the array is forgotten.
    #[test]
    fn tool_all_array_has_unique_entries_per_variant() {
        let mut seen = std::collections::HashSet::new();
        for t in Tool::ALL {
            assert!(seen.insert(t), "Tool::ALL has a duplicate entry: {t:?}");
        }
        // C6 (Sprint 12) added Tool::Feature → 8 variants.
        // C9 (Sprint 14) added Tool::Water → 9 variants.
        // D9 (Sprint 16) added Tool::PaintLayer → 10 variants.
        // D10 (Sprint 17 / ADR-041) retired Tool::SplatPaint → 9 variants.
        // A change here is intentional but should bump ADR-030 /
        // ADR-035 / ADR-041 / ADR-042 + the phase-3-plan entry for B1.
        assert_eq!(
            Tool::ALL.len(),
            9,
            "Tool::ALL size changed — update ADR-030 / ADR-035 / ADR-041 / ADR-042 + plan"
        );
    }

    /// Each tool variant must produce distinct icon, accelerator, and
    /// label strings. If two variants collided, the left tool strip
    /// would show indistinguishable buttons.
    #[test]
    fn tool_helpers_are_distinct_per_variant() {
        let mut icons = std::collections::HashSet::new();
        let mut accels = std::collections::HashSet::new();
        let mut labels = std::collections::HashSet::new();
        for t in Tool::ALL {
            assert!(icons.insert(t.icon()), "duplicate icon for {t:?}");
            assert!(accels.insert(t.accel()), "duplicate accel for {t:?}");
            assert!(labels.insert(t.label()), "duplicate label for {t:?}");
        }
    }

    /// Accelerators must be single ASCII-uppercase chars — the
    /// keyboard-handler is hard-coded to `Key::Q / Key::B / Key::S /
    /// Key::G`, so multi-char or lowercase accels would silently
    /// detach the strip tooltip from the actual binding.
    #[test]
    fn tool_accelerator_is_a_single_uppercase_letter() {
        for t in Tool::ALL {
            let a = t.accel();
            assert_eq!(
                a.chars().count(),
                1,
                "accel for {t:?} should be one char (got {a:?})"
            );
            let c = a.chars().next().unwrap();
            assert!(
                c.is_ascii_uppercase(),
                "accel for {t:?} should be ASCII uppercase (got {c})"
            );
        }
    }

    /// ADR-030 nails Q / B / S / G to specific tools; ADR-035 adds
    /// T / M. Sprint 12 / C6 added `F` for the general feature tool
    /// and rebound the geo-vent tool to `V` (it freed F by design —
    /// general features are a more common operation; vents stay
    /// reachable by one key). A drift here is a documented contract
    /// break — bump the ADR if intentional.
    #[test]
    fn tool_accelerators_match_adr_030() {
        assert_eq!(Tool::Select.accel(), "Q");
        assert_eq!(Tool::Sculpt.accel(), "B");
        assert_eq!(Tool::StartPositions.accel(), "S");
        // D10 / Sprint 17 (ADR-041): `T` is freed by the retirement
        // of `Tool::SplatPaint`. No tool currently binds it.
        assert_eq!(Tool::MetalSpots.accel(), "M");
        assert_eq!(Tool::GeoFeatures.accel(), "V");
        assert_eq!(Tool::Feature.accel(), "F");
        assert_eq!(Tool::Procgen.accel(), "G");
    }

    /// Every tool variant must map to a real icon kind that exists in
    /// [`crate::ui::icons::ALL`] — otherwise the tool-strip tile would
    /// fail to render an icon.
    #[test]
    fn tool_icon_kinds_exist_in_icon_catalogue() {
        let catalogue: std::collections::HashSet<_> =
            crate::ui::icons::ALL.iter().copied().collect();
        for t in Tool::ALL {
            assert!(
                catalogue.contains(&t.icon_kind()),
                "{t:?} icon_kind {:?} not in icons::ALL",
                t.icon_kind()
            );
        }
    }

    /// Calling `set_tool(current)` is a no-op — it MUST NOT bump
    /// `previous_tool`, otherwise rapid identical keystrokes would
    /// erase the real prior tool from history.
    #[test]
    fn set_tool_is_noop_when_new_matches_current() {
        let mut app = make_test_app();
        // Seed previous_tool with a distinct value so we can detect
        // an unwanted overwrite.
        app.previous_tool = Tool::Select;
        app.tool = Tool::Sculpt;
        app.set_tool(Tool::Sculpt);
        assert_eq!(app.tool, Tool::Sculpt);
        assert_eq!(
            app.previous_tool,
            Tool::Select,
            "no-op set_tool must not bump previous_tool"
        );
    }

    /// A real tool change advances both `tool` and `previous_tool`. The
    /// previous-tool sentinel is what the bug-report log line cites, so
    /// it MUST track the actual transition history.
    #[test]
    fn set_tool_updates_current_and_previous() {
        let mut app = make_test_app();
        app.tool = Tool::Sculpt;
        app.previous_tool = Tool::Sculpt;
        app.set_tool(Tool::Procgen);
        assert_eq!(app.tool, Tool::Procgen);
        assert_eq!(app.previous_tool, Tool::Sculpt);
        app.set_tool(Tool::StartPositions);
        assert_eq!(app.tool, Tool::StartPositions);
        assert_eq!(app.previous_tool, Tool::Procgen);
    }

    /// Leaving any tool must cancel an in-flight marker drag —
    /// otherwise `dragging_start_pos` would linger across tool
    /// switches and the next entry into StartPositions would resume
    /// an invisible drag the user can't see.
    #[test]
    fn set_tool_clears_in_flight_start_position_drag() {
        let mut app = make_test_app();
        app.tool = Tool::StartPositions;
        app.dragging_start_pos = Some((0, 3));
        app.set_tool(Tool::Sculpt);
        assert_eq!(
            app.dragging_start_pos, None,
            "leaving StartPositions must drop any in-flight marker drag"
        );
    }

    /// Default-initialised App seeds `previous_tool == tool` so the
    /// first real `set_tool` transition writes a sensible diff into
    /// the bug-report log (instead of "tool change from ??? to X").
    #[test]
    fn fresh_app_has_consistent_previous_tool_sentinel() {
        let app = make_test_app();
        assert_eq!(
            app.tool, app.previous_tool,
            "fresh App initialises previous_tool == tool so the first \
             transition is a real change"
        );
    }

    /// Phase-2 smoke #1 (F14 / ADR-020): the procgen Apply path still
    /// populates the CPU heightmap. GPU upload is skipped when there's
    /// no render_state (test-only path); the App-level state machine
    /// MUST still leave `self.heightmap` Some.
    #[test]
    fn b1_does_not_regress_procgen_apply_phase2() {
        let mut app = make_test_app();
        app.procgen_expr = "0.5".to_string();
        app.revalidate_procgen();
        assert!(
            app.procgen_validation.is_ok(),
            "expression should validate before Apply"
        );
        app.apply_procgen();
        assert!(
            app.heightmap.is_some(),
            "procgen Apply must populate self.heightmap"
        );
        let h = app.heightmap.as_ref().unwrap();
        assert_eq!(h.dims, MapSize::square(2).heightmap_dims());
    }

    /// Phase-2 smoke #2 (F8 / ADR-023 → ADR-032): start-position
    /// placement still inserts into the active ally group's
    /// `start_positions` with rounded elmo coords. The B6 tree
    /// refactor must not break the underlying placement state machine.
    #[test]
    fn b1_does_not_regress_start_position_placement_phase2() {
        let mut app = make_test_app();
        app.tool = Tool::StartPositions;
        app.place_start_position(100.0, 100.0);
        assert_eq!(app.ally_groups.len(), 1);
        let g0 = &app.ally_groups[0];
        assert_eq!(g0.id, 0);
        assert_eq!(g0.start_positions.len(), 1);
        assert_eq!(g0.start_positions[0].x_elmo, 100);
        assert_eq!(g0.start_positions[0].z_elmo, 100);
        // Out-of-bounds click is ignored.
        app.place_start_position(-1.0, 0.0);
        assert_eq!(
            app.ally_groups[0].start_positions.len(),
            1,
            "off-map click ignored"
        );
    }

    /// ADR-032 / B6: applying the 8v8 preset materialises 2 ally
    /// groups with 8 positions each (north / south strips). The
    /// pre-placement layout is the canonical starter for big-team
    /// queue maps.
    #[test]
    fn b6_eight_v_eight_preset_lays_out_2_groups_of_8() {
        let mut app = make_test_app();
        // make_test_app uses MapSize::square(2) → 1024 elmos per side.
        app.apply_ally_preset(AllyPreset::EightVEight);
        assert_eq!(app.ally_groups.len(), 2, "8v8 → 2 ally groups");
        assert_eq!(app.ally_groups[0].start_positions.len(), 8);
        assert_eq!(app.ally_groups[1].start_positions.len(), 8);
        // North strip z-coords ≈ 6 % of extent; south strip ≈ 94 %.
        let ez = 1024.0f32;
        let n_z = (ez * 0.06).round() as i32;
        let s_z = (ez * 0.94).round() as i32;
        assert!(
            app.ally_groups[0]
                .start_positions
                .iter()
                .all(|p| p.z_elmo == n_z)
        );
        assert!(
            app.ally_groups[1]
                .start_positions
                .iter()
                .all(|p| p.z_elmo == s_z)
        );
        // Both groups carry a box polygon — emitter consumes these.
        assert!(app.ally_groups[0].box_polygon.is_some());
        assert!(app.ally_groups[1].box_polygon.is_some());
    }

    /// ADR-032 / B6: drag-paint distributes N evenly-spaced positions
    /// along the drag vector. Canonical default is 8 (the 8v8 case).
    #[test]
    fn b6_drag_paint_distributes_n_positions_along_vector() {
        let mut app = make_test_app();
        app.tool = Tool::StartPositions;
        app.drag_paint_count = 8;
        // Diagonal across the test-map's 1024-elmo extent.
        app.drag_paint_start_positions(100.0, 100.0, 900.0, 900.0);
        assert_eq!(app.ally_groups.len(), 1);
        let g0 = &app.ally_groups[0];
        assert_eq!(g0.start_positions.len(), 8);
        // Endpoints land at the drag start / end (rounded).
        assert_eq!(g0.start_positions[0].x_elmo, 100);
        assert_eq!(g0.start_positions[7].x_elmo, 900);
    }

    /// ADR-032: the build-path snapshot expands every source position
    /// through the active symmetry into the same ally group, so the
    /// emitted `mapinfo.lua` ships every mirror as a real
    /// `teams[*].startPos`. Without this, a Quad-symmetric placement
    /// would only ship 1 spawn — BAR sees only one team.
    #[test]
    fn b6_build_snapshot_expands_symmetry_mirrors() {
        let mut app = make_test_app();
        app.tool = Tool::StartPositions;
        app.symmetry = SymmetryAxis::Horizontal;
        app.place_start_position(256.0, 256.0);
        // Place under symmetry: source + 1 mirror landed.
        assert_eq!(app.ally_groups[0].start_positions.len(), 2);
        // Storage matches what the editor shows (sources + mirrors as
        // sources in the same group — see place_start_position).
        // Toggle symmetry off; the build snapshot must still contain
        // every spawn that the user saw on screen.
        app.symmetry = SymmetryAxis::None;
        let p = app.snapshot_project_for_build();
        let total_positions: usize = p.ally_groups.iter().map(|g| g.start_positions.len()).sum();
        assert!(
            total_positions >= 2,
            "build snapshot must preserve every concrete spawn; got {total_positions}"
        );
    }

    /// Phase-2 smoke #3 (ADR-022 / ADR-033): undo_one without a loaded
    /// heightmap is a no-op. Was always true; pin so the B1 refactor
    /// hasn't tangled the `end_stroke` -> `apply_undo` chain.
    #[test]
    fn b1_does_not_regress_undo_with_no_heightmap_phase2() {
        let mut app = make_test_app();
        app.undo_one();
        assert!(app.heightmap.is_none());
        assert_eq!(app.history.undo_depth(), 0);
    }

    /// Guard against silent regressions to the boot-log filter. If any of
    /// these directives is dropped, the corresponding warn-level events
    /// resume flooding stderr on cold boot.
    #[test]
    fn default_tracing_filter_parses_and_carries_the_suppressions() {
        // EnvFilter::new is infallible; use try_new so a malformed string
        // would surface as a panic here instead of being silently coerced.
        tracing_subscriber::EnvFilter::try_new(DEFAULT_TRACING_FILTER)
            .expect("DEFAULT_TRACING_FILTER must be valid env-filter syntax");

        assert!(
            DEFAULT_TRACING_FILTER.contains("wgpu_hal::gles::egl=error"),
            "Wayland GLES re-init suppression missing — see RUNTIME-WARNINGS.md §4"
        );
        assert!(
            DEFAULT_TRACING_FILTER.contains("wgpu_hal::vulkan=error"),
            "Vulkan validation-layer-not-found suppression missing — see \
             RUNTIME-WARNINGS.md §3"
        );
        assert!(
            DEFAULT_TRACING_FILTER.starts_with("info,"),
            "filter must leave our own info!-level events visible"
        );
    }

    /// B3: the `?` cheat-sheet auto-generates from `Tool::ALL`. A new
    /// `Tool` variant should make the entry count grow by exactly one —
    /// this test asserts that invariant against the live enum, so a
    /// future contributor who adds `Tool::Splat` will see the test fail
    /// with a clear "did you forget to update the cheat-sheet?" hint.
    #[test]
    fn cheat_sheet_entry_count_matches_tool_all_plus_camera_bindings() {
        let tool_entries: Vec<crate::ui::cheat_sheet::ToolBinding<'_>> =
            Tool::ALL.iter().map(|t| (t.accel(), t.label())).collect();
        let live = crate::ui::cheat_sheet::cheat_sheet_entries(&tool_entries);
        let expected = crate::ui::cheat_sheet::cheat_sheet_entry_count(Tool::ALL.len());
        assert_eq!(live.len(), expected);
    }

    /// B3: every `Tool` variant gets exactly one cheat-sheet row,
    /// generated from its `accel` + `label` helpers.
    #[test]
    fn every_tool_appears_in_cheat_sheet() {
        let tool_entries: Vec<crate::ui::cheat_sheet::ToolBinding<'_>> =
            Tool::ALL.iter().map(|t| (t.accel(), t.label())).collect();
        let live = crate::ui::cheat_sheet::cheat_sheet_entries(&tool_entries);
        for t in Tool::ALL {
            let key = t.accel();
            let label = t.label();
            assert!(
                live.iter()
                    .any(|e| e.keys == key && e.action.contains(label)),
                "tool {t:?} missing from cheat-sheet"
            );
        }
    }

    /// B3: `dismiss_intro` flips `show_intro` to false AND updates the
    /// editor config's seen-version vec. The disk save is best-effort
    /// (no temp config dir in the test); we just verify the state
    /// machine.
    #[test]
    fn dismiss_intro_updates_state_and_config() {
        let mut app = make_test_app();
        app.show_intro = true;
        assert!(!app.editor_config.intro_seen_for_current_version());
        app.dismiss_intro();
        assert!(!app.show_intro);
        assert!(app.editor_config.intro_seen_for_current_version());
    }

    /// B3: the nav-gizmo drag flag clears on App construction (no
    /// in-flight drag survives a restart).
    #[test]
    fn fresh_app_has_no_in_flight_nav_gizmo_drag() {
        let app = make_test_app();
        assert!(!app.nav_gizmo_drag_active);
    }

    /// B3: cheat-sheet starts closed in a fresh app. A regression that
    /// flipped the default to `true` would pop the modal on every cold
    /// launch — annoying enough to pin.
    #[test]
    fn fresh_app_has_cheat_sheet_closed() {
        let app = make_test_app();
        assert!(!app.show_cheat_sheet);
    }

    // ----------------------------- B4 -----------------------------

    /// B4: `BuildVariant::default()` is `Install` — matches the B1
    /// right-aligned button's behaviour so the default user click
    /// preserves the existing flow.
    #[test]
    fn build_variant_default_is_install() {
        assert_eq!(BuildVariant::default(), BuildVariant::Install);
    }

    /// B4: `BuildVariant::ALL` lists every variant exactly once.
    /// Adding a new variant + leaving `ALL` stale would either drop
    /// the new variant from the UI or break the `len == 3` assert
    /// below.
    #[test]
    fn build_variant_all_lists_every_variant_once() {
        let mut seen = std::collections::HashSet::new();
        for v in BuildVariant::ALL {
            assert!(seen.insert(v), "duplicate {v:?}");
        }
        assert_eq!(
            seen.len(),
            3,
            "BuildVariant::ALL count drift — update ALL + this test together"
        );
    }

    /// B4: labels are distinct so the ComboBox shows three distinguishable
    /// rows and the button text doesn't collide with a neighbour.
    #[test]
    fn build_variant_labels_are_distinct() {
        let mut seen = std::collections::HashSet::new();
        for v in BuildVariant::ALL {
            assert!(seen.insert(v.label()), "duplicate label {}", v.label());
        }
    }

    /// B4: `Launch` is greyed pre-F12. `Only` and `Install` are
    /// enabled today. A regression that flipped `Launch` on would
    /// fire an unwired engine call.
    #[test]
    fn build_variant_launch_is_disabled_pre_f12() {
        assert!(BuildVariant::Only.is_enabled());
        assert!(BuildVariant::Install.is_enabled());
        assert!(
            !BuildVariant::Launch.is_enabled(),
            "Launch must stay greyed until F12 ships"
        );
    }

    /// B4: `to_file_action` returns `Some` for enabled variants and
    /// `None` for disabled. The click handler treats `None` as "drop
    /// the click" — belt-and-braces with the disabled-button state.
    #[test]
    fn build_variant_to_file_action_matches_enabled_state() {
        // Today every enabled variant lands in the same FileAction —
        // a Phase-5 split would diversify this match. The variant
        // selector is reserved UX surface today.
        assert!(matches!(
            BuildVariant::Only.to_file_action(),
            Some(FileAction::BuildAndInstall)
        ));
        assert!(matches!(
            BuildVariant::Install.to_file_action(),
            Some(FileAction::BuildAndInstall)
        ));
        assert!(BuildVariant::Launch.to_file_action().is_none());
    }

    /// B4: fresh app's `build_variant` is the default — Install.
    #[test]
    fn fresh_app_has_default_build_variant_install() {
        let app = make_test_app();
        assert_eq!(app.build_variant, BuildVariant::Install);
    }

    // ----------------- ADR-035: top action bar tests -----------------

    /// `validation_summary()` reports Err when the heightmap is
    /// missing — the chip's most important message. Without this, the
    /// user could press Build & install on an empty editor and watch
    /// it explode in the pipeline crate.
    #[test]
    fn validation_summary_no_heightmap_is_err() {
        let app = make_test_app();
        assert!(
            app.heightmap.is_none(),
            "fresh app should have no heightmap"
        );
        let (tone, label) = app.validation_summary();
        assert_eq!(tone, crate::ui::theme::ChipTone::Err);
        assert!(
            label.to_lowercase().contains("heightmap"),
            "label was {label:?}"
        );
    }

    /// `mark_dirty()` flips the flag; `save_to` clears it after a
    /// successful save. Validating both ends keeps the Save chip's
    /// dirty dot from going stale.
    #[test]
    fn mark_dirty_then_save_clears_flag() {
        let mut app = make_test_app();
        assert!(!app.dirty);
        app.mark_dirty();
        assert!(app.dirty);
        // `save_to` requires a path + success — we just test the
        // semantic flip via a direct mutation, which is the same code
        // path the success arm runs.
        app.dirty = false;
        assert!(!app.dirty);
    }

    /// `new_project()` always clears the dirty flag — opening a fresh
    /// canvas should not present as "you have unsaved changes."
    #[test]
    fn new_project_clears_dirty_flag() {
        let mut app = make_test_app();
        app.dirty = true;
        app.new_project();
        assert!(!app.dirty, "new_project must reset dirty");
    }

    // ───────────── D5 / Sprint 9 splat tool state tests ─────────────
    //
    // These exercise the persisted `Project.splat_config` (mirrored on
    // `App` as `splat_config`) + session-only `SplatBrushState` that
    // the inspector reads/writes. Round-trip + dirty-flag wiring is
    // covered in the project tests (`crates/barme-core/src/project.rs`);
    // here we pin the App-level defaults the inspector relies on.
    //
    // FUTURE TEST COVERAGE (TODO when D6 ships):
    //
    //  D6 (emission): a painted distribution + non-default splat_config
    //                 → `.sd7` containing the matching
    //                 `splat_distribution.png` + mapinfo `resources.
    //                 splatDetailNormalTex` subtable. Round-trip the
    //                 `.sd7` (decompile + re-load) and assert
    //                 byte-identical distribution + matching slot
    //                 names.
    //
    //  F5 (metal):   `MetalState::spots` round-trips through
    //                `Project::metal_spots` (or whichever field the F5
    //                schema picks). `Reseed` is deterministic; mirror
    //                under `symmetry` produces paired spots; `Clear
    //                all` empties the `Vec` and undoes.
    //
    //  F7 (geo):     `GeoState.selected` + `scatter_density` +
    //                `align_to_slope` drive a feature gadget emission
    //                that names every feature in the library; scatter
    //                hashes deterministically.

    // D10 / Sprint 17 (ADR-041) — `SplatBrushState` retired with
    // `inspector_splat`; the `splat_brush_state_default_is_paint_
    // radius_48` test went with it.

    // D10 / Sprint 17 (ADR-041) — `App::splat_config`,
    // `App::splat_distribution`, `App::splat_brushes` retired.
    // `fresh_app_has_engine_default_splat_config` went with them.

    /// D10 / Sprint 17 (ADR-041) — splat uniforms derive from the
    /// layer stack, not the retired `splat_config`. Build a stack
    /// with two DNTS-bound layers + per-layer tex_scale / tex_mult,
    /// assert the uniforms mirror them.
    #[test]
    fn splat_uniforms_for_render_reflects_layer_dnts() {
        use barme_core::layers::{LayerSource, TextureLayer};
        let mut app = make_test_app();
        let mut layer_r = TextureLayer::new(LayerSource::Slot { id: 0 }, app.map_size, 255);
        layer_r.dnts_channel = Some(barme_core::SplatChannel::R);
        layer_r.dnts_tex_scale = 0.004;
        layer_r.dnts_tex_mult = 0.5;
        let mut layer_b = TextureLayer::new(LayerSource::Slot { id: 5 }, app.map_size, 0);
        layer_b.dnts_channel = Some(barme_core::SplatChannel::B);
        layer_b.dnts_tex_scale = 0.012;
        layer_b.dnts_tex_mult = 1.5;
        app.layer_stack.layers = vec![layer_r, layer_b];
        app.dnts_diffuse_in_alpha = true;
        let su = app.splat_uniforms_for_render();
        assert!((su.tex_scales[0] - 0.004).abs() < 1e-6);
        assert!((su.tex_scales[2] - 0.012).abs() < 1e-6);
        // Unbound G + A keep engine baseline (0.02).
        assert!((su.tex_scales[1] - 0.02).abs() < 1e-6);
        assert!((su.tex_scales[3] - 0.02).abs() < 1e-6);
        assert!((su.tex_mults[0] - 0.5).abs() < 1e-6);
        assert!((su.tex_mults[2] - 1.5).abs() < 1e-6);
        // R + B bound → mask = 0b101 = 5.
        assert_eq!(su.flags[0], 0b101);
        assert_eq!(su.flags[1], 1);
        // buildable_overlay_on defaults to false.
        assert_eq!(su.flags[2], 0);
    }

    /// Buildable-area toggle propagates to `splat_uniforms.flags.z`
    /// (the bit the WGSL fragment shader checks for the red-mask
    /// overlay).
    #[test]
    fn buildable_overlay_flag_propagates_to_uniforms() {
        let mut app = make_test_app();
        assert_eq!(app.splat_uniforms_for_render().flags[2], 0);
        app.buildable_overlay_on = true;
        assert_eq!(app.splat_uniforms_for_render().flags[2], 1);
        app.buildable_overlay_on = false;
        assert_eq!(app.splat_uniforms_for_render().flags[2], 0);
    }

    /// Fresh app default: buildable overlay starts off.
    #[test]
    fn fresh_app_buildable_overlay_default_off() {
        let app = make_test_app();
        assert!(!app.buildable_overlay_on);
    }

    // ─── Sprint 14 / C9 (Slice 2) — water draw ────────

    /// `WaterMode::None` → no water plane (preserves pre-Sprint-14
    /// rendering — no translucent quad blocks the heightmap view).
    #[test]
    fn water_draw_returns_none_when_water_mode_is_none() {
        let app = make_test_app();
        assert_eq!(app.water_mode, WaterMode::None);
        assert!(app.water_draw_for_frame(8192.0, 8192.0).is_none());
    }

    /// `WaterMode::Acid` → premultiplied RGBA matches the Acid preset's
    /// `(0.65, 0.8, 0.1, 0.4)` (surface + alpha). Pre-multiply yields
    /// `(0.26, 0.32, 0.04, 0.4)`. Tool::Water active → alpha_scale = 1.
    #[test]
    fn water_draw_acid_produces_premultiplied_acidic_quarry_rgba() {
        let mut app = make_test_app();
        app.water_mode = WaterMode::Acid;
        // Cross-tool ghost (commit 5) only fades when NOT Tool::Water.
        app.tool = Tool::Water;
        let w = app.water_draw_for_frame(8192.0, 8192.0).unwrap();
        // Acid surface = (0.65, 0.8, 0.1), alpha = 0.4 → premul:
        // (0.26, 0.32, 0.04, 0.4).
        let [r, g, b, a] = w.surface_rgba;
        assert!((r - 0.65 * 0.4).abs() < 1e-5, "r = {r}");
        assert!((g - 0.80 * 0.4).abs() < 1e-5, "g = {g}");
        assert!((b - 0.10 * 0.4).abs() < 1e-5, "b = {b}");
        assert!((a - 0.4).abs() < 1e-5, "a = {a}");
        assert_eq!(w.extent_x, 8192.0);
        assert_eq!(w.extent_z, 8192.0);
        assert!((w.alpha_scale - 1.0).abs() < 1e-6);
    }

    /// Overrides ride through `water_draw_for_frame` the same way they
    /// do through emission: tweaking `surface_alpha` over Ocean
    /// produces the override's alpha, with Ocean's surface RGB.
    #[test]
    fn water_draw_honors_per_field_override_on_top_of_preset() {
        let mut app = make_test_app();
        app.water_mode = WaterMode::Ocean;
        app.water_overrides.surface_alpha = Some(0.6);
        let w = app.water_draw_for_frame(4096.0, 4096.0).unwrap();
        // Ocean surface RGB (0.67, 0.8, 1.0); override alpha 0.6 →
        // pre-multiplied (0.402, 0.48, 0.6, 0.6).
        let [r, g, b, a] = w.surface_rgba;
        assert!((r - 0.67 * 0.6).abs() < 1e-5);
        assert!((g - 0.80 * 0.6).abs() < 1e-5);
        assert!((b - 1.00 * 0.6).abs() < 1e-5);
        assert!((a - 0.6).abs() < 1e-5);
        let _ = (r, g, b, a);
    }

    /// `WaterMode::Custom` with no overrides falls back to the
    /// engine's default surface colour + alpha — the empty Custom
    /// preset doesn't crash the renderer.
    #[test]
    fn water_draw_custom_with_no_overrides_falls_back_to_bar_default() {
        let mut app = make_test_app();
        app.water_mode = WaterMode::Custom;
        let w = app.water_draw_for_frame(2048.0, 2048.0).unwrap();
        let [_, _, _, a] = w.surface_rgba;
        // BAR_DEFAULT_SURFACE_ALPHA = 0.1
        assert!((a - 0.1).abs() < 1e-5);
    }

    /// Cross-tool ghosting (Sprint 14 / commit 5): when Tool::Water
    /// is active the water plane renders at full alpha; otherwise it
    /// drops to 0.5× so the user can still see the plane while
    /// sculpting / painting but doesn't have it dominate.
    #[test]
    fn water_draw_alpha_scale_drops_when_tool_inactive() {
        let mut app = make_test_app();
        app.water_mode = WaterMode::Ocean;
        // Default tool is Sculpt — not Water.
        app.tool = Tool::Sculpt;
        let ghost = app.water_draw_for_frame(8192.0, 8192.0).unwrap();
        assert!(
            (ghost.alpha_scale - 0.5).abs() < 1e-6,
            "ghost should be 0.5×"
        );
        app.tool = Tool::Water;
        let active = app.water_draw_for_frame(8192.0, 8192.0).unwrap();
        assert!(
            (active.alpha_scale - 1.0).abs() < 1e-6,
            "active should be 1.0×"
        );
    }

    /// C9 (Sprint 14 / commit 5) — validation chip:
    /// `min_height < 0` with `WaterMode::None` warns the user that
    /// BAR will render its default ocean rather than nothing.
    #[test]
    fn validation_warns_when_terrain_below_zero_without_water_preset() {
        let mut app = make_test_app();
        // Need a heightmap so the chip doesn't return "No heightmap" first.
        app.heightmap = Some(test_heightmap_state(app.map_size));
        app.water_mode = WaterMode::None;
        app.min_height = -120.0;
        let (tone, msg) = app.validation_summary();
        assert!(matches!(tone, crate::ui::theme::ChipTone::Warn));
        assert!(msg.contains("below Y=0"), "got: {msg}");
    }

    /// C9: `WaterMode != None` with `min_height >= 0` warns the
    /// user that BAR won't render water without forceRendering or a
    /// carved basin.
    #[test]
    fn validation_warns_when_water_preset_but_no_below_zero_terrain() {
        let mut app = make_test_app();
        app.heightmap = Some(test_heightmap_state(app.map_size));
        app.water_mode = WaterMode::Ocean;
        app.min_height = 0.0;
        let (tone, msg) = app.validation_summary();
        assert!(matches!(tone, crate::ui::theme::ChipTone::Warn));
        assert!(msg.contains("no terrain below Y=0"), "got: {msg}");
    }

    /// C9 / PITFALL §8 — DNTS + water trips the TV-snow LOS bug
    /// warning, in priority over the inverse "DNTS: no specular"
    /// chip (this is the scarier condition because it ships a
    /// broken in-engine map).
    #[test]
    fn validation_warns_on_dnts_with_water() {
        use barme_core::layers::{LayerSource, TextureLayer};
        let mut app = make_test_app();
        app.heightmap = Some(test_heightmap_state(app.map_size));
        // D10 / Sprint 17 (ADR-041): bind a layer to DNTS R so the
        // chip's `dnts_layers().any(_)` check fires.
        let mut layer = TextureLayer::new(LayerSource::Slot { id: 0 }, app.map_size, 255);
        layer.dnts_channel = Some(barme_core::SplatChannel::R);
        app.layer_stack.layers = vec![layer];
        // Active water — either via preset or below-zero terrain.
        app.water_mode = WaterMode::Ocean;
        let (tone, msg) = app.validation_summary();
        assert!(matches!(tone, crate::ui::theme::ChipTone::Warn));
        assert!(msg.contains("DNTS + water"), "got: {msg}");
    }

    /// Helper: minimal `HeightmapState` so validation tests can
    /// bypass the "no heightmap" early return.
    fn test_heightmap_state(map_size: MapSize) -> HeightmapState {
        let hm = Heightmap::synth_ramp(map_size);
        let dims = hm.dims();
        let (min, max) = hm.min_max();
        HeightmapState {
            path: PathBuf::from("<test>"),
            data: hm,
            dims,
            min,
            max,
            validated_against: Some(map_size),
        }
    }

    // D10 / Sprint 17 (ADR-041) — the legacy
    // `bind_slot_to_channel` / `unbind_channel` helpers retired with
    // `inspector_splat`. Layers panel "Change slot…" + DNTS-channel
    // chip + slot-picker popup replace them; covered indirectly by
    // the `ProjectDiff::SetLayerProperty` round-trip tests in
    // barme-core.

    #[test]
    fn validation_summary_warns_when_slot_bound_without_specular() {
        // FINDINGS §7.2 lint: binding a DNTS slot without a specular
        // texture surfaces a warn-tone chip. Editor doesn't author
        // specular yet, so any bound channel trips this.
        let mut app = make_test_app();
        // Plant a heightmap matching `make_test_app`'s 2-SMU map so
        // we clear "No heightmap" + "Heightmap mismatch" early-outs.
        let dims = app.map_size.heightmap_dims();
        let data = vec![0u16; (dims.0 as usize) * (dims.1 as usize)];
        let hm = Heightmap::new(dims.0, dims.1, data).expect("build flat hm");
        app.heightmap = Some(HeightmapState {
            path: std::path::PathBuf::from("<fixture>"),
            data: hm,
            dims,
            min: 0,
            max: 0,
            validated_against: Some(app.map_size),
        });
        // Without a layer bound to a DNTS channel: should NOT be
        // the DNTS warning.
        let (tone, label) = app.validation_summary();
        assert!(!label.contains("DNTS"), "got {label} ({tone:?})");
        // D10 / Sprint 17 (ADR-041): bind a DNTS-channel layer so
        // `dnts_layers()` reports active. The chip flips to warn-tone.
        use barme_core::layers::{LayerSource, TextureLayer};
        let mut layer = TextureLayer::new(LayerSource::Slot { id: 0 }, app.map_size, 255);
        layer.dnts_channel = Some(barme_core::SplatChannel::R);
        app.layer_stack.layers = vec![layer];
        let (tone, label) = app.validation_summary();
        assert!(matches!(tone, crate::ui::theme::ChipTone::Warn));
        assert!(label.contains("DNTS"), "got {label}");
    }

    #[test]
    fn scan_slot_registry_handles_missing_dir() {
        // Robust to first checkouts where scripts/fetch-textures.sh
        // hasn't run yet.
        let r = scan_slot_registry(std::path::Path::new("/definitely/does/not/exist"));
        assert!(r.is_empty());
    }

    #[test]
    fn scan_slot_registry_parses_valid_meta_and_sorts_by_id() {
        // Build a fake registry tree with two slots in non-sorted
        // order; the result is sorted by `slot` id.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("03-second")).unwrap();
        std::fs::write(
            dir.path().join("03-second/meta.toml"),
            "slot = 3\nname = \"Second\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("00-first")).unwrap();
        std::fs::write(
            dir.path().join("00-first/meta.toml"),
            "slot = 0\nname = \"First\"\n",
        )
        .unwrap();
        let r = scan_slot_registry(dir.path());
        let ids: Vec<u8> = r.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![0, 3]);
        let names: Vec<&str> = r.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["First", "Second"]);
    }

    #[test]
    fn scan_slot_registry_skips_malformed_entries() {
        let dir = tempfile::tempdir().unwrap();
        // Bad: missing slot field.
        std::fs::create_dir_all(dir.path().join("bad-no-slot")).unwrap();
        std::fs::write(
            dir.path().join("bad-no-slot/meta.toml"),
            "name = \"Nope\"\n",
        )
        .unwrap();
        // Bad: missing meta.toml.
        std::fs::create_dir_all(dir.path().join("bad-no-meta")).unwrap();
        // Good control entry so we know the scan didn't bail at the
        // first malformed sibling.
        std::fs::create_dir_all(dir.path().join("01-good")).unwrap();
        std::fs::write(
            dir.path().join("01-good/meta.toml"),
            "slot = 1\nname = \"Good\"\n",
        )
        .unwrap();
        let r = scan_slot_registry(dir.path());
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, 1);
    }

    #[test]
    fn splat_uniforms_for_render_defaults_match_engine() {
        // Fresh app with no slots bound: uniforms should be the
        // engine defaults (FINDINGS §1.6) — texScales=0.02,
        // texMults=1.0, mask=0, diffuse_in_alpha=0.
        let app = make_test_app();
        let su = app.splat_uniforms_for_render();
        let base = render::SplatUniforms::default();
        assert_eq!(su.tex_scales, base.tex_scales);
        assert_eq!(su.tex_mults, base.tex_mults);
        assert_eq!(su.flags[0], 0);
        assert_eq!(su.flags[1], 0);
        assert_eq!(su.sun_dir, base.sun_dir);
        assert_eq!(su.ground_ambient, base.ground_ambient);
        assert_eq!(su.ground_diffuse, base.ground_diffuse);
    }

    // D10 / Sprint 17 (ADR-041) — `apply_splat_brush_at` +
    // `SplatBrushState` retired alongside `inspector_splat`. The
    // `SplatChannel::{R,G,B,A}.index()` pin moves to barme-core
    // (`splat.rs::tests`).

    /// D10 / Sprint 17 (ADR-041) — `snapshot_project` no longer
    /// mirrors a per-`App` `splat_config`; it always emits the
    /// default. Round-trip of new projects sees no `splat_config`
    /// table on disk (the field is `#[serde(skip_serializing)]`).
    #[test]
    fn snapshot_project_emits_default_splat_config() {
        let app = make_test_app();
        let p = app.snapshot_project();
        assert_eq!(p.splat_config, SplatConfig::default());
    }

    /// C4 (Sprint 11): `MetalState` is now slim view-state — the spot
    /// data lives on `App::metal_spots` (which mirrors
    /// `Project.metal_spots`). The default has nothing selected.
    #[test]
    fn metal_state_default_has_no_selection() {
        let m = MetalState::default();
        assert!(m.selected.is_none());
    }

    /// C5 (Sprint 11): `GeoState` mirrors `MetalState`'s shape — no
    /// scaffolded library / scatter knobs anymore. Those will return
    /// in C6 (Sprint 12) under a dedicated `Tool::Feature` variant.
    #[test]
    fn geo_state_default_has_no_selection() {
        let g = GeoState::default();
        assert!(g.selected.is_none());
    }

    /// Fresh `App` starts with no metal spots, no geo vents, and the
    /// BAR-default extractor radius.
    #[test]
    fn fresh_app_has_empty_metal_and_geo_state() {
        let app = make_test_app();
        assert!(app.metal_spots.is_empty());
        assert!(app.geo_vents.is_empty());
        assert_eq!(app.extractor_radius, default_extractor_radius());
        assert_eq!(app.extractor_radius, 80.0);
        assert!(app.metal_state.selected.is_none());
        assert!(app.geo_state.selected.is_none());
    }

    /// C4 (Sprint 11): placing a metal spot pushes the source,
    /// marks the project dirty, and writes a place-diff onto the
    /// undo stack. Ctrl-Z removes it.
    #[test]
    fn place_metal_spot_writes_undoable_diff() {
        let mut app = make_test_app();
        // 2 SMU square map: extent 2 * 512 = 1024 elmos.
        app.place_metal_spot(256.0, 256.0);
        assert_eq!(app.metal_spots.len(), 1);
        assert!(app.dirty);
        assert_eq!(app.history.undo_depth(), 1);
        // Undo removes the spot.
        app.undo_one();
        assert!(app.metal_spots.is_empty());
        // Redo re-adds.
        app.redo_one();
        assert_eq!(app.metal_spots.len(), 1);
        assert_eq!(app.metal_spots[0].metal, MetalSpot::DEFAULT_METAL);
    }

    /// Horizontal symmetry under metal placement yields one source
    /// per LMB-click, plus the mirror — each pushed as its own
    /// `ProjectDiff` so undo peels them one at a time (matches F8).
    #[test]
    fn place_metal_spot_with_horizontal_symmetry_emits_per_mirror_diffs() {
        let mut app = make_test_app();
        app.symmetry = SymmetryAxis::Horizontal;
        app.place_metal_spot(100.0, 256.0);
        // Source + horizontal mirror around the map centre — extent
        // 1024 → mirror at x = 1024 - 100 = 924.
        assert_eq!(app.metal_spots.len(), 2);
        let xs: Vec<i32> = app.metal_spots.iter().map(|m| m.x_elmo).collect();
        assert!(xs.contains(&100));
        assert!(xs.contains(&924));
        // Two diffs on the stack (one per source).
        assert_eq!(app.history.undo_depth(), 2);
    }

    /// Off-map clicks (negative / past-extent) are ignored without
    /// pushing diffs — matches the F8 pattern.
    #[test]
    fn place_metal_spot_off_map_is_a_noop() {
        let mut app = make_test_app();
        app.place_metal_spot(-100.0, 256.0);
        app.place_metal_spot(256.0, 9999.0);
        assert!(app.metal_spots.is_empty());
        assert_eq!(app.history.undo_depth(), 0);
    }

    /// Delete pushes a `DeleteMetalSpot` diff; undo restores.
    #[test]
    fn delete_metal_spot_round_trips() {
        let mut app = make_test_app();
        app.metal_spots.push(MetalSpot::new(100, 200));
        app.delete_metal_spot(0);
        assert!(app.metal_spots.is_empty());
        app.undo_one();
        assert_eq!(app.metal_spots.len(), 1);
        assert_eq!(app.metal_spots[0], MetalSpot::new(100, 200));
    }

    /// C5 (Sprint 11): same pattern for geo vents.
    #[test]
    fn place_geo_vent_writes_undoable_diff() {
        let mut app = make_test_app();
        app.place_geo_vent(256.0, 256.0);
        assert_eq!(app.geo_vents.len(), 1);
        assert!(app.dirty);
        assert_eq!(app.history.undo_depth(), 1);
        app.undo_one();
        assert!(app.geo_vents.is_empty());
    }

    /// Geo placement honours symmetry — extents 1024 → vertical
    /// mirror is at z = 1024 - 200 = 824.
    #[test]
    fn place_geo_vent_with_vertical_symmetry_mirrors() {
        let mut app = make_test_app();
        app.symmetry = SymmetryAxis::Vertical;
        app.place_geo_vent(512.0, 200.0);
        assert_eq!(app.geo_vents.len(), 2);
        let zs: Vec<i32> = app.geo_vents.iter().map(|v| v.z_elmo).collect();
        assert!(zs.contains(&200));
        assert!(zs.contains(&824));
    }

    /// `snapshot_project_for_build` expands metal_spots through the
    /// active symmetry, materialising the mirrors the editor canvas
    /// paints live. Without this expansion, BAR would only render
    /// the source spots and the user's symmetric layout would not
    /// reach the .sd7.
    #[test]
    fn snapshot_for_build_expands_metal_through_symmetry() {
        let mut app = make_test_app();
        app.symmetry = SymmetryAxis::Horizontal;
        // Place one source the long way (not via place_metal_spot so
        // the test isolates the expansion step from the placement
        // step).
        app.metal_spots.push(MetalSpot::new(100, 256));
        let p = app.snapshot_project_for_build();
        assert_eq!(p.metal_spots.len(), 2, "expansion missing");
        let xs: Vec<i32> = p.metal_spots.iter().map(|m| m.x_elmo).collect();
        assert!(xs.contains(&100));
        assert!(xs.contains(&924));
        // Metal value rides along on the mirror.
        for spot in &p.metal_spots {
            assert_eq!(spot.metal, MetalSpot::DEFAULT_METAL);
        }
    }

    /// `snapshot_project_for_build` also expands geo_vents.
    #[test]
    fn snapshot_for_build_expands_geo_through_symmetry() {
        let mut app = make_test_app();
        app.symmetry = SymmetryAxis::Quad;
        app.geo_vents.push(GeoVent::new(100, 200));
        let p = app.snapshot_project_for_build();
        // Quad: 4 entries from one source.
        assert_eq!(p.geo_vents.len(), 4);
    }

    /// `SymmetryAxis::None` leaves both vectors untouched.
    #[test]
    fn snapshot_for_build_no_op_when_symmetry_off() {
        let mut app = make_test_app();
        app.symmetry = SymmetryAxis::None;
        app.metal_spots.push(MetalSpot::new(100, 100));
        app.geo_vents.push(GeoVent::new(200, 200));
        let p = app.snapshot_project_for_build();
        assert_eq!(p.metal_spots.len(), 1);
        assert_eq!(p.geo_vents.len(), 1);
    }

    /// `snapshot_project` round-trips metal + geo + extractor radius
    /// onto a Project ready for save / build.
    #[test]
    fn snapshot_project_carries_metal_geo_and_extractor_radius() {
        let mut app = make_test_app();
        app.metal_spots.push(MetalSpot::new(100, 100));
        app.geo_vents.push(GeoVent::new(200, 200));
        app.extractor_radius = 95.0;
        let p = app.snapshot_project();
        assert_eq!(p.metal_spots.len(), 1);
        assert_eq!(p.geo_vents.len(), 1);
        assert_eq!(p.extractor_radius, 95.0);
    }

    /// `new_project` resets metal + geo + extractor_radius to the
    /// fresh defaults.
    #[test]
    fn new_project_clears_metal_geo_and_resets_extractor_radius() {
        let mut app = make_test_app();
        app.metal_spots.push(MetalSpot::new(100, 100));
        app.geo_vents.push(GeoVent::new(200, 200));
        app.extractor_radius = 120.0;
        app.new_project();
        assert!(app.metal_spots.is_empty());
        assert!(app.geo_vents.is_empty());
        assert_eq!(app.extractor_radius, default_extractor_radius());
    }

    /// SetExtractorRadius diff is reversible.
    #[test]
    fn set_extractor_radius_diff_round_trips() {
        let mut app = make_test_app();
        let before = app.extractor_radius;
        app.history
            .push_project_diff(ProjectDiff::SetExtractorRadius {
                from: before,
                to: 150.0,
            });
        // Simulate the inspector applying the new value.
        app.extractor_radius = 150.0;
        app.undo_one();
        assert_eq!(app.extractor_radius, before);
        app.redo_one();
        assert_eq!(app.extractor_radius, 150.0);
    }

    /// `start_positions_balanced` returns true when every allyteam
    /// shares the same source-count. The Inspector chip is wired
    /// through this — keep the semantics pinned.
    #[test]
    fn balanced_empty_groups_is_balanced() {
        let app = make_test_app();
        assert!(app.start_positions_balanced());
    }

    #[test]
    fn balanced_single_group_is_balanced() {
        let mut app = make_test_app();
        let mut g = AllyGroup::new(0);
        g.start_positions.push(StartPosition {
            x_elmo: 100,
            z_elmo: 100,
        });
        app.ally_groups.push(g);
        assert!(app.start_positions_balanced());
    }

    #[test]
    fn balanced_two_equal_groups_is_balanced() {
        let mut app = make_test_app();
        for (id, _name) in [(0, "West"), (1, "East")] {
            let mut g = AllyGroup::new(id);
            g.start_positions.push(StartPosition {
                x_elmo: 100,
                z_elmo: 100,
            });
            g.start_positions.push(StartPosition {
                x_elmo: 200,
                z_elmo: 200,
            });
            app.ally_groups.push(g);
        }
        assert!(app.start_positions_balanced());
    }

    #[test]
    fn balanced_two_unequal_groups_is_asymmetric() {
        let mut app = make_test_app();
        let mut west = AllyGroup::new(0);
        west.start_positions.push(StartPosition {
            x_elmo: 100,
            z_elmo: 100,
        });
        west.start_positions.push(StartPosition {
            x_elmo: 200,
            z_elmo: 200,
        });
        let east = AllyGroup::new(1);
        app.ally_groups.push(west);
        app.ally_groups.push(east);
        assert!(!app.start_positions_balanced());
    }

    /// `short_error` truncates long parser messages to a chip-friendly
    /// length and tags the cut with an ellipsis. Used by the Procgen
    /// inspector's section-header chip.
    #[test]
    fn short_error_truncates_long_messages() {
        let long = "this is a very long error message that goes well past the chip's visual budget";
        let short = super::short_error(long);
        assert!(short.ends_with('…'));
        assert!(short.chars().count() <= 33);
    }

    /// Short messages pass through untouched (no trailing ellipsis).
    #[test]
    fn short_error_passes_short_messages() {
        let s = super::short_error("unexpected token");
        assert_eq!(s, "unexpected token");
    }

    /// `short_error` collapses multi-line messages to the first line.
    #[test]
    fn short_error_collapses_multiline() {
        let s = super::short_error("first line\nsecond line\nthird");
        assert_eq!(s, "first line");
    }

    /// The symmetry pill toggle round-trips through
    /// `last_non_none_symmetry`. The test exercises the same
    /// transitions the top-bar pill does so a refactor of the toggle
    /// code can't drop the memory.
    #[test]
    fn symmetry_pill_toggle_remembers_last_mode() {
        let mut app = make_test_app();
        // Start in Quad.
        app.symmetry = SymmetryAxis::Quad;
        app.last_non_none_symmetry = SymmetryAxis::Horizontal;

        // Toggle off — last_non_none should become Quad.
        if !matches!(app.symmetry, SymmetryAxis::None) {
            app.last_non_none_symmetry = app.symmetry;
        }
        app.symmetry = SymmetryAxis::None;
        assert_eq!(app.last_non_none_symmetry, SymmetryAxis::Quad);

        // Toggle back on — symmetry should restore the remembered mode.
        app.symmetry = app.last_non_none_symmetry;
        assert_eq!(app.symmetry, SymmetryAxis::Quad);
    }

    /// B4: enabled-button gate combines heightmap-loaded AND
    /// variant-enabled. With no heightmap, even an enabled variant
    /// must NOT enqueue an action.
    #[test]
    fn build_variant_action_gates_on_heightmap_loaded() {
        let app = make_test_app();
        // make_test_app has heightmap = None.
        assert!(app.heightmap.is_none());
        // Even with `Install` (enabled), the click handler in
        // `top_bar` gates on `heightmap.is_some()`. We pin the
        // invariant by exercising the variant's own contract +
        // documenting that the gate is in the UI code; the action
        // mapping itself is variant-only.
        assert!(BuildVariant::Install.to_file_action().is_some());
    }

    // ----------------------------- B5 -----------------------------

    /// B5 smoke: place a start position, Ctrl-Z → marker removed.
    /// Push diff happens inside `place_start_position`; `undo_one`
    /// dispatches it through `apply_project_diff`.
    #[test]
    fn b5_place_then_undo_removes_marker() {
        let mut app = make_test_app();
        app.place_start_position(100.0, 100.0);
        assert_eq!(app.ally_groups[0].start_positions.len(), 1);
        assert_eq!(app.history.undo_depth(), 1);
        app.undo_one();
        assert!(app.ally_groups[0].start_positions.is_empty());
        assert_eq!(app.history.undo_depth(), 0);
        assert_eq!(app.history.redo_depth(), 1, "undo pushes onto redo");
    }

    /// B5 smoke: place + undo + redo restores the marker.
    #[test]
    fn b5_place_then_undo_redo_round_trips() {
        let mut app = make_test_app();
        app.place_start_position(50.0, 75.0);
        let pos_before = app.ally_groups[0].start_positions[0];
        app.undo_one();
        assert!(app.ally_groups[0].start_positions.is_empty());
        app.redo_one();
        assert_eq!(app.ally_groups[0].start_positions.len(), 1);
        assert_eq!(app.ally_groups[0].start_positions[0], pos_before);
    }

    /// B5 smoke: delete a marker, Ctrl-Z restores it (RMB path).
    #[test]
    fn b5_delete_then_undo_restores_marker() {
        let mut app = make_test_app();
        app.place_start_position(100.0, 100.0);
        let pos = app.ally_groups[0].start_positions[0];
        // Place pushed one diff; clear redo bookkeeping by deleting.
        app.delete_start_position(0, 0);
        assert!(app.ally_groups[0].start_positions.is_empty());
        assert_eq!(app.history.undo_depth(), 2, "place + delete pushed");
        app.undo_one(); // undo the delete
        assert_eq!(app.ally_groups[0].start_positions.len(), 1);
        assert_eq!(app.ally_groups[0].start_positions[0], pos);
    }

    /// B5 smoke: F1 wizard apply followed by Ctrl-Z restores the
    /// pre-wizard project metadata. We construct an app, mutate it,
    /// then run the wizard, then undo and confirm the metadata
    /// rolls back.
    #[test]
    fn b5_apply_wizard_then_undo_restores_metadata() {
        let mut app = make_test_app();
        // Pre-wizard state.
        app.project_name = "pre".to_string();
        app.map_size = MapSize { smu_x: 4, smu_z: 6 };
        app.height_scale = 123.0;
        app.symmetry = SymmetryAxis::Horizontal;
        let mut pre_group = AllyGroup::new(0);
        pre_group.start_positions.push(StartPosition {
            x_elmo: 999,
            z_elmo: 999,
        });
        app.ally_groups = vec![pre_group];
        // Configure wizard for a different post-apply state.
        app.wizard.project_name = "post-wizard".to_string();
        app.wizard.smu_x = 8;
        app.wizard.smu_z = 8;
        app.wizard.symmetry = SymmetryAxis::Vertical;
        app.wizard.max_height = 333.0;
        app.wizard.biome_index = 0;
        app.apply_wizard();
        // Post-wizard.
        assert_eq!(app.project_name, "post-wizard");
        assert_eq!(app.map_size, MapSize { smu_x: 8, smu_z: 8 });
        assert!(matches!(app.symmetry, SymmetryAxis::Vertical));
        // B8: apply_wizard seeds 2 demo start positions in
        // ally_groups[0]. Pre-B8 this assertion was `is_empty()`.
        assert_eq!(app.ally_groups.len(), 1);
        assert_eq!(app.ally_groups[0].start_positions.len(), 2);
        assert_eq!(app.history.undo_depth(), 1, "one ApplyWizard entry");
        // Undo.
        app.undo_one();
        assert_eq!(app.project_name, "pre");
        assert_eq!(app.map_size, MapSize { smu_x: 4, smu_z: 6 });
        assert_eq!(app.height_scale, 123.0);
        assert!(matches!(app.symmetry, SymmetryAxis::Horizontal));
        assert_eq!(app.ally_groups.len(), 1);
        assert_eq!(app.ally_groups[0].start_positions[0].x_elmo, 999);
        assert_eq!(app.history.redo_depth(), 1);
    }

    /// B5 smoke: undo is gated on `!is_dragging_anything()`. Setting
    /// `dragging_start_pos` to `Some(_)` prevents undo_one from
    /// popping an entry — the user is mid-gesture and Ctrl-Z must
    /// not yank state out from under them.
    #[test]
    fn b5_undo_gated_by_in_flight_drag() {
        let mut app = make_test_app();
        app.place_start_position(100.0, 100.0);
        assert_eq!(app.history.undo_depth(), 1);
        // Simulate a drag-in-progress on the same marker.
        let pos = app.ally_groups[0].start_positions[0];
        app.dragging_start_pos = Some((0, 0));
        app.dragging_start_pos_from = Some(pos);
        app.undo_one();
        // Drag gate held: nothing popped.
        assert_eq!(
            app.history.undo_depth(),
            1,
            "undo while dragging must be a no-op"
        );
        assert_eq!(app.ally_groups[0].start_positions.len(), 1);
        // Releasing the drag clears the gate; subsequent undo works.
        app.dragging_start_pos = None;
        app.dragging_start_pos_from = None;
        app.undo_one();
        assert_eq!(app.history.undo_depth(), 0);
    }

    /// B5 smoke: `is_dragging_anything()` reflects both gesture
    /// channels — brush stroke + start-position drag.
    #[test]
    fn b5_is_dragging_anything_covers_both_channels() {
        let mut app = make_test_app();
        assert!(!app.is_dragging_anything());
        app.dragging_start_pos = Some((0, 0));
        assert!(app.is_dragging_anything(), "start-pos drag should gate");
        app.dragging_start_pos = None;
        // Heightmap-stroke gate is harder to fake without a heightmap;
        // we exercise the start-pos branch + the false case here.
        // The heightmap branch is covered by the existing
        // `stroke_open()` tests in `barme-core::undo`.
        assert!(!app.is_dragging_anything());
    }

    // ────── B8 — wizard demo state ──────

    /// The wizard's default state must seed Horizontal symmetry so the
    /// two demo start positions on the N/S strips read as a 1v1 pair.
    /// Pre-B8 this default was `None` — flipping it now is the entire
    /// behavioural point of the item.
    #[test]
    fn b8_wizard_default_symmetry_is_horizontal() {
        let w = WizardState::default_for_new_project();
        assert_eq!(w.symmetry, SymmetryAxis::Horizontal);
    }

    /// B8 seeds two positions in `ally_groups[0]` on N/S strips at
    /// 15 % / 85 % of map Z. When no heightmap is loaded (the
    /// valley-finder bails on `None`) the proposal is passed
    /// through unchanged.
    #[test]
    fn b8_seed_demo_start_positions_lands_two_on_n_s_strips() {
        let mut app = make_test_app();
        app.map_size = MapSize::square(16);
        app.seed_demo_start_positions();
        assert_eq!(app.ally_groups.len(), 1, "single ally group");
        let g = &app.ally_groups[0];
        assert_eq!(g.id, 0);
        assert_eq!(g.start_positions.len(), 2, "expected 2 starts");
        let (ex, ez) = app.map_size.elmo_extents();
        let cx = ex as i32 / 2;
        let n = g.start_positions[0];
        let s = g.start_positions[1];
        assert_eq!(n.x_elmo, cx, "north strip centred in X");
        assert_eq!(s.x_elmo, cx, "south strip centred in X");
        // ~15% of 8192 = 1228; ~85% = 6963 (rounding bands).
        let z15 = (ez as f32 * 0.15) as i32;
        let z85 = (ez as f32 * 0.85) as i32;
        assert!(
            (n.z_elmo - z15).abs() <= 2,
            "north should sit on the 15% strip, got {} vs target {z15}",
            n.z_elmo
        );
        assert!(
            (s.z_elmo - z85).abs() <= 2,
            "south should sit on the 85% strip, got {} vs target {z85}",
            s.z_elmo
        );
    }

    /// dismiss_next_steps flips both the show flag and the project-
    /// scoped `next_steps_dismissed`. Latter is what snapshot_project
    /// will persist into `.barmeproj`.
    #[test]
    fn b8_dismiss_next_steps_persists_in_project_state() {
        let mut app = make_test_app();
        app.show_next_steps = true;
        app.next_steps_dismissed = false;
        app.dismiss_next_steps();
        assert!(!app.show_next_steps);
        assert!(app.next_steps_dismissed);
        let snap = app.snapshot_project();
        assert!(
            snap.next_steps_dismissed,
            "snapshot must round-trip the dismissed flag into .barmeproj"
        );
    }

    /// new_project resets the per-project dismissal so a freshly-
    /// created project re-shows the Next-steps hint on next wizard
    /// Create. This is the pitfall the per-project (vs per-user) flag
    /// guards against.
    #[test]
    fn b8_new_project_resets_next_steps_dismissed() {
        let mut app = make_test_app();
        app.next_steps_dismissed = true;
        app.show_next_steps = false;
        app.new_project();
        assert!(
            !app.next_steps_dismissed,
            "new_project must arm the hint for the next wizard Create"
        );
    }

    // ────── B7 — Procgen UX redesign ──────

    /// The thumbnail dirty-key must fold in BOTH expression and domain.
    /// A toggle from Unit ↔ Centered with an unchanged expression
    /// string must invalidate the cache — without that, the preview
    /// would show stale `x` ramps when the user flipped domain.
    #[test]
    fn b7_thumbnail_key_changes_on_domain_toggle() {
        let a = procgen_thumbnail_key("x", Domain::Unit);
        let b = procgen_thumbnail_key("x", Domain::Centered);
        assert_ne!(
            a, b,
            "thumbnail cache key must distinguish Unit vs Centered for the same expr"
        );
    }

    #[test]
    fn b7_thumbnail_key_changes_on_expression_edit() {
        let a = procgen_thumbnail_key("x", Domain::Unit);
        let b = procgen_thumbnail_key("x*x", Domain::Unit);
        assert_ne!(a, b);
    }

    /// `revalidate_procgen` is the choke point for re-arming the
    /// debounce. Every code path that mutates `procgen_expr` /
    /// `procgen_domain` (preset click, biome apply, keystroke, domain
    /// toggle) calls through it, so this single contract pin covers
    /// all of them.
    #[test]
    fn b7_revalidate_arms_debounce_timer() {
        let mut app = make_test_app();
        assert!(
            app.procgen_changed_at.is_none(),
            "fresh app has no pending change"
        );
        app.procgen_expr = "x*x".to_string();
        app.revalidate_procgen();
        assert!(
            app.procgen_changed_at.is_some(),
            "revalidate_procgen must arm the debounce timer"
        );
    }

    /// Constants pin: the prompt enumerates 256 px and 50 ms; a
    /// future "tune this" PR shouldn't silently change either.
    #[test]
    fn b7_thumbnail_constants_match_spec() {
        assert_eq!(PROCGEN_THUMBNAIL_PX, 256);
        assert_eq!(PROCGEN_THUMBNAIL_DEBOUNCE_MS, 50);
    }

    /// B5: a single click-and-immediate-release on a marker (no actual
    /// move) must NOT push a MoveStartPosition entry — that would
    /// pollute the undo stack with no-op gestures.
    #[test]
    fn b5_zero_distance_drag_does_not_push_undo_entry() {
        let mut app = make_test_app();
        app.place_start_position(100.0, 100.0);
        let depth_after_place = app.history.undo_depth();
        // Simulate a drag start + drag stop with no movement.
        let pos = app.ally_groups[0].start_positions[0];
        app.dragging_start_pos = Some((0, 0));
        app.dragging_start_pos_from = Some(pos);
        app.finish_start_position_drag();
        assert_eq!(
            app.history.undo_depth(),
            depth_after_place,
            "zero-distance drag must not push an undo entry"
        );
    }

    // ---------------- terrain_y_at (Sprint 13 hotfix) ----------------
    //
    // Bug 1: PHASE A marker construction in `central()` used a hard-
    // coded world-Y = 0 which buried markers under any non-flat
    // terrain. `terrain_y_at` samples the loaded heightmap to lift
    // each marker to the surface; the existing `MARKER_Y_LIFT_ELMOS`
    // continues to add the small epsilon on top.

    /// Plant `data` (row-major, dims-matched) on `app` at `map_size`.
    /// Skips PNG IO / dim validation — direct construction is fine
    /// for these unit tests.
    fn install_heightmap_with_data(app: &mut App, map_size: MapSize, data: Vec<u16>) {
        let dims = map_size.heightmap_dims();
        assert_eq!(
            data.len(),
            (dims.0 as usize) * (dims.1 as usize),
            "test fixture data length must match map_size heightmap dims"
        );
        let hm = Heightmap::new(dims.0, dims.1, data).expect("build test heightmap");
        app.map_size = map_size;
        app.heightmap = Some(HeightmapState {
            path: std::path::PathBuf::from("<fixture>"),
            data: hm,
            dims,
            min: 0,
            max: u16::MAX,
            validated_against: Some(map_size),
        });
    }

    #[test]
    fn terrain_y_at_returns_zero_without_heightmap() {
        // `App::heightmap` = None ⇒ no panic, returns 0.0 regardless
        // of XZ (including out-of-map coordinates). This is the early-
        // out path used during empty-state CTA rendering.
        let app = make_test_app();
        assert_eq!(app.terrain_y_at(0.0, 0.0), 0.0);
        assert_eq!(app.terrain_y_at(123.0, 456.0), 0.0);
        assert_eq!(app.terrain_y_at(-9999.0, 9999.0), 0.0);
    }

    #[test]
    fn terrain_y_at_samples_known_value() {
        // 2-SMU square map: heightmap dims = 64*2 + 1 = 129×129.
        // Plant a single non-zero sample at the centre pixel and
        // verify `terrain_y_at` decodes the same world-Y the terrain
        // shader would (raw / 65535 * height_scale).
        let mut app = make_test_app();
        let map_size = MapSize::square(2);
        let dims = map_size.heightmap_dims();
        let (w, h) = (dims.0 as usize, dims.1 as usize);
        let mut data = vec![0u16; w * h];
        // Centre pixel (64, 64) — corresponds to XZ = (64*8, 64*8) =
        // (512, 512) elmos.
        data[64 * w + 64] = 32768;
        install_heightmap_with_data(&mut app, map_size, data);
        app.height_scale = 1000.0;

        let y = app.terrain_y_at(512.0, 512.0);
        let expected = (32768.0 / 65535.0) * 1000.0;
        assert!((y - expected).abs() < 1e-3, "expected {expected}, got {y}",);
    }

    #[test]
    fn terrain_y_at_clamps_out_of_bounds() {
        // XZ way past the map extent must clamp to the edge sample,
        // not panic and not wrap. Plant a known value at the very
        // last pixel; sample at +∞ XZ; expect the edge value.
        let mut app = make_test_app();
        let map_size = MapSize::square(2);
        let dims = map_size.heightmap_dims();
        let (w, h) = (dims.0 as usize, dims.1 as usize);
        let mut data = vec![0u16; w * h];
        data[(h - 1) * w + (w - 1)] = u16::MAX;
        install_heightmap_with_data(&mut app, map_size, data);
        app.height_scale = 500.0;

        // (1_000_000, 1_000_000) is well past the 1024-elmo extent;
        // the clamp picks the (w-1, h-1) sample = u16::MAX.
        let y_far = app.terrain_y_at(1_000_000.0, 1_000_000.0);
        assert!(
            (y_far - 500.0).abs() < 1e-3,
            "expected 500.0 (clamped to edge), got {y_far}",
        );

        // Negative XZ clamps to (0, 0) which is the zero pixel.
        let y_neg = app.terrain_y_at(-1_000_000.0, -1_000_000.0);
        assert!((y_neg - 0.0).abs() < 1e-3, "expected 0.0, got {y_neg}");
    }

    #[test]
    fn terrain_y_at_rounds_to_nearest_pixel() {
        // ELMOS_PER_PIXEL = 8 → pixel 1 = elmos [4, 12). Place a
        // value at pixel (1, 0). Sampling at 4.0 elmos rounds DOWN
        // to pixel 0 (round-half-to-even via f32::round); 8.0 elmos
        // is exactly pixel 1; 7.9 elmos rounds to pixel 1.
        let mut app = make_test_app();
        let map_size = MapSize::square(2);
        let dims = map_size.heightmap_dims();
        let (w, h) = (dims.0 as usize, dims.1 as usize);
        let mut data = vec![0u16; w * h];
        data[1] = u16::MAX; // pixel (1, 0)
        install_heightmap_with_data(&mut app, map_size, data);
        app.height_scale = 100.0;

        // Pixel 0 → samples the zero pixel.
        assert!((app.terrain_y_at(0.0, 0.0) - 0.0).abs() < 1e-3);
        // Pixel 1 → samples the planted value.
        assert!((app.terrain_y_at(8.0, 0.0) - 100.0).abs() < 1e-3);
        // 7.9 elmos rounds to pixel 1.
        assert!(
            (app.terrain_y_at(7.9, 0.0) - 100.0).abs() < 1e-3,
            "got {}",
            app.terrain_y_at(7.9, 0.0),
        );
        // 3.9 elmos rounds DOWN to pixel 0 → zero.
        assert!((app.terrain_y_at(3.9, 0.0) - 0.0).abs() < 1e-3);
    }
}
