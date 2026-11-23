use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use obs_core::{ObsError, Result, SceneId, SourceId};
use obs_filters::{VideoFilterInstance, VideoFilterKind};
use obs_scenes::{BlendMode, BoundsType, ResolvedRenderItem, SceneCollection};
use obs_transitions::{ActiveTransition, TransitionDirection, TransitionKind};
use obs_video::{ColorRange, ColorSpace, ScaleFilter, VideoConfig, VideoFrame};
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

const SHADERS_WGSL: &str = include_str!("shaders.wgsl");

#[derive(Clone, Copy)]
enum TargetSlot {
    Program,
    Preview,
    TransitionFrom,
    TransitionTo,
}

pub type PluginFilterCallback =
    Arc<dyn Fn(&str, &str, &serde_json::Value, &mut [u8], u32, u32) + Send + Sync>;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SpriteUniforms {
    mvp: [[f32; 4]; 4],
    uv_rect: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FilterUniforms {
    mode: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    p0: [f32; 4],
    p1: [f32; 4],
    p2: [f32; 4],
    p3: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CustomShaderUniforms {
    size_time: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TransitionUniforms {
    kind: u32,
    dir: u32,
    invert: u32,
    swipe_in: u32,
    progress: f32,
    softness: f32,
    switch_point: f32,
    _pad: f32,
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Nv12Uniforms {
    width: u32,
    height: u32,
    color_space: u32,
    full_range: u32,
}

struct CachedSourceTex {
    tex: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
    seq: u64,
}

struct RenderTarget {
    tex: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl RenderTarget {
    fn new(device: &wgpu::Device, width: u32, height: u32, label: &str) -> Self {
        let w = width.max(2);
        let h = height.max(2);
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            tex,
            view,
            width: w,
            height: h,
        }
    }
}

pub struct GpuCompositor {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    adapter_name: String,
    backend_name: String,
    start_time: Instant,

    sampler_linear: wgpu::Sampler,
    sampler_nearest: wgpu::Sampler,

    sprite_bgl: wgpu::BindGroupLayout,
    sprite_pipelines: HashMap<u8, wgpu::RenderPipeline>,

    filter_bgl: wgpu::BindGroupLayout,
    filter_pipeline: wgpu::RenderPipeline,

    custom_bgl: wgpu::BindGroupLayout,
    custom_pipelines: Mutex<HashMap<String, std::result::Result<wgpu::RenderPipeline, String>>>,

    transition_bgl: wgpu::BindGroupLayout,
    transition_pipeline: wgpu::RenderPipeline,

    nv12_bgl: wgpu::BindGroupLayout,
    nv12_pipeline: wgpu::ComputePipeline,

    white_tex: RenderTarget,
    green_tex: RenderTarget,
    red_tex: RenderTarget,
    fallback_luma_tex: RenderTarget,
    luma_cache: Mutex<HashMap<String, RenderTarget>>,
    mask_cache: Mutex<HashMap<String, RenderTarget>>,
    source_textures: Mutex<HashMap<SourceId, CachedSourceTex>>,
    delay_queues: Mutex<HashMap<SourceId, VecDeque<(Instant, VideoFrame)>>>,

    program_rt: RenderTarget,
    preview_rt: RenderTarget,
    transition_from_rt: RenderTarget,
    transition_to_rt: RenderTarget,
    output_rt: RenderTarget,
    multiview_rt: RenderTarget,
    filter_ping: RenderTarget,
    filter_pong: RenderTarget,

    nv12_gpu_buf: wgpu::Buffer,
    nv12_staging_buf: wgpu::Buffer,
    nv12_buf_bytes: u64,
    last_nv12: Vec<u8>,

    plugin_filter_hook: Option<PluginFilterCallback>,
    luma_dir: std::path::PathBuf,
    video_config: VideoConfig,
}

impl GpuCompositor {
    /// Creates a headless GPU compositor (used for integration tests, CLI `--self-test`, and fallback).
    pub fn new_headless(config: VideoConfig) -> Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| ObsError::Renderer("No compatible wgpu adapter found".to_string()))?;

        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("OxideStudio Headless Device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .map_err(|e| ObsError::Renderer(format!("request_device failed: {e}")))?;

        Ok(Self::from_wgpu(
            Arc::new(device),
            Arc::new(queue),
            info.name,
            format!("{:?}", info.backend),
            config,
        ))
    }

    /// Creates a GPU compositor sharing `eframe`'s `wgpu::Device` and `wgpu::Queue` for zero-copy UI preview.
    pub fn from_wgpu(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        adapter_name: String,
        backend_name: String,
        config: VideoConfig,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("OxideStudio Core Shaders"),
            source: wgpu::ShaderSource::Wgsl(SHADERS_WGSL.into()),
        });

        let sampler_linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Linear Clamp Sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let sampler_nearest = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Nearest Clamp Sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let sprite_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Sprite BGL"),
            entries: &[tex_entry(0), samp_entry(1), uniform_entry(2)],
        });
        let sprite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Sprite Pipeline Layout"),
            bind_group_layouts: &[&sprite_bgl],
            push_constant_ranges: &[],
        });

        let mut sprite_pipelines = HashMap::new();
        for mode in [
            BlendMode::Normal,
            BlendMode::Additive,
            BlendMode::Subtract,
            BlendMode::Screen,
            BlendMode::Multiply,
            BlendMode::Lighten,
            BlendMode::Darken,
        ] {
            let key = blend_mode_key(mode);
            let pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Sprite Pipeline"),
                layout: Some(&sprite_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_sprite"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_sprite"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: Some(blend_mode_to_wgpu(mode)),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });
            sprite_pipelines.insert(key, pipe);
        }

        let filter_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Filter BGL"),
            entries: &[tex_entry(0), samp_entry(1), uniform_entry(2), tex_entry(3)],
        });
        let filter_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Filter Pipeline Layout"),
            bind_group_layouts: &[&filter_bgl],
            push_constant_ranges: &[],
        });
        let filter_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Filter Pipeline"),
            layout: Some(&filter_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_fullscreen"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_filter"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let custom_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Custom Filter BGL"),
            entries: &[tex_entry(0), samp_entry(1), uniform_entry(2)],
        });

        let transition_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Transition BGL"),
            entries: &[
                tex_entry(0),
                tex_entry(1),
                tex_entry(2),
                samp_entry(3),
                uniform_entry(4),
            ],
        });
        let transition_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Transition Pipeline Layout"),
            bind_group_layouts: &[&transition_bgl],
            push_constant_ranges: &[],
        });
        let transition_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Transition Pipeline"),
            layout: Some(&transition_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_fullscreen"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_transition"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let nv12_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("NV12 BGL"),
            entries: &[
                tex_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let nv12_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("NV12 Compute Layout"),
            bind_group_layouts: &[&nv12_bgl],
            push_constant_ranges: &[],
        });
        let nv12_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("RGBA to NV12 Compute"),
            layout: Some(&nv12_layout),
            module: &shader,
            entry_point: Some("cs_rgba_to_nv12"),
            compilation_options: Default::default(),
            cache: None,
        });

        let white_tex = create_solid_rt(&device, &queue, [255, 255, 255, 255], "White 4x4");
        let green_tex = create_solid_rt(&device, &queue, [36, 214, 96, 255], "Tally Green");
        let red_tex = create_solid_rt(&device, &queue, [235, 58, 58, 255], "Tally Red");
        let fallback_luma_tex = create_gradient_luma_rt(&device, &queue);

        let (cw, ch) = config.sanitized_canvas_size();
        let (ow, oh) = config.sanitized_output_size();
        let program_rt = RenderTarget::new(&device, cw, ch, "Program Canvas");
        let preview_rt = RenderTarget::new(&device, cw, ch, "Preview Canvas");
        let transition_from_rt = RenderTarget::new(&device, cw, ch, "Transition From");
        let transition_to_rt = RenderTarget::new(&device, cw, ch, "Transition To");
        let output_rt = RenderTarget::new(&device, ow, oh, "Scaled Output");
        let multiview_rt = RenderTarget::new(&device, 1280, 720, "Multiview Canvas");
        let filter_ping = RenderTarget::new(&device, 1920, 1080, "Filter Ping");
        let filter_pong = RenderTarget::new(&device, 1920, 1080, "Filter Pong");

        let nv12_bytes = (((ow as u64) * (oh as u64) * 3) / 2).max(256);
        let nv12_gpu_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("NV12 GPU Output Buffer"),
            size: nv12_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let nv12_staging_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("NV12 Readback Staging"),
            size: nv12_bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let luma_dir = discover_luma_dir();

        Self {
            device,
            queue,
            adapter_name,
            backend_name,
            start_time: Instant::now(),
            sampler_linear,
            sampler_nearest,
            sprite_bgl,
            sprite_pipelines,
            filter_bgl,
            filter_pipeline,
            custom_bgl,
            custom_pipelines: Mutex::new(HashMap::new()),
            transition_bgl,
            transition_pipeline,
            nv12_bgl,
            nv12_pipeline,
            white_tex,
            green_tex,
            red_tex,
            fallback_luma_tex,
            luma_cache: Mutex::new(HashMap::new()),
            mask_cache: Mutex::new(HashMap::new()),
            source_textures: Mutex::new(HashMap::new()),
            delay_queues: Mutex::new(HashMap::new()),
            program_rt,
            preview_rt,
            transition_from_rt,
            transition_to_rt,
            output_rt,
            multiview_rt,
            filter_ping,
            filter_pong,
            nv12_gpu_buf,
            nv12_staging_buf,
            nv12_buf_bytes: nv12_bytes,
            last_nv12: vec![0u8; nv12_bytes as usize],
            plugin_filter_hook: None,
            luma_dir,
            video_config: config,
        }
    }

    pub fn set_plugin_filter_hook(&mut self, hook: PluginFilterCallback) {
        self.plugin_filter_hook = Some(hook);
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    pub fn backend_name(&self) -> &str {
        &self.backend_name
    }

    pub fn program_texture(&self) -> &wgpu::Texture {
        &self.program_rt.tex
    }

    pub fn program_view(&self) -> &wgpu::TextureView {
        &self.program_rt.view
    }

    pub fn preview_view(&self) -> &wgpu::TextureView {
        &self.preview_rt.view
    }

    pub fn multiview_view(&self) -> &wgpu::TextureView {
        &self.multiview_rt.view
    }

    pub fn canvas_size(&self) -> (u32, u32) {
        (self.program_rt.width, self.program_rt.height)
    }

    pub fn output_size(&self) -> (u32, u32) {
        (self.output_rt.width, self.output_rt.height)
    }

    pub fn source_dimensions(&self, source_id: SourceId) -> Option<(u32, u32)> {
        self.source_textures
            .lock()
            .get(&source_id)
            .map(|t| (t.width, t.height))
    }

    pub fn update_video_config(&mut self, config: VideoConfig) {
        let (cw, ch) = config.sanitized_canvas_size();
        let (ow, oh) = config.sanitized_output_size();

        if cw != self.program_rt.width || ch != self.program_rt.height {
            self.program_rt = RenderTarget::new(&self.device, cw, ch, "Program Canvas");
            self.preview_rt = RenderTarget::new(&self.device, cw, ch, "Preview Canvas");
            self.transition_from_rt = RenderTarget::new(&self.device, cw, ch, "Transition From");
            self.transition_to_rt = RenderTarget::new(&self.device, cw, ch, "Transition To");
        }
        if ow != self.output_rt.width || oh != self.output_rt.height {
            self.output_rt = RenderTarget::new(&self.device, ow, oh, "Scaled Output");
            let nv12_bytes = (((ow as u64) * (oh as u64) * 3) / 2).max(256);
            self.nv12_gpu_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("NV12 GPU Output Buffer"),
                size: nv12_bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            self.nv12_staging_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("NV12 Readback Staging"),
                size: nv12_bytes,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.nv12_buf_bytes = nv12_bytes;
            self.last_nv12.resize(nv12_bytes as usize, 0);
        }
        self.video_config = config;
    }

    /// Uploads changed source frames, renders Program, Preview (when Studio Mode), Transitions,
    /// Multiview (when requested), and performs GPU Output Scaling + NV12 conversion when encoding.
    pub fn render_frame(
        &mut self,
        collection: &SceneCollection,
        source_frames: &HashMap<SourceId, VideoFrame>,
        active_transition: Option<&ActiveTransition>,
        studio_mode: bool,
        render_multiview: bool,
        need_nv12_readback: bool,
    ) -> Option<&[u8]> {
        self.upload_dirty_sources(collection, source_frames);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("OxideStudio Render Frame"),
            });

        // 1. Render Program (with active transition if running)
        if let Some(tr) = active_transition {
            let from_items = collection.resolve_render_items(tr.from_scene);
            let to_items = collection.resolve_render_items(tr.to_scene);
            self.render_items_into(&mut encoder, TargetSlot::TransitionFrom, &from_items);
            self.render_items_into(&mut encoder, TargetSlot::TransitionTo, &to_items);
            self.render_transition(&mut encoder, tr);
        } else {
            let prog_items = collection.resolve_render_items(collection.active_scene_id);
            self.render_items_into(&mut encoder, TargetSlot::Program, &prog_items);
        }

        // 2. Render Preview (if Studio Mode)
        if studio_mode {
            let prev_id = collection
                .preview_scene_id
                .unwrap_or(collection.active_scene_id);
            let prev_items = collection.resolve_render_items(prev_id);
            self.render_items_into(&mut encoder, TargetSlot::Preview, &prev_items);
        }

        // 3. Render Multiview (2 top Preview/Program + up to 8 scenes bottom)
        if render_multiview {
            self.render_multiview(&mut encoder, collection, studio_mode);
        }

        // 4. Scale Program -> Output & convert RGBA -> NV12 on GPU when encoding
        let use_gpu_nv12 = need_nv12_readback
            && self.output_rt.width.is_multiple_of(4)
            && self.output_rt.height.is_multiple_of(2);
        if need_nv12_readback {
            self.render_scaled_output(&mut encoder);
            if use_gpu_nv12 {
                self.dispatch_rgba_to_nv12(&mut encoder);
                encoder.copy_buffer_to_buffer(
                    &self.nv12_gpu_buf,
                    0,
                    &self.nv12_staging_buf,
                    0,
                    self.nv12_buf_bytes,
                );
            }
        }

        self.queue.submit(Some(encoder.finish()));

        if use_gpu_nv12 {
            let slice = self.nv12_staging_buf.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |res| {
                let _ = tx.send(res);
            });
            self.device.poll(wgpu::Maintain::Wait);
            if let Ok(Ok(())) = rx.recv() {
                let mapped = slice.get_mapped_range();
                if mapped.len() == self.last_nv12.len() {
                    self.last_nv12.copy_from_slice(&mapped);
                }
                drop(mapped);
                self.nv12_staging_buf.unmap();
                return Some(&self.last_nv12);
            }
        } else if need_nv12_readback {
            // Fallback CPU NV12 if width is not multiple of 4
            if let Ok(rgba) = self.read_target_rgba(&self.output_rt) {
                let nv12 = obs_video::rgba_to_nv12(
                    &rgba,
                    self.output_rt.width,
                    self.output_rt.height,
                    self.video_config.color_space,
                    self.video_config.color_range,
                );
                self.last_nv12 = nv12;
                return Some(&self.last_nv12);
            }
        }
        None
    }

    pub fn read_program_rgba(&self) -> Result<Vec<u8>> {
        self.read_target_rgba(&self.program_rt)
    }

    pub fn read_target_rgba_for_test(&self) -> Result<Vec<u8>> {
        self.read_target_rgba(&self.program_rt)
    }

    fn read_target_rgba(&self, rt: &RenderTarget) -> Result<Vec<u8>> {
        let bytes_per_row_unpadded = rt.width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = bytes_per_row_unpadded.div_ceil(align) * align;
        let size = (padded_bytes_per_row as u64) * (rt.height as u64);

        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("RGBA Readback Staging"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("RGBA Readback"),
            });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &rt.tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(rt.height),
                },
            },
            wgpu::Extent3d {
                width: rt.width,
                height: rt.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(enc.finish()));

        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .map_err(|e| ObsError::Renderer(format!("Readback recv failed: {e}")))?
            .map_err(|e| ObsError::Renderer(format!("Readback map failed: {e}")))?;

        let view = slice.get_mapped_range();
        let mut out = Vec::with_capacity((bytes_per_row_unpadded as usize) * (rt.height as usize));
        for row in 0..rt.height as usize {
            let start = row * (padded_bytes_per_row as usize);
            out.extend_from_slice(&view[start..start + (bytes_per_row_unpadded as usize)]);
        }
        drop(view);
        staging.unmap();
        Ok(out)
    }

    fn upload_dirty_sources(
        &self,
        collection: &SceneCollection,
        source_frames: &HashMap<SourceId, VideoFrame>,
    ) {
        let now = Instant::now();
        let mut tex_map = self.source_textures.lock();
        tex_map.retain(|id, _| collection.sources.contains_key(id));

        for (id, frame) in source_frames {
            // Check if source has RenderDelay or PluginVideoFilter
            let mut effective_frame = frame.clone();
            if let Some(src) = collection.sources.get(id) {
                let delay_ms: u32 = src
                    .video_filters
                    .iter()
                    .filter(|f| f.enabled)
                    .filter_map(|f| match f.kind {
                        VideoFilterKind::RenderDelay { delay_ms } => Some(delay_ms),
                        _ => None,
                    })
                    .sum();
                if delay_ms > 0 {
                    let mut dq = self.delay_queues.lock();
                    let q = dq.entry(*id).or_default();
                    let need_push = q
                        .back()
                        .map(|(_, f)| f.seq != effective_frame.seq)
                        .unwrap_or(true);
                    if need_push {
                        q.push_back((now, effective_frame.clone()));
                        while q.len() > 240 {
                            q.pop_front();
                        }
                    }
                    let delay_dur = std::time::Duration::from_millis(delay_ms.min(5000) as u64);
                    while q.len() > 1 && now.duration_since(q[1].0) >= delay_dur {
                        q.pop_front();
                    }
                    if let Some((_, delayed)) = q.front() {
                        effective_frame = delayed.clone();
                    }
                }

                // Apply any enabled PluginVideoFilter directly before GPU upload
                let plugin_filters: Vec<(&str, &str, &serde_json::Value)> = src
                    .video_filters
                    .iter()
                    .filter(|f| f.enabled)
                    .filter_map(|f| match &f.kind {
                        VideoFilterKind::PluginVideoFilter {
                            plugin_id,
                            filter_id,
                            settings,
                        } => Some((plugin_id.as_str(), filter_id.as_str(), settings)),
                        _ => None,
                    })
                    .collect();

                if !plugin_filters.is_empty() {
                    if let Some(hook) = &self.plugin_filter_hook {
                        let mut buf = (*effective_frame.data).clone();
                        for (pid, fid, cfg) in plugin_filters {
                            hook(
                                pid,
                                fid,
                                cfg,
                                &mut buf,
                                effective_frame.width,
                                effective_frame.height,
                            );
                        }
                        effective_frame = VideoFrame::new_rgba(
                            effective_frame.width,
                            effective_frame.height,
                            buf,
                            effective_frame.pts_us,
                            effective_frame.seq.wrapping_add(1_000_000_000),
                        );
                    }
                }
            }

            let need_upload = tex_map
                .get(id)
                .map(|c| {
                    c.seq != effective_frame.seq
                        || c.width != effective_frame.width
                        || c.height != effective_frame.height
                })
                .unwrap_or(true);

            if !need_upload || effective_frame.width == 0 || effective_frame.height == 0 {
                continue;
            }

            let need_recreate = tex_map
                .get(id)
                .map(|c| c.width != effective_frame.width || c.height != effective_frame.height)
                .unwrap_or(true);

            if need_recreate {
                let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Source Texture"),
                    size: wgpu::Extent3d {
                        width: effective_frame.width,
                        height: effective_frame.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
                tex_map.insert(
                    *id,
                    CachedSourceTex {
                        tex,
                        view,
                        width: effective_frame.width,
                        height: effective_frame.height,
                        seq: 0,
                    },
                );
            }

            if let Some(entry) = tex_map.get_mut(id) {
                let expected =
                    (effective_frame.width as usize) * (effective_frame.height as usize) * 4;
                if effective_frame.data.len() >= expected {
                    self.queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &entry.tex,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        &effective_frame.data[..expected],
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(effective_frame.width * 4),
                            rows_per_image: Some(effective_frame.height),
                        },
                        wgpu::Extent3d {
                            width: effective_frame.width,
                            height: effective_frame.height,
                            depth_or_array_layers: 1,
                        },
                    );
                    entry.seq = effective_frame.seq;
                }
            }
        }
    }

    fn target_rt(&self, slot: TargetSlot) -> &RenderTarget {
        match slot {
            TargetSlot::Program => &self.program_rt,
            TargetSlot::Preview => &self.preview_rt,
            TargetSlot::TransitionFrom => &self.transition_from_rt,
            TargetSlot::TransitionTo => &self.transition_to_rt,
        }
    }

    fn render_items_into(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        slot: TargetSlot,
        items: &[ResolvedRenderItem],
    ) {
        let (cw, ch) = {
            let t = self.target_rt(slot);
            // Clear target to opaque black first
            let _clear_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Clear Canvas"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            (t.width as f32, t.height as f32)
        };

        for item in items {
            let Some((src_w, src_h)) = self
                .source_textures
                .lock()
                .get(&item.source_id)
                .map(|t| (t.width, t.height))
            else {
                continue;
            };

            let gpu_filters: Vec<&VideoFilterInstance> = item
                .filters
                .iter()
                .filter(|f| {
                    f.enabled
                        && !matches!(
                            f.kind,
                            VideoFilterKind::RenderDelay { .. }
                                | VideoFilterKind::PluginVideoFilter { .. }
                        )
                })
                .collect();

            // Run GPU filter chain into ping/pong render targets when filters are active
            let mut use_ping = false;
            if !gpu_filters.is_empty() {
                self.ensure_filter_targets(src_w, src_h);
                // Copy source texture into filter_ping via identity sprite or filter pass
                {
                    let tex_guard = self.source_textures.lock();
                    if let Some(src_tex) = tex_guard.get(&item.source_id) {
                        self.blit_view_to_rt(encoder, &src_tex.view, &self.filter_ping);
                    }
                }
                let mut current_is_ping = true;
                for f in &gpu_filters {
                    let success = self.apply_single_gpu_filter(encoder, f, current_is_ping);
                    if success {
                        current_is_ping = !current_is_ping;
                    }
                }
                use_ping = true;
                // Store whether final output is in ping or pong via temporary swap
                if !current_is_ping {
                    self.blit_view_to_rt(encoder, &self.filter_pong.view, &self.filter_ping);
                }
            }

            // Compute crop / bounds / transform matrix
            let mut eff_w = src_w as f32;
            let mut eff_h = src_h as f32;
            let mut u0 = (item.crop.left as f32 / eff_w).clamp(0.0, 0.99);
            let mut v0 = (item.crop.top as f32 / eff_h).clamp(0.0, 0.99);
            let mut u1 = (1.0 - (item.crop.right as f32 / eff_w)).clamp(u0 + 0.005, 1.0);
            let mut v1 = (1.0 - (item.crop.bottom as f32 / eff_h)).clamp(v0 + 0.005, 1.0);

            for f in &gpu_filters {
                if let VideoFilterKind::CropPad {
                    left,
                    top,
                    right,
                    bottom,
                    ..
                } = f.kind
                {
                    u0 = (u0 + (left.max(0) as f32 / eff_w)).clamp(0.0, 0.98);
                    v0 = (v0 + (top.max(0) as f32 / eff_h)).clamp(0.0, 0.98);
                    u1 = (u1 - (right.max(0) as f32 / eff_w)).clamp(u0 + 0.005, 1.0);
                    v1 = (v1 - (bottom.max(0) as f32 / eff_h)).clamp(v0 + 0.005, 1.0);
                } else if let VideoFilterKind::ScalingAspect {
                    target_width,
                    target_height,
                    ..
                } = f.kind
                {
                    eff_w = target_width.max(8) as f32;
                    eff_h = target_height.max(8) as f32;
                }
            }

            let cropped_w = (eff_w * (u1 - u0)).max(1.0);
            let cropped_h = (eff_h * (v1 - v0)).max(1.0);
            let (draw_w, draw_h) = compute_bounded_size(
                cropped_w,
                cropped_h,
                item.scale,
                item.bounds_type,
                item.bounds_size,
            );

            // Pixel ortho projection: (0,0) top-left -> (cw, ch) bottom-right
            let proj = Mat4::orthographic_rh(0.0, cw, ch, 0.0, -1.0, 1.0);
            let model = Mat4::from_translation(Vec3::new(item.pos[0], item.pos[1], 0.0))
                * Mat4::from_rotation_z(item.rotation_deg.to_radians())
                * Mat4::from_scale(Vec3::new(draw_w, draw_h, 1.0));
            let mvp = proj * model;

            let uniform = SpriteUniforms {
                mvp: mvp.to_cols_array_2d(),
                uv_rect: [u0, v0, u1, v1],
                params: [item.opacity, 0.0, 0.0, 0.0],
            };
            let ubuf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Sprite Uniform"),
                size: std::mem::size_of::<SpriteUniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue
                .write_buffer(&ubuf, 0, bytemuck::bytes_of(&uniform));

            let bg = {
                let tex_guard = self.source_textures.lock();
                let view_ref = if use_ping {
                    &self.filter_ping.view
                } else {
                    &tex_guard.get(&item.source_id).unwrap().view
                };
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Sprite BG"),
                    layout: &self.sprite_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(view_ref),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler_linear),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: ubuf.as_entire_binding(),
                        },
                    ],
                })
            };

            let pipe = self
                .sprite_pipelines
                .get(&blend_mode_key(item.blend_mode))
                .unwrap_or_else(|| self.sprite_pipelines.get(&0).unwrap());

            let target_ref = self.target_rt(slot);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Draw Scene Item"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_ref.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..6, 0..1);
        }
    }

    fn ensure_filter_targets(&mut self, w: u32, h: u32) {
        let tw = w.clamp(16, 3840);
        let th = h.clamp(16, 2160);
        if self.filter_ping.width != tw || self.filter_ping.height != th {
            self.filter_ping = RenderTarget::new(&self.device, tw, th, "Filter Ping");
            self.filter_pong = RenderTarget::new(&self.device, tw, th, "Filter Pong");
        }
    }

    fn blit_view_to_rt(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        src_view: &wgpu::TextureView,
        dst: &RenderTarget,
    ) {
        let u = FilterUniforms {
            mode: 7,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
            p0: [0.0; 4],
            p1: [0.0; 4],
            p2: [0.0; 4],
            p3: [0.0; 4],
        };
        self.run_filter_pass(encoder, src_view, &self.white_tex.view, dst, &u, false);
    }

    fn apply_single_gpu_filter(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        filter: &VideoFilterInstance,
        src_is_ping: bool,
    ) -> bool {
        let (src_rt, dst_rt) = if src_is_ping {
            (&self.filter_ping, &self.filter_pong)
        } else {
            (&self.filter_pong, &self.filter_ping)
        };

        if let VideoFilterKind::CustomWgslShader {
            wgsl_source,
            param_a,
            param_b,
            param_c,
            param_d,
            ..
        } = &filter.kind
        {
            return self.run_custom_wgsl(
                encoder,
                wgsl_source,
                &src_rt.view,
                dst_rt,
                [*param_a, *param_b, *param_c, *param_d],
            );
        }

        let elapsed = self.start_time.elapsed().as_secs_f32();
        let mut mask_view_owned: Option<wgpu::TextureView> = None;

        let uniforms = match &filter.kind {
            VideoFilterKind::ChromaKey {
                preset,
                custom_color_rgba,
                similarity,
                smoothness,
                key_color_spill_reduction,
                opacity,
                contrast,
                brightness,
                gamma,
            } => {
                let rgb = preset.rgba(*custom_color_rgba);
                let r = rgb[0] as f32 / 255.0;
                let g = rgb[1] as f32 / 255.0;
                let b = rgb[2] as f32 / 255.0;
                let cb = -0.100644 * r - 0.338572 * g + 0.439216 * b + 0.501961;
                let cr = 0.439216 * r - 0.398942 * g - 0.040274 * b + 0.501961;
                FilterUniforms {
                    mode: 0,
                    _pad0: 0,
                    _pad1: 0,
                    _pad2: 0,
                    p0: [cb, cr, *similarity, *smoothness],
                    p1: [*key_color_spill_reduction, *opacity, *contrast, *brightness],
                    p2: [*gamma, 0.0, 0.0, 0.0],
                    p3: [0.0; 4],
                }
            }
            VideoFilterKind::ColorKey {
                key_color_rgba,
                similarity,
                smoothness,
                opacity,
                contrast,
                brightness,
                gamma,
            } => FilterUniforms {
                mode: 1,
                _pad0: 0,
                _pad1: 0,
                _pad2: 0,
                p0: [
                    key_color_rgba[0] as f32 / 255.0,
                    key_color_rgba[1] as f32 / 255.0,
                    key_color_rgba[2] as f32 / 255.0,
                    *similarity,
                ],
                p1: [*smoothness, *opacity, *contrast, *brightness],
                p2: [*gamma, 0.0, 0.0, 0.0],
                p3: [0.0; 4],
            },
            VideoFilterKind::LumaKey {
                luma_max,
                luma_max_smooth,
                luma_min,
                luma_min_smooth,
            } => FilterUniforms {
                mode: 2,
                _pad0: 0,
                _pad1: 0,
                _pad2: 0,
                p0: [*luma_min, *luma_min_smooth, *luma_max, *luma_max_smooth],
                p1: [0.0; 4],
                p2: [0.0; 4],
                p3: [0.0; 4],
            },
            VideoFilterKind::ColorCorrection {
                gamma,
                contrast,
                brightness,
                saturation,
                hue_shift_deg,
                opacity,
                color_multiply_rgba,
                color_add_rgba,
            } => FilterUniforms {
                mode: 3,
                _pad0: 0,
                _pad1: 0,
                _pad2: 0,
                p0: [*gamma, *contrast, *brightness, *saturation],
                p1: [hue_shift_deg.to_radians(), *opacity, 0.0, 0.0],
                p2: [
                    color_multiply_rgba[0] as f32 / 255.0,
                    color_multiply_rgba[1] as f32 / 255.0,
                    color_multiply_rgba[2] as f32 / 255.0,
                    color_multiply_rgba[3] as f32 / 255.0,
                ],
                p3: [
                    color_add_rgba[0] as f32 / 255.0,
                    color_add_rgba[1] as f32 / 255.0,
                    color_add_rgba[2] as f32 / 255.0,
                    0.0,
                ],
            },
            VideoFilterKind::Sharpen { sharpness } => FilterUniforms {
                mode: 4,
                _pad0: 0,
                _pad1: 0,
                _pad2: 0,
                p0: [*sharpness, 0.0, 0.0, 0.0],
                p1: [0.0; 4],
                p2: [0.0; 4],
                p3: [0.0; 4],
            },
            VideoFilterKind::Scroll {
                speed_x,
                speed_y,
                loop_scroll,
                ..
            } => {
                let u_off = (speed_x * elapsed) / (src_rt.width.max(1) as f32);
                let v_off = (speed_y * elapsed) / (src_rt.height.max(1) as f32);
                FilterUniforms {
                    mode: 5,
                    _pad0: 0,
                    _pad1: 0,
                    _pad2: 0,
                    p0: [u_off, v_off, if *loop_scroll { 1.0 } else { 0.0 }, 0.0],
                    p1: [0.0; 4],
                    p2: [0.0; 4],
                    p3: [0.0; 4],
                }
            }
            VideoFilterKind::ImageMaskBlend {
                mask_type,
                image_path,
                color_rgba,
                opacity,
                ..
            } => {
                if !image_path.trim().is_empty() {
                    mask_view_owned = self.load_cached_image_view(image_path, &self.mask_cache);
                }
                let mtype = match mask_type {
                    obs_filters::MaskBlendType::AlphaMaskColorChannel => 0.0,
                    obs_filters::MaskBlendType::AlphaMaskAlphaChannel => 1.0,
                    obs_filters::MaskBlendType::BlendMultiply => 2.0,
                    obs_filters::MaskBlendType::BlendAddition => 3.0,
                    obs_filters::MaskBlendType::BlendSubtraction => 4.0,
                };
                FilterUniforms {
                    mode: 6,
                    _pad0: 0,
                    _pad1: 0,
                    _pad2: 0,
                    p0: [mtype, *opacity, 0.0, 0.0],
                    p1: [
                        color_rgba[0] as f32 / 255.0,
                        color_rgba[1] as f32 / 255.0,
                        color_rgba[2] as f32 / 255.0,
                        color_rgba[3] as f32 / 255.0,
                    ],
                    p2: [0.0; 4],
                    p3: [0.0; 4],
                }
            }
            VideoFilterKind::CropPad { .. }
            | VideoFilterKind::ScalingAspect { .. }
            | VideoFilterKind::RenderDelay { .. }
            | VideoFilterKind::PluginVideoFilter { .. }
            | VideoFilterKind::CustomWgslShader { .. } => return false,
        };

        let mask_ref = mask_view_owned.as_ref().unwrap_or(&self.white_tex.view);
        self.run_filter_pass(encoder, &src_rt.view, mask_ref, dst_rt, &uniforms, false);
        true
    }

    fn run_filter_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        src_view: &wgpu::TextureView,
        mask_view: &wgpu::TextureView,
        dst: &RenderTarget,
        uniforms: &FilterUniforms,
        nearest: bool,
    ) {
        let ubuf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Filter Uniform"),
            size: std::mem::size_of::<FilterUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&ubuf, 0, bytemuck::bytes_of(uniforms));

        let samp = if nearest {
            &self.sampler_nearest
        } else {
            &self.sampler_linear
        };
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Filter BG"),
            layout: &self.filter_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(samp),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: ubuf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(mask_view),
                },
            ],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Filter Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &dst.view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.filter_pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..6, 0..1);
    }

    fn run_custom_wgsl(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        wgsl_src: &str,
        src_view: &wgpu::TextureView,
        dst: &RenderTarget,
        params: [f32; 4],
    ) -> bool {
        let mut cache = self.custom_pipelines.lock();
        if !cache.contains_key(wgsl_src) {
            let compiled = self.compile_custom_wgsl_pipeline(wgsl_src);
            cache.insert(wgsl_src.to_string(), compiled);
        }
        let Ok(pipe) = cache.get(wgsl_src).unwrap() else {
            return false;
        };

        let cu = CustomShaderUniforms {
            size_time: [
                dst.width as f32,
                dst.height as f32,
                self.start_time.elapsed().as_secs_f32(),
                params[0],
            ],
            params,
        };
        let ubuf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Custom Filter Uniform"),
            size: std::mem::size_of::<CustomShaderUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&ubuf, 0, bytemuck::bytes_of(&cu));

        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Custom Filter BG"),
            layout: &self.custom_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler_linear),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: ubuf.as_entire_binding(),
                },
            ],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Custom WGSL Filter Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &dst.view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(pipe);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..6, 0..1);
        true
    }

    fn compile_custom_wgsl_pipeline(
        &self,
        wgsl_src: &str,
    ) -> std::result::Result<wgpu::RenderPipeline, String> {
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let vs_mod = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Custom VS"),
                source: wgpu::ShaderSource::Wgsl(SHADERS_WGSL.into()),
            });
        let fs_mod = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Custom FS"),
                source: wgpu::ShaderSource::Wgsl(wgsl_src.into()),
            });
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Custom Filter Layout"),
                bind_group_layouts: &[&self.custom_bgl],
                push_constant_ranges: &[],
            });
        let pipe = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Custom WGSL Pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &vs_mod,
                    entry_point: Some("vs_fullscreen"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &fs_mod,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });
        if let Some(err) = pollster::block_on(self.device.pop_error_scope()) {
            return Err(format!("WGSL validation failed: {err}"));
        }
        Ok(pipe)
    }

    fn render_transition(&self, encoder: &mut wgpu::CommandEncoder, tr: &ActiveTransition) {
        let progress = tr.eased_progress();
        let (kind, dir, invert, swipe_in, softness, switch_point, color, luma_name) =
            match &tr.config.kind {
                TransitionKind::Cut => (0u32, 0, 0, 0, 0.1, 0.5, [0.0; 4], None),
                TransitionKind::Fade | TransitionKind::Stinger { .. } => {
                    (0u32, 0, 0, 0, 0.1, 0.5, [0.0; 4], None)
                }
                TransitionKind::Swipe {
                    direction,
                    swipe_in,
                } => (
                    1u32,
                    dir_to_u32(*direction),
                    0,
                    u32::from(*swipe_in),
                    0.1,
                    0.5,
                    [0.0; 4],
                    None,
                ),
                TransitionKind::Slide { direction } => {
                    (2u32, dir_to_u32(*direction), 0, 0, 0.1, 0.5, [0.0; 4], None)
                }
                TransitionKind::FadeToColor {
                    color_rgba,
                    switch_point,
                } => (
                    3u32,
                    0,
                    0,
                    0,
                    0.1,
                    *switch_point,
                    [
                        color_rgba[0] as f32 / 255.0,
                        color_rgba[1] as f32 / 255.0,
                        color_rgba[2] as f32 / 255.0,
                        color_rgba[3] as f32 / 255.0,
                    ],
                    None,
                ),
                TransitionKind::LumaWipe {
                    wipe_name,
                    softness,
                    invert,
                } => (
                    4u32,
                    0,
                    u32::from(*invert),
                    0,
                    *softness,
                    0.5,
                    [0.0; 4],
                    Some(wipe_name.as_str()),
                ),
            };

        let luma_view_owned = luma_name.and_then(|name| {
            let p = self.luma_dir.join(name);
            self.load_cached_image_view(&p.to_string_lossy(), &self.luma_cache)
        });
        let luma_view = luma_view_owned
            .as_ref()
            .unwrap_or(&self.fallback_luma_tex.view);

        let tu = TransitionUniforms {
            kind,
            dir,
            invert,
            swipe_in,
            progress,
            softness,
            switch_point,
            _pad: 0.0,
            color,
        };
        let ubuf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Transition Uniform"),
            size: std::mem::size_of::<TransitionUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&ubuf, 0, bytemuck::bytes_of(&tu));

        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Transition BG"),
            layout: &self.transition_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.transition_from_rt.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.transition_to_rt.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(luma_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler_linear),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: ubuf.as_entire_binding(),
                },
            ],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Transition Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.program_rt.view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.transition_pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..6, 0..1);
    }

    fn render_multiview(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        collection: &SceneCollection,
        studio_mode: bool,
    ) {
        // Pre-render up to 8 scenes into transition_to_rt temporarily as we blit them into multiview slots
        let mw = self.multiview_rt.width as f32;
        let mh = self.multiview_rt.height as f32;
        let top_h = mh * 0.5;
        let half_w = mw * 0.5;

        // Clear multiview canvas
        {
            let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Clear Multiview"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.multiview_rt.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.07,
                            g: 0.08,
                            b: 0.10,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }

        // Top-left: Preview (with green tally border), Top-right: Program (with red tally border)
        let prev_view = if studio_mode {
            &self.preview_rt.view
        } else {
            &self.program_rt.view
        };
        self.draw_textured_rect(
            encoder,
            &self.multiview_rt,
            &self.green_tex.view,
            4.0,
            4.0,
            half_w - 8.0,
            top_h - 8.0,
        );
        self.draw_textured_rect(
            encoder,
            &self.multiview_rt,
            prev_view,
            8.0,
            8.0,
            half_w - 16.0,
            top_h - 16.0,
        );

        self.draw_textured_rect(
            encoder,
            &self.multiview_rt,
            &self.red_tex.view,
            half_w + 4.0,
            4.0,
            half_w - 8.0,
            top_h - 8.0,
        );
        self.draw_textured_rect(
            encoder,
            &self.multiview_rt,
            &self.program_rt.view,
            half_w + 8.0,
            8.0,
            half_w - 16.0,
            top_h - 16.0,
        );

        // Bottom 2x4 grid: up to 8 scenes
        let cell_w = mw / 4.0;
        let cell_h = top_h / 2.0;
        let scene_ids: Vec<SceneId> = collection.scenes.iter().take(8).map(|s| s.id).collect();
        for (idx, sid) in scene_ids.into_iter().enumerate() {
            let items = collection.resolve_render_items(sid);
            self.render_items_into(encoder, TargetSlot::TransitionTo, &items);
            let col = (idx % 4) as f32;
            let row = (idx / 4) as f32;
            let x = col * cell_w;
            let y = top_h + row * cell_h;
            if sid == collection.active_scene_id {
                self.draw_textured_rect(
                    encoder,
                    &self.multiview_rt,
                    &self.red_tex.view,
                    x + 3.0,
                    y + 3.0,
                    cell_w - 6.0,
                    cell_h - 6.0,
                );
            } else if collection.preview_scene_id == Some(sid) {
                self.draw_textured_rect(
                    encoder,
                    &self.multiview_rt,
                    &self.green_tex.view,
                    x + 3.0,
                    y + 3.0,
                    cell_w - 6.0,
                    cell_h - 6.0,
                );
            }
            self.draw_textured_rect(
                encoder,
                &self.multiview_rt,
                &self.transition_to_rt.view,
                x + 6.0,
                y + 6.0,
                cell_w - 12.0,
                cell_h - 12.0,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_textured_rect(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        dst: &RenderTarget,
        src_view: &wgpu::TextureView,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    ) {
        let proj = Mat4::orthographic_rh(0.0, dst.width as f32, dst.height as f32, 0.0, -1.0, 1.0);
        let model =
            Mat4::from_translation(Vec3::new(x, y, 0.0)) * Mat4::from_scale(Vec3::new(w, h, 1.0));
        let u = SpriteUniforms {
            mvp: (proj * model).to_cols_array_2d(),
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            params: [1.0, 0.0, 0.0, 0.0],
        };
        let ubuf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rect Uniform"),
            size: std::mem::size_of::<SpriteUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&ubuf, 0, bytemuck::bytes_of(&u));
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Rect BG"),
            layout: &self.sprite_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler_linear),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: ubuf.as_entire_binding(),
                },
            ],
        });
        let pipe = self.sprite_pipelines.get(&0).unwrap();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Multiview Tile"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &dst.view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(pipe);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..6, 0..1);
    }

    fn render_scaled_output(&self, encoder: &mut wgpu::CommandEncoder) {
        let (nearest, sharpen_k) = match self.video_config.scale_filter {
            ScaleFilter::Point => (true, 0.0),
            ScaleFilter::Bilinear => (false, 0.0),
            ScaleFilter::Bicubic => (false, 0.25),
            ScaleFilter::Lanczos => (false, 0.45),
            ScaleFilter::Area => (false, 0.10),
        };
        let u = FilterUniforms {
            mode: 7,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
            p0: [sharpen_k, 0.0, 0.0, 0.0],
            p1: [0.0; 4],
            p2: [0.0; 4],
            p3: [0.0; 4],
        };
        self.run_filter_pass(
            encoder,
            &self.program_rt.view,
            &self.white_tex.view,
            &self.output_rt,
            &u,
            nearest,
        );
    }

    fn dispatch_rgba_to_nv12(&self, encoder: &mut wgpu::CommandEncoder) {
        let cs_code = match self.video_config.color_space {
            ColorSpace::Rec709 => 0u32,
            ColorSpace::Rec601 => 1u32,
            ColorSpace::Rec2100Pq | ColorSpace::Rec2100Hlg => 2u32,
        };
        let nu = Nv12Uniforms {
            width: self.output_rt.width,
            height: self.output_rt.height,
            color_space: cs_code,
            full_range: u32::from(self.video_config.color_range == ColorRange::Full),
        };
        let ubuf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("NV12 Uniform"),
            size: std::mem::size_of::<Nv12Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&ubuf, 0, bytemuck::bytes_of(&nu));

        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("NV12 BG"),
            layout: &self.nv12_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.output_rt.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.nv12_gpu_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: ubuf.as_entire_binding(),
                },
            ],
        });

        let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("RGBA to NV12 Compute"),
            timestamp_writes: None,
        });
        cpass.set_pipeline(&self.nv12_pipeline);
        cpass.set_bind_group(0, &bg, &[]);
        let gx = (self.output_rt.width / 4).div_ceil(8);
        let gy = self.output_rt.height.div_ceil(8);
        cpass.dispatch_workgroups(gx.max(1), gy.max(1), 1);
    }

    fn load_cached_image_view(
        &self,
        path: &str,
        cache: &Mutex<HashMap<String, RenderTarget>>,
    ) -> Option<wgpu::TextureView> {
        let mut map = cache.lock();
        if let Some(rt) = map.get(path) {
            return Some(rt.tex.create_view(&wgpu::TextureViewDescriptor::default()));
        }
        let img = image::open(Path::new(path)).ok()?.to_rgba8();
        let (w, h) = img.dimensions();
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Cached Aux Image"),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &img.into_raw(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
        );
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let rt_view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        map.insert(
            path.to_string(),
            RenderTarget {
                tex,
                view,
                width: w,
                height: h,
            },
        );
        Some(rt_view)
    }
}

