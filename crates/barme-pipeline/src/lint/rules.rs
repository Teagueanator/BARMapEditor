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

// ─── Warning rules ───

/// PITFALL §11 / FINDINGS §1.4. Fires when `lighting.sun_dir`'s xyz
/// is the zero vector — a degenerate direction the engine treats as
/// "no sun" (everything renders dark / unlit). The schema's
/// `bar_default` ships `[0.3, 1.0, -0.2, 1.0]`; this lint catches a
/// user resetting the field to all zeros through F9.
pub(super) fn check_lighting_sun_dir_missing(info: &MapInfo, out: &mut Vec<LintIssue>) {
    let [x, y, z, _w] = info.lighting.sun_dir;
    if x.abs() < f32::EPSILON && y.abs() < f32::EPSILON && z.abs() < f32::EPSILON {
        out.push(issue(
            LintRule::LightingSunDirMissing,
            "`lighting.sunDir` xyz is the zero vector — terrain will \
             render unlit. Set a real direction (BAR convention: \
             ~[0.3, 1.0, -0.2, 1.0])."
                .to_string(),
            Some("lighting.sunDir"),
            Some(LintFix::MapInfoPatch(MapInfoPatch::LightingSunDir([
                0.3, 1.0, -0.2, 1.0,
            ]))),
        ));
    }
}

/// PITFALL §12 / FINDINGS §1.3. `atmosphere.skyDir` is deprecated.
/// The emitter never writes it (always emits `skyAxisAngle`). This
/// lint guards against user-imported state where `mapinfo_overrides`
/// carries the deprecated key.
pub(super) fn check_atmosphere_sky_dir_present(project: &Project, out: &mut Vec<LintIssue>) {
    if project
        .mapinfo_overrides
        .keys()
        .any(|k| k == "atmosphere.skyDir" || k == "atmosphere.sky_dir")
    {
        out.push(issue(
            LintRule::AtmosphereSkyDirPresent,
            "`atmosphere.skyDir` is deprecated (engine logs `L_DEPRECATED`). \
             Use `atmosphere.skyAxisAngle` (float4: axis xyz + radians) \
             instead. Drop the override; the emitter writes the modern key."
                .to_string(),
            Some("atmosphere.skyAxisAngle"),
            None,
        ));
    }
}

/// PITFALL §19 / FINDINGS §1.11. `gui.minimapRotation` is unused.
/// Same pattern as [`check_atmosphere_sky_dir_present`] — fires on
/// `mapinfo_overrides` containing the dead key.
pub(super) fn check_gui_minimap_rotation(project: &Project, out: &mut Vec<LintIssue>) {
    if project
        .mapinfo_overrides
        .keys()
        .any(|k| k == "gui.minimapRotation" || k == "gui.minimap_rotation")
    {
        out.push(issue(
            LintRule::GuiMinimapRotationPresent,
            "`gui.minimapRotation` is not consumed by current Recoil \
             (`MapInfo.cpp:119-124`). Drop the override; the field is dead."
                .to_string(),
            Some("gui.minimapRotation"),
            None,
        ));
    }
}

/// PITFALL §6. `extractorRadius = 500` (the engine default) breaks
/// BAR's mex snap. BAR uses 80.
pub(super) fn check_extractor_radius_five_hundred(
    project: &Project,
    _info: &MapInfo,
    out: &mut Vec<LintIssue>,
) {
    if (project.extractor_radius - 500.0).abs() < 0.5 {
        out.push(issue(
            LintRule::ExtractorRadiusFiveHundred,
            "`extractorRadius = 500` is the engine default but breaks BAR's \
             mex-snap UI. Set it to 80 (BAR convention)."
                .to_string(),
            Some("extractorRadius"),
            Some(LintFix::MapInfoPatch(MapInfoPatch::ExtractorRadius(Some(
                80.0,
            )))),
        ));
    }
}

