# Sprint 30 — Directional shadows: single-cascade sun-view depth pass (R4)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 30** — the sixth renderer-parity sprint (R4 in
ROADMAP numbering). The editor's terrain is now Lit + Atmospheric
(Sprints 25, 28) and water/features render correctly (Sprints 26,
29), but **shadows are missing**. A tall hill casts no shadow on
the leeward side; tall features don't shade the ground beneath
them. Sprint 30 adds directional shadows from the sun direction.

After this sprint:

- A **shadow-pass pipeline** renders the scene from the sun's
  perspective into a depth texture.
- The terrain shader (Sprint 25) samples the shadow map per-fragment
  and attenuates lighting by `mapinfo.lighting.groundShadowDensity`.
- The feature shader (Sprint 29 Phase B) likewise — features cast
  AND receive shadows.
- Water samples shadows for caustic visibility (Sprint 26 already
  has caustic geometry; shadow attenuates them).

**Single-cascade is the MVP**; multi-cascade shadow maps
(distance-banded resolutions) are deferred per ROADMAP. The
single cascade covers the full map at a fixed resolution.

**Prerequisites:**
- Sprint 25 (terrain shader parity) MUST be ticked.
- Sprint 28 (atmosphere + fog) MUST be ticked. Shadow density
  comes from the lighting block.
- Sprint 29 (feature decoding) recommended but not required;
  features render with or without shadow.

**Reference clone of RecoilEngine:** `/home/teague/code/RecoilEngine`.
Critical files:
- `rts/Rendering/ShadowHandler.cpp` — shadow-pass orchestration.
- `rts/Rendering/GL/ShadowFrustumLocker.cpp` — sun camera setup.
- `cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl` — the
  shadow sample call in the existing terrain shader.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §3.3 NFR-Performance
   (shadow pass within frame budget).
3. `/home/teague/code/BARMapEditor/docs/research/renderer-bar-parity/ROADMAP.md`
   — Sprint 19 section (original numbering) = this Sprint 30.
4. `/home/teague/code/RecoilEngine/rts/Rendering/ShadowHandler.cpp`.
5. `/home/teague/code/RecoilEngine/cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl`
   — search "shadow" / "shadowMap".
6. `/home/teague/code/BARMapEditor/crates/barme-app/src/render.rs`
   — extend with shadow pass.
7. `/home/teague/code/BARMapEditor/crates/barme-app/src/terrain.wgsl`
   — Sprint 25's shader gains a `sample_shadow()` call.
8. `/home/teague/code/BARMapEditor/crates/barme-core/src/mapinfo_schema.rs`
   — `lighting.ground_shadow_density` + `unit_shadow_density`
   already in the schema.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-30-directional-shadows
```

## Step 3 — Scope

In order, one commit per chunk:

### 1. Shadow camera + frustum

`crates/barme-app/src/render.rs` — `ShadowCamera` struct:

```rust
pub struct ShadowCamera {
    view_matrix: Mat4,
    projection_matrix: Mat4,
    light_pos: Vec3,  // virtual; for orthographic projection
    light_dir: Vec3,  // from mapinfo.lighting.sun_dir
}

