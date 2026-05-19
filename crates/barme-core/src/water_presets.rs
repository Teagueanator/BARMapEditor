//! Water / lava presets driving `mapinfo.water` emission (C9 / ADR-042).
//!
//! BAR ships exactly one water plane at `Y = 0` (`Ground.h::GetWaterPlaneLevel`
//! is `consteval 0.0f`). Every "lava map" / "acid map" / "ocean" in the
//! wild is the same flat plane with a different `water = { … }` block.
//! `WaterMode` selects which preset baseline the editor uses;
//! `WaterBlock` overrides on top of that preset surface the per-field
//! tweaks the user makes in the Sprint-14 inspector form.
//!
//! ## Forward-compat
//!
//! [`WaterMode`] carries `#[serde(other)]` on `Custom`, so a future
//! preset (e.g. `Geyser`) loaded by an older editor degrades to
//! `Custom` instead of panicking. The user keeps their overrides;
//! switching back to the new mode in a newer editor is lossless.
//!
//! ## Preset citations
//!
//! Each non-None preset is anchored to a real BAR map. The literal
//! values were extracted from those maps' `mapinfo.lua` blocks (see
//! `devlog/research-water-lava/logs/2026-05-19T10-59-15__water-lava-engine-research.md`):
//!
//! | Preset    | Anchor                     | `damage` | `surface_color`     |
//! |-----------|----------------------------|----------|---------------------|
//! | Ocean     | Coastlines Dry             | 0        | (0.67, 0.80, 1.00)  |
//! | Tropical  | Gecko Isle Remake (surface)| 0        | (0.30, 0.65, 0.25)  |
//! | Acid      | Acidic Quarry              | 200      | (0.65, 0.80, 0.10)  |
//! | Lava      | (synth, Lava family)       | 1000     | (1.00, 0.40, 0.10)  |
//! | Magma     | (synth, Lava family)       | 5000     | (1.00, 0.20, 0.05)  |
//!
//! Damage thresholds (`Sim/MoveTypes/MoveDefHandler.cpp:81-160`):
//! - `>= 1e3` → `waterDamageCost = 0` (ground units blocked).
//! - `>= 1e4` → `noHoverWaterMove = true` (hovers blocked too).
//!
//! Lava sits at `1000` — exactly at the ground-block threshold, hovers
//! still cross. Magma sits at `5000` — well above ground-block, still
//! below the hover-block ceiling so amphibious gameplay survives.

use serde::{Deserialize, Serialize};

use crate::mapinfo_schema::WaterBlock;

/// The active water preset for a project.
///
/// Drives `From<&Project> for MapInfo`:
/// - [`WaterMode::None`] emits no `water = { … }` sub-table; the engine
///   uses its built-in `BasicWater` defaults (flat blue ocean) if the
///   terrain dips below 0.
/// - Any other variant emits the corresponding preset's `WaterBlock`,
///   per-field merged with `Project.water_overrides` (override fields
///   win; unset overrides fall through to the preset).
///
/// **Forward-compat:** `#[serde(other)]` on `Custom` means an unknown
/// preset string in a `.barmeproj` decodes as `Custom` rather than
/// failing — overrides survive across editor versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum WaterMode {
    /// No water sub-table emitted. Lints fire if `min_height < 0`
    /// (engine will render its default blue ocean — usually
    /// unintentional).
    #[default]
    None,
    /// Coastlines-style cool blue ocean. Surface (0.67, 0.80, 1.00),
    /// shore waves on, no damage.
    Ocean,
    /// Greener, more turbid tropical ocean. Surface (0.30, 0.65, 0.25),
    /// shore waves on, no damage.
    Tropical,
    /// Yellow-green damaging pools. Anchored to *Acidic Quarry*.
    /// `damage = 200` — visibly hurts but doesn't insta-kill.
    Acid,
    /// Orange-red hot liquid. `damage = 1000` — at the ground-block
    /// threshold (units cannot enter). Hovers still cross.
    Lava,
    /// Deep red molten rock. `damage = 5000` — deeply blocks ground,
    /// stays just under the `1e4` hover-block ceiling. Use sparingly:
    /// players' hover units can cross strategically but ground armies
    /// are utterly excluded.
    Magma,
    /// All overrides bleed through; preset baseline is empty. Drives
    /// the "Custom (N overrides)" chip in the inspector and unlocks
    /// the Advanced (raw-fields) disclosure.
    #[serde(other)]
    Custom,
}

