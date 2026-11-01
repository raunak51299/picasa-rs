//! wgpu device, surface, photo textures, and egui overlay rendering.

mod mipmap;
mod quad_pipeline;

use std::sync::Arc;

use anyhow::{Context, Result};
use egui_wgpu::{Renderer as EguiRenderer, RendererOptions, ScreenDescriptor};
use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::loader::decoder::FullImage;
use mipmap::MipmapGenerator;
use quad_pipeline::{PhotoUniforms, QuadPipelines, ShadowUniforms};

const DIM_BACKDROP_ALPHA: f64 = 0.75;
const PHOTO_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

pub struct GpuPhoto {
    /// EXIF-oriented dimensions.
    pub display_width: u32,
    pub display_height: u32,
    uv_row0: [f32; 2],
    uv_row1: [f32; 2],
    uv_offset: [f32; 2],
    _texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

impl Drop for GpuPhoto {
    fn drop(&mut self) {
        // Return the allocation immediately instead of waiting for the last
        // in-flight submission that referenced it.
        self._texture.destroy();
    }
}

pub struct PhotoDraw<'a> {
    pub photo: &'a GpuPhoto,
    pub center: [f32; 2],
    pub half_size: [f32; 2],
    pub angle: f32,
    pub alpha: f32,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    format: wgpu::TextureFormat,
    transparent: bool,
    gamma_target: bool,
    max_texture_side: u32,
    quad: QuadPipelines,
    mipmaps: MipmapGenerator,
    /// Created lazily so the first photo can be presented before egui's
    /// shader pipeline and font atlas are ready.
    egui_renderer: Option<EguiRenderer>,
}

