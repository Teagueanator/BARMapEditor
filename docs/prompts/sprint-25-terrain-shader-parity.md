# Sprint 25 — Terrain shader parity: port SMFFragProg (R1)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 25** — the **foundation of the renderer-parity arc**
(Sprints 25-36). The user reversed SRS §2.1 #11 on 2026-05-18: the
editor's renderer must visually reproduce what BAR renders. Sprint 13
(ADR-037) shipped the offscreen RT + depth + GPU marker pipeline.
Sprint 25 starts the actual shader work by porting BAR's
**`SMFFragProg.glsl`** terrain fragment shader into our
**`terrain.wgsl`**.

After this sprint, the terrain shows the actual BAR-textured surface
at editor camera distances (2–8 SMU). Specifically:

- SMT base diffuse (from the composited layer stack — Sprint 17's
  output, which mirrors what `.sd7` ships).
- DNTS splat normals (4-channel composite per
  `SMFFragProg.glsl:174-198`).
- Base normal map (R+A encoded; Y derived).
- Sun lighting (Lambert + Blinn-Phong).
- Per-layer specular (specularCol.a × 16.0 exponent rule).
- `SMF_INTENSITY_MULT = 210/255` pre-applied CPU-side per FINDINGS
  §7.1.
- `splatCofac` normal-blend strength (`min(1.0, dot(splatCofac,
  vec4(1.0)))`).
- TBN built per-fragment from `cross(normal, vec3(-1, 0, 0))` per
  FINDINGS §7.4.

