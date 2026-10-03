//! Shared image rendering pipeline for terminal graphics protocols.
//!
//! Renders inline images using wgpu textures.

use std::collections::HashMap;
use termeric_core::{ImageId, PresentationImage, StoredImage};
use wgpu::*;

use super::buffer::GrowableBuffer;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PlacementGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub u_min: f32,
    pub v_min: f32,
    pub u_max: f32,
    pub v_max: f32,
}

pub(super) fn placement_geometry(
    placement: &termeric_core::ImagePlacement,
    image: &StoredImage,
    cell_width: f32,
    cell_height: f32,
    scroll_offset: usize,
    viewport: Option<(f32, f32)>,
) -> Option<PlacementGeometry> {
    let source_x = placement.source_x.min(image.width);
    let source_y = placement.source_y.min(image.height);
    let source_width = placement
        .source_width
        .unwrap_or(image.width.saturating_sub(source_x))
        .min(image.width.saturating_sub(source_x));
    let source_height = placement
        .source_height
        .unwrap_or(image.height.saturating_sub(source_y))
        .min(image.height.saturating_sub(source_y));
    if source_width == 0 || source_height == 0 || image.width == 0 || image.height == 0 {
        return None;
    }

    let x = placement.col as f32 * cell_width + placement.x_offset as f32;
    let visual_row = placement.row.saturating_add(scroll_offset as i64);
    let y = visual_row as f32 * cell_height + placement.y_offset as f32;
    let aspect = source_width as f32 / source_height as f32;
    let (width, height) = match (placement.pixel_width, placement.pixel_height) {
        (Some(width), Some(height)) => (width as f32, height as f32),
        (Some(width), None) => {
            let width = width as f32;
            (width, width / aspect)
        }
        (None, Some(height)) => {
            let height = height as f32;
            (height * aspect, height)
        }
        (None, None) => match (placement.width, placement.height) {
            (Some(cols), Some(rows)) => (cols as f32 * cell_width, rows as f32 * cell_height),
            (Some(cols), None) => {
                let width = cols as f32 * cell_width;
                (width, width / aspect)
            }
            (None, Some(rows)) => {
                let height = rows as f32 * cell_height;
                (height * aspect, height)
            }
            (None, None) => (source_width as f32, source_height as f32),
        },
    };
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let (visible_x, visible_y, clipped_width, clipped_height) = viewport
        .map(|(viewport_width, viewport_height)| {
            let visible_x = x.max(0.0);
            let visible_y = y.max(0.0);
            let right = (x + width).min(viewport_width);
            let bottom = (y + height).min(viewport_height);
            (
                visible_x,
                visible_y,
                (right - visible_x).max(0.0),
                (bottom - visible_y).max(0.0),
            )
        })
        .unwrap_or((x, y, width, height));
    if clipped_width <= 0.0 || clipped_height <= 0.0 {
        return None;
    }
    Some(PlacementGeometry {
        x: visible_x,
        y: visible_y,
        width: clipped_width,
        height: clipped_height,
        u_min: (source_x as f32 + source_width as f32 * (visible_x - x) / width)
            / image.width as f32,
        v_min: (source_y as f32 + source_height as f32 * (visible_y - y) / height)
            / image.height as f32,
        u_max: (source_x as f32 + source_width as f32 * ((visible_x - x) + clipped_width) / width)
            / image.width as f32,
        v_max: (source_y as f32
            + source_height as f32 * ((visible_y - y) + clipped_height) / height)
            / image.height as f32,
    })
}

pub(super) fn presentation_geometry(
    presentation: &PresentationImage,
    cell_width: f32,
    cell_height: f32,
    scroll_offset: usize,
    viewport: Option<(f32, f32)>,
) -> Option<PlacementGeometry> {
    let (pixel_rect, source_uv) = match (presentation.pixel_rect, presentation.source_uv) {
        (Some(pixel_rect), Some(source_uv)) => (pixel_rect, source_uv),
        (None, None) => {
            return placement_geometry(
                &presentation.placement,
                &presentation.image,
                cell_width,
                cell_height,
                scroll_offset,
                viewport,
            );
        }
        _ => return None,
    };
    let placement = &presentation.placement;
    let x = placement.col as f32 * cell_width + pixel_rect[0];
    let visual_row = placement.row.saturating_add(scroll_offset as i64);
    let y = visual_row as f32 * cell_height + pixel_rect[1];
    let width = pixel_rect[2];
    let height = pixel_rect[3];
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let (visible_x, visible_y, clipped_width, clipped_height) = viewport
        .map(|(viewport_width, viewport_height)| {
            let visible_x = x.max(0.0);
            let visible_y = y.max(0.0);
            let right = (x + width).min(viewport_width);
            let bottom = (y + height).min(viewport_height);
            (
                visible_x,
                visible_y,
                (right - visible_x).max(0.0),
                (bottom - visible_y).max(0.0),
            )
        })
        .unwrap_or((x, y, width, height));
    if clipped_width <= 0.0 || clipped_height <= 0.0 {
        return None;
    }
    let u_span = source_uv[2] - source_uv[0];
    let v_span = source_uv[3] - source_uv[1];
    Some(PlacementGeometry {
        x: visible_x,
        y: visible_y,
        width: clipped_width,
        height: clipped_height,
        u_min: source_uv[0] + u_span * (visible_x - x) / width,
        v_min: source_uv[1] + v_span * (visible_y - y) / height,
        u_max: source_uv[0] + u_span * ((visible_x - x) + clipped_width) / width,
        v_max: source_uv[1] + v_span * ((visible_y - y) + clipped_height) / height,
    })
}

