// Stage 0 terrain shader. No lighting, just a height-based gradient so the
// mesh is legible. ADR-008 governs the world-space conventions.

struct Uniforms {
    view_proj: mat4x4<f32>,
    // .w = max world height (elmos) for the gradient lerp.
    height_extent: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_y: f32,
};

@vertex
fn vs_main(@location(0) pos: vec3<f32>) -> VsOut {
    var out: VsOut;
    out.clip_pos = u.view_proj * vec4<f32>(pos, 1.0);
    out.world_y = pos.y;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let t = clamp(in.world_y / u.height_extent.w, 0.0, 1.0);
    // Low ground = soil brown, high ground = snow white.
    let low  = vec3<f32>(0.32, 0.24, 0.16);
    let high = vec3<f32>(0.95, 0.95, 0.92);
    let color = mix(low, high, t);
    return vec4<f32>(color, 1.0);
}