The renderer-parity arc is **8 sprints long**: 25 (this), 26 (water
polish), 28 (atmosphere + fog), 29 (feature S3O/decals), 30 (shadows),
34 (grass), 35 (emission + sky-reflect + parallax), 36 (parity
validation + SRS §2.1 #11 closeout).

**Prerequisites:**
- Sprint 24 (multithreading) MUST be ticked. The shader work uses
  parallel procgen for test fixtures.
- Sprint 17 (D10 / ADR-041) layered painter trio is the source of
  the composited diffuse this sprint consumes.
- Sprint 13 (ADR-037) offscreen RT + depth + markers is the render-
  state foundation.

**Reference clone of RecoilEngine:** the parity work cross-references
against the local clone at `/home/teague/code/RecoilEngine`. If not
present, clone it from `github.com/beyond-all-reason/spring`. Pin
commit ref in the devlog.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §1.2 (renderer-parity
   commitment), §2.1 #11 (now committed to parity).
3. **`/home/teague/code/BARMapEditor/docs/research/renderer-bar-parity/ROADMAP.md`**
   — the 9-sprint arc. Section "Sprint 16 — Terrain shader parity"
   (in the original numbering — that's this Sprint 25).
4. **`/home/teague/code/BARMapEditor/docs/research/source-audit-2026-05-18/FINDINGS.md`**
   §7 — the entire authoritative source on the SMFFragProg port.
   Read §7.1–7.6 in full. **This is the spec; the shader port is
   a transcription, not interpretation.**
5. `/home/teague/code/BARMapEditor/docs/research/splat-rendering/FINDINGS.md`
   — splat math details.
6. `/home/teague/code/RecoilEngine/cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl`
   — the source GLSL shader. ~250 LoC; the entire port lives here.
7. `/home/teague/code/RecoilEngine/cont/base/springcontent/shaders/GLSL/SMFVertProg.glsl`
   — terrain vertex shader. Our existing `terrain.wgsl` vertex
   stage already does roughly the right thing; verify TBN
   construction matches.
8. `/home/teague/code/RecoilEngine/rts/Map/SMF/SMFGroundDrawer.cpp`
   + `SMFRenderState.cpp` — engine-side pipeline state. The
   uniform / texture bind order is here.
9. `/home/teague/code/BARMapEditor/crates/barme-app/src/terrain.wgsl`
   — the current shader. Major rewrite.
10. `/home/teague/code/BARMapEditor/crates/barme-app/src/render.rs`
    — bind group setup. Expands to include base normal map,
    composited diffuse (already there from Sprint 16), specular
    texture, detail-normal slot textures (4-array), splat
    distribution.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-25-terrain-shader-parity
```

## Step 3 — Scope

In order, one commit per item:

### 1. Render-state plumbing: new uniforms + bind group entries

**`crates/barme-app/src/render.rs`**:

Expand the terrain bind group to support:
- Group 0: heightmap + sampler (existing).
- Group 1: composite diffuse RT (existing, from Sprint 16).
- Group 2 (NEW): base normal map (RGBA8 — only R and A
  channels are read per FINDINGS §7.5; B/G unused).
- Group 3 (NEW): specular texture (RGBA8 — RGB = colour, A = roughness
  coefficient).
- Group 4 (NEW): DNTS slot textures — 4 × RGBA8 texture array (the
  existing `splat0..3` slot diffuses, but the **normal** path
  needs the slot's normal map; see pitfall #2).
- Group 5: splat distribution (existing, 1024² RGBA).
- Group 6 (NEW): TerrainUniforms — extended with the new
  per-channel fields:
  ```rust
  #[repr(C)]
  pub struct TerrainUniforms {
      // existing
      sun_dir: [f32; 4],
      ambient: [f32; 4],
      diffuse: [f32; 4],
      min_max_height: [f32; 4],

      // new (Sprint 25)
      splat_cofac_a: [f32; 4],   // per-channel cofactor (FINDINGS §7.3)
      splat_tex_scales: [f32; 4], // per-channel UV multipliers
      splat_tex_mults: [f32; 4],  // per-channel intensity multipliers
      specular_color: [f32; 4],   // global fallback when no specular tex
      specular_exponent: f32,     // global fallback exponent
      smf_intensity_mult: f32,    // 210/255 per FINDINGS §7.1, pre-applied
      has_specular_tex: u32,      // 0 or 1
      has_normal_tex: u32,        // 0 or 1
  }
  ```

**Texture defaults**: when no specular / normal / DNTS textures are
bound, the shader samples a 1×1 fallback texture (white for diffuse,
blue for normal, grey for specular). Bind these as default zero-size
textures so the shader's bind-group layout doesn't change per-frame.

### 2. Port SMFFragProg → terrain.wgsl

The transcription. Maintain a **line-by-line correspondence**
between the GLSL source and the WGSL port; cite the source line
in WGSL comments. Per FINDINGS §7:

**§7.1**: pre-apply `SMF_INTENSITY_MULT = 210.0 / 255.0` to the
ambient uniform on the CPU side. Don't multiply per-fragment.

**§7.2**: gate DNTS on `splatDistrTex && splatDetailNormalTex[]`,
NOT on `specularTex`. WGSL:
```wgsl
let dnts_enabled = bool(uniforms.has_dnts_distribution)
    && bool(uniforms.dnts_layer_count > 0u);
```

**§7.3**: per-channel UV multipliers, decode each layer
`* 2.0 - 1.0` across full RGBA, splatCofac = dist × texMults,
blend strength = `clamp(dot(splatCofac, vec4(1.0)), 0.0, 1.0)`.

```wgsl
let dist = textureSample(splat_distribution, sampler_lin, world_uv).rgba;
let cofac = dist * uniforms.splat_tex_mults;
let blend_strength = clamp(dot(cofac, vec4<f32>(1.0)), 0.0, 1.0);

let n0 = textureSample(slot_normals[0], sampler_lin, world_uv * uniforms.splat_tex_scales.r).rgba * 2.0 - 1.0;
let n1 = textureSample(slot_normals[1], sampler_lin, world_uv * uniforms.splat_tex_scales.g).rgba * 2.0 - 1.0;
let n2 = textureSample(slot_normals[2], sampler_lin, world_uv * uniforms.splat_tex_scales.b).rgba * 2.0 - 1.0;
let n3 = textureSample(slot_normals[3], sampler_lin, world_uv * uniforms.splat_tex_scales.a).rgba * 2.0 - 1.0;

let blended_normal = (n0 * cofac.r + n1 * cofac.g + n2 * cofac.b + n3 * cofac.a) / max(1e-3, dot(cofac, vec4<f32>(1.0)));
```

**§7.4**: build TBN per-fragment from `cross(normal, vec3(-1, 0, 0))`.
Do NOT assume a static `T = +X, B = +Z` basis.

**§7.5**: base normal map is `R+A` only. `nx = normalsTex.r; nz =
normalsTex.a; ny = sqrt(max(0.0, 1.0 - nx*nx - nz*nz));`.

**§7.6**: specular exponent = `specularCol.a * 16.0` (per-fragment,
from specular texture's alpha). Global `lighting.specularExponent`
only consulted when `has_specular_tex == 0u`.

### 3. Subsume Sprint 9 / D4's splat shader

Sprint 9 / D4 (ADR-036) shipped a minimal splat shader. Sprint 25
replaces it. Drop:
- `compose_splat` function in the old shader.
- `inspector_splat` references in `render.rs` (already retired
  Sprint 17, just verify).
- Any `// Sprint 9` comments still applicable to the old path.

Per ROADMAP.md "Sequencing" section: ADR-036 is retired or amended;
ADR-038 (new) covers the unified terrain shader.

### 4. ADR-038 (new)

`/home/teague/code/BARMapEditor/docs/DECISIONS.md` — new entry:

```
## ADR-038 — Unified terrain shader (Sprint 25 / R1)

Status: ADOPTED 2026-05-XX
Context: Sprint 13 (ADR-037) shipped depth + offscreen RT but kept
the placeholder splat shader from Sprint 9 (ADR-036). Sprint 25
replaces the placeholder with a full port of `SMFFragProg.glsl`...
```

Cover: scope, source mapping (line-for-line table of GLSL → WGSL),
texture bind order, uniform layout, fallback texture handling,
deferred items (see Out of Scope).

### 5. Validation fixtures

Build a small fixture suite in `crates/barme-app/tests/parity_fixtures.rs`:

- **Reference BAR map**: extract `Comet_Catcher_Remake.sd7`'s
  textures + heightmap; place in `assets/parity-fixtures/comet/`.
  Build a `Project` that points to those exact assets.
- **Render**: open the fixture in headless mode; render at
  3 standard camera angles (top-down, 35°, grazing); save PNGs.
- **Side-by-side**: a manual visual diff against BAR screenshots
  (stashed in `assets/parity-fixtures/comet/bar-reference/`).
- **Acceptance**: visually indistinguishable at 2-8 SMU camera
  distances. Sprint 36 ships the automated ΔE harness; Sprint 25
  ships the fixture extraction + reference screenshots.

**Platform-portability checklist** (mandatory for renderer
sprints):
- WGSL only — no Vulkan-specific extensions, no Metal-specific
  syntax.
- Test on at least Linux Vulkan + Linux GL (Mesa software
  fallback). Document failures.
- macOS Metal: untested (no CI runner). Document the limitation.
- Windows D3D12: untested. Document.

### 6. Rollup

STATUS UPDATEs in SRS / ROADMAP (R1 done; renderer-parity arc
1/8). closing devlog log. "Sprint 26 = water polish (fresnel /
foam / caustics / perlin / refraction / reflection)" handoff.

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on shader recompile;
`warn!` on missing texture binds; `trace!` on per-frame uniform
upload.

## Step 5 — Out of scope

- **DNTS slot NORMALS** beyond the 4-array (Sprint 9 / D2 only
  bakes diffuse + normal pair; the 4-layer DNTS-bound limit
  applies here). 16-slot composited diffuse stays per Sprint 17
  but only 4 active DNTS-bound layers feed the runtime shader.
- **Atmosphere / fog** — Sprint 28.
- **Water polish** — Sprint 26.
- **Shadows** — Sprint 30.
- **Features (S3O / 3DO)** — Sprint 29.
- **Grass** — Sprint 34.
- **Emission / sky-reflect / parallax** — Sprint 35.
- **High-pass diffuse alpha (ADR-034)** — schema field stays
  plumbed but DNTS shader still consumes the layer mask's alpha;
  ADR-034 deferred.

## Step 6 — Critical pitfalls (read twice)

1. **FINDINGS §7.1**: `SMF_INTENSITY_MULT` is **pre-applied to the
   ambient uniform on the CPU side**, NOT multiplied per-fragment.
   Get the math wrong and the entire terrain reads too dark or
   too bright by 20%. Pin a fixture that visually matches BAR's
   default Comet Catcher Remake at sun=(0.7,0.5,0.5).

2. **FINDINGS §7.2**: DNTS gating is on `splatDistrTex &&
   splatDetailNormalTex[]`, NOT on `specularTex`. A common mistake
   is to AND-in the specular check; that disables DNTS on
   specular-less maps. Don't.

3. **FINDINGS §7.3**: per-channel UV multipliers are
   `splats.texScales.r/g/b/a` — each channel can have a different
   tex_scale. The blend uses dot(cofac, 1) for strength, not max
   or per-channel.

4. **FINDINGS §7.4**: TBN from `cross(normal, vec3(-1, 0, 0))`.
   Static TBN (T=+X, B=+Z) is the standard noob mistake; it
   causes normal maps to "swim" on sloped surfaces. Test on a
   parabolic dome.

5. **FINDINGS §7.5**: base normal map uses R + A channels only.
   Test fixtures with `R=255, G=0, B=0, A=128` should
   reconstruct a normal pointing east (-1, sqrt(1-1-0), -0) which
   normalises into the correct direction. Verify.

6. **FINDINGS §7.6**: specular exponent is per-fragment
   (specularCol.a × 16.0). Global `lighting.specularExponent` is
   ONLY consulted when no specular texture is bound. Get this
   wrong and the entire terrain looks plastic.

7. **Bind-group layout cap**: wgpu's `MAX_BIND_GROUPS_PER_PIPELINE`
   is 4 on most hardware. We have 7 groups in the proposed layout
   — split across multiple `wgpu::BindGroupLayout`s. Use
   `wgpu::BindGroup` array of up to 4 groups per draw; pack texture
   arrays where possible. Verify on a Vega 8 iGPU before merging.

8. **Default-texture fallbacks**: every bound texture needs a 1×1
   fallback texture for the "not loaded yet" state. WGSL's
   uniform branching on `has_X_tex` flags is fine but adds shader
   complexity; static binds with default textures are cleaner.

9. **Sprint 9 / D4 deprecation**: don't leave dead code paths in
   `render.rs`. The old splat shader's bind-group setup gets
   replaced entirely; remove `// TODO: Sprint 9` and friends.

10. **WGSL `textureSample` inside conditional branches**: WGSL
    requires uniform control-flow for `textureSample` outside
    fragment-default-sampler. If you branch on
    `has_normal_tex`, hoist the sample out: always sample, then
    blend with `mix(fallback, sample, has_normal_tex_factor)`.

11. **Platform portability**: see the checklist in chunk 5. Lavapipe
    (Mesa Vulkan software impl) is slow but the most accurate test
    for "does it work on Linux without a GPU." macOS / Windows
    untested — call out the gap in the devlog and Sprint 33 closes
    it with CI.

12. **Test against the editor's existing presets**: Comet Catcher
    Remake is a great test (real-world DNTS + specular). All That
    Simmers (wasteland) and a void-water-style map round out the
    visual coverage.

13. **PR-style review**: ask a teammate / second pair of eyes to
    visually compare editor vs BAR before merging. The 8-pixel
    differences are easy to spot, the 4-pixel ones aren't. Sprint
    36 ships ΔE automation; this sprint relies on human review.

## Step 7 — Exit criteria

- 4+ commits on `main`: render-state plumbing, shader port,
  fixture suite + ADR, rollup.
- 1 devlog folder filled.
- SRS / ROADMAP STATUS UPDATEs (R1 done, renderer-parity arc 1/8).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test:
  - Load Comet Catcher Remake fixture → render top-down,
    35°, grazing → visually indistinguishable from BAR
    reference at 2-8 SMU camera distance.
  - DNTS slots blend correctly across 4 channels.
  - Specular highlights track sun on per-fragment basis.
  - Base normal map causes the right hue shift on sloped
    surfaces.
- Final devlog: summary + "Sprint 26 = water polish (fresnel /
  foam / caustics / perlin / refraction / reflection)" handoff.

Start by reading `SMFFragProg.glsl` end-to-end with FINDINGS §7
open beside it. Then write the WGSL line-for-line with comments
citing the GLSL source line. The render.rs plumbing is mechanical
once the shader is right.
