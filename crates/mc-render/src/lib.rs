//! wgpu renderer: chunk meshing, GPU buffers, frustum and cave culling. The crate where the performance goal is won.

mod cull;
mod gui;
mod mesh;

pub use gui::{Gui, GuiTexture, GuiVertex};
pub use mesh::{Area, ChunkMesh, ChunkVertex, Geometry, Vertex, brightness, lightmap, mesh_chunk};

use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use mc_assets::{Atlas, GuiAtlas};
use std::collections::{HashMap, HashSet};
use wgpu::util::DeviceExt;

const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    camera: [f32; 4],
    sky: [f32; 4],
    fog: [f32; 4],
    sun: [f32; 4],
    sunrise: [f32; 4],
    /// xy = window size in pixels.
    screen: [f32; 4],
    sky_light: [f32; 4],
    ambient: [f32; 4],
    block_tint: [f32; 4],
}

/// Sky and light for this frame (see `sky.rs` in the client for how vanilla computes them).
#[derive(Clone, Copy, Debug)]
pub struct Environment {
    /// Sky color overhead (sRGB 0..1).
    pub sky: [f32; 3],
    /// Fog and horizon color; distant chunks fade into it.
    pub fog: [f32; 3],
    /// Multiplier on sky light, 0.2 at midnight to 1 at noon.
    pub daylight: f32,
    /// Direction to the sun.
    pub sun: [f32; 3],
    /// Sunrise/sunset glow color and strength.
    pub sunrise: [f32; 4],
    /// The Brightness option, 0 (Moody) .. 1 (Bright).
    pub gamma: f32,
    /// Lightmap inputs (26.3 environment attributes): sky light color, ambient light, block light tint.
    pub sky_light: [f32; 3],
    pub ambient: [f32; 3],
    pub block_tint: [f32; 3],
    /// Fog start and end in blocks when they differ from the render distance (underwater, in lava).
    pub fog_range: Option<[f32; 2]>,
}

impl Default for Environment {
    fn default() -> Self {
        Self { sky: [0.47, 0.65, 1.0], fog: [0.75, 0.85, 1.0], daylight: 1.0, sun: [0.0, 1.0, 0.0], sunrise: [0.0; 4], gamma: 0.5, sky_light: [1.0; 3], ambient: [0.04; 3], block_tint: [1.0, 0.85, 0.55], fog_range: None }
    }
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

struct GpuSection {
    opaque: Option<GpuMesh>,
    translucent: Option<GpuMesh>,
    /// Face-to-face visibility for cave culling.
    vis: u64,
}

impl Default for GpuSection {
    fn default() -> Self {
        Self { opaque: None, translucent: None, vis: cull::ALL }
    }
}

impl GpuSection {
    fn bytes(&self) -> u64 {
        self.opaque.as_ref().map_or(0, GpuMesh::bytes) + self.translucent.as_ref().map_or(0, GpuMesh::bytes)
    }

    fn triangles(&self) -> u64 {
        (self.opaque.as_ref().map_or(0, |m| m.count) + self.translucent.as_ref().map_or(0, |m| m.count)) as u64 / 3
    }
}

struct GpuColumn {
    min_section: i32,
    sections: Vec<GpuSection>,
}

/// A vertex of the line overlay (block outline, crosshair).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub color: [u8; 4],
}

/// Per-frame geometry besides the chunks. All triangle lists are in world space, 3 vertices per triangle.
#[derive(Default)]
pub struct Overlay {
    /// Depth-tested lines (block outline, hitboxes, chunk borders).
    pub world: Vec<LineVertex>,
    /// Lines drawn over everything.
    pub top: Vec<LineVertex>,
    /// Blended, depth-tested triangles textured from the block atlas (cracks, particles).
    pub quads: Vec<Vertex>,
    /// Entity triangles textured from the GUI atlas (skins, white for plain boxes).
    pub entities: Vec<Vertex>,
    /// Entity triangles textured from the block atlas (dropped items, falling blocks).
    pub entity_items: Vec<Vertex>,
    /// Sun, moon and stars, textured from the GUI atlas and added onto the sky.
    pub celestial: Vec<Vertex>,
    /// First-person arm triangles, textured from the GUI atlas (player skin).
    pub hand_skin: Vec<Vertex>,
    /// First-person held item triangles, textured from the block/item atlas.
    pub hand_item: Vec<Vertex>,
}

