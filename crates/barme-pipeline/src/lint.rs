//! C8 / Sprint 21 — "Lint My Map" rule registry.
//!
//! Walks a [`Project`] and emits [`LintIssue`]s for every silent
//! failure mode catalogued in `docs/PITFALLS.md` §1–§28 plus the
//! source-audit additions (`docs/research/source-audit-2026-05-18/
//! FINDINGS.md` §12, NEW-1..NEW-10). One [`LintRule`] variant per
//! pitfall; the rule registry is the codebase's single source of
//! truth for "what does the editor consider a broken build."
//!
//! ## Severity tiers
//!
//! - [`LintSeverity::Error`]: blocks the Build button. Examples:
//!   `modtype != 3`, missing `smtFileName0`, `fogStart == fogEnd`,
//!   heightmap dims not `64·N+1`.
//! - [`LintSeverity::Warning`]: surfaces in the panel + chip tone but
//!   doesn't block. The user might intentionally ship a map that
//!   trips the rule (e.g. a `voidWater` map with explicit
//!   `forceRendering`).
//! - [`LintSeverity::Info`]: convention notes ("BAR uses gravity 130,
//!   you have 95 — fine but unusual"). Surfaces in the panel only.
//!
//! ## Per-rule contract
//!
//! Every [`LintRule`] variant is a function `fn(&Project, &MapInfo,
//! &StockManifest, &mut Vec<LintIssue>)`. The dispatcher in [`lint`]
//! walks every rule in [`LintRule::ALL`] order. Rules that don't fire
//! push nothing; the panel surfaces "passing" rules by checking
//! `LintRule::ALL` against the emitted set.
//!
//! ## Fix actions
//!
//! When a rule has a one-shot remediation, the issue carries a
//! [`LintFix`]. For Sprint 21 every fix is a [`MapInfoPatch`] —
//! reuses the F9 form's undo path (`ProjectDiff::EditMapInfo`) so
//! a Fix click is undoable. Compound fixes ship in later sprints if
//! demand surfaces.
//!
//! ## Performance
//!
//! Rules walk in-memory project state — no I/O. The pipe runs every
//! frame against `App::lint_summary` (per Sprint 20's prequel "rules
//! should evaluate on every frame against project state — cheap; no
//! debounce needed for Sprint 21"). On a 32-SMU map with 500 features
//! the walk is sub-millisecond; threading is unneeded.

use barme_core::{MapInfo, MapInfoPatch, Project};
use tracing::{info, trace};

/// Embedded stock-feature catalogue — used by
/// [`LintRule::FeatureNotInStockManifest`] to validate
/// `Project.features[].name` against BAR's `mapfeatures` registry.
/// Source: `assets/mapfeatures_catalog.json` (Sprint 12 / C6).
const MAPFEATURES_CATALOG_JSON: &str = include_str!("../../../assets/mapfeatures_catalog.json");

/// Severity of a [`LintIssue`].
///
/// `Error` gates the Build button (Sprint 20's `build_runner` refuses
/// to start when `lint_summary` has any `Error`); `Warning` and
/// `Info` surface in the panel + chip but don't block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LintSeverity {
    Error,
    Warning,
    Info,
}

