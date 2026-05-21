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

use barme_core::{MapInfo, MapInfoPatch, Project};

use super::{LintFix, LintIssue, LintRule, StockManifest};

fn issue(
    rule: LintRule,
    message: impl Into<String>,
    field_path: Option<&str>,
    fix: Option<LintFix>,
) -> LintIssue {
    LintIssue {
        rule,
        severity: rule.default_severity(),
        message: message.into(),
        field_path: field_path.map(str::to_string),
        fix,
    }
}

// ─── Hard-error rules ───

/// PITFALL §21. `modtype` must equal 3 (map).
pub(super) fn check_modtype(info: &MapInfo, out: &mut Vec<LintIssue>) {
    if info.modtype != 3 {
        out.push(issue(
            LintRule::ModtypeNotThree,
            format!(
                "modtype = {} (must be 3). 0=hidden, 1=primary, 2=unused, \
                 3=map, 4=base, 5=menu. Maps with modtype ≠ 3 do not appear \
                 in Chobby's map browser.",
                info.modtype
            ),
            Some("modtype"),
            None, // modtype is App-side state; the F9 form doesn't expose it. Sprint 22's help center will surface a fix.
        ));
    }
}

/// PITFALL §6 / SRS §1.3. `depend` must include `"Map Helper v1"`.
pub(super) fn check_depend(info: &MapInfo, out: &mut Vec<LintIssue>) {
    let has_helper = info.depend.iter().any(|d| d == "Map Helper v1");
    if !has_helper {
        out.push(issue(
            LintRule::DependMissingMapHelper,
            "`depend` must include \"Map Helper v1\". Without it, the engine \
             falls back to its untextured render path."
                .to_string(),
            Some("depend"),
            None,
        ));
    }
}

/// PITFALL §7. `smf.smtFileName0` must be non-empty — the pink-map
/// trap on rename.
pub(super) fn check_smt_file_name(info: &MapInfo, out: &mut Vec<LintIssue>) {
    if info.smf.smt_file_name_0.trim().is_empty() {
        out.push(issue(
            LintRule::SmtFileNameZeroMissing,
            "`smf.smtFileName0` is empty. The engine reads this to locate \
             the `.smt` tile file; missing or renamed → the infamous pink \
             map (PITFALL §7)."
                .to_string(),
            Some("smf.smtFileName0"),
            None,
        ));
    }
}

/// PITFALL §6 / archive scanner. `name`, `mapfile`, and `version`
/// are non-optional. Emits ONE issue listing every empty field.
pub(super) fn check_name_mapfile_version(info: &MapInfo, out: &mut Vec<LintIssue>) {
    let mut missing: Vec<&str> = Vec::new();
    if info.name.trim().is_empty() {
        missing.push("name");
    }
    if info.mapfile.trim().is_empty() {
        missing.push("mapfile");
    }
    if info.version.trim().is_empty() {
        missing.push("version");
    }
    if !missing.is_empty() {
        let path = if missing.len() == 1 {
            Some(missing[0])
        } else {
            None
        };
        out.push(issue(
            LintRule::NameOrMapfileOrVersionMissing,
            format!(
                "Required archive-scanner field(s) missing or empty: {}. \
                 SpringFiles indexing rejects archives without these.",
                missing.join(", ")
            ),
            path,
            None,
        ));
    }
}

/// PITFALL §6 / FINDINGS §1.5. `voidWater = true` and
/// `water.planeColor = Some(_)` are mutually exclusive. The emitter
/// auto-resolves by dropping `planeColor`, but the lint flags the
/// upstream state so the user knows the override is being silently
/// dropped.
pub(super) fn check_void_water(project: &Project, info: &MapInfo, out: &mut Vec<LintIssue>) {
    if !project.void_water {
        return;
    }
    let has_plane = project.water_overrides.plane_color.is_some()
        || info.water.as_ref().and_then(|w| w.plane_color).is_some();
    if has_plane {
        out.push(issue(
            LintRule::VoidWaterWithPlaneColor,
            "`voidWater = true` requires omitting `water.planeColor` — \
             setting any plane colour silently defeats voidWater. The \
             emitter clears it for you and warns; remove the override to \
             silence this lint."
                .to_string(),
            Some("water.planeColor"),
            Some(LintFix::MapInfoPatch(MapInfoPatch::VoidWater(false))),
        ));
    }
}