pub struct FrameStats {
    pub drawn: usize,
    pub loaded: usize,
    pub triangles: u64,
    /// Vertex + index buffer bytes for all loaded chunks.
    pub gpu_bytes: u64,
}

/// A vertex buffer that grows to fit what's written to it.
struct DynBuffer {
    buffer: wgpu::Buffer,
    label: &'static str,
}

impl DynBuffer {
    fn new(device: &wgpu::Device, label: &'static str, size: u64) -> Self {
        Self { buffer: Self::make(device, label, size), label }
    }

    fn make(device: &wgpu::Device, label: &'static str, size: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: size.max(256),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Uploads `data` and returns how many elements it holds.
    fn write<T: Pod>(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[T]) -> u32 {
        let bytes: &[u8] = bytemuck::cast_slice(data);
        if bytes.is_empty() {
            return 0;
        }
        if bytes.len() as u64 > self.buffer.size() {
            self.buffer = Self::make(device, self.label, (bytes.len() as u64).next_power_of_two());
        }
        queue.write_buffer(&self.buffer, 0, bytes);
        data.len() as u32
    }
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    depth: wgpu::TextureView,
    opaque: wgpu::RenderPipeline,
    lod: wgpu::RenderPipeline,
    lod_uniforms: wgpu::Buffer,
    lod_bind_group: wgpu::BindGroup,
    /// Distant terrain by region (x, z): its mesh and Y range.
    lod_meshes: HashMap<(i32, i32), (GpuMesh, f32, f32)>,
    /// This frame's distant-terrain projection and the distance inside which it hides (real chunks are there).
    pub lod_view: Option<(Mat4, f32)>,
    /// The block atlas size in 16-pixel tiles (distant terrain picks textures by tile).
    atlas_tiles: (f32, f32),
    translucent: wgpu::RenderPipeline,
    overlay_pipeline: wgpu::RenderPipeline,
    cutout: wgpu::RenderPipeline,
    hand: wgpu::RenderPipeline,
    sky_pipeline: wgpu::RenderPipeline,
    celestial_pipeline: wgpu::RenderPipeline,
    lines_world: wgpu::RenderPipeline,
    lines_top: wgpu::RenderPipeline,
    gui_pipeline: wgpu::RenderPipeline,
    gui_blur_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    gui_bind_group: wgpu::BindGroup,
    panorama_bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    lines: DynBuffer,
    gui_buffer: DynBuffer,
    overlay_buffer: DynBuffer,
    hand_buffer: DynBuffer,
    entity_buffer: DynBuffer,
    entity_item_buffer: DynBuffer,
    celestial_buffer: DynBuffer,
    chunks: HashMap<(i32, i32), GpuColumn>,
    gpu_bytes: u64,
    /// Skip sections hidden behind terrain (and the entities in them).
    pub cave_culling: bool,
    /// Skip entities outside the view or in sections cave culling hid.
    pub entity_culling: bool,
    /// The sections cave culling reached last frame, for [`Renderer::entity_visible`].
    reached: Option<HashSet<[i32; 3]>>,
    pub adapter_name: String,
    block_texture: wgpu::Texture,
    /// Frame each animated texture shows now.
    anim_frames: Vec<usize>,
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

/// A texture with all its mip levels (base first), RGBA8 sRGB.
fn texture(device: &wgpu::Device, queue: &wgpu::Queue, label: &str, w: u32, h: u32, levels: &[&[u8]]) -> wgpu::Texture {
    let texels: Vec<u8> = levels.concat();
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: levels.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &texels,
        )
}

impl Renderer {
    pub fn new(target: impl Into<wgpu::SurfaceTarget<'static>>, width: u32, height: u32, atlas: &Atlas, gui_atlas: &GuiAtlas, vsync: bool) -> Result<Self> {
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

        let mut levels: Vec<&[u8]> = vec![&atlas.rgba];
        levels.extend(atlas.mips.iter().map(Vec::as_slice));
        let block_texture = texture(&device, &queue, "block atlas", atlas.width, atlas.height, &levels);
        let block_view = block_texture.create_view(&Default::default());
        let gui_view = texture(&device, &queue, "gui atlas", gui_atlas.width, gui_atlas.height, &[&gui_atlas.rgba]).create_view(&Default::default());
        let panorama_view = match &gui_atlas.panorama {
            Some(p) => texture(&device, &queue, "panorama", p.width, p.height, &p.levels.iter().map(Vec::as_slice).collect::<Vec<_>>()).create_view(&Default::default()),
            None => texture(&device, &queue, "panorama", 1, 1, &[&[40, 40, 40, 255]]).create_view(&Default::default()),
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let smooth = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
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
        let group = |label: &str, view: &wgpu::TextureView, sampler: &wgpu::Sampler| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
                ],
            })
        };
        let bind_group = group("blocks", &block_view, &sampler);
        let gui_bind_group = group("gui", &gui_view, &sampler);
        let panorama_bind_group = group("panorama", &panorama_view, &smooth);
        // Distant terrain draws with its own projection (far plane out at the LOD distance), so its own uniforms.
        let lod_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lod uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let lod_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lod"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: lod_uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&block_view) },
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
        let chunk_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ChunkVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Unorm8x4, 3 => Unorm8x4],
        };
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent::OVER,
        };
        struct P<'a> {
            label: &'a str,
            vs: &'a str,
            fs: &'a str,
            buffers: Option<wgpu::VertexBufferLayout<'a>>,
            topology: wgpu::PrimitiveTopology,
            blend: Option<wgpu::BlendState>,
            depth_write: bool,
            compare: wgpu::CompareFunction,
            cull: Option<wgpu::Face>,
        }
        let make = |p: P| {
            let buffers = [p.buffers];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(p.label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(p.vs),
                    compilation_options: Default::default(),
                    buffers: if buffers[0].is_some() { &buffers } else { &[] },
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(p.fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend: p.blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: wgpu::PrimitiveState { topology: p.topology, cull_mode: p.cull, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(p.depth_write),
                    depth_compare: Some(p.compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let tri = wgpu::PrimitiveTopology::TriangleList;
        let less = wgpu::CompareFunction::LessEqual;
        let always = wgpu::CompareFunction::Always;
        let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);
        let v = || Some(vertex_layout.clone());
        let c = || Some(chunk_layout.clone());
        let opaque = make(P { label: "opaque", vs: "vs_chunk", fs: "fs_opaque", buffers: c(), topology: tri, blend: None, depth_write: true, compare: less, cull: Some(wgpu::Face::Back) });
        let lod = make(P { label: "lod", vs: "vs_lod", fs: "fs_lod", buffers: c(), topology: tri, blend: None, depth_write: true, compare: less, cull: Some(wgpu::Face::Back) });
        let translucent = make(P { label: "translucent", vs: "vs_chunk", fs: "fs_translucent", buffers: c(), topology: tri, blend: alpha, depth_write: false, compare: less, cull: Some(wgpu::Face::Back) });
        let overlay_pipeline = make(P { label: "overlay", vs: "vs", fs: "fs_translucent", buffers: v(), topology: tri, blend: alpha, depth_write: false, compare: less, cull: None });
        // Entities, dropped items and the hand: alpha-tested, both sides.
        let cutout = make(P { label: "cutout", vs: "vs", fs: "fs_cutout", buffers: v(), topology: tri, blend: None, depth_write: true, compare: less, cull: None });
        let hand = make(P { label: "hand", vs: "vs", fs: "fs_hand", buffers: v(), topology: tri, blend: None, depth_write: true, compare: less, cull: None });
        let sky_pipeline = make(P { label: "sky", vs: "vs_sky", fs: "fs_sky", buffers: None, topology: tri, blend: None, depth_write: false, compare: always, cull: None });
        let celestial_pipeline = make(P { label: "celestial", vs: "vs", fs: "fs_celestial", buffers: v(), topology: tri, blend: Some(additive), depth_write: false, compare: always, cull: None });

        let line_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4],
        };
        let lines = wgpu::PrimitiveTopology::LineList;
        let lines_world = make(P { label: "outline", vs: "vs_line", fs: "fs_line", buffers: Some(line_layout.clone()), topology: lines, blend: alpha, depth_write: false, compare: less, cull: None });
        let lines_top = make(P { label: "crosshair", vs: "vs_line", fs: "fs_line", buffers: Some(line_layout), topology: lines, blend: alpha, depth_write: false, compare: always, cull: None });

        let gui_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<GuiVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Unorm8x4],
        };
        let gui_pipeline = make(P { label: "gui", vs: "vs_gui", fs: "fs_gui", buffers: Some(gui_layout.clone()), topology: tri, blend: alpha, depth_write: false, compare: always, cull: None });
        let gui_blur_pipeline = make(P { label: "gui blur", vs: "vs_gui", fs: "fs_gui_blur", buffers: Some(gui_layout), topology: tri, blend: alpha, depth_write: false, compare: always, cull: None });

        let depth = depth_view(&device, config.width, config.height);
        let buf = |label| DynBuffer::new(&device, label, 1 << 16);
        Ok(Self {
            lines: buf("lines"),
            gui_buffer: buf("gui vertices"),
            overlay_buffer: buf("overlay quads"),
            hand_buffer: buf("hand"),
            entity_buffer: buf("entities"),
            entity_item_buffer: buf("entity items"),
            celestial_buffer: buf("celestial"),
            surface,
            device,
            queue,
            config,
            depth,
            opaque,
            lod,
            lod_uniforms,
            lod_bind_group,
            lod_meshes: HashMap::new(),
            lod_view: None,
            atlas_tiles: ((atlas.width / 16) as f32, (atlas.height / 16) as f32),
            translucent,
            overlay_pipeline,
            cutout,
            hand,
            sky_pipeline,
            celestial_pipeline,
            lines_world,
            lines_top,
            gui_pipeline,
            gui_blur_pipeline,
            bind_group,
            gui_bind_group,
            panorama_bind_group,
            uniforms,
            chunks: HashMap::new(),
            gpu_bytes: 0,
            cave_culling: true,
            entity_culling: true,
            reached: None,
            adapter_name,
            anim_frames: vec![0; atlas.animations.len()],
            block_texture,
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

    pub fn set_vsync(&mut self, vsync: bool) {
        self.config.present_mode = if vsync { wgpu::PresentMode::AutoVsync } else { wgpu::PresentMode::AutoNoVsync };
        self.surface.configure(&self.device, &self.config);
    }

    pub fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height as f32
    }

    pub fn height(&self) -> u32 {
        self.config.height
    }

    pub fn upload(&mut self, mesh: &ChunkMesh) {
        let fresh = || GpuColumn { min_section: mesh.min_section, sections: (0..mesh.height).map(|_| GpuSection::default()).collect() };
        let column = self.chunks.entry((mesh.x, mesh.z)).or_insert_with(fresh);
        if column.min_section != mesh.min_section || column.sections.len() != mesh.height {
            self.gpu_bytes -= column.sections.iter().map(GpuSection::bytes).sum::<u64>();
            *column = fresh();
        }
        for (i, s) in &mesh.sections {
            let gpu = GpuSection { opaque: upload(&self.device, &s.opaque), translucent: upload(&self.device, &s.translucent), vis: s.vis };
            self.gpu_bytes += gpu.bytes();
            self.gpu_bytes -= std::mem::replace(&mut column.sections[*i], gpu).bytes();
        }
    }

    pub fn remove(&mut self, x: i32, z: i32) {
        if let Some(old) = self.chunks.remove(&(x, z)) {
            self.gpu_bytes -= old.sections.iter().map(GpuSection::bytes).sum::<u64>();
        }
    }

    /// Whether an entity box is on screen: inside the view and, with cave culling, in a section last frame reached.
    pub fn entity_visible(&self, view_proj: &Mat4, min: Vec3, max: Vec3) -> bool {
        if !self.entity_culling {
            return true;
        }
        if !visible(view_proj, min, max) {
            return false;
        }
        let Some(reached) = &self.reached else { return true };
        let s = |v: f32| (v.floor() as i32).div_euclid(16);
        (s(min.x)..=s(max.x)).any(|x| (s(min.y)..=s(max.y)).any(|y| (s(min.z)..=s(max.z)).any(|z| reached.contains(&[x, y, z]))))
    }

    /// Drops every chunk mesh (leaving a world or changing dimension).
    pub fn clear_chunks(&mut self) {
        self.chunks.clear();
        self.gpu_bytes = 0;
        self.lod_meshes.clear();
    }

    /// Sets (or with empty geometry removes) the distant-terrain mesh of region `key`, spanning Y `y`.
    pub fn upload_lod(&mut self, key: (i32, i32), g: &Geometry, y: (f32, f32)) {
        match upload(&self.device, g) {
            Some(m) => self.lod_meshes.insert(key, (m, y.0, y.1)),
            None => self.lod_meshes.remove(&key),
        };
    }

    pub fn clear_lod(&mut self) {
        self.lod_meshes.clear();
    }

    pub fn remove_lod(&mut self, key: (i32, i32)) {
        self.lod_meshes.remove(&key);
    }

    /// Regions of distant terrain loaded, and their GPU bytes.
    pub fn lod_stats(&self) -> (usize, u64) {
        (self.lod_meshes.len(), self.lod_meshes.values().map(|m| m.0.bytes()).sum())
    }

    /// Draws one frame. Without `world` only the GUI is drawn (menus outside a world).
    pub fn render(&mut self, view_proj: Mat4, camera: Vec3, fog_end: f32, env: &Environment, overlay: &Overlay, gui: &Gui, world: bool) -> FrameStats {
        let loaded = self.chunks.values().flat_map(|c| &c.sections).filter(|s| s.opaque.is_some() || s.translucent.is_some()).count();
        let mut stats = FrameStats { drawn: 0, loaded, triangles: 0, gpu_bytes: self.gpu_bytes };
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return stats;
            }
            _ => return stats,
        };
        let view = frame.texture.create_view(&Default::default());
        let rgb = |c: [f32; 3], w: f32| [c[0], c[1], c[2], w];
        let u = Uniforms {
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            camera: [camera.x, camera.y, camera.z, env.fog_range.map_or(fog_end, |r| r[1])],
            sky: rgb(env.sky, env.fog_range.map_or(fog_end * 0.7, |r| r[0])),
            fog: rgb(env.fog, env.daylight),
            sun: rgb(env.sun, env.sunrise[3]),
            sunrise: [env.sunrise[0], env.sunrise[1], env.sunrise[2], env.gamma],
            screen: [self.config.width as f32, self.config.height as f32, 0.0, 0.0],
            sky_light: rgb(env.sky_light, 1.0),
            ambient: rgb(env.ambient, 1.0),
            block_tint: rgb(env.block_tint, 1.0),
        };
        self.queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&u));
        let (device, queue) = (&self.device, &self.queue);
        let mut lines = overlay.world.clone();
        lines.extend_from_slice(&overlay.top);
        let world_lines = overlay.world.len() as u32;
        let line_count = self.lines.write(device, queue, &lines);
        let entity_count = self.entity_buffer.write(device, queue, &overlay.entities[..overlay.entities.len() / 3 * 3]);
        let item_count = self.entity_item_buffer.write(device, queue, &overlay.entity_items[..overlay.entity_items.len() / 3 * 3]);
        let quad_count = self.overlay_buffer.write(device, queue, &overlay.quads[..overlay.quads.len() / 3 * 3]);
        let celestial_count = self.celestial_buffer.write(device, queue, &overlay.celestial[..overlay.celestial.len() / 3 * 3]);
        let mut hand = overlay.hand_skin.clone();
        hand.extend_from_slice(&overlay.hand_item);
        let skin_count = overlay.hand_skin.len() as u32;
        let hand_count = self.hand_buffer.write(device, queue, &hand);
        let gui_count = self.gui_buffer.write(device, queue, &gui.vertices);

        let bounds = |p: [i32; 3]| {
            let min = Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32) * 16.0;
            (min, min + Vec3::splat(16.0))
        };
        let in_view = |p: [i32; 3]| {
            let (min, max) = bounds(p);
            visible(&view_proj, min, max)
        };
        let section_at = |p: [i32; 3]| {
            let c = self.chunks.get(&(p[0], p[2]))?;
            c.sections.get(usize::try_from(p[1] - c.min_section).ok()?)
        };
        let candidates: Vec<[i32; 3]> = if !world {
            Vec::new()
        } else if self.cave_culling {
            let (cx, cz) = ((camera.x / 16.0).floor() as i32, (camera.z / 16.0).floor() as i32);
            let radius = self.chunks.keys().map(|k| (k.0 - cx).abs().max((k.1 - cz).abs())).max().unwrap_or(0);
            let range = self.chunks.values().next().map_or((0, -1), |c| (c.min_section, c.min_section + c.sections.len() as i32 - 1));
            cull::search(camera, radius, range, |x, y, z| section_at([x, y, z]).map(|s| s.vis), in_view)
        } else {
            self.chunks
                .iter()
                .flat_map(|(k, c)| (0..c.sections.len() as i32).map(move |i| [k.0, c.min_section + i, k.1]))
                .filter(|p| in_view(*p))
                .collect()
        };
        let mut visible_chunks: Vec<(&GpuSection, f32)> = candidates
            .iter()
            .filter_map(|p| {
                let s = section_at(*p).filter(|s| s.opaque.is_some() || s.translucent.is_some())?;
                let (min, max) = bounds(*p);
                Some((s, ((min + max) * 0.5).distance_squared(camera)))
            })
            .collect();
        self.reached = (world && self.cave_culling).then(|| candidates.into_iter().collect());
        stats.drawn = visible_chunks.len();
        stats.triangles = visible_chunks.iter().map(|(s, _)| s.triangles()).sum();
        // Near-to-far for opaque (early depth rejects), far-to-near for translucent blending.
        visible_chunks.sort_by(|a, b| a.1.total_cmp(&b.1));

        let mut encoder = self.device.create_command_encoder(&Default::default());
        let clear = if world { env.fog } else { [0.0; 3] };
        // The surface is sRGB: clear with the linear value of the fog color.
        let lin = |c: f32| (c as f64).powf(2.2);
        let clear_color = wgpu::LoadOp::Clear(wgpu::Color { r: lin(clear[0]), g: lin(clear[1]), b: lin(clear[2]), a: 1.0 });
        // Distant terrain first, with its own far-reaching projection; the world then draws over it on fresh depth.
        let lod = self.lod_view.filter(|_| world && !self.lod_meshes.is_empty());
        if let Some((lod_vp, inner)) = lod {
            let mut lu = u;
            lu.view_proj = lod_vp.to_cols_array_2d();
            lu.inv_view_proj = lod_vp.inverse().to_cols_array_2d();
            lu.screen[2] = inner;
            lu.ambient[3] = self.atlas_tiles.0;
            lu.block_tint[3] = self.atlas_tiles.1;
            self.queue.write_buffer(&self.lod_uniforms, 0, bytemuck::bytes_of(&lu));
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("lod"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: clear_color, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.gui_bind_group, &[]);
            pass.set_pipeline(&self.sky_pipeline);
            pass.draw(0..3, 0..1);
            if celestial_count > 0 {
                pass.set_pipeline(&self.celestial_pipeline);
                pass.set_vertex_buffer(0, self.celestial_buffer.buffer.slice(..));
                pass.draw(0..celestial_count, 0..1);
            }
            pass.set_bind_group(0, &self.lod_bind_group, &[]);
            pass.set_pipeline(&self.lod);
            for (&(x, z), (m, y0, y1)) in &self.lod_meshes {
                let min = Vec3::new(x as f32 * 512.0, *y0, z as f32 * 512.0);
                if visible(&lod_vp, min, Vec3::new(min.x + 512.0, *y1, min.z + 512.0)) {
                    pass.set_vertex_buffer(0, m.vertices.slice(..));
                    pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..m.count, 0, 0..1);
                }
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("world"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: if lod.is_some() { wgpu::LoadOp::Load } else { clear_color }, store: wgpu::StoreOp::Store },
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
            if world && lod.is_none() {
                pass.set_bind_group(0, &self.gui_bind_group, &[]);
                pass.set_pipeline(&self.sky_pipeline);
                pass.draw(0..3, 0..1);
                if celestial_count > 0 {
                    pass.set_pipeline(&self.celestial_pipeline);
                    pass.set_vertex_buffer(0, self.celestial_buffer.buffer.slice(..));
                    pass.draw(0..celestial_count, 0..1);
                }
            }
            if world {
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_pipeline(&self.opaque);
                for (c, _) in &visible_chunks {
                    if let Some(m) = &c.opaque {
                        pass.set_vertex_buffer(0, m.vertices.slice(..));
                        pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..m.count, 0, 0..1);
                    }
                }
                pass.set_pipeline(&self.cutout);
                if item_count > 0 {
                    pass.set_vertex_buffer(0, self.entity_item_buffer.buffer.slice(..));
                    pass.draw(0..item_count, 0..1);
                }
                if entity_count > 0 {
                    pass.set_bind_group(0, &self.gui_bind_group, &[]);
                    pass.set_vertex_buffer(0, self.entity_buffer.buffer.slice(..));
                    pass.draw(0..entity_count, 0..1);
                    pass.set_bind_group(0, &self.bind_group, &[]);
                }
                pass.set_pipeline(&self.translucent);
                for (c, _) in visible_chunks.iter().rev() {
                    if let Some(m) = &c.translucent {
                        pass.set_vertex_buffer(0, m.vertices.slice(..));
                        pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..m.count, 0, 0..1);
                    }
                }
                if quad_count > 0 {
                    pass.set_pipeline(&self.overlay_pipeline);
                    pass.set_vertex_buffer(0, self.overlay_buffer.buffer.slice(..));
                    pass.draw(0..quad_count, 0..1);
                }
                if line_count > 0 {
                    pass.set_vertex_buffer(0, self.lines.buffer.slice(..));
                    pass.set_pipeline(&self.lines_world);
                    pass.draw(0..world_lines, 0..1);
                    pass.set_pipeline(&self.lines_top);
                    pass.draw(world_lines..line_count, 0..1);
                }
            }
        }
        {
            // First-person hand and held item, then the GUI: a fresh depth buffer so the hand never clips into walls.
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hand and gui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            if world && hand_count > 0 {
                pass.set_pipeline(&self.hand);
                pass.set_vertex_buffer(0, self.hand_buffer.buffer.slice(..));
                if skin_count > 0 {
                    pass.set_bind_group(0, &self.gui_bind_group, &[]);
                    pass.draw(0..skin_count, 0..1);
                }
                if hand_count > skin_count {
                    pass.set_bind_group(0, &self.bind_group, &[]);
                    pass.draw(skin_count..hand_count, 0..1);
                }
            }
            if gui_count > 0 {
                pass.set_vertex_buffer(0, self.gui_buffer.buffer.slice(..));
                let (sw, sh) = (self.config.width, self.config.height);
                for b in &gui.batches {
                    let end = b.end.min(gui_count);
                    if b.start >= end {
                        continue;
                    }
                    match b.clip {
                        Some([x, y, w, h]) => {
                            let (x, y) = (x.min(sw), y.min(sh));
                            let (w, h) = (w.min(sw - x), h.min(sh - y));
                            if w == 0 || h == 0 {
                                continue;
                            }
                            pass.set_scissor_rect(x, y, w, h);
                        }
                        None => pass.set_scissor_rect(0, 0, sw, sh),
                    }
                    let (pipeline, group) = match b.texture {
                        GuiTexture::Gui => (&self.gui_pipeline, &self.gui_bind_group),
                        GuiTexture::Blocks => (&self.gui_pipeline, &self.bind_group),
                        GuiTexture::Panorama => (&self.gui_pipeline, &self.panorama_bind_group),
                        GuiTexture::PanoramaBlur => (&self.gui_blur_pipeline, &self.panorama_bind_group),
                    };
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, group, &[]);
                    pass.draw(b.start..end, 0..1);
                }
            }
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        stats
    }
}

impl Renderer {
    /// Shows the frame each animated block texture (water, lava, fire, ...) has at game tick `tick`.
    pub fn animate(&mut self, atlas: &Atlas, tick: u64) {
        for (i, a) in atlas.animations.iter().enumerate() {
            let f = a.frame_at(tick);
            if self.anim_frames.get(i) == Some(&f) {
                continue;
            }
            self.anim_frames[i] = f;
            for (level, px) in a.frames[f].iter().enumerate() {
                let size = (16u32 >> level).max(1);
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.block_texture,
                        mip_level: level as u32,
                        origin: wgpu::Origin3d { x: a.x >> level, y: a.y >> level, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    px,
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size * 4), rows_per_image: Some(size) },
                    wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
                );
            }
        }
    }
}
