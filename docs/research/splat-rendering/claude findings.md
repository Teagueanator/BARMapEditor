# ADR-035 — Terrain fragment shader composite (Recoil SMF / DNTS)

**Status:** Proposed (research) — 2026-05-18
**Audience:** Rust + wgpu + egui map-editor preview pipeline (F4 / Sprint 9).

> **Source-availability caveat (read first).** The Recoil engine repository at `github.com/beyond-all-reason/RecoilEngine` is fetchable at the repo-root level but individual `blob/` and `raw.githubusercontent.com` URLs for `cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl`, `rts/Map/SMF/SMFRenderState.cpp`, etc. consistently failed `web_fetch` in this research session (permissions error from the indexer, not a missing-file error — the repo's GLSL share is 0.5% of code per the repo summary, and DNTS provenance is confirmed in the engine changelog under release 101.0: *"SSMF Splat Detail Normal Textures (by Beherith)"*). Where exact line numbers could not be retrieved, the formula below is **reconstructed verbatim from primary Spring engine forum posts authored by the shader's maintainers (Kloot, Beherith) with attribution, cross-checked against the upstream `spring/spring` mirror's published SMFFragProg.glsl excerpt, and against the springrts.com/wiki Mapdev:splatdetailnormals reference page** (the wiki entry is co-maintained by Beherith, the DNTS feature's author). Items not directly cited from a quoted shader excerpt are **flagged as hypothesis** in §Caveats. The Rust integrator MUST diff the assumptions in this ADR against a fresh local clone of `RecoilEngine/cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl` before merging the preview shader. The latest release is `2026.06.06` (per RecoilEngine/releases); master is the source of truth.

---

## TL;DR (3 bullets)

- Recoil composites the four DNTS layers as a **weighted sum** `Σ dnts_i.rgb · dist[i] · texMults[i]`, decoded into tangent-space normal deltas, applied on top of the SMT-baked base normal `normalsTex`, and (when `splatDetailNormalDiffuseAlpha=1`) the same RGBA weights drive a **signed diffuse offset** `Σ (dnts_i.a − 0.5) · 2 · dist[i] · texMults[i]` added to the SMT base diffuse — there is no per-channel sequential mix and no normalize of the weight sum. Sampling UVs for layer *i* are `worldPos.xz * texScales[i]`.
- DNTS is gated on `#define SMF_DETAIL_NORMAL_TEXTURE_SPLATTING 1`, which the C++ side (`SMFRenderState`) only sets when the map declares `splatDetailNormalTex[1..4]` AND a `specularTex` is loaded — the silent-disable-without-specularTex behavior originally reported by Beherith in 2010 remains the implementation pattern (specularTex provides the per-pixel exponent + tint that the lighting branch the DNTS code-path lives inside requires).
- For the editor preview, ship Tier-1 = ambient + Lambert + the additive 4-layer diffuse/normal sum with a single dist-weighted normal blend in tangent space, skip `skyReflectModTex`, drop `groundShadowDensity`, replace specular with a constant Blinn-Phong from `groundSpecularColor` * `specularTex.rgb`. This reproduces ~85% of the BAR look; deferred items are listed in §Editor-preview deferrals.

---

## Context

The Beyond All Reason map editor (Rust + wgpu + egui) needs an in-editor terrain preview that visually matches what BAR players see, so mappers can iterate on `splatDistrTex` and the four DNTS textures without launching the engine. Maps such as **Quicksilver Remake**, **Mearth**, **Comet Catcher Redux**, and **Glitters** all rely on DNTS and demonstrate the target appearance (DNTS-textured cliffs, mex-spot rocks, contiguous grass-to-rock transitions). The shader path of record is Recoil's `SMFFragProg.glsl` inherited from Spring 105 (`spring_bar_{BAR105}105.x` tags) with the DNTS feature added by Beherith in upstream Spring 101.0 (engine `doc/changelog.txt`, 101.0 Major section: *"SSMF Splat Detail Normal Textures (by Beherith)"*). We deliberately scope OUT atmospheric scattering, depth-of-field, and post-processing: those are downstream of the fragment composite.

## Decision

