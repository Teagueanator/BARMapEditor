//! Stage 1 terrain renderer (ADR-017 vertex; ADR-036 fragment).
//!
//! Heightmap lives on the GPU as an `r16uint` texture; the vertex shader
//! samples it for Y displacement so brush edits are texture writes, not
//! full-mesh rebuilds. A 4-tap finite difference of neighbouring heights
//! produces a per-vertex world-space normal used by the fragment stage
//! for Lambert lighting.
//!
//! The fragment stage composites four diffuse layers from a
//! `texture_2d_array` by the `splatCofac` weight (per FINDINGS §7.3),
//! falling back to a heightmap-driven biome gradient when no slot is
//! painted — see `crates/barme-app/src/terrain.wgsl` for the math and
//! ADR-036 for the rationale.
//!
//! See ADR-008 for coords (Y-up, left-handed, 8 elmos per heightmap
//! pixel). Persistent GPU state (pipeline + bind group + grid + textures)
//! lives inside `egui_wgpu::CallbackResources` as [`RenderResources`];
//! the per-frame [`TerrainCallback`] only carries camera matrix +
//! lighting tunables.

use barme_core::{
    DirtyRect, Heightmap, LayerMask, SPLAT_DIM, SplatDistribution, TILE_DIM, TILE_PIXELS, TileCoord,
};
use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu;
use eframe::wgpu;
use glam::{Mat4, Vec3};
use tracing::{debug, info, trace, warn};
use wgpu::util::DeviceExt;

/// 8 elmos per heightmap pixel — `MapSize::ELMOS_PER_SMU / HEIGHTMAP_PER_SMU`.
pub const ELMOS_PER_PIXEL: f32 = 8.0;

const HEIGHTMAP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Uint;
const SPLAT_DISTR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const SLOT_DIFFUSE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Sprint 25 / R1 / ADR-038 — base normal map format. Engine `normalsTex`
/// is a two-channel BC5 / RG8 map; we use RGBA8Unorm so the shader can
/// sample R+A per FINDINGS §7.5. The 1×1 fallback packs `(128, 0, 0, 128)`
/// so the R+A decode `(0, sqrt(1), 0)` produces a pure-up normal — i.e.
/// the texture contributes nothing and the vertex normal carries the
/// real signal until a future sprint bakes the heightmap-derived
/// normal map.
const BASE_NORMAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Sprint 25 / R1 / ADR-038 — specular texture format. RGB = colour,
/// A = exponent coefficient (per FINDINGS §7.6 specular_exp = A × 16.0).
/// The 1×1 fallback packs `(128, 128, 128, 64)` for a neutral grey with
/// `exp ≈ 4` (matte) so the shader never produces a runaway highlight
/// when no real map is bound.
const SPECULAR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Sprint 25 / R1 / ADR-038 — DNTS slot normal-array format. 4-layer
/// `Rgba8Unorm`; each layer holds one of the engine's
/// `splatDetailNormalTex1..4` slots. Defaults to "flat-up"
/// `(128, 128, 255, 128)` so the per-fragment `* 2 - 1` decode produces
/// (0, 0, 1, 0) — the slot contributes no detail until a real DDS or
/// normal PNG uploads.
const SLOT_NORMAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// `SMF_INTENSITY_MULT` from the engine — `210/255 ≈ 0.8235`. Engine
/// definition: `cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl:4`.
/// Per FINDINGS §7.1 we pre-apply this CPU-side to `ground_ambient`
/// and `ground_diffuse` so the WGSL fragment stage stays free of the
/// multiply (the engine multiplies once per-fragment inside
/// `GetShadeInt`; we hoist).
///
/// Exposed publicly so the parity-fixture loader (Sprint 25 commit 4)
/// can compute pre-dimmed ambient/diffuse values for Comet Catcher
/// Remake's `lighting.groundAmbientColor = (0.55, 0.51, 0.51)`
/// without duplicating the `210/255` constant.
#[allow(dead_code)] // Consumed by parity_fixtures + future renderer-parity sprints.
pub const SMF_INTENSITY_MULT: f32 = 210.0 / 255.0;

/// Colour format of the Sprint-13 offscreen render target. Plain sRGB
/// matches the typical swapchain on desktop and lets egui composite the
/// result without colour-space surprises. ADR-037.
pub const OFFSCREEN_COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Depth format of the Sprint-13 offscreen render target. 32-bit float
/// keeps z-precision tight enough for the auto-tuned near/far range
/// (Phase 3). Universally supported on desktop wgpu; a future fall-back
/// to `Depth32FloatStencil8` is documented in ADR-037.
pub const OFFSCREEN_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Maximum offscreen RT edge length. At 4K display × pixels_per_point=2
/// = 8K pixels across the viewport rect, an unclamped RT would burn
/// 256 MB (8 B/pixel × 8K × 4K) — too much for an iGPU budget per
/// PITFALLS §1. We cap each axis and let egui upscale the composite
/// image.
pub const OFFSCREEN_CLAMP: u32 = 2048;

/// Pre-allocated capacity (in marker instances) of the GPU storage
/// buffer used by the Sprint-13 marker pipeline. 10 000 covers Sprint
/// 13's foreseeable load (a 16-SMU map with the planned Sprint 24
/// feature density still pushes <2 000 markers in view); resizing the
/// buffer mid-frame is more painful than over-allocating ~480 KB up
/// front. ADR-037.
pub const MARKER_INSTANCE_CAPACITY: u32 = 10_000;

/// Pre-allocated capacity (in `LineVertex` count, NOT segments) of the
/// GPU vertex buffer used by the Sprint-13 line pipeline. Belt-and-
/// suspenders alongside the per-axis dash cap in
/// `overlay::collect_symmetry_segments`: 256 dashes × 2 verts × 4
/// axes (worst case Quad symmetry + one geo-vent plume) ≈ 2 050 verts;
/// 8 000 leaves comfortable headroom and removes the warn-spam path
/// even if a future renderer-parity sprint expands the line workload.
/// 32 B/vertex × 8 000 = 256 KB. ADR-037 / Sprint 13 hotfix.
pub const LINE_VERTEX_CAPACITY: u32 = 8_000;

/// Pixel side of one layer of the slot diffuse texture array. The
/// starter pack (ADR-025) ships ambientCG `_1K-PNG.zip` at 1024², so
/// the happy path matches without resizing. Per FINDINGS H2 the
/// upload path falls back to `image::imageops::resize` when an
/// imported slot's PNG is a different size.
pub const SLOT_DIFFUSE_DIM: u32 = 1024;

/// Fixed-size array bound to the fragment shader. Each layer is one
/// slot's diffuse. The shader only samples layers whose bit is set in
/// `SplatU::flags.x` (the active-slot mask).
pub const SLOT_LAYER_COUNT: u32 = 4;

// ─── Composite pipeline constants (Sprint 16 / D9 / ADR-039) ────────

/// Cap on the layered composite pipeline (D9 / ADR-039). The CPU bake
/// in `barme_core::layers::bake_diffuse` accepts any layer count; the
/// GPU preview clamps at 16 to keep the per-pixel work in the
/// fragment loop bounded and the slot-diffuse / mask texture arrays
/// at fixed sizes. Maps with >16 layers fall back to the CPU bake for
/// `.sd7` export and show a "preview is approximate" chip.
pub const COMPOSITE_MAX_LAYERS: u32 = 16;

/// Side of one layer of the composite-side slot diffuse texture array.
/// Matches `SLOT_DIFFUSE_DIM` so the same source PNGs feed both the
/// legacy Sprint-9 4-layer DNTS path and the Sprint-16 composite.
pub const SLOT_COMPOSITE_DIM: u32 = 1024;

/// Max per-axis edge length of the composite render target. Maps >
/// 8 SMU produce a `texture_dims` > 4096²; we cap the RT at 4096²
/// and let the terrain shader's bilinear sampler upscale at view
/// time. The CPU bake (D8 / ADR-038) runs at full `texture_dims`
/// for the .sd7 export. ADR-039 / PITFALLS §5.
pub const COMPOSITE_RT_CLAMP: u32 = 4096;

/// Composite RT colour format. `Rgba8Unorm` (NOT sRGB) so the
/// blending math matches the CPU bake byte-for-byte (the bake works
/// in sRGB-space-but-not-decoded; the slot diffuses are also
/// `Rgba8Unorm`).
pub const COMPOSITE_RT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Per-layer mask texture format. R8Unorm matches `LayerMask`'s u8
/// byte payload exactly; the shader reads `.r` and treats it as a
/// 0..=1 alpha.
pub const COMPOSITE_MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    /// `[max_height, elmos_per_pixel, world_extent_x, world_extent_z]`.
    /// The world extents drive the splat-distribution UV math in the
    /// fragment stage (`uv = world_pos.xz / extent`).
    params: [f32; 4],
    /// `[min_height, 0, 0, 0]` — the world Y at raw heightmap value 0.
    /// Sprint 14 introduced this for the Water tool: without
    /// `min_height < 0` the heightmap can't carve below BAR's water
    /// plane at `Y = 0` and the water preview at the floor of the map
    /// is invisible. WGSL `sample_y` reads this as `params2.x` and
    /// linearly maps raw → `min_h + t * (max_h - min_h)`.
    params2: [f32; 4],
}

/// CPU mirror of `markers.wgsl::MarkerU`. Drives the per-frame
/// projection inside the marker vertex shader; `viewport_size` is the
/// offscreen RT's physical pixel dimensions so screen-space radii hold
/// regardless of DPI scaling. ADR-037.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub struct MarkerU {
    pub view_proj: [[f32; 4]; 4],
    pub viewport_size: [f32; 2],
    pub _pad: [f32; 2],
}

/// CPU mirror of `lines.wgsl::VsIn`. One pair per `LineList` segment.
/// 32 B per vertex: vec3 position (offset 0) + 4 B pad + vec4 color
/// (offset 16). Premultiplied colour, matches the line pipeline's
/// `PREMULTIPLIED_ALPHA_BLENDING` blend state.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub _pad0: f32,
    pub color: [f32; 4],
}

impl LineVertex {
    /// Convert a world-space point + `egui::Color32` (already premul
    /// internally) to a GPU vertex.
    pub fn new(pos: glam::Vec3, color: egui::Color32) -> Self {
        let [r, g, b, a] = color.to_array();
        let inv = 1.0 / 255.0;
        Self {
            pos: [pos.x, pos.y, pos.z],
            _pad0: 0.0,
            color: [
                r as f32 * inv,
                g as f32 * inv,
                b as f32 * inv,
                a as f32 * inv,
            ],
        }
    }
}

/// One layer's slice of the composite shader's per-frame uniform.
/// Mirror of `composite.wgsl::LayerU`. ADR-039.
///
/// Inactive slots set `params[3] = 0.0`; the shader skips them in
/// the per-pixel loop.
///
/// The forward CPU bake (`LayerStack::bake_diffuse`) order is
/// `mirror → rotate → translate-by-(-offset) → scale → re-centre`
/// (pinned by `bake_mirror_then_rotate_matches_reference`). The
/// shader replays the same chain with `cos(theta)` / `sin(theta)`
/// pre-computed CPU-side to keep the per-pixel loop divide-free.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub struct CompositeLayerU {
    /// `[mirror_x_sign, mirror_y_sign, cos_theta, sin_theta]`.
    /// `mirror_*_sign` ∈ {-1.0, +1.0}.
    pub rot_mirror: [f32; 4],
    /// `[offset_x_elmos, offset_y_elmos, _pad, _pad]`.
    pub offset: [f32; 4],
    /// `[1.0 / scale, opacity, brightness_add, active_flag]`. The
    /// CPU pre-inverts scale so the shader does one multiply instead
    /// of a divide per pixel.
    pub params: [f32; 4],
    /// `[r, g, b, _reserved]` tint multiplier.
    pub tint: [f32; 4],
}

impl Default for CompositeLayerU {
    fn default() -> Self {
        // Identity layer: no mirror, no rotation, scale = 1, opacity
        // = 0, inactive. cos(0) = 1, sin(0) = 0.
        Self {
            rot_mirror: [1.0, 1.0, 1.0, 0.0],
            offset: [0.0; 4],
            params: [1.0, 0.0, 0.0, 0.0],
            tint: [1.0, 1.0, 1.0, 0.0],
        }
    }
}

impl CompositeLayerU {
    /// Build a layer uniform from a [`barme_core::TextureLayer`]'s
    /// transform / colour / blend / opacity state. The caller is
    /// responsible for setting `active` to `1.0` on the layers it
    /// wants the shader to render (typically every layer with
    /// `visible && opacity > 0`).
    pub fn from_layer(
        transform: &barme_core::LayerTransform,
        color: &barme_core::LayerColor,
        opacity: f32,
        active: bool,
    ) -> Self {
        let mx = if transform.mirror_x { -1.0_f32 } else { 1.0 };
        let my = if transform.mirror_y { -1.0_f32 } else { 1.0 };
        let (s, c) = transform.rotation_rad.sin_cos();
        let inv_scale = 1.0 / transform.scale.max(1e-4);
        Self {
            rot_mirror: [mx, my, c, s],
            offset: [
                transform.offset_elmos[0],
                transform.offset_elmos[1],
                0.0,
                0.0,
            ],
            params: [
                inv_scale,
                opacity.clamp(0.0, 1.0),
                color.brightness,
                if active { 1.0 } else { 0.0 },
            ],
            tint: [color.tint_rgb[0], color.tint_rgb[1], color.tint_rgb[2], 0.0],
        }
    }
}

/// CPU mirror of `composite.wgsl::CompositeU`. The `dims` vec carries
/// the RT dims in `.x` / `.y`; layer-loop bound is encoded per-layer
/// via the `active_flag` field (so widening `COMPOSITE_MAX_LAYERS`
/// past 16 in the future needs only a pipeline rebuild, not a uniform-
/// shape change).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub struct CompositeU {
    /// `[width_px, height_px, layer_count, _]`. Width / height are
    /// the RT's physical dimensions; layer_count is informational
    /// (the shader walks every slot and gates via `active_flag`).
    pub dims: [f32; 4],
    pub layers: [CompositeLayerU; COMPOSITE_MAX_LAYERS as usize],
}

impl Default for CompositeU {
    fn default() -> Self {
        Self {
            dims: [0.0, 0.0, 0.0, 0.0],
            layers: [CompositeLayerU::default(); COMPOSITE_MAX_LAYERS as usize],
        }
    }
}

