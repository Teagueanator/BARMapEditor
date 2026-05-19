//! Typed `mapinfo.lua` schema (ADR-028).
//!
//! ## What this module is
//!
//! A strongly-typed Rust representation of the entire `mapinfo.lua`
//! surface the engine + BAR mod-side gadgets consume. Built once,
//! re-used three times: C2 emits it, C7's F9 form edits it, C8's
//! linter walks it.
//!
//! This module DOES NOT serialise to Lua text — that's C2's job
//! (`barme-pipeline::mapinfo` / `barme-mapinfo`, with a Lua AST). C1's
//! contract is the data shape + BAR-convention defaults + the
//! `From<&Project>` conversion.
//!
//! ## Source of truth
//!
//! `docs/research/mapinfo/claude-research-findings.md`. On every
//! divergence from Gemini's report, Claude wins — Gemini's report had
//! fabricated line numbers, an incorrect feature `rot` type, and a
//! confused tidal claim. Doubly-safe fields (both reports agree) are
//! flagged inline.
//!
//! ## Top-level layout
//!
//! Mirrors `MapInfo.cpp::ReadGlobal` plus the named sub-tables under
//! their canonical Lua paths. Field names use Rust `snake_case`; the
//! Lua key mapping (camelCase, e.g. `sunDir`, `splatDetailNormalTex`)
//! is C2's responsibility when it ships the emitter.
//!
//! ## Pitfalls modelled at the type level
//!
//! - `lighting.sun_dir` is `[f32; 4]` — vec3 + `w` intensity scalar
//!   (engine default `1.0`, per `MapInfo.cpp:213`, FINDINGS NEW-6).
//!   Not `[f32; 3]`, and not the `1e9` sunStartDistance the
//!   pre-audit research mis-attributed. Encoded in [`SunDir`].
//! - [`TeamBlock`] carries ONLY `start_pos`. NO `ally_team` field —
//!   that belongs to `mapconfig/map_startboxes.lua`, a separate file
//!   that C2 emits from `Project.ally_groups` (when B6 lands).
//! - [`MapInfo::extractor_radius`] defaults to BAR's 80, NOT the
//!   engine's 500. Pinned by [`MapInfo::bar_default`] + a regression
//!   test.
//! - [`AtmosphereBlock::fog_start`] and `fog_end` are distinct
//!   (0.1 / 1.0). Setting them equal breaks build-ETA grid rendering;
//!   the lint pass (C8) enforces.
//! - [`ResourcesBlock::splat_detail_normal_tex`] without
//!   [`ResourcesBlock::specular_tex`] silently disables. Both modelled;
//!   lint enforces the pairing.
//!
//! ## Forward-compat
//!
//! Every Option field uses `#[serde(default, skip_serializing_if = "Option::is_none")]`
//! so a future Recoil-side field addition won't break load. Required
//! non-Option fields ([`MapInfo::name`], [`MapInfo::version`],
//! [`MapInfo::mapfile`], [`MapInfo::modtype`]) reject silent default
//! loss by design.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::project::Project;

/// RGB triple in the engine's `[0, 1]` float space. Same shape as
/// `glam::Vec3` but kept opaque since the schema crosses serde + Lua.
pub type Rgb = [f32; 3];

/// Engine default for `voidAlphaMin` (`MapInfo.cpp:107`).
fn default_void_alpha_min() -> f32 {
    0.9
}

/// Sun direction with a `w` component holding the engine's
/// intensity scalar (NOT the `sunStartDistance` pre-audit research
/// mis-attributed). Engine default is `{0, 1, 2, 1.0}` per
/// `MapInfo.cpp:213` (FINDINGS §1.4 / NEW-6). The fourth element
/// is **not** padding — emitting `1e9` over-saturates sunlight on
/// map load and would surface as a blown-out preview.
pub type SunDir = [f32; 4];

