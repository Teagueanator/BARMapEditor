# Coastlines Dry — water-polish parity fixture

**Purpose:** Sprint 26 / R3 / ADR-044 reference for the BumpWater port.
Coastlines is the BAR ocean baseline — flat low-tide map with a wide
shallow coast band, ideal for verifying shore foam, fresnel at grazing
angles, caustics on the sea floor, and the basic refraction +
reflection composite.

## What this fixture exercises (Sprint 26 polish surface)

- **Surface tint** — `mapinfo.water.surfaceColor = (0.67, 0.80, 1.00)`,
  the cool blue Ocean preset anchor in `water_presets.rs::ocean()`.
- **Fresnel curve** — `fresnelMin/Max/Power = 0.2 / 1.6 / 8.0` produce
  the visible grazing-angle reflection brightening characteristic of
  Coastlines.
- **Shore foam** — `shoreWaves = true` and a substantial flat ocean
  floor below `min_height = -100` give the refraction-luma foam proxy
  (Sprint 26 commit 4) plenty of bright-coast to highlight.
- **Reflection** — small island clusters mid-map produce a clean
  silhouette in the reflection RT; verify the mirrored image aligns
  with the above-water geometry at moderate orbit angles.
- **Caustics** — the wide shallow band makes the procedural sine
  caustics visible without needing a deep-water comparison.

## How to use (manual smoke until Sprint 36 automates)

1. Load Coastlines's `mapinfo.lua` values into a fresh project:
   - `water_mode = WaterMode::Ocean`
   - `min_height = -100`
   - `height_scale = 256`
2. Render in the editor at three standard angles:
   - **Top-down (pitch ≈ 88°)** — caustics + refracted seafloor
     should read clean; reflection contributes little.
   - **35° pitch** — fresnel becomes visible on water near the
     horizon; reflection of any in-frame terrain should be
     recognisable.
   - **Grazing (pitch ≈ 10°)** — reflection dominates; foam edge
     between water and any above-water island reads as a soft
     bright band.
3. Compare against BAR's render of Coastlines at the same camera
   angles. **Acceptance:** mean ΔE < 5.0 across all three camera
   angles (Sprint 36 will automate this; today it's eyeball).

## Known divergences (Sprint 26 — documented in ADR-044)

- The water normal map is procedural (Quilez 2D hash + 4-octave fbm)
  rather than BAR's vendored `waterbump.png`. The wave shape will
  differ subtly — same frequency content, different specific phase.
- Foam intensity proxies through the refraction-sample luma rather
  than a precomputed coastmap. Bright sand reads as "shore" even
  when offshore. Sprint 27 / R4 candidate for the coastmap bake.
- Caustics are a two-octave sine pattern, not BAR's 32-jpg cycle.
  Visual rhythm differs; brightness band roughly matches.

## Provenance

- **Map:** Coastlines Dry, distributed via BAR's certified maps list.
- **mapinfo.lua source:** sample the Coastlines `.sd7` from BAR's
  rapid feed; the `water = { … }` block in
  `crates/barme-core/src/water_presets.rs::ocean()` is the verbatim
  baseline used by Sprint 14 / ADR-042.

## What Sprint 36 will automate

- Headless render at the three reference angles.
- Pixel-wise ΔE diff against a BAR-engine screenshot of the same
  fixture (rendered via `recoil --isolation` against the same
  mapinfo).
- Pass/fail at mean ΔE < 5.0 (the renderer-parity arc's target).