/// PITFALL §6 / SRS §1.3. Engine requires `teams[*].startPos`. The
/// emitter falls back to a 25/75 diagonal pair on empty
/// `ally_groups`, but the lint flags the upstream emptiness so the
/// user knows what will ship.
pub(super) fn check_teams_empty(project: &Project, out: &mut Vec<LintIssue>) {
    let total_positions: usize = project
        .ally_groups
        .iter()
        .map(|g| g.start_positions.len())
        .sum();
    if total_positions == 0 {
        out.push(issue(
            LintRule::TeamsEmpty,
            "No team start positions authored. The emitter will ship a \
             25 % / 75 % diagonal default pair so the engine has something \
             to read, but the resulting map plays as 1v1 only. Use the F8 \
             tool to author positions."
                .to_string(),
            Some("teams"),
            None,
        ));
    }
}

/// PITFALL §6 (Stage 2). `Project.features[].name` must exist in
/// BAR's stock `mapfeatures` manifest (or the map's own bundled
/// FeatureDef set — out of scope until Stage 2).
pub(super) fn check_features_in_manifest(
    project: &Project,
    stock: &StockManifest,
    out: &mut Vec<LintIssue>,
) {
    if project.features.is_empty() {
        return;
    }
    // Empty manifest = stock catalogue failed to load (logged at
    // load time). Don't fire the rule — false-positive on every
    // feature is worse than a missed unknown-name here.
    if stock.is_empty() {
        return;
    }
    let mut unknown: Vec<&str> = project
        .features
        .iter()
        .map(|f| f.name.as_str())
        .filter(|n| !stock.contains(n))
        .collect();
    unknown.sort();
    unknown.dedup();
    if !unknown.is_empty() {
        let preview = unknown
            .iter()
            .take(3)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        let suffix = if unknown.len() > 3 {
            format!(" (and {} more)", unknown.len() - 3)
        } else {
            String::new()
        };
        out.push(issue(
            LintRule::FeatureNotInStockManifest,
            format!(
                "Feature name(s) not in BAR's stock mapfeatures: {preview}{suffix}. \
                 Engine will `[GetFeatureDef] could not find FeatureDef` and \
                 silently drop these. Delete the bad features in the Inspector \
                 or rebuild after bundling a map-side FeatureDef set."
            ),
            None,
            None,
        ));
    }
}

/// PITFALL §6 + §17 reworded per FINDINGS §7.2. DNTS layers active
/// without a `specularTex` render visibly flatter than published BAR
/// maps. The D6 / Sprint 12 build pipeline ships a grey-BC1 fallback
/// when `Project.specular_tex_path` is `None` — this lint flags the
/// upstream state so the user knows a stock specular is shipping.
///
/// **Wording per FINDINGS §7.2:** NOT "DNTS silently disables."
/// The engine no longer gates DNTS on `specularTex` at the C++
/// render-state level (`SMFRenderState.cpp:114`).
pub(super) fn check_dnts_without_spec(project: &Project, info: &MapInfo, out: &mut Vec<LintIssue>) {
    let dnts_active = !info.resources.splat_detail_normal_tex.is_empty()
        || project.layers.dnts_layers().iter().any(|l| l.is_some());
    let has_spec = info.resources.specular_tex.is_some() || project.specular_tex_path.is_some();
    if dnts_active && !has_spec {
        out.push(issue(
            LintRule::SplatDetailNormalTexWithoutSpecular,
            "DNTS layers are active but no specular texture is set. DNTS \
             still renders (engine no longer gates on `specularTex` — see \
             FINDINGS §7.2), but the result looks noticeably flatter than \
             published BAR maps. The build pipeline ships a grey-BC1 \
             fallback; supply a real specular for parity."
                .to_string(),
            Some("resources.specularTex"),
            None,
        ));
    }
}

