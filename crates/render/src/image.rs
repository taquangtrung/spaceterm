//! Textured-quad pass: draws decoded raster images on the GPU, so image blocks
//! render natively without a webview. Textures are uploaded once and cached by
//! block id; each frame the caller supplies pixel placements and the pass draws
//! the visible ones.

use std::collections::HashMap;

use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingResource, BindingType, BlendState, Buffer, BufferDescriptor,
    BufferUsages, ColorTargetState, ColorWrites, Device, Extent3d, FilterMode, FragmentState,
    FrontFace, MultisampleState, Origin3d, PipelineLayoutDescriptor, PrimitiveState,
    PrimitiveTopology, Queue, RenderPass, RenderPipeline, RenderPipelineDescriptor,
    SamplerBindingType, SamplerDescriptor, ShaderModuleDescriptor, ShaderSource, ShaderStages,
    TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect, TextureDescriptor,
    TextureDimension, TextureFormat, TextureSampleType, TextureUsages, TextureViewDescriptor,
    TextureViewDimension, VertexAttribute, VertexBufferLayout, VertexFormat, VertexState,
    VertexStepMode,
};

// ========================================================================
// Constants
// ========================================================================

const IMAGE_SHADER: &str = include_str!("image.wgsl");
const MAX_QUADS: usize = 256;
const VERTEX_BYTES: usize = 16;
const VERTS_PER_QUAD: u32 = 6;

// ========================================================================
// Data Structures
// ========================================================================

/// Where to draw a cached image this frame, in pixels from the surface
/// top-left. `id` keys the uploaded texture. `v_max` is the bottom texture
/// coordinate (1.0 = whole image); values below 1.0 clip the bottom, used to
/// keep a tall block inside the content area without squashing it.
#[derive(Clone, Copy, Debug)]
pub struct ImagePlacement {
    pub height: f32,
    pub id: u64,
    pub v_max: f32,
    pub width: f32,
    pub x: f32,
    pub y: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ImageVertex {
    u: f32,
    v: f32,
    x: f32,
    y: f32,
}

/// GPU pipeline plus a per-image texture/bind-group cache.
pub struct ImagePass {
    bind_group_layout: BindGroupLayout,
    draws: Vec<(u64, u32)>,
    pipeline: RenderPipeline,
    sampler: wgpu::Sampler,
    textures: HashMap<u64, BindGroup>,
    vertex_buffer: Buffer,
}

// ========================================================================
// ImageVertex
// ========================================================================

impl ImageVertex {
    fn new(x: f32, y: f32, u: f32, v: f32) -> Self {
        Self { u, v, x, y }
    }

    fn to_bytes(self) -> [u8; VERTEX_BYTES] {
        let mut out = [0u8; VERTEX_BYTES];
        out[0..4].copy_from_slice(&self.x.to_le_bytes());
        out[4..8].copy_from_slice(&self.y.to_le_bytes());
        out[8..12].copy_from_slice(&self.u.to_le_bytes());
        out[12..16].copy_from_slice(&self.v.to_le_bytes());
        out
    }
}

// ========================================================================
// ImagePass
// ========================================================================

impl ImagePass {
    pub fn new(device: &Device, format: TextureFormat) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("spaceterm image shader"),
            source: ShaderSource::Wgsl(IMAGE_SHADER.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("spaceterm image bind group layout"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("spaceterm image layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("spaceterm image pipeline"),
            layout: Some(&layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[VertexBufferLayout {
                    array_stride: VERTEX_BYTES as u64,
                    step_mode: VertexStepMode::Vertex,
                    attributes: &[
                        VertexAttribute {
                            offset: 0,
                            format: VertexFormat::Float32x2,
                            shader_location: 0,
                        },
                        VertexAttribute {
                            offset: 8,
                            format: VertexFormat::Float32x2,
                            shader_location: 1,
                        },
                    ],
                }],
            },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(ColorTargetState {
                    format,
                    blend: Some(BlendState::ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
            }),
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: MultisampleState::default(),
            cache: None,
            multiview_mask: None,
        });

        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("spaceterm image sampler"),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });

        let vertex_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("spaceterm image vertices"),
            size: (MAX_QUADS * VERTS_PER_QUAD as usize * VERTEX_BYTES) as u64,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            bind_group_layout,
            draws: Vec::new(),
            pipeline,
            sampler,
            textures: HashMap::new(),
            vertex_buffer,
        }
    }

    /// Whether a texture is already cached for `id`.
    pub fn has(&self, id: u64) -> bool {
        self.textures.contains_key(&id)
    }

    /// Upload `rgba` (tightly packed, `width * height * 4` bytes) as a texture
    /// cached under `id`, replacing any previous one.
    pub fn upload(
        &mut self,
        device: &Device,
        queue: &Queue,
        id: u64,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) {
        let size = Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("spaceterm image texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            // Non-sRGB: sampled bytes are written to the non-sRGB surface as-is,
            // matching how the bg/text passes treat color in this renderer.
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            rgba,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            size,
        );

        let view = texture.create_view(&TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("spaceterm image bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.textures.insert(id, bind_group);
    }

    /// Build the vertex buffer for this frame's `placements`. Placements with no
    /// cached texture, or beyond [`MAX_QUADS`], are skipped.
    pub fn prepare(
        &mut self,
        queue: &Queue,
        placements: &[ImagePlacement],
        surface_w: f32,
        surface_h: f32,
    ) {
        self.draws.clear();
        let mut verts: Vec<ImageVertex> = Vec::new();
        for placement in placements {
            if !self.textures.contains_key(&placement.id) || self.draws.len() >= MAX_QUADS {
                continue;
            }
            let x0 = placement.x / surface_w * 2.0 - 1.0;
            let x1 = (placement.x + placement.width) / surface_w * 2.0 - 1.0;
            let y0 = 1.0 - placement.y / surface_h * 2.0;
            let y1 = 1.0 - (placement.y + placement.height) / surface_h * 2.0;
            let vm = placement.v_max;
            let first = verts.len() as u32;
            verts.extend_from_slice(&[
                ImageVertex::new(x0, y0, 0.0, 0.0),
                ImageVertex::new(x1, y0, 1.0, 0.0),
                ImageVertex::new(x0, y1, 0.0, vm),
                ImageVertex::new(x0, y1, 0.0, vm),
                ImageVertex::new(x1, y0, 1.0, 0.0),
                ImageVertex::new(x1, y1, 1.0, vm),
            ]);
            self.draws.push((placement.id, first));
        }
        if !verts.is_empty() {
            let bytes: Vec<u8> = verts.iter().flat_map(|v| v.to_bytes()).collect();
            queue.write_buffer(&self.vertex_buffer, 0, &bytes);
        }
    }

    /// Draw the placements prepared this frame. Call inside an active pass after
    /// the background and text have been drawn.
    pub fn render<'pass>(&'pass self, pass: &mut RenderPass<'pass>) {
        if self.draws.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        for (id, first) in &self.draws {
            if let Some(bind_group) = self.textures.get(id) {
                pass.set_bind_group(0, bind_group, &[]);
                pass.draw(*first..*first + VERTS_PER_QUAD, 0..1);
            }
        }
    }
}