/// PITFALL §6. `tidalStrength > 0` without a `water.surfaceColor`
/// means tidal generators visually clip into invisible water. Fires
/// when tidal is on but the emitted water block has no surface
/// colour.
pub(super) fn check_tidal_without_surface(
    project: &Project,
    info: &MapInfo,
    out: &mut Vec<LintIssue>,
) {
    let tidal = project.tidal_strength.unwrap_or(0.0);
    if tidal <= 0.0 {
        return;
    }
    let has_surface = info.water.as_ref().and_then(|w| w.surface_color).is_some();
    if !has_surface {
        out.push(issue(
            LintRule::TidalStrengthWithoutWaterSurfaceColor,
            format!(
                "`tidalStrength = {tidal}` but no `water.surfaceColor` is \
                 set. Tidal generators will visually clip into invisible \
                 water. Pick a water preset (Ocean / Tropical / …) so the \
                 surface renders."
            ),
            Some("water.surfaceColor"),
            None,
        ));
    }
}

/// PITFALL §6 — `min_height < 0` with no water preset = engine
/// renders its default blue ocean (silent surprise). Migrated from
/// `App::validation_summary`'s WARN tier (Sprint 14 / C9).
pub(super) fn check_terrain_below_without_water(project: &Project, out: &mut Vec<LintIssue>) {
    use barme_core::WaterMode;
    if project.min_height < 0.0 && project.water_mode == WaterMode::None {
        out.push(issue(
            LintRule::TerrainBelowZeroWithoutWater,
            format!(
                "Terrain dips below Y = 0 (min_height = {:.1}) but no water \
                 preset is selected. The engine will render its default blue \
                 ocean — pick a preset in the Water tool to make it explicit.",
                project.min_height
            ),
            Some("water"),
            None,
        ));
    }
}

/// Sprint 14 / C9 inverse — `water_mode != None` with
/// `min_height >= 0` = no water visible without `forceRendering`.
/// Migrated from `App::validation_summary`.
pub(super) fn check_water_without_terrain_below(project: &Project, out: &mut Vec<LintIssue>) {
    use barme_core::WaterMode;
    if project.water_mode != WaterMode::None && project.min_height >= 0.0 {
        out.push(issue(
            LintRule::WaterModeSetWithoutTerrainBelowZero,
            format!(
                "Water preset `{:?}` is set but terrain doesn't dip below \
                 Y = 0 (min_height = {:.1}) — water won't be visible. Either \
                 carve a basin with the Water tool's flood brush or set \
                 `forceRendering = true`.",
                project.water_mode, project.min_height
            ),
            Some("water"),
            None,
        ));
    }
}

/// PITFALL §6. Large maps (≥4 ally groups) should surface ≥16
/// teams so BAR's lobby can match expected slot counts. Fires when
/// the project has ≥4 ally groups but total team positions < 16.
pub(super) fn check_teams_less_than_sixteen(project: &Project, out: &mut Vec<LintIssue>) {
    let ally_count = project.ally_groups.len();
    let total: usize = project
        .ally_groups
        .iter()
        .map(|g| g.start_positions.len())
        .sum();
    if ally_count >= 4 && total < 16 {
        out.push(issue(
            LintRule::TeamsLessThanSixteenOnLargeMap,
            format!(
                "{ally_count} ally groups but only {total} team positions. \
                 Large-map BAR lobbies expect ≥16 slots — add more start \
                 positions per group."
            ),
            Some("teams"),
            None,
        ));
    }
}

/// PITFALL §26. Multi-ally-group projects (>2 groups) without any
/// `box_polygon` set emit no `map_startboxes.lua`, which falls back
/// to BAR's auto-N/S split — fine for 1v1 but jarring for 3+ team
/// maps where players expect explicit corner/sector boxes.
pub(super) fn check_startboxes_missing(project: &Project, out: &mut Vec<LintIssue>) {
    if project.ally_groups.len() <= 2 {
        return;
    }
    let any_box = project.ally_groups.iter().any(|g| g.box_polygon.is_some());
    if !any_box {
        out.push(issue(
            LintRule::StartboxesLuaMissingWhenMultiTeam,
            format!(
                "{} ally groups but no start-box polygons authored. BAR will \
                 fall back to its auto-N/S split, which doesn't fit FFA / \
                 multi-corner layouts. Author a polygon per ally group in \
                 the F8 tool.",
                project.ally_groups.len()
            ),
            Some("ally_groups"),
            None,
        ));
    }
}

