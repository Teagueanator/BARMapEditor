# Sprint 35 — Emission + skybox-reflect + parallax close-out (R7)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 35** — the eighth renderer-parity sprint (R7 in
ROADMAP numbering). The remaining `mapinfo.resources` texture
bindings get wired:

- **`lightEmissionTex`** — emission texture modulates fragment
  output with self-illumination (lava maps glow even at night).
- **`skyReflectModTex`** — modulates terrain reflectivity with
  the skybox cubemap (Sprint 28). Wet rocks shimmer with sky.
- **`parallaxHeightTex`** — verify engine consumes this; if so,
  port. Otherwise document as deferred (the engine may not
  actually consume it, per ROADMAP).
- **`grassBladeTex`** — the blade silhouette texture (Sprint 34
  ships procedural; Sprint 35 wires the resource).

This is the LAST renderer feature sprint. Sprint 36 ships
validation only.

**Prerequisites:**
- Sprint 34 (grass) MUST be ticked. Grass shader gains the blade
  texture bind.
- Sprint 28 (atmosphere + fog) — skybox cubemap is the
  sky-reflect source.

**Reference clone of RecoilEngine:** `/home/teague/code/RecoilEngine`.
Critical files:
- `cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl` —
  `lightEmissionTex` + `skyReflectModTex` sampling.
- `rts/Map/SMF/SMFRenderState.cpp` — bind state.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — `MapInfo::ResourcesBlock`.
3. `/home/teague/code/BARMapEditor/docs/research/renderer-bar-parity/ROADMAP.md`
   — Sprint 22 (original numbering) = this Sprint 35.
4. `/home/teague/code/RecoilEngine/cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl`
   — look for `emissionTex`, `skyReflectModTex`.
5. `/home/teague/code/BARMapEditor/crates/barme-app/src/terrain.wgsl`
   (Sprint 25 baseline).

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-35-emission-skybox-reflect-parallax
```

## Step 3 — Scope

One commit per chunk:

### 1. Emission texture binding + shader

In `terrain.wgsl`, after lighting / fog:

```wgsl
let emission = textureSample(emission_tex, sampler_lin, world_uv).rgb * uniforms.emission_strength;
final_color += emission;
```

`uniforms.emission_strength` defaults to 1.0; lava maps may amplify.

Bind `lightEmissionTex` as a new texture group; default = 1×1 black
(no emission).

### 2. Sky-reflect modulation

```wgsl
let reflect_mod = textureSample(sky_reflect_mod_tex, sampler_lin, world_uv).rgb;
let reflect_dir = reflect(-view_dir, surface_normal);
let sky_color = textureSample(skybox_cube, sampler_lin, reflect_dir).rgb;
let reflected = sky_color * reflect_mod;
final_color = mix(final_color, reflected, reflect_mod * 0.5);
```

`skyReflectModTex` defaults to 1×1 black (no reflection). Wet
texture maps modulate the reflection.

### 3. Parallax (verify before porting)

The engine MAY not actually consume `parallaxHeightTex` — verify
by grepping `RecoilEngine/rts/Map/SMF/` for `parallaxHeightTex`.

**If consumed**: implement parallax UV offset:
```wgsl
let view_tangent = view_dir_in_tangent_space;
let height = textureSample(parallax_tex, sampler_lin, world_uv).r;
let parallax_offset = view_tangent.xy * (height - 0.5) * uniforms.parallax_scale;
let parallaxed_uv = world_uv + parallax_offset;
```
**If not consumed**: skip; document in ADR-044.

### 4. Grass blade texture wiring

Sprint 34 shipped a procedural blade. Sprint 35 wires the
`grassBladeTex` resource:

```wgsl
let blade_alpha = textureSample(grass_blade_tex, sampler_lin, blade_uv).a;
return vec4<f32>(lit * shadow, blade_alpha);
```

Default 1×1 white (procedural fallback).

### 5. F9 form Resources tab integration

Already shipped in Sprint 18 / C7. Verify all four fields
(`lightEmissionTex`, `skyReflectModTex`, `parallaxHeightTex`,
`grassBladeTex`) accept TextEdit input with file pickers + per-
field tooltips from Sprint 19.

### 6. ADR-044

```
## ADR-044 — Final shader bindings (Sprint 35 / R7)

Status: ADOPTED 2026-05-XX
...
```

Cover: emission strength, sky-reflect mix factor, parallax
inclusion decision (yes or skipped), grass blade texture sourcing.

### 7. Validation + rollup

Add `assets/parity-fixtures/lava-emission/` — a lava map with
glowing cracks. Add `assets/parity-fixtures/wet-rocks/` — a
post-rain coastline with sky reflections.

**Platform-portability checklist** — see prior R-sprints.

STATUS UPDATEs in SRS / ROADMAP (R7 done; renderer 8/8 except
validation). "Sprint 36 = parity validation + SRS §2.1 #11
closeout" handoff.

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on resource-texture
loads; `warn!` on missing files (fall back to defaults).

## Step 5 — Out of scope

- **Per-vertex emission** (vs per-fragment texture sample) —
  texture only.
- **Animated emission** (lava bubbling) — Stage 2 polish.
- **Parallax occlusion mapping (POM)** — basic parallax only.
- **Multi-bounce reflections** — single-bounce only.

## Step 6 — Critical pitfalls (read twice)

1. **Emission must NOT be modulated by shadow**. Emission =
   self-illumination. Add emission AFTER `lit * shadow_factor`
   in the shader.

2. **Sky-reflect at horizon**: the `reflect()` operation at
   grazing angles can yield directions below the horizon (into
   the ground). Clamp the Y component to 0 before sampling the
   skybox.

3. **Parallax verification**: empirically check the engine
   consumes `parallaxHeightTex` before implementing. If not,
   the lint rule should warn users not to set it.

4. **Texture defaults**: 1×1 fallback textures (black for
   emission, black for reflect-mod, white for grass blade)
   must exist as static resources baked into the binary.

5. **Bind-group bloat**: with all the textures Sprint 25 + 35
   added, the terrain pipeline's bind groups may exceed wgpu's
   default limit. Verify and reorganise if needed.

6. **Lava-emission day/night**: Sprint 26 ramped lava water
   glow by sun angle. Terrain emission should NOT ramp — it's
   the volcanic ground itself glowing, independent of sun.

7. **Sky-reflect requires Sprint 28's skybox**. If no skybox is
   set, fall back to `sky_color` cubemap-fake (sample as if the
   sky is a uniform colour).

8. **Grass blade texture cache**: use the same texture loading
   path as DNTS slot textures (Sprint 7+). Path resolution per
   PITFALLS §11 (pink-map on rename).

9. **Platform portability**: WGSL `textureSample` + `reflect`
   are standard; no concerns.

10. **`grassBladeTex` resolution**: typically 64×64. Loaded via
    `image` crate's PNG decode.

## Step 7 — Exit criteria

- 5+ commits on `main`: emission, sky-reflect, parallax
  (or skip), grass-blade-texture, ADR-044, validation,
  rollup.
- 1 devlog folder filled.
- SRS / ROADMAP STATUS UPDATEs (R7 done; renderer 8/8 except
  validation).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test:
  - Lava-emission fixture: cracks visibly glow.
  - Wet-rocks fixture: surface reflects sky.
  - Grass field uses the loaded blade texture (or procedural
    fallback works).
- Final devlog: summary + "Sprint 36 = parity validation +
  SRS §2.1 #11 closeout" handoff.

Start by verifying parallax engine consumption. Then emission +
sky-reflect (the easier ports). Grass blade texture is
trivial.
