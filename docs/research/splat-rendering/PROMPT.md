# Research prompt — Recoil terrain shader + splat / DNTS composite math

**Use:** Paste the section below verbatim into a fresh Claude (or Gemini)
deep-research session. The session has no prior context on this codebase;
everything it needs is in the prompt. As with prior research topics, run
BOTH a Claude and a Gemini session and save outputs to
`claude-research-findings.md` and `gemini-research-findings.md` for
side-by-side review.

**Expected output:** a shader-pseudo-code spec + a field-roles map letting
us write a wgpu fragment shader (WGSL) that reproduces the Recoil terrain
look closely enough to author maps reliably. We will adopt the accepted
formula as `ADR-035` (or whichever ADR number is next when the research
lands).

---

## Prompt (copy from here)

You are scoping the **terrain fragment shader** for an open-source desktop
map editor called *BAR Map Editor*. It is a single-binary Rust + egui +
wgpu app that produces playable `.sd7` map archives for **Beyond All
Reason** (BAR), a free RTS built on the Recoil engine (a fork of Spring RTS
105). PyMapConv (the canonical SMF/SMT compiler, license CC0-1.0) runs as
a bundled sidecar; the editor authors a heightmap + a splat distribution
texture + 4 DNTS detail textures, then PyMapConv compiles the heightmap
into the `.smf` while the splat distribution + DNTS files ship inside the
`.sd7` and are loaded by Recoil at runtime via the `mapinfo.lua`
`resources` block.

The editor renders a live 3D preview of the terrain using the same source
assets BAR will see at runtime. The preview SHOULD diverge from in-game
final pixels (we don't try to reproduce BAR's atmospheric scatter, dynamic
shadows, etc.), but it MUST be visually believable: the dominant base
colour, splat blend regions, and overall lighting direction need to look
"like the same map." Today the editor renders heightfield-only with no
texture sampling at all — it's a Stage-0 prototype shader.

**The decision you are scoping:** *what fragment-shader formula composites
the 4 DNTS detail textures via the splat distribution texture, and what
auxiliary inputs (specular, base detail, normal blending, lighting) does
it need?* The output should be a WGSL-pseudocode shader plus a clear map
of every `mapinfo.resources.*` and `mapinfo.splats.*` field to its role in
the formula.

This is needed before we can implement F4-stage shader work (Sprint 9 in
our internal planning), because the splat brush preview must reflect the
in-game blend or users will paint to the wrong look.

### Background facts we already know

You can take these as given — don't re-research them, just cite them when
useful.

1. **Engine**: Recoil RTS engine
   (`github.com/beyond-all-reason/RecoilEngine`, GPL-2.0; fork of Spring
   105). The terrain renderer lives under `rts/Map/SMF/`. Files of
   particular interest: `SMFRenderState.cpp`,
   `SMFGroundDrawer.cpp`, `SMFReadMap.cpp`. The actual fragment shader
   source is in `cont/base/springcontent/shaders/GLSL/`.

2. **Splat distribution**: a single `.png` texture whose 4 RGBA channels
   are per-pixel weights for the 4 detail textures. Authored by the
   user via splat brushes.

3. **DNTS** = Detail Normal Texture Splatting. 4 DDS files
   (`splatDetailNormalTex1..4` in `mapinfo.resources`), each in
   `BC3 / DXT5` with normal-in-RGB. **Recoil uses OpenGL tangent
   space** (per source-audit FINDINGS §7.4). DirectX-convention
   source normals (e.g. Substance / Quixel exports) need a
   green-channel inversion before bake; OpenGL-convention sources do
   NOT. ambientCG ships BOTH `_NormalGL.*` and `_NormalDX.*` in each
   ZIP — the D1-shipped starter pack (`scripts/fetch-textures.sh`,
   ADR-025) extracts only `_NormalGL.png`, so no flip is needed for
   it. Primary-source evidence that BAR maps sometimes need the
   flip: the `_flipped` filename suffix on shipped BAR DNTS files
   like `Metal_BrushedMetalTilesDirty_2k_dnts_flipped.dds`, which
   originate from DirectX-convention source pipelines. Our bake
   pipeline (D2 / ADR-026) makes the Y-flip a configurable
   `BakeOptions::yflip_normal` knob, default OFF for the starter
   pack and exposed at F23 (user-import) for DX-source imports.

