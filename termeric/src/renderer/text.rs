//! Text rendering pipeline

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use wgpu::*;
use wgpu::util::DeviceExt;

use super::glyph_cache::{GlyphCache, LigatureRun};

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
    bind_group_layout: BindGroupLayout,
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
            bind_group_layout,
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

    /// Upload with ligature shaping for potential ligature sequences
    ///
    /// This method scans for common ligature-forming character sequences and only
    /// shapes those specific runs. Regular text is rendered using standard
    /// per-character rendering to maintain proper monospace grid alignment.
    fn upload_with_ligatures(&mut self, queue: &Queue, glyph_cache: &mut GlyphCache, atlas_w: f32, atlas_h: f32, cell_width: f32, baseline: f32) {
        // Sort queued chars by y then x to ensure proper run detection
        let mut sorted: Vec<_> = self.queued.iter().cloned().collect();
        sorted.sort_by(|a, b| {
            let y_cmp = a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal);
            if y_cmp != std::cmp::Ordering::Equal {
                return y_cmp;
            }
            a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal)
        });

        // Group into runs of consecutive characters with same y and color.
        // Each run tracks the original queued characters so we can preserve their positions.
        // Run format: (start_x, y, color, chars_with_positions)
        let mut runs: Vec<(f32, f32, [f32; 4], Vec<(char, f32)>)> = Vec::new();

        for queued in &sorted {
            let can_extend = if let Some((_, run_y, run_color, run_chars)) = runs.last_mut() {
                // Check if this char continues the current run
                // Infer spacing from last character's position
                let last_x = run_chars.last().map(|(_, x)| *x).unwrap_or(0.0);
                let spacing = queued.x - last_x;

                // Consider consecutive if spacing is reasonable (close to cell_width)
                let same_row = (queued.y - *run_y).abs() < 0.1;
                let consecutive = spacing > cell_width * 0.5 && spacing < cell_width * 1.5;
                let same_color = queued.color == *run_color;

                if same_row && consecutive && same_color {
                    run_chars.push((queued.c, queued.x));
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if !can_extend {
                // Start new run
                runs.push((queued.x, queued.y, queued.color, vec![(queued.c, queued.x)]));
            }
        }

        // Process each run
        for (start_x, y, color, chars) in runs {
            let char_count = chars.len();

            // Only try ligature shaping for runs of 2+ chars that might form ligatures
            // Check if this run contains potential ligature-forming characters
            let text: String = chars.iter().map(|(c, _)| *c).collect();
            let may_have_ligatures = char_count >= 2 && Self::may_form_ligature(&text);

            if may_have_ligatures {
                if let Some(run) = glyph_cache.shape_run(&text, queue) {
                    // Use shaped rendering for potential ligature runs
                    // This handles both true ligatures (fewer glyphs) and contextual
                    // alternates like FiraCode arrows (same glyph count, different shapes)
                    if !run.glyphs.is_empty() {
                        self.push_shaped_run(&run, start_x, y, color, baseline, atlas_w, atlas_h, cell_width, char_count);
                        continue;
                    }
                }
            }

            // Standard per-character rendering at original grid positions
            // This preserves exact monospace alignment for regular text
            for (c, char_x) in chars {
                if let Some(entry) = glyph_cache.get_or_insert(c, queue) {
                    if entry.width > 0 && entry.height > 0 {
                        let x0 = char_x + entry.bearing_x as f32;
                        let y0 = y + baseline - entry.bearing_y as f32;
                        let x1 = x0 + entry.width as f32;
                        let y1 = y0 + entry.height as f32;

                        let u0 = entry.x as f32 / atlas_w;
                        let v0 = entry.y as f32 / atlas_h;
                        let u1 = (entry.x + entry.width) as f32 / atlas_w;
                        let v1 = (entry.y + entry.height) as f32 / atlas_h;

                        let base_idx = self.vertices.len() as u32;
                        self.vertices.push(TextVertex { position: [x0, y0], tex_coords: [u0, v0], color });
                        self.vertices.push(TextVertex { position: [x1, y0], tex_coords: [u1, v0], color });
                        self.vertices.push(TextVertex { position: [x1, y1], tex_coords: [u1, v1], color });
                        self.vertices.push(TextVertex { position: [x0, y1], tex_coords: [u0, v1], color });
                        self.indices.extend_from_slice(&[base_idx, base_idx + 1, base_idx + 2, base_idx, base_idx + 2, base_idx + 3]);
                    }
                }
            }
        }
    }

    /// Check if a text run might contain ligature-forming character sequences
    ///
    /// This checks for common programming ligatures like arrows, comparisons,
    /// and other sequences that fonts like FiraCode, JetBrains Mono support.
    fn may_form_ligature(text: &str) -> bool {
        // Common ligature patterns in programming fonts
        const LIGATURE_PATTERNS: &[&str] = &[
            // Arrows
            "->", "=>", "<-", "-->", "<--", "==>", "<==",
            "->-", "->>", "<<-", ">>=", "=<<",
            // Comparisons
            "==", "!=", "<=", ">=", "===", "!==", "=/=",
            "<>", "<<", ">>", "<<<", ">>>",
            // Logic
            "&&", "||", "!!", "??", "?.", "?:",
            // Math/operators
            "++", "--", "**", "//", "/*", "*/", "///",
            ":::", "::", "..", "...", "..<",
            // Haskell/functional
            "<$>", "<*>", "<|>", "|>", "<|", ">>=", "=<<",
            // Other
            "www", "fi", "fl", "ff", "ffi", "ffl",
            "__", "##", "###",
        ];

        for pattern in LIGATURE_PATTERNS {
            if text.contains(pattern) {
                return true;
            }
        }
        false
    }

    /// Push a shaped run using cosmic-text's glyph positions, scaled to terminal grid
    ///
    /// This renders glyphs at positions scaled from cosmic-text's shaping to fit
    /// within our terminal grid cells. This ensures proper spacing regardless of
    /// font metrics.
    fn push_shaped_run(
        &mut self,
        run: &LigatureRun,
        start_x: f32,
        y: f32,
        color: [f32; 4],
        baseline: f32,
        atlas_w: f32,
        atlas_h: f32,
        cell_width: f32,
        char_count: usize,
    ) {
        // Calculate scale factor to fit shaped glyphs into terminal grid
        // cosmic-text may use different advance widths than our cell_width
        let total_grid_width = char_count as f32 * cell_width;
        let scale = if run.total_width > 0.0 {
            total_grid_width / run.total_width
        } else {
            1.0
        };

        for shaped in &run.glyphs {
            let entry = &shaped.entry;

            // Skip empty glyphs
            if entry.width == 0 || entry.height == 0 {
                continue;
            }

            // Scale cosmic-text's x_offset to fit our terminal grid
            let scaled_x_offset = shaped.x_offset * scale;
            let gx0 = start_x + scaled_x_offset + entry.bearing_x as f32;
            let gy0 = y + baseline - entry.bearing_y as f32;
            let gx1 = gx0 + entry.width as f32;
            let gy1 = gy0 + entry.height as f32;

            // Calculate texture coordinates
            let u0 = entry.x as f32 / atlas_w;
            let v0 = entry.y as f32 / atlas_h;
            let u1 = (entry.x + entry.width) as f32 / atlas_w;
            let v1 = (entry.y + entry.height) as f32 / atlas_h;

            let base_idx = self.vertices.len() as u32;

            // Push quad vertices
            self.vertices.push(TextVertex {
                position: [gx0, gy0],
                tex_coords: [u0, v0],
                color,
            });
            self.vertices.push(TextVertex {
                position: [gx1, gy0],
                tex_coords: [u1, v0],
                color,
            });
            self.vertices.push(TextVertex {
                position: [gx1, gy1],
                tex_coords: [u1, v1],
                color,
            });
            self.vertices.push(TextVertex {
                position: [gx0, gy1],
                tex_coords: [u0, v1],
                color,
            });

            // Push indices for two triangles
            self.indices.extend_from_slice(&[
                base_idx,
                base_idx + 1,
                base_idx + 2,
                base_idx,
                base_idx + 2,
                base_idx + 3,
            ]);
        }
    }

    /// Draw ALL text (legacy single-pass method, kept for compatibility)
    pub fn draw<'a>(&'a self, render_pass: &mut RenderPass<'a>, _glyph_cache: &'a GlyphCache) {
        if self.indices.is_empty() {
            return;
        }

        let Some(vertex_buffer) = &self.vertex_buffer else { return };
        let Some(index_buffer) = &self.index_buffer else { return };
        let Some(bind_group) = &self.bind_group else { return };

        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, bind_group, &[]);
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.set_index_buffer(index_buffer.slice(..), IndexFormat::Uint32);
        render_pass.draw_indexed(0..self.indices.len() as u32, 0, 0..1);
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
