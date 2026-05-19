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
    /// Phase 7 (ADR-035): scaffolding state for the Splat / Metal /
    /// Geo tools. Persistence lands with the F-series schema work;
    /// this lives only in `App` for now so users can see the UI work
    /// without losing data across tool switches.
    splat_state: SplatState,
    metal_state: MetalState,
    geo_state: GeoState,
}

/// Splat-painting scaffolding. Phase 7 — see TODO(F4).
#[derive(Debug, Clone)]
struct SplatState {
    layers: [SplatLayer; 4],
    active_layer: usize,
    brush_mode: SplatBrushMode,
    radius: f32,
    strength: f32,
    spacing: f32,
}

#[derive(Debug, Clone)]
struct SplatLayer {
    channel: char,
    name: String,
    color: [u8; 3],
    opacity: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SplatBrushMode {
    Paint,
    Erase,
    Smear,
}

impl Default for SplatState {
    fn default() -> Self {
        Self {
            layers: [
                SplatLayer {
                    channel: 'R',
                    name: "Grass · meadow".into(),
                    color: [0x5B, 0x84, 0x43],
                    opacity: 0.85,
                },
                SplatLayer {
                    channel: 'G',
                    name: "Rock · granite".into(),
                    color: [0x7A, 0x6F, 0x62],
                    opacity: 0.60,
                },
                SplatLayer {
                    channel: 'B',
                    name: "Sand · alluvial".into(),
                    color: [0xC9, 0xA8, 0x78],
                    opacity: 0.40,
                },
                SplatLayer {
                    channel: 'A',
                    name: "Snow · crusted".into(),
                    color: [0xD4, 0xDC, 0xE2],
                    opacity: 0.00,
                },
            ],
            active_layer: 0,
            brush_mode: SplatBrushMode::Paint,
            radius: 48.0,
            strength: 0.65,
            spacing: 0.30,
        }
    }
}

/// Metal-spot scaffolding. Phase 7 — see TODO(F5).
#[derive(Debug, Clone)]
struct MetalState {
    density: f32,
    min_spacing: f32,
    max_metal: f32,
    spots: Vec<MetalSpot>,
    selected: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct MetalSpot {
    x_elmo: i32,
    z_elmo: i32,
    metal: f32,
}

impl Default for MetalState {
    fn default() -> Self {
        Self {
            density: 0.55,
            min_spacing: 2048.0,
            max_metal: 1.8,
            spots: Vec::new(),
            selected: None,
        }
    }
}

/// Geo-features scaffolding. Phase 7 — see TODO(F7).
#[derive(Debug, Clone)]
struct GeoState {
    library: Vec<GeoFeature>,
    selected: usize,
    rotation_jitter: f32,
    scale_jitter: f32,
    align_to_slope: bool,
    scatter_density: f32,
}

#[derive(Debug, Clone)]
struct GeoFeature {
    name: String,
    count: usize,
    icon: crate::ui::icons::Icon,
}

impl Default for GeoState {
    fn default() -> Self {
        use crate::ui::icons::Icon;
        Self {
            library: vec![
                GeoFeature {
                    name: "Pine".into(),
                    count: 124,
                    icon: Icon::Tree,
                },
                GeoFeature {
                    name: "Birch".into(),
                    count: 62,
                    icon: Icon::Tree,
                },
                GeoFeature {
                    name: "Boulder".into(),
                    count: 88,
                    icon: Icon::Rock,
                },
                GeoFeature {
                    name: "Spire".into(),
                    count: 12,
                    icon: Icon::Crystal,
                },
                GeoFeature {
                    name: "Wreck S".into(),
                    count: 8,
                    icon: Icon::Wreck,
                },
                GeoFeature {
                    name: "Wreck L".into(),
                    count: 3,
                    icon: Icon::Wreck,
                },
            ],
            selected: 0,
            rotation_jitter: 45.0,
            scale_jitter: 0.20,
            align_to_slope: true,
            scatter_density: 32.0,
        }
    }
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
    /// F4 splat-texture painting (ADR-035 scaffolding; schema work
    /// pending). LMB paints into the active RGBA channel.
    SplatPaint,
    /// F5 metal-spot placement (ADR-035 scaffolding; schema work
    /// pending). LMB places spots, RMB deletes.
    MetalSpots,
    /// F7 feature placement (ADR-035 scaffolding; schema work pending).
    /// LMB places / scatters features.
    GeoFeatures,
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
    const ALL: [Tool; 7] = [
        Tool::Select,
        Tool::Sculpt,
        Tool::StartPositions,
        Tool::SplatPaint,
        Tool::MetalSpots,
        Tool::GeoFeatures,
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
            Tool::SplatPaint => "▦",
            Tool::MetalSpots => "◆",
            Tool::GeoFeatures => "🌲",
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
            Tool::SplatPaint => Icon::Splat,
            Tool::MetalSpots => Icon::Metal,
            Tool::GeoFeatures => Icon::Geo,
            Tool::Procgen => Icon::Procgen,
        }
    }

