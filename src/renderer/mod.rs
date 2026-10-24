//! GPU pipeline: vello renders the vector scene with compute shaders into
//! an RGBA storage texture; a tiny wgpu render pass premultiplies alpha and
//! copies it into the DirectComposition swap chain (DX12).

pub mod icons;
pub mod shapes;
pub mod text;

use std::ffi::c_void;
use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use vello::peniko::Color;
use vello::wgpu;
use vello::{AaConfig, AaSupport, RenderParams, RendererOptions, Scene};
use windows::Win32::System::ProcessStatus::{
    K32EmptyWorkingSet, K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::GetCurrentProcess;

/// Private bytes and working set of this process in MB.
pub fn memory_usage() -> (f64, f64) {
    let mut counters =
        PROCESS_MEMORY_COUNTERS_EX { cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS,
            counters.cb,
        )
    };
    if !ok.as_bool() {
        return (0.0, 0.0);
    }
    (counters.PrivateUsage as f64 / 1_048_576.0, counters.WorkingSetSize as f64 / 1_048_576.0)
}

/// Releases pages that are no longer touched (shader compiler scratch,
/// one-time driver initialization) from the working set.
pub fn trim_working_set() {
    unsafe {
        let _ = K32EmptyWorkingSet(GetCurrentProcess());
    }
}

pub fn log_memory(stage: &str) {
    let (private, working) = memory_usage();
    log::debug!("memory [{stage}]: private {private:.1} MB, working set {working:.1} MB");
}

/// Device-level GPU objects. Creating them compiles vello's pipelines, which
/// takes a few seconds, so this happens on a background thread.
pub struct GpuContext {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    vello: vello::Renderer,
}

impl GpuContext {
    pub fn create(instance: &wgpu::Instance) -> Result<Self, String> {
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        }))
        .map_err(|err| format!("no DX12 adapter: {err}"))?;
        let info = adapter.get_info();
        log::info!("GPU: {} ({:?}, {:?})", info.name, info.backend, info.device_type);

        let limits = wgpu::Limits {
            // The DX12 backend sizes its shader-visible descriptor heap from
            // this limit (1,000,000 by default); the notch needs a few dozen.
            max_non_sampler_bindings: 4096,
            ..wgpu::Limits::default()
        };
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("notch-device"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
            // Small sub-allocation blocks: the notch needs a few MB of GPU
            // memory, and on integrated GPUs every block is system RAM.
            memory_hints: wgpu::MemoryHints::Manual { suballocated_device_memory_block_size: (4 << 20)..(8 << 20) },
            ..Default::default()
        }))
        .map_err(|err| format!("device creation failed: {err}"))?;
        device.on_uncaptured_error(std::sync::Arc::new(|err: wgpu::Error| {
            log::error!("wgpu: {err}");
        }));

        let started = std::time::Instant::now();
        let vello = vello::Renderer::new(
            &device,
            RendererOptions {
                use_cpu: false,
                antialiasing_support: AaSupport::area_only(),
                num_init_threads: None,
                pipeline_cache: None,
            },
        )
        .map_err(|err| format!("vello init failed: {err}"))?;
        log::info!("vello pipelines ready in {:?}", started.elapsed());
        log_memory("pipelines");
        Ok(Self { adapter, device, queue, vello })
    }
}

struct Target {
    view: wgpu::TextureView,
    blit_group: wgpu::BindGroup,
}

