//! Sprint 20 / chunk 5 — build log panel.
//!
//! Floating `egui::Window` that surfaces the build's bounded ring
//! buffer. Body is a scrollable monospace `Label` per line; auto-
//! scrolls to bottom when new lines arrive. Footer carries
//! `[Copy]` / `[Save log…]` / `[Clear]` actions.
//!
//! Reuses the docked-floating shape Sprint 19's lint panel
//! established. The header summarises the current `BuildState`
//! (Idle / Running stage / Done in 14s / Failed: PyMapConv exit 1).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;

use eframe::egui;

use crate::build_runner::{BuildLogLine, BuildState};
use barme_pipeline::LogStream;

use crate::ui::theme::Tokens;

/// Caller-visible click outcome. The App's update loop translates
/// these into the right state mutations (clearing the ring buffer
/// for `Clear`; writing the buffer to a user-picked file for
/// `SaveAs`).
#[derive(Debug, Default)]
pub struct LogPanelClicks {
    pub clear: bool,
    pub save_as: Option<PathBuf>,
}

/// Render the build log panel. Caller passes a mutable `bool` that
/// drives visibility; the window's close-X sets it back to `false`.
///
/// Returns a [`LogPanelClicks`] tagged with this frame's actions so
/// the App can apply them outside the egui closure (lock the log,
/// truncate it, etc.).
pub fn render(ctx: &egui::Context, open: &mut bool, state: &BuildState) -> LogPanelClicks {
    let mut clicks = LogPanelClicks::default();
    if !*open {
        return clicks;
    }
    let t = Tokens::DARK;
    let mut local_open = true;
    egui::Window::new("Build log")
        .open(&mut local_open)
        .collapsible(true)
        .resizable(true)
        .default_width(560.0)
        .default_height(360.0)
        .show(ctx, |ui| {
            // ─── Header — current state summary ─────────────────────
            render_header(ui, state, &t);
            ui.separator();

            // ─── Body — scrollable monospace lines ──────────────────
            let log_arc = state.log();
            let scroll = egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true);
            scroll.show(ui, |ui| {
                if let Some(log) = log_arc {
                    render_log_body(ui, log, &t);
                } else {
                    ui.label(
                        egui::RichText::new(
                            "No build has been run yet. Click \"Build & Install\" to start.",
                        )
                        .color(t.muted)
                        .size(11.0),
                    );
                }
            });

            ui.separator();
            // ─── Footer — actions ──────────────────────────────────
            ui.horizontal(|ui| {
                let copy_btn = ui
                    .button("Copy")
                    .on_hover_text("Copy the full visible log to the clipboard.");
                if copy_btn.clicked()
                    && let Some(log) = log_arc
                    && let Ok(guard) = log.lock()
                {
                    let text = render_log_as_text(&guard);
                    ctx.copy_text(text);
                }
                let save_btn = ui
                    .button("Save log…")
                    .on_hover_text("Save the full visible log to a .log file on disk.");
                if save_btn.clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("Build log", &["log", "txt"])
                        .set_file_name("barme-build.log")
                        .save_file()
                {
                    clicks.save_as = Some(path);
                }
                let clear_btn = ui
                    .button("Clear")
                    .on_hover_text("Drop every line currently in the ring buffer.");
                if clear_btn.clicked() {
                    clicks.clear = true;
                }
            });
        });
    if !local_open {
        *open = false;
    }
    clicks
}

