//! Weapon swing trails (`AFTER_IMAGE4_ON_arg29` and friends) in the 3D preview.
//!
//! A trail is not an effect file entry: the script names two textures and two points, each on
//! a bone, and the game sweeps a ribbon between those points over the last few frames. Link's
//! sword swings are drawn this way:
//!
//! `AFTER_IMAGE4_ON_arg29(tex_link_sword1, tex_link_sword2, 4, sword1, (1, 0, 0), sword1,
//! (14.6, 0.2, -0.2), ...)`
//!
//! The first texture is the slash's colour and shape, laid out with time across (newest at the
//! right) and the blade down (its far point at the top); the second is a grey ramp that fades it
//! with age. The trail system is the game's own, not nn::vfx, so this follows the arguments and
//! the textures; its exact falloff has not been measured against the game.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glam::Vec3;

/// What a trail call asks for.
#[derive(Debug, Clone, PartialEq)]
pub struct TrailSpec {
    pub texture: String,
    pub fade: String,
    /// How many frames of motion the ribbon covers.
    pub frames: f32,
    pub bone_a: String,
    pub point_a: Vec3,
    pub bone_b: String,
    pub point_b: Vec3,
}

/// Read a trail from its source line; `None` for lines in another shape.
pub fn parse(line: &str) -> Option<TrailSpec> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    let args: Vec<&str> = line[open + 1..close].split(',').map(str::trim).collect();
    let hash = |s: &str| {
        s.strip_prefix("Hash40::new(\"")
            .and_then(|r| r.strip_suffix("\")"))
            .map(str::to_owned)
    };
    let num = |s: &str| s.trim_end_matches("f32").parse::<f32>().ok();
    // agent, tex1, tex2, frames, bone, x, y, z, bone, x, y, z, ...
    if args.len() < 12 {
        return None;
    }
    Some(TrailSpec {
        texture: hash(args[1])?,
        fade: hash(args[2])?,
        frames: num(args[3])?,
        bone_a: hash(args[4])?,
        point_a: Vec3::new(num(args[5])?, num(args[6])?, num(args[7])?),
        bone_b: hash(args[8])?,
        point_b: Vec3::new(num(args[9])?, num(args[10])?, num(args[11])?),
    })
}

/// One trail to draw this frame: the textures' files and the blade's two ends at successive
/// moments, oldest first.
#[derive(Debug, Clone)]
pub struct TrailDraw {
    pub texture: PathBuf,
    pub fade: PathBuf,
    pub edges: Vec<[Vec3; 2]>,
}

/// Extract the costume slot number from a slot-scoped `.eff` path such as
/// `ef_pacman_c40.eff` → `Some(40)`.
pub fn slot_from_eff_path(path: &Path) -> Option<u8> {
    let stem = path.file_stem()?.to_str()?;
    let tail = stem.rsplit("_c").next()?;
    if tail.len() >= 2 && tail.len() <= 3 && tail.bytes().all(|b| b.is_ascii_digit()) {
        tail.parse::<u8>().ok()
    } else {
        None
    }
}

