// grass.wgsl — Sprint 34 / R6 / ADR-050.
//
// Instanced billboard grass. Each instance is one blade: a 4-vertex
// TriangleStrip unit quad, anchored at the blade's world position, its
// base on the terrain and its top swaying in the wind. The quad
// billboards toward the camera (per-blade orientation jitter keeps the
// field from looking identical from every angle — pitfall #2).
//
// Pipeline state (mirrors water / markers per Sprint-13/26):
// - Depth TEST:  on  — terrain bumps occlude blades behind them.
// - Depth WRITE: off — translucent edges; CPU render order owns
//                      blending (terrain -> grass -> water -> markers).
// - Blend:       ALPHA_BLENDING.
// - Cull:        None — a blade is a flat quad seen from both sides.
//
// Wind sync (pitfall #3): the sway reads the SAME `atmosphere.wind`
// block the water shader animates against, and the SAME `time_s` the
// App feeds water, so grass and water move together.
//
// Shadows (pitfall #6): blades RECEIVE shadows from the Sprint-30
// depth-only shadow pass (same `sample_shadow` math as terrain.wgsl).
// They do NOT cast — too small to matter and the shadow pass only
// renders the terrain mesh. Documented in ADR-050.

// CPU mirror: render::GrassUniforms. Field order MUST match exactly.
struct GrassU {
    view_proj:      mat4x4<f32>,
    // .xyz = world-space camera eye (billboard + LOD distance). .w _.
    camera_pos:     vec4<f32>,
    // .rgb = mapinfo grass.bladeColor. .w _.
    blade_color:    vec4<f32>,
    // (blade_width, blade_height, blade_wave_scale, time_s).
    blade_params:   vec4<f32>,
    // (max_distance, fade_start, _, _) — LOD fade band (elmos).
    lod:            vec4<f32>,
    // .xyz = world-space to-sun direction (normalized). .w _.
    sun_dir:        vec4<f32>,
    // .rgb = lighting.groundAmbient (SMF_INTENSITY_MULT pre-applied).
    ground_ambient: vec4<f32>,
    // .rgb = lighting.groundDiffuse.
    ground_diffuse: vec4<f32>,
};

// Shared with terrain/water — same CPU `AtmosphereUniforms` mirror.
struct AtmosphereU {
    sun_color:     vec4<f32>,
    sky_color:     vec4<f32>,
    fog_color:     vec4<f32>,
    fog_start_end: vec4<f32>,
    cloud_color:   vec4<f32>,
    // (wind_x, wind_z, wind_speed, _) — App-side deterministic ramp.
    wind:          vec4<f32>,
    sky_axis_angle: vec4<f32>,
    sun_dir:       vec4<f32>,
    flags:         vec4<u32>,
};

// Shared with terrain — same CPU `ShadowUniforms` mirror.
struct ShadowU {
    view_proj: mat4x4<f32>,
    params:    vec4<f32>, // (bias, ground_density, unit_density, enabled)
};

@group(0) @binding(0) var<uniform> g: GrassU;
@group(0) @binding(1) var<uniform> atmos: AtmosphereU;
@group(0) @binding(2) var shadow_map: texture_depth_2d;
@group(0) @binding(3) var shadow_sampler: sampler_comparison;
@group(0) @binding(4) var<uniform> shadow: ShadowU;

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) blade_uv: vec2<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) color: vec3<f32>,
};

// 3x3 PCF shadow sample — identical math to terrain.wgsl::sample_shadow
// (kept in sync by the ShadowU layout; WGSL has no cross-file import).
fn sample_shadow(world_pos: vec3<f32>) -> f32 {
    if (shadow.params.w < 0.5) {
        return 1.0;
    }
    let clip = shadow.view_proj * vec4<f32>(world_pos, 1.0);
    let ndc = clip.xyz / clip.w;
    if (ndc.x < -1.0 || ndc.x > 1.0
     || ndc.y < -1.0 || ndc.y > 1.0
     || ndc.z < 0.0 || ndc.z > 1.0) {
        return 1.0;
    }
    let uv_center = vec2<f32>(ndc.x * 0.5 + 0.5, ndc.y * -0.5 + 0.5);
    let ref_depth = ndc.z - shadow.params.x;
    let dims = textureDimensions(shadow_map);
    let texel = vec2<f32>(1.0 / f32(dims.x), 1.0 / f32(dims.y));
    var total: f32 = 0.0;
    for (var dx: i32 = -1; dx <= 1; dx = dx + 1) {
        for (var dy: i32 = -1; dy <= 1; dy = dy + 1) {
            let offset = vec2<f32>(f32(dx), f32(dy)) * texel;
            total = total
                + textureSampleCompareLevel(shadow_map, shadow_sampler, uv_center + offset, ref_depth);
        }
    }
    return mix(1.0, total / 9.0, shadow.params.y);
}

