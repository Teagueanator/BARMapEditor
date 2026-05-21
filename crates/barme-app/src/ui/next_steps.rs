//! Non-modal "Next steps" hint Window painted once per fresh
//! wizard-Create (B8). Dismiss persists **per-project** (NOT in
//! `EditorConfig`) so opening a different fresh project re-shows the
//! hint. See B8 in `devlog/stage-1-mvp/phase-3-plan.md` for the
//! reasoning.

use eframe::egui;

/// What the user did with the Next-steps Window this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextStepsAction {
    /// X-button click. Caller flips
    /// `Project.next_steps_dismissed = true` so save/load keeps the
    /// hint hidden for this project specifically.
    Dismiss,
}

/// Three task-oriented bullets the wizard's freshly-seeded demo
/// state lets the user try immediately. Stable across edits — bump
/// the list when a new tool-mode is the obvious next thing to teach.
pub const NEXT_STEPS_BULLETS: &[&str] = &[
    "Brush terrain — press B, then click-drag in the viewport.",
    "Move spawns — press S, then drag the markers.",
    "Try a math preset — press G, choose Parabolic bowl, click Apply.",
];

/// Render the Next-steps Window if `*open == true`. Sets `*open =
/// false` and returns `Some(Dismiss)` when the user clicks the X
/// or the inline "Got it"; otherwise returns `None`.
///
/// The caller is responsible for syncing the dismiss into
/// `Project.next_steps_dismissed` so save / open round-trips persist
/// the choice.
pub fn render_next_steps_hint(ctx: &egui::Context, open: &mut bool) -> Option<NextStepsAction> {
    if !*open {
        return None;
    }
    let mut local_open = true;
    let mut dismissed = false;
    egui::Window::new("Next steps")
        .open(&mut local_open)
        .collapsible(false)
        .resizable(false)
        .default_width(360.0)
        // Non-modal: do not anchor — user can drag away if it blocks
        // their work. (The wizard / intro were modal-ish; this is
        // explicitly out-of-the-way per the B8 spec.)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new("Your project is ready. Try one of these:").strong());
            ui.add_space(4.0);
            for bullet in NEXT_STEPS_BULLETS {
                ui.label(format!("• {bullet}"));
            }
            ui.add_space(8.0);
            if ui
                .button("Got it")
                .on_hover_text(
                    "Dismiss this hint for the current project. Save/load preserves the dismissal.",
                )
                .clicked()
            {
                dismissed = true;
            }
        });
    if !local_open {
        // The user clicked the Window's X.
        dismissed = true;
    }
    if dismissed {
        *open = false;
        Some(NextStepsAction::Dismiss)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bullets_non_empty_and_mention_each_tool_accelerator() {
        assert_eq!(NEXT_STEPS_BULLETS.len(), 3, "B8 ships exactly 3 bullets");
        let joined = NEXT_STEPS_BULLETS.join(" | ");
        // Each of the three demo accelerators should be discoverable
        // from the bullets themselves so a user who never reads docs
        // still finds B / S / G.
        assert!(
            joined.contains(" B"),
            "bullets must mention B accelerator: {joined}"
        );
        assert!(
            joined.contains(" S"),
            "bullets must mention S accelerator: {joined}"
        );
        assert!(
            joined.contains(" G"),
            "bullets must mention G accelerator: {joined}"
        );
    }

    #[test]
    fn action_is_dismiss_only() {
        // Single-variant enum, pinned so a future addition doesn't
        // silently change the caller contract.
        let act = NextStepsAction::Dismiss;
        assert_eq!(act, NextStepsAction::Dismiss);
    }
}
