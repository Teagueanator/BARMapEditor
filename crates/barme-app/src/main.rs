mod launcher;
mod render;

use std::path::{Path, PathBuf};

use anyhow::Result;
use barme_core::{
    BrushRegistry, BrushStamp, DirtyRect, Heightmap, History, MapSize, PROJECT_EXTENSION, Project,
    StampSnapshot, StartPosition, SymmetryAxis, UndoEntry,
    brushes::pixel_bbox,
    procgen::{Domain, PRESETS, generate as procgen_generate},
    start_pos::assign_team_ids,
};
use barme_pipeline::PyMapConvDriver;
use eframe::egui;
use eframe::egui_wgpu;
use tracing::{error, info, trace, warn};

use crate::render::{OrbitCamera, TerrainCallback};

fn main() -> Result<()> {
    // wgpu/vulkan/naga emit a lot of INFO-level chatter at startup (adapter
    // enumeration, layer loading) that drowns out our own logs. Keep them at
    // WARN by default; users can override with RUST_LOG.
    let default_filter = "info,wgpu=warn,wgpu_core=warn,wgpu_hal=warn,naga=warn,egui_wgpu=warn";
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter)),
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
    map_size_smu: u32,
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
    /// Persistent undo/redo across the session. Cleared on barrier events
    /// (procgen, heightmap load, new project) — see ADR-022.
    history: History,
    /// Open stroke — populated on the first stamp after LMB-down, flushed
    /// to `history` when the pointer is released.
    stroke: Option<UndoEntry>,
    /// Active tool mode for the central preview rect (ADR-023).
    tool_mode: ToolMode,
    /// Authored team start positions; round-trips through `Project`.
    /// Empty by default — the pipeline falls back to a 25/75 default pair.
    start_positions: Vec<StartPosition>,
    /// While LMB is held in `StartPositions` mode on an existing marker,
    /// holds that team's id so the drag re-positions it. Cleared on release.
    dragging_start_pos: Option<u8>,
}

