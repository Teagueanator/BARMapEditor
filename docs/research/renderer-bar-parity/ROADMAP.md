# Renderer parity arc — Sprint 15 → 23

**Status:** Authored 2026-05-18 after user reversal of SRS §2.1 #11
("3D preview ≠ in-game rendering").

**New policy:** the editor must visually reproduce what Recoil renders
for every BAR map feature. The renderer is the source of truth for the
user — mappers iterate against what they see. Approximation isn't
acceptable; we close the gap.

This roadmap sketches the 9-sprint arc. Sprint 15 is drafted in full at
`docs/prompts/sprint-15-renderer-depth-rework.md`. Sprints 16–23 are
sketched here and will be drafted in detail when their predecessor
closes (each sprint's pitfalls and exit criteria depend heavily on what
shipped in the prior one).

## Why a roadmap doc, not 9 prompt files

Drafting all 9 prompts up front risks:
- Specifying exit criteria that change once Sprint 15's foundation
  reveals which wgpu APIs actually behave as expected.
- Locking ADR numbers (037+) before downstream sprints choose
  alternatives.
- Stale instruction sets — each sprint inherits the previous one's
  shader / pipeline state.

So this doc is the **shape** of the arc; individual prompts get
written one-sprint-ahead. Sprint 16's prompt should be drafted when
Sprint 15 commits land. Etc.

## Source-of-truth references for every sprint

Every sprint in this arc cross-references against the local
RecoilEngine clone at `/home/teague/code/RecoilEngine`. Critical files:

- `cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl` — terrain
  fragment shader (verified at HEAD 2026-05-18 in
  `docs/research/source-audit-2026-05-18/FINDINGS.md` §7).
- `cont/base/springcontent/shaders/GLSL/SMFVertProg.glsl` — terrain
  vertex shader.
- `cont/base/springcontent/shaders/GLSL/WaterRefraction.glsl` and
  the water-related shaders — Sprint 18 inputs.
- `cont/base/springcontent/shaders/GLSL/Grass*.glsl` — Sprint 21
  inputs.
- `cont/base/springcontent/shaders/GLSL/Sky*.glsl` and
  `Atmosphere*.glsl` — Sprint 17 inputs.
- `cont/base/springcontent/shaders/GLSL/Model*.glsl` — Sprint 20
  S3O / 3DO model rendering.
- `rts/Map/SMF/SMFGroundDrawer.cpp` + `SMFRenderState.cpp` — pipeline
  state for the SMF draw path.
- `rts/Rendering/Env/*.cpp` — environment effects (sky, water, grass).
- `rts/Rendering/Models/*.cpp` — model loading + draw paths.

Also: real BAR maps in `scratch/bar-maps/extracted/` (TitanDuel,
Comet Catcher Remake) — visual ground truth.

## The 9 sprints

### Sprint 15 — Foundation (drafted)

**Goal:** offscreen render target with depth attachment + GPU markers
pipeline. Translucent markers blend correctly when orbiting; markers
depth-test against terrain.

**ADR:** 037 (offscreen RT pattern + marker pipeline).

**Touch:** `render.rs`, `terrain.wgsl`, new `markers.wgsl`,
`main.rs::central`, `ui/overlay.rs`.

**Drafted in:** `docs/prompts/sprint-15-renderer-depth-rework.md`.

---

### Sprint 16 — Terrain shader parity

**Goal:** port `SMFFragProg.glsl`'s composite math into `terrain.wgsl`.
After this sprint, the terrain shows the actual BAR-textured surface:
SMT base diffuse + DNTS splat normals (4-channel composite) + base
normal map (R+A encoding) + sun lighting (Lambert + Blinn-Phong) +
specular texture + `SMF_INTENSITY_MULT`.

**Subsumes:** the splat-shader portion of Sprint 9 / D4. After Sprint 16
ships, the D4 ADR-036 work folds into ADR-038 (or 036 is re-claimed
for the unified terrain shader — TBD when Sprint 16 starts).

**Critical inputs (cross-ref source-audit FINDINGS §7):**

- §7.1 — `SMF_INTENSITY_MULT = 210/255`, apply CPU-side to ambient
  uniform.
- §7.2 — DNTS gating is `splatDistrTex && splatDetailNormalTex[]`,
  NOT specularTex. Lint warn but don't gate.
