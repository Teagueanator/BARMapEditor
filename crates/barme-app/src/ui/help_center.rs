//! Sprint 22 / U2 — re-openable Help Center Window.
//!
//! Replaces Sprint 19's `?` cheat sheet as the principal discovery
//! surface. The cheat sheet stays as a tab inside the help center
//! (and the `?` chord stays alive as the keyboard backstop — critical
//! pitfall #10).
//!
//! ## Markdown rendering
//!
//! The 2026-05-20 prompt recommended `egui_commonmark` (~50 KB
//! compiled). Sprint 22 ships a minimal in-module subset renderer
//! instead because:
//!
//! - `egui_commonmark` is not in the offline cargo cache; pulling
//!   it in now would add a workspace-dep version coupling we'd
//!   have to audit in lockstep with egui 0.33.
//! - The article catalogue uses headings (`#`, `##`, `###`),
//!   paragraphs, bullet lists, inline code spans, and fenced code
//!   blocks — a small enough subset that ~80 lines of parser is
//!   trivially correct.
//! - Dark-theme colours (heading vs body vs code) are pinned
//!   directly against [`crate::ui::theme::Tokens::DARK`] instead of
//!   wrestling with `egui_commonmark`'s defaults (critical pitfall
//!   #5).
//!
//! Future polish (Stage 2) can swap to `egui_commonmark` once the
//! dep is available; the renderer is isolated in [`render_markdown`].
//!
//! ## Articles
//!
//! Article bodies live as one `.md` file each under `help_content/`
//! and are baked into the binary via `include_str!`. Editing an
//! article requires a recompile (acceptable for Sprint 22; Stage 2
//! may move to runtime loading per critical pitfall #4).

use eframe::egui;
use tracing::trace;

use crate::ui::theme::Tokens;

// ─── Article catalogue ──────────────────────────────────────────────

/// Tab-strip category for the article tree. The strip pins Getting
/// Started + What's New + Shortcuts + Build pipeline at the top
/// (Meta category) and groups the rest by Tools / Pitfalls /
/// Reference. Within each non-meta category articles render in the
/// order they appear in [`HelpArticleId::ALL`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HelpArticleCategory {
    Meta,
    Tools,
    Pitfalls,
    Reference,
}

impl HelpArticleCategory {
    /// All categories in tab-strip display order.
    pub const ALL: [HelpArticleCategory; 4] = [
        HelpArticleCategory::Meta,
        HelpArticleCategory::Tools,
        HelpArticleCategory::Pitfalls,
        HelpArticleCategory::Reference,
    ];

    pub fn label(self) -> &'static str {
        match self {
            HelpArticleCategory::Meta => "Getting started",
            HelpArticleCategory::Tools => "Tools",
            HelpArticleCategory::Pitfalls => "Pitfalls",
            HelpArticleCategory::Reference => "Reference",
        }
    }
}

/// Stable identifier for every article. New variants must extend
/// [`HelpArticleId::ALL`] AND the [`HelpArticleId::body`] /
/// [`HelpArticleId::title`] / [`HelpArticleId::category`] match
/// arms; compile errors flag the missing edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HelpArticleId {
    // ── Meta ───────────────────────────────────────────────────────
    GettingStarted,
    WhatsNew,
    Shortcuts,
    BuildPipeline,

    // ── Tools (9 variants — one per [`crate::Tool`]) ──────────────
    ToolSelect,
    ToolSculpt,
    ToolStartPositions,
    ToolMetalSpots,
    ToolGeoFeatures,
    ToolFeature,
    ToolWater,
    ToolPaintLayer,
    ToolProcgen,

    // ── Reference ──────────────────────────────────────────────────
    LayeredPainter,
    AllyTeams,
    WaterAndLava,

    // ── Pitfalls (numbered per docs/PITFALLS.md §1..§28) ───────────
    Pitfall01TextureMemory,
    Pitfall02Dxt1,
    Pitfall03SmtDedup,
    Pitfall04HeightmapDims,
    Pitfall06MapInfoSilentDeps,
    Pitfall07PinkMap,
    Pitfall08DntsWaterLos,
    Pitfall09Sd7Solidity,
    Pitfall11SundirCase,
    Pitfall13MetalmapZero,
    Pitfall14GeoVentsSpringboard,
    Pitfall15SplatSubtableForm,
    Pitfall18SundirW,
    Pitfall22MaxMetalScale,
    Pitfall25LuaGaiaBootstrap,
    Pitfall26StartBoxes,
    Pitfall28WaterPlaneConsteval,
}