fn compute_bounded_size(
    cw: f32,
    ch: f32,
    scale: [f32; 2],
    bounds_type: BoundsType,
    bounds: [f32; 2],
) -> (f32, f32) {
    let bw = bounds[0].max(1.0);
    let bh = bounds[1].max(1.0);
    match bounds_type {
        BoundsType::None => (cw * scale[0], ch * scale[1]),
        BoundsType::Stretch => (bw, bh),
        BoundsType::ScaleInner => {
            let r = (bw / cw).min(bh / ch);
            (cw * r, ch * r)
        }
        BoundsType::ScaleOuter => {
            let r = (bw / cw).max(bh / ch);
            (cw * r, ch * r)
        }
        BoundsType::ScaleToWidth => {
            let r = bw / cw;
            (bw, ch * r)
        }
        BoundsType::ScaleToHeight => {
            let r = bh / ch;
            (cw * r, bh)
        }
        BoundsType::MaxOnly => {
            let r = (bw / cw).min(bh / ch).min(1.0);
            (cw * r, ch * r)
        }
    }
}

fn dir_to_u32(dir: TransitionDirection) -> u32 {
    match dir {
        TransitionDirection::Left => 0,
        TransitionDirection::Right => 1,
        TransitionDirection::Up => 2,
        TransitionDirection::Down => 3,
    }
}

