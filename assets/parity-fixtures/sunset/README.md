# Sunset — sun-colour angle ramp parity fixture

**Purpose:** Sprint 28 / R2 / ADR-045 reference for the sun-colour
angle ramp in `terrain.wgsl` fragment stage and the lava daylight
ramp in `water.wgsl`. Verifies that a low sun (`sun_dir.y ≈ 0.1..
0.3`) produces a warm-tinted ground (`fog_color` end of the
`mix(fog_color, ground_diffuse, sun_angle_factor)` blend) and that
the water shader's lava emission brightens at low sun.

## What this fixture exercises

- **Low sun direction** — `lighting.sun_dir = (0.95, 0.2, 0.05)`
  normalised. The `sun_angle_factor = clamp(sun_dir.y, 0, 1)` lands
  at ~0.2 — the lit terrain takes ~80 % of its colour from
  `atmosphere.fog_color` (sunset glow) and only ~20 % from
  `lighting.ground_diffuse_color` (full-daylight white).
- **Warm fog tone** — `fogColor = (0.9, 0.6, 0.3)` (warm orange);
  the terrain's lit side blends toward this at the sun's altitude.
- **Lava daylight under low sun** — `daylight = pow(1 - sun_angle_factor,
  0.7)` evaluates to ~0.86 at `sun_angle_factor = 0.2`. Lava lights
  up almost as brightly as at midnight. (Compare against a high-sun
  fixture where `daylight ≈ 0.3` and lava dims under direct
  illumination.)
- **Per-fragment Blinn-Phong + ramped diffuse** — the Sprint 25
  specular term is unchanged; only the diffuse path inherits the
  warm tint. Confirm specular highlights stay neutral white (not
  warm-tinted).

## Suggested project settings

- **Map size:** 8 × 8 SMU (enough terrain to show the gradient
  without overpowering the sunset framing).
- **Min height:** 0 elmos.
- **Max height:** 200 elmos (a moderate ridge for the lit/shadowed
  contrast).
- **MapInfo overrides:**
  ```lua
  lighting = {
    sunDir = { 0.95, 0.2, 0.05, 1.0 },  -- low sun, near horizon
    groundAmbientColor  = { 0.5, 0.5, 0.5 },
    groundDiffuseColor  = { 1.0, 0.95, 0.9 }, -- slight warm cast
    groundSpecularColor = { 0.1, 0.1, 0.1 },
    specularExponent    = 100.0,
  }
  atmosphere = {
    fogStart  = 0.2,
    fogEnd    = 1.0,
    fogColor  = { 0.9, 0.6, 0.3 },   -- warm orange (sunset glow)
    skyColor  = { 0.85, 0.55, 0.3 }, -- matching sunset sky
    sunColor  = { 1.0, 0.9, 0.7 },
    minWind   = 5,
    maxWind   = 15,
    cloudColor = { 0.95, 0.7, 0.5 },
    cloudDensity = 0.4,
  }
  ```
- **Water mode:** Lava (so the daylight ramp's effect on emission
  is visible). Set `min_height = -20` to expose a small lava basin.

## How to use (manual smoke until Sprint 36 automates)

1. Load the project. Orbit to face the sun direction (camera
   roughly `+X +Y -Z` looking back toward `-X`); the lit side of
   ridges should read warm.
2. Compare with a fresh `MapInfo::bar_default()` project (sun
   `(0.3, 1.0, -0.2)` normalised — high sun). The lit terrain on
   the default project reads neutral white; the sunset fixture
   reads warm orange-pink.
3. Confirm specular highlights on the ridge tops stay neutral
   (the ramp only modulates the diffuse path, not specular).
4. Orbit to the lava basin; confirm the glow is near-maximum
   brightness under the low sun. Switch the project to
   `MapInfo::bar_default()` lighting and observe the lava dim by
   roughly 3×.

## Acceptance

- Lit terrain warms visibly with the low sun. The colour reads
  closer to the fog tint than the diffuse tint.
- Specular highlights stay neutral white (the ramp doesn't touch
  the specular path).
- Lava emission brightens at low sun, dims at high sun.
- Sky background reads the configured sunset sky_color, not the
  legacy navy.

## Out of scope (deferred to a future sprint)

- **Skybox cubemap with sunset bake** — the deferred-cubemap sprint
  will let mappers ship a sunset skybox PNG-folder cubemap.
- **Sun disc rendering** — `atmosphere.sun_color` carries the disc
  tint per FINDINGS §1.4 but no disc is drawn in Sprint 28.
- **Animated time-of-day** — sun direction is static.
