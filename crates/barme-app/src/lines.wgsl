// lines.wgsl — Sprint 13 / ADR-037 line pipeline.
//
// Renders world-space line segments (symmetry axes, geo-vent plumes)
// through a `wgpu::PrimitiveTopology::LineList` pipeline. Each segment
// is one `LineVertex` pair in the pre-allocated vertex buffer.
//
// Depth state: `depth_write_enabled = false, depth_compare = Less` —
// lines respect terrain occlusion (a hill in the foreground hides the
// axis crossing behind it) without writing depth themselves. Blend
// state: `PREMULTIPLIED_ALPHA_BLENDING` (pitfall #6).
//
// Reads `view_proj` from a uniform buffer SHARED with the marker
// pipeline (`MarkerU`). The shader only consumes the leading 64 bytes
// (the matrix); the trailing `viewport_size + _pad` are ignored.

struct LineU {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> u: LineU;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip_pos = u.view_proj * vec4<f32>(in.pos, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