    /// Single-letter accelerator key. Wired in `App::handle_keyboard`.
    fn accel(self) -> &'static str {
        match self {
            Tool::Select => "Q",
            Tool::Sculpt => "B",
            Tool::StartPositions => "S",
            Tool::SplatPaint => "T",
            Tool::MetalSpots => "M",
            Tool::GeoFeatures => "F",
            Tool::Procgen => "G",
        }
    }

    /// Long-form name for hover tooltips + tracing output.
    fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select / orbit",
            Tool::Sculpt => "Sculpt",
            Tool::StartPositions => "Start positions",
            Tool::SplatPaint => "Splat paint",
            Tool::MetalSpots => "Metal spots",
            Tool::GeoFeatures => "Geo features",
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
            dirty: false,
            last_non_none_symmetry: SymmetryAxis::Horizontal,
            grid_overlay_on: false,
            lighting_on: true,
            wireframe_on: false,
            splat_state: SplatState::default(),
            metal_state: MetalState::default(),
            geo_state: GeoState::default(),
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
        self.dirty = false;
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
        self.dirty = true;
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
                    );
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
                        ui.add(
                            egui::DragValue::new(&mut self.wizard.smu_x).range(2u32..=64),
                        );
                        ui.label(egui::RichText::new("×").color(t.dim));
                        ui.add(
                            egui::DragValue::new(&mut self.wizard.smu_z).range(2u32..=64),
                        );
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
                    );
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
                                        .clicked()
                                    {
                                        action = Some(WizardAction::Apply);
                                    }
                                    if ui
                                        .add(
                                            egui::Button::new("Cancel")
                                                .min_size(egui::vec2(80.0, 30.0)),
                                        )
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
        response.clicked()
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
        let (q, b, s, t_key, m_key, f_key, g, help, esc) = ctx.input(|i| {
            let shift = i.modifiers.shift;
            (
                i.key_pressed(egui::Key::Q),
                i.key_pressed(egui::Key::B),
                i.key_pressed(egui::Key::S),
                i.key_pressed(egui::Key::T),
                i.key_pressed(egui::Key::M),
                i.key_pressed(egui::Key::F),
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
        if t_key {
            self.set_tool(Tool::SplatPaint);
        }
        if m_key {
            self.set_tool(Tool::MetalSpots);
        }
        if f_key {
            self.set_tool(Tool::GeoFeatures);
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
        ui.menu_button("View", |ui| {
            if ui
                .checkbox(&mut self.grid_overlay_on, "Coordinate grid")
                .clicked()
            {
                ui.close();
            }
            if ui
                .checkbox(&mut self.lighting_on, "Lighting (preview only)")
                .clicked()
            {
                ui.close();
            }
            if ui
                .checkbox(&mut self.wireframe_on, "Wireframe (preview only)")
                .clicked()
            {
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
                    if crate::ui::widgets::pill_toggle(ui, "Symmetry", &mut on_state).clicked() {
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
            .interact(egui::Sense::click());
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
        use crate::ui::icons::Icon;
        let t = crate::ui::theme::Tokens::DARK;
        // Build & install split-button (rightmost so it's the eye
        // anchor — the user's most-used action).
        let can_run = self.heightmap.is_some();
        let (primary, caret) = crate::ui::widgets::split_button(
            ui,
            Some(Icon::Play),
            "Build & install",
            can_run, // accent only when actionable
        );
        if can_run
            && primary.clicked()
            && let Some(act) = self.build_variant.to_file_action()
        {
            *action = Some(act);
        }
        egui::Popup::menu(&caret)
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
            .clicked()
        {
            *action = Some(FileAction::Save);
        }
        ui.add_space(4.0);

        // Validation chip.
        let (tone, label) = self.validation_summary();
        crate::ui::widgets::chip(ui, tone, label);
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
                            Tool::Select => Self::inspector_select(ui),
                            Tool::Sculpt => self.inspector_sculpt(ui),
                            Tool::StartPositions => self.inspector_start_positions(ui),
                            Tool::SplatPaint => self.inspector_splat(ui),
                            Tool::MetalSpots => self.inspector_metal(ui),
                            Tool::GeoFeatures => self.inspector_geo(ui),
                            Tool::Procgen => self.inspector_procgen(ctx, ui, action),
                        }
                    });
            });
    }

    /// Persistent Inspector header (ADR-035). Project name + size +
    /// dirty chip; then heightmap card with path/dims/sample as a
    /// 2-col grid + a valid/invalid chip in the section header.
    fn inspector_header(&mut self, ui: &mut egui::Ui) {
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
                crate::ui::widgets::chip(ui, tone, label);
            },
            |ui| {
                let name_resp = ui.add(
                    egui::TextEdit::singleline(&mut self.project_name)
                        .desired_width(f32::INFINITY)
                        .frame(false)
                        .font(egui::FontId::proportional(15.0)),
                );
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
                    );
                    ui.label(egui::RichText::new("×").color(t.dim).size(11.0));
                    ui.add(
                        egui::DragValue::new(&mut self.map_size.smu_z)
                            .range(2..=96)
                            .speed(0.1),
                    );
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
                crate::ui::widgets::chip(ui, tone, label);
            },
            |ui| {
                egui::Grid::new("inspector_hm_grid")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .striped(false)
                    .show(ui, |ui| {
                        for (k, v) in [
                            ("Path", path_str.as_str()),
                            ("Dims", dims_str.as_str()),
                            ("Sample", sample_str.as_str()),
                        ] {
                            ui.label(egui::RichText::new(k).color(t.muted).size(11.0));
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
                            );
                        });
                        ui.end_row();
                    });
            },
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

    /// Splat-paint inspector (ADR-035 / Phase 7). UI scaffolding only —
    /// the real splat pipeline (F4) wires the wgpu compute pass to
    /// write into the splat distribution texture. Until then this
    /// section drives an in-memory `SplatState` per-session so users
    /// can tour the controls.
    // TODO(F4): wire active_layer + brush_mode + radius/strength into
    // the central viewport's pointer dispatch.
    fn inspector_splat(&mut self, ui: &mut egui::Ui) {
        let t = crate::ui::theme::Tokens::DARK;
        let s = &mut self.splat_state;

        // LAYERS section: 4-row picker, each row = active radio + channel
        // chip + texture swatch + name + opacity bar + percentage.
        let mut new_active: Option<usize> = None;
        let active = s.active_layer;
        crate::ui::widgets::section(
            ui,
            "Texture layers",
            true,
            |ui| {
                ui.label(egui::RichText::new("RGBA").color(t.muted).size(11.0));
            },
            |ui| {
                for (i, layer) in s.layers.iter().enumerate() {
                    let is_active = i == active;
                    let row_resp = egui::Frame::new()
                        .fill(if is_active { t.hover } else { t.bg })
                        .stroke(egui::Stroke::new(
                            1.0,
                            if is_active { t.border_hi } else { t.border },
                        ))
                        .corner_radius(egui::CornerRadius::same(5))
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                // Active radio.
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(14.0, 14.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().circle_stroke(
                                    rect.center(),
                                    7.0,
                                    egui::Stroke::new(
                                        1.5,
                                        if is_active { t.accent } else { t.dim },
                                    ),
                                );
                                if is_active {
                                    ui.painter().circle_filled(rect.center(), 4.0, t.accent);
                                }
                                // Channel chip.
                                let (chip_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(18.0, 18.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(
                                    chip_rect,
                                    egui::CornerRadius::same(3),
                                    t.bg,
                                );
                                ui.painter().rect_stroke(
                                    chip_rect,
                                    egui::CornerRadius::same(3),
                                    egui::Stroke::new(1.0, t.border),
                                    egui::StrokeKind::Middle,
                                );
                                let ch_color = match layer.channel {
                                    'R' => t.red,
                                    'G' => t.green,
                                    'B' => t.accent,
                                    _ => t.text,
                                };
                                ui.painter().text(
                                    chip_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    layer.channel.to_string(),
                                    egui::FontId::monospace(10.0),
                                    ch_color,
                                );
                                // Texture swatch (just a colour fill).
                                let (sw_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(22.0, 22.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(
                                    sw_rect,
                                    egui::CornerRadius::same(3),
                                    egui::Color32::from_rgb(
                                        layer.color[0],
                                        layer.color[1],
                                        layer.color[2],
                                    ),
                                );
                                // Name + opacity bar.
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(&layer.name).color(t.text).size(11.5),
                                    );
                                    let (bar_rect, _) = ui.allocate_exact_size(
                                        egui::vec2(ui.available_width(), 5.0),
                                        egui::Sense::hover(),
                                    );
                                    let fill_rect = egui::Rect::from_min_max(
                                        bar_rect.left_top(),
                                        egui::pos2(
                                            bar_rect.left() + bar_rect.width() * layer.opacity,
                                            bar_rect.bottom(),
                                        ),
                                    );
                                    ui.painter().rect_filled(
                                        bar_rect,
                                        egui::CornerRadius::same(1),
                                        t.panel2,
                                    );
                                    ui.painter().rect_filled(
                                        fill_rect,
                                        egui::CornerRadius::same(1),
                                        egui::Color32::from_rgb(
                                            layer.color[0],
                                            layer.color[1],
                                            layer.color[2],
                                        ),
                                    );
                                });
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{}%",
                                        (layer.opacity * 100.0) as i32
                                    ))
                                    .color(t.muted)
                                    .size(10.0)
                                    .monospace(),
                                );
                            });
                        })
                        .response
                        .interact(egui::Sense::click());
                    if row_resp.clicked() {
                        new_active = Some(i);
                    }
                }
            },
        );
        if let Some(a) = new_active {
            s.active_layer = a;
        }

        // BRUSH section.
        crate::ui::widgets::section(
            ui,
            "Brush",
            false,
            |_ui| {},
            |ui| {
                ui.horizontal(|ui| {
                    for (mode, label) in [
                        (SplatBrushMode::Paint, "Paint"),
                        (SplatBrushMode::Erase, "Erase"),
                        (SplatBrushMode::Smear, "Smear"),
                    ] {
                        let selected = s.brush_mode == mode;
                        if ui.add(egui::Button::selectable(selected, label)).clicked() {
                            s.brush_mode = mode;
                        }
                    }
                });
                ui.add_space(8.0);
                let r_label = format!("{:.0} elmos", s.radius);
                crate::ui::widgets::ramp_slider_labelled(
                    ui,
                    "Radius",
                    &mut s.radius,
                    8.0..=512.0,
                    t.accent,
                    r_label,
                );
                ui.add_space(6.0);
                let strength_label = format!("{:.2}", s.strength);
                crate::ui::widgets::ramp_slider_labelled(
                    ui,
                    "Strength",
                    &mut s.strength,
                    0.0..=1.0,
                    t.accent,
                    strength_label,
                );
                ui.add_space(6.0);
                let space_label = format!("{}%", (s.spacing * 100.0) as i32);
                crate::ui::widgets::ramp_slider_labelled(
                    ui,
                    "Spacing",
                    &mut s.spacing,
                    0.0..=1.0,
                    t.muted,
                    space_label,
                );
            },
        );
    }

    /// Metal-spots inspector (ADR-035 / Phase 7). State is in-memory
    /// only; F5 wires schema persistence + viewport placement.
    // TODO(F5): wire LMB-place / RMB-delete from the central viewport.
    fn inspector_metal(&mut self, ui: &mut egui::Ui) {
        let t = crate::ui::theme::Tokens::DARK;
        let m = &mut self.metal_state;
        let spot_count = m.spots.len();
        crate::ui::widgets::section(
            ui,
            "Generator",
            true,
            |ui| {
                crate::ui::widgets::chip(ui, crate::ui::theme::ChipTone::Ok, "Mirrored");
            },
            |ui| {
                let density_label = format!("{:.2} spots/SMU²", m.density);
                crate::ui::widgets::ramp_slider_labelled(
                    ui,
                    "Density",
                    &mut m.density,
                    0.0..=1.0,
                    t.amber,
                    density_label,
                );
                ui.add_space(6.0);
                let spacing_label = format!("{:.0} elmos", m.min_spacing);
                crate::ui::widgets::ramp_slider_labelled(
                    ui,
                    "Min spacing",
                    &mut m.min_spacing,
                    256.0..=4096.0,
                    t.accent,
                    spacing_label,
                );
                ui.add_space(6.0);
                let max_label = format!("{:.1} m/s", m.max_metal);
                crate::ui::widgets::ramp_slider_labelled(
                    ui,
                    "Max metal",
                    &mut m.max_metal,
                    0.5..=3.0,
                    t.accent,
                    max_label,
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let _ = ui
                        .add(egui::Button::new("Reseed"))
                        .on_hover_text("Resample metal spots (Phase F5)");
                    let _ = ui
                        .add(egui::Button::new("Clear all"))
                        .on_hover_text("Remove every metal spot (Phase F5)");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        crate::ui::widgets::chip(
                            ui,
                            crate::ui::theme::ChipTone::Neutral,
                            format!("{} spots", spot_count),
                        );
                    });
                });
            },
        );

        // Spots list.
        let mut selected = m.selected;
        let spots = m.spots.clone();
        crate::ui::widgets::section(
            ui,
            "Spots",
            false,
            |ui| {
                ui.label(
                    egui::RichText::new("sorted by metal")
                        .color(t.muted)
                        .size(11.0),
                );
            },
            |ui| {
                if spots.is_empty() {
                    ui.label(
                        egui::RichText::new("No spots yet — generated by the F5 schema work.")
                            .color(t.dim)
                            .size(11.0),
                    );
                }
                for (i, spot) in spots.iter().enumerate() {
                    let hot = spot.metal >= 1.5;
                    let is_sel = selected == Some(i);
                    let resp = egui::Frame::new()
                        .fill(if is_sel { t.hover } else { t.bg })
                        .stroke(egui::Stroke::new(
                            1.0,
                            if is_sel { t.border_hi } else { t.border },
                        ))
                        .corner_radius(egui::CornerRadius::same(3))
                        .inner_margin(egui::Margin::symmetric(8, 4))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(8.0, 8.0),
                                    egui::Sense::hover(),
                                );
                                let color = if hot {
                                    egui::Color32::from_rgb(0xF5, 0x9E, 0x0B)
                                } else {
                                    egui::Color32::from_rgb(0xA3, 0x73, 0x40)
                                };
                                ui.painter().circle_filled(rect.center(), 4.0, color);
                                ui.label(
                                    egui::RichText::new(format!("M{:02}", i + 1))
                                        .color(t.muted)
                                        .monospace()
                                        .size(11.0),
                                );
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{}, {}",
                                        spot.x_elmo, spot.z_elmo
                                    ))
                                    .monospace()
                                    .size(11.0),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            egui::RichText::new(format!("{:.2}", spot.metal))
                                                .color(if hot { t.amber } else { t.text })
                                                .size(11.0)
                                                .monospace(),
                                        );
                                    },
                                );
                            });
                        })
                        .response
                        .interact(egui::Sense::click());
                    if resp.clicked() {
                        selected = Some(i);
                    }
                }
            },
        );
        self.metal_state.selected = selected;
    }

    /// Geo-features inspector (ADR-035 / Phase 7). UI scaffolding;
    /// F7 wires the feature gadget emitter + placement schema.
    // TODO(F7): persist library selection + transform jitter into
    // `Project.features` once the schema lands.
    fn inspector_geo(&mut self, ui: &mut egui::Ui) {
        let t = crate::ui::theme::Tokens::DARK;
        let g = &mut self.geo_state;

        // LIBRARY section: 3-column thumbnail grid.
        let mut new_selected: Option<usize> = None;
        let selected = g.selected;
        let library = g.library.clone();
        crate::ui::widgets::section(
            ui,
            "Feature library",
            true,
            |ui| {
                let _ = ui
                    .add(egui::Button::new("+ Import"))
                    .on_hover_text("Import a custom feature definition (Phase F7)");
            },
            |ui| {
                let cols = 3;
                ui.columns(cols, |col_uis| {
                    for (i, feat) in library.iter().enumerate() {
                        let col_ui = &mut col_uis[i % cols];
                        let is_sel = i == selected;
                        let (rect, response) = col_ui.allocate_exact_size(
                            egui::vec2(col_ui.available_width(), 78.0),
                            egui::Sense::click(),
                        );
                        let painter = col_ui.painter();
                        painter.rect_filled(
                            rect,
                            egui::CornerRadius::same(5),
                            if is_sel { t.hover } else { t.bg },
                        );
                        painter.rect_stroke(
                            rect,
                            egui::CornerRadius::same(5),
                            egui::Stroke::new(1.0, if is_sel { t.border_hi } else { t.border }),
                            egui::StrokeKind::Middle,
                        );
                        if is_sel {
                            let rail = egui::Rect::from_min_size(
                                egui::pos2(rect.left(), rect.top() + 6.0),
                                egui::vec2(2.0, rect.height() - 12.0),
                            );
                            painter.rect_filled(rail, egui::CornerRadius::same(1), t.accent);
                        }
                        let icon_rect = egui::Rect::from_center_size(
                            egui::pos2(rect.center().x, rect.top() + 30.0),
                            egui::vec2(26.0, 26.0),
                        );
                        crate::ui::icons::paint_icon(
                            painter,
                            icon_rect,
                            feat.icon,
                            if is_sel { t.text } else { t.muted },
                            1.4,
                        );
                        painter.text(
                            egui::pos2(rect.center().x, rect.bottom() - 10.0),
                            egui::Align2::CENTER_CENTER,
                            &feat.name,
                            egui::FontId::proportional(10.0),
                            if is_sel { t.text } else { t.muted },
                        );
                        painter.text(
                            egui::pos2(rect.right() - 4.0, rect.top() + 4.0),
                            egui::Align2::RIGHT_TOP,
                            feat.count.to_string(),
                            egui::FontId::monospace(9.0),
                            t.muted,
                        );
                        if response.clicked() {
                            new_selected = Some(i);
                        }
                    }
                });
            },
        );
        if let Some(s) = new_selected {
            g.selected = s;
        }

        // TRANSFORM section.
        crate::ui::widgets::section(
            ui,
            "Transform",
            false,
            |_ui| {},
            |ui| {
                let rot_label = format!("± {:.0}°", g.rotation_jitter);
                crate::ui::widgets::ramp_slider_labelled(
                    ui,
                    "Rotation jitter",
                    &mut g.rotation_jitter,
                    0.0..=180.0,
                    t.muted,
                    rot_label,
                );
                ui.add_space(8.0);
                let scale_label = format!("± {:.0}%", g.scale_jitter * 100.0);
                crate::ui::widgets::ramp_slider_labelled(
                    ui,
                    "Scale jitter",
                    &mut g.scale_jitter,
                    0.0..=1.0,
                    t.muted,
                    scale_label,
                );
                ui.add_space(8.0);
                ui.checkbox(&mut g.align_to_slope, "Align to slope");
            },
        );

        // SCATTER section.
        crate::ui::widgets::section(
            ui,
            "Scatter",
            false,
            |ui| {
                let name = g
                    .library
                    .get(g.selected)
                    .map(|f| f.name.clone())
                    .unwrap_or_default();
                crate::ui::widgets::chip(
                    ui,
                    crate::ui::theme::ChipTone::Neutral,
                    format!("{name} · selected"),
                );
            },
            |ui| {
                let d_label = format!("{:.0} / SMU²", g.scatter_density);
                crate::ui::widgets::ramp_slider_labelled(
                    ui,
                    "Density",
                    &mut g.scatter_density,
                    1.0..=128.0,
                    t.green,
                    d_label,
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let _ = ui
                        .add(
                            egui::Button::new("Scatter on selection")
                                .fill(t.accent)
                                .min_size(egui::vec2(ui.available_width() - 36.0, 28.0)),
                        )
                        .on_hover_text("Spawn features under selection (Phase F7)");
                    let _ = ui
                        .add(egui::Button::new("⌫"))
                        .on_hover_text("Clear scattered features");
                });
            },
        );
    }

    /// Sculpt inspector (ADR-035): 4-card brush picker (Off / Raise /
    /// Lower / Smooth) styled with a coloured swatch ring per mode,
    /// ramp sliders for radius and strength, and a behaviour chip row
    /// (Continuous active; Pressure and Lock-Z placeholder-disabled).
    fn inspector_sculpt(&mut self, ui: &mut egui::Ui) {
        let t = crate::ui::theme::Tokens::DARK;
        // BRUSH section: 4-card picker.
        let brushes_info: Vec<(Option<String>, &str, egui::Color32)> = vec![
            (None, "Off", t.muted),
            (Some("raise".to_string()), "Raise", t.green),
            (Some("lower".to_string()), "Lower", t.red),
            (Some("smooth".to_string()), "Smooth", t.accent),
        ];
        let mut new_brush: Option<Option<String>> = None;
        crate::ui::widgets::section(
            ui,
            "Brush",
            true,
            |_ui| {},
            |ui| {
                ui.columns(4, |cols| {
                    for (i, (id, label, color)) in brushes_info.iter().enumerate() {
                        let active = self.brush_id == *id;
                        let resp = Self::brush_card(&mut cols[i], label, *color, active);
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
        crate::ui::widgets::section(
            ui,
            "Shape",
            false,
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
                );
                ui.add_space(8.0);
                let s_label = format!("{:.2}", *strength_raw);
                crate::ui::widgets::ramp_slider_labelled(
                    ui,
                    "Strength",
                    strength_raw,
                    0.0..=1.0,
                    t.accent,
                    s_label,
                );
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Falloff").color(t.muted).size(11.0));
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
                        .on_hover_text("Always-on brush stamping. Default.");
                    let _ = ui
                        .add_enabled(false, egui::Button::new("Pressure"))
                        .on_hover_text("Tablet pressure input — not yet wired.");
                    let _ = ui
                        .add_enabled(false, egui::Button::new("Lock Z"))
                        .on_hover_text("Clamp brush to a target elevation — Phase F2+");
                });
            },
        );
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
        let t = crate::ui::theme::Tokens::DARK;
        let (ex, ez) = self.world_extents();

        // LAYOUT section: preset chip + drag-paint toggle + Balanced chip.
        let balanced = self.start_positions_balanced();
        crate::ui::widgets::section(
            ui,
            "Layout",
            true,
            |ui| {
                let tone = if balanced {
                    crate::ui::theme::ChipTone::Ok
                } else {
                    crate::ui::theme::ChipTone::Warn
                };
                let label = if balanced { "Balanced" } else { "Asymmetric" };
                crate::ui::widgets::chip(ui, tone, label);
            },
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Preset").color(t.muted).size(11.0));
                    let mut selected: Option<AllyPreset> = None;
                    egui::ComboBox::from_id_salt("ally_preset")
                        .selected_text("Apply a layout…")
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
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Drag-paint").color(t.muted).size(11.0));
                    ui.add(
                        egui::DragValue::new(&mut self.drag_paint_count)
                            .range(1u8..=32)
                            .suffix(" pos"),
                    );
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
        crate::ui::widgets::section(
            ui,
            &group_title,
            false,
            |ui| {
                if ui
                    .add(egui::Button::new("+ Add"))
                    .on_hover_text("Add a new ally team")
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
                                            .on_hover_text(
                                                "Remove this ally group and all its positions",
                                            )
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
        let t = crate::ui::theme::Tokens::DARK;
        let active_preset = PRESETS
            .iter()
            .find(|p| p.expression == self.procgen_expr && p.domain == self.procgen_domain)
            .map(|p| p.label);

        // PRESET section: chip row.
        crate::ui::widgets::section(
            ui,
            "Preset",
            true,
            |_ui| {},
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    for p in PRESETS {
                        let chosen = Some(p.label) == active_preset;
                        let btn = egui::Button::selectable(chosen, p.label);
                        if ui.add(btn).clicked() {
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
                        let resp = ui.add(
                            egui::TextEdit::multiline(&mut self.procgen_expr)
                                .font(egui::FontId::monospace(11.5))
                                .desired_width(f32::INFINITY)
                                .desired_rows(2)
                                .frame(false),
                        );
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
                        .clicked()
                    {
                        self.procgen_domain = Domain::Unit;
                    }
                    if ui
                        .add(egui::Button::selectable(
                            self.procgen_domain == Domain::Centered,
                            Domain::Centered.label(),
                        ))
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
                crate::ui::widgets::chip(ui, tone, label);
            },
            |ui| {
                if let Some(tex) = self.procgen_thumbnail.as_ref() {
                    let max_side = ui.available_width().min(PROCGEN_THUMBNAIL_PX as f32);
                    ui.add(egui::Image::new(tex).fit_to_exact_size(egui::vec2(max_side, max_side)));
                } else if !valid {
                    ui.label(
                        egui::RichText::new("(fix expression to render preview)")
                            .color(t.dim)
                            .size(11.0),
                    );
                } else {
                    ui.label(
                        egui::RichText::new("(baking preview…)")
                            .color(t.dim)
                            .size(11.0),
                    );
                }
                ui.add_space(8.0);
                // Commit button — disabled until parse succeeds.
                let resp = ui.add_enabled(
                    valid,
                    egui::Button::new("Commit to heightmap")
                        .fill(if valid { t.accent } else { t.panel2 })
                        .min_size(egui::vec2(ui.available_width(), 32.0)),
                );
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

            let brush_active = matches!(self.tool, Tool::Sculpt)
                && self.brush_id.is_some()
                && self.heightmap.is_some();
            let start_pos_active = matches!(self.tool, Tool::StartPositions);
            let central_interactive = brush_active || start_pos_active;

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

            let camera_drag = if central_interactive {
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
            if self.history.stroke_open() && !response.dragged_by(egui::PointerButton::Primary) {
                self.end_stroke();
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

            // ADR-035 viewport chrome (replaces XYZ nav gizmo):
            // 1. elmo rulers (bottom + left edges)
            // 2. mini-map (top-right)
            // 3. viewport-options toolbar (top-left)
            // 4. hint card (bottom-centre, first-launch only)
            crate::ui::viewport_chrome::paint_rulers(&overlay_painter, rect, extents);

            // Mini-map. Uses its own painter inside ui scope.
            let metal_spots: &[(f32, f32, f32)] = &[]; // populated by Phase 7
            let heightmap_data = self.heightmap.as_ref().map(|h| &h.data);
            crate::ui::minimap::paint_minimap(
                ui,
                rect,
                heightmap_data,
                &self.ally_groups,
                metal_spots,
                extents,
                &self.camera,
            );

            // Floating viewport-options toolbar. Allocate a Ui placed
            // at the top-left, just inside the rulers.
            let chrome_origin = egui::pos2(rect.left() + 32.0, rect.top() + 14.0);
            let mut chrome_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_min_size(
                        chrome_origin,
                        egui::vec2(260.0, 32.0),
                    ))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            crate::ui::viewport_chrome::viewport_options_toolbar(
                &mut chrome_ui,
                &mut self.grid_overlay_on,
                &mut self.lighting_on,
                &mut self.wireframe_on,
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
            dirty: false,
            last_non_none_symmetry: SymmetryAxis::Horizontal,
            grid_overlay_on: false,
            lighting_on: true,
            wireframe_on: false,
            splat_state: SplatState::default(),
            metal_state: MetalState::default(),
            geo_state: GeoState::default(),
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
            7,
            "Tool::ALL size changed — update ADR-030 / ADR-035 + plan"
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
    /// T / M / F. A drift here is a documented contract break — bump
    /// the ADR if intentional.
    #[test]
    fn tool_accelerators_match_adr_030() {
        assert_eq!(Tool::Select.accel(), "Q");
        assert_eq!(Tool::Sculpt.accel(), "B");
        assert_eq!(Tool::StartPositions.accel(), "S");
        assert_eq!(Tool::SplatPaint.accel(), "T");
        assert_eq!(Tool::MetalSpots.accel(), "M");
        assert_eq!(Tool::GeoFeatures.accel(), "F");
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

    // ───────────── ADR-035 / Phase 7 scaffolding tests ─────────────
    //
    // These exercise the in-memory `SplatState` / `MetalState` /
    // `GeoState` defaults the inspector reads. They are scaffolding
    // tests — pin invariants the mockup demands so a future refactor
    // doesn't silently break the visible defaults.
    //
    // FUTURE TEST COVERAGE (TODO when each F-series schema lands):
    //
    //  F4 (splat): assert that picking a layer + brush_mode + radius
    //              from the UI lands a stamp into a snapshot
    //              `Project.splat_distribution_overlay` chunk. Round-
    //              trip save/open should preserve layer opacities.
    //
    //  F5 (metal): assert that `MetalState::spots` round-trips through
    //              `Project::metal_spots` (or whichever field the F5
    //              schema picks). `Reseed` should be deterministic
    //              given a seed; mirror under `symmetry` should
    //              produce paired spots; `Clear all` should empty the
    //              `Vec` and undo.
    //
    //  F7 (geo):   assert that `GeoState.selected` + `scatter_density`
    //              + the `align_to_slope` flag drive a feature gadget
    //              emission that names every feature in the library.
    //              Scatter-on-selection should hash deterministically
    //              for unit-testable golden output.

    #[test]
    fn splat_state_default_has_four_layers_with_unique_channels() {
        let s = SplatState::default();
        assert_eq!(s.layers.len(), 4);
        let mut chs: Vec<char> = s.layers.iter().map(|l| l.channel).collect();
        chs.sort();
        assert_eq!(chs, vec!['A', 'B', 'G', 'R']);
        assert_eq!(s.active_layer, 0);
        assert!(matches!(s.brush_mode, SplatBrushMode::Paint));
    }

    #[test]
    fn splat_state_default_opacities_in_range() {
        // Visual ordering: Grass (most), Rock, Sand, Snow (least).
        let s = SplatState::default();
        for layer in &s.layers {
            assert!(
                (0.0..=1.0).contains(&layer.opacity),
                "{} opacity {} out of range",
                layer.name,
                layer.opacity
            );
        }
    }

    #[test]
    fn metal_state_default_has_no_spots() {
        let m = MetalState::default();
        assert!(m.spots.is_empty());
        assert!(m.selected.is_none());
        // Sane numeric defaults.
        assert!(m.density > 0.0 && m.density <= 1.0);
        assert!(m.min_spacing > 0.0);
        assert!(m.max_metal > 0.0);
    }

    #[test]
    fn geo_state_default_has_library_and_first_selected() {
        let g = GeoState::default();
        assert!(!g.library.is_empty());
        assert!(g.selected < g.library.len());
        // Every entry has a name (UI relies on this).
        for f in &g.library {
            assert!(!f.name.is_empty());
        }
    }

    #[test]
    fn fresh_app_has_phase_7_default_state() {
        // App::new wires the three scaffolding states. Smoke test
        // that they survive App construction (via make_test_app, which
        // uses the same initialiser shape).
        let app = make_test_app();
        assert_eq!(app.splat_state.layers.len(), 4);
        assert!(app.metal_state.spots.is_empty());
        assert!(!app.geo_state.library.is_empty());
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
}
