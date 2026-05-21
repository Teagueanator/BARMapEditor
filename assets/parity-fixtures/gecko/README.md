# Gecko Isle Remake — tropical-water parity fixture

**Purpose:** Sprint 26 / R3 / ADR-044 reference for the green-tropical
preset variant. Gecko Isle is the BAR Tropical anchor — a tidal map
with a more turbid green water palette, ideal for verifying that the
surface tint + alpha composite reads correctly when the refraction
underneath is heavily green-shifted.

## What this fixture exercises

- **Tropical surface tint** — `surfaceColor = (0.30, 0.65, 0.25)`,
  green-leaning compared to Coastlines's blue. The
  `water_presets.rs::tropical()` baseline.
- **Higher surface alpha** — `surfaceAlpha = 0.15` (vs Ocean's 0.10),
  giving a more opaque surface that veils the refraction.
- **Tidal variation** — Gecko's tidal range is wider than Coastlines;
  test the surface composite at different submerged heights.
- **Reflection over irregular geometry** — Gecko has steeper island
  silhouettes than Coastlines; verify the reflection RT captures
  vertical cliff faces without z-fighting.

## How to use (manual smoke until Sprint 36 automates)

1. Load Gecko's `mapinfo.lua` values into a fresh project:
   - `water_mode = WaterMode::Tropical`
   - `min_height = -180`
   - `height_scale = 280`
2. Render at the same three reference angles as Coastlines (top-down,
   35° pitch, grazing).
3. Verify:
   - The surface tint reads visibly more green-saturated than
     Coastlines's blue.
   - Reflection of cliff faces aligns with the above-water cliff
     geometry under rotation.
   - Fresnel at grazing angles produces the same kind of brightening
     as Coastlines (the fresnel triple is the same).

## Acceptance

- Same as Coastlines: eyeball pass today, Sprint 36 ΔE < 5.0 automated.

## Known divergences

- Same procedural-normal-map caveat as Coastlines.
- Same foam-proxy caveat — Gecko's underwater terrain is more
  varied; the foam proxy may produce stripes where bright underwater
  features cluster.

## Provenance

- **Map:** Gecko Isle Remake.
- **mapinfo source:** the `water = { … }` block transcribed into
  `crates/barme-core/src/water_presets.rs::tropical()`. Per FINDINGS
  §1.5, the engine reads the same key set.
