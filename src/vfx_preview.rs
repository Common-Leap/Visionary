//! Effects in the 3D preview, drawn by `vfx_runtime` with the game's own shaders.
//!
//! The app hands over which effect sets are out this frame (file, set name, placement and
//! frame since spawn); this module keeps one renderer per effect file with its textures on the
//! GPU, runs each set's emitters to that frame and prepares their draws.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glam::Mat4;
use vfx_runtime::{bnsh, ptcl, render, sim, static_block, view_block};

/// One effect set out this frame.
#[derive(Clone, Debug)]
pub struct EffectDraw {
    pub file: PathBuf,
    /// Entry name as the game spawns it (case-insensitive).
    pub name: String,
    /// Effect space to world: bone, offset, rotation and scale.
    pub placement: Mat4,
    /// Frames since the effect was spawned.
    pub frame: u32,
}

struct Emitter {
    data: effect_library::emitter::EmitterData,
    raw: ptcl::Emitter,
    fixed: Vec<u8>,
}

struct EffectFile {
    renderer: render::EffectRenderer,
    /// Entry name (lower case) to emitter set index.
    sets: HashMap<String, usize>,
    emitters: Vec<Vec<Emitter>>,
    texture_ids: Vec<u64>,
    texture_names: Vec<String>,
    pool: Option<Vec<u8>>,
    textures: HashMap<usize, Option<wgpu::TextureView>>,
    samplers: HashMap<(u8, u8, u8), wgpu::Sampler>,
}

pub struct VfxPreview {
    target: wgpu::TextureFormat,
    depth: Option<wgpu::TextureFormat>,
    files: HashMap<PathBuf, Option<EffectFile>>,
    prepared: Vec<(PathBuf, render::PreparedDraw)>,
}

impl VfxPreview {
    pub fn new(target: wgpu::TextureFormat, depth: Option<wgpu::TextureFormat>) -> Self {
        Self {
            target,
            depth,
            files: HashMap::new(),
            prepared: Vec::new(),
        }
    }

    fn load(&self, device: &wgpu::Device, queue: &wgpu::Queue, path: &Path) -> Option<EffectFile> {
        let bytes = std::fs::read(path).ok()?;
        let file = effect_library::NamcoEffectFile::load(&bytes).ok()?;
        let ptcl_file = file.ptcl_file.as_ref()?;
        let shaders =
            bnsh::ShaderFile::read(ptcl_file.shader_info.as_ref()?.binary_data.as_ref()?).ok()?;
        let raw_sets = ptcl::emitter_sets(&ptcl_file.base_bytes);
        let mut emitters = Vec::new();
        for (index, set) in ptcl_file.emitter_list.emitter_sets.iter().enumerate() {
            let raws = raw_sets
                .get(index)
                .map(|s| s.emitters.clone())
                .unwrap_or_default();
            emitters.push(
                set.emitters
                    .iter()
                    .zip(raws)
                    .map(|(parsed, raw)| Emitter {
                        fixed: static_block::fixed_emitter_data(&raw, &|_| None, None),
                        data: parsed.data.clone(),
                        raw,
                    })
                    .collect(),
            );
        }
        let sets = crate::effects::load_effect(path)
            .ok()?
            .index
            .handles
            .into_iter()
            .filter_map(|(name, set)| Some((name.to_lowercase(), usize::try_from(set).ok()?)))
            .collect();
        let descriptors = ptcl_file
            .texture_info
            .as_ref()
            .map(|info| info.descriptors.clone())
            .unwrap_or_default();
        Some(EffectFile {
            renderer: render::EffectRenderer::new(device, queue, &shaders, self.target, self.depth),
            sets,
            emitters,
            texture_ids: descriptors.iter().map(|d| d.id).collect(),
            texture_names: descriptors.iter().map(|d| d.name.clone()).collect(),
            pool: ptcl_file
                .texture_info
                .as_ref()
                .and_then(|info| info.binary_data.clone()),
            textures: HashMap::new(),
            samplers: HashMap::new(),
        })
    }