4. **`splatDetailNormalDiffuseAlpha`** (Boolean field in
   `mapinfo.resources`): when `true`, the alpha channel of each DNTS DDS
   carries a high-pass-filtered diffuse instead of being `0xFF` solid.
   When `false` (the safer default we ship), alpha is solid and the
   shader uses an external `detailTex` for the base diffuse.

5. **`specularTex`** (texture path in `mapinfo.resources`): a global
   specular texture. **Documented gotcha**: the `splatDetailNormalTex`
   path silently disables if `specularTex` is missing. We have not
   verified this in current Recoil — that's part of your research.

6. **`detailTex`** (texture path in `mapinfo.resources`): a global detail
   texture overlay. Used in non-DNTS maps and possibly as the "base
   diffuse" in DNTS maps when `splatDetailNormalDiffuseAlpha = false`.

7. **`splatDistrTex`** (texture path in `mapinfo.resources`): the splat
   distribution itself, in `.png` or `.bmp` form.

8. **`splats.texScales`** (float[4]): per-channel UV scale multiplier on
   each DNTS sample. Default `{0.02, 0.02, 0.02, 0.02}`. Real maps drop
   as low as `{0.004, 0.007, 0.008, 0.0015}` (Enceladus example).

9. **`splats.texMults`** (float[4]): per-channel intensity multiplier.
   Default `{1, 1, 1, 1}`. Semantic unknown — possibly brightness scaling
   on each DNTS sample.

10. **SMF binary** holds the base diffuse via the SMT tile pool (32×32
    DXT1 tiles, deduplicated). The fragment shader's "ground colour"
    comes from sampling this SMT-derived base diffuse FIRST; the splat
    distribution then **modulates** that base by mixing in the 4 DNTS
    details. The exact composite math is the core question.

11. **Lighting** comes from `mapinfo.lighting.sunDir` (vec3 + w) plus a
    ground ambient/diffuse/specular triplet. The shader uses standard
    Lambert + Phong-like specular, with shadow density modulating both.

12. **`skyReflectModTex`** (cube map in `mapinfo.resources`): used for
    PBR-ish reflection on the ground specular highlight. Optional.

### Constraints on the answer

1. **WGSL-compatible output.** We don't need GLSL; we need a formula we
   can drop into a wgpu fragment shader. WGSL has the same math
   primitives as GLSL; pseudocode is fine as long as it's translatable.

2. **No new texture types beyond what BAR maps already ship.** We accept
   `base_diffuse` (from the SMT), `splat_distribution` (RGBA), 4 × DNTS
   (BC3), `detail_tex` (optional), `specular_tex` (optional). No PBR
   metallic/roughness textures; those don't exist in BAR's mapinfo
   schema.

3. **"Visually believable" is the bar, not pixel-perfect.** If the
   research finds Recoil applies sub-pixel filmic tonemapping or a
   custom atmospheric scatter pass on top of the ground colour, we
   document those as out-of-scope for the editor preview. The editor
   wants the dominant blend math correct; perceptual matching is a v2
   problem.

4. **Source primacy.** Every quoted formula must cite an exact filename +
   line range in `RecoilEngine/`. If a shader uniform is read but its
   purpose isn't documented in the comments, infer from the math and
   flag the inference explicitly.

### Research questions

Answer each in your output. Cite primary sources (engine source files
under `RecoilEngine/`, not secondary blog posts). If a fact is
unverifiable in 2-3 searches, mark it explicitly as a hypothesis the
implementation phase will need to test against a real BAR map.

**A. The shader path itself.**

1. Where in `RecoilEngine/rts/Map/SMF/` is the GLSL fragment shader that
   composites the splat distribution × 4 DNTS detail textures? Filename
   + approximate line range. Quote the relevant `main()` body or the
   key composite function.

2. Is there ONE shader path, or are there branches for "no DNTS"
   (legacy `splatDetailTex`) vs DNTS vs DNTS+diffuse-in-alpha? List the
   branches; identify what `mapinfo.resources` configuration triggers
   each.

3. What happens when `splatDetailNormalTex` is set but `specularTex` is
   missing? Is the silent-disable claim still accurate in current
   Recoil source, or has it been fixed? (Verify against the most recent
   tag on `master`.)

**B. Composite math — diffuse.**

