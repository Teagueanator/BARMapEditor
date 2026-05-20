# Sprint 34 — Grass rendering: instanced quads + wind animation + density-from-terraintype (R6)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 34** — the seventh renderer-parity sprint (R6 in
ROADMAP numbering). The editor renders terrain, water, atmosphere,
features, and shadows. **Grass** is the last common BAR aesthetic
not yet matched.

After this sprint:

- **Grass blades** render as instanced quads at terrain elevation,
  with per-blade jitter for natural variation.
- **Wind animation** sways blades using `mapinfo.atmosphere`
  min/max wind (Sprint 28 propagation).
- **Density** comes from `mapinfo.grass.maxStrawsPerTurf` × a
  CPU-generated density texture (currently derived from
  `terrain_types[0]` mask — BAR convention: grass grows on
  terrain-type 0).
- **Blade colour** from `mapinfo.grass.bladeColor`.
- **LOD**: only render blades within ~200 elmos of the camera.

**Prerequisites:**
- Sprint 33 (NFR/CI gates) MUST be ticked. Grass shaders go
  through the same matrix.
- Sprint 28 (atmosphere + fog) MUST be ticked. Wind state lives
  in atmosphere uniforms.
- Sprint 30 (shadows) MUST be ticked. Grass receives shadows.

**Reference clone of RecoilEngine:** `/home/teague/code/RecoilEngine`.
Critical files:
- `cont/base/springcontent/shaders/GLSL/Grass*.glsl`
- `rts/Rendering/Env/Decals/GrassDrawer.cpp`

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — `MapInfo::GrassBlock`
   schema fields.
3. `/home/teague/code/BARMapEditor/docs/research/renderer-bar-parity/ROADMAP.md`
   — Sprint 21 section (original numbering) = this Sprint 34.
4. `/home/teague/code/RecoilEngine/cont/base/springcontent/shaders/GLSL/Grass*.glsl`.
5. `/home/teague/code/RecoilEngine/rts/Rendering/Env/Decals/GrassDrawer.cpp`.
6. `/home/teague/code/BARMapEditor/crates/barme-core/src/mapinfo_schema.rs`
   — `GrassBlock` fields: `bladeWidth`, `bladeHeight`,
   `bladeColor`, `bladeAngle`, `maxStrawsPerTurf`, `bladeWaveScale`.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-34-grass-rendering
```

## Step 3 — Scope

In order, one commit per chunk:

### 1. Grass density texture (CPU bake)

`crates/barme-core/src/grass.rs` (new):

```rust
pub struct GrassDensity {
    pub texture: Vec<u8>,  // 1-byte per pixel, 0..=255 = density
    pub dim: (u32, u32),   // square, sized to (smu * 8 + 1) typically
}

pub fn bake_grass_density(
    heightmap: &Heightmap,
    type_map: &TypeMap,  // currently unused; Sprint 36 / F15 wires this
    grass: &GrassBlock,
) -> GrassDensity;
```

Algorithm:
1. For each pixel: density = `(type_map[pixel] == 0)
   * sigmoid_falloff(slope)`. Steep slopes get less grass.
2. Multiply by `maxStrawsPerTurf / 255` to normalise.
3. Persist to `<project>/.barme-cache/grass-density.png` for
   reuse across runs.

For Sprint 34 (Sprint 36 / F15 type-map editor not yet shipped),
`type_map[pixel] == 0` is always true. The slope falloff is the
primary modulator. Once F15 ships, the type-map drives the mask.

### 2. Per-blade instance generation

`crates/barme-app/src/grass.rs` (new):

```rust
pub struct GrassInstance {
    pub position: [f32; 3],   // world XYZ
    pub orientation: f32,     // rotation around Y
    pub height_scale: f32,    // jitter 0.8..=1.2
    pub color_jitter: [f32; 3],
}

pub fn generate_grass_instances(
    density: &GrassDensity,
    heightmap: &Heightmap,
    camera_pos: Vec3,
    max_distance: f32,  // 200 elmos default
) -> Vec<GrassInstance>;
```

Walk the density texture; for each pixel within `max_distance` of
the camera, sample N blades (where N = density × maxStrawsPerTurf).
Each blade gets jitter (position offset, orientation, height scale,
color jitter).

Regenerate the instance buffer **per camera move** (debounced) +
on terrain edit. Bounded list (~100k blades max on Vega 8).

### 3. Grass shader (`grass.wgsl`)

```wgsl
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) blade_uv: vec2<f32>,   // (0..1, 0..1) per blade quad
    @location(1) world_pos: vec3<f32>,
    @location(2) color: vec3<f32>,
}