/// Catalogue of every rule the linter knows about. Adding a variant
/// here triggers the exhaustiveness checker in [`LintRule::name`] /
/// [`LintRule::title`] / [`LintRule::default_severity`] /
/// [`LintRule::pitfall_anchor`] / [`LintRule::ALL`] so a new rule
/// can't ship without metadata + registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LintRule {
    // ─── Hard errors (block Build) ───
    /// PITFALL §21. `modtype` must equal 3 (map) for Chobby's
    /// map-browser filter to surface the map.
    ModtypeNotThree,
    /// PITFALL §6 / SRS §1.3. Without `"Map Helper v1"` in `depend`,
    /// the engine falls back to its untextured render.
    DependMissingMapHelper,
    /// PITFALL §6 / "pink map". `smf.smtFileName0` must match the
    /// actual `.smt` filename inside the `.sd7`.
    SmtFileNameZeroMissing,
    /// PITFALL §6. `name`, `mapfile`, and `version` are archive-scanner
    /// requirements — empty values silently break archive indexing.
    NameOrMapfileOrVersionMissing,
    /// PITFALL §6 / FINDINGS §1.5. `voidWater = true` requires
    /// omitting `water.planeColor`; setting both defeats voidWater
    /// silently. The emitter auto-resolves but the lint flags it
    /// upstream so the user sees the conflict.
    VoidWaterWithPlaneColor,
    /// PITFALL §6 / SRS §1.3. Engine requires at least one
    /// `teams[*].startPos`. Empty `ally_groups` triggers the emitter's
    /// 25/75 fallback, but the lint flags the underlying state so the
    /// user knows what's about to ship.
    TeamsEmpty,
    /// PITFALL §6 (Stage 2). A feature name not in
    /// `assets/mapfeatures_catalog.json` is silently dropped by the
    /// engine's `[GetFeatureDef] could not find FeatureDef` codepath.
    FeatureNotInStockManifest,
    /// PITFALL §6 + §17 reworded per FINDINGS §7.2. DNTS still renders
    /// without `specularTex`, but the result looks noticeably flatter
    /// than published BAR maps. The build pipeline ships a grey-BC1
    /// fallback in D6 (Sprint 12); this lint catches the upstream
    /// case where the user actively unset both.
    SplatDetailNormalTexWithoutSpecular,
    /// PITFALL §6. `fogStart == fogEnd` breaks the build-ETA grid
    /// renderer.
    FogStartEqualsFogEnd,
    /// PITFALL §4. Heightmap dims must be `(64·N + 1)²`. The wizard
    /// prevents this from happening; the lint guards against external
    /// import (F13 / Stage 2).
    HeightmapDimsWrong,

    // ─── Warnings (surface but don't block) ───
    /// PITFALL §11. `lighting.sundir` (lowercase) AND `lighting.sunDir`
    /// (camelCase) must BOTH be emitted — engine reads camelCase,
    /// BAR's `unit_sunfacing.lua` reads lowercase. The emitter writes
    /// both; this lint guards against `lighting` going missing
    /// upstream.
    LightingSunDirMissing,
    /// PITFALL §12 / FINDINGS §1.3. Engine logs `L_DEPRECATED` if
    /// `atmosphere.skyDir` is set; `atmosphere.skyAxisAngle` is the
    /// modern key.
    AtmosphereSkyDirPresent,
    /// PITFALL §19 / FINDINGS §1.11. `gui.minimapRotation` is unused
    /// by current Recoil; emitting it is dead data. The emitter
    /// doesn't ship a `gui` subtable; this lint catches imports that
    /// carry one.
    GuiMinimapRotationPresent,
    /// PITFALL §6. Engine default `extractorRadius = 500` breaks
    /// BAR's mex-snap; BAR uses 80. Fires when a user explicitly sets
    /// it to 500.
    ExtractorRadiusFiveHundred,
    /// PITFALL §6. `tidalStrength > 0` without a `water.surfaceColor`
    /// means tidal generators visually clip into invisible water.
    TidalStrengthWithoutWaterSurfaceColor,
    /// PITFALL §6. `min_height < 0` with no water preset = engine
    /// renders its default blue ocean (silent surprise). Migrated
    /// from `validation_summary`'s WARN tier (Sprint 14 / C9).
    TerrainBelowZeroWithoutWater,
    /// Sprint 14 / C9 inverse. `water_mode != None` with `min_height
    /// >= 0` = no water visible without `forceRendering`. Migrated
    /// from `validation_summary`.
    WaterModeSetWithoutTerrainBelowZero,
    /// PITFALL §6. Maps with `≥4` ally groups should expose ≥16
    /// teams to match BAR's lobby expectations on large maps.
    TeamsLessThanSixteenOnLargeMap,
    /// PITFALL §6. Multi-ally-group projects (more than 2 groups)
    /// without any `box_polygon` set emit no `map_startboxes.lua`,
    /// which falls back to BAR's auto-N/S split — fine for 1v1 but
    /// jarring for FFA / 3+ team maps.
    StartboxesLuaMissingWhenMultiTeam,
    /// PITFALL §6. DNTS layers active without `resources.detailTex`
    /// — the base detail texture — render visibly flat.
    ResourcesDetailTexMissingOnDntsMap,
    /// PITFALL §14 / FINDINGS §5. Some imported projects (Zero-K
    /// convention) carry a `geos = {...}` array in
    /// `map_metal_layout.lua`. BAR ignores it; geo vents must reach
    /// BAR via the Springboard featureplacer trio. The editor never
    /// emits this, but the lint guards against user-imported state.
    GeoInMetalLayoutGeosArray,
    /// PITFALL §13 / FINDINGS §5. `map_metal_spot_placer.lua` bails
    /// if any SMF metalmap pixel is non-zero. The pipeline ships an
    /// all-zero metalmap PNG when `metal_spots` is non-empty;
    /// non-zero pixels on disk would silently disable BAR's Lua-spot
    /// path. Lint can only flag the upstream cause (user-overridden
    /// metalmap path); the pipeline itself enforces zero bytes.
    SmfMetalmapNonZeroWithLuaSpots,
    /// PITFALL §18 / FINDINGS §1.4 / NEW-6. The `w` component of
    /// `lighting.sun_dir` is an intensity scalar with engine default
    /// `1.0`. Older research used `1e9` (sunStartDistance leakage)
    /// which over-saturates sunlight on load. Lint warns when w > 100.
    SunDirWIsLarge,
    /// PITFALL §15 / FINDINGS §1.8. The legacy
    /// `splatDetailNormalTex1..4` numbered keys are shadowed by the
    /// subtable form. The emitter writes only the subtable; the lint
    /// catches imports that leak the legacy form.
    SplatDetailNormalTexLegacyForm,
    /// PITFALL §8. DNTS layers + `min_height < 0` triggers the
    /// LOS animated-snow bug (Beherith forum t=35202). Migrated from
    /// `validation_summary`'s WARN tier.
    DntsOnMapWithMinHeightBelowZero,

    // ─── Audit additions (PITFALLS §22+ from Sprint 11 / 14 hotfixes) ───
    /// PITFALL §23. Springboard featureplacer rotation is an unquoted
    /// integer; the emitter writes that form. The lint guards against
    /// imported state with quoted-string rotations (PyMapConv `-k`
    /// format leaked into a map's `set.lua`).
    StartPositionShapeWrong,
    /// PITFALL §25. Every map shipping a `LuaGaia/Gadgets/` gadget
    /// (Springboard featureplacer included) needs `LuaGaia/main.lua`
    /// and `LuaGaia/draw.lua` to bootstrap the gadget handler. The
    /// pipeline stages both files; the lint surfaces when the
    /// underlying state implies the gadget will run but the bootstrap
    /// would be skipped.
    LuaGaiaTeamMissing,
    /// PITFALL §22. `mapinfo.maxMetal` outside `0.5..=5.0` means
    /// every metal spot's displayed F4 income is scaled inconsistently
    /// vs published BAR maps. Lint fires per the PITFALL §22 range.
    MetalValueOutOfBARRange,

    // ─── Info-tier (convention notes) ───
    /// BAR convention is gravity 130. Outside `100..=160` is unusual
    /// but valid (light-gravity / heavy-gravity maps exist).
    GravityNotOneThirty,
    /// PITFALL §6 — BAR uses extractorRadius = 80. Values that drift
    /// away from 80 (but aren't the broken 500) are valid for unusual
    /// maps; info-tier so the user notices without blocking.
    ExtractorRadiusDriftFromEighty,
    /// PITFALL §20. When `voidGround = true` the user may want to
    /// tune `voidAlphaMin` (default 0.9). Info-tier prompt to surface
    /// the knob.
    VoidGroundWithoutVoidAlphaMinTuning,
}

