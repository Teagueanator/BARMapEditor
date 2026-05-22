// Stage 1 terrain shader.
//
// Sprint 25 / R1 / ADR-043 — UNIFIED port of Recoil's
// `cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl`
// (`SMF_DETAIL_NORMAL_TEXTURE_SPLATTING` branch). Each fragment-stage
// section cites the GLSL source line so a reviewer can read the WGSL
// alongside the engine source. The transcription follows
// FINDINGS §7.1–§7.6 (source-audit 2026-05-18) which corrects five
// pre-existing claims about the splat composite math.
//
// Vertex stage samples the heightmap as a storage texture (r16uint) so
// brush edits become texture writes, not full-mesh re-uploads. A 4-tap
// finite difference of neighbouring heights yields a per-vertex normal
// in world space — kept as a fallback for the fragment stage's base
// normal when no `normalsTex` is bound (the engine's base normal is
// sampled per-fragment from a baked R+A texture; the editor doesn't
// run that bake at preview time so the vertex normal carries the real
// signal until a future sprint plumbs the heightmap → R+A bake).
//
// Fragment stage:
//   1. Sample base normal from `normals_tex.ra` (FINDINGS §7.5). The
//      `has_base_normal_tex` bit (flags.w & 1) picks between sampled
//      and vertex normal — both are uniform per-frame so the branch
//      is uniform control flow.
//   2. Build TBN per-fragment via cross(normal, vec3(-1, 0, 0))
//      (FINDINGS §7.4 — NOT static `T=+X, B=+Z`).
//   3. Sample the 4 DNTS slot normals with PER-CHANNEL UV multipliers
//      `worldPos.xz * tex_scales.{r,g,b,a}` (FINDINGS §7.3).
//   4. splatCofac = textureSample(distr, uv) × tex_mults × active_mask
//      (FINDINGS §7.3); blend strength = min(1, dot(cofac, 1)).
//   5. splat_detail_normal = sum of (decoded × cofac) per channel;
//      `.y = max(.y, 0.01)` per `SMFFragProg.glsl:189`.
//   6. Final normal = normalize(mix(base_normal, normalize(stnMatrix
//      × splat_detail_normal.xyz), blend_strength))
//      (`SMFFragProg.glsl:328`).
//   7. Lambert + Blinn-Phong using the final normal, with per-fragment
//      specular exponent `specularCol.a × 16.0` (FINDINGS §7.6).
//   8. fragColor = (diffuseCol + detailCol) × shadeInt + specularInt
//      (`SMFFragProg.glsl:381 + 422`).
//
// Per FINDINGS §7.1, `SMF_INTENSITY_MULT = 210/255` is pre-applied to
// `ground_ambient` + `ground_diffuse` CPU-side so the WGSL stays free
// of the multiply. The engine multiplies once per-fragment inside
// `GetShadeInt`; we hoist.
//
// ADR-008 governs world-space conventions: Y-up left-handed,
// +X east, +Z south, 8 elmos per heightmap pixel.

struct Uniforms {
    view_proj: mat4x4<f32>,
    // .x = max world height (elmos) for the gradient + Y displacement.
    // .y = elmos per heightmap pixel (= 8.0 per ADR-008).
    // .z = world extent X (elmos) — for splat-distribution UV math.
    // .w = world extent Z (elmos) — same.
    params: vec4<f32>,
    // .x = MIN world height (elmos). Negative = the lowest heightmap
    //   sample (`raw u16 = 0`) sits below BAR's water plane at Y = 0,
    //   the only way to make a flooded basin visible
    //   (`Ground.h::GetWaterPlaneLevel` is `consteval 0.0`). Together
    //   with `params.x` (= max_height), gives the linear mapping
    //   `y = min_h + (raw / 65535) * (max_h - min_h)`.
    // .y = use_composite_rt flag (0.0 = sample the Sprint-9 splat
    //   composite + biome ramp; > 0.5 = sample the Sprint-16
    //   composite RT instead). Sprint 16 / D9 / ADR-039.
    // .z / .w reserved for future scalars (lighting tuning,
    //   tide animation, etc.).
    params2: vec4<f32>,
};

