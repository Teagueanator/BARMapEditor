//! Sprint 22 / U2 — guided tour. A 7-step walkthrough that
//! highlights each editor panel in sequence with a darkened
//! backdrop, a cutout around the target widget, and a small
//! callout Window with `[Next]` / `[Skip tour]` buttons.
//!
//! ## Auto-trigger
//!
//! The tour auto-runs on a brand-new project when
//! [`crate::config::EditorConfig::tour_completed_for_current_version`]
//! is false. The user can skip; the skip persists for the editor
//! version (critical pitfall #1: tour can interfere with active
//! work).
//!
//! ## Re-trigger
//!
//! The Help menu's "Start guided tour" item calls
//! [`crate::config::EditorConfig::reset_tour_completion`] +
//! [`TourState::start`].
//!
//! ## Target rects
//!
//! The tour module does not import `App`; the App pokes rects in
//! per-frame via [`TourState::targets`] before
//! [`render`]. Each [`TourStep`] declares a [`TourTarget`]; if the
//! target rect is absent (e.g. the user closed a panel), the
//! tour silently skips that step (critical pitfall #6).
//!
//! ## Auto-advance
//!
//! After 8 s of inactivity the tour auto-advances. A click on
//! the callout or anywhere on the backdrop resets the timer.
//! Inactivity advance can be disabled via [`TourState::auto_advance`]
//! — Stage 2 polish can expose this in the Help menu.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui;
use tracing::{info, trace};

use crate::ui::theme::Tokens;

/// Targets the tour can highlight. Each one corresponds to an
/// App-rendered widget that registers its rect per-frame via
/// [`TourState::targets`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TourTarget {
    ProjectHeader,
    ToolStrip,
    Inspector,
    Canvas,
    Minimap,
    StatusStrip,
    TopBarHelpIcon,
}

/// One step in the tour. Stable across editor versions — bump
/// [`TOUR_STEPS`] when the layout changes meaningfully.
#[derive(Debug, Clone, Copy)]
pub struct TourStep {
    pub target: TourTarget,
    pub headline: &'static str,
    pub body: &'static str,
    /// Optional one-line action hint shown beneath the body
    /// (e.g. "Press B to enter Sculpt mode").
    pub action_hint: Option<&'static str>,
}

/// Canonical 7-step walkthrough. Order matches the typical user
/// scan: top-left to bottom-right. The tour completes in well
/// under 90 s of user time (critical pitfall: `<90s` is an exit
/// criterion).
pub const TOUR_STEPS: &[TourStep] = &[
    TourStep {
        target: TourTarget::ProjectHeader,
        headline: "Project header",
        body: "Your project's metadata lives here — name, size, heightmap dims. \
               Map size is in Spring Map Units (SMU); 1 SMU = 512 elmos = \
               65 heightmap pixels.",
        action_hint: Some("Heightmap dims always come out to 64·N + 1 — PITFALL §4."),
    },
    TourStep {
        target: TourTarget::ToolStrip,
        headline: "Tools",
        body: "Nine tools live in the left strip — Select, Sculpt, Start \
               positions, Metal spots, Geo vents, Features, Water/Lava, \
               Paint layer, and Procgen. Each has a single-key accelerator.",
        action_hint: Some("Try B for Sculpt, M for Metal spots, L for Paint layer."),
    },
    TourStep {
        target: TourTarget::Inspector,
        headline: "Inspector",
        body: "The right inspector tracks the active tool and surfaces just \
               the controls that apply. Sticky chips at the top recap the \
               global context (symmetry mode + map size) so you don't lose \
               track between tools.",
        action_hint: None,
    },
    TourStep {
        target: TourTarget::Canvas,
        headline: "Canvas",
        body: "The central viewport is a 3D orbit view. Left-drag applies \
               the active tool; right-drag orbits; middle-drag pans; scroll \
               zooms. The Paint layer tool switches to a top-down 2D view \
               of the composite render target.",
        action_hint: Some("Arrow keys pan the camera; Shift makes them 5× faster."),
    },
    TourStep {
        target: TourTarget::Minimap,
        headline: "Minimap",
        body: "The minimap auto-updates as you paint and sculpt. It mirrors \
               what BAR's lobby will display — useful for spotting layout \
               problems before you build.",
        action_hint: None,
    },
    TourStep {
        target: TourTarget::StatusStrip,
        headline: "Status strip",
        body: "Bottom-of-window status — camera, map size, issue count from \
               the lint pass, and the latest build/install outcome. Click \
               the issue count to open the lint panel.",
        action_hint: Some("Click the validation chip in the top-right to open lint."),
    },
    TourStep {
        target: TourTarget::TopBarHelpIcon,
        headline: "Help any time",
        body: "Click the Help icon to re-open the help center. The `?` chord \
               still opens the keyboard cheat sheet; Ctrl+K opens the \
               command palette; Ctrl+Shift+H toggles the what's-this \
               hover-popover mode.",
        action_hint: Some("Tour complete — happy mapping."),
    },
];

