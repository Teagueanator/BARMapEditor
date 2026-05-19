//! `?` cheat-sheet modal. Auto-generated from the runtime `Tool`
//! enum + a static camera-binding table so a static `keymaps.md`
//! that drifts from code can never exist (pitfall §B3.2).

use eframe::egui;

/// One row in the cheat-sheet table. Kept for the test harness +
/// future plain-text export; the runtime renderer now walks the
/// `*_BINDINGS` tables directly.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheatSheetEntry {
    pub keys: String,
    pub action: String,
}

/// Per-tool entries the cheat-sheet renders. Callers pass a slice of
/// `(accelerator, label)` pairs — typically built from `Tool::ALL` in
/// the caller — so this module doesn't import `Tool` and stays
/// independent of `main.rs`.
pub type ToolBinding<'a> = (&'a str, &'a str);

/// Camera + global bindings, hand-maintained. New global bindings
/// (e.g. cheat-sheet `?` itself) must be added here in lockstep
/// with the keyboard handler. ADR-035 removed the XYZ nav gizmo —
/// the mini-map is read-only for camera input at the moment.
pub const CAMERA_BINDINGS: &[(&str, &str)] = &[
    ("LMB drag", "Sculpt (in Sculpt mode), orbit otherwise"),
    ("RMB drag", "Orbit camera (in tool mode)"),
    ("Scroll wheel", "Zoom in / out"),
    ("Ctrl+Z", "Undo"),
    ("Ctrl+Shift+Z / Ctrl+Y", "Redo"),
    ("?", "Open this cheat-sheet"),
    ("Esc", "Close cheat-sheet / dismiss intro"),
];

/// Sculpt-mode bindings (ADR-035). The tool-specific group on the
/// cheat sheet. Today only the symmetry toggle is bound; the bracket
/// brush-resize keys are reserved (Phase 9+).
pub const SCULPT_BINDINGS: &[(&str, &str)] = &[
    ("X", "Toggle symmetry (reserved)"),
    ("[", "Brush radius − (reserved)"),
    ("]", "Brush radius + (reserved)"),
    ("Shift+drag", "Smooth (reserved)"),
];

/// Project lifecycle bindings (ADR-035).
pub const PROJECT_BINDINGS: &[(&str, &str)] = &[
    ("Ctrl+S", "Save"),
    ("Ctrl+Shift+S", "Save as…"),
    ("Ctrl+B", "Build & install (reserved)"),
];

/// Build the full keymap table. First comes a "Tools" section
/// generated from `tools`, then the global / camera section.
///
/// Tools render as `(accel, "Switch to <label>")`. Camera entries
/// render verbatim.
#[allow(dead_code)]
pub fn cheat_sheet_entries(tools: &[ToolBinding<'_>]) -> Vec<CheatSheetEntry> {
    let mut out = Vec::with_capacity(tools.len() + CAMERA_BINDINGS.len());
    for (accel, label) in tools {
        out.push(CheatSheetEntry {
            keys: (*accel).to_string(),
            action: format!("Switch to {label}"),
        });
    }
    for (keys, action) in CAMERA_BINDINGS {
        out.push(CheatSheetEntry {
            keys: (*keys).to_string(),
            action: (*action).to_string(),
        });
    }
    out
}

/// Number of entries the cheat-sheet will produce for `tools`. Used
/// by tests that pin the count against `Tool::ALL.len()` so a new
/// `Tool` variant must update either the test or the static table.
/// Test-only — the runtime path uses `cheat_sheet_entries().len()`
/// directly.
#[cfg(test)]
pub fn cheat_sheet_entry_count(tool_count: usize) -> usize {
    tool_count + CAMERA_BINDINGS.len()
}

/// Render the cheat-sheet `egui::Window` (ADR-035). Two-column grid
/// of grouped bindings (Camera / Tools / Sculpt / Project) with
/// `key_combo`-style chips. Closes on the X button or when `open` is
/// set to `false` externally (e.g. via the Esc key handler in
/// `App::handle_keyboard`).
pub fn render_cheat_sheet(ctx: &egui::Context, open: &mut bool, tools: &[ToolBinding<'_>]) {
    if !*open {
        return;
    }
    let t = crate::ui::theme::Tokens::DARK;
    let tools_section: Vec<(String, String)> = tools
        .iter()
        .map(|(accel, label)| ((*accel).to_string(), (*label).to_string()))
        .collect();
    let mut local_open = true;
    egui::Window::new("Keyboard reference")
        .open(&mut local_open)
        .collapsible(false)
        .resizable(false)
        .default_width(680.0)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new("All shortcuts, grouped by context.")
                    .color(t.muted)
                    .size(12.0),
            );
            ui.add_space(8.0);
            ui.columns(2, |cols| {
                render_group(&mut cols[0], "Camera", CAMERA_BINDINGS);
                render_group(&mut cols[1], "Tools", &as_str_slice(&tools_section));
                render_group(&mut cols[0], "Sculpt", SCULPT_BINDINGS);
                render_group(&mut cols[1], "Project", PROJECT_BINDINGS);
            });
        });
    if !local_open {
        *open = false;
    }
}