@vertex
fn vs_main(
    @location(0) quad_uv: vec2<f32>,        // (0..1, 0..1) of unit quad
    @location(1) inst_pos: vec3<f32>,
    @location(2) inst_orient: f32,
    @location(3) inst_height: f32,
    @location(4) inst_color: vec3<f32>,
) -> VertexOutput {
    // Build a quad facing the camera with bottom anchored at inst_pos.
    let blade_width = grass.blade_width;
    let blade_height = grass.blade_height * inst_height;

    let local_x = (quad_uv.x - 0.5) * blade_width;
    let local_y = quad_uv.y * blade_height;

    // Apply wind sway.
    let wind_sway = sin(uniforms.time * grass.blade_wave_scale + inst_pos.x * 0.1) * grass.wind_amplitude;
    let displaced_x = local_x + wind_sway * quad_uv.y;

    // Orient toward camera (billboarding).
    let view_dir = normalize(uniforms.camera_pos - inst_pos);
    let right = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), view_dir));

    let world_pos = inst_pos + right * displaced_x + vec3<f32>(0.0, local_y, 0.0);

    var out: VertexOutput;
    out.position = uniforms.view_proj * vec4<f32>(world_pos, 1.0);
    out.blade_uv = quad_uv;
    out.world_pos = world_pos;
    out.color = grass.blade_color * inst_color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let alpha = smoothstep(0.0, 0.1, in.blade_uv.x) * smoothstep(1.0, 0.9, in.blade_uv.x);
    // ^ taper edges for a leaf shape.
    let lit = lighting(in.world_pos, in.color);
    let shadow = sample_shadow(in.world_pos);
    return vec4<f32>(lit * shadow, alpha);
}
```

### 4. Grass pipeline + draw call

`crates/barme-app/src/render.rs`: new pipeline.
- Vertex buffer: shared 4-vertex unit quad.
- Instance buffer: GrassInstance per blade.
- Draw indexed instanced.

Render pass order: terrain → grass → features → water → markers.
Grass needs depth-test on (occluded by terrain bumps), depth-write
off (translucent at edges).

### 5. Frame budget

Bench: 100k blades on Vega 8 iGPU. Target: < 4 ms additional.
If too slow:
- Reduce `max_distance` to 150 elmos.
- Reduce `maxStrawsPerTurf` cap.
- Skip blades behind camera (frustum cull on CPU).

### 6. Inspector polish

`inspector_select` (or a new `inspector_grass` if it's worth its
own tool) doesn't exist today. The grass parameters live in the
F9 form's "Grass" tab (Sprint 18 ship it). No new tool needed.

### 7. ADR-043

```
## ADR-043 — Grass rendering (Sprint 34 / R6)

Status: ADOPTED 2026-05-XX
...
```

Cover: density-texture bake, per-blade instance generation,
LOD strategy, wind/blade-sway math.

### 8. Validation + rollup

Add `assets/parity-fixtures/grass-field/` — a 4-SMU flat map
with `maxStrawsPerTurf = 64`. Compare editor vs BAR side-by-side.

**Platform-portability checklist** — same as other R-sprints.

STATUS UPDATEs in SRS / ROADMAP (R6 done, renderer 7/8).
"Sprint 35 = emission + sky-reflect + parallax" handoff.

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on grass density bake +
size; `trace!` on per-frame instance count + cull stats.

## Step 5 — Out of scope

- **Grass on per-feature types** (e.g., grass under trees that
  varies by tree type) — too granular.
- **Grass dust kicked up by units** — gameplay polish; out of
  scope.
- **Grass biomes / multiple density layers** — uniform stock-BAR
  grass only.
- **GPU-driven culling / vertex generation** — CPU instance
  buffer for simplicity.

## Step 6 — Critical pitfalls (read twice)

1. **Frame budget is tight**. 100k blades × 6 vertices each =
   600k vertices. Vega 8 iGPU can do this in <4 ms; older
   hardware may not. Add a "Grass density" slider (0..=1) in
   View menu to throttle.

2. **Billboarding artefacts**: blades facing the camera look
   identical from all angles. Add per-blade orientation jitter
   so the field looks varied.

3. **Wind synchronisation with water**: Sprint 26's water
   already animates by wind. Use the SAME time / wind direction
   uniforms for grass — otherwise the visuals desync.

4. **Per-blade jitter determinism**: use a hashed-position PRNG
   (e.g., `hash(world_xz)`) so the same blade always has the
   same jitter. Otherwise the field shimmers as the camera
   moves.

5. **Alpha-edge taper**: simple `smoothstep` works but ATI
   hardware can show banding. Test on Vega 8.

6. **Shadow receiving**: grass receives shadows from Sprint 30's
   shadow pass. But does grass CAST shadows? Probably not
   (too small + perf cost). Document the choice.

7. **Density texture resolution**: matches the heightmap
   (~SMU × 64 + 1 per side). Persisting as PNG keeps file size
   manageable.

8. **Editor performance fallback**: if grass kills frame rate
   on a user's iGPU, expose a `View > Grass` toggle (default
   ON for high-end, OFF for low-end). Detect via
   `wgpu::AdapterInfo::device_type == DeviceType::IntegratedGpu`.

9. **Camera-far-plane interaction**: grass density falls off at
   `max_distance = 200 elmos`. The terrain shader's depth
   range (Sprint 13 / ADR-037) is auto-tuned by orbit distance.
   Match the grass fade to the same falloff curve so blades
   don't pop in/out.

10. **BAR's grass blade texture**: the `grassBladeTex` from
    `mapinfo.resources` is a texture for the blade silhouette.
    Sprint 34 ships a procedural simple-quad shape; loading the
    blade texture is Sprint 35's resource pass.

11. **Platform portability**: pure WGSL; no concerns.

## Step 7 — Exit criteria

- 5+ commits on `main`: density bake, instance generation,
  shader + pipeline, ADR-043, validation, rollup.
- 1 devlog folder filled.
- SRS / ROADMAP STATUS UPDATEs (R6 done; renderer 7/8).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test:
  - Grass-field fixture renders 100k blades within frame budget
    on Vega 8.
  - Wind direction matches water motion.
  - LOD: blades fade smoothly at max_distance.
- Final devlog: summary + "Sprint 35 = emission + sky-reflect
  + parallax" handoff.

Start with the density-texture bake; that's the lightweight
part. The shader is mechanical; the perf tuning is the unknown.
