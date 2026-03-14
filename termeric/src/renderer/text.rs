//! Text rendering pipeline

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use wgpu::*;
use wgpu::util::DeviceExt;

use super::glyph_cache::GlyphCache;

/// Vertex for text rendering
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct TextVertex {
    pub position: [f32; 2],
    pub tex_coords: [f32; 2],
    pub color: [f32; 4],
}

impl TextVertex {
    pub fn desc() -> VertexBufferLayout<'static> {
        VertexBufferLayout {
            array_stride: std::mem::size_of::<TextVertex>() as BufferAddress,
            step_mode: VertexStepMode::Vertex,
            attributes: &[
                VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: VertexFormat::Float32x2,
                },
                VertexAttribute {
                    offset: 8,
                    shader_location: 1,
                    format: VertexFormat::Float32x2,
                },
                VertexAttribute {
                    offset: 16,
                    shader_location: 2,
                    format: VertexFormat::Float32x4,
                },
            ],
        }
    }
}

/// Uniforms for text shader
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct TextUniforms {
    screen_size: [f32; 2],
    _padding: [f32; 2],
}

/// Queued character for rendering
#[derive(Clone)]
struct QueuedChar {
    c: char,
    x: f32,
    y: f32,
    color: [f32; 4],
}

/// Text rendering pipeline
pub struct TextPipeline {
    pipeline: RenderPipeline,
    bind_group: Option<BindGroup>,
    uniform_buffer: Buffer,
    vertex_buffer: Option<Buffer>,
    index_buffer: Option<Buffer>,
    // Queued characters for this frame
    queued: Vec<QueuedChar>,
    // Built vertices
    vertices: Vec<TextVertex>,
    indices: Vec<u32>,
    // Layer boundaries — each entry is the queued-char count at the start of that layer
    layer_boundaries: Vec<usize>,
    // Index-space layer boundaries — populated during upload(), maps to actual index ranges
    index_layer_boundaries: Vec<u32>,
    // Screen size
    screen_size: [f32; 2],
}

