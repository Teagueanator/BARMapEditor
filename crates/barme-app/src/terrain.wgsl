// Stage 1 terrain shader (ADR-017). Vertex stage samples the heightmap as
// a storage texture (r16uint) so brush edits become texture writes, not
// full-mesh re-uploads. No lighting yet — height-based gradient for
// legibility. ADR-008 governs world-space conventions: Y-up left-handed,
// +X east, +Z south, 8 elmos per heightmap pixel.

struct Uniforms {
    view_proj: mat4x4<f32>,
    // .x = max world height (elmos) for the gradient lerp + Y displacement.
    // .y = elmos per heightmap pixel (= 8.0 per ADR-008).
    // .zw unused.
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var heightmap: texture_2d<u32>;

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_y: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    let dims = textureDimensions(heightmap);
    let px = vid % dims.x;
    let pz = vid / dims.x;

    let texel = textureLoad(heightmap, vec2<i32>(i32(px), i32(pz)), 0);
    let h_norm = f32(texel.r) * (1.0 / 65535.0);

    let elmos_per_px = u.params.y;
    let max_h = u.params.x;
    let world_pos = vec3<f32>(
        f32(px) * elmos_per_px,
        h_norm * max_h,
        f32(pz) * elmos_per_px,
    );

    var out: VsOut;
    out.clip_pos = u.view_proj * vec4<f32>(world_pos, 1.0);
    out.world_y = world_pos.y;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let max_h = u.params.x;
    let t = clamp(in.world_y / max(max_h, 1.0), 0.0, 1.0);
    let low  = vec3<f32>(0.32, 0.24, 0.16);
    let high = vec3<f32>(0.95, 0.95, 0.92);
    let color = mix(low, high, t);
    return vec4<f32>(color, 1.0);
}