/// The texture file for a trail texture name: the fighter's slot-scoped `trail_cNN` folder
/// (for costume-slot movesets such as Shy Guy on Pac-Man `c40`), then the fighter's own `trail`
/// folder across `effect_dirs`, then the shared `effect/fighter/trail` folder.
pub fn texture_path_for_slot(
    effect_dirs: &[PathBuf],
    slot: Option<u8>,
    data_root: Option<&Path>,
    name: &str,
) -> Option<PathBuf> {
    let file = format!("{name}.nutexb");
    if let Some(slot) = slot {
        let slot_dir = format!("trail_c{slot:02}");
        for dir in effect_dirs {
            let candidate = dir.join(&slot_dir).join(&file);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    for dir in effect_dirs {
        let own = dir.join("trail").join(&file);
        if own.exists() {
            return Some(own);
        }
    }
    let shared = data_root?.join("effect/fighter/trail").join(&file);
    shared.exists().then_some(shared)
}

const SHADER: &str = "
struct Frame { view_proj: mat4x4<f32> };
@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var colour: texture_2d<f32>;
@group(1) @binding(1) var fade: texture_2d<f32>;
@group(1) @binding(2) var smp: sampler;
struct In { @location(0) pos: vec3<f32>, @location(1) uv: vec2<f32> };
struct Out { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(v: In) -> Out {
    var o: Out;
    o.pos = frame.view_proj * vec4<f32>(v.pos, 1.0);
    o.uv = v.uv;
    return o;
}
@fragment fn fs(o: Out) -> @location(0) vec4<f32> {
    let c = textureSample(colour, smp, o.uv);
    let f = textureSample(fade, smp, o.uv).r;
    // The sheet is the first texture: dark body, silver edge, white leading streak, as the
    // game draws it. The second is a grey mask that only fades it; adding its grey as light
    // turned Sephiroth's sheet flat white.
    // The scene holds half intensity; the post pass doubles it (as for effects).
    return vec4<f32>(c.rgb * 0.5, c.a * f);
}
";

struct Prepared {
    vertices: wgpu::Buffer,
    count: u32,
    textures: wgpu::BindGroup,
}

pub struct PreviewTrails {
    pipeline: wgpu::RenderPipeline,
    frame: wgpu::Buffer,
    frame_bind: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Decoded textures by file, `None` when a file would not decode.
    textures: HashMap<PathBuf, Option<wgpu::TextureView>>,
    prepared: Vec<Prepared>,
}

impl PreviewTrails {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("preview trail frame"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("preview trail textures"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let frame = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("preview trail frame"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("preview trail frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame.as_entire_binding(),
            }],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("preview trail"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("preview trail"),
            bind_group_layouts: &[Some(&frame_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("preview trail"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 20,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
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
                    // The trail calls ask for TRAIL_BLEND_ALPHA.
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            frame,
            frame_bind,
            texture_layout,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            textures: HashMap::new(),
            prepared: Vec::new(),
        }
    }

    fn texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: &Path,
    ) -> Option<wgpu::TextureView> {
        self.textures
            .entry(path.to_path_buf())
            .or_insert_with(|| load_texture(device, queue, path))
            .clone()
    }

    /// Build this frame's ribbons. `view_proj` is the preview camera's.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view_proj: glam::Mat4,
        trails: &[TrailDraw],
    ) {
        use wgpu::util::DeviceExt;
        self.prepared.clear();
        let matrix: Vec<u8> = view_proj
            .to_cols_array()
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        queue.write_buffer(&self.frame, 0, &matrix);
        for trail in trails {
            if trail.edges.len() < 2 {
                continue;
            }
            let (Some(colour), Some(fade)) = (
                self.texture(device, queue, &trail.texture),
                self.texture(device, queue, &trail.fade),
            ) else {
                continue;
            };
            let edges = smooth(&trail.edges, 4);
            let mut vertices: Vec<f32> = Vec::new();
            let last = (edges.len() - 1) as f32;
            let vertex = |out: &mut Vec<f32>, p: Vec3, u: f32, v: f32| {
                out.extend([p.x, p.y, p.z, u, v]);
            };
            for (i, pair) in edges.windows(2).enumerate() {
                // Time runs across the textures, newest at the right; the far point is the
                // top edge.
                let (u0, u1) = (i as f32 / last, (i + 1) as f32 / last);
                let ([a0, b0], [a1, b1]) = (pair[0], pair[1]);
                for (p, u, v) in [
                    (b0, u0, 0.0),
                    (a0, u0, 1.0),
                    (a1, u1, 1.0),
                    (b0, u0, 0.0),
                    (a1, u1, 1.0),
                    (b1, u1, 0.0),
                ] {
                    vertex(&mut vertices, p, u, v);
                }
            }
            let bytes: Vec<u8> = vertices.iter().flat_map(|f| f.to_le_bytes()).collect();
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("preview trail"),
                contents: &bytes,
                usage: wgpu::BufferUsages::VERTEX,
            });
            let textures = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("preview trail textures"),
                layout: &self.texture_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&colour),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&fade),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            self.prepared.push(Prepared {
                vertices: buffer,
                count: (vertices.len() / 5) as u32,
                textures,
            });
        }
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.prepared.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.frame_bind, &[]);
        for trail in &self.prepared {
            pass.set_bind_group(1, &trail.textures, &[]);
            pass.set_vertex_buffer(0, trail.vertices.slice(..));
            pass.draw(0..trail.count, 0..1);
        }
    }
}