/// CPU mirror of the WGSL `SplatU` block. Field order MUST match
/// `terrain.wgsl::SplatU` exactly (`bytemuck::Pod` enforces no padding
/// gymnastics, but order is on us).
///
/// Sprint 25 / R1 / ADR-038 extended the block with `ground_specular`
/// (per-fragment specular fallback when no specular texture is bound,
/// FINDINGS §7.6) and `camera_pos` (the world-space eye position the
/// fragment shader needs for the Blinn-Phong half-vector).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct SplatUniforms {
    pub tex_scales: [f32; 4],
    pub tex_mults: [f32; 4],
    /// `[active_slot_mask, diffuse_in_alpha, buildable_overlay_on, tex_present_bits]`.
    /// - **`flags.x`** mask bit `i` is set when channel `i` is bound
    ///   to a slot (mirrors `Project.splat_config.channels[i].is_some()`
    ///   from D5; the Sprint 17 retirement leaves the per-DNTS-layer
    ///   bind tracked in `LayerStack::dnts_layers()` instead, but the
    ///   uniform shape stays).
    /// - **`flags.y`** plumbs ADR-034's high-pass workflow toggle
    ///   through the uniform buffer; the shader treats it as a no-op
    ///   this sprint.
    /// - **`flags.z`** = `1` when the viewport's buildable-area
    ///   overlay is on. The fragment shader mixes red into the
    ///   composite where `world_normal.y < cos(10°)` (factory cap).
    /// - **`flags.w`** = texture-presence bitfield (Sprint 25 / R1):
    ///   bit 0 = base-normal texture bound (`1` ⇒ sample
    ///   `normals_tex.ra`; `0` ⇒ fall back to the vertex normal);
    ///   bit 1 = specular texture bound (`1` ⇒ per-fragment
    ///   `specular_col.a × 16.0`; `0` ⇒ use the
    ///   `ground_specular.w` global exponent);
    ///   bit 2 = DNTS normal array populated (`1` ⇒ blend the
    ///   per-fragment DNTS detail normal per FINDINGS §7.3; `0` ⇒
    ///   skip the slot sample entirely). Bit 2 is the engine's
    ///   `SMF_DETAIL_NORMAL_TEXTURE_SPLATTING` gate per
    ///   `SMFRenderState.cpp:114` (FINDINGS §7.2) — note it does NOT
    ///   require bit 1.
    pub flags: [u32; 4],
    /// World-space to-sun direction. `.w` unused.
    pub sun_dir: [f32; 4],
    /// Pre-dimmed by `SMF_INTENSITY_MULT = 210/255` CPU-side (FINDINGS
    /// §7.1) so the WGSL stays clean. `.w` unused.
    pub ground_ambient: [f32; 4],
    pub ground_diffuse: [f32; 4],
    /// Sprint 25 / R1 / ADR-038 — fallback specular state used when no
    /// per-fragment specular texture is bound. `.xyz` = colour;
    /// `.w` = exponent (engine default `100.0`, per
    /// `MapInfo.cpp:line 145 specularExponent = 100.0` /
    /// `SMFRenderState.cpp:167 sunLighting->specularExponent`).
    pub ground_specular: [f32; 4],
    /// Sprint 25 / R1 / ADR-038 — world-space camera position. Drives
    /// the per-fragment Blinn-Phong half-vector
    /// `halfDir = normalize(sun_dir + normalize(camera - worldPos))`.
    /// The engine computes `halfDir` in `SMFVertProg.glsl:34-41`; we
    /// move it to the fragment stage because our vertex shader doesn't
    /// have access to the bound camera. `.w` unused.
    pub camera_pos: [f32; 4],
}

impl Default for SplatUniforms {
    fn default() -> Self {
        // Defaults match `splats.texScales = vec4(0.02)` and `splats.
        // texMults = vec4(1.0)` from MapInfo.cpp::ReadSplats. ADR-025
        // baseline: no diffuse-in-alpha, no slots bound (mask = 0 →
        // fallback gradient shows for fresh projects).
        Self {
            tex_scales: [0.02; 4],
            tex_mults: [1.0; 4],
            flags: [0, 0, 0, 0],
            sun_dir: default_sun_dir(),
            ground_ambient: default_ground_ambient(),
            ground_diffuse: default_ground_diffuse(),
            ground_specular: default_ground_specular(),
            // Camera eye is per-frame; default is "at origin" so unit
            // tests that don't construct a TerrainCallback still
            // produce deterministic uniforms. The real value lands via
            // `TerrainCallback::new` → `prepare()`.
            camera_pos: [0.0, 0.0, 0.0, 1.0],
        }
    }
}

/// `lighting.sunDir` BAR-default normalized. Matches
/// `MapInfo::bar_default()` (FINDINGS §1.4 / pitfall #18 — W = 1.0).
/// The shader only consumes `.xyz`.
fn default_sun_dir() -> [f32; 4] {
    let v = glam::Vec3::new(0.5, 0.7, 0.5).normalize_or_zero();
    [v.x, v.y, v.z, 1.0]
}

/// `lighting.groundAmbientColor = (0.5, 0.5, 0.5)` × `SMF_INTENSITY_MULT
/// = 210/255` per FINDINGS §7.1. Pre-applied CPU-side so the WGSL
/// stays clean.
fn default_ground_ambient() -> [f32; 4] {
    let m = 210.0 / 255.0;
    [0.5 * m, 0.5 * m, 0.5 * m, 0.0]
}

/// `lighting.groundDiffuseColor = (0.5, 0.5, 0.5)` × the same intensity
/// dim. The full Recoil shading is `(diffuseCol + detailCol) *
/// (ambient + diffuse * NdotL * shadow)`; the preview omits shadow
/// and replaces the diffuse texture with the splat composite.
fn default_ground_diffuse() -> [f32; 4] {
    let m = 210.0 / 255.0;
    [0.5 * m, 0.5 * m, 0.5 * m, 0.0]
}

/// `lighting.groundSpecularColor = (0.1, 0.1, 0.1)` per
/// `MapInfo.cpp:142`, with the engine-default `specularExponent =
/// 100.0` per `MapInfo.cpp:145`. Used by the shader's fallback branch
/// when no specular texture is bound (FINDINGS §7.6: `lighting.
/// specularExponent` is ONLY consulted when no specularTex). Not
/// pre-multiplied by `SMF_INTENSITY_MULT` — the engine doesn't dim
/// specular through that constant.
fn default_ground_specular() -> [f32; 4] {
    [0.1, 0.1, 0.1, 100.0]
}

struct Grid {
    index_buf: wgpu::Buffer,
    index_count: u32,
    /// Heightmap dims this grid was built for; we rebuild when they change.
    dims: (u32, u32),
}

struct HeightmapTex {
    tex: wgpu::Texture,
    dims: (u32, u32),
}

/// GPU-side colour + depth pair the Sprint-13 renderer encodes terrain,
/// lines, and markers into. The egui composite samples `color_view`
/// through `egui_texture_id`; the offscreen pass clears `depth_view` to
/// 1.0 every frame so markers depth-test against terrain (ADR-037).
///
/// The `color` / `depth` fields keep the GPU textures alive while the
/// views are bound to the pipelines. Re-allocated when the central
/// viewport rect changes physical pixel size; see [`ensure_offscreen`].
pub struct OffscreenTarget {
    /// Kept alive for the view; bound via `color_view`.
    #[allow(dead_code)]
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    /// Kept alive for the view; bound via `depth_view`.
    #[allow(dead_code)]
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    /// egui-side handle registered with `Renderer::register_native_texture`
    /// so `ui.painter().image(id, rect, ...)` can sample the offscreen
    /// colour. Re-registered on every resize; the old id is freed
    /// before the new one is created.
    pub egui_texture_id: egui::TextureId,
    /// Physical pixel dimensions of the textures. Compared against the
    /// requested size each frame to decide whether to re-allocate.
    pub size: (u32, u32),
}

impl OffscreenTarget {
    /// Borrow the colour view (consumed by the offscreen render-pass
    /// attachment in [`TerrainCallback::prepare`]).
    pub fn color_view(&self) -> &wgpu::TextureView {
        &self.color_view
    }

    /// Borrow the depth view (consumed by the offscreen render-pass
    /// attachment in [`TerrainCallback::prepare`]).
    pub fn depth_view(&self) -> &wgpu::TextureView {
        &self.depth_view
    }
}

/// Allocate the colour + depth textures for an offscreen target at the
/// given physical pixel size. Kept private; callers go through
/// [`ensure_offscreen`].
fn allocate_offscreen_textures(
    device: &wgpu::Device,
    size: (u32, u32),
) -> (
    wgpu::Texture,
    wgpu::TextureView,
    wgpu::Texture,
    wgpu::TextureView,
) {
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen.color"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OFFSCREEN_COLOR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen.depth"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OFFSCREEN_DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    (color, color_view, depth, depth_view)
}

/// Clamp and validate a requested offscreen size. Returns `None` when
/// the request is degenerate (either axis < 2 px) so callers skip the
/// frame instead of allocating a useless 1-px texture.
///
/// Pure / GPU-free — exercised by unit tests. The clamp policy lives in
/// [`OFFSCREEN_CLAMP`] (PITFALLS §1: iGPU memory budget on 4K displays).
pub fn resolve_offscreen_size(requested: (u32, u32)) -> Option<(u32, u32)> {
    let (w, h) = requested;
    if w < 2 || h < 2 {
        return None;
    }
    Some((w.min(OFFSCREEN_CLAMP), h.min(OFFSCREEN_CLAMP)))
}

/// GPU state for the Sprint-13 marker pipeline (ADR-037). One bind
/// group, one uniform buffer (`MarkerU`), one pre-allocated storage
/// buffer for up to [`MARKER_INSTANCE_CAPACITY`] marker instances.
/// The pipeline depth-tests against terrain (which writes depth) but
/// doesn't write depth itself — translucent blending order is owned
/// by the CPU-side back-to-front sort in
/// `ui::markers::MarkerBatch::sort_back_to_front`.
struct MarkerResources {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buf: wgpu::Buffer,
    /// 10 000 × 48 = 480 KB; sized for the foreseeable Sprint 13-24
    /// load. Storage-buffer layout matches `markers.wgsl::Instance`.
    instance_buf: wgpu::Buffer,
}

/// GPU state for the Sprint-13 line pipeline (ADR-037 / Phase 5).
/// `LineList` topology — each pair of consecutive vertices forms one
/// segment. Shares `MarkerU::uniform_buf` (only consumes the
/// `view_proj` prefix). Depth-test only; premul-alpha blend.
struct LineResources {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    /// Pre-allocated [`LINE_VERTEX_CAPACITY`] verts × 32 B = 160 KB.
    vertex_buf: wgpu::Buffer,
}

/// CPU mirror of the WGSL `WaterU` block (C9 / ADR-042 — Sprint 14).
/// Field order MUST match `water.wgsl::WaterU` exactly. `bytemuck::Pod`
/// enforces no-padding sanity at compile time.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct WaterU {
    pub view_proj: [[f32; 4]; 4],
    /// Pre-multiplied RGBA. `(r·a, g·a, b·a, a)`.
    pub surface_rgba: [f32; 4],
    /// `[extent_x, extent_z, plane_y, alpha_scale]`. `plane_y` stays
    /// at `0.0` per the engine's `consteval GetWaterPlaneLevel` — the
    /// field is plumbed in advance so a future renderer-parity sprint
    /// can carry it forward without reshuffling the uniform.
    pub extent: [f32; 4],
}

/// GPU state for the Sprint-14 water plane pipeline (C9 / ADR-042).
/// Single 4-vertex `TriangleStrip` quad rendered with depth-test on /
/// depth-write off + `PREMULTIPLIED_ALPHA_BLENDING`. Draws AFTER the
/// terrain pipeline (so cliffs occlude the plane through depth) and
/// BEFORE lines/markers (so brush rings + start-pos markers still
/// stand on top of the water).
struct WaterResources {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buf: wgpu::Buffer,
}

struct SplatResources {
    // `_tex` fields keep the GPU texture alive; only the views are
    // bound. Dead-code allowance is removed in D5 (Sprint 9) once
    // `upload_*` helpers wire them.
    #[allow(dead_code)]
    distr_tex: wgpu::Texture,
    distr_view: wgpu::TextureView,
    distr_dims: (u32, u32),
    distr_samp: wgpu::Sampler,
    /// Sprint 25 / R1 / ADR-038 — 4-layer DNTS slot NORMAL array. The
    /// engine binds `splatDetailNormalTex1..4` to four separate
    /// samplers; we coalesce into one `texture_2d_array` so the WGSL
    /// can index by channel. Sprint 9 / D4's original "slot diffuse
    /// array" use of this binding was retired in Sprint 17 (the
    /// composite RT covers the diffuse base); the binding slot now
    /// carries normals. Default contents = "flat-up" (128, 128, 255,
    /// 128) so an unbound layer contributes no detail through the
    /// `* 2 - 1` decode.
    #[allow(dead_code)]
    slot_normals_tex: wgpu::Texture,
    slot_normals_view: wgpu::TextureView,
    slot_normals_samp: wgpu::Sampler,
    /// Sprint 25 / R1 / ADR-038 — base-normal map. Engine `normalsTex`;
    /// FINDINGS §7.5 reads only `.r` and `.a`. Defaults to a 1×1
    /// "up" texture; a future sprint bakes a heightmap-derived R+A
    /// normal map and uploads via `upload_base_normal`.
    #[allow(dead_code)]
    normals_tex: wgpu::Texture,
    normals_view: wgpu::TextureView,
    normals_samp: wgpu::Sampler,
    /// Sprint 25 / R1 / ADR-038 — specular map. Engine `specularTex`.
    /// RGBA8 with `.rgb` = colour and `.a` = exponent coefficient per
    /// FINDINGS §7.6 (`specular_exp = a × 16.0`). Defaults to a 1×1
    /// matte grey; `upload_specular` swaps in a real map.
    #[allow(dead_code)]
    specular_tex: wgpu::Texture,
    specular_view: wgpu::TextureView,
    specular_samp: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
}

/// GPU state for the Sprint-16 layered composite pipeline (D9 /
/// ADR-039). The `rt` is the offscreen colour target the
/// `composite.wgsl` pipeline writes into; the terrain shader
/// samples it as the diffuse base when the project carries a
/// non-empty layer stack.
///
/// Lifetimes: the `_tex` fields keep the textures alive while the
/// `_view`s are bound to pipelines. The slot diffuse array is
/// pre-sized at install time (the registry is fixed at app start);
/// the RT + mask array are re-sized on demand via
/// [`ensure_composite_rt`] when the central viewport changes.
struct CompositeResources {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    uniform_buf: wgpu::Buffer,

    /// Composite RT (`rgba8unorm`, max 4096²). `None` until the first
    /// successful [`ensure_composite_rt`] call. Re-allocated when the
    /// requested size changes; the terrain bind group + the egui
    /// texture id are refreshed alongside.
    rt: Option<CompositeRt>,

    /// 16-layer slot diffuse array (1024²). Pre-loaded once at app
    /// start; each per-slot rebind goes through
    /// [`upload_composite_slot_diffuse`].
    #[allow(dead_code)]
    slot_array_tex: wgpu::Texture,
    slot_array_view: wgpu::TextureView,
    slot_array_samp: wgpu::Sampler,

    /// 16-layer mask array (`r8unorm`, sized to RT). Allocated on the
    /// first [`ensure_composite_rt`] call; re-allocated alongside
    /// the RT when the central viewport changes.
    mask_tex: Option<wgpu::Texture>,
    mask_view: Option<wgpu::TextureView>,
    mask_samp: wgpu::Sampler,
}

/// Composite render target + the egui handle for the paint viewport.
/// Re-allocated when `ensure_composite_rt` is called with a different
/// size; the old egui id is freed before the new one is registered to
/// avoid renderer handle leaks (PITFALLS §5 — same pattern as
/// `OffscreenTarget`).
struct CompositeRt {
    #[allow(dead_code)]
    tex: wgpu::Texture,
    view: wgpu::TextureView,
    egui_texture_id: egui::TextureId,
    size: (u32, u32),
}