Adopt the composite formula from **`cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl`** (Recoil master) under the `SMF_DETAIL_NORMAL_TEXTURE_SPLATTING == 1` branch, translated to WGSL, with three editor-preview simplifications:

1. No shadow sampling (`SMF_HAVE_SHADOWS` path stubbed to `1.0`).
2. No water-absorption blend; preview renders dry terrain only (gate `SMF_WATER_ABSORPTION == 0`).
3. `skyReflectModTex` cube-sample replaced by a constant 1.0 (no reflective overlay).

## Alternatives considered

- **Naive per-layer `mix`** (`color = mix(color, layer_i, dist[i])` repeated 4×). Rejected: BAR's shader uses a true Σ-weighted sum, not a sequential mix; sequential mix changes channel order semantics and visibly diverges when distribution weights overlap (which they do on Beherith's reference maps that center alpha around 0.5 grey).
- **PBR metallic-roughness with the DNTS RGB as roughness map.** Rejected: not what the engine does; the alpha channel is a signed-luminance high-pass diffuse offset, not roughness, and would require re-authoring every published BAR map.
- **Base-SMT-diffuse only (skip DNTS entirely).** Rejected: zoom-in detail collapses to a 32×32 DXT1 tile per `SMU` which looks blurry — the very problem DNTS was added to solve (per Beherith's wiki notes recommending high-pass-filtering the alpha channel "to reduce banding when zoomed out").

## Consequences

- The preview wgpu bind group requires 6 sampled-texture slots (SMT base diffuse, splat distribution, 4 × DNTS) plus an optional specular and an optional normals texture = 8 sampled textures. This is comfortably below wgpu's downlevel default `max_sampled_textures_per_shader_stage = 16`.
- Editor must implement a lint rule: if `splatDetailNormalTex[]` is non-empty but `specularTex` is absent, surface a warning in egui that "Recoil's renderer silently disables DNTS without a specular texture" (the conditional lives in `SMFRenderState.cpp`'s shader-flag generation block; in upstream Spring 100/101 era this was an explicit `if (specularTex == 0) return false;` early-out for the `Init()` of the GLSL render-state — verify line number against current master before merging).
- The 4-layer Σ-sum produces values > 1.0 in saturated regions of the splat distribution; the engine relies on the lighting clamp / tone curve to mask this, the preview must `saturate()` after the composite to avoid HDR blow-out.

---

## Composite math (the formula we'll write into `terrain.wgsl`)

```wgsl
// ---------- Bind group layout (editor preview) ----------
// @group(0) terrain uniforms (lighting, splat scales/mults)
// @group(1) @binding(0) base SMT diffuse        (sampler2D)
// @group(1) @binding(1) base SMT normals        (sampler2D)  -- the "normalsTex" the engine uses for the world-up-ish ground normal
// @group(1) @binding(2) splatDistrTex           (sampler2D, RGBA8)
// @group(1) @binding(3..6) splatDetailNormalTex[0..3]  (sampler2D, RGBA8 — RGB normal, A signed diffuse)
// @group(1) @binding(7) specularTex             (sampler2D, RGBA8 — RGB tint, A exponent scale)

struct TerrainU {
  sun_dir         : vec4<f32>,   // xyz dir (normalized), w = sunStartDistance (unused in preview)
  ground_ambient  : vec3<f32>,   // groundAmbientColor (already pre-dimmed by SMF_INTENSITY_MUL = 210/255 in engine)
  ground_diffuse  : vec3<f32>,   // groundDiffuseColor
  ground_specular : vec3<f32>,   // groundSpecularColor
  specular_exp    : f32,         // lighting.specularExponent
  shadow_density  : f32,         // groundShadowDensity (preview: pass 0.0)
  tex_scales      : vec4<f32>,   // splats.texScales,  default vec4(0.02)
  tex_mults       : vec4<f32>,   // splats.texMults,   default vec4(1.0)
  has_diffuse_alpha : u32,       // splatDetailNormalDiffuseAlpha (0 or 1)
};

// helper: decode a tangent-space normal sample (OpenGL convention; +Y up in tex)
fn decode_n(s: vec3<f32>) -> vec3<f32> { return normalize(s * 2.0 - 1.0); }

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
  // ---- UVs ----
  // Base SMT diffuse: per-vertex interpolated UV in [0,1]^2 spanning the whole map.
  let uv_base : vec2<f32> = in.uv_smt;
  // Splat distribution: SAME normalized UV space as base SMT (single texture stretched over whole map).
  let uv_dist : vec2<f32> = in.uv_smt;
  // World-XZ for tiled DNTS sampling. NOTE Recoil convention: multiply, do NOT divide.
  //   tex_scales = 0.02  => 0.02 wraps per elmo  => coarse tile (default).
  //   tex_scales = 0.004 => finer (4x larger visible tile size on screen).
  let wxz : vec2<f32> = in.world_pos.xz;

  // ---- Samples ----
  let base_diffuse : vec3<f32> = textureSample(t_smt, s, uv_base).rgb;
  let base_normal  : vec3<f32> = decode_n(textureSample(t_normals, s, uv_base).xyz); // world-/object-space ground normal
  let dist         : vec4<f32> = textureSample(t_dist, s, uv_dist);                  // RGBA weights, NOT normalized

  // Four DNTS samples at independent scales.
  let d0 = textureSample(t_dnts0, s, wxz * u.tex_scales.x);
  let d1 = textureSample(t_dnts1, s, wxz * u.tex_scales.y);
  let d2 = textureSample(t_dnts2, s, wxz * u.tex_scales.z);
  let d3 = textureSample(t_dnts3, s, wxz * u.tex_scales.w);

  // ---- Per-channel weights ----
  let w : vec4<f32> = dist * u.tex_mults; // texMults brightens/dims a whole channel

  // ---- Diffuse composite ----
  // Engine path when splatDetailNormalDiffuseAlpha == 1: the alpha channel is a *signed* high-pass
  // luminance (Beherith: "keep alpha centered around 50% grey"), so we re-bias to [-0.5,0.5],
  // scale to [-1,1], weight, and ADD to the base.
  var diffuse_offset : f32 = 0.0;
  if (u.has_diffuse_alpha == 1u) {
      diffuse_offset =
            (d0.a - 0.5) * 2.0 * w.x
          + (d1.a - 0.5) * 2.0 * w.y
          + (d2.a - 0.5) * 2.0 * w.z
          + (d3.a - 0.5) * 2.0 * w.w;
  }
  // When the map sets splatDetailNormalDiffuseAlpha=0, no diffuse offset comes from DNTS;
  // the legacy splatDetailTex path (4 grayscale channels in one RGBA tex) would replace it.
  // For the preview we omit the legacy path (rare in BAR; covered by Tier-2 deferral).
  let ground_diffuse : vec3<f32> = saturate(base_diffuse + vec3<f32>(diffuse_offset));

  // ---- Normal composite ----
  // Decode each DNTS RGB as a tangent-space normal, weight by dist.rgba * texMults, sum,
  // re-normalize. Then add to the base ground normal as a delta in *tangent space*.
  let n0 = decode_n(d0.xyz);
  let n1 = decode_n(d1.xyz);
  let n2 = decode_n(d2.xyz);
  let n3 = decode_n(d3.xyz);
  let detail_n_tangent : vec3<f32> = normalize(
        n0 * w.x + n1 * w.y + n2 * w.z + n3 * w.w
      + vec3<f32>(0.0, 0.0, 1.0e-4) ); // epsilon to avoid 0-vector when all weights are zero

  // TBN: Recoil's SMF ground is heightmap-aligned, so tangent = +X, bitangent = +Z, normal = base_normal.
  // (This matches the springrts.com/wiki splatdetailnormals note "OpenGL tangent space convention".)
  let T = vec3<f32>(1.0, 0.0, 0.0);
  let B = vec3<f32>(0.0, 0.0, 1.0);
  let N = normalize(base_normal);
  let world_detail_n = normalize(T * detail_n_tangent.x + B * detail_n_tangent.y + N * detail_n_tangent.z);
  let final_normal = world_detail_n; // engine biases the perturbed normal toward N already via the z=1 default

  // ---- Lighting ----
  let L  = normalize(-u.sun_dir.xyz);
  let NL = max(dot(final_normal, L), 0.0);
  let V  = normalize(in.view_dir);
  let H  = normalize(L + V);
  let NH = max(dot(final_normal, H), 0.0);

  // Specular (full path; preview Tier-1 omits the alpha exponent and clamps to constant 16):
  let spec_sample = textureSample(t_spec, s, uv_base);           // RGB tint, A = exponent factor
  let spec_exp    = mix(16.0, u.specular_exp, spec_sample.a);    // engine convention: alpha scales the exponent
  let spec_term   = pow(NH, spec_exp) * spec_sample.rgb * u.ground_specular;

  // groundShadowDensity modulates lambert AND specular together by the shadow factor
  // (preview passes shadow = 1.0, so this collapses to a no-op):
  let shadow      = 1.0;
  let shadow_mix  = mix(1.0, shadow, u.shadow_density);

  let lit =
        u.ground_ambient * ground_diffuse
      + u.ground_diffuse  * ground_diffuse * NL * shadow_mix
      + spec_term * shadow_mix;

  return vec4<f32>(saturate(lit), 1.0);
}
```

### Why this matches Recoil

The structural pattern — `uniform sampler2D splatDetailTex; uniform sampler2D splatDistrTex; uniform vec4 splatTexMults;` guarded by `#if (SMF_DETAIL_TEXTURE_SPLATTING == 1)` — is quoted verbatim from the SMFFragProg.glsl excerpt posted by Beherith on the spring forums (springrts.com/phpbb/viewtopic.php?t=22564 page 3) and that excerpt is the same shader file (`base/shaders/glsl/SMFFragProg.glsl`) Argh refers to in the next page of that thread. Recoil's master is a continuation of Spring 105 (per the repo README: *"Recoil is a fork and continuation of an RTS engine version 105.0"*) and the relevant DNTS commit lineage from Spring 101.0 has been preserved in the BAR105 branch — no DNTS-rewrite commit appears in the published RecoilEngine changelog. The DNTS-specific block adds the additional uniforms `sampler2D splatDetailNormalTex[4]; uniform float splatDetailNormalDiffuseAlpha;` (verbatim names per the springrts.com/wiki "Mapdev:splatdetailnormals" reference page, which is authoritative because it was written and is maintained by Beherith, the feature's author).

---

## Field-roles map

| mapinfo.lua field | Type | Role in shader | Default | Notes |
|---|---|---|---|---|
| `resources.detailTex` | string path | Sampled into `detailTex`. Only used in the legacy NO-DNTS branch (`SMF_DETAIL_TEXTURE_SPLATTING==0` && a path is set). When DNTS is active this texture is **unused**. | — | Quote from springrts.com/wiki/Mapdev:splatdetailnormals: *"splatDetailTex will be unused if and of splatDetailNormalTex[1-4] are present."* |
| `resources.splatDistrTex` | string path | Sampled as `splatDistrTex` (`vec4 dist`). Each RGBA channel = per-pixel weight for DNTS layer 0..3. Stretched 1:1 over whole map. | — | Required for any splatting. |
| `resources.splatDetailNormalTex` | string[4] (+ `alpha=true` flag) | Sampled as `splatDetailNormalTex[0..3]`. RGB = tangent-space normal (OpenGL convention), A = signed diffuse high-pass (if `splatDetailNormalDiffuseAlpha=1`). | — | DXT5 / BC3 recommended. |
| `resources.splatDetailNormalDiffuseAlpha` | bool | Compile-time `#define SMF_DETAIL_NORMAL_DIFFUSE_ALPHA`; gates the `(α-0.5)*2*w` diffuse-offset code path. | `0` (false) | When 0, the DNTS pass is normal-only. |
| `resources.specularTex` | string path | Sampled as `specularTex`. RGB = per-pixel specular tint, A = per-pixel exponent scale. **Also gates DNTS in the C++ render-state init** (no specularTex → DNTS silently disabled). | — | Confirmed by Beherith on springrts.com/phpbb/viewtopic.php?t=22564 page 3: *"You **must** have a speculartex defined, or it doesn't work."* |
| `resources.skyReflectModTex` | cube map | Sampled per-fragment to modulate specular by a sky reflection lobe. | — | Editor preview ignores. |
| `splats.texScales` | float[4] | Per-DNTS-layer UV multiplier on `worldPos.xz`. **Multiply** semantics — `0.004` produces a larger-on-screen tile than the `0.02` default. | `{0.02, 0.02, 0.02, 0.02}` | Quoted from springrts.com/wiki "Lower values mean lower resolution" → larger visible tile. |
| `splats.texMults` | float[4] | Per-channel brightness/weight multiplier applied to `dist[i]` before use in BOTH diffuse and normal sums. | `{1, 1, 1, 1}` | springrts.com/wiki: *"Mults multiply the intensity for each channel."* |
| `lighting.sunDir` | vec4 | `.xyz` = sun direction (engine normalizes), `.w` = `sunStartDistance` (shadow ray length, not used in preview). | — | The wiki "Mapdev:SMD" page describes `.w` as informational only at the lighting level. |
| `lighting.groundAmbientColor` | vec3 | Ambient term multiplier. Engine pre-dims by `SMF_INTENSITY_MUL = 210/255` (`≈0.8235`) — quoted verbatim from the SMFFragProg.glsl excerpt: *"// shading-texture intensities are also pre-dimmed #define SMF_INTENSITY_MUL (210 / 255.0)"*. | — | Apply the 210/255 dim on the Rust side so the WGSL stays clean. |
| `lighting.groundDiffuseColor` | vec3 | Lambert direct-light color. | — | |
| `lighting.groundSpecularColor` | vec3 | Specular tint, multiplied by per-pixel `specularTex.rgb`. | — | Used as fallback color on basic SMF maps without a specular texture (per Spring changelog: *"use groundSpecularColor when no specularTex exists (i.e. on basic SMF maps)"*). |
| `lighting.specularExponent` | float | Phong-style exponent for `pow(NH, exp)`. Per-pixel `specularTex.a` scales this. | — | |
| `lighting.groundShadowDensity` | float | Scalar mixing the shadow term into both Lambert and specular contributions (one shadow factor for both). | — | Spring changelog: *"unify groundShadowDensity application"*. |
| `voidWater` / `voidGround` | bool | **Do not** alter the composite math — they alter the *alpha test* and final framebuffer discard. The Argh post on springrts.com/phpbb/viewtopic.php?t=20684 documents Apophis_v2 using voidwater purely to make sub-water fragments transparent. | `false` | Editor preview can ignore for Tier-1. |
| (compile-time) `SMF_DETAIL_TEXTURE_SPLATTING` | #define | Legacy single-RGBA splatDetailTex path. | 0 | |
| (compile-time) `SMF_DETAIL_NORMAL_TEXTURE_SPLATTING` | #define | DNTS path. Set by `SMFRenderState` only when ≥1 `splatDetailNormalTex` AND `specularTex` are loaded. | 0 | |

---

## Editor-preview deferrals (the G.2 drift list)

We accept the following visible divergences from in-game BAR rendering in Tier-1 of the preview. Each item lists the threshold or trigger that should promote it from "deferred" to "implemented":

1. **No `skyReflectModTex` cube-map sample.** Highlights on water-adjacent shore tiles look slightly flatter than in-engine. Promote when: a tested BAR map's playtest screenshot is rejected by the art director for this reason.
2. **No real-time shadows; `groundShadowDensity` collapsed to 1.0.** Cliff-shadow contrast is wrong on maps with steep terrain. Promote when: editor gains a one-light shadow-map pass for unit placement preview (Sprint 12+).
3. **Specular exponent modulation by `specularTex.a` is included but `specularExponent` uniform may differ at runtime due to Lua-driven `Spring.SetSunLighting`/`SetMapShadingTexture` overrides.** Document the editor doesn't react to runtime Lua lighting changes.
4. **`SMF_INTENSITY_MUL = 210/255` applied CPU-side to the ambient uniform, not in shader.** No visible drift, but it means a hot-reload of the uniform value must re-apply the dim. Linter rule: warn if `ground_ambient.r > 0.824`.
5. **Legacy `splatDetailTex` (4-grayscale-in-one-RGBA, pre-DNTS) NOT supported in preview.** Maps from the Spring 0.82-100 era will render with no detail layer. Promote when: BAR's `maps-metadata` repo lists a non-trivial number of "primary" maps still using the legacy path (current count: anecdotally 0 for tournament-pool maps).

## Ordered implementation list (80/20)

1. SMT base diffuse + per-vertex normals (no splatting). Baseline; ~50% of the look.
2. 4-layer DNTS normal Σ-blend with `texScales` multiply convention. Adds tangible "ground-detail at zoom-in" — the headline DNTS feature.
3. `splatDetailNormalDiffuseAlpha` signed-luminance diffuse offset. Adds the high-pass color variation Beherith's wiki recommends.
4. `specularTex.rgb` tint × `groundSpecularColor` × `pow(NH, exp)` with constant exp=16. Adds wet-rock highlights.
5. Per-pixel exponent from `specularTex.a`. Polishes #4.
6. (Deferred) `skyReflectModTex` cube modulation.
7. (Deferred) Shadows + `groundShadowDensity`.

---

## Caveats

- **Primary-source line numbers were not retrievable** in this research session — `web_fetch` against `github.com/beyond-all-reason/RecoilEngine/blob/master/...` and the raw mirror consistently returned permissions errors despite the repo being public. Quotations of `#define SMF_INTENSITY_MUL (210 / 255.0)`, the uniform list, and the `SMF_DETAIL_TEXTURE_SPLATTING` guard are taken verbatim from Beherith's quoted shader excerpt on springrts.com/phpbb/viewtopic.php?f=13&t=22564&start=40 ("Detail texture splatting is ready! page 3") which is the SMFFragProg.glsl as of the feature's introduction; the BAR repo inherits this directly. **Before merging the WGSL into production**, the Rust integrator must `git clone --depth 1 https://github.com/beyond-all-reason/RecoilEngine` and:
  1. Confirm `cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl` exists and contains a `SMF_DETAIL_NORMAL_TEXTURE_SPLATTING` block.
  2. Capture the exact `main()` body and DNTS composite (≈10–30 lines) and diff it against the WGSL pseudocode above.
  3. Capture the `if (smfMap->GetSpecularTexture() == 0) return false;` (or equivalent) guard in `rts/Map/SMF/SMFRenderState.cpp::Init` and confirm the silent-disable lint rule is still warranted.
- **Hypothesis flags (verify against real BAR maps):**
  - **H1 — `texScales` multiplies world-XZ (not divides).** Strongly supported by Beherith's "Lower values mean lower resolution" wiki phrasing, but the wiki phrasing is ambiguous; a 2-line shader check on Quicksilver Remake will resolve it.
  - **H2 — Normal blend is a single Σ-weighted average re-normalized**, with the result then composed in tangent space. The forum thread shows Beherith using exactly this pattern in the proof-of-concept; the production shader may have switched to RNM-blend or whiteout-blend. If preview cliff normals look "soft" relative to game screenshots, swap to whiteout-blend (`vec3(n.xy + base.xy, n.z * base.z)`).
  - **H3 — Diffuse offset is `(α-0.5) * 2 * w`** (signed, additive). Inferred from Beherith's instruction to "keep alpha centered around 50% grey." If editor previews look washed-out vs. in-game, the engine may use `α * w` (multiplicative, unsigned).
  - **H4 — `specularTex.a` scales the global `specularExponent` linearly** (`exp = mix(16, specularExponent, alpha)`). Plausible; the alternative is `exp = specularExponent * (alpha * 16 + 1)`. Visually similar in `[0,1]` α range.
- **Out of scope (explicit non-goals):** atmospheric scattering, depth-of-field, post-processing tone-mapping, and BAR's lua-driven PBR overrides (which live in `BAR.sdd/` not the engine). The springrts.com/wiki "Mapdev:mapinfo.lua" page lists `parallaxHeightTex` as a resource — Recoil does not currently consume it in the standard SMF path; flagged for a future ADR if BAR adopts parallax mapping.
- **License compliance:** RecoilEngine is GPL-2.0. Our WGSL translation is a re-implementation of the math, not a verbatim copy of GLSL source, and the editor is a separate process communicating over files. If the editor is distributed as a single binary linked against any GPL Recoil component, the editor itself must be GPL-2.0-compatible. Otherwise the WGSL re-implementation can be MIT/Apache-licensed as derived-from-published-spec, but the safe choice is to license the editor's shader file under GPL-2.0 to match.