/// Top-level `mapinfo.lua` table. Mirrors `MapInfo.cpp::ReadGlobal`
/// plus the named sub-tables. Required fields ([`name`],
/// [`version`], [`mapfile`], [`modtype`]) are non-Option; optional
/// fields default to the engine's value when absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapInfo {
    /// Display name shown in Chobby and the engine HUD.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// BAR convention: `"1.0"`. Used by the archive scanner + Chobby.
    pub version: String,
    /// Path to the `.smf` inside the `.sd7` (e.g. `"maps/foo.smf"`).
    pub mapfile: String,
    /// **Must equal 3** for Chobby's map-browser filter to surface the
    /// map outside Skirmish. Engine ignores otherwise.
    pub modtype: u8,
    /// Must contain `"Map Helper v1"`. Empty is a configurable footgun;
    /// emitter must default it via `bar_default`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depend: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replace: Vec<String>,
    /// Crater deformation resistance multiplier (BAR default 100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maphardness: Option<f32>,
    /// `true` disables terrain deformation entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_deformable: Option<bool>,
    /// Per-frame gravity. **BAR convention: 130** (engine default 130 too).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gravity: Option<f32>,
    /// Non-zero on water maps to power Tidal Generators.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tidal_strength: Option<f32>,
    /// Per-pixel metalmap scale. BAR mostly ignores (Lua spots win).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_metal: Option<f32>,
    /// Engine-wide mex exclusion radius. **BAR convention: 80**, NOT
    /// the engine default 500. Pinned by `bar_default` + test.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extractor_radius: Option<f32>,
    /// Removes water plane (Apophis-style). Requires omitting
    /// [`WaterBlock::plane_color`].
    #[serde(default)]
    pub void_water: bool,
    /// Alpha-cuts ground using diffuse alpha channel.
    #[serde(default)]
    pub void_ground: bool,
    /// Alpha threshold below which `voidGround` discards fragments
    /// (engine default `0.9`, per `MapInfo.cpp:107`). The emitter
    /// only writes the key when [`void_ground`] is `true` — the
    /// engine default applies otherwise. Surfaced in F9 (Sprint 13)
    /// only when `voidGround` is on.
    #[serde(default = "default_void_alpha_min")]
    pub void_alpha_min: f32,
    /// Auto F4 view on mex queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_show_metal: Option<bool>,

    /// `smf.*` sub-table — minimum/maximum height + SMT filename.
    /// Required because [`SmfBlock::smt_file_name_0`] gates the
    /// pink-map pitfall (PITFALL #7).
    pub smf: SmfBlock,
    /// `lighting.*` sub-table. Required because BAR mod-side gadgets
    /// (e.g. `map_nightmode.lua`) read `lighting.sun_dir` without
    /// nil-checking the subtable; missing → gadget load crash.
    pub lighting: LightingBlock,
    /// `atmosphere.*` sub-table — fog, wind, sky tinting.
    pub atmosphere: AtmosphereBlock,
    /// `water.*` sub-table. Only emitted when `tidal_strength > 0` or
    /// terrain dips below `minHeight == 0`; otherwise `None` lets the
    /// engine defaults handle the deep-sea case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub water: Option<WaterBlock>,
    /// `splats.*` sub-table — RGBA splat scale + mult arrays.
    pub splats: SplatsBlock,
    /// `resources.*` sub-table — texture filenames.
    pub resources: ResourcesBlock,
    /// `terrainTypes[].*` array — typemap-index-keyed gameplay
    /// modifiers. BAR ships 4 types; `bar_default` seeds just type 0
    /// (the universal default). C3 fleshes out the four-entry array.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terrain_types: Vec<TerrainTypeBlock>,
    /// `grass.*` sub-table. BAR mostly uses the `map_grass_gl4.lua`
    /// widget so this defaults to `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grass: Option<GrassBlock>,
    /// `teams[].*` — flat pool of every concrete spawn coordinate.
    /// Each entry carries ONLY `start_pos`. Allyteam membership is
    /// **not** here; it materialises into `mapconfig/map_startboxes.lua`
    /// from `Project.ally_groups` at C2 emission time (B6 / C2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub teams: Vec<TeamBlock>,
    /// `sound.*` sub-table — reverb / EFX preset overrides. Rarely
    /// authored in BAR maps; defaults to `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound: Option<SoundBlock>,
    // NOTE: a `gui` subtable was dropped in Sprint 10 (PITFALL §19 /
    // FINDINGS §1.11). Engine reader at `MapInfo.cpp:119-124` only
    // consumes `autoShowMetal`, which already lives at the
    // top-level [`MapInfo::auto_show_metal`]. `minimapRotation` —
    // the only field a `GuiBlock` would have carried — is unused by
    // current Recoil. If F9 (Sprint 13) ever exposes another `gui.*`
    // override, re-add the struct then.
    /// `custom.*` — free-form per-gadget config (dual-fog,
    /// precipitation, volumetric clouds, etc.). Surfaced as
    /// `Spring.GetMapOptions()` to gadgets at runtime.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub custom: HashMap<String, toml::Value>,
}

/// `smf.*` sub-table — SMF binary overrides + texture references.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SmfBlock {
    /// Engine compiles this into the SMF binary; override here when
    /// the PNG min/max differs from the compiled SMF. BAR emitter
    /// always sets it from the project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_height: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_height: Option<f32>,
    /// Override minimap `.dds`. Empty string → engine extracts from SMF.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimap_tex: Option<String>,
    /// **PITFALL #7 (pink map on rename).** Must match the actual
    /// `.smt` filename inside the `.sd7`. The emitter pins this from
    /// the project name; do not let it diverge.
    pub smt_file_name_0: String,
    /// Multi-SMT maps (rare in BAR). One entry per extra SMT slice.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub smt_file_name_extras: Vec<String>,
}

/// `lighting.*` sub-table — sun direction + ambient/diffuse/specular
/// colours.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LightingBlock {
    /// Sun direction `{x, y, z, w}`. **`[f32; 4]`** — `w` is the
    /// engine's intensity scalar (default `1.0` per
    /// `MapInfo.cpp:213`, FINDINGS NEW-6), NOT the
    /// `sunStartDistance` value (`1e9`) older notes claim. Many BAR
    /// widgets crash if the `lighting` subtable is missing
    /// entirely; emit a real value.
    pub sun_dir: SunDir,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground_ambient_color: Option<Rgb>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground_diffuse_color: Option<Rgb>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground_specular_color: Option<Rgb>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground_shadow_density: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_ambient_color: Option<Rgb>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_diffuse_color: Option<Rgb>,
    /// `None` means the engine falls back to [`unit_diffuse_color`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_specular_color: Option<Rgb>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_shadow_density: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specular_exponent: Option<f32>,
}

/// `atmosphere.*` sub-table — wind range, fog, sky tinting, skybox.
///
/// **Migration (PITFALL §12 / FINDINGS §1.3):** pre-Sprint-10
/// `.barmeproj` / `MapInfo` fixtures carry a legacy
/// `sky_dir: [f32; 3]` key. Deserialization routes through
/// [`AtmosphereBlockWire`] which accepts either `sky_axis_angle`
/// (the new key) or `sky_dir` (legacy) and emits the canonical
/// shape: `sky_axis_angle = [x, y, z, 0.0]` when only legacy data
/// is present (preserves the original direction; sets angle = 0
/// radians, i.e. no skybox rotation). Engine default is
/// `[0, 0, 1, 0]` per `MapInfo.cpp:149`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "AtmosphereBlockWire")]
pub struct AtmosphereBlock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_wind: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wind: Option<f32>,
    /// **Setting equal to [`fog_end`] breaks the build-ETA grid
    /// renderer** (digest §7). BAR default is `0.1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fog_start: Option<f32>,
    /// BAR default `1.0`. Must differ from [`fog_start`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fog_end: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fog_color: Option<Rgb>,
    /// Tint AND size — values > 1 grow the sun disc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sun_color: Option<Rgb>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sky_color: Option<Rgb>,
    /// Skybox rotation as `{x, y, z, angle_radians}`. PITFALL §12 /
    /// FINDINGS §1.3 — the predecessor `sky_dir` key is deprecated
    /// (engine logs `L_DEPRECATED` if it ever sees it). Engine
    /// default `[0, 0, 1, 0]` = +Z axis, 0 radians (no rotation).
    #[serde(default = "default_sky_axis_angle")]
    pub sky_axis_angle: [f32; 4],
    /// Skybox `.dds` cube filename (in `maps/` or `bitmaps/`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sky_box: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_density: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_color: Option<Rgb>,
}