impl HelpArticleId {
    /// Every variant in tab-strip display order. The tree renderer
    /// walks this slice and groups by [`Self::category`]; tests pin
    /// completeness.
    pub const ALL: &'static [HelpArticleId] = &[
        // Meta
        HelpArticleId::GettingStarted,
        HelpArticleId::WhatsNew,
        HelpArticleId::Shortcuts,
        HelpArticleId::BuildPipeline,
        // Tools
        HelpArticleId::ToolSelect,
        HelpArticleId::ToolSculpt,
        HelpArticleId::ToolStartPositions,
        HelpArticleId::ToolMetalSpots,
        HelpArticleId::ToolGeoFeatures,
        HelpArticleId::ToolFeature,
        HelpArticleId::ToolWater,
        HelpArticleId::ToolPaintLayer,
        HelpArticleId::ToolProcgen,
        // Pitfalls
        HelpArticleId::Pitfall01TextureMemory,
        HelpArticleId::Pitfall02Dxt1,
        HelpArticleId::Pitfall03SmtDedup,
        HelpArticleId::Pitfall04HeightmapDims,
        HelpArticleId::Pitfall06MapInfoSilentDeps,
        HelpArticleId::Pitfall07PinkMap,
        HelpArticleId::Pitfall08DntsWaterLos,
        HelpArticleId::Pitfall09Sd7Solidity,
        HelpArticleId::Pitfall11SundirCase,
        HelpArticleId::Pitfall13MetalmapZero,
        HelpArticleId::Pitfall14GeoVentsSpringboard,
        HelpArticleId::Pitfall15SplatSubtableForm,
        HelpArticleId::Pitfall18SundirW,
        HelpArticleId::Pitfall22MaxMetalScale,
        HelpArticleId::Pitfall25LuaGaiaBootstrap,
        HelpArticleId::Pitfall26StartBoxes,
        HelpArticleId::Pitfall28WaterPlaneConsteval,
        // Reference
        HelpArticleId::LayeredPainter,
        HelpArticleId::AllyTeams,
        HelpArticleId::WaterAndLava,
    ];

    pub fn title(self) -> &'static str {
        match self {
            HelpArticleId::GettingStarted => "Getting started",
            HelpArticleId::WhatsNew => "What's new",
            HelpArticleId::Shortcuts => "Keyboard shortcuts",
            HelpArticleId::BuildPipeline => "Build pipeline",

            HelpArticleId::ToolSelect => "Select / orbit",
            HelpArticleId::ToolSculpt => "Sculpt",
            HelpArticleId::ToolStartPositions => "Start positions",
            HelpArticleId::ToolMetalSpots => "Metal spots",
            HelpArticleId::ToolGeoFeatures => "Geo vents",
            HelpArticleId::ToolFeature => "Features",
            HelpArticleId::ToolWater => "Water / Lava",
            HelpArticleId::ToolPaintLayer => "Paint layer",
            HelpArticleId::ToolProcgen => "Procgen",

            HelpArticleId::LayeredPainter => "Layered painter",
            HelpArticleId::AllyTeams => "Ally teams",
            HelpArticleId::WaterAndLava => "Water and Lava",

            HelpArticleId::Pitfall01TextureMemory => "§1 Texture memory",
            HelpArticleId::Pitfall02Dxt1 => "§2 DXT1 compression",
            HelpArticleId::Pitfall03SmtDedup => "§3 SMT dedup",
            HelpArticleId::Pitfall04HeightmapDims => "§4 Heightmap dims",
            HelpArticleId::Pitfall06MapInfoSilentDeps => "§6 mapinfo deps",
            HelpArticleId::Pitfall07PinkMap => "§7 Pink map on rename",
            HelpArticleId::Pitfall08DntsWaterLos => "§8 DNTS + water + LOS",
            HelpArticleId::Pitfall09Sd7Solidity => "§9 .sd7 solidity",
            HelpArticleId::Pitfall11SundirCase => "§11 sundir vs sunDir",
            HelpArticleId::Pitfall13MetalmapZero => "§13 metalmap zero",
            HelpArticleId::Pitfall14GeoVentsSpringboard => "§14 Geo vents",
            HelpArticleId::Pitfall15SplatSubtableForm => "§15 splat subtable",
            HelpArticleId::Pitfall18SundirW => "§18 sunDir.w = 1.0",
            HelpArticleId::Pitfall22MaxMetalScale => "§22 maxMetal scale",
            HelpArticleId::Pitfall25LuaGaiaBootstrap => "§25 LuaGaia bootstrap",
            HelpArticleId::Pitfall26StartBoxes => "§26 map_startboxes.lua",
            HelpArticleId::Pitfall28WaterPlaneConsteval => "§28 Water plane",
        }
    }

    pub fn category(self) -> HelpArticleCategory {
        match self {
            HelpArticleId::GettingStarted
            | HelpArticleId::WhatsNew
            | HelpArticleId::Shortcuts
            | HelpArticleId::BuildPipeline => HelpArticleCategory::Meta,

            HelpArticleId::ToolSelect
            | HelpArticleId::ToolSculpt
            | HelpArticleId::ToolStartPositions
            | HelpArticleId::ToolMetalSpots
            | HelpArticleId::ToolGeoFeatures
            | HelpArticleId::ToolFeature
            | HelpArticleId::ToolWater
            | HelpArticleId::ToolPaintLayer
            | HelpArticleId::ToolProcgen => HelpArticleCategory::Tools,

            HelpArticleId::LayeredPainter
            | HelpArticleId::AllyTeams
            | HelpArticleId::WaterAndLava => HelpArticleCategory::Reference,

            HelpArticleId::Pitfall01TextureMemory
            | HelpArticleId::Pitfall02Dxt1
            | HelpArticleId::Pitfall03SmtDedup
            | HelpArticleId::Pitfall04HeightmapDims
            | HelpArticleId::Pitfall06MapInfoSilentDeps
            | HelpArticleId::Pitfall07PinkMap
            | HelpArticleId::Pitfall08DntsWaterLos
            | HelpArticleId::Pitfall09Sd7Solidity
            | HelpArticleId::Pitfall11SundirCase
            | HelpArticleId::Pitfall13MetalmapZero
            | HelpArticleId::Pitfall14GeoVentsSpringboard
            | HelpArticleId::Pitfall15SplatSubtableForm
            | HelpArticleId::Pitfall18SundirW
            | HelpArticleId::Pitfall22MaxMetalScale
            | HelpArticleId::Pitfall25LuaGaiaBootstrap
            | HelpArticleId::Pitfall26StartBoxes
            | HelpArticleId::Pitfall28WaterPlaneConsteval => HelpArticleCategory::Pitfalls,
        }
    }

    /// Returns the PITFALLS.md anchor number for pitfall articles —
    /// the lint-rule wiring uses this to route `[Help…]` button
    /// clicks to the right article (per
    /// `LintRule::pitfall_anchor()`).
    #[allow(dead_code)] // wired by the lint-panel `[Help…]` button in commit 6
    pub fn pitfall_number(self) -> Option<u8> {
        match self {
            HelpArticleId::Pitfall01TextureMemory => Some(1),
            HelpArticleId::Pitfall02Dxt1 => Some(2),
            HelpArticleId::Pitfall03SmtDedup => Some(3),
            HelpArticleId::Pitfall04HeightmapDims => Some(4),
            HelpArticleId::Pitfall06MapInfoSilentDeps => Some(6),
            HelpArticleId::Pitfall07PinkMap => Some(7),
            HelpArticleId::Pitfall08DntsWaterLos => Some(8),
            HelpArticleId::Pitfall09Sd7Solidity => Some(9),
            HelpArticleId::Pitfall11SundirCase => Some(11),
            HelpArticleId::Pitfall13MetalmapZero => Some(13),
            HelpArticleId::Pitfall14GeoVentsSpringboard => Some(14),
            HelpArticleId::Pitfall15SplatSubtableForm => Some(15),
            HelpArticleId::Pitfall18SundirW => Some(18),
            HelpArticleId::Pitfall22MaxMetalScale => Some(22),
            HelpArticleId::Pitfall25LuaGaiaBootstrap => Some(25),
            HelpArticleId::Pitfall26StartBoxes => Some(26),
            HelpArticleId::Pitfall28WaterPlaneConsteval => Some(28),
            _ => None,
        }
    }

    /// Inverse of [`Self::pitfall_number`]: anchor number → article
    /// id. Used by the lint panel + build-overlay error linkers.
    /// Falls back to the closest available article for unrecognised
    /// numbers.
    #[allow(dead_code)] // wired by the lint panel `[Help…]` button in commit 6
    pub fn from_pitfall_anchor(n: u8) -> HelpArticleId {
        HelpArticleId::ALL
            .iter()
            .find(|id| id.pitfall_number() == Some(n))
            .copied()
            .unwrap_or(HelpArticleId::BuildPipeline)
    }

    /// Map a [`crate::Tool`] keyboard accelerator (single-char) to
    /// the tool's article id. Used by the tool-intro overlay's
    /// `[Read more in Help Center]` button + the command palette
    /// registrations.
    #[allow(dead_code)] // wired by the tool-intro overlay in commit 3 and palette in commit 4
    pub fn from_tool_accel(accel: &str) -> Option<HelpArticleId> {
        Some(match accel {
            "Q" => HelpArticleId::ToolSelect,
            "B" => HelpArticleId::ToolSculpt,
            "S" => HelpArticleId::ToolStartPositions,
            "M" => HelpArticleId::ToolMetalSpots,
            "V" => HelpArticleId::ToolGeoFeatures,
            "F" => HelpArticleId::ToolFeature,
            "W" => HelpArticleId::ToolWater,
            "L" => HelpArticleId::ToolPaintLayer,
            "G" => HelpArticleId::ToolProcgen,
            _ => return None,
        })
    }

    /// Inline article body. Lives as a markdown file under
    /// `help_content/` and is baked into the binary via
    /// `include_str!`.
    pub fn body(self) -> &'static str {
        match self {
            HelpArticleId::GettingStarted => include_str!("help_content/getting_started.md"),
            HelpArticleId::WhatsNew => include_str!("help_content/whats_new.md"),
            HelpArticleId::Shortcuts => include_str!("help_content/shortcuts.md"),
            HelpArticleId::BuildPipeline => include_str!("help_content/build_pipeline.md"),

            HelpArticleId::ToolSelect => include_str!("help_content/tool_select.md"),
            HelpArticleId::ToolSculpt => include_str!("help_content/tool_sculpt.md"),
            HelpArticleId::ToolStartPositions => {
                include_str!("help_content/tool_start_positions.md")
            }
            HelpArticleId::ToolMetalSpots => include_str!("help_content/tool_metal_spots.md"),
            HelpArticleId::ToolGeoFeatures => include_str!("help_content/tool_geo_features.md"),
            HelpArticleId::ToolFeature => include_str!("help_content/tool_feature.md"),
            HelpArticleId::ToolWater => include_str!("help_content/tool_water.md"),
            HelpArticleId::ToolPaintLayer => include_str!("help_content/tool_paint_layer.md"),
            HelpArticleId::ToolProcgen => include_str!("help_content/tool_procgen.md"),

            HelpArticleId::LayeredPainter => include_str!("help_content/layered_painter.md"),
            HelpArticleId::AllyTeams => include_str!("help_content/ally_teams.md"),
            HelpArticleId::WaterAndLava => include_str!("help_content/water_and_lava.md"),

            HelpArticleId::Pitfall01TextureMemory => {
                include_str!("help_content/pitfall_01_texture_pipeline_memory.md")
            }
            HelpArticleId::Pitfall02Dxt1 => {
                include_str!("help_content/pitfall_02_dxt1_slow_lossy.md")
            }
            HelpArticleId::Pitfall03SmtDedup => {
                include_str!("help_content/pitfall_03_smt_dedup.md")
            }
            HelpArticleId::Pitfall04HeightmapDims => {
                include_str!("help_content/pitfall_04_heightmap_dims.md")
            }
            HelpArticleId::Pitfall06MapInfoSilentDeps => {
                include_str!("help_content/pitfall_06_mapinfo_silent_deps.md")
            }
            HelpArticleId::Pitfall07PinkMap => {
                include_str!("help_content/pitfall_07_pink_map_rename.md")
            }
            HelpArticleId::Pitfall08DntsWaterLos => {
                include_str!("help_content/pitfall_08_dnts_water_los.md")
            }
            HelpArticleId::Pitfall09Sd7Solidity => {
                include_str!("help_content/pitfall_09_sd7_solidity.md")
            }
            HelpArticleId::Pitfall11SundirCase => {
                include_str!("help_content/pitfall_11_sundir_case.md")
            }
            HelpArticleId::Pitfall13MetalmapZero => {
                include_str!("help_content/pitfall_13_metalmap_zero.md")
            }
            HelpArticleId::Pitfall14GeoVentsSpringboard => {
                include_str!("help_content/pitfall_14_geo_vents_springboard.md")
            }
            HelpArticleId::Pitfall15SplatSubtableForm => {
                include_str!("help_content/pitfall_15_splat_subtable_form.md")
            }
            HelpArticleId::Pitfall18SundirW => {
                include_str!("help_content/pitfall_18_sundir_w.md")
            }
            HelpArticleId::Pitfall22MaxMetalScale => {
                include_str!("help_content/pitfall_22_max_metal_scale.md")
            }
            HelpArticleId::Pitfall25LuaGaiaBootstrap => {
                include_str!("help_content/pitfall_25_luagaia_bootstrap.md")
            }
            HelpArticleId::Pitfall26StartBoxes => {
                include_str!("help_content/pitfall_26_startboxes.md")
            }
            HelpArticleId::Pitfall28WaterPlaneConsteval => {
                include_str!("help_content/pitfall_28_water_plane_consteval.md")
            }
        }
    }
}

