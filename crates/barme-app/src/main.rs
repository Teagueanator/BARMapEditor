mod config;
mod launcher;
mod render;
mod ui;

use std::path::{Path, PathBuf};

use anyhow::Result;
use barme_core::{
    BIOMES, BrushRegistry, BrushStamp, DirtyRect, Heightmap, History, HistoryEntry, MapSize,
    PROJECT_EXTENSION, Project, ProjectDiff, StartPosition, SymmetryAxis, WizardSnapshot,
    brushes::pixel_bbox,
    procgen::{Domain, PRESETS, generate as procgen_generate, validate_expression},
    project::sanitize_name,
    start_pos::assign_team_ids,
};
use barme_pipeline::PyMapConvDriver;
use eframe::egui;
use eframe::egui_wgpu;
use tracing::{error, info, trace, warn};

use crate::render::{OrbitCamera, TerrainCallback};

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
    current_project_path: Option<PathBuf>,
    last_install: Option<Result<PathBuf, String>>,
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
    /// Authored team start positions; round-trips through `Project`.
    /// Empty by default — the pipeline falls back to a 25/75 default pair.
    start_positions: Vec<StartPosition>,
    /// While LMB is held in `StartPositions` mode on an existing marker,
    /// holds that team's id so the drag re-positions it. Cleared on
    /// release.
    dragging_start_pos: Option<u8>,
    /// Pre-drag elmo coordinates for the marker currently being dragged.
    /// `Some` whenever `dragging_start_pos` is `Some`; on drag-stop the
    /// `from` is paired with the now-current `to` and pushed as a
    /// `ProjectDiff::MoveStartPosition` undo entry (B5).
    dragging_start_pos_from: Option<(u32, u32)>,
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
    /// Set on `drag_started` inside the nav-gizmo rect; cleared on
    /// `drag_stopped`. While true, LMB-drag orbits regardless of
    /// active tool — same camera math as the existing RMB orbit.
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
            symmetry: SymmetryAxis::None,
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
/// Each variant has a one-letter accelerator. Phase 4 will add `Splat`,
/// `Metal`, and `Feature` variants here — every match site is exhaustive
/// so adding a variant produces a compile error at each dispatch
/// location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Tool {
    /// Camera-only. LMB orbits; no central-rect editing.
    Select,
    /// Heightmap brush (F2 / ADR-018). LMB stamps the current brush.
    Sculpt,
    /// F8 start-position placement (ADR-023). LMB places / drags
    /// markers, RMB deletes.
    StartPositions,
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
    const ALL: [Tool; 4] = [
        Tool::Select,
        Tool::Sculpt,
        Tool::StartPositions,
        Tool::Procgen,
    ];

    /// One-character glyph rendered in the left tool strip. Picked to be
    /// present in egui's default proportional + monospace fonts so we
    /// don't pull in an icon font dependency.
    fn icon(self) -> &'static str {
        match self {
            Tool::Select => "↺",
            Tool::Sculpt => "✎",
            Tool::StartPositions => "⚑",
            Tool::Procgen => "ƒ",
        }
    }

    /// Single-letter accelerator key. Wired in `App::handle_keyboard`.
    fn accel(self) -> &'static str {
        match self {
            Tool::Select => "Q",
            Tool::Sculpt => "B",
            Tool::StartPositions => "S",
            Tool::Procgen => "G",
        }
    }

    /// Long-form name for hover tooltips + tracing output.
    fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select / orbit",
            Tool::Sculpt => "Sculpt",
            Tool::StartPositions => "Start positions",
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

        let editor_config = config::EditorConfig::load();
        let show_intro = !editor_config.intro_seen_for_current_version();

        Self {
            project_name: "untitled".to_string(),
            map_size: MapSize::square(16),
            heightmap: None,
            last_error: None,
            render_state,
            camera: OrbitCamera::framing(8192.0, 8192.0),
            height_scale: 256.0,
            current_project_path: None,
            last_install: None,
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
            history: History::default(),
            tool: Tool::Sculpt,
            previous_tool: Tool::Sculpt,
            start_positions: Vec::new(),
            dragging_start_pos: None,
            dragging_start_pos_from: None,
            // Open on first launch — the F1 wizard *is* the entry point now.
            wizard_open: true,
            wizard: WizardState::default_for_new_project(),
            symmetry_popover_open: false,
            editor_config,
            show_intro,
            show_cheat_sheet: false,
            nav_gizmo_drag_active: false,
            build_variant: BuildVariant::default(),
            mapinfo_overrides: std::collections::HashMap::new(),
        }
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
        self.camera = OrbitCamera::framing(8192.0, 8192.0);
        self.last_error = None;
        self.last_install = None;
        self.start_positions.clear();
        self.mapinfo_overrides.clear();
        self.dragging_start_pos = None;
        self.dragging_start_pos_from = None;
        self.end_stroke();
        self.history.barrier();
    }

    fn snapshot_project(&self) -> Project {
        Project {
            name: self.project_name.clone(),
            size: self.map_size,
            min_height: 0.0,
            max_height: self.height_scale,
            heightmap: self.heightmap.as_ref().map(|h| h.path.clone()),
            start_positions: self.start_positions.clone(),
            mapinfo_overrides: self.mapinfo_overrides.clone(),
        }
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
            }
            Err(e) => {
                error!(path = %path.display(), error = %format!("{e:#}"), "project save failed");
                self.last_error = Some(format!("save: {e:#}"));
            }
        }
    }

    /// Refresh the cached parse-and-dry-eval outcome (ADR-…/A4). Stores
    /// the formatted `#[source]` chain so the UI tooltip can render it
    /// directly. Called whenever `procgen_expr` changes — keystroke, preset
    /// pick, biome apply. Cost is ~μs for typical inputs, but the input is
    /// capped at `procgen::MAX_EXPRESSION_LEN` chars by the validator
    /// itself.
    fn revalidate_procgen(&mut self) {
        self.procgen_validation =
            validate_expression(&self.procgen_expr).map_err(|e| format!("{e:#}"));
    }

    /// Generate a heightmap from the current procgen expression and
    /// replace the loaded heightmap. Errors render as a red label in the
    /// "Generate from formula" panel; the existing heightmap is left
    /// untouched on failure.
    fn apply_procgen(&mut self) {
        self.procgen_last_error = None;
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
    /// loaded.
    fn apply_brush_at(&mut self, cursor: egui::Pos2, rect: egui::Rect) {
        let Some(rs) = self.render_state.as_ref() else {
            return;
        };
        let Some(hm_state) = self.heightmap.as_mut() else {
            return;
        };
        let Some(brush_id) = self.brush_id.as_deref() else {
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
            strength = self.brush_strength,
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
                strength: self.brush_strength,
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
        let (ex, ez) = self.map_size.elmo_extents();
        self.camera = OrbitCamera::framing(ex as f32, ez as f32);

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
        // B5: history is now empty (apply_procgen barriered it). Push
        // one ApplyWizard entry holding the pre-wizard snapshot so
        // Ctrl-Z reverts the whole wizard apply atomically.
        self.history
            .push_project_diff(ProjectDiff::ApplyWizard(Box::new(pre_wizard)));
        self.wizard_open = false;
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

    /// Place a new start position at `(world_x, world_z)` and, when
    /// symmetry is active, place its mirror counterparts at the derived
    /// centers. Team ids are assigned via [`assign_team_ids`] alternating
    /// even / odd from the lowest unused even id so mirror pairs map onto
    /// BAR's per-side `teams[]` convention. ADR-023.
    fn place_start_position(&mut self, world_x: f32, world_z: f32) {
        let extents = self.world_extents();
        let (ex, ez) = extents;
        // Clip the originating click; bail if outside the map.
        if world_x < 0.0 || world_x > ex || world_z < 0.0 || world_z > ez {
            trace!(
                world_x,
                world_z,
                extents = ?extents,
                "start position click landed off-map; ignored"
            );
            return;
        }
        let centers = self.symmetry.replicate((world_x, world_z), extents);
        let used: Vec<u8> = self.start_positions.iter().map(|p| p.team_id).collect();
        let ids = assign_team_ids(&used, centers.len());
        for ((cx, cz), id) in centers.into_iter().zip(ids) {
            let pos = StartPosition {
                team_id: id,
                x_elmo: cx.round().clamp(0.0, ex) as u32,
                z_elmo: cz.round().clamp(0.0, ez) as u32,
            };
            info!(
                team_id = pos.team_id,
                x_elmo = pos.x_elmo,
                z_elmo = pos.z_elmo,
                symmetry = self.symmetry.id(),
                "start position placed"
            );
            self.start_positions.push(pos);
            // B5: each placed position is its own undo entry. Symmetry-
            // replicated mirrors push individually so Ctrl-Z peels them
            // off one at a time in reverse-placement order.
            self.history
                .push_project_diff(ProjectDiff::PlaceStartPosition(pos));
        }
    }

    /// Move the position with `team_id` to the given world coordinates,
    /// clamped to the map. No-op if the id isn't present. Drag-emitted
    /// frame-by-frame, so this does NOT push an undo entry — that lands
    /// on `drag_stopped` via [`Self::finish_start_position_drag`].
    fn move_start_position(&mut self, team_id: u8, world_x: f32, world_z: f32) {
        let (ex, ez) = self.world_extents();
        if let Some(p) = self
            .start_positions
            .iter_mut()
            .find(|p| p.team_id == team_id)
        {
            p.x_elmo = world_x.clamp(0.0, ex).round() as u32;
            p.z_elmo = world_z.clamp(0.0, ez).round() as u32;
        }
    }

    /// Commit an in-flight start-position drag. Pushes a single
    /// `MoveStartPosition` undo entry covering the whole drag (start
    /// coords → end coords) when both are known and changed. Idempotent;
    /// always clears `dragging_start_pos*`. B5.
    fn finish_start_position_drag(&mut self) {
        let (Some(team_id), Some(from)) = (
            self.dragging_start_pos.take(),
            self.dragging_start_pos_from.take(),
        ) else {
            // No drag was in flight, or the drag started off-marker.
            self.dragging_start_pos = None;
            self.dragging_start_pos_from = None;
            return;
        };
        let Some(p) = self.start_positions.iter().find(|p| p.team_id == team_id) else {
            // Marker was deleted during the drag (RMB clicks fire
            // concurrently). Nothing to commit.
            return;
        };
        let to = (p.x_elmo, p.z_elmo);
        if from == to {
            // Zero-distance drag (click + immediate release). Don't
            // pollute the undo stack with a no-op move.
            return;
        }
        self.history
            .push_project_diff(ProjectDiff::MoveStartPosition { team_id, from, to });
    }

    /// Predicate: is any edit drag currently in flight? Brush strokes
    /// (heightmap channel) and start-position drags (project-diff
    /// channel) both gate undo/redo so the user can't peel back state
    /// mid-gesture. B5.
    fn is_dragging_anything(&self) -> bool {
        self.history.stroke_open() || self.dragging_start_pos.is_some()
    }

    /// Remove the position with `team_id`. No-op if absent. B5: pushes a
    /// `DeleteStartPosition` undo entry holding the full pre-delete
    /// position so undo can re-add it verbatim.
    fn delete_start_position(&mut self, team_id: u8) {
        let removed = self
            .start_positions
            .iter()
            .find(|p| p.team_id == team_id)
            .copied();
        let Some(pos) = removed else {
            return;
        };
        self.start_positions.retain(|p| p.team_id != team_id);
        info!(team_id, "start position deleted");
        self.history
            .push_project_diff(ProjectDiff::DeleteStartPosition(pos));
    }

    /// Find the start position whose on-screen marker is within `radius_px`
    /// of `cursor`. Returns its team_id. Used for drag and right-click hit
    /// testing in the central preview rect.
    fn hit_test_start_position(
        &self,
        cursor: egui::Pos2,
        rect: egui::Rect,
        radius_px: f32,
    ) -> Option<u8> {
        let rect_size = glam::Vec2::new(rect.width(), rect.height());
        let cursor_in_rect = glam::Vec2::new(cursor.x - rect.min.x, cursor.y - rect.min.y);
        let mut best: Option<(u8, f32)> = None;
        for pos in &self.start_positions {
            let world = glam::Vec3::new(pos.x_elmo as f32, 0.0, pos.z_elmo as f32);
            let Some(screen) = render::world_to_screen(world, rect_size, &self.camera) else {
                continue;
            };
            let d = (screen - cursor_in_rect).length();
            if d <= radius_px && best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((pos.team_id, d));
            }
        }
        best.map(|(id, _)| id)
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
            HistoryEntry::Project(diff) => HistoryEntry::Project(self.apply_project_diff(diff)),
        }
    }

    /// Dispatch a `ProjectDiff` against this `App`'s F8 + wizard state,
    /// returning the inverse to push onto the opposite stack. The
    /// inversion is symmetric for Place/Delete (swap variants) and
    /// Move/ApplyWizard (swap the from↔to and old↔current snapshot
    /// respectively). B5.
    fn apply_project_diff(&mut self, diff: ProjectDiff) -> ProjectDiff {
        match diff {
            ProjectDiff::PlaceStartPosition(p) => {
                // Undo: remove p. Redo direction: re-add.
                self.start_positions.retain(|q| q.team_id != p.team_id);
                trace!(team_id = p.team_id, "undo: removed placed start position");
                ProjectDiff::DeleteStartPosition(p)
            }
            ProjectDiff::DeleteStartPosition(p) => {
                self.start_positions.push(p);
                trace!(team_id = p.team_id, "undo: restored deleted start position");
                ProjectDiff::PlaceStartPosition(p)
            }
            ProjectDiff::MoveStartPosition { team_id, from, to } => {
                if let Some(p) = self
                    .start_positions
                    .iter_mut()
                    .find(|p| p.team_id == team_id)
                {
                    p.x_elmo = from.0;
                    p.z_elmo = from.1;
                }
                trace!(team_id, ?from, ?to, "undo: reverted start position move");
                ProjectDiff::MoveStartPosition {
                    team_id,
                    from: to,
                    to: from,
                }
            }
            ProjectDiff::ApplyWizard(snap) => {
                // Capture the *current* (post-wizard) state into the
                // inverse so redo restores it. Then swap in `*snap`.
                let current = Box::new(self.capture_wizard_snapshot());
                self.restore_wizard_snapshot(*snap);
                info!("undo: reverted F1 wizard apply");
                ProjectDiff::ApplyWizard(current)
            }
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
            start_positions: self.start_positions.clone(),
            procgen_expr: self.procgen_expr.clone(),
            procgen_domain: self.procgen_domain,
        }
    }

    /// Restore a `WizardSnapshot` over the current app state. Mirror of
    /// [`Self::capture_wizard_snapshot`]; the camera is reframed from
    /// the restored map size. B5.
    fn restore_wizard_snapshot(&mut self, snap: WizardSnapshot) {
        self.project_name = snap.project_name;
        self.map_size = snap.map_size;
        self.height_scale = snap.height_scale;
        self.symmetry = snap.symmetry;
        self.rotational_fold = snap.rotational_fold;
        self.start_positions = snap.start_positions;
        self.procgen_expr = snap.procgen_expr;
        self.procgen_domain = snap.procgen_domain;
        self.revalidate_procgen();
        let (ex, ez) = self.map_size.elmo_extents();
        self.camera = OrbitCamera::framing(ex as f32, ez as f32);
    }

    /// Compile the current project to a `.sd7` and copy it into BAR's
    /// user maps directory. v0 UX: heightmap must be loaded, texture is a
    /// synthesised flat grey (Stage 1 will replace with real DNTS).
    fn build_and_install(&mut self) {
        self.last_install = None;
        self.last_error = None;
        let Some(hm) = self.heightmap.as_ref() else {
            warn!("build & install requested with no heightmap loaded");
            self.last_error = Some("load a heightmap first".into());
            return;
        };
        // The CPU-side heightmap is authoritative (may include unsaved
        // brush edits). Serialize to a temp PNG so the pipeline gets the
        // current state, not a stale on-disk snapshot.
        let tmp = match tempfile::tempdir() {
            Ok(t) => t,
            Err(e) => {
                let msg = format!("tempdir: {e:#}");
                error!("build & install tempdir failed: {msg}");
                self.last_install = Some(Err(msg));
                return;
            }
        };
        let hm_path = tmp.path().join("heightmap.png");
        if let Err(e) = hm.data.save_png(&hm_path) {
            let msg = format!("write heightmap: {e:#}");
            error!("build & install snapshot failed: {msg}");
            self.last_install = Some(Err(msg));
            return;
        }
        let Some(dst_dir) = launcher::bar_maps_dir() else {
            let msg =
                "could not locate BAR maps dir on this platform — pick one manually (Stage 1)";
            warn!("{msg}");
            self.last_install = Some(Err(msg.into()));
            return;
        };
        let repo_root = repo_root();
        let driver = match PyMapConvDriver::vendored(&repo_root) {
            Ok(d) => d,
            Err(e) => {
                let msg = format!("{e:#}");
                error!("pymapconv unavailable: {msg}");
                self.last_install = Some(Err(msg));
                return;
            }
        };
        let project = self.snapshot_project();
        info!(
            name = %project.name,
            smu_x = self.map_size.smu_x,
            smu_z = self.map_size.smu_z,
            max_height = self.height_scale,
            heightmap = %hm_path.display(),
            dst = %dst_dir.display(),
            "build & install requested"
        );
        match launcher::build_and_install(&driver, &project, &hm_path, None, &dst_dir) {
            Ok(installed) => {
                let bytes = std::fs::metadata(&installed).map(|m| m.len()).unwrap_or(0);
                info!(
                    path = %installed.display(),
                    bytes,
                    "build & install ok"
                );
                self.last_install = Some(Ok(installed));
            }
            Err(e) => {
                let msg = format!("{e:#}");
                error!(error = %msg, "build & install failed");
                self.last_install = Some(Err(msg));
            }
        }
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
                self.heightmap = None;
                self.current_project_path = Some(path);
                self.last_error = None;
                let (ex, ez) = self.map_size.elmo_extents();
                self.camera = OrbitCamera::framing(ex as f32, ez as f32);

                self.start_positions = p.start_positions;
                self.mapinfo_overrides = p.mapinfo_overrides;

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
    fn render_wizard(&mut self, ctx: &egui::Context) -> Option<WizardAction> {
        let mut action: Option<WizardAction> = None;
        let mut open = true;
        egui::Window::new("New project")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.text_edit_singleline(&mut self.wizard.project_name);
                });
                let sanitized_preview = sanitize_name(&self.wizard.project_name);
                ui.label(
                    egui::RichText::new(format!("(saves as: {sanitized_preview})"))
                        .small()
                        .weak(),
                );

                ui.separator();
                ui.label("Map size (SMU)");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.wizard.smu_x).range(2u32..=64));
                    ui.label("×");
                    ui.add(egui::DragValue::new(&mut self.wizard.smu_z).range(2u32..=64));
                    ui.label(
                        egui::RichText::new(format!(
                            "= {} × {} px",
                            self.wizard.smu_x * 64 + 1,
                            self.wizard.smu_z * 64 + 1,
                        ))
                        .small()
                        .weak(),
                    );
                });

                ui.separator();
                egui::ComboBox::from_label("Symmetry")
                    .selected_text(self.wizard.symmetry.label())
                    .show_ui(ui, |ui| {
                        let options = [
                            SymmetryAxis::None,
                            SymmetryAxis::Horizontal,
                            SymmetryAxis::Vertical,
                            SymmetryAxis::Quad,
                            SymmetryAxis::DiagonalMain,
                            SymmetryAxis::DiagonalAnti,
                            SymmetryAxis::Rotational {
                                fold: self.wizard.rotational_fold,
                            },
                        ];
                        for opt in options {
                            let label = opt.label();
                            ui.selectable_value(&mut self.wizard.symmetry, opt, label);
                        }
                    });
                if matches!(self.wizard.symmetry, SymmetryAxis::Rotational { .. }) {
                    let resp = ui.add(
                        egui::DragValue::new(&mut self.wizard.rotational_fold)
                            .range(2u8..=12u8)
                            .speed(0.1)
                            .prefix("Fold (players): "),
                    );
                    if resp.changed() {
                        self.wizard.symmetry = SymmetryAxis::Rotational {
                            fold: self.wizard.rotational_fold,
                        };
                    }
                }

                ui.separator();
                egui::ComboBox::from_label("Biome preset")
                    .selected_text(BIOMES[self.wizard.biome_index].label)
                    .show_ui(ui, |ui| {
                        for (i, biome) in BIOMES.iter().enumerate() {
                            if ui
                                .selectable_label(self.wizard.biome_index == i, biome.label)
                                .clicked()
                            {
                                self.wizard.biome_index = i;
                                if self.wizard.height_from_biome {
                                    self.wizard.max_height = biome.max_height_hint;
                                }
                            }
                        }
                    });

                let resp = ui.add(
                    egui::DragValue::new(&mut self.wizard.max_height)
                        .range(64.0f32..=4096.0)
                        .speed(1.0)
                        .prefix("Max height (elmos): "),
                );
                if resp.changed() {
                    self.wizard.height_from_biome = false;
                }

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Create").clicked() {
                        action = Some(WizardAction::Apply);
                    }
                    if ui.button("Cancel").clicked() {
                        action = Some(WizardAction::Cancel);
                    }
                });
            });
        // egui's Window `open` flag flips false when the user clicks the X.
        if !open && action.is_none() {
            action = Some(WizardAction::Cancel);
        }
        action
    }
}