/// PITFALL §6. `fogStart == fogEnd` breaks the build-ETA grid
/// renderer.
pub(super) fn check_fog_start_end(info: &MapInfo, out: &mut Vec<LintIssue>) {
    let (Some(start), Some(end)) = (info.atmosphere.fog_start, info.atmosphere.fog_end) else {
        return;
    };
    if (start - end).abs() < f32::EPSILON {
        out.push(issue(
            LintRule::FogStartEqualsFogEnd,
            format!(
                "`atmosphere.fogStart` ({start}) equals `fogEnd` ({end}) — \
                 this breaks the build-ETA grid renderer. BAR convention is \
                 fogStart = 0.1, fogEnd = 1.0."
            ),
            Some("atmosphere.fogEnd"),
            Some(LintFix::MapInfoPatch(MapInfoPatch::AtmosphereFogEnd(Some(
                1.0,
            )))),
        ));
    }
}

/// PITFALL §4. Heightmap dims must be `(64·N + 1)²`. `MapSize`
/// always produces compliant dims, so this fires only on external
/// import paths (F13 / Stage 2). Sprint 21 leaves it as a future
/// guard.
pub(super) fn check_heightmap_dims(project: &Project, out: &mut Vec<LintIssue>) {
    let (w, h) = project.size.heightmap_dims();
    let bad_w = (w as i64 - 1) % 64 != 0 || w < 65;
    let bad_h = (h as i64 - 1) % 64 != 0 || h < 65;
    if bad_w || bad_h {
        out.push(issue(
            LintRule::HeightmapDimsWrong,
            format!(
                "Heightmap dims {w}×{h} are not (64·N + 1)². PyMapConv will \
                 warn + silently resize, producing visibly wrong terrain. \
                 Use the wizard to pick a valid SMU count."
            ),
            None,
            None,
        ));
    }
}

// ─── Warning rules (commit 3 fills these in) ───

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

#[cfg(test)]
mod tests {
    use super::super::LintSeverity;
    use super::*;
    use barme_core::{
        AllyGroup, FeatureInstance, MapInfo, MapSize, Project, StartPosition, WaterMode,
    };

    /// Shared positive-test starting point: a `Project` that lints
    /// clean (no hard errors). Negative tests start from this and
    /// mutate exactly one field to trigger their rule.
    fn fixture_project() -> Project {
        let mut p = Project::new("smoke", 4);
        // TeamsEmpty fires on zero positions; seed one ally group
        // with one start position so the fixture matches the wizard's
        // post-state.
        let mut g = AllyGroup::new(0);
        g.start_positions.push(StartPosition {
            x_elmo: 512,
            z_elmo: 512,
        });
        p.ally_groups.push(g);
        p
    }

    /// Helper: lint with a populated stock manifest so the
    /// feature-not-in-manifest negative tests don't depend on the
    /// embedded JSON.
    fn lint(project: &Project) -> Vec<LintIssue> {
        let info: MapInfo = project.into();
        let stock = StockManifest::from_names(["geovent", "pinetree", "rock1", "agorm_talltree6"]);
        super::super::lint_with(project, &info, &stock)
    }

    fn fired(issues: &[LintIssue], rule: LintRule) -> bool {
        issues.iter().any(|i| i.rule == rule)
    }

    // ─── ModtypeNotThree ───

    #[test]
    fn modtype_not_three_fires_when_modtype_is_zero() {
        let p = fixture_project();
        let mut info: MapInfo = (&p).into();
        info.modtype = 0;
        let issues = super::super::lint_with(&p, &info, &StockManifest::default());
        assert!(
            fired(&issues, LintRule::ModtypeNotThree),
            "expected ModtypeNotThree to fire; issues:\n{issues:#?}"
        );
    }

    #[test]
    fn modtype_not_three_silent_on_clean_project() {
        let p = fixture_project();
        assert!(!fired(&lint(&p), LintRule::ModtypeNotThree));
    }

    // ─── DependMissingMapHelper ───

    #[test]
    fn depend_missing_map_helper_fires_when_depend_empty() {
        let p = fixture_project();
        let mut info: MapInfo = (&p).into();
        info.depend.clear();
        let issues = super::super::lint_with(&p, &info, &StockManifest::default());
        assert!(fired(&issues, LintRule::DependMissingMapHelper));
    }

    #[test]
    fn depend_missing_map_helper_silent_when_helper_present() {
        let p = fixture_project();
        assert!(!fired(&lint(&p), LintRule::DependMissingMapHelper));
    }

    // ─── SmtFileNameZeroMissing ───