struct GpuState {
    ctx: GpuContext,
    config: wgpu::SurfaceConfiguration,
    target: Target,
    blit_layout: wgpu::BindGroupLayout,
    blit_pipeline: wgpu::RenderPipeline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameResult {
    Presented,
    /// Not ready yet or the surface is temporarily unavailable.
    Skipped,
}

pub struct Renderer {
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    gpu: Option<GpuState>,
    width: u32,
    height: u32,
}

impl Renderer {
    /// Creates the DX12 instance and a surface on the DirectComposition
    /// visual. The device is attached later via [`Renderer::attach`].
    ///
    /// # Safety
    /// `visual` must be a live `IDCompositionVisual*` that outlives the
    /// renderer.
    pub unsafe fn new(visual: *mut c_void, width: u32, height: u32) -> Result<Self, String> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::DX12;
        desc.flags = if cfg!(debug_assertions) {
            wgpu::InstanceFlags::debugging().with_env()
        } else {
            wgpu::InstanceFlags::empty().with_env()
        };
        let instance = wgpu::Instance::new(desc);
        let surface = unsafe { instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(visual)) }
            .map_err(|err| format!("surface creation failed: {err}"))?;
        Ok(Self { instance, surface, gpu: None, width: width.max(1), height: height.max(1) })
    }

    pub fn instance(&self) -> wgpu::Instance {
        self.instance.clone()
    }

    pub fn is_ready(&self) -> bool {
        self.gpu.is_some()
    }

    /// Configures the swap chain for premultiplied alpha and builds the
    /// premultiply/blit pipeline.
    pub fn attach(&mut self, ctx: GpuContext) -> Result<(), String> {
        let caps = self.surface.get_capabilities(&ctx.adapter);
        let format = [wgpu::TextureFormat::Bgra8Unorm, wgpu::TextureFormat::Rgba8Unorm]
            .into_iter()
            .find(|f| caps.formats.contains(f))
            .ok_or_else(|| format!("no supported 8-bit UNORM surface format in {:?}", caps.formats))?;
        let alpha_mode = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            log::warn!("premultiplied alpha unavailable ({:?}); transparency will not work", caps.alpha_modes);
            caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Auto)
        };
        let mut config = self
            .surface
            .get_default_config(&ctx.adapter, self.width, self.height)
            .ok_or("surface is not supported by the adapter")?;
        config.format = format;
        config.alpha_mode = alpha_mode;
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.desired_maximum_frame_latency = 1;
        config.view_formats = Vec::new();
        self.surface.configure(&ctx.device, &config);

        let device = &ctx.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("premultiply-blit"),
            source: wgpu::ShaderSource::Wgsl(BLIT_SHADER.into()),
        });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blit-pipeline-layout"),
            bind_group_layouts: &[Some(&blit_layout)],
            immediate_size: 0,
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blit-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let target = create_target(device, &blit_layout, self.width, self.height);
        self.gpu = Some(GpuState { ctx, config, target, blit_layout, blit_pipeline });
        Ok(())
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) == (self.width, self.height) {
            return;
        }
        self.width = width;
        self.height = height;
        if let Some(gpu) = &mut self.gpu {
            gpu.config.width = width;
            gpu.config.height = height;
            self.surface.configure(&gpu.ctx.device, &gpu.config);
            gpu.target = create_target(&gpu.ctx.device, &gpu.blit_layout, width, height);
        }
    }

    /// Renders `scene` and presents it (vsync-paced).
    pub fn render(&mut self, scene: &Scene) -> FrameResult {
        let Some(gpu) = &mut self.gpu else { return FrameResult::Skipped };
        let params = RenderParams {
            base_color: Color::TRANSPARENT,
            width: self.width,
            height: self.height,
            antialiasing_method: AaConfig::Area,
        };
        if let Err(err) = gpu.ctx.vello.render_to_texture(&gpu.ctx.device, &gpu.ctx.queue, scene, &gpu.target.view, &params)
        {
            log::error!("vello render failed: {err}");
            return FrameResult::Skipped;
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&gpu.ctx.device, &gpu.config);
                return FrameResult::Skipped;
            }
            other => {
                log::debug!("surface texture unavailable: {other:?}");
                return FrameResult::Skipped;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            gpu.ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("blit") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("premultiply-blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&gpu.blit_pipeline);
            pass.set_bind_group(0, &gpu.target.blit_group, &[]);
            pass.draw(0..3, 0..1);
        }
        gpu.ctx.queue.submit([encoder.finish()]);
        gpu.ctx.queue.present(frame);
        // Reclaim resources of completed submissions; never blocks.
        let _ = gpu.ctx.device.poll(wgpu::PollType::Poll);
        if log::log_enabled!(log::Level::Trace) {
            let (private, working) = memory_usage();
            log::trace!("frame presented: private {private:.1} MB, working set {working:.1} MB");
        }
        FrameResult::Presented
    }
}

fn create_target(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, width: u32, height: u32) -> Target {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vello-target"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let blit_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("blit-group"),
        layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }],
    });
    Target { view, blit_group }
}

/// Full-screen triangle; vello writes straight (non-premultiplied) alpha,
/// DirectComposition expects premultiplied.
const BLIT_SHADER: &str = r#"
@group(0) @binding(0) var source: texture_2d<f32>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VertexOutput;
    out.position = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureLoad(source, vec2<i32>(in.position.xy), 0);
    return vec4<f32>(color.rgb * color.a, color.a);
}
"#;

/// Minimal executor for wgpu's native futures, which complete immediately.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
        std::thread::yield_now();
    }
}