// ─── Window state ──────────────────────────────────────────────────

/// Mutable runtime state owned by `App`. Persistence lives in
/// [`crate::config::EditorConfig::tour_completed_for`] /
/// `tool_intros_seen`; this struct is session-only.
#[derive(Debug, Clone)]
pub struct HelpCenter {
    /// Window visible this frame?
    pub open: bool,
    /// Currently-rendered article. Defaults to Getting started on
    /// fresh App init.
    pub active_article: HelpArticleId,
    /// Search box state. Substring-matches against article bodies;
    /// empty string disables filtering.
    pub search: String,
    /// Previous-frame `open` for trace! transitions; mirrors the
    /// lint panel's pattern (see `ui::lint_panel`).
    pub previously_open: bool,
}

impl Default for HelpCenter {
    fn default() -> Self {
        HelpCenter {
            open: false,
            active_article: HelpArticleId::GettingStarted,
            search: String::new(),
            previously_open: false,
        }
    }
}

impl HelpCenter {
    /// Open the window and jump straight to `article`. Used by the
    /// lint panel's `[Help…]` button, the build-overlay error path,
    /// the wizard `[Start the tour]` Read-more pivot, and the
    /// command palette's `OpenHelp(...)` action.
    #[allow(dead_code)] // wired by the lint panel + tool intros + palette in commits 3–6
    pub fn open_at(&mut self, article: HelpArticleId) {
        self.open = true;
        self.active_article = article;
    }
}