/// Engine default for `skyAxisAngle` (`MapInfo.cpp:149`). Axis +Z,
/// angle 0 radians — no skybox rotation.
fn default_sky_axis_angle() -> [f32; 4] {
    [0.0, 0.0, 1.0, 0.0]
}

/// Deserialization-only projection of [`AtmosphereBlock`] that
/// accepts both the new `sky_axis_angle` field and the legacy
/// `sky_dir` field. Migration runs in `From<AtmosphereBlockWire>`.
/// See PITFALL §12 / FINDINGS §1.3.
#[derive(Debug, Deserialize)]
struct AtmosphereBlockWire {
    #[serde(default)]
    min_wind: Option<f32>,
    #[serde(default)]
    max_wind: Option<f32>,
    #[serde(default)]
    fog_start: Option<f32>,
    #[serde(default)]
    fog_end: Option<f32>,
    #[serde(default)]
    fog_color: Option<Rgb>,
    #[serde(default)]
    sun_color: Option<Rgb>,
    #[serde(default)]
    sky_color: Option<Rgb>,
    #[serde(default)]
    sky_axis_angle: Option<[f32; 4]>,
    /// Legacy key — pre-Sprint-10 schemas. Migrated by appending
    /// `0.0` as the rotation angle, preserving the xyz direction.
    /// Engine ignored this key once `skyAxisAngle` was added, so
    /// the "no rotation" outcome matches what mappers were already
    /// getting in practice.
    #[serde(default)]
    sky_dir: Option<[f32; 3]>,
    #[serde(default)]
    sky_box: Option<String>,
    #[serde(default)]
    cloud_density: Option<f32>,
    #[serde(default)]
    cloud_color: Option<Rgb>,
}

impl From<AtmosphereBlockWire> for AtmosphereBlock {
    fn from(w: AtmosphereBlockWire) -> Self {
        let sky_axis_angle = w
            .sky_axis_angle
            .or_else(|| w.sky_dir.map(|d| [d[0], d[1], d[2], 0.0]))
            .unwrap_or_else(default_sky_axis_angle);
        Self {
            min_wind: w.min_wind,
            max_wind: w.max_wind,
            fog_start: w.fog_start,
            fog_end: w.fog_end,
            fog_color: w.fog_color,
            sun_color: w.sun_color,
            sky_color: w.sky_color,
            sky_axis_angle,
            sky_box: w.sky_box,
            cloud_density: w.cloud_density,
            cloud_color: w.cloud_color,
        }
    }
}

/// `water.*` sub-table — rendering parameters for the water plane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WaterBlock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damage: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_y: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_color: Option<Rgb>,
    /// **Must be absent for [`MapInfo::void_water`] = true to work.**
    /// Setting any value defeats voidWater silently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plane_color: Option<Rgb>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absorb: Option<Rgb>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_color: Option<Rgb>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_color: Option<Rgb>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambient_factor: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diffuse_factor: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specular_factor: Option<f32>,
    /// `None` means engine defaults to [`AtmosphereBlock::sun_color`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specular_color: Option<Rgb>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specular_power: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fresnel_min: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fresnel_max: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fresnel_power: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflection_distortion: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur_base: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur_exponent: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perlin_start_freq: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perlin_lacunarity: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perlin_amplitude: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_tiles: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shore_waves: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_rendering: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foam_texture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normal_texture: Option<String>,
    /// Animated caustic texture frames.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub caustics: Vec<String>,
}

/// `splats.*` sub-table — UV scale + intensity per RGBA channel.
/// Required by splat-textured (DNTS) maps. Engine ignores otherwise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplatsBlock {
    /// UV scale per RGBA channel. BAR default: `[0.02; 4]`.
    pub tex_scales: [f32; 4],
    /// Intensity per RGBA channel. BAR default: `[1.0; 4]`.
    pub tex_mults: [f32; 4],
}

/// `resources.*` sub-table — texture filename references.
///
/// **Pitfall:** [`splat_detail_normal_tex`] without [`specular_tex`]
/// silently disables splat normal mapping. The schema models both;
/// the lint pass (C8) enforces the pairing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResourcesBlock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail_tex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specular_tex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub splat_detail_tex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub splat_distr_tex: Option<String>,
    /// Up to 4 detail-normal textures (DNTS). The engine also accepts
    /// an `alpha = true` flag on this table; mappers wanting that
    /// behaviour set [`splat_detail_normal_diffuse_alpha`] to 1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub splat_detail_normal_tex: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub splat_detail_normal_diffuse_alpha: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sky_reflect_mod_tex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail_normal_tex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light_emission_tex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallax_height_tex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grass_blade_tex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grass_shading_tex: Option<String>,
}

/// One entry in `terrainTypes[]`. Indexed 0..255 to match typemap byte
/// values. BAR ships 4 types; emitter starts with type 0 (default
/// everything) and C3 adds the canonical 4-entry seed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainTypeBlock {
    /// Index 0..255 — must match the byte value in the typemap PNG.
    pub index: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Multiplier × [`MapInfo::maphardness`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardness: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receive_tracks: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub move_speeds: Option<TerrainMoveSpeeds>,
}

/// `terrainTypes[i].moveSpeeds.*` — per-unit-family movement
/// scalars. Defaults to 1.0 across the board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TerrainMoveSpeeds {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tank: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kbot: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hover: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ship: Option<f32>,
}

/// `grass.*` sub-table — bladed-grass shader params. BAR mostly uses
/// the `map_grass_gl4.lua` widget so this is rarely populated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GrassBlock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blade_width: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blade_height: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blade_angle: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blade_color: Option<Rgb>,
}

/// One entry in `teams[]`. **Carries ONLY `start_pos`.** No
/// `ally_team` — allyteam membership lives in
/// `mapconfig/map_startboxes.lua` (emitted by C2 from
/// `Project.ally_groups`, when B6 lands the data model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamBlock {
    pub start_pos: TeamStartPos,
}

