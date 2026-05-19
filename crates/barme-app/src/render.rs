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

use barme_core::{DirtyRect, Heightmap, SPLAT_DIM, SplatDistribution};
use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu;
use eframe::wgpu;
use glam::{Mat4, Vec3};
use tracing::{info, trace, warn};
use wgpu::util::DeviceExt;

/// 8 elmos per heightmap pixel — `MapSize::ELMOS_PER_SMU / HEIGHTMAP_PER_SMU`.
pub const ELMOS_PER_PIXEL: f32 = 8.0;

const HEIGHTMAP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Uint;
const SPLAT_DISTR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const SLOT_DIFFUSE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

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

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    /// `[max_height, elmos_per_pixel, world_extent_x, world_extent_z]`.
    /// The world extents drive the splat-distribution UV math in the
    /// fragment stage (`uv = world_pos.xz / extent`).
    params: [f32; 4],
}

/// CPU mirror of the WGSL `SplatU` block. Field order MUST match
/// `terrain.wgsl::SplatU` exactly (`bytemuck::Pod` enforces no padding
/// gymnastics, but order is on us).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct SplatUniforms {
    pub tex_scales: [f32; 4],
    pub tex_mults: [f32; 4],
    /// `[active_slot_mask, diffuse_in_alpha, _, _]`. The mask bit `i`
    /// is set when channel `i` is bound to a slot (mirrors
    /// `Project.splat_config.channels[i].is_some()` from D5).
    /// `diffuse_in_alpha` plumbs ADR-034's high-pass workflow toggle
    /// through the uniform buffer; the shader treats it as a no-op
    /// this sprint.
    pub flags: [u32; 4],
    /// World-space to-sun direction. `.w` unused.
    pub sun_dir: [f32; 4],
    /// Pre-dimmed by `SMF_INTENSITY_MULT = 210/255` CPU-side (FINDINGS
    /// §7.1) so the WGSL stays clean. `.w` unused.
    pub ground_ambient: [f32; 4],
    pub ground_diffuse: [f32; 4],
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

struct Grid {
    index_buf: wgpu::Buffer,
    index_count: u32,
    /// Heightmap dims this grid was built for; we rebuild when they change.
    dims: (u32, u32),
}

struct HeightmapTex {
    tex: wgpu::Texture,
    view: wgpu::TextureView,
    dims: (u32, u32),
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
    #[allow(dead_code)]
    slot_array_tex: wgpu::Texture,
    slot_array_view: wgpu::TextureView,
    slot_array_samp: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
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
    grid: Option<Grid>,
    heightmap: Option<HeightmapTex>,
    splat: SplatResources,
}

