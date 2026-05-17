mod render;

use std::path::{Path, PathBuf};

use anyhow::Result;
use barme_core::{Heightmap, MapSize};
use eframe::egui;
use eframe::egui_wgpu;
use tracing::{info, warn};

use crate::render::{OrbitCamera, TerrainCallback};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
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
}

fn fixture_path(smu: u32) -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir.parent().and_then(|p| p.parent()).unwrap();
    let edge = smu * 64 + 1;
    repo_root
        .join("assets")
        .join("fixtures")
        .join(format!("r16_ramp_{smu}x{smu}smu_{edge}px.png"))
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut load_request: Option<PathBuf> = None;
        let mut rebuild_mesh = false;

        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New project…").clicked() {
                        ui.close();
                    }
                    ui.separator();
                    ui.label("Load fixture heightmap");
                    for smu in [2u32, 4, 16] {
                        if ui.button(format!("{smu}×{smu} SMU")).clicked() {
                            load_request = Some(fixture_path(smu));
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Build", |ui| {
                    if ui.button("Compile .sd7").clicked() {
                        ui.close();
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

        if let Some(path) = load_request {
            self.load_heightmap(path);
        } else if rebuild_mesh {
            self.rebuild_mesh();
        }
    }
}
