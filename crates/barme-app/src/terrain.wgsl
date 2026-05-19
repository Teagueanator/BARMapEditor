// Stage 1 terrain shader (ADR-017 vertex; ADR-036 fragment).
//
// Vertex stage samples the heightmap as a storage texture (r16uint) so
// brush edits become texture writes, not full-mesh re-uploads. A 4-tap
// finite difference of neighbouring heights yields a per-vertex normal
// in world space — interpolated into the fragment stage as the "base
// normal" (the engine uses a baked SMT normal texture sampled per
// fragment; the editor has no SMT bake at preview time).
//
// Fragment stage composites four diffuse layers from a `texture_2d_array`
// by the per-channel `splatCofac = textureSample(splatDistr, uv) *
// texMults` weights — directly translated from
// `RecoilEngine/cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl`
// lines 174-198 (the SMF_DETAIL_NORMAL_TEXTURE_SPLATTING branch) with
// the diffuse-only simplification documented in ADR-036. When the
// cofactor sum is zero (no slots painted, or all weights ramp to 0),
// the fragment falls back to a heightmap-driven biome gradient so
// unpainted maps still look like terrain.
//
// Per FINDINGS §7 (source-audit 2026-05-18), the constant is
// SMF_INTENSITY_MULT (with T); we pre-apply it CPU-side on the
// ambient colour to keep the shader clean.

// ADR-008 governs world-space conventions: Y-up left-handed,
// +X east, +Z south, 8 elmos per heightmap pixel.

struct Uniforms {
    view_proj: mat4x4<f32>,
    // .x = max world height (elmos) for the gradient + Y displacement.
    // .y = elmos per heightmap pixel (= 8.0 per ADR-008).
    // .z = world extent X (elmos) — for splat-distribution UV math.
    // .w = world extent Z (elmos) — same.
    params: vec4<f32>,
};

// Per-channel splat tuning + lighting state (ADR-036).
//
// `tex_scales` maps directly to mapinfo `splats.texScales` — each
// channel scales `worldPos.xz` to produce the per-layer detail UV.
// `tex_mults` maps to `splats.texMults` — per-channel weight multiplier
// applied to the splat distribution sample.
//
// `flags.x = active_slot_mask` (bit i set = layer i is bound to a
// real slot). Unbound layers contribute zero so a fresh project with
// no slots assigned still renders the fallback gradient.
//
// `flags.y = diffuse_in_alpha` (0 / 1) — ADR-034 placeholder. Plumbed
// but UNUSED in this sprint: the high-pass diffuse-offset workflow
// from `splatDetailNormalDiffuseAlpha = 1` requires the DNTS-encoded
// alpha bake which D5 deliberately keeps off (ADR-025 baseline).
//
// `sun_dir` is the world-space *to-sun* direction (normalized).
// `ground_ambient` already has `SMF_INTENSITY_MULT = 210/255`
// pre-multiplied CPU-side (FINDINGS §7.1).
struct SplatU {
    tex_scales: vec4<f32>,
    tex_mults:  vec4<f32>,
    flags:      vec4<u32>,
    sun_dir:    vec4<f32>,
    ground_ambient: vec4<f32>,
    ground_diffuse: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var heightmap: texture_2d<u32>;
@group(0) @binding(2) var<uniform> sp: SplatU;
@group(0) @binding(3) var splat_distr: texture_2d<f32>;
@group(0) @binding(4) var splat_distr_samp: sampler;
@group(0) @binding(5) var slot_diffuse: texture_2d_array<f32>;
@group(0) @binding(6) var slot_diffuse_samp: sampler;

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv_norm: vec2<f32>, // map-normalized UV for splat distribution
};

