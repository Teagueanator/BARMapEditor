# Foggy-map — exponential height fog parity fixture

**Purpose:** Sprint 28 / R2 / ADR-045 reference for the exponential
height fog blend in `terrain.wgsl` fragment stage. Verifies that
terrain at far range tints toward `atmosphere.fog_color` and that
altitude thins the fog (a mountain peak reads sharper than a valley
floor at the same horizontal distance).

## What this fixture exercises

- **Dense fog tuning** — `fogStart = 0.1`, `fogEnd = 1.0`,
  `fogColor = (0.4, 0.5, 0.7)` (cool grey-blue). Cranks the density
  high enough that the haze is obvious at default 16-SMU framing.
- **Exponential height falloff** — the `height_falloff = 0.01`
  coefficient drops the fog blend factor with altitude. Sample
  altitudes 0 / 50 / 100 elmos to confirm the curve.
- **Smoothstep clamp** — `fog_start == fog_end` would degenerate to
  a binary step (Sprint 21 lint catches it as a hard error, but the
  shader's `smoothstep` clamps to [0, 1] without NaN as a defensive
  fallback). Edit `fog_start` to match `fog_end` and confirm no
  crash.
- **Per-frame uniform write** — `App::atmosphere_uniforms_for_render`
  is the single CPU-side mapping point; verify that edits via the F9
  form's Atmosphere tab take effect on the next frame without an
  app restart.

## Suggested project settings

- **Map size:** 16 × 16 SMU (`MapSize::SMU { x: 16, z: 16 }`).
- **Min height:** −50 elmos (creates a basin at the centre that
  reads as the foggiest area).
- **Max height:** 300 elmos (peaks above the fog curve).
- **MapInfo overrides:**
  ```lua
  atmosphere = {
    fogStart = 0.1,
    fogEnd = 1.0,
    fogColor = { 0.4, 0.5, 0.7 },
    skyColor = { 0.7, 0.75, 0.85 }, -- distinct from fog (pitfall #6)
    sunColor = { 1.0, 0.95, 0.85 },
    minWind  = 5,
    maxWind  = 25,
    cloudColor = { 0.95, 0.95, 1.0 },
    cloudDensity = 0.6,
  }
  ```
- **Lighting:** keep `MapInfo::bar_default()` (sun direction
  `(0.3, 1.0, -0.2)` normalised; ambient + diffuse `(0.5, 0.5, 0.5)`).

## How to use (manual smoke until Sprint 36 automates)

1. Load these values into a fresh 16-SMU project (the F9 form is
   the path; edits commit through `MapInfoPatch` and survive Ctrl-Z).
2. Orbit to a far-edge view; confirm the terrain past `dist_norm ≈
   0.5` blends toward the cool grey-blue fog tint.
3. Sample the highest peak; confirm it reads sharper (less fogged)
   than the basin floor at the same horizontal distance.
4. Toggle `fog_start = 1.0` to match `fog_end`; confirm the fog goes
   to a binary in/out without crashing (smoothstep clamp working).
5. Reset `fog_start = 0.1`.

## Acceptance

- Distance haze visible at the horizon (terrain colour blends ~60 %
  toward `fog_color` at the far plane per the 0.6 density default
  in `AtmosphereUniforms`).
- Altitude thinning visible: a 300-elmo peak at the same XZ as a
  valley floor reads more sharply contrasted.
- Sky background past terrain shows `sky_color` (not the legacy
  navy `[0.04, 0.05, 0.07]`).
- No NaN / crash when `fog_start == fog_end` (lint warns; shader
  defensively clamps).

## Out of scope (deferred to a future sprint)

- **Skybox cubemap** — `atmosphere.sky_box` ignored this sprint per
  ADR-045. The cubemap-aware sky-pass replaces the plain
  `sky_color` clear when the deferred-cubemap sprint lands.
- **Volumetric clouds** — `cloud_density` is plumbed but the shader
  doesn't modulate by it.
- **Day/night cycle animation** — sun direction is static per project.
