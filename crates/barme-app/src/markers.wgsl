// markers.wgsl — Sprint 13 / ADR-037 marker pipeline.
//
// Renders billboarded screen-space markers (filled circles, outline
// rings, filled-with-stroke, upward triangles) at world-space positions.
// Each instance is a 4-vertex quad drawn with TriangleStrip topology;
// the vertex shader expands the centre to a screen-aligned quad sized
// in pixels, and the fragment stage carves the requested shape via SDF
// discard.
//
// Depth state on the pipeline side: `depth_write_enabled = false,
// depth_compare = Less` — markers respect terrain occlusion (terrain
// writes depth) but don't occlude each other via depth (back-to-front
// CPU sort drives translucent blending).

struct MarkerU {
    view_proj: mat4x4<f32>,
    /// Physical pixel dimensions of the offscreen render target so the
    /// vertex shader can convert `radius_px` into clip-space offsets.
    viewport_size: vec2<f32>,
    _pad: vec2<f32>,
};

struct Instance {
    world_pos: vec3<f32>,
    radius_px: f32,
    /// Premultiplied RGBA in `[0, 1]`. The blend state is
    /// `PREMULTIPLIED_ALPHA_BLENDING` (pitfall #6).
    color: vec4<f32>,
    /// 0 = filled circle, 1 = outline ring, 2 = filled-with-stroke,
    /// 3 = upward triangle. Mapped from `ui::markers::MarkerShape`.
    shape_id: u32,
    _pad: vec3<u32>,
};

@group(0) @binding(0) var<uniform> u: MarkerU;
@group(0) @binding(1) var<storage, read> instances: array<Instance>;

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    /// Unit-quad coordinates in `[-1, 1]^2`, used by the fragment SDF.
    @location(0) uv: vec2<f32>,
    /// Forwarded instance index — fragment looks up `colour` / `shape_id`
    /// directly so we don't widen the varyings.
    @location(1) @interpolate(flat) inst_id: u32,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vid: u32,
    @builtin(instance_index) iid: u32,
) -> VsOut {
    let inst = instances[iid];

    // Project the marker centre to clip space.
    let centre_clip = u.view_proj * vec4<f32>(inst.world_pos, 1.0);

    // 4-vertex TriangleStrip quad corners: (-1,-1), (1,-1), (-1,1), (1,1).
    let corner = vec2<f32>(
        f32((vid & 1u) * 2u) - 1.0,
        f32(((vid >> 1u) & 1u) * 2u) - 1.0,
    );

    // Convert the screen-space radius into a clip-space offset.
    // Multiplying by `centre_clip.w` cancels the perspective divide so
    // the quad stays a constant size in pixels regardless of distance.
    let px_to_clip = inst.radius_px * 2.0 / u.viewport_size;
    let offset_clip = corner * px_to_clip * centre_clip.w;

    var out: VsOut;
    out.clip_pos = centre_clip + vec4<f32>(offset_clip, 0.0, 0.0);
    out.uv = corner;
    out.inst_id = iid;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let inst = instances[in.inst_id];
    let d = length(in.uv);

    switch inst.shape_id {
        case 0u: {
            // Filled circle with a 1-px AA band at the edge.
            if (d > 1.0) { discard; }
            let a = 1.0 - smoothstep(0.95, 1.0, d);
            return inst.color * a;
        }
        case 1u: {
            // Outline ring at ~0.85..1.0 with AA on both sides.
            if (d > 1.0) { discard; }
            if (d < 0.85) { discard; }
            let a = (1.0 - smoothstep(0.95, 1.0, d))
                  * smoothstep(0.85, 0.90, d);
            return inst.color * a;
        }
        case 2u: {
            // Filled body with a white outer ring (start-pos source).
            // Inner: inst.color. Outer ring uses the instance alpha so
            // ghost markers fade together with their fill.
            if (d > 1.0) { discard; }
            if (d > 0.85) {
                return vec4<f32>(inst.color.a, inst.color.a, inst.color.a, inst.color.a);
            }
            return inst.color;
        }
        case 3u: {
            // Upward-pointing equilateral triangle inscribed in the
            // unit quad. Apex at uv = (0, 1), base from
            // (-0.866, -0.5) to (0.866, -0.5). Inside iff
            //   uv.y >= -0.5  AND  uv.y + 1.732 * |uv.x| <= 1.0
            if (in.uv.y < -0.5) { discard; }
            if (in.uv.y + 1.732 * abs(in.uv.x) > 1.0) { discard; }
            return inst.color;
        }
        case 4u: {
            // Outline triangle — same outer shape as case 3u, with the
            // inner ~75 %-scale triangle carved out so only the ring
            // remains. Used for geo-vent mirror glyphs (Phase 5).
            if (in.uv.y < -0.5) { discard; }
            if (in.uv.y + 1.732 * abs(in.uv.x) > 1.0) { discard; }
            // Inner inverse test: scale uv by 1/0.75 = 1.333 and check
            // the same inside-triangle constraints. Inside-inner →
            // discard so only the outer ring keeps a fragment.
            let inner = in.uv / 0.75;
            if (inner.y >= -0.5 && inner.y + 1.732 * abs(inner.x) <= 1.0) {
                discard;
            }
            return inst.color;
        }
        default: {
            // Unknown shape — emit nothing rather than render garbage.
            discard;
        }
    }
}
