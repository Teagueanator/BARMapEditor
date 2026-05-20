//! Project root — the editable, on-disk representation of a map under construction.
//!
//! Persisted as `<name>.barmeproj` (TOML manifest) plus a sibling directory of
//! raw asset PNGs (heightmap, metal, type, splat distribution, diffuse). The
//! `.sd7` is build output, not source of truth.
//!
//! ## ADR-032 — F8 allyteam model
//!
//! Phase 3 replaces the flat `Vec<StartPosition>` (ADR-023) with a
//! two-level tree anchored by [`AllyGroup`]. Each group carries an id, a
//! name, an sRGB display colour, its source start positions, and an
//! optional `box_polygon` that emits into `mapconfig/map_startboxes.lua`
//! (C2 / ADR-029).
//!
//! Pre-Phase-3 `.barmeproj` files carry the flat `[[start_positions]]`
//! TOML array (with `team_id`). They load forward via `#[serde(from = …)]`
//! → the legacy vec materialises into `ally_groups[0]` with the default
//! colour and name. Tested by [`tests::legacy_flat_start_positions_load_into_ally_group_zero`].

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::warn;

use crate::layers::{LayerStack, SlotResolver};
use crate::mapinfo_schema::WaterBlock;
use crate::water_presets::WaterMode;
use crate::{MapSize, SplatConfig, SplatDistribution};

/// File extension for the project manifest (no leading dot).
pub const PROJECT_EXTENSION: &str = "barmeproj";

/// Default sRGB palette for fresh ally groups. The first four match
/// the BAR faction colours surfaced in lobby UIs (Armada blue, Cortex
/// red, Legion green, Raptors yellow). Indices ≥ 4 are fallback
/// distinct colours for unusual layouts (5+ ally FFA).
pub const ALLY_GROUP_PALETTE: [[u8; 3]; 8] = [
    [70, 130, 220], // Armada blue
    [220, 50, 50],  // Cortex red
    [60, 180, 80],  // Legion green
    [240, 200, 50], // Raptors yellow
    [80, 200, 200], // cyan
    [200, 80, 200], // magenta
    [230, 130, 50], // orange
    [150, 80, 220], // purple
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(from = "ProjectWire")]
pub struct Project {
    pub name: String,
    pub size: MapSize,
    pub min_height: f32,
    pub max_height: f32,
    /// Path to the heightmap PNG. Relative paths resolve against the project
    /// file's parent directory (see [`Project::resolve_heightmap`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heightmap: Option<PathBuf>,
    /// Per-side configuration tree (ADR-032). Empty in pre-Phase-3
    /// projects; the legacy `[[start_positions]]` array migrates into
    /// `ally_groups[0]` on load.
    ///
    /// The pipeline emits each group's `start_positions` into
    /// `mapinfo.teams[]` in id-order (concatenated). The `box_polygon`
    /// field — when set — emits into
    /// `mapconfig/map_startboxes.lua` keyed by `id`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ally_groups: Vec<AllyGroup>,
    /// User-authored `mapinfo.lua` field overrides (C1 / ADR-028).
    /// Populated by the F9 form editor (C7) on top of the
    /// `MapInfo::bar_default()` baseline so unusual maps (skybox
    /// changes, custom gadget `custom.*` blobs, etc.) can ship without
    /// schema bumps. Keys are dotted Lua paths
    /// (e.g. `"atmosphere.sky_box"`); values are TOML scalars / arrays
    /// the emitter knows how to render. Default empty.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub mapinfo_overrides: HashMap<String, toml::Value>,
    /// B8: when `true`, the wizard's "Next steps" hint window stays
    /// hidden for this project. Persisted **per-project** (NOT in the
    /// per-user `EditorConfig`) so reopening a fresh project re-shows
    /// the hint — the editor opens many projects per user and a
    /// per-user dismiss would suppress the hint forever after one
    /// click.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub next_steps_dismissed: bool,
    /// D10 / Sprint 17 (ADR-041): one-shot dismissal flag for the
    /// "your splat layers were migrated to the new Layers panel"
    /// toast that surfaces the first time a pre-Sprint-14 project
    /// loads through the Layers-panel-era editor. Default false;
    /// flipped to true the first time the user dismisses the toast
    /// + persisted so re-opens of the same project stay quiet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub migration_toast_dismissed: bool,
    /// D5 / Sprint 9: per-channel splat slot bindings, scales, mults,
    /// and the ADR-034 placeholder toggle. Round-trips through TOML;
    /// `#[serde(default)]` materialises the engine defaults for
    /// pre-Sprint-9 `.barmeproj` files.
    ///
    /// **Sprint 17 (ADR-041):** retired at the project boundary. The
    /// load path still hydrates this for pre-Sprint-14 projects so
    /// [`Self::after_load_migrate`] can seed [`Self::layers`] +
    /// [`Self::dnts_diffuse_in_alpha`] from it, but Commit 6 of
    /// Sprint 17 marks it `#[serde(skip_serializing)]` so new saves
    /// drop the legacy block.
    #[serde(default)]
    pub splat_config: SplatConfig,
    /// D10 / Sprint 17 (ADR-041): mirrors
    /// `mapinfo.resources.splatDetailNormalDiffuseAlpha`. Replaces the
    /// per-channel `SplatConfig.diffuse_in_alpha` flag (which lived in
    /// the legacy splat block) with a per-project setting that the
    /// Layers panel surfaces in its footer toggle. Migration in
    /// [`Self::after_load_migrate`] copies the legacy value across.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dnts_diffuse_in_alpha: bool,
    /// D8 / Sprint 15 (ADR-038): the Photoshop-style texture layer
    /// stack. Drives the `.sd7` diffuse bake. `#[serde(default)]` so
    /// pre-Sprint-15 projects load with an empty stack and then
    /// migrate via [`Project::after_load_migrate`]. The legacy
    /// `splat_config` field above stays as the source of truth for
    /// the runtime DNTS path until Sprint 17 retires it.
    #[serde(default, skip_serializing_if = "layer_stack_is_empty")]
    pub layers: LayerStack,
    /// D5 / Sprint 9: the painted RGBA distribution. Allocated on
    /// first stamp; `#[serde(skip)]` because 4 MB does not belong in
    /// a TOML manifest. D6 (Sprint 12) will ship the PNG sidecar
    /// persistence path; until then the distribution lives only for
    /// the current editor session.
    #[serde(skip)]
    pub splat_distribution: Option<SplatDistribution>,
    /// C4 / Sprint 11: F5 metal-spot sources. Mirrors are derived from
    /// the active `SymmetryAxis` per frame; only the source spots are
    /// stored. The pipeline expands sources through the active
    /// symmetry before emission (mirrors the F8 / ADR-032 rule).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metal_spots: Vec<MetalSpot>,
    /// C5 / Sprint 11: F6 geo-vent sources. Same symmetry rule as
    /// `metal_spots`. Emitted only as `geovent` features into
    /// `mapconfig/featureplacer/features.lua` (PITFALL §14 / FINDINGS
    /// §5 — there is no `geos = {}` table in BAR).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub geo_vents: Vec<GeoVent>,
    /// C6 / Sprint 12: F7 user-placed feature sources (trees / rocks /
    /// wreckage / props / geo vents authored through the picker UI).
    /// Symmetry mirrors derive at emission time the same way as
    /// `metal_spots` and `geo_vents`; only sources are stored.
    ///
    /// Emitted into `mapconfig/featureplacer/set.lua`'s `objectlist`
    /// next to the geovents Sprint 11 ships. PITFALL §21 / §23 capture
    /// the file shape — unquoted-integer `rot`, no `y`, gadget samples
    /// `GroundHeight(x, z) + 5` at spawn so the feature rides the live
    /// terrain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<FeatureInstance>,
    /// D6 / Sprint 12: optional override path for the project's
    /// specular texture. When `None`, the build pipeline ships a stock
    /// 1024² grey BC1 fallback at `maps/<projectname>_specular.dds`
    /// so the engine's DNTS branch doesn't render flat (FINDINGS §7.2
    /// — engine no longer gates DNTS on specularTex but the visual
    /// result is noticeably muddier without one). The path is resolved
    /// against the `.barmeproj` directory the same way `heightmap` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specular_tex_path: Option<PathBuf>,
    /// C4 / Sprint 11: BAR-convention extractor radius in elmos. Engine
    /// default is 500 but BAR overrides it to 80; setting it back to
    /// 500 silently breaks mex-snap (PITFALL §6). Surfaced to the F5
    /// inspector with a tooltip; the F9 form editor (Sprint 13) will
    /// also reach this through `mapinfo.extractor_radius`.
    #[serde(default = "default_extractor_radius")]
    pub extractor_radius: f32,
    /// C9 / Sprint 14 (ADR-042): active water preset. Drives
    /// `From<&Project> for MapInfo` — `None` emits no `water` sub-table,
    /// any other variant emits the preset's `WaterBlock` merged with
    /// [`Project::water_overrides`].
    ///
    /// On first load of a pre-Sprint-14 `.barmeproj` (`schema_v == 0`),
    /// the `From<ProjectWire>` migration sets `Ocean` when
    /// `min_height < 0`. Runs exactly once per project — re-saved
    /// files carry `schema_v == 1` and skip the rule.
    #[serde(default)]
    pub water_mode: WaterMode,
    /// C9 / Sprint 14 (ADR-042): sparse user overrides on top of the
    /// active preset. All fields `Option<…>` (`WaterBlock`'s shape)
    /// — `None` everywhere means "use the preset as-is." Switching
    /// presets does NOT clear this; the user's tweaks persist so a
    /// per-field `damage = 30` rides through Ocean → Acid → Magma.
    /// [`WaterMode::Custom`] uses an empty preset, letting overrides
    /// bleed through verbatim.
    #[serde(default, skip_serializing_if = "water_block_is_empty")]
    pub water_overrides: WaterBlock,
    /// C9 / Sprint 14 (ADR-042): top-level `mapinfo.voidWater` shadow.
    /// Emitted into `MapInfo.void_water` by `From<&Project>`.
    /// **Mutually exclusive with `water_overrides.plane_color`** (PITFALL §6):
    /// when `true`, the emission path forces `plane_color = None`
    /// and `warn!`s. The inspector co-locates this with the water
    /// fields for UX even though the schema field lives at MapInfo
    /// top level.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub void_water: bool,
    /// C9 / Sprint 14 (ADR-042): top-level `mapinfo.tidalStrength`
    /// shadow. `Some(0..=30)` enables BAR's tidal-energy economy.
    /// Lives at MapInfo top level (NOT inside `water = {}`); the
    /// inspector surfaces it under the Water section purely for UX.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tidal_strength: Option<f32>,
    /// C9 / Sprint 14 (ADR-042): one-click lava-atmosphere offer.
    /// `true` overrides `info.atmosphere` with the hard-coded
    /// `LAVA_ATMOSPHERE_PATCH` values (red-orange fog, dim warm
    /// sun) on top of the BAR default — common pairing for
    /// `WaterMode::Lava` / `Magma` maps. Independent of `water_mode`
    /// so the user can mix freely. The inspector surfaces the offer
    /// in the Preset section when Lava / Magma is active.
    ///
    /// Coarser than full per-field atmosphere overrides; Sprint 18's
    /// F9 form ships the granular surface. See ADR-042 / phase-3-plan
    /// C9's "lava-atmosphere link" slice.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lava_atmosphere: bool,
    /// C9 / Sprint 14: monotonic project-file schema version. Bumped
    /// every time a load-time migration runs, so subsequent loads of
    /// the same project skip the migration. Sprint 14 introduced
    /// `v = 1` — the water-mode migration. Pre-Sprint-14 files load
    /// as `v = 0`, run the migration, save back as `v = 1`.
    ///
    /// Future migrations append: bump the constant in
    /// [`Project::SCHEMA_V`] and add a step in `From<ProjectWire>`.
    #[serde(default)]
    pub schema_v: u32,
}