impl WaterMode {
    /// Every variant in canonical inspector-chip order.
    pub const ALL: [WaterMode; 7] = [
        WaterMode::None,
        WaterMode::Ocean,
        WaterMode::Tropical,
        WaterMode::Acid,
        WaterMode::Lava,
        WaterMode::Magma,
        WaterMode::Custom,
    ];

    /// Display label for the inspector chip and tracing.
    pub fn label(self) -> &'static str {
        match self {
            WaterMode::None => "None",
            WaterMode::Ocean => "Ocean",
            WaterMode::Tropical => "Tropical",
            WaterMode::Acid => "Acid",
            WaterMode::Lava => "Lava",
            WaterMode::Magma => "Magma",
            WaterMode::Custom => "Custom",
        }
    }
}

/// Get the baseline [`WaterBlock`] for a preset. `None` for
/// [`WaterMode::None`] and [`WaterMode::Custom`] — those modes either
/// emit no water block (`None`) or rely entirely on
/// `Project.water_overrides` (`Custom`).
pub fn preset_water_block(mode: WaterMode) -> Option<WaterBlock> {
    match mode {
        WaterMode::None => None,
        WaterMode::Ocean => Some(ocean()),
        WaterMode::Tropical => Some(tropical()),
        WaterMode::Acid => Some(acid()),
        WaterMode::Lava => Some(lava()),
        WaterMode::Magma => Some(magma()),
        // Custom carries no preset — the merge skips this branch and
        // only the user's overrides land in `info.water`. If the user
        // hasn't authored anything, `merged` is `WaterBlock::default()`
        // (all `None`) and the emitter writes an empty `water = {}`
        // table. That's the user's choice; lint fires.
        WaterMode::Custom => Some(WaterBlock::default()),
    }
}

/// Per-field merge: `override.field.or(preset.field)`. Returns a new
/// `WaterBlock` where each field is the override value when the user
/// has set one and the preset value otherwise.
///
/// Switching presets keeps the user's `water_overrides` intact, so a
/// tweaked `damage = 30` rides through preset changes — matches the
/// Photoshop-style behaviour the research report argues for.
pub fn merge_overrides(preset: &WaterBlock, overrides: &WaterBlock) -> WaterBlock {
    WaterBlock {
        damage: overrides.damage.or(preset.damage),
        repeat_x: overrides.repeat_x.or(preset.repeat_x),
        repeat_y: overrides.repeat_y.or(preset.repeat_y),
        surface_color: overrides.surface_color.or(preset.surface_color),
        surface_alpha: overrides.surface_alpha.or(preset.surface_alpha),
        plane_color: overrides.plane_color.or(preset.plane_color),
        absorb: overrides.absorb.or(preset.absorb),
        base_color: overrides.base_color.or(preset.base_color),
        min_color: overrides.min_color.or(preset.min_color),
        ambient_factor: overrides.ambient_factor.or(preset.ambient_factor),
        diffuse_factor: overrides.diffuse_factor.or(preset.diffuse_factor),
        specular_factor: overrides.specular_factor.or(preset.specular_factor),
        specular_color: overrides.specular_color.or(preset.specular_color),
        specular_power: overrides.specular_power.or(preset.specular_power),
        fresnel_min: overrides.fresnel_min.or(preset.fresnel_min),
        fresnel_max: overrides.fresnel_max.or(preset.fresnel_max),
        fresnel_power: overrides.fresnel_power.or(preset.fresnel_power),
        reflection_distortion: overrides
            .reflection_distortion
            .or(preset.reflection_distortion),
        blur_base: overrides.blur_base.or(preset.blur_base),
        blur_exponent: overrides.blur_exponent.or(preset.blur_exponent),
        perlin_start_freq: overrides.perlin_start_freq.or(preset.perlin_start_freq),
        perlin_lacunarity: overrides.perlin_lacunarity.or(preset.perlin_lacunarity),
        perlin_amplitude: overrides.perlin_amplitude.or(preset.perlin_amplitude),
        wave_foam_intensity: overrides.wave_foam_intensity.or(preset.wave_foam_intensity),
        num_tiles: overrides.num_tiles.or(preset.num_tiles),
        shore_waves: overrides.shore_waves.or(preset.shore_waves),
        force_rendering: overrides.force_rendering.or(preset.force_rendering),
        texture: overrides.texture.clone().or_else(|| preset.texture.clone()),
        foam_texture: overrides
            .foam_texture
            .clone()
            .or_else(|| preset.foam_texture.clone()),
        normal_texture: overrides
            .normal_texture
            .clone()
            .or_else(|| preset.normal_texture.clone()),
        caustics: if overrides.caustics.is_empty() {
            preset.caustics.clone()
        } else {
            overrides.caustics.clone()
        },
    }
}