// ─── Render ────────────────────────────────────────────────────────

/// Render the Help Center window if `hc.open == true`. The cheat
/// sheet `?` chord is still active in parallel — closing the help
/// center does not affect the cheat sheet (critical pitfall #10).
pub fn help_window(ctx: &egui::Context, hc: &mut HelpCenter) {
    if hc.open && !hc.previously_open {
        trace!(target: "barme::help_center", article = ?hc.active_article, "help_center opened");
    } else if !hc.open && hc.previously_open {
        trace!(target: "barme::help_center", "help_center closed");
    }
    hc.previously_open = hc.open;
    if !hc.open {
        return;
    }

    let t = Tokens::DARK;
    let mut local_open = true;
    egui::Window::new("Help center")
        .open(&mut local_open)
        .collapsible(false)
        .resizable(true)
        .default_width(820.0)
        .default_height(560.0)
        .show(ctx, |ui| {
            render_body(ui, hc, t);
        });
    if !local_open {
        hc.open = false;
    }
}

fn render_body(ui: &mut egui::Ui, hc: &mut HelpCenter, t: Tokens) {
    // ─── Search header ─────────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Search")
                .color(t.muted)
                .size(11.0),
        );
        let edit = egui::TextEdit::singleline(&mut hc.search)
            .desired_width(280.0)
            .hint_text("substring across article bodies");
        let response = ui.add(edit);
        if response.changed() {
            trace!(target: "barme::help_center", query = %hc.search, "help_center search edited");
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{} articles",
                    HelpArticleId::ALL.len()
                ))
                .color(t.muted)
                .size(11.0),
            );
        });
    });
    ui.separator();

    // ─── Split: tree on the left, article body on the right ───────
    let available = ui.available_size_before_wrap();
    let tree_width = 220.0_f32.min(available.x * 0.32);
    ui.horizontal_top(|ui| {
        ui.set_min_height(available.y);
        ui.vertical(|ui| {
            ui.set_width(tree_width);
            render_tree(ui, hc, t);
        });
        ui.separator();
        ui.vertical(|ui| {
            render_article(ui, hc, t);
        });
    });
}