pub struct RenderResources {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    uniform_buf: wgpu::Buffer,
    /// Default 1×1 dummy view bound until a real heightmap is uploaded.
    /// Keeps the bind group always valid so the pipeline can be created
    /// once at startup.
    _dummy_tex: wgpu::Texture,
    /// Default 1×1 dummy composite RT view bound until
    /// [`ensure_composite_rt`] allocates a real one. Keeps the terrain
    /// bind group valid pre-allocation so the pipeline doesn't need to
    /// be rebuilt on first composite use.
    _dummy_composite_tex: wgpu::Texture,
    grid: Option<Grid>,
    heightmap: Option<HeightmapTex>,
    splat: SplatResources,
    /// Sprint-16 composite pipeline (D9 / ADR-039). The pipeline lives
    /// here; the RT view drops in when [`ensure_composite_rt`] runs.
    composite: CompositeResources,
    /// Sprint-13 offscreen RT (ADR-037). `None` until the first
    /// successful [`ensure_offscreen`] call — `central()` allocates it
    /// on the first frame with a real central viewport rect.
    offscreen: Option<OffscreenTarget>,
    /// Sprint-13 marker pipeline + buffers (ADR-037). Created once at
    /// install time; per-frame uploads write into the pre-allocated
    /// uniform + instance buffers.
    marker: MarkerResources,
    /// Sprint-13 line pipeline + vertex buffer (ADR-037 / Phase 5).
    /// Shares the marker's uniform buffer.
    line: LineResources,
    /// Sprint-14 water plane pipeline (C9 / ADR-042). Drawn between
    /// terrain and lines in `TerrainCallback::prepare`.
    water: WaterResources,
}

impl RenderResources {
    fn write_uniforms(&self, queue: &wgpu::Queue, u: &Uniforms) {
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(u));
    }

    fn write_splat_uniforms(&self, queue: &wgpu::Queue, su: &SplatUniforms) {
        queue.write_buffer(&self.splat.uniform_buf, 0, bytemuck::bytes_of(su));
    }

    fn write_composite_uniforms(&self, queue: &wgpu::Queue, cu: &CompositeU) {
        queue.write_buffer(&self.composite.uniform_buf, 0, bytemuck::bytes_of(cu));
    }

    /// View bound at the terrain bind group's composite slot. Falls
    /// back to the 1×1 dummy when no composite RT is allocated yet so
    /// the bind group stays valid even on a fresh app start.
    fn composite_terrain_view(&self) -> wgpu::TextureView {
        match &self.composite.rt {
            Some(rt) => rt.tex.create_view(&wgpu::TextureViewDescriptor::default()),
            None => self
                ._dummy_composite_tex
                .create_view(&wgpu::TextureViewDescriptor::default()),
        }
    }

    /// View bound at the terrain bind group's heightmap slot. Falls
    /// back to the 1×1 dummy when no heightmap has been uploaded yet —
    /// keeps `rebind()` safe to call from upload paths that fire before
    /// the project's heightmap lands (e.g. the parity-fixture loader
    /// uploading base normal + specular for a dim-mismatched fixture).
    /// Mirrors [`Self::composite_terrain_view`].
    fn heightmap_terrain_view(&self) -> wgpu::TextureView {
        match &self.heightmap {
            Some(h) => h
                .tex
                .create_view(&wgpu::TextureViewDescriptor::default()),
            None => self
                ._dummy_tex
                .create_view(&wgpu::TextureViewDescriptor::default()),
        }
    }

    fn rebind(&mut self, device: &wgpu::Device) {
        let heightmap_view = self.heightmap_terrain_view();
        let composite_view = self.composite_terrain_view();
        self.bind_group = make_bind_group(
            device,
            &self.bind_group_layout,
            &self.uniform_buf,
            &heightmap_view,
            &self.splat,
            &composite_view,
            &self.composite.mask_samp,
        );
    }
}

fn build_index_buffer(device: &wgpu::Device, dims: (u32, u32)) -> Grid {
    let (w, h) = dims;
    let quads_x = w.saturating_sub(1);
    let quads_z = h.saturating_sub(1);
    let mut indices: Vec<u32> = Vec::with_capacity((quads_x as usize) * (quads_z as usize) * 6);
    // CW winding so pipeline's front_face = Cw keeps front faces visible
    // from above (left-handed projection per ADR-008).
    for z in 0..quads_z {
        for x in 0..quads_x {
            let i = z * w + x;
            let i_r = i + 1;
            let i_d = i + w;
            let i_dr = i_d + 1;
            indices.extend_from_slice(&[i, i_d, i_r, i_r, i_d, i_dr]);
        }
    }
    let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("terrain.ib"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    Grid {
        index_buf,
        index_count: indices.len() as u32,
        dims,
    }
}

fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("terrain.bgl"),
        entries: &[
            // 0: terrain uniforms (view-proj + params)
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // 1: heightmap (vertex only — fragment derives normal from
            // varyings, not from this texture)
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Uint,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            // 2: splat uniforms
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // 3: splat distribution (rgba8unorm, filterable)
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            // 4: splat distribution sampler (clamp)
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            // 5: DNTS slot normal array (Sprint 25 / R1 / ADR-038).
            // 4 layers — one per engine `splatDetailNormalTex[i]`. Was
            // the Sprint-9 "slot diffuse" array; that role retired in
            // Sprint 17 when the composite RT took over the diffuse
            // base.
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            // 6: slot normal sampler (repeat — DNTS layers wallpaper-tile
            // by `splatTexScales` per FINDINGS §7.3)
            wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            // 7: composite RT (Sprint 16 / D9 / ADR-039). Diffuse-base
            // input when `params2.y > 0.5`; ignored otherwise. The
            // view rebinds on composite RT resize via `rebind()`.
            wgpu::BindGroupLayoutEntry {
                binding: 7,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            // 8: composite RT sampler. ClampToEdge — the composite RT
            // covers the full map; sampling past its edge is a
            // programmer error, not a wallpaper-tile case.
            wgpu::BindGroupLayoutEntry {
                binding: 8,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            // 9: base normal map (Sprint 25 / R1 / ADR-038). Engine
            // `normalsTex`; FINDINGS §7.5 reads R + A only.
            wgpu::BindGroupLayoutEntry {
                binding: 9,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            // 10: base normal sampler. ClampToEdge — one normal map
            // covers the full terrain.
            wgpu::BindGroupLayoutEntry {
                binding: 10,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            // 11: specular map (Sprint 25 / R1 / ADR-038). Engine
            // `specularTex`; FINDINGS §7.6 — `.a × 16.0` is the
            // per-fragment exponent.
            wgpu::BindGroupLayoutEntry {
                binding: 11,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            // 12: specular sampler. ClampToEdge — one specular map per
            // terrain.
            wgpu::BindGroupLayoutEntry {
                binding: 12,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

fn make_bind_group(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    uniform_buf: &wgpu::Buffer,
    heightmap_view: &wgpu::TextureView,
    splat: &SplatResources,
    composite_view: &wgpu::TextureView,
    composite_samp: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("terrain.bg"),
        layout: bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(heightmap_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: splat.uniform_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&splat.distr_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(&splat.distr_samp),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(&splat.slot_normals_view),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::Sampler(&splat.slot_normals_samp),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(composite_view),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::Sampler(composite_samp),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: wgpu::BindingResource::TextureView(&splat.normals_view),
            },
            wgpu::BindGroupEntry {
                binding: 10,
                resource: wgpu::BindingResource::Sampler(&splat.normals_samp),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: wgpu::BindingResource::TextureView(&splat.specular_view),
            },
            wgpu::BindGroupEntry {
                binding: 12,
                resource: wgpu::BindingResource::Sampler(&splat.specular_samp),
            },
        ],
    })
}

fn install_splat_resources(device: &wgpu::Device, queue: &wgpu::Queue) -> SplatResources {
    // Distribution: SPLAT_DIM² rgba8 zeros. ADR-036 — fragment shader
    // multiplies the sample by `tex_mults * active_slot_mask`, so all-
    // zero distribution + zero mask = fallback gradient shows.
    let distr_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("terrain.splat.distr"),
        size: wgpu::Extent3d {
            width: SPLAT_DIM,
            height: SPLAT_DIM,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SPLAT_DISTR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let distr_view = distr_tex.create_view(&wgpu::TextureViewDescriptor::default());
    // wgpu zero-initialises textures on first use (Sprint 23 / H1 lesson:
    // the prior manual row-by-row zero-fill saturated the staging arena
    // on Vega 8 iGPU). We rely on that default — the shader's `cofac =
    // dist × mults × mask` produces zero detail until a real
    // distribution lands via `upload_splat_distribution`.

    let distr_samp = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("terrain.splat.distr.sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });

    // Sprint 25 / R1 / ADR-038 — DNTS slot normal array. 4 layers at
    // SLOT_DIFFUSE_DIM² (1024²) — one per engine
    // `splatDetailNormalTex[i]`. Default content: "flat-up"
    // `(128, 128, 255, 128)` so the WGSL's `* 2 - 1` decode produces a
    // pure +Z normal, and the slot contributes nothing through
    // `splatCofac * decoded` until a real normal uploads.
    let slot_normals_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("terrain.splat.slot_normals"),
        size: wgpu::Extent3d {
            width: SLOT_DIFFUSE_DIM,
            height: SLOT_DIFFUSE_DIM,
            depth_or_array_layers: SLOT_LAYER_COUNT,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SLOT_NORMAL_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let slot_normals_view = slot_normals_tex.create_view(&wgpu::TextureViewDescriptor {
        label: Some("terrain.splat.slot_normals.view"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let flat_up_pixel = [0x80u8, 0x80, 0xFF, 0x80];
    let flat_up_layer: Vec<u8> = flat_up_pixel
        .iter()
        .copied()
        .cycle()
        .take((SLOT_DIFFUSE_DIM as usize) * (SLOT_DIFFUSE_DIM as usize) * 4)
        .collect();
    for layer in 0..SLOT_LAYER_COUNT {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &slot_normals_tex,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &flat_up_layer,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SLOT_DIFFUSE_DIM * 4),
                rows_per_image: Some(SLOT_DIFFUSE_DIM),
            },
            wgpu::Extent3d {
                width: SLOT_DIFFUSE_DIM,
                height: SLOT_DIFFUSE_DIM,
                depth_or_array_layers: 1,
            },
        );
    }

    let slot_normals_samp = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("terrain.splat.slot_normals.sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });

    // Sprint 25 / R1 / ADR-038 — base normal map. 1×1 fallback.
    // `(128, 0, 0, 128)` so the R+A decode (FINDINGS §7.5) produces
    // `nx = 0, nz = 0, ny = sqrt(1)` = pure up. The shader gates the
    // sampled normal on the `has_base_normal_tex` bit (flags.w & 1u);
    // when no real map is bound the vertex normal carries the signal.
    let (normals_tex, normals_view, normals_samp) = install_default_2d_texture(
        device,
        queue,
        "terrain.normals",
        BASE_NORMAL_FORMAT,
        &[0x80, 0x00, 0x00, 0x80],
        wgpu::AddressMode::ClampToEdge,
    );

    // Sprint 25 / R1 / ADR-038 — specular map. 1×1 matte grey fallback.
    // `(128, 128, 128, 64)` → colour mid-grey, exponent ≈ 4 (matte).
    let (specular_tex, specular_view, specular_samp) = install_default_2d_texture(
        device,
        queue,
        "terrain.specular",
        SPECULAR_FORMAT,
        &[0x80, 0x80, 0x80, 0x40],
        wgpu::AddressMode::ClampToEdge,
    );

    let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("terrain.splat.uniforms"),
        size: std::mem::size_of::<SplatUniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &uniform_buf,
        0,
        bytemuck::bytes_of(&SplatUniforms::default()),
    );

    SplatResources {
        distr_tex,
        distr_view,
        distr_dims: (SPLAT_DIM, SPLAT_DIM),
        distr_samp,
        slot_normals_tex,
        slot_normals_view,
        slot_normals_samp,
        normals_tex,
        normals_view,
        normals_samp,
        specular_tex,
        specular_view,
        specular_samp,
        uniform_buf,
    }
}

/// Allocate a 1×1 default 2D texture pre-filled with `pixel_rgba`.
/// Returns the kept-alive `Texture`, its default view, and a matching
/// sampler. Sprint 25 / R1 / ADR-038 — each Group-0 base normal /
/// specular slot uses one of these as the fallback while no real
/// terrain texture is bound, so the shader bind group never changes
/// shape per frame.
fn install_default_2d_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label_prefix: &str,
    format: wgpu::TextureFormat,
    pixel_rgba: &[u8; 4],
    address_mode: wgpu::AddressMode,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label_prefix),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        pixel_rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some(label_prefix),
        address_mode_u: address_mode,
        address_mode_v: address_mode,
        address_mode_w: address_mode,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    (tex, view, sampler)
}

/// Build the Sprint-13 marker pipeline + bind group + buffers
/// (ADR-037). The instance buffer is pre-allocated for
/// [`MARKER_INSTANCE_CAPACITY`] markers; per-frame uploads write into
/// it via `queue.write_buffer`. The pipeline shares the offscreen
/// colour + depth attachments with terrain (drawn back-to-back in
/// `TerrainCallback::prepare`'s render pass).
fn install_marker_resources(device: &wgpu::Device) -> MarkerResources {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("markers.wgsl"),
        source: wgpu::ShaderSource::Wgsl(include_str!("markers.wgsl").into()),
    });

    let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("markers.uniforms"),
        size: std::mem::size_of::<MarkerU>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let instance_stride = std::mem::size_of::<crate::ui::markers::MarkerInstanceGpu>() as u64;
    let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("markers.instances"),
        size: instance_stride * MARKER_INSTANCE_CAPACITY as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("markers.bgl"),
        entries: &[
            // 0: MarkerU uniform — view_proj + viewport_size
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // 1: instances storage buffer (read-only)
            // VS reads to project; FS reads to look up colour/shape_id.
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("markers.bg"),
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: instance_buf.as_entire_binding(),
            },
        ],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("markers.pl"),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("markers.pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[], // no vertex buffers — vs_main uses vid + storage
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            // No back-face culling — markers are camera-aligned billboards.
            cull_mode: None,
            ..Default::default()
        },
        // Depth-test only — markers DON'T write depth (else they'd
        // occlude each other in iteration order). Translucent blending
        // order is owned by the CPU-side back-to-front sort.
        depth_stencil: Some(wgpu::DepthStencilState {
            format: OFFSCREEN_DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: OFFSCREEN_COLOR_FORMAT,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    });

    MarkerResources {
        pipeline,
        bind_group,
        uniform_buf,
        instance_buf,
    }
}

/// Build the Sprint-13 line pipeline + bind group + vertex buffer
/// (ADR-037 / Phase 5). Shares the marker uniform buffer for
/// `view_proj` (the shader ignores the trailing `viewport_size` /
/// `_pad` fields of `MarkerU`).
fn install_line_resources(
    device: &wgpu::Device,
    marker_uniform_buf: &wgpu::Buffer,
) -> LineResources {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("lines.wgsl"),
        source: wgpu::ShaderSource::Wgsl(include_str!("lines.wgsl").into()),
    });

    let vertex_stride = std::mem::size_of::<LineVertex>() as u64;
    let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("lines.vertices"),
        size: vertex_stride * LINE_VERTEX_CAPACITY as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("lines.bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("lines.bg"),
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: marker_uniform_buf.as_entire_binding(),
        }],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("lines.pl"),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });

    let vertex_buffer_layout = wgpu::VertexBufferLayout {
        array_stride: vertex_stride,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 16,
                shader_location: 1,
            },
        ],
    };

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("lines.pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[vertex_buffer_layout],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::LineList,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: OFFSCREEN_DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: OFFSCREEN_COLOR_FORMAT,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    });

    LineResources {
        pipeline,
        bind_group,
        vertex_buf,
    }
}