impl RenderResources {
    fn write_uniforms(&self, queue: &wgpu::Queue, u: &Uniforms) {
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(u));
    }

    fn write_splat_uniforms(&self, queue: &wgpu::Queue, su: &SplatUniforms) {
        queue.write_buffer(&self.splat.uniform_buf, 0, bytemuck::bytes_of(su));
    }

    fn rebind(&mut self, device: &wgpu::Device) {
        let view = self.heightmap.as_ref().map(|h| &h.view).unwrap_or_else(|| {
            // The dummy tex's view lives only as long as we hold it; the
            // bind group keeps an internal reference, so we must hold a
            // long-lived dummy view too. See `install()` for setup.
            unreachable!("rebind called with no heightmap and no dummy view")
        });
        self.bind_group = make_bind_group(
            device,
            &self.bind_group_layout,
            &self.uniform_buf,
            view,
            &self.splat,
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
            // 5: slot diffuse texture array (4 layers)
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
            // 6: slot diffuse sampler (repeat — tiles across the map)
            wgpu::BindGroupLayoutEntry {
                binding: 6,
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
                resource: wgpu::BindingResource::TextureView(&splat.slot_array_view),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::Sampler(&splat.slot_array_samp),
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
    let zero_row = vec![0u8; (SPLAT_DIM as usize) * 4];
    // Cheaper than allocating SPLAT_DIM² zeros up front: write row-by-row.
    for y in 0..SPLAT_DIM {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &distr_tex,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &zero_row,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SPLAT_DIM * 4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: SPLAT_DIM,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
    }

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

    // Slot diffuse texture array. Allocated at SLOT_DIFFUSE_DIM² × 4
    // layers, all initialised to a neutral grey so an unbound layer
    // doesn't render garbage if the user pokes the active mask
    // manually.
    let slot_array_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("terrain.splat.slot_array"),
        size: wgpu::Extent3d {
            width: SLOT_DIFFUSE_DIM,
            height: SLOT_DIFFUSE_DIM,
            depth_or_array_layers: SLOT_LAYER_COUNT,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SLOT_DIFFUSE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let slot_array_view = slot_array_tex.create_view(&wgpu::TextureViewDescriptor {
        label: Some("terrain.splat.slot_array.view"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let grey_layer: Vec<u8> =
        vec![0x80; (SLOT_DIFFUSE_DIM as usize) * (SLOT_DIFFUSE_DIM as usize) * 4];
    for layer in 0..SLOT_LAYER_COUNT {
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
            &grey_layer,
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

    let slot_array_samp = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("terrain.splat.slot_array.sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });

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
        slot_array_tex,
        slot_array_view,
        slot_array_samp,
        uniform_buf,
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

    let splat = install_splat_resources(device, queue);
    let bind_group = make_bind_group(device, &bgl, &uniform_buf, &dummy_view, &splat);

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
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: render_state.target_format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    });

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
            grid: None,
            heightmap: None,
            splat,
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
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        res.heightmap = Some(HeightmapTex { tex, view, dims });
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

/// Camera state owned by the App; computes a view-projection matrix per
/// frame. Orbit around `target` at radius `distance`, yaw/pitch in radians.
#[derive(Clone)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
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
            near: 10.0,
            far: max * 8.0,
        }
    }

    fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let dir = Vec3::new(cp * sy, sp, cp * cy);
        self.target + dir * self.distance
    }

    pub fn view_proj_matrix(&self, aspect: f32) -> Mat4 {
        let proj = Mat4::perspective_lh(self.fov_y, aspect, self.near, self.far);
        let view = Mat4::look_at_lh(self.eye(), self.target, Vec3::Y);
        proj * view
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
/// `None` if the point is behind the camera (clip-space `w <= 0`) or
/// projects outside `[-1, 1]` NDC (off-screen). Used by the F8 / ADR-023
/// start-position overlay so 2D markers track their 3D positions.
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
    let ndc = glam::Vec3::new(clip.x / clip.w, clip.y / clip.w, clip.z / clip.w);
    if !(-1.0..=1.0).contains(&ndc.x) || !(-1.0..=1.0).contains(&ndc.y) {
        return None;
    }
    Some(glam::Vec2::new(
        (ndc.x + 1.0) * 0.5 * rect_size.x,
        (1.0 - ndc.y) * 0.5 * rect_size.y,
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

/// Copy one slot's diffuse image into layer `layer` of the slot
/// diffuse array. The source `rgba` slice is treated as
/// `SLOT_DIFFUSE_DIM × SLOT_DIFFUSE_DIM` `Rgba<u8>` pixels — callers
/// (D5's inspector) resize incoming PNGs to that fixed dim before
/// invoking. Logs at `info!` per the tracing convention.
#[allow(dead_code)] // D5 wires this from the inspector's slot picker.
pub fn upload_diffuse_layer(render_state: &egui_wgpu::RenderState, layer: u32, rgba: &[u8]) {
    if layer >= SLOT_LAYER_COUNT {
        warn!(layer, "upload_diffuse_layer: layer out of range");
        return;
    }
    let expected = (SLOT_DIFFUSE_DIM as usize) * (SLOT_DIFFUSE_DIM as usize) * 4;
    if rgba.len() != expected {
        warn!(
            got = rgba.len(),
            expected, "upload_diffuse_layer: byte length mismatch; resize before calling"
        );
        return;
    }
    let renderer = render_state.renderer.read();
    let Some(res) = renderer.callback_resources.get::<RenderResources>() else {
        warn!("upload_diffuse_layer: no RenderResources");
        return;
    };
    let queue = &render_state.queue;
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &res.splat.slot_array_tex,
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
    info!(layer, "upload_diffuse_layer: slot diffuse written");
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

pub struct TerrainCallback {
    pub view_proj: [[f32; 4]; 4],
    pub max_height: f32,
    pub world_extent_x: f32,
    pub world_extent_z: f32,
    pub splat: SplatUniforms,
}

impl TerrainCallback {
    pub fn new(
        camera: &OrbitCamera,
        rect: egui::Rect,
        max_height: f32,
        world_extent_x: f32,
        world_extent_z: f32,
        splat: SplatUniforms,
    ) -> Self {
        let aspect = (rect.width() / rect.height()).max(0.0001);
        Self {
            view_proj: camera.view_proj_matrix(aspect).to_cols_array_2d(),
            max_height,
            world_extent_x,
            world_extent_z,
            splat,
        }
    }
}

impl egui_wgpu::CallbackTrait for TerrainCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(res) = resources.get::<RenderResources>() {
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
                },
            );
            res.write_splat_uniforms(queue, &self.splat);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(res) = resources.get::<RenderResources>() else {
            return;
        };
        let Some(grid) = res.grid.as_ref() else {
            return;
        };
        // Skip draw if no real heightmap uploaded yet (dummy 1×1 + no grid).
        if res.heightmap.is_none() {
            return;
        }
        pass.set_pipeline(&res.pipeline);
        pass.set_bind_group(0, &res.bind_group, &[]);
        pass.set_index_buffer(grid.index_buf.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..grid.index_count, 0, 0..1);
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
    fn world_to_screen_off_screen_returns_none() {
        let camera = cam();
        let rect = glam::Vec2::new(800.0, 600.0);
        let world = glam::Vec3::new(-100000.0, 0.0, -100000.0);
        assert!(world_to_screen(world, rect, &camera).is_none());
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
}
