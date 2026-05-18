//! Stage 1 terrain renderer (ADR-017). Heightmap lives on the GPU as an
//! `r16uint` texture; the vertex shader samples it for Y displacement so
//! brush edits are texture writes, not full-mesh rebuilds.
//!
//! See ADR-008 for coords (Y-up, left-handed, 8 elmos per heightmap pixel).
//! Persistent GPU state (pipeline + bind group + grid + heightmap texture)
//! lives inside `egui_wgpu::CallbackResources` as [`RenderResources`]; the
//! per-frame [`TerrainCallback`] only carries camera matrix + max_height.

use barme_core::{DirtyRect, Heightmap};
use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu;
use eframe::wgpu;
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

/// 8 elmos per heightmap pixel — `MapSize::ELMOS_PER_SMU / HEIGHTMAP_PER_SMU`.
pub const ELMOS_PER_PIXEL: f32 = 8.0;

const HEIGHTMAP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Uint;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    /// [max_height, elmos_per_pixel, _, _]
    params: [f32; 4],
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
}

impl RenderResources {
    fn write_uniforms(&self, queue: &wgpu::Queue, u: &Uniforms) {
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(u));
    }

    fn rebind(&mut self, device: &wgpu::Device) {
        let view = self.heightmap.as_ref().map(|h| &h.view).unwrap_or_else(|| {
            // The dummy tex's view lives only as long as we hold it; the
            // bind group keeps an internal reference, so we must hold a
            // long-lived dummy view too. See `install()` for setup.
            unreachable!("rebind called with no heightmap and no dummy view")
        });
        self.bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain.bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
            ],
        });
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

/// Install the pipeline, uniform buffer, and a 1×1 dummy heightmap. Called
/// once from `App::new`.
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

    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("terrain.bgl"),
        entries: &[
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
        ],
    });

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

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("terrain.bg"),
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&dummy_view),
            },
        ],
    });

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
        return;
    };

    // (Re)allocate texture if dims changed or none yet.
    let need_alloc = match &res.heightmap {
        Some(h) => h.dims != dims,
        None => true,
    };
    if need_alloc {
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
    let tex = &res.heightmap.as_ref().expect("just allocated").tex;
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
        return;
    };
    let Some(hm_tex) = res.heightmap.as_ref() else {
        return;
    };
    if hm_tex.dims != full_dims {
        // Dims changed since the last upload — caller should be using
        // `upload_heightmap` instead. Skip silently rather than corrupt.
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

pub struct TerrainCallback {
    pub view_proj: [[f32; 4]; 4],
    pub max_height: f32,
}

impl TerrainCallback {
    pub fn new(camera: &OrbitCamera, rect: egui::Rect, max_height: f32) -> Self {
        let aspect = (rect.width() / rect.height()).max(0.0001);
        Self {
            view_proj: camera.view_proj_matrix(aspect).to_cols_array_2d(),
            max_height,
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
                    params: [self.max_height.max(1.0), ELMOS_PER_PIXEL, 0.0, 0.0],
                },
            );
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
