# Sprint 28 — Atmosphere + fog: exponential height fog + sun colour + skybox cubemap (R2)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 28** — the third renderer-parity sprint (R2 in
original ROADMAP numbering). After this sprint the editor's view of
the terrain has:

- **Exponential height fog** between `mapinfo.atmosphere.fogStart`
  and `fogEnd`, tinted by `fogColor`.
- **Sun colour + direction** modulates terrain lighting via
  `lighting.sun_dir` (already wired in Sprint 25; this sprint adds
  the colour ramp from atmosphere data).
- **Sky colour background** for the part of the viewport not
  covered by terrain (`mapinfo.atmosphere.skyColor`).
- **Skybox cubemap** when `mapinfo.atmosphere.skyBox` is set —
  samples a 6-face cubemap into the background pass.
- **Wind direction** affects water motion (ties to Sprint 26's
  water shader) and is the foundation Sprint 34 (grass) uses for
  blade animation.

This is the cheapest renderer-parity sprint per the ROADMAP
("exponential height fog" + "fullscreen clear with sky color" +
optional cubemap sample). Most of the work is plumbing — the
shader math is one fog equation.

**Prerequisites:**
- Sprint 25 (terrain shader parity) MUST be ticked. Atmosphere
  applies on top of terrain output.
- Sprint 26 (water polish) MUST be ticked. Lava emission glow
  uses sun-angle ramp from this sprint.

**Reference clone of RecoilEngine:** `/home/teague/code/RecoilEngine`.
Critical files:
- `cont/base/springcontent/shaders/GLSL/Atmosphere*.glsl`
- `cont/base/springcontent/shaders/GLSL/Sky*.glsl`
- `rts/Rendering/Env/SkyLight*.cpp`, `Atmosphere.cpp`.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — atmosphere schema,
   §2.1 #11 parity commitment.
3. `/home/teague/code/BARMapEditor/docs/research/renderer-bar-parity/ROADMAP.md`
   — Sprint 17 section (original numbering) = this Sprint 28.
4. `/home/teague/code/BARMapEditor/docs/research/source-audit-2026-05-18/FINDINGS.md`
   — atmosphere field semantics.
5. `/home/teague/code/RecoilEngine/cont/base/springcontent/shaders/GLSL/Atmosphere.glsl`
   + `Sky*.glsl`.
6. `/home/teague/code/RecoilEngine/rts/Rendering/Env/Atmosphere.cpp`
   + `SkyLight.cpp`.
7. `/home/teague/code/BARMapEditor/crates/barme-app/src/terrain.wgsl`
   (Sprint 25) — final output composes with fog.
8. `/home/teague/code/BARMapEditor/crates/barme-core/src/mapinfo_schema.rs`
   — `AtmosphereBlock` field layout.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-28-atmosphere-and-fog