/// Build the Sprint-14 water plane pipeline + bind group + uniform
/// buffer (C9 / ADR-042). One bind-group entry (`WaterU` uniform);
/// no vertex buffer (the shader generates the 4 quad corners from
/// `@builtin(vertex_index)` + extent).
///
/// Pipeline state:
/// - Depth TEST: ON (terrain cliffs occlude the plane).
/// - Depth WRITE: OFF (translucent; CPU-side draw order owns blend
///   ordering — terrain → water → lines → markers).
/// - Blend: `PREMULTIPLIED_ALPHA_BLENDING` (matches marker / line
///   pipelines; CPU pre-multiplies the surface RGBA on its way to
///   the uniform).
/// - Cull: `Back`. Quad winding is CW seen from above.
fn install_water_resources(device: &wgpu::Device) -> WaterResources {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("water.wgsl"),
        source: wgpu::ShaderSource::Wgsl(include_str!("water.wgsl").into()),
    });

    let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("water.uniforms"),
        size: std::mem::size_of::<WaterU>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("water.bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("water.bg"),
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buf.as_entire_binding(),
        }],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("water.pl"),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("water.pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            // No culling — viewing from below the water plane (rare,
            // but possible during orbit) should still show the
            // underside. The marker pipeline takes the same call for
            // camera-aligned billboards (PrimitiveState L793).
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: OFFSCREEN_DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: OFFSCREEN_COLOR_FORMAT,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    });

    WaterResources {
        pipeline,
        bind_group,
        uniform_buf,
    }
}

/// Build the composite pipeline + bind-group layout + slot-array
/// texture + mask sampler. Called once at install time; the per-frame
/// RT + mask array allocate later via [`ensure_composite_rt`] once
/// the central viewport size is known.
///
/// Sprint 16 / D9 / ADR-039.
fn install_composite_resources(device: &wgpu::Device, queue: &wgpu::Queue) -> CompositeResources {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("composite.wgsl"),
        source: wgpu::ShaderSource::Wgsl(include_str!("composite.wgsl").into()),
    });

    // 16-layer slot diffuse array at 1024². Initialised to a magenta
    // diagnostic so an unbound layer doesn't render garbage if the
    // user's slot registry has gaps. Per-slot `upload_composite_slot_
    // diffuse` overwrites this with the real diffuse when a slot
    // binds.
    let slot_array_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("composite.slot_array"),
        size: wgpu::Extent3d {
            width: SLOT_COMPOSITE_DIM,
            height: SLOT_COMPOSITE_DIM,
            depth_or_array_layers: COMPOSITE_MAX_LAYERS,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SLOT_DIFFUSE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let slot_array_view = slot_array_tex.create_view(&wgpu::TextureViewDescriptor {
        label: Some("composite.slot_array.view"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    // Magenta = (255, 0, 255, 255). Loud unbound-layer diagnostic.
    let magenta_layer: Vec<u8> = vec![0xFF, 0x00, 0xFF, 0xFF]
        .into_iter()
        .cycle()
        .take((SLOT_COMPOSITE_DIM as usize) * (SLOT_COMPOSITE_DIM as usize) * 4)
        .collect();
    for layer in 0..COMPOSITE_MAX_LAYERS {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &slot_array_tex,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &magenta_layer,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SLOT_COMPOSITE_DIM * 4),
                rows_per_image: Some(SLOT_COMPOSITE_DIM),
            },
            wgpu::Extent3d {
                width: SLOT_COMPOSITE_DIM,
                height: SLOT_COMPOSITE_DIM,
                depth_or_array_layers: 1,
            },
        );
    }

    let slot_array_samp = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("composite.slot_array.sampler"),
        // CRITICAL — wallpaper-tile. ClampToEdge would stretch scaled-
        // down textures into a smeared seam (PITFALLS §2 in the
        // Sprint-16 prompt).
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });

    let mask_samp = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("composite.mask.sampler"),
        // ClampToEdge — masks must NOT tile (else a stroke near one
        // edge would bleed into the opposite edge of the wallpaper
        // composite).
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });

    let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("composite.uniforms"),
        size: std::mem::size_of::<CompositeU>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&CompositeU::default()));

    // 1×16 dummy mask layer — keeps the bind group valid before the
    // first real `ensure_composite_rt` lands a properly-sized mask
    // array. Initialised to all zero so the shader-side composite
    // produces the mid-grey background.
    let dummy_mask_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("composite.mask.dummy"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: COMPOSITE_MAX_LAYERS,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COMPOSITE_MASK_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    // Zero-initialise each layer so the dummy reads predictably.
    for layer in 0..COMPOSITE_MAX_LAYERS {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &dummy_mask_tex,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &[0u8],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(1),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
    }
    let dummy_mask_view = dummy_mask_tex.create_view(&wgpu::TextureViewDescriptor {
        label: Some("composite.mask.dummy.view"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });

    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("composite.bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("composite.bg"),
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&slot_array_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&slot_array_samp),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&dummy_mask_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(&mask_samp),
            },
        ],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("composite.pl"),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("composite.pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: COMPOSITE_RT_FORMAT,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    });

    // Keep the dummy mask alive while the bind group references it.
    // The CompositeResources owns it via `mask_tex` until the real
    // mask array allocates; the first `ensure_composite_rt` swaps
    // both `mask_tex` / `mask_view` AND rebuilds the bind group.
    CompositeResources {
        pipeline,
        bind_group_layout: bgl,
        bind_group,
        uniform_buf,
        rt: None,
        slot_array_tex,
        slot_array_view,
        slot_array_samp,
        mask_tex: Some(dummy_mask_tex),
        mask_view: Some(dummy_mask_view),
        mask_samp,
    }
}

/// Install the pipeline, uniform buffer, splat resources, and a 1×1
/// dummy heightmap. Called once from `App::new`.
pub fn install(render_state: &egui_wgpu::RenderState) {
    let device = &render_state.device;
    let queue = &render_state.queue;

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("terrain.wgsl"),
        source: wgpu::ShaderSource::Wgsl(include_str!("terrain.wgsl").into()),
    });

    let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("terrain.uniforms"),
        size: std::mem::size_of::<Uniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let bgl = bind_group_layout(device);

    // Dummy 1×1 r16uint texture so the bind group is always valid even
    // before a real heightmap loads. Uploaded with a single 0 sample.
    let dummy_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("terrain.heightmap.dummy"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: HEIGHTMAP_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &dummy_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&[0u16]),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(2),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let dummy_view = dummy_tex.create_view(&wgpu::TextureViewDescriptor::default());

    // Sprint 16 / D9 — 1×1 dummy composite RT view bound until
    // `ensure_composite_rt` allocates a real one. Same shape as the
    // heightmap dummy; keeps the terrain bind group always valid so
    // a fresh app start doesn't need an early "create real composite
    // RT" branch.
    let dummy_composite_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("terrain.composite.dummy"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COMPOSITE_RT_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    // Zero-fill the dummy so the shader sees a deterministic (0, 0,
    // 0, 0) sample if it ever reaches here. The terrain shader's
    // `use_composite_rt` gate guards against that in practice.
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &dummy_composite_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[0u8, 0, 0, 0],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let dummy_composite_view =
        dummy_composite_tex.create_view(&wgpu::TextureViewDescriptor::default());

    let splat = install_splat_resources(device, queue);
    let composite = install_composite_resources(device, queue);
    let bind_group = make_bind_group(
        device,
        &bgl,
        &uniform_buf,
        &dummy_view,
        &splat,
        &dummy_composite_view,
        &composite.mask_samp,
    );

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("terrain.pl"),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("terrain.pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            // No vertex buffers — vs_main derives XZ from @builtin(vertex_index)
            // and Y from the heightmap texture.
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Cw,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        // ADR-037 — terrain WRITES depth so markers + lines (later
        // passes in the same offscreen render pass) can test against
        // it. Depth32Float matches the offscreen depth attachment.
        depth_stencil: Some(wgpu::DepthStencilState {
            format: OFFSCREEN_DEPTH_FORMAT,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            // Pinned to the offscreen RT's colour format (NOT the
            // swapchain's `render_state.target_format`) per pitfall #2.
            // wgpu rejects a pipeline whose colour-target format
            // disagrees with the actual RenderPassColorAttachment view.
            targets: &[Some(wgpu::ColorTargetState {
                format: OFFSCREEN_COLOR_FORMAT,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    });

    let marker = install_marker_resources(device);
    let line = install_line_resources(device, &marker.uniform_buf);
    let water = install_water_resources(device);

    render_state
        .renderer
        .write()
        .callback_resources
        .insert(RenderResources {
            pipeline,
            bind_group_layout: bgl,
            bind_group,
            uniform_buf,
            _dummy_tex: dummy_tex,
            _dummy_composite_tex: dummy_composite_tex,
            grid: None,
            heightmap: None,
            splat,
            composite,
            offscreen: None,
            marker,
            line,
            water,
        });
}

/// Allocate/refresh the heightmap texture for the given CPU heightmap and
/// rebuild the index grid if dims changed. Replaces any previously uploaded
/// heightmap.
pub fn upload_heightmap(render_state: &egui_wgpu::RenderState, heightmap: &Heightmap) {
    let device = &render_state.device;
    let queue = &render_state.queue;
    let dims = heightmap.dims();

    let mut renderer = render_state.renderer.write();
    let Some(res) = renderer.callback_resources.get_mut::<RenderResources>() else {
        warn!(
            "upload_heightmap called before render::install — heightmap discarded; \
             this is a programming error"
        );
        return;
    };

    // (Re)allocate texture if dims changed or none yet.
    let need_alloc = match &res.heightmap {
        Some(h) => h.dims != dims,
        None => true,
    };
    if need_alloc {
        info!(
            width = dims.0,
            height = dims.1,
            bytes = (dims.0 as u64) * (dims.1 as u64) * 2,
            format = ?HEIGHTMAP_FORMAT,
            "allocating heightmap texture"
        );
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terrain.heightmap"),
            size: wgpu::Extent3d {
                width: dims.0,
                height: dims.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HEIGHTMAP_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        res.heightmap = Some(HeightmapTex { tex, dims });
        res.rebind(device);
    }

    // Full write — small relative to texture size, cheap.
    let tex = &res
        .heightmap
        .as_ref()
        .expect("upload_heightmap: texture was just allocated above")
        .tex;
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(heightmap.data()),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(dims.0 * 2),
            rows_per_image: Some(dims.1),
        },
        wgpu::Extent3d {
            width: dims.0,
            height: dims.1,
            depth_or_array_layers: 1,
        },
    );

    let need_grid = match &res.grid {
        Some(g) => g.dims != dims,
        None => true,
    };
    if need_grid {
        res.grid = Some(build_index_buffer(device, dims));
    }
}

/// Allocate (or re-allocate) the Sprint-13 offscreen render target so it
/// matches the central viewport's physical pixel size. Returns the
/// `egui::TextureId` `central()` should hand to `ui.painter().image(...)`
/// when compositing the offscreen colour into the viewport rect.
///
/// Returns `None` on degenerate inputs (`requested < 2 px on either
/// axis`) or when [`install`] hasn't run; in either case `central()`
/// should skip the composite that frame (the prior frame's image stays
/// on screen — less jarring than a green flash).
///
/// Idempotency: if the offscreen RT is already at `resolve_offscreen_size
/// (requested)`, this is a no-op aside from returning the cached id.
/// Re-registers the egui texture only on actual size change; the old id
/// is freed first to prevent renderer handle leaks (pitfall #5).
///
/// ADR-037.
pub fn ensure_offscreen(
    render_state: &egui_wgpu::RenderState,
    requested: (u32, u32),
) -> Option<egui::TextureId> {
    let size = resolve_offscreen_size(requested)?;
    let was_clamped = size != requested;

    let device = render_state.device.clone();
    let mut renderer = render_state.renderer.write();

    // Step 1: decide whether to re-allocate, and if so take out the old
    // target so we can drop the `get_mut` borrow before calling
    // `renderer.free_texture` (which needs its own &mut Renderer).
    let (needs_realloc, old_id) = {
        let Some(res) = renderer.callback_resources.get_mut::<RenderResources>() else {
            warn!("ensure_offscreen: no RenderResources (install() not run)");
            return None;
        };
        let same_size = matches!(&res.offscreen, Some(rt) if rt.size == size);
        if same_size {
            return Some(
                res.offscreen
                    .as_ref()
                    .expect("matched on Some above")
                    .egui_texture_id,
            );
        }
        let old = res.offscreen.take().map(|rt| rt.egui_texture_id);
        (true, old)
    };

    if let Some(id) = old_id {
        renderer.free_texture(&id);
    }

    if needs_realloc {
        if was_clamped {
            warn!(
                requested = ?requested,
                clamped = ?size,
                cap = OFFSCREEN_CLAMP,
                "offscreen RT size clamped (PITFALLS §1)"
            );
        }
        let (color, color_view, depth, depth_view) = allocate_offscreen_textures(&device, size);
        // Register the colour view as an egui-side texture so the
        // painter can sample it. Linear filter so any DPI / clamp-driven
        // upscale doesn't look pixellated.
        let egui_texture_id =
            renderer.register_native_texture(&device, &color_view, wgpu::FilterMode::Linear);
        info!(
            width = size.0,
            height = size.1,
            color_format = ?OFFSCREEN_COLOR_FORMAT,
            depth_format = ?OFFSCREEN_DEPTH_FORMAT,
            "offscreen RT (re)allocated"
        );

        let Some(res) = renderer.callback_resources.get_mut::<RenderResources>() else {
            // Renderer state vanished between the two borrows — almost
            // impossible (we hold the write lock the whole way), but
            // bail cleanly rather than panic if it ever does.
            renderer.free_texture(&egui_texture_id);
            return None;
        };
        res.offscreen = Some(OffscreenTarget {
            color,
            color_view,
            depth,
            depth_view,
            egui_texture_id,
            size,
        });
        return Some(egui_texture_id);
    }

    // Unreachable: we either returned early (same size) or set
    // needs_realloc = true above.
    None
}

/// Camera state owned by the App; computes a view-projection matrix per
/// frame. Orbit around `target` at radius `distance`, yaw/pitch in radians.
///
/// Phase 3 / ADR-037: near + far planes are no longer struct fields —
/// they're derived from `distance` by [`OrbitCamera::near_far`] every
/// frame so depth precision auto-tracks zoom level.
#[derive(Clone)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov_y: f32,
}

impl OrbitCamera {
    /// Frame a map whose horizontal extent is `(extent_x, extent_z)` elmos.
    pub fn framing(extent_x: f32, extent_z: f32) -> Self {
        let max = extent_x.max(extent_z);
        Self {
            target: Vec3::new(extent_x * 0.5, 0.0, extent_z * 0.5),
            yaw: std::f32::consts::FRAC_PI_4,
            pitch: std::f32::consts::FRAC_PI_4,
            distance: max * 1.4,
            fov_y: 60f32.to_radians(),
        }
    }

    fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let dir = Vec3::new(cp * sy, sp, cp * cy);
        self.target + dir * self.distance
    }

    /// Auto-tuned near/far for the Sprint-13 depth pipeline. Picks a
    /// distance-relative near (1 % of orbit distance, floored at 50
    /// elmos) and a far that holds the whole map in view even when the
    /// camera tilts down (4 × distance, with a 100× near/far ratio
    /// floor so close-up zooms don't blow out depth precision). The
    /// formula is pinned by `camera_near_far_*` tests.
    ///
    /// Phase 3 / ADR-037. The struct's `near` / `far` fields stay (other
    /// readers still see them); `view_proj_matrix` now reads from this
    /// helper instead.
    pub fn near_far(&self) -> (f32, f32) {
        let near = (self.distance * 0.01).max(50.0);
        let far = (self.distance * 4.0).max(near * 100.0);
        (near, far)
    }

    /// Left-handed view matrix. Exposed for the Sprint-13 marker batch
    /// back-to-front sort (`MarkerBatch::sort_back_to_front` takes
    /// the view matrix to compute per-marker view-space Z). Production
    /// always calls `view_proj_matrix` for shader uploads — sharing the
    /// matrix here means sort + shader projection agree by
    /// construction.
    pub fn view_matrix(&self) -> Mat4 {
        Mat4::look_at_lh(self.eye(), self.target, Vec3::Y)
    }

    pub fn view_proj_matrix(&self, aspect: f32) -> Mat4 {
        let (near, far) = self.near_far();
        let proj = Mat4::perspective_lh(self.fov_y, aspect, near, far);
        proj * self.view_matrix()
    }
}

/// Unproject a cursor position (relative to a screen rect) onto the y=0
/// world plane. Used for brush picking — accurate enough at moderate map
/// inclinations, sloppy when the camera looks edge-on. Real ray-vs-
/// heightmap is Stage 1 polish.
pub fn screen_to_world_y0(
    cursor_in_rect: glam::Vec2,
    rect_size: glam::Vec2,
    camera: &OrbitCamera,
) -> Option<Vec3> {
    let aspect = (rect_size.x / rect_size.y).max(0.0001);
    let vp_inv = camera.view_proj_matrix(aspect).inverse();
    let ndc_x = 2.0 * cursor_in_rect.x / rect_size.x - 1.0;
    let ndc_y = 1.0 - 2.0 * cursor_in_rect.y / rect_size.y;
    let near = vp_inv * glam::Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
    let far = vp_inv * glam::Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
    if near.w.abs() < 1e-6 || far.w.abs() < 1e-6 {
        return None;
    }
    let near = near.truncate() / near.w;
    let far = far.truncate() / far.w;
    let dir = far - near;
    if dir.y.abs() < 1e-6 {
        return None;
    }
    let t = -near.y / dir.y;
    if t < 0.0 {
        return None;
    }
    Some(near + dir * t)
}

/// Project a world-space point onto the central preview rect. Returns
/// `None` only when the point is behind the camera (`clip.w <= 0`);
/// off-screen points (NDC outside `[-1, 1]`) still return their
/// projected position — let the caller clip via `ui.painter_at(rect)`
/// or compare distance against a hit-test radius.
///
/// Sprint 13 / Phase 6 (ADR-037): relaxed off-screen rejection so
/// label projection agrees with the GPU marker pipeline on screen-edge
/// points (the GPU rasterizer naturally discards off-RT fragments).
/// Previously rejected any NDC outside `[-1, 1]`, which made labels
/// disappear when the marker was a few pixels past the rect edge.
pub fn world_to_screen(
    world: glam::Vec3,
    rect_size: glam::Vec2,
    camera: &OrbitCamera,
) -> Option<glam::Vec2> {
    let aspect = (rect_size.x / rect_size.y).max(0.0001);
    let clip = camera.view_proj_matrix(aspect) * world.extend(1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let ndc_x = clip.x / clip.w;
    let ndc_y = clip.y / clip.w;
    Some(glam::Vec2::new(
        (ndc_x + 1.0) * 0.5 * rect_size.x,
        (1.0 - ndc_y) * 0.5 * rect_size.y,
    ))
}

/// Upload a sub-rect of the heightmap to the GPU texture. The full
/// heightmap data + dims are passed so we can compute the byte offset
/// into the source slice; this avoids a copy on the CPU side.
pub fn write_heightmap_rect(
    render_state: &egui_wgpu::RenderState,
    full_dims: (u32, u32),
    full_data: &[u16],
    rect: DirtyRect,
) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }
    let renderer = render_state.renderer.read();
    let Some(res) = renderer.callback_resources.get::<RenderResources>() else {
        warn!("write_heightmap_rect: no RenderResources (install() not run)");
        return;
    };
    let Some(hm_tex) = res.heightmap.as_ref() else {
        warn!(
            "write_heightmap_rect called before upload_heightmap — dropping update; \
             this is a programming error"
        );
        return;
    };
    if hm_tex.dims != full_dims {
        // Dims changed since the last upload — caller should be using
        // `upload_heightmap` instead. Refuse rather than corrupt and warn
        // loudly so the bug is found.
        warn!(
            tex_dims = ?hm_tex.dims,
            caller_dims = ?full_dims,
            "write_heightmap_rect: dim mismatch with GPU texture; update dropped"
        );
        return;
    }
    let queue = &render_state.queue;
    let start = (rect.y as usize) * (full_dims.0 as usize) + (rect.x as usize);
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &hm_tex.tex,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: rect.x,
                y: rect.y,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&full_data[start..]),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(full_dims.0 * 2),
            rows_per_image: Some(rect.h),
        },
        wgpu::Extent3d {
            width: rect.w,
            height: rect.h,
            depth_or_array_layers: 1,
        },
    );
}

