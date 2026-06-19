# Grass-field — grass-rendering parity fixture

**Purpose:** Sprint 34 / R6 / ADR-050 reference for the instanced
billboard-grass pipeline. Verifies that blades render at terrain
elevation across a flat field, sway with the wind in sync with the
water plane, receive shadows, and fade smoothly at the LOD radius.

## What this fixture exercises

- **Density bake → instance scatter.** A dead-flat field bakes to
  near-full coverage (slope ≈ 0 → sigmoid ≈ 0.98), so every 16-elmo
  turf scatters its full `maxStrawsPerTurf × density_scale` blade
  count. Confirms the bake → generate → draw chain end to end.
- **Per-blade jitter / no shimmer.** Orbit the camera slowly and
  confirm the field is static underfoot — individual blades keep
  their position / lean / colour as the camera moves (hashed-cell
  PRNG, pitfall #4). A shimmering field means the jitter is keyed on
  the camera instead of the turf cell.
- **Wind sync with water.** Lower the field to sit at the water plane
  (or place a water plane over part of it) and confirm the grass
  sway and the water surface motion share a phase / direction — both
  read the same `atmosphere.wind` block + `water_time_seconds`
  (pitfall #3).
- **Shadow receive.** Add a tall feature or a hill at one edge; its
  shadow should darken the blades it falls across (3×3 PCF, same
  shadow map as terrain). Grass does NOT cast — the blades' own
  silhouettes leave no shadow on the ground (documented in ADR-050).
- **LOD fade.** From a high orbit, confirm blades fade out smoothly
  toward ~200 elmos from the camera rather than popping at a hard
  ring (shader alpha fade across the outer quarter, pitfall #9).
- **Density throttle.** `View > Grass density` from 1.0 → 0.0 thins
  the field continuously to bare ground; `View > Grass` off removes
  it entirely.

## Suggested project settings

- **Map size:** 4 × 4 SMU (`MapSize::square(4)`) — small flat field,
  fast to author, keeps the blade count well inside the 100k budget
  at `maxStrawsPerTurf = 64`.
- **Min height:** 0 elmos. **Max height:** 200 elmos (a gentle band;
  the field itself is flat so the exact max only matters if you carve
  a hill for the shadow test).
- **Heightmap:** flat (procgen "flat" or a freshly-sized project).
  Optionally carve one tall hill at an edge for the shadow check.
- **MapInfo overrides:**
  ```lua
  grass = {
    bladeColor       = { 0.10, 0.40, 0.10 },  -- engine green
    bladeWidth       = 0.7,
    bladeHeight      = 4.5,
    bladeWaveScale   = 1.0,
    maxStrawsPerTurf = 64,
  }
  atmosphere = {
    minWind = 5.0,
    maxWind = 25.0,   -- wind band drives the sway amplitude
  }
  lighting = {
    sunDir              = { 0.3, 1.0, -0.2, 1.0 },
    groundAmbientColor  = { 0.5, 0.5, 0.5 },
    groundDiffuseColor  = { 0.5, 0.5, 0.5 },
    groundShadowDensity = 0.8,
  }
  ```

## How to use (manual smoke until Sprint 36 automates)

1. Load these values into a fresh 4-SMU project (F9 → Map tab) and
   set `mapinfo.grass.maxStrawsPerTurf = 64` (F9 → Grass tab).
2. Confirm a green grass field covers the flat terrain at elevation
   (blades stand ON the surface, not floating above or sunk below).
3. Orbit slowly — the field must stay static underfoot (no shimmer).
4. Watch the sway: blades lean and oscillate in the wind direction.
   If a water plane is present, its motion shares the phase.
5. `View > Grass density` slider 1.0 → 0.0: the field thins smoothly
   to nothing. `View > Grass` off: field disappears entirely.
6. From a high orbit, confirm the outer blades fade rather than pop.
7. (Optional) Carve a tall hill at one edge and confirm its shadow
   darkens the blades beneath it.

## Reference

- Engine grass vertex/fragment: `RecoilEngine/cont/base/springcontent/
  shaders/GLSL/GrassVertProg.glsl` + `GrassFragProg.glsl`.
- Engine grass drawer: `rts/Rendering/Env/GrassDrawer.cpp`
  (`strawPerTurf` cap, wind vector × `bladeWaveScale`).
- Engine grass mapinfo read: `rts/Map/MapInfo.cpp:190-195`
  (the six grass keys + their defaults).
- Editor shader: `crates/barme-app/src/grass.wgsl`.
- Editor density bake: `crates/barme-core/src/grass.rs`.
- Editor instance scatter: `crates/barme-app/src/grass.rs`.
- Sprint 34 ADR: [`docs/DECISIONS.md`](../../../docs/DECISIONS.md)
  ADR-050.
- Devlog: `devlog/sprint-34-grass-rendering/`.

## Status

**Manual smoke procedure only**, same as every other parity fixture
through Sprint 33. The CPU bake, instance scatter, WGSL validity, and
GPU layout contract are unit-tested in headless CI; the live visual
match and the 100k-blade < 4 ms Vega 8 frame budget need a GPU session
and are NOT verified in CI. Sprint 36 (parity validation) automates ΔE
comparison against BAR's render. Until then this README is the truth.