/// Mutable runtime state owned by `App`. Persistence lives in
/// [`crate::config::EditorConfig::tour_completed_for`].
#[derive(Debug, Clone)]
pub struct TourState {
    pub active: bool,
    pub step_index: usize,
    pub last_advance: Option<Instant>,
    pub targets: HashMap<TourTarget, egui::Rect>,
    pub auto_advance: bool,
}

impl Default for TourState {
    fn default() -> Self {
        TourState {
            active: false,
            step_index: 0,
            last_advance: None,
            targets: HashMap::new(),
            auto_advance: true,
        }
    }
}

impl TourState {
    /// Begin the walkthrough from step 0. Replays even if the tour
    /// previously completed in this session.
    pub fn start(&mut self) {
        self.active = true;
        self.step_index = 0;
        self.last_advance = Some(Instant::now());
        info!(target: "barme::tour", "guided tour started");
    }

    /// Stop the tour. Caller is responsible for marking the
    /// EditorConfig as completed (so the tour doesn't auto-restart
    /// next launch).
    pub fn end(&mut self) {
        self.active = false;
        self.step_index = 0;
        self.last_advance = None;
        info!(target: "barme::tour", "guided tour ended");
    }

    /// Per-frame: write the rect for `target`. Targets the tour
    /// step does not reference are ignored.
    pub fn set_target_rect(&mut self, target: TourTarget, rect: egui::Rect) {
        if self.active {
            self.targets.insert(target, rect);
        }
    }

    /// Current step, or `None` if the index is out of range
    /// (terminal state).
    pub fn current_step(&self) -> Option<&'static TourStep> {
        TOUR_STEPS.get(self.step_index)
    }

    /// Move to the next step or finish. Returns `true` when the
    /// tour completed this call.
    pub fn advance(&mut self) -> bool {
        if !self.active {
            return false;
        }
        let next = self.step_index + 1;
        if next >= TOUR_STEPS.len() {
            self.end();
            return true;
        }
        self.step_index = next;
        self.last_advance = Some(Instant::now());
        trace!(target: "barme::tour", step = self.step_index, "tour advanced");
        false
    }
}

/// Per-frame outcome from [`render`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TourAction {
    None,
    /// User clicked Next, OR the inactivity timer fired. The
    /// caller already saw `state.step_index` advance.
    Advanced,
    /// User clicked Skip — the caller should mark tour completion
    /// in `EditorConfig` so it doesn't auto-restart.
    Skipped,
    /// User clicked Next on the last step — same as Skipped from
    /// the caller's POV (both finalise completion), but the
    /// "tour finished naturally" code path can log differently.
    Finished,
}

