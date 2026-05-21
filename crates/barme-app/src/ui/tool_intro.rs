//! Sprint 22 / U2 — per-tool intro overlay.
//!
//! When the user enters a tool for the first time (i.e. the tool's
//! keyboard accelerator isn't in
//! [`crate::config::EditorConfig::tool_intros_seen`]), pop a small
//! non-modal Window in the central viewport with the essentials:
//! what the tool does, the gesture set, and "Read more in Help
//! Center" / "Don't show again" buttons.
//!
//! ## Persistence semantics
//!
//! - **Esc / X / click-outside** = "I get it for now; show me next
//!   time." Does NOT add the accel to `tool_intros_seen` (critical
//!   pitfall #2).
//! - **"Don't show again" checkbox + close** = persistent dismissal.
//!   Adds the accel to `tool_intros_seen`.
//! - The Help menu's "Reset tool intros" item clears the set so all
//!   intros replay on next entry.

use eframe::egui;
use tracing::{info, trace};

use crate::ui::help_center::HelpArticleId;
use crate::ui::theme::Tokens;

/// Static content for one tool's intro.
#[derive(Debug, Clone, Copy)]
pub struct ToolIntroContent {
    pub title: &'static str,
    /// Paragraphs of body text. Rendered as separate `egui::Label`s
    /// with vertical spacing between.
    pub body: &'static [&'static str],
    /// Optional control table: `(combo, action)` pairs rendered as a
    /// two-column grid below the body.
    pub controls: &'static [(&'static str, &'static str)],
    /// The article in the help center the "Read more" button jumps
    /// to.
    pub article: HelpArticleId,
}

/// Lookup the intro for a tool accelerator. Returns `None` for
/// unrecognised accels.
pub fn intro_for(accel: &str) -> Option<&'static ToolIntroContent> {
    Some(match accel {
        "Q" => &SELECT_INTRO,
        "B" => &SCULPT_INTRO,
        "S" => &START_POSITIONS_INTRO,
        "M" => &METAL_SPOTS_INTRO,
        "V" => &GEO_FEATURES_INTRO,
        "F" => &FEATURE_INTRO,
        "W" => &WATER_INTRO,
        "L" => &PAINT_LAYER_INTRO,
        "G" => &PROCGEN_INTRO,
        _ => return None,
    })
}

/// All registered tool accel keys — pinned by tests + used by the
/// "Reset tool intros" menu item (and to verify the App's
/// `Tool::ALL` stays in sync).
#[allow(dead_code)] // wired by the Help > Reset tool intros menu item in commit 6
pub const ALL_TOOL_ACCELS: &[&str] = &["Q", "B", "S", "M", "V", "F", "W", "L", "G"];

// ─── Content ───────────────────────────────────────────────────────

const SELECT_INTRO: ToolIntroContent = ToolIntroContent {
    title: "Select / orbit",
    body: &[
        "Camera-only mode. No canvas editing — useful when you want \
         to look around without risking an accidental stamp.",
        "Pick a tool from the left strip or press its accelerator \
         (B for Sculpt, M for Metal spots, …) to start editing.",
    ],
    controls: &[
        ("LMB drag", "Orbit camera"),
        ("MMB drag", "Pan"),
        ("RMB drag", "Orbit"),
        ("Scroll", "Zoom"),
        ("Arrow keys", "Pan (Shift = 5×)"),
    ],
    article: HelpArticleId::ToolSelect,
};

const SCULPT_INTRO: ToolIntroContent = ToolIntroContent {
    title: "Sculpt",
    body: &[
        "Heightmap brush — the headline tool. Pick Raise, Lower, or \
         Smooth from the Inspector; LMB-drag stamps the active brush.",
        "Radius (8..4096 elmos) and strength (0..1) tune the per-stamp \
         behaviour. 1 SMU = 512 elmos = 65 heightmap pixels, so a \
         256-elmo brush is about half an SMU wide.",
        "Symmetry replicates strokes across the chosen axis — toggle \
         from the top bar. Ctrl+Z undoes.",
    ],
    controls: &[
        ("LMB drag", "Stamp active brush"),
        ("RMB drag", "Orbit camera"),
        ("Ctrl+Z", "Undo"),
    ],
    article: HelpArticleId::ToolSculpt,
};