/// One-line header describing the current build state.
fn render_header(ui: &mut egui::Ui, state: &BuildState, t: &Tokens) {
    match state {
        BuildState::Idle => {
            ui.label(egui::RichText::new("Idle").color(t.muted).size(12.0));
        }
        BuildState::Running {
            project_name,
            current_stage,
            started_at,
            ..
        } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    egui::RichText::new(format!(
                        "Building {project_name} · {} · {}s elapsed",
                        current_stage.label(),
                        started_at.elapsed().as_secs(),
                    ))
                    .color(t.text)
                    .size(12.0),
                );
            });
        }
        BuildState::Done {
            sd7_path, duration, ..
        } => {
            ui.label(
                egui::RichText::new(format!(
                    "✓ Done in {}s — {}",
                    duration.as_secs(),
                    sd7_path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("(unknown).sd7"),
                ))
                .color(egui::Color32::from_rgb(110, 200, 120))
                .size(12.0),
            );
        }
        BuildState::Failed {
            error, duration, ..
        } => {
            ui.label(
                egui::RichText::new(format!("✗ Failed after {}s: {error}", duration.as_secs()))
                    .color(egui::Color32::from_rgb(220, 110, 90))
                    .size(12.0),
            );
        }
        BuildState::Cancelled { duration, .. } => {
            ui.label(
                egui::RichText::new(format!("Cancelled after {}s", duration.as_secs()))
                    .color(t.muted)
                    .size(12.0),
            );
        }
    }
}

/// Render each line in the ring buffer with stream-appropriate
/// tinting. Monospace so PyMapConv's column-aligned diagnostics stay
/// readable.
fn render_log_body(ui: &mut egui::Ui, log: &Mutex<VecDeque<BuildLogLine>>, t: &Tokens) {
    let Ok(guard) = log.lock() else {
        ui.label(
            egui::RichText::new("(log mutex poisoned)")
                .color(egui::Color32::from_rgb(220, 110, 90)),
        );
        return;
    };
    if guard.is_empty() {
        ui.label(
            egui::RichText::new("(no output yet)")
                .color(t.muted)
                .size(11.0),
        );
        return;
    }
    for line in guard.iter() {
        let colour = match line.stream {
            LogStream::Stdout => t.text,
            LogStream::Stderr => egui::Color32::from_rgb(220, 175, 90),
            LogStream::Warn => egui::Color32::from_rgb(220, 175, 90),
            LogStream::Error => egui::Color32::from_rgb(220, 110, 90),
            LogStream::Info => t.muted,
        };
        ui.add(
            egui::Label::new(
                egui::RichText::new(&line.text)
                    .monospace()
                    .color(colour)
                    .size(11.0),
            )
            .wrap(),
        );
    }
}

/// Flatten the ring buffer to a single newline-joined string for the
/// Copy / Save handlers. Each line is prefixed with the stream tag
/// (`[OUT] `, `[ERR] `, `[INF] `) so a saved log is unambiguous when
/// later opened in a plain text editor.
pub fn render_log_as_text(log: &VecDeque<BuildLogLine>) -> String {
    let mut out = String::with_capacity(log.iter().map(|l| l.text.len() + 8).sum());
    for line in log.iter() {
        let tag = match line.stream {
            LogStream::Stdout => "[OUT]",
            LogStream::Stderr => "[ERR]",
            LogStream::Warn => "[WRN]",
            LogStream::Error => "[ERR]",
            LogStream::Info => "[INF]",
        };
        out.push_str(tag);
        out.push(' ');
        out.push_str(&line.text);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn fixture_log() -> VecDeque<BuildLogLine> {
        let mut log = VecDeque::new();
        log.push_back(BuildLogLine {
            text: "▸ PrepareStaging".into(),
            stream: LogStream::Info,
            captured_at: Instant::now(),
        });
        log.push_back(BuildLogLine {
            text: "All Done!".into(),
            stream: LogStream::Stdout,
            captured_at: Instant::now(),
        });
        log.push_back(BuildLogLine {
            text: "warning: NPOT".into(),
            stream: LogStream::Stderr,
            captured_at: Instant::now(),
        });
        log
    }

    /// `render_log_as_text` produces a deterministic newline-joined
    /// flatten suitable for clipboard / file output.
    #[test]
    fn save_format_includes_stream_tags() {
        let log = fixture_log();
        let text = render_log_as_text(&log);
        assert!(text.contains("[INF] ▸ PrepareStaging\n"));
        assert!(text.contains("[OUT] All Done!\n"));
        assert!(text.contains("[ERR] warning: NPOT\n"));
        // Lines end with `\n`.
        assert!(text.ends_with('\n'));
    }

    /// Empty log flattens to the empty string.
    #[test]
    fn save_format_empty_log() {
        let log = VecDeque::new();
        assert_eq!(render_log_as_text(&log), "");
    }
}