1. Walk the diffuse composite step-by-step. For a ground pixel with:
   - `base_diffuse` = sample from SMT-derived texture at (u, v)
   - `dist` = RGBA of `splat_distribution` at (u, v)
   - `dnts_i` = sample of `splatDetailNormalTex<i+1>` at (u × texScales[i], v × texScales[i])
     for i in {0, 1, 2, 3}
   - what is the output `ground_diffuse` colour?

   Write the formula. Examples:
   - `final = base + sum(dist[i] * (dnts_i.alpha * detail_color) * texMults[i])` — if alpha-modulated.
   - `final = mix(base, dnts_0.rgb, dist[0]) ... ` — sequential mix.
   - Or some other form.

2. What's the **channel sum invariant**? If a pixel has
   `dist = (255, 255, 255, 255)`, does the shader saturate (white-out),
   normalize (treat as `1/4` each), or clamp at the engine level?

3. Is `detailTex` always sampled and added (an "everywhere" overlay), or
   only used when DNTS is disabled? If always sampled, what's its
   weight relative to the DNTS terms?

4. What's `texMults[i]` doing in the formula? Brightness multiplier on
   the DNTS sample? Weight multiplier on the channel? Verify against
   `SMFRenderState.cpp`'s uniform setter or the shader's uniform read.

**C. Composite math — normals.**

1. How are the 4 DNTS normals composited into a single per-pixel normal?
   Weighted sum of (x, y, z) vectors? Re-normalised after sum? Or
   weighted slerp / blended in tangent space and re-orthogonalised?

2. Is there a **ground base normal** the DNTS deltas are added to, or
   does the splat distribution exclusively drive normal data on
   DNTS-flagged maps?

3. Does the normal blend use the SAME splat-distribution weights as the
   diffuse blend (`dist.rgba`), or is there a separate normal-blend
   path?

4. Tangent-space vs world-space: which space are the DNTS normals
   interpreted in, and where is the TBN matrix constructed?
   `SMFRenderState.cpp` or the shader itself?

**D. Specular and reflection.**

1. When `specularTex` is provided, how is it used? RGB scale on the
   specular colour? Per-pixel specular exponent (in alpha)? Both?

2. Quote the exact formula combining `lighting.specularExponent`,
   `lighting.groundSpecularColor`, the per-pixel `specularTex` sample,
   and the per-pixel normal (from C).

3. `skyReflectModTex` — when is the cube map sampled, and how does the
   sample modulate the specular highlight? Is there a `clearcoat`-style
   second specular lobe?

4. Confirm or refute: does `splatDetailNormalTex` actually depend on
   `specularTex` being present? Show the conditional that guards the
   DNTS path.

**E. UV coordinate spaces.**

1. `base_diffuse`: world-aligned UVs computed from `(world_x / texelSize,
   world_z / texelSize)` where `texelSize = 8 elmos`? Or per-vertex
   UVs interpolated from the heightmap mesh? Quote the vertex-shader UV
   computation.

2. `splat_distribution`: same UV space as base_diffuse, or scaled (the
   distribution is `512 px / SMU` per Spring conventions, the diffuse
   is `512 px / SMU` per SMT)? Verify.

3. `dnts_i`: UVs are `(world_x * texScales[i], world_z * texScales[i])`?
   Or `(uv_base / texScales[i])`? The semantic of `texScales` (multiply
   vs divide) determines whether real-map `texScales = 0.004` produces
   a finer or coarser tile.

4. `detailTex` and `specularTex` UV spaces — same as base, or distinct?

**F. Lighting integration.**

1. Show the final fragment output as
   `output = ambient_term + lambert_term + specular_term [+ ...]`.
   Identify each input.

2. Where does `lighting.sunDir`'s 4th component (the `w` "sunStart
   distance") feed in? Some Spring documentation says it's a ray-march
   limit for shadow density; verify.

3. Shadow density: how does it modulate `lambert_term` and
   `specular_term` independently? `lighting.groundShadowDensity` is a
   scalar; quote its application.

4. `voidWater` / `voidGround` interactions: do these alter the
   fragment-output equation, or just the alpha test that culls
   below-water / void-ground fragments?

**G. Editor-preview simplifications we should NOT skip.**

1. From the formulas in B–F, identify the **minimum** subset that
   produces a "visually believable" preview. Specifically: can we skip
   `skyReflectModTex` cube-map sampling? Can we use a hard-coded
   ambient + Lambert without specular? Can we approximate the 4-normal
   blend as a single normal sample of `dist`-weighted average?

2. List up to 5 "preview drift" items (things our shader will visibly
   diverge on) so we can document them in the editor's About panel:
   "preview is approximate; final BAR rendering differs in X, Y, Z."