fn render_tree(ui: &mut egui::Ui, hc: &mut HelpCenter, t: Tokens) {
    egui::ScrollArea::vertical()
        .id_salt("help_center_tree")
        .show(ui, |ui| {
            let q = hc.search.to_ascii_lowercase();
            let filter = !q.is_empty();
            for category in HelpArticleCategory::ALL {
                let matches: Vec<HelpArticleId> = HelpArticleId::ALL
                    .iter()
                    .copied()
                    .filter(|id| id.category() == category)
                    .filter(|id| {
                        !filter
                            || id.title().to_ascii_lowercase().contains(&q)
                            || id.body().to_ascii_lowercase().contains(&q)
                    })
                    .collect();
                if matches.is_empty() {
                    continue;
                }
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(category.label().to_uppercase())
                        .color(t.muted)
                        .size(10.0)
                        .strong(),
                );
                ui.add_space(2.0);
                for id in matches {
                    let is_active = hc.active_article == id;
                    let label = if is_active {
                        egui::RichText::new(id.title()).color(t.text).strong()
                    } else {
                        egui::RichText::new(id.title()).color(t.text)
                    };
                    if ui.selectable_label(is_active, label).clicked() {
                        hc.active_article = id;
                        trace!(target: "barme::help_center", article = ?id, "help_center article switched");
                    }
                }
                ui.add_space(6.0);
            }
            if filter
                && HelpArticleId::ALL.iter().all(|id| {
                    !id.title().to_ascii_lowercase().contains(&q)
                        && !id.body().to_ascii_lowercase().contains(&q)
                })
            {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("No matches.")
                        .color(t.muted)
                        .size(11.0),
                );
            }
        });
}