// Per-channel splat tuning + lighting state (ADR-036 / ADR-043).
//
// `tex_scales` maps directly to mapinfo `splats.texScales` — each
// channel scales `worldPos.xz` to produce the per-layer detail UV
// (FINDINGS §7.3).
// `tex_mults` maps to `splats.texMults` — per-channel weight
// multiplier applied to the splat distribution sample.
//
// `flags.x = active_slot_mask` (bit i set = layer i is bound to a
// real slot). Unbound layers contribute zero so a fresh project with
// no slots assigned still renders the fallback gradient.
//
// `flags.y = diffuse_in_alpha` (0 / 1). When set, the engine's
// `SMF_DETAIL_NORMAL_DIFFUSE_ALPHA` path adds
// `clamp(splat_detail_normal.a, -1, 1)` to the diffuse colour
// (`SMFFragProg.glsl:192 + 323`); we implement the same.
//
// `flags.z = buildable_overlay_on` — when 1, fragments where the
// world-normal Y slope drops below cos(10°) get mixed with a red
// "too steep" overlay.
//
// `flags.w = tex_present_bits` (Sprint 25 / R1 / ADR-043):
//   bit 0 = base-normal texture bound (sample normals_tex.ra);
//   bit 1 = specular texture bound (per-fragment exponent);
//   bit 2 = DNTS slot-normal array populated (do the §7.3 blend).
// `sun_dir` is the world-space *to-sun* direction (normalized).
// `ground_ambient` / `ground_diffuse` are pre-multiplied by
// `SMF_INTENSITY_MULT = 210/255` CPU-side (FINDINGS §7.1).
// `ground_specular.xyz` is the fallback specular colour;
// `ground_specular.w` is the global exponent used when no specular
// texture is bound (FINDINGS §7.6).
// `camera_pos.xyz` is the world-space eye for the Blinn-Phong
// half-vector (the engine computes this in
// `SMFVertProg.glsl:34-41`; we move it to the fragment stage because
// the vertex shader doesn't carry it).
struct SplatU {
    tex_scales: vec4<f32>,
    tex_mults:  vec4<f32>,
    flags:      vec4<u32>,
    sun_dir:    vec4<f32>,
    ground_ambient: vec4<f32>,
    ground_diffuse: vec4<f32>,
    ground_specular: vec4<f32>,
    camera_pos: vec4<f32>,
};

// Sprint 30 / R4 / ADR-048 — shadow sampling uniform block. View-proj
// matrix from `ShadowCamera::view_proj_matrix` (Commit 1) plus bias +
// `mapinfo.lighting.ground_shadow_density` + enabled flag. Field
// order MUST match the CPU `ShadowUniforms` mirror exactly.
//
//   view_proj: shadow camera VP, transforms world_pos into shadow
//              NDC ([-1,1] xy, [0,1] z per wgpu/D3D/Vulkan convention)
//   params:    (bias, ground_shadow_density, unit_shadow_density,
//              enabled). Engine `groundShadowDensity` is the blend
//              factor between "always lit" (density=0) and "sampled
//              shadow" (density=1).
struct ShadowU {
    view_proj: mat4x4<f32>,
    params:    vec4<f32>,
};

// Sprint 28 / R2 / ADR-045 — atmosphere uniform block. Composes fog,
// sky colour, sun-angle ramp, deterministic wind state, and cloud tint
// on top of Sprint 25's terrain output. Field order MUST match the
// CPU `AtmosphereUniforms` mirror (`render.rs::AtmosphereUniforms`)
// exactly; the size-pin test in `render.rs::tests` catches drift.
//
//   sun_color:     lighting.sunColor (.w = intensity, reserved)
//   sky_color:     atmosphere.skyColor (.w = ambient strength, reserved)
//   fog_color:     atmosphere.fogColor + .w = fog density [0, 1]
//   fog_start_end: (fog_start, fog_end, height_falloff, _)
//   cloud_color:   atmosphere.cloudColor + .w = cloud density
//   wind:          (wind_x, wind_z, wind_speed, _) — pre-rotated by
//                  App-side deterministic sin/cos ramp (PITFALL #7)
//   sky_axis_angle: atmosphere.skyAxisAngle (xyz axis, .w radians).
//                   Reserved for the deferred-cubemap sprint.
//   flags:         (has_skybox, sun_disc_size, _, _) — `has_skybox`
//                  stays 0 for Sprint 28 (cubemap deferred per
//                  ADR-045).
struct AtmosphereU {
    sun_color:     vec4<f32>,
    sky_color:     vec4<f32>,
    fog_color:     vec4<f32>,
    fog_start_end: vec4<f32>,
    cloud_color:   vec4<f32>,
    wind:          vec4<f32>,
    sky_axis_angle: vec4<f32>,
    sun_dir:       vec4<f32>,
    flags:         vec4<u32>,
};

