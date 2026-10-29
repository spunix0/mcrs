//! wgpu renderer: chunk meshing, GPU buffers, culling, indirect draws. The crate where the performance goal is won.

mod mesh;

pub use mesh::{Area, ChunkMesh, Vertex, mesh_chunk};

use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use mc_assets::Atlas;
use std::collections::HashMap;
use wgpu::util::DeviceExt;

const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const SKY: [f32; 3] = [0.62, 0.76, 1.0];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    camera: [f32; 4],
    sky: [f32; 4],
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
}

impl GpuMesh {
    fn bytes(&self) -> u64 {
        self.vertices.size() + self.indices.size()
    }
}

impl GpuChunk {
    fn bytes(&self) -> u64 {
        self.opaque.as_ref().map_or(0, GpuMesh::bytes) + self.translucent.as_ref().map_or(0, GpuMesh::bytes)
    }
}

struct GpuChunk {
    opaque: Option<GpuMesh>,
    translucent: Option<GpuMesh>,
    min: Vec3,
    max: Vec3,
}

/// A vertex of the line overlay (block outline, crosshair).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub color: [u8; 4],
}

const MAX_LINE_VERTICES: u64 = 8192;

/// Lines drawn after the world: `world` is depth-tested (outlines), `top` is drawn over everything (crosshair).
#[derive(Default)]
pub struct Overlay {
    pub world: Vec<LineVertex>,
    pub top: Vec<LineVertex>,
}

pub struct FrameStats {
    pub drawn: usize,
    pub loaded: usize,
    pub triangles: u64,
    /// Vertex + index buffer bytes for all loaded chunks.
    pub gpu_bytes: u64,
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    depth: wgpu::TextureView,
    opaque: wgpu::RenderPipeline,
    translucent: wgpu::RenderPipeline,
    lines_world: wgpu::RenderPipeline,
    lines_top: wgpu::RenderPipeline,
    line_buffer: wgpu::Buffer,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    chunks: HashMap<(i32, i32), GpuChunk>,
    gpu_bytes: u64,
    pub adapter_name: String,
}

fn depth_view(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn upload(device: &wgpu::Device, g: &mesh::Geometry) -> Option<GpuMesh> {
    if g.indices.is_empty() {
        return None;
    }
    Some(GpuMesh {
        vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chunk vertices"),
            contents: bytemuck::cast_slice(&g.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        }),
        indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chunk indices"),
            contents: bytemuck::cast_slice(&g.indices),
            usage: wgpu::BufferUsages::INDEX,
        }),
        count: g.indices.len() as u32,
    })
}

/// Conservative test of an AABB against the six frustum planes of `m`.
fn visible(m: &Mat4, min: Vec3, max: Vec3) -> bool {
    let rows = [m.row(0), m.row(1), m.row(2), m.row(3)];
    let planes = [rows[3] + rows[0], rows[3] - rows[0], rows[3] + rows[1], rows[3] - rows[1], rows[2], rows[3] - rows[2]];
    planes.iter().all(|p: &Vec4| {
        let v = Vec3::new(if p.x >= 0.0 { max.x } else { min.x }, if p.y >= 0.0 { max.y } else { min.y }, if p.z >= 0.0 { max.z } else { min.z });
        p.truncate().dot(v) + p.w >= 0.0
    })
}

