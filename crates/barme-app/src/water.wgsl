// water.wgsl — Sprint 14 / C9 / ADR-042 — flat water plane MVP.
//
// Renders a single alpha-blended quad at `y = 0` covering the map's
// XZ extent. Surface colour + alpha come from the active preset's
// `WaterBlock` merged with `Project.water_overrides`.
//
// Polish (fresnel / foam / caustics / lava emission) ships with the
// renderer-parity arc downstream. The MVP cut is enough to make the
// feature self-explanatory: the user sees where BAR will flood the
// map and can tint it by preset choice in real time.
//
// Pipeline state (mirrors markers / lines per Sprint-13):
// - Depth TEST: on (terrain occludes water above Y=0 cliffs).
// - Depth WRITE: off (translucent — CPU-side render order owns
//   blend ordering; water draws AFTER terrain and BEFORE markers).
// - Blend: PREMULTIPLIED_ALPHA_BLENDING (matches the existing
//   offscreen target's expectation; CPU pre-multiplies the RGBA
//   payload before writing the uniform).
// - Cull: Back. Quad winding is CW seen from +Y (above).
//
// Vertex math: `@builtin(vertex_index)` 0..=3 generates the four
// corner positions from `extent.xy`. No vertex buffer.

struct WaterU {
    view_proj: mat4x4<f32>,
    /// Premultiplied RGBA — `(r*a, g*a, b*a, a)`. The fragment shader
    /// returns this verbatim and the PREMULTIPLIED_ALPHA_BLENDING
    /// blend state composites it over the terrain.
    surface_rgba: vec4<f32>,
    /// `.x = extent_x` (elmos along world X — width of the map).
    /// `.y = extent_z` (elmos along world Z — depth of the map).
    /// `.z = plane_y` (always 0.0 per `Ground.h::GetWaterPlaneLevel`
    ///   `consteval`; carried explicitly so the renderer-parity arc
    ///   can plumb it if the engine ever lifts the constant).
    /// `.w = alpha_scale` (1.0 when `Tool::Water` active; 0.5 for
    ///   cross-tool ghost — commit 5 wires this).
    extent: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: WaterU;

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    // 4-vertex TriangleStrip quad. Corners (CW from +Y):
    //   vid 0 → (0, 0)
    //   vid 1 → (extent_x, 0)
    //   vid 2 → (0, extent_z)
    //   vid 3 → (extent_x, extent_z)
    // TriangleStrip winds 0-1-2 then 2-1-3; with the corner mapping
    // above the visible top face is CW from above.
    let x = f32(vid & 1u);
    let z = f32((vid >> 1u) & 1u);
    let world = vec3<f32>(
        x * u.extent.x,
        u.extent.z,
        z * u.extent.y,
    );
    var out: VsOut;
    out.clip_pos = u.view_proj * vec4<f32>(world, 1.0);
    return out;
}

@fragment
fn fs_main(_in: VsOut) -> @location(0) vec4<f32> {
    // `surface_rgba` is already premultiplied; `alpha_scale` is a
    // separate scalar so the cross-tool ghost can fade the plane
    // without re-doing the premultiplication on the CPU. Apply the
    // scale to all four channels — premultiplied scaling preserves
    // the invariant (kα·c, kα·c, kα·c, kα).
    return u.surface_rgba * u.extent.w;
}