fn render_article(ui: &mut egui::Ui, hc: &HelpCenter, t: Tokens) {
    egui::ScrollArea::vertical()
        .id_salt("help_center_article")
        .show(ui, |ui| {
            ui.set_max_width(580.0);
            render_markdown(ui, hc.active_article.body(), t);
        });
}

// ─── Minimal markdown subset renderer ──────────────────────────────

/// Render a small markdown subset into `ui`. Supported:
/// - `# `, `## `, `### ` headings (sized 18 / 15 / 13 pt, strong, body
///   text colour for H1 / H2, muted for H3).
/// - Paragraphs separated by blank lines.
/// - `- ` or `* ` bulleted lists.
/// - Inline `code spans` (rendered monospace).
/// - Fenced ```` ``` ```` code blocks (rendered as a monospace frame).
///
/// Not supported (and not used by the Sprint 22 catalogue): nested
/// lists, links, images, blockquotes, tables-with-pipes (the shortcuts
/// article ships ASCII tables which still render readably as a code
/// block when wrapped — they aren't wrapped at present, so they read
/// as paragraphs). Stage 2 polish can swap to `egui_commonmark`.
pub fn render_markdown(ui: &mut egui::Ui, body: &str, t: Tokens) {
    let mut in_code = false;
    let mut code_buf = String::new();
    let mut paragraph: Vec<String> = Vec::new();

    fn flush_paragraph(ui: &mut egui::Ui, paragraph: &mut Vec<String>, t: Tokens) {
        if paragraph.is_empty() {
            return;
        }
        let joined = paragraph.join(" ");
        paragraph.clear();
        render_inline(ui, &joined, t);
        ui.add_space(4.0);
    }

    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            // Toggle fenced block.
            if in_code {
                // End of block.
                flush_paragraph(ui, &mut paragraph, t);
                render_code_block(ui, &code_buf, t);
                code_buf.clear();
                in_code = false;
            } else {
                flush_paragraph(ui, &mut paragraph, t);
                in_code = true;
            }
            continue;
        }
        if in_code {
            code_buf.push_str(line);
            code_buf.push('\n');
            continue;
        }

        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            flush_paragraph(ui, &mut paragraph, t);
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("# ") {
            flush_paragraph(ui, &mut paragraph, t);
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(rest)
                    .color(t.text)
                    .size(18.0)
                    .strong(),
            );
            ui.add_space(6.0);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("## ") {
            flush_paragraph(ui, &mut paragraph, t);
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(rest)
                    .color(t.text)
                    .size(15.0)
                    .strong(),
            );
            ui.add_space(4.0);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("### ") {
            flush_paragraph(ui, &mut paragraph, t);
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(rest)
                    .color(t.muted)
                    .size(13.0)
                    .strong(),
            );
            ui.add_space(3.0);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("- ") {
            flush_paragraph(ui, &mut paragraph, t);
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("•").color(t.muted));
                render_inline(ui, rest, t);
            });
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("* ") {
            flush_paragraph(ui, &mut paragraph, t);
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("•").color(t.muted));
                render_inline(ui, rest, t);
            });
            continue;
        }

        paragraph.push(trimmed.to_string());
    }
    flush_paragraph(ui, &mut paragraph, t);
    if in_code && !code_buf.is_empty() {
        // Unterminated fence — render the buffer anyway so the
        // content isn't silently dropped.
        render_code_block(ui, &code_buf, t);
    }
}

