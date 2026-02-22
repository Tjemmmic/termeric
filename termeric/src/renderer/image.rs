//! Image rendering pipeline for Kitty Graphics Protocol
//!
//! Renders inline images using wgpu textures.

use std::collections::HashMap;
use wgpu::*;
use wgpu::util::DeviceExt;
use termeric_core::{ImageId, StoredImage};

/// Vertex for image rendering
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ImageVertex {
    /// Position (x, y)
    pub position: [f32; 2],
    /// Texture coordinates (u, v)
    pub tex_coords: [f32; 2],
}

/// A GPU texture for an image
struct GpuImage {
    #[allow(dead_code)]
    texture: Texture,
    view: TextureView,
    bind_group: BindGroup,
    width: u32,
    height: u32,
}

/// Image rendering pipeline
pub struct ImagePipeline {
    pipeline: RenderPipeline,
    uniform_buffer: Buffer,
    uniform_bind_group: BindGroup,
    texture_bind_group_layout: BindGroupLayout,
    sampler: Sampler,
    images: HashMap<ImageId, GpuImage>,
    vertex_buffer: Option<Buffer>,
    index_buffer: Option<Buffer>,
    instances: Vec<ImageInstance>,
}

/// Instance of an image to render
struct ImageInstance {
    image_id: ImageId,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
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
        let uniform_bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
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
        let texture_bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
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
                    attributes: &vertex_attr_array![0 => Float32x2, 1 => Float32x2],
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
            vertex_buffer: None,
            index_buffer: None,
            instances: Vec::new(),
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
    pub fn upload_image(&mut self, device: &Device, queue: &Queue, image: &StoredImage) {
        if self.images.contains_key(&image.id) {
            return; // Already uploaded
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
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            &image.data,
            ImageDataLayout {
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

        self.images.insert(image.id, GpuImage {
            texture,
            view,
            bind_group,
            width: image.width,
            height: image.height,
        });
    }

    /// Queue an image for rendering
    pub fn push_image(&mut self, image_id: ImageId, x: f32, y: f32, width: f32, height: f32) {
        self.instances.push(ImageInstance {
            image_id,
            x,
            y,
            width,
            height,
        });
    }

    /// Upload vertex data for rendering
    pub fn upload(&mut self, device: &Device, _queue: &Queue) {
        if self.instances.is_empty() {
            return;
        }

        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        for instance in &self.instances {
            let x = instance.x;
            let y = instance.y;
            let w = instance.width;
            let h = instance.height;

            let base_idx = vertices.len() as u16;

            // Four corners
            vertices.push(ImageVertex { position: [x, y], tex_coords: [0.0, 0.0] });
            vertices.push(ImageVertex { position: [x + w, y], tex_coords: [1.0, 0.0] });
            vertices.push(ImageVertex { position: [x + w, y + h], tex_coords: [1.0, 1.0] });
            vertices.push(ImageVertex { position: [x, y + h], tex_coords: [0.0, 1.0] });

            // Two triangles
            indices.push(base_idx);
            indices.push(base_idx + 1);
            indices.push(base_idx + 2);
            indices.push(base_idx);
            indices.push(base_idx + 2);
            indices.push(base_idx + 3);
        }

        self.vertex_buffer = Some(device.create_buffer_init(&util::BufferInitDescriptor {
            label: Some("Image Vertex Buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: BufferUsages::VERTEX,
        }));

        self.index_buffer = Some(device.create_buffer_init(&util::BufferInitDescriptor {
            label: Some("Image Index Buffer"),
            contents: bytemuck::cast_slice(&indices),
            usage: BufferUsages::INDEX,
        }));
    }

    /// Draw all queued images
    pub fn draw<'a>(&'a self, render_pass: &mut RenderPass<'a>) {
        if self.instances.is_empty() {
            return;
        }

        let Some(vertex_buffer) = &self.vertex_buffer else { return };
        let Some(index_buffer) = &self.index_buffer else { return };

        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.set_index_buffer(index_buffer.slice(..), IndexFormat::Uint16);

        let mut index_offset = 0u32;
        for instance in &self.instances {
            if let Some(gpu_image) = self.images.get(&instance.image_id) {
                render_pass.set_bind_group(1, &gpu_image.bind_group, &[]);
                render_pass.draw_indexed(index_offset..index_offset + 6, 0, 0..1);
            }
            index_offset += 6;
        }
    }

    /// Remove an image from the GPU
    pub fn remove_image(&mut self, id: ImageId) {
        self.images.remove(&id);
    }

    /// Check if an image is loaded
    pub fn has_image(&self, id: ImageId) -> bool {
        self.images.contains_key(&id)
    }
}
