# Shadow-test — directional-shadow parity fixture

**Purpose:** Sprint 30 / R4 / ADR-048 reference for the depth-only
shadow-gen pipeline + terrain shader's `sample_shadow` PCF sample.
Verifies that a tall central hill casts a shadow on its lee side and
that `mapinfo.lighting.ground_shadow_density` endpoints (0.0 / 1.0)
control the shadow's visibility without crashing.

## What this fixture exercises

- **Shadow camera frustum.** `ShadowCamera::for_map` builds the
  orthographic frustum tight-fit to the map AABB rotated by the sun
  direction. The 8-corner walk handles sloped sun angles correctly —
  this fixture's `sun_dir = (0.7, 0.5, 0.5)` is sloped both east
  AND south, so the frustum rotates noticeably from the trivial
  axis-aligned case.
- **3×3 PCF.** The hill's silhouette edge on the lee side should
  show a soft (sub-pixel-fuzzy) transition rather than a hard
  1-bit comparison artefact. Compare a flat single-sample baseline
  by temporarily setting `shadow.params.w = 0` in the WGSL — the
  hard edge confirms PCF is active in the default path.
- **Density endpoints.** `ground_shadow_density = 0` → no shadow
  visible (the lee side of the hill matches the lit side).
  `ground_shadow_density = 1.0` → full sampled shadow (the lee side
  goes to pure ambient).
- **Acne / Peter-Panning balance.** The default `SHADOW_DEPTH_BIAS =
  0.005` should leave the hill's casting edge attached to its
  silhouette (no Peter-Panning gap) and the flat terrain around the
  hill speckle-free (no acne).
- **Frustum bounds check.** A fragment outside the shadow frustum
  must return lit (`sample_shadow` returns 1.0 above the NDC bounds
  check). Build a project larger than the shadow camera's max
  frustum and confirm the corner fragments don't show garbage.

## Suggested project settings

- **Map size:** 4 × 4 SMU (`MapSize::SMU { x: 4, z: 4 }`) — small
  enough that the shadow camera frustum's per-texel resolution is
  ~1 elmo, giving sharp silhouettes the eye can verify against
  expectations.
- **Min height:** 0 elmos.
- **Max height:** 800 elmos.
- **Heightmap:** a tall central hill — easiest to author via the
  procgen "cone peak" preset, then carve the cone's flank with a
  smooth brush to broaden the silhouette so the lee shadow has
  visible area to fall on. Approximate target:
  - Central peak at world (1024, 800, 1024).
  - Base radius ~600 elmos (peak fades to 0 height by ~1500 elmos
    from centre on the flat terrain around the hill).
  - Flat terrain (Y = 0) outside the cone.
- **MapInfo overrides:**
  ```lua
  lighting = {
    sunDir              = { 0.7, 0.5, 0.5, 1.0 },
    groundAmbientColor  = { 0.5, 0.5, 0.5 },
    groundDiffuseColor  = { 0.5, 0.5, 0.5 },
    groundShadowDensity = 0.8,  -- BAR default; visible shadow
    groundSpecularColor = { 0.1, 0.1, 0.1 },
    specularExponent    = 100.0,
  }
  atmosphere = {
    fogStart  = 0.1,
    fogEnd    = 1.0,
    fogColor  = { 0.7, 0.7, 0.8 },
    skyColor  = { 0.5, 0.6, 0.9 },
    cloudColor = { 1.0, 1.0, 1.0 },
    cloudDensity = 0.5,
  }
  ```
  (`sunDir.xyz` is `(0.7, 0.5, 0.5)` unnormalised — the renderer
  normalises it on consumption; the shader's `dot(sun, +Y)` ramp
  reads the normalised form.)

## How to use (manual smoke until Sprint 36 automates)

1. Load these values into a fresh 4-SMU project (`MapSize::SMU { x:
   4, z: 4 }`; the F9 form's Map tab is the path).
2. Sculpt the tall central hill with the Sculpt tool — raise to ~800
   elmos at the centre, smooth the flank radius outward to ~600
   elmos.
3. Orbit so the camera looks from the **south-east** (sun direction
   is `(0.7, 0.5, 0.5)` → light travels from `+x +y +z` toward `-x
   -y -z`, so the lee side is the `-x -z` quadrant of the hill).
4. Confirm:
   - The lee side of the hill is visibly darker than the lit side.
   - The shadow edge is soft (sub-pixel fuzzy) thanks to 3×3 PCF.
   - The lee shadow on the flat terrain around the hill is a clean
     silhouette with no acne (random speckle in the lit area) and no
     Peter-Panning (gap between the hill's base and the shadow's
     start).
5. Density endpoint test — open the F9 form's Lighting tab, edit
   `groundShadowDensity`:
   - Set to `0.0` → confirm the lee side matches the lit side
     (no shadow visible).
   - Set to `1.0` → confirm the lee side reads as ambient-only
     (full sampled shadow).
   - Set to `0.5` → confirm the shadow is at half strength.
   - Reset to `0.8` (BAR default).

## Reference

- Engine shadow shader: `RecoilEngine/cont/base/springcontent/
  shaders/GLSL/SMFFragProg.glsl:362-372` (the `shadowCoeff` blend)
  + `:421` (`specularInt *= shadowCoeff`).
- Engine depth-only vertex: `cont/base/springcontent/shaders/GLSL/
  ShadowGenVertMapProg.glsl` — we ported a minimal version as
  `crates/barme-app/src/shadow_gen.wgsl`.
- Engine shadow camera setup: `rts/Rendering/ShadowHandler.cpp::
  InitFBOAndTextures` (depth-format + sampler state) + `::Update`
  (the SetShadowMatrix call we replace with our pure-CPU
  `ShadowCamera::for_map`).
- Sprint 30 ADR: [`docs/DECISIONS.md`](../../../docs/DECISIONS.md)
  ADR-048.
- Devlog: `devlog/sprint-30-directional-shadows/`.

## Status

**Manual smoke procedure only**, same as every other parity fixture
through Sprint 29b. Sprint 36 (parity validation) automates ΔE
comparison against BAR's render. Until then this README is the
truth, and a future drift surfaces as visual divergence the next
maintainer notices during a code review.