/// Render a body span that may contain inline `code`. Splits the
/// span on backticks; backtick-quoted runs render monospace.
fn render_inline(ui: &mut egui::Ui, span: &str, t: Tokens) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let mut in_code = false;
        let mut buf = String::new();
        for ch in span.chars() {
            if ch == '`' {
                if !buf.is_empty() {
                    if in_code {
                        ui.label(
                            egui::RichText::new(&buf)
                                .color(t.text)
                                .monospace()
                                .background_color(egui::Color32::from_rgba_premultiplied(
                                    255, 255, 255, 10,
                                )),
                        );
                    } else {
                        ui.label(egui::RichText::new(&buf).color(t.text));
                    }
                    buf.clear();
                }
                in_code = !in_code;
                continue;
            }
            buf.push(ch);
        }
        if !buf.is_empty() {
            if in_code {
                ui.label(
                    egui::RichText::new(&buf)
                        .color(t.text)
                        .monospace()
                        .background_color(egui::Color32::from_rgba_premultiplied(
                            255, 255, 255, 10,
                        )),
                );
            } else {
                ui.label(egui::RichText::new(&buf).color(t.text));
            }
        }
        // Preserve word-spacing between consecutive `horizontal_wrapped`
        // labels — the `item_spacing.x = 0` above kills the default
        // gap, but we add a single space at the end so successive
        // bullet lines wrap cleanly.
        ui.label(egui::RichText::new(" ").color(t.text));
    });
}