/// Replace the GPU splat distribution texture wholesale. Called on
/// project open / new / wizard-apply where the entire distribution
/// changes at once; brush strokes go through [`write_splat_rect`]
/// instead.
#[allow(dead_code)] // D5 wires this from `Project.splat_distribution`.
pub fn upload_splat_distribution(render_state: &egui_wgpu::RenderState, dist: &SplatDistribution) {
    let renderer = render_state.renderer.read();
    let Some(res) = renderer.callback_resources.get::<RenderResources>() else {
        warn!("upload_splat_distribution: no RenderResources");
        return;
    };
    let (w, h) = res.splat.distr_dims;
    if (dist.width, dist.height) != (w, h) {
        // SPLAT_DIM is a compile-time constant on the core side; if a
        // future change widens it, we'd need to reallocate the GPU
        // texture. Loud failure beats silent stretching.
        warn!(
            tex_dims = ?(w, h),
            cpu_dims = ?(dist.width, dist.height),
            "upload_splat_distribution: dim mismatch; update dropped"
        );
        return;
    }
    let queue = &render_state.queue;
    let bytes: &[u8] = bytemuck::cast_slice(&dist.rgba);
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &res.splat.distr_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    trace!(
        width = w,
        height = h,
        "upload_splat_distribution: full write"
    );
}

/// Sub-upload a dirty rect of the splat distribution. Mirrors
/// [`write_heightmap_rect`] (ADR-017 / D4 ADR-036): the caller hands
/// the full RGBA8 array + dims so we can compute the byte offset
/// without copying.
#[allow(dead_code)] // D5 wires this from the brush-stroke dispatch.
pub fn write_splat_rect(
    render_state: &egui_wgpu::RenderState,
    full_dims: (u32, u32),
    full_data: &[[u8; 4]],
    rect: DirtyRect,
) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }
    let renderer = render_state.renderer.read();
    let Some(res) = renderer.callback_resources.get::<RenderResources>() else {
        warn!("write_splat_rect: no RenderResources");
        return;
    };
    if res.splat.distr_dims != full_dims {
        warn!(
            tex_dims = ?res.splat.distr_dims,
            caller_dims = ?full_dims,
            "write_splat_rect: dim mismatch with GPU texture; update dropped"
        );
        return;
    }
    let queue = &render_state.queue;
    let bytes: &[u8] = bytemuck::cast_slice(full_data);
    let row_stride = (full_dims.0 as usize) * 4;
    let start = (rect.y as usize) * row_stride + (rect.x as usize) * 4;
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &res.splat.distr_tex,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: rect.x,
                y: rect.y,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        &bytes[start..],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(full_dims.0 * 4),
            rows_per_image: Some(rect.h),
        },
        wgpu::Extent3d {
            width: rect.w,
            height: rect.h,
            depth_or_array_layers: 1,
        },
    );
    trace!(
        x = rect.x,
        y = rect.y,
        w = rect.w,
        h = rect.h,
        bytes = (rect.w as u64) * (rect.h as u64) * 4,
        "write_splat_rect"
    );
}

/// Copy one slot's normal map into layer `layer` of the DNTS slot
/// normal array (engine `splatDetailNormalTex[layer]`). The source
/// `rgba` slice is treated as `SLOT_DIFFUSE_DIM × SLOT_DIFFUSE_DIM`
/// `Rgba<u8>` pixels — callers resize incoming PNGs to that fixed dim
/// before invoking. Logs at `info!` per the tracing convention.
///
/// Sprint 25 / R1 / ADR-038 — the binding's old "slot diffuse" role
/// retired in Sprint 17 (the composite RT carries the diffuse base);
/// the same texture slot now holds normals.
#[allow(dead_code)] // Future sprint wires this from the layer-stack DNTS bind path.
pub fn upload_slot_normal_layer(
    render_state: &egui_wgpu::RenderState,
    layer: u32,
    rgba: &[u8],
) {
    if layer >= SLOT_LAYER_COUNT {
        warn!(layer, "upload_slot_normal_layer: layer out of range");
        return;
    }
    let expected = (SLOT_DIFFUSE_DIM as usize) * (SLOT_DIFFUSE_DIM as usize) * 4;
    if rgba.len() != expected {
        warn!(
            got = rgba.len(),
            expected,
            "upload_slot_normal_layer: byte length mismatch; resize before calling"
        );
        return;
    }
    let renderer = render_state.renderer.read();
    let Some(res) = renderer.callback_resources.get::<RenderResources>() else {
        warn!("upload_slot_normal_layer: no RenderResources");
        return;
    };
    let queue = &render_state.queue;
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &res.splat.slot_normals_tex,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: layer,
            },
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(SLOT_DIFFUSE_DIM * 4),
            rows_per_image: Some(SLOT_DIFFUSE_DIM),
        },
        wgpu::Extent3d {
            width: SLOT_DIFFUSE_DIM,
            height: SLOT_DIFFUSE_DIM,
            depth_or_array_layers: 1,
        },
    );
    info!(layer, "upload_slot_normal_layer: slot normal written");
}

/// Upload a full base-normal map texture into the terrain bind group.
/// Allocates a fresh `Rgba8Unorm` texture sized to `(width, height)`
/// and re-binds the terrain group so subsequent draws sample the new
/// map. `rgba` MUST be `width × height × 4` bytes (RGBA8).
///
/// Sprint 25 / R1 / ADR-038. The caller is responsible for setting
/// `SplatUniforms.flags.w |= 1` (the `has_base_normal_tex` bit) so the
/// fragment shader actually consumes the sample instead of falling back
/// to the vertex normal — `upload_base_normal` doesn't touch the
/// uniform because the typical flow is "upload once, leave bit set".
#[allow(dead_code)] // Wired by the parity-fixture loader; future sprint adds the heightmap bake path.
pub fn upload_base_normal(
    render_state: &egui_wgpu::RenderState,
    width: u32,
    height: u32,
    rgba: &[u8],
) {
    let expected = (width as usize) * (height as usize) * 4;
    if rgba.len() != expected {
        warn!(
            got = rgba.len(),
            expected, "upload_base_normal: byte length mismatch"
        );
        return;
    }
    let device = &render_state.device;
    let queue = &render_state.queue;
    let mut renderer = render_state.renderer.write();
    let Some(res) = renderer.callback_resources.get_mut::<RenderResources>() else {
        warn!("upload_base_normal: no RenderResources (install() not run)");
        return;
    };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("terrain.normals"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: BASE_NORMAL_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    res.splat.normals_tex = tex;
    res.splat.normals_view = view;
    res.rebind(device);
    info!(width, height, "upload_base_normal: bound");
}

/// Upload a full specular map texture into the terrain bind group.
/// Same shape contract as [`upload_base_normal`]; the texture is
/// `Rgba8Unorm` where `.rgb` is colour and `.a` is the per-fragment
/// exponent coefficient (per FINDINGS §7.6 `specular_exp = a × 16.0`).
/// Caller sets `SplatUniforms.flags.w |= 2` (the `has_specular_tex`
/// bit).
#[allow(dead_code)] // Wired by the parity-fixture loader.
pub fn upload_specular(
    render_state: &egui_wgpu::RenderState,
    width: u32,
    height: u32,
    rgba: &[u8],
) {
    let expected = (width as usize) * (height as usize) * 4;
    if rgba.len() != expected {
        warn!(
            got = rgba.len(),
            expected, "upload_specular: byte length mismatch"
        );
        return;
    }
    let device = &render_state.device;
    let queue = &render_state.queue;
    let mut renderer = render_state.renderer.write();
    let Some(res) = renderer.callback_resources.get_mut::<RenderResources>() else {
        warn!("upload_specular: no RenderResources (install() not run)");
        return;
    };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("terrain.specular"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SPECULAR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    res.splat.specular_tex = tex;
    res.splat.specular_view = view;
    res.rebind(device);
    info!(width, height, "upload_specular: bound");
}

// ─── Composite pipeline API (Sprint 16 / D9 / ADR-039) ──────────────

/// Clamp + validate a requested composite RT size. Returns `None` for
/// degenerate inputs (`< 2 px` on either axis). The CPU bake stays
/// authoritative at full `texture_dims` regardless of this clamp —
/// the GPU preview merely loses sub-pixel detail past 4096².
///
/// Pure / GPU-free — exercised by unit tests.
pub fn resolve_composite_rt_size(requested: (u32, u32)) -> Option<(u32, u32)> {
    let (w, h) = requested;
    if w < 2 || h < 2 {
        return None;
    }
    Some((w.min(COMPOSITE_RT_CLAMP), h.min(COMPOSITE_RT_CLAMP)))
}

/// Allocate / refresh the composite render target + mask array at
/// the given physical pixel size. Resizes the mask array alongside
/// so per-tile sub-uploads land at the right dims. Re-registers the
/// egui texture id on resize and frees the old one to avoid renderer
/// handle leaks.
///
/// Returns the `egui::TextureId` the paint viewport (Sprint 16 /
/// Commit 3) should hand to `ui.painter().image(...)` to render the
/// composite into the 2D viewport. `None` on degenerate inputs or
/// when [`install`] hasn't run.
///
/// Idempotency: if the composite RT is already at the requested
/// (clamped) size, this is a no-op apart from returning the cached id.
pub fn ensure_composite_rt(
    render_state: &egui_wgpu::RenderState,
    requested: (u32, u32),
) -> Option<egui::TextureId> {
    let size = resolve_composite_rt_size(requested)?;
    let device = render_state.device.clone();
    let mut renderer = render_state.renderer.write();

    let (needs_realloc, old_id) = {
        let Some(res) = renderer.callback_resources.get_mut::<RenderResources>() else {
            warn!("ensure_composite_rt: no RenderResources (install() not run)");
            return None;
        };
        let same_size = matches!(&res.composite.rt, Some(rt) if rt.size == size);
        if same_size {
            return Some(
                res.composite
                    .rt
                    .as_ref()
                    .expect("matched on Some above")
                    .egui_texture_id,
            );
        }
        let old = res.composite.rt.take().map(|rt| rt.egui_texture_id);
        (true, old)
    };

    if let Some(id) = old_id {
        renderer.free_texture(&id);
    }
    let _ = needs_realloc;

    // Allocate the new RT.
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("composite.rt"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COMPOSITE_RT_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor {
        label: Some("composite.rt.view"),
        ..Default::default()
    });
    let egui_texture_id =
        renderer.register_native_texture(&device, &view, wgpu::FilterMode::Linear);
    info!(
        width = size.0,
        height = size.1,
        format = ?COMPOSITE_RT_FORMAT,
        "composite RT (re)allocated"
    );

    // Allocate the new 16-layer mask array at the matching dims.
    // wgpu zero-initialises textures on first use (its safe-by-default
    // policy — uninitialised reads are not undefined behaviour),
    // so the shader sees all-zero masks until the paint dispatcher
    // writes real bytes via `write_composite_layer_mask_tiles`. The
    // pre-Sprint-23 manual row-by-row zero-fill loop (16 layers ×
    // 4096 rows = 65,536 `queue.write_texture` calls) was redundant
    // and saturated wgpu's staging arena on Vega 8 iGPU — root cause
    // of the 16-SMU PaintLayer-entry OOM (Sprint 23 / T1 / H1+H4).
    let mask_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("composite.mask_array"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: COMPOSITE_MAX_LAYERS,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COMPOSITE_MASK_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mask_view = mask_tex.create_view(&wgpu::TextureViewDescriptor {
        label: Some("composite.mask_array.view"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });

    // Drop the borrow chain so we can take `&mut res` again to swap
    // the new resources in.
    let Some(res) = renderer.callback_resources.get_mut::<RenderResources>() else {
        renderer.free_texture(&egui_texture_id);
        return None;
    };
    res.composite.rt = Some(CompositeRt {
        tex,
        view,
        egui_texture_id,
        size,
    });
    res.composite.mask_tex = Some(mask_tex);
    res.composite.mask_view = Some(mask_view);

    // Rebuild the composite bind group with the new mask array view.
    let mask_view = res.composite.mask_view.as_ref().expect("just set above");
    res.composite.bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("composite.bg"),
        layout: &res.composite.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: res.composite.uniform_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&res.composite.slot_array_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&res.composite.slot_array_samp),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(mask_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(&res.composite.mask_samp),
            },
        ],
    });

    // Rebuild the terrain bind group so its composite RT view points
    // at the new RT (was the 1×1 dummy until now).
    res.rebind(&device);

    Some(egui_texture_id)
}

