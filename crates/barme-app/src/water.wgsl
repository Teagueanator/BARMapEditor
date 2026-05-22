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
    /// `[eye_x, eye_y, eye_z, _]` — world-space camera position.
    eye: vec4<f32>,
};

// Sprint 28 / R2 / ADR-040 — atmosphere block (shared with the
// terrain pipeline). Same struct shape as `terrain.wgsl::AtmosphereU`;
// the CPU `AtmosphereUniforms` mirror is the source of truth. We
// re-declare here because WGSL doesn't have cross-file `import`.
struct AtmosphereU {
    sun_color:     vec4<f32>,
    sky_color:     vec4<f32>,
    fog_color:     vec4<f32>,
    fog_start_end: vec4<f32>,
    cloud_color:   vec4<f32>,
    wind:          vec4<f32>,
    sky_axis_angle: vec4<f32>,
    sun_dir:       vec4<f32>,
    flags:         vec4<u32>,
};

@group(0) @binding(0) var<uniform> u: WaterU;
@group(0) @binding(1) var refraction_tex: texture_2d<f32>;
@group(0) @binding(2) var refraction_samp: sampler;
@group(0) @binding(3) var reflection_tex: texture_2d<f32>;
@group(0) @binding(4) var reflection_samp: sampler;
// Sprint 28 / R2 / ADR-040 — atmosphere block bound from the same
// uniform buffer the terrain shader reads. The water shader uses
// `sun_dir` for the lava-emission daylight ramp (commits 3 of this
// sprint) and `wind` for surface motion (commit 5).
@group(0) @binding(5) var<uniform> atmos: AtmosphereU;

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

// ─── Sprint 26 / R3 / ADR-044 — procedural surface normal ─────
//
// BAR's `BumpWaterFS.glsl::GetNormal` samples `normalmap` four times
// at progressively offset UVs and combines them with amplitudes
// `1, a, a², a³, a⁴` (a = `PerlinAmp`). That's morally fbm against a
// noise texture; we replace it with WGSL gradient noise so we avoid
// the GPL-2.0 asset vendoring discussion (the engine's waterbump.png
// is GPL inherited from the springcontent base; we'd inherit the
// licence into the editor's binary distribution). The visual contract
// is identical — a tilable animated normal map driven by the same
// `perlinStartFreq` / `Lacunarity` / `Amplitude` parameters.

/// Cheap deterministic 2D hash. Source: Quilez's "Inigo's hash" at
/// `https://www.shadertoy.com/view/4djSRW` — chosen for tight WGSL
/// portability (no out-of-spec ints, no bitwise tricks that vary by
/// backend).
fn hash22(p: vec2<f32>) -> vec2<f32> {
    let q = vec2<f32>(
        dot(p, vec2<f32>(127.1, 311.7)),
        dot(p, vec2<f32>(269.5, 183.3)),
    );
    return fract(sin(q) * 43758.5453) * 2.0 - 1.0;
}

/// 2D gradient (Perlin-style) noise. Returns `-1..1`. Standard
/// quintic-smoothed bilerp of the four corner gradients.
fn gradient_noise(p: vec2<f32>) -> f32 {
    let pi = floor(p);
    let pf = fract(p);
    // Quintic falloff `6t⁵ − 15t⁴ + 10t³`. Smoother first + second
    // derivatives than the simpler `3t² − 2t³` cubic — important for
    // taking the normal as a finite difference of noise samples.
    let w = pf * pf * pf * (pf * (pf * 6.0 - 15.0) + 10.0);
    let g00 = hash22(pi + vec2<f32>(0.0, 0.0));
    let g10 = hash22(pi + vec2<f32>(1.0, 0.0));
    let g01 = hash22(pi + vec2<f32>(0.0, 1.0));
    let g11 = hash22(pi + vec2<f32>(1.0, 1.0));
    let d00 = dot(g00, pf - vec2<f32>(0.0, 0.0));
    let d10 = dot(g10, pf - vec2<f32>(1.0, 0.0));
    let d01 = dot(g01, pf - vec2<f32>(0.0, 1.0));
    let d11 = dot(g11, pf - vec2<f32>(1.0, 1.0));
    let x0 = mix(d00, d10, w.x);
    let x1 = mix(d01, d11, w.x);
    return mix(x0, x1, w.y);
}

