//! Rectangle rendering pipeline for backgrounds and cursor

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use wgpu::*;

/// Vertex for rectangle rendering (with rounded-corner metadata)
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct RectVertex {
    pub position: [f32; 2],
    pub color: [f32; 4],
    pub rect_center: [f32; 2],
    pub rect_half_size: [f32; 2],
    pub corner_radius: f32,
    pub _pad: f32,
}

impl RectVertex {
    pub fn desc() -> VertexBufferLayout<'static> {
        VertexBufferLayout {
            array_stride: std::mem::size_of::<RectVertex>() as BufferAddress,
            step_mode: VertexStepMode::Vertex,
            attributes: &[
                // position
                VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: VertexFormat::Float32x2,
                },
                // color
                VertexAttribute {
                    offset: 8,
                    shader_location: 1,
                    format: VertexFormat::Float32x4,
                },
                // rect_center
                VertexAttribute {
                    offset: 24,
                    shader_location: 2,
                    format: VertexFormat::Float32x2,
                },
                // rect_half_size
                VertexAttribute {
                    offset: 32,
                    shader_location: 3,
                    format: VertexFormat::Float32x2,
                },
                // corner_radius
                VertexAttribute {
                    offset: 40,
                    shader_location: 4,
                    format: VertexFormat::Float32,
                },
            ],
        }
    }
}

/// Uniforms for rect shader
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct RectUniforms {
    screen_size: [f32; 2],
    _padding: [f32; 2],
}

/// Rectangle rendering pipeline
pub struct RectPipeline {
    pipeline: RenderPipeline,
    uniform_buffer: Buffer,
    bind_group: BindGroup,
    vertex_buffer: Option<Buffer>,
    index_buffer: Option<Buffer>,
    // Built vertices and indices
    vertices: Vec<RectVertex>,
    indices: Vec<u32>,
    // Layer boundaries — each entry is the index count at the start of that layer
    layer_boundaries: Vec<u32>,
    // Screen size
    screen_size: [f32; 2],
}