/// Render the tour overlay. Returns the action the user took
/// this frame.
///
/// `viewport_rect` is the full window rect (typically
/// `ctx.screen_rect()`); the backdrop covers it entirely.
pub fn render(ctx: &egui::Context, viewport_rect: egui::Rect, state: &mut TourState) -> TourAction {
    if !state.active {
        return TourAction::None;
    }

    // Resolve the current step's target rect. If absent (panel
    // closed, layout in flux), skip the step on the next frame.
    let Some(step) = state.current_step() else {
        state.end();
        return TourAction::Finished;
    };
    let target_rect = match state.targets.get(&step.target).copied() {
        Some(r) => r,
        None => {
            trace!(target: "barme::tour", target = ?step.target, "tour target absent; skipping step");
            let finished = state.advance();
            return if finished {
                TourAction::Finished
            } else {
                TourAction::Advanced
            };
        }
    };

    let t = Tokens::DARK;

    // ─── Darkened backdrop with target cutout ─────────────────────
    // egui doesn't expose a true cutout; we approximate by painting
    // four rectangles around the target.
    egui::Area::new(egui::Id::new("tour_backdrop"))
        .order(egui::Order::Foreground)
        .fixed_pos(viewport_rect.min)
        .show(ctx, |ui| {
            let painter = ui.painter_at(viewport_rect);
            let dim = egui::Color32::from_rgba_premultiplied(0, 0, 0, 180);
            // Top
            painter.rect_filled(
                egui::Rect::from_min_max(
                    viewport_rect.min,
                    egui::pos2(viewport_rect.max.x, target_rect.min.y),
                ),
                0.0,
                dim,
            );
            // Bottom
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(viewport_rect.min.x, target_rect.max.y),
                    viewport_rect.max,
                ),
                0.0,
                dim,
            );
            // Left
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(viewport_rect.min.x, target_rect.min.y),
                    egui::pos2(target_rect.min.x, target_rect.max.y),
                ),
                0.0,
                dim,
            );
            // Right
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(target_rect.max.x, target_rect.min.y),
                    egui::pos2(viewport_rect.max.x, target_rect.max.y),
                ),
                0.0,
                dim,
            );
            // Highlight ring around target
            painter.rect_stroke(
                target_rect,
                4.0,
                egui::Stroke::new(2.0, egui::Color32::from_rgb(220, 175, 90)),
                egui::StrokeKind::Outside,
            );
        });

    // ─── Callout window ───────────────────────────────────────────
    // Anchor the callout adjacent to the target — to the right if
    // there's room, otherwise below.
    let card_size = egui::vec2(360.0, 140.0);
    let mut anchor = egui::pos2(
        target_rect.max.x + 16.0,
        target_rect.min.y.max(viewport_rect.min.y + 8.0),
    );
    if anchor.x + card_size.x > viewport_rect.max.x - 8.0 {
        anchor = egui::pos2(
            target_rect.min.x.max(viewport_rect.min.x + 8.0),
            target_rect.max.y + 16.0,
        );
    }
    if anchor.y + card_size.y > viewport_rect.max.y - 8.0 {
        anchor.y = viewport_rect.max.y - card_size.y - 8.0;
    }

    let mut action = TourAction::None;
    let total_steps = TOUR_STEPS.len();
    let step_index = state.step_index;
    let step = *step;

    egui::Area::new(egui::Id::new("tour_callout"))
        .order(egui::Order::Foreground)
        .fixed_pos(anchor)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_premultiplied(24, 26, 32, 245))
                .stroke(egui::Stroke::new(1.0, t.border))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::same(14))
                .show(ui, |ui| {
                    ui.set_min_width(card_size.x - 28.0);
                    ui.set_max_width(card_size.x - 28.0);

                    // Header: step counter + headline
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "Step {}/{}",
                                step_index + 1,
                                total_steps
                            ))
                            .color(t.muted)
                            .size(11.0),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button("Skip tour")
                                .on_hover_text(
                                    "Skip the rest of the tour. Re-run anytime \
                                     from Help > Start guided tour.",
                                )
                                .clicked()
                            {
                                action = TourAction::Skipped;
                            }
                        });
                    });
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(step.headline)
                            .color(t.text)
                            .size(14.0)
                            .strong(),
                    );
                    ui.add_space(4.0);

                    // Body
                    ui.label(
                        egui::RichText::new(step.body)
                            .color(t.text)
                            .size(12.0),
                    );
                    if let Some(hint) = step.action_hint {
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(hint)
                                .color(t.muted)
                                .size(11.0)
                                .italics(),
                        );
                    }

                    ui.add_space(10.0);

                    // Footer: Next / Finish
                    ui.horizontal(|ui| {
                        let is_last = step_index + 1 >= total_steps;
                        let primary_label = if is_last { "Finish" } else { "Next" };
                        if ui
                            .button(
                                egui::RichText::new(primary_label).size(12.0).strong(),
                            )
                            .clicked()
                        {
                            action = if is_last {
                                TourAction::Finished
                            } else {
                                TourAction::Advanced
                            };
                        }
                    });
                });
        });

    // Inactivity auto-advance.
    if state.auto_advance
        && matches!(action, TourAction::None)
        && let Some(last) = state.last_advance
        && last.elapsed() >= Duration::from_secs(8)
    {
        trace!(target: "barme::tour", step = state.step_index, "tour auto-advancing on inactivity");
        let finished = state.advance();
        return if finished {
            TourAction::Finished
        } else {
            TourAction::Advanced
        };
    }

    match action {
        TourAction::Advanced => {
            let finished = state.advance();
            if finished {
                TourAction::Finished
            } else {
                TourAction::Advanced
            }
        }
        TourAction::Skipped => {
            state.end();
            TourAction::Skipped
        }
        TourAction::Finished => {
            state.end();
            TourAction::Finished
        }
        TourAction::None => TourAction::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sprint 22 exit criterion: ≥7 tour steps.
    #[test]
    fn at_least_seven_tour_steps() {
        assert!(
            TOUR_STEPS.len() >= 7,
            "Sprint 22 requires ≥7 tour steps; got {}",
            TOUR_STEPS.len(),
        );
    }

    /// All tour steps target distinct widget surfaces — pin so a
    /// future edit can't collapse two steps onto the same target
    /// silently.
    #[test]
    fn tour_targets_are_distinct() {
        let mut seen = std::collections::HashSet::new();
        for step in TOUR_STEPS {
            assert!(
                seen.insert(step.target),
                "duplicate target {:?} in TOUR_STEPS",
                step.target
            );
        }
    }

    /// Every step has a non-empty headline + body.
    #[test]
    fn tour_steps_have_content() {
        for step in TOUR_STEPS {
            assert!(!step.headline.is_empty(), "empty headline for {:?}", step.target);
            assert!(!step.body.is_empty(), "empty body for {:?}", step.target);
        }
    }

    /// Default state is inactive at step 0.
    #[test]
    fn default_state_is_inactive() {
        let s = TourState::default();
        assert!(!s.active);
        assert_eq!(s.step_index, 0);
        assert!(s.targets.is_empty());
    }

    /// `start` activates the tour and records a last-advance
    /// timestamp.
    #[test]
    fn start_activates_and_arms_timer() {
        let mut s = TourState::default();
        s.start();
        assert!(s.active);
        assert_eq!(s.step_index, 0);
        assert!(s.last_advance.is_some());
    }

    /// `advance` from any non-terminal step increments the index
    /// and returns false.
    #[test]
    fn advance_increments_and_does_not_finish_mid_tour() {
        let mut s = TourState::default();
        s.start();
        let finished = s.advance();
        assert!(!finished);
        assert_eq!(s.step_index, 1);
        assert!(s.active);
    }

    /// `advance` from the last step finishes the tour.
    #[test]
    fn advance_from_last_step_finishes() {
        let mut s = TourState::default();
        s.start();
        s.step_index = TOUR_STEPS.len() - 1;
        let finished = s.advance();
        assert!(finished);
        assert!(!s.active);
        assert_eq!(s.step_index, 0);
    }

    /// `set_target_rect` is no-op when the tour is inactive.
    #[test]
    fn set_target_rect_is_no_op_when_inactive() {
        let mut s = TourState::default();
        s.set_target_rect(
            TourTarget::ToolStrip,
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(10.0, 10.0)),
        );
        assert!(s.targets.is_empty());
    }

    /// `set_target_rect` records when the tour is active.
    #[test]
    fn set_target_rect_records_when_active() {
        let mut s = TourState::default();
        s.start();
        let r = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(10.0, 10.0));
        s.set_target_rect(TourTarget::ToolStrip, r);
        assert_eq!(s.targets.get(&TourTarget::ToolStrip).copied(), Some(r));
    }

    /// `current_step` returns Some up to the last step, None
    /// once past it.
    #[test]
    fn current_step_is_some_in_range_none_past() {
        let mut s = TourState::default();
        s.start();
        for i in 0..TOUR_STEPS.len() {
            s.step_index = i;
            assert!(s.current_step().is_some());
        }
        s.step_index = TOUR_STEPS.len();
        assert!(s.current_step().is_none());
    }

    /// `end` clears active + resets step index.
    #[test]
    fn end_clears_active_and_resets_index() {
        let mut s = TourState::default();
        s.start();
        s.step_index = 3;
        s.end();
        assert!(!s.active);
        assert_eq!(s.step_index, 0);
        assert!(s.last_advance.is_none());
    }
}
