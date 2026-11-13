//! Effects in the 3D preview, drawn by `vfx_runtime` with the game's own shaders.
//!
//! The app hands over which effect sets are out this frame (file, set name, placement and
//! frame since spawn); this module keeps one renderer per effect file with its textures on the
//! GPU, runs each set's emitters to that frame and prepares their draws.
//!
//! Nothing slow happens on the paint thread. Reading a file runs on a loader thread; decoding
//! textures, translating shaders and building pipelines run on a warm thread, the ones a paint
//! is waiting for first and the rest of a fighter's file after. Until an emitter is ready the
//! paint skips it, so the first frame of a new move never waits on the driver.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Condvar, Mutex};

use glam::Mat4;
use vfx_runtime::{bnsh, ptcl, render, sim, static_block, view_block};

/// One effect set out this frame.
#[derive(Clone, Debug)]
pub struct EffectDraw {
    pub file: PathBuf,
    /// Entry name as the game spawns it (case-insensitive).
    pub name: String,
    /// Effect space to world (bone, offset, rotation and scale) on every frame since the effect
    /// was spawned, the spawn frame first and this frame last. The emitters step through them,
    /// so effects that emit by distance or leave particles behind follow the real path.
    pub placements: Vec<Mat4>,
}

/// The preview viewport's backdrop colour.
const VIEWPORT_BACKDROP: [u8; 4] = [62, 61, 88, 255];

/// The shared effect file. Its meshes serve every other file, and it is far too big to warm
/// whole, so only what a move uses gets built.
const SHARED_FILE: &str = "ef_common.eff";

struct Emitter {
    data: effect_library::emitter::EmitterData,
    raw: ptcl::Emitter,
    fixed: Vec<u8>,
}

/// A loaded file. Textures and pipelines fill in from the warm thread; the rest is fixed.
struct EffectFile {
    renderer: render::EffectRenderer,
    /// Entry name (lower case) to emitter set index.
    sets: HashMap<String, usize>,
    emitters: Vec<Vec<Emitter>>,
    ptcl: effect_library::ptcl_file::PtclFile,
    texture_ids: Vec<u64>,
    /// `None` for a texture that does not decode: its slot draws blank.
    textures: Mutex<HashMap<usize, Option<Arc<render::GpuTexture>>>>,
    samplers: HashMap<(u8, u8, u8, u32), wgpu::Sampler>,
    meshes: HashMap<u64, vfx_runtime::mesh::Mesh>,
}

type PipelineKey = (usize, render::RenderState, bool);

/// One piece of work for the warm thread.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Job {
    Pipeline(PipelineKey),
    Texture(usize),
}

enum Slot {
    Loading,
    Ready(Arc<EffectFile>),
    Failed,
}

/// Work for the warm thread: what a paint is waiting for, then everything else.
#[derive(Default)]
struct WarmQueue {
    urgent: VecDeque<(Arc<EffectFile>, Job)>,
    later: VecDeque<(Arc<EffectFile>, Job)>,
    closed: bool,
}

type Shared = Arc<(Mutex<WarmQueue>, Condvar)>;

struct Workers {
    load: mpsc::Sender<PathBuf>,
    /// Behind a lock only so the preview can live in shared render state.
    loaded: Mutex<mpsc::Receiver<(PathBuf, Option<EffectFile>)>>,
    warm: Shared,
}

pub struct VfxPreview {
    target: wgpu::TextureFormat,
    depth: Option<wgpu::TextureFormat>,
    files: HashMap<PathBuf, Slot>,
    prepared: Vec<(Arc<EffectFile>, render::PreparedDraw)>,
    workers: Option<Workers>,
    /// Jobs already handed to the warm thread, by file.
    queued: HashSet<(PathBuf, Job)>,
    /// Whether the last prepare skipped something still loading or warming.
    waiting: bool,
}

impl VfxPreview {
    pub fn new(target: wgpu::TextureFormat, depth: Option<wgpu::TextureFormat>) -> Self {
        Self {
            target,
            depth,
            files: HashMap::new(),
            prepared: Vec::new(),
            workers: None,
            queued: HashSet::new(),
            waiting: false,
        }
    }

