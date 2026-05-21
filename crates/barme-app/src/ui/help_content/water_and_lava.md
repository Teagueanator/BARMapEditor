# Water and Lava (reference)

Cross-tool reference for the Sprint 14 / ADR-042 water + lava
authoring model. The **Water / Lava** tool article covers the
in-place workflow.

## Data model

```rust
enum WaterMode {
    None,
    Ocean,
    Tropical,
    Acid,
    Lava,
    Magma,
    Custom,
}

struct Project {
    water_mode: WaterMode,
    water_overrides: WaterBlock,  // sparse Option<…>
    void_water: bool,
    tidal_strength: f32,
    min_height: f32,
    // …
}
```

`From<&Project> for MapInfo` builds the emitted `water` block as
`merge_overrides(preset_water_block(mode), water_overrides)`.
Sparse overrides persist across preset changes (Photoshop-style).

## The flat water plane

BAR's `Ground.h::GetWaterPlaneLevel` is `consteval` and returns
`0.0` (PITFALL §28). You cannot raise or lower the water plane;
you raise or lower terrain instead. `Project.min_height < 0`
puts the lowest sample below the water plane, and the basin
floods on render.

The terrain shader's `sample_y` MUST consult `min_height` when
projecting raw heightmap values to world Y — otherwise the
"flood a basin" workflow looks broken even with the right data
(catastrophic 2026-05-19 regression caught + pinned by the C9
inspector emission tests + the terrain shader uniform pin).

## void_water vs water.planeColor

`void_water = true` produces the popular "space map" look — no
water plane rendered (Apophis, Quicksilver). It MUST clear
`water.planeColor` (PITFALL §6); the editor warns and
auto-clears.

## Lava atmosphere offer

The Inspector's Lava / Magma presets surface an Apply
lava-atmosphere button — patches fog, sun, and cloud to warm
tones. The atmosphere patch is a `ProjectDiff::SetLavaAtmosphere`,
fully undoable.