/// BAR-default `surface_color` used by the editor's water plane when
/// no preset is active (i.e. the value the engine would composite
/// against if the user shipped `water = {}` empty).
pub const BAR_DEFAULT_SURFACE_COLOR: [f32; 3] = [0.75, 0.80, 0.85];

/// BAR-default `surface_alpha` for the same fallback.
pub const BAR_DEFAULT_SURFACE_ALPHA: f32 = 0.1;

// ─────── Preset bodies (anchored to real BAR maps) ───────

/// Coastlines Dry verbatim — the BAR ocean baseline.
fn ocean() -> WaterBlock {
    WaterBlock {
        damage: Some(0.0),
        absorb: Some([0.05, 0.005, 0.001]),
        base_color: Some([0.3, 0.5, 0.5]),
        min_color: Some([0.0, 0.3, 0.3]),
        surface_color: Some([0.67, 0.8, 1.0]),
        surface_alpha: Some(0.1),
        // Coastlines comments out planeColor → ordinary deep ocean.
        plane_color: None,
        ambient_factor: Some(1.0),
        diffuse_factor: Some(1.0),
        specular_factor: Some(1.4),
        specular_power: Some(40.0),
        fresnel_min: Some(0.2),
        fresnel_max: Some(1.6),
        fresnel_power: Some(8.0),
        reflection_distortion: Some(1.0),
        perlin_start_freq: Some(8.0),
        perlin_lacunarity: Some(3.0),
        perlin_amplitude: Some(0.9),
        shore_waves: Some(true),
        force_rendering: Some(false),
        ..WaterBlock::default()
    }
}

/// Gecko Isle Remake surface tint over a Coastlines-like baseline.
/// Greener, more turbid — the report calls out the surface RGB
/// `(0.30, 0.65, 0.25)`.
fn tropical() -> WaterBlock {
    WaterBlock {
        damage: Some(0.0),
        absorb: Some([0.04, 0.005, 0.04]),
        base_color: Some([0.2, 0.5, 0.3]),
        min_color: Some([0.0, 0.25, 0.1]),
        surface_color: Some([0.30, 0.65, 0.25]),
        // Slightly more opaque than ocean — tropical water reads more
        // turbid in BAR maps that use this palette.
        surface_alpha: Some(0.15),
        plane_color: None,
        ambient_factor: Some(1.0),
        diffuse_factor: Some(1.0),
        specular_factor: Some(1.2),
        specular_power: Some(40.0),
        fresnel_min: Some(0.2),
        fresnel_max: Some(1.6),
        fresnel_power: Some(8.0),
        reflection_distortion: Some(1.0),
        perlin_start_freq: Some(8.0),
        perlin_lacunarity: Some(3.0),
        perlin_amplitude: Some(0.9),
        shore_waves: Some(true),
        force_rendering: Some(false),
        ..WaterBlock::default()
    }
}