/// Upload one slot's 1024² diffuse into the composite slot array at
/// the given `layer_idx`. Mirrors [`upload_diffuse_layer`] for the
/// Sprint-9 / legacy 4-layer path but targets the 16-layer composite
/// side. Source `rgba` MUST be exactly `SLOT_COMPOSITE_DIM² × 4`
/// bytes; the caller resizes on import.
pub fn upload_composite_slot_diffuse(
    render_state: &egui_wgpu::RenderState,
    layer_idx: u32,
    rgba: &[u8],
) {
    if layer_idx >= COMPOSITE_MAX_LAYERS {
        warn!(
            layer = layer_idx,
            "upload_composite_slot_diffuse: layer out of range"
        );
        return;
    }
    let expected = (SLOT_COMPOSITE_DIM as usize) * (SLOT_COMPOSITE_DIM as usize) * 4;
    if rgba.len() != expected {
        warn!(
            got = rgba.len(),
            expected, "upload_composite_slot_diffuse: byte length mismatch; resize before calling"
        );
        return;
    }
    let renderer = render_state.renderer.read();
    let Some(res) = renderer.callback_resources.get::<RenderResources>() else {
        warn!("upload_composite_slot_diffuse: no RenderResources");
        return;
    };
    let queue = &render_state.queue;
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &res.composite.slot_array_tex,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: layer_idx,
            },
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(SLOT_COMPOSITE_DIM * 4),
            rows_per_image: Some(SLOT_COMPOSITE_DIM),
        },
        wgpu::Extent3d {
            width: SLOT_COMPOSITE_DIM,
            height: SLOT_COMPOSITE_DIM,
            depth_or_array_layers: 1,
        },
    );
    info!(layer = layer_idx, "composite slot diffuse uploaded");
}

/// Sub-upload one or more tiles of a layer's mask to the composite
/// mask array. Reads each tile via [`LayerMask::read_tile`] so the
/// fast path for `Uniform` tiles is a single byte-fill instead of a
/// heap allocation.
///
/// `layer_idx` must be `< COMPOSITE_MAX_LAYERS`. `tiles` may be the
/// output of [`LayerMask::dirty_tiles_since`]. Empty `tiles` is a
/// no-op. Out-of-array `(tx, ty)` clip silently.
///
/// **Pitfall (Sprint-16 prompt #6):** ALWAYS prefer this path over
/// uploading the entire mask. A full mask write at 4096² × 16 layers
/// is 256 MB per frame.
pub fn write_composite_layer_mask_tiles(
    render_state: &egui_wgpu::RenderState,
    layer_idx: u32,
    mask: &LayerMask,
    tiles: &[TileCoord],
) {
    if tiles.is_empty() {
        return;
    }
    if layer_idx >= COMPOSITE_MAX_LAYERS {
        warn!(
            layer = layer_idx,
            "write_composite_layer_mask_tiles: layer out of range"
        );
        return;
    }
    let renderer = render_state.renderer.read();
    let Some(res) = renderer.callback_resources.get::<RenderResources>() else {
        warn!("write_composite_layer_mask_tiles: no RenderResources");
        return;
    };
    let Some(rt) = res.composite.rt.as_ref() else {
        // No composite RT yet — nothing to upload to. The paint
        // viewport allocates the RT on its first frame; until then,
        // upload requests are a no-op.
        debug!(
            "write_composite_layer_mask_tiles: no composite RT — skipping {} tiles",
            tiles.len()
        );
        return;
    };
    let Some(mask_tex) = res.composite.mask_tex.as_ref() else {
        debug!("write_composite_layer_mask_tiles: no mask array — skipping");
        return;
    };
    let queue = &render_state.queue;
    let (rt_w, rt_h) = rt.size;
    let (mtx, mty) = mask.tile_grid_dims();

    let mut tile_buf = [0u8; TILE_PIXELS];
    for coord in tiles {
        debug_assert!(
            coord.tile_x < mtx && coord.tile_y < mty,
            "tile coord ({}, {}) outside mask tile grid ({mtx}, {mty})",
            coord.tile_x,
            coord.tile_y,
        );
        let tile_x_px = coord.tile_x * TILE_DIM;
        let tile_y_px = coord.tile_y * TILE_DIM;
        if tile_x_px >= rt_w || tile_y_px >= rt_h {
            // Composite RT may be smaller than the mask dims (the
            // 4096² clamp kicks in for >8 SMU maps). Skip tiles that
            // land entirely past the RT — the preview will be
            // approximate there.
            continue;
        }
        let copy_w = TILE_DIM.min(rt_w - tile_x_px);
        let copy_h = TILE_DIM.min(rt_h - tile_y_px);
        mask.read_tile(coord.tile_x, coord.tile_y, &mut tile_buf);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: mask_tex,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: tile_x_px,
                    y: tile_y_px,
                    z: layer_idx,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &tile_buf[..],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(TILE_DIM),
                rows_per_image: Some(TILE_DIM),
            },
            wgpu::Extent3d {
                width: copy_w,
                height: copy_h,
                depth_or_array_layers: 1,
            },
        );
        trace!(
            layer = layer_idx,
            tile_x = coord.tile_x,
            tile_y = coord.tile_y,
            "composite mask tile uploaded"
        );
    }
}

/// Push the latest composite uniforms (per-layer transform / tint /
/// opacity / active flag + RT dims) to the GPU. The
/// `TerrainCallback::prepare` path writes these per-frame when the
/// layered preview is active; this helper exists for the Sprint-17
/// Layers panel's outside-the-callback dispatch (e.g. an inspector
/// edit that needs immediate GPU sync without waiting on the next
/// paint).
#[allow(dead_code)] // Sprint 17 (Layers panel) wires this in.
pub fn update_composite_uniforms(render_state: &egui_wgpu::RenderState, cu: &CompositeU) {
    let renderer = render_state.renderer.read();
    let Some(res) = renderer.callback_resources.get::<RenderResources>() else {
        warn!("update_composite_uniforms: no RenderResources");
        return;
    };
    res.write_composite_uniforms(&render_state.queue, cu);
}

/// Encode the layered composite pass into the composite RT. Shared by
/// the 3D path (`TerrainCallback::prepare`) and the 2D paint-view path
/// (`CompositeCallback::prepare`) so both viewports see the same
/// composited preview without one-frame lag.
///
/// Returns silently when the composite RT isn't allocated yet — the
/// caller is responsible for `ensure_composite_rt` before dispatch.
fn encode_composite_pass(
    res: &RenderResources,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    cu: &CompositeU,
) {
    let Some(rt) = res.composite.rt.as_ref() else {
        return;
    };
    res.write_composite_uniforms(queue, cu);
    let mut cpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("composite.pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &rt.view,
            resolve_target: None,
            ops: wgpu::Operations {
                // Clear to fully-opaque mid-grey so a no-layer pass
                // produces the same background tone the CPU bake's
                // `bg = 0.18` flattens against.
                load: wgpu::LoadOp::Clear(wgpu::Color {
                    r: 0.18,
                    g: 0.18,
                    b: 0.18,
                    a: 1.0,
                }),
                store: wgpu::StoreOp::Store,
            },
            depth_slice: None,
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    cpass.set_pipeline(&res.composite.pipeline);
    cpass.set_bind_group(0, &res.composite.bind_group, &[]);
    cpass.draw(0..3, 0..1);
    drop(cpass);
    trace!(rt_size = ?rt.size, "composite RT recomposited");
}

/// Standalone composite-pass dispatcher for the 2D paint viewport
/// (`Tool::PaintLayer`). The 3D path's `TerrainCallback::prepare`
/// re-runs the same pass per frame and writes the same uniforms, so
/// dropping this into `central_paint_layer` keeps the 2D viewport in
/// sync with live mask, opacity, tint, and transform edits.
///
/// `paint()` is a deliberate no-op: this callback only writes the
/// composite RT. egui samples the RT separately via
/// `ui.painter().image(composite_rt_id, …)` after the prepare hook.
pub struct CompositeCallback {
    pub composite: CompositeU,
}

impl egui_wgpu::CallbackTrait for CompositeCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = resources.get::<RenderResources>() else {
            return Vec::new();
        };
        encode_composite_pass(res, queue, encoder, &self.composite);
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        _render_pass: &mut wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        // Intentionally empty — the composite RT is sampled by an
        // egui::Painter::image() call after this callback returns.
    }
}

/// Push the latest splat uniforms (active mask, scales, mults,
/// diffuse-in-alpha flag) to the GPU. The TerrainCallback also
/// writes these every frame via `prepare`, so this helper is only
/// used when state changes outside the render loop (e.g. an inspector
/// edit that needs to take effect before the next callback fires).
#[allow(dead_code)] // D5 may call this; the callback path covers per-frame.
pub fn update_splat_uniforms(render_state: &egui_wgpu::RenderState, su: &SplatUniforms) {
    let renderer = render_state.renderer.read();
    let Some(res) = renderer.callback_resources.get::<RenderResources>() else {
        warn!("update_splat_uniforms: no RenderResources");
        return;
    };
    res.write_splat_uniforms(&render_state.queue, su);
}

/// Sprint-14 / C9 / ADR-042 — water plane draw payload. `None` skips
/// the water pipeline entirely (matches `WaterMode::None`). When
/// `Some`, the pre-multiplied RGBA + alpha scale drive the quad's
/// fragment output; `extent_x` / `extent_z` size the quad.
#[derive(Debug, Clone, Copy)]
pub struct WaterDraw {
    /// Pre-multiplied RGBA — `(r·a, g·a, b·a, a)`.
    pub surface_rgba: [f32; 4],
    pub extent_x: f32,
    pub extent_z: f32,
    /// `1.0` when `Tool::Water` is active (full opacity); `0.5` for
    /// cross-tool ghosting. The shader multiplies the fragment output
    /// by this scalar — pre-multiplied scaling preserves the
    /// `(kα·c, kα·c, kα·c, kα)` invariant.
    pub alpha_scale: f32,
}

pub struct TerrainCallback {
    pub view_proj: [[f32; 4]; 4],
    pub max_height: f32,
    /// Sprint-14 follow-up: world Y at raw heightmap value 0. Negative
    /// values let the heightmap dip below BAR's water plane at Y = 0.
    pub min_height: f32,
    pub world_extent_x: f32,
    pub world_extent_z: f32,
    pub splat: SplatUniforms,
    /// Sprint 25 / R1 / ADR-038 — world-space camera eye position.
    /// Written into `SplatUniforms.camera_pos` at `prepare()` time so
    /// the fragment shader can build the Blinn-Phong half-vector
    /// `halfDir = normalize(sun_dir + normalize(eye - worldPos))`.
    /// We stash it on the callback (not in `App`'s persistent
    /// `SplatUniforms`) because the eye moves every frame.
    pub camera_pos: [f32; 3],
    /// Sprint 16 / D9 / ADR-039 — when `Some`, `prepare()` encodes the
    /// layered composite pipeline pass into the composite RT BEFORE
    /// the terrain pass, and the terrain shader samples the composite
    /// RT as its diffuse base via the `params2.y = 1.0` flag. `None`
    /// keeps the pre-Sprint-16 splat-based diffuse path active.
    pub composite: Option<CompositeU>,
    /// Sprint-13 (ADR-037) — pre-sorted, GPU-encoded marker instances.
    /// `central()` builds these by walking the start-positions / metal-
    /// spots / geo-vents / brush rings and pushing one `Marker` per
    /// glyph into a frame-local `MarkerBatch`, then calling
    /// `sort_back_to_front(view)` + `into_instances()`.
    pub marker_instances: Vec<crate::ui::markers::MarkerInstanceGpu>,
    /// Logical viewport size — drives the marker shader's
    /// screen-space radius conversion (radius_px is in logical units).
    pub viewport_size: [f32; 2],
    /// Sprint-13 / Phase 5 — interleaved `LineList` vertex pairs for
    /// the line pipeline (symmetry axes + geo-vent plumes). Each pair
    /// of consecutive verts forms one segment.
    pub line_vertices: Vec<LineVertex>,
    /// Sprint-14 / C9 — optional water plane draw. `None` when
    /// `Project.water_mode == WaterMode::None` (no water sub-table in
    /// the emitted mapinfo, no plane in the preview).
    pub water: Option<WaterDraw>,
}