    /// Whether the last prepare left out an effect that is still loading. The viewport should
    /// paint again shortly so it appears as soon as it is ready.
    pub fn waiting(&self) -> bool {
        self.waiting
    }

    fn workers(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> &Workers {
        self.workers.get_or_insert_with(|| {
            let (load, jobs) = mpsc::channel::<PathBuf>();
            let (done, loaded) = mpsc::channel();
            let (target, depth) = (self.target, self.depth);
            let (d, q) = (device.clone(), queue.clone());
            let _ = std::thread::Builder::new()
                .name("vfx load".into())
                .spawn(move || {
                    for path in jobs {
                        let file = load_file(&d, &q, target, depth, &path);
                        if done.send((path, file)).is_err() {
                            break;
                        }
                    }
                });
            let warm: Shared = Arc::default();
            let shared = warm.clone();
            let (d, q) = (device.clone(), queue.clone());
            let _ = std::thread::Builder::new()
                .name("vfx warm".into())
                .spawn(move || warm_thread(&d, &q, &shared));
            Workers {
                load,
                loaded: Mutex::new(loaded),
                warm,
            }
        })
    }

    /// Start loading a file before anything asks to draw from it (a fighter's own effects when
    /// the fighter opens), and build everything in it in the background.
    pub fn preload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, path: &Path) {
        self.request(device, queue, path);
    }

    /// The file if it is ready; otherwise starts loading it and returns `None`.
    fn request(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: &Path,
    ) -> Option<Arc<EffectFile>> {
        match self.files.get(path) {
            Some(Slot::Ready(file)) => return Some(file.clone()),
            Some(_) => return None,
            None => {}
        }
        let _ = self.workers(device, queue).load.send(path.to_path_buf());
        self.files.insert(path.to_path_buf(), Slot::Loading);
        None
    }

    /// Take in files the loader finished, and queue the rest of a fighter's file to warm.
    fn collect_loaded(&mut self) {
        let Some(workers) = &self.workers else {
            return;
        };
        let finished: Vec<_> = workers
            .loaded
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .try_iter()
            .collect();
        for (path, file) in finished {
            let Some(file) = file else {
                self.files.insert(path, Slot::Failed);
                continue;
            };
            let file = Arc::new(file);
            if !path.ends_with(SHARED_FILE) {
                let jobs: HashSet<Job> = file
                    .emitters
                    .iter()
                    .flatten()
                    .flat_map(|e| file.jobs(e))
                    .collect();
                let mut queue = workers.warm.0.lock().unwrap_or_else(|e| e.into_inner());
                for job in jobs {
                    if self.queued.insert((path.clone(), job)) {
                        queue.later.push_back((file.clone(), job));
                    }
                }
                workers.warm.1.notify_one();
            }
            self.files.insert(path, Slot::Ready(file));
        }
    }