/// Acidic Quarry verbatim — yellow-green damaging pools.
fn acid() -> WaterBlock {
    WaterBlock {
        damage: Some(200.0),
        absorb: Some([0.0074, 0.0090, 0.0035]),
        base_color: Some([0.84, 0.90, 0.4]),
        min_color: Some([0.13, 0.12, 0.01]),
        surface_color: Some([0.65, 0.8, 0.1]),
        surface_alpha: Some(0.4),
        plane_color: Some([0.024, 0.03, 0.1]),
        specular_color: Some([0.2, 0.3, 0.1]),
        specular_power: Some(50.0),
        fresnel_min: Some(0.1),
        fresnel_max: Some(0.8),
        fresnel_power: Some(4.0),
        perlin_start_freq: Some(2.0),
        perlin_lacunarity: Some(3.0),
        perlin_amplitude: Some(0.95),
        shore_waves: Some(true),
        force_rendering: Some(false),
        ..WaterBlock::default()
    }
}

/// Synthesized Lava preset. Damage sits at the `>= 1e3` ground-block
/// threshold (units cannot enter). Hovers still cross.
fn lava() -> WaterBlock {
    WaterBlock {
        damage: Some(1000.0),
        absorb: Some([0.1, 0.05, 0.01]),
        base_color: Some([0.6, 0.2, 0.05]),
        min_color: Some([0.2, 0.05, 0.0]),
        surface_color: Some([1.0, 0.4, 0.1]),
        surface_alpha: Some(0.6),
        plane_color: Some([0.5, 0.1, 0.0]),
        specular_color: Some([1.0, 0.4, 0.1]),
        specular_power: Some(30.0),
        // Lava reflects very little — soft sub-1 fresnel.
        fresnel_min: Some(0.0),
        fresnel_max: Some(0.5),
        fresnel_power: Some(2.0),
        // Slow, gentle waves — lava is viscous.
        perlin_start_freq: Some(1.0),
        perlin_lacunarity: Some(2.0),
        perlin_amplitude: Some(0.3),
        shore_waves: Some(false),
        force_rendering: Some(false),
        ..WaterBlock::default()
    }
}