/// `teams[i].startPos = { x, z }` — two integers in elmos.
/// **`y` is intentionally absent.** The engine samples ground height
/// at spawn time so units float correctly across water-level changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamStartPos {
    pub x: i32,
    pub z: i32,
}

/// `sound.*` sub-table — reverb / EFX preset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SoundBlock {
    /// `presets/EFXPresets.cpp` preset name (e.g. `"OUTDOORS"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
}

impl MapInfo {
    /// Return a fresh [`MapInfo`] populated with the BAR convention
    /// values from the research digest. Empty `teams[]` and empty
    /// `custom` — the per-project values land via
    /// [`From<&Project>`][From].
    ///
    /// **Pitfall coverage (pinned by tests below):**
    /// - `modtype == 3` — Chobby visibility gate.
    /// - `depend` includes `"Map Helper v1"` — engine fallback render.
    /// - `extractor_radius == Some(80)` — BAR convention, NOT engine 500.
    /// - `atmosphere.fog_start == Some(0.1)`, `fog_end == Some(1.0)` —
    ///   not equal (would break build-ETA renderer).
    /// - `splats.tex_scales == [0.02; 4]`, `tex_mults == [1.0; 4]`.
    /// - `lighting.sun_dir.len() == 4` (vec3 + intensity).
    pub fn bar_default() -> Self {
        Self {
            name: String::new(),
            shortname: None,
            description: None,
            author: None,
            version: "1.0".to_string(),
            mapfile: String::new(),
            modtype: 3,
            depend: vec!["Map Helper v1".to_string()],
            replace: Vec::new(),
            maphardness: Some(100.0),
            not_deformable: Some(false),
            gravity: Some(130.0),
            tidal_strength: None,
            max_metal: Some(0.02),
            extractor_radius: Some(80.0),
            void_water: false,
            void_ground: false,
            void_alpha_min: default_void_alpha_min(),
            auto_show_metal: Some(true),
            smf: SmfBlock {
                min_height: None,
                max_height: None,
                minimap_tex: None,
                smt_file_name_0: String::new(),
                smt_file_name_extras: Vec::new(),
            },
            lighting: LightingBlock {
                // Mild SE sun direction (xyz). `w` is the engine's
                // intensity scalar — default 1.0 per `MapInfo.cpp:213`
                // (FINDINGS NEW-6); the older `1e9` value was a
                // sunStartDistance leakage from a different code
                // path that over-saturates sunlight on load.
                sun_dir: [0.3, 1.0, -0.2, 1.0],
                ground_ambient_color: Some([0.5, 0.5, 0.5]),
                ground_diffuse_color: Some([0.5, 0.5, 0.5]),
                ground_specular_color: Some([0.1, 0.1, 0.1]),
                ground_shadow_density: Some(0.8),
                unit_ambient_color: Some([0.4, 0.4, 0.4]),
                unit_diffuse_color: Some([0.7, 0.7, 0.7]),
                unit_specular_color: None, // engine falls back to diffuse
                unit_shadow_density: Some(0.8),
                specular_exponent: Some(100.0),
            },
            atmosphere: AtmosphereBlock {
                min_wind: Some(5.0),
                max_wind: Some(25.0),
                // Distinct — equal fog_start/fog_end breaks build-ETA.
                fog_start: Some(0.1),
                fog_end: Some(1.0),
                fog_color: Some([0.7, 0.7, 0.8]),
                sun_color: Some([1.0, 1.0, 1.0]),
                sky_color: Some([0.1, 0.15, 0.7]),
                sky_axis_angle: default_sky_axis_angle(),
                sky_box: None,
                cloud_density: Some(0.5),
                cloud_color: Some([1.0, 1.0, 1.0]),
            },
            water: None,
            splats: SplatsBlock {
                tex_scales: [0.02; 4],
                tex_mults: [1.0; 4],
            },
            resources: ResourcesBlock::default(),
            terrain_types: bar_default_terrain_types(),
            grass: None,
            teams: Vec::new(),
            sound: None,
            custom: HashMap::new(),
        }
    }

    /// Variant of [`MapInfo::bar_default`] that also populates the
    /// `water` sub-table with the BAR-tuned defaults from the digest
    /// (§water table). Use when the project opts into water rendering
    /// — typically `tidal_strength > 0` or `min_height < 0`. Dry maps
    /// should keep [`MapInfo::bar_default`] (no water block) so the
    /// emitter doesn't ship a water plane that the engine then has to
    /// clip below the terrain.
    pub fn bar_default_with_water() -> Self {
        let mut info = Self::bar_default();
        info.water = Some(WaterBlock {
            damage: Some(0.0),
            // BAR-style cool grey-blue surface; digest §water defaults.
            surface_color: Some([0.75, 0.8, 0.85]),
            // Must be Some(..) for non-void-water; voidWater stays false.
            plane_color: Some([0.2, 0.34, 0.48]),
            base_color: Some([0.4, 0.7, 0.8]),
            min_color: Some([0.1, 0.2, 0.3]),
            absorb: Some([0.0, 0.0, 0.0]),
            ambient_factor: Some(1.0),
            diffuse_factor: Some(1.0),
            specular_factor: Some(1.0),
            specular_color: None, // engine defaults to atmosphere.sunColor
            specular_power: Some(20.0),
            fresnel_min: Some(0.2),
            fresnel_max: Some(0.8),
            fresnel_power: Some(4.0),
            reflection_distortion: Some(1.0),
            blur_base: Some(2.0),
            blur_exponent: Some(1.5),
            perlin_start_freq: Some(8.0),
            perlin_lacunarity: Some(3.0),
            perlin_amplitude: Some(0.9),
            num_tiles: Some(1),
            shore_waves: Some(true),
            force_rendering: Some(false),
            repeat_x: None,
            repeat_y: None,
            texture: None,
            foam_texture: None,
            normal_texture: None,
            caustics: Vec::new(),
        });
        info
    }
}

