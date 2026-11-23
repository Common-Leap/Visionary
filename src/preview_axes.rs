//! Lines on the ground in the 3D preview: a grid, and a red X line and a blue Z line through
//! the origin, as SSBH Studio draws them.
//!
//! Each pixel's view ray is intersected with the plane y = 0, so the lines reach the horizon
//! and stay about 1.5 pixels wide at any zoom. They are drawn into the scene before the effects
//! and depth tested against it, so the fighter and the stage hide them as solid things would
//! and effects show over them. (ssbh_wgpu's own floor grid is drawn after the effects, which
//! write no depth, and covered them.)

const SHADER: &str = "
struct Axes { mvp: mat4x4<f32>, inverse: mat4x4<f32>, eye: vec4<f32>, lines: vec4<f32> };
@group(0) @binding(0) var<uniform> axes: Axes;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) near: vec3<f32>,
    @location(1) far: vec3<f32>,
};

fn unproject(x: f32, y: f32, z: f32) -> vec3<f32> {
    let p = axes.inverse * vec4(x, y, z, 1.0);
    return p.xyz / p.w;
}

@vertex fn vs(@builtin(vertex_index) i: u32) -> VertexOut {
    // One triangle covering the screen.
    var corners = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    let c = corners[i];
    var out: VertexOut;
    out.position = vec4(c, 0.0, 1.0);
    out.near = unproject(c.x, c.y, 0.0);
    out.far = unproject(c.x, c.y, 1.0);
    return out;
}

struct FragmentOut {
    @builtin(frag_depth) depth: f32,
    @location(0) color: vec4<f32>,
};

@fragment fn fs(in: VertexOut) -> FragmentOut {
    let t = -in.near.y / (in.far.y - in.near.y);
    let position = in.near + t * (in.far - in.near);
    // Derivatives before any discard, so every pixel of a quad takes them.
    let width = fwidth(position.xz) * 1.5;
    // A line every ten units.
    let cell = abs(fract(position.xz / 10.0 - 0.5) - 0.5) * 10.0;
    if (t <= 0.0) {
        discard;
    }
    // Fade with distance relative to the camera's height, so the lines reach farther when
    // looking down and fade sooner at grazing angles.
    let height = max(abs(axes.eye.y), 20.0);
    let distance = length(position - axes.eye.xyz);
    let fade = (1.0 - smoothstep(height * 8.0, height * 40.0, distance)) * (1.0 - abs(t));
    let on_x = (1.0 - min(abs(position.z) / max(width.y, 1e-6), 1.0)) * axes.lines.y;
    let on_z = (1.0 - min(abs(position.x) / max(width.x, 1e-6), 1.0)) * axes.lines.y;
    let on_grid = 1.0 - min(min(cell.x / max(width.x, 1e-6), cell.y / max(width.y, 1e-6)), 1.0);
    var color = vec3(1.0);
    var alpha = on_grid * 0.5 * fade * axes.lines.x;
    if (on_x > 0.0) {
        color = vec3(0.91, 0.30, 0.30);
        alpha = on_x * 0.9 * fade;
    }
    if (on_z > 0.0 && on_z * 0.9 * fade > alpha) {
        color = vec3(0.27, 0.51, 1.0);
        alpha = on_z * 0.9 * fade;
    }
    if (alpha <= 0.001) {
        discard;
    }
    // A hair toward the camera, so the opaque floor at the same height doesn't hide them.
    let lifted = axes.eye.xyz + (position - axes.eye.xyz) * 0.999;
    let clip = axes.mvp * vec4(lifted, 1.0);
    var out: FragmentOut;
    out.depth = clamp(clip.z / clip.w, 0.0, 1.0);
    // Premultiplied alpha.
    out.color = vec4(color * alpha, alpha);
    return out;
}
";

/// Bytes in the uniform block: two matrices, the eye position and which lines show.
const UNIFORM_SIZE: u64 = 64 + 64 + 16 + 16;

/// Which lines show.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Lines {
    pub grid: bool,
    pub axes: bool,
}

impl Lines {
    pub fn any(self) -> bool {
        self.grid || self.axes
    }
}

pub struct PreviewAxes {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind: wgpu::BindGroup,
}

impl PreviewAxes {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("preview axes"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("preview axes"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("preview axes"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("preview axes"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("preview axes"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("preview axes"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            uniforms,
            bind,
        }
    }

    /// Place the lines for this frame's camera.
    pub fn update(&self, queue: &wgpu::Queue, mvp: glam::Mat4, eye: glam::Vec4, lines: Lines) {
        let mut bytes = Vec::with_capacity(UNIFORM_SIZE as usize);
        let shown = [lines.grid, lines.axes, false, false].map(|on| if on { 1.0f32 } else { 0.0 });
        for value in mvp
            .to_cols_array()
            .iter()
            .chain(mvp.inverse().to_cols_array().iter())
            .chain(eye.to_array().iter())
            .chain(shown.iter())
        {
            bytes.extend(value.to_le_bytes());
        }
        queue.write_buffer(&self.uniforms, 0, &bytes);
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_shader_is_valid_wgsl() {
        let module = naga::front::wgsl::parse_str(super::SHADER).expect("parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("validates");
    }
}