    /// Simulate and upload every effect for this frame. `view` and `projection` are the
    /// preview camera's (world to camera, camera to clip).
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: Mat4,
        projection: Mat4,
        eye: [f32; 3],
        near: f32,
        far: f32,
        draws: &[EffectDraw],
    ) {
        self.prepared.clear();
        let rows = |m: Mat4| m.transpose().to_cols_array_2d();
        let camera = view_block::view_block(&rows(view), &rows(projection), eye, near, far, 1.0);
        let custom2: Vec<u8> = [1.0f32; 20].iter().flat_map(|v| v.to_le_bytes()).collect();
        for draw in draws {
            if !self.files.contains_key(&draw.file) {
                let loaded = self.load(device, queue, &draw.file);
                self.files.insert(draw.file.clone(), loaded);
            }
            let Some(Some(file)) = self.files.get_mut(&draw.file) else {
                continue;
            };
            let Some(&set) = file.sets.get(&draw.name.to_lowercase()) else {
                continue;
            };
            file.renderer.set_view(queue, &camera);
            let (scale, rotation, translation) = draw.placement.to_scale_rotation_translation();
            let rt = Mat4::from_rotation_translation(rotation, translation);
            let _ = scale;
            for index in 0..file.emitters[set].len() {
                let emitter = &file.emitters[set][index];
                // Mesh emitters need their model, which this preview does not load yet.
                let mesh =
                    u64::from_le_bytes(emitter.fixed[0x8c8..0x8d0].try_into().unwrap()) != u64::MAX;
                if mesh {
                    continue;
                }
                let mut s = sim::EmitterSim::new(&emitter.data, &emitter.fixed);
                for _ in 0..draw.frame.min(600) {
                    s.step();
                }
                if s.count() == 0 {
                    continue;
                }
                let streams = s.streams(&draw.placement);
                let dynamic = s.dynamic_block(&draw.placement, &rt, [1.0; 4]);
                let statics = static_block::static_block(&emitter.fixed);
                let custom0 = static_block::custom_block(&emitter.raw);
                let rs = &emitter.data.render_state;
                let state = render::RenderState {
                    blend: render::Blend::from_type(rs.is_blend_enable, rs.blend_type),
                    depth_test: rs.is_depth_test,
                    depth_write: rs.is_depth_mask,
                    depth_func: rs.depth_func,
                    display_side: 0,
                };
                let samplers = [
                    emitter.data.sampler0.clone(),
                    emitter.data.sampler1.clone(),
                    emitter.data.sampler2.clone(),
                ];
                let program = emitter.data.shader_references.shader_index.max(0) as usize;
                for sampler in samplers.iter().flatten() {
                    let Some(texture) = file
                        .texture_ids
                        .iter()
                        .position(|id| *id == sampler.texture_id)
                    else {
                        continue;
                    };
                    if !file.textures.contains_key(&texture) {
                        let view = upload_texture(
                            device,
                            queue,
                            file.pool.as_deref(),
                            texture,
                            file.texture_names.get(texture),
                        );
                        file.textures.insert(texture, view);
                    }
                    let key = (
                        sampler.wrap_u.as_u8(),
                        sampler.wrap_v.as_u8(),
                        sampler.filter,
                    );
                    file.samplers
                        .entry(key)
                        .or_insert_with(|| make_sampler(device, key));
                }
                let views: [Option<&wgpu::TextureView>; 3] = std::array::from_fn(|i| {
                    let id = samplers[i].as_ref()?.texture_id;
                    let texture = file.texture_ids.iter().position(|t| *t == id)?;
                    file.textures.get(&texture)?.as_ref()
                });
                let sampler_refs: [Option<&wgpu::Sampler>; 3] = std::array::from_fn(|i| {
                    let s = samplers[i].as_ref()?;
                    file.samplers
                        .get(&(s.wrap_u.as_u8(), s.wrap_v.as_u8(), s.filter))
                });
                let draw_info = render::EmitterDraw {
                    program,
                    state,
                    static_block: &statics,
                    dynamic_block: &dynamic,
                    custom0_block: custom0.as_deref(),
                    custom2_block: Some(&custom2),
                    streams: &streams,
                    count: s.count() as u32,
                    textures: views,
                    samplers: sampler_refs,
                };
                if let Some(prepared) = file.renderer.prepare(queue, &draw_info) {
                    self.prepared.push((draw.file.clone(), prepared));
                }
            }
        }
    }

    pub fn paint(&self, pass: &mut wgpu::RenderPass<'_>) {
        for (file, prepared) in &self.prepared {
            if let Some(Some(file)) = self.files.get(file) {
                file.renderer.draw(pass, prepared);
            }
        }
    }
}

fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pool: Option<&[u8]>,
    index: usize,
    name: Option<&String>,
) -> Option<wgpu::TextureView> {
    use wgpu::util::DeviceExt;
    let image = crate::texture_import::decode_as_sampled(pool?, index, name?, None).ok()?;
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("vfx texture"),
            size: wgpu::Extent3d {
                width: image.width(),
                height: image.height(),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        image.as_raw(),
    );
    Some(texture.create_view(&Default::default()))
}

/// nn::vfx wrap modes: 0 mirror, 1 repeat, 2 clamp, 3 mirror once (approximated as mirror).
fn make_sampler(device: &wgpu::Device, (u, v, filter): (u8, u8, u8)) -> wgpu::Sampler {
    let mode = |m: u8| match m {
        1 => wgpu::AddressMode::Repeat,
        2 => wgpu::AddressMode::ClampToEdge,
        _ => wgpu::AddressMode::MirrorRepeat,
    };
    let f = if filter == 1 {
        wgpu::FilterMode::Nearest
    } else {
        wgpu::FilterMode::Linear
    };
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("vfx sampler"),
        address_mode_u: mode(u),
        address_mode_v: mode(v),
        mag_filter: f,
        min_filter: f,
        ..Default::default()
    })
}
