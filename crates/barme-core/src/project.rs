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
    /// D5 / Sprint 9: per-channel splat slot bindings, scales, mults,
    /// and the ADR-034 placeholder toggle. Round-trips through TOML;
    /// `#[serde(default)]` materialises the engine defaults for
    /// pre-Sprint-9 `.barmeproj` files.
    #[serde(default)]
    pub splat_config: SplatConfig,
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
    /// C4 / Sprint 11: BAR-convention extractor radius in elmos. Engine
    /// default is 500 but BAR overrides it to 80; setting it back to
    /// 500 silently breaks mex-snap (PITFALL §6). Surfaced to the F5
    /// inspector with a tooltip; the F9 form editor (Sprint 13) will
    /// also reach this through `mapinfo.extractor_radius`.
    #[serde(default = "default_extractor_radius")]
    pub extractor_radius: f32,
}

/// BAR-convention default for `Project.extractor_radius`. Engine
/// default is 500 but BAR overrides to 80 via the mod gadgets, and
/// every player's UI snaps mexes against this value.
pub fn default_extractor_radius() -> f32 {
    80.0
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
/// strong central mex. The engine multiplies by `0.43 × 9 / 21 × 255`
/// at spawn time (FINDINGS §5 / `map_metal_spot_placer.lua`); the user
/// sees the BAR-facing scalar in the inspector. Symmetry-derived
/// mirrors are NOT stored — `Project.metal_spots` is the source set
/// and the active `SymmetryAxis` recomputes mirrors per frame in the
/// editor and per build in the pipeline (matches F8 / ADR-032).
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
    splat_config: SplatConfig,
    #[serde(default)]
    metal_spots: Vec<MetalSpot>,
    #[serde(default)]
    geo_vents: Vec<GeoVent>,
    #[serde(default = "default_extractor_radius")]
    extractor_radius: f32,
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
            splat_config: w.splat_config,
            splat_distribution: None,
            metal_spots: w.metal_spots,
            geo_vents: w.geo_vents,
            extractor_radius: w.extractor_radius,
        };
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
    pub fn new(name: impl Into<String>, smu: u32) -> Self {
        Self {
            name: name.into(),
            size: MapSize::square(smu),
            min_height: 0.0,
            max_height: 256.0,
            heightmap: None,
            ally_groups: Vec::new(),
            mapinfo_overrides: HashMap::new(),
            next_steps_dismissed: false,
            splat_config: SplatConfig::default(),
            splat_distribution: None,
            metal_spots: Vec::new(),
            geo_vents: Vec::new(),
            extractor_radius: default_extractor_radius(),
        }
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
