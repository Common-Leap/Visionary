//! Effects in the 3D preview, drawn by `eff_wgpu` with the game's own shaders.
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

use eff_wgpu::{bnsh, ptcl, render, sim, static_block, view_block};
use glam::Mat4;

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
    /// The step (index into `placements`) on which the move kills the effect
    /// (`EFFECT_OFF_KIND`): from there its emitters stop and fade as their flags say.
    pub kill_at: Option<usize>,
    /// The set colour: red, green, blue and opacity multiplying the effect's own (see
    /// `effect_colour`).
    pub colour: [f32; 4],
    /// The particle colour (`LAST_PARTICLE_SET_COLOR`), which the shaders read per particle
    /// rather than as the set colour.
    pub particle_colour: [f32; 4],
    /// The fighter's ink colour, for spawns that take it (Inkling's).
    pub ink: Option<[f32; 3]>,
    /// Draw only this emitter of the set (the Effects page's "Show only this part").
    pub solo: Option<usize>,
    /// Play the effect over and over on its own length, this many frames since the loop
    /// started, at the last placement (the Effects page shows an effect no move spawns so).
    pub looping: Option<usize>,
    /// Effect frames a game frame (`LAST_EFFECT_SET_RATE`); 1 unless the script sets one.
    pub rate: f32,
}

/// The placements an effect's emitters step through when it plays at `rate` effect frames a
/// game frame, and the step that kills it: `LAST_EFFECT_SET_RATE` makes the game update an
/// effect's emitters that much faster or slower. Each step takes the placement of the game
/// frame it falls in. Whole steps only: a rate below 1 holds a frame for a while.
fn rated_steps(
    placements: &[Mat4],
    kill_at: Option<usize>,
    rate: f32,
) -> (Vec<Mat4>, Option<usize>) {
    if !rate.is_finite() || rate <= 0.0 || (rate - 1.0).abs() < 1e-6 || placements.is_empty() {
        return (placements.to_vec(), kill_at);
    }
    let steps = ((placements.len() as f32 * rate).floor() as usize).max(1);
    let at = |step: usize| ((step as f32 / rate) as usize).min(placements.len() - 1);
    let stepped = (0..steps).map(|step| placements[at(step)]).collect();
    let kill = kill_at.map(|kill| (0..steps).find(|&step| at(step) >= kill).unwrap_or(steps));
    (stepped, kill)
}

/// Where the Effects page puts an effect it plays on its own, from the bones its moves spawn
/// it on. On the fighter's root bones ("top", "rot", "trans") an effect turns with the fighter,
/// which in the preview, facing right, is no turn at all. Any other bone (a hand, a weapon) has
/// a pose the view cannot know without the move, so the effect keeps the game's world axes,
/// the ones effects are made in. Sephiroth's blue charge (edge_fire2_hold, on "haver") throws
/// its rays, sparks and lightning across the screen in game; turned as if on the top bone, that
/// plane was seen edge on and they all stood upright.
pub fn alone_placement(bones: &[&str]) -> Mat4 {
    let root = |bone: &&str| ["top", "rot", "trans"].contains(&bone.to_ascii_lowercase().as_str());
    if bones.is_empty() || bones.iter().any(root) {
        Mat4::IDENTITY
    } else {
        Mat4::from_rotation_y(-std::f32::consts::FRAC_PI_2)
    }
}

/// The draw path the game gives its soft glows (3,389 fighter emitters, nearly all named flare,
/// light or smoke).
const GLOW_PATH: u8 = 21;

/// The order an effect's emitters are drawn in. Captured in game (RenderDoc, Sephiroth's blue
/// charge): the glows go into a half-size buffer that is laid over the scene first, then the
/// other emitters draw in the order the file lists them.
fn draw_order(emitters: &[Emitter]) -> Vec<usize> {
    let glow = |index: &usize| emitters[*index].data.emitter_info.draw_path as u8 == GLOW_PATH;
    let all = 0..emitters.len();
    all.clone()
        .filter(glow)
        .chain(all.filter(|index| !glow(index)))
        .collect()
}

/// How an effect plays on its own, over and over: `length` frames, the last `kill` of them
/// after it is killed, then a short gap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Loop {
    length: usize,
    /// The step that kills the effect, when it has emitters that never stop by themselves.
    kill: Option<usize>,
}

