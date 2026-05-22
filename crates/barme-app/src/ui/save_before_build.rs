//! Sprint 20 / chunk 8 — "Save before build?" confirmation modal.
//!
//! Sprint 31 / U4 dropped this stub's bespoke window-only shape
//! and gave it the same backdrop + Esc-cancel keyboard that
//! [`crate::ui::confirm::confirm_modal`] uses. The 3-button shape
//! (Save & build / Build without saving / Cancel) is kept — the
//! generic `confirm_modal` is the 2-button (Confirm / Cancel)
//! variant.

use eframe::egui::{self, Color32, CornerRadius, Sense};

use crate::ui::theme::Tokens;

/// User's choice from the modal this frame. The App's `drain_action`
/// matches on this and applies the right combination of save +
/// start-build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveBeforeBuildChoice {
    /// User dismissed without choosing (close-X, click outside —
    /// future, when we get a proper modal). Build is not started.
    Dismissed,
    /// Save first, then start the build. Caller routes through the
    /// existing `FileAction::Save` then `App::build_and_install`.
    SaveAndBuild,
    /// Skip save; start the build against the in-memory snapshot.
    /// User accepts that the on-disk `.barmeproj` lags behind what
    /// they're about to compile.
    BuildWithoutSaving,
    /// Cancel the build entirely. Modal closes; no save, no build.
    Cancel,
}

/// Render the modal. Returns the user's choice this frame; `Dismissed`
/// indicates no decision was made (e.g. the modal stayed open).
///
/// `open` is a mutable bool; the close-X sets it back to `false`
/// (functionally equivalent to `Cancel`).
pub fn render(ctx: &egui::Context, open: &mut bool) -> SaveBeforeBuildChoice {
    if !*open {
        return SaveBeforeBuildChoice::Dismissed;
    }
    let t = Tokens::DARK;
    let mut choice = SaveBeforeBuildChoice::Dismissed;
    let viewport = ctx.content_rect();

    // Sprint 31 / U4 — click-eating backdrop. Same pattern as
    // [`crate::ui::confirm::confirm_modal`].
    egui::Area::new(egui::Id::new("save_before_build_backdrop"))
        .order(egui::Order::Foreground)
        .fixed_pos(viewport.min)
        .interactable(true)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(viewport.size(), Sense::click());
            ui.painter().rect_filled(
                rect,
                CornerRadius::same(0),
                Color32::from_rgba_premultiplied(0, 0, 0, 0x90),
            );
        });

    egui::Area::new(egui::Id::new("save_before_build_dialog"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .interactable(true)
        .show(ctx, |ui| {
            let frame = egui::Frame::new()
                .fill(t.panel)
                .stroke(egui::Stroke::new(1.0, t.border_hi))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::same(16));
            frame.show(ui, |ui| {
                ui.set_min_width(360.0);
                ui.set_max_width(360.0);
                ui.label(
                    egui::RichText::new("Save before building?")
                        .strong()
                        .size(14.0)
                        .color(t.text),
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "This project has unsaved changes. The build uses your in-memory \
                         edits regardless — but the .barmeproj on disk won't reflect them \
                         until you save.",
                    )
                    .color(t.muted)
                    .size(12.0),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui
                        .button("Save & build")
                        .on_hover_text(
                            "Save the project to its current path, then start the build.",
                        )
                        .clicked()
                    {
                        choice = SaveBeforeBuildChoice::SaveAndBuild;
                    }
                    if ui
                        .button("Build without saving")
                        .on_hover_text(
                            "Skip save; build the in-memory snapshot directly. The on-disk \
                             .barmeproj is unchanged.",
                        )
                        .clicked()
                    {
                        choice = SaveBeforeBuildChoice::BuildWithoutSaving;
                    }
                    if ui
                        .button("Cancel")
                        .on_hover_text("Don't build. Return to the editor.")
                        .clicked()
                    {
                        choice = SaveBeforeBuildChoice::Cancel;
                    }
                });
            });
        });

    // Sprint 31 / U4 — Esc cancels (matches confirm_modal).
    if choice == SaveBeforeBuildChoice::Dismissed
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        choice = SaveBeforeBuildChoice::Cancel;
    }

    if choice != SaveBeforeBuildChoice::Dismissed {
        *open = false;
    }
    choice
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Closed modal yields Dismissed and doesn't run any UI.
    #[test]
    fn closed_modal_is_no_op() {
        let ctx = egui::Context::default();
        let mut open = false;
        let mut choice = SaveBeforeBuildChoice::SaveAndBuild;
        let _ = ctx.run(Default::default(), |ctx| {
            choice = render(ctx, &mut open);
        });
        assert_eq!(choice, SaveBeforeBuildChoice::Dismissed);
        assert!(!open);
    }

    /// Open modal stays open across a frame when no button is clicked.
    #[test]
    fn open_modal_stays_open_without_click() {
        let ctx = egui::Context::default();
        let mut open = true;
        let mut choice = SaveBeforeBuildChoice::SaveAndBuild;
        let _ = ctx.run(Default::default(), |ctx| {
            choice = render(ctx, &mut open);
        });
        assert_eq!(choice, SaveBeforeBuildChoice::Dismissed);
        assert!(open);
    }
}