/// PITFALL §6. DNTS layers active without `resources.detailTex` —
/// the base detail texture — render visibly flat. Fires when DNTS
/// is on and `info.resources.detail_tex.is_none()`.
pub(super) fn check_resources_detail_tex_dnts(
    project: &Project,
    info: &MapInfo,
    out: &mut Vec<LintIssue>,
) {
    let dnts_active = !info.resources.splat_detail_normal_tex.is_empty()
        || project.layers.dnts_layers().iter().any(|l| l.is_some());
    if dnts_active && info.resources.detail_tex.is_none() {
        out.push(issue(
            LintRule::ResourcesDetailTexMissingOnDntsMap,
            "DNTS layers active but `resources.detailTex` is unset. The base \
             detail texture is what the splat normals modulate; without it \
             the terrain looks visibly flat under close-camera. The engine \
             falls back to `resources.lua` `graphics.maps.detailtex` → \
             `detailtex2.bmp` — ship a real one for parity."
                .to_string(),
            Some("resources.detailTex"),
            None,
        ));
    }
}

/// PITFALL §14 / FINDINGS §5. Future-import guard: BAR has no
/// gadget reading `map_metal_layout.geos[]` — geo vents go through
/// the Springboard featureplacer trio. Fires when
/// `mapinfo_overrides` carries any dotted key under
/// `map_metal_layout.geos`.
pub(super) fn check_geo_in_metal_layout(project: &Project, out: &mut Vec<LintIssue>) {
    if project
        .mapinfo_overrides
        .keys()
        .any(|k| k.starts_with("map_metal_layout.geos") || k.starts_with("metal_layout.geos"))
    {
        out.push(issue(
            LintRule::GeoInMetalLayoutGeosArray,
            "Project carries a `map_metal_layout.geos[]` override (Zero-K \
             convention). BAR derives geo vents from features with \
             `geoThermal = true` — the array is silently ignored. Migrate \
             the entries to `Project.geo_vents` via the V (geovent) tool."
                .to_string(),
            Some("metal_layout.geos"),
            None,
        ));
    }
}

/// PITFALL §13 / FINDINGS §5. `map_metal_spot_placer.lua` bails if
/// any SMF metalmap pixel is non-zero. The pipeline ships an
/// all-zero PNG when `metal_spots` is non-empty; this lint guards
/// the future-import path where a user-supplied metalmap could
/// shadow the zero-bytes contract.
pub(super) fn check_metalmap_nonzero(project: &Project, out: &mut Vec<LintIssue>) {
    if project.metal_spots.is_empty() {
        return;
    }
    let custom_metalmap = project
        .mapinfo_overrides
        .get("smf.metalmapTex")
        .or_else(|| project.mapinfo_overrides.get("smf.metalmap_tex"));
    if let Some(v) = custom_metalmap {
        let path = match v {
            toml::Value::String(s) => s.as_str(),
            _ => "<unknown>",
        };
        if !path.trim().is_empty() {
            out.push(issue(
                LintRule::SmfMetalmapNonZeroWithLuaSpots,
                format!(
                    "Project has {} Lua metal spot(s) AND a custom \
                     `smf.metalmapTex` override (`{path}`). BAR's \
                     `map_metal_spot_placer.lua` bails when ANY SMF \
                     metalmap pixel is non-zero. Drop the override or \
                     ensure the texture is all-black.",
                    project.metal_spots.len()
                ),
                Some("smf.metalmapTex"),
                None,
            ));
        }
    }
}

/// PITFALL §18 / FINDINGS §1.4 / NEW-6. `lighting.sun_dir.w` is
/// an intensity scalar with engine default `1.0`. Fires when w > 100
/// (the older `1e9` sunStartDistance leakage would land here).
pub(super) fn check_sun_dir_w_large(info: &MapInfo, out: &mut Vec<LintIssue>) {
    let w = info.lighting.sun_dir[3];
    if w > 100.0 {
        out.push(issue(
            LintRule::SunDirWIsLarge,
            format!(
                "`lighting.sunDir.w` = {w} (engine default 1.0). Older \
                 research mis-attributed this as `sunStartDistance` (1e9), \
                 which over-saturates sunlight on load. Set w to ~1.0."
            ),
            Some("lighting.sunDir"),
            Some(LintFix::MapInfoPatch(MapInfoPatch::LightingSunDir([
                info.lighting.sun_dir[0],
                info.lighting.sun_dir[1],
                info.lighting.sun_dir[2],
                1.0,
            ]))),
        ));
    }
}

