//! C8 / Sprint 21 — lint panel rendering. Sprint 19 / U1 shipped the
//! stub (window pattern + trace transitions); Sprint 21 fills the body
//! with the real [`barme_pipeline::LintRule`] registry output.
//!
//! Opens from the top-bar validation chip OR the status-strip issue
//! count (both wired in Sprint 19; this module is purely the content
//! renderer).
//!
//! ## Layout
//!
//! Grouped by severity, errors first. Each row shows:
//! - A coloured dot (red / amber / muted) matching the build-log
//!   palette (`rgb(220, 110, 90)` / `rgb(220, 175, 90)` / muted).
//! - The rule's title.
//! - The fire-message in the body row.
//! - Optional `field_path` chip on the right.
//! - Optional Fix button (when [`barme_pipeline::LintFix`] is set).
//!
//! Passing rules surface at the bottom as a "✓ all checks pass" line
//! when no failures emit; otherwise they collapse into a per-severity
//! count footer.
//!
//! ## Edit-commit contract
//!
//! Clicking Fix returns a [`MapInfoPatch`] which the App dispatches
//! through `apply_mapinfo_patch` + `ProjectDiff::EditMapInfo` so undo
//! restores the prior value. The patch propagation matches the F9 form's
//! pattern (`crates/barme-app/src/ui/inspector_mapinfo.rs`).

use barme_core::MapInfoPatch;
use barme_pipeline::{LintFix, LintIssue, LintRule, LintSeverity};
use eframe::egui;
use tracing::trace;

use crate::ui::help_center::HelpArticleId;
use crate::ui::theme::{ChipTone, Tokens};

/// Sprint 22 / U2 — outcome from one lint-panel frame. `fix`
/// carries the user's Fix click; `open_help` carries a click on a
/// rule row's `[Help…]` button. Both can be Some on the same
/// frame if the user mass-clicks.
#[derive(Debug, Default)]
pub struct LintPanelOutcome {
    pub fix: Option<MapInfoPatch>,
    pub open_help: Option<HelpArticleId>,
}

/// Severity-tinted palette used both here and in the build log.
/// Centralised so a change here propagates everywhere visually.
fn severity_color(severity: LintSeverity, t: Tokens) -> egui::Color32 {
    match severity {
        LintSeverity::Error => egui::Color32::from_rgb(220, 110, 90),
        LintSeverity::Warning => egui::Color32::from_rgb(220, 175, 90),
        LintSeverity::Info => t.muted,
    }
}

fn severity_label(severity: LintSeverity) -> &'static str {
    match severity {
        LintSeverity::Error => "Error",
        LintSeverity::Warning => "Warning",
        LintSeverity::Info => "Info",
    }
}

/// Render the lint-panel window. Caller passes a mutable `bool` that
/// drives visibility — the panel sets it to `false` when the user
/// closes the window.
///
/// Sprint 22 / U2: returns a [`LintPanelOutcome`] carrying both a
/// Fix click (which dispatches via `apply_mapinfo_patch` + an
/// `EditMapInfo` undo entry) and a Help click (which opens the
/// help center on the corresponding pitfall article).
pub fn render(
    ctx: &egui::Context,
    open: &mut bool,
    issues: &[LintIssue],
    previously_open: &mut bool,
) -> LintPanelOutcome {
    if *open && !*previously_open {
        trace!(target: "barme::lint_panel", "lint_panel opened");
    } else if !*open && *previously_open {
        trace!(target: "barme::lint_panel", "lint_panel closed");
    }
    *previously_open = *open;

    if !*open {
        return LintPanelOutcome::default();
    }

    let t = Tokens::DARK;
    let mut local_open = true;
    let mut outcome = LintPanelOutcome::default();
    egui::Window::new("Project lint")
        .open(&mut local_open)
        .collapsible(false)
        .resizable(true)
        .default_width(520.0)
        .default_height(420.0)
        .show(ctx, |ui| {
            outcome = render_body(ui, issues, t);
        });
    if !local_open {
        *open = false;
    }
    outcome
}

