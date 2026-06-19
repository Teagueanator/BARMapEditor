# Lava-emission — terrain self-illumination parity fixture

**Purpose:** Sprint 35 / R7 / ADR-051 reference for the **terrain**
emission path (`lightEmissionTex`). Distinct from the Sprint-26
`lava-sample` fixture, which exercises the *water* shader's lava-mode
glow. This one verifies that the SOLID GROUND — volcanic rock with
glowing cracks — self-illuminates via the terrain fragment shader,
independent of sun angle and shadow.

Engine reference: `SMFFragProg.glsl:392-401` —
`fragColor.rgb = fragColor.rgb·(1 − emissionCol.a) + emissionCol.rgb`.

## What this fixture exercises

- **Alpha-masked emission composite.** Where the emission texture's
  `.a` is high (a crack), the lit terrain colour is replaced by the
  glow `.rgb`; where `.a` is 0 (cold rock), the terrain is untouched.
  This is the engine's MASKED blend, not an additive `+= emission`.
- **Not shadow-masked (pitfall #1).** Emission is applied AFTER the
  `lit · shadow_factor` lighting compose, so a crack in shadow still
  glows. Cast a shadow across a glowing crack (tall ridge + low sun)
  and confirm the crack stays bright.
- **No day/night ramp (pitfall #6).** Unlike the Sprint-26 lava WATER
  (whose glow ramps with sun altitude), terrain emission is constant —
  the volcanic ground glows the same at noon and midnight. Rotate the
  sun (`lighting.sunDir`) and confirm crack brightness is invariant.
- **EMISSION_STRENGTH = 1.0.** Exact engine parity (the engine has no
  strength scalar). The shader const exists only as a future per-map
  amplification hook.

## How to use (manual smoke until Sprint 36 automates)

1. Load a fresh 8-SMU project; sculpt a cracked volcanic surface
   (a few deep fissures).
2. Author (or drop in) an emission PNG the size of the terrain whose
   `.rgb` is a hot orange `(1.0, 0.35, 0.05)` along the crack lines and
   black elsewhere, with `.a` = 1 on the cracks, 0 on the rock.
   Upload via `render::upload_emission` (the parity-fixture loader
   path; the F9 `lightEmissionTex` field carries the filename for the
   `.sd7` export).
3. Drop the ambient light low (dusk) and confirm the cracks glow
   against the dim rock.
4. Rotate the camera and the sun: the glow does not move, dim, or
   brighten with either.

## Acceptance

- Cracks visibly glow against low-ambient rock.
- A shadow cast over a crack does NOT dim the glow.
- Sun rotation leaves crack brightness unchanged.
- No oversaturation where emission `.a` and specular highlight
  overlap (emission applied post-lighting; specular is already folded
  into `lit`).

## Known divergences

- The editor's base normal is usually the 1×1 fallback (no R+A bake at
  preview time), so micro-relief on the rock between cracks is flatter
  than BAR until the heightmap → normal bake ships. Emission itself is
  unaffected (it's a flat texture sample).
- `EMISSION_STRENGTH` is a shader constant (1.0), not a uniform; no F9
  amplification control yet (ADR-051).
