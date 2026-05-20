# Sprint 26 — Water polish: fresnel + foam + caustics + perlin + refraction + reflection (R3)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 26** — the second renderer-parity sprint (R3). Sprint
14 / C9 / ADR-042 shipped a **flat alpha-blended water plane MVP** at
`Y = 0`. The polish (foam, fresnel, caustics, lava emission, perlin
wave motion) was deferred to the renderer-parity arc per the C9 prompt's
"Out of scope" section. This sprint closes that deferral.

After this sprint, water and lava render with full visual parity to
BAR's `BumpWater.glsl` for the editor's camera distances. Specifically:

- **Surface normal map** sampling from `mapinfo.water.normalTexture`
  with animated UV per `mapinfo.water.windSpeed`.
- **Fresnel** from `fresnelMin`, `fresnelMax`, `fresnelPower`.
- **Perlin wave motion** from `perlinStartFreq` / `Lacunarity` /
  `Amplitude` (FINDINGS §1.5).
- **Foam** at shorelines, sampled when terrain Y is within
  `foamHeight` of water plane.
- **Caustics** — animated cycle through
  `mapinfo.water.causticsResolution / Strength`.
- **Refractions** — sample the offscreen color buffer through
  perturbed UV (refractionDistortion).
- **Planar reflections** — render the terrain a second time
  mirrored through `y = 0` into a reflection texture, sample in
  the water shader.
- **Lava emission glow** — when `Project.water_mode == Lava |
  Magma`, the plane self-illuminates per `mapinfo.water.surfaceColor`.

**Prerequisites:**
- Sprint 25 (terrain shader parity) MUST be ticked. The water
  shader composes with the terrain shader's output via depth +
  refraction.
- Sprint 13 (ADR-037) offscreen RT + depth is the foundation.
- Sprint 14 (ADR-042) is the MVP this sprint replaces.

**Reference clone of RecoilEngine:** as Sprint 25 — `/home/teague/code/RecoilEngine`.
Critical files:
- `cont/base/springcontent/shaders/GLSL/BumpWater.glsl` — water
  shader.
- `cont/base/springcontent/shaders/GLSL/WaterRefraction.glsl` —
  refraction-pass helper.
- `rts/Rendering/Env/BumpWater.cpp` — engine-side state.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — water schema model
   (`MapInfo::WaterBlock`), §2.1 #11 (parity commitment).
3. `/home/teague/code/BARMapEditor/docs/research/renderer-bar-parity/ROADMAP.md`
   — Sprint 18 section (in original numbering) = this Sprint 26.
4. `/home/teague/code/BARMapEditor/docs/research/source-audit-2026-05-18/FINDINGS.md`
   §1.5 — all 21+ `mapinfo.water` parameters.
5. **`/home/teague/code/BARMapEditor/docs/research-water-lava/`**
   — devlog research outputs from prior water work.