fn render_code_block(ui: &mut egui::Ui, body: &str, t: Tokens) {
    egui::Frame::new()
        .fill(egui::Color32::from_rgba_premultiplied(255, 255, 255, 8))
        .stroke(egui::Stroke::new(1.0, t.border))
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            for line in body.trim_end().lines() {
                ui.label(
                    egui::RichText::new(line)
                        .color(t.text)
                        .monospace()
                        .size(11.0),
                );
            }
        });
    ui.add_space(6.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sprint 22 exit criterion: ≥28 articles total.
    #[test]
    fn at_least_28_articles_registered() {
        assert!(
            HelpArticleId::ALL.len() >= 28,
            "Sprint 22 ships ≥28 articles, got {}",
            HelpArticleId::ALL.len()
        );
    }

    /// At least 9 tool articles — one per [`crate::Tool`] variant.
    #[test]
    fn nine_tool_articles_present() {
        let tools = HelpArticleId::ALL
            .iter()
            .filter(|id| id.category() == HelpArticleCategory::Tools)
            .count();
        assert!(tools >= 9, "expected ≥9 tool articles, got {tools}");
    }

    /// At least 15 pitfall articles per the prompt.
    #[test]
    fn fifteen_pitfall_articles_present() {
        let pitfalls = HelpArticleId::ALL
            .iter()
            .filter(|id| id.category() == HelpArticleCategory::Pitfalls)
            .count();
        assert!(
            pitfalls >= 15,
            "expected ≥15 pitfall articles, got {pitfalls}"
        );
    }

    /// 4 meta articles pinned to top of strip.
    #[test]
    fn four_meta_articles_present() {
        let meta = HelpArticleId::ALL
            .iter()
            .filter(|id| id.category() == HelpArticleCategory::Meta)
            .count();
        assert_eq!(meta, 4, "expected exactly 4 meta articles, got {meta}");
    }

    /// Compile-time check that every article body is non-empty and
    /// has ≥3 paragraphs (substring-counted by '\n\n' separators).
    #[test]
    fn every_article_has_at_least_three_paragraphs() {
        for id in HelpArticleId::ALL {
            let body = id.body();
            assert!(!body.is_empty(), "{:?} body is empty", id);
            // Count blank-line-separated paragraphs.
            let paragraphs = body.split("\n\n").filter(|p| !p.trim().is_empty()).count();
            assert!(
                paragraphs >= 3,
                "{:?} ({}) has only {paragraphs} paragraphs",
                id,
                id.title(),
            );
        }
    }

    /// `HelpArticleId::ALL` has no duplicate entries — a regression
    /// here would make the article-count test pass with a missing
    /// variant.
    #[test]
    fn no_duplicate_article_ids_in_all() {
        let mut seen = std::collections::HashSet::new();
        for id in HelpArticleId::ALL {
            assert!(seen.insert(*id), "duplicate id in HelpArticleId::ALL: {id:?}");
        }
    }

    /// Every pitfall article has a `pitfall_number()` and the
    /// `from_pitfall_anchor` inverse is a left-inverse on that set.
    #[test]
    fn pitfall_anchor_round_trips() {
        for id in HelpArticleId::ALL {
            if let Some(n) = id.pitfall_number() {
                assert_eq!(
                    HelpArticleId::from_pitfall_anchor(n),
                    *id,
                    "round-trip failed for §{n}"
                );
            }
        }
    }

    /// `from_tool_accel` covers every [`crate::Tool::ALL`] keyboard
    /// accelerator. We hand-tabulate here so a new tool variant
    /// must update both this catalogue AND the help_center mapping.
    #[test]
    fn tool_accel_mapping_covers_all_tools() {
        let known_accels = ["Q", "B", "S", "M", "V", "F", "W", "L", "G"];
        for accel in known_accels {
            assert!(
                HelpArticleId::from_tool_accel(accel).is_some(),
                "no help article for tool accel `{accel}`",
            );
        }
        assert!(
            HelpArticleId::from_tool_accel("Z").is_none(),
            "unknown accel should return None",
        );
    }

    /// Title strings are non-empty for every variant.
    #[test]
    fn titles_are_non_empty() {
        for id in HelpArticleId::ALL {
            assert!(!id.title().is_empty(), "empty title for {:?}", id);
        }
    }

    /// `HelpCenter::default()` is closed with Getting started
    /// selected.
    #[test]
    fn default_state_is_closed_on_getting_started() {
        let hc = HelpCenter::default();
        assert!(!hc.open);
        assert_eq!(hc.active_article, HelpArticleId::GettingStarted);
        assert!(hc.search.is_empty());
    }

    /// `open_at` jumps to the requested article and flips open.
    #[test]
    fn open_at_jumps_to_article_and_opens() {
        let mut hc = HelpCenter::default();
        hc.open_at(HelpArticleId::Pitfall04HeightmapDims);
        assert!(hc.open);
        assert_eq!(hc.active_article, HelpArticleId::Pitfall04HeightmapDims);
    }
}
