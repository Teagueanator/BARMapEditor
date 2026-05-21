//! Sprint 20 / chunk 4 — build progress overlay.
//!
//! Shown when `App.build_state` is [`build_runner::BuildState::Running`].
//! Renders a centered card over the central viewport listing the
//! current stage, sub-progress (0..1), elapsed time, and the
//! `[Cancel]` / `[View log…]` action buttons.
//!
//! Uses `egui::Area::new(...).order(Foreground)` so it floats above
//! the central panel without participating in the panel-add-order
//! tree. The overlay is non-blocking — clicks outside the card pass
//! through to the underlying viewport — which is intentional: the
//! user can still pan the camera while a 30 s PyMapConv compile runs.

use std::sync::atomic::Ordering;
use std::time::Duration;

use eframe::egui;

use crate::build_runner::BuildState;
use crate::ui::theme::Tokens;

/// Click outcome for the overlay's action buttons. The caller (the
/// App's central panel) translates these into mutations on
/// `App.build_log_open` / the cancel flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayClick {
    None,
    /// User pressed Cancel. The caller should set the
    /// `BuildState::Running { cancel, .. }` flag to `true`.
    Cancel,
    /// User pressed View log. The caller should set
    /// `App.build_log_open = true`.
    ViewLog,
}

/// Render the progress overlay. Returns a [`OverlayClick`] when the
/// user pressed Cancel or View log this frame. Caller is responsible
/// for forwarding the click into the build state machine.
///
/// `viewport_rect` should be the central panel's `max_rect` so the
/// card centres correctly even when side panels resize.
pub fn render(ctx: &egui::Context, viewport_rect: egui::Rect, state: &BuildState) -> OverlayClick {
    let BuildState::Running {
        project_name,
        started_at,
        current_stage,
        latest_progress,
        ..
    } = state
    else {
        return OverlayClick::None;
    };

    let t = Tokens::DARK;
    let card_size = egui::vec2(380.0, 170.0);
    let card_rect = egui::Rect::from_center_size(viewport_rect.center(), card_size);

    let mut click = OverlayClick::None;

    egui::Area::new("build_overlay".into())
        .order(egui::Order::Foreground)
        .fixed_pos(card_rect.min)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_premultiplied(20, 22, 28, 230))
                .stroke(egui::Stroke::new(1.0, t.border))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::same(16))
                .show(ui, |ui| {
                    ui.set_min_width(card_size.x - 32.0);

                    // ─── Header ─────────────────────────────────────
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(format!("Building {project_name}.sd7"))
                                .size(14.0)
                                .color(t.text),
                        );
                    });
                    ui.add_space(10.0);

                    // ─── Stage label + progress bar ─────────────────
                    ui.label(
                        egui::RichText::new(format!("Stage: {}", current_stage.label()))
                            .color(t.muted)
                            .size(12.0),
                    );
                    ui.add_space(4.0);
                    let fraction = (*latest_progress).clamp(0.0, 1.0);
                    let bar = egui::ProgressBar::new(fraction)
                        .show_percentage()
                        .desired_width(card_size.x - 32.0);
                    ui.add(bar);
                    ui.add_space(8.0);

                    // ─── Elapsed ────────────────────────────────────
                    let elapsed = started_at.elapsed();
                    ui.label(
                        egui::RichText::new(format!("Elapsed: {}", format_mmss(elapsed)))
                            .color(t.muted)
                            .size(11.0),
                    );
                    ui.add_space(10.0);

                    // ─── Buttons ────────────────────────────────────
                    ui.horizontal(|ui| {
                        let cancel = ui
                            .add(egui::Button::new(egui::RichText::new("Cancel").size(12.0)))
                            .on_hover_text(
                                "Stop the build. Subprocesses are killed best-effort; the \
                                 temp dir auto-cleans on drop.",
                            );
                        if cancel.clicked() {
                            click = OverlayClick::Cancel;
                        }
                        ui.add_space(8.0);
                        let view_log = ui
                            .add(egui::Button::new(
                                egui::RichText::new("View log…").size(12.0),
                            ))
                            .on_hover_text(
                                "Open the build log panel to see PyMapConv's live output.",
                            );
                        if view_log.clicked() {
                            click = OverlayClick::ViewLog;
                        }
                    });
                });
        });

    click
}