impl LintRule {
    /// Every variant in registration order. The lint dispatcher walks
    /// this slice; the UI panel uses it to enumerate "passing" rules.
    pub const ALL: &'static [LintRule] = &[
        // Hard errors
        LintRule::ModtypeNotThree,
        LintRule::DependMissingMapHelper,
        LintRule::SmtFileNameZeroMissing,
        LintRule::NameOrMapfileOrVersionMissing,
        LintRule::VoidWaterWithPlaneColor,
        LintRule::TeamsEmpty,
        LintRule::FeatureNotInStockManifest,
        LintRule::SplatDetailNormalTexWithoutSpecular,
        LintRule::FogStartEqualsFogEnd,
        LintRule::HeightmapDimsWrong,
        // Warnings
        LintRule::LightingSunDirMissing,
        LintRule::AtmosphereSkyDirPresent,
        LintRule::GuiMinimapRotationPresent,
        LintRule::ExtractorRadiusFiveHundred,
        LintRule::TidalStrengthWithoutWaterSurfaceColor,
        LintRule::TerrainBelowZeroWithoutWater,
        LintRule::WaterModeSetWithoutTerrainBelowZero,
        LintRule::TeamsLessThanSixteenOnLargeMap,
        LintRule::StartboxesLuaMissingWhenMultiTeam,
        LintRule::ResourcesDetailTexMissingOnDntsMap,
        LintRule::GeoInMetalLayoutGeosArray,
        LintRule::SmfMetalmapNonZeroWithLuaSpots,
        LintRule::SunDirWIsLarge,
        LintRule::SplatDetailNormalTexLegacyForm,
        LintRule::DntsOnMapWithMinHeightBelowZero,
        LintRule::StartPositionShapeWrong,
        LintRule::LuaGaiaTeamMissing,
        LintRule::MetalValueOutOfBARRange,
        // Info
        LintRule::GravityNotOneThirty,
        LintRule::ExtractorRadiusDriftFromEighty,
        LintRule::VoidGroundWithoutVoidAlphaMinTuning,
    ];

    /// Short stable identifier — used by tests, the lint panel's row
    /// header, and any future "open PITFALLS.md anchor" affordance.
    pub fn name(self) -> &'static str {
        match self {
            Self::ModtypeNotThree => "modtype_not_three",
            Self::DependMissingMapHelper => "depend_missing_map_helper",
            Self::SmtFileNameZeroMissing => "smt_file_name_zero_missing",
            Self::NameOrMapfileOrVersionMissing => "name_or_mapfile_or_version_missing",
            Self::VoidWaterWithPlaneColor => "void_water_with_plane_color",
            Self::TeamsEmpty => "teams_empty",
            Self::FeatureNotInStockManifest => "feature_not_in_stock_manifest",
            Self::SplatDetailNormalTexWithoutSpecular => "splat_detail_normal_tex_without_specular",
            Self::FogStartEqualsFogEnd => "fog_start_equals_fog_end",
            Self::HeightmapDimsWrong => "heightmap_dims_wrong",
            Self::LightingSunDirMissing => "lighting_sun_dir_missing",
            Self::AtmosphereSkyDirPresent => "atmosphere_sky_dir_present",
            Self::GuiMinimapRotationPresent => "gui_minimap_rotation_present",
            Self::ExtractorRadiusFiveHundred => "extractor_radius_five_hundred",
            Self::TidalStrengthWithoutWaterSurfaceColor => {
                "tidal_strength_without_water_surface_color"
            }
            Self::TerrainBelowZeroWithoutWater => "terrain_below_zero_without_water",
            Self::WaterModeSetWithoutTerrainBelowZero => {
                "water_mode_set_without_terrain_below_zero"
            }
            Self::TeamsLessThanSixteenOnLargeMap => "teams_less_than_sixteen_on_large_map",
            Self::StartboxesLuaMissingWhenMultiTeam => "startboxes_lua_missing_when_multi_team",
            Self::ResourcesDetailTexMissingOnDntsMap => "resources_detail_tex_missing_on_dnts_map",
            Self::GeoInMetalLayoutGeosArray => "geo_in_metal_layout_geos_array",
            Self::SmfMetalmapNonZeroWithLuaSpots => "smf_metalmap_nonzero_with_lua_spots",
            Self::SunDirWIsLarge => "sun_dir_w_is_large",
            Self::SplatDetailNormalTexLegacyForm => "splat_detail_normal_tex_legacy_form",
            Self::DntsOnMapWithMinHeightBelowZero => "dnts_on_map_with_min_height_below_zero",
            Self::StartPositionShapeWrong => "start_position_shape_wrong",
            Self::LuaGaiaTeamMissing => "lua_gaia_team_missing",
            Self::MetalValueOutOfBARRange => "metal_value_out_of_bar_range",
            Self::GravityNotOneThirty => "gravity_not_one_thirty",
            Self::ExtractorRadiusDriftFromEighty => "extractor_radius_drift_from_eighty",
            Self::VoidGroundWithoutVoidAlphaMinTuning => {
                "void_ground_without_void_alpha_min_tuning"
            }
        }
    }

    /// Human-readable one-line summary surfaced in the panel header.
    /// Specific failures override this with a richer
    /// [`LintIssue::message`].
    pub fn title(self) -> &'static str {
        match self {
            Self::ModtypeNotThree => "modtype must equal 3 (map)",
            Self::DependMissingMapHelper => "depend must include \"Map Helper v1\"",
            Self::SmtFileNameZeroMissing => "smf.smtFileName0 is required",
            Self::NameOrMapfileOrVersionMissing => "name / mapfile / version must be set",
            Self::VoidWaterWithPlaneColor => "voidWater + water.planeColor are mutually exclusive",
            Self::TeamsEmpty => "no team start positions",
            Self::FeatureNotInStockManifest => "feature name unknown to BAR's mapfeatures",
            Self::SplatDetailNormalTexWithoutSpecular => "DNTS without a specular texture",
            Self::FogStartEqualsFogEnd => "fogStart must differ from fogEnd",
            Self::HeightmapDimsWrong => "heightmap dims must be (64·N + 1)²",
            Self::LightingSunDirMissing => "lighting subtable missing sunDir",
            Self::AtmosphereSkyDirPresent => "atmosphere.skyDir is deprecated",
            Self::GuiMinimapRotationPresent => "gui.minimapRotation is unused by Recoil",
            Self::ExtractorRadiusFiveHundred => {
                "extractorRadius = 500 breaks BAR mex-snap (use 80)"
            }
            Self::TidalStrengthWithoutWaterSurfaceColor => {
                "tidalStrength > 0 without a water.surfaceColor"
            }
            Self::TerrainBelowZeroWithoutWater => "terrain below Y=0 with no water preset",
            Self::WaterModeSetWithoutTerrainBelowZero => "water preset set, no terrain below Y=0",
            Self::TeamsLessThanSixteenOnLargeMap => "≥4 ally groups but <16 team start positions",
            Self::StartboxesLuaMissingWhenMultiTeam => {
                "multi-team project with no start-box polygons"
            }
            Self::ResourcesDetailTexMissingOnDntsMap => "DNTS active without resources.detailTex",
            Self::GeoInMetalLayoutGeosArray => "geo vents must not live in a geos[] array",
            Self::SmfMetalmapNonZeroWithLuaSpots => "SMF metalmap non-zero with Lua metal spots",
            Self::SunDirWIsLarge => "lighting.sunDir.w is large (intensity over-saturation)",
            Self::SplatDetailNormalTexLegacyForm => {
                "splatDetailNormalTex uses legacy numbered keys"
            }
            Self::DntsOnMapWithMinHeightBelowZero => "DNTS + water: LOS animated-snow bug",
            Self::StartPositionShapeWrong => "feature rot must be an unquoted integer",
            Self::LuaGaiaTeamMissing => "LuaGaia bootstrap pair missing",
            Self::MetalValueOutOfBARRange => "maxMetal outside BAR's 0.5..=5.0 range",
            Self::GravityNotOneThirty => "gravity drifts from BAR's 130",
            Self::ExtractorRadiusDriftFromEighty => "extractorRadius drifts from BAR's 80",
            Self::VoidGroundWithoutVoidAlphaMinTuning => {
                "voidGround active — voidAlphaMin may need tuning"
            }
        }
    }

    /// Default severity when the rule fires. Individual fires may
    /// downgrade (rare) but the default suffices for chip aggregation.
    pub fn default_severity(self) -> LintSeverity {
        match self {
            Self::ModtypeNotThree
            | Self::DependMissingMapHelper
            | Self::SmtFileNameZeroMissing
            | Self::NameOrMapfileOrVersionMissing
            | Self::VoidWaterWithPlaneColor
            | Self::TeamsEmpty
            | Self::FeatureNotInStockManifest
            | Self::SplatDetailNormalTexWithoutSpecular
            | Self::FogStartEqualsFogEnd
            | Self::HeightmapDimsWrong => LintSeverity::Error,
            Self::LightingSunDirMissing
            | Self::AtmosphereSkyDirPresent
            | Self::GuiMinimapRotationPresent
            | Self::ExtractorRadiusFiveHundred
            | Self::TidalStrengthWithoutWaterSurfaceColor
            | Self::TerrainBelowZeroWithoutWater
            | Self::WaterModeSetWithoutTerrainBelowZero
            | Self::TeamsLessThanSixteenOnLargeMap
            | Self::StartboxesLuaMissingWhenMultiTeam
            | Self::ResourcesDetailTexMissingOnDntsMap
            | Self::GeoInMetalLayoutGeosArray
            | Self::SmfMetalmapNonZeroWithLuaSpots
            | Self::SunDirWIsLarge
            | Self::SplatDetailNormalTexLegacyForm
            | Self::DntsOnMapWithMinHeightBelowZero
            | Self::StartPositionShapeWrong
            | Self::LuaGaiaTeamMissing
            | Self::MetalValueOutOfBARRange => LintSeverity::Warning,
            Self::GravityNotOneThirty
            | Self::ExtractorRadiusDriftFromEighty
            | Self::VoidGroundWithoutVoidAlphaMinTuning => LintSeverity::Info,
        }
    }

    /// PITFALLS.md anchor (Sprint 22 will wire this to an in-app help
    /// center). Returns the section identifier without the leading
    /// `#`; e.g. `"21"` for "21. modtype is a six-value enum."
    pub fn pitfall_anchor(self) -> &'static str {
        match self {
            Self::ModtypeNotThree => "21",
            Self::DependMissingMapHelper => "6",
            Self::SmtFileNameZeroMissing => "7",
            Self::NameOrMapfileOrVersionMissing => "6",
            Self::VoidWaterWithPlaneColor => "6",
            Self::TeamsEmpty => "6",
            Self::FeatureNotInStockManifest => "6",
            Self::SplatDetailNormalTexWithoutSpecular => "6",
            Self::FogStartEqualsFogEnd => "6",
            Self::HeightmapDimsWrong => "4",
            Self::LightingSunDirMissing => "11",
            Self::AtmosphereSkyDirPresent => "12",
            Self::GuiMinimapRotationPresent => "19",
            Self::ExtractorRadiusFiveHundred => "6",
            Self::TidalStrengthWithoutWaterSurfaceColor => "6",
            Self::TerrainBelowZeroWithoutWater => "6",
            Self::WaterModeSetWithoutTerrainBelowZero => "6",
            Self::TeamsLessThanSixteenOnLargeMap => "6",
            Self::StartboxesLuaMissingWhenMultiTeam => "26",
            Self::ResourcesDetailTexMissingOnDntsMap => "6",
            Self::GeoInMetalLayoutGeosArray => "14",
            Self::SmfMetalmapNonZeroWithLuaSpots => "13",
            Self::SunDirWIsLarge => "18",
            Self::SplatDetailNormalTexLegacyForm => "15",
            Self::DntsOnMapWithMinHeightBelowZero => "8",
            Self::StartPositionShapeWrong => "23",
            Self::LuaGaiaTeamMissing => "25",
            Self::MetalValueOutOfBARRange => "22",
            Self::GravityNotOneThirty => "6",
            Self::ExtractorRadiusDriftFromEighty => "6",
            Self::VoidGroundWithoutVoidAlphaMinTuning => "20",
        }
    }
}

