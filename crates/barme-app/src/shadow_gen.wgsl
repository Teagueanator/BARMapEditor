// Sprint 30 / R4 / ADR-048 — depth-only shadow-gen vertex stage.
//
// Terrain mesh is heightmap-driven (same as terrain.wgsl::vs_main); the
// shadow pass only needs the world position transformed through the
// shadow camera's view-projection matrix. No fragment stage — the
// pipeline writes depth only (matching the engine's
// `cont/base/springcontent/shaders/GLSL/ShadowGenVertMapProg.glsl`
// + a no-op fragment stage which is equivalent to omitting fragment
// in wgpu).
//
// The vertex stage shares the terrain shader's `Uniforms` layout
// (mat4 view_proj + params + params2) so a single `Uniforms` Rust
// type drives both buffers — the only difference is the matrix and
// the buffer that holds it.
//
// ADR-008 governs world-space conventions: Y-up left-handed, +X east,
// +Z south, 8 elmos per heightmap pixel.

struct Uniforms {
    view_proj: mat4x4<f32>,
    // .x = max world height (elmos), .y = elmos per heightmap pixel.
    // .z / .w unused by shadow gen (carried for `Uniforms` layout
    // parity with terrain.wgsl — same struct, separate buffer).
    params: vec4<f32>,
    // .x = MIN world height (elmos). Allows the heightmap to dip
    // below BAR's water plane at Y = 0; the shadow camera frustum
    // is sized to cover that range too.
    // .y / .z / .w unused by shadow gen.
    params2: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var heightmap: texture_2d<u32>;

// Mirror of `terrain.wgsl::sample_y`. The shadow vertex stage MUST
// produce the same world Y as the main terrain pass — otherwise the
// shadow map disagrees with the receiver's geometry and shadow
// acne / detachment artefacts dominate (PITFALL §1, §2).
fn sample_y(px: i32, pz: i32, dims: vec2<u32>, min_h: f32, max_h: f32) -> f32 {
    let cx = clamp(px, 0, i32(dims.x) - 1);
    let cz = clamp(pz, 0, i32(dims.y) - 1);
    let texel = textureLoad(heightmap, vec2<i32>(cx, cz), 0);
    let t = f32(texel.r) * (1.0 / 65535.0);
    return min_h + t * (max_h - min_h);
}

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> @builtin(position) vec4<f32> {
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
    return u.view_proj * vec4<f32>(world_pos, 1.0);
}