@group(0) @binding(0)  var<uniform> u: Uniforms;
@group(0) @binding(1)  var heightmap: texture_2d<u32>;
@group(0) @binding(2)  var<uniform> sp: SplatU;
@group(0) @binding(3)  var splat_distr: texture_2d<f32>;
@group(0) @binding(4)  var splat_distr_samp: sampler;
// Sprint 25 / R1 / ADR-043 — slot-normal array. Was the Sprint-9
// 4-layer slot diffuse array; Sprint 17 retired the diffuse role
// (composite RT takes over) and Sprint 25 repurposes the binding
// as DNTS normals (engine `splatDetailNormalTex1..4`).
@group(0) @binding(5)  var slot_normals: texture_2d_array<f32>;
@group(0) @binding(6)  var slot_normals_samp: sampler;
// Sprint 16 / D9 / ADR-039 — layered composite RT bound as the
// diffuse base when `u.params2.y > 0.5`. Cap dims 4096²; the shader
// bilinearly upsamples to the terrain's per-fragment sample
// coordinate. CPU bake stays authoritative for the .sd7 export.
@group(0) @binding(7)  var composite_rt: texture_2d<f32>;
@group(0) @binding(8)  var composite_samp: sampler;
// Sprint 25 / R1 / ADR-043 — base normal map (engine `normalsTex`).
// FINDINGS §7.5 — only R + A channels read; ny = sqrt(1 - nx² - nz²).
@group(0) @binding(9)  var normals_tex: texture_2d<f32>;
@group(0) @binding(10) var normals_samp: sampler;
// Sprint 25 / R1 / ADR-043 — specular map (engine `specularTex`).
// FINDINGS §7.6 — `specular_exp = sample.a × 16.0` per fragment.
@group(0) @binding(11) var specular_tex: texture_2d<f32>;
@group(0) @binding(12) var specular_samp: sampler;
// Sprint 28 / R2 / ADR-045 — atmosphere block.
@group(0) @binding(13) var<uniform> atmos: AtmosphereU;
// Sprint 30 / R4 / ADR-048 — directional shadow sampling. The depth
// texture is the output of the depth-only shadow-gen pass (`shadow_gen.
// wgsl`); the comparison sampler does the hardware-PCF / depth-test
// for us. `textureSampleCompareLevel` is well-supported on every wgpu
// backend (Vulkan / Metal / D3D12 / GL).
@group(0) @binding(14) var shadow_map: texture_depth_2d;
@group(0) @binding(15) var shadow_sampler: sampler_comparison;
@group(0) @binding(16) var<uniform> shadow: ShadowU;

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv_norm: vec2<f32>, // map-normalized UV for splat distribution
};

// Sample heightmap at integer pixel (px, pz), clamped to bounds, and
// return the world-space Y in elmos. The raw `u16` value linearly maps
// to `[min_h, max_h]` — when `min_h < 0` the heightmap can dip below
// BAR's water plane at Y = 0.
fn sample_y(px: i32, pz: i32, dims: vec2<u32>, min_h: f32, max_h: f32) -> f32 {
    let cx = clamp(px, 0, i32(dims.x) - 1);
    let cz = clamp(pz, 0, i32(dims.y) - 1);
    let texel = textureLoad(heightmap, vec2<i32>(cx, cz), 0);
    let t = f32(texel.r) * (1.0 / 65535.0);
    return min_h + t * (max_h - min_h);
}

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    let dims = textureDimensions(heightmap);
    let px = i32(vid % dims.x);
    let pz = i32(vid / dims.x);

    let elmos_per_px = u.params.y;
    let max_h = u.params.x;
    let min_h = u.params2.x;
    let y = sample_y(px, pz, dims, min_h, max_h);
    let world_pos = vec3<f32>(
        f32(px) * elmos_per_px,
        y,
        f32(pz) * elmos_per_px,
    );

    // 4-tap finite difference for the per-vertex normal. Engine path
    // samples a baked SMT normal texture per-fragment; until the
    // editor bakes that texture, the vertex normal carries the real
    // signal and the fragment stage's `has_base_normal_tex` flag
    // stays at 0 so the sampled `normals_tex` (1×1 "up" fallback)
    // is ignored.
    let h_l = sample_y(px - 1, pz, dims, min_h, max_h);
    let h_r = sample_y(px + 1, pz, dims, min_h, max_h);
    let h_u = sample_y(px, pz - 1, dims, min_h, max_h);
    let h_d = sample_y(px, pz + 1, dims, min_h, max_h);
    // Cross-product of the X and Z gradient vectors gives a +Y-up
    // normal in world space (8 elmos per pixel along XZ, height delta
    // along Y). The 2.0 factor accounts for the symmetric finite
    // difference span = 2 × elmos_per_pixel.
    let dx = vec3<f32>(2.0 * elmos_per_px, h_r - h_l, 0.0);
    let dz = vec3<f32>(0.0, h_d - h_u, 2.0 * elmos_per_px);
    let n_raw = cross(dz, dx);
    let world_normal = normalize(n_raw);

    let ex = max(u.params.z, 1.0);
    let ez = max(u.params.w, 1.0);
    let uv_norm = vec2<f32>(world_pos.x / ex, world_pos.z / ez);

    var out: VsOut;
    out.clip_pos = u.view_proj * vec4<f32>(world_pos, 1.0);
    out.world_pos = world_pos;
    out.world_normal = world_normal;
    out.uv_norm = uv_norm;
    return out;
}