/// Image IDs are terminal-local, so the GPU cache key must include the owning
/// grid identity when multiple terminals are visible in one tab.
pub(super) type ImageKey = (usize, ImageId);

/// Vertex for image rendering
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ImageVertex {
    /// Position (x, y)
    pub position: [f32; 2],
    /// Texture coordinates (u, v)
    pub tex_coords: [f32; 2],
    /// Per-placement alpha multiplier.
    pub opacity: f32,
}

/// A GPU texture for an image. The bind group keeps its texture and view
/// alive.
struct GpuImage {
    bind_group: BindGroup,
    revision: u64,
}

/// Image rendering pipeline
pub struct ImagePipeline {
    pipeline: RenderPipeline,
    uniform_buffer: Buffer,
    uniform_bind_group: BindGroup,
    texture_bind_group_layout: BindGroupLayout,
    sampler: Sampler,
    images: HashMap<ImageKey, GpuImage>,
    vertex_buffer: GrowableBuffer,
    index_buffer: GrowableBuffer,
    instances: Vec<ImageInstance>,
    vertices: Vec<ImageVertex>,
    indices: Vec<u16>,
}

/// Instance of an image to render
struct ImageInstance {
    image_key: ImageKey,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    u_min: f32,
    v_min: f32,
    u_max: f32,
    v_max: f32,
    z_index: i32,
    opacity: f32,
}