/// BAR-convention default for `Project.extractor_radius`. Engine
/// default is 500 but BAR overrides to 80 via the mod gadgets, and
/// every player's UI snaps mexes against this value.
pub fn default_extractor_radius() -> f32 {
    80.0
}

/// `WaterBlock` test used by `Project.water_overrides`'s
/// `skip_serializing_if`. Avoids writing an empty `[water_overrides]`
/// table for the 99 % of projects that haven't touched any water
/// field.
fn water_block_is_empty(b: &WaterBlock) -> bool {
    *b == WaterBlock::default()
}

/// `Project.layers` skip-serialize-if guard. Keeps the TOML quiet for
/// fresh-from-disk projects whose stack hasn't been seeded yet (e.g.
/// the `barme-pipeline` smoke example that builds a bare `Project`
/// directly).
fn layer_stack_is_empty(stack: &LayerStack) -> bool {
    stack.layers.is_empty()
}

/// One ally team's worth of spawn data (ADR-032).
///
/// `id` is the stable identifier used by emission + undo dispatch;
/// reordering ally groups changes their position in the flat `teams[]`
/// pool emitted to mapinfo.lua. Reorder consciously.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AllyGroup {
    /// Stable identifier. Used by `mapconfig/map_startboxes.lua` keys
    /// and by undo dispatch. Reusing an id from a deleted group is
    /// valid; the editor allocates the lowest unused u8.
    pub id: u8,
    /// Display name in the Inspector tree ("AllyGroup 0", "North",
    /// "Allies", …). Purely cosmetic — never emitted to mapinfo.lua.
    pub name: String,
    /// sRGB triple in `[0, 255]`. UI converts to / from `egui::Color32`.
    /// Persists across save / load and across tool switches.
    pub color: [u8; 3],
    /// Source start positions (mirror counterparts under symmetry are
    /// NOT stored; the F8 editor recomputes them every frame from
    /// `Project.symmetry`-class state on the app side).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub start_positions: Vec<StartPosition>,
    /// Per-ally start-box polygon in `[0, 1]` fractions of the map
    /// extent. `None` = no box, this group is omitted from
    /// `map_startboxes.lua`. C2's emitter walks this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub box_polygon: Option<Vec<(f32, f32)>>,
}

impl AllyGroup {
    /// Construct a fresh ally group with the palette colour for its
    /// id, empty positions, and no box polygon.
    pub fn new(id: u8) -> Self {
        Self {
            id,
            name: format!("AllyGroup {id}"),
            color: ALLY_GROUP_PALETTE[(id as usize) % ALLY_GROUP_PALETTE.len()],
            start_positions: Vec::new(),
            box_polygon: None,
        }
    }
}