/// A single fire of a [`LintRule`] against the project. The dispatch
/// site is the only place these are constructed — outside callers
/// observe them through [`lint`]'s return value.
#[derive(Debug, Clone, PartialEq)]
pub struct LintIssue {
    pub rule: LintRule,
    pub severity: LintSeverity,
    /// One-line failure message. Should be specific enough that the
    /// user knows what the offending value is without opening the
    /// panel — e.g. `"modtype = 0; must be 3"`, not just `"bad
    /// modtype"`.
    pub message: String,
    /// Dotted Lua path of the offending field, when applicable. Used
    /// by the F9 form's per-tab dots: `lighting.*` lights up the
    /// Lighting tab. `None` for rules that don't map to a single
    /// field (e.g. heightmap-dims).
    pub field_path: Option<String>,
    /// One-click fix, when available. Currently always a
    /// [`MapInfoPatch`] — surfaces in the App as a
    /// `ProjectDiff::EditMapInfo` undo entry.
    pub fix: Option<LintFix>,
}

/// A remediation the user can apply with one click. For Sprint 21
/// every variant is a [`MapInfoPatch`]; later sprints can add
/// compound variants if needed.
#[derive(Debug, Clone, PartialEq)]
pub enum LintFix {
    /// Set a single MapInfo field via the F9 form's patch enum.
    /// The App composes the `from` side from current state and
    /// dispatches a `ProjectDiff::EditMapInfo` undo entry.
    MapInfoPatch(MapInfoPatch),
}

