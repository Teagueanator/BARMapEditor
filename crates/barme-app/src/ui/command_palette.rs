//! Sprint 22 / U2 — Ctrl+K command palette.
//!
//! Fuzzy-match every menu item, tool, preset, and keyboard
//! shortcut by name. Selection + Enter executes; arrow keys
//! navigate; Esc closes. Per critical pitfall #3: **must NOT
//! execute on type** — the Enter gesture is the action gesture.
//!
//! ## Population
//!
//! The command list is built once at startup
//! ([`CommandPalette::new`]). State changes that add commands
//! (e.g. a freshly-imported recent project) push them in via
//! [`CommandPalette::extend`]. The list never shrinks below the
//! baseline registrations (critical pitfall #7: build at startup,
//! not per-frame).
//!
//! ## Filtering
//!
//! Substring match on the lowered (label + category + shortcut)
//! haystack against each token in the query. Multi-token queries
//! (e.g. `"build install"`) require every token to appear somewhere
//! in the haystack — order doesn't matter.

use eframe::egui;
use tracing::{info, trace};

use crate::ui::help_center::HelpArticleId;
use crate::ui::theme::Tokens;

/// Coarse-grained grouping for visual section headers in the
/// palette. Order in [`CommandCategory::ALL`] matches the display
/// order when no query is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandCategory {
    File,
    Edit,
    View,
    Build,
    Tools,
    Presets,
    Help,
}

impl CommandCategory {
    #[allow(dead_code)] // used by future ordered grouping in the palette body
    pub const ALL: [CommandCategory; 7] = [
        CommandCategory::File,
        CommandCategory::Edit,
        CommandCategory::View,
        CommandCategory::Build,
        CommandCategory::Tools,
        CommandCategory::Presets,
        CommandCategory::Help,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CommandCategory::File => "File",
            CommandCategory::Edit => "Edit",
            CommandCategory::View => "View",
            CommandCategory::Build => "Build",
            CommandCategory::Tools => "Tools",
            CommandCategory::Presets => "Presets",
            CommandCategory::Help => "Help",
        }
    }
}