3. Recommend an ordered implementation list: which shader features to
   ship FIRST (the 80% perceptual-quality items) and which to defer
   (the 20% polish items that gate F4 looking "right").

**H. Field-roles map.**

Produce a single table mapping every `mapinfo` field involved to its
role:

| `mapinfo` field | Type | Role in shader | Required? |
|---|---|---|---|
| `resources.detailTex` | string (path) | Base detail overlay (sampled at world UV / 2.0) | Optional |
| `resources.splatDistrTex` | string (path) | Per-pixel channel weights for DNTS | Required for DNTS |
| `resources.splatDetailNormalTex` | string[4] | The 4 DNTS BC3 normals | Required for DNTS |
| `resources.splatDetailNormalDiffuseAlpha` | bool | Use DNTS alpha as diffuse term | Optional |
| `resources.specularTex` | string (path) | Per-pixel specular RGB + exp | **Required when DNTS active (verify)** |
| `resources.skyReflectModTex` | string (path) | Cube map for specular reflection | Optional |
| `splats.texScales` | float[4] | Per-channel UV scale on DNTS samples | Required for DNTS |
| `splats.texMults` | float[4] | Per-channel intensity multiplier | Required for DNTS |
| `lighting.sunDir` | vec4 | (xyz)=light direction, w=sunStart | Required |
| `lighting.groundAmbientColor` | vec3 | Constant ambient term | Required |
| `lighting.groundDiffuseColor` | vec3 | Lambert term colour scale | Required |
| `lighting.groundSpecularColor` | vec3 | Specular base colour | Required |
| `lighting.specularExponent` | float | Phong exponent | Required |
| `lighting.groundShadowDensity` | float | Shadow attenuation factor | Required |

— extend the table with anything else the shader reads.

### Deliverable

A draft ADR-shaped document in this exact shape (we'll commit it as
`docs/DECISIONS.md` § ADR-035 — or the next available number — once
reviewed):

```markdown
## ADR-035 — Terrain fragment shader composite

**Status:** Proposed (research) — 2026-MM-DD
**Context:** [why this is needed now; reference F4; cite the BAR maps
              that demonstrate the visual look we're targeting]

**Decision:** Adopt the composite formula from Recoil's
              `<filename>.glsl` (see §"Composite math" below), with
              [list of editor-preview simplifications].

**Alternatives considered:**
- Naïve "average DNTS by weight": rejected — produces muddy blend.
- PBR-like metallic/roughness: rejected — outside BAR's schema.
- Sample only `base_diffuse`, skip DNTS in preview: rejected — user
  authoring intent invisible.

**Consequences:**
- Fragment shader gets a 6-texture bind group (base + dist + 4 × DNTS
  + optional specular + optional detail). Within wgpu's
  `max_sampled_textures_per_shader_stage` default of 16.
- Editor preview will visibly drift from in-engine final on [list of
  items] — documented in the About panel.
- `splatDetailNormalTex + specularTex` requirement is enforced by the
  linter (C8 / sprint 13).

### Composite math (the formula we'll write into terrain.wgsl)

[pseudocode block — translatable to WGSL]

### Field-roles map

[the table from research question H, finalised]

### Editor-preview deferrals

[the list from research question G.2]
```

### Constraints on your research process

- **Cite primary sources.** Engine shader questions → `RecoilEngine/cont/base/springcontent/shaders/GLSL/*.glsl` or the C++ that injects shader text. Quote filename + line range.
- **Don't reimplement BAR's PBR.** We're targeting the dominant blend math, not photorealism. If a feature is purely cosmetic (atmospheric scatter, depth-of-field, post-processing), mark it as out-of-scope.
- **Two-pass output.** First pass: research question answers with citations. Second pass: the draft ADR. If they conflict, prefer the answers (the ADR is a synthesis; the answers are the evidence).
- **≤ 2500 words excluding the WGSL pseudocode + tables.** Crisp recommendations, not exhaustive engine archeology.

---

## What we'll do with the output

1. Save the session output verbatim as
   `docs/research/splat-rendering/{claude,gemini}-research-findings.md`.
2. Review side-by-side in `docs/research/splat-rendering/NOTES.md`
   (section-by-section accept / modify / reject).
3. Reconcile accepted recommendations into ADR-035 (or next-available
   ADR number when the work lands — likely between Sprint 8 (DNTS bake)
   and Sprint 9 (fragment shader + UI)).
4. Use the WGSL pseudocode to write the fragment shader for Sprint 9
   item D4.
