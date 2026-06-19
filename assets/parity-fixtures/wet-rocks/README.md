# Wet-rocks — sky-reflection parity fixture

**Purpose:** Sprint 35 / R7 / ADR-051 reference for the terrain
sky-reflection path (`skyReflectModTex`). A post-rain coastline whose
wet rock surfaces shimmer with the sky while the dry ground stays
matte.

Engine reference: `SMFFragProg.glsl:341-350` —
`diffuseCol.rgb = mix(diffuseCol.rgb, reflectCol, reflectMod)`, applied
to the diffuse base BEFORE lighting.

## Important: uniform-sky fallback (no cubemap yet)

The engine's reflected colour is a `skyReflectTex` **cubemap** sample.
Sprint 28 / ADR-045 **deferred** the skybox cubemap, so per the
Sprint-35 prompt's pitfall #7 this fixture reflects the **uniform sky
colour** (`atmosphere.skyColor`), not a real environment cubemap. The
shimmer therefore reads as a flat sky-tinted sheen rather than a
mirrored horizon — that gap closes when a future sprint binds a real
cubemap (see ADR-051). This is the documented, intended behaviour for
Sprint 35.

## What this fixture exercises

- **Per-channel reflectivity modulation.** Where `skyReflectModTex.rgb`
  is high (wet rock), the diffuse mixes toward the sky colour; where
  it's 0 (dry sand), the surface is untouched. Paint a mask with wet
  patches near the waterline fading to dry inland.
- **Reflection is lit + shadowed.** Because the mix happens on the
  diffuse base BEFORE lighting (matching the engine), the reflected
  sky on a shadowed wet rock is correspondingly dimmer — verify a wet
  rock in shadow is darker than one in sun.
- **Mix factor = reflectMod (no `× 0.5`).** The reflectivity comes
  straight from the texture; the prompt's `× 0.5` was dropped to match
  the engine.

## How to use (manual smoke until Sprint 36 automates)

1. Load a fresh 8-SMU project; sculpt a coastline (terrain dipping
   below `y = 0` into water).
2. Set a distinct `atmosphere.skyColor` (e.g. a pale blue) so the
   reflection is visible against the rock diffuse.
3. Author a sky-reflect-mod PNG: bright `(0.8, 0.8, 0.8)` on the wet
   rocks at the waterline, fading to black inland. Upload via
   `render::upload_sky_reflect_mod` (the F9 `skyReflectModTex` field
   carries the filename for the `.sd7` export).
4. Confirm the wet rocks take on the sky tint while the dry ground
   stays its diffuse colour.

## Acceptance

- Wet rocks visibly shimmer with the sky colour; dry ground does not.
- A wet rock in shadow reflects a dimmer sky than one in sun.
- No reflection where the mod texture is black.

## Known divergences

- **Uniform-sky fallback** (see above) — no mirrored horizon until the
  cubemap ships. The reflected colour is flat `atmosphere.skyColor`.
- Grazing-angle horizon clamp (pitfall #2) is moot under the uniform
  fallback (the reflection direction doesn't affect a uniform colour);
  it becomes load-bearing when the cubemap lands.
