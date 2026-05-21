# Lava sample — emission-glow parity fixture

**Purpose:** Sprint 26 / R3 / ADR-044 reference for the Lava + Magma
emission branch (commit 5). The lava water mode's defining visual
feature is self-illumination: the surface glows even under low
ambient light, modulated by the caustic field for the characteristic
pulsating-magma effect. This fixture isolates that path.

## What this fixture exercises

- **Lava preset** — `damage = 1000` (at the ground-block threshold),
  `surfaceColor = (1.0, 0.4, 0.1)` orange-red, the
  `water_presets.rs::lava()` baseline.
- **Magma preset variant** — `damage = 5000`, `surfaceColor = (1.0,
  0.2, 0.05)` deeper red, the `water_presets.rs::magma()` variant.
- **Lava-atmosphere offer** — the inspector's one-click patch
  (red-orange fog, dim warm sun, dusty clouds @ 0.7) applied on top.
- **Hardcoded daylight factor** — Sprint 26 ships `0.5`; Sprint 28
  will replace with `dot(sun_dir, world_up)` so the glow ramps to
  max at night.
- **Reflection branch under emission** — the lava emission is added
  on top of the fresnel-mixed refraction/reflection composite;
  verify the reflection of any above-water terrain doesn't drown
  out the underlying glow.

## How to use (manual smoke until Sprint 36 automates)

1. Load these values into a fresh project:
   - `water_mode = WaterMode::Lava`
   - `min_height = -120`
   - `lava_atmosphere = true` (apply the patch via the offer card)
2. Sculpt a basin or pit and verify the lava surface glows
   independently of camera angle (rotate around it; the brightness
   stays constant — it's emissive, not lit).
3. Switch to `WaterMode::Magma` and confirm the deeper-red shift.
4. Toggle `lava_atmosphere` on and off; the lava surface itself
   doesn't change (atmosphere modifies fog + sun, not the water
   shader's emission branch).

## Acceptance

- Lava-mode surface visibly self-illuminates against a dark
  background.
- Caustic field modulates the brightness over time — the surface
  pulses softly.
- No NaN / oversaturated highlights at grazing angles (clamps in
  the fresnel branch prevent the PITFALL #6 pow-of-negative).

## Known divergences

- BAR's actual lava maps use additional gadget-side effects
  (particle plumes, screen-shake on damage tick) that the editor
  doesn't reproduce — out of scope for renderer parity.
- The hardcoded `0.5` daylight factor means the emission isn't
  day/night-aware until Sprint 28.
- Shadows (Sprint 30) will require updating the emission branch to
  inhibit under cast shadows. The `lit = false` semantic hook lives
  in the shader as a comment marker until Sprint 30 wires it.

## Provenance

- **Map source:** synthesised values from `water_presets.rs::lava()` /
  `magma()`. The BAR community doesn't ship a canonical "lava sample
  map" today; user-made lava maps anchor to these damage thresholds
  (`MoveDefHandler.cpp:81-160`).