```

## Step 3 — Scope

In order, one commit per chunk:

### 1. Atmosphere uniform block + bind groups

`crates/barme-app/src/render.rs`:

```rust
#[repr(C)]
pub struct AtmosphereUniforms {
    sun_color: [f32; 4],     // RGB + intensity
    sky_color: [f32; 4],     // RGB + ambient_strength
    fog_color: [f32; 4],     // RGB + density
    fog_start_end: [f32; 4], // (start_dist, end_dist, height_falloff, _)
    cloud_color: [f32; 4],
    cloud_density: f32,
    min_max_wind: [f32; 2],
    sky_axis_angle: [f32; 4], // axis xyz + radians angle (from skyAxisAngle)
    has_skybox: u32,
}
```

Add `AtmosphereUniforms` to the terrain pipeline's bind group
(extends Sprint 25's TerrainUniforms or as a separate bind group).
The skybox cubemap (if set) binds as `texture_cube<f32>` to a new
slot.

### 2. Fog in terrain fragment shader

In `terrain.wgsl`'s fragment stage, after lighting is computed:

```wgsl
let dist_from_camera = length(world_pos - uniforms.eye.xyz);
let height_factor = exp(-world_pos.y * atmosphere.fog_start_end.z);
let fog_t = smoothstep(
    atmosphere.fog_start_end.x,
    atmosphere.fog_start_end.y,
    dist_from_camera * height_factor
);
let fogged = mix(lighting_result, atmosphere.fog_color.rgb, fog_t * atmosphere.fog_color.a);
return vec4<f32>(fogged, 1.0);
```

The height-factor falls off exponentially — atmospheres get thinner
with altitude. Test with `fogStart = 0.1, fogEnd = 1.0, fogColor =
(0.4, 0.5, 0.7, 0.6)` on a 16-SMU map; the terrain should haze
toward blue at far range.

### 3. Sky background pass

When the rasterizer doesn't cover a pixel (terrain doesn't fill
the viewport), render the sky color OR skybox.

**Implementation:**
- Allocate a "sky" pipeline that renders a full-screen quad behind
  everything. Depth-test disabled (writes at far plane), runs
  BEFORE terrain.
- If `has_skybox == 0u`: shader outputs `atmosphere.sky_color.rgb`
  (with optional gradient toward horizon — see pitfall #2).
- If `has_skybox == 1u`: shader samples the cubemap using a
  ray-from-camera-through-pixel direction. The skybox is rotated
  by `sky_axis_angle` (from Sprint 10 / mapinfo audit fix).

`crates/barme-app/src/sky.wgsl` (new):

```wgsl
@fragment
fn fs_main(@location(0) world_dir: vec3<f32>) -> @location(0) vec4<f32> {
    let dir = normalize(world_dir);
    if (uniforms.has_skybox != 0u) {
        let rotated = rotate_axis_angle(dir, uniforms.sky_axis_angle.xyz, uniforms.sky_axis_angle.w);
        return textureSample(skybox_cube, sampler_lin, rotated);
    }
    // gradient toward horizon
    let horizon_blend = smoothstep(-0.2, 0.4, dir.y);
    let sky = mix(uniforms.fog_color.rgb, uniforms.sky_color.rgb, horizon_blend);
    return vec4<f32>(sky, 1.0);
}
```

### 4. Sun colour ramp + day/night cycle

The sun direction comes from `mapinfo.lighting.sun_dir`. Sun
colour at horizon vs zenith ramps via:

```wgsl
let sun_angle_factor = clamp(dot(sun_dir.xyz, vec3<f32>(0.0, 1.0, 0.0)), 0.0, 1.0);
let sun_color_effective = mix(
    atmosphere.fog_color.rgb,     // horizon = sunset/sunrise tinted
    atmosphere.sun_color.rgb,     // zenith = full white-ish
    sun_angle_factor
);
```

The terrain shader (Sprint 25) consumed `sun_color` as a flat
colour; Sprint 28 makes it angle-dependent. This is also where
Sprint 26's lava day/night ramp gets its `sun_angle_factor`.

### 5. Skybox cubemap loading

When `mapinfo.atmosphere.skyBox` points to a `.dds` cubemap file:
- Load via `image_dds` or `bcdec_rs` (already considered for
  Sprint 29). For Sprint 28 simplicity, support uncompressed RGBA8
  cubemaps in PNG (6 PNGs in a folder, `<base>_px.png`,
  `_nx.png`, `_py.png`, etc.) — matches BAR's stock cube format.
- Cache via `tools/skybox-cache/<sha>.dds` content-addressed.
- Bind as `texture_cube<f32>`.

**Default**: no cubemap = solid `sky_color`. The user can paste a
path in the F9 form's Atmosphere tab.

### 6. F9 form atmosphere tab fields

Already shipped in Sprint 18 / C7. Verify all fields render with
tooltips from Sprint 19. Specifically:
- minWind / maxWind (DragValues, range 0..=20)
- fogStart / fogEnd (DragValues, range 0..=2)
- fogColor / sunColor / skyColor / cloudColor (color buttons)
- skyAxisAngle (`[f32; 4]`)
- skyBox (TextEdit; file picker?)
- cloudDensity (Slider 0..=1)

### 7. Wind direction passing to water + grass

The water shader (Sprint 26) takes `wind_speed_x` / `wind_speed_z`
directly. Sprint 28 derives these from
`mapinfo.atmosphere.minWind` / `maxWind`. Stochastic per-frame
between min and max, with a slow rotation (so wind shifts
direction over time, matching BAR's wind sim).

Grass (Sprint 34) consumes the same wind direction for blade
animation. The wind state lives in `AtmosphereUniforms` so both
shaders read it.

### 8. ADR-040 (new)

`/home/teague/code/BARMapEditor/docs/DECISIONS.md`:

```
## ADR-040 — Atmosphere + fog (Sprint 28 / R2)

