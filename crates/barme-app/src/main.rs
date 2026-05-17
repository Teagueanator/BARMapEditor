use anyhow::Result;
use eframe::egui;
use tracing::info;

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
}

impl App {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            project_name: "untitled".to_string(),
            map_size_smu: 16,
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New project…").clicked() {
                        ui.close();
                    }
                    if ui.button("Open .sd7…").clicked() {
                        ui.close();
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
            ui.label("Stage 0 prototype — UI placeholders only.");
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.centered_and_justified(|ui| {
                ui.label("3D preview viewport will live here.");
            });
        });
    }
}