/// Side-effect-free intent the App applies after the palette
/// returns. The palette doesn't import `App`; the App holds the
/// authority to mutate.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandAction {
    // ── File ────────────────────────────────────────────────────
    OpenWizard,
    OpenProject,
    SaveProject,
    SaveAs,

    // ── Edit ────────────────────────────────────────────────────
    Undo,
    Redo,

    // ── Tools (carries the accelerator: "Q"/"B"/…) ───────────────
    SwitchTool(&'static str),

    // ── View / toggles ──────────────────────────────────────────
    ToggleWireframe,
    ToggleLighting,
    ToggleGrid,
    ToggleBuildableOverlay,
    ToggleWhatsThisMode,
    Recenter,

    // ── Build ───────────────────────────────────────────────────
    BuildAndInstall,
    OpenLintPanel,
    OpenBuildLog,
    OpenMapinfoForm,

    // ── Presets ─────────────────────────────────────────────────
    ApplyWaterPreset(&'static str),
    ApplyProcgenPreset(&'static str),

    // ── Help ────────────────────────────────────────────────────
    OpenHelpCenter,
    OpenHelpArticle(HelpArticleId),
    OpenCheatSheet,
    StartTour,
    ResetToolIntros,
}

/// One palette row.
#[derive(Debug, Clone)]
pub struct Command {
    pub label: &'static str,
    pub shortcut: Option<&'static str>,
    pub category: CommandCategory,
    pub action: CommandAction,
}

impl Command {
    /// Lowered (label + category + shortcut) string used by
    /// [`fuzzy_match`].
    fn haystack(&self) -> String {
        let mut s = self.label.to_ascii_lowercase();
        s.push(' ');
        s.push_str(&self.category.label().to_ascii_lowercase());
        if let Some(sc) = self.shortcut {
            s.push(' ');
            s.push_str(&sc.to_ascii_lowercase());
        }
        s
    }
}

/// Mutable runtime state owned by `App`.
#[derive(Debug, Clone)]
pub struct CommandPalette {
    pub open: bool,
    pub query: String,
    pub commands: Vec<Command>,
    pub selected_index: usize,
}

impl Default for CommandPalette {
    fn default() -> Self {
        CommandPalette::new()
    }
}

impl CommandPalette {
    /// Build the baseline catalogue. ≥40 commands per Sprint 22
    /// exit criterion (pinned by test).
    pub fn new() -> Self {
        let mut commands = Vec::with_capacity(64);

        // ── File ─────────────────────────────────────────────────
        commands.extend([
            Command {
                label: "New project (wizard)",
                shortcut: None,
                category: CommandCategory::File,
                action: CommandAction::OpenWizard,
            },
            Command {
                label: "Open project…",
                shortcut: None,
                category: CommandCategory::File,
                action: CommandAction::OpenProject,
            },
            Command {
                label: "Save",
                shortcut: Some("Ctrl+S"),
                category: CommandCategory::File,
                action: CommandAction::SaveProject,
            },
            Command {
                label: "Save as…",
                shortcut: Some("Ctrl+Shift+S"),
                category: CommandCategory::File,
                action: CommandAction::SaveAs,
            },
        ]);

        // ── Edit ─────────────────────────────────────────────────
        commands.extend([
            Command {
                label: "Undo",
                shortcut: Some("Ctrl+Z"),
                category: CommandCategory::Edit,
                action: CommandAction::Undo,
            },
            Command {
                label: "Redo",
                shortcut: Some("Ctrl+Shift+Z"),
                category: CommandCategory::Edit,
                action: CommandAction::Redo,
            },
        ]);

        // ── Tools (one Switch entry per Tool::ALL accel) ─────────
        commands.extend([
            Command {
                label: "Switch to Select",
                shortcut: Some("Q"),
                category: CommandCategory::Tools,
                action: CommandAction::SwitchTool("Q"),
            },
            Command {
                label: "Switch to Sculpt",
                shortcut: Some("B"),
                category: CommandCategory::Tools,
                action: CommandAction::SwitchTool("B"),
            },
            Command {
                label: "Switch to Start positions",
                shortcut: Some("S"),
                category: CommandCategory::Tools,
                action: CommandAction::SwitchTool("S"),
            },
            Command {
                label: "Switch to Metal spots",
                shortcut: Some("M"),
                category: CommandCategory::Tools,
                action: CommandAction::SwitchTool("M"),
            },
            Command {
                label: "Switch to Geo vents",
                shortcut: Some("V"),
                category: CommandCategory::Tools,
                action: CommandAction::SwitchTool("V"),
            },
            Command {
                label: "Switch to Features",
                shortcut: Some("F"),
                category: CommandCategory::Tools,
                action: CommandAction::SwitchTool("F"),
            },
            Command {
                label: "Switch to Water / Lava",
                shortcut: Some("W"),
                category: CommandCategory::Tools,
                action: CommandAction::SwitchTool("W"),
            },
            Command {
                label: "Switch to Paint layer",
                shortcut: Some("L"),
                category: CommandCategory::Tools,
                action: CommandAction::SwitchTool("L"),
            },
            Command {
                label: "Switch to Procgen",
                shortcut: Some("G"),
                category: CommandCategory::Tools,
                action: CommandAction::SwitchTool("G"),
            },
        ]);

        // ── View / toggles ───────────────────────────────────────
        commands.extend([
            Command {
                label: "Toggle wireframe",
                shortcut: None,
                category: CommandCategory::View,
                action: CommandAction::ToggleWireframe,
            },
            Command {
                label: "Toggle lighting",
                shortcut: None,
                category: CommandCategory::View,
                action: CommandAction::ToggleLighting,
            },
            Command {
                label: "Toggle grid overlay",
                shortcut: None,
                category: CommandCategory::View,
                action: CommandAction::ToggleGrid,
            },
            Command {
                label: "Toggle buildable overlay",
                shortcut: None,
                category: CommandCategory::View,
                action: CommandAction::ToggleBuildableOverlay,
            },
            Command {
                label: "Toggle what's-this mode",
                shortcut: Some("Ctrl+Shift+H"),
                category: CommandCategory::View,
                action: CommandAction::ToggleWhatsThisMode,
            },
            Command {
                label: "Recenter camera",
                shortcut: None,
                category: CommandCategory::View,
                action: CommandAction::Recenter,
            },
        ]);

        // ── Build ────────────────────────────────────────────────
        commands.extend([
            Command {
                label: "Build and install",
                shortcut: Some("Ctrl+B"),
                category: CommandCategory::Build,
                action: CommandAction::BuildAndInstall,
            },
            Command {
                label: "Open lint panel",
                shortcut: None,
                category: CommandCategory::Build,
                action: CommandAction::OpenLintPanel,
            },
            Command {
                label: "Open build log",
                shortcut: None,
                category: CommandCategory::Build,
                action: CommandAction::OpenBuildLog,
            },
            Command {
                label: "Open mapinfo form",
                shortcut: Some("F9"),
                category: CommandCategory::Build,
                action: CommandAction::OpenMapinfoForm,
            },
        ]);

        // ── Presets ──────────────────────────────────────────────
        commands.extend([
            Command {
                label: "Apply water preset: Ocean",
                shortcut: None,
                category: CommandCategory::Presets,
                action: CommandAction::ApplyWaterPreset("Ocean"),
            },
            Command {
                label: "Apply water preset: Tropical",
                shortcut: None,
                category: CommandCategory::Presets,
                action: CommandAction::ApplyWaterPreset("Tropical"),
            },
            Command {
                label: "Apply water preset: Acid",
                shortcut: None,
                category: CommandCategory::Presets,
                action: CommandAction::ApplyWaterPreset("Acid"),
            },
            Command {
                label: "Apply water preset: Lava",
                shortcut: None,
                category: CommandCategory::Presets,
                action: CommandAction::ApplyWaterPreset("Lava"),
            },
            Command {
                label: "Apply water preset: Magma",
                shortcut: None,
                category: CommandCategory::Presets,
                action: CommandAction::ApplyWaterPreset("Magma"),
            },
            Command {
                label: "Apply procgen preset: Parabolic bowl",
                shortcut: None,
                category: CommandCategory::Presets,
                action: CommandAction::ApplyProcgenPreset("Parabolic bowl"),
            },
            Command {
                label: "Apply procgen preset: Saddle",
                shortcut: None,
                category: CommandCategory::Presets,
                action: CommandAction::ApplyProcgenPreset("Saddle"),
            },
            Command {
                label: "Apply procgen preset: Diagonal ramp",
                shortcut: None,
                category: CommandCategory::Presets,
                action: CommandAction::ApplyProcgenPreset("Diagonal ramp"),
            },
            Command {
                label: "Apply procgen preset: Plateau",
                shortcut: None,
                category: CommandCategory::Presets,
                action: CommandAction::ApplyProcgenPreset("Plateau"),
            },
        ]);

        // ── Help ─────────────────────────────────────────────────
        commands.extend([
            Command {
                label: "Open help center",
                shortcut: None,
                category: CommandCategory::Help,
                action: CommandAction::OpenHelpCenter,
            },
            Command {
                label: "Open cheat sheet",
                shortcut: Some("?"),
                category: CommandCategory::Help,
                action: CommandAction::OpenCheatSheet,
            },
            Command {
                label: "Start guided tour",
                shortcut: None,
                category: CommandCategory::Help,
                action: CommandAction::StartTour,
            },
            Command {
                label: "Reset tool intros",
                shortcut: None,
                category: CommandCategory::Help,
                action: CommandAction::ResetToolIntros,
            },
            Command {
                label: "Open help: Getting started",
                shortcut: None,
                category: CommandCategory::Help,
                action: CommandAction::OpenHelpArticle(HelpArticleId::GettingStarted),
            },
            Command {
                label: "Open help: What's new",
                shortcut: None,
                category: CommandCategory::Help,
                action: CommandAction::OpenHelpArticle(HelpArticleId::WhatsNew),
            },
            Command {
                label: "Open help: Shortcuts",
                shortcut: None,
                category: CommandCategory::Help,
                action: CommandAction::OpenHelpArticle(HelpArticleId::Shortcuts),
            },
            Command {
                label: "Open help: Build pipeline",
                shortcut: None,
                category: CommandCategory::Help,
                action: CommandAction::OpenHelpArticle(HelpArticleId::BuildPipeline),
            },
            Command {
                label: "Open help: Layered painter",
                shortcut: None,
                category: CommandCategory::Help,
                action: CommandAction::OpenHelpArticle(HelpArticleId::LayeredPainter),
            },
            Command {
                label: "Open help: §4 Heightmap dims",
                shortcut: None,
                category: CommandCategory::Help,
                action: CommandAction::OpenHelpArticle(HelpArticleId::Pitfall04HeightmapDims),
            },
            Command {
                label: "Open help: §6 Mapinfo silent deps",
                shortcut: None,
                category: CommandCategory::Help,
                action: CommandAction::OpenHelpArticle(HelpArticleId::Pitfall06MapInfoSilentDeps),
            },
            Command {
                label: "Open help: §13 Metalmap zero",
                shortcut: None,
                category: CommandCategory::Help,
                action: CommandAction::OpenHelpArticle(HelpArticleId::Pitfall13MetalmapZero),
            },
            Command {
                label: "Open help: §15 Splat subtable form",
                shortcut: None,
                category: CommandCategory::Help,
                action: CommandAction::OpenHelpArticle(HelpArticleId::Pitfall15SplatSubtableForm),
            },
        ]);

        CommandPalette {
            open: false,
            query: String::new(),
            commands,
            selected_index: 0,
        }
    }

    /// Open the palette + reset the query and selection.
    pub fn show(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected_index = 0;
        info!(target: "barme::command_palette", "command palette opened");
    }

    /// Close the palette without executing.
    pub fn hide(&mut self) {
        self.open = false;
        self.query.clear();
        self.selected_index = 0;
    }

    /// Filter the catalogue against the current query. Returns
    /// indices into `self.commands`.
    pub fn filtered_indices(&self) -> Vec<usize> {
        let q = self.query.to_ascii_lowercase();
        let tokens: Vec<&str> = q.split_whitespace().collect();
        let mut out = Vec::with_capacity(self.commands.len());
        for (i, cmd) in self.commands.iter().enumerate() {
            let haystack = cmd.haystack();
            if fuzzy_match(&haystack, &tokens) {
                out.push(i);
            }
        }
        out
    }
}

/// Multi-token substring match: every token in `tokens` must appear
/// somewhere in `haystack` (case-insensitive — caller passes both
/// lowercased). An empty `tokens` slice matches everything.
fn fuzzy_match(haystack: &str, tokens: &[&str]) -> bool {
    tokens.iter().all(|t| haystack.contains(t))
}

/// Per-frame outcome.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandPaletteAction {
    None,
    Execute(CommandAction),
    Close,
}

/// Render the palette window if `state.open == true`. Returns the
/// user's intent this frame.
///
/// Keyboard handling:
/// - ArrowDown / ArrowUp: navigate the filtered list.
/// - Enter: execute the highlighted row.
/// - Esc: close.
pub fn render(ctx: &egui::Context, state: &mut CommandPalette) -> CommandPaletteAction {
    if !state.open {
        return CommandPaletteAction::None;
    }
    let t = Tokens::DARK;

    let filtered = state.filtered_indices();
    if state.selected_index >= filtered.len().max(1) {
        state.selected_index = filtered.len().saturating_sub(1);
    }

    // Snapshot keyboard intent BEFORE rendering so the text input
    // doesn't eat the navigation keys.
    let (arrow_down, arrow_up, enter, esc) = ctx.input(|i| {
        (
            i.key_pressed(egui::Key::ArrowDown),
            i.key_pressed(egui::Key::ArrowUp),
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
        )
    });
    if !filtered.is_empty() {
        if arrow_down {
            state.selected_index = (state.selected_index + 1).min(filtered.len() - 1);
        }
        if arrow_up {
            state.selected_index = state.selected_index.saturating_sub(1);
        }
    }

    let mut action = CommandPaletteAction::None;

    if esc {
        state.hide();
        return CommandPaletteAction::Close;
    }

    let mut local_open = true;
    egui::Window::new("Command palette")
        .open(&mut local_open)
        .collapsible(false)
        .resizable(true)
        .default_width(540.0)
        .default_height(420.0)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 80.0))
        .show(ctx, |ui| {
            // Text input
            let edit = egui::TextEdit::singleline(&mut state.query)
                .desired_width(f32::INFINITY)
                .hint_text("Type to search… (Enter executes, ↑↓ navigate, Esc closes)")
                .lock_focus(true);
            let response = ui.add(edit);
            response.request_focus();

            ui.separator();
            ui.label(
                egui::RichText::new(format!(
                    "{} of {} commands",
                    filtered.len(),
                    state.commands.len()
                ))
                .color(t.muted)
                .size(10.0),
            );
            ui.add_space(4.0);

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .id_salt("command_palette_results")
                .show(ui, |ui| {
                    if filtered.is_empty() {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("No matches.")
                                .color(t.muted)
                                .size(11.0),
                        );
                        return;
                    }
                    for (row_idx, cmd_idx) in filtered.iter().copied().enumerate() {
                        let cmd = &state.commands[cmd_idx];
                        let is_selected = row_idx == state.selected_index;
                        let row = render_row(ui, cmd, is_selected, t);
                        if row.clicked() {
                            state.selected_index = row_idx;
                            trace!(target: "barme::command_palette", label = cmd.label, "command executed via click");
                            action =
                                CommandPaletteAction::Execute(cmd.action.clone());
                        }
                    }
                });
        });
    if !local_open {
        state.hide();
        return CommandPaletteAction::Close;
    }

    if enter && !filtered.is_empty() {
        let cmd_idx = filtered[state.selected_index.min(filtered.len() - 1)];
        let cmd = &state.commands[cmd_idx];
        trace!(target: "barme::command_palette", label = cmd.label, "command executed via Enter");
        action = CommandPaletteAction::Execute(cmd.action.clone());
    }

    if matches!(action, CommandPaletteAction::Execute(_)) {
        state.hide();
    }
    action
}