const START_POSITIONS_INTRO: ToolIntroContent = ToolIntroContent {
    title: "Start positions",
    body: &[
        "Place per-ally-team spawn positions. The Inspector ships a \
         tree of ally teams — colour, name, and active marker. The \
         active team (star icon) receives new placements.",
        "Drag-paint distributes N positions evenly along an LMB drag \
         (count configurable in the Inspector). LMB on an existing \
         marker drag-moves it. RMB deletes.",
        "Stock presets cover OneVOne, TwoVTwo, EightVEight (corner \
         mirror), and FFA layouts (120° / 90° rotations).",
    ],
    controls: &[
        ("LMB", "Place / drag"),
        ("LMB drag (empty)", "Drag-paint N positions"),
        ("RMB", "Delete"),
    ],
    article: HelpArticleId::ToolStartPositions,
};

const METAL_SPOTS_INTRO: ToolIntroContent = ToolIntroContent {
    title: "Metal spots",
    body: &[
        "Place mex extraction points. BAR's gui_metalspots widget \
         reads spots from mapconfig/map_metal_layout.lua; the SMF \
         metalmap stays all-zero (PITFALL §13).",
        "Per-spot yield: 0.5 = perimeter, 2.0 = standard, 4.0–5.2 = \
         strategic. Set freely via the Inspector DragValue.",
        "extractor_radius defaults to 80 (BAR convention). The chip \
         flags any other value — 500 (engine default) breaks \
         mex-snap.",
    ],
    controls: &[
        ("LMB", "Place spot"),
        ("LMB drag", "Move spot"),
        ("RMB", "Delete"),
    ],
    article: HelpArticleId::ToolMetalSpots,
};

const GEO_FEATURES_INTRO: ToolIntroContent = ToolIntroContent {
    title: "Geo vents",
    body: &[
        "Place steam vents — the geovent feature BAR scans for to \
         seed geothermal generator slots. Emits through the \
         Springboard featureplacer trio (PITFALL §14 + §21 + §25), \
         not a geos = {…} array.",
        "Geo vents are gameplay-critical so they live as their own \
         tool. General features (trees, rocks, props) live under \
         the F tool.",
    ],
    controls: &[
        ("LMB", "Place vent"),
        ("LMB drag", "Move vent"),
        ("RMB", "Delete"),
    ],
    article: HelpArticleId::ToolGeoFeatures,
};

const FEATURE_INTRO: ToolIntroContent = ToolIntroContent {
    title: "Features",
    body: &[
        "General feature placement: trees, rocks, props, wreckage. \
         The Inspector ships a category combo + filter to find \
         features quickly. Click a row to arm it for placement.",
        "LMB places the armed feature; LMB-drag on an existing \
         instance rotates it (~1° per pixel). RMB deletes. Rotation \
         emits as an unquoted Lua integer (PITFALL §23).",
    ],
    controls: &[
        ("LMB", "Place armed feature"),
        ("LMB drag", "Rotate instance"),
        ("RMB", "Delete"),
    ],
    article: HelpArticleId::ToolFeature,
};

const WATER_INTRO: ToolIntroContent = ToolIntroContent {
    title: "Water / Lava",
    body: &[
        "Map-property tool — pick a water preset (Ocean / Tropical / \
         Acid / Lava / Magma) and patch the mapinfo water block. The \
         Inspector splits into Preset / Behaviour / Appearance / \
         Flood / Advanced sections.",
        "BAR's water plane is consteval at Y = 0 (PITFALL §28) — you \
         can't move the water; you raise or lower min_height so the \
         terrain dips below sea level. The auto-min-height shortcut \
         pairs the carve depth with min_height in one click.",
    ],
    controls: &[
        ("LMB drag", "Flood (Lower brush)"),
        ("RMB drag", "Raise terrain"),
    ],
    article: HelpArticleId::ToolWater,
};