- §7.3 — per-channel UV multipliers `splats.texScales.{r,g,b,a}`;
  each layer decoded `* 2 - 1` across full RGBA;
  `splatCofac = dist * texMults`;
  normal-blend strength `min(1.0, dot(splatCofac, vec4(1.0)))`.
- §7.4 — TBN built from per-fragment normal via
  `cross(normal, vec3(-1, 0, 0))`. NOT static `T=+X / B=+Z`.
- §7.5 — base normal R+A only, Y derived: `nx = normalsTex.r;
  nz = normalsTex.a; ny = sqrt(1 - nx*nx - nz*nz)`.
- §7.6 — specular exponent `specularCol.a * 16.0` (flat). Global
  `lighting.specularExponent` only consulted when no specularTex
  bound.

**Touch:** `terrain.wgsl` (major rewrite), `render.rs` (more bind
group slots: base normal, splat distribution, 4 × DNTS, specular,
detail-normal, emission, sky-reflect-mod, parallax-height), the
mapinfo schema may grow `lighting` uniforms passed via the terrain
uniform buffer.

**Test:** load a pre-built BAR map's textures (DNTS DDS files + base
normal + specular) into the editor; render; compare to a BAR
screenshot of the same map. Target: visually indistinguishable at
camera distances 2-8 SMU.

---

### Sprint 17 — Atmosphere + fog