Status: ADOPTED 2026-05-XX
...
```

Cover: fog math (exponential vs linear; chose exponential per
BAR), skybox loading + caching, wind state propagation to water /
grass, day/night sun-colour ramp.

### 9. Validation fixtures

Reuse parity fixtures from Sprints 25 + 26. Add:
- `assets/parity-fixtures/foggy-map/` — a map with strong fog.
- `assets/parity-fixtures/sunset/` — sun at horizon.
- `assets/parity-fixtures/skybox/` — a map with a custom skybox.

**Platform-portability checklist** (renderer sprints):
- `texture_cube<f32>` works on Vulkan + Metal + D3D12 + GL.
- Cubemap sampling does not require backend-specific extensions.
- Test on Vulkan + GL (Mesa) for software-fallback parity.
- Document untested macOS / Windows.

### 10. Rollup

STATUS UPDATEs in SRS / ROADMAP (R2 done, renderer-parity 3/8).
closing devlog log. "Sprint 29 = feature asset decoding (S3O +
decal sprites)" handoff.

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on skybox-cubemap load
+ cache hit/miss; `warn!` on missing skybox; `trace!` on per-
frame wind state.

## Step 5 — Out of scope

- **Sky reflection on terrain / water** — Sprint 35 (sky-reflect
  modulation texture).
- **Volumetric clouds** — out of scope; `cloudDensity` only
  modulates a flat cloud layer.
- **Day/night cycle animation** — sun direction is static per
  project; SRS doesn't commit to animated time-of-day.
- **Aurora / particle effects** — out of scope.
- **HDR pipeline** — RGBA8 output stays; HDR is Stage-2 polish.

## Step 6 — Critical pitfalls (read twice)

1. **Fog must NOT apply to skybox**. The sky background pass
   already represents the "infinite far plane"; fogging the
   skybox doubles-fogs it. Render terrain → apply fog → composite
   over sky background.

2. **Skybox rotation around `skyAxisAngle`** — Sprint 10 audit
   fixed this. Axis is XYZ, angle is radians. Default `[0, 0, 1,
   0]` (no rotation). Verify your `rotate_axis_angle` WGSL helper
   uses the Rodrigues formula correctly.

3. **Cubemap face orientation**: Vulkan and OpenGL use different
   face conventions for cubemaps. wgpu normalises to GL convention.
   Test by binding a cubemap with each face a distinct colour and
   verifying the centre of each face renders the right colour.

4. **Wind direction in water**: Sprint 26 used editor-camera-
   aligned wind. Sprint 28 replaces with atmosphere data. Don't
   leave the camera-aligned fallback path — switch decisively.

5. **`fogStartEqualsFogEnd` is a Sprint 21 lint warning**. If the
   user mis-sets these, the fog becomes a step function (binary
   in/out). The shader should handle this gracefully via
   `smoothstep` (clamps to 0..1 with no NaN).

6. **`skyColor + fogColor` choice**: BAR maps often set skyColor
   ≠ fogColor (sky is brighter; fog is duller). The horizon
   gradient blends fogColor at horizon to skyColor at zenith.
   Test with a real BAR map (`Comet_Catcher_Remake.sd7`'s mapinfo
   has both).

7. **Stochastic wind**: don't introduce a seed-controlled noise
   for wind. Use a slow `sin(time)` + `cos(time)` ramp. Deterministic
   reproducible output matters for the parity fixture.

8. **Skybox file path resolution**: relative to the project's
   `mapconfig/` directory by default. Absolute paths supported.
   F13 (Stage 2) will bundle the cubemap into the `.sd7`; Sprint
   28 ships only the editor-preview path.

9. **Sky-pass depth state**: depth-test disabled, depth-write
   disabled, drawn FIRST. Subsequent terrain/water/markers pass
   on top.

10. **Platform portability**: cubemap is standard WGSL; no
    backend-specific concerns.

11. **`cloudDensity` field**: the legacy plumbing (Sprint 6 / C3)
    includes cloudDensity in mapinfo. Sprint 28 reads it but
    doesn't render volumetric clouds — just a uniform value the
    fog blends with. The flat-cloud-color blend lives in the
    background pass.

## Step 7 — Exit criteria

- 5+ commits on `main`: uniforms + binds, fog in terrain shader,
  sky pass, cubemap loading, ADR-040, rollup.
- 1 devlog folder filled.
- SRS / ROADMAP STATUS UPDATEs (R2 done, renderer-parity 3/8).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test:
  - Foggy-map fixture renders with visible distance haze.
  - Sunset fixture has correct horizon colour ramp.
  - Skybox fixture loads + renders cubemap behind terrain.
  - Water motion changes with min/max wind values.
  - F9 atmosphere tab tooltips visible.
- Final devlog: summary + "Sprint 29 = feature asset decoding"
  handoff.

Start by writing the uniform layout, then the fog equation in
the existing terrain.wgsl, then the sky-background pass. Cubemap
loading is the heaviest engineering work.