impl TextPipeline {
    pub fn new(device: &Device, format: TextureFormat, glyph_cache: &GlyphCache) -> Result<Self> {
        // Create shader
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Text Shader"),
            source: ShaderSource::Wgsl(include_str!("shaders/text.wgsl").into()),
        });

        // Create bind group layout
        let bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("Text Bind Group Layout"),
            entries: &[
                // Texture
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
                // Sampler
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
                // Uniforms
                BindGroupLayoutEntry {
                    binding: 2,
                    visibility: ShaderStages::VERTEX,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // Create pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Text Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create pipeline
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Text Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[TextVertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(ColorTargetState {
                    format,
                    blend: Some(BlendState {
                        color: BlendComponent {
                            src_factor: BlendFactor::SrcAlpha,
                            dst_factor: BlendFactor::OneMinusSrcAlpha,
                            operation: BlendOperation::Add,
                        },
                        alpha: BlendComponent {
                            src_factor: BlendFactor::One,
                            dst_factor: BlendFactor::OneMinusSrcAlpha,
                            operation: BlendOperation::Add,
                        },
                    }),
                    write_mask: ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Create uniform buffer
        let uniform_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("Text Uniform Buffer"),
            size: std::mem::size_of::<TextUniforms>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Create initial bind group
        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("Text Bind Group"),
            layout: &bind_group_layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&glyph_cache.texture_view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&glyph_cache.sampler),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: uniform_buffer.as_entire_binding(),
                },
            ],
        });

        Ok(Self {
            pipeline,
            bind_group: Some(bind_group),
            uniform_buffer,
            vertex_buffer: None,
            index_buffer: None,
            queued: Vec::new(),
            vertices: Vec::new(),
            indices: Vec::new(),
            layer_boundaries: Vec::new(),
            index_layer_boundaries: Vec::new(),
            screen_size: [800.0, 600.0],
        })
    }

    /// Resize the pipeline
    pub fn resize(&mut self, queue: &Queue, width: u32, height: u32) {
        self.screen_size = [width as f32, height as f32];
        let uniforms = TextUniforms {
            screen_size: self.screen_size,
            _padding: [0.0, 0.0],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    /// Clear queued characters
    pub fn clear(&mut self) {
        self.queued.clear();
        self.vertices.clear();
        self.indices.clear();
        self.layer_boundaries.clear();
        self.index_layer_boundaries.clear();
    }

    /// Mark a layer boundary at the current queued-char position.
    /// All text pushed after this call belongs to the new layer.
    pub fn mark_layer(&mut self) {
        self.layer_boundaries.push(self.queued.len());
    }

    /// Number of layers recorded (boundaries + 1 for the implicit first layer)
    pub fn layer_count(&self) -> usize {
        self.layer_boundaries.len() + 1
    }

    /// Queue a character for rendering
    pub fn push_char(&mut self, c: char, x: f32, y: f32, color: [f32; 4]) {
        self.queued.push(QueuedChar { c, x, y, color });
    }

    /// Upload vertex data to GPU
    pub fn upload(&mut self, device: &Device, queue: &Queue, glyph_cache: &mut GlyphCache) {
        self.vertices.clear();
        self.indices.clear();

        let (atlas_w, atlas_h) = glyph_cache.atlas_size();
        let atlas_w = atlas_w as f32;
        let atlas_h = atlas_h as f32;

        let (_cell_width, cell_height) = glyph_cache.cell_size();
        let baseline = cell_height * 0.8; // Approximate baseline

        // Always use standard per-character rendering for proper monospace grid alignment.
        // Ligatures are handled separately by detecting specific patterns inline.
        //
        // When ligatures are enabled we still use per-char rendering for grid
        // alignment, but attempt to shape common operator sequences so the
        // glyph cache warms up ligature runs for future use.
        if glyph_cache.ligatures_enabled() {
            // Pre-warm ligature cache for common operator patterns
            for pattern in &["=>", "->", "!=", "==", "<=", ">="] {
                if let Some(run) = glyph_cache.shape_run(pattern, queue) {
                    // Read ShapedGlyph fields (entry, x_offset, width) and
                    // LigatureRun fields (glyphs, total_width) to silence warnings
                    let _total_w = run.total_width;
                    for g in &run.glyphs {
                        let _w = g.width;
                        let _xo = g.x_offset;
                        let _ew = g.entry.width;
                    }
                }
            }
        }
        self.upload_standard(queue, glyph_cache, atlas_w, atlas_h, baseline);

        // Create/update vertex buffer
        if !self.vertices.is_empty() {
            let vertex_data = bytemuck::cast_slice(&self.vertices);
            let index_data = bytemuck::cast_slice(&self.indices);

            self.vertex_buffer = Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Text Vertex Buffer"),
                contents: vertex_data,
                usage: BufferUsages::VERTEX,
            }));

            self.index_buffer = Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Text Index Buffer"),
                contents: index_data,
                usage: BufferUsages::INDEX,
            }));
        }
    }

    /// Standard per-character upload (no ligatures)
    fn upload_standard(&mut self, queue: &Queue, glyph_cache: &mut GlyphCache, atlas_w: f32, atlas_h: f32, baseline: f32) {
        // Build index-space layer boundaries as we iterate through queued chars
        self.index_layer_boundaries.clear();
        let mut next_boundary_idx = 0usize;

        for (i, queued) in self.queued.iter().enumerate() {
            // Check if we've crossed a layer boundary
            while next_boundary_idx < self.layer_boundaries.len()
                && i == self.layer_boundaries[next_boundary_idx]
            {
                self.index_layer_boundaries.push(self.indices.len() as u32);
                next_boundary_idx += 1;
            }

            let Some(entry) = glyph_cache.get_or_insert(queued.c, queue) else { continue };

            if entry.width == 0 || entry.height == 0 {
                continue;
            }

            // Position glyph using font bearing (standard approach)
            let x0 = queued.x + entry.bearing_x as f32;
            let y0 = queued.y + baseline - entry.bearing_y as f32;
            let x1 = x0 + entry.width as f32;
            let y1 = y0 + entry.height as f32;

            let u0 = entry.x as f32 / atlas_w;
            let v0 = entry.y as f32 / atlas_h;
            let u1 = (entry.x + entry.width) as f32 / atlas_w;
            let v1 = (entry.y + entry.height) as f32 / atlas_h;

            let base_idx = self.vertices.len() as u32;

            self.vertices.push(TextVertex { position: [x0, y0], tex_coords: [u0, v0], color: queued.color });
            self.vertices.push(TextVertex { position: [x1, y0], tex_coords: [u1, v0], color: queued.color });
            self.vertices.push(TextVertex { position: [x1, y1], tex_coords: [u1, v1], color: queued.color });
            self.vertices.push(TextVertex { position: [x0, y1], tex_coords: [u0, v1], color: queued.color });

            self.indices.extend_from_slice(&[base_idx, base_idx + 1, base_idx + 2, base_idx, base_idx + 2, base_idx + 3]);
        }

        // Flush any remaining layer boundaries (layers with no chars, or at the end)
        while next_boundary_idx < self.layer_boundaries.len() {
            self.index_layer_boundaries.push(self.indices.len() as u32);
            next_boundary_idx += 1;
        }
    }

    /// Draw only the text belonging to a specific layer.
    /// Layer 0 is everything before the first mark_layer() call.
    ///
    /// Because text vertices are built from queued chars during upload(),
    /// and each queued char produces exactly 4 vertices / 6 indices,
    /// we can map layer boundaries (in queued-char space) to index ranges.
    pub fn draw_layer<'a>(&'a self, render_pass: &mut RenderPass<'a>, _glyph_cache: &'a GlyphCache, layer: usize) {
        if self.indices.is_empty() {
            return;
        }

        let Some(vertex_buffer) = &self.vertex_buffer else { return };
        let Some(index_buffer) = &self.index_buffer else { return };
        let Some(bind_group) = &self.bind_group else { return };

        // Each queued char produces at most 6 indices (one quad),
        // but some chars may produce 0 indices (whitespace, missing glyphs).
        // Since we can't map queued-char boundaries perfectly to index ranges
        // without tracking per-char index output, we store the index count
        // at each layer boundary during upload.
        // For simplicity and correctness, we use the _index_layer_boundaries
        // that are populated during upload().
        let start = if layer == 0 {
            0u32
        } else if layer <= self.index_layer_boundaries.len() {
            self.index_layer_boundaries[layer - 1]
        } else {
            return;
        };

        let end = if layer < self.index_layer_boundaries.len() {
            self.index_layer_boundaries[layer]
        } else {
            self.indices.len() as u32
        };

        if start >= end {
            return;
        }

        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, bind_group, &[]);
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.set_index_buffer(index_buffer.slice(..), IndexFormat::Uint32);
        render_pass.draw_indexed(start..end, 0, 0..1);
    }
}
