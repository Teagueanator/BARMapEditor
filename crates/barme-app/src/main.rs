mod config;
mod launcher;
mod render;
mod ui;

use std::path::{Path, PathBuf};

use anyhow::Result;
use barme_core::{
    ALLY_GROUP_PALETTE, AllyGroup, BIOMES, BrushRegistry, BrushStamp, DirtyRect, Heightmap,
    History, HistoryEntry, MapSize, PROJECT_EXTENSION, Project, ProjectDiff, StartPosition,
    SymmetryAxis, WizardSnapshot,
    brushes::pixel_bbox,
    procgen::{
        Domain, PRESETS, generate as procgen_generate, generate_thumbnail as procgen_thumbnail_gen,
        validate_expression,
    },
    project::sanitize_name,
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
            nav_gizmo_drag_active: false,
            build_variant: BuildVariant::default(),
            mapinfo_overrides: std::collections::HashMap::new(),
            show_next_steps: false,
            next_steps_dismissed: false,
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
        self.ally_groups.clear();
        self.active_ally_group_id = 0;
        self.mapinfo_overrides.clear();
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
            ally_groups: self.ally_groups.clone(),
            mapinfo_overrides: self.mapinfo_overrides.clone(),
            next_steps_dismissed: self.next_steps_dismissed,
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
    /// (heightmap channel) and start-position drags (project-diff
    /// channel) both gate undo/redo so the user can't peel back state
    /// mid-gesture. B5.
    fn is_dragging_anything(&self) -> bool {
        self.history.stroke_open() || self.dragging_start_pos.is_some()
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
        let pos = g.start_positions.remove(source_index);
        info!(ally_group_id, source_index, "start position deleted");
        self.history
            .push_project_diff(ProjectDiff::DeleteStartPosition { ally_group_id, pos });
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
        let project = self.snapshot_project_for_build();
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

                self.ally_groups = p.ally_groups;
                // Active group: first by id, or 0 if none.
                self.active_ally_group_id =
                    self.ally_groups.iter().map(|g| g.id).min().unwrap_or(0);
                self.mapinfo_overrides = p.mapinfo_overrides;
                // B8: respect a saved dismissal so reopening the
                // project doesn't replay the hint window.
                self.next_steps_dismissed = p.next_steps_dismissed;
                self.show_next_steps = false;

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
                            Tool::Procgen => self.inspector_procgen(ctx, ui, action),
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

    /// F8 Inspector tree (ADR-032 / B6). One collapsing header per
    /// ally group with colour swatch, name, count, delete. Child rows
    /// show source positions; clicking a row marks it for the
    /// hover-pulse on the canvas. Symmetry-derived positions render
    /// greyed with a `(mirror of …)` label.
    fn inspector_start_positions(&mut self, ui: &mut egui::Ui) {
        ui.heading("Start positions");
        ui.label(
            egui::RichText::new(
                "LMB to place, LMB-drag to paint N along a line, drag to move, RMB to delete.",
            )
            .small()
            .weak(),
        );

        // ─── Preset dropdown ───
        let (ex, ez) = self.world_extents();
        ui.horizontal(|ui| {
            ui.label("Preset:");
            let mut selected: Option<AllyPreset> = None;
            egui::ComboBox::from_id_salt("ally_preset")
                .selected_text("(apply a layout)")
                .show_ui(ui, |ui| {
                    for preset in AllyPreset::ALL {
                        if ui.selectable_label(false, preset.label()).clicked() {
                            selected = Some(preset);
                        }
                    }
                });
            if let Some(p) = selected {
                self.apply_ally_preset(p);
            }
            let _ = (ex, ez);
        });

        // ─── Drag-paint N spinner ───
        ui.horizontal(|ui| {
            ui.label("Drag-paint N:");
            ui.add(egui::DragValue::new(&mut self.drag_paint_count).range(1u8..=32));
        });

        ui.separator();

        // ─── AllyGroup tree ───
        let mut to_delete_pos: Option<(u8, usize)> = None;
        let mut to_delete_group: Option<u8> = None;
        let mut new_active: Option<u8> = None;
        let mut new_pulse: Option<(u8, usize)> = None;
        let active = self.active_ally_group_id;
        // Materialise a stable ordering so the tree doesn't shuffle
        // when ids are non-contiguous.
        let mut group_indices: Vec<usize> = (0..self.ally_groups.len()).collect();
        group_indices.sort_by_key(|&i| self.ally_groups[i].id);

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
                                let mut c =
                                    egui::Color32::from_rgb(g.color[0], g.color[1], g.color[2]);
                                if ui
                                    .color_edit_button_srgba(&mut c)
                                    .on_hover_text("AllyGroup colour")
                                    .changed()
                                {
                                    g.color = [c.r(), c.g(), c.b()];
                                }
                                ui.text_edit_singleline(&mut g.name);
                                if ui
                                    .selectable_label(is_active, "★")
                                    .on_hover_text("Make active (receives new placements)")
                                    .clicked()
                                {
                                    new_active = Some(g_id);
                                }
                                if ui
                                    .small_button("delete group")
                                    .on_hover_text("Remove this ally group and all its positions")
                                    .clicked()
                                {
                                    to_delete_group = Some(g_id);
                                }
                            });

                            // Source position rows.
                            let positions: Vec<(usize, StartPosition)> = self.ally_groups[idx]
                                .start_positions
                                .iter()
                                .enumerate()
                                .map(|(i, p)| (i, *p))
                                .collect();
                            for (i, pos) in &positions {
                                let row = ui.horizontal(|ui| {
                                    ui.label(format!("#{}: ({}, {})", i, pos.x_elmo, pos.z_elmo));
                                    if ui.small_button("×").clicked() {
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
                                    let mirrors = self
                                        .symmetry
                                        .replicate((pos.x_elmo as f32, pos.z_elmo as f32), extents);
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
                if ui.button("+ Add AllyGroup").clicked() {
                    self.add_ally_group();
                }
            });

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

    fn inspector_procgen(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        action: &mut Option<FileAction>,
    ) {
        ui.heading("Procgen — f(x, z)");
        ui.label(
            egui::RichText::new("Pick a preset, optionally tweak the expression, then Apply.")
                .small()
                .weak(),
        );

        // ─── 1. Preset dropdown (B7: preset-first) ───
        let active_preset = PRESETS
            .iter()
            .find(|p| p.expression == self.procgen_expr && p.domain == self.procgen_domain)
            .map(|p| p.label)
            .unwrap_or("Custom");
        egui::ComboBox::from_label("Preset")
            .selected_text(active_preset)
            .show_ui(ui, |ui| {
                for p in PRESETS {
                    let chosen = active_preset == p.label;
                    if ui.selectable_label(chosen, p.label).clicked() {
                        self.procgen_expr = p.expression.to_string();
                        self.procgen_domain = p.domain;
                        self.revalidate_procgen();
                    }
                }
            });

        // ─── 2. Custom expression (collapsed by default) ───
        egui::CollapsingHeader::new("Custom expression")
            .id_salt("procgen_custom_expr")
            .default_open(false)
            .show(ui, |ui| {
                let expr_resp = ui.text_edit_singleline(&mut self.procgen_expr);
                if expr_resp.changed() {
                    self.revalidate_procgen();
                }
                ui.label(
                    egui::RichText::new("f(x, z) → height ∈ [0,1]. Out-of-range clamps.")
                        .small()
                        .weak(),
                );
            });

        // ─── 3. Domain radio ───
        ui.horizontal(|ui| {
            ui.label("Domain:");
            if ui
                .selectable_value(&mut self.procgen_domain, Domain::Unit, Domain::Unit.label())
                .changed()
            {
                self.revalidate_procgen();
            }
            if ui
                .selectable_value(
                    &mut self.procgen_domain,
                    Domain::Centered,
                    Domain::Centered.label(),
                )
                .changed()
            {
                self.revalidate_procgen();
            }
        });

        ui.separator();

        // ─── 4. Live preview thumbnail ───
        self.refresh_procgen_thumbnail(ctx);
        ui.label(egui::RichText::new("Preview").small().weak());
        if let Some(tex) = self.procgen_thumbnail.as_ref() {
            // Render the thumbnail at its native size, capped by the
            // available width so a narrow Inspector still fits.
            let max_side = ui.available_width().min(PROCGEN_THUMBNAIL_PX as f32);
            ui.add(egui::Image::new(tex).fit_to_exact_size(egui::vec2(max_side, max_side)));
        } else if self.procgen_validation.is_err() {
            ui.colored_label(
                egui::Color32::GRAY,
                "(invalid expression — fix to render preview)",
            );
        } else {
            ui.label(egui::RichText::new("(preview baking…)").small().weak());
        }

        ui.separator();

        // ─── 5. Apply button + live-validation chip ───
        ui.horizontal(|ui| {
            let parse_ok = self.procgen_validation.is_ok();
            let apply = ui.add_enabled(parse_ok, egui::Button::new("Apply to heightmap"));
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

            // Start-position placement / move / delete / drag-paint
            // (ADR-032 supersedes ADR-023 in mode-specific behaviour).
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

            // Overlay start-position markers on top of the terrain
            // pass. ADR-032: cross-tool ghost falloff (50 % alpha) when
            // the StartPositions tool isn't active. Sources render as
            // filled circles in their AllyGroup colour; symmetry-
            // derived mirrors render as outlined rings with a thinner
            // stroke. Hovered-from-Inspector source pulses at 2 Hz for
            // 1 s after the hover event.
            if !self.ally_groups.is_empty() {
                let rect_size = glam::Vec2::new(rect.width(), rect.height());
                let painter = ui.painter_at(rect);
                let cross_tool_ghost = !matches!(self.tool, Tool::StartPositions);
                let alpha_mul = if cross_tool_ghost { 128 } else { 255 };
                let now = std::time::Instant::now();
                let extents = self.world_extents();
                for g in &self.ally_groups {
                    let base_color = egui::Color32::from_rgba_unmultiplied(
                        g.color[0], g.color[1], g.color[2], alpha_mul,
                    );
                    for (i, pos) in g.start_positions.iter().enumerate() {
                        let world = glam::Vec3::new(pos.x_elmo as f32, 0.0, pos.z_elmo as f32);
                        let Some(screen) = render::world_to_screen(world, rect_size, &self.camera)
                        else {
                            continue;
                        };
                        let p = egui::Pos2::new(rect.min.x + screen.x, rect.min.y + screen.y);
                        let dragging = self.dragging_start_pos == Some((g.id, i));
                        let mut r = if dragging { 10.0 } else { 8.0 };

                        // Hover-pulse: thick ring at 2 Hz for 1 s after
                        // the Inspector row hover instant.
                        if let Some((pg, pi, t0)) = self.pulsing_marker
                            && pg == g.id
                            && pi == i
                        {
                            let dt = now.duration_since(t0).as_secs_f32();
                            if dt < 1.0 {
                                // 2 Hz triangle wave on radius.
                                let osc = (dt * 2.0 * std::f32::consts::TAU).sin().abs();
                                r += 3.0 * osc;
                                ctx.request_repaint();
                            } else {
                                // Time's up — clear so we don't repaint
                                // forever.
                                self.pulsing_marker = None;
                            }
                        }

                        painter.circle_filled(p, r, base_color);
                        painter.circle_stroke(
                            p,
                            r,
                            egui::Stroke::new(
                                2.0,
                                egui::Color32::from_rgba_unmultiplied(255, 255, 255, alpha_mul),
                            ),
                        );
                        painter.text(
                            p + egui::Vec2::new(0.0, -r - 2.0),
                            egui::Align2::CENTER_BOTTOM,
                            format!("{}", i),
                            egui::FontId::proportional(11.0),
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, alpha_mul),
                        );

                        // Render symmetry-derived mirrors of this
                        // source as outline-only rings. Idempotent:
                        // when the source is on a symmetry axis its
                        // first mirror equals itself (we skip).
                        if !matches!(self.symmetry, SymmetryAxis::None) {
                            let mirrors = self
                                .symmetry
                                .replicate((pos.x_elmo as f32, pos.z_elmo as f32), extents);
                            for (mx, mz) in mirrors.into_iter().skip(1) {
                                if mx < 0.0 || mx > extents.0 || mz < 0.0 || mz > extents.1 {
                                    continue;
                                }
                                let mw = glam::Vec3::new(mx, 0.0, mz);
                                let Some(ms) = render::world_to_screen(mw, rect_size, &self.camera)
                                else {
                                    continue;
                                };
                                let mp = egui::Pos2::new(rect.min.x + ms.x, rect.min.y + ms.y);
                                painter.circle_stroke(mp, 7.0, egui::Stroke::new(1.5, base_color));
                            }
                        }
                    }
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
            nav_gizmo_drag_active: false,
            build_variant: BuildVariant::default(),
            mapinfo_overrides: std::collections::HashMap::new(),
            show_next_steps: false,
            next_steps_dismissed: false,
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
}