fn render_row(ui: &mut egui::Ui, cmd: &Command, is_selected: bool, t: Tokens) -> egui::Response {
    let bg = if is_selected {
        Some(t.accent_alpha(0x40))
    } else {
        None
    };
    let frame_resp = egui::Frame::new()
        .fill(bg.unwrap_or(egui::Color32::TRANSPARENT))
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("[{}]", cmd.category.label()))
                        .monospace()
                        .color(t.muted)
                        .size(10.0),
                );
                ui.label(
                    egui::RichText::new(cmd.label)
                        .color(t.text)
                        .size(12.0)
                        .strong(),
                );
                if let Some(sc) = cmd.shortcut {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(sc)
                                .monospace()
                                .color(t.muted)
                                .size(10.0),
                        );
                    });
                }
            });
        });
    frame_resp.response.interact(egui::Sense::click())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sprint 22 exit criterion: ≥40 commands registered.
    #[test]
    fn baseline_catalogue_has_at_least_forty_commands() {
        let p = CommandPalette::new();
        assert!(
            p.commands.len() >= 40,
            "expected ≥40 commands, got {}",
            p.commands.len(),
        );
    }

    /// Every Tool::ALL accel has a SwitchTool command (pinned to
    /// the help_center accel set).
    #[test]
    fn every_tool_accel_has_switch_command() {
        let p = CommandPalette::new();
        for accel in &["Q", "B", "S", "M", "V", "F", "W", "L", "G"] {
            assert!(
                p.commands.iter().any(|c| matches!(
                    c.action,
                    CommandAction::SwitchTool(a) if a == *accel,
                )),
                "no SwitchTool command for accel `{accel}`",
            );
        }
    }

    /// Default state is closed with empty query.
    #[test]
    fn default_state_is_closed_with_empty_query() {
        let p = CommandPalette::default();
        assert!(!p.open);
        assert!(p.query.is_empty());
        assert_eq!(p.selected_index, 0);
    }

    /// `show` opens + resets state.
    #[test]
    fn show_opens_and_resets() {
        let mut p = CommandPalette::new();
        p.query = "stale".into();
        p.selected_index = 5;
        p.show();
        assert!(p.open);
        assert!(p.query.is_empty());
        assert_eq!(p.selected_index, 0);
    }

    /// `hide` closes + resets state.
    #[test]
    fn hide_closes_and_resets() {
        let mut p = CommandPalette::new();
        p.open = true;
        p.query = "abc".into();
        p.selected_index = 3;
        p.hide();
        assert!(!p.open);
        assert!(p.query.is_empty());
        assert_eq!(p.selected_index, 0);
    }

    /// Empty query matches everything.
    #[test]
    fn empty_query_matches_every_command() {
        let p = CommandPalette::new();
        let filtered = p.filtered_indices();
        assert_eq!(filtered.len(), p.commands.len());
    }

    /// Single-token substring filter narrows results.
    #[test]
    fn build_query_narrows_to_build_category() {
        let mut p = CommandPalette::new();
        p.query = "build".into();
        let filtered = p.filtered_indices();
        assert!(!filtered.is_empty());
        // Every matched command must contain "build" in label OR category.
        for &i in &filtered {
            let cmd = &p.commands[i];
            let haystack = cmd.haystack();
            assert!(
                haystack.contains("build"),
                "matched `{}` shouldn't match `build`",
                cmd.label
            );
        }
    }

    /// Multi-token search: every token must appear somewhere.
    #[test]
    fn multi_token_query_requires_all_tokens() {
        let mut p = CommandPalette::new();
        p.query = "open help".into();
        let filtered = p.filtered_indices();
        for &i in &filtered {
            let haystack = p.commands[i].haystack();
            assert!(
                haystack.contains("open") && haystack.contains("help"),
                "matched `{}` is missing one of the query tokens",
                p.commands[i].label,
            );
        }
        // Spot-check: at minimum, "Open help center" survives.
        assert!(
            p.commands
                .iter()
                .enumerate()
                .filter(|(idx, _)| filtered.contains(idx))
                .any(|(_, c)| c.label == "Open help center"),
            "expected `Open help center` to survive `open help` query"
        );
    }

    /// Filter results are stable in insertion order (no
    /// out-of-order shuffling without a relevance score).
    #[test]
    fn filtered_indices_are_in_registration_order() {
        let p = CommandPalette::new();
        let filtered = p.filtered_indices();
        let mut prev: i64 = -1;
        for &i in &filtered {
            assert!(
                (i as i64) > prev,
                "filtered indices not in registration order"
            );
            prev = i as i64;
        }
    }

    /// No two commands share an identical label (regression: would
    /// confuse the user).
    #[test]
    fn command_labels_are_unique() {
        let p = CommandPalette::new();
        let mut seen = std::collections::HashSet::new();
        for cmd in &p.commands {
            assert!(seen.insert(cmd.label), "duplicate label: {}", cmd.label);
        }
    }

    /// Every Help-category command resolves to either OpenHelp,
    /// OpenHelpArticle, OpenCheatSheet, StartTour, or
    /// ResetToolIntros — i.e. category and action stay in sync.
    #[test]
    fn help_category_actions_are_help_shaped() {
        let p = CommandPalette::new();
        for cmd in p
            .commands
            .iter()
            .filter(|c| c.category == CommandCategory::Help)
        {
            assert!(
                matches!(
                    cmd.action,
                    CommandAction::OpenHelpCenter
                        | CommandAction::OpenHelpArticle(_)
                        | CommandAction::OpenCheatSheet
                        | CommandAction::StartTour
                        | CommandAction::ResetToolIntros
                ),
                "Help-category command `{}` has non-help action",
                cmd.label,
            );
        }
    }

    /// Tools-category commands all carry SwitchTool actions.
    #[test]
    fn tools_category_actions_are_switch_tool() {
        let p = CommandPalette::new();
        for cmd in p
            .commands
            .iter()
            .filter(|c| c.category == CommandCategory::Tools)
        {
            assert!(
                matches!(cmd.action, CommandAction::SwitchTool(_)),
                "Tools-category command `{}` is not SwitchTool",
                cmd.label,
            );
        }
    }
}