fn as_str_slice(v: &[(String, String)]) -> Vec<(&str, &str)> {
    v.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect()
}

fn render_group(ui: &mut egui::Ui, title: &str, bindings: &[(&str, &str)]) {
    let t = crate::ui::theme::Tokens::DARK;
    ui.label(
        egui::RichText::new(title.to_uppercase())
            .color(t.muted)
            .size(10.0)
            .strong(),
    );
    ui.add_space(4.0);
    for (keys, action) in bindings {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(*action).color(t.text).size(12.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                crate::ui::widgets::key_combo(ui, keys);
            });
        });
    }
    ui.add_space(12.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_TOOLS: &[ToolBinding<'static>] = &[
        ("Q", "Select"),
        ("B", "Sculpt"),
        ("S", "Start positions"),
        ("G", "Procgen"),
    ];

    #[test]
    fn entry_count_matches_tool_plus_camera_bindings() {
        let entries = cheat_sheet_entries(TEST_TOOLS);
        assert_eq!(entries.len(), TEST_TOOLS.len() + CAMERA_BINDINGS.len());
        assert_eq!(entries.len(), cheat_sheet_entry_count(TEST_TOOLS.len()));
    }

    #[test]
    fn tool_section_comes_before_camera_section() {
        let entries = cheat_sheet_entries(TEST_TOOLS);
        // First TEST_TOOLS.len() entries are tools.
        for (i, t) in TEST_TOOLS.iter().enumerate() {
            assert_eq!(entries[i].keys, t.0);
            assert!(entries[i].action.contains(t.1));
        }
        // Next CAMERA_BINDINGS.len() entries are camera.
        for (i, c) in CAMERA_BINDINGS.iter().enumerate() {
            let e = &entries[TEST_TOOLS.len() + i];
            assert_eq!(e.keys, c.0);
            assert_eq!(e.action, c.1);
        }
    }

    #[test]
    fn tool_action_format_includes_label() {
        let entries = cheat_sheet_entries(&[("Q", "Select")]);
        assert_eq!(entries[0].action, "Switch to Select");
    }

    #[test]
    fn empty_tools_slice_still_yields_camera_entries() {
        let entries = cheat_sheet_entries(&[]);
        assert_eq!(entries.len(), CAMERA_BINDINGS.len());
        // First entry is the first camera binding.
        assert_eq!(entries[0].keys, CAMERA_BINDINGS[0].0);
    }

    #[test]
    fn entries_have_unique_keys_within_tool_section() {
        let entries = cheat_sheet_entries(TEST_TOOLS);
        let tool_keys: Vec<&str> = entries
            .iter()
            .take(TEST_TOOLS.len())
            .map(|e| e.keys.as_str())
            .collect();
        let mut seen = std::collections::HashSet::new();
        for k in &tool_keys {
            assert!(seen.insert(*k), "duplicate tool key {}", k);
        }
    }

    #[test]
    fn camera_bindings_all_have_non_empty_keys_and_actions() {
        for (keys, action) in CAMERA_BINDINGS {
            assert!(!keys.is_empty(), "empty key in camera bindings");
            assert!(!action.is_empty(), "empty action in camera bindings");
        }
    }

    #[test]
    fn camera_bindings_include_help_and_undo() {
        // These two are non-negotiable — the cheat-sheet exists to
        // surface them. A future trim would have to update this test
        // explicitly.
        let keys: Vec<&str> = CAMERA_BINDINGS.iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&"?"));
        assert!(keys.contains(&"Ctrl+Z"));
    }
}