    #[test]
    fn smt_file_name_zero_fires_when_empty() {
        let p = fixture_project();
        let mut info: MapInfo = (&p).into();
        info.smf.smt_file_name_0 = String::new();
        let issues = super::super::lint_with(&p, &info, &StockManifest::default());
        assert!(fired(&issues, LintRule::SmtFileNameZeroMissing));
    }

    #[test]
    fn smt_file_name_zero_silent_when_set() {
        let p = fixture_project();
        assert!(!fired(&lint(&p), LintRule::SmtFileNameZeroMissing));
    }

    // ─── NameOrMapfileOrVersionMissing ───

    #[test]
    fn name_missing_fires() {
        let p = fixture_project();
        let mut info: MapInfo = (&p).into();
        info.name = String::new();
        let issues = super::super::lint_with(&p, &info, &StockManifest::default());
        assert!(fired(&issues, LintRule::NameOrMapfileOrVersionMissing));
    }

    #[test]
    fn version_missing_fires() {
        let p = fixture_project();
        let mut info: MapInfo = (&p).into();
        info.version = String::new();
        let issues = super::super::lint_with(&p, &info, &StockManifest::default());
        assert!(fired(&issues, LintRule::NameOrMapfileOrVersionMissing));
    }

    #[test]
    fn name_mapfile_version_silent_when_all_set() {
        let p = fixture_project();
        assert!(!fired(&lint(&p), LintRule::NameOrMapfileOrVersionMissing));
    }

    // ─── VoidWaterWithPlaneColor ───

    #[test]
    fn void_water_with_plane_color_fires() {
        let mut p = fixture_project();
        p.void_water = true;
        p.water_overrides.plane_color = Some([0.1, 0.2, 0.3]);
        assert!(fired(&lint(&p), LintRule::VoidWaterWithPlaneColor));
    }

    #[test]
    fn void_water_silent_when_plane_color_unset() {
        let mut p = fixture_project();
        p.void_water = true;
        // plane_color stays None
        assert!(!fired(&lint(&p), LintRule::VoidWaterWithPlaneColor));
    }

    // ─── TeamsEmpty ───

    #[test]
    fn teams_empty_fires_with_no_ally_groups() {
        let p = Project::new("empty", 4);
        assert!(p.ally_groups.is_empty());
        assert!(fired(&lint(&p), LintRule::TeamsEmpty));
    }

    #[test]
    fn teams_empty_silent_with_authored_positions() {
        let p = fixture_project();
        assert!(!fired(&lint(&p), LintRule::TeamsEmpty));
    }

    // ─── FeatureNotInStockManifest ───

    #[test]
    fn feature_not_in_manifest_fires_on_unknown_name() {
        let mut p = fixture_project();
        p.features
            .push(FeatureInstance::new("nonsense_blob", 100, 100, 0));
        let issues = lint(&p);
        assert!(
            fired(&issues, LintRule::FeatureNotInStockManifest),
            "expected fire on unknown feature; issues:\n{issues:#?}"
        );
    }

    #[test]
    fn feature_not_in_manifest_silent_on_known_name() {
        let mut p = fixture_project();
        p.features
            .push(FeatureInstance::new("pinetree", 100, 100, 0));
        assert!(!fired(&lint(&p), LintRule::FeatureNotInStockManifest));
    }

    #[test]
    fn feature_not_in_manifest_silent_on_empty_manifest() {
        // Empty manifest = catalogue load failure; lint should not
        // false-positive every project under those circumstances.
        let mut p = fixture_project();
        p.features
            .push(FeatureInstance::new("anything", 100, 100, 0));
        let info: MapInfo = (&p).into();
        let issues = super::super::lint_with(&p, &info, &StockManifest::default());
        assert!(!fired(&issues, LintRule::FeatureNotInStockManifest));
    }

    // ─── SplatDetailNormalTexWithoutSpecular ───

    #[test]
    fn dnts_without_spec_fires_when_dnts_paths_set_no_specular() {
        let p = fixture_project();
        let mut info: MapInfo = (&p).into();
        info.resources.splat_detail_normal_tex = vec!["a.dds".into(), "b.dds".into()];
        info.resources.specular_tex = None;
        let issues = super::super::lint_with(&p, &info, &StockManifest::default());
        assert!(fired(
            &issues,
            LintRule::SplatDetailNormalTexWithoutSpecular
        ));
    }

