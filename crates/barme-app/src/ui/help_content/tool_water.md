# Water / Lava

Map-property tool — picks a water preset and patches `mapinfo`'s
`water` block. LMB-drag floods (calls `Brush::Lower` with a
strength derived from `water_carve_depth`); RMB-drag raises
terrain back.

## Presets

Seven presets, each anchored to a real BAR map:

- **None** — engine renders default blue ocean if
  `min_height < 0`; otherwise no water visible.
- **Custom** — hand-tuned overrides.
- **Ocean** — cool blue, `Coastlines` palette.
- **Tropical** — bright teal shallows with strong foam.
- **Acid** — sickly green, high damage.
- **Lava** — orange-red surface, high damage.
- **Magma** — lava with thicker fog and dimmer sun.

The Inspector exposes Behaviour (`damage`, `void_water`,
`tidal_strength`), Appearance (surface + plane colour, alpha,
wave size, foam strength), Flood (carve depth + auto-min-height
shortcut), and an Advanced placeholder for the F9 raw-fields
backstop.

## The flat water plane

BAR's water plane is `consteval` and pinned at `Y = 0`
(PITFALL §28). You CAN'T move it; you raise or lower `min_height`
so the terrain dips below sea level. The auto-min-height button
sets `min_height = min(0, carve_depth)` so the first flood
gesture immediately produces visible water in the carved basin.

## Lava atmosphere

For Lava / Magma presets the Inspector offers an "Apply
lava-style atmosphere" button — patches fog (warm orange), sun
(warm), and cloud (dim). The atmosphere patch is independent of
the preset so you can stack it on a custom water block.

Polished water rendering (fresnel, foam, caustics, perlin wave
motion) is the renderer-parity arc's job (Sprint 26+); the MVP
flat tinted plane is enough to read whether water + terrain are
in agreement.