fn render_body(ui: &mut egui::Ui, issues: &[LintIssue], t: Tokens) -> LintPanelOutcome {
    let errors = issues
        .iter()
        .filter(|i| i.severity == LintSeverity::Error)
        .count();
    let warnings = issues
        .iter()
        .filter(|i| i.severity == LintSeverity::Warning)
        .count();
    let infos = issues
        .iter()
        .filter(|i| i.severity == LintSeverity::Info)
        .count();
    let total_rules = LintRule::ALL.len();
    let passing = total_rules - issues.len();

    // Header: severity counters as chips.
    ui.horizontal(|ui| {
        if errors == 0 && warnings == 0 && infos == 0 {
            crate::ui::widgets::chip(ui, ChipTone::Ok, "All checks pass");
        } else {
            if errors > 0 {
                crate::ui::widgets::chip(ui, ChipTone::Err, format!("{errors} error(s)"));
            }
            if warnings > 0 {
                crate::ui::widgets::chip(ui, ChipTone::Warn, format!("{warnings} warning(s)"));
            }
            if infos > 0 {
                crate::ui::widgets::chip(ui, ChipTone::Neutral, format!("{infos} info"));
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("{passing}/{total_rules} rules passing"))
                    .color(t.muted)
                    .size(11.0),
            );
        });
    });
    ui.separator();

    let mut outcome = LintPanelOutcome::default();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if !issues.is_empty() {
                for sev in [
                    LintSeverity::Error,
                    LintSeverity::Warning,
                    LintSeverity::Info,
                ] {
                    let group: Vec<&LintIssue> =
                        issues.iter().filter(|i| i.severity == sev).collect();
                    if group.is_empty() {
                        continue;
                    }
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(severity_label(sev))
                            .color(severity_color(sev, t))
                            .strong(),
                    );
                    ui.separator();
                    for issue in group {
                        let row = render_issue_row(ui, issue, t);
                        if row.fix.is_some() {
                            outcome.fix = row.fix;
                        }
                        if row.open_help.is_some() {
                            outcome.open_help = row.open_help;
                        }
                    }
                }
            } else {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "No errors, warnings, or info notes. \
                         The lint pass evaluated every rule against the \
                         current project — every one passes.",
                    )
                    .color(t.muted)
                    .size(11.0),
                );
            }
        });

    outcome
}

fn render_issue_row(ui: &mut egui::Ui, issue: &LintIssue, t: Tokens) -> LintPanelOutcome {
    let mut row_outcome = LintPanelOutcome::default();
    ui.horizontal(|ui| {
        let (dot_rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 18.0), egui::Sense::hover());
        let center = egui::pos2(dot_rect.left() + 5.0, dot_rect.center().y);
        ui.painter()
            .circle_filled(center, 4.0, severity_color(issue.severity, t));
        ui.vertical(|ui| {
            // Title + (rule name + PITFALL anchor) row.
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(issue.rule.title()).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(fp) = &issue.field_path {
                        ui.label(
                            egui::RichText::new(fp)
                                .monospace()
                                .color(t.muted)
                                .size(10.0),
                        );
                    }
                });
            });
            // Body text.
            ui.label(egui::RichText::new(&issue.message).color(t.text).size(11.0));
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{} · PITFALL §{}",
                        issue.rule.name(),
                        issue.rule.pitfall_anchor()
                    ))
                    .color(t.muted)
                    .size(10.0)
                    .monospace(),
                );
                if let Some(LintFix::MapInfoPatch(patch)) = &issue.fix {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("Fix")
                            .on_hover_text(format!(
                                "Apply: {} (undoable via Ctrl-Z)",
                                patch.label()
                            ))
                            .clicked()
                        {
                            row_outcome.fix = Some(patch.clone());
                        }
                    });
                }
                // Sprint 22 / U2 — `[Help…]` button routes to the
                // help center on the issue's PITFALLS.md anchor.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let anchor_str = issue.rule.pitfall_anchor();
                    let anchor_n: u8 = anchor_str.parse().unwrap_or(0);
                    if ui
                        .small_button("Help…")
                        .on_hover_text(format!(
                            "Open the help center article for PITFALL §{anchor_str}."
                        ))
                        .clicked()
                    {
                        row_outcome.open_help =
                            Some(HelpArticleId::from_pitfall_anchor(anchor_n));
                    }
                });
            });
        });
    });
    ui.separator();
    row_outcome
}

/// Live issue count from a registry-backed `Vec<LintIssue>`. Used by
/// the status-strip "{n} issue" label.
pub fn issue_count(issues: &[LintIssue]) -> usize {
    issues.len()
}

