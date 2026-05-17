//! Stage 0 terrain renderer. Single draw call, no LOD, no normals.
//!
//! See ADR-008 for coords (Y-up, left-handed, 8 elmos per heightmap pixel).
//! Persistent GPU state (pipeline + mesh buffers) lives inside
//! `egui_wgpu::CallbackResources` as [`RenderResources`]; the per-frame
//! [`TerrainCallback`] only carries the camera matrix.

use barme_core::Heightmap;
use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu;
use eframe::wgpu;
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

/// 8 elmos per heightmap pixel — `MapSize::ELMOS_PER_SMU / HEIGHTMAP_PER_SMU`.
pub const ELMOS_PER_PIXEL: f32 = 8.0;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 3],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    /// xyz unused, w = max world height for the gradient lerp.
    height_extent: [f32; 4],
}

struct Mesh {
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    index_count: u32,
}

pub struct RenderResources {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buf: wgpu::Buffer,
    mesh: Option<Mesh>,
}

impl RenderResources {
    fn write_uniforms(&self, queue: &wgpu::Queue, u: &Uniforms) {
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(u));
    }
}

/// Install the pipeline and uniform buffer once. Called from `App::new`.
pub fn install(render_state: &egui_wgpu::RenderState) {
    let device = &render_state.device;

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
        label: Some("terrain.bg"),
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buf.as_entire_binding(),
        }],
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
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3],
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            // Left-handed + CCW winding: front faces are CW in screen space,
            // so cull back faces with `front_face = Cw`. Camera below also
            // built with `_lh` matrices — keep both consistent.
            front_face: wgpu::FrontFace::Cw,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        // No depth attachment — egui's render pass doesn't provide one. For
        // a heightmap viewed from above this looks mostly fine; oblique
        // angles get some triangle-order bleed. Stage 1 will render
        // offscreen with depth and present via a sampled texture.
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
            bind_group,
            uniform_buf,
            mesh: None,
        });
}

/// Build a vertex + index buffer for the given heightmap and swap it into
/// callback resources. Existing buffers (if any) are dropped here.
pub fn upload_mesh(
    render_state: &egui_wgpu::RenderState,
    heightmap: &Heightmap,
    height_scale: f32,
) {
    let (w, h) = heightmap.dims();
    let data = heightmap.data();

    let mut vertices = Vec::with_capacity((w as usize) * (h as usize));
    let inv = 1.0 / u16::MAX as f32;
    for z in 0..h {
        for x in 0..w {
            let s = data[(z * w + x) as usize] as f32 * inv;
            vertices.push(Vertex {
                pos: [
                    x as f32 * ELMOS_PER_PIXEL,
                    s * height_scale,
                    z as f32 * ELMOS_PER_PIXEL,
                ],
            });
        }
    }

    // Two triangles per quad. Winding CW in left-handed screen space so the
    // pipeline's `front_face = Cw` keeps them visible from above.
    let quads_x = w - 1;
    let quads_z = h - 1;
    let mut indices: Vec<u32> = Vec::with_capacity((quads_x as usize) * (quads_z as usize) * 6);
    for z in 0..quads_z {
        for x in 0..quads_x {
            let i = z * w + x;
            let i_r = i + 1;
            let i_d = i + w;
            let i_dr = i_d + 1;
            indices.extend_from_slice(&[i, i_d, i_r, i_r, i_d, i_dr]);
        }
    }

    let device = &render_state.device;
    let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("terrain.vb"),
        contents: bytemuck::cast_slice(&vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("terrain.ib"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    let index_count = indices.len() as u32;

    if let Some(res) = render_state
        .renderer
        .write()
        .callback_resources
        .get_mut::<RenderResources>()
    {
        res.mesh = Some(Mesh {
            vertex_buf,
            index_buf,
            index_count,
        });
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

    fn view_proj(&self, aspect: f32) -> Mat4 {
        let proj = Mat4::perspective_lh(self.fov_y, aspect, self.near, self.far);
        let view = Mat4::look_at_lh(self.eye(), self.target, Vec3::Y);
        proj * view
    }
}

pub struct TerrainCallback {
    pub view_proj: [[f32; 4]; 4],
    pub max_height: f32,
}

impl TerrainCallback {
    pub fn new(camera: &OrbitCamera, rect: egui::Rect, max_height: f32) -> Self {
        let aspect = (rect.width() / rect.height()).max(0.0001);
        Self {
            view_proj: camera.view_proj(aspect).to_cols_array_2d(),
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
                    height_extent: [0.0, 0.0, 0.0, self.max_height.max(1.0)],
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
        let Some(mesh) = res.mesh.as_ref() else {
            return;
        };
        pass.set_pipeline(&res.pipeline);
        pass.set_bind_group(0, &res.bind_group, &[]);
        pass.set_vertex_buffer(0, mesh.vertex_buf.slice(..));
        pass.set_index_buffer(mesh.index_buf.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
    }
}