impl TerrainCallback {
    #[allow(clippy::too_many_arguments)] // Each arg is load-bearing for the offscreen pass.
    pub fn new(
        camera: &OrbitCamera,
        rect: egui::Rect,
        max_height: f32,
        min_height: f32,
        world_extent_x: f32,
        world_extent_z: f32,
        splat: SplatUniforms,
        marker_instances: Vec<crate::ui::markers::MarkerInstanceGpu>,
        viewport_size: [f32; 2],
        line_vertices: Vec<LineVertex>,
        water: Option<WaterDraw>,
        composite: Option<CompositeU>,
    ) -> Self {
        let aspect = (rect.width() / rect.height()).max(0.0001);
        let eye = camera.eye();
        Self {
            view_proj: camera.view_proj_matrix(aspect).to_cols_array_2d(),
            max_height,
            min_height,
            world_extent_x,
            world_extent_z,
            splat,
            camera_pos: [eye.x, eye.y, eye.z],
            marker_instances,
            viewport_size,
            line_vertices,
            water,
            composite,
        }
    }
}

/// Clear colour for the offscreen colour attachment. Dark navy reads
/// as a neutral 3D-viewport sky and produces enough contrast against
/// both light and dark egui themes. Premultiplied — the alpha is 1.0
/// so RGB = pre/post multiplied are identical for this opaque value.
const OFFSCREEN_CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.04,
    g: 0.05,
    b: 0.07,
    a: 1.0,
};