/// Format `elapsed` as `MM:SS` (e.g. `0:14`). Caps at 99 minutes —
/// a build that long is broken regardless.
fn format_mmss(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    let minutes = (secs / 60).min(99);
    let seconds = secs % 60;
    format!("{minutes}:{seconds:02}")
}

/// Wired by the App's central panel after [`render`] returns. Hidden
/// in this module so the caller doesn't have to know that "Cancel"
/// means `flag.store(true, Relaxed)`.
pub fn apply_click(click: OverlayClick, state: &BuildState, build_log_open: &mut bool) {
    match click {
        OverlayClick::None => {}
        OverlayClick::Cancel => {
            if let BuildState::Running { cancel, .. } = state {
                cancel.store(true, Ordering::Relaxed);
                tracing::info!("build_overlay: cancel requested by user");
            }
        }
        OverlayClick::ViewLog => {
            *build_log_open = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn format_mmss_pads_seconds() {
        assert_eq!(format_mmss(Duration::from_secs(0)), "0:00");
        assert_eq!(format_mmss(Duration::from_secs(9)), "0:09");
        assert_eq!(format_mmss(Duration::from_secs(60)), "1:00");
        assert_eq!(format_mmss(Duration::from_secs(125)), "2:05");
        // Caps minutes at 99.
        assert_eq!(format_mmss(Duration::from_secs(99 * 60 + 99)), "99:39");
        assert_eq!(format_mmss(Duration::from_secs(200 * 60)), "99:00");
    }

    /// Apply Cancel against a Running state sets the cancel flag.
    #[test]
    fn apply_click_cancel_sets_flag() {
        use crate::build_runner::{BuildLogLine, BuildState};
        use barme_pipeline::BuildStage;
        use std::collections::VecDeque;
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        use std::sync::mpsc;

        let cancel = Arc::new(AtomicBool::new(false));
        let log = Arc::new(std::sync::Mutex::new(VecDeque::<BuildLogLine>::new()));
        let (_tx, rx) = mpsc::channel::<barme_pipeline::BuildEvent>();
        let state = BuildState::Running {
            project_name: "smoke".into(),
            started_at: Instant::now(),
            current_stage: BuildStage::PrepareStaging,
            latest_progress: 0.1,
            events: rx,
            log,
            cancel: cancel.clone(),
            thread: None,
        };
        let mut log_open = false;
        apply_click(OverlayClick::Cancel, &state, &mut log_open);
        assert!(cancel.load(Ordering::Relaxed));
        assert!(!log_open);
    }

    /// Apply ViewLog against a Running state opens the log panel.
    #[test]
    fn apply_click_view_log_opens_panel() {
        use crate::build_runner::{BuildLogLine, BuildState};
        use barme_pipeline::BuildStage;
        use std::collections::VecDeque;
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        use std::sync::mpsc;

        let (_tx, rx) = mpsc::channel::<barme_pipeline::BuildEvent>();
        let log = Arc::new(std::sync::Mutex::new(VecDeque::<BuildLogLine>::new()));
        let state = BuildState::Running {
            project_name: "smoke".into(),
            started_at: Instant::now(),
            current_stage: BuildStage::PrepareStaging,
            latest_progress: 0.1,
            events: rx,
            log,
            cancel: Arc::new(AtomicBool::new(false)),
            thread: None,
        };
        let mut log_open = false;
        apply_click(OverlayClick::ViewLog, &state, &mut log_open);
        assert!(log_open);
    }
}
