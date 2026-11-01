//! Textured-quad and drop-shadow pipelines.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct PhotoUniforms {
    pub center: [f32; 2],
    pub half_size: [f32; 2],
    pub angle: f32,
    pub alpha: f32,
    pub gamma: f32,
    pub _pad: f32,
    pub screen_size: [f32; 2],
    pub uv_row0: [f32; 2],
    pub uv_row1: [f32; 2],
    pub uv_offset: [f32; 2],
    pub adjust: [f32; 4],
    pub gains: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ShadowUniforms {
    pub center: [f32; 2],
    pub half_size: [f32; 2],
    pub angle: f32,
    pub expand: f32,
    pub alpha: f32,
    pub radius: f32,
    pub screen_size: [f32; 2],
    pub _pad: [f32; 2],
}

pub struct QuadPipelines {
    pub photo_pipeline: wgpu::RenderPipeline,
    pub shadow_pipeline: wgpu::RenderPipeline,
    pub texture_layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
    photo_uniform_buffer: wgpu::Buffer,
    photo_uniform_bind_group: wgpu::BindGroup,
    shadow_uniform_buffer: wgpu::Buffer,
    shadow_uniform_bind_group: wgpu::BindGroup,
}

impl QuadPipelines {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let photo_uniform_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("photo_uniform_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let shadow_uniform_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("shadow_uniform_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("photo_texture_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("photo_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            lod_min_clamp: 0.0,
            lod_max_clamp: 32.0,
            compare: None,
            anisotropy_clamp: 8,
            border_color: None,
        });

        let photo_uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("photo_uniforms"),
            contents: bytemuck::bytes_of(&PhotoUniforms::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let photo_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("photo_uniform_bind_group"),
            layout: &photo_uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: photo_uniform_buffer.as_entire_binding(),
            }],
        });

        let shadow_uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("shadow_uniforms"),
            contents: bytemuck::bytes_of(&ShadowUniforms::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let shadow_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow_uniform_bind_group"),
            layout: &shadow_uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: shadow_uniform_buffer.as_entire_binding(),
            }],
        });

        let photo_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("photo_pipeline_layout"),
            bind_group_layouts: &[Some(&photo_uniform_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow_pipeline_layout"),
            bind_group_layouts: &[Some(&shadow_uniform_layout)],
            immediate_size: 0,
        });

        let photo_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("photo_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/photo.wgsl").into()),
        });
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shadow_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/shadow.wgsl").into()),
        });

        let blend = Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        });

        let photo_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("photo_pipeline"),
            layout: Some(&photo_layout),
            vertex: wgpu::VertexState {
                module: &photo_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &photo_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow_pipeline"),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shadow_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            photo_pipeline,
            shadow_pipeline,
            texture_layout,
            sampler,
            photo_uniform_buffer,
            photo_uniform_bind_group,
            shadow_uniform_buffer,
            shadow_uniform_bind_group,
        }
    }

    pub fn write_photo(&self, queue: &wgpu::Queue, uniforms: &PhotoUniforms) {
        queue.write_buffer(&self.photo_uniform_buffer, 0, bytemuck::bytes_of(uniforms));
    }

    pub fn write_shadow(&self, queue: &wgpu::Queue, uniforms: &ShadowUniforms) {
        queue.write_buffer(&self.shadow_uniform_buffer, 0, bytemuck::bytes_of(uniforms));
    }

    pub fn photo_bind_group(&self) -> &wgpu::BindGroup {
        &self.photo_uniform_bind_group
    }

    pub fn shadow_bind_group(&self) -> &wgpu::BindGroup {
        &self.shadow_uniform_bind_group
    }
}