/// Synthesized Magma preset. Damage stays under the `1e4` hover-block
/// ceiling so amphibious gameplay survives — strategic hover crosses
/// are still possible.
fn magma() -> WaterBlock {
    WaterBlock {
        damage: Some(5000.0),
        absorb: Some([0.2, 0.15, 0.05]),
        base_color: Some([0.4, 0.1, 0.02]),
        min_color: Some([0.15, 0.03, 0.0]),
        surface_color: Some([1.0, 0.2, 0.05]),
        surface_alpha: Some(0.8),
        plane_color: Some([0.3, 0.05, 0.0]),
        specular_color: Some([1.0, 0.3, 0.05]),
        specular_power: Some(30.0),
        fresnel_min: Some(0.0),
        fresnel_max: Some(0.5),
        fresnel_power: Some(2.0),
        perlin_start_freq: Some(0.5),
        perlin_lacunarity: Some(2.0),
        perlin_amplitude: Some(0.4),
        shore_waves: Some(false),
        force_rendering: Some(false),
        ..WaterBlock::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn water_mode_default_is_none() {
        assert_eq!(WaterMode::default(), WaterMode::None);
    }

    #[test]
    fn water_mode_all_is_complete() {
        assert_eq!(WaterMode::ALL.len(), 7);
        let mut seen = std::collections::HashSet::new();
        for m in WaterMode::ALL {
            assert!(seen.insert(m), "duplicate variant in ALL: {m:?}");
        }
    }

    #[test]
    fn unknown_serde_variant_falls_back_to_custom() {
        // Serde's #[serde(other)] on `Custom` makes a future "Geyser"
        // preset decode as Custom rather than crashing — forward-compat
        // contract per the prompt's critical-pitfall list.
        #[derive(Deserialize)]
        struct Wrap {
            mode: WaterMode,
        }
        let w: Wrap = toml::from_str(r#"mode = "Geyser""#).unwrap();
        assert_eq!(w.mode, WaterMode::Custom);
    }

    #[test]
    fn known_variants_round_trip() {
        #[derive(Serialize, Deserialize)]
        struct Wrap {
            mode: WaterMode,
        }
        for m in WaterMode::ALL {
            let s = toml::to_string(&Wrap { mode: m }).unwrap();
            let back: Wrap = toml::from_str(&s).unwrap();
            assert_eq!(m, back.mode, "{m:?} did not round-trip");
        }
    }

    #[test]
    fn preset_water_block_none_returns_none() {
        assert!(preset_water_block(WaterMode::None).is_none());
    }

    #[test]
    fn ocean_preset_carries_coastlines_surface_color() {
        let w = preset_water_block(WaterMode::Ocean).unwrap();
        assert_eq!(w.surface_color, Some([0.67, 0.8, 1.0]));
        assert_eq!(w.damage, Some(0.0));
        assert_eq!(w.surface_alpha, Some(0.1));
        // Ocean has no planeColor — Coastlines comments it out.
        assert!(w.plane_color.is_none());
    }

    #[test]
    fn acid_preset_carries_acidic_quarry_damage_and_surface() {
        let w = preset_water_block(WaterMode::Acid).unwrap();
        assert_eq!(w.damage, Some(200.0));
        assert_eq!(w.surface_color, Some([0.65, 0.8, 0.1]));
        assert_eq!(w.surface_alpha, Some(0.4));
    }

    #[test]
    fn lava_sits_at_ground_block_threshold() {
        // Per MoveDefHandler.cpp:81-160: damage >= 1e3 blocks ground;
        // >= 1e4 blocks hovers. Lava: 1000 (block ground, allow hover).
        let w = preset_water_block(WaterMode::Lava).unwrap();
        assert_eq!(w.damage, Some(1000.0));
        assert!(w.damage.unwrap() >= 1e3, "lava must block ground");
        assert!(w.damage.unwrap() < 1e4, "lava must NOT block hovers");
    }

    #[test]
    fn magma_blocks_ground_but_allows_strategic_hover_crossing() {
        // Magma: 5000 (deeply block ground, still under hover-block).
        let w = preset_water_block(WaterMode::Magma).unwrap();
        assert_eq!(w.damage, Some(5000.0));
        assert!(w.damage.unwrap() >= 1e3);
        assert!(
            w.damage.unwrap() < 1e4,
            "PITFALL: damage >= 1e4 blocks hovers — keep BAR amphib gameplay viable"
        );
    }

    #[test]
    fn custom_preset_is_empty_water_block() {
        // Custom relies on user overrides; the preset itself contributes
        // nothing. Without overrides, the emit would produce an empty
        // `water = {}` table; lint surfaces the issue.
        let w = preset_water_block(WaterMode::Custom).unwrap();
        assert_eq!(w, WaterBlock::default());
    }

    #[test]
    fn merge_falls_back_to_preset_when_overrides_empty() {
        let preset = preset_water_block(WaterMode::Ocean).unwrap();
        let merged = merge_overrides(&preset, &WaterBlock::default());
        assert_eq!(merged, preset);
    }

    #[test]
    fn merge_lets_overrides_win_per_field() {
        let preset = preset_water_block(WaterMode::Ocean).unwrap();
        let overrides = WaterBlock {
            damage: Some(30.0),
            surface_alpha: Some(0.5),
            ..WaterBlock::default()
        };
        let merged = merge_overrides(&preset, &overrides);
        // Override fields survive.
        assert_eq!(merged.damage, Some(30.0));
        assert_eq!(merged.surface_alpha, Some(0.5));
        // Untouched fields still carry Ocean's values.
        assert_eq!(merged.surface_color, preset.surface_color);
        assert_eq!(merged.fresnel_min, preset.fresnel_min);
    }

    /// Critical pitfall: switching presets must NOT clobber user
    /// overrides. The data path lets overrides bleed through Ocean →
    /// Acid → Magma; the inspector layer only mutates `water_mode`,
    /// not `water_overrides`.
    #[test]
    fn overrides_persist_across_preset_changes() {
        let overrides = WaterBlock {
            surface_alpha: Some(0.9),
            ..WaterBlock::default()
        };
        let m_ocean = merge_overrides(&preset_water_block(WaterMode::Ocean).unwrap(), &overrides);
        let m_acid = merge_overrides(&preset_water_block(WaterMode::Acid).unwrap(), &overrides);
        assert_eq!(m_ocean.surface_alpha, Some(0.9));
        assert_eq!(m_acid.surface_alpha, Some(0.9));
        // Per-preset fields swap correctly.
        assert_eq!(m_ocean.damage, Some(0.0));
        assert_eq!(m_acid.damage, Some(200.0));
    }
}