/// A single metal-spot source in world coordinates (elmos).
///
/// `metal` follows BAR's convention: `2.0` is a standard mex, `4.0` a
/// strong central mex, `0.5` a perimeter mex. The
/// `map_metal_spot_placer.lua` gadget multiplies by `0.43 × 9 / 21 × 255`
/// at spawn time (FINDINGS §5); the user sees the BAR-facing scalar in
/// the inspector.
///
/// **Displayed F4 income** also scales linearly with the project's
/// `mapinfo.maxMetal` (see [`MapInfo::bar_default`] — `1.0` BAR median).
/// Setting `metal = 2.0` with `maxMetal = 1.0` gives ~2.0 m/s in F4;
/// the pre-2026-05-19 `maxMetal = 0.02` made the same spot read as
/// 0.1 m/s. See PITFALL §22.
///
/// Symmetry-derived mirrors are NOT stored — `Project.metal_spots` is
/// the source set and the active `SymmetryAxis` recomputes mirrors per
/// frame in the editor and per build in the pipeline (matches F8 /
/// ADR-032).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct MetalSpot {
    pub x_elmo: i32,
    pub z_elmo: i32,
    pub metal: f32,
}

impl MetalSpot {
    /// Default metal value for a fresh spot. Matches BAR's standard
    /// mex (`spot.metal = 2.0`); the strong central-mex convention is
    /// 4.0, set by the user.
    pub const DEFAULT_METAL: f32 = 2.0;

    pub fn new(x_elmo: i32, z_elmo: i32) -> Self {
        Self {
            x_elmo,
            z_elmo,
            metal: Self::DEFAULT_METAL,
        }
    }
}

/// A single geo-vent source in world coordinates (elmos). Geo vents
/// carry no `metal` or rotation field — the stock `geovent` FeatureDef
/// owns its own size and (engine-default-zero) facing. Symmetry rules
/// match `MetalSpot`: sources stored, mirrors derived per frame.
///
/// **Emission path:** geo vents reach BAR through the Springboard
/// featureplacer trio — a vendored `LuaGaia/Gadgets/FP_featureplacer.lua`
/// (PD-licensed Gnome / Smoth gadget, 2008) + a
/// `mapconfig/featureplacer/config.lua` redirect + a
/// `mapconfig/featureplacer/set.lua` data file with `objectlist =
/// { { name = "geovent", x, z, rot = 0 }, ... }`. See
/// `barme-pipeline::featureplacer` + PITFALL §21.
///
/// **Y-coordinate is intentionally absent.** The FP_featureplacer
/// gadget calls `Spring.CreateFeature(name, x, GroundHeight(x, z) + 5,
/// z, rot)` — the vent rides the live terrain. Sculpting the heightmap
/// after authoring vents does not detach the plume; it re-snaps on
/// the next map load.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct GeoVent {
    pub x_elmo: i32,
    pub z_elmo: i32,
}

impl GeoVent {
    pub fn new(x_elmo: i32, z_elmo: i32) -> Self {
        Self { x_elmo, z_elmo }
    }
}

/// A single user-placed feature source (C6 / Sprint 12). `name` matches
/// a stock `FeatureDef` from BAR's `mapfeatures` repo (pinned in
/// `assets/mapfeatures_catalog.json`); unknown names emit a `warn!` at
/// build time and trigger an in-engine
/// `[GetFeatureDef] could not find FeatureDef` skip — caught by C8 lint
/// (Sprint 14) rather than gated here.
///
/// Coordinates are elmos (engine world units). `rot_heading` is Spring's
/// 16-bit heading: `0` = north, `16384` = east, `32768` = south,
/// `49152` = west — full turn at 65536. Emission casts to a signed
/// 16-bit integer (`as i16`) so wrap-around preserves the bit pattern
/// that `Spring.CreateFeature(..., fDef.rot)` consumes. Per PITFALL §23
/// the Lua value is an UNQUOTED integer (the gadget's numeric arg —
/// distinct from PyMapConv's `-k` text-file format, which uses quoted
/// strings).
///
/// **`y` is intentionally absent.** Same rationale as [`GeoVent`] — the
/// FP_featureplacer gadget calls
/// `Spring.CreateFeature(name, x, GroundHeight(x, z) + 5, z, rot)` at
/// spawn, so the feature rides the live terrain and sculpting the
/// heightmap after authoring does not detach it.
///
/// **Symmetry mirrors are NOT stored.** `Project.features` is the
/// source set; the build pipeline expands sources through the active
/// `SymmetryAxis` before emission, matching the F8 / metal-spot /
/// geo-vent convention.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeatureInstance {
    /// `FeatureDef.name` from `mapfeatures` (e.g. `"pinetree"`,
    /// `"agorm_talltree6"`, `"geovent"`).
    pub name: String,
    pub x_elmo: i32,
    pub z_elmo: i32,
    /// Spring heading 0..65535. UI surfaces as degrees via
    /// `rot * 360.0 / 65536.0`.
    pub rot_heading: u16,
}

impl FeatureInstance {
    pub fn new(name: impl Into<String>, x_elmo: i32, z_elmo: i32, rot_heading: u16) -> Self {
        Self {
            name: name.into(),
            x_elmo,
            z_elmo,
            rot_heading,
        }
    }
}

/// A single team start position in world coordinates (elmos).
///
/// **ADR-032 (B6):** `team_id` was removed. Position identity is now
/// `(ally_group_id, index_within_group)` for app-level operations
/// (undo, drag, delete), and the flat `teams[]` index is computed at
/// emission time by walking ally groups in id order.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartPosition {
    pub x_elmo: i32,
    pub z_elmo: i32,
}

// ─────── Legacy wire format (pre-Phase-3 .barmeproj migration) ───────

/// Pre-ADR-032 flat start position. Carried `team_id`; the field is
/// dropped on migration (team ids are now computed positionally).
/// `_team_id` is read by serde but intentionally ignored — kept so a
/// legacy file with `team_id` doesn't fail deserialization.
#[derive(Debug, Deserialize)]
struct LegacyStartPosition {
    #[serde(default, rename = "team_id")]
    _team_id: serde::de::IgnoredAny,
    x_elmo: i64,
    z_elmo: i64,
}