**Goal:** exponential height fog from `mapinfo.atmosphere.fogStart`/
`fogEnd`/`fogColor`. Sun color + direction modulates terrain lighting.
Sky color background (when terrain doesn't fill viewport). Skybox
sampling if `mapinfo.atmosphere.skyBox` is set.

**Inputs:**
- `mapinfo.atmosphere.*` (Sprint 10 wires the schema).
- Engine reference: `rts/Rendering/Env/SkyLight*.cpp`,
  `Atmosphere.cpp`.

**Touch:** `terrain.wgsl` (fog blend on output), `render.rs` (sky
background pass or fullscreen clear with sky color, skybox texture
binding), maybe new `sky.wgsl` for cubemap sampling.

**Test:** a map with `fogStart = 0.1`, `fogEnd = 1.0`, dark blue
fog — terrain at far range should haze toward that color, matching
how the same mapinfo renders in BAR.

---

### Sprint 18 — Water rendering

**Goal:** water at `y = 0`. Includes:
- Plane color from `mapinfo.water.planeColor`.
- Surface color modulation by `surfaceColor` + `surfaceAlpha`.
- Planar reflections — render the terrain a second time mirrored
  through `y = 0` into a reflection texture, sample in the water
  shader.
- Refractions — sample the offscreen color buffer through perturbed
  UV (refractionDistortion).
- Foam at shorelines — sample terrain Y at slightly above 0 to detect
  water/terrain transition.
- Caustics — animated `mapinfo.water.causticsResolution / Strength`.
- Surface normal map from `mapinfo.water.normalTexture`.
- Fresnel from `fresnelMin/Max/Power`.
- Wave perlin from `perlinStartFreq/Lacunarity/Amplitude`.

**Inputs:**
- All 21+ `mapinfo.water` parameters from FINDINGS §1.5.
- Engine reference: `rts/Rendering/Env/BumpWater.cpp`,
  `WaterRefraction.glsl`, `BumpWater.glsl`.

**Touch:** new `water.wgsl`, new water pipeline in `render.rs`,
mapinfo schema `water` block fully consumed.

**Test:** load a sea map (e.g., a 32-SMU map with `min_height = -100`,
`tidal_strength = 1.0`, full water block configured). Render. Compare
to BAR's water render of the same.

---

### Sprint 19 — Directional shadows

**Status:** **SHIPPED** as Sprint 30 / R4 / ADR-048 (renumbered by
the planner-arc). 2026-05-22.

**Goal:** cascaded shadow map pass for sun direction. Terrain
fragment shader samples the shadow map; `mapinfo.lighting.groundShadowDensity`
controls how dark shadows are. Features (Sprint 20+) also receive
shadows once they render.

**Inputs:**
- `mapinfo.lighting.sun_dir` for the shadow camera direction.
- `groundShadowDensity` for intensity.
- Engine reference: `rts/Rendering/ShadowHandler.cpp`,
  `rts/Rendering/GL/ShadowFrustumLocker.cpp`.

**Touch:** new shadow-pass pipeline (depth-only render from sun's
view), shadow map texture (typically 2048² Depth32Float),
`terrain.wgsl` gains a shadow sample, `render.rs` adds the shadow
pass before the main pass.

**Caveat:** cascaded shadow maps (multiple resolutions for distance
bands) are the standard real-time solution but add complexity.
Sprint 19 can ship a single-cascade shadow map first; multi-cascade
is a polish item.

**Test:** sculpt a tall hill. Sun direction at `(0.7, 0.5, 0.5, 1.0)`
casts a shadow on the lee side of the hill. Verify in BAR-vs-editor
side-by-side.

**Shipped scope (Sprint 30 / R4 / ADR-048):**
- Single-cascade orthographic shadow camera tight-fit to the map
  AABB (`crates/barme-app/src/render.rs::ShadowCamera::for_map`).
- 2048² `Depth32Float` shadow map (`SHADOW_MAP_SIZE` /
  `SHADOW_MAP_FORMAT`).
- Depth-only shadow-gen pipeline (`crates/barme-app/src/
  shadow_gen.wgsl`) reusing the terrain `Uniforms` + heightmap
  binding.
- 3×3 PCF soft-edge sample in `terrain.wgsl::sample_shadow`.
- Sampling-side `ShadowUniforms` (mat4 VP + bias + density +
  enabled), bound at terrain group 0 binding 16.
- `App::shadow_uniforms_for_render` reads
  `mapinfo.lighting.ground_shadow_density` /
  `unit_shadow_density` (default 0.8 each per `bar_default`).
- Multi-cascade (CSM), VSM, PCSS, slope-scaled bias, feature
  shadow CASTING (sprites already RECEIVE shadows through the
  terrain shader's per-fragment world position) all deferred to
  Stage-2 polish.

---

### Sprint 20 — Feature rendering (S3O / 3DO)

**Goal:** load and render the stock BAR features (trees, rocks,
wreckage, geovent) as 3D meshes with diffuse / normal / specular
textures. Sprint 11 / C5 places `geovent` features (data); Sprint 20
makes them visually correct.

**Inputs:**
- Stock `mapfeatures` repo: `github.com/beyond-all-reason/mapfeatures`.
- S3O binary format: header + vertices + UV + texture refs.
- 3DO format (legacy): older format; trees often still 3DO.
- Engine reference: `rts/Rendering/Models/S3OParser.cpp`,
  `3DOParser.cpp`, `Models/ModelRenderer*.cpp`.

**Touch:** new `crates/barme-core/src/models.rs` (S3O / 3DO parser),
new `model.wgsl` and pipeline, instance buffer for placed-feature
positions (from `Project.features`).

**Caveat:** S3O / 3DO parsing is non-trivial — Recoil's parser is
~1500 LOC of C++. Sprint 20 may need its own sub-arc (parser,
renderer, animation if features animate, team-color masking).

**Test:** place 3 pine trees, 2 rocks. Render. They should match
the BAR render of the same features.

---

### Sprint 21 — Grass rendering

**Goal:** grass blades render as instanced quads with wind animation.
Parameters from `mapinfo.grass.{bladeWidth, bladeHeight, bladeColor,
bladeAngle, maxStrawsPerTurf, bladeWaveScale}`.

**Inputs:**
- `mapinfo.resources.grassBladeTex` (the blade texture).
- `mapinfo.grass.*` parameters.
- Engine reference: `rts/Rendering/Env/Decals/GrassDrawer.cpp`,
  `Grass*.glsl`.

**Touch:** new `grass.wgsl` and pipeline, grass density texture
(generated CPU-side from terrain typemap — grass grows on
terrain-type 0 by convention), instance buffer for blade transforms.

**Caveat:** grass on a 16-SMU map = millions of blades. Standard
solution: instance LOD by camera distance, only render within
~200 elmos of the camera. Bake the density texture once on terrain
edit.

**Test:** wizard creates a 16-SMU map with default biome → grass
visible on the playable terrain.

---

### Sprint 22 — Emission + skybox + parallax

**Goal:** close the remaining shader features.

- **Emission** — `mapinfo.resources.lightEmissionTex` modulates fragment
  output with self-illumination (lava maps glow even at night).
- **Skybox cubemap reflections** —
  `mapinfo.resources.skyReflectModTex` modulates terrain reflectivity
  with the skybox cubemap. Wet rocks shimmer with sky.
- **Parallax** — `mapinfo.resources.parallaxHeightTex`. Engine doesn't
  currently consume this; verify before implementing.

**Touch:** `terrain.wgsl` final pass (emission added, sky-reflect
modulated, parallax UV offset if implementing).

**Test:** load a lava map (e.g., one of BAR's lava archetypes if
available); confirm the lava cracks glow at low ambient.

---

### Sprint 23 — Parity validation + SRS update

**Goal:** validate the editor's render matches BAR's render. Drift
list. Final SRS §2.1 #11 update.

**Validation procedure:**
- Pick 3 reference BAR maps with diverse aesthetics (e.g., Comet
  Catcher Remake — Earth-temperate; Quicksilver — alien-industrial;
  All That Simmers — wasteland).
- For each map: load in the editor with full mapinfo; render at
  3 standard camera angles (top-down, 35° pitch, grazing); save
  screenshot.
- Render the SAME map in BAR (headless via `--isolation`); save
  screenshot at matching angles.
- Side-by-side diff per pixel; compute mean ΔE (perceptual color
  difference).
- Target: mean ΔE < 5.0 per scene (eyeball-indistinguishable).

**Drift list:** items where the editor still diverges from BAR.
Each gets either a polish task or a documented "we don't render X
because Y" note.

**SRS update:** §2.1 #11 changes from "3D preview ≠ in-game
rendering. Do not pretend WYSIWYG" to "3D preview reproduces BAR's
render at editor camera distances within mean ΔE 5.0 across the
validation suite. See Sprint 23 validation report."

**Touch:** validation harness in
`crates/barme-app/tests/parity_validation.rs`, screenshot fixtures
in `assets/parity_screenshots/`.

---

## Sequencing with the feature-completion arc (Sprints 10–14)

The renderer arc (15–23) and the feature arc (10–14) are **parallel
streams**:

```
Feature arc:    10 → 11 → 12 → 13 → 14
                ↓
Renderer arc:   15 → 16 → 17 → 18 → 19 → 20 → 21 → 22 → 23
```

Cross-stream coupling:

- **Sprint 11 (C4/C5 markers)** ideally runs AFTER Sprint 15 so its
  metal/geo markers use the GPU marker pipeline from day one. If
  Sprint 11 ships first, Sprint 15 retroactively ports its markers
  (small cost).
- **Sprint 9 (D4/D5 splat shader/UI)** — D4's splat shader work is
  subsumed by Sprint 16's terrain shader parity. Options:
  - Keep Sprint 9 D4 as a minimal stand-alone splat shader (just to
    show painted colors), then Sprint 16 replaces with the
    full SMFFragProg port.
  - Skip Sprint 9 D4 entirely; ship D5 (the UI) with placeholder
    rendering; Sprint 16 catches up.
  - Recommend the second — saves a redundant ADR cycle.
- **Sprint 12 (D6 splat emission)** is independent of rendering. The
  `.sd7` round-trip is data-only.
- **Sprint 13 (D7 minimap)** uses the offscreen render to produce the
  minimap PNG. After Sprint 16, the minimap reflects real-textured
  terrain. Sprint 13 can run any time after Sprint 16.
- **Sprint 20 (features)** unlocks Sprint 12's F7 placement preview
  — placed features get visual feedback in the editor.

## Total scope

- 9 sprints, ~3-6 commits each = ~30-50 commits.
- Approximate timeline at one sprint per week = 9 weeks of renderer
  work, in parallel with the feature arc.
- New crates / modules expected:
  - `crates/barme-core/src/models.rs` (Sprint 20 — S3O / 3DO).
  - `crates/barme-app/src/markers.rs` (Sprint 15 — MarkerBatch).
  - New `.wgsl` shaders: `markers`, `water`, `sky`, `model`, `grass`,
    `shadow`.
- ADRs likely needed: 037 (offscreen RT, Sprint 15), 038 (terrain
  shader parity), 039 (atmosphere), 040 (water), 041 (shadows),
  042 (features / S3O), 043 (grass), 044 (emission + sky reflect).

## Open questions to resolve before drafting each sprint

- Does eframe / egui-wgpu support a depth attachment in its callback
  pass? Sprint 15 sidesteps this via offscreen RT, but if eframe ever
  exposes it directly, simplify.
- HDR (Rgba16Float) — defer to Stage 2 or include in Sprint 22?
- MSAA — defer to Stage 2 polish or include in Sprint 15 from the
  start?
- Are we OK with the editor's grass / water having animation that
  the in-game render's wind doesn't match exactly? (Wind is set by
  `mapinfo.atmosphere.minWind`/`maxWind` but is a stochastic
  game-state simulation, not deterministic for screenshots.)