/// PITFALL §15 / FINDINGS §1.8. The legacy
/// `splatDetailNormalDiffuseAlpha` top-level key is shadowed by the
/// subtable's `alpha` field. The emitter writes the subtable form;
/// this lint guards user-imported state with the legacy key.
pub(super) fn check_splat_detail_normal_legacy(info: &MapInfo, out: &mut Vec<LintIssue>) {
    if info.resources.splat_detail_normal_diffuse_alpha.is_some() {
        out.push(issue(
            LintRule::SplatDetailNormalTexLegacyForm,
            "`resources.splatDetailNormalDiffuseAlpha` is the legacy form \
             (engine prefers the subtable's `alpha` companion). Mixing the \
             two silently shadows the subtable. Drop the legacy key — the \
             emitter writes the modern form."
                .to_string(),
            Some("resources.splatDetailNormalTex"),
            None,
        ));
    }
}

/// PITFALL §8. DNTS layers + `min_height < 0` triggers the LOS
/// animated-snow bug (Beherith forum t=35202). Migrated from
/// `App::validation_summary`'s WARN tier (Sprint 14 / C9).
pub(super) fn check_dnts_below_zero(project: &Project, out: &mut Vec<LintIssue>) {
    let dnts_active = project.layers.dnts_layers().iter().any(|l| l.is_some());
    if dnts_active && project.min_height < 0.0 {
        out.push(issue(
            LintRule::DntsOnMapWithMinHeightBelowZero,
            format!(
                "DNTS layers active on a map with `min_height = {:.1}` < 0. \
                 Any LOS-touching Lua widget will trigger TV-snow artefacts \
                 (Beherith forum t=35202). Raise min_height above 0 OR \
                 retire the DNTS layers.",
                project.min_height
            ),
            None,
            None,
        ));
    }
}

/// PITFALL §23 — featureplacer rotation is an unquoted integer.
/// The editor's `FeatureInstance.rot_heading: u16` is always an
/// integer, so this rule cannot fire from editor-created state. It
/// guards a future F13 import path where `mapinfo_overrides`
/// carries `featureplacer.quoted_rot = true` (indicating the
/// imported `set.lua` had string-quoted rotations that the parser
/// preserved verbatim).
pub(super) fn check_start_position_shape(project: &Project, out: &mut Vec<LintIssue>) {
    let flagged = project
        .mapinfo_overrides
        .get("featureplacer.quoted_rot")
        .map(|v| matches!(v, toml::Value::Boolean(true)))
        .unwrap_or(false);
    if flagged {
        out.push(issue(
            LintRule::StartPositionShapeWrong,
            "Featureplacer rotations are quoted strings (PyMapConv `-k` \
             format leaked in via import). The `FP_featureplacer.lua` \
             gadget calls `Spring.CreateFeature(..., fDef.rot)` which \
             expects an unquoted integer. Re-emit via the editor to fix."
                .to_string(),
            Some("featureplacer.rot"),
            None,
        ));
    }
}

/// PITFALL §25. Every multiplayer map with a `LuaGaia/Gadgets/`
/// gadget needs the bootstrap pair. The pipeline always stages it;
/// this rule guards a future import path where the user
/// deliberately opted out (e.g. for diagnostics) via
/// `mapinfo_overrides["build.luagaia_bootstrap"] = false`.
pub(super) fn check_lua_gaia_team_missing(project: &Project, out: &mut Vec<LintIssue>) {
    let multi_player = project.ally_groups.len() >= 2;
    if !multi_player {
        return;
    }
    let opted_out = project
        .mapinfo_overrides
        .get("build.luagaia_bootstrap")
        .map(|v| matches!(v, toml::Value::Boolean(false)))
        .unwrap_or(false);
    if opted_out {
        out.push(issue(
            LintRule::LuaGaiaTeamMissing,
            "Project has ≥2 ally groups (multiplayer) and \
             `build.luagaia_bootstrap = false`. Without \
             `LuaGaia/main.lua` + `LuaGaia/draw.lua`, the engine never \
             scans `LuaGaia/Gadgets/` and the Springboard featureplacer \
             never runs — geo vents and features fail to spawn."
                .to_string(),
            None,
            None,
        ));
    }
}