@vertex
fn vs_main(
    @builtin(vertex_index) vid: u32,
    @location(0) inst_pos: vec3<f32>,
    @location(1) inst_orient: f32,
    @location(2) inst_color: vec3<f32>,
    @location(3) inst_height: f32,
) -> VsOut {
    // 4-vertex TriangleStrip unit quad: (0,0)(1,0)(0,1)(1,1).
    let quad_uv = vec2<f32>(f32(vid & 1u), f32((vid >> 1u) & 1u));

    let blade_width = g.blade_params.x;
    let blade_height = g.blade_params.y * inst_height;
    let local_x = (quad_uv.x - 0.5) * blade_width;
    let local_y = quad_uv.y * blade_height;

    // Billboard "right" axis = perpendicular to the view direction in
    // the XZ plane, then rotated by the per-blade orientation jitter so
    // a head-on field still looks varied.
    let view_dir = normalize(g.camera_pos.xyz - inst_pos);
    var right = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), view_dir));
    let c = cos(inst_orient);
    let s = sin(inst_orient);
    right = normalize(vec3<f32>(
        right.x * c - right.z * s,
        0.0,
        right.x * s + right.z * c,
    ));

    // Wind sway — leans the blade tip downwind. Shares the atmosphere
    // wind vector + the App's `time_s` with the water shader so the two
    // animate in lock-step (pitfall #3). Only the tip moves: scaled by
    // quad_uv.y so the base stays planted.
    let wind_dir = normalize(atmos.wind.xy + vec2<f32>(0.0001, 0.0));
    let phase = g.blade_params.w * 1.5 + (inst_pos.x + inst_pos.z) * 0.05;
    let sway = sin(phase) * atmos.wind.z * g.blade_params.z * 0.02;
    let sway_off = vec3<f32>(wind_dir.x, 0.0, wind_dir.y) * sway * quad_uv.y;

    let world_pos = inst_pos
        + right * local_x
        + vec3<f32>(0.0, local_y, 0.0)
        + sway_off;

    var out: VsOut;
    out.clip_pos = g.view_proj * vec4<f32>(world_pos, 1.0);
    out.blade_uv = quad_uv;
    out.world_pos = world_pos;
    out.color = g.blade_color.rgb * inst_color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Taper the quad's vertical edges into a leaf silhouette (pitfall
    // #5 — keep the smoothstep wide enough that ATI/Vega doesn't band).
    let edge = smoothstep(0.0, 0.12, in.blade_uv.x)
             * smoothstep(1.0, 0.88, in.blade_uv.x);

    // LOD: fade alpha across (fade_start .. max_distance) so blades
    // dissolve smoothly at the radius instead of popping (pitfall #9).
    let dist = distance(g.camera_pos.xyz, in.world_pos);
    let lod_fade = 1.0 - smoothstep(g.lod.y, g.lod.x, dist);

    let alpha = edge * lod_fade;
    if (alpha < 0.01) {
        discard;
    }

    // Cheap Lambert: grass is lit mostly from above + the sun. A flat
    // +Y normal avoids per-blade normals while still responding to the
    // sun angle and the project's ground lighting colours.
    let ndl = max(dot(vec3<f32>(0.0, 1.0, 0.0), normalize(g.sun_dir.xyz)), 0.0);
    let lit = g.ground_ambient.rgb + g.ground_diffuse.rgb * ndl;
    let shadow_f = sample_shadow(in.world_pos);

    let rgb = in.color * lit * shadow_f;
    return vec4<f32>(rgb, alpha);
}