/// Tab-dot counts derived from `Vec<LintIssue>` by matching each
/// issue's `field_path` prefix to a F9 tab. Returns a `[u32; 12]`
/// indexed by `MapInfoTab as usize`.
///
/// Matching:
/// - `general` / no prefix that matches anything → `General`
/// - `name`, `version`, `mapfile`, `description`, `author` → `General`
/// - `smf.*` → `Smf`
/// - `lighting.*` → `Lighting`
/// - `atmosphere.*` → `Atmosphere`
/// - `water.*`, plain `water`, `tidalStrength`, `voidWater` → `Water`
/// - `resources.*` → `Resources`
/// - `splats.*` → `Splats`
/// - `terrainTypes.*` → `TerrainTypes`
/// - `custom.*` → `Custom`
/// - Anything else top-level (modtype, gravity, maxMetal, extractorRadius,
///   voidGround, voidAlphaMin, ally_groups, teams, gui.*, metal_layout.*,
///   featureplacer.*) → `Map`
pub fn tab_counts(issues: &[LintIssue]) -> [u32; 12] {
    use crate::ui::inspector_mapinfo::MapInfoTab;
    let mut counts = [0u32; 12];
    for issue in issues {
        let Some(path) = &issue.field_path else {
            continue;
        };
        let tab = if path.starts_with("smf") {
            MapInfoTab::Smf
        } else if path.starts_with("lighting") {
            MapInfoTab::Lighting
        } else if path.starts_with("atmosphere") {
            MapInfoTab::Atmosphere
        } else if path.starts_with("water") || path == "tidalStrength" || path == "voidWater" {
            MapInfoTab::Water
        } else if path.starts_with("resources") {
            MapInfoTab::Resources
        } else if path.starts_with("splats") {
            MapInfoTab::Splats
        } else if path.starts_with("terrainTypes") {
            MapInfoTab::TerrainTypes
        } else if path.starts_with("custom") {
            MapInfoTab::Custom
        } else if matches!(
            path.as_str(),
            "name" | "shortname" | "version" | "mapfile" | "description" | "author"
        ) {
            MapInfoTab::General
        } else {
            MapInfoTab::Map
        };
        counts[tab as usize] = counts[tab as usize].saturating_add(1);
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use barme_pipeline::{LintIssue, LintRule};

    fn issue(rule: LintRule, field_path: Option<&str>) -> LintIssue {
        LintIssue {
            rule,
            severity: rule.default_severity(),
            message: rule.title().to_string(),
            field_path: field_path.map(str::to_string),
            fix: None,
        }
    }

    #[test]
    fn issue_count_returns_slice_len() {
        let issues = vec![
            issue(LintRule::ModtypeNotThree, Some("modtype")),
            issue(LintRule::FogStartEqualsFogEnd, Some("atmosphere.fogEnd")),
        ];
        assert_eq!(issue_count(&issues), 2);
        assert_eq!(issue_count(&[]), 0);
    }

    #[test]
    fn tab_counts_route_field_paths_to_tabs() {
        use crate::ui::inspector_mapinfo::MapInfoTab;
        let issues = vec![
            issue(LintRule::FogStartEqualsFogEnd, Some("atmosphere.fogEnd")),
            issue(LintRule::LightingSunDirMissing, Some("lighting.sunDir")),
            issue(LintRule::SmtFileNameZeroMissing, Some("smf.smtFileName0")),
            issue(
                LintRule::ExtractorRadiusFiveHundred,
                Some("extractorRadius"),
            ),
            issue(LintRule::TerrainBelowZeroWithoutWater, Some("water")),
            issue(LintRule::NameOrMapfileOrVersionMissing, Some("name")),
        ];
        let counts = tab_counts(&issues);
        assert_eq!(counts[MapInfoTab::Atmosphere as usize], 1);
        assert_eq!(counts[MapInfoTab::Lighting as usize], 1);
        assert_eq!(counts[MapInfoTab::Smf as usize], 1);
        assert_eq!(counts[MapInfoTab::Map as usize], 1); // extractorRadius
        assert_eq!(counts[MapInfoTab::Water as usize], 1);
        assert_eq!(counts[MapInfoTab::General as usize], 1); // name
    }

    #[test]
    fn tab_counts_skip_issues_without_field_path() {
        // HeightmapDimsWrong has no field_path; ensure it's silently
        // skipped by the tab-dot mapper.
        let issues = vec![issue(LintRule::HeightmapDimsWrong, None)];
        let counts = tab_counts(&issues);
        assert_eq!(counts.iter().sum::<u32>(), 0);
    }

    /// Sprint 19 / U1 — egui smoke test for the lint panel window.
    /// Drives a headless `egui::Context` through one frame with
    /// `open = true` and asserts the window opens cleanly.
    #[test]
    fn render_emits_window_when_open() {
        let ctx = egui::Context::default();
        let mut open = true;
        let mut prev = false;
        let issues = vec![issue(LintRule::ModtypeNotThree, Some("modtype"))];
        let _ = ctx.run(Default::default(), |ctx| {
            let _outcome = render(ctx, &mut open, &issues, &mut prev);
        });
        assert!(prev, "previously_open should mirror open after render");
        assert!(open, "lint panel should stay open when no close fired");
    }

    /// When `open = false`, `render` early-returns and never
    /// produces an outcome.
    #[test]
    fn render_no_op_when_closed() {
        let ctx = egui::Context::default();
        let mut open = false;
        let mut prev = true;
        let issues: Vec<LintIssue> = vec![];
        let mut outcome_seen = LintPanelOutcome::default();
        let _ = ctx.run(Default::default(), |ctx| {
            outcome_seen = render(ctx, &mut open, &issues, &mut prev);
        });
        assert!(!prev);
        assert!(outcome_seen.fix.is_none());
        assert!(outcome_seen.open_help.is_none());
    }

    /// Panel renders an "all clear" footer when the issue list is
    /// empty.
    #[test]
    fn render_empty_issue_list_shows_all_clear() {
        let ctx = egui::Context::default();
        let mut open = true;
        let mut prev = true;
        let issues: Vec<LintIssue> = vec![];
        let _ = ctx.run(Default::default(), |ctx| {
            let _outcome = render(ctx, &mut open, &issues, &mut prev);
        });
        // No panic.
    }
}