impl RectPipeline {
    pub fn new(device: &Device, format: TextureFormat) -> Result<Self> {
        // Create shader
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Rect Shader"),
            source: ShaderSource::Wgsl(include_str!("shaders/rect.wgsl").into()),
        });

        // Create bind group layout
        let bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("Rect Bind Group Layout"),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VERTEX,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        // Create pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Rect Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create pipeline
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Rect Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[RectVertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(ColorTargetState {
                    format,
                    blend: Some(BlendState::ALPHA_BLENDING),
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
            label: Some("Rect Uniform Buffer"),
            size: std::mem::size_of::<RectUniforms>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Create bind group
        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("Rect Bind Group"),
            layout: &bind_group_layout,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        Ok(Self {
            pipeline,
            uniform_buffer,
            bind_group,
            vertex_buffer: None,
            index_buffer: None,
            vertices: Vec::new(),
            indices: Vec::new(),
            layer_boundaries: Vec::new(),
            screen_size: [800.0, 600.0],
        })
    }

    /// Resize the pipeline
    pub fn resize(&mut self, queue: &Queue, width: u32, height: u32) {
        self.screen_size = [width as f32, height as f32];
        let uniforms = RectUniforms {
            screen_size: self.screen_size,
            _padding: [0.0, 0.0],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    /// Clear queued rectangles
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
        self.layer_boundaries.clear();
    }

    /// Mark a layer boundary at the current index position.
    /// All rects pushed after this call belong to the new layer.
    pub fn mark_layer(&mut self) {
        self.layer_boundaries.push(self.indices.len() as u32);
    }

    /// Number of layers recorded (boundaries + 1 for the implicit first layer)
    pub fn layer_count(&self) -> usize {
        self.layer_boundaries.len() + 1
    }

    /// Push a line segment from (x1,y1) to (x2,y2) with the given thickness.
    /// This uses the line segment SDF in the shader for perfect anti-aliasing at any angle.
    pub fn push_line(
        &mut self,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        thickness: f32,
        color: [f32; 4],
    ) {
        let base_idx = self.vertices.len() as u32;
        let half_t = thickness / 2.0;
        let pad = half_t + 1.0; // Extra pixel for anti-aliasing

        // Axis-aligned bounding box of the line with padding
        let min_x = x1.min(x2) - pad;
        let min_y = y1.min(y2) - pad;
        let max_x = x1.max(x2) + pad;
        let max_y = y1.max(y2) + pad;

        // Encode line endpoints in rect_center (start) and rect_half_size (end)
        // Negative corner_radius signals the shader to use line SDF
        let make_vert = |pos: [f32; 2]| RectVertex {
            position: pos,
            color,
            rect_center: [x1, y1],
            rect_half_size: [x2, y2],
            corner_radius: -half_t,
            _pad: 0.0,
        };

        self.vertices.push(make_vert([min_x, min_y]));
        self.vertices.push(make_vert([max_x, min_y]));
        self.vertices.push(make_vert([max_x, max_y]));
        self.vertices.push(make_vert([min_x, max_y]));

        self.indices.extend_from_slice(&[
            base_idx,
            base_idx + 1,
            base_idx + 2,
            base_idx,
            base_idx + 2,
            base_idx + 3,
        ]);
    }

    /// Push a sharp rectangle (no rounding)
    pub fn push_rect(&mut self, x: f32, y: f32, width: f32, height: f32, color: [f32; 4]) {
        self.push_rounded_rect(x, y, width, height, color, 0.0);
    }

    /// Push a rounded rectangle with the given corner radius
    pub fn push_rounded_rect(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: [f32; 4],
        radius: f32,
    ) {
        let base_idx = self.vertices.len() as u32;

        let x0 = x;
        let y0 = y;
        let x1 = x + width;
        let y1 = y + height;

        let center = [x + width * 0.5, y + height * 0.5];
        let half_size = [width * 0.5, height * 0.5];

        let make_vert = |pos: [f32; 2]| RectVertex {
            position: pos,
            color,
            rect_center: center,
            rect_half_size: half_size,
            corner_radius: radius,
            _pad: 0.0,
        };

        self.vertices.push(make_vert([x0, y0]));
        self.vertices.push(make_vert([x1, y0]));
        self.vertices.push(make_vert([x1, y1]));
        self.vertices.push(make_vert([x0, y1]));

        self.indices.extend_from_slice(&[
            base_idx,
            base_idx + 1,
            base_idx + 2,
            base_idx,
            base_idx + 2,
            base_idx + 3,
        ]);
    }

    /// Upload vertex data to GPU
    pub fn upload(&mut self, device: &Device, _queue: &Queue) {
        if self.vertices.is_empty() {
            return;
        }

        let vertex_data = bytemuck::cast_slice(&self.vertices);
        let index_data = bytemuck::cast_slice(&self.indices);

        self.vertex_buffer = Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Rect Vertex Buffer"),
                contents: vertex_data,
                usage: BufferUsages::VERTEX,
            }),
        );

        self.index_buffer = Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Rect Index Buffer"),
                contents: index_data,
                usage: BufferUsages::INDEX,
            }),
        );
    }

    /// Draw only the rectangles belonging to a specific layer.
    /// Layer 0 is everything before the first mark_layer() call.
    pub fn draw_layer<'a>(&'a self, render_pass: &mut RenderPass<'a>, layer: usize) {
        if self.indices.is_empty() {
            return;
        }

        let Some(vertex_buffer) = &self.vertex_buffer else {
            return;
        };
        let Some(index_buffer) = &self.index_buffer else {
            return;
        };

        // Determine index range for this layer
        let start = if layer == 0 {
            0u32
        } else if layer <= self.layer_boundaries.len() {
            self.layer_boundaries[layer - 1]
        } else {
            return; // layer out of range
        };

        let end = if layer < self.layer_boundaries.len() {
            self.layer_boundaries[layer]
        } else {
            self.indices.len() as u32
        };

        if start >= end {
            return; // empty layer
        }

        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.bind_group, &[]);
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.set_index_buffer(index_buffer.slice(..), IndexFormat::Uint32);
        render_pass.draw_indexed(start..end, 0, 0..1);
    }
}