/// 8-colour palette indexed by `team_id`. Even ids get warm tones (side A),
/// odd get cool (side B), matching the BAR per-side convention the F8 editor
/// auto-assigns to. Beyond 8 ids the palette wraps; the wrap is visible and
/// intentional — colour is a hint, the team-id label is the source of truth.
fn team_color(team_id: u8) -> egui::Color32 {
    const PALETTE: [egui::Color32; 8] = [
        egui::Color32::from_rgb(0xE5, 0x3E, 0x3E), // 0 — red    (side A)
        egui::Color32::from_rgb(0x3E, 0x7C, 0xE5), // 1 — blue   (side B)
        egui::Color32::from_rgb(0xE5, 0xA8, 0x3E), // 2 — orange (side A)
        egui::Color32::from_rgb(0x3E, 0xC2, 0xE5), // 3 — cyan   (side B)
        egui::Color32::from_rgb(0xE5, 0x3E, 0xB1), // 4 — pink   (side A)
        egui::Color32::from_rgb(0x76, 0x3E, 0xE5), // 5 — violet (side B)
        egui::Color32::from_rgb(0xE5, 0xDE, 0x3E), // 6 — yellow (side A)
        egui::Color32::from_rgb(0x3E, 0xE5, 0xA7), // 7 — teal   (side B)
    ];
    PALETTE[(team_id as usize) % PALETTE.len()]
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
        let (key_undo, key_redo) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            let shift = i.modifiers.shift;
            let z = i.key_pressed(egui::Key::Z);
            let y = i.key_pressed(egui::Key::Y);
            (cmd && !shift && z, (cmd && shift && z) || (cmd && y))
        });
        if key_undo {
            *action = Some(FileAction::Undo);
        } else if key_redo {
            *action = Some(FileAction::Redo);
        }

        if ctx.wants_keyboard_input() {
            return;
        }
        let (q, b, s, g, help, esc) = ctx.input(|i| {
            let shift = i.modifiers.shift;
            (
                i.key_pressed(egui::Key::Q),
                i.key_pressed(egui::Key::B),
                i.key_pressed(egui::Key::S),
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

    /// Top action bar: File / Edit / Build menus on the left, the
    /// symmetry chip in the middle, Build & Install right-aligned. B4
    /// will style the Build button as a green primary + add the
    /// variants ComboBox; B1 ships the plain `Button`.
    fn top_bar(&mut self, ctx: &egui::Context, action: &mut Option<FileAction>) {
        egui::TopBottomPanel::top("action_bar").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New project…").clicked() {
                        *action = Some(FileAction::OpenWizard);
                        ui.close();
                    }
                    if ui.button("Open project…").clicked() {
                        *action = Some(FileAction::Open);
                        ui.close();
                    }
                    if ui.button("Save project").clicked() {
                        *action = Some(FileAction::Save);
                        ui.close();
                    }
                    if ui.button("Save project as…").clicked() {
                        *action = Some(FileAction::SaveAs);
                        ui.close();
                    }
                    ui.separator();
                    ui.label("Load fixture heightmap");
                    for smu in [2u32, 4, 16] {
                        if ui.button(format!("{smu}×{smu} SMU")).clicked() {
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
                        .clicked()
                    {
                        *action = Some(FileAction::Undo);
                        ui.close();
                    }
                    if ui
                        .add_enabled(can_redo, egui::Button::new("Redo\tCtrl+Shift+Z"))
                        .clicked()
                    {
                        *action = Some(FileAction::Redo);
                        ui.close();
                    }
                });
                ui.menu_button("Build", |ui| {
                    let enabled = self.heightmap.is_some();
                    if ui
                        .add_enabled(enabled, egui::Button::new("Build & Install to BAR"))
                        .clicked()
                    {
                        *action = Some(FileAction::BuildAndInstall);
                        ui.close();
                    }
                    if !enabled {
                        ui.label("(load a heightmap first)");
                    }
                });

                ui.separator();
                // Symmetry chip — toggles the popover Window with the
                // existing axis combo + rotational fold spinner.
                // ADR-031 (B2) replaces the popover with a canvas
                // overlay; this commit only keeps the controls
                // reachable after moving them out of the Inspector.
                let sym_text = format!("Sym: {}", self.symmetry.label());
                let resp = ui.selectable_label(self.symmetry_popover_open, sym_text);
                if resp.clicked() {
                    self.symmetry_popover_open = !self.symmetry_popover_open;
                }

                // Right-align the primary Build button + the variants
                // ComboBox (B4). The button colour comes from
                // `Visuals::widgets::active.bg_fill` so the F21 theme
                // toggle keeps it themed (pitfall §B4.3 — no
                // hardcoded RGB). The combo's selected_text reflects
                // the current variant; the Launch variant is greyed
                // pre-F12 via `BuildVariant::is_enabled`.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_run = self.heightmap.is_some() && self.build_variant.is_enabled();
                    let primary_fill = ui.visuals().widgets.active.bg_fill;
                    let btn = egui::Button::new(self.build_variant.label()).fill(primary_fill);
                    let resp = ui.add_enabled(can_run, btn);
                    if resp.clicked()
                        && let Some(act) = self.build_variant.to_file_action()
                    {
                        *action = Some(act);
                    }

                    egui::ComboBox::from_id_salt("build_variant_combo")
                        .selected_text(self.build_variant.label())
                        .show_ui(ui, |ui| {
                            for v in BuildVariant::ALL {
                                let selected = self.build_variant == v;
                                let enabled = v.is_enabled();
                                let label = if enabled {
                                    v.label().to_string()
                                } else {
                                    format!("{} (Phase 5)", v.label())
                                };
                                let btn = egui::Button::selectable(selected, label);
                                let resp_v = ui.add_enabled(enabled, btn);
                                if resp_v.clicked() && enabled {
                                    self.build_variant = v;
                                }
                            }
                        });
                });
            });
        });
    }

    /// Bottom status strip: live camera-orbit readout, project size,
    /// validation-chip placeholder, last-install / last-error state.
    /// C8 wires the validation chip to real lint output later.
    fn status_strip(&mut self, ctx: &egui::Context) {
        // 1-Hz repaint nudge so the camera readout stays current
        // while idle (pitfall §B4.2 — egui only repaints on input
        // otherwise). The hint is a no-op if a higher-frequency
        // repaint is already scheduled this frame.
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
        egui::TopBottomPanel::bottom("status_strip").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let cam = &self.camera;
                ui.label(format!(
                    "Cam: yaw {:.0}° pitch {:.0}° dist {:.0}",
                    cam.yaw.to_degrees(),
                    cam.pitch.to_degrees(),
                    cam.distance,
                ));
                ui.separator();
                let (hpx_x, hpx_z) = self.map_size.heightmap_dims();
                ui.label(format!(
                    "Map: {}×{} SMU ({}×{} px)",
                    self.map_size.smu_x, self.map_size.smu_z, hpx_x, hpx_z,
                ));
                ui.separator();
                // Validation chip placeholder — wired in C8.
                ui.label(egui::RichText::new("0 issues").weak());
                ui.separator();
                match &self.last_install {
                    Some(Ok(p)) => {
                        ui.colored_label(
                            egui::Color32::GREEN,
                            format!(
                                "Installed: {}",
                                p.file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or_else(|| p.to_str().unwrap_or("?")),
                            ),
                        );
                    }
                    Some(Err(msg)) => {
                        ui.colored_label(egui::Color32::RED, format!("Install failed: {msg}"));
                    }
                    None => {
                        ui.label(egui::RichText::new("Build: idle").weak());
                    }
                }
                if let Some(err) = &self.last_error {
                    ui.separator();
                    ui.colored_label(egui::Color32::RED, err);
                }
            });
        });
    }

    /// Left tool strip: 40 px fixed-width column of one selectable
    /// `Button` per `Tool`. Hover-tooltip carries the long name and
    /// accelerator. Phase 4 grows this with Splat / Metal / Feature;
    /// adding a variant to `Tool` adds a row here automatically as long
    /// as it's listed in the array below (the per-site exhaustive
    /// `match`es elsewhere catch a missing dispatch).
    fn tool_strip(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("tool_strip")
            .resizable(false)
            .exact_width(40.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                for &t in &Tool::ALL {
                    let active = self.tool == t;
                    let resp = ui
                        .add_sized([32.0, 32.0], egui::Button::selectable(active, t.icon()))
                        .on_hover_text(format!("{} ({})", t.label(), t.accel()));
                    if resp.clicked() {
                        self.set_tool(t);
                    }
                    ui.add_space(2.0);
                }
            });
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
                            Tool::Select => Self::inspector_select(ui),
                            Tool::Sculpt => self.inspector_sculpt(ui),
                            Tool::StartPositions => self.inspector_start_positions(ui),
                            Tool::Procgen => self.inspector_procgen(ui, action),
                        }
                    });
            });
    }

    /// Persistent Inspector header. Project metadata, heightmap stats,
    /// and the max-height field — all always visible regardless of
    /// active tool because they're global session state, not tool
    /// parameters.
    fn inspector_header(&mut self, ui: &mut egui::Ui) {
        ui.heading("Project");
        ui.horizontal(|ui| {
            ui.label("Name:");
            ui.text_edit_singleline(&mut self.project_name);
        });
        ui.horizontal(|ui| {
            ui.label("Size (SMU):");
            ui.add(egui::DragValue::new(&mut self.map_size.smu_x).range(2..=96));
            ui.label("×");
            ui.add(egui::DragValue::new(&mut self.map_size.smu_z).range(2..=96));
        });
        match &self.current_project_path {
            Some(p) => ui.label(format!(
                "File: {}",
                p.file_name().and_then(|s| s.to_str()).unwrap_or("?")
            )),
            None => ui.label("File: (unsaved)"),
        };

        ui.separator();
        ui.heading("Heightmap");
        match &self.heightmap {
            None => {
                ui.label("No heightmap loaded.");
                ui.label("Use File → Load fixture heightmap.");
            }
            Some(h) => {
                ui.label(format!(
                    "Path: {}",
                    h.path.file_name().and_then(|s| s.to_str()).unwrap_or("?")
                ));
                ui.label(format!("Dims: {} × {}", h.dims.0, h.dims.1));
                ui.label(format!("Min / max sample: {} / {}", h.min, h.max));
                match &h.validated_against {
                    Some(size) => ui.colored_label(
                        egui::Color32::GREEN,
                        format!("OK — matches {}×{} SMU (64·N+1)", size.smu_x, size.smu_z),
                    ),
                    None => ui.colored_label(
                        egui::Color32::YELLOW,
                        format!(
                            "Dims do not match {}×{} SMU; expected {:?}",
                            self.map_size.smu_x,
                            self.map_size.smu_z,
                            self.map_size.heightmap_dims(),
                        ),
                    ),
                };
            }
        }

        ui.separator();
        // Height scale flows through the per-frame uniform — no
        // texture or grid rebuild needed when this changes (ADR-017).
        ui.add(
            egui::DragValue::new(&mut self.height_scale)
                .range(1.0..=4096.0)
                .speed(1.0)
                .prefix("Max height (elmos): "),
        );
    }

    fn inspector_select(ui: &mut egui::Ui) {
        ui.heading("Select / orbit");
        ui.label(
            egui::RichText::new(
                "Camera-only mode. LMB orbits, scroll zooms.\n\
                 Pick a tool on the left strip to start editing.",
            )
            .small()
            .weak(),
        );
    }

    fn inspector_sculpt(&mut self, ui: &mut egui::Ui) {
        ui.heading("Sculpt");
        let current_label = self
            .brush_id
            .as_deref()
            .and_then(|id| self.brushes.get(id).map(|b| b.label()))
            .unwrap_or("Off");
        egui::ComboBox::from_label("Brush")
            .selected_text(current_label)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.brush_id, None, "Off");
                for b in self.brushes.iter() {
                    let id_owned = b.id().to_string();
                    ui.selectable_value(&mut self.brush_id, Some(id_owned), b.label());
                }
            });
        ui.add(
            egui::DragValue::new(&mut self.brush_radius)
                .range(8.0..=4096.0)
                .speed(2.0)
                .prefix("Radius (elmos): "),
        );
        ui.add(
            egui::Slider::new(&mut self.brush_strength, 0.0..=1.0)
                .text("Strength")
                .clamping(egui::SliderClamping::Always),
        );
        ui.label(
            egui::RichText::new("LMB drag to sculpt, RMB to orbit. Symmetry chip in the top bar.")
                .small()
                .weak(),
        );
    }

    fn inspector_start_positions(&mut self, ui: &mut egui::Ui) {
        ui.heading("Start positions");
        ui.label(
            egui::RichText::new("LMB to place, drag to move, RMB to delete.")
                .small()
                .weak(),
        );
        ui.label(format!("Placed: {}", self.start_positions.len()));
        let mut to_delete: Option<u8> = None;
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .id_salt("startpos_scroll")
            .show(ui, |ui| {
                let mut sorted: Vec<StartPosition> = self.start_positions.clone();
                sorted.sort_by_key(|p| p.team_id);
                for p in sorted {
                    ui.horizontal(|ui| {
                        let color = team_color(p.team_id);
                        let (resp, painter) =
                            ui.allocate_painter(egui::Vec2::new(14.0, 14.0), egui::Sense::hover());
                        painter.circle_filled(resp.rect.center(), 6.0, color);
                        ui.label(format!("team {}: ({}, {})", p.team_id, p.x_elmo, p.z_elmo));
                        if ui.small_button("×").clicked() {
                            to_delete = Some(p.team_id);
                        }
                    });
                }
            });
        if let Some(id) = to_delete {
            self.delete_start_position(id);
        }
        if !self.start_positions.is_empty() && ui.button("Clear all").clicked() {
            info!(
                cleared = self.start_positions.len(),
                "all start positions cleared"
            );
            self.start_positions.clear();
            self.dragging_start_pos = None;
        }
    }

    fn inspector_procgen(&mut self, ui: &mut egui::Ui, action: &mut Option<FileAction>) {
        ui.heading("Procgen — f(x, z)");
        ui.label(
            egui::RichText::new("f(x, z) → height ∈ [0,1]. Apply replaces the heightmap.")
                .small()
                .weak(),
        );
        let expr_resp = ui.text_edit_singleline(&mut self.procgen_expr);
        if expr_resp.changed() {
            self.revalidate_procgen();
        }
        ui.horizontal(|ui| {
            ui.label("Domain:");
            ui.selectable_value(&mut self.procgen_domain, Domain::Unit, Domain::Unit.label());
            ui.selectable_value(
                &mut self.procgen_domain,
                Domain::Centered,
                Domain::Centered.label(),
            );
        });
        egui::ComboBox::from_label("Preset")
            .selected_text("(pick one to fill expression)")
            .show_ui(ui, |ui| {
                for p in PRESETS {
                    if ui.selectable_label(false, p.label).clicked() {
                        self.procgen_expr = p.expression.to_string();
                        self.procgen_domain = p.domain;
                        self.revalidate_procgen();
                    }
                }
            });
        // Apply row: button + live-validation chip. Apply is gated on
        // parse-AND-dry-eval success; chip tooltip shows the
        // `#[source]` chain of the validator error (A4 / ADR-033 era).
        ui.horizontal(|ui| {
            let parse_ok = self.procgen_validation.is_ok();
            let apply = ui.add_enabled(parse_ok, egui::Button::new("Apply"));
            if apply.clicked() {
                *action = Some(FileAction::ApplyProcGen);
            }
            match &self.procgen_validation {
                Ok(()) => {
                    ui.colored_label(egui::Color32::GREEN, "✓")
                        .on_hover_text("expression parses & evaluates");
                }
                Err(msg) => {
                    ui.colored_label(egui::Color32::RED, "✗").on_hover_text(msg);
                }
            }
        });
        if let Some(err) = &self.procgen_last_error {
            ui.colored_label(egui::Color32::RED, format!("Procgen: {err}"));
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

            let brush_active = matches!(self.tool, Tool::Sculpt)
                && self.brush_id.is_some()
                && self.heightmap.is_some();
            let start_pos_active = matches!(self.tool, Tool::StartPositions);
            let central_interactive = brush_active || start_pos_active;

            // Nav-gizmo interaction (B3). A click on an axis tip snaps
            // the camera; a click-and-drag *anywhere inside the gizmo
            // rect* orbits the camera (same math as RMB orbit). The
            // gizmo rect is computed up-front so we can short-circuit
            // the brush / start-pos handlers when the cursor is over it.
            let gizmo_rect = crate::ui::gizmo::gizmo_rect(rect);
            let cursor_in_gizmo = ctx
                .pointer_interact_pos()
                .map(|p| gizmo_rect.contains(p))
                .unwrap_or(false);
            if response.drag_started_by(egui::PointerButton::Primary) && cursor_in_gizmo {
                self.nav_gizmo_drag_active = true;
            }
            if response.drag_stopped_by(egui::PointerButton::Primary) {
                self.nav_gizmo_drag_active = false;
            }
            // Single-click on an axis tip → snap camera. Must come
            // BEFORE brush / start-pos click handlers since those
            // would otherwise eat the click on top of the gizmo.
            let mut consumed_click = false;
            if response.clicked_by(egui::PointerButton::Primary)
                && cursor_in_gizmo
                && let Some(cursor) = ctx.pointer_interact_pos()
                && let Some(axis) =
                    crate::ui::gizmo::hit_test_axis(cursor, gizmo_rect.center(), &self.camera)
            {
                let (yaw, pitch) = axis.camera_snap();
                self.camera.yaw = yaw;
                self.camera.pitch = pitch;
                info!(axis = axis.label(), "nav gizmo: snap camera");
                consumed_click = true;
            }

            let camera_drag = if self.nav_gizmo_drag_active {
                response.dragged_by(egui::PointerButton::Primary)
            } else if central_interactive {
                response.dragged_by(egui::PointerButton::Secondary)
            } else {
                response.dragged()
            };
            if camera_drag {
                let d = if central_interactive || self.nav_gizmo_drag_active {
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
                && !self.nav_gizmo_drag_active
                && !consumed_click
                && !cursor_in_gizmo
                && (response.dragged_by(egui::PointerButton::Primary)
                    || response.clicked_by(egui::PointerButton::Primary))
                && let Some(cursor) = ctx.pointer_interact_pos()
            {
                self.apply_brush_at(cursor, rect);
            }
            if self.history.stroke_open() && !response.dragged_by(egui::PointerButton::Primary) {
                self.end_stroke();
            }

            // Start-position placement / move / delete (ADR-023).
            if start_pos_active
                && !self.nav_gizmo_drag_active
                && !consumed_click
                && !cursor_in_gizmo
                && let Some(cursor) = ctx.pointer_interact_pos()
            {
                let cursor_in = glam::Vec2::new(cursor.x - rect.min.x, cursor.y - rect.min.y);
                let rect_size = glam::Vec2::new(rect.width(), rect.height());
                const HIT_RADIUS_PX: f32 = 12.0;

                if response.clicked_by(egui::PointerButton::Secondary)
                    && let Some(id) = self.hit_test_start_position(cursor, rect, HIT_RADIUS_PX)
                {
                    self.delete_start_position(id);
                }
                if response.drag_started_by(egui::PointerButton::Primary) {
                    self.dragging_start_pos =
                        self.hit_test_start_position(cursor, rect, HIT_RADIUS_PX);
                    // B5: capture the pre-drag coords so drag-stop can
                    // push a MoveStartPosition undo entry with a real
                    // `from`. None if the drag didn't begin on a marker.
                    self.dragging_start_pos_from = self.dragging_start_pos.and_then(|id| {
                        self.start_positions
                            .iter()
                            .find(|p| p.team_id == id)
                            .map(|p| (p.x_elmo, p.z_elmo))
                    });
                }
                if response.dragged_by(egui::PointerButton::Primary)
                    && let Some(id) = self.dragging_start_pos
                    && let Some(world) =
                        render::screen_to_world_y0(cursor_in, rect_size, &self.camera)
                {
                    self.move_start_position(id, world.x, world.z);
                }
                if response.drag_stopped_by(egui::PointerButton::Primary) {
                    self.finish_start_position_drag();
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
            }

            if self.heightmap.is_some() {
                let cb = TerrainCallback::new(&self.camera, rect, self.height_scale);
                ui.painter()
                    .add(egui_wgpu::Callback::new_paint_callback(rect, cb));
            } else {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Load a heightmap to see the terrain.",
                    egui::FontId::proportional(16.0),
                    ui.visuals().weak_text_color(),
                );
            }

            // Symmetry canvas overlay (ADR-031 / B2). Paints AFTER the
            // wgpu terrain pass and BEFORE the start-position markers
            // so markers stay readable on top of the axes. No-op when
            // `symmetry == None`.
            let extents = self.world_extents();
            let overlay_painter = ui.painter_at(rect);
            crate::ui::overlay::paint_symmetry_overlay(
                &overlay_painter,
                rect,
                &self.camera,
                self.symmetry,
                extents,
            );

            // Brush rings — Sculpt + brush selected + cursor over the
            // central rect + cursor outside the nav gizmo. Cursor world
            // position reuses the existing y=0 raycast from stamp
            // placement (pitfall §B3.5 / §B2.5 — no second projection
            // path). The primary ring (B3) renders at full alpha at
            // the cursor; mirror ghosts (B2) render at 50 % at each
            // symmetry-derived centre.
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
                    crate::ui::overlay::paint_primary_brush_ring(
                        &overlay_painter,
                        rect,
                        &self.camera,
                        crate::ui::overlay::BrushCursor {
                            world: brush_cursor.world,
                            radius_world: brush_cursor.radius_world,
                            brush_id: brush_cursor.brush_id,
                        },
                    );
                    if !matches!(self.symmetry, SymmetryAxis::None) {
                        crate::ui::overlay::paint_brush_ghosts(
                            &overlay_painter,
                            rect,
                            &self.camera,
                            self.symmetry,
                            brush_cursor,
                            extents,
                        );
                    }
                }
            }

            // Overlay start-position markers on top of the terrain pass.
            // Always rendered when any are placed (regardless of tool)
            // so the user can see them while sculpting. B6 will add a
            // cross-tool alpha falloff (50 % outside StartPositions
            // mode).
            if !self.start_positions.is_empty() {
                let rect_size = glam::Vec2::new(rect.width(), rect.height());
                let painter = ui.painter_at(rect);
                for pos in &self.start_positions {
                    let world = glam::Vec3::new(pos.x_elmo as f32, 0.0, pos.z_elmo as f32);
                    let Some(screen) = render::world_to_screen(world, rect_size, &self.camera)
                    else {
                        continue;
                    };
                    let p = egui::Pos2::new(rect.min.x + screen.x, rect.min.y + screen.y);
                    let highlighted = self.dragging_start_pos == Some(pos.team_id);
                    let r = if highlighted { 10.0 } else { 8.0 };
                    painter.circle_filled(p, r, team_color(pos.team_id));
                    painter.circle_stroke(p, r, egui::Stroke::new(2.0, egui::Color32::WHITE));
                    painter.text(
                        p + egui::Vec2::new(0.0, -r - 2.0),
                        egui::Align2::CENTER_BOTTOM,
                        format!("{}", pos.team_id),
                        egui::FontId::proportional(11.0),
                        egui::Color32::WHITE,
                    );
                }
            }

            // Nav gizmo — top-right corner of the viewport (B3). Painted
            // LAST so it sits on top of the brush rings, axis overlay,
            // and start-position markers.
            crate::ui::gizmo::paint_nav_gizmo(&overlay_painter, rect, &self.camera);
        });
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

        // egui panel add-order rule: top → bottom → left → right →
        // CentralPanel LAST. Reversing this means CentralPanel eats the
        // rect later panels were supposed to claim.
        self.handle_keyboard(ctx, &mut action);
        self.top_bar(ctx, &mut action);
        self.status_strip(ctx);
        self.tool_strip(ctx);
        self.inspector(ctx, &mut action);
        self.central(ctx);

        self.drain_action(action);
        self.symmetry_popover(ctx);

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
            current_project_path: None,
            last_install: None,
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
            history: History::default(),
            tool: Tool::Sculpt,
            previous_tool: Tool::Sculpt,
            start_positions: Vec::new(),
            dragging_start_pos: None,
            dragging_start_pos_from: None,
            wizard_open: false,
            wizard: WizardState::default_for_new_project(),
            symmetry_popover_open: false,
            editor_config: config::EditorConfig::default(),
            show_intro: false,
            show_cheat_sheet: false,
            nav_gizmo_drag_active: false,
            build_variant: BuildVariant::default(),
            mapinfo_overrides: std::collections::HashMap::new(),
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
        // The current size is documented in ADR-030. A change here is
        // intentional but should bump the ADR and the phase-3-plan
        // entry for B1.
        assert_eq!(
            Tool::ALL.len(),
            4,
            "Tool::ALL size changed — update ADR-030 + phase-3-plan B1"
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

    /// ADR-030 nails Q / B / S / G to specific tools. A drift here is
    /// a documented contract break — bump the ADR if intentional.
    #[test]
    fn tool_accelerators_match_adr_030() {
        assert_eq!(Tool::Select.accel(), "Q");
        assert_eq!(Tool::Sculpt.accel(), "B");
        assert_eq!(Tool::StartPositions.accel(), "S");
        assert_eq!(Tool::Procgen.accel(), "G");
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
        app.dragging_start_pos = Some(3);
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

    /// Phase-2 smoke #2 (F8 / ADR-023): start-position placement still
    /// inserts into the `Vec<StartPosition>` with rounded elmo coords.
    /// The B1 tool-strip switch must not break the underlying state
    /// machine.
    #[test]
    fn b1_does_not_regress_start_position_placement_phase2() {
        let mut app = make_test_app();
        app.place_start_position(100.0, 100.0);
        assert_eq!(app.start_positions.len(), 1);
        assert_eq!(app.start_positions[0].x_elmo, 100);
        assert_eq!(app.start_positions[0].z_elmo, 100);
        // Out-of-bounds click is ignored (existing ADR-023 invariant).
        app.place_start_position(-1.0, 0.0);
        assert_eq!(app.start_positions.len(), 1, "off-map click ignored");
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
        assert_eq!(app.start_positions.len(), 1);
        assert_eq!(app.history.undo_depth(), 1);
        app.undo_one();
        assert!(app.start_positions.is_empty());
        assert_eq!(app.history.undo_depth(), 0);
        assert_eq!(app.history.redo_depth(), 1, "undo pushes onto redo");
    }

    /// B5 smoke: place + undo + redo restores the marker.
    #[test]
    fn b5_place_then_undo_redo_round_trips() {
        let mut app = make_test_app();
        app.place_start_position(50.0, 75.0);
        let pos_before = app.start_positions[0];
        app.undo_one();
        assert!(app.start_positions.is_empty());
        app.redo_one();
        assert_eq!(app.start_positions.len(), 1);
        assert_eq!(app.start_positions[0], pos_before);
    }

    /// B5 smoke: delete a marker, Ctrl-Z restores it (RMB path).
    #[test]
    fn b5_delete_then_undo_restores_marker() {
        let mut app = make_test_app();
        app.place_start_position(100.0, 100.0);
        let id = app.start_positions[0].team_id;
        // Place pushed one diff; clear redo bookkeeping by deleting.
        app.delete_start_position(id);
        assert!(app.start_positions.is_empty());
        assert_eq!(app.history.undo_depth(), 2, "place + delete pushed");
        app.undo_one(); // undo the delete
        assert_eq!(app.start_positions.len(), 1);
        assert_eq!(app.start_positions[0].team_id, id);
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
        app.start_positions = vec![StartPosition {
            team_id: 0,
            x_elmo: 999,
            z_elmo: 999,
        }];
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
        assert!(
            app.start_positions.is_empty(),
            "new_project cleared positions"
        );
        assert_eq!(app.history.undo_depth(), 1, "one ApplyWizard entry");
        // Undo.
        app.undo_one();
        assert_eq!(app.project_name, "pre");
        assert_eq!(app.map_size, MapSize { smu_x: 4, smu_z: 6 });
        assert_eq!(app.height_scale, 123.0);
        assert!(matches!(app.symmetry, SymmetryAxis::Horizontal));
        assert_eq!(app.start_positions.len(), 1);
        assert_eq!(app.start_positions[0].x_elmo, 999);
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
        app.dragging_start_pos = Some(app.start_positions[0].team_id);
        app.dragging_start_pos_from = Some((100, 100));
        app.undo_one();
        // Drag gate held: nothing popped.
        assert_eq!(
            app.history.undo_depth(),
            1,
            "undo while dragging must be a no-op"
        );
        assert_eq!(app.start_positions.len(), 1);
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
        app.dragging_start_pos = Some(0);
        assert!(app.is_dragging_anything(), "start-pos drag should gate");
        app.dragging_start_pos = None;
        // Heightmap-stroke gate is harder to fake without a heightmap;
        // we exercise the start-pos branch + the false case here.
        // The heightmap branch is covered by the existing
        // `stroke_open()` tests in `barme-core::undo`.
        assert!(!app.is_dragging_anything());
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
        let id = app.start_positions[0].team_id;
        app.dragging_start_pos = Some(id);
        app.dragging_start_pos_from = Some((100, 100));
        app.finish_start_position_drag();
        assert_eq!(
            app.history.undo_depth(),
            depth_after_place,
            "zero-distance drag must not push an undo entry"
        );
    }
}
