use std::path::{Path, PathBuf};

use anyhow::Result;
use barme_core::{Heightmap, MapSize};
use eframe::egui;
use tracing::{info, warn};

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
}

struct HeightmapState {
    path: PathBuf,
    dims: (u32, u32),
    min: u16,
    max: u16,
    validated_against: Option<MapSize>,
}

impl App {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            project_name: "untitled".to_string(),
            map_size_smu: 16,
            heightmap: None,
            last_error: None,
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
}

fn fixture_path(smu: u32) -> PathBuf {
    // Two parents up from the binary's manifest dir; works in `cargo run` and
    // `cargo run --release`. Fixtures live at <repo>/assets/fixtures/.
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

            if let Some(err) = &self.last_error {
                ui.separator();
                ui.colored_label(egui::Color32::RED, err);
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.centered_and_justified(|ui| {
                ui.label("3D preview viewport will live here.");
            });
        });

        if let Some(path) = load_request {
            self.load_heightmap(path);
        }
    }
}