/// Wire-format projection for [`Project`] deserialize. Accepts BOTH
/// the new `[[ally_groups]]` shape AND the legacy `[[start_positions]]`
/// flat vec. Migration runs in `From<ProjectWire>`.
#[derive(Debug, Deserialize)]
struct ProjectWire {
    name: String,
    size: MapSize,
    min_height: f32,
    max_height: f32,
    #[serde(default)]
    heightmap: Option<PathBuf>,
    #[serde(default)]
    ally_groups: Vec<AllyGroup>,
    /// Legacy: pre-ADR-032 flat vec. Materialised into `ally_groups[0]`
    /// if `ally_groups` is empty. Ignored otherwise (corrupt file —
    /// `warn!`).
    #[serde(default)]
    start_positions: Vec<LegacyStartPosition>,
    #[serde(default)]
    mapinfo_overrides: HashMap<String, toml::Value>,
    #[serde(default)]
    next_steps_dismissed: bool,
    #[serde(default)]
    migration_toast_dismissed: bool,
    #[serde(default)]
    splat_config: SplatConfig,
    #[serde(default)]
    dnts_diffuse_in_alpha: bool,
    #[serde(default)]
    layers: LayerStack,
    #[serde(default)]
    metal_spots: Vec<MetalSpot>,
    #[serde(default)]
    geo_vents: Vec<GeoVent>,
    #[serde(default)]
    features: Vec<FeatureInstance>,
    #[serde(default)]
    specular_tex_path: Option<PathBuf>,
    #[serde(default = "default_extractor_radius")]
    extractor_radius: f32,
    #[serde(default)]
    water_mode: WaterMode,
    #[serde(default)]
    water_overrides: WaterBlock,
    #[serde(default)]
    void_water: bool,
    #[serde(default)]
    tidal_strength: Option<f32>,
    #[serde(default)]
    lava_atmosphere: bool,
    #[serde(default)]
    schema_v: u32,
}

impl From<ProjectWire> for Project {
    fn from(w: ProjectWire) -> Self {
        let mut p = Project {
            name: w.name,
            size: w.size,
            min_height: w.min_height,
            max_height: w.max_height,
            heightmap: w.heightmap,
            ally_groups: w.ally_groups,
            mapinfo_overrides: w.mapinfo_overrides,
            next_steps_dismissed: w.next_steps_dismissed,
            migration_toast_dismissed: w.migration_toast_dismissed,
            splat_config: w.splat_config,
            dnts_diffuse_in_alpha: w.dnts_diffuse_in_alpha,
            splat_distribution: None,
            layers: w.layers,
            metal_spots: w.metal_spots,
            geo_vents: w.geo_vents,
            features: w.features,
            specular_tex_path: w.specular_tex_path,
            extractor_radius: w.extractor_radius,
            water_mode: w.water_mode,
            water_overrides: w.water_overrides,
            void_water: w.void_water,
            tidal_strength: w.tidal_strength,
            lava_atmosphere: w.lava_atmosphere,
            schema_v: w.schema_v,
        };
        Project::run_migrations(&mut p);
        if !w.start_positions.is_empty() {
            if p.ally_groups.is_empty() {
                let positions: Vec<StartPosition> = w
                    .start_positions
                    .into_iter()
                    .map(|l| StartPosition {
                        x_elmo: l.x_elmo as i32,
                        z_elmo: l.z_elmo as i32,
                    })
                    .collect();
                let group = AllyGroup {
                    id: 0,
                    name: "AllyGroup 0".to_string(),
                    color: ALLY_GROUP_PALETTE[0],
                    start_positions: positions,
                    box_polygon: None,
                };
                p.ally_groups.push(group);
            } else {
                warn!(
                    legacy_count = w.start_positions.len(),
                    new_count = p.ally_groups.len(),
                    "legacy start_positions present alongside ally_groups in .barmeproj; ignoring legacy"
                );
            }
        }
        p
    }
}

#[derive(Debug, Error)]
pub enum ProjectLoadError {
    #[error("read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
}

#[derive(Debug, Error)]
pub enum ProjectSaveError {
    #[error("serialize: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Sanitize a user-entered project name into a form safe for the
/// downstream pipeline (filenames, `mapinfo.lua` strings, `.sd7` archive
/// names). Allowed characters: ASCII alphanumeric, `_`, `-`. Anything
/// else is collapsed into a single `_`. Leading/trailing `_` are trimmed.
/// Empty input maps to `"untitled"`.
///
/// The pipeline already escapes `"` and `\` in Lua emit (`lua_string`),
/// but a name like `"my map: 1.0"` would otherwise produce an `.sd7` with
/// a colon in its filename and trigger PITFALL #7 (pink-map on rename)
/// in subtle ways. Sanitizing at the project boundary is defence-in-depth.
pub fn sanitize_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_underscore = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
            last_was_underscore = c == '_';
        } else if !last_was_underscore && !out.is_empty() {
            out.push('_');
            last_was_underscore = true;
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed
    }
}

impl Project {
    /// Monotonic project-file schema version. Bump when a new
    /// load-time migration ships; add the migration step to
    /// [`Project::run_migrations`].
    ///
    /// History:
    /// - `v = 0`: pre-Sprint-14 (no water_mode tracking).
    /// - `v = 1`: Sprint 14 / C9 — water-mode derivation from
    ///   `min_height < 0`.
    pub const SCHEMA_V: u32 = 1;

    pub fn new(name: impl Into<String>, smu: u32) -> Self {
        let size = MapSize::square(smu);
        Self {
            name: name.into(),
            size,
            min_height: 0.0,
            max_height: 256.0,
            heightmap: None,
            ally_groups: Vec::new(),
            mapinfo_overrides: HashMap::new(),
            next_steps_dismissed: false,
            migration_toast_dismissed: false,
            splat_config: SplatConfig::default(),
            dnts_diffuse_in_alpha: false,
            splat_distribution: None,
            // D8 / Sprint 15 (ADR-038): single-layer biome-base seed
            // so fresh projects always have a non-empty stack.
            // Callers with a wizard biome label can override by
            // re-seeding via `LayerStack::from_biome` after `new`.
            layers: LayerStack::from_biome("", size),
            metal_spots: Vec::new(),
            geo_vents: Vec::new(),
            features: Vec::new(),
            specular_tex_path: None,
            extractor_radius: default_extractor_radius(),
            water_mode: WaterMode::default(),
            water_overrides: WaterBlock::default(),
            void_water: false,
            tidal_strength: None,
            lava_atmosphere: false,
            schema_v: Self::SCHEMA_V,
        }
    }

    /// D8 / Sprint 15 (ADR-038): seed [`Self::layers`] from
    /// [`Self::splat_config`] if (and only if) the stack is currently
    /// empty. Idempotent — guarded by `layers.layers.is_empty()` so a
    /// user who explicitly deletes every layer in Sprint 17+ and
    /// re-opens does NOT get the migration re-fired.
    ///
    /// Call this once at load time from the app (after
    /// [`Self::load_from_file`]). The pre-D8 splat painting is NOT
    /// migrated to mask pixels in Sprint 15 — Sprint 17 will surface
    /// a one-time migration prompt when the new emission path takes
    /// over.
    pub fn after_load_migrate(&mut self, slot_resolver: &dyn SlotResolver) {
        if !self.layers.layers.is_empty() {
            return;
        }
        let size = self.size;
        let cfg = self.splat_config.clone();
        let seeded =
            LayerStack::migrate_from_splat_config(&cfg, |i| cfg.channels[i as usize], size);
        if !seeded.layers.is_empty() {
            tracing::info!(
                layer_count = seeded.layers.len(),
                "project: seeding layer stack from pre-D8 splat_config"
            );
            self.layers = seeded;
            // D10 / Sprint 17 (ADR-041): the legacy
            // `splat_config.diffuse_in_alpha` flag becomes a per-project
            // setting. Carry it over once at migration time. If the
            // project already had `dnts_diffuse_in_alpha = true` (a
            // mid-sprint hand-edit) we preserve that.
            self.dnts_diffuse_in_alpha = self.dnts_diffuse_in_alpha || cfg.diffuse_in_alpha;
        }
        let _ = slot_resolver; // future: warn when bound slot ids miss the registry
    }