/// The loop for an effect played on its own. An emitter that emits for a set time stops by
/// itself; one that keeps emitting runs until whatever spawned it kills it. The game kills
/// such an effect when its move or state ends, which is when the timed emitters are done
/// (Sephiroth's charge, edge_fire2_hold: its rays and lightning keep coming until its swirl,
/// 73 frames of particles, is gone, and the next charge level replaces it). So the effect is
/// killed when the last timed emitter stops emitting, and no earlier than one particle life
/// after the last running emitter starts, so every emitter shows; the loop ends when the last
/// particle is gone. With no timed emitter at all it plays for 240 frames and restarts.
fn effect_loop(emitters: &[Emitter]) -> Loop {
    const CAP: usize = 240;
    let runs_on = |e: &Emitter| !e.data.emission.is_one_time;
    let mut timed_end = None::<usize>;
    let mut particles_end = 0;
    for emitter in emitters.iter().filter(|e| !runs_on(e)) {
        let emission = &emitter.data.emission;
        let stops = emission.start as usize + emission.duration.max(1) as usize;
        timed_end = Some(timed_end.unwrap_or(0).max(stops));
        let mut s = sim::EmitterSim::new(&emitter.data, &emitter.fixed);
        for step in 1..=CAP {
            s.step();
            if s.count() > 0 {
                particles_end = particles_end.max(step + 1);
            }
        }
    }
    let Some(timed_end) = timed_end else {
        return Loop {
            length: CAP,
            kill: None,
        };
    };
    if !emitters.iter().any(runs_on) {
        return Loop {
            length: particles_end.max(1),
            kill: None,
        };
    }
    let shown = emitters
        .iter()
        .filter(|e| runs_on(e))
        .map(|e| e.data.emission.start as usize + e.data.particle_data.life.max(1) as usize)
        .max()
        .unwrap_or(0);
    let kill = timed_end.max(shown).min(CAP);
    // After the kill the running emitters stop and their particles live out their lives.
    let mut length = kill.max(particles_end);
    for emitter in emitters.iter().filter(|e| runs_on(e)) {
        let mut s = sim::EmitterSim::new(&emitter.data, &emitter.fixed);
        for step in 1..=CAP + kill {
            if step > kill {
                s.kill();
            }
            s.step();
            if s.count() > 0 {
                length = length.max(step + 1);
            }
        }
    }
    Loop {
        length: length.min(kill + CAP),
        kill: Some(kill),
    }
}

/// The shared effect file. Its meshes serve every other file, and it is far too big to warm
/// whole, so only what a move uses gets built.
const SHARED_FILE: &str = "ef_common.eff";

struct Emitter {
    data: effect_library::emitter::EmitterData,
    raw: ptcl::Emitter,
    fixed: Vec<u8>,
    /// Child emitters, which emit from this emitter's particles and draw after it.
    children: Vec<Emitter>,
}