impl egui_wgpu::CallbackTrait for TerrainCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = resources.get::<RenderResources>() else {
            return Vec::new();
        };

        // Step 1 — terrain uniforms. `params2.x` carries
        // `min_height` (Sprint 14); `params2.y` carries the Sprint-16
        // composite-RT diffuse-source flag — `1.0` when the project
        // has a non-empty layer stack AND the composite RT is allocated.
        let use_composite_rt = self.composite.is_some() && res.composite.rt.is_some();
        res.write_uniforms(
            queue,
            &Uniforms {
                view_proj: self.view_proj,
                params: [
                    self.max_height.max(1.0),
                    ELMOS_PER_PIXEL,
                    self.world_extent_x.max(1.0),
                    self.world_extent_z.max(1.0),
                ],
                params2: [
                    self.min_height,
                    if use_composite_rt { 1.0 } else { 0.0 },
                    0.0,
                    0.0,
                ],
            },
        );
        // Sprint 25 / R1 / ADR-038 — inject the per-frame camera eye
        // into the splat uniforms. The App owns the persistent
        // `SplatUniforms` (sun direction, lighting colours, tex
        // multipliers, flags), but the camera moves every frame and
        // doesn't belong in that struct's persistent state.
        let mut splat = self.splat;
        splat.camera_pos = [self.camera_pos[0], self.camera_pos[1], self.camera_pos[2], 1.0];
        res.write_splat_uniforms(queue, &splat);

        // Step 1d — Sprint 16 / D9 / ADR-039 — encode the layered
        // composite pass into the composite RT. The terrain pass that
        // follows samples from the same RT (via binding 7) so the
        // composite must land first. Skipped when the project has no
        // layer stack OR the composite RT hasn't been allocated yet.
        // The 2D paint viewport runs the same encode via
        // `CompositeCallback` so live edits stay synced across both
        // viewports.
        if let Some(cu) = self.composite.as_ref() {
            encode_composite_pass(res, queue, encoder, cu);
        }

        // Step 1b — marker uniforms + instance upload (Sprint 13 /
        // ADR-037). Cap instance count to the pre-allocated capacity
        // so we never overrun the storage buffer; log + drop the tail
        // if exceeded (won't happen at Sprint 13's expected loads).
        let marker_count = (self.marker_instances.len() as u32).min(MARKER_INSTANCE_CAPACITY);
        if (self.marker_instances.len() as u32) > MARKER_INSTANCE_CAPACITY {
            warn!(
                requested = self.marker_instances.len(),
                capacity = MARKER_INSTANCE_CAPACITY,
                "marker instance buffer exceeded; tail dropped"
            );
        }
        queue.write_buffer(
            &res.marker.uniform_buf,
            0,
            bytemuck::bytes_of(&MarkerU {
                view_proj: self.view_proj,
                viewport_size: self.viewport_size,
                _pad: [0.0, 0.0],
            }),
        );
        if marker_count > 0 {
            queue.write_buffer(
                &res.marker.instance_buf,
                0,
                bytemuck::cast_slice(&self.marker_instances[..marker_count as usize]),
            );
        }

        // Step 1c — line vertex upload (Sprint 13 / Phase 5). LineList
        // topology: every consecutive pair is one segment. Cap at the
        // pre-allocated capacity; the marker uniform buffer also
        // backs the line pipeline (shared `view_proj`) so we don't
        // need a separate uniform write.
        let line_vert_count = (self.line_vertices.len() as u32).min(LINE_VERTEX_CAPACITY);
        if (self.line_vertices.len() as u32) > LINE_VERTEX_CAPACITY {
            warn!(
                requested = self.line_vertices.len(),
                capacity = LINE_VERTEX_CAPACITY,
                "line vertex buffer exceeded; tail dropped"
            );
        }
        if line_vert_count > 0 {
            queue.write_buffer(
                &res.line.vertex_buf,
                0,
                bytemuck::cast_slice(&self.line_vertices[..line_vert_count as usize]),
            );
        }
        trace!(
            markers = marker_count,
            line_verts = line_vert_count,
            "TerrainCallback::prepare"
        );

        // Step 2 — encode the offscreen pass.
        //
        // We need an offscreen RT (allocated by `central()` via
        // `ensure_offscreen`), a grid (built by `upload_heightmap`), and
        // a real heightmap. Any missing → bail and let the previous
        // frame's image stay on screen this frame (pitfall #12: no
        // green flash on resize).
        let Some(offscreen) = res.offscreen.as_ref() else {
            return Vec::new();
        };
        let Some(grid) = res.grid.as_ref() else {
            return Vec::new();
        };
        if res.heightmap.is_none() {
            return Vec::new();
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("offscreen.terrain"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: offscreen.color_view(),
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(OFFSCREEN_CLEAR_COLOR),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: offscreen.depth_view(),
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        // Terrain pipeline writes depth.
        pass.set_pipeline(&res.pipeline);
        pass.set_bind_group(0, &res.bind_group, &[]);
        pass.set_index_buffer(grid.index_buf.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..grid.index_count, 0, 0..1);

        // Sprint-14 / C9 — Water plane. Depth-tests against terrain
        // (cliffs above Y=0 occlude the plane); doesn't write depth
        // (translucent, blend order owned CPU-side: terrain → water →
        // lines → markers).
        if let Some(w) = &self.water {
            queue.write_buffer(
                &res.water.uniform_buf,
                0,
                bytemuck::bytes_of(&WaterU {
                    view_proj: self.view_proj,
                    surface_rgba: w.surface_rgba,
                    extent: [w.extent_x, w.extent_z, 0.0, w.alpha_scale],
                }),
            );
            pass.set_pipeline(&res.water.pipeline);
            pass.set_bind_group(0, &res.water.bind_group, &[]);
            pass.draw(0..4, 0..1);
            trace!(
                surface_rgba = ?w.surface_rgba,
                extent = ?(w.extent_x, w.extent_z),
                alpha_scale = w.alpha_scale,
                "water plane drawn"
            );
        }

        // Line pipeline depth-tests against terrain but doesn't write
        // depth. Drawn before markers so marker glyphs land on top of
        // any line crossings at the same depth (e.g. a brush ring
        // through a symmetry axis).
        if line_vert_count > 0 {
            pass.set_pipeline(&res.line.pipeline);
            pass.set_bind_group(0, &res.line.bind_group, &[]);
            pass.set_vertex_buffer(0, res.line.vertex_buf.slice(..));
            pass.draw(0..line_vert_count, 0..1);
        }

        // Marker pipeline depth-tests against terrain but doesn't write
        // depth (back-to-front sort owns translucent ordering). Skip
        // the draw entirely when the batch is empty — saves a state
        // change and one bind-group set per idle frame.
        if marker_count > 0 {
            pass.set_pipeline(&res.marker.pipeline);
            pass.set_bind_group(0, &res.marker.bind_group, &[]);
            pass.draw(0..4, 0..marker_count);
        }

        // `pass` drops here — releases the encoder for egui's own pass
        // and any other callbacks. We share `encoder` with egui per
        // pitfall #1, so we DO NOT `encoder.finish()` ourselves.
        drop(pass);

        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        _pass: &mut wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        // ADR-037 / Sprint 13 Phase 2 — the terrain pass now renders to
        // the offscreen RT inside `prepare()`. `central()` composites
        // the result by drawing `OffscreenTarget::egui_texture_id` into
        // the central viewport rect via `ui.painter().image(...)`. This
        // `paint()` runs inside egui's own pass (which has no depth
        // attachment) and intentionally does nothing.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> OrbitCamera {
        OrbitCamera::framing(8192.0, 8192.0)
    }

    #[test]
    fn world_to_screen_round_trip_with_screen_to_world() {
        let camera = cam();
        let rect = glam::Vec2::new(800.0, 600.0);
        // Map center on y=0 plane.
        let world = glam::Vec3::new(4096.0, 0.0, 4096.0);
        let screen = world_to_screen(world, rect, &camera).expect("center should be on screen");
        let back = screen_to_world_y0(screen, rect, &camera).expect("inverse projects back");
        // f32 round-trip through perspective matrices loses a couple
        // elmos; a few units of slack is fine for a UI hit-test helper.
        assert!((back.x - world.x).abs() < 5.0, "got back.x = {}", back.x);
        assert!((back.z - world.z).abs() < 5.0, "got back.z = {}", back.z);
    }

    #[test]
    fn world_to_screen_returns_none_for_behind_camera_point() {
        // Phase 6 (Sprint 13 / ADR-037): the only None case is
        // behind-camera (clip.w <= 0). Pick a world point behind the
        // camera eye by walking back along the look direction.
        let camera = cam();
        let rect = glam::Vec2::new(800.0, 600.0);
        let eye = camera.eye();
        let look_dir = (camera.target - eye).normalize();
        // 100 elmos behind the eye, opposite the look direction.
        let behind = eye - look_dir * 100.0;
        assert!(
            world_to_screen(behind, rect, &camera).is_none(),
            "behind-camera should project to None"
        );
    }

    #[test]
    fn world_to_screen_returns_some_for_off_screen_in_front_of_camera() {
        // Phase 6 (Sprint 13 / ADR-037): off-screen points (NDC
        // outside [-1, 1]) are no longer rejected — only behind-camera.
        // Default framing puts the eye in the +x+y+z octant looking
        // back toward the map centre; a point past the origin
        // (negative side, opposite to the eye) is well in front of
        // the camera but projects to NDC well outside the rect.
        let camera = cam();
        let rect = glam::Vec2::new(800.0, 600.0);
        let off = glam::Vec3::new(-200_000.0, 0.0, 0.0);
        let r = world_to_screen(off, rect, &camera);
        assert!(
            r.is_some(),
            "off-screen-but-in-front-of-camera should return Some"
        );
        // Sanity: the result should be well outside [0, 800] × [0,
        // 600] — the relaxed semantics let callers receive the
        // projected coordinate and clip via painter_at(rect).
        let p = r.unwrap();
        assert!(
            p.x < 0.0 || p.x > 800.0 || p.y < 0.0 || p.y > 600.0,
            "off-screen result {p:?} should be outside the rect"
        );
    }

    #[test]
    fn world_to_screen_center_lands_near_rect_center() {
        let camera = cam();
        let rect = glam::Vec2::new(800.0, 600.0);
        let screen =
            world_to_screen(camera.target, rect, &camera).expect("camera target is visible");
        let cx = rect.x * 0.5;
        let cy = rect.y * 0.5;
        assert!((screen.x - cx).abs() < 50.0, "screen.x = {}", screen.x);
        assert!((screen.y - cy).abs() < 50.0, "screen.y = {}", screen.y);
    }

    #[test]
    fn default_splat_uniforms_match_engine_defaults() {
        // FINDINGS §1.6 — splats default tex_scales=0.02, tex_mults=1.0.
        let su = SplatUniforms::default();
        assert_eq!(su.tex_scales, [0.02; 4]);
        assert_eq!(su.tex_mults, [1.0; 4]);
        // No slot bound on a fresh project → shader falls back to the
        // biome gradient.
        assert_eq!(su.flags[0], 0);
        // ADR-034 reserved — placeholder default is off.
        assert_eq!(su.flags[1], 0);
    }

    #[test]
    fn default_ground_ambient_pre_dimmed_by_intensity_mult() {
        // FINDINGS §7.1 — `SMF_INTENSITY_MULT = 210/255` is applied
        // CPU-side. Verify the default ambient is the dimmed 0.5 grey.
        let su = SplatUniforms::default();
        let expected = 0.5 * (210.0 / 255.0);
        for c in &su.ground_ambient[..3] {
            assert!((c - expected).abs() < 1e-6, "got {c}, expected {expected}");
        }
    }

    #[test]
    fn default_sun_dir_w_is_one() {
        // FINDINGS §1.4 / pitfall #18 — sunDir.w defaults to 1.0, NOT
        // 1e9. The shader doesn't consume `.w` but pinning the constant
        // here catches a future regression in `default_sun_dir`.
        let su = SplatUniforms::default();
        assert!((su.sun_dir[3] - 1.0).abs() < 1e-6);
        // Direction itself should be normalized.
        let m = (su.sun_dir[0].powi(2) + su.sun_dir[1].powi(2) + su.sun_dir[2].powi(2)).sqrt();
        assert!(
            (m - 1.0).abs() < 1e-6,
            "sun_dir.xyz not normalized: |m| = {m}"
        );
    }

    #[test]
    fn slot_layer_count_matches_splat_channel_count() {
        // The texture array is sized to SLOT_LAYER_COUNT = 4 because
        // the splat distribution is RGBA (4 channels). A future
        // mismatch is a load-bearing bug — D5 indexes by channel.
        assert_eq!(SLOT_LAYER_COUNT, 4);
    }

    /// Sprint 25 / R1 / ADR-038 — `SplatUniforms` MUST match the WGSL
    /// `terrain.wgsl::SplatU` block layout exactly, otherwise the
    /// per-frame uniform write uploads garbage into the shader's
    /// bindings.
    ///
    /// Expected layout:
    /// - `tex_scales`: 16 B (vec4<f32>)
    /// - `tex_mults`:  16 B (vec4<f32>)
    /// - `flags`:      16 B (vec4<u32>)
    /// - `sun_dir`:    16 B (vec4<f32>)
    /// - `ground_ambient`:  16 B
    /// - `ground_diffuse`:  16 B
    /// - `ground_specular`: 16 B (Sprint 25)
    /// - `camera_pos`:      16 B (Sprint 25)
    ///
    /// Total: 128 B.
    #[test]
    fn splat_uniforms_size_matches_wgsl_layout() {
        assert_eq!(
            std::mem::size_of::<SplatUniforms>(),
            128,
            "SplatUniforms layout drift — terrain.wgsl expects 128 bytes"
        );
    }

    #[test]
    fn default_ground_specular_matches_engine_default() {
        // FINDINGS §1.4 — `lighting.groundSpecularColor = (0.1, 0.1,
        // 0.1)`; `lighting.specularExponent = 100.0`.
        let su = SplatUniforms::default();
        assert_eq!(su.ground_specular, [0.1, 0.1, 0.1, 100.0]);
    }

    #[test]
    fn default_camera_pos_is_origin() {
        // Camera eye lands at prepare() time; the default is at origin
        // so unit tests that don't construct a TerrainCallback see a
        // deterministic uniform.
        let su = SplatUniforms::default();
        assert_eq!(su.camera_pos, [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn smf_intensity_mult_constant_pinned() {
        // FINDINGS §7.1 — engine defines `SMF_INTENSITY_MULT = 210/255
        // ≈ 0.8235294...`. Pinning the constant here so a future drift
        // is caught at compile time. Source:
        // `cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl:4`.
        assert!((SMF_INTENSITY_MULT - 210.0 / 255.0).abs() < 1e-6);
        assert!((SMF_INTENSITY_MULT - 0.8235294).abs() < 1e-4);
    }

    #[test]
    fn texture_format_constants_pinned() {
        // Sprint 25 / R1 / ADR-038 — the new texture bindings carry
        // RGBA8 in unsigned-normalised form (the shader does `* 2 - 1`
        // for the normal-map decode). A future change to a signed or
        // BC-compressed format is a coordinated WGSL + sampler edit.
        assert_eq!(BASE_NORMAL_FORMAT, wgpu::TextureFormat::Rgba8Unorm);
        assert_eq!(SPECULAR_FORMAT, wgpu::TextureFormat::Rgba8Unorm);
        assert_eq!(SLOT_NORMAL_FORMAT, wgpu::TextureFormat::Rgba8Unorm);
    }

    /// Sprint 25 / R1 / ADR-038 — parse + validate `terrain.wgsl` at
    /// `cargo test` time using wgpu's re-exported naga. wgpu only
    /// compiles the WGSL at GPU `create_shader_module` time, which we
    /// can't reach in headless CI; this test catches WGSL syntax /
    /// type / binding-layout drift before the user runs the app.
    #[test]
    fn terrain_wgsl_parses_and_validates() {
        use wgpu::naga::front::wgsl;
        use wgpu::naga::valid::{Capabilities, ValidationFlags, Validator};

        let src = include_str!("terrain.wgsl");
        let module = match wgsl::parse_str(src) {
            Ok(m) => m,
            Err(e) => panic!("terrain.wgsl parse failed:\n{}", e.emit_to_string(src)),
        };
        let mut validator = Validator::new(ValidationFlags::all(), Capabilities::all());
        if let Err(e) = validator.validate(&module) {
            panic!("terrain.wgsl validate failed:\n{e:?}");
        }
    }

    // ---- Phase 1 (Sprint 13 / ADR-037): offscreen RT size resolution ----

    #[test]
    fn offscreen_size_passes_through_in_range() {
        // Typical 720p viewport — well under the clamp.
        assert_eq!(resolve_offscreen_size((1280, 720)), Some((1280, 720)));
    }

    #[test]
    fn offscreen_size_clamps_each_axis_to_2048() {
        // 4K display × pixels_per_point=2 = 8K wide; clamp engages.
        assert_eq!(resolve_offscreen_size((8192, 8192)), Some((2048, 2048)));
        // Asymmetric: only the over-cap axis is clamped.
        assert_eq!(resolve_offscreen_size((4096, 1080)), Some((2048, 1080)));
        assert_eq!(resolve_offscreen_size((1024, 3000)), Some((1024, 2048)));
    }

    #[test]
    fn offscreen_size_skips_degenerate_inputs() {
        // Zero on either axis = no allocation.
        assert_eq!(resolve_offscreen_size((0, 720)), None);
        assert_eq!(resolve_offscreen_size((1280, 0)), None);
        assert_eq!(resolve_offscreen_size((0, 0)), None);
        // Single-pixel axis also rejected (a 1-px depth target produces
        // no useful image and risks driver corner cases on some
        // backends).
        assert_eq!(resolve_offscreen_size((1, 100)), None);
        assert_eq!(resolve_offscreen_size((100, 1)), None);
        // Two pixels is the smallest accepted size.
        assert_eq!(resolve_offscreen_size((2, 2)), Some((2, 2)));
    }

    #[test]
    fn offscreen_size_clamp_constant_is_2048() {
        // Sprint 13 perf budget on iGPU caps each axis at 2048 px to
        // hold the offscreen RT under ~32 MB. Pinning here so a change
        // to OFFSCREEN_CLAMP is forced through code review.
        assert_eq!(OFFSCREEN_CLAMP, 2048);
    }

    #[test]
    fn offscreen_formats_match_adr_037() {
        assert_eq!(
            OFFSCREEN_COLOR_FORMAT,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            "ADR-037: colour format pinned to Rgba8UnormSrgb"
        );
        assert_eq!(
            OFFSCREEN_DEPTH_FORMAT,
            wgpu::TextureFormat::Depth32Float,
            "ADR-037: depth format pinned to Depth32Float"
        );
    }

    // ---- Phase 3 (Sprint 13 / ADR-037): camera near/far auto-tune ----

    fn at_distance(distance: f32) -> OrbitCamera {
        let mut c = OrbitCamera::framing(8192.0, 8192.0);
        c.distance = distance;
        c
    }

    #[test]
    fn camera_near_far_at_typical_distance() {
        // 16-SMU map default framing → distance ≈ 8192 * 1.4 = 11468.8.
        // Near = distance * 0.01 = 114.7; Far = distance * 4 = 45875.
        // Ratio = ~400.
        let c = OrbitCamera::framing(8192.0, 8192.0);
        let (near, far) = c.near_far();
        let expected_near = c.distance * 0.01;
        let expected_far = c.distance * 4.0;
        assert!(
            (near - expected_near).abs() < 1e-3,
            "near = {near}, expected ≈ {expected_near}"
        );
        assert!(
            (far - expected_far).abs() < 1e-3,
            "far = {far}, expected ≈ {expected_far}"
        );
    }

    #[test]
    fn camera_near_far_close_zoom_clamps_near_to_50() {
        // Distance = 100 elmos (zoomed in tight). distance * 0.01 = 1.0
        // which would shred depth precision; the 50-elmo floor kicks in.
        let c = at_distance(100.0);
        let (near, _far) = c.near_far();
        assert!(
            (near - 50.0).abs() < 1e-6,
            "near should be clamped to 50.0, got {near}"
        );
    }

    #[test]
    fn camera_near_far_zoom_out_keeps_ratio() {
        // Wide pull-back (50 000 elmos). near = 500, far = 200 000;
        // ratio = 400 — within float depth precision for 32-bit depth.
        let c = at_distance(50_000.0);
        let (near, far) = c.near_far();
        assert!((near - 500.0).abs() < 1e-3, "near = {near}");
        assert!((far - 200_000.0).abs() < 1e-3, "far = {far}");
        let ratio = far / near;
        assert!((ratio - 400.0).abs() < 1e-3, "ratio = {ratio}");
    }

    #[test]
    fn camera_near_far_close_zoom_keeps_minimum_far_distance() {
        // Even when near gets clamped to 50, far stays at >= 100 ×
        // near so close-up sculpting still sees the whole hill.
        let c = at_distance(100.0);
        let (near, far) = c.near_far();
        assert!(far >= near * 100.0 - 1.0, "far / near ratio too tight");
    }

    #[test]
    fn view_proj_matrix_uses_auto_tuned_near_far() {
        // Regression pin: view_proj_matrix should produce the same
        // matrix as if you'd manually plugged near_far() into a
        // perspective_lh call.
        let c = OrbitCamera::framing(8192.0, 8192.0);
        let (near, far) = c.near_far();
        let aspect = 16.0 / 9.0;
        let expected = Mat4::perspective_lh(c.fov_y, aspect, near, far)
            * Mat4::look_at_lh(c.eye(), c.target, Vec3::Y);
        let actual = c.view_proj_matrix(aspect);
        for (col, (a, e)) in actual
            .to_cols_array()
            .iter()
            .zip(expected.to_cols_array().iter())
            .enumerate()
        {
            assert!(
                (a - e).abs() < 1e-4,
                "col {col}: actual = {a}, expected = {e}"
            );
        }
    }

    // ─── Sprint 14 / C9 — water plane uniform layout ────────

    /// `WaterU` is `#[repr(C)]` + `bytemuck::Pod`. Its byte size must
    /// match the WGSL `WaterU` block layout exactly, otherwise the
    /// uniform write at `prepare()` time uploads garbage data into
    /// the shader's bindings.
    ///
    /// Expected layout:
    /// - `view_proj`: 64 B (4×4 f32)
    /// - `surface_rgba`: 16 B (f32×4)
    /// - `extent`: 16 B (f32×4)
    ///
    /// Total: 96 B.
    #[test]
    fn water_uniform_size_matches_wgsl_layout() {
        assert_eq!(
            std::mem::size_of::<WaterU>(),
            96,
            "WaterU layout drift — water.wgsl expects 96 bytes"
        );
    }

    // ─── Sprint 16 / D9 — composite uniform + RT sizing ────────

    /// `CompositeU` byte size MUST match the WGSL `composite.wgsl::
    /// CompositeU` layout exactly — `dims` (16 B) + `[LayerU; 16]` ×
    /// 64 B = 1040 B. A drift here uploads garbage to the GPU.
    #[test]
    fn composite_uniform_size_matches_wgsl_layout() {
        // LayerU = 4 × vec4<f32> = 64 B.
        assert_eq!(std::mem::size_of::<CompositeLayerU>(), 64);
        // CompositeU = vec4<f32> + array<LayerU, 16> = 16 + 1024 = 1040 B.
        assert_eq!(std::mem::size_of::<CompositeU>(), 1040);
    }

    #[test]
    fn composite_layer_u_default_is_inactive_identity() {
        let l = CompositeLayerU::default();
        assert_eq!(l.rot_mirror, [1.0, 1.0, 1.0, 0.0]);
        assert_eq!(l.offset, [0.0; 4]);
        assert_eq!(l.params[3], 0.0, "default active flag is OFF");
        assert_eq!(l.tint[0..3], [1.0, 1.0, 1.0]);
    }

    #[test]
    fn composite_layer_u_from_identity_transform() {
        let t = barme_core::LayerTransform::default();
        let c = barme_core::LayerColor::default();
        let lu = CompositeLayerU::from_layer(&t, &c, 1.0, true);
        // Identity: mirror = (+1, +1), rotation = 0 → cos=1, sin=0.
        assert_eq!(lu.rot_mirror, [1.0, 1.0, 1.0, 0.0]);
        // inv_scale = 1, opacity = 1, brightness = 0, active = 1.
        assert_eq!(lu.params, [1.0, 1.0, 0.0, 1.0]);
        assert_eq!(lu.offset, [0.0; 4]);
    }

    #[test]
    fn composite_layer_u_from_mirror_and_rotation_packs_correctly() {
        let t = barme_core::LayerTransform {
            mirror_x: true,
            mirror_y: false,
            rotation_rad: std::f32::consts::FRAC_PI_2, // 90°
            offset_elmos: [100.0, -200.0],
            scale: 2.0,
        };
        let c = barme_core::LayerColor::default();
        let lu = CompositeLayerU::from_layer(&t, &c, 0.5, true);
        // mirror_x = -1, mirror_y = +1, cos(π/2) ≈ 0, sin(π/2) ≈ 1.
        assert_eq!(lu.rot_mirror[0], -1.0);
        assert_eq!(lu.rot_mirror[1], 1.0);
        assert!((lu.rot_mirror[2] - 0.0).abs() < 1e-5);
        assert!((lu.rot_mirror[3] - 1.0).abs() < 1e-5);
        // offset elmos hoisted into .xy.
        assert_eq!(lu.offset[0], 100.0);
        assert_eq!(lu.offset[1], -200.0);
        // inv_scale = 1 / 2 = 0.5.
        assert!((lu.params[0] - 0.5).abs() < 1e-6);
        assert_eq!(lu.params[1], 0.5);
        assert_eq!(lu.params[3], 1.0);
    }

    #[test]
    fn composite_layer_u_inactive_when_visible_false_passed() {
        let t = barme_core::LayerTransform::default();
        let c = barme_core::LayerColor::default();
        let lu = CompositeLayerU::from_layer(&t, &c, 1.0, false);
        assert_eq!(lu.params[3], 0.0, "inactive layer skipped by shader");
    }

    #[test]
    fn composite_rt_size_passes_through_in_range() {
        // Typical 8-SMU = 4096² — fits exactly at the clamp.
        assert_eq!(resolve_composite_rt_size((4096, 4096)), Some((4096, 4096)));
        assert_eq!(resolve_composite_rt_size((2048, 1024)), Some((2048, 1024)));
    }

    #[test]
    fn composite_rt_size_clamps_each_axis_to_4096() {
        // 16-SMU = 8192² — clamp engages on both axes.
        assert_eq!(resolve_composite_rt_size((8192, 8192)), Some((4096, 4096)));
        // Asymmetric: only the over-cap axis is clamped.
        assert_eq!(resolve_composite_rt_size((6000, 2048)), Some((4096, 2048)));
        assert_eq!(resolve_composite_rt_size((1024, 5000)), Some((1024, 4096)));
    }

    #[test]
    fn composite_rt_size_skips_degenerate_inputs() {
        assert_eq!(resolve_composite_rt_size((0, 4096)), None);
        assert_eq!(resolve_composite_rt_size((4096, 0)), None);
        assert_eq!(resolve_composite_rt_size((1, 4096)), None);
        assert_eq!(resolve_composite_rt_size((4096, 1)), None);
        assert_eq!(resolve_composite_rt_size((2, 2)), Some((2, 2)));
    }

    #[test]
    fn composite_rt_clamp_constant_is_4096() {
        // Pinned by ADR-039 — 4096² is the cap where every reasonable
        // wgpu backend agrees a 2D RT works. Bumping this requires a
        // memory-budget review.
        assert_eq!(COMPOSITE_RT_CLAMP, 4096);
    }

    #[test]
    fn composite_max_layers_is_sixteen() {
        // The 16-layer cap is hard-coded into composite.wgsl's
        // `MAX_LAYERS` and the `array<LayerU, 16>` size. Changing this
        // is a coordinated WGSL + uniform-layout edit.
        assert_eq!(COMPOSITE_MAX_LAYERS, 16);
    }

    /// `WaterU` is `bytemuck::Pod`; constructing one and round-tripping
    /// it through `bytemuck::bytes_of` should produce 96 bytes whose
    /// f32 view matches the original.
    #[test]
    fn water_uniform_round_trips_through_pod() {
        let u = WaterU {
            view_proj: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            surface_rgba: [0.5, 0.4, 0.3, 0.6],
            extent: [8192.0, 8192.0, 0.0, 1.0],
        };
        let bytes = bytemuck::bytes_of(&u);
        assert_eq!(bytes.len(), 96);
        let view: &[f32] = bytemuck::cast_slice(bytes);
        // Cell [3][3] of view_proj is at offset 15 (4*4 - 1 = 15
        // f32 entries before extent starts).
        assert_eq!(view[15], 1.0);
        // surface_rgba starts at offset 16.
        assert_eq!(view[16], 0.5);
        assert_eq!(view[19], 0.6);
        // extent starts at offset 20.
        assert_eq!(view[20], 8192.0);
        assert_eq!(view[22], 0.0);
        assert_eq!(view[23], 1.0);
    }
}