impl ShadowCamera {
    pub fn for_map(map_bounds: AABB, sun_dir: Vec3) -> Self {
        // Orthographic projection from sun direction.
        // Frustum tight-fit to map AABB.
        ...
    }
}
```

The projection is orthographic (sun is "infinitely far"), with
the frustum sized to cover the full map AABB from above + sides.

### 2. Shadow render pass

A new pipeline that runs BEFORE the main terrain pass:

```rust
pub struct ShadowPass {
    pipeline: wgpu::RenderPipeline,  // depth-only
    map: wgpu::Texture,  // Depth32Float
    view: wgpu::TextureView,
}
```

**Resolution**: 2048² Depth32Float for Sprint 30 (uses ~16 MB).
A future multi-cascade upgrade could go to 4096² + cascades for
distance bands.

The pipeline:
- Vertex stage: transform terrain mesh + features into shadow-
  camera clip space. Same vertex shader as the main pass,
  but with the shadow camera's view-projection matrix.
- Fragment stage: empty (depth-only write).
- Color attachments: none.
- Depth attachment: write enabled.

### 3. Shadow sampling in `terrain.wgsl`

```wgsl
fn sample_shadow(world_pos: vec3<f32>) -> f32 {
    let shadow_clip = uniforms.shadow_view_proj * vec4<f32>(world_pos, 1.0);
    let shadow_ndc = shadow_clip.xyz / shadow_clip.w;
    if (shadow_ndc.x < -1.0 || shadow_ndc.x > 1.0
     || shadow_ndc.y < -1.0 || shadow_ndc.y > 1.0
     || shadow_ndc.z < 0.0 || shadow_ndc.z > 1.0) {
        return 1.0;  // outside shadow frustum → lit
    }
    let shadow_uv = shadow_ndc.xy * 0.5 + vec2<f32>(0.5);
    let shadow_depth = textureSample(shadow_map, sampler_shadow, shadow_uv);
    let bias = 0.005;  // depth bias to avoid shadow acne
    if (shadow_ndc.z - bias > shadow_depth) {
        return 1.0 - uniforms.ground_shadow_density;  // in shadow
    }
    return 1.0;  // lit
}
```

Multiplied into the diffuse lighting result:
```wgsl
let shadow_factor = sample_shadow(world_pos);
let lit = (ambient + sun_color * lambert * shadow_factor + specular);
```

### 4. PCF (Percentage-Closer Filtering) for soft edges

Sharp shadow edges look harsh. PCF samples a 3×3 neighbourhood:

```wgsl
fn sample_shadow_pcf(world_pos: vec3<f32>) -> f32 {
    var total: f32 = 0.0;
    let texel = 1.0 / SHADOW_MAP_RESOLUTION;
    for (var dx = -1; dx <= 1; dx++) {
        for (var dy = -1; dy <= 1; dy++) {
            total += sample_shadow_at(world_pos, vec2<f32>(f32(dx), f32(dy)) * texel);
        }
    }
    return total / 9.0;
}
```

3×3 PCF is ~9× the texture samples. Test perf budget on Vega 8
iGPU; if too slow, ship 1× then iterate.

### 5. Feature shadow casting + receiving

For Sprint 29 Phase A (decals), features don't cast shadows
(they're 2D sprites). For Phase B (S3O), features cast via the
shadow pass (rendered as solid geometry, depth-only).

Both phases: features RECEIVE shadows by sampling the shadow
map in their fragment shader.

### 6. ADR-041 (new)

```
## ADR-041 — Directional shadows (Sprint 30 / R4)

