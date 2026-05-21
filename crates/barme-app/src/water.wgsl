// water.wgsl — Sprint 26 / R3 / ADR-044 (supersedes Sprint 14 MVP).
//
// Renders a single alpha-blended quad at `y = 0` covering the map's
// XZ extent. Surface colour + alpha come from the active preset's
// `WaterBlock` merged with `Project.water_overrides`. Sprint 26
// promotes the MVP "flat tinted plane" into a parity port of
// `cont/base/springcontent/shaders/GLSL/BumpWaterFS.glsl`:
//
// - Refraction — screen-space sample of the pre-water snapshot
//   (`refraction_copy`), perturbed by surface normal × the engine's
//   `reflectionDistortion` (mis-named in BAR but it ALSO drives the
//   refraction UV; FINDINGS §1.5).
// - Reflection — screen-space sample of the half-res reflection RT
//   (terrain mirrored through y=0).
// - (Commit 3) Surface normal — 4-octave Perlin fbm; mirrors the
//   engine's `GetNormal` (4 `normalmap` taps at different UVs +
//   amplitudes).
// - (Commit 4) Fresnel + foam + caustics. Hooks live here; the math
//   gets the per-effect strength dials in subsequent commits.
// - (Commit 5) Lava emission glow.
//
// Pipeline state (mirrors markers / lines per Sprint-13):
// - Depth TEST: on (terrain occludes water above Y=0 cliffs).
// - Depth WRITE: off (translucent; CPU-side render order owns blend
//   ordering — terrain → copy → water → lines → markers).
// - Blend: PREMULTIPLIED_ALPHA_BLENDING.
// - Cull: None — viewing from below the plane (rare during orbit)
//   should still produce a sensible image.
//
// Vertex math: `@builtin(vertex_index)` 0..=3 generates the four
// corner positions from `extent.xy`. No vertex buffer.

struct WaterU {
    view_proj: mat4x4<f32>,
    /// Premultiplied RGBA — `(r*a, g*a, b*a, a)`.
    surface_rgba: vec4<f32>,
    /// `.x = extent_x` (elmos along world X — width of the map).
    /// `.y = extent_z` (elmos along world Z — depth of the map).
    /// `.z = plane_y` (always 0.0 per `Ground.h::GetWaterPlaneLevel`).
    /// `.w = alpha_scale` (1.0 when `Tool::Water` active; 0.5 for
    ///   cross-tool ghost).
    extent: vec4<f32>,
    /// `[refraction_distortion, reflection_distortion, time_s,
    /// reflections_enabled]`.
    polish_a: vec4<f32>,
    /// `[wind_speed_x, wind_speed_z, normal_scale, foam_height]`.
    polish_b: vec4<f32>,
    /// `[fresnel_min, fresnel_max, fresnel_power, lava_emission]`.
    polish_c: vec4<f32>,
    /// `[caustics_resolution, caustics_strength, perlin_start_freq,
    ///   perlin_amplitude]`.
    polish_d: vec4<f32>,
    /// `[screen_w_px, screen_h_px, 1/w, 1/h]`.
    screen: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: WaterU;
@group(0) @binding(1) var refraction_tex: texture_2d<f32>;
@group(0) @binding(2) var refraction_samp: sampler;
@group(0) @binding(3) var reflection_tex: texture_2d<f32>;
@group(0) @binding(4) var reflection_samp: sampler;

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    // 4-vertex TriangleStrip quad. Corners (CW from +Y):
    //   vid 0 → (0, 0)
    //   vid 1 → (extent_x, 0)
    //   vid 2 → (0, extent_z)
    //   vid 3 → (extent_x, extent_z)
    let x = f32(vid & 1u);
    let z = f32((vid >> 1u) & 1u);
    let world = vec3<f32>(
        x * u.extent.x,
        u.extent.z,
        z * u.extent.y,
    );
    var out: VsOut;
    out.clip_pos = u.view_proj * vec4<f32>(world, 1.0);
    out.world_pos = world;
    return out;
}

/// Convert clip-space position (post-perspective-divide) into [0, 1]
/// screen-space UV the refraction + reflection samplers expect. The
/// fragment shader receives `clip_pos` AFTER the divide so we just
/// remap from [-1, 1] NDC to [0, 1] UV. wgpu's clip-space Y axis
/// points up; texture UVs point down → flip Y.
fn clip_to_screen_uv(clip_pos: vec4<f32>) -> vec2<f32> {
    let ndc = clip_pos.xy / max(clip_pos.w, 1e-6);
    return vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // For Sprint 26 / commit 2, `surface_normal` is a fixed up-vector;
    // commit 3 swaps it for a Perlin-driven animated normal. The
    // refraction / reflection sampling structure stays — only the
    // perturbation source changes.
    let surface_normal = vec3<f32>(0.0, 1.0, 0.0);

    // Screen-space UV for the refraction sampler. Perturbed by the
    // surface normal projected onto the screen (engine
    // `BumpWaterFS.glsl:298`).
    let base_uv = clip_to_screen_uv(in.clip_pos);
    let refr_distort = u.polish_a.x;
    let refr_offset = surface_normal.xz * refr_distort * vec2<f32>(0.06, 0.06);
    let refr_uv = clamp(base_uv + refr_offset, vec2<f32>(0.0), vec2<f32>(1.0));
    let refr_color = textureSample(refraction_tex, refraction_samp, refr_uv).rgb;

    // Reflection sampler. The reflection RT was rendered with a
    // mirrored-Y camera (`view_proj_matrix_reflected_y0` in render.rs);
    // sampling it at the same screen UV produces the in-water mirror
    // image. Skip when reflections are disabled — sample stays
    // bound but contribution gates to zero so the shader stays
    // uniform across configurations (no divergent branches).
    let refl_distort = u.polish_a.y;
    let refl_enabled = u.polish_a.w;
    let refl_uv = clamp(
        base_uv + surface_normal.xz * refl_distort * vec2<f32>(0.09, 0.09),
        vec2<f32>(0.0),
        vec2<f32>(1.0),
    );
    let refl_color = textureSample(reflection_tex, reflection_samp, refl_uv).rgb * refl_enabled;

    // Sprint 26 / commit 2 — basic mix: refraction underneath, surface
    // tint on top, reflection added at a fixed weight. Commit 4
    // replaces the mix factor with the Schlick fresnel curve so
    // grazing angles favour reflection and head-on angles favour
    // refraction.
    let surf_premul = u.surface_rgba.rgb;
    let surf_alpha = u.surface_rgba.a;
    let alpha_scale = u.extent.w;

    // Blend: refraction is the "below-water" base; surface tint is
    // composited on top with its alpha; reflection layers above with
    // a fixed 0.25 weight (commit 4's fresnel replaces the constant).
    let below = refr_color * (1.0 - surf_alpha) + surf_premul;
    let final_rgb = below + refl_color * 0.25 * refl_enabled;

    // Premultiplied output. The pipeline's blend state already
    // multiplies by the alpha-write-through; we return RGB
    // premultiplied by `alpha_scale × surf_alpha` so the blend
    // equation reduces to `dst + src - src·dst·α`.
    let out_alpha = surf_alpha * alpha_scale;
    return vec4<f32>(final_rgb * alpha_scale, out_alpha);
}
