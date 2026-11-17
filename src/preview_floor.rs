//! The stage floor under grounded moves in the 3D preview.
//!
//! In game the fighter stands on a stage whose surface is at height 0, and the depth test hides
//! whatever part of an effect goes below it (half of a dust cloud, the lower half of a ground
//! shockwave). The preview draws an opaque plane there, into the scene before the effects, so
//! they are cut off the same way. A move in the air has no floor under it.

/// Whether a move (by its motion name, e.g. `attack_air_f`) is done standing on the ground.
pub fn move_is_grounded(motion: &str) -> bool {
    let motion = motion.to_ascii_lowercase();
    // Landings are on the ground even though their names carry the aerial they land from.
    if motion.starts_with("landing") {
        return true;
    }
    const AIRBORNE: [&str; 7] = ["air", "jump", "fall", "fly", "cliff", "glide", "swim"];
    !AIRBORNE.iter().any(|word| motion.contains(word))
}

const SHADER: &str = "
struct Floor { view_proj: mat4x4<f32>, color: vec4<f32>, half_size: f32 };
@group(0) @binding(0) var<uniform> floor: Floor;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // Two triangles covering the square, on the plane y = 0.
    var corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0));
    let c = corners[i] * floor.half_size;
    return floor.view_proj * vec4<f32>(c.x, 0.0, c.y, 1.0);
}
@fragment fn fs() -> @location(0) vec4<f32> { return floor.color; }
";

/// Far enough that the floor's edge never shows in the preview.
const HALF_SIZE: f32 = 5000.0;

pub struct PreviewFloor {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind: wgpu::BindGroup,
}

impl PreviewFloor {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("preview floor"),
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
            label: Some("preview floor"),
            size: 96,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("preview floor"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("preview floor"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("preview floor"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("preview floor"),
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
                depth_write_enabled: Some(true),
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
                    blend: None,
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

    /// Place the floor for this frame's camera. `color` is the floor as stored in the scene
    /// (before post processing), alpha 1.
    pub fn update(&self, queue: &wgpu::Queue, view_proj: glam::Mat4, color: [f32; 4]) {
        let mut bytes = Vec::with_capacity(96);
        bytes.extend(
            view_proj
                .to_cols_array()
                .iter()
                .flat_map(|v| v.to_le_bytes()),
        );
        bytes.extend(color.iter().flat_map(|v| v.to_le_bytes()));
        bytes.extend(HALF_SIZE.to_le_bytes());
        bytes.resize(96, 0);
        queue.write_buffer(&self.uniforms, 0, &bytes);
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.draw(0..6, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::move_is_grounded;

    #[test]
    fn ground_moves_get_a_floor_and_air_moves_do_not() {
        for grounded in [
            "attack_11",
            "attack_s4_s",
            "special_n",
            "landing_air_f",
            "landing_heavy",
            "throw_f",
            "appeal_hi_r",
        ] {
            assert!(move_is_grounded(grounded), "{grounded}");
        }
        for airborne in [
            "attack_air_f",
            "special_air_hi",
            "jump_f",
            "fall_special",
            "damage_fly_hi",
            "cliff_attack",
        ] {
            assert!(!move_is_grounded(airborne), "{airborne}");
        }
    }
}