Status: ADOPTED 2026-05-XX
...
```

Cover: shadow resolution choice, PCF cost, single-cascade
trade-offs vs multi-cascade, depth bias selection, frustum-fit
algorithm.

### 7. Validation fixtures

Add `assets/parity-fixtures/shadow-test/` — a 4-SMU project with a
tall central hill (carved via procgen) at sun direction
`(0.7, 0.5, 0.5)`. The lee side of the hill should be in shadow
when rendered. Side-by-side BAR vs editor reference.

**Platform-portability checklist**:
- `texture_depth_2d` is standard WGSL.
- `sampler_comparison` (used for shadow sampling) is standard.
- Depth bias varies slightly per backend; test on Vulkan + GL.
- Document untested macOS / Windows.

### 8. Rollup

STATUS UPDATEs in SRS / ROADMAP (R4 done, renderer-parity 5/8 or
6/8 depending on Sprint 29 Phase). closing devlog log. "Sprint
31 = Toast queue + confirmation modals" handoff.

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on shadow-pass init +
resolution choice; `warn!` on shadow-map allocation failure;
`trace!` on per-frame shadow-view recalc.

## Step 5 — Out of scope

- **Multi-cascade shadows** (CSM) — single cascade only.
- **Variance shadow maps** (VSM) — standard depth-comparison
  only.
- **Soft shadows via PCSS** — basic 3×3 PCF only.
- **Self-shadowing on the heightmap** — terrain casts on itself;
  this works naturally with the depth-pass approach. No extra
  work needed.
- **Shadow LOD** — fixed resolution.
- **Animated time-of-day shadows** — sun direction is static
  per project per the SRS.

## Step 6 — Critical pitfalls (read twice)

1. **Shadow acne** is the #1 issue. Without depth bias, surfaces
   self-shadow randomly. Start with `bias = 0.005` and adjust per
   testing. Sloped surfaces need higher bias; flat surfaces lower.

2. **Peter-Panning** is the dual: too much bias causes shadows
   to detach from caster. Tune the bias to balance.

3. **PCF on Vega 8 iGPU**: 3×3 PCF is 9 samples per pixel. On a
   2048² shadow map for a 16-SMU terrain (~33M fragments after
   tessellation), that's 300M shadow-map fetches per frame.
   Acceptable on modern hardware; iffy on Vega 8. Profile;
   fall back to 1× sample if budget breaks.

4. **Shadow camera frustum**: orthographic, sized to the map's
   AABB. The map's height range matters — a 4-SMU map with a
   2048-elmo-tall mountain needs more frustum depth than a
   flat 16-SMU map.

5. **`groundShadowDensity` vs `unitShadowDensity`** — terrain
   gets ground; features get unit. Pass both as uniforms; the
   shader chooses based on the bound texture (or render-pass
   tag).

6. **Depth precision**: Depth32Float gives ~7 decimal digits of
   precision. For a 16-SMU map with 4096-elmo height range, the
   effective depth precision is ~0.5 mm. Plenty.

7. **Shadow-pass vertex transform**: reuse the existing terrain
   vertex shader (Sprint 25) but with the shadow camera's
   view-projection matrix. Don't write a separate vertex shader
   unless tessellation differs.

8. **Sample comparison sampler**: WGSL's `texture_depth_2d`
   pairs with `sampler_comparison` (not regular `sampler`).
   The comparison function is `<=` for the standard "is fragment
   in front of shadow depth?" test.

9. **Light direction normalisation**: `mapinfo.lighting.sun_dir`
   is `[x, y, z, w]` where `w = 1.0` (per Sprint 10 fix).
   Normalise the XYZ before using as light direction.

10. **WGSL `texture_depth_2d` not available on all backends**:
    `texture_depth_2d` is standard. Older WebGL2 may not
    support; we don't target WebGL2.

11. **Stage-2 polish**: cascaded shadow maps (3 cascades typical)
    + variance shadow maps (soft shadows without PCF cost) +
    transparent-feature shadows (S3O models with alpha tested
    against the shadow map). All deferred.

12. **Platform portability**: see checklist. Depth-only render
    pass is well-supported on all wgpu backends.

## Step 7 — Exit criteria

- 4+ commits on `main`: shadow camera + pass, sampling in
  terrain.wgsl, PCF, ADR-041, rollup.
- 1 devlog folder filled.
- SRS / ROADMAP STATUS UPDATEs (R4 done, renderer-parity 5/8 or
  6/8).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test:
  - Shadow-test fixture: tall hill casts shadow on lee side.
  - `groundShadowDensity = 0` → no shadow visible.
  - `groundShadowDensity = 1` → full black shadow.
  - Frame budget within 16 ms on Vega 8 iGPU at 16-SMU.
- Final devlog: summary + "Sprint 31 = Toast queue +
  confirmation modals" handoff.

Start by writing the `ShadowCamera::for_map` orthographic
projection, then the depth-only pass pipeline, then the shader
sample. Tuning the bias is the last 10% — start with the literal
spec values and adjust visually.