// Sprint 30 / R4 / ADR-048 — sample the directional shadow map with
// 3×3 Percentage-Closer Filtering (PCF) for soft edges.
//
// Returns the engine's `shadowCoeff` value: 1.0 = fully lit, lower =
// darker. Math mirrors `SMFFragProg.glsl:362-372`:
//   shadowCoeff = mix(1.0, sampled, groundShadowDensity)
//
// Single-sample shadows produce hard, aliased edges that look ugly on
// curved terrain (the shadow map's per-texel quantisation shows
// through). 3×3 PCF averages 9 comparison samples in a one-texel-wide
// kernel around the fragment's shadow UV — each sample returns 0.0
// (occluded) or 1.0 (lit), so the average is a smooth in-between
// value at shadow edges. 9 fetches per fragment costs ~300M fetches
// per frame at 16-SMU on Vega 8 (PITFALL §3); profile if frames break
// budget. Disabling PCF means dropping the loop back to a single
// fetch at `uv_center` — gated by `shadow.params.w == 1` (enabled
// flag). A future sprint can widen `params.w` to a kernel-size
// selector if a Vega-8 fallback is needed.
//
// PITFALL §1 (shadow acne): we subtract a depth bias from the
// fragment's shadow-space Z BEFORE the comparison, loosening the
// "fragment depth ≤ stored depth" test by `bias` so floating-point
// noise on flat surfaces stops producing self-shadow speckle.
// PITFALL §2 (Peter-Panning): with too much bias the shadow detaches
// from the caster. The default 0.005 (= `SHADOW_DEPTH_BIAS`) balances
// the two; per-surface slope-scaled bias is a Stage-2 polish.
// PITFALL §8: WGSL `texture_depth_2d` pairs with `sampler_comparison`
// (not regular `sampler`); compare-function is `LessEqual` (set CPU-
// side in `install_shadow_resources`).
//
// NDC convention reminder: glam's `Mat4::orthographic_lh` outputs
// `x, y ∈ [-1, 1]` (y-up) and `z ∈ [0, 1]` (0 = near, 1 = far) —
// matches wgpu's clip space. Texture UVs are y-DOWN, so the y axis
// flips during the NDC → UV conversion.
fn sample_shadow(world_pos: vec3<f32>) -> f32 {
    // Shadow disabled → no shadow attenuation at all.
    if (shadow.params.w < 0.5) {
        return 1.0;
    }
    let clip = shadow.view_proj * vec4<f32>(world_pos, 1.0);
    let ndc = clip.xyz / clip.w;
    // Outside the shadow camera's frustum → return lit. Without this
    // bounds check, fragments past the AABB would sample the
    // ClampToEdge border of the depth texture (which would itself
    // depend on what the depth-only pass left there last frame).
    if (ndc.x < -1.0 || ndc.x > 1.0
     || ndc.y < -1.0 || ndc.y > 1.0
     || ndc.z < 0.0 || ndc.z > 1.0) {
        return 1.0;
    }
    // NDC → texture UV. wgpu/D3D: NDC y is up; texture v is down.
    let uv_center = vec2<f32>(ndc.x * 0.5 + 0.5, ndc.y * -0.5 + 0.5);
    // Subtract bias from the reference depth — loosens the
    // LessEqual comparison so flat surfaces don't self-shadow.
    let bias = shadow.params.x;
    let ref_depth = ndc.z - bias;
    // Pull texel size from the shadow map's actual dims so a future
    // SHADOW_MAP_SIZE change doesn't drift the WGSL out of sync with
    // the Rust constant (PITFALL §11).
    let dims = textureDimensions(shadow_map);
    let texel = vec2<f32>(1.0 / f32(dims.x), 1.0 / f32(dims.y));
    // 3×3 PCF: average 9 comparison samples in a one-texel-wide
    // kernel. Each `textureSampleCompareLevel` returns 0 (occluded)
    // or 1 (lit); the average is a smooth in-between at shadow edges.
    var total: f32 = 0.0;
    for (var dx: i32 = -1; dx <= 1; dx = dx + 1) {
        for (var dy: i32 = -1; dy <= 1; dy = dy + 1) {
            let offset = vec2<f32>(f32(dx), f32(dy)) * texel;
            total = total
                + textureSampleCompareLevel(shadow_map, shadow_sampler, uv_center + offset, ref_depth);
        }
    }
    let s = total / 9.0;
    // Engine convention: shadowCoeff = mix(1, sampled, density).
    // density = 0 → no shadow visible. density = 1 → full sampled
    // shadow. Default is 0.8 per `MapInfo::bar_default`.
    return mix(1.0, s, shadow.params.y);
}