/// Walk the entire rule registry and return every issue that fires
/// against the given project. Stable order — rules walk in
/// [`LintRule::ALL`] order so the panel rendering is deterministic.
///
/// **Cost:** sub-millisecond on a 32-SMU map with 500 features.
/// Caller may invoke every frame.
pub fn lint(project: &Project) -> Vec<LintIssue> {
    let info: MapInfo = project.into();
    let stock = StockManifest::load();
    lint_with(project, &info, &stock)
}

/// Variant of [`lint`] that takes a pre-materialised [`MapInfo`] —
/// useful when the caller already has one (e.g. the F9 form snapshot)
/// or when tests want to inject a synthetic mapinfo.
pub fn lint_with(project: &Project, info: &MapInfo, stock: &StockManifest) -> Vec<LintIssue> {
    let t0 = std::time::Instant::now();
    let mut out = Vec::new();
    rules::check_modtype(info, &mut out);
    rules::check_depend(info, &mut out);
    rules::check_smt_file_name(info, &mut out);
    rules::check_name_mapfile_version(info, &mut out);
    rules::check_void_water(project, info, &mut out);
    rules::check_teams_empty(project, &mut out);
    rules::check_features_in_manifest(project, stock, &mut out);
    rules::check_dnts_without_spec(project, info, &mut out);
    rules::check_fog_start_end(info, &mut out);
    rules::check_heightmap_dims(project, &mut out);

    rules::check_lighting_sun_dir_missing(info, &mut out);
    rules::check_atmosphere_sky_dir_present(project, &mut out);
    rules::check_gui_minimap_rotation(project, &mut out);
    rules::check_extractor_radius_five_hundred(project, info, &mut out);
    rules::check_tidal_without_surface(project, info, &mut out);
    rules::check_terrain_below_without_water(project, &mut out);
    rules::check_water_without_terrain_below(project, &mut out);
    rules::check_teams_less_than_sixteen(project, &mut out);
    rules::check_startboxes_missing(project, &mut out);
    rules::check_resources_detail_tex_dnts(project, info, &mut out);
    rules::check_geo_in_metal_layout(project, &mut out);
    rules::check_metalmap_nonzero(project, &mut out);
    rules::check_sun_dir_w_large(info, &mut out);
    rules::check_splat_detail_normal_legacy(info, &mut out);
    rules::check_dnts_below_zero(project, &mut out);
    rules::check_start_position_shape(project, &mut out);
    rules::check_lua_gaia_team_missing(project, &mut out);
    rules::check_metal_value_range(info, &mut out);

    rules::check_gravity_drift(info, &mut out);
    rules::check_extractor_radius_drift(project, info, &mut out);
    rules::check_void_ground_alpha_min(info, &mut out);

    let elapsed_us = t0.elapsed().as_micros();
    let errors = out
        .iter()
        .filter(|i| i.severity == LintSeverity::Error)
        .count();
    let warnings = out
        .iter()
        .filter(|i| i.severity == LintSeverity::Warning)
        .count();
    let infos = out
        .iter()
        .filter(|i| i.severity == LintSeverity::Info)
        .count();
    info!(
        target: "barme::lint",
        errors,
        warnings,
        infos,
        elapsed_us,
        "lint pass complete"
    );
    for issue in &out {
        trace!(
            target: "barme::lint",
            rule = issue.rule.name(),
            severity = ?issue.severity,
            field = issue.field_path.as_deref().unwrap_or(""),
            "lint rule fired: {}",
            issue.message
        );
    }
    out
}