/// PITFALL §22. `mapinfo.maxMetal` outside `0.5..=5.0` scales every
/// metal spot's F4-displayed income atypically vs published BAR
/// maps (jade_empress 0.99 — starwatcher 4.11).
pub(super) fn check_metal_value_range(info: &MapInfo, out: &mut Vec<LintIssue>) {
    let Some(mm) = info.max_metal else {
        return;
    };
    if !(0.5..=5.0).contains(&mm) {
        out.push(issue(
            LintRule::MetalValueOutOfBARRange,
            format!(
                "`maxMetal = {mm}` is outside BAR's 0.5..=5.0 range. Every \
                 spot's F4 income scales linearly — values too low make \
                 mexes display ~0.1 m/s; values too high stack hot. \
                 Published BAR maps cluster 0.93..=4.11."
            ),
            Some("maxMetal"),
            Some(LintFix::MapInfoPatch(MapInfoPatch::MaxMetal(Some(1.0)))),
        ));
    }
}

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

    // ─────── Warning rules ───────

    // ─── LightingSunDirMissing ───

    #[test]
    fn lighting_sun_dir_fires_on_zero_xyz() {
        let p = fixture_project();
        let mut info: MapInfo = (&p).into();
        info.lighting.sun_dir = [0.0, 0.0, 0.0, 1.0];
        let issues = super::super::lint_with(&p, &info, &StockManifest::default());
        assert!(fired(&issues, LintRule::LightingSunDirMissing));
    }

    #[test]
    fn lighting_sun_dir_silent_with_non_zero_direction() {
        let p = fixture_project();
        assert!(!fired(&lint(&p), LintRule::LightingSunDirMissing));
    }

    // ─── AtmosphereSkyDirPresent ───

    #[test]
    fn atmosphere_sky_dir_fires_when_override_present() {
        let mut p = fixture_project();
        p.mapinfo_overrides.insert(
            "atmosphere.skyDir".into(),
            toml::Value::String("legacy".into()),
        );
        assert!(fired(&lint(&p), LintRule::AtmosphereSkyDirPresent));
    }

    #[test]
    fn atmosphere_sky_dir_silent_without_override() {
        let p = fixture_project();
        assert!(!fired(&lint(&p), LintRule::AtmosphereSkyDirPresent));
    }

    // ─── GuiMinimapRotationPresent ───

    #[test]
    fn gui_minimap_rotation_fires_when_override_present() {
        let mut p = fixture_project();
        p.mapinfo_overrides
            .insert("gui.minimapRotation".into(), toml::Value::Float(1.57));
        assert!(fired(&lint(&p), LintRule::GuiMinimapRotationPresent));
    }

    #[test]
    fn gui_minimap_rotation_silent_without_override() {
        let p = fixture_project();
        assert!(!fired(&lint(&p), LintRule::GuiMinimapRotationPresent));
    }

    // ─── ExtractorRadiusFiveHundred ───

    #[test]
    fn extractor_radius_five_hundred_fires() {
        let mut p = fixture_project();
        p.extractor_radius = 500.0;
        assert!(fired(&lint(&p), LintRule::ExtractorRadiusFiveHundred));
    }

    #[test]
    fn extractor_radius_silent_at_bar_eighty() {
        let p = fixture_project();
        assert!(!fired(&lint(&p), LintRule::ExtractorRadiusFiveHundred));
    }

    // ─── TidalStrengthWithoutWaterSurfaceColor ───

    #[test]
    fn tidal_without_surface_color_fires() {
        let mut p = fixture_project();
        p.tidal_strength = Some(10.0);
        // water_mode stays None so info.water = None → no surface_color.
        assert!(fired(
            &lint(&p),
            LintRule::TidalStrengthWithoutWaterSurfaceColor
        ));
    }

    #[test]
    fn tidal_with_surface_color_silent() {
        let mut p = fixture_project();
        p.tidal_strength = Some(10.0);
        p.water_mode = WaterMode::Ocean; // ships a surface_color from the preset
        assert!(!fired(
            &lint(&p),
            LintRule::TidalStrengthWithoutWaterSurfaceColor
        ));
    }

    #[test]
    fn tidal_zero_silent_even_without_water() {
        let p = fixture_project(); // tidal_strength = None
        assert!(!fired(
            &lint(&p),
            LintRule::TidalStrengthWithoutWaterSurfaceColor
        ));
    }

    // ─── TerrainBelowZeroWithoutWater ───

    #[test]
    fn terrain_below_zero_without_water_fires() {
        let mut p = fixture_project();
        p.min_height = -50.0;
        // water_mode stays None.
        assert!(fired(&lint(&p), LintRule::TerrainBelowZeroWithoutWater));
    }

    #[test]
    fn terrain_below_zero_with_water_silent() {
        let mut p = fixture_project();
        p.min_height = -50.0;
        p.water_mode = WaterMode::Ocean;
        assert!(!fired(&lint(&p), LintRule::TerrainBelowZeroWithoutWater));
    }

    // ─── WaterModeSetWithoutTerrainBelowZero ───

    #[test]
    fn water_mode_without_terrain_below_fires() {
        let mut p = fixture_project();
        p.water_mode = WaterMode::Ocean;
        // min_height stays 0.0
        assert!(fired(
            &lint(&p),
            LintRule::WaterModeSetWithoutTerrainBelowZero
        ));
    }

    #[test]
    fn water_mode_with_terrain_below_silent() {
        let mut p = fixture_project();
        p.water_mode = WaterMode::Ocean;
        p.min_height = -50.0;
        assert!(!fired(
            &lint(&p),
            LintRule::WaterModeSetWithoutTerrainBelowZero
        ));
    }

    // ─── TeamsLessThanSixteenOnLargeMap ───

    #[test]
    fn teams_less_than_sixteen_fires_with_four_groups_few_positions() {
        let mut p = fixture_project();
        // Add 3 more ally groups (total 4) with 2 positions each.
        for id in 1..4 {
            let mut g = AllyGroup::new(id);
            g.start_positions.push(StartPosition {
                x_elmo: id as i32 * 100,
                z_elmo: 100,
            });
            g.start_positions.push(StartPosition {
                x_elmo: id as i32 * 100,
                z_elmo: 200,
            });
            p.ally_groups.push(g);
        }
        // 4 groups × 2 positions + first group's 1 = 9 total → < 16.
        assert!(fired(&lint(&p), LintRule::TeamsLessThanSixteenOnLargeMap));
    }

    #[test]
    fn teams_less_than_sixteen_silent_with_two_groups() {
        let mut p = fixture_project();
        let mut g = AllyGroup::new(1);
        g.start_positions.push(StartPosition {
            x_elmo: 200,
            z_elmo: 200,
        });
        p.ally_groups.push(g);
        // Only 2 groups → rule doesn't fire even with few positions.
        assert!(!fired(&lint(&p), LintRule::TeamsLessThanSixteenOnLargeMap));
    }

    // ─── StartboxesLuaMissingWhenMultiTeam ───

    #[test]
    fn startboxes_missing_fires_with_three_groups_no_boxes() {
        let mut p = fixture_project();
        for id in 1..3 {
            let mut g = AllyGroup::new(id);
            g.start_positions.push(StartPosition {
                x_elmo: id as i32 * 200,
                z_elmo: 200,
            });
            p.ally_groups.push(g);
        }
        assert!(fired(
            &lint(&p),
            LintRule::StartboxesLuaMissingWhenMultiTeam
        ));
    }

    #[test]
    fn startboxes_missing_silent_when_any_group_has_polygon() {
        let mut p = fixture_project();
        for id in 1..3 {
            let mut g = AllyGroup::new(id);
            g.start_positions.push(StartPosition {
                x_elmo: id as i32 * 200,
                z_elmo: 200,
            });
            if id == 1 {
                g.box_polygon = Some(vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]);
            }
            p.ally_groups.push(g);
        }
        assert!(!fired(
            &lint(&p),
            LintRule::StartboxesLuaMissingWhenMultiTeam
        ));
    }

    #[test]
    fn startboxes_missing_silent_with_two_groups() {
        let mut p = fixture_project();
        let mut g = AllyGroup::new(1);
        g.start_positions.push(StartPosition {
            x_elmo: 200,
            z_elmo: 200,
        });
        p.ally_groups.push(g);
        // 2 groups → rule doesn't fire even with no boxes (1v1 fallback OK).
        assert!(!fired(
            &lint(&p),
            LintRule::StartboxesLuaMissingWhenMultiTeam
        ));
    }

    // ─── ResourcesDetailTexMissingOnDntsMap ───

    #[test]
    fn resources_detail_tex_missing_on_dnts_fires() {
        let p = fixture_project();
        let mut info: MapInfo = (&p).into();
        info.resources.splat_detail_normal_tex = vec!["a.dds".into()];
        info.resources.detail_tex = None;
        let issues = super::super::lint_with(&p, &info, &StockManifest::default());
        assert!(fired(&issues, LintRule::ResourcesDetailTexMissingOnDntsMap));
    }

    #[test]
    fn resources_detail_tex_silent_when_set() {
        let p = fixture_project();
        let mut info: MapInfo = (&p).into();
        info.resources.splat_detail_normal_tex = vec!["a.dds".into()];
        info.resources.detail_tex = Some("detail.dds".into());
        let issues = super::super::lint_with(&p, &info, &StockManifest::default());
        assert!(!fired(
            &issues,
            LintRule::ResourcesDetailTexMissingOnDntsMap
        ));
    }

    #[test]
    fn resources_detail_tex_silent_without_dnts() {
        let p = fixture_project();
        assert!(!fired(
            &lint(&p),
            LintRule::ResourcesDetailTexMissingOnDntsMap
        ));
    }

    // ─── GeoInMetalLayoutGeosArray ───

    #[test]
    fn geo_in_metal_layout_fires_when_override_present() {
        let mut p = fixture_project();
        p.mapinfo_overrides.insert(
            "map_metal_layout.geos.0.x".into(),
            toml::Value::Integer(4096),
        );
        assert!(fired(&lint(&p), LintRule::GeoInMetalLayoutGeosArray));
    }

    #[test]
    fn geo_in_metal_layout_silent_without_override() {
        let p = fixture_project();
        assert!(!fired(&lint(&p), LintRule::GeoInMetalLayoutGeosArray));
    }

    // ─── SmfMetalmapNonZeroWithLuaSpots ───

    #[test]
    fn metalmap_nonzero_fires_with_spots_and_custom_metalmap() {
        let mut p = fixture_project();
        p.metal_spots.push(barme_core::MetalSpot::new(100, 100));
        p.mapinfo_overrides.insert(
            "smf.metalmapTex".into(),
            toml::Value::String("maps/custom_metal.png".into()),
        );
        assert!(fired(&lint(&p), LintRule::SmfMetalmapNonZeroWithLuaSpots));
    }

    #[test]
    fn metalmap_nonzero_silent_without_custom_override() {
        let mut p = fixture_project();
        p.metal_spots.push(barme_core::MetalSpot::new(100, 100));
        assert!(!fired(&lint(&p), LintRule::SmfMetalmapNonZeroWithLuaSpots));
    }

    #[test]
    fn metalmap_nonzero_silent_without_spots() {
        let mut p = fixture_project();
        p.mapinfo_overrides.insert(
            "smf.metalmapTex".into(),
            toml::Value::String("maps/custom_metal.png".into()),
        );
        // No metal spots → rule doesn't fire (engine metalmap is the source).
        assert!(!fired(&lint(&p), LintRule::SmfMetalmapNonZeroWithLuaSpots));
    }

    // ─── SunDirWIsLarge ───

    #[test]
    fn sun_dir_w_large_fires_at_1e9() {
        let p = fixture_project();
        let mut info: MapInfo = (&p).into();
        info.lighting.sun_dir = [0.3, 1.0, -0.2, 1.0e9];
        let issues = super::super::lint_with(&p, &info, &StockManifest::default());
        assert!(fired(&issues, LintRule::SunDirWIsLarge));
    }

    #[test]
    fn sun_dir_w_small_silent() {
        let p = fixture_project();
        assert!(!fired(&lint(&p), LintRule::SunDirWIsLarge));
    }

    // ─── SplatDetailNormalTexLegacyForm ───

    #[test]
    fn splat_detail_normal_legacy_fires() {
        let p = fixture_project();
        let mut info: MapInfo = (&p).into();
        info.resources.splat_detail_normal_diffuse_alpha = Some(1);
        let issues = super::super::lint_with(&p, &info, &StockManifest::default());
        assert!(fired(&issues, LintRule::SplatDetailNormalTexLegacyForm));
    }

    #[test]
    fn splat_detail_normal_legacy_silent_without_field() {
        let p = fixture_project();
        assert!(!fired(&lint(&p), LintRule::SplatDetailNormalTexLegacyForm));
    }

    // ─── DntsOnMapWithMinHeightBelowZero ───
    //
    // Note: fires when `Project.layers` has any DNTS-bound layer
    // AND `min_height < 0`. Synthesizing DNTS-bound layers from
    // outside `barme-core` would require crossing the layer API
    // surface; covered indirectly by the silent path below and by
    // the existing `validation_summary` tests in `barme-app`.

    #[test]
    fn dnts_on_map_silent_with_no_layers_below_zero() {
        let mut p = fixture_project();
        p.min_height = -50.0;
        // No DNTS layers seeded → rule silent.
        assert!(!fired(&lint(&p), LintRule::DntsOnMapWithMinHeightBelowZero));
    }

    // ─── StartPositionShapeWrong ───

    #[test]
    fn start_position_shape_fires_when_override_flagged() {
        let mut p = fixture_project();
        p.mapinfo_overrides.insert(
            "featureplacer.quoted_rot".into(),
            toml::Value::Boolean(true),
        );
        assert!(fired(&lint(&p), LintRule::StartPositionShapeWrong));
    }

    #[test]
    fn start_position_shape_silent_by_default() {
        let p = fixture_project();
        assert!(!fired(&lint(&p), LintRule::StartPositionShapeWrong));
    }

    // ─── LuaGaiaTeamMissing ───

    #[test]
    fn lua_gaia_team_missing_fires_when_bootstrap_opted_out() {
        let mut p = fixture_project();
        // Second ally group → multiplayer.
        let mut g = AllyGroup::new(1);
        g.start_positions.push(StartPosition {
            x_elmo: 800,
            z_elmo: 800,
        });
        p.ally_groups.push(g);
        p.mapinfo_overrides.insert(
            "build.luagaia_bootstrap".into(),
            toml::Value::Boolean(false),
        );
        assert!(fired(&lint(&p), LintRule::LuaGaiaTeamMissing));
    }

    #[test]
    fn lua_gaia_team_silent_for_single_ally_group() {
        let mut p = fixture_project();
        p.mapinfo_overrides.insert(
            "build.luagaia_bootstrap".into(),
            toml::Value::Boolean(false),
        );
        // Only one ally group from fixture_project — not multiplayer.
        assert!(!fired(&lint(&p), LintRule::LuaGaiaTeamMissing));
    }

    #[test]
    fn lua_gaia_team_silent_when_bootstrap_present() {
        let mut p = fixture_project();
        let mut g = AllyGroup::new(1);
        g.start_positions.push(StartPosition {
            x_elmo: 800,
            z_elmo: 800,
        });
        p.ally_groups.push(g);
        // No opt-out override → bootstrap stages by default; lint silent.
        assert!(!fired(&lint(&p), LintRule::LuaGaiaTeamMissing));
    }

    // ─── MetalValueOutOfBARRange ───

    #[test]
    fn metal_value_below_range_fires() {
        let p = fixture_project();
        let mut info: MapInfo = (&p).into();
        info.max_metal = Some(0.02);
        let issues = super::super::lint_with(&p, &info, &StockManifest::default());
        assert!(fired(&issues, LintRule::MetalValueOutOfBARRange));
    }

    #[test]
    fn metal_value_above_range_fires() {
        let p = fixture_project();
        let mut info: MapInfo = (&p).into();
        info.max_metal = Some(10.0);
        let issues = super::super::lint_with(&p, &info, &StockManifest::default());
        assert!(fired(&issues, LintRule::MetalValueOutOfBARRange));
    }

    #[test]
    fn metal_value_in_bar_range_silent() {
        let p = fixture_project();
        // Default 1.0 — BAR median.
        assert!(!fired(&lint(&p), LintRule::MetalValueOutOfBARRange));
    }

    /// Aggregate warning-tier severity check: a wizard-style fixture
    /// produces ≤ 2 warnings (the Sprint 21 exit criterion).
    #[test]
    fn wizard_style_fixture_emits_at_most_two_warnings() {
        let p = fixture_project();
        let issues = lint(&p);
        let warnings: Vec<&LintIssue> = issues
            .iter()
            .filter(|i| i.severity == LintSeverity::Warning)
            .collect();
        assert!(
            warnings.len() <= 2,
            "wizard-style fixture produced {} warnings (cap is 2): {:#?}",
            warnings.len(),
            warnings
        );
    }
}
