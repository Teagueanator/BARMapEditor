//! Per-rule implementations for the C8 lint registry.
//!
//! Each rule is a `pub(super) fn check_<rule>(&Project, &MapInfo,
//! &mut Vec<LintIssue>)` (some take only a subset of those args).
//! The dispatcher in [`super::lint_with`] calls every check in
//! [`LintRule::ALL`] order.
//!
//! ## Style
//!
//! - Push at most one [`LintIssue`] per rule per call. Multiple
//!   offending fields collapse into one issue with an enumerated
//!   message; the panel shows one row per rule, not one per offending
//!   value.
//! - Use `LintRule::default_severity` unless the rule has a
//!   genuinely variable severity (rare).
//! - Set `field_path` whenever a single Lua field is the focus —
//!   `lighting.sunDir`, `atmosphere.fogStart`. Multi-field rules use
//!   the shortest prefix that uniquely identifies the F9 tab.
//! - Include a `LintFix` only when the remediation is unambiguous.
//!   "Set X to BAR's convention" is fine; "rewrite this whole
//!   feature subtree" is not.
//!
//! For Sprint 21 / commit 1 the rules are stubs that always pass —
//! commits 2–4 fill in the real logic.

use barme_core::{MapInfo, Project};

use super::{LintIssue, LintRule, LintSeverity, StockManifest};

// ─── Hard-error stubs (commit 2 fills these in) ───

pub(super) fn check_modtype(_info: &MapInfo, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_depend(_info: &MapInfo, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_smt_file_name(_info: &MapInfo, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_name_mapfile_version(_info: &MapInfo, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_void_water(_project: &Project, _info: &MapInfo, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_teams_empty(_project: &Project, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_features_in_manifest(
    _project: &Project,
    _stock: &StockManifest,
    _out: &mut Vec<LintIssue>,
) {
}
pub(super) fn check_dnts_without_spec(
    _project: &Project,
    _info: &MapInfo,
    _out: &mut Vec<LintIssue>,
) {
}
pub(super) fn check_fog_start_end(_info: &MapInfo, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_heightmap_dims(_project: &Project, _out: &mut Vec<LintIssue>) {}

// ─── Warning stubs (commit 3 fills these in) ───

pub(super) fn check_lighting_sun_dir_missing(_info: &MapInfo, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_atmosphere_sky_dir_present(_project: &Project, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_gui_minimap_rotation(_project: &Project, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_extractor_radius_five_hundred(
    _project: &Project,
    _info: &MapInfo,
    _out: &mut Vec<LintIssue>,
) {
}
pub(super) fn check_tidal_without_surface(
    _project: &Project,
    _info: &MapInfo,
    _out: &mut Vec<LintIssue>,
) {
}
pub(super) fn check_terrain_below_without_water(_project: &Project, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_water_without_terrain_below(_project: &Project, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_teams_less_than_sixteen(_project: &Project, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_startboxes_missing(_project: &Project, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_resources_detail_tex_dnts(
    _project: &Project,
    _info: &MapInfo,
    _out: &mut Vec<LintIssue>,
) {
}
pub(super) fn check_geo_in_metal_layout(_project: &Project, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_metalmap_nonzero(_project: &Project, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_sun_dir_w_large(_info: &MapInfo, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_splat_detail_normal_legacy(_info: &MapInfo, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_dnts_below_zero(_project: &Project, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_start_position_shape(_project: &Project, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_lua_gaia_team_missing(_project: &Project, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_metal_value_range(_info: &MapInfo, _out: &mut Vec<LintIssue>) {}

// ─── Info-tier stubs (commit 4 fills these in) ───

pub(super) fn check_gravity_drift(_info: &MapInfo, _out: &mut Vec<LintIssue>) {}
pub(super) fn check_extractor_radius_drift(
    _project: &Project,
    _info: &MapInfo,
    _out: &mut Vec<LintIssue>,
) {
}
pub(super) fn check_void_ground_alpha_min(_info: &MapInfo, _out: &mut Vec<LintIssue>) {}

// Until commits 2–4 ship, suppress dead-code warnings on
// LintRule::name -> _ on rule construction. The match arms in
// LintRule::name/title/etc. exhaustively cover ALL variants, so the
// only way variants are "constructed" is through ALL itself. That
// keeps clippy quiet on stub functions.
#[allow(dead_code)]
fn _suppress_unused_imports(severity: LintSeverity, rule: LintRule) {
    let _ = severity;
    let _ = rule;
}