impl Renderer {
    pub fn new(target: impl Into<wgpu::SurfaceTarget<'static>>, width: u32, height: u32, atlas: &Atlas, vsync: bool) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(target)?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .context("no GPU adapter")?;
        let adapter_name = adapter.get_info().name;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
        let mut config = surface.get_default_config(&adapter, width.max(1), height.max(1)).context("surface unsupported")?;
        config.format = format;
        config.present_mode = if vsync { wgpu::PresentMode::AutoVsync } else { wgpu::PresentMode::AutoNoVsync };
        config.desired_maximum_frame_latency = 2;
        surface.configure(&device, &config);

        // Base level followed by every mip level, as create_texture_with_data expects for one layer.
        let mut texels = atlas.rgba.clone();
        for level in &atlas.mips {
            texels.extend_from_slice(level);
        }
        let texture = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor {
                label: Some("block atlas"),
                size: wgpu::Extent3d { width: atlas.width, height: atlas.height, depth_or_array_layers: 1 },
                mip_level_count: 1 + atlas.mips.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &texels,
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
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
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&texture.create_view(&Default::default())) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blocks"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Unorm8x4],
        };
        let make = |fs: &str, blend: Option<wgpu::BlendState>, depth_write: bool, cull: Option<wgpu::Face>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(vertex_layout.clone())],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: wgpu::PrimitiveState { cull_mode: cull, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let opaque = make("fs_opaque", None, true, Some(wgpu::Face::Back));
        let translucent = make("fs_translucent", Some(wgpu::BlendState::ALPHA_BLENDING), false, None);

        let line_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4],
        };
        let make_lines = |label: &str, compare: wgpu::CompareFunction| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_line"),
                    compilation_options: Default::default(),
                    buffers: &[Some(line_layout.clone())],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_line"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::LineList, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let lines_world = make_lines("outline", wgpu::CompareFunction::LessEqual);
        let lines_top = make_lines("crosshair", wgpu::CompareFunction::Always);
        let line_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lines"),
            size: MAX_LINE_VERTICES * std::mem::size_of::<LineVertex>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let depth = depth_view(&device, config.width, config.height);

        Ok(Self {
            surface,
            device,
            queue,
            config,
            depth,
            opaque,
            translucent,
            lines_world,
            lines_top,
            line_buffer,
            uniforms,
            bind_group,
            chunks: HashMap::new(),
            gpu_bytes: 0,
            adapter_name,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth = depth_view(&self.device, width, height);
    }

    pub fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height as f32
    }

    pub fn height(&self) -> u32 {
        self.config.height
    }

    pub fn upload(&mut self, mesh: &ChunkMesh) {
        let gpu = GpuChunk {
            opaque: upload(&self.device, &mesh.opaque),
            translucent: upload(&self.device, &mesh.translucent),
            min: Vec3::new(mesh.x as f32 * 16.0, mesh.min_y, mesh.z as f32 * 16.0),
            max: Vec3::new(mesh.x as f32 * 16.0 + 16.0, mesh.max_y, mesh.z as f32 * 16.0 + 16.0),
        };
        self.gpu_bytes += gpu.bytes();
        if let Some(old) = self.chunks.insert((mesh.x, mesh.z), gpu) {
            self.gpu_bytes -= old.bytes();
        }
    }

    pub fn remove(&mut self, x: i32, z: i32) {
        if let Some(old) = self.chunks.remove(&(x, z)) {
            self.gpu_bytes -= old.bytes();
        }
    }

    pub fn render(&mut self, view_proj: Mat4, camera: Vec3, fog_end: f32, overlay: &Overlay) -> FrameStats {
        let mut stats = FrameStats { drawn: 0, loaded: self.chunks.len(), triangles: 0, gpu_bytes: self.gpu_bytes };
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return stats;
            }
            _ => return stats,
        };
        let view = frame.texture.create_view(&Default::default());
        let u = Uniforms {
            view_proj: view_proj.to_cols_array_2d(),
            camera: [camera.x, camera.y, camera.z, fog_end],
            sky: [SKY[0], SKY[1], SKY[2], 1.0],
        };
        self.queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&u));
        let mut lines = overlay.world.clone();
        lines.extend_from_slice(&overlay.top);
        lines.truncate(MAX_LINE_VERTICES as usize);
        let (world_lines, line_count) = (overlay.world.len().min(lines.len()) as u32, lines.len() as u32);
        if line_count > 0 {
            self.queue.write_buffer(&self.line_buffer, 0, bytemuck::cast_slice(&lines));
        }

        let mut visible_chunks: Vec<(&GpuChunk, f32)> = self
            .chunks
            .values()
            .filter(|c| c.min.y <= c.max.y && visible(&view_proj, c.min, c.max))
            .map(|c| (c, ((c.min + c.max) * 0.5).distance_squared(camera)))
            .collect();
        stats.drawn = visible_chunks.len();
        stats.triangles = visible_chunks
            .iter()
            .map(|(c, _)| (c.opaque.as_ref().map_or(0, |m| m.count) + c.translucent.as_ref().map_or(0, |m| m.count)) as u64 / 3)
            .sum();
        // Near-to-far for opaque (early depth rejects), far-to-near for translucent blending.
        visible_chunks.sort_by(|a, b| a.1.total_cmp(&b.1));

        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("world"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: SKY[0] as f64, g: SKY[1] as f64, b: SKY[2] as f64, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_pipeline(&self.opaque);
            for (c, _) in &visible_chunks {
                if let Some(m) = &c.opaque {
                    pass.set_vertex_buffer(0, m.vertices.slice(..));
                    pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..m.count, 0, 0..1);
                }
            }
            pass.set_pipeline(&self.translucent);
            for (c, _) in visible_chunks.iter().rev() {
                if let Some(m) = &c.translucent {
                    pass.set_vertex_buffer(0, m.vertices.slice(..));
                    pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..m.count, 0, 0..1);
                }
            }
            if line_count > 0 {
                pass.set_vertex_buffer(0, self.line_buffer.slice(..));
                pass.set_pipeline(&self.lines_world);
                pass.draw(0..world_lines, 0..1);
                pass.set_pipeline(&self.lines_top);
                pass.draw(world_lines..line_count, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        stats
    }
}
