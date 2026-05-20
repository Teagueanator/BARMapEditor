// Sprint 16 / D9 / ADR-039 — GPU layered composite shader.
//
// Reads N (≤16) layer slot diffuses + per-layer alpha masks +
// per-layer transform / colour / opacity state, alpha-overs them
// back-to-front into an offscreen `rgba8unorm` render target.
//
// Mirrors the CPU bake at `barme_core::layers::bake_diffuse` (D8 /
// Sprint 15) for the same source data — the WGSL and CPU paths
// should agree on every pixel within DXT1 quantisation. The CPU
// path is authoritative for the .sd7 export; the GPU path is the
// live preview the paint viewport and the 3D terrain shader sample.
//
// Per-pixel work: 16 × (1 transform + 1 diffuse sample + 1 mask
// sample + 1 blend) ≈ 64 ALU + 32 texture ops. Well inside the
// per-frame budget at 4096² on an iGPU.

const MAX_LAYERS: u32 = 16u;

// Sprint 16 caps slot diffuses at 1024² — see `render::SLOT_COMPOSITE_DIM`.
// Imported layers fall back to a magenta diagnostic on the CPU side
// (Sprint 17 widens this).
const SLOT_DIM: f32 = 1024.0;
const SLOT_DIM_HALF: f32 = 512.0;
const INV_SLOT_DIM: f32 = 1.0 / 1024.0;

// CPU mirror lives at `render::CompositeLayerU`. Field order MUST
// match exactly — `bytemuck::Pod` enforces no-padding sanity but the
// order is on us.
struct LayerU {
    // .xy = mirror signs (±1, ±1) applied to the centred (px - half_dim)
    // .zw = (cos(theta), sin(theta)) for the rotation that follows the
    //        mirror. The forward order is `mirror → rotate` (pinned by
    //        the CPU `bake_mirror_then_rotate_matches_reference` test).
    rot_mirror: vec4<f32>,
    // .xy = offset_elmos (the layer's translation in the diffuse).
    // .zw = padding.
    offset:     vec4<f32>,
    // .x = 1.0 / scale (CPU pre-inverts to save the shader a divide
    //       per pixel).
    // .y = opacity 0..=1.
    // .z = brightness add in sRGB space.
    // .w = active flag (1.0 = render, 0.0 = skip).
    params:     vec4<f32>,
    // .rgb = tint multiplier, .a = reserved.
    tint:       vec4<f32>,
};

struct CompositeU {
    // .x = composite RT width in pixels,
    // .y = composite RT height in pixels,
    // .z = map extent X in elmos,
    // .w = map extent Z in elmos.
    //
    // For 16-SMU maps the RT clamps to 4096² but extent stays at
    // 8192 elmos; the per-fragment math converts RT pixels to elmos
    // via `(extent / dim)` so a layer's `offset_elmos` lines up with
    // the wallpaper-tiled diffuse regardless of clamp.
    dims:   vec4<f32>,
    layers: array<LayerU, 16>,
};

@group(0) @binding(0) var<uniform> cu: CompositeU;
@group(0) @binding(1) var slot_diffuse: texture_2d_array<f32>;
@group(0) @binding(2) var slot_diffuse_samp: sampler;
@group(0) @binding(3) var mask_array: texture_2d_array<f32>;
@group(0) @binding(4) var mask_samp: sampler;

// Full-screen triangle vs full-screen quad: a single triangle that
// covers the [-1, 1] NDC square works on every wgpu backend and
// avoids the second-triangle edge artifact. The vertex IDs (0, 1, 2)
// map to NDC (-1, -1), (3, -1), (-1, 3). Fragment SV_Position drives
// UV directly.
@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(vid << 1u) & 2);
    let y = f32(i32(vid) & 2);
    return vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    // `frag.xy` is in RT pixel coordinates. Convert to elmos via the
    // (extent / dim) ratio so layer offsets (in elmos) compose
    // correctly even when the RT is clamped below `texture_dims`
    // (16-SMU maps land at the 4096² cap; their world extent is
    // still 8192 elmos). The mask sampler stays in `[0, 1]` so
    // `mask_uv` is `frag / rt_dims`, NOT `frag / extent_elmos`.
    let mask_uv = vec2<f32>(frag.x / cu.dims.x, frag.y / cu.dims.y);
    let elmos_per_px = vec2<f32>(cu.dims.z / cu.dims.x, cu.dims.w / cu.dims.y);
    let world_xy = vec2<f32>(frag.x * elmos_per_px.x, frag.y * elmos_per_px.y);
    let diffuse_centre = vec2<f32>(cu.dims.z * 0.5, cu.dims.w * 0.5);
    let centered = world_xy - diffuse_centre;

    var acc_rgb = vec3<f32>(0.0);
    var acc_a   = 0.0;
    for (var i: u32 = 0u; i < MAX_LAYERS; i = i + 1u) {
        let L = cu.layers[i];
        if (L.params.w < 0.5) {
            continue;
        }

        // mirror → rotate → translate-by-(-offset) → scale → re-centre.
        let m = vec2<f32>(centered.x * L.rot_mirror.x, centered.y * L.rot_mirror.y);
        let c_t = L.rot_mirror.z;
        let s_t = L.rot_mirror.w;
        let r = vec2<f32>(c_t * m.x - s_t * m.y, s_t * m.x + c_t * m.y);
        let inv_scale = L.params.x;
        // (r - offset) / scale + slot_dim_half puts the centre of the
        // diffuse at (slot_dim_half, slot_dim_half). The `Repeat`
        // sampler wraps past the seam — DON'T switch to ClampToEdge or
        // scaled-down layers stretch instead of tile.
        let uv_px = (r - L.offset.xy) * inv_scale + vec2<f32>(SLOT_DIM_HALF);
        let uv = uv_px * INV_SLOT_DIM;
        let diff = textureSample(slot_diffuse, slot_diffuse_samp, uv, i32(i));

        // Mask sample at the matched RT pixel.
        let m_alpha = textureSample(mask_array, mask_samp, mask_uv, i32(i)).r;
        let alpha = m_alpha * L.params.y;

        // Tint + brightness in sRGB. Multiplicative tint, additive
        // brightness, both before the mask multiply — matches the
        // CPU bake exactly.
        let rgb_pre = clamp(
            diff.rgb * L.tint.rgb + vec3<f32>(L.params.z),
            vec3<f32>(0.0),
            vec3<f32>(1.0),
        );
        let rgb = rgb_pre * alpha;

        // Alpha-over: dst = src + dst * (1 - alpha).
        let inv = 1.0 - alpha;
        acc_rgb = rgb + acc_rgb * inv;
        acc_a   = alpha + acc_a * inv;
    }
    // Flatten against an opaque mid-grey background so under-painted
    // pixels read as grey rather than pure black. Matches the CPU
    // bake's `bg = 0.18`.
    let inv_a = 1.0 - acc_a;
    let out_rgb = clamp(acc_rgb + vec3<f32>(0.18) * inv_a, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(out_rgb, 1.0);
}