6. `/home/teague/code/BARMapEditor/docs/DECISIONS.md` — ADR-042
   (water MVP) is amended; the polish ships as ADR-039 (next
   number after Sprint 25's ADR-038).
7. `/home/teague/code/RecoilEngine/cont/base/springcontent/shaders/GLSL/BumpWater.glsl`
   + `WaterRefraction.glsl` — source GLSL.
8. `/home/teague/code/RecoilEngine/rts/Rendering/Env/BumpWater.cpp`
   — engine state.
9. `/home/teague/code/BARMapEditor/crates/barme-app/src/water.wgsl`
   — current MVP shader. Major rewrite.
10. `/home/teague/code/BARMapEditor/crates/barme-app/src/render.rs`
    — current MVP water-pipeline setup. Extend with reflection
    pass + refraction sampling.
11. `/home/teague/code/BARMapEditor/crates/barme-core/src/water_presets.rs`
    — preset patches. Extend with per-preset polish parameters
    (foam, caustics, fresnel defaults).
12. `/home/teague/code/BARMapEditor/crates/barme-app/src/main.rs::inspector_water`
    — the F-Water inspector. Polish parameters surface here.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-26-water-polish
```

## Step 3 — Scope

In order, one commit per chunk:

### 1. Planar reflection pass

A second terrain render pass, mirrored through `y = 0`, into a
reflection texture:

```rust
struct ReflectionPass {
    texture: wgpu::Texture,  // RGBA8, half-res (1024² or 2048²)
    view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,  // reuses terrain shader with flipped projection
}
```

**Implementation**:
- Allocate a half-resolution offscreen RT (1024² for editor
  preview; 2048² for `.sd7` export not applicable — reflection
  is editor-preview-only). Sized down for perf.
- Mirror the camera through y=0: `eye.y *= -1`, `target.y *= -1`,
  `up.y *= -1`. Reuse the existing camera + view matrix.
- Render the terrain pipeline (Sprint 25) into the reflection
  RT. Skip features / grass for perf.
- Cull terrain below y=0 in the reflection pass (no need to
  render submerged terrain — invisible after the flip).

**Frame-budget concern**: this doubles terrain render cost. On
Vega 8 iGPU, the terrain pass is ~3 ms; doubling fits in the
16 ms budget. Bench before merging.

### 2. Refraction sampling

Sample the editor's main color RT (offscreen target from
ADR-037) inside the water shader, with UV perturbed by the
surface normal map:

```wgsl
let refr_offset = surface_normal.xz * uniforms.refraction_distortion;
let refr_uv = clip_uv + refr_offset;
let refr_color = textureSample(main_color_rt, sampler_lin, refr_uv);
```

The main color RT must be available as an SRV at water-pass time.
This means a render-pass split: terrain + features pass writes to
main RT; water pass samples from it. The depth attachment carries
across (depth-test-only for water; depth-write off, per Sprint
14's contract).

**Caveat**: sampling-from-and-writing-to the same texture is
undefined in Vulkan/D3D12. Allocate a SECOND main RT and ping-pong
if needed — or, since water is alpha-blended on top, use the
pre-water RT as a read-only source and let water-pass write to a
separate output RT, composited later. Document the choice in the
ADR.

### 3. Surface normal map + perlin wave motion

Sample `mapinfo.water.normalTexture` (a tiling normal map) with
animated UV:

```wgsl
let time_s = uniforms.time;
let uv_a = world_xz * uniforms.normal_scale + vec2<f32>(time_s * uniforms.wind_speed_x, 0.0);
let uv_b = world_xz * uniforms.normal_scale * 0.5 + vec2<f32>(0.0, time_s * uniforms.wind_speed_z);
let n_a = textureSample(water_normal, sampler_lin, uv_a).xyz * 2.0 - 1.0;
let n_b = textureSample(water_normal, sampler_lin, uv_b).xyz * 2.0 - 1.0;
let surface_normal = normalize(n_a + n_b);
```

Layer perlin wave displacement on top of the normal map. Perlin
implementation: WGSL fbm function (3-octave Perlin via gradient
noise; ~50 LoC). Parameters from
`mapinfo.water.perlinStartFreq / Lacunarity / Amplitude`.

The `normalTexture` is bundled with BAR per FINDINGS; ship a
vendored copy under `tools/water-assets/normal.png`.

### 4. Fresnel + foam + caustics

**Fresnel** (per fragment):
```wgsl
let view_dir = normalize(uniforms.eye.xyz - world_pos.xyz);
let fresnel_raw = pow(1.0 - max(0.0, dot(view_dir, surface_normal)), uniforms.fresnel_power);
let fresnel = mix(uniforms.fresnel_min, uniforms.fresnel_max, fresnel_raw);
```

**Foam**: sample terrain depth (from main RT's depth view) and
test against water plane:
```wgsl
let terrain_y_at_uv = sample_terrain_y(clip_uv);  // via depth-to-world
let foam_factor = smoothstep(0.0, uniforms.foam_height, uniforms.water_plane_y - terrain_y_at_uv);
```

**Caustics**: animated sampling of a caustics texture (vendored
from BAR or procedural):
```wgsl
let caustic_uv = world_xz * uniforms.caustics_resolution + vec2<f32>(time_s * 0.05, time_s * 0.03);
let caustic = textureSample(caustics_tex, sampler_lin, caustic_uv).r * uniforms.caustics_strength;
```

### 5. Lava emission glow

When `Project.water_mode == Lava | Magma`, the water plane self-
illuminates:

```wgsl
let emission = uniforms.lava_emission_color.rgb
    * (1.0 + caustic * 0.5)  // caustic-modulated for surface motion
    * uniforms.lava_emission_strength;
final_color += emission;
```

The Lava / Magma preset's `surfaceColor` becomes the emission
colour. The strength fades to 0 in daylight (sun-angle conditional)
and ramps to max at night — useful for the eventual atmosphere
sprint (Sprint 28).

### 6. Inspector polish + preset extensions

`inspector_water` (`main.rs`):
- Add a "Polish" section (collapsible by default) below the
  existing "Appearance" section.
- Fields: foam_height, caustics_resolution, caustics_strength,
  fresnel_min / max / power, wind_speed_x / wind_speed_z,
  normal_scale, perlin_start_freq / lacunarity / amplitude.
- Per-preset defaults updated in `water_presets.rs` (tuned to
  match real BAR maps — Coastlines, Acidic Quarry, Gecko Isle,
  Lava sample maps).
- Tooltips per Sprint 19 convention.

### 7. ADR-039 (new)

`/home/teague/code/BARMapEditor/docs/DECISIONS.md`:

```
## ADR-039 — Water polish (Sprint 26 / R3)

Status: ADOPTED 2026-05-XX
Supersedes: ADR-042 (Sprint 14, MVP water plane) in part.
...
```

Cover: ping-pong vs single RT, reflection-pass cost trade-off,
WGSL perlin implementation, the lava-emission day/night ramp,
deferred items (sky reflections — Sprint 35; underwater fog —
parts of Sprint 28).

### 8. Validation fixtures

Add `assets/parity-fixtures/coastlines/` (sea map),
`assets/parity-fixtures/gecko/` (tidal map), `assets/parity-fixtures/lava-sample/`.
Sprint 36 ships the ΔE automation; this sprint extracts the
fixtures + reference screenshots.

**Platform-portability checklist** (mandatory):
- WGSL perlin must compile on all wgpu backends. Test on Vulkan +
  GL (Mesa software fallback).
- The reflection pass's flipped-Y projection works identically on
  Metal / D3D12 / Vulkan — wgpu normalises clip-space y direction.
- Document untested macOS / Windows in devlog.

### 9. Rollup

STATUS UPDATEs in SRS / ROADMAP (R3 done, renderer-parity arc
2/8). closing devlog log. "Sprint 27 = Inspector consistency
refactor + brush-card lift" handoff.

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on reflection-pass init;
`warn!` on missing water normal texture; `trace!` on per-frame
water uniform upload.

## Step 5 — Out of scope

- **Sky cubemap reflections** — Sprint 35. The reflection pass
  in this sprint reflects terrain only.
- **Underwater fog / absorption / colour-shift below water** —
  partly Sprint 28 (atmosphere) and partly here (the
  `mapinfo.water.absorb` field). Ship a basic underwater tint
  here; the rest is Sprint 28.
- **Wind direction-from-mapinfo.atmosphere** — wind affects
  water motion. For Sprint 26, wind direction is editor-camera-
  aligned for simplicity; Sprint 28 ties it to atmosphere data.
- **Shoreline foam SPLAT painting** — interactive foam-mask
  brush is Stage-2 polish.
- **Lava bubble animation** — Stage-2 polish.

## Step 6 — Critical pitfalls (read twice)

1. **Reflection pass doubles terrain cost**. Half-res RT mitigates;
   bench on Vega 8 iGPU before merging. If frame budget breaks,
   ship reflections behind a `View > Reflections` toggle (default
   OFF on low-end hardware).

2. **Refraction read-from-write-to-same-texture is UB**. Use
   ping-pong RTs or a separate water-output RT. Don't write to
   the main RT inside the water pass.

3. **Perlin in WGSL**: standard 3-octave gradient noise is ~50
   LoC. Use a known-good reference (e.g., Inigo Quilez's).
   Floating-point precision differs per backend; test on Vulkan
   vs Metal for visual identity.

4. **Animated UV from `time_s`**: time uniform is a global
   `f32`. After 24 hours of runtime, `time_s ≈ 86400`, and
   `sin(86400 * freq)` precision degrades. Use `time_s %
   (2π / min_freq)` to keep it bounded; the visual cycle is
   identical.

5. **Foam-height threshold**: use `smoothstep` not `step` for
   anti-aliasing the shoreline. The threshold is in elmos
   (default ~1 elmo). Test on a beach sample.

6. **Fresnel at grazing angles**: `pow(1 - dot, 5)` can NaN
   at exactly 90° (dot = 0; pow(1, 5) = 1, fine; but if
   dot < 0 due to backface, pow(2, 5) = 32, oversaturates).
   Clamp `dot` to `[0, 1]` before pow.

7. **Lava emission with shadow**: Sprint 30 ships shadows.
   When that lands, lava-emission should NOT receive shadow
   (self-illuminated). Add a `lit = false` branch path now;
   Sprint 30 wires the shadow sample.

8. **WGSL `textureSample` in conditional**: see Sprint 25
   pitfall #10. Same applies here — hoist samples.

9. **`mapinfo.water.normalTexture` path resolution**: when
   the user sets a custom path, the file must exist in the
   project's `<project>/textures/` directory OR be an absolute
   path. The pipeline `splat_pipeline.rs`-style path resolution
   applies. Sprint 14 didn't load the texture; this sprint does.

10. **Default water assets**: vendor BAR's default `normal.png`
    + `caustics.png` under `tools/water-assets/`. CC0 license per
    BAR. SHA-pin like the texture-pack scripts.

11. **Platform portability**: WGSL is automatically cross-compiled.
    No backend-specific feature flags. The perlin noise + smoothstep
    + textureSample are all standard.

12. **Lava preset day/night ramp** — without an atmosphere sun
    intensity (Sprint 28), use a hardcoded `0.5` daylight factor
    for Sprint 26. Sprint 28 replaces with `dot(sun_dir, world_up)`.

## Step 7 — Exit criteria

- 6+ commits on `main`: reflection pass, refraction, normal +
  perlin, fresnel + foam + caustics, lava emission, inspector
  polish, ADR-039, rollup.
- 1 devlog folder filled.
- SRS / ROADMAP STATUS UPDATEs (R3 done, renderer-parity 2/8).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test:
  - Coastlines fixture renders with foam at shoreline +
    fresnel at grazing angles + caustics on sea floor.
  - Lava-sample fixture renders glowing.
  - 4-SMU project's water frame-time within 16 ms budget on
    Vega 8 iGPU (bench it).
  - Reflection toggle off → frame-time recovers ~3 ms.
  - Inspector polish section exposes all new fields with
    tooltips.
- Final devlog: summary + "Sprint 27 = Inspector consistency
  refactor + brush-card lift" handoff.

Start by reading `BumpWater.glsl` end-to-end with FINDINGS §1.5
open. The WGSL port is mostly mechanical; the reflection-pass
plumbing is the real engineering work.