/// A loaded file. Textures and pipelines fill in from the warm thread; the rest is fixed.
struct EffectFile {
    /// Shared with the next version of this file when an edit leaves the shaders alone, so an
    /// edit does not recompile anything.
    renderer: Arc<render::EffectRenderer>,
    shader_hash: u64,
    texture_hash: u64,
    /// Entry name (lower case) to emitter set index.
    sets: HashMap<String, usize>,
    emitters: Vec<Vec<Emitter>>,
    ptcl: effect_library::ptcl_file::PtclFile,
    texture_ids: Vec<u64>,
    /// `None` for a texture that does not decode: its slot draws blank.
    textures: Mutex<HashMap<usize, Option<Arc<render::GpuTexture>>>>,
    samplers: HashMap<(u8, u8, u8, u32), wgpu::Sampler>,
    meshes: HashMap<u64, eff_wgpu::mesh::Mesh>,
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

/// A file to load: its path, the edited bytes to use instead of the file on disk, the version
/// it replaces (whose shaders and textures it may reuse) and the request's number.
type LoadJob = (PathBuf, Option<Arc<Vec<u8>>>, Option<Arc<EffectFile>>, u64);

struct Workers {
    load: mpsc::Sender<LoadJob>,
    /// Behind a lock only so the preview can live in shared render state.
    loaded: Mutex<mpsc::Receiver<(PathBuf, u64, Option<EffectFile>)>>,
    warm: Shared,
}

pub struct VfxPreview {
    target: wgpu::TextureFormat,
    depth: Option<wgpu::TextureFormat>,
    samples: u32,
    /// What refraction shaders see behind them: the scene's backdrop, as stored in the scene.
    backdrop: [u8; 4],
    files: HashMap<PathBuf, Slot>,
    prepared: Vec<(Arc<EffectFile>, render::PreparedDraw)>,
    workers: Option<Workers>,
    /// Jobs already handed to the warm thread, by file.
    queued: HashSet<(PathBuf, Job)>,
    /// Whether the last prepare skipped something still loading or warming.
    waiting: bool,
    /// Edited bytes standing in for files on disk, and the number of the latest load of each
    /// file, so an older load finishing late is dropped.
    overrides: HashMap<PathBuf, Arc<Vec<u8>>>,
    latest: HashMap<PathBuf, u64>,
    /// Each effect's loop for looping play, by file version (its address) and set.
    lengths: HashMap<(usize, usize), Loop>,
    /// Files with a newer version loading while the old one draws: the view keeps painting
    /// until it lands, so an edit shows without waiting for the mouse to move.
    reloading: HashSet<PathBuf>,
    next_load: u64,
}

impl VfxPreview {
    /// Effects drawn into a `target` with `samples` samples per pixel and `depth`.
    pub fn new(
        target: wgpu::TextureFormat,
        depth: Option<wgpu::TextureFormat>,
        samples: u32,
    ) -> Self {
        Self {
            target,
            depth,
            samples,
            backdrop: [0, 0, 0, 255],
            files: HashMap::new(),
            prepared: Vec::new(),
            workers: None,
            queued: HashSet::new(),
            waiting: false,
            overrides: HashMap::new(),
            latest: HashMap::new(),
            lengths: HashMap::new(),
            reloading: HashSet::new(),
            next_load: 0,
        }
    }

    /// The scene's backdrop colour as stored in the scene (before post processing).
    pub fn set_backdrop(&mut self, rgba: [u8; 4]) {
        self.backdrop = rgba;
    }

    /// Whether the last prepare left out an effect that is still loading. The viewport should
    /// paint again shortly so it appears as soon as it is ready.
    pub fn waiting(&self) -> bool {
        self.waiting
    }