/// 4-octave fbm — mirrors the engine shader's four normalmap taps with
/// progressive lacunarity. `amp_decay` halves the contribution per
/// octave (engine uses `a, a², a³, a⁴` with `a = polish_d.w`; we follow
/// the same geometric falloff).
fn fbm4(p: vec2<f32>, start_freq: f32, amp: f32) -> f32 {
    // Engine `PerlinLacunarity` default is 3.0 (FINDINGS §1.5).
    let l = 3.0;
    var v = 0.0;
    var f = start_freq;
    var a = amp;
    v += gradient_noise(p * f) * a;
    f = f * l;
    a = a * amp;
    v += gradient_noise(p * f) * a;
    f = f * l;
    a = a * amp;
    v += gradient_noise(p * f) * a;
    f = f * l;
    a = a * amp;
    v += gradient_noise(p * f) * a;
    return v;
}

/// Reconstruct a world-space surface normal from the fbm field via a
/// 4-tap finite difference. World XZ → noise field → height; partial
/// derivatives give the tangent plane, cross-product the normal.
/// `time_s` shifts the field over time to give the surface its
/// animated chop. `wind` xy drives the per-axis flow direction.
fn fbm_surface_normal(
    world_xz: vec2<f32>,
    time_s: f32,
    wind: vec2<f32>,
    normal_scale: f32,
    start_freq: f32,
    amp: f32,
) -> vec3<f32> {
    // Convert world-space XZ to a noise-space coordinate. `normal_scale`
    // controls one fbm "tile" per `1/normal_scale` elmos.
    let base = world_xz * normal_scale + wind * time_s;
    // 4-tap finite difference; the offset matches the engine's
    // `WaveFoamDistortion` magnitude class. Larger eps → smoother
    // normals (loses detail); smaller → noisier (precision-limited).
    let eps = 0.5;
    let n_c = fbm4(base, start_freq, amp);
    let n_x = fbm4(base + vec2<f32>(eps, 0.0), start_freq, amp);
    let n_z = fbm4(base + vec2<f32>(0.0, eps), start_freq, amp);
    let dx = (n_x - n_c) / eps;
    let dz = (n_z - n_c) / eps;
    // Normal from height field: `(-dx, 1, -dz)` then normalize. Y
    // dominates so the normal stays close to vertical (the surface
    // isn't a wall; it's a gently rippling plane).
    return normalize(vec3<f32>(-dx, 1.0, -dz));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Sprint 26 / R3 / commit 3 — fbm-driven surface normal. Replaces
    // the commit-2 placeholder `vec3(0, 1, 0)`. The refraction +
    // reflection UV perturbation, fresnel angle (commit 4), and
    // shorewave foam tap (commit 4) all read from this single normal,
    // so the visual coherence holds across the per-effect work.
    let wind = vec2<f32>(u.polish_b.x, u.polish_b.y);
    let normal_scale = u.polish_b.z;
    let perlin_start = u.polish_d.z;
    let perlin_amp = u.polish_d.w;
    let surface_normal = fbm_surface_normal(
        in.world_pos.xz,
        u.polish_a.z,
        wind,
        normal_scale,
        perlin_start,
        perlin_amp,
    );

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

    // ─── FRESNEL (Schlick's approximation) ─────────────────────────
    //
    // Engine `BumpWaterFS.glsl:246`:
    //   fresnel = FresnelMin + FresnelMax × pow(angle, FresnelPower)
    // where `angle = 1 - abs(dot(-view, normal))` — grazing angles
    // approach 1, head-on approach 0. We clamp `dot` to `[0, 1]` to
    // dodge the PITFALL-#6 NaN at backface (dot < 0 → pow blows up).
    let view_dir = normalize(u.eye.xyz - in.world_pos);
    let cos_view = clamp(dot(view_dir, surface_normal), 0.0, 1.0);
    let angle = 1.0 - cos_view;
    let fresnel_min = u.polish_c.x;
    let fresnel_max = u.polish_c.y;
    let fresnel_power = u.polish_c.z;
    let fresnel = clamp(fresnel_min + fresnel_max * pow(angle, fresnel_power), 0.0, 1.0);

    // ─── FOAM (refraction-brightness proxy) ────────────────────────
    //
    // The engine's foam reads a precomputed coastmap (BumpWaterCoast
    // BlurFS produces a per-pixel "distance to shore" texture). We
    // don't ship that bake yet — Sprint 27 candidate — so we proxy
    // shoreline proximity through the brightness of the refraction
    // sample. Where the refraction is bright, the terrain is shallow
    // and probably near the shoreline → apply foam. Where dark, deep
    // water → no foam. Imperfect but visually plausible at editor
    // camera distances. `polish_b.w = foam_height` (elmos) is the
    // smoothstep band width.
    let foam_height = u.polish_b.w;
    let refr_luma = dot(refr_color, vec3<f32>(0.299, 0.587, 0.114));
    let foam_factor = smoothstep(0.45, 0.85, refr_luma)
        * smoothstep(0.0, foam_height / 100.0, refr_luma - 0.5);

    // ─── CAUSTICS (procedural sine pattern) ───────────────────────
    //
    // Engine samples 32 caustic JPEGs in a loop; we synthesise the
    // same visual via two layered sine fields warped by the surface
    // normal. Animated by `polish_a.z = time_s`. Modulated by the
    // refraction-brightness proxy: caustics only shimmer in shallow
    // water (matches BumpWaterFS.glsl:325 `if (waterdepth > 0)`).
    let caust_res = u.polish_d.x;
    let caust_str = u.polish_d.y;
    let caust_uv = in.world_pos.xz / max(caust_res, 1.0)
        + surface_normal.xz * 0.5
        + vec2<f32>(u.polish_a.z * 0.05, u.polish_a.z * 0.03);
    let c0 = sin(caust_uv.x * 6.2832 + u.polish_a.z * 0.5);
    let c1 = sin(caust_uv.y * 6.2832 + u.polish_a.z * 0.4);
    let caust = max(0.0, c0 * c1) * caust_str * smoothstep(0.0, 1.0, refr_luma);

    // ─── COMPOSITE (BumpWaterFS-style) ─────────────────────────────
    //
    // Engine's GL_FragColor.rgb is:
    //   refr_color   (refraction underneath)
    //   × `mix(refr, water_surface, surfaceMix + 0.1)`
    //   + caustics  (where shallow)
    //   + shorewaves
    //   then mixed with reflection via `mix(rgb, reflColor, fresnel)`.
    // Our condensed translation: refr_color is the underwater base;
    // surface tint composites on top with `surf_alpha`; foam adds
    // brightness near the shoreline; caustics shimmer below; the
    // reflection-vs-refraction mix uses `fresnel`. Reflection
    // contribution gates on `polish_a.w` so disabled reflections
    // produce a clean refractive-only image.
    let surf_premul = u.surface_rgba.rgb;
    let surf_alpha = u.surface_rgba.a;
    let alpha_scale = u.extent.w;

    let below = refr_color * (1.0 - surf_alpha)
        + surf_premul
        + vec3<f32>(caust)
        + vec3<f32>(foam_factor);
    let composite_with_reflection = mix(below, refl_color, fresnel * refl_enabled);

    // ─── LAVA EMISSION (Sprint 26 / R3 commit 5) ───────────────────
    //
    // When the preset is Lava or Magma, the surface self-illuminates
    // with `surface_color × emission_strength × (1 + caust · 0.5)`.
    // The caustic modulation gives the lava its characteristic
    // pulsating glow. Sprint 28 (atmosphere) will replace the
    // hardcoded `0.5` daylight factor with `dot(sun_dir, world_up)`
    // so the lava ramps to max brightness at night and dims under
    // direct sun. Sprint 30 (shadows) inhibits this branch under
    // cast shadows.
    //
    // `polish_c.w` gates the branch — 1.0 for Lava/Magma, 0.0
    // otherwise. Keeping it as a multiply (not a divergent `if`)
    // means the shader stays uniform across preset switches — no
    // recompile cost when the user clicks between Ocean and Lava.
    let lava_emission_flag = u.polish_c.w;
    // Sprint 28 / R2 / ADR-040 — real daylight ramp from atmosphere
    // sun direction. `clamp(dot(sun_dir, +Y), 0, 1)` ranges 0 at
    // horizon (sunset) to 1 at zenith (noon). The remapped curve
    // `(1.0 - clamp_dot)^0.7` brightens lava at night (low sun)
    // and dims under direct sun — matches BAR's `gadget_lava.lua`
    // behaviour qualitatively without the gameplay coupling. The
    // exponent < 1 keeps lava noticeable even at twilight rather
    // than collapsing to "only visible at midnight".
    let sun_world_up = clamp(atmos.sun_dir.y, 0.0, 1.0);
    let daylight = pow(1.0 - sun_world_up, 0.7);
    let emission_strength = 0.8;
    // Take the un-premultiplied surface tint as the emission colour
    // (`surf_premul / surf_alpha`) so emission strength is independent
    // of the user's `surface_alpha` slider. Guard against div-by-zero.
    let emission_color = surf_premul / max(surf_alpha, 1e-3);
    let lava_glow = emission_color
        * (1.0 + caust * 0.5)
        * emission_strength
        * daylight
        * lava_emission_flag;

    let final_rgb = composite_with_reflection + lava_glow;

    // Pre-multiplied output. The pipeline's blend state multiplies
    // by alpha; we return RGB pre-multiplied by `alpha_scale × surf_alpha`
    // so the blend equation reduces to `dst + src - src·dst·α`.
    let out_alpha = surf_alpha * alpha_scale;
    return vec4<f32>(final_rgb * alpha_scale, out_alpha);
}
