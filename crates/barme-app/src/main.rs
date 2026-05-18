mod launcher;
mod render;

use std::path::{Path, PathBuf};

use anyhow::Result;
use barme_core::{Heightmap, MapSize, PROJECT_EXTENSION, Project};
use barme_pipeline::PyMapConvDriver;
use eframe::egui;
use eframe::egui_wgpu;
use tracing::{error, info, warn};

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
}

struct HeightmapState {
    path: PathBuf,
    dims: (u32, u32),
    min: u16,
    max: u16,
    validated_against: Option<MapSize>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let render_state = cc.wgpu_render_state.clone();
        if let Some(rs) = render_state.as_ref() {
            render::install(rs);
        } else {
            warn!("no wgpu render state — terrain preview disabled");
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
        }
    }

    fn load_heightmap(&mut self, path: PathBuf) {
        self.last_error = None;
        match Heightmap::load_png(&path) {
            Ok(h) => {
                let dims = h.dims();
                let (min, max) = h.min_max();
                let size = MapSize::square(self.map_size_smu);
                let validated_against = h.validate_against(size).ok().map(|_| size);
                if validated_against.is_none() {
                    warn!(
                        "loaded heightmap {} dims {:?} do not match {}×{} SMU ({:?})",
                        path.display(),
                        dims,
                        self.map_size_smu,
                        self.map_size_smu,
                        size.heightmap_dims(),
                    );
                }
                if let Some(rs) = self.render_state.as_ref() {
                    render::upload_mesh(rs, &h, self.height_scale);
                    let extent_x = (dims.0 - 1) as f32 * render::ELMOS_PER_PIXEL;
                    let extent_z = (dims.1 - 1) as f32 * render::ELMOS_PER_PIXEL;
                    self.camera = OrbitCamera::framing(extent_x, extent_z);
                }
                self.heightmap = Some(HeightmapState {
                    path,
                    dims,
                    min,
                    max,
                    validated_against,
                });
            }
            Err(e) => {
                warn!("failed to load heightmap: {e:#}");
                self.last_error = Some(format!("{e:#}"));
            }
        }
    }

    fn rebuild_mesh(&mut self) {
        let Some(rs) = self.render_state.as_ref() else {
            return;
        };
        let Some(state) = self.heightmap.as_ref() else {
            return;
        };
        match Heightmap::load_png(&state.path) {
            Ok(h) => render::upload_mesh(rs, &h, self.height_scale),
            Err(e) => warn!("rebuild_mesh: {e:#}"),
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
    }

    fn snapshot_project(&self) -> Project {
        Project {
            name: self.project_name.clone(),
            size: MapSize::square(self.map_size_smu),
            min_height: 0.0,
            max_height: self.height_scale,
            heightmap: self.heightmap.as_ref().map(|h| h.path.clone()),
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
                warn!("save to {} failed: {e}", path.display());
                self.last_error = Some(format!("save: {e}"));
            }
        }
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
        let hm_path = hm.path.clone();
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
                warn!("open {} failed: {e}", path.display());
                self.last_error = Some(format!("open: {e}"));
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
    NewProject,
    Save,
    SaveAs,
    Open,
    BuildAndInstall,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut action: Option<FileAction> = None;
        let mut rebuild_mesh = false;

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
            let resp = ui.add(
                egui::DragValue::new(&mut self.height_scale)
                    .range(1.0..=4096.0)
                    .speed(1.0)
                    .prefix("Max height (elmos): "),
            );
            if resp.changed() {
                rebuild_mesh = true;
            }
            ui.label(format!(
                "Camera: yaw {:.0}° pitch {:.0}° dist {:.0}",
                self.camera.yaw.to_degrees(),
                self.camera.pitch.to_degrees(),
                self.camera.distance,
            ));

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

            if response.dragged() {
                let d = response.drag_delta();
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
            None if rebuild_mesh => self.rebuild_mesh(),
            None => {}
        }
    }
}