fn blend_mode_key(mode: BlendMode) -> u8 {
    match mode {
        BlendMode::Normal => 0,
        BlendMode::Additive => 1,
        BlendMode::Subtract => 2,
        BlendMode::Screen => 3,
        BlendMode::Multiply => 4,
        BlendMode::Lighten => 5,
        BlendMode::Darken => 6,
    }
}

fn blend_mode_to_wgpu(mode: BlendMode) -> wgpu::BlendState {
    match mode {
        BlendMode::Normal => wgpu::BlendState::ALPHA_BLENDING,
        BlendMode::Additive => wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        },
        BlendMode::Subtract => wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::ReverseSubtract,
            },
            alpha: wgpu::BlendComponent::OVER,
        },
        BlendMode::Screen => wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrc,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        },
        BlendMode::Multiply => wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Dst,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        },
        BlendMode::Lighten => wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Max,
            },
            alpha: wgpu::BlendComponent::OVER,
        },
        BlendMode::Darken => wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Min,
            },
            alpha: wgpu::BlendComponent::OVER,
        },
    }
}

fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn samp_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn create_solid_rt(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    rgba: [u8; 4],
    label: &str,
) -> RenderTarget {
    let rt = RenderTarget::new(device, 4, 4, label);
    let data: Vec<u8> = rgba.iter().copied().cycle().take(4 * 4 * 4).collect();
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &rt.tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(16),
            rows_per_image: Some(4),
        },
        wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
    );
    rt
}

fn create_gradient_luma_rt(device: &wgpu::Device, queue: &wgpu::Queue) -> RenderTarget {
    let rt = RenderTarget::new(device, 256, 4, "Fallback Luma Wipe");
    let mut data = Vec::with_capacity(256 * 4 * 4);
    for _y in 0..4 {
        for x in 0..256 {
            let v = x as u8;
            data.extend_from_slice(&[v, v, v, 255]);
        }
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &rt.tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(256 * 4),
            rows_per_image: Some(4),
        },
        wgpu::Extent3d {
            width: 256,
            height: 4,
            depth_or_array_layers: 1,
        },
    );
    rt
}

fn discover_luma_dir() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("assets").join("luma_wipes");
            if p.exists() {
                return p;
            }
        }
    }
    std::path::PathBuf::from("assets").join("luma_wipes")
}