impl ImagePipeline {
    /// Create a new image rendering pipeline
    pub fn new(device: &Device, format: TextureFormat) -> anyhow::Result<Self> {
        // Create shader
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Image Shader"),
            source: ShaderSource::Wgsl(include_str!("shaders/image.wgsl").into()),
        });

        // Create uniform buffer for screen dimensions
        let uniform_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("Image Uniform Buffer"),
            size: 16, // 2 floats for screen size + padding
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Uniform bind group layout
        let uniform_bind_group_layout =
            device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("Image Uniform Bind Group Layout"),
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

        let uniform_bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("Image Uniform Bind Group"),
            layout: &uniform_bind_group_layout,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        // Texture bind group layout
        let texture_bind_group_layout =
            device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("Image Texture Bind Group Layout"),
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

        // Create pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Image Pipeline Layout"),
            bind_group_layouts: &[&uniform_bind_group_layout, &texture_bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create render pipeline
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Image Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[VertexBufferLayout {
                    array_stride: std::mem::size_of::<ImageVertex>() as BufferAddress,
                    step_mode: VertexStepMode::Vertex,
                    attributes: &vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32],
                }],
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
                ..Default::default()
            },
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Create sampler
        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("Image Sampler"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });

        Ok(Self {
            pipeline,
            uniform_buffer,
            uniform_bind_group,
            texture_bind_group_layout,
            sampler,
            images: HashMap::new(),
            vertex_buffer: GrowableBuffer::new("Image Vertex Buffer", BufferUsages::VERTEX),
            index_buffer: GrowableBuffer::new("Image Index Buffer", BufferUsages::INDEX),
            instances: Vec::new(),
            vertices: Vec::new(),
            indices: Vec::new(),
        })
    }

    /// Resize the viewport
    pub fn resize(&mut self, queue: &Queue, width: u32, height: u32) {
        let uniforms: [f32; 4] = [width as f32, height as f32, 0.0, 0.0];
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&uniforms));
    }

    /// Clear render data
    pub fn clear(&mut self) {
        self.instances.clear();
    }

    /// Upload an image to the GPU
    pub fn upload_image(
        &mut self,
        device: &Device,
        queue: &Queue,
        key: ImageKey,
        image: &StoredImage,
    ) {
        if self
            .images
            .get(&key)
            .is_some_and(|existing| existing.revision == image.revision)
        {
            return;
        }

        let texture = device.create_texture(&TextureDescriptor {
            label: Some("Image Texture"),
            size: Extent3d {
                width: image.width,
                height: image.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: super::ENCODED_RGBA_FORMAT,
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
            &image.data,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.width * 4),
                rows_per_image: Some(image.height),
            },
            Extent3d {
                width: image.width,
                height: image.height,
                depth_or_array_layers: 1,
            },
        );

        let view = texture.create_view(&TextureViewDescriptor::default());

        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("Image Texture Bind Group"),
            layout: &self.texture_bind_group_layout,
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

        self.images.insert(
            key,
            GpuImage {
                bind_group,
                revision: image.revision,
            },
        );
    }

    /// Queue an image for rendering
    pub fn push_image(
        &mut self,
        image_key: ImageKey,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        u_min: f32,
        v_min: f32,
        u_max: f32,
        v_max: f32,
        opacity: f32,
        z_index: i32,
    ) {
        self.instances.push(ImageInstance {
            image_key,
            x,
            y,
            width,
            height,
            u_min,
            v_min,
            u_max,
            v_max,
            opacity: opacity.clamp(0.0, 1.0),
            z_index,
        });
    }

    /// Upload vertex data for rendering
    pub fn upload(&mut self, device: &Device, queue: &Queue) {
        if self.instances.is_empty() {
            return;
        }

        self.instances
            .sort_by_key(|instance| (instance.z_index, instance.image_key.1.0));

        self.vertices.clear();
        self.indices.clear();

        for instance in &self.instances {
            let x = instance.x;
            let y = instance.y;
            let w = instance.width;
            let h = instance.height;

            let Ok(base_idx) = u16::try_from(self.vertices.len()) else {
                log::warn!("Too many image placements for the u16 index buffer");
                break;
            };

            // Four corners
            self.vertices.push(ImageVertex {
                position: [x, y],
                tex_coords: [instance.u_min, instance.v_min],
                opacity: instance.opacity,
            });
            self.vertices.push(ImageVertex {
                position: [x + w, y],
                tex_coords: [instance.u_max, instance.v_min],
                opacity: instance.opacity,
            });
            self.vertices.push(ImageVertex {
                position: [x + w, y + h],
                tex_coords: [instance.u_max, instance.v_max],
                opacity: instance.opacity,
            });
            self.vertices.push(ImageVertex {
                position: [x, y + h],
                tex_coords: [instance.u_min, instance.v_max],
                opacity: instance.opacity,
            });

            // Two triangles
            self.indices.push(base_idx);
            self.indices.push(base_idx + 1);
            self.indices.push(base_idx + 2);
            self.indices.push(base_idx);
            self.indices.push(base_idx + 2);
            self.indices.push(base_idx + 3);
        }

        self.vertex_buffer
            .upload(device, queue, bytemuck::cast_slice(&self.vertices));
        self.index_buffer
            .upload(device, queue, bytemuck::cast_slice(&self.indices));
    }

    /// Remove an image from the GPU
    pub fn remove_image(&mut self, key: ImageKey) {
        self.images.remove(&key);
    }

    #[cfg(test)]
    pub(super) fn retained_image_count(&self) -> usize {
        self.images.len()
    }

    /// Remove images that are no longer referenced by any active placement.
    ///
    /// `active_ids` is the set of image IDs that are still in use.
    pub fn gc_images(&mut self, active_keys: &[ImageKey]) {
        let stale: Vec<ImageKey> = self
            .images
            .keys()
            .filter(|key| !active_keys.contains(key))
            .copied()
            .collect();
        for key in stale {
            self.remove_image(key);
        }
    }

    /// Draw images below terminal text (negative z-index).
    pub fn draw_below_text<'a>(&'a self, render_pass: &mut RenderPass<'a>) {
        self.draw_matching(render_pass, |z_index| z_index < 0);
    }

    /// Draw images above terminal text (zero or positive z-index).
    pub fn draw_above_text<'a>(&'a self, render_pass: &mut RenderPass<'a>) {
        self.draw_matching(render_pass, |z_index| z_index >= 0);
    }

    fn draw_matching<'a>(
        &'a self,
        render_pass: &mut RenderPass<'a>,
        predicate: impl Fn(i32) -> bool,
    ) {
        if self.instances.is_empty() {
            return;
        }

        let Some(vertex_buffer) = self.vertex_buffer.buffer() else {
            return;
        };
        let Some(index_buffer) = self.index_buffer.buffer() else {
            return;
        };

        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.set_index_buffer(index_buffer.slice(..), IndexFormat::Uint16);

        let mut index_offset = 0u32;
        for instance in &self.instances {
            if predicate(instance.z_index)
                && let Some(gpu_image) = self.images.get(&instance.image_key)
            {
                render_pass.set_bind_group(1, &gpu_image.bind_group, &[]);
                render_pass.draw_indexed(index_offset..index_offset + 6, 0, 0..1);
            }
            index_offset += 6;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scrolled_graphics_fixture() -> termeric_core::Terminal {
        let mut terminal = termeric_core::Terminal::new(8, 3);
        terminal.set_pixel_geometry(80, 60);
        terminal.handle_graphics_command(b"a=T,i=7,p=1,c=2,r=2,C=1,f=32,s=1,v=1,q=2;AQIDBA==");
        for id in [8, 9] {
            terminal.handle_graphics_command(
                format!("a=t,i={id},f=32,s=1,v=1,q=2;AQIDBA==").as_bytes(),
            );
        }
        terminal.handle_graphics_command(b"a=p,i=8,P=7,Q=1,H=-1,V=1,c=2,r=1,q=2");
        terminal.handle_graphics_command(
            b"a=T,U=1,i=42,p=7,c=2,r=2,f=32,s=2,v=2,q=2;AQIDBAUGBwgJCgsMDQ4PEA==",
        );
        terminal.process("\x1b[1;4H\x1b[38;5;42;58;5;7m\u{10eeee}\u{0305}\u{0305}\u{10eeee}\x1b[2;4H\u{10eeee}\u{030d}\u{0305}\u{10eeee}\x1b[0m".as_bytes());
        terminal.handle_graphics_command(b"a=p,i=9,P=42,Q=7,H=2,c=1,r=1,q=2");
        terminal.process(b"\x1b[3;1Hbottom\r\none\r\ntwo\r\nthree\r\nfour");
        assert_eq!(terminal.grid().scrollback_len(), 4);
        terminal
    }

    fn rendered_images(
        terminal: &termeric_core::Terminal,
        cell: (f32, f32),
    ) -> Vec<(ImageId, Option<PlacementGeometry>)> {
        let grid = terminal.presentation_grid();
        terminal
            .presentation_images()
            .iter()
            .map(|presentation| {
                (
                    presentation.placement.image_id,
                    presentation_geometry(
                        presentation,
                        cell.0,
                        cell.1,
                        grid.scroll_offset(),
                        Some((grid.cols() as f32 * cell.0, grid.rows() as f32 * cell.1)),
                    ),
                )
            })
            .collect()
    }

    #[test]
    fn pixel_image_margin_scroll_crops_actual_pixels_before_rendering() {
        for down in [false, true] {
            let mut terminal = termeric_core::Terminal::new(4, 4);
            terminal.set_pixel_geometry(16, 16);
            // A non-top-origin region cannot archive removed image rows.
            terminal.process(b"\x1b[2;3r\x1b[2;1H\x1bP7q\"1;1;6;6!6~\x1b\\");
            terminal.process(if down { b"\x1b[T" } else { b"\x1b[S" });
            let images = rendered_images(&terminal, (4.0, 4.0));
            assert_eq!(images.len(), 1);
            let geometry = images[0].1.unwrap();
            assert_eq!((geometry.x, geometry.width), (0.0, 6.0));
            assert_eq!(
                (geometry.y, geometry.height),
                if down { (8.0, 4.0) } else { (4.0, 2.0) }
            );
            assert_eq!(
                (geometry.v_min, geometry.v_max),
                if down {
                    (0.0, 4.0 / 6.0)
                } else {
                    (4.0 / 6.0, 1.0)
                }
            );
            terminal.process(b"\x1b[?2026h");
            assert_eq!(rendered_images(&terminal, (4.0, 4.0)), images);
        }
    }

    #[test]
    fn synchronized_images_keep_render_geometry_when_scrolled_text_is_rebased() {
        for offset in 0..=4 {
            let mut terminal = scrolled_graphics_fixture();
            terminal.grid_mut().set_scroll_offset(offset);
            let before = rendered_images(&terminal, (10.0, 20.0));
            if offset == 4 {
                for id in [7, 8, 9, 42] {
                    assert!(
                        before
                            .iter()
                            .any(|(image, geometry)| *image == ImageId(id) && geometry.is_some())
                    );
                }
            }
            let text: Vec<_> = (0..3)
                .map(|row| {
                    terminal
                        .grid()
                        .row_at_offset(row - offset as i32)
                        .unwrap()
                        .to_string()
                })
                .collect();
            let original = terminal.presentation_images();
            terminal.process(b"\x1b[?2026h");
            assert_eq!(terminal.presentation_grid().scroll_offset(), 0);
            assert_eq!(
                rendered_images(&terminal, (10.0, 20.0)),
                before,
                "offset={offset}"
            );
            let frozen = terminal.presentation_images();
            assert_eq!(original.len(), frozen.len());
            for (live, frozen) in original.iter().zip(&frozen) {
                assert_eq!(frozen.placement.row, live.placement.row + offset as i64);
                assert_eq!(frozen.pixel_rect, live.pixel_rect);
                assert_eq!(frozen.source_uv, live.source_uv);
                assert!(std::sync::Arc::ptr_eq(&frozen.image.data, &live.image.data));
            }
            for (row, text) in text.iter().enumerate() {
                assert_eq!(
                    terminal.presentation_grid().row(row).unwrap().to_string(),
                    *text
                );
            }
            // Repeated DECSET must not apply the original offset a second time.
            terminal.process(b"\x1b[?2026h");
            assert_eq!(rendered_images(&terminal, (10.0, 20.0)), before);
            terminal.process(b"\x1b[?2026l");
            assert_eq!(rendered_images(&terminal, (10.0, 20.0)), before);
        }
    }

    #[test]
    fn synchronized_images_ignore_hidden_output_navigation_and_screen_changes() {
        let mut terminal = scrolled_graphics_fixture();
        terminal.grid_mut().set_scroll_offset(4);
        let before = rendered_images(&terminal, (10.0, 20.0));
        terminal.process(b"\x1b[?2026h");
        terminal.grid_mut().set_scroll_offset(0);
        terminal.process(b"\x1b[3J\x1b[?1049h");
        terminal.handle_graphics_command(b"a=T,i=50,c=1,r=1,C=1,f=32,s=1,v=1,q=2;BQYHCA==");
        assert!(!terminal.presentation_alternate_screen_active());
        assert_eq!(rendered_images(&terminal, (10.0, 20.0)), before);
        terminal.process(b"\x1b[?2026l");
        assert!(terminal.presentation_alternate_screen_active());
        let alternate = rendered_images(&terminal, (10.0, 20.0));
        assert_eq!(alternate.len(), 1);
        assert_eq!(alternate[0].0, ImageId(50));
        assert!(alternate[0].1.is_some());
        terminal.process(b"\x1b[?2026h");
        assert_eq!(rendered_images(&terminal, (10.0, 20.0)), alternate);
        terminal.process(b"\x1b[?2026l\x1b[?1049l");
        terminal.grid_mut().set_scroll_offset(0);
        let primary = rendered_images(&terminal, (10.0, 20.0));
        terminal.process(b"\x1b[?2026h");
        assert_eq!(rendered_images(&terminal, (10.0, 20.0)), primary);
    }

    #[test]
    fn synchronized_images_preserve_pixel_sized_clipping_across_fragmented_capture() {
        let mode = b"\x1b[?2026h";
        for split in 0..=mode.len() {
            for offset in [2, 3, 4] {
                let mut terminal = termeric_core::Terminal::new(8, 3);
                terminal.set_pixel_geometry(16, 9);
                terminal.process(b"\x1bPq\"1;1!4~\x1b\\\x1b[3S\x1b[S");
                terminal.grid_mut().set_scroll_offset(offset);
                let before = rendered_images(&terminal, (2.0, 3.0));
                if offset > 2 {
                    let geometry = before[0].1.unwrap();
                    assert_eq!(geometry.height, if offset == 3 { 3.0 } else { 6.0 });
                    assert_eq!(geometry.v_min, if offset == 3 { 0.5 } else { 0.0 });
                }
                terminal.process(&mode[..split]);
                terminal.process(&mode[split..]);
                assert_eq!(
                    rendered_images(&terminal, (2.0, 3.0)),
                    before,
                    "split={split} offset={offset}"
                );
            }
        }
    }

    #[test]
    fn height_resize_moves_frozen_images_with_their_own_text_not_hidden_output() {
        let mut terminal = scrolled_graphics_fixture();
        terminal.grid_mut().set_scroll_offset(4);
        terminal.process(b"\x1b[?2026h");
        // The frozen cursor is on row two; the hidden live cursor is on row
        // zero, so only the frozen grid archives a row on this resize.
        terminal.process(b"\x1b[1;1H");
        terminal.resize(8, 2);
        assert_eq!(terminal.grid().scrollback_len(), 4);
        assert_eq!(terminal.presentation_grid().scrollback_len(), 1);
        let frozen = terminal.presentation_images();
        let parent = frozen
            .iter()
            .find(|p| p.placement.image_id == ImageId(7))
            .unwrap();
        assert_eq!(parent.placement.row, -1);
        let geometry = presentation_geometry(parent, 10.0, 20.0, 0, Some((80.0, 40.0))).unwrap();
        assert_eq!(
            (geometry.y, geometry.height, geometry.v_min),
            (0.0, 20.0, 0.5)
        );
        let child = frozen
            .iter()
            .find(|p| p.placement.image_id == ImageId(8))
            .unwrap();
        assert_eq!(child.placement.row, 0);
        let placeholder_rows: Vec<_> = frozen
            .iter()
            .filter(|p| p.placement.image_id == ImageId(42))
            .map(|p| p.placement.row)
            .collect();
        // Only cells in the displayed viewport produce placeholder instances.
        assert_eq!(placeholder_rows, vec![0]);
        let pixels = parent.image.data.clone();
        terminal.presentation_grid_mut().set_scroll_offset(1);
        assert_eq!(
            terminal
                .presentation_images()
                .iter()
                .filter(|p| p.placement.image_id == ImageId(42))
                .map(|p| p.placement.row)
                .collect::<Vec<_>>(),
            vec![-1, 0]
        );
        let restored = rendered_images(&terminal, (10.0, 20.0));
        let parent_geometry = restored
            .iter()
            .find(|(id, _)| *id == ImageId(7))
            .unwrap()
            .1
            .unwrap();
        assert_eq!(
            (
                parent_geometry.y,
                parent_geometry.height,
                parent_geometry.v_min
            ),
            (0.0, 40.0, 0.0)
        );
        terminal.resize(8, 4);
        let parent = terminal
            .presentation_images()
            .into_iter()
            .find(|p| p.placement.image_id == ImageId(7))
            .unwrap();
        assert_eq!(parent.placement.row, -1);
        assert!(std::sync::Arc::ptr_eq(&pixels, &parent.image.data));
        terminal.process(b"\x1b[?2026l");
        let live = terminal
            .graphics_placements()
            .find(|p| p.image_id == ImageId(7))
            .unwrap();
        assert_eq!(live.row, -4);
    }

    #[test]
    fn frozen_images_use_current_cell_geometry_without_taking_live_content() {
        let mut reference = scrolled_graphics_fixture();
        let mut frozen = scrolled_graphics_fixture();
        reference.grid_mut().set_scroll_offset(4);
        frozen.grid_mut().set_scroll_offset(4);
        let captured = frozen.presentation_images();
        frozen.process(b"\x1b[?2026h");
        frozen.handle_graphics_command(b"a=d,d=A,q=2");
        frozen.enter_alt_screen();
        frozen.handle_graphics_command(b"a=T,i=42,c=1,r=1,C=1,f=32,s=1,v=1,q=2;BQYHCA==");

        // Include proportional zoom, shrink, changed cell aspect, and a
        // return to the original metrics. No terminal text is reflowed here.
        for (width, height) in [(20, 40), (5, 10), (14, 9), (10, 20)] {
            reference.set_pixel_geometry(8 * width, 3 * height);
            frozen.set_pixel_geometry(8 * width, 3 * height);
            let cell = (width as f32, height as f32);
            let expected = rendered_images(&reference, cell);
            assert!(expected.iter().all(|(_, geometry)| geometry.is_some()));
            assert_eq!(
                rendered_images(&frozen, cell),
                expected,
                "cell size={width}x{height}"
            );
            for image in frozen.presentation_images() {
                let original = captured
                    .iter()
                    .find(|original| original.placement.image_id == image.placement.image_id)
                    .unwrap();
                assert_eq!(image.image.revision, original.image.revision);
                assert!(std::sync::Arc::ptr_eq(
                    &image.image.data,
                    &original.image.data
                ));
            }
        }
        frozen.process(b"\x1b[?2026l");
        let live = frozen.presentation_images();
        assert_eq!(live.len(), 1);
        assert_eq!(&*live[0].image.data, &[5, 6, 7, 8]);
    }

    #[test]
    fn frozen_unicode_images_reflow_from_captured_cells_and_pixels() {
        let mut frozen = scrolled_graphics_fixture();
        frozen.grid_mut().set_scroll_offset(4);
        frozen.process(b"\x1b[?2026h");
        // Build an independent reference with exactly the captured viewport.
        // The fixture's first three rows have no omitted logical-line prefix.
        let mut reference = termeric_core::Terminal::new(8, 3);
        reference.set_pixel_geometry(80, 60);
        reference.handle_graphics_command(
            b"a=T,U=1,i=42,p=7,c=2,r=2,f=32,s=2,v=2,q=2;AQIDBAUGBwgJCgsMDQ4PEA==",
        );
        reference.handle_graphics_command(b"a=t,i=9,f=32,s=1,v=1,q=2;AQIDBA==");
        reference.handle_graphics_command(b"a=p,i=9,P=42,Q=7,H=2,c=1,r=1,q=2");
        reference.process("\x1b[1;4H\x1b[38;5;42;58;5;7m\u{10eeee}\u{0305}\u{0305}\u{10eeee}\x1b[2;4H\u{10eeee}\u{030d}\u{0305}\u{10eeee}\x1b[0m\x1b[3;1Hbottom\x1b[3;5H".as_bytes());
        // Delete and reuse the live image ID, then switch screens. Neither
        // metadata nor replacement pixels may leak into the frozen frame.
        frozen.handle_graphics_command(b"a=d,d=A,q=2");
        frozen.handle_graphics_command(b"a=T,U=1,i=42,p=7,c=1,r=1,f=32,s=1,v=1,q=2;AQIDBQ==");
        frozen.enter_alt_screen();
        for columns in [4, 2, 8, 12] {
            reference.resize(columns, 3);
            frozen.resize(columns, 3);
            for offset in 0..=reference.grid().scrollback_len() {
                reference.grid_mut().set_scroll_offset(offset);
                frozen.presentation_grid_mut().set_scroll_offset(offset);
                let expected = rendered_images(&reference, (10.0, 20.0));
                let actual: Vec<_> = rendered_images(&frozen, (10.0, 20.0))
                    .into_iter()
                    .filter(|(id, _)| matches!(id.0, 9 | 42))
                    .collect();
                assert_eq!(actual, expected, "columns={columns} offset={offset}");
                for image in frozen
                    .presentation_images()
                    .iter()
                    .filter(|image| image.placement.image_id == ImageId(42))
                {
                    assert_eq!(
                        &*image.image.data,
                        &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
                    );
                }
            }
        }
        frozen.process(b"\x1b[?2026l");
        assert!(frozen.presentation_images().is_empty());
    }

    #[test]
    fn column_resize_clips_fixed_images_without_erasing_their_geometry() {
        for frozen in [false, true] {
            let mut terminal = termeric_core::Terminal::new(8, 4);
            terminal.set_pixel_geometry(80, 80);
            terminal.process(b"\x1b[1;7H");
            terminal.handle_graphics_command(b"a=T,i=7,p=1,c=2,r=2,C=1,f=32,s=1,v=1,q=2;AQIDBA==");
            terminal.handle_graphics_command(b"a=t,i=8,f=32,s=1,v=1,q=2;AQIDBA==");
            terminal.handle_graphics_command(b"a=p,i=8,P=7,Q=1,H=-1,V=1,c=2,r=1,q=2");
            let original = terminal.presentation_images();
            if frozen {
                terminal.process(b"\x1b[?2026h");
            }
            for columns in [7, 4, 8, 12] {
                terminal.resize(columns, 4);
                let images = terminal.presentation_images();
                assert_eq!(images.len(), 2);
                for (image, before) in images.iter().zip(&original) {
                    assert_eq!(
                        format!("{:?}", image.placement),
                        format!("{:?}", before.placement)
                    );
                    assert!(std::sync::Arc::ptr_eq(
                        &image.image.data,
                        &before.image.data
                    ));
                }
                let rendered = rendered_images(&terminal, (10.0, 20.0));
                if columns == 4 {
                    assert!(rendered.iter().all(|(_, geometry)| geometry.is_none()));
                } else {
                    let parent = rendered
                        .iter()
                        .find(|(id, _)| *id == ImageId(7))
                        .unwrap()
                        .1
                        .unwrap();
                    assert_eq!((parent.x, parent.y, parent.height), (60.0, 0.0, 40.0));
                    assert_eq!(parent.width, if columns == 7 { 10.0 } else { 20.0 });
                    assert_eq!(parent.u_max, if columns == 7 { 0.5 } else { 1.0 });
                    let child = rendered
                        .iter()
                        .find(|(id, _)| *id == ImageId(8))
                        .unwrap()
                        .1
                        .unwrap();
                    assert_eq!(
                        (child.x, child.y, child.width, child.height),
                        (50.0, 20.0, 20.0, 20.0)
                    );
                }
            }
        }
    }

    #[test]
    fn source_destination_aspect_offsets_and_viewport_clipping_share_one_geometry_path() {
        let image = StoredImage {
            id: ImageId(1),
            width: 100,
            height: 50,
            data: Vec::new().into(),
            revision: 1,
        };
        let placement = termeric_core::ImagePlacement {
            image_id: image.id,
            placement_id: Some(2),
            col: 2,
            row: 1,
            alternate_screen: false,
            source_x: 10,
            source_y: 5,
            source_width: Some(40),
            source_height: Some(20),
            x_offset: 3,
            y_offset: 4,
            width: Some(4),
            height: None,
            pixel_width: None,
            pixel_height: None,
            z_index: -1,
        };
        let geometry =
            placement_geometry(&placement, &image, 10.0, 20.0, 0, Some((50.0, 40.0))).unwrap();
        assert_eq!((geometry.x, geometry.y), (23.0, 24.0));
        assert_eq!((geometry.width, geometry.height), (27.0, 16.0));
        assert!((geometry.u_min - 0.1).abs() < f32::EPSILON);
        assert!((geometry.v_min - 0.1).abs() < f32::EPSILON);
        assert!((geometry.u_max - 0.37).abs() < 0.0001);
        assert!((geometry.v_max - 0.42).abs() < 0.0001);

        let mut history_placement = placement.clone();
        history_placement.row = -1;
        history_placement.y_offset = 0;
        let history = placement_geometry(
            &history_placement,
            &image,
            10.0,
            20.0,
            1,
            Some((100.0, 100.0)),
        )
        .unwrap();
        assert_eq!(history.y, 0.0);
    }

    #[test]
    fn unicode_placeholder_geometry_preserves_subcell_uvs_when_clipped() {
        let image = StoredImage {
            id: ImageId(9),
            width: 20,
            height: 20,
            data: Vec::new().into(),
            revision: 1,
        };
        let presentation = PresentationImage {
            placement: termeric_core::ImagePlacement {
                image_id: image.id,
                placement_id: Some(3),
                col: 1,
                row: 0,
                alternate_screen: false,
                source_x: 0,
                source_y: 0,
                source_width: Some(20),
                source_height: Some(20),
                x_offset: 0,
                y_offset: 0,
                width: Some(1),
                height: Some(1),
                pixel_width: None,
                pixel_height: None,
                z_index: -1,
            },
            image,
            pixel_rect: Some([2.0, 3.0, 8.0, 10.0]),
            source_uv: Some([0.25, 0.1, 0.75, 0.9]),
        };
        let geometry =
            presentation_geometry(&presentation, 10.0, 20.0, 0, Some((16.0, 8.0))).unwrap();
        assert_eq!((geometry.x, geometry.y), (12.0, 3.0));
        assert_eq!((geometry.width, geometry.height), (4.0, 5.0));
        assert!((geometry.u_min - 0.25).abs() < f32::EPSILON);
        assert!((geometry.v_min - 0.1).abs() < f32::EPSILON);
        assert!((geometry.u_max - 0.5).abs() < f32::EPSILON);
        assert!((geometry.v_max - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn negative_relative_geometry_clips_pixels_and_texture_at_the_left_edge() {
        let image = StoredImage {
            id: ImageId(10),
            width: 20,
            height: 10,
            data: Vec::new().into(),
            revision: 1,
        };
        let presentation = PresentationImage {
            placement: termeric_core::ImagePlacement {
                image_id: image.id,
                placement_id: Some(4),
                col: 0,
                row: 1,
                alternate_screen: false,
                source_x: 0,
                source_y: 0,
                source_width: Some(20),
                source_height: Some(10),
                x_offset: 0,
                y_offset: 0,
                width: Some(2),
                height: Some(1),
                pixel_width: None,
                pixel_height: None,
                z_index: 0,
            },
            image,
            pixel_rect: Some([-10.0, 0.0, 20.0, 10.0]),
            source_uv: Some([0.0, 0.0, 1.0, 1.0]),
        };
        let geometry =
            presentation_geometry(&presentation, 10.0, 10.0, 0, Some((80.0, 40.0))).unwrap();
        assert_eq!((geometry.x, geometry.y), (0.0, 10.0));
        assert_eq!((geometry.width, geometry.height), (10.0, 10.0));
        assert!((geometry.u_min - 0.5).abs() < f32::EPSILON);
        assert!((geometry.u_max - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn pixel_addressed_protocols_keep_exact_extent_and_pane_clipping() {
        let image = StoredImage {
            id: ImageId(9),
            width: 4,
            height: 6,
            data: vec![0; 4 * 6 * 4].into(),
            revision: 1,
        };
        let placement = termeric_core::ImagePlacement {
            image_id: image.id,
            placement_id: None,
            col: 1,
            row: 1,
            alternate_screen: false,
            source_x: 0,
            source_y: 0,
            source_width: Some(4),
            source_height: Some(6),
            x_offset: 0,
            y_offset: 0,
            width: Some(1),
            height: Some(1),
            pixel_width: Some(4),
            pixel_height: Some(6),
            z_index: 0,
        };

        let exact = placement_geometry(&placement, &image, 10.0, 20.0, 0, None).unwrap();
        assert_eq!(
            (exact.x, exact.y, exact.width, exact.height),
            (10.0, 20.0, 4.0, 6.0)
        );

        let clipped =
            placement_geometry(&placement, &image, 10.0, 20.0, 0, Some((12.0, 24.0))).unwrap();
        assert_eq!((clipped.width, clipped.height), (2.0, 4.0));
        assert!((clipped.u_max - 0.5).abs() < f32::EPSILON);
        assert!((clipped.v_max - (4.0 / 6.0)).abs() < 0.0001);
    }
}