/// The four BAR-convention terrain types — Default / Rock / Sand /
/// Water — keyed by typemap byte 0..=3 per digest §8 (lines 500–509)
/// and the per-field defaults from digest §terrainTypes.
///
/// These values aren't load-bearing in the empty-project case (the
/// emitter writes the table regardless), but having the convention
/// seeded means an authored typemap immediately gets sensible
/// gameplay scalars when bytes 1/2/3 appear in the byte stream.
fn bar_default_terrain_types() -> Vec<TerrainTypeBlock> {
    vec![
        TerrainTypeBlock {
            index: 0,
            name: Some("Default".to_string()),
            hardness: Some(1.0),
            receive_tracks: Some(true),
            move_speeds: Some(TerrainMoveSpeeds {
                tank: Some(1.0),
                kbot: Some(1.0),
                hover: Some(1.0),
                ship: Some(1.0),
            }),
        },
        TerrainTypeBlock {
            index: 1,
            name: Some("Rock".to_string()),
            hardness: Some(2.0),
            receive_tracks: Some(false),
            move_speeds: Some(TerrainMoveSpeeds {
                tank: Some(0.85),
                kbot: Some(1.0),
                hover: Some(1.0),
                ship: Some(0.0),
            }),
        },
        TerrainTypeBlock {
            index: 2,
            name: Some("Sand".to_string()),
            hardness: Some(0.4),
            receive_tracks: Some(true),
            move_speeds: Some(TerrainMoveSpeeds {
                tank: Some(0.7),
                kbot: Some(0.9),
                hover: Some(1.2),
                ship: Some(0.0),
            }),
        },
        TerrainTypeBlock {
            index: 3,
            name: Some("Water".to_string()),
            hardness: Some(0.1),
            receive_tracks: Some(false),
            move_speeds: Some(TerrainMoveSpeeds {
                tank: Some(0.0),
                kbot: Some(0.0),
                hover: Some(1.0),
                ship: Some(1.0),
            }),
        },
    ]
}