    /// Queue jobs a paint is waiting for, ahead of background work.
    fn hurry(&mut self, path: &Path, file: &Arc<EffectFile>, jobs: &[Job]) {
        let Some(workers) = &self.workers else {
            return;
        };
        let mut queue = workers.warm.0.lock().unwrap_or_else(|e| e.into_inner());
        for job in jobs {
            let key = (path.to_path_buf(), *job);
            if self.queued.insert(key) {
                queue.urgent.push_back((file.clone(), *job));
            } else if let Some(at) = queue
                .later
                .iter()
                .position(|(f, j)| Arc::ptr_eq(f, file) && j == job)
            {
                // Already waiting behind the background work: move it to the front.
                let entry = queue.later.remove(at).unwrap();
                queue.urgent.push_back(entry);
            }
        }
        workers.warm.1.notify_one();
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
        self.waiting = false;
        self.collect_loaded();
        let rows = |m: Mat4| m.transpose().to_cols_array_2d();
        let camera = view_block::view_block(&rows(view), &rows(projection), eye, near, far, 1.0);
        let custom2 = render::default_custom2();
        // The shared effect file, whose meshes other files' emitters may name.
        let shared = self.files.iter().find_map(|(path, slot)| match slot {
            Slot::Ready(file) if path.ends_with(SHARED_FILE) => Some(file.clone()),
            _ => None,
        });
        for draw in draws {
            let Some(file) = self.request(device, queue, &draw.file) else {
                self.waiting |= matches!(self.files.get(&draw.file), Some(Slot::Loading));
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
            let Some(placement) = draw.placements.last().copied() else {
                continue;
            };
            let (_, rotation, translation) = placement.to_scale_rotation_translation();
            let rt = Mat4::from_rotation_translation(rotation, translation);
            for emitter in &file.emitters[set] {
                // A mesh primitive is looked up in this file, then in the shared one.
                let mesh_id = mesh_id(emitter);
                let mesh = (mesh_id != u64::MAX)
                    .then(|| {
                        file.meshes
                            .get(&mesh_id)
                            .or_else(|| shared.as_ref()?.meshes.get(&mesh_id))
                    })
                    .flatten();
                if mesh_id != u64::MAX && mesh.is_none() {
                    continue;
                }
                let missing: Vec<Job> = file
                    .jobs(emitter)
                    .into_iter()
                    .filter(|job| !file.done(job))
                    .collect();
                if !missing.is_empty() {
                    self.hurry(&draw.file, &file, &missing);
                    self.waiting = true;
                    continue;
                }
                let mut s = sim::EmitterSim::new(&emitter.data, &emitter.fixed);
                // The game updates an emitter once before each draw, the spawn frame included
                // (live capture: time 0 drawn after one Calculate): one step per placement.
                let history = &draw.placements[draw.placements.len().saturating_sub(601)..];
                for set in history {
                    s.step_at(set);
                }
                if s.count() == 0 {
                    continue;
                }
                let srt = s.emitter_srt(&placement);
                let streams = s.streams();
                let dynamic = s.dynamic_block(&srt, &s.emitter_rt(&rt), [1.0; 4]);
                let statics = static_block::static_block(&emitter.fixed);
                let custom0 = static_block::custom_block(&emitter.raw);
                let samplers = samplers(emitter);
                let textures = file.textures.lock().unwrap_or_else(|e| e.into_inner());
                let views: [Option<Arc<render::GpuTexture>>; 3] = std::array::from_fn(|i| {
                    let index = file.texture_index(samplers[i]?)?;
                    textures.get(&index)?.clone()
                });
                drop(textures);
                let draw_info = render::EmitterDraw {
                    program: program(emitter),
                    state: render_state(emitter),
                    static_block: &statics,
                    dynamic_block: &dynamic,
                    custom0_block: custom0.as_deref(),
                    custom2_block: Some(&custom2),
                    streams: &streams,
                    count: s.count() as u32,
                    textures: std::array::from_fn(|i| views[i].as_deref()),
                    samplers: std::array::from_fn(|i| {
                        file.samplers.get(&sampler_key(samplers[i]?))
                    }),
                    mesh,
                };
                if let Some(prepared) = file.renderer.prepare(queue, &draw_info) {
                    self.prepared.push((file.clone(), prepared));
                }
            }
        }
    }

    /// Shader or pipeline errors the renderers caught (and skipped) since the last call.
    pub fn take_failures(&mut self) -> Vec<String> {
        self.files
            .values()
            .filter_map(|slot| match slot {
                Slot::Ready(file) => Some(file.renderer.take_failures()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    pub fn paint(&self, pass: &mut wgpu::RenderPass<'_>) {
        for (file, prepared) in &self.prepared {
            file.renderer.draw(pass, prepared);
        }
    }
}

impl Drop for VfxPreview {
    fn drop(&mut self) {
        if let Some(workers) = &self.workers {
            workers
                .warm
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .closed = true;
            workers.warm.1.notify_all();
        }
    }
}

impl EffectFile {
    fn texture_index(&self, sampler: &effect_library::emitter::TextureSampler) -> Option<usize> {
        self.texture_ids
            .iter()
            .position(|id| *id == sampler.texture_id)
    }

    /// What an emitter needs built before it can draw: its pipeline and its textures.
    fn jobs(&self, emitter: &Emitter) -> Vec<Job> {
        let mesh = mesh_id(emitter) != u64::MAX;
        let mut jobs = vec![Job::Pipeline((
            program(emitter),
            render_state(emitter),
            mesh,
        ))];
        for sampler in samplers(emitter).into_iter().flatten() {
            jobs.extend(self.texture_index(sampler).map(Job::Texture));
        }
        jobs
    }

    fn done(&self, job: &Job) -> bool {
        match *job {
            Job::Pipeline((program, state, mesh)) => self.renderer.is_ready(program, state, mesh),
            Job::Texture(index) => self
                .textures
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(&index),
        }
    }

    fn run(&self, device: &wgpu::Device, queue: &wgpu::Queue, job: Job) {
        if self.done(&job) {
            return;
        }
        match job {
            Job::Pipeline((program, state, mesh)) => self.renderer.warm(program, state, mesh),
            Job::Texture(index) => {
                let texture = vfx_runtime::texture::decode(&self.ptcl, index)
                    .map(|t| Arc::new(render::upload_texture(device, queue, &t)));
                self.textures
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(index, texture);
            }
        }
    }
}

fn warm_thread(device: &wgpu::Device, queue: &wgpu::Queue, shared: &Shared) {
    let (lock, signal) = &**shared;
    loop {
        let (file, job) = {
            let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if q.closed {
                    return;
                }
                if let Some(next) = q.urgent.pop_front().or_else(|| q.later.pop_front()) {
                    break next;
                }
                q = signal.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        file.run(device, queue, job);
    }
}

fn load_file(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: wgpu::TextureFormat,
    depth: Option<wgpu::TextureFormat>,
    path: &Path,
) -> Option<EffectFile> {
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
                .collect::<Vec<_>>(),
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
    // Samplers are cheap and few; make every one the file uses now.
    let mut sampler_map = HashMap::new();
    for sampler in emitters.iter().flatten().flat_map(samplers).flatten() {
        let key = sampler_key(sampler);
        sampler_map
            .entry(key)
            .or_insert_with(|| render::make_sampler(device, key.0, key.1, key.2, sampler.max_lod));
    }
    Some(EffectFile {
        renderer: render::EffectRenderer::new(device, queue, &shaders, target, depth),
        sets,
        emitters,
        texture_ids: descriptors.iter().map(|d| d.id).collect(),
        textures: Mutex::default(),
        samplers: sampler_map,
        meshes: vfx_runtime::mesh::meshes(ptcl_file),
        ptcl: ptcl_file.clone(),
    })
}

fn mesh_id(emitter: &Emitter) -> u64 {
    u64::from_le_bytes(emitter.fixed[0x8c8..0x8d0].try_into().unwrap())
}

fn program(emitter: &Emitter) -> usize {
    emitter.data.shader_references.shader_index.max(0) as usize
}

fn render_state(emitter: &Emitter) -> render::RenderState {
    let rs = &emitter.data.render_state;
    render::RenderState {
        blend: render::Blend::from_type(rs.is_blend_enable, rs.blend_type),
        depth_test: rs.is_depth_test,
        depth_write: rs.is_depth_mask,
        depth_func: rs.depth_func,
        display_side: 0,
    }
}

fn samplers(emitter: &Emitter) -> [Option<&effect_library::emitter::TextureSampler>; 3] {
    [
        emitter.data.sampler0.as_ref(),
        emitter.data.sampler1.as_ref(),
        emitter.data.sampler2.as_ref(),
    ]
}

fn sampler_key(s: &effect_library::emitter::TextureSampler) -> (u8, u8, u8, u32) {
    (
        s.wrap_u.as_u8(),
        s.wrap_v.as_u8(),
        s.filter,
        s.max_lod.to_bits(),
    )
}