    /// Apply load-time migrations in order. Runs exactly once on
    /// `From<ProjectWire>` after wire fields have been populated.
    /// Each migration is gated on the current `schema_v` so re-loading
    /// a migrated project is a no-op.
    ///
    /// **Pitfall (PITFALL §6 / C9 prompt):** the water-mode migration
    /// must NOT fire on subsequent loads — otherwise a user who
    /// explicitly set `water_mode = None` on a `min_height < 0` map
    /// would get `Ocean` clobbered every load.
    fn run_migrations(p: &mut Project) {
        // v0 -> v1: derive `water_mode = Ocean` when terrain dips below
        // 0 and the user hasn't already chosen a mode.
        if p.schema_v < 1 {
            if p.water_mode == WaterMode::default() && p.min_height < 0.0 {
                tracing::info!(
                    min_height = p.min_height,
                    "project schema migration v0->v1: setting water_mode = Ocean"
                );
                p.water_mode = WaterMode::Ocean;
            }
            p.schema_v = 1;
        }
        // Future migrations append here.
    }

    /// Resolve `heightmap` against the project file's parent directory.
    /// Returns `None` if no heightmap is set.
    pub fn resolve_heightmap(&self, project_path: &Path) -> Option<PathBuf> {
        let rel = self.heightmap.as_ref()?;
        if rel.is_absolute() {
            return Some(rel.clone());
        }
        let base = project_path.parent().unwrap_or_else(|| Path::new("."));
        Some(base.join(rel))
    }

    /// Rewrite `heightmap` as relative to the project file's parent when
    /// possible. Falls back to the absolute path otherwise. Call before save.
    pub fn relativize_heightmap(&mut self, project_path: &Path) {
        let Some(abs) = self.heightmap.as_ref() else {
            return;
        };
        if !abs.is_absolute() {
            return;
        }
        let Some(base) = project_path.parent() else {
            return;
        };
        if let Ok(rel) = abs.strip_prefix(base) {
            self.heightmap = Some(rel.to_path_buf());
        }
    }

    pub fn save_to_file(&self, path: &Path) -> Result<(), ProjectSaveError> {
        let s = toml::to_string_pretty(self)?;
        fs::write(path, s).map_err(|e| ProjectSaveError::Io {
            path: path.to_path_buf(),
            source: e,
        })
    }