/// Build a [`MapInfo`] from a [`Project`] — name + heights + mapfile
/// path + teams[] from `start_positions`. Everything else inherits
/// from [`MapInfo::bar_default`]; F9 (C7) will write project-specific
/// overrides on top of this.
///
/// This is the **data shape** part of C1. The Lua-text serializer is
/// C2's job — DO NOT call this and then string-format here.
impl From<&Project> for MapInfo {
    fn from(p: &Project) -> Self {
        let mut info = MapInfo::bar_default();
        info.name = p.name.clone();
        info.shortname = Some(p.name.clone());
        info.mapfile = format!("maps/{}.smf", p.name);
        info.smf.smt_file_name_0 = format!("maps/{}.smt", p.name);
        info.smf.min_height = Some(p.min_height);
        info.smf.max_height = Some(p.max_height);
        // Teams: flat ordered pool, ally-group id order + within-group
        // order. Emission walks groups sorted by `id` for determinism
        // (NFR-Det); within each group positions emit in the order the
        // user placed them. `start_pos` only — never `ally_team`.
        let mut groups: Vec<&crate::project::AllyGroup> = p.ally_groups.iter().collect();
        groups.sort_by_key(|g| g.id);
        info.teams = groups
            .into_iter()
            .flat_map(|g| g.start_positions.iter())
            .map(|s| TeamBlock {
                start_pos: TeamStartPos {
                    x: s.x_elmo,
                    z: s.z_elmo,
                },
            })
            .collect();
        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MapSize;

    // ────── BAR-convention defaults (digest §1 + §6 + §7) ──────

    #[test]
    fn bar_default_modtype_is_3() {
        // Chobby's map-browser gate. Without modtype == 3 the map
        // never appears in multiplayer lobby search.
        assert_eq!(MapInfo::bar_default().modtype, 3);
    }

    #[test]
    fn bar_default_depend_includes_map_helper_v1() {
        // Missing dep → engine fallback render (the "untextured" look).
        let info = MapInfo::bar_default();
        assert!(
            info.depend.iter().any(|d| d == "Map Helper v1"),
            "depend missing 'Map Helper v1'; got {:?}",
            info.depend
        );
    }

    #[test]
    fn bar_default_extractor_radius_is_80_not_engine_default_500() {
        // BAR convention is 80; engine default 500 breaks mex snap.
        assert_eq!(MapInfo::bar_default().extractor_radius, Some(80.0));
    }

    #[test]
    fn bar_default_gravity_is_130() {
        assert_eq!(MapInfo::bar_default().gravity, Some(130.0));
    }

    #[test]
    fn bar_default_atmosphere_fog_is_not_equal() {
        // Equal fog_start/fog_end breaks the build-ETA grid renderer
        // (digest §7 silent-failure landmine).
        let info = MapInfo::bar_default();
        assert_eq!(info.atmosphere.fog_start, Some(0.1));
        assert_eq!(info.atmosphere.fog_end, Some(1.0));
        assert_ne!(
            info.atmosphere.fog_start, info.atmosphere.fog_end,
            "fog_start == fog_end is a silent build-ETA breaker"
        );
    }

    #[test]
    fn bar_default_splats_are_bar_convention() {
        let info = MapInfo::bar_default();
        assert_eq!(info.splats.tex_scales, [0.02; 4]);
        assert_eq!(info.splats.tex_mults, [1.0; 4]);
    }

    #[test]
    fn bar_default_lighting_sun_dir_is_four_floats() {
        // Pitfall: easy to model as [f32; 3]. The fourth element is
        // the engine's intensity scalar (NOT a sunStartDistance —
        // see PITFALL §18 / FINDINGS §1.4 / NEW-6). Engine default
        // is exactly 1.0 per `MapInfo.cpp:213`; the older `1e9`
        // value over-saturates sunlight on load.
        let info = MapInfo::bar_default();
        let sd: SunDir = info.lighting.sun_dir;
        assert_eq!(sd.len(), 4, "sun_dir must be a 4-element array");
        assert_eq!(
            sd[3], 1.0,
            "sun_dir.w (intensity scalar) must equal 1.0; got {}",
            sd[3]
        );
    }

    #[test]
    fn bar_default_lighting_has_ambient_and_diffuse_for_ground_and_units() {
        // BAR mod gadgets read these directly without nil checks; the
        // emitter MUST include them.
        let l = MapInfo::bar_default().lighting;
        assert!(l.ground_ambient_color.is_some());
        assert!(l.ground_diffuse_color.is_some());
        assert!(l.unit_ambient_color.is_some());
        assert!(l.unit_diffuse_color.is_some());
    }

    #[test]
    fn bar_default_terrain_types_seeds_index_zero() {
        let info = MapInfo::bar_default();
        assert!(!info.terrain_types.is_empty());
        let t0 = &info.terrain_types[0];
        assert_eq!(t0.index, 0);
        assert_eq!(t0.name.as_deref(), Some("Default"));
        assert_eq!(t0.hardness, Some(1.0));
    }

    /// C3: BAR ships four default terrain types (Default / Rock / Sand
    /// / Water). The schema seeds all four so an authored typemap with
    /// bytes 0..3 immediately gets sensible gameplay scalars.
    #[test]
    fn bar_default_terrain_types_has_four_entries() {
        let info = MapInfo::bar_default();
        assert_eq!(
            info.terrain_types.len(),
            4,
            "expected 4 default terrain types; got {:?}",
            info.terrain_types
        );
    }

    #[test]
    fn bar_default_terrain_types_index_one_is_rock() {
        let t = &MapInfo::bar_default().terrain_types[1];
        assert_eq!(t.index, 1);
        assert_eq!(t.name.as_deref(), Some("Rock"));
        assert_eq!(t.hardness, Some(2.0));
        // Rock blocks ships and refuses tracks (digest §8).
        assert_eq!(t.receive_tracks, Some(false));
        let ms = t.move_speeds.as_ref().expect("rock has moveSpeeds");
        assert_eq!(ms.ship, Some(0.0));
        assert_eq!(ms.tank, Some(0.85));
    }

    #[test]
    fn bar_default_terrain_types_index_two_is_sand() {
        let t = &MapInfo::bar_default().terrain_types[2];
        assert_eq!(t.index, 2);
        assert_eq!(t.name.as_deref(), Some("Sand"));
        assert_eq!(t.hardness, Some(0.4));
        assert_eq!(t.receive_tracks, Some(true));
        let ms = t.move_speeds.as_ref().expect("sand has moveSpeeds");
        // Hovers gain on sand; ships still blocked (it's a beach).
        assert_eq!(ms.hover, Some(1.2));
        assert_eq!(ms.ship, Some(0.0));
    }

    #[test]
    fn bar_default_terrain_types_index_three_is_water() {
        let t = &MapInfo::bar_default().terrain_types[3];
        assert_eq!(t.index, 3);
        assert_eq!(t.name.as_deref(), Some("Water"));
        assert_eq!(t.hardness, Some(0.1));
        assert_eq!(t.receive_tracks, Some(false));
        let ms = t.move_speeds.as_ref().expect("water has moveSpeeds");
        // Only ships + hovers move through water.
        assert_eq!(ms.tank, Some(0.0));
        assert_eq!(ms.kbot, Some(0.0));
        assert_eq!(ms.hover, Some(1.0));
        assert_eq!(ms.ship, Some(1.0));
    }

    // ────── lighting + atmosphere default values (digest §lighting / §atmosphere) ──────

    #[test]
    fn bar_default_lighting_colour_values_match_digest() {
        let l = MapInfo::bar_default().lighting;
        assert_eq!(l.ground_ambient_color, Some([0.5, 0.5, 0.5]));
        assert_eq!(l.ground_diffuse_color, Some([0.5, 0.5, 0.5]));
        assert_eq!(l.ground_specular_color, Some([0.1, 0.1, 0.1]));
        assert_eq!(l.ground_shadow_density, Some(0.8));
        assert_eq!(l.unit_ambient_color, Some([0.4, 0.4, 0.4]));
        assert_eq!(l.unit_diffuse_color, Some([0.7, 0.7, 0.7]));
        // None means "engine falls back to unit_diffuse_color" — the
        // emitter should not write a specific value here.
        assert_eq!(l.unit_specular_color, None);
        assert_eq!(l.unit_shadow_density, Some(0.8));
        assert_eq!(l.specular_exponent, Some(100.0));
    }

    #[test]
    fn bar_default_atmosphere_wind_matches_digest() {
        let a = MapInfo::bar_default().atmosphere;
        // BAR convention 5..25 (balanced economy per digest §atmosphere).
        assert_eq!(a.min_wind, Some(5.0));
        assert_eq!(a.max_wind, Some(25.0));
    }

    #[test]
    fn bar_default_atmosphere_colour_values_match_digest() {
        let a = MapInfo::bar_default().atmosphere;
        assert_eq!(a.fog_color, Some([0.7, 0.7, 0.8]));
        assert_eq!(a.sun_color, Some([1.0, 1.0, 1.0]));
        assert_eq!(a.sky_color, Some([0.1, 0.15, 0.7]));
        // PITFALL §12 / FINDINGS §1.3: legacy `sky_dir` was
        // deprecated — engine reads `skyAxisAngle` (xyz axis +
        // radians angle). Default is `[0, 0, 1, 0]` per
        // `MapInfo.cpp:149`: +Z axis, no rotation.
        assert_eq!(a.sky_axis_angle, [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(a.cloud_density, Some(0.5));
        assert_eq!(a.cloud_color, Some([1.0, 1.0, 1.0]));
        // No skybox is set by default — projects opt in.
        assert!(a.sky_box.is_none());
    }

    // ────── water-opt-in constructor (C3) ──────

    #[test]
    fn bar_default_with_water_populates_block() {
        let info = MapInfo::bar_default_with_water();
        let w = info.water.expect("water block populated");
        // Surface, plane, min, base are the four colour fields the
        // digest flags as required for water maps.
        assert!(w.surface_color.is_some());
        assert!(w.plane_color.is_some());
        assert!(w.min_color.is_some());
        assert!(w.base_color.is_some());
        // BAR-tuned defaults: shore foam on, force_rendering off.
        assert_eq!(w.shore_waves, Some(true));
        assert_eq!(w.force_rendering, Some(false));
        // Everything else still inherits bar_default's dry layout
        // (modtype 3, gravity 130, etc.).
        assert_eq!(info.modtype, 3);
        assert_eq!(info.gravity, Some(130.0));
    }

    #[test]
    fn bar_default_with_water_does_not_break_void_water_pairing() {
        // Void-water + plane_color is the silent-disable pitfall in
        // PITFALLS.md §6. The constructor sets plane_color so it
        // must leave void_water = false.
        let info = MapInfo::bar_default_with_water();
        assert!(!info.void_water);
        let w = info.water.as_ref().unwrap();
        assert!(w.plane_color.is_some());
    }

    // ────── version + mapfile + smt defaults ──────

    #[test]
    fn bar_default_version_and_modtype_pinned() {
        let info = MapInfo::bar_default();
        assert_eq!(info.version, "1.0");
        // mapfile is filled in by From<&Project>; the bare default is
        // empty and that's intentional — saving as-is is invalid.
        assert!(info.mapfile.is_empty());
    }

    #[test]
    fn bar_default_maphardness_is_one_hundred() {
        // Digest §scalars: maphardness default 100.
        assert_eq!(MapInfo::bar_default().maphardness, Some(100.0));
    }

    #[test]
    fn bar_default_water_block_is_none() {
        // BAR convention: omit water block unless tidal > 0 or
        // minHeight < 0. `bar_default` ships dry.
        assert!(MapInfo::bar_default().water.is_none());
    }

    #[test]
    fn bar_default_max_metal_is_bar_value() {
        assert_eq!(MapInfo::bar_default().max_metal, Some(0.02));
    }

    #[test]
    fn bar_default_void_water_and_ground_are_false() {
        let info = MapInfo::bar_default();
        assert!(!info.void_water);
        assert!(!info.void_ground);
    }

    /// PITFALL §20 / FINDINGS §1.1: `MapInfo::voidAlphaMin` defaults
    /// to the engine value `0.9` (per `MapInfo.cpp:107`). The
    /// emitter only writes the key when `void_ground` is true; the
    /// field is always carried so F9 can surface it.
    #[test]
    fn bar_default_void_alpha_min_is_engine_default() {
        assert_eq!(MapInfo::bar_default().void_alpha_min, 0.9);
    }

    /// PITFALL §12 / FINDINGS §1.3: legacy `sky_dir = [x, y, z]`
    /// from a pre-Sprint-10 schema dump migrates into
    /// `sky_axis_angle = [x, y, z, 0]`, preserving the direction
    /// and setting the rotation angle to zero (no skybox spin —
    /// which matches what the engine was doing in practice, since
    /// `sky_dir` was deprecated once `skyAxisAngle` shipped).
    #[test]
    fn atmosphere_legacy_sky_dir_migrates_to_axis_angle() {
        let toml_src = r#"
            sky_dir = [0.5, -0.25, 0.8]
        "#;
        let block: AtmosphereBlock = toml::from_str(toml_src).expect("deserialize");
        assert_eq!(
            block.sky_axis_angle,
            [0.5, -0.25, 0.8, 0.0],
            "legacy sky_dir must migrate xyz unchanged with angle = 0"
        );
    }

    /// PITFALL §12: an `.barmeproj` carrying the NEW `sky_axis_angle`
    /// key takes precedence over legacy `sky_dir` if both somehow
    /// coexist (defensive; the emitter never writes both).
    #[test]
    fn atmosphere_new_sky_axis_angle_wins_over_legacy_sky_dir() {
        let toml_src = r#"
            sky_dir = [1.0, 0.0, 0.0]
            sky_axis_angle = [0.0, 0.0, 1.0, 0.5]
        "#;
        let block: AtmosphereBlock = toml::from_str(toml_src).expect("deserialize");
        assert_eq!(block.sky_axis_angle, [0.0, 0.0, 1.0, 0.5]);
    }

    /// PITFALL §12: an empty atmosphere block (no sky key at all)
    /// loads with the engine default `[0, 0, 1, 0]` via
    /// `default_sky_axis_angle`.
    #[test]
    fn atmosphere_missing_sky_key_defaults_to_engine_value() {
        let block: AtmosphereBlock = toml::from_str("").expect("deserialize");
        assert_eq!(block.sky_axis_angle, [0.0, 0.0, 1.0, 0.0]);
    }

    /// PITFALL §20: a `.barmeproj` without the field still loads —
    /// `#[serde(default = "default_void_alpha_min")]` produces 0.9.
    #[test]
    fn void_alpha_min_defaults_when_missing_from_toml() {
        // Minimal TOML lacking void_alpha_min — should round-trip
        // back to the engine default.
        let mut info = MapInfo::bar_default();
        info.void_alpha_min = 0.42; // serialise a non-default value
        let s = toml::to_string(&info).expect("serialize");
        // Strip the line containing void_alpha_min to simulate a
        // legacy file lacking the key.
        let stripped: String = s
            .lines()
            .filter(|l| !l.trim_start().starts_with("void_alpha_min"))
            .collect::<Vec<_>>()
            .join("\n");
        let back: MapInfo = toml::from_str(&stripped).expect("deserialize");
        assert_eq!(back.void_alpha_min, 0.9);
    }

    // ────── pitfall coverage — teams[] schema (digest §2) ──────

    /// **Pitfall:** `TeamBlock` MUST NOT have an `ally_team` field.
    /// This test verifies field-level shape by exhaustive
    /// destructuring — adding an `ally_team` member would force the
    /// pattern to update + fail compile. (Both research reports agree
    /// the engine ignores `teams[].ally_team`; the editor would feed
    /// the user a footgun by modelling it.)
    #[test]
    fn team_block_carries_only_start_pos() {
        let t = TeamBlock {
            start_pos: TeamStartPos { x: 100, z: 200 },
        };
        // Exhaustive destructure — adding a new field to TeamBlock
        // breaks this pattern.
        let TeamBlock { start_pos } = t;
        let TeamStartPos { x, z } = start_pos;
        assert_eq!(x, 100);
        assert_eq!(z, 200);
    }

    // ────── From<&Project> ──────

    #[test]
    fn from_project_populates_name_mapfile_and_smt() {
        let mut p = Project::new("alpha", 4);
        p.min_height = -50.0;
        p.max_height = 500.0;
        let info: MapInfo = (&p).into();
        assert_eq!(info.name, "alpha");
        assert_eq!(info.shortname.as_deref(), Some("alpha"));
        assert_eq!(info.mapfile, "maps/alpha.smf");
        assert_eq!(info.smf.smt_file_name_0, "maps/alpha.smt");
        assert_eq!(info.smf.min_height, Some(-50.0));
        assert_eq!(info.smf.max_height, Some(500.0));
        // Defaults preserved.
        assert_eq!(info.modtype, 3);
        assert_eq!(info.extractor_radius, Some(80.0));
    }

    #[test]
    fn from_project_teams_are_flattened_in_ally_group_id_order() {
        use crate::project::{AllyGroup, StartPosition};
        let mut p = Project::new("teams", 4);
        // Add groups out of order — emission must sort by `id`.
        let mut g1 = AllyGroup::new(1);
        g1.start_positions = vec![
            StartPosition {
                x_elmo: 500,
                z_elmo: 500,
            },
            StartPosition {
                x_elmo: 600,
                z_elmo: 600,
            },
        ];
        p.ally_groups.push(g1);
        let mut g0 = AllyGroup::new(0);
        g0.start_positions = vec![
            StartPosition {
                x_elmo: 100,
                z_elmo: 100,
            },
            StartPosition {
                x_elmo: 200,
                z_elmo: 200,
            },
        ];
        p.ally_groups.push(g0);
        let info: MapInfo = (&p).into();
        assert_eq!(info.teams.len(), 4);
        // Emission order: group 0's positions first (in insertion
        // order), then group 1's.
        assert_eq!(info.teams[0].start_pos.x, 100);
        assert_eq!(info.teams[1].start_pos.x, 200);
        assert_eq!(info.teams[2].start_pos.x, 500);
        assert_eq!(info.teams[3].start_pos.x, 600);
    }

    #[test]
    fn from_project_empty_ally_groups_yields_empty_teams() {
        let p = Project::new("empty", 4);
        let info: MapInfo = (&p).into();
        assert!(info.teams.is_empty());
        // The 25/75 fallback is the *emitter*'s job (ADR-013 → C2),
        // not the schema's.
    }

    #[test]
    fn from_project_smt_filename_always_matches_project_name() {
        // Pitfall #7 (pink map on rename) — schema mirror of the
        // emitter test in barme-pipeline. The schema's mapfile +
        // smtFileName0 are both derived from `project.name` in
        // lock-step; renaming the project drags both along.
        let p = Project::new("rename_me", 2);
        let info: MapInfo = (&p).into();
        assert_eq!(info.mapfile, "maps/rename_me.smf");
        assert_eq!(info.smf.smt_file_name_0, "maps/rename_me.smt");
    }

    // ────── completeness ──────

    /// Cross-check: the schema's top-level field count matches the
    /// 20+ fields enumerated in the research digest §1. A new field
    /// added without bumping this count is fine; *removing* one
    /// fires this test.
    #[test]
    fn top_level_field_set_is_complete() {
        // Exhaustive destructure of `MapInfo`. If the schema loses a
        // field, the pattern fails to compile.
        let info = MapInfo::bar_default();
        let MapInfo {
            name,
            shortname,
            description,
            author,
            version,
            mapfile,
            modtype,
            depend,
            replace,
            maphardness,
            not_deformable,
            gravity,
            tidal_strength,
            max_metal,
            extractor_radius,
            void_water,
            void_ground,
            void_alpha_min,
            auto_show_metal,
            smf,
            lighting,
            atmosphere,
            water,
            splats,
            resources,
            terrain_types,
            grass,
            teams,
            sound,
            custom,
        } = info;
        // Silence the unused-variable lints; we exist for the pattern
        // exhaustiveness check.
        let _ = (
            name,
            shortname,
            description,
            author,
            version,
            mapfile,
            modtype,
            depend,
            replace,
            maphardness,
            not_deformable,
            gravity,
            tidal_strength,
            max_metal,
            extractor_radius,
            void_water,
            void_ground,
            void_alpha_min,
            auto_show_metal,
            smf,
            lighting,
            atmosphere,
            water,
            splats,
            resources,
            terrain_types,
            grass,
            teams,
            sound,
            custom,
        );
    }

    #[test]
    fn bar_default_round_trips_through_toml() {
        // serde + forward-compat smoke: bar_default serialises and
        // deserialises losslessly. The `#[serde(default,
        // skip_serializing_if = ...)]` attributes are what make the
        // round-trip clean once we strip the runtime-only `_` fields.
        let info = MapInfo::bar_default();
        let s = toml::to_string(&info).expect("serialize");
        let back: MapInfo = toml::from_str(&s).expect("deserialize");
        assert_eq!(info, back);
    }

    /// MapSize is plumbed through `From<&Project>` indirectly — the
    /// schema doesn't carry map dims; the SMF binary owns them. This
    /// test pins that the schema does NOT include map dims so a
    /// future contributor doesn't add a redundant field.
    #[test]
    fn schema_does_not_duplicate_map_size() {
        let p = Project {
            name: "dims".to_string(),
            size: MapSize {
                smu_x: 16,
                smu_z: 18,
            },
            min_height: 0.0,
            max_height: 256.0,
            heightmap: None,
            ally_groups: vec![],
            mapinfo_overrides: HashMap::new(),
            next_steps_dismissed: false,
            splat_config: crate::SplatConfig::default(),
            splat_distribution: None,
            metal_spots: vec![],
            geo_vents: vec![],
            extractor_radius: crate::project::default_extractor_radius(),
        };
        let info: MapInfo = (&p).into();
        // No top-level smu/dims field exists; the SMF binary carries
        // the canonical map size. The schema only echoes name +
        // heights.
        assert_eq!(info.smf.min_height, Some(0.0));
        assert_eq!(info.smf.max_height, Some(256.0));
    }
}