impl Renderer {
    pub fn new(window: Arc<Window>) -> Result<Self> {
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        // DX12 is the best-behaved backend on Windows here: the Intel Vulkan
        // driver crashes inside vkCreateDevice, and DXGI exposes transparent
        // composition modes that Vulkan on this machine does not.
        #[cfg(target_os = "windows")]
        let backends = wgpu::Backends::DX12;
        #[cfg(not(target_os = "windows"))]
        let backends = wgpu::Backends::PRIMARY;

        // A DirectComposition-backed swapchain is the only DX12 presentation
        // path that advertises alpha composition modes. With it, the dimmed
        // backdrop is blended with the desktop instead of covering it.
        #[cfg(target_os = "windows")]
        let backend_options = {
            let mut options = wgpu::BackendOptions::from_env_or_default();
            options.dx12.presentation_system = wgpu::Dx12SwapchainKind::DxgiFromVisual;
            log::info!("presentation: DirectComposition visual (transparent swapchain)");
            options
        };
        #[cfg(not(target_os = "windows"))]
        let backend_options = wgpu::BackendOptions::from_env_or_default();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            backend_options,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        log::info!(
            "startup[{:.1}ms]: wgpu instance created",
            crate::startup_ms()
        );

        let surface = instance
            .create_surface(window)
            .context("could not create a wgpu surface for the window")?;
        log::info!(
            "startup[{:.1}ms]: wgpu surface created",
            crate::startup_ms()
        );

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            ..Default::default()
        }))
        .context("no compatible GPU adapter was found")?;
        log::info!("startup[{:.1}ms]: adapter selected", crate::startup_ms());

        let adapter_info = adapter.get_info();
        log::info!(
            "using GPU: {} ({:?}, {:?})",
            adapter_info.name,
            adapter_info.backend,
            adapter_info.device_type
        );

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("picasa-rs-device"),
            ..Default::default()
        }))
        .context("could not create a wgpu device")?;
        log::info!("startup[{:.1}ms]: device created", crate::startup_ms());

        let capabilities = surface.get_capabilities(&adapter);
        if capabilities.formats.is_empty() {
            anyhow::bail!("the window surface has no supported texture formats");
        }

        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .unwrap_or(capabilities.formats[0]);

        let mut config = surface
            .get_default_config(&adapter, width, height)
            .context("the surface is not compatible with the selected adapter")?;
        config.format = format;
        config.width = width;
        config.height = height;
        config.desired_maximum_frame_latency = 2;
        config.present_mode = if capabilities
            .present_modes
            .contains(&wgpu::PresentMode::Fifo)
        {
            wgpu::PresentMode::Fifo
        } else {
            *capabilities
                .present_modes
                .first()
                .unwrap_or(&wgpu::PresentMode::Fifo)
        };

        // PreMultiplied is the only transparent mode whose blend semantics
        // match both our photo shader and egui's premultiplied output. If the
        // compositor cannot do it, fall back to an opaque dark backdrop.
        let transparent = capabilities
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied);
        config.alpha_mode = if transparent {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            log::warn!("compositor does not support PreMultiplied alpha; using an opaque backdrop");
            wgpu::CompositeAlphaMode::Opaque
        };

        surface.configure(&device, &config);
        log::info!("startup[{:.1}ms]: surface configured", crate::startup_ms());

        let quad = QuadPipelines::new(&device, format);
        log::info!(
            "startup[{:.1}ms]: quad pipelines ready",
            crate::startup_ms()
        );
        let mipmaps = MipmapGenerator::new(&device, PHOTO_TEXTURE_FORMAT);
        log::info!(
            "startup[{:.1}ms]: photo pipelines ready",
            crate::startup_ms()
        );
        let max_texture_side = device.limits().max_texture_dimension_2d;

        log::info!(
            "surface: {:?}, alpha mode {:?}, transparent={}, max texture {}px",
            format,
            config.alpha_mode,
            transparent,
            max_texture_side
        );

        Ok(Self {
            device,
            queue,
            surface,
            config,
            format,
            transparent,
            gamma_target: !format.is_srgb(),
            max_texture_side,
            quad,
            mipmaps,
            egui_renderer: None,
        })
    }

    pub fn max_texture_side(&self) -> u32 {
        self.max_texture_side
    }

    fn ensure_egui(&mut self) {
        if self.egui_renderer.is_some() {
            return;
        }

        let start = std::time::Instant::now();
        self.egui_renderer = Some(EguiRenderer::new(
            &self.device,
            self.format,
            RendererOptions::default(),
        ));
        log::info!(
            "egui renderer ready in {:.1}ms",
            start.elapsed().as_secs_f64() * 1000.0
        );
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        let width = size.width.max(1);
        let height = size.height.max(1);
        if width == self.config.width && height == self.config.height {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn create_photo(&self, full: &FullImage) -> GpuPhoto {
        let t0 = std::time::Instant::now();
        let mip_level_count = full.width.max(full.height).max(1).ilog2() + 1;
        let size = wgpu::Extent3d {
            width: full.width,
            height: full.height,
            depth_or_array_layers: 1,
        };

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("photo_texture"),
            size,
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: PHOTO_TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });

        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &full.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * full.width),
                rows_per_image: Some(full.height),
            },
            size,
        );

        let t_upload = t0.elapsed();
        self.mipmaps
            .generate(&self.device, &self.queue, &texture, mip_level_count);
        let t_mips = t0.elapsed();

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("photo_texture_bind_group"),
            layout: &self.quad.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.quad.sampler),
                },
            ],
        });

        let (uv_row0, uv_row1, uv_offset) = full.uv_transform();
        let (display_width, display_height) = full.display_size();
        log::debug!(
            "create_photo {}x{}: upload {:?}, mips {:?}, total {:?}",
            full.width,
            full.height,
            t_upload,
            t_mips - t_upload,
            t0.elapsed()
        );

        GpuPhoto {
            display_width,
            display_height,
            uv_row0,
            uv_row1,
            uv_offset,
            _texture: texture,
            bind_group,
        }
    }

    /// Draw one frame. `paint_jobs` must already be tessellated by egui.
    pub fn render(
        &mut self,
        photo: Option<PhotoDraw<'_>>,
        paint_jobs: &[egui::ClippedPrimitive],
        mut textures_delta: egui::TexturesDelta,
        screen: ScreenDescriptor,
    ) -> Result<()> {
        // Consume the deltas up front: egui 0.37 panics if a TexturesDelta
        // with pending set/free entries is dropped, including on early returns.
        if !paint_jobs.is_empty()
            || !textures_delta.set.is_empty()
            || !textures_delta.free.is_empty()
        {
            self.ensure_egui();
        }
        if let Some(egui_renderer) = self.egui_renderer.as_mut() {
            for (id, deltas) in &textures_delta.set {
                for delta in deltas {
                    egui_renderer.update_texture(&self.device, &self.queue, *id, delta);
                }
            }
            textures_delta.set.clear();
            for id in &textures_delta.free {
                egui_renderer.free_texture(id);
            }
            textures_delta.free.clear();
        }

        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                log::warn!("surface lost; reconfiguring");
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                anyhow::bail!("wgpu validation error while acquiring the surface texture");
            }
        };

        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame_encoder"),
            });

        let user_command_buffers = match self.egui_renderer.as_mut() {
            Some(egui_renderer) => egui_renderer.update_buffers(
                &self.device,
                &self.queue,
                &mut encoder,
                paint_jobs,
                &screen,
            ),
            None => Vec::new(),
        };

        if let Some(draw) = &photo {
            let uniforms = PhotoUniforms {
                center: draw.center,
                half_size: draw.half_size,
                angle: draw.angle,
                alpha: draw.alpha,
                gamma: if self.gamma_target { 1.0 } else { 0.0 },
                _pad: 0.0,
                screen_size: [
                    screen.size_in_pixels[0] as f32,
                    screen.size_in_pixels[1] as f32,
                ],
                uv_row0: draw.photo.uv_row0,
                uv_row1: draw.photo.uv_row1,
                uv_offset: draw.photo.uv_offset,
            };
            self.quad.write_photo(&self.queue, &uniforms);

            let expand = (draw.half_size[0].min(draw.half_size[1]) * 0.32).clamp(14.0, 34.0);
            let shadow = ShadowUniforms {
                center: [draw.center[0], draw.center[1] + 9.0],
                half_size: draw.half_size,
                angle: draw.angle,
                expand,
                alpha: 0.55 * draw.alpha,
                radius: 8.0,
                screen_size: [
                    screen.size_in_pixels[0] as f32,
                    screen.size_in_pixels[1] as f32,
                ],
                _pad: [0.0; 2],
            };
            self.quad.write_shadow(&self.queue, &shadow);
        }

        let clear = if self.transparent {
            wgpu::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: DIM_BACKDROP_ALPHA,
            }
        } else {
            wgpu::Color {
                r: 0.015,
                g: 0.015,
                b: 0.02,
                a: 1.0,
            }
        };

        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("photo_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(clear),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();

            if let Some(draw) = &photo {
                pass.set_pipeline(&self.quad.shadow_pipeline);
                pass.set_bind_group(0, self.quad.shadow_bind_group(), &[]);
                pass.draw(0..6, 0..1);

                pass.set_pipeline(&self.quad.photo_pipeline);
                pass.set_bind_group(0, self.quad.photo_bind_group(), &[]);
                pass.set_bind_group(1, &draw.photo.bind_group, &[]);
                pass.draw(0..6, 0..1);
            }

            if let Some(egui_renderer) = self.egui_renderer.as_ref() {
                egui_renderer.render(&mut pass, paint_jobs, &screen);
            }
        }

        if !user_command_buffers.is_empty() {
            self.queue.submit(user_command_buffers);
        }
        self.queue.submit(Some(encoder.finish()));
        self.queue.present(surface_texture);

        Ok(())
    }
}