    fn workers(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> &Workers {
        self.workers.get_or_insert_with(|| {
            let (load, jobs) = mpsc::channel::<LoadJob>();
            let (done, loaded) = mpsc::channel();
            let (target, depth, samples) = (self.target, self.depth, self.samples);
            let (d, q) = (device.clone(), queue.clone());
            let _ = std::thread::Builder::new()
                .name("vfx load".into())
                .spawn(move || {
                    for (path, bytes, previous, number) in jobs {
                        let started = std::time::Instant::now();
                        let bytes = match bytes {
                            Some(bytes) => Some(bytes),
                            None => std::fs::read(&path).ok().map(Arc::new),
                        };
                        let file = bytes.and_then(|bytes| {
                            load_file(
                                &d,
                                &q,
                                (target, depth, samples),
                                &bytes,
                                previous.as_deref(),
                            )
                        });
                        if std::env::var_os("VFX_TIMING").is_some() {
                            eprintln!(
                                "[vfx timing] read {} in {:.1} ms",
                                path.display(),
                                started.elapsed().as_secs_f64() * 1000.0
                            );
                        }
                        if done.send((path, number, file)).is_err() {
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
        self.start_load(device, queue, path, None);
        self.files.insert(path.to_path_buf(), Slot::Loading);
        None
    }

    fn start_load(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: &Path,
        previous: Option<Arc<EffectFile>>,
    ) {
        self.next_load += 1;
        let number = self.next_load;
        self.latest.insert(path.to_path_buf(), number);
        let bytes = self.overrides.get(path).cloned();
        let _ =
            self.workers(device, queue)
                .load
                .send((path.to_path_buf(), bytes, previous, number));
    }

    /// Draw `path` from these bytes instead of the file on disk (`None` goes back to the
    /// file). The current version keeps drawing until the new one is loaded, and an edit that
    /// leaves the shaders and textures alone reuses them, so editing never flickers.
    pub fn set_override(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: &Path,
        bytes: Option<Arc<Vec<u8>>>,
    ) {
        let same = match (self.overrides.get(path), &bytes) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        match bytes {
            Some(bytes) => self.overrides.insert(path.to_path_buf(), bytes),
            None => self.overrides.remove(path),
        };
        let previous = match self.files.get(path) {
            Some(Slot::Ready(file)) => Some(file.clone()),
            Some(Slot::Loading) => None,
            // Never loaded: load it when something draws from it.
            None => return,
            Some(Slot::Failed) => {
                self.files.insert(path.to_path_buf(), Slot::Loading);
                None
            }
        };
        if previous.is_some() {
            self.reloading.insert(path.to_path_buf());
        }
        self.start_load(device, queue, path, previous);
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
        for (path, number, file) in finished {
            if self.latest.get(&path) != Some(&number) {
                continue;
            }
            self.reloading.remove(&path);
            // Lengths are keyed by file address, which a dropped version may hand on.
            self.lengths.clear();
            // The jobs queued for the version this replaces name that version's file.
            self.queued.retain(|(p, _)| *p != path);
            let Some(file) = file else {
                self.files.insert(path, Slot::Failed);
                continue;
            };
            let file = Arc::new(file);
            if std::env::var_os("VFX_TIMING").is_some() {
                eprintln!("[vfx timing] loaded {} (version {number})", path.display());
            }
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
    #[allow(clippy::too_many_arguments)]
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
        self.collect_loaded();
        self.waiting = !self.reloading.is_empty();
        let rows = |m: Mat4| m.transpose().to_cols_array_2d();
        let camera = view_block::view_block(&rows(view), &rows(projection), eye, near, far, 1.0);
        // Effects draw into the scene before bloom and colour grading, so they take the game's
        // half scale, which the post pass doubles back.
        let custom2 = render::game_custom2();
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
            // A looping draw replays the effect from the start every length plus a short gap.
            let looped: Vec<Mat4>;
            let mut kill_at = draw.kill_at;
            let placements: &[Mat4] = match (draw.looping, draw.placements.last()) {
                (Some(clock), Some(at)) => {
                    let key = (Arc::as_ptr(&file) as usize, set);
                    let plan = *self
                        .lengths
                        .entry(key)
                        .or_insert_with(|| effect_loop(&file.emitters[set]));
                    looped = vec![*at; clock % (plan.length + 15) + 1];
                    kill_at = plan.kill;
                    &looped
                }
                _ => &draw.placements,
            };
            let rated;
            let placements: &[Mat4] = if (draw.rate - 1.0).abs() < 1e-6 {
                placements
            } else {
                let (steps, kill) = rated_steps(placements, kill_at, draw.rate);
                rated = steps;
                kill_at = kill;
                &rated
            };
            file.renderer.set_view(queue, &camera);
            // Refraction shaders read the frame behind them; the preview has only its backdrop.
            file.renderer.set_background(queue, self.backdrop);
            let Some(placement) = placements.last().copied() else {
                continue;
            };
            let (_, rotation, translation) = placement.to_scale_rotation_translation();
            let rt = Mat4::from_rotation_translation(rotation, translation);
            for index in draw_order(&file.emitters[set]) {
                let emitter = &file.emitters[set][index];
                if draw.solo.is_some_and(|solo| solo != index) {
                    continue;
                }
                // A mesh primitive is looked up in this file, then in the shared one; an emitter
                // whose mesh is in neither draws nothing.
                let find_mesh = |emitter: &Emitter| {
                    let id = mesh_id(emitter);
                    if id == u64::MAX {
                        return Some(None);
                    }
                    file.meshes
                        .get(&id)
                        .or_else(|| shared.as_ref()?.meshes.get(&id))
                        .map(Some)
                };
                let Some(mesh) = find_mesh(emitter) else {
                    continue;
                };
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
                s.set_particle_colour(draw.particle_colour);
                for child in &emitter.children {
                    s.add_child(&child.data, &child.fixed);
                }
                // The game updates an emitter once before each draw, the spawn frame included
                // (live capture: time 0 drawn after one Calculate): one step per placement.
                let first = placements.len().saturating_sub(601);
                for (index, set) in placements.iter().enumerate().skip(first) {
                    if kill_at.is_some_and(|kill| index >= kill) {
                        s.kill();
                    }
                    s.step_at(set);
                }
                let srt = s.emitter_srt(&placement);
                let rt = s.emitter_rt(&rt);
                let parts = std::iter::once((emitter, &s, mesh)).chain(
                    emitter
                        .children
                        .iter()
                        .zip(s.children())
                        .filter_map(|(child, sim)| Some((child, sim, find_mesh(child)?))),
                );
                for (emitter, s, mesh) in parts {
                    if s.count() > 0 {
                        self.prepare_emitter(
                            queue,
                            &file,
                            emitter,
                            s,
                            mesh,
                            (&srt, &rt),
                            draw,
                            &custom2,
                        );
                    }
                }
            }
        }
    }

    /// Simulated particles of one emitter (or child) to a draw. `matrices` are the emitter's
    /// with scale and without.
    #[allow(clippy::too_many_arguments)]
    fn prepare_emitter(
        &mut self,
        queue: &wgpu::Queue,
        file: &Arc<EffectFile>,
        emitter: &Emitter,
        s: &sim::EmitterSim,
        mesh: Option<&eff_wgpu::mesh::Mesh>,
        (srt, rt): (&Mat4, &Mat4),
        draw: &EffectDraw,
        custom2: &[u8],
    ) {
        let streams = s.streams();
        if std::env::var_os("VFX_DUMP").is_some() {
            let name = emitter.data.name.clone().unwrap_or_default();
            let first = |stream: &str| {
                streams
                    .get(stream)
                    .map(|v| {
                        v.iter()
                            .take(3)
                            .flatten()
                            .map(|x| format!("{x:.3}"))
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default()
            };
            eprintln!(
                "[vfx particles] {} {} age {} count={} pos {} vec {} scale {}",
                draw.name,
                name,
                draw.placements.len(),
                s.count(),
                first("sysLocalPosAttr"),
                first("sysLocalVecAttr"),
                first("sysScaleAttr")
            );
            let r = srt.transpose().to_cols_array();
            eprintln!(
                "[vfx dump] {} age {} n {} srt ({:.3},{:.3},{:.3},{:.3}) ({:.3},{:.3},{:.3},{:.3}) ({:.3},{:.3},{:.3},{:.3})",
                draw.name, draw.placements.len(), s.count(),
                r[0], r[1], r[2], r[3], r[4], r[5], r[6], r[7], r[8], r[9], r[10], r[11]
            );
        }
        let dynamic = s.dynamic_block(srt, rt, draw.colour);
        let statics = static_block::static_block(&emitter.fixed);
        let custom0 = static_block::custom_block(&emitter.raw);
        // Ink spawns: the game hands the ink shaders the ink colour in this block, row 0
        // multiplying the effect's colour and row 1 added (zero).
        let user_block: Option<Vec<u8>> = draw.ink.map(|[r, g, b]| {
            [r, g, b, 1.0, 0.0, 0.0, 0.0, 0.0]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect()
        });
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
            user_block: user_block.as_deref(),
            custom2_block: Some(custom2),
            streams: &streams,
            count: s.count() as u32,
            textures: std::array::from_fn(|i| views[i].as_deref()),
            samplers: std::array::from_fn(|i| file.samplers.get(&sampler_key(samplers[i]?))),
            mesh,
        };
        if let Some(prepared) = file.renderer.prepare(queue, &draw_info) {
            self.prepared.push((file.clone(), prepared));
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
        for child in &emitter.children {
            jobs.extend(self.jobs(child));
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
                let texture = eff_wgpu::texture::decode(&self.ptcl, index)
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
    (target, depth, samples): (wgpu::TextureFormat, Option<wgpu::TextureFormat>, u32),
    bytes: &[u8],
    previous: Option<&EffectFile>,
) -> Option<EffectFile> {
    let file = effect_library::NamcoEffectFile::load(bytes).ok()?;
    let ptcl_file = file.ptcl_file.as_ref()?;
    let shader_bytes = ptcl_file.shader_info.as_ref()?.binary_data.as_ref()?;
    let shader_hash = hash_bytes(shader_bytes);
    let texture_hash = ptcl_file
        .texture_info
        .as_ref()
        .and_then(|info| info.binary_data.as_deref())
        .map(hash_bytes)
        .unwrap_or(0);
    let renderer = match previous.filter(|p| p.shader_hash == shader_hash) {
        Some(previous) => previous.renderer.clone(),
        None => Arc::new(
            render::EffectRenderer::new(
                device,
                queue,
                &bnsh::ShaderFile::read(shader_bytes).ok()?,
                target,
                depth,
            )
            .with_sample_count(samples),
        ),
    };
    let emitters = set_emitters(ptcl_file);
    let sets = crate::effects::entry_set_handles(&file)
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
    fn with_children(e: &Emitter) -> Vec<&Emitter> {
        std::iter::once(e)
            .chain(e.children.iter().flat_map(with_children))
            .collect()
    }
    for sampler in emitters
        .iter()
        .flatten()
        .flat_map(with_children)
        .flat_map(samplers)
        .flatten()
    {
        let key = sampler_key(sampler);
        sampler_map
            .entry(key)
            .or_insert_with(|| render::make_sampler(device, key.0, key.1, key.2, sampler.max_lod));
    }
    let texture_ids: Vec<u64> = descriptors.iter().map(|d| d.id).collect();
    // Decoded textures carry over when the texture section is unchanged.
    let textures = previous
        .filter(|p| p.texture_hash == texture_hash && p.texture_ids == texture_ids)
        .map(|p| p.textures.lock().unwrap_or_else(|e| e.into_inner()).clone())
        .unwrap_or_default();
    Some(EffectFile {
        renderer,
        shader_hash,
        texture_hash,
        sets,
        emitters,
        texture_ids,
        textures: Mutex::new(textures),
        samplers: sampler_map,
        meshes: eff_wgpu::mesh::meshes(ptcl_file),
        ptcl: ptcl_file.clone(),
    })
}

/// Every set's emitters: parsed, raw and with the runtime's load-time fixes.
fn set_emitters(ptcl_file: &effect_library::ptcl_file::PtclFile) -> Vec<Vec<Emitter>> {
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
                .map(|(parsed, raw)| emitter(parsed, raw))
                .collect::<Vec<_>>(),
        );
    }
    emitters
}

/// An emitter and its children, parsed and raw.
fn emitter(parsed: &effect_library::structs::Emitter, raw: ptcl::Emitter) -> Emitter {
    Emitter {
        fixed: static_block::fixed_emitter_data(&raw, &|_| None, None),
        data: parsed.data.clone(),
        children: parsed
            .children
            .iter()
            .zip(raw.children.iter().cloned())
            .map(|(parsed, raw)| emitter(parsed, raw))
            .collect(),
        raw,
    }
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Sephiroth's blue charge (edge_fire2_hold) played on its own. Its swirl, ball and haze
    /// are timed (one emission, 73 to 76 frames of life); its rays, sparks and lightning keep
    /// emitting until the effect is killed. The loop ran to the 240 frame cap, so for two thirds
    /// of it the swirl was gone and only the rays and lightning played; in game the charge is
    /// replaced as the swirl ends. The swirl must fill most of the loop, and the rays must stop.
    #[test]
    fn a_charge_loop_ends_with_its_timed_emitters() {
        let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(std::path::PathBuf::from)
        else {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
            return;
        };
        let bytes = std::fs::read(root.join("effect/fighter/edge/ef_edge.eff")).unwrap();
        let file = effect_library::NamcoEffectFile::load(&bytes).unwrap();
        let ptcl_file = file.ptcl_file.as_ref().unwrap();
        let set = ptcl_file
            .emitter_list
            .emitter_sets
            .iter()
            .position(|s| s.name == "P_EdgeFire2Hold")
            .unwrap();
        let emitters = &set_emitters(ptcl_file)[set];
        let plan = effect_loop(emitters);
        let period = plan.length + 15;
        let named = |name: &str| {
            emitters
                .iter()
                .find(|e| e.data.name.as_deref() == Some(name))
                .unwrap()
        };
        let alive = |emitter: &Emitter, steps: usize| {
            let mut s = sim::EmitterSim::new(&emitter.data, &emitter.fixed);
            for index in 0..steps {
                if plan.kill.is_some_and(|kill| index >= kill) {
                    s.kill();
                }
                s.step();
            }
            s.count()
        };
        let swirl = named("sphere1");
        let shown = (1..=period)
            .filter(|&steps| alive(swirl, steps) > 0)
            .count();
        assert!(
            shown * 10 >= period * 6,
            "swirl shows {shown} of {period} frames ({plan:?})"
        );
        let rays = named("light2");
        assert!(alive(rays, 60) > 0, "{plan:?}");
        assert_eq!(alive(rays, period), 0, "{plan:?}");
    }

    /// Sephiroth's charge sparks (edge_fire2_hold sparks4) trail red and gold dots from two
    /// child emitters. The preview loaded only the top emitters, so the sparks drew bare. The
    /// children must load with their parent and run from its particles.
    #[test]
    fn child_emitters_load_and_run_with_their_parent() {
        let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(std::path::PathBuf::from)
        else {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
            return;
        };
        let bytes = std::fs::read(root.join("effect/fighter/edge/ef_edge.eff")).unwrap();
        let file = effect_library::NamcoEffectFile::load(&bytes).unwrap();
        let ptcl_file = file.ptcl_file.as_ref().unwrap();
        let set = ptcl_file
            .emitter_list
            .emitter_sets
            .iter()
            .position(|s| s.name == "P_EdgeFire2Hold")
            .unwrap();
        let emitters = &set_emitters(ptcl_file)[set];
        let sparks = emitters
            .iter()
            .find(|e| e.data.name.as_deref() == Some("sparks4"))
            .unwrap();
        let names: Vec<_> = sparks
            .children
            .iter()
            .map(|c| c.data.name.clone().unwrap_or_default())
            .collect();
        assert_eq!(names, ["Emitter1", "Emitter2"]);
        let mut s = sim::EmitterSim::new(&sparks.data, &sparks.fixed);
        for child in &sparks.children {
            s.add_child(&child.data, &child.fixed);
        }
        for _ in 0..30 {
            s.step();
        }
        assert!(s.children().iter().all(|c| c.count() > 0));
    }

    /// The same charge on its own, as the preview camera sees it: from the side, the preview's
    /// +Z to the right and +Y up. Its move spawns it on Sephiroth's hand ("haver"), and in game
    /// its rays (light2) go every way across the screen; placed as on the top bone they all
    /// stood upright, their plane edge on. A top bone effect still sits unturned.
    #[test]
    fn a_hand_effect_on_its_own_throws_its_rays_across_the_screen() {
        assert_eq!(alone_placement(&["top"]), Mat4::IDENTITY);
        assert_eq!(alone_placement(&[]), Mat4::IDENTITY);
        let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(std::path::PathBuf::from)
        else {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
            return;
        };
        let bytes = std::fs::read(root.join("effect/fighter/edge/ef_edge.eff")).unwrap();
        let file = effect_library::NamcoEffectFile::load(&bytes).unwrap();
        let ptcl_file = file.ptcl_file.as_ref().unwrap();
        let set = ptcl_file
            .emitter_list
            .emitter_sets
            .iter()
            .position(|s| s.name == "P_EdgeFire2Hold")
            .unwrap();
        let emitters = &set_emitters(ptcl_file)[set];
        let rays = emitters
            .iter()
            .find(|e| e.data.name.as_deref() == Some("light2"))
            .unwrap();
        // The bone edge/SpecialNStart spawns it on.
        let placement = alone_placement(&["haver"]);
        let mut s = sim::EmitterSim::new(&rays.data, &rays.fixed);
        let (mut across, mut total) = (0, 0);
        for _ in 0..80 {
            s.step_at(&placement);
            let srt = s.emitter_srt(&placement);
            for v in s.streams().get("sysLocalVecAttr").into_iter().flatten() {
                let d = srt
                    .transform_vector3(glam::Vec3::new(v[0], v[1], v[2]))
                    .normalize_or_zero();
                total += 1;
                // More than 30 degrees off upright on screen.
                if d.z.abs() > 0.5 {
                    across += 1;
                }
            }
        }
        assert!(total > 20);
        assert!(across * 3 > total, "{across} of {total} rays go across");
    }

    /// An effect the script plays at another rate (`LAST_EFFECT_SET_RATE`, 282 vanilla calls)
    /// updates that many frames a game frame. The preview ignored the rate, so a rate 2 spawn
    /// played at half speed and a 0.5 one at double.
    #[test]
    fn an_effect_rate_scales_the_steps_and_the_kill() {
        let placements: Vec<Mat4> = (0..10)
            .map(|i| Mat4::from_translation(glam::Vec3::new(i as f32, 0.0, 0.0)))
            .collect();
        let (steps, kill) = rated_steps(&placements, Some(6), 2.0);
        assert_eq!(steps.len(), 20);
        assert_eq!(kill, Some(12));
        assert_eq!(steps[13].w_axis.x, 6.0);
        let (steps, kill) = rated_steps(&placements, Some(6), 0.5);
        assert_eq!(steps.len(), 5);
        assert_eq!(kill, Some(3));
        assert_eq!(steps.last().unwrap().w_axis.x, 8.0);
        let (steps, kill) = rated_steps(&placements, Some(6), 1.0);
        assert_eq!((steps.len(), kill), (10, Some(6)));
    }
}