/// Stock-feature manifest loaded from
/// `assets/mapfeatures_catalog.json`. Used by
/// [`LintRule::FeatureNotInStockManifest`].
///
/// Loaded once at startup (the JSON is `include_str!`d at compile
/// time, so loading is a JSON parse, not file I/O). The `Default`
/// impl returns an empty manifest — used by tests that don't care
/// about feature names.
#[derive(Debug, Clone, Default)]
pub struct StockManifest {
    names: std::collections::HashSet<String>,
}

impl StockManifest {
    /// Parse the embedded catalogue. Fails closed: a parse error
    /// produces an empty manifest (the feature-not-in-manifest lint
    /// won't fire — better than gating builds on a malformed JSON).
    pub fn load() -> Self {
        match serde_json::from_str::<CatalogJson>(MAPFEATURES_CATALOG_JSON) {
            Ok(c) => {
                let names = c
                    .categories
                    .into_values()
                    .flatten()
                    .map(|e| e.name)
                    .collect();
                Self { names }
            }
            Err(e) => {
                tracing::warn!(
                    target: "barme::lint",
                    error = %e,
                    "mapfeatures_catalog.json parse failed; FeatureNotInStockManifest disabled"
                );
                Self::default()
            }
        }
    }

    /// Construct a manifest from an explicit name list — used by
    /// tests to inject synthetic stock sets without touching the
    /// embedded JSON.
    pub fn from_names<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            names: names.into_iter().map(Into::into).collect(),
        }
    }

    pub fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// `true` when the manifest holds zero names — used by the
    /// `FeatureNotInStockManifest` rule to skip emission when the
    /// stock catalogue failed to load.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

