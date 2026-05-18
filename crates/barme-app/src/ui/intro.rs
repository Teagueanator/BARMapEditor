//! First-launch hint Window painted once per editor version. The
//! "dismissed" flag persists in [`crate::config::EditorConfig`] —
//! pitfall §B3.4 forbids putting it in `.barmeproj`.

use eframe::egui;

/// What the user did with the intro Window this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntroAction {
    /// User clicked "Got it" or the Window's X. Caller should mark
    /// the current version as seen + persist to disk.
    Dismiss,
}

/// Bullet list rendered inside the Window. Stable across versions —
/// when the editor gains a different "first 30 seconds" interaction,
/// bump these bullets and the version-keyed flag (re-shows once per
/// distinct version).
pub const INTRO_BULLETS: &[&str] = &[
    "LMB drag to sculpt (B / Sculpt tool).",
    "RMB drag to orbit the camera.",
    "Press ? at any time for the full cheat-sheet.",
];

/// Render the intro Window if `*open == true`. Sets `*open = false`
/// and returns `Some(IntroAction::Dismiss)` when the user dismisses;
/// otherwise returns `None`. The caller is responsible for the
/// version-flag write + disk save when it sees a `Dismiss`.
pub fn render_intro_hint(ctx: &egui::Context, open: &mut bool) -> Option<IntroAction> {
    if !*open {
        return None;
    }
    let mut local_open = true;
    let mut dismissed = false;
    egui::Window::new("Welcome to BAR Map Editor")
        .open(&mut local_open)
        .collapsible(false)
        .resizable(false)
        .default_width(380.0)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new("Quick start — three things to know:").strong());
            ui.add_space(4.0);
            for bullet in INTRO_BULLETS {
                ui.label(format!("• {bullet}"));
            }
            ui.add_space(8.0);
            if ui.button("Got it").clicked() {
                dismissed = true;
            }
        });
    if !local_open {
        // The user clicked the Window's X.
        dismissed = true;
    }
    if dismissed {
        *open = false;
        Some(IntroAction::Dismiss)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intro_bullets_non_empty() {
        assert!(!INTRO_BULLETS.is_empty());
        for b in INTRO_BULLETS {
            assert!(!b.is_empty(), "empty intro bullet");
        }
    }

    #[test]
    fn intro_bullets_mention_core_interactions() {
        // Belt-and-braces — the intro exists to teach these. A future
        // rewording must keep the keywords visible.
        let joined: String = INTRO_BULLETS.join(" | ").to_lowercase();
        assert!(joined.contains("lmb"), "no LMB mention: {joined}");
        assert!(joined.contains("rmb"), "no RMB mention: {joined}");
        assert!(joined.contains("?"), "no ? mention: {joined}");
    }

    #[test]
    fn intro_action_is_dismiss_only() {
        // Single-variant enum, but pin it so a future ADD doesn't
        // silently change the caller contract.
        let act = IntroAction::Dismiss;
        assert_eq!(act, IntroAction::Dismiss);
    }
}
