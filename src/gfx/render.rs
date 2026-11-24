use super::{DrawFrame, geometry::*, resources::Pixels};
use mlua::Result;
use std::{
    collections::HashMap,
    rc::{Rc, Weak},
    sync::{Arc, Mutex},
};
use winit::{event_loop::ActiveEventLoop, window::Window};

pub(super) struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub(super) instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    errors: Arc<Mutex<Option<String>>>,
}
impl Gpu {
    fn descriptor(mut descriptor: wgpu::InstanceDescriptor) -> wgpu::InstanceDescriptor {
        // Initialize the platform backend rather than probing every driver on
        // hybrid-GPU machines. Explicit WGPU_BACKEND can override this choice.
        descriptor.backends = if cfg!(windows) {
            wgpu::Backends::DX12
        } else if cfg!(target_os = "macos") {
            wgpu::Backends::METAL
        } else {
            wgpu::Backends::VULKAN | wgpu::Backends::GL
        };
        descriptor.with_env()
    }
    pub fn new(
        event_loop: &ActiveEventLoop,
        window: Arc<Window>,
    ) -> Result<(Rc<Self>, wgpu::Surface<'static>)> {
        let instance = wgpu::Instance::new(Self::descriptor(
            wgpu::InstanceDescriptor::new_with_display_handle(Box::new(
                event_loop.owned_display_handle(),
            )),
        ));
        let surface = instance
            .create_surface(window)
            .map_err(|e| error(format!("graphics surface unavailable: {e}")))?;
        let gpu = pollster::block_on(Self::from_instance(instance, Some(&surface)))?;
        Ok((Rc::new(gpu), surface))
    }
    async fn from_instance(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Self> {
        let adapter = if let Ok(name) = std::env::var("WGPU_ADAPTER_NAME") {
            instance
                .enumerate_adapters(wgpu::Backends::all())
                .await
                .into_iter()
                .find(|a| {
                    a.get_info()
                        .name
                        .to_lowercase()
                        .contains(&name.to_lowercase())
                        && surface.is_none_or(|s| a.is_surface_supported(s))
                })
                .ok_or_else(|| {
                    error(format!(
                        "no graphics adapter matches WGPU_ADAPTER_NAME={name:?}"
                    ))
                })?
        } else {
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    compatible_surface: surface,
                    power_preference: wgpu::PowerPreference::from_env()
                        .unwrap_or(wgpu::PowerPreference::HighPerformance),
                    ..Default::default()
                })
                .await
                .map_err(|e| error(format!("no graphics adapter available: {e}")))?
        };
        if std::env::var_os("LUAUXTX_GFX_DIAGNOSTICS").is_some() {
            eprintln!("gfx adapter: {:?}", adapter.get_info());
        }
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("LuauXTX gfx"),
                ..Default::default()
            })
            .await
            .map_err(|e| error(format!("graphics device unavailable: {e}")))?;
        let errors = Arc::new(Mutex::new(None));
        let sink = errors.clone();
        device.on_uncaptured_error(Arc::new(move |e| {
            if let Ok(mut slot) = sink.lock() {
                *slot = Some(format!("graphics device error: {e}"));
            }
        }));
        Ok(Self {
            device,
            queue,
            instance,
            adapter,
            errors,
        })
    }
    fn check(&self) -> Result<()> {
        if let Ok(mut errors) = self.errors.lock() {
            if let Some(e) = errors.take() {
                return Err(error(e));
            }
        }
        Ok(())
    }
}
struct Texture {
    _texture: wgpu::Texture,
    bind: wgpu::BindGroup,
    source: Weak<Pixels>,
}
pub(super) struct Painter {
    gpu: Rc<Gpu>,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: HashMap<u64, Texture>,
    white: Rc<Pixels>,
    vertices: wgpu::Buffer,
    capacity: u64,
}
impl Painter {
    fn new(gpu: Rc<Gpu>, format: wgpu::TextureFormat) -> Result<Self> {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gfx primitives"),
            source: wgpu::ShaderSource::Wgsl(include_str!("render.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gfx textures"),
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gfx layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attrs = wgpu::vertex_attr_array![0=>Float32x2,1=>Float32x2,2=>Float32x4,3=>Float32x2,4=>Float32,5=>Float32,6=>Float32];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gfx 2D"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 52,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attrs,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("gfx linear sampling"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let capacity = 4096;
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gfx vertices"),
            size: capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let result = Self {
            gpu,
            pipeline,
            layout,
            sampler,
            textures: HashMap::new(),
            white: Rc::new(Pixels::new(1, 1, vec![255; 4])),
            vertices,
            capacity,
        };
        result.gpu.check()?;
        Ok(result)
    }
    fn texture(&mut self, pixels: &Rc<Pixels>) -> Result<()> {
        if self.textures.contains_key(&pixels.id) {
            return Ok(());
        }
        if pixels.width > self.gpu.device.limits().max_texture_dimension_2d
            || pixels.height > self.gpu.device.limits().max_texture_dimension_2d
        {
            return Err(error("image/text exceeds GPU texture limits"));
        }
        let size = wgpu::Extent3d {
            width: pixels.width,
            height: pixels.height,
            depth_or_array_layers: 1,
        };
        let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gfx image/text"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels.bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(pixels.width * 4),
                rows_per_image: Some(pixels.height),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = self
            .gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("gfx image/text"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
        self.textures.insert(
            pixels.id,
            Texture {
                _texture: texture,
                bind,
                source: Rc::downgrade(pixels),
            },
        );
        Ok(())
    }
    fn paint(&mut self, view: &wgpu::TextureView, frame: DrawFrame) -> Result<wgpu::CommandBuffer> {
        self.gpu.check()?;
        self.textures.retain(|_, v| v.source.strong_count() > 0);
        let mut bytes = Vec::new();
        let mut draws: Vec<(std::ops::Range<u32>, u64)> = Vec::new();
        let mut count = 0u32;
        for draw in frame.draws {
            let texture = draw.texture.as_ref().unwrap_or(&self.white).clone();
            self.texture(&texture)?;
            let start = count;
            let c = draw.color;
            let c = [linear(c[0]), linear(c[1]), linear(c[2]), c[3]];
            for index in 1..draw.vertices.len() - 1 {
                for v in [
                    draw.vertices[0],
                    draw.vertices[index],
                    draw.vertices[index + 1],
                ] {
                    count += 1;
                    if count > 1_000_000 {
                        return Err(error("frame exceeds 1000000 generated vertices"));
                    }
                    let data = [
                        v.position[0] / frame.size[0] * 2. - 1.,
                        1. - v.position[1] / frame.size[1] * 2.,
                        v.uv[0],
                        v.uv[1],
                        c[0],
                        c[1],
                        c[2],
                        c[3],
                        draw.size[0],
                        draw.size[1],
                        draw.radius,
                        draw.border,
                        if draw.texture.is_some() { 1. } else { 0. },
                    ];
                    for number in data {
                        bytes.extend_from_slice(&number.to_ne_bytes());
                    }
                }
            }
            if let Some((range, _)) = draws.last_mut().filter(|(_, id)| *id == texture.id) {
                range.end = count;
            } else {
                draws.push((start..count, texture.id));
            }
        }
        if bytes.len() as u64 > self.capacity {
            self.capacity = (bytes.len() as u64).next_power_of_two();
            self.vertices = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gfx vertices"),
                size: self.capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !bytes.is_empty() {
            self.gpu.queue.write_buffer(&self.vertices, 0, &bytes);
        }
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gfx frame"),
            });
        {
            let c = frame.clear;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gfx frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: linear(c[0]) as f64,
                            g: linear(c[1]) as f64,
                            b: linear(c[2]) as f64,
                            a: c[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            for (range, id) in draws {
                let texture = self
                    .textures
                    .get(&id)
                    .ok_or_else(|| error("graphics texture expired"))?;
                pass.set_bind_group(0, &texture.bind, &[]);
                pass.draw(range, 0..1);
            }
        }
        self.gpu.check()?;
        Ok(encoder.finish())
    }
}
pub(super) struct Renderer {
    gpu: Rc<Gpu>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    painter: Painter,
    window: Arc<Window>,
}
impl Renderer {
    pub fn new(
        gpu: Rc<Gpu>,
        surface: wgpu::Surface<'static>,
        window: Arc<Window>,
        vsync: bool,
    ) -> Result<Self> {
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| error("GPU cannot present to this window"))?;
        if let Some(format) = surface
            .get_capabilities(&gpu.adapter)
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
        {
            config.format = format;
        }
        config.present_mode = if vsync {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        let painter = Painter::new(gpu.clone(), config.format)?;
        let mut r = Self {
            gpu,
            surface,
            config,
            painter,
            window,
        };
        r.resize()?;
        Ok(r)
    }
    pub fn resize(&mut self) -> Result<()> {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        let max = self.gpu.device.limits().max_texture_dimension_2d;
        if size.width > max || size.height > max {
            return Err(error("window exceeds GPU surface dimension limits"));
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.gpu.device, &self.config);
        self.gpu.check()
    }
    pub fn draw(&mut self, mut frame: DrawFrame) -> Result<bool> {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(false);
        }
        if self.config.width != size.width || self.config.height != size.height {
            self.resize()?;
        }
        // A native size request can take effect before its resize event. Keep
        // logical coordinates stable even if that happens during a frame.
        let logical = size.to_logical::<f32>(self.window.scale_factor());
        frame.size = [logical.width, logical.height];
        let surface_frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => f,
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => {
                drop(f);
                self.resize()?;
                self.window.request_redraw();
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.resize()?;
                self.window.request_redraw();
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self
                    .gpu
                    .instance
                    .create_surface(self.window.clone())
                    .map_err(|e| error(format!("surface recovery failed: {e}")))?;
                self.resize()?;
                self.window.request_redraw();
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(error("graphics surface/device is unavailable"));
            }
        };
        let view = surface_frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let command = self.painter.paint(&view, frame)?;
        self.gpu.queue.submit(Some(command));
        self.gpu.queue.present(surface_frame);
        self.gpu.check()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gpu_pixels_preserve_order_alpha_rounding_clips_and_textures() {
        eprintln!("Initializing GPU readback fixture");
        let gpu = match pollster::block_on(Gpu::from_instance(
            wgpu::Instance::new(Gpu::descriptor(
                wgpu::InstanceDescriptor::new_without_display_handle(),
            )),
            None,
        )) {
            Ok(g) => Rc::new(g),
            Err(e) => {
                assert!(
                    std::env::var_os("LUAUXTX_REQUIRE_GPU").is_none(),
                    "GPU was required: {e}"
                );
                eprintln!("GPU readback test unavailable: {e}");
                return;
            }
        };
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut painter = Painter::new(gpu.clone(), format).unwrap();
        let mut frame = DrawFrame::new([256., 128.], 0.);
        frame.clear = [0., 0., 0., 1.];
        frame
            .add(0., 0., 80., 80., [1., 0., 0., 1.], 0., 0., None)
            .unwrap();
        frame
            .add(20., 20., 60., 60., [0., 0., 1., 0.5], 0., 0., None)
            .unwrap();
        frame
            .add(90., 0., 60., 60., [0., 1., 0., 1.], 20., 0., None)
            .unwrap();
        frame
            .clips
            .push(Some(vec![[160., 10.], [200., 10.], [180., 50.]]));
        frame
            .add(150., 0., 70., 70., [1.; 4], 0., 0., None)
            .unwrap();
        frame.clips.pop();
        frame
            .add(90., 75., 60., 45., [0., 1., 0., 1.], 10., 3., None)
            .unwrap();
        frame
            .transforms
            .push(Transform([0., 1., -1., 0., 240., 80.]));
        frame
            .add(0., 0., 30., 10., [0., 1., 1., 1.], 0., 0., None)
            .unwrap();
        frame.transforms.pop();
        let pixels = Rc::new(Pixels::new(1, 1, vec![255, 255, 0, 255]));
        frame
            .add(220., 10., 20., 20., [1.; 4], 0., 0., Some(pixels))
            .unwrap();
        let mut resources = super::super::resources::Resources::default();
        let font = resources.font(None, 18.).unwrap();
        let (text, pad) = font
            .0
            .borrow_mut()
            .as_mut()
            .unwrap()
            .raster("Hello", 1.)
            .unwrap();
        frame
            .add(
                5. - pad,
                90. - pad,
                text.width as f32,
                text.height as f32,
                [1.; 4],
                0.,
                0.,
                Some(text),
            )
            .unwrap();
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gfx test"),
            size: wgpu::Extent3d {
                width: 256,
                height: 128,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let command = painter.paint(&view, frame).unwrap();
        gpu.queue.submit(Some(command));
        let output = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256 * 128 * 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &output,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(1024),
                    rows_per_image: Some(128),
                },
            },
            wgpu::Extent3d {
                width: 256,
                height: 128,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit(Some(encoder.finish()));
        let (tx, rx) = std::sync::mpsc::channel();
        output
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        gpu.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(10)),
            })
            .unwrap();
        rx.recv().unwrap().unwrap();
        let bytes = output.slice(..).get_mapped_range().unwrap();
        let pixel = |x: usize, y: usize| bytes[(y * 256 + x) * 4..(y * 256 + x) * 4 + 4].to_vec();
        assert_eq!(pixel(5, 5), vec![255, 0, 0, 255]);
        let overlap = pixel(40, 40);
        assert!(
            (180..=195).contains(&overlap[0]) && (180..=195).contains(&overlap[2]),
            "alpha/order {overlap:?}"
        );
        assert_eq!(pixel(90, 0), vec![0, 0, 0, 255]);
        assert_eq!(pixel(120, 25), vec![0, 255, 0, 255]);
        assert_eq!(pixel(180, 20), vec![255; 4]);
        assert_eq!(pixel(160, 45), vec![0, 0, 0, 255]);
        assert_eq!(pixel(225, 15), vec![255, 255, 0, 255]);
        assert_eq!(pixel(120, 98), vec![0, 0, 0, 255]);
        assert_eq!(pixel(120, 76), vec![0, 255, 0, 255]);
        assert_eq!(pixel(235, 95), vec![0, 255, 255, 255]);
        assert!(
            (90..120).any(|y| (5..65).any(|x| pixel(x, y)[0] > 100)),
            "text raster is empty"
        );
        if let Some(path) = std::env::var_os("LUAUXTX_GFX_SNAPSHOT") {
            image::save_buffer(path, &bytes, 256, 128, image::ColorType::Rgba8).unwrap();
        }
        drop(bytes);
        output.unmap();
        gpu.check().unwrap();
        drop(font);
        gpu.queue.submit(Some(
            painter
                .paint(&view, DrawFrame::new([256., 128.], 0.))
                .unwrap(),
        ));
        assert_eq!(
            painter.textures.len(),
            1,
            "expired image/text GPU resources should be reclaimed; retain only white texture"
        );
    }
}