const PAINT_LAYER_INTRO: ToolIntroContent = ToolIntroContent {
    title: "Paint layer",
    body: &[
        "The Photoshop-style layered painter (Sprints 15–17). The \
         central viewport switches to a top-down 2D view of the \
         composite render target.",
        "The right inspector becomes a Layers panel — each row has \
         a thumbnail, visibility toggle, blend mode, opacity, and a \
         context caret for slot-change / duplicate / import.",
        "Mask brushes: Reveal (alpha → 1), Hide (alpha → 0), Smooth, \
         Fill. Strength tunes per-stamp delta; spacing tunes drag \
         density.",
    ],
    controls: &[
        ("LMB drag", "Paint active layer mask"),
        ("RMB drag", "Orbit camera"),
        ("MMB drag", "Pan"),
        ("Scroll", "Zoom"),
    ],
    article: HelpArticleId::ToolPaintLayer,
};

const PROCGEN_INTRO: ToolIntroContent = ToolIntroContent {
    title: "Procgen",
    body: &[
        "Math-function heightmap generator. Type a formula f(x, z) \
         → [0, 1] and Apply replaces the current heightmap.",
        "Two domain choices: Unit (x, z ∈ 0..1) and Centered (x, z \
         ∈ -1..1). The 256² live preview thumbnail refreshes ~50 ms \
         after the last keystroke.",
        "Apply is undoable (Ctrl+Z reverts). Stock presets seed \
         common shapes: Parabolic bowl, Saddle, Diagonal ramp, \
         Plateau, Custom.",
    ],
    controls: &[
        ("Type expression", "Live preview rebakes"),
        ("Apply", "Commit to heightmap"),
        ("Ctrl+Z", "Revert"),
    ],
    article: HelpArticleId::ToolProcgen,
};

// ─── Runtime state + render ────────────────────────────────────────

/// Mutable runtime state owned by `App`. Set
/// `pending = Some(accel)` to pop the intro for that tool on the
/// next frame; the [`render`] entry point clears it on user action.
#[derive(Debug, Clone, Default)]
pub struct ToolIntroState {
    pub pending: Option<String>,
    /// Local "don't show again" checkbox state for the current
    /// frame's intro. Reset on every fresh pop.
    pub dont_show_again: bool,
}

impl ToolIntroState {
    /// Reset to a fresh "no intro pending" state.
    pub fn dismiss_local(&mut self) {
        self.pending = None;
        self.dont_show_again = false;
    }
}

/// Per-frame outcome from [`render`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolIntroAction {
    /// User did nothing this frame.
    None,
    /// User dismissed via Esc / X (NO "Don't show again" check) —
    /// the intro pops again next time they enter the tool.
    DismissTemp,
    /// User dismissed with "Don't show again" checked — caller
    /// records `accel` in `EditorConfig.tool_intros_seen`.
    DismissPersist { accel: String },
    /// User clicked "Read more in Help Center" — caller opens the
    /// help center at `article`.
    OpenHelp { article: HelpArticleId },
}