// Heightmap → biome gradient. Mirrors `crates/barme-app/src/ui/minimap.rs`
// `biome_ramp` so the central viewport and the mini-map agree on the
// fallback colour family. Used as the diffuse base when no composite
// RT is bound (i.e. the project's layer stack is empty).
fn biome_ramp(t: f32) -> vec3<f32> {
    let tc = clamp(t, 0.0, 1.0);
    if (tc < 0.30) { return vec3<f32>(0.157, 0.235, 0.337); }
    if (tc < 0.45) { return vec3<f32>(0.243, 0.392, 0.306); }
    if (tc < 0.65) { return vec3<f32>(0.408, 0.478, 0.361); }
    if (tc < 0.82) { return vec3<f32>(0.502, 0.486, 0.439); }
    return vec3<f32>(0.863, 0.878, 0.902);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let max_h = u.params.x;
    let min_h = u.params2.x;

    // ─── 1. Texture-presence bits (FINDINGS §7.2) ────────────────
    // Engine's `SMF_DETAIL_NORMAL_TEXTURE_SPLATTING` gate is
    // `splatDistrTex && HaveSplatNormalTexture()` per
    // `SMFRenderState.cpp:114` — NOT AND-ed with specularTex.
    let tex_bits = sp.flags.w;
    let has_base_normal = f32(tex_bits & 1u);
    let has_specular   = f32((tex_bits >> 1u) & 1u);
    let has_dnts_norm  = f32((tex_bits >> 2u) & 1u);

    // ─── 2. Base normal (FINDINGS §7.5) ──────────────────────────
    // `vec3 normal = GetFragmentNormal(normTexCoords);` —
    // SMFFragProg.glsl:269 / function defined at lines 146-150.
    // `normal.xz = texture2D(normalsTex, uv).ra; normal.y = sqrt(1
    // - dot(normal.xz, normal.xz));`. Per pitfall #10 we sample
    // unconditionally and blend with the uniform-controlled
    // `has_base_normal` factor; the 1×1 fallback decodes to
    // (0, 1, 0) so the mix is a no-op when nothing is bound.
    let n_raw = textureSample(normals_tex, normals_samp, in.uv_norm);
    let n_x = n_raw.r * 2.0 - 1.0;
    let n_z = n_raw.a * 2.0 - 1.0;
    let n_y = sqrt(max(0.0, 1.0 - n_x * n_x - n_z * n_z));
    let sampled_normal = vec3<f32>(n_x, n_y, n_z);
    // Fall back to the vertex normal when no real base-normal texture
    // is bound (the editor's pre-bake state). `normalize` guards
    // against denormal sums from the interpolator.
    let vertex_normal = normalize(in.world_normal);
    var normal = normalize(mix(vertex_normal, sampled_normal, has_base_normal));

    // ─── 3. Per-fragment TBN (FINDINGS §7.4 / SMFFragProg.glsl:276-278)
    // The engine uses the per-fragment NORMAL to build a stable TBN —
    // NOT a static `T=+X, B=+Z` basis. `tTangent = normalize(cross(
    // normal, vec3(-1, 0, 0)))`, `sTangent = cross(normal, tTangent)`.
    // The columns are (sTangent, tTangent, normal). Normal maps "swim"
    // on sloped surfaces if you skip this rebuild.
    let t_tangent = normalize(cross(normal, vec3<f32>(-1.0, 0.0, 0.0)));
    let s_tangent = cross(normal, t_tangent);
    let stn = mat3x3<f32>(s_tangent, t_tangent, normal);

    // ─── 4. DNTS composite (FINDINGS §7.3 / SMFFragProg.glsl:174-198)
    // `splatCofac = texture2D(splatDistrTex, uv) * splatTexMults`,
    // then per-channel layer UVs `worldPos.xzxz * texScales.rrgg`
    // and `worldPos.xzxz * texScales.bbaa`. The full vec4 sample is
    // signed-decoded `(* 2 - 1)`, then multiplied by the per-channel
    // cofactor. The `active_slot_mask` gates unbound layers so a
    // fresh project doesn't sample a stale slot.
    let dist = textureSample(splat_distr, splat_distr_samp, in.uv_norm);
    let mask = sp.flags.x;
    let active_mask = vec4<f32>(
        f32((mask >> 0u) & 1u),
        f32((mask >> 1u) & 1u),
        f32((mask >> 2u) & 1u),
        f32((mask >> 3u) & 1u),
    );
    let cofac = dist * sp.tex_mults * active_mask;
    let blend_strength = clamp(dot(cofac, vec4<f32>(1.0, 1.0, 1.0, 1.0)), 0.0, 1.0);

    // Per-channel UV streams. Each layer scales worldPos.xz by its
    // own `tex_scales[i]` — SMFFragProg.glsl:175-176 packs them into
    // two vec4s via the `.xzxz * .rrgg` / `.bbaa` swizzles; we
    // unroll into four vec2s for clarity.
    let uv0 = in.world_pos.xz * sp.tex_scales.x;
    let uv1 = in.world_pos.xz * sp.tex_scales.y;
    let uv2 = in.world_pos.xz * sp.tex_scales.z;
    let uv3 = in.world_pos.xz * sp.tex_scales.w;
    let s0 = textureSample(slot_normals, slot_normals_samp, uv0, 0);
    let s1 = textureSample(slot_normals, slot_normals_samp, uv1, 1);
    let s2 = textureSample(slot_normals, slot_normals_samp, uv2, 2);
    let s3 = textureSample(slot_normals, slot_normals_samp, uv3, 3);
    let d0 = s0 * 2.0 - 1.0;
    let d1 = s1 * 2.0 - 1.0;
    let d2 = s2 * 2.0 - 1.0;
    let d3 = s3 * 2.0 - 1.0;

    // splatDetailNormal aggregates the decoded layers, weighted by
    // the channel cofactor (SMFFragProg.glsl:183-186). The result is
    // intentionally NOT normalised — the engine waits until the TBN
    // rotates it into world space.
    var splat_detail_normal =
          d0 * cofac.r
        + d1 * cofac.g
        + d2 * cofac.b
        + d3 * cofac.a;
    // SMFFragProg.glsl:189 — clamp .y to a minimum so the y=0 case
    // (all cofacs zero) doesn't produce a degenerate normal.
    splat_detail_normal.y = max(splat_detail_normal.y, 0.01);

    // SMFFragProg.glsl:192 — when SMF_DETAIL_NORMAL_DIFFUSE_ALPHA is
    // set, the splat-detail strength's `.y` is the clamped alpha of
    // the summed DNTS sample (used as the diffuse-add value at
    // SMFFragProg.glsl:323).
    let detail_strength_y = clamp(splat_detail_normal.a, -1.0, 1.0)
        * f32(sp.flags.y);

    // ─── 5. Final normal (SMFFragProg.glsl:328) ──────────────────
    // `normal = normalize(mix(normal, normalize(stnMatrix *
    // splatDetailNormal.xyz), splatDetailStrength.x));`
    // Gated on `has_dnts_norm`: when no DNTS array is bound the
    // sampled detail is identically zero (the 1×1 fallback decodes
    // to (0, 0, 1, 0) so cofac × decoded = 0 for any cofac) and
    // mixing has no effect — but we still mask the mix factor to
    // zero so the math is explicit.
    let dnts_blend = blend_strength * has_dnts_norm;
    let stn_detail = normalize(stn * splat_detail_normal.xyz);
    normal = normalize(mix(normal, stn_detail, dnts_blend));

    // ─── 6. Diffuse base ─────────────────────────────────────────
    // Sprint 16 / D9 / ADR-039 — when the project has a non-empty
    // layer stack the composite RT is bound; otherwise fall back to
    // the height-keyed biome ramp. The Sprint-9 4-layer slot-diffuse
    // path is retired (the binding now carries DNTS normals).
    //
    // Sample both unconditionally + uniform-select (pitfall #10) so
    // WGSL pedants don't flag `textureSample` inside non-uniform
    // control flow. `u.params2.y` is uniform per frame, so this is
    // already uniform control flow at the spec level — hoisting is
    // just defensive.
    let range = max(max_h - min_h, 1.0);
    let height_t = clamp((in.world_pos.y - min_h) / range, 0.0, 1.0);
    let fallback_rgb = biome_ramp(height_t);
    let composite_rgb = textureSample(composite_rt, composite_samp, in.uv_norm).rgb;
    let use_composite = step(0.5, u.params2.y);
    let diffuse_rgb = mix(fallback_rgb, composite_rgb, use_composite);
    // SMFFragProg.glsl:323 — detailCol is `vec4(splatDetailStrength.y)`,
    // a single greyscale value added to the diffuse colour.
    let detail_rgb = vec3<f32>(detail_strength_y);

    // ─── 7. Lighting (SMFFragProg.glsl:333-334, 412-422) ─────────
    // Lambert + Blinn-Phong, with the per-fragment specular exponent
    // = `specularCol.a * 16.0` (FINDINGS §7.6). The engine multiplies
    // ambient + diffuse by `SMF_INTENSITY_MULT = 210/255` inside
    // GetShadeInt; we pre-applied that CPU-side per FINDINGS §7.1
    // so the fragment stage doesn't repeat the dim.
    let sun = normalize(sp.sun_dir.xyz);
    let cos_diffuse = clamp(dot(sun, normal), 0.0, 1.0);
    // View direction toward the camera, in world space. Falling back
    // to the world's +Y axis when the eye sits exactly on the
    // fragment (degenerate, but safer than a NaN normalize).
    let to_eye = sp.camera_pos.xyz - in.world_pos;
    let view_dir = select(
        vec3<f32>(0.0, 1.0, 0.0),
        normalize(to_eye),
        dot(to_eye, to_eye) > 1e-6,
    );
    let half_dir = normalize(sun + view_dir);
    let cos_specular = clamp(dot(half_dir, normal), 0.001, 1.0);

    // Specular colour + exponent. SMFFragProg.glsl:404-416 — when the
    // map ships a specularTex, the colour comes from the sample and
    // the exponent is `sample.a × 16.0`. Without a specularTex the
    // engine falls back to `vec4(groundSpecularColor, 1.0)` and the
    // global `groundSpecularExponent` uniform — we stash both in
    // `sp.ground_specular` (`.xyz` = colour, `.w` = exponent).
    let spec_sample = textureSample(specular_tex, specular_samp, in.uv_norm);
    let spec_col = mix(sp.ground_specular.xyz, spec_sample.rgb, has_specular);
    let spec_exp = mix(sp.ground_specular.w, spec_sample.a * 16.0, has_specular);
    let spec_pow = pow(cos_specular, max(spec_exp, 1.0));
    let specular_int = spec_col * spec_pow;

    // Sprint 28 / R2 / ADR-045 — sun-colour angle ramp. The engine's
    // `Atmosphere::DrawSun` modulates the effective sun colour by the
    // sun's altitude: at the horizon (sun_dir.y → 0) the sun is
    // tinted by `atmosphere.fog_color` (sunset/sunrise glow); at the
    // zenith (sun_dir.y = 1) it's the full `lighting.ground_diffuse`
    // colour. The ramp is `clamp(dot(sun_dir, +Y), 0, 1)`.
    //
    // Sprint 25 consumed `sp.ground_diffuse` flat; we keep the
    // SplatU value as the "max altitude" endpoint and add the
    // atmosphere's fog-tint as the horizon endpoint. The terrain
    // gets warmer at sunrise/sunset matching BAR's sky.
    let sun_world_up = atmos.sun_dir.y;
    let sun_angle_factor = clamp(sun_world_up, 0.0, 1.0);
    let sun_color_effective = mix(
        atmos.fog_color.rgb,
        sp.ground_diffuse.rgb,
        sun_angle_factor,
    );

    // Sprint 30 / R4 / ADR-048 — sample directional shadow. The
    // engine multiplies shadowCoeff into both the diffuse contribution
    // (line 205) and the specular contribution (line 420 — `specularInt
    // *= shadowCoeff;`). Ambient is preserved so shadowed terrain
    // still reads as "lit by the sky" rather than going black.
    let shadow_factor = sample_shadow(in.world_pos);

    // SMFFragProg.glsl:205-206 — shadeInt.rgb = groundAmbientColor +
    // groundDiffuseColor * cosAngleDiffuse * shadowCoeff.
    let shade_int = sp.ground_ambient.rgb
        + sun_color_effective * (cos_diffuse * shadow_factor);

    // ─── 8. Final compose (SMFFragProg.glsl:381 + 422) ───────────
    // `fragColor.rgb = (diffuseCol.rgb + detailCol.rgb) * shadeInt.rgb;`
    // `fragColor.rgb += specularInt * shadowCoeff;`
    var lit = (diffuse_rgb + detail_rgb) * shade_int + specular_int * shadow_factor;

    // Buildable-area overlay (Sprint 11 hotfix follow-up). When
    // `flags.z == 1`, mix red into the composite where the surface is
    // too steep for a factory. BAR's `armlab.lua` / `corlab.lua` set
    // `maxslope = 15`; the engine divides by 1.5 in `movedefs.lua:551`
    // so the effective cap is ~10°. `cos(10°) ≈ 0.9848`. We compare
    // against the BASE world normal (pre-DNTS) so the overlay tracks
    // engine-side gameplay slope, not per-fragment detail.
    let buildable_on = sp.flags.z;
    if (buildable_on == 1u) {
        let cos_max_slope = 0.9848;
        if (vertex_normal.y < cos_max_slope) {
            let too_steep = vec3<f32>(0.95, 0.20, 0.20);
            lit = mix(lit, too_steep, 0.55);
        }
    }

    // ─── 9. Exponential height fog (Sprint 28 / R2 / ADR-045) ────
    //
    // BAR's `Atmosphere.cpp::DrawFog` shapes terrain fog as a function
    // of distance from the camera AND fragment altitude — atmospheres
    // thin with altitude, so a mountain peak at the same horizontal
    // distance reads as less foggy than a valley floor. The math is a
    // smoothstep over the BAR-normalised `(fog_start, fog_end)` range
    // (typically 0.1..1.0), with the input pre-scaled by an
    // exponential height factor.
    //
    // Normalised distance: we measure dist along the view axis as a
    // fraction of the world's far-plane extent (sqrt of XZ map size
    // ≈ camera's framing distance). That matches BAR's `fogStart` /
    // `fogEnd` being unitless `[0, 1]` ranges, not elmos.
    //
    // Height factor: `exp(-y · falloff)`. At sea level (y=0) the
    // factor is 1; at altitude 100 elmos with falloff 0.01 the factor
    // drops to ~0.37 (e^-1). Multiplying the normalised distance by
    // this factor moves the smoothstep input toward 0 with altitude,
    // bringing high terrain *out* of the fog band.
    //
    // PITFALL #1 from the sprint prompt: fog does NOT apply to the
    // skybox. Our offscreen RT's clear-colour IS the sky (commit 3
    // changes it to `atmos.sky_color`), so terrain fog blending
    // toward `fog_color` and the sky background colour being separate
    // is what makes the horizon read correctly. The fragment shader
    // only runs on rasterized terrain pixels, so the clear-colour sky
    // background is untouched here.
    //
    // PITFALL #5: `fog_start == fog_end` is a Sprint 21 lint error,
    // but a freshly-typed value could transit through the F9 form
    // before the lint runs. `smoothstep` is defensively safe — it
    // clamps to [0, 1] without NaN even at the degenerate input
    // (returns 0 below the range, 1 above).
    // `to_eye` is in scope from §7's view-vector setup; reuse it
    // instead of redefining (WGSL flags redefinition as an error).
    let world_extent = max(u.params.z, u.params.w);
    let view_dist = length(to_eye);
    let dist_norm = view_dist / max(world_extent, 1.0);
    let height_factor = exp(-max(in.world_pos.y, 0.0) * atmos.fog_start_end.z);
    let fog_input = dist_norm * height_factor;
    let fog_t = smoothstep(
        atmos.fog_start_end.x,
        atmos.fog_start_end.y,
        fog_input,
    );
    // Fog density (`fog_color.w`) scales the blend so even a strong
    // fog setting doesn't fully overwrite terrain colour at the far
    // plane — keeps distant features just legible.
    let fog_blend = fog_t * atmos.fog_color.a;
    let fogged = mix(lit, atmos.fog_color.rgb, fog_blend);

    return vec4<f32>(fogged, 1.0);
}