#[derive(serde::Deserialize)]
struct CatalogJson {
    categories: std::collections::HashMap<String, Vec<CatalogEntry>>,
}

#[derive(serde::Deserialize)]
struct CatalogEntry {
    name: String,
}

mod rules;

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke: the rule registry has the expected variant count
    /// (≥30 per Sprint 21 exit criteria) and every entry's
    /// `default_severity` matches the bucket comment.
    #[test]
    fn registry_has_at_least_thirty_rules() {
        assert!(
            LintRule::ALL.len() >= 30,
            "expected ≥ 30 rules; got {}",
            LintRule::ALL.len()
        );
    }

    /// Every rule's `name()` is unique.
    #[test]
    fn rule_names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for r in LintRule::ALL {
            assert!(seen.insert(r.name()), "duplicate rule name: {}", r.name());
        }
    }

    /// `LintRule::ALL` covers every variant. We can't introspect
    /// the enum directly, but counting unique names + sanity-checking
    /// against the variant count below catches "forgot to add to ALL".
    #[test]
    fn registry_all_array_is_complete() {
        // Bump this when adding a new rule to keep the array
        // honest. Fails when a variant exists but didn't make it
        // into ALL.
        let expected_count = 31;
        assert_eq!(
            LintRule::ALL.len(),
            expected_count,
            "LintRule::ALL count drifted from expected {}; \
             either bump the constant or wire a missing rule",
            expected_count
        );
    }

    /// The embedded catalogue loads at least one feature name; this
    /// guards against a malformed JSON breaking the lint pass
    /// silently.
    #[test]
    fn stock_manifest_loads_non_empty() {
        let m = StockManifest::load();
        // A few canonical names that exist in the Sprint 12 baseline.
        assert!(m.contains("geovent"), "stock manifest missing geovent");
        assert!(m.contains("pinetree"), "stock manifest missing pinetree");
    }

    /// A wizard-style fixture (one ally group with one start
    /// position; everything else default) lints clean for hard
    /// errors. Warnings are allowed (the project might trip a sane
    /// warning), but no `LintSeverity::Error` should fire.
    ///
    /// `Project::new` alone has no ally groups → `TeamsEmpty` would
    /// fire; the wizard seeds at least one. This test mirrors the
    /// wizard's post-state, matching the Sprint 21 prompt's smoke
    /// criterion "Default wizard project → 0 errors".
    #[test]
    fn wizard_style_fixture_emits_no_hard_errors() {
        use barme_core::{AllyGroup, StartPosition};
        let mut p = Project::new("smoke", 4);
        let mut g = AllyGroup::new(0);
        g.start_positions.push(StartPosition {
            x_elmo: 512,
            z_elmo: 512,
        });
        p.ally_groups.push(g);
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
}
