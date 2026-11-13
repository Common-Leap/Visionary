//! The 3D viewport's own colour target.
//!
//! The model renderer composites into a pass of the window's format, which is not sRGB, so
//! anything drawn after it in that pass blends on screen-encoded values. The game draws its
//! effects into a linear buffer and encodes only when presenting, so effect blending there
//! happens on linear values. To match, the viewport draws into this texture: the models through
//! a view of the window's format (as before), the effects through an sRGB view of the same
//! memory (the GPU decodes, blends and re-encodes per pixel), and the result is copied into the
//! window's pass.

pub struct ViewportTarget {
    format: wgpu::TextureFormat,
    size: (u32, u32),
    texture: Option<wgpu::Texture>,
    /// The window's format: what the model composite writes and what the copy reads.
    pub plain: Option<wgpu::TextureView>,
    /// The same memory as sRGB: what effects draw into.
    pub srgb: Option<wgpu::TextureView>,
    blit: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    bind: Option<wgpu::BindGroup>,
}

const BLIT: &str = "
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
struct Out { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> Out {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: Out;
    o.pos = vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(xy.x, 1.0 - xy.y);
    return o;
}
@fragment fn fs(o: Out) -> @location(0) vec4<f32> { return textureSample(tex, smp, o.uv); }
";

impl ViewportTarget {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport copy"),
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
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viewport copy"),
            source: wgpu::ShaderSource::Wgsl(BLIT.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viewport copy"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let blit = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport copy"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            format,
            size: (0, 0),
            texture: None,
            plain: None,
            srgb: None,
            blit,
            layout,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor::default()),
            bind: None,
        }
    }

    /// The format effects draw in (the sRGB form of the window's format).
    pub fn effect_format(&self) -> wgpu::TextureFormat {
        self.format.add_srgb_suffix()
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self.size == (width, height) && self.texture.is_some() {
            return;
        }
        let srgb = self.effect_format();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewport"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[srgb],
        });
        let plain = texture.create_view(&Default::default());
        let srgb_view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(srgb),
            ..Default::default()
        });
        self.bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport copy"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&plain),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        }));
        self.texture = Some(texture);
        self.plain = Some(plain);
        self.srgb = Some(srgb_view);
        self.size = (width, height);
    }

    /// Copy the viewport into the window's pass (its viewport is already the panel's rect).
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some(bind) = &self.bind {
            pass.set_pipeline(&self.blit);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
