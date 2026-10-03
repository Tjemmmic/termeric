//! Rectangle rendering pipeline for backgrounds and cursor

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use wgpu::*;

use super::buffer::GrowableBuffer;
use super::text::TextClip;

/// Vertex for rectangle rendering (with rounded-corner metadata)
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct RectVertex {
    pub position: [f32; 2],
    pub color: [f32; 4],
    pub rect_center: [f32; 2],
    pub rect_half_size: [f32; 2],
    pub corner_radius: f32,
    /// Outline width; zero fills the shape.
    pub stroke: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct RectBatch {
    pub vertices: Vec<RectVertex>,
    pub indices: Vec<u32>,
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
                // stroke
                VertexAttribute {
                    offset: 44,
                    shader_location: 5,
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
    vertex_buffer: GrowableBuffer,
    index_buffer: GrowableBuffer,
    // Built vertices and indices
    vertices: Vec<RectVertex>,
    indices: Vec<u32>,
    // Layer boundaries — each entry is the index count at the start of that layer
    layer_boundaries: Vec<u32>,
    // Screen size
    screen_size: [f32; 2],
    // Bounds applied to every shape pushed while set.
    clip: Option<TextClip>,
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
            vertex_buffer: GrowableBuffer::new("Rect Vertex Buffer", BufferUsages::VERTEX),
            index_buffer: GrowableBuffer::new("Rect Index Buffer", BufferUsages::INDEX),
            vertices: Vec::new(),
            indices: Vec::new(),
            layer_boundaries: Vec::new(),
            screen_size: [800.0, 600.0],
            clip: None,
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
        self.clip = None;
    }

    /// Clips every shape pushed from now on to `clip`, or stops clipping.
    /// Clipping cuts a shape's quad but keeps its outline, so a rounded card
    /// scrolled half out of view keeps its visible corners.
    pub(super) fn set_clip(&mut self, clip: Option<TextClip>) -> Option<TextClip> {
        std::mem::replace(&mut self.clip, clip)
    }

    /// The quad `x0..x1, y0..y1` cut to the current clip, if any remains.
    fn clipped(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Option<[f32; 4]> {
        let [x0, y0, x1, y1] = match self.clip {
            Some(clip) => [
                x0.max(clip.left),
                y0.max(clip.top),
                x1.min(clip.right),
                y1.min(clip.bottom),
            ],
            None => [x0, y0, x1, y1],
        };
        (x0 < x1 && y0 < y1).then_some([x0, y0, x1, y1])
    }

    fn push_quad(&mut self, quad: [f32; 4], template: RectVertex) {
        let [x0, y0, x1, y1] = quad;
        let base_idx = self.vertices.len() as u32;
        for position in [[x0, y0], [x1, y0], [x1, y1], [x0, y1]] {
            self.vertices.push(RectVertex {
                position,
                ..template
            });
        }
        self.indices.extend_from_slice(&[
            base_idx,
            base_idx + 1,
            base_idx + 2,
            base_idx,
            base_idx + 2,
            base_idx + 3,
        ]);
    }

    pub(super) fn checkpoint(&self) -> (usize, usize) {
        (self.vertices.len(), self.indices.len())
    }

    /// Remove and return geometry added since `checkpoint`, normalizing its
    /// indices so the batch can live in an independent GPU buffer.
    pub(super) fn take_since(&mut self, checkpoint: (usize, usize)) -> RectBatch {
        let (vertex_start, index_start) = checkpoint;
        let vertices = self.vertices.split_off(vertex_start);
        let mut indices = self.indices.split_off(index_start);
        for index in &mut indices {
            *index -= vertex_start as u32;
        }
        RectBatch { vertices, indices }
    }

    pub(super) fn draw_external<'a>(
        &'a self,
        render_pass: &mut RenderPass<'a>,
        vertex_buffer: &'a Buffer,
        index_buffer: &'a Buffer,
        index_count: u32,
    ) {
        if index_count == 0 {
            return;
        }
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.bind_group, &[]);
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.set_index_buffer(index_buffer.slice(..), IndexFormat::Uint32);
        render_pass.draw_indexed(0..index_count, 0, 0..1);
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
        let half_t = thickness / 2.0;
        let pad = half_t + 1.0; // Extra pixel for anti-aliasing

        // Axis-aligned bounding box of the line with padding
        let Some(quad) = self.clipped(
            x1.min(x2) - pad,
            y1.min(y2) - pad,
            x1.max(x2) + pad,
            y1.max(y2) + pad,
        ) else {
            return;
        };

        // Encode line endpoints in rect_center (start) and rect_half_size (end)
        // Negative corner_radius signals the shader to use line SDF
        self.push_quad(
            quad,
            RectVertex {
                position: [0.0, 0.0],
                color,
                rect_center: [x1, y1],
                rect_half_size: [x2, y2],
                corner_radius: -half_t,
                stroke: 0.0,
            },
        );
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
        self.push_shape(x, y, width, height, color, radius, 0.0);
    }

    /// Push the outline of a rounded rectangle, `stroke` pixels wide and
    /// drawn inside its bounds.
    pub fn push_rounded_outline(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: [f32; 4],
        radius: f32,
        stroke: f32,
    ) {
        if stroke > 0.0 {
            self.push_shape(x, y, width, height, color, radius, stroke);
        }
    }

    fn push_shape(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: [f32; 4],
        radius: f32,
        stroke: f32,
    ) {
        let Some(quad) = self.clipped(x, y, x + width, y + height) else {
            return;
        };
        // A radius beyond half the shorter side has no shape to round; the
        // largest possible one draws the intended pill or circle.
        let radius = radius.min(width.min(height) * 0.5).max(0.0);
        self.push_quad(
            quad,
            RectVertex {
                position: [0.0, 0.0],
                color,
                rect_center: [x + width * 0.5, y + height * 0.5],
                rect_half_size: [width * 0.5, height * 0.5],
                corner_radius: radius,
                stroke,
            },
        );
    }

    /// Upload vertex data to GPU
    pub fn upload(&mut self, device: &Device, queue: &Queue) {
        if self.vertices.is_empty() {
            return;
        }

        let vertex_data = bytemuck::cast_slice(&self.vertices);
        let index_data = bytemuck::cast_slice(&self.indices);

        self.vertex_buffer.upload(device, queue, vertex_data);
        self.index_buffer.upload(device, queue, index_data);
    }

    #[cfg(test)]
    pub(super) fn buffer_allocations(&self) -> u64 {
        self.vertex_buffer.allocations() + self.index_buffer.allocations()
    }

    /// Draw only the rectangles belonging to a specific layer.
    /// Layer 0 is everything before the first mark_layer() call.
    pub fn draw_layer<'a>(&'a self, render_pass: &mut RenderPass<'a>, layer: usize) {
        if self.indices.is_empty() {
            return;
        }

        let Some(vertex_buffer) = self.vertex_buffer.buffer() else {
            return;
        };
        let Some(index_buffer) = self.index_buffer.buffer() else {
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