/// Render the intro Window for the tool whose accelerator is
/// `state.pending`. Returns the user's action this frame.
pub fn render(ctx: &egui::Context, state: &mut ToolIntroState) -> ToolIntroAction {
    let Some(accel) = state.pending.clone() else {
        return ToolIntroAction::None;
    };
    let Some(content) = intro_for(&accel) else {
        // Unrecognised accel — clear and bail. Belt-and-braces.
        state.dismiss_local();
        return ToolIntroAction::None;
    };
    let t = Tokens::DARK;
    let mut local_open = true;
    let mut action = ToolIntroAction::None;
    egui::Window::new(content.title)
        .open(&mut local_open)
        .collapsible(false)
        .resizable(false)
        .default_width(380.0)
        .show(ctx, |ui| {
            for paragraph in content.body {
                ui.label(egui::RichText::new(*paragraph).color(t.text).size(12.0));
                ui.add_space(4.0);
            }
            if !content.controls.is_empty() {
                ui.add_space(4.0);
                egui::Grid::new(format!("tool_intro_controls_{accel}"))
                    .num_columns(2)
                    .spacing(egui::vec2(12.0, 4.0))
                    .show(ui, |ui| {
                        for (combo, what) in content.controls {
                            ui.label(egui::RichText::new(*combo).monospace().color(t.muted));
                            ui.label(egui::RichText::new(*what).color(t.text));
                            ui.end_row();
                        }
                    });
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .button("Read more in Help Center")
                    .on_hover_text("Open the full article in the help center.")
                    .clicked()
                {
                    action = ToolIntroAction::OpenHelp {
                        article: content.article,
                    };
                }
                ui.add_space(8.0);
                ui.checkbox(&mut state.dont_show_again, "Don't show again")
                    .on_hover_text("Persist across sessions. Re-arm via Help > Reset tool intros.");
            });
        });
    if !local_open {
        action = if state.dont_show_again {
            ToolIntroAction::DismissPersist {
                accel: accel.clone(),
            }
        } else {
            ToolIntroAction::DismissTemp
        };
    } else if matches!(action, ToolIntroAction::OpenHelp { .. }) {
        // Read-more click is sticky: it does NOT close the intro
        // popup. The caller opens the help center; if the user
        // then wants to dismiss, they click X.
    }

    // Trace + clear local state when the user took an action.
    match &action {
        ToolIntroAction::None => {}
        ToolIntroAction::DismissTemp => {
            trace!(target: "barme::tool_intro", accel = %accel, "intro dismissed (temporary)");
            state.dismiss_local();
        }
        ToolIntroAction::DismissPersist { accel } => {
            info!(target: "barme::tool_intro", %accel, "intro pinned 'don't show again'");
            state.dismiss_local();
        }
        ToolIntroAction::OpenHelp { article } => {
            trace!(target: "barme::tool_intro", accel = %accel, ?article, "intro: read-more clicked");
        }
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tool accel from `Tool::ALL` has an intro entry. We
    /// hand-tabulate `Tool::ALL`'s accels here (`Q`, `B`, `S`, `M`,
    /// `V`, `F`, `W`, `L`, `G`) so a new tool variant must update
    /// both `Tool::ALL` and `intro_for`.
    #[test]
    fn intro_covers_every_tool_accel() {
        for accel in ALL_TOOL_ACCELS {
            assert!(
                intro_for(accel).is_some(),
                "no tool intro for accel `{accel}`",
            );
        }
    }

    /// Every intro has a non-empty title, body, and article id.
    #[test]
    fn intros_have_content() {
        for accel in ALL_TOOL_ACCELS {
            let intro = intro_for(accel).expect("intro registered");
            assert!(!intro.title.is_empty(), "empty title for accel `{accel}`");
            assert!(!intro.body.is_empty(), "empty body for accel `{accel}`");
            for paragraph in intro.body {
                assert!(
                    !paragraph.is_empty(),
                    "empty paragraph in intro for `{accel}`"
                );
            }
        }
    }

    /// Article ids referenced by tool intros all exist in the help
    /// center catalogue. Catches a typo / out-of-sync rename.
    #[test]
    fn intro_article_ids_exist_in_help_catalogue() {
        for accel in ALL_TOOL_ACCELS {
            let intro = intro_for(accel).expect("intro registered");
            assert!(
                HelpArticleId::ALL.contains(&intro.article),
                "intro for `{accel}` references missing article {:?}",
                intro.article,
            );
        }
    }

    /// `intro_for` on an unknown accel returns None.
    #[test]
    fn intro_for_unknown_accel_returns_none() {
        assert!(intro_for("Z").is_none());
        assert!(intro_for("").is_none());
    }

    /// Default state is empty / no pending intro.
    #[test]
    fn default_state_has_no_pending() {
        let s = ToolIntroState::default();
        assert!(s.pending.is_none());
        assert!(!s.dont_show_again);
    }

    /// `dismiss_local` clears pending + the checkbox state.
    #[test]
    fn dismiss_local_clears_state() {
        let mut s = ToolIntroState {
            pending: Some("L".into()),
            dont_show_again: true,
        };
        s.dismiss_local();
        assert!(s.pending.is_none());
        assert!(!s.dont_show_again);
    }

    /// Exit gesture inference: `DismissPersist` carries the accel
    /// for the caller's `EditorConfig.mark_tool_intro_seen(&accel)`.
    #[test]
    fn dismiss_persist_carries_accel() {
        let a = ToolIntroAction::DismissPersist { accel: "L".into() };
        match a {
            ToolIntroAction::DismissPersist { accel } => assert_eq!(accel, "L"),
            _ => panic!("expected DismissPersist"),
        }
    }
}