    pub fn load_from_file(path: &Path) -> Result<Self, ProjectLoadError> {
        let s = fs::read_to_string(path).map_err(|e| ProjectLoadError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        toml::from_str(&s).map_err(|e| ProjectLoadError::Parse {
            path: path.to_path_buf(),
            source: e,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_round_trips_through_toml() {
        let mut p = Project::new("apophis-clone", 16);
        p.heightmap = Some(PathBuf::from("heightmap.png"));
        let s = toml::to_string(&p).unwrap();
        let p2: Project = toml::from_str(&s).unwrap();
        assert_eq!(p.name, p2.name);
        assert_eq!(p.size, p2.size);
        assert_eq!(p.min_height, p2.min_height);
        assert_eq!(p.max_height, p2.max_height);
        assert_eq!(p.heightmap, p2.heightmap);
    }

    #[test]
    fn heightmap_omitted_when_none() {
        let p = Project::new("no-hm", 4);
        let s = toml::to_string(&p).unwrap();
        assert!(!s.contains("heightmap"), "got:\n{s}");
    }

    #[test]
    fn ally_groups_omitted_when_empty() {
        let p = Project::new("no-teams", 4);
        let s = toml::to_string(&p).unwrap();
        assert!(!s.contains("ally_groups"), "got:\n{s}");
        assert!(!s.contains("start_positions"), "got:\n{s}");
    }

    #[test]
    fn ally_groups_round_trip() {
        let mut p = Project::new("teams", 8);
        let mut g = AllyGroup::new(0);
        g.start_positions = vec![
            StartPosition {
                x_elmo: 1024,
                z_elmo: 1024,
            },
            StartPosition {
                x_elmo: 1280,
                z_elmo: 1024,
            },
        ];
        g.box_polygon = Some(vec![(0.0, 0.0), (1.0, 0.12)]);
        p.ally_groups.push(g);

        let mut g1 = AllyGroup::new(1);
        g1.start_positions = vec![StartPosition {
            x_elmo: 3072,
            z_elmo: 3072,
        }];
        p.ally_groups.push(g1);

        let s = toml::to_string(&p).unwrap();
        let p2: Project = toml::from_str(&s).unwrap();
        assert_eq!(p.ally_groups, p2.ally_groups);
    }

    /// Pre-ADR-032 `.barmeproj` files carry `[[start_positions]]`
    /// with a `team_id` field. Loading must put every position in
    /// `ally_groups[0]` and drop the team_id. A silent data-loss
    /// bug here destroys user projects.
    #[test]
    fn legacy_flat_start_positions_load_into_ally_group_zero() {
        let toml_str = r#"
name = "legacy_v2"
min_height = 0.0
max_height = 256.0

[size]
smu_x = 4
smu_z = 4

[[start_positions]]
team_id = 0
x_elmo = 100
z_elmo = 200

[[start_positions]]
team_id = 1
x_elmo = 900
z_elmo = 800
"#;
        let p: Project = toml::from_str(toml_str).unwrap();
        assert_eq!(p.ally_groups.len(), 1, "should produce exactly 1 group");
        let g0 = &p.ally_groups[0];
        assert_eq!(g0.id, 0);
        assert_eq!(g0.color, ALLY_GROUP_PALETTE[0]);
        assert_eq!(g0.name, "AllyGroup 0");
        assert_eq!(g0.start_positions.len(), 2);
        assert_eq!(
            g0.start_positions[0],
            StartPosition {
                x_elmo: 100,
                z_elmo: 200
            }
        );
        assert_eq!(
            g0.start_positions[1],
            StartPosition {
                x_elmo: 900,
                z_elmo: 800
            }
        );
    }

    #[test]
    fn legacy_with_modern_ally_groups_ignores_legacy() {
        // Corrupt file: both shapes present. Migration ignores the
        // legacy vec rather than blending it into ally_groups[0].
        let toml_str = r#"
name = "mixed"
min_height = 0.0
max_height = 256.0

[size]
smu_x = 4
smu_z = 4

[[ally_groups]]
id = 0
name = "Modern"
color = [10, 20, 30]
[[ally_groups.start_positions]]
x_elmo = 1
z_elmo = 2

[[start_positions]]
team_id = 9
x_elmo = 9999
z_elmo = 9999
"#;
        let p: Project = toml::from_str(toml_str).unwrap();
        assert_eq!(p.ally_groups.len(), 1);
        let g0 = &p.ally_groups[0];
        assert_eq!(g0.color, [10, 20, 30]);
        assert_eq!(g0.start_positions.len(), 1);
        // Legacy position discarded.
        assert!(!g0.start_positions.iter().any(|p| p.x_elmo == 9999));
    }

    #[test]
    fn pre_f8_project_without_any_start_positions_loads_forward() {
        let toml_str = r#"
name = "pre_f8"
min_height = 0.0
max_height = 256.0

[size]
smu_x = 4
smu_z = 4
"#;
        let p: Project = toml::from_str(toml_str).unwrap();
        assert!(p.ally_groups.is_empty());
        assert!(p.mapinfo_overrides.is_empty());
    }

    #[test]
    fn ally_group_default_uses_palette() {
        let g0 = AllyGroup::new(0);
        let g1 = AllyGroup::new(1);
        let g2 = AllyGroup::new(2);
        assert_eq!(g0.color, ALLY_GROUP_PALETTE[0]);
        assert_eq!(g1.color, ALLY_GROUP_PALETTE[1]);
        assert_eq!(g2.color, ALLY_GROUP_PALETTE[2]);
        assert_eq!(g0.name, "AllyGroup 0");
        assert_eq!(g0.start_positions.len(), 0);
        assert!(g0.box_polygon.is_none());
    }

    /// B8: `next_steps_dismissed` is a transient marker the editor
    /// uses to suppress a hint Window. Default false; serialised only
    /// when true so a fresh project doesn't carry the flag.
    #[test]
    fn next_steps_dismissed_default_false() {
        let p = Project::new("hint", 4);
        assert!(!p.next_steps_dismissed);
    }

    #[test]
    fn next_steps_dismissed_omitted_when_false() {
        let p = Project::new("hint", 4);
        let s = toml::to_string(&p).unwrap();
        assert!(
            !s.contains("next_steps_dismissed"),
            "default-false flag must not serialise; got:\n{s}"
        );
    }

    #[test]
    fn next_steps_dismissed_round_trips_when_true() {
        let mut p = Project::new("hint", 4);
        p.next_steps_dismissed = true;
        let s = toml::to_string(&p).unwrap();
        assert!(
            s.contains("next_steps_dismissed = true"),
            "true flag must serialise; got:\n{s}"
        );
        let p2: Project = toml::from_str(&s).unwrap();
        assert!(p2.next_steps_dismissed);
    }

    #[test]
    fn mapinfo_overrides_omitted_when_empty() {
        let p = Project::new("clean", 4);
        let s = toml::to_string(&p).unwrap();
        assert!(
            !s.contains("mapinfo_overrides"),
            "empty mapinfo_overrides must not serialise; got:\n{s}"
        );
    }

    #[test]
    fn mapinfo_overrides_round_trip() {
        let mut p = Project::new("overrides", 4);
        p.mapinfo_overrides
            .insert("atmosphere.sky_box".to_string(), "clear_day.dds".into());
        p.mapinfo_overrides
            .insert("gravity".to_string(), toml::Value::Float(150.0));
        let s = toml::to_string(&p).unwrap();
        let p2: Project = toml::from_str(&s).unwrap();
        assert_eq!(p.mapinfo_overrides, p2.mapinfo_overrides);
    }

    #[test]
    fn save_and_load_round_trip_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("demo.barmeproj");
        let p = Project::new("demo", 8);
        p.save_to_file(&path).unwrap();
        let p2 = Project::load_from_file(&path).unwrap();
        assert_eq!(p.name, p2.name);
        assert_eq!(p.size, p2.size);
    }

    #[test]
    fn sanitize_name_passes_alphanumeric_through() {
        assert_eq!(sanitize_name("apophis-clone-1"), "apophis-clone-1");
        assert_eq!(sanitize_name("my_map_v2"), "my_map_v2");
    }

    #[test]
    fn sanitize_name_collapses_disallowed_into_underscore() {
        assert_eq!(sanitize_name("my map: 1.0"), "my_map_1_0");
        assert_eq!(sanitize_name("hello!!!world"), "hello_world");
    }

    #[test]
    fn sanitize_name_trims_edges_and_handles_empty() {
        assert_eq!(sanitize_name("   "), "untitled");
        assert_eq!(sanitize_name(""), "untitled");
        assert_eq!(sanitize_name("___"), "untitled");
        assert_eq!(sanitize_name(" foo "), "foo");
    }

    #[test]
    fn sanitize_name_creates_safe_filenames() {
        for input in ["maps/foo", "C:\\Users\\me", "a b c", "x/y\\z"] {
            let s = sanitize_name(input);
            assert!(!s.contains('/'));
            assert!(!s.contains('\\'));
            assert!(!s.contains(':'));
            assert!(!s.contains(' '));
        }
    }

    /// D5 / Sprint 9: per-channel splat slot bindings persist across
    /// save / open. Round-trip a non-default config through TOML.
    #[test]
    fn splat_config_round_trips() {
        let mut p = Project::new("splat", 8);
        p.splat_config.channels = [Some(0), Some(2), None, Some(8)];
        p.splat_config.tex_scales = [0.02, 0.004, 0.02, 0.0015];
        p.splat_config.tex_mults = [1.0, 1.5, 1.0, 0.8];
        p.splat_config.diffuse_in_alpha = true;
        let s = toml::to_string(&p).unwrap();
        let p2: Project = toml::from_str(&s).unwrap();
        assert_eq!(p.splat_config, p2.splat_config);
    }

    /// `splat_distribution` is `#[serde(skip)]` — the 4 MB RGBA buffer
    /// stays out of the TOML manifest (D6 ships PNG sidecar
    /// persistence). Confirm the field is never written.
    #[test]
    fn splat_distribution_omitted_from_serialization() {
        let mut p = Project::new("no-splat-blob", 4);
        p.splat_distribution = Some(crate::SplatDistribution::new(p.size));
        let s = toml::to_string(&p).unwrap();
        assert!(
            !s.contains("splat_distribution"),
            "splat_distribution must not serialise; got:\n{s}"
        );
    }

    /// C4 (Sprint 11): `metal_spots` round-trip through TOML. The
    /// vec is `skip_serializing_if = "Vec::is_empty"` so pre-C4 files
    /// load forward without surprise.
    #[test]
    fn metal_spots_round_trip_through_toml() {
        let mut p = Project::new("metals", 8);
        p.metal_spots.push(MetalSpot {
            x_elmo: 1024,
            z_elmo: 1024,
            metal: 2.0,
        });
        p.metal_spots.push(MetalSpot {
            x_elmo: 3072,
            z_elmo: 3072,
            metal: 4.0,
        });
        let s = toml::to_string(&p).unwrap();
        assert!(s.contains("metal_spots"), "got:\n{s}");
        let p2: Project = toml::from_str(&s).unwrap();
        assert_eq!(p.metal_spots, p2.metal_spots);
    }

    /// Empty `metal_spots` should not serialise — keeps fresh-project
    /// TOML noise-free.
    #[test]
    fn metal_spots_omitted_when_empty() {
        let p = Project::new("clean", 4);
        let s = toml::to_string(&p).unwrap();
        assert!(!s.contains("metal_spots"), "got:\n{s}");
    }

    /// C5 (Sprint 11): `geo_vents` round-trip. Same omit-when-empty
    /// rule as `metal_spots`.
    #[test]
    fn geo_vents_round_trip_through_toml() {
        let mut p = Project::new("vents", 8);
        p.geo_vents.push(GeoVent {
            x_elmo: 4096,
            z_elmo: 4096,
        });
        p.geo_vents.push(GeoVent {
            x_elmo: 4096,
            z_elmo: 8192,
        });
        let s = toml::to_string(&p).unwrap();
        assert!(s.contains("geo_vents"), "got:\n{s}");
        let p2: Project = toml::from_str(&s).unwrap();
        assert_eq!(p.geo_vents, p2.geo_vents);
    }

    #[test]
    fn geo_vents_omitted_when_empty() {
        let p = Project::new("clean", 4);
        let s = toml::to_string(&p).unwrap();
        assert!(!s.contains("geo_vents"), "got:\n{s}");
    }

    /// C6 (Sprint 12): `features` round-trip through TOML. Same
    /// omit-when-empty rule as the other source-vec fields.
    #[test]
    fn features_round_trip_through_toml() {
        let mut p = Project::new("trees", 8);
        p.features
            .push(FeatureInstance::new("pinetree", 1024, 1024, 0));
        p.features
            .push(FeatureInstance::new("agorm_talltree6", 2048, 3072, 32768));
        let s = toml::to_string(&p).unwrap();
        assert!(s.contains("features"), "got:\n{s}");
        let p2: Project = toml::from_str(&s).unwrap();
        assert_eq!(p.features, p2.features);
    }

    #[test]
    fn features_omitted_when_empty() {
        let p = Project::new("clean", 4);
        let s = toml::to_string(&p).unwrap();
        assert!(!s.contains("\nfeatures = "), "got:\n{s}");
        assert!(!s.contains("\n[[features]]"), "got:\n{s}");
    }

    /// C6: a pre-Sprint-12 project (no `features`, no `specular_tex_path`)
    /// loads forward with empty defaults rather than failing.
    #[test]
    fn pre_c6_project_without_features_loads_with_defaults() {
        let toml_str = r#"
name = "pre_c6"
min_height = 0.0
max_height = 256.0

[size]
smu_x = 4
smu_z = 4
"#;
        let p: Project = toml::from_str(toml_str).unwrap();
        assert!(p.features.is_empty());
        assert!(p.specular_tex_path.is_none());
    }

    /// C6: `FeatureInstance::new` is the convenience constructor.
    #[test]
    fn feature_instance_new_pins_fields() {
        let f = FeatureInstance::new("pinetree", 100, 200, 16384);
        assert_eq!(f.name, "pinetree");
        assert_eq!(f.x_elmo, 100);
        assert_eq!(f.z_elmo, 200);
        assert_eq!(f.rot_heading, 16384);
    }

    /// D6 (Sprint 12): `specular_tex_path` round-trips when set.
    #[test]
    fn specular_tex_path_round_trips() {
        let mut p = Project::new("spec", 4);
        p.specular_tex_path = Some(PathBuf::from("custom_specular.dds"));
        let s = toml::to_string(&p).unwrap();
        assert!(s.contains("specular_tex_path"), "got:\n{s}");
        let p2: Project = toml::from_str(&s).unwrap();
        assert_eq!(p.specular_tex_path, p2.specular_tex_path);
    }

    /// PITFALL §6: setting `extractor_radius = 500` (the engine
    /// default) silently breaks BAR's mex-snap. The Project default
    /// is 80, the BAR-mod-override convention.
    #[test]
    fn extractor_radius_defaults_to_bar_convention_80() {
        let p = Project::new("default-radius", 4);
        assert_eq!(p.extractor_radius, 80.0);
    }

    /// A pre-C4 project file without the `extractor_radius` key loads
    /// with the BAR default rather than `0.0` (which would crash mex
    /// snap or default to the engine's 500).
    #[test]
    fn pre_c4_project_without_extractor_radius_loads_with_default() {
        let toml_str = r#"
name = "pre_c4"
min_height = 0.0
max_height = 256.0

[size]
smu_x = 4
smu_z = 4
"#;
        let p: Project = toml::from_str(toml_str).unwrap();
        assert_eq!(p.extractor_radius, 80.0);
        assert!(p.metal_spots.is_empty());
        assert!(p.geo_vents.is_empty());
    }

    #[test]
    fn metal_spot_new_uses_default_metal() {
        let m = MetalSpot::new(100, 200);
        assert_eq!(m.x_elmo, 100);
        assert_eq!(m.z_elmo, 200);
        assert_eq!(m.metal, MetalSpot::DEFAULT_METAL);
        assert_eq!(MetalSpot::DEFAULT_METAL, 2.0);
    }

    /// Pre-D5 `.barmeproj` files have no `[splat_config]` block —
    /// loading must materialise the engine defaults rather than
    /// failing.
    #[test]
    fn pre_d5_project_without_splat_config_loads_with_defaults() {
        let toml_str = r#"
name = "pre_d5"
min_height = 0.0
max_height = 256.0

[size]
smu_x = 4
smu_z = 4
"#;
        let p: Project = toml::from_str(toml_str).unwrap();
        let d = crate::SplatConfig::default();
        assert_eq!(p.splat_config, d);
        assert!(p.splat_distribution.is_none());
    }

    /// C9 (Sprint 14 / ADR-042): pre-Sprint-14 `.barmeproj` files
    /// carry no `water_mode` and no `schema_v`. Loading them with
    /// `min_height < 0` must materialise `water_mode = Ocean` (the
    /// "you were probably expecting an ocean" inference). Re-loads
    /// of the same project must NOT re-fire — gate on `schema_v`.
    #[test]
    fn pre_sprint_14_project_with_negative_min_height_migrates_to_ocean() {
        let toml_str = r#"
name = "pre_c9_ocean"
min_height = -120.0
max_height = 256.0

[size]
smu_x = 8
smu_z = 8
"#;
        let p: Project = toml::from_str(toml_str).unwrap();
        assert_eq!(
            p.water_mode,
            WaterMode::Ocean,
            "min_height < 0 should migrate to Ocean"
        );
        assert_eq!(
            p.schema_v,
            Project::SCHEMA_V,
            "migration must bump schema_v so re-loads skip it"
        );
    }

    /// Pre-Sprint-14 fixtures with `min_height >= 0` stay at the
    /// default `WaterMode::None` — no water sub-table emits and the
    /// engine renders nothing (the user wanted a dry map).
    #[test]
    fn pre_sprint_14_dry_project_stays_at_water_mode_none() {
        let toml_str = r#"
name = "pre_c9_dry"
min_height = 50.0
max_height = 256.0

[size]
smu_x = 4
smu_z = 4
"#;
        let p: Project = toml::from_str(toml_str).unwrap();
        assert_eq!(p.water_mode, WaterMode::None);
        assert_eq!(p.schema_v, Project::SCHEMA_V);
    }

    /// Critical pitfall: the migration must run exactly once per
    /// project. A user who explicitly sets `water_mode = None` on an
    /// `min_height < 0` map must keep `None` after save + reload.
    /// Re-firing the rule would silently overwrite the user's choice.
    #[test]
    fn migration_does_not_re_fire_on_subsequent_loads() {
        // Simulate first load (schema_v = 0, min_height < 0 → Ocean).
        let toml_first = r#"
name = "ocean_user"
min_height = -100.0
max_height = 256.0

[size]
smu_x = 4
smu_z = 4
"#;
        let mut p: Project = toml::from_str(toml_first).unwrap();
        assert_eq!(p.water_mode, WaterMode::Ocean);
        assert_eq!(p.schema_v, Project::SCHEMA_V);

        // User then explicitly chooses `None` and re-saves.
        p.water_mode = WaterMode::None;
        let s = toml::to_string(&p).unwrap();
        // Re-load — schema_v should be 1, so migration sees the
        // explicit None and does NOT clobber it.
        let p2: Project = toml::from_str(&s).unwrap();
        assert_eq!(
            p2.water_mode,
            WaterMode::None,
            "explicit None must survive re-load"
        );
        assert_eq!(p2.schema_v, Project::SCHEMA_V);
    }

    /// A fresh `Project::new` carries the current schema version, so
    /// it never triggers a migration on first save+load.
    #[test]
    fn fresh_project_carries_current_schema_v() {
        let p = Project::new("fresh", 4);
        assert_eq!(p.schema_v, Project::SCHEMA_V);
        assert_eq!(p.water_mode, WaterMode::None);
    }

    /// `water_overrides` round-trips through TOML when the user has
    /// authored fields.
    #[test]
    fn water_overrides_round_trip_when_populated() {
        let mut p = Project::new("overrides", 4);
        p.water_mode = WaterMode::Ocean;
        p.water_overrides.damage = Some(30.0);
        p.water_overrides.surface_alpha = Some(0.5);
        let s = toml::to_string(&p).unwrap();
        let p2: Project = toml::from_str(&s).unwrap();
        assert_eq!(p2.water_mode, WaterMode::Ocean);
        assert_eq!(p2.water_overrides.damage, Some(30.0));
        assert_eq!(p2.water_overrides.surface_alpha, Some(0.5));
    }

    /// Empty `water_overrides` must NOT serialise — keeps fresh-
    /// project TOML noise-free.
    #[test]
    fn water_overrides_omitted_when_empty() {
        let p = Project::new("clean", 4);
        let s = toml::to_string(&p).unwrap();
        assert!(
            !s.contains("[water_overrides]"),
            "empty water_overrides must not serialise; got:\n{s}"
        );
    }

    /// `void_water` round-trips when set; omitted when false (default).
    #[test]
    fn void_water_round_trips_and_omits_when_default() {
        let mut p = Project::new("void", 4);
        assert!(!p.void_water);
        let s = toml::to_string(&p).unwrap();
        assert!(!s.contains("void_water"));
        p.void_water = true;
        let s = toml::to_string(&p).unwrap();
        assert!(s.contains("void_water = true"));
        let p2: Project = toml::from_str(&s).unwrap();
        assert!(p2.void_water);
    }

    /// `lava_atmosphere` round-trips and omits the false default.
    #[test]
    fn lava_atmosphere_round_trips_and_omits_when_default() {
        let mut p = Project::new("hellscape", 4);
        assert!(!p.lava_atmosphere);
        let s = toml::to_string(&p).unwrap();
        assert!(!s.contains("lava_atmosphere"));
        p.lava_atmosphere = true;
        let s = toml::to_string(&p).unwrap();
        assert!(s.contains("lava_atmosphere = true"));
        let p2: Project = toml::from_str(&s).unwrap();
        assert!(p2.lava_atmosphere);
    }

    // ─────── D8 / Sprint 15 (ADR-038) — layered painter ───────

    /// `Project::new` seeds a single-layer biome-base stack (slot 0),
    /// so a fresh project's bake hits the layer path rather than the
    /// `synth_biome_bmp` fallback.
    #[test]
    fn new_project_seeds_single_layer_stack() {
        let p = Project::new("fresh", 4);
        assert_eq!(p.layers.layers.len(), 1);
        match &p.layers.layers[0].source {
            crate::layers::LayerSource::Slot { id } => assert_eq!(*id, 0),
            other => panic!("expected Slot{{0}}, got {other:?}"),
        }
    }

    /// Pre-Sprint-15 `.barmeproj` files have no `[layers]` block — they
    /// must load with an empty stack (which the app's
    /// `after_load_migrate` then seeds from `splat_config`).
    #[test]
    fn pre_sprint_15_project_loads_with_empty_layer_stack() {
        let toml_str = r#"
name = "pre_d8"
min_height = 0.0
max_height = 256.0

[size]
smu_x = 4
smu_z = 4

[splat_config]
channels = [0, 1, -1, -1]
tex_scales = [0.02, 0.02, 0.02, 0.02]
tex_mults = [1.0, 1.0, 1.0, 1.0]
"#;
        let p: Project = toml::from_str(toml_str).unwrap();
        assert!(p.layers.layers.is_empty());
        // `splat_config` survives — Sprint 17 retires it; Sprint 15
        // keeps both side-by-side.
        assert_eq!(p.splat_config.channels[0], Some(0));
        assert_eq!(p.splat_config.channels[1], Some(1));
    }

    /// `after_load_migrate` seeds one layer per bound DNTS channel
    /// when the stack starts empty. Idempotent on re-run.
    #[test]
    fn after_load_migrate_seeds_layers_from_splat_config_once() {
        struct NullResolver;
        impl crate::layers::SlotResolver for NullResolver {
            fn diffuse_path(&self, _slot_id: u8) -> Option<std::path::PathBuf> {
                None
            }
        }
        let mut p = Project::new("legacy", 4);
        p.layers.layers.clear(); // pretend we just loaded a pre-D8 file
        p.splat_config.channels = [Some(0), Some(2), None, Some(7)];
        p.after_load_migrate(&NullResolver);
        assert_eq!(p.layers.layers.len(), 3);
        // Calling again is a no-op (stack non-empty).
        p.after_load_migrate(&NullResolver);
        assert_eq!(p.layers.layers.len(), 3);
    }

    /// User-deletes-everything path: even if `splat_config` has bound
    /// channels, an explicitly-emptied stack must not be re-seeded by
    /// `after_load_migrate`.
    #[test]
    fn after_load_migrate_does_not_re_seed_an_explicitly_emptied_stack() {
        // The guard is `layers.is_empty()`. Sprint 17 will surface a
        // UI to delete every layer; until then this test pins the
        // contract.
        struct NullResolver;
        impl crate::layers::SlotResolver for NullResolver {
            fn diffuse_path(&self, _slot_id: u8) -> Option<std::path::PathBuf> {
                None
            }
        }
        let mut p = Project::new("user-empty", 4);
        p.splat_config.channels = [Some(0), Some(1), None, None];
        // Seed once.
        p.layers.layers.clear();
        p.after_load_migrate(&NullResolver);
        assert!(!p.layers.layers.is_empty());
        // Save / re-load: stack persists; migrate is now a no-op
        // because the stack is non-empty.
        let s = toml::to_string(&p).unwrap();
        let mut p2: Project = toml::from_str(&s).unwrap();
        let layer_count_before = p2.layers.layers.len();
        p2.after_load_migrate(&NullResolver);
        assert_eq!(p2.layers.layers.len(), layer_count_before);
    }

    #[test]
    fn relativize_heightmap_strips_project_dir() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path().join("demo.barmeproj");
        let hm_abs = dir.path().join("heightmap.png");
        let mut p = Project::new("demo", 4);
        p.heightmap = Some(hm_abs.clone());
        p.relativize_heightmap(&project_path);
        assert_eq!(p.heightmap, Some(PathBuf::from("heightmap.png")));
        let resolved = p.resolve_heightmap(&project_path).unwrap();
        assert_eq!(resolved, hm_abs);
    }
}