// Sample heightmap at integer pixel (px, pz), clamped to bounds, and
// return the world-space Y in elmos.
fn sample_y(px: i32, pz: i32, dims: vec2<u32>, max_h: f32) -> f32 {
    let cx = clamp(px, 0, i32(dims.x) - 1);
    let cz = clamp(pz, 0, i32(dims.y) - 1);
    let texel = textureLoad(heightmap, vec2<i32>(cx, cz), 0);
    return f32(texel.r) * (1.0 / 65535.0) * max_h;
}

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    let dims = textureDimensions(heightmap);
    let px = i32(vid % dims.x);
    let pz = i32(vid / dims.x);

    let elmos_per_px = u.params.y;
    let max_h = u.params.x;
    let y = sample_y(px, pz, dims, max_h);
    let world_pos = vec3<f32>(
        f32(px) * elmos_per_px,
        y,
        f32(pz) * elmos_per_px,
    );

    // 4-tap finite difference for the per-vertex normal. Engine path
    // samples a baked SMT normal texture per-fragment; we approximate
    // by hand because the editor doesn't run the normal bake at
    // preview time.
    let h_l = sample_y(px - 1, pz, dims, max_h);
    let h_r = sample_y(px + 1, pz, dims, max_h);
    let h_u = sample_y(px, pz - 1, dims, max_h);
    let h_d = sample_y(px, pz + 1, dims, max_h);
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

// Heightmap → biome gradient. Mirrors `crates/barme-app/src/ui/minimap.rs`
// `biome_ramp` so the central viewport and the mini-map agree on the
// fallback colour family.
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

    // splatCofac = per-pixel distribution × per-channel weight scale
    // (FINDINGS §7.3). The active_slot_mask zeroes layers the user
    // hasn't bound so a fresh project doesn't sample a stale layer.
    let dist = textureSample(splat_distr, splat_distr_samp, in.uv_norm);
    let mask = sp.flags.x;
    let active = vec4<f32>(
        f32((mask >> 0u) & 1u),
        f32((mask >> 1u) & 1u),
        f32((mask >> 2u) & 1u),
        f32((mask >> 3u) & 1u),
    );
    let splat_cofac = dist * sp.tex_mults * active;
    // Saturated sum used as the diffuse-blend strength (mirrors the
    // engine's `splatDetailStrength.x = min(1.0, dot(splatCofac,
    // vec4(1.0)))` from SMFFragProg.glsl:180).
    let detail_strength = min(1.0, dot(splat_cofac, vec4<f32>(1.0)));

    // Per-channel diffuse sample. Each layer uses an independent UV
    // stream `worldPos.xz * tex_scales[i]` — straight from
    // SMFFragProg.glsl:175-176. The texture array binds all four
    // slots so reassigning a slot is a single `queue.write_texture`
    // into the affected layer (ADR-036).
    let uv0 = in.world_pos.xz * sp.tex_scales.x;
    let uv1 = in.world_pos.xz * sp.tex_scales.y;
    let uv2 = in.world_pos.xz * sp.tex_scales.z;
    let uv3 = in.world_pos.xz * sp.tex_scales.w;
    let d0 = textureSample(slot_diffuse, slot_diffuse_samp, uv0, 0).rgb;
    let d1 = textureSample(slot_diffuse, slot_diffuse_samp, uv1, 1).rgb;
    let d2 = textureSample(slot_diffuse, slot_diffuse_samp, uv2, 2).rgb;
    let d3 = textureSample(slot_diffuse, slot_diffuse_samp, uv3, 3).rgb;
    let splat_rgb =
          d0 * splat_cofac.x
        + d1 * splat_cofac.y
        + d2 * splat_cofac.z
        + d3 * splat_cofac.w;

    // Fallback gradient — keep unpainted regions readable rather than
    // black-on-painted. `detail_strength` mixes between gradient and
    // splat composite. When fully painted (strength = 1), the gradient
    // disappears.
    let t = clamp(in.world_pos.y / max(max_h, 1.0), 0.0, 1.0);
    let fallback = biome_ramp(t);
    let base_rgb = mix(fallback, splat_rgb, detail_strength);

    // Lambert + ambient lighting (FINDINGS §7 simplified path — no
    // shadows, no specular). The ambient was pre-multiplied by
    // SMF_INTENSITY_MULT CPU-side so the shader stays clean.
    let n = normalize(in.world_normal);
    let l = normalize(sp.sun_dir.xyz);
    let n_dot_l = clamp(dot(n, l), 0.0, 1.0);
    let lit = base_rgb * (sp.ground_ambient.rgb + sp.ground_diffuse.rgb * n_dot_l);

    return vec4<f32>(lit, 1.0);
}