/// Central-rect interaction mode. Stays in `Sculpt` for the existing brush
/// flow; flips to `StartPositions` when F8 placement is active. Mode is
/// exposed in the side panel as a radio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolMode {
    Sculpt,
    StartPositions,
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

        Self {
            project_name: "untitled".to_string(),
            map_size_smu: 16,
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
            history: History::default(),
            stroke: None,
            tool_mode: ToolMode::Sculpt,
            start_positions: Vec::new(),
            dragging_start_pos: None,
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
                let size = MapSize::square(self.map_size_smu);
                let validated_against = h.validate_against(size).ok().map(|_| size);
                if validated_against.is_none() {
                    warn!(
                        path = %path.display(),
                        loaded_dims = ?dims,
                        expected_dims = ?size.heightmap_dims(),
                        smu = self.map_size_smu,
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
        self.map_size_smu = 16;
        self.heightmap = None;
        self.current_project_path = None;
        self.height_scale = 256.0;
        self.camera = OrbitCamera::framing(8192.0, 8192.0);
        self.last_error = None;
        self.last_install = None;
        self.start_positions.clear();
        self.dragging_start_pos = None;
        self.end_stroke();
        self.history.barrier();
    }

    fn snapshot_project(&self) -> Project {
        Project {
            name: self.project_name.clone(),
            size: MapSize::square(self.map_size_smu),
            min_height: 0.0,
            max_height: self.height_scale,
            heightmap: self.heightmap.as_ref().map(|h| h.path.clone()),
            start_positions: self.start_positions.clone(),
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
        let size = MapSize::square(self.map_size_smu);
        let expr = self.procgen_expr.clone();
        let domain = self.procgen_domain;
        info!(
            expr = %expr,
            domain = ?domain,
            smu = self.map_size_smu,
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
        // so we can snapshot the unioned region pre-edit for undo (ADR-022).
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
        let before = hm_state
            .data
            .copy_rect(snap_rect.x, snap_rect.y, snap_rect.w, snap_rect.h);

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
        // Record the pre-edit snapshot into the open stroke (created on
        // demand).
        self.stroke
            .get_or_insert_with(UndoEntry::new)
            .push(StampSnapshot {
                rect: snap_rect,
                before,
            });

        render::write_heightmap_rect(rs, dims, hm_state.data.data(), rect_dirty);
        let (mn, mx) = hm_state.data.min_max();
        hm_state.min = mn;
        hm_state.max = mx;
    }

    /// Flush the in-progress stroke into the undo stack. Idempotent; a no-op
    /// when no stroke is open. Called on pointer-release and before every
    /// barrier event (procgen / load / new project).
    fn end_stroke(&mut self) {
        if let Some(entry) = self.stroke.take()
            && !entry.is_empty()
        {
            trace!(
                stamps = entry.stamp_count(),
                bytes = entry.bytes(),
                "stroke committed to undo history"
            );
            self.history.push(entry);
        }
    }

    /// Pop one stroke off the undo stack and re-upload the affected pixels.
    /// Also flushes an open stroke first so the user always undoes a
    /// finished unit.
    fn undo_one(&mut self) {
        self.end_stroke();
        let Some(hm_state) = self.heightmap.as_mut() else {
            return;
        };
        let Some(rect) = self.history.apply_undo(&mut hm_state.data) else {
            trace!("undo: nothing to undo");
            return;
        };
        info!(
            rect = ?(rect.x, rect.y, rect.w, rect.h),
            undo_depth = self.history.undo_depth(),
            redo_depth = self.history.redo_depth(),
            "undo applied"
        );
        if let Some(rs) = self.render_state.as_ref() {
            render::write_heightmap_rect(rs, hm_state.dims, hm_state.data.data(), rect);
        }
        let (mn, mx) = hm_state.data.min_max();
        hm_state.min = mn;
        hm_state.max = mx;
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
            let (ex, ez) = MapSize::square(self.map_size_smu).elmo_extents();
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
        }
    }

    /// Move the position with `team_id` to the given world coordinates,
    /// clamped to the map. No-op if the id isn't present.
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

    /// Remove the position with `team_id`. No-op if absent.
    fn delete_start_position(&mut self, team_id: u8) {
        let before = self.start_positions.len();
        self.start_positions.retain(|p| p.team_id != team_id);
        if self.start_positions.len() < before {
            info!(team_id, "start position deleted");
        }
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

    /// Inverse of `undo_one`.
    fn redo_one(&mut self) {
        self.end_stroke();
        let Some(hm_state) = self.heightmap.as_mut() else {
            return;
        };
        let Some(rect) = self.history.apply_redo(&mut hm_state.data) else {
            trace!("redo: nothing to redo");
            return;
        };
        info!(
            rect = ?(rect.x, rect.y, rect.w, rect.h),
            undo_depth = self.history.undo_depth(),
            redo_depth = self.history.redo_depth(),
            "redo applied"
        );
        if let Some(rs) = self.render_state.as_ref() {
            render::write_heightmap_rect(rs, hm_state.dims, hm_state.data.data(), rect);
        }
        let (mn, mx) = hm_state.data.min_max();
        hm_state.min = mn;
        hm_state.max = mx;
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
            smu = self.map_size_smu,
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
                self.map_size_smu = p.size.smu_x;
                self.height_scale = p.max_height.max(1.0);
                self.heightmap = None;
                self.current_project_path = Some(path);
                self.last_error = None;
                let (ex, ez) = MapSize::square(self.map_size_smu).elmo_extents();
                self.camera = OrbitCamera::framing(ex as f32, ez as f32);

                self.start_positions = p.start_positions;

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
    NewProject,
    Save,
    SaveAs,
    Open,
    BuildAndInstall,
    ApplyProcGen,
    Undo,
    Redo,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut action: Option<FileAction> = None;

        // Ctrl-Z / Ctrl-Shift-Z keybinds (Cmd on macOS via egui's `command`).
        // Queued through `action` so we don't mutate state mid-frame.
        let (key_undo, key_redo) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            let shift = i.modifiers.shift;
            let z = i.key_pressed(egui::Key::Z);
            let y = i.key_pressed(egui::Key::Y);
            (cmd && !shift && z, (cmd && shift && z) || (cmd && y))
        });
        if key_undo {
            action = Some(FileAction::Undo);
        } else if key_redo {
            action = Some(FileAction::Redo);
        }

        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New project").clicked() {
                        action = Some(FileAction::NewProject);
                        ui.close();
                    }
                    if ui.button("Open project…").clicked() {
                        action = Some(FileAction::Open);
                        ui.close();
                    }
                    if ui.button("Save project").clicked() {
                        action = Some(FileAction::Save);
                        ui.close();
                    }
                    if ui.button("Save project as…").clicked() {
                        action = Some(FileAction::SaveAs);
                        ui.close();
                    }
                    ui.separator();
                    ui.label("Load fixture heightmap");
                    for smu in [2u32, 4, 16] {
                        if ui.button(format!("{smu}×{smu} SMU")).clicked() {
                            action = Some(FileAction::LoadHeightmap(fixture_path(smu)));
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Edit", |ui| {
                    let can_undo = self.history.can_undo() || self.stroke.is_some();
                    let can_redo = self.history.can_redo();
                    if ui
                        .add_enabled(can_undo, egui::Button::new("Undo\tCtrl+Z"))
                        .clicked()
                    {
                        action = Some(FileAction::Undo);
                        ui.close();
                    }
                    if ui
                        .add_enabled(can_redo, egui::Button::new("Redo\tCtrl+Shift+Z"))
                        .clicked()
                    {
                        action = Some(FileAction::Redo);
                        ui.close();
                    }
                });
                ui.menu_button("Build", |ui| {
                    let enabled = self.heightmap.is_some();
                    if ui
                        .add_enabled(enabled, egui::Button::new("Build & Install to BAR"))
                        .clicked()
                    {
                        action = Some(FileAction::BuildAndInstall);
                        ui.close();
                    }
                    if !enabled {
                        ui.label("(load a heightmap first)");
                    }
                });
            });
        });

        egui::SidePanel::left("tools").show(ctx, |ui| {
            ui.heading("Project");
            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.text_edit_singleline(&mut self.project_name);
            });
            ui.horizontal(|ui| {
                ui.label("Size (SMU):");
                ui.add(egui::DragValue::new(&mut self.map_size_smu).range(2..=96));
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
                                self.map_size_smu,
                                self.map_size_smu,
                                MapSize::square(self.map_size_smu).heightmap_dims(),
                            ),
                        ),
                    };
                }
            }

            ui.separator();
            ui.heading("Render");
            // Height scale flows through the per-frame uniform — no
            // texture or grid rebuild needed when this changes (ADR-017).
            ui.add(
                egui::DragValue::new(&mut self.height_scale)
                    .range(1.0..=4096.0)
                    .speed(1.0)
                    .prefix("Max height (elmos): "),
            );

            ui.separator();
            ui.heading("Tool");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tool_mode, ToolMode::Sculpt, "Sculpt");
                ui.selectable_value(
                    &mut self.tool_mode,
                    ToolMode::StartPositions,
                    "Start positions",
                );
            });

            ui.separator();
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
            // Symmetry — one stroke produces N mirrored / rotated stamps
            // (ADR-019). Lock rotational fold to {2,3,4,6,8}.
            egui::ComboBox::from_label("Symmetry")
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
                // Free-form fold count — 3 for 3-player maps, 4 for 4-player,
                // 5/6/7/8 for FFA. 2..=12 covers everything BAR supports;
                // larger folds quickly become indistinguishable from radial
                // symmetry.
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

            ui.label(format!(
                "Camera: yaw {:.0}° pitch {:.0}° dist {:.0}",
                self.camera.yaw.to_degrees(),
                self.camera.pitch.to_degrees(),
                self.camera.distance,
            ));

            ui.separator();
            ui.heading("Start positions");
            ui.label(
                egui::RichText::new(
                    "Tool = Start positions: LMB to place, drag to move, RMB to delete.",
                )
                .small()
                .weak(),
            );
            ui.label(format!("Placed: {}", self.start_positions.len()));
            let mut to_delete: Option<u8> = None;
            egui::ScrollArea::vertical()
                .max_height(140.0)
                .id_salt("startpos_scroll")
                .show(ui, |ui| {
                    let mut sorted: Vec<StartPosition> = self.start_positions.clone();
                    sorted.sort_by_key(|p| p.team_id);
                    for p in sorted {
                        ui.horizontal(|ui| {
                            let color = team_color(p.team_id);
                            let (resp, painter) = ui.allocate_painter(
                                egui::Vec2::new(14.0, 14.0),
                                egui::Sense::hover(),
                            );
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

            ui.separator();
            ui.heading("Generate from formula");
            ui.label(
                egui::RichText::new("f(x, z) → height ∈ [0,1]. Replaces heightmap.")
                    .small()
                    .weak(),
            );
            ui.text_edit_singleline(&mut self.procgen_expr);
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
                        }
                    }
                });
            if ui.button("Apply").clicked() {
                action = Some(FileAction::ApplyProcGen);
            }
            if let Some(err) = &self.procgen_last_error {
                ui.colored_label(egui::Color32::RED, format!("Procgen: {err}"));
            }

            ui.separator();
            ui.heading("Build & Install");
            let can_install = self.heightmap.is_some();
            let resp = ui.add_enabled(can_install, egui::Button::new("Build & Install to BAR"));
            if resp.clicked() {
                action = Some(FileAction::BuildAndInstall);
            }
            if !can_install {
                ui.label("Load a heightmap to enable.");
            }
            match &self.last_install {
                Some(Ok(p)) => {
                    ui.colored_label(
                        egui::Color32::GREEN,
                        format!(
                            "Installed: {}",
                            p.file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or_else(|| p.to_str().unwrap_or("?"))
                        ),
                    );
                    if let Some(parent) = p.parent() {
                        ui.label(format!("Dir: {}", parent.display()));
                    }
                }
                Some(Err(msg)) => {
                    ui.colored_label(egui::Color32::RED, format!("Install failed: {msg}"));
                }
                None => {}
            }

            if let Some(err) = &self.last_error {
                ui.separator();
                ui.colored_label(egui::Color32::RED, err);
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let (rect, response) =
                ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());

            // Tool mode determines what the left pointer button does.
            // Brush sculpt or start-position placement → LMB is tool, RMB
            // orbits. Otherwise LMB orbits (Stage 0 idle-camera behaviour).
            let brush_active = matches!(self.tool_mode, ToolMode::Sculpt)
                && self.brush_id.is_some()
                && self.heightmap.is_some();
            let start_pos_active = matches!(self.tool_mode, ToolMode::StartPositions);
            let central_interactive = brush_active || start_pos_active;
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
            // on the y=0 plane. Spacing along the drag is implicit (frame
            // rate). One LMB-down → LMB-up coalesces into a single undo
            // unit via `end_stroke` on pointer release (ADR-022).
            if brush_active
                && (response.dragged_by(egui::PointerButton::Primary)
                    || response.clicked_by(egui::PointerButton::Primary))
                && let Some(cursor) = ctx.pointer_interact_pos()
            {
                self.apply_brush_at(cursor, rect);
            }
            if self.stroke.is_some() && !response.dragged_by(egui::PointerButton::Primary) {
                self.end_stroke();
            }

            // Start-position placement / move / delete (ADR-023).
            if start_pos_active && let Some(cursor) = ctx.pointer_interact_pos() {
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
                }
                if response.dragged_by(egui::PointerButton::Primary)
                    && let Some(id) = self.dragging_start_pos
                    && let Some(world) =
                        render::screen_to_world_y0(cursor_in, rect_size, &self.camera)
                {
                    self.move_start_position(id, world.x, world.z);
                }
                if response.drag_stopped_by(egui::PointerButton::Primary) {
                    self.dragging_start_pos = None;
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

            // Overlay start-position markers on top of the terrain pass.
            // Always rendered when any are placed (regardless of tool mode)
            // so the user can see them while sculpting.
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
        });

        match action {
            Some(FileAction::LoadHeightmap(p)) => self.load_heightmap(p),
            Some(FileAction::NewProject) => self.new_project(),
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