    #[test]
    fn dnts_without_spec_silent_when_spec_present() {
        let p = fixture_project();
        let mut info: MapInfo = (&p).into();
        info.resources.splat_detail_normal_tex = vec!["a.dds".into()];
        info.resources.specular_tex = Some("spec.dds".into());
        let issues = super::super::lint_with(&p, &info, &StockManifest::default());
        assert!(!fired(
            &issues,
            LintRule::SplatDetailNormalTexWithoutSpecular
        ));
    }

    #[test]
    fn dnts_without_spec_silent_when_no_dnts_active() {
        let p = fixture_project();
        // No DNTS layers, no splat normals set, no specular: lint silent.
        assert!(!fired(
            &lint(&p),
            LintRule::SplatDetailNormalTexWithoutSpecular
        ));
    }

    // ─── FogStartEqualsFogEnd ───

    #[test]
    fn fog_start_equals_fog_end_fires() {
        let p = fixture_project();
        let mut info: MapInfo = (&p).into();
        info.atmosphere.fog_start = Some(0.5);
        info.atmosphere.fog_end = Some(0.5);
        let issues = super::super::lint_with(&p, &info, &StockManifest::default());
        assert!(fired(&issues, LintRule::FogStartEqualsFogEnd));
    }

    #[test]
    fn fog_start_differs_from_fog_end_silent() {
        let p = fixture_project();
        // Default 0.1 / 1.0 → differ.
        assert!(!fired(&lint(&p), LintRule::FogStartEqualsFogEnd));
    }

    // ─── HeightmapDimsWrong ───

    #[test]
    fn heightmap_dims_silent_for_valid_smu() {
        // Any MapSize::square(N) → dims = (64·N + 1, 64·N + 1).
        for n in [2, 4, 8, 16, 32] {
            let mut p = fixture_project();
            p.size = MapSize::square(n);
            assert!(
                !fired(&lint(&p), LintRule::HeightmapDimsWrong),
                "SMU {n} should produce valid heightmap dims"
            );
        }
    }

    #[test]
    fn heightmap_dims_fires_on_off_grid_dims() {
        // Construct a synthetic project whose MapSize would yield
        // non-conforming heightmap dims. MapSize itself prevents
        // this, so we hand-bind a value that has bad dims by
        // construction.
        let mut p = fixture_project();
        // smu_x = 0 → heightmap_dims (1, 1) → (1 - 1) % 64 == 0 BUT
        // width < 65 trips the < 65 guard.
        p.size = MapSize { smu_x: 0, smu_z: 4 };
        assert!(fired(&lint(&p), LintRule::HeightmapDimsWrong));
    }

    // ─── End-to-end fresh-project smoke ───

    #[test]
    fn wizard_style_fixture_has_zero_hard_errors() {
        let p = fixture_project();
        let issues = lint(&p);
        let errors: Vec<&LintIssue> = issues
            .iter()
            .filter(|i| i.severity == LintSeverity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "wizard-style fixture produced hard errors: {errors:#?}"
        );
    }

    /// Sanity: a project with several deliberate breakages produces
    /// each expected hard-error rule exactly once.
    #[test]
    fn multiple_hard_errors_compose() {
        let mut p = Project::new("brokenmap", 4);
        // (TeamsEmpty fires via no ally_groups.)
        p.water_mode = WaterMode::Ocean;
        p.void_water = true;
        p.water_overrides.plane_color = Some([0.1, 0.2, 0.3]);
        // VoidWaterWithPlaneColor fires.
        let mut info: MapInfo = (&p).into();
        info.modtype = 0;
        // ModtypeNotThree fires.
        info.depend.clear();
        // DependMissingMapHelper fires.
        info.atmosphere.fog_start = Some(0.5);
        info.atmosphere.fog_end = Some(0.5);
        // FogStartEqualsFogEnd fires.
        let issues = super::super::lint_with(&p, &info, &StockManifest::default());
        for rule in &[
            LintRule::TeamsEmpty,
            LintRule::ModtypeNotThree,
            LintRule::DependMissingMapHelper,
            LintRule::VoidWaterWithPlaneColor,
            LintRule::FogStartEqualsFogEnd,
        ] {
            assert!(
                fired(&issues, *rule),
                "{rule:?} should fire; issues:\n{issues:#?}"
            );
        }
    }
}
