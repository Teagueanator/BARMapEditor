//! Sprint 20 / chunk 8 — "Save before build?" confirmation modal.
//!
//! Stub `egui::Window` with three buttons (Save & build / Build
//! without saving / Cancel). Sprint 31 promotes this to a proper
//! modal primitive (with backdrop, escape key, etc.); the API here
//! is intentionally minimal so the swap is mechanical.

use eframe::egui;

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
    let mut local_open = true;
    egui::Window::new("Save before building?")
        .open(&mut local_open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .default_width(360.0)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(
                    "This project has unsaved changes. The build uses your in-memory edits \
                     regardless — but the .barmeproj on disk won't reflect them until you save.",
                )
                .color(t.muted)
                .size(11.0),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .button("Save & build")
                    .on_hover_text("Save the project to its current path, then start the build.")
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
    if !local_open && choice == SaveBeforeBuildChoice::Dismissed {
        // Close-X: treat as Cancel so the App clears its flag.
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