/// The blade's ends at `per` points between each pair of samples along a Catmull-Rom curve
/// through them, so the ribbon bends smoothly instead of turning at every sample.
fn smooth(edges: &[[Vec3; 2]], per: usize) -> Vec<[Vec3; 2]> {
    if edges.len() < 3 || per < 2 {
        return edges.to_vec();
    }
    let at = |i: isize| edges[i.clamp(0, edges.len() as isize - 1) as usize];
    let curve = |p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f32| {
        let (t2, t3) = (t * t, t * t * t);
        0.5 * ((2.0 * p1)
            + (p2 - p0) * t
            + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
            + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3)
    };
    let mut out = Vec::with_capacity((edges.len() - 1) * per + 1);
    for i in 0..edges.len() as isize - 1 {
        let (a, b, c, d) = (at(i - 1), at(i), at(i + 1), at(i + 2));
        for k in 0..per {
            let t = k as f32 / per as f32;
            out.push([
                curve(a[0], b[0], c[0], d[0], t),
                curve(a[1], b[1], c[1], d[1], t),
            ]);
        }
    }
    out.push(*edges.last().unwrap());
    out
}

fn load_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    path: &Path,
) -> Option<wgpu::TextureView> {
    use nutexb::NutexbFormat as F;
    let nutexb = nutexb::NutexbFile::read_from_file(path).ok()?;
    // Every fighter trail texture is stored as sRGB, the fading mask too; follow the file.
    let srgb = matches!(
        nutexb.footer.image_format,
        F::R8G8B8A8Srgb | F::B8G8R8A8Srgb | F::BC1Srgb | F::BC2Srgb | F::BC3Srgb | F::BC7Srgb
    );
    // nutexb and image_dds use different ddsfile releases; the file between them is the same.
    let mut dds = Vec::new();
    nutexb.to_dds().ok()?.write(&mut dds).ok()?;
    let dds = image_dds::ddsfile::Dds::read(dds.as_slice()).ok()?;
    let image = image_dds::image_from_dds(&dds, 0).ok()?;
    Some(upload(
        device,
        queue,
        (image.width(), image.height()),
        srgb,
        image.as_raw(),
    ))
}

fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    (width, height): (u32, u32),
    srgb: bool,
    rgba: &[u8],
) -> wgpu::TextureView {
    use wgpu::util::DeviceExt;
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("preview trail texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: if srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        rgba,
    );
    texture.create_view(&Default::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_s_sword_trail_reads_its_textures_and_blade() {
        let line = r#"macros::AFTER_IMAGE4_ON_arg29(agent, Hash40::new("tex_link_sword1"), Hash40::new("tex_link_sword2"), 4, Hash40::new("sword1"), 1, 0, 0, Hash40::new("sword1"), 14.6, 0.2, -0.2, true, Hash40::new("null"), Hash40::new("sword1"), 0, 0, 0, 0, 0, 0, 1, 0, *EFFECT_AXIS_X, 0, *TRAIL_BLEND_ALPHA, 101, *TRAIL_CULL_NONE, 1.3, 0.2);"#;
        let spec = parse(line).unwrap();
        assert_eq!(spec.texture, "tex_link_sword1");
        assert_eq!(spec.fade, "tex_link_sword2");
        assert_eq!(spec.frames, 4.0);
        assert_eq!(spec.bone_a, "sword1");
        assert_eq!(spec.point_a, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(spec.point_b, Vec3::new(14.6, 0.2, -0.2));
    }

    #[test]
    fn slot_from_eff_path_reads_costume_suffix() {
        assert_eq!(slot_from_eff_path(Path::new("ef_pacman_c40.eff")), Some(40));
        assert_eq!(slot_from_eff_path(Path::new("/a/b/ef_mario_c08.eff")), Some(8));
        assert_eq!(slot_from_eff_path(Path::new("ef_pacman.eff")), None);
        assert_eq!(slot_from_eff_path(Path::new("ef_common.eff")), None);
    }

    #[test]
    fn texture_path_for_slot_prefers_trail_cnn_then_trail_then_shared() {
        let tmp = tempfile::tempdir().unwrap();
        let mod_eff = tmp.path().join("mod/effect/fighter/pacman");
        let data_root = tmp.path().join("data");
        std::fs::create_dir_all(mod_eff.join("trail_c40")).unwrap();
        std::fs::create_dir_all(mod_eff.join("trail")).unwrap();
        std::fs::create_dir_all(data_root.join("effect/fighter/trail")).unwrap();
        let slot_tex = mod_eff.join("trail_c40/tex_shyguy_stilt1.nutexb");
        let base_tex = mod_eff.join("trail/tex_base.nutexb");
        let shared_tex = data_root.join("effect/fighter/trail/tex_shared.nutexb");
        std::fs::write(&slot_tex, b"").unwrap();
        std::fs::write(&base_tex, b"").unwrap();
        std::fs::write(&shared_tex, b"").unwrap();

        let dirs = vec![mod_eff.clone()];
        assert_eq!(
            texture_path_for_slot(&dirs, Some(40), Some(&data_root), "tex_shyguy_stilt1"),
            Some(slot_tex)
        );
        assert_eq!(
            texture_path_for_slot(&dirs, Some(0), Some(&data_root), "tex_shyguy_stilt1"),
            None
        );
        assert_eq!(
            texture_path_for_slot(&dirs, Some(40), Some(&data_root), "tex_base"),
            Some(base_tex)
        );
        assert_eq!(
            texture_path_for_slot(&dirs, Some(40), Some(&data_root), "tex_shared"),
            Some(shared_tex)
        );
    }

    #[test]
    fn a_smoothed_ribbon_passes_through_its_samples_and_bends_between_them() {
        // A blade end sweeping a quarter circle, sampled at 0, 45 and 90 degrees.
        let p = |deg: f32| Vec3::new(deg.to_radians().cos(), deg.to_radians().sin(), 0.0) * 10.0;
        let edges = [
            [Vec3::ZERO, p(0.0)],
            [Vec3::ZERO, p(45.0)],
            [Vec3::ZERO, p(90.0)],
        ];
        let out = smooth(&edges, 4);
        assert_eq!(out.len(), 2 * 4 + 1);
        for (k, edge) in edges.iter().enumerate() {
            assert!((out[k * 4][1] - edge[1]).length() < 1e-4);
        }
        // Between samples the end stays out near the circle, where a straight panel cuts in.
        let mid = out[2][1].length();
        let chord = ((p(0.0) + p(45.0)) * 0.5).length();
        assert!(mid > chord + 0.1, "{mid} vs chord {chord}");
    }

    /// The device a GPU test draws on, `None` on a machine with no adapter. wgpu's native
    /// requests are ready on their first poll.
    fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
        fn ready<T>(f: impl std::future::Future<Output = T>) -> T {
            let mut f = std::pin::pin!(f);
            let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
            loop {
                if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                    return v;
                }
                std::thread::yield_now();
            }
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = ready(instance.request_adapter(&Default::default())).ok()?;
        ready(adapter.request_device(&Default::default())).ok()
    }

    /// One pixel of a ribbon that fills the view, drawn over clear black with a one-texel
    /// sRGB colour texture and fade mask, as the game's trail textures are stored.
    fn draw_texel(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        colour: [u8; 4],
        fade: u8,
    ) -> [u8; 4] {
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let depth_format = wgpu::TextureFormat::Depth32Float;
        let mut trails = PreviewTrails::new(device, format, depth_format, 1);
        let (colour_path, fade_path) = (PathBuf::from("colour"), PathBuf::from("fade"));
        let one = |rgba: [u8; 4]| Some(upload(device, queue, (1, 1), true, &rgba));
        trails.textures.insert(colour_path.clone(), one(colour));
        trails
            .textures
            .insert(fade_path.clone(), one([fade, fade, fade, 255]));
        let edge = |x: f32| [Vec3::new(x, -1.0, 0.5), Vec3::new(x, 1.0, 0.5)];
        trails.prepare(
            device,
            queue,
            glam::Mat4::IDENTITY,
            &[TrailDraw {
                texture: colour_path,
                fade: fade_path,
                edges: vec![edge(-1.0), edge(1.0)],
            }],
        );
        let size = wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        };
        let target = |format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let attach = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let colour_target = target(format, attach | wgpu::TextureUsages::COPY_SRC);
        let depth_target = target(depth_format, attach);
        let (colour_view, depth_view) = (
            colour_target.create_view(&Default::default()),
            depth_target.create_view(&Default::default()),
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &colour_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            trails.draw(&mut pass);
        }
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256 * 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            colour_target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: None,
                },
            },
            size,
        );
        queue.submit([encoder.finish()]);
        readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let bytes = readback
            .slice(..)
            .get_mapped_range()
            .expect("mapped")
            .to_vec();
        // A pixel away from the ribbon's diagonal seam.
        let at = 256 + 4;
        [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]
    }

    #[test]
    fn a_trail_draws_its_first_texture_s_colour_faded_by_the_second() {
        let Some((device, queue)) = gpu() else {
            eprintln!("skipped: no GPU adapter");
            return;
        };
        let linear = |v: u8| {
            let c = v as f32 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        // What the scene holds over black: the first texture's colour at half intensity (the
        // post pass doubles it), covering by its alpha times the second texture's grey.
        let expected = |colour: [u8; 4], fade: u8| {
            let cover = colour[3] as f32 / 255.0 * linear(fade);
            let channel = |v: u8| (linear(v) * 0.5 * cover * 255.0).round() as i32;
            [
                channel(colour[0]),
                channel(colour[1]),
                channel(colour[2]),
                (cover * 255.0).round() as i32,
            ]
        };
        // Texels of tex_edge_sword1 (Sephiroth) with tex_edge_sword2 at the same place: the dark
        // body of the sheet at (0.7, 0.3), the white leading streak at (0.97, 0.3), and the
        // body again under the mask's half-faded grey from (0.4, 0.5).
        for (colour, fade) in [
            ([57, 64, 85, 229], 255),
            ([213, 215, 227, 251], 255),
            ([57, 64, 85, 229], 144),
        ] {
            let drawn = draw_texel(&device, &queue, colour, fade).map(i32::from);
            let want = expected(colour, fade);
            assert!(
                drawn.iter().zip(want).all(|(d, w)| (d - w).abs() <= 2),
                "colour {colour:?} fade {fade}: drew {drawn:?}, want {want:?}"
            );
        }
    }

    #[test]
    fn a_trail_texture_decodes() {
        let Some(root) = std::env::var_os("VISIONARY_DATA_ROOT") else {
            return;
        };
        let path = Path::new(&root).join("effect/fighter/link/trail/tex_link_sword1.nutexb");
        let nutexb = nutexb::NutexbFile::read_from_file(&path).unwrap();
        let mut dds = Vec::new();
        nutexb.to_dds().unwrap().write(&mut dds).unwrap();
        let dds = image_dds::ddsfile::Dds::read(dds.as_slice()).unwrap();
        let image = image_dds::image_from_dds(&dds, 0).unwrap();
        assert_eq!((image.width(), image.height()), (256, 256));
    }
}
