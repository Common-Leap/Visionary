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

/// The preview viewport's backdrop colour.
const VIEWPORT_BACKDROP: [u8; 4] = [62, 61, 88, 255];

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
    ptcl: effect_library::ptcl_file::PtclFile,
    texture_ids: Vec<u64>,
    textures: HashMap<usize, Option<render::GpuTexture>>,
    samplers: HashMap<(u8, u8, u8, u32), wgpu::Sampler>,
    meshes: HashMap<u64, vfx_runtime::mesh::Mesh>,
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
            textures: HashMap::new(),
            samplers: HashMap::new(),
            meshes: vfx_runtime::mesh::meshes(ptcl_file),
            ptcl: ptcl_file.clone(),
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
        let custom2 = render::default_custom2();
        // Meshes of the shared effect file, which other files' emitters may name.
        let shared: HashMap<u64, vfx_runtime::mesh::Mesh> = self
            .files
            .iter()
            .filter(|(path, _)| path.ends_with("ef_common.eff"))
            .filter_map(|(_, f)| f.as_ref())
            .flat_map(|f| f.meshes.iter().map(|(k, v)| (*k, v.clone())))
            .collect();
        for draw in draws {
            if !self.files.contains_key(&draw.file) {
                let loaded = self.load(device, queue, &draw.file);
                self.files.insert(draw.file.clone(), loaded);
            }
            let Some(Some(file)) = self.files.get_mut(&draw.file) else {
                continue;
            };
            // A set index past the file's sets (a hand-edited or mismatched eff) draws nothing.
            let Some(&set) = file
                .sets
                .get(&draw.name.to_lowercase())
                .filter(|set| **set < file.emitters.len())
            else {
                continue;
            };
            file.renderer.set_view(queue, &camera);
            // Refraction shaders read the frame behind them; the preview has only its backdrop.
            file.renderer.set_background(queue, VIEWPORT_BACKDROP);
            let (_, rotation, translation) = draw.placement.to_scale_rotation_translation();
            let rt = Mat4::from_rotation_translation(rotation, translation);
            for index in 0..file.emitters[set].len() {
                let emitter = &file.emitters[set][index];
                // A mesh primitive is looked up in this file, then in the shared one.
                let mesh_id = u64::from_le_bytes(emitter.fixed[0x8c8..0x8d0].try_into().unwrap());
                let mesh = (mesh_id != u64::MAX)
                    .then(|| {
                        file.meshes
                            .get(&mesh_id)
                            .cloned()
                            .or_else(|| shared.get(&mesh_id).cloned())
                    })
                    .flatten();
                if mesh_id != u64::MAX && mesh.is_none() {
                    continue;
                }
                let mut s = sim::EmitterSim::new(&emitter.data, &emitter.fixed);
                // The game updates an emitter once before its first draw: on the spawn frame
                // it is already one step in (live capture: time 0 drawn after one Calculate).
                for _ in 0..=draw.frame.min(600) {
                    s.step();
                }
                if s.count() == 0 {
                    continue;
                }
                let srt = s.emitter_srt(&draw.placement);
                let streams = s.streams(&srt);
                let dynamic = s.dynamic_block(&srt, &s.emitter_rt(&rt), [1.0; 4]);
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
                        let view = vfx_runtime::texture::decode(&file.ptcl, texture)
                            .map(|t| render::upload_texture(device, queue, &t));
                        file.textures.insert(texture, view);
                    }
                    let key = sampler_key(sampler);
                    file.samplers.entry(key).or_insert_with(|| {
                        render::make_sampler(device, key.0, key.1, key.2, sampler.max_lod)
                    });
                }
                let views: [Option<&render::GpuTexture>; 3] = std::array::from_fn(|i| {
                    let id = samplers[i].as_ref()?.texture_id;
                    let texture = file.texture_ids.iter().position(|t| *t == id)?;
                    file.textures.get(&texture)?.as_ref()
                });
                let sampler_refs: [Option<&wgpu::Sampler>; 3] = std::array::from_fn(|i| {
                    let s = samplers[i].as_ref()?;
                    file.samplers.get(&sampler_key(s))
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
                    mesh: mesh.as_ref(),
                };
                if let Some(prepared) = file.renderer.prepare(queue, &draw_info) {
                    self.prepared.push((draw.file.clone(), prepared));
                }
            }
        }
    }

    /// Shader or pipeline errors the renderers caught (and skipped) since the last call.
    pub fn take_failures(&mut self) -> Vec<String> {
        self.files
            .values_mut()
            .flatten()
            .flat_map(|f| f.renderer.take_failures())
            .collect()
    }

    pub fn paint(&self, pass: &mut wgpu::RenderPass<'_>) {
        for (file, prepared) in &self.prepared {
            if let Some(Some(file)) = self.files.get(file) {
                file.renderer.draw(pass, prepared);
            }
        }
    }
}

fn sampler_key(s: &effect_library::emitter::TextureSampler) -> (u8, u8, u8, u32) {
    (
        s.wrap_u.as_u8(),
        s.wrap_v.as_u8(),
        s.filter,
        s.max_lod.to_bits(),
    )
}
