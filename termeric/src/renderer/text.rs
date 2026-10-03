//! Text rendering pipeline

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use wgpu::*;

use super::buffer::{BufferStats, GrowableBuffer};
use super::glyph_cache::{FontStyle, GlyphCache, GlyphEntry};

/// Vertex for text rendering
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct TextVertex {
    pub position: [f32; 2],
    pub tex_coords: [f32; 2],
    pub color: [f32; 4],
    pub atlas_page: f32,
    pub color_glyph: f32,
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
                VertexAttribute {
                    offset: 32,
                    shader_location: 3,
                    format: VertexFormat::Float32,
                },
                VertexAttribute {
                    offset: 36,
                    shader_location: 4,
                    format: VertexFormat::Float32,
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

/// Complete text content queued at one terminal cell origin.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum QueuedContent {
    Char(char),
    Grapheme(Box<str>),
    /// One OSC 66 text block shaped at a multiple of the configured font.
    MultiCell {
        text: Box<str>,
        font_scale: f32,
    },
    /// A contiguous sequence of one-column terminal cells shaped together.
    Run {
        text: String,
        cell_width: f32,
    },
    /// Application-chrome text in a desktop UI face. Its queued `y` is the
    /// baseline rather than a cell top, and its weight and slant come from
    /// the queued style.
    UiText {
        text: Box<str>,
        size_px: f32,
        mono: bool,
    },
}

/// Pixel-space clipping bounds for terminal glyph overhangs at pane edges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TextClip {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// Queued character or grapheme for rendering
#[derive(Clone, Debug, PartialEq)]
pub(super) struct QueuedChar {
    pub content: QueuedContent,
    pub x: f32,
    pub y: f32,
    pub color: [f32; 4],
    pub style: FontStyle,
    pub clip: Option<TextClip>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct TextBatch {
    pub queued: Vec<QueuedChar>,
    pub vertices: Vec<TextVertex>,
    pub indices: Vec<u32>,
    pub atlas_pages: Vec<u16>,
}

/// Collects compatible adjacent terminal cells into logical shaping runs.
/// Whitespace, wide cells, graphemes, and attribute boundaries are flushed by
/// the caller so terminal cell semantics remain independent of typography.
#[derive(Clone, Debug, Default)]
pub(super) struct TextRunAccumulator {
    pending: Option<QueuedChar>,
    cell_count: usize,
}

impl TextRunAccumulator {
    pub(super) fn push(
        &mut self,
        pipeline: &mut TextPipeline,
        character: char,
        x: f32,
        y: f32,
        color: [f32; 4],
        style: FontStyle,
        clip: TextClip,
        cell_width: f32,
    ) {
        if let Some(queued) = self.accumulate(character, x, y, color, style, clip, cell_width) {
            pipeline.queued.push(queued);
        }
    }

    fn accumulate(
        &mut self,
        character: char,
        x: f32,
        y: f32,
        color: [f32; 4],
        style: FontStyle,
        clip: TextClip,
        cell_width: f32,
    ) -> Option<QueuedChar> {
        let compatible = self.pending.as_ref().is_some_and(|pending| {
            pending.y == y
                && pending.color == color
                && pending.style == style
                && pending.clip == Some(clip)
                && matches!(pending.content, QueuedContent::Run { cell_width: width, .. } if width == cell_width)
                && (pending.x + self.cell_count as f32 * cell_width - x).abs() <= 0.01
        });
        if !compatible {
            let flushed = self.take_pending();
            self.pending = Some(QueuedChar {
                content: QueuedContent::Run {
                    text: character.to_string(),
                    cell_width,
                },
                x,
                y,
                color,
                style,
                clip: Some(clip),
            });
            self.cell_count = 1;
            return flushed;
        }

        let Some(QueuedChar {
            content: QueuedContent::Run { text, .. },
            ..
        }) = self.pending.as_mut()
        else {
            unreachable!("compatible pending text is always a run");
        };
        text.push(character);
        self.cell_count += 1;
        None
    }

    pub(super) fn flush(&mut self, pipeline: &mut TextPipeline) {
        if let Some(pending) = self.take_pending() {
            pipeline.queued.push(pending);
        }
    }

    fn take_pending(&mut self) -> Option<QueuedChar> {
        let mut pending = self.pending.take()?;
        if self.cell_count == 1
            && let QueuedContent::Run { text, .. } = &pending.content
            && let Some(character) = text.chars().next()
        {
            pending.content = QueuedContent::Char(character);
        }
        self.cell_count = 0;
        Some(pending)
    }
}

fn queued_glyphs(
    queued: &QueuedChar,
    glyph_cache: &mut GlyphCache,
    queue: &Queue,
) -> Vec<(GlyphEntry, f32)> {
    match &queued.content {
        QueuedContent::Char(character) => glyph_cache
            .get_or_insert_styled(*character, queued.style, queue)
            .map(|entry| vec![(entry, 0.0)])
            .unwrap_or_default(),
        QueuedContent::Grapheme(grapheme) => glyph_cache
            .shape_grapheme_styled(grapheme, queued.style, queue)
            .map(|run| {
                run.glyphs
                    .into_iter()
                    .map(|glyph| (glyph.entry, glyph.x_offset))
                    .collect()
            })
            .unwrap_or_else(|| {
                // Atlas exhaustion or a shaping failure must not silently
                // truncate the cluster to its first scalar. Queue every
                // independently available glyph at the same cell origin.
                grapheme
                    .chars()
                    .filter_map(|character| {
                        glyph_cache
                            .get_or_insert_styled(character, queued.style, queue)
                            .map(|entry| (entry, 0.0))
                    })
                    .collect()
            }),
        QueuedContent::MultiCell { text, font_scale } => glyph_cache
            .shape_multicell_styled(text, queued.style, *font_scale, queue)
            .map(|run| {
                run.glyphs
                    .into_iter()
                    .map(|glyph| (glyph.entry, glyph.x_offset))
                    .collect()
            })
            .unwrap_or_default(),
        QueuedContent::UiText {
            text,
            size_px,
            mono,
        } => glyph_cache
            .shape_ui_text(
                text,
                *size_px,
                crate::ui::text_layout::UiFace {
                    mono: *mono,
                    bold: queued.style.bold,
                    italic: queued.style.italic,
                },
                queue,
            )
            .map(|run| {
                run.glyphs
                    .into_iter()
                    .map(|glyph| (glyph.entry, glyph.x_offset))
                    .collect()
            })
            .unwrap_or_default(),
        QueuedContent::Run { text, cell_width } => glyph_cache
            .shape_run_styled(text, queued.style, queue)
            .map(|run| {
                run.glyphs
                    .into_iter()
                    .map(|glyph| (glyph.entry, glyph.run_offset(*cell_width)))
                    .collect()
            })
            .unwrap_or_else(|| {
                text.chars()
                    .enumerate()
                    .filter_map(|(index, character)| {
                        glyph_cache
                            .get_or_insert_styled(character, queued.style, queue)
                            .map(|entry| (entry, index as f32 * *cell_width))
                    })
                    .collect()
            }),
    }
}

fn multicell_origin_and_clip(
    block_x: f32,
    block_y: f32,
    pane_clip: TextClip,
    cell_width: f32,
    cell_height: f32,
    descriptor: termeric_core::MultiCell,
) -> (f32, f32, TextClip) {
    let block_width = descriptor.columns() as f32 * cell_width;
    let block_height = descriptor.rows() as f32 * cell_height;
    let fraction = if descriptor.denominator() == 0 {
        1.0
    } else {
        f32::from(descriptor.numerator()) / f32::from(descriptor.denominator())
    };
    let align_offset =
        |remaining: f32, alignment: termeric_core::MultiCellAlignment| match alignment {
            termeric_core::MultiCellAlignment::Start => 0.0,
            termeric_core::MultiCellAlignment::End => remaining,
            termeric_core::MultiCellAlignment::Center => remaining * 0.5,
        };
    let x = block_x
        + align_offset(
            block_width * (1.0 - fraction),
            descriptor.horizontal_alignment(),
        );
    let y = block_y
        + align_offset(
            block_height * (1.0 - fraction),
            descriptor.vertical_alignment(),
        );
    let clip = TextClip {
        left: pane_clip.left.max(block_x),
        top: pane_clip.top.max(block_y),
        right: pane_clip.right.min(block_x + block_width),
        bottom: pane_clip.bottom.min(block_y + block_height),
    };
    (x, y, clip)
}

fn append_glyph_quad(
    vertices: &mut Vec<TextVertex>,
    indices: &mut Vec<u32>,
    entry: GlyphEntry,
    x: f32,
    y: f32,
    color: [f32; 4],
    atlas_width: f32,
    atlas_height: f32,
    baseline: f32,
    clip: Option<TextClip>,
) {
    if entry.width == 0 || entry.height == 0 {
        return;
    }
    // Rasterized bitmaps already contain any subpixel offset, so the quad must
    // sit on whole pixels: a fractional origin makes the sampler blend
    // neighbouring texels and smears every stem.
    let original_x0 = x.round() + entry.bearing_x as f32;
    let original_y0 = (y + baseline).round() - entry.bearing_y as f32;
    let original_x1 = original_x0 + entry.width as f32;
    let original_y1 = original_y0 + entry.height as f32;
    let (x0, y0, x1, y1) = clip.map_or(
        (original_x0, original_y0, original_x1, original_y1),
        |clip| {
            (
                original_x0.max(clip.left),
                original_y0.max(clip.top),
                original_x1.min(clip.right),
                original_y1.min(clip.bottom),
            )
        },
    );
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let source_u0 = entry.x as f32 / atlas_width;
    let source_v0 = entry.y as f32 / atlas_height;
    let source_u1 = (entry.x + entry.width) as f32 / atlas_width;
    let source_v1 = (entry.y + entry.height) as f32 / atlas_height;
    let x_span = original_x1 - original_x0;
    let y_span = original_y1 - original_y0;
    let u0 = source_u0 + (x0 - original_x0) / x_span * (source_u1 - source_u0);
    let v0 = source_v0 + (y0 - original_y0) / y_span * (source_v1 - source_v0);
    let u1 = source_u0 + (x1 - original_x0) / x_span * (source_u1 - source_u0);
    let v1 = source_v0 + (y1 - original_y0) / y_span * (source_v1 - source_v0);
    let base = vertices.len() as u32;
    vertices.extend_from_slice(&[
        TextVertex {
            position: [x0, y0],
            tex_coords: [u0, v0],
            color,
            atlas_page: f32::from(entry.page),
            color_glyph: f32::from(entry.color),
        },
        TextVertex {
            position: [x1, y0],
            tex_coords: [u1, v0],
            color,
            atlas_page: f32::from(entry.page),
            color_glyph: f32::from(entry.color),
        },
        TextVertex {
            position: [x1, y1],
            tex_coords: [u1, v1],
            color,
            atlas_page: f32::from(entry.page),
            color_glyph: f32::from(entry.color),
        },
        TextVertex {
            position: [x0, y1],
            tex_coords: [u0, v1],
            color,
            atlas_page: f32::from(entry.page),
            color_glyph: f32::from(entry.color),
        },
    ]);
    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// Text rendering pipeline
pub struct TextPipeline {
    pipeline: RenderPipeline,
    bind_group: Option<BindGroup>,
    uniform_buffer: Buffer,
    vertex_buffer: GrowableBuffer,
    index_buffer: GrowableBuffer,
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
                        view_dimension: TextureViewDimension::D2Array,
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
                // Native RGBA glyph texture (emoji and other color fonts)
                BindGroupLayoutEntry {
                    binding: 3,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2Array,
                        multisampled: false,
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
                BindGroupEntry {
                    binding: 3,
                    resource: BindingResource::TextureView(&glyph_cache.color_texture_view),
                },
            ],
        });

        Ok(Self {
            pipeline,
            bind_group: Some(bind_group),
            uniform_buffer,
            vertex_buffer: GrowableBuffer::new("Text Vertex Buffer", BufferUsages::VERTEX),
            index_buffer: GrowableBuffer::new("Text Index Buffer", BufferUsages::INDEX),
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

    pub(super) fn queued_checkpoint(&self) -> usize {
        self.queued.len()
    }

    pub(super) fn take_queued_since(&mut self, start: usize) -> Vec<QueuedChar> {
        self.queued.split_off(start)
    }

    pub(super) fn build_batch(
        queued: Vec<QueuedChar>,
        queue: &Queue,
        glyph_cache: &mut GlyphCache,
    ) -> TextBatch {
        let (atlas_width, atlas_height) = glyph_cache.atlas_size();
        let baseline = glyph_cache.cell_baseline();
        let mut vertices = Vec::with_capacity(queued.len() * 4);
        let mut indices = Vec::with_capacity(queued.len() * 6);
        let mut atlas_pages = Vec::new();

        for character in &queued {
            let character_baseline = match character.content {
                QueuedContent::MultiCell { font_scale, .. } => baseline * font_scale,
                QueuedContent::UiText { .. } => 0.0,
                _ => baseline,
            };
            for (entry, x_offset) in queued_glyphs(character, glyph_cache, queue) {
                let page = entry.page_key();
                if !atlas_pages.contains(&page) {
                    atlas_pages.push(page);
                }
                append_glyph_quad(
                    &mut vertices,
                    &mut indices,
                    entry,
                    character.x + x_offset,
                    character.y,
                    character.color,
                    atlas_width as f32,
                    atlas_height as f32,
                    character_baseline,
                    character.clip,
                );
            }
        }
        TextBatch {
            queued,
            vertices,
            indices,
            atlas_pages,
        }
    }

    pub(super) fn draw_external<'a>(
        &'a self,
        render_pass: &mut RenderPass<'a>,
        vertex_buffer: &'a Buffer,
        index_buffer: &'a Buffer,
        index_count: u32,
    ) {
        let Some(bind_group) = &self.bind_group else {
            return;
        };
        if index_count == 0 {
            return;
        }
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, bind_group, &[]);
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.set_index_buffer(index_buffer.slice(..), IndexFormat::Uint32);
        render_pass.draw_indexed(0..index_count, 0, 0..1);
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
        self.queued.push(QueuedChar {
            content: QueuedContent::Char(c),
            x,
            y,
            color,
            style: FontStyle::default(),
            clip: None,
        });
    }

    /// Queue chrome text in the UI face with its baseline at `baseline_y`.
    pub(super) fn push_ui_text(
        &mut self,
        text: &str,
        x: f32,
        baseline_y: f32,
        size_px: f32,
        color: [f32; 4],
        clip: Option<TextClip>,
    ) {
        self.push_ui_text_in(
            text,
            crate::ui::text_layout::UiFace::SANS,
            x,
            baseline_y,
            size_px,
            color,
            clip,
        );
    }

    /// Queue UI text in a specific UI face.
    pub(super) fn push_ui_text_in(
        &mut self,
        text: &str,
        face: crate::ui::text_layout::UiFace,
        x: f32,
        baseline_y: f32,
        size_px: f32,
        color: [f32; 4],
        clip: Option<TextClip>,
    ) {
        if !text.is_empty() {
            self.queued.push(QueuedChar {
                content: QueuedContent::UiText {
                    text: text.into(),
                    size_px,
                    mono: face.mono,
                },
                x,
                y: baseline_y,
                color,
                style: FontStyle {
                    bold: face.bold,
                    italic: face.italic,
                },
                clip,
            });
        }
    }

    /// Queue a terminal character with resolved SGR face and pane clipping.
    pub(super) fn push_styled_char(
        &mut self,
        c: char,
        x: f32,
        y: f32,
        color: [f32; 4],
        style: FontStyle,
        clip: TextClip,
    ) {
        self.queued.push(QueuedChar {
            content: QueuedContent::Char(c),
            x,
            y,
            color,
            style,
            clip: Some(clip),
        });
    }

    /// Queue a complete grapheme cluster at one terminal cell origin.
    #[cfg(test)]
    pub fn push_grapheme(&mut self, grapheme: &str, x: f32, y: f32, color: [f32; 4]) {
        if let Some(character) = grapheme.chars().next()
            && character.len_utf8() == grapheme.len()
        {
            self.push_char(character, x, y, color);
        } else {
            self.queued.push(QueuedChar {
                content: QueuedContent::Grapheme(grapheme.into()),
                x,
                y,
                color,
                style: FontStyle::default(),
                clip: None,
            });
        }
    }

    /// Queue UI text as one shaping run with terminal-cell advances and an
    /// explicit owning-field clip, including glyph overhangs.
    pub(super) fn push_ui_run(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        color: [f32; 4],
        cell_width: f32,
        clip: TextClip,
    ) {
        if !text.is_empty() {
            self.queued.push(QueuedChar {
                content: QueuedContent::Run {
                    text: text.to_owned(),
                    cell_width,
                },
                x,
                y,
                color,
                style: FontStyle::default(),
                clip: Some(clip),
            });
        }
    }

    /// Queue a terminal grapheme with resolved SGR face and pane clipping.
    pub(super) fn push_styled_grapheme(
        &mut self,
        grapheme: &str,
        x: f32,
        y: f32,
        color: [f32; 4],
        style: FontStyle,
        clip: TextClip,
    ) {
        if let Some(character) = grapheme.chars().next()
            && character.len_utf8() == grapheme.len()
        {
            self.push_styled_char(character, x, y, color, style, clip);
        } else {
            self.queued.push(QueuedChar {
                content: QueuedContent::Grapheme(grapheme.into()),
                x,
                y,
                color,
                style,
                clip: Some(clip),
            });
        }
    }

    /// Queue an OSC 66 leader, clipping its scaled glyphs to the occupied
    /// rectangle and aligning a fractional render area within that rectangle.
    pub(super) fn push_styled_multicell(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        color: [f32; 4],
        style: FontStyle,
        pane_clip: TextClip,
        cell_width: f32,
        cell_height: f32,
        descriptor: termeric_core::MultiCell,
    ) {
        if text.is_empty() {
            return;
        }
        let (x, y, clip) =
            multicell_origin_and_clip(x, y, pane_clip, cell_width, cell_height, descriptor);
        self.queued.push(QueuedChar {
            content: QueuedContent::MultiCell {
                text: text.into(),
                font_scale: descriptor.font_scale(),
            },
            x,
            y,
            color,
            style,
            clip: Some(clip),
        });
    }

    /// Upload vertex data to GPU
    pub fn upload(&mut self, device: &Device, queue: &Queue, glyph_cache: &mut GlyphCache) {
        self.vertices.clear();
        self.indices.clear();

        let (atlas_w, atlas_h) = glyph_cache.atlas_size();
        let atlas_w = atlas_w as f32;
        let atlas_h = atlas_h as f32;

        let baseline = glyph_cache.cell_baseline();

        self.upload_queued(queue, glyph_cache, atlas_w, atlas_h, baseline);

        // Create/update vertex buffer
        if !self.vertices.is_empty() {
            let vertex_data = bytemuck::cast_slice(&self.vertices);
            let index_data = bytemuck::cast_slice(&self.indices);

            self.vertex_buffer.upload(device, queue, vertex_data);
            self.index_buffer.upload(device, queue, index_data);
        }
    }

    pub(super) fn buffer_stats(&self) -> (BufferStats, BufferStats) {
        (self.vertex_buffer.stats(), self.index_buffer.stats())
    }

    /// Upload queued characters, graphemes, and shaped row runs.
    fn upload_queued(
        &mut self,
        queue: &Queue,
        glyph_cache: &mut GlyphCache,
        atlas_w: f32,
        atlas_h: f32,
        baseline: f32,
    ) {
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

            let queued_baseline = match queued.content {
                QueuedContent::MultiCell { font_scale, .. } => baseline * font_scale,
                QueuedContent::UiText { .. } => 0.0,
                _ => baseline,
            };
            for (entry, x_offset) in queued_glyphs(queued, glyph_cache, queue) {
                append_glyph_quad(
                    &mut self.vertices,
                    &mut self.indices,
                    entry,
                    queued.x + x_offset,
                    queued.y,
                    queued.color,
                    atlas_w,
                    atlas_h,
                    queued_baseline,
                    queued.clip,
                );
            }
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
    pub fn draw_layer<'a>(
        &'a self,
        render_pass: &mut RenderPass<'a>,
        _glyph_cache: &'a GlyphCache,
        layer: usize,
    ) {
        if self.indices.is_empty() {
            return;
        }

        let Some(vertex_buffer) = self.vertex_buffer.buffer() else {
            return;
        };
        let Some(index_buffer) = self.index_buffer.buffer() else {
            return;
        };
        let Some(bind_group) = &self.bind_group else {
            return;
        };

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

#[cfg(test)]
mod tests {
    use super::*;

    const COLOR: [f32; 4] = [0.8, 0.7, 0.6, 1.0];
    const CLIP: TextClip = TextClip {
        left: 0.0,
        top: 0.0,
        right: 100.0,
        bottom: 40.0,
    };

    fn append(
        accumulator: &mut TextRunAccumulator,
        character: char,
        x: f32,
        color: [f32; 4],
        style: FontStyle,
    ) -> Option<QueuedChar> {
        accumulator.accumulate(character, x, 4.0, color, style, CLIP, 10.0)
    }

    #[test]
    fn adjacent_compatible_cells_form_one_shaping_run() {
        let mut accumulator = TextRunAccumulator::default();
        assert!(append(&mut accumulator, '-', 0.0, COLOR, FontStyle::default()).is_none());
        assert!(append(&mut accumulator, '>', 10.0, COLOR, FontStyle::default()).is_none());
        let queued = accumulator.take_pending().expect("pending run");
        assert_eq!(
            queued.content,
            QueuedContent::Run {
                text: "->".into(),
                cell_width: 10.0,
            }
        );
        assert_eq!(queued.x, 0.0);
    }

    #[test]
    fn style_color_and_geometry_boundaries_break_runs() {
        let bold = FontStyle {
            bold: true,
            italic: false,
        };
        for (second_x, second_color, second_style) in [
            (20.0, COLOR, FontStyle::default()),
            (10.0, [0.2, 0.3, 0.4, 1.0], FontStyle::default()),
            (10.0, COLOR, bold),
        ] {
            let mut accumulator = TextRunAccumulator::default();
            assert!(append(&mut accumulator, 'a', 0.0, COLOR, FontStyle::default()).is_none());
            let flushed = append(&mut accumulator, 'b', second_x, second_color, second_style)
                .expect("boundary should flush the first cell");
            assert_eq!(flushed.content, QueuedContent::Char('a'));
            assert_eq!(
                accumulator.take_pending().unwrap().content,
                QueuedContent::Char('b')
            );
        }
    }

    #[test]
    fn a_single_pending_cell_stays_on_the_character_fast_path() {
        let mut accumulator = TextRunAccumulator::default();
        append(&mut accumulator, 'x', 0.0, COLOR, FontStyle::default());
        assert_eq!(
            accumulator.take_pending().unwrap().content,
            QueuedContent::Char('x')
        );
    }

    #[test]
    fn fractional_multicell_alignment_uses_the_full_block_and_pane_clip() {
        let descriptor = termeric_core::MultiCell::new(
            2,
            3,
            1,
            2,
            termeric_core::MultiCellAlignment::End,
            termeric_core::MultiCellAlignment::Center,
            0,
            0,
        )
        .unwrap();
        let pane = TextClip {
            left: 5.0,
            top: 8.0,
            right: 55.0,
            bottom: 35.0,
        };
        let (x, y, clip) = multicell_origin_and_clip(0.0, 10.0, pane, 10.0, 10.0, descriptor);
        assert_eq!(x, 30.0);
        assert_eq!(y, 15.0);
        assert_eq!(clip.left, 5.0);
        assert_eq!(clip.top, 10.0);
        assert_eq!(clip.right, 55.0);
        assert_eq!(clip.bottom, 30.0);
    }

    #[test]
    fn every_fractional_multicell_alignment_has_deterministic_origin() {
        let alignments = [
            (termeric_core::MultiCellAlignment::Start, 0.0, 0.0),
            (termeric_core::MultiCellAlignment::End, 30.0, 10.0),
            (termeric_core::MultiCellAlignment::Center, 15.0, 5.0),
        ];
        for (horizontal, expected_x, _) in alignments {
            for (vertical, _, expected_y) in alignments {
                let descriptor =
                    termeric_core::MultiCell::new(2, 3, 1, 2, horizontal, vertical, 0, 0).unwrap();
                let (x, y, _) = multicell_origin_and_clip(0.0, 0.0, CLIP, 10.0, 10.0, descriptor);
                assert_eq!((x, y), (expected_x, expected_y));
            }
        }
    }
}
