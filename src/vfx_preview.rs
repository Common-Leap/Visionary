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
use glam::{Mat4, Vec3};

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
    /// Which spawn of the effect this is. The game seeds every spawn anew (each emitter's seed
    /// type says how), so an effect never plays the same particles twice; the same number here
    /// plays the same ones. A looping draw takes a new one each time round.
    pub spawn: u32,
}

/// The spawn number of a looping draw `clock` frames in, when each time round is `period`
/// frames: the draw's own number for the first, a new one for each after.
fn looped_spawn(spawn: u32, clock: usize, period: usize) -> u32 {
    let round = (clock / period.max(1)) as u32;
    spawn ^ round.wrapping_mul(0x9e37_79b9)
}

/// Each emitter's seed and its children's for one spawn of a set, as the game draws them
/// (see `sim::SpawnSeeds`): in file order, each emitter and then its children.
#[derive(Clone, Debug, PartialEq, Eq)]
struct EmitterSeeds {
    own: u32,
    children: Vec<u32>,
}

fn set_seeds(emitters: &[Emitter], spawn: u32) -> Vec<EmitterSeeds> {
    let mut seeds = sim::SpawnSeeds::new(spawn);
    emitters
        .iter()
        .map(|e| EmitterSeeds {
            own: seeds.emitter(&e.fixed),
            children: e.children.iter().map(|c| seeds.emitter(&c.fixed)).collect(),
        })
        .collect()
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

/// The game's world from the preview's: the preview turns the game a quarter about Y so the
/// fighter faces +Z (model = Ry(-90) * game), and the game's +X, the way a fighter facing right
/// faces and screen right, is the preview's +Z. Effects run in the game's world: an emitter's
/// world gravity (5,076 of them) and world-oriented velocity (749) are the game's axes, and in
/// the preview's they pointed a quarter turn off.
const GAME_FROM_PREVIEW: Mat4 = Mat4::from_cols(
    glam::Vec4::new(0.0, 0.0, -1.0, 0.0),
    glam::Vec4::Y,
    glam::Vec4::new(1.0, 0.0, 0.0, 0.0),
    glam::Vec4::W,
);

/// The preview's view matrix for points in the game's world (see [`GAME_FROM_PREVIEW`]): the
/// same camera, so the picture is unchanged.
fn game_view(view: Mat4) -> Mat4 {
    view * GAME_FROM_PREVIEW.inverse()
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
    /// Present for `nn::vfx` stripe emitters (`EP01`, `EP02`, `EP03`), whose shaders read
    /// per-vertex `sysEmitterPluginAttr*` streams rather than instanced particle quads.
    stripe: Option<StripeConfig>,
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
    stripes: Option<StripeRenderer>,
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
    /// The last prepare's looping draw: its length and its period with the gap, in frames.
    looped: Option<(usize, usize)>,
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
            stripes: None,
            workers: None,
            queued: HashSet::new(),
            waiting: false,
            overrides: HashMap::new(),
            latest: HashMap::new(),
            lengths: HashMap::new(),
            reloading: HashSet::new(),
            next_load: 0,
            looped: None,
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

    /// The length and period, in frames, of the effect the last prepare played on a loop.
    pub fn looped(&self) -> Option<(usize, usize)> {
        self.looped
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
        if let Some(stripes) = &mut self.stripes {
            stripes.clear();
        }
        let game_v = game_view(view);
        let game_view_proj = projection * game_v;
        self.looped = None;
        self.collect_loaded();
        self.waiting = !self.reloading.is_empty();
        let rows = |m: Mat4| m.transpose().to_cols_array_2d();
        // The emitters run in the game's world, where world gravity and world-oriented
        // velocities point; the camera looks at it from the preview's side.
        let eye = GAME_FROM_PREVIEW
            .transform_point3(glam::Vec3::from(eye))
            .to_array();
        let game_eye_vec = Vec3::from(eye);
        let camera = view_block::view_block(
            &rows(game_v),
            &rows(projection),
            eye,
            near,
            far,
            1.0,
        );
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
            let in_game: Vec<Mat4> = draw
                .placements
                .iter()
                .map(|p| GAME_FROM_PREVIEW * *p)
                .collect();
            // A looping draw replays the effect from the start every length plus a short gap.
            let looped: Vec<Mat4>;
            let mut kill_at = draw.kill_at;
            let mut spawn = draw.spawn;
            let placements: &[Mat4] = match (draw.looping, in_game.last()) {
                (Some(clock), Some(at)) => {
                    let key = (Arc::as_ptr(&file) as usize, set);
                    let plan = *self
                        .lengths
                        .entry(key)
                        .or_insert_with(|| effect_loop(&file.emitters[set]));
                    let period = plan.length + 15;
                    self.looped = Some((plan.length, period));
                    looped = vec![*at; clock % period + 1];
                    kill_at = plan.kill;
                    spawn = looped_spawn(draw.spawn, clock, period);
                    &looped
                }
                _ => &in_game,
            };
            let seeds = set_seeds(&file.emitters[set], spawn);
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
                if find_mesh(emitter).is_none() {
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
                if let Some(cfg) = &emitter.stripe {
                    let start = emitter.data.emission.start.max(0) as usize;
                    let tint = [
                        draw.colour[0] * draw.particle_colour[0],
                        draw.colour[1] * draw.particle_colour[1],
                        draw.colour[2] * draw.particle_colour[2],
                        draw.colour[3] * draw.particle_colour[3],
                    ];
                    let vertices = build_stripe_vertices(
                        placements,
                        cfg.local,
                        start,
                        kill_at,
                        cfg,
                        game_eye_vec,
                        tint,
                    );
                    if !vertices.is_empty() {
                        let sampler_defs = samplers(emitter);
                        let textures = file.textures.lock().unwrap_or_else(|e| e.into_inner());
                        let views: [Option<Arc<render::GpuTexture>>; 2] = std::array::from_fn(|i| {
                            let idx = file.texture_index(sampler_defs[i]?)?;
                            textures.get(&idx)?.clone()
                        });
                        drop(textures);
                        let smp_refs: [Option<&wgpu::Sampler>; 2] = std::array::from_fn(|i| {
                            file.samplers.get(&sampler_key(sampler_defs[i]?))
                        });
                        let renderer = self.stripes.get_or_insert_with(|| {
                            StripeRenderer::new(device, queue, self.target, self.depth, self.samples)
                        });
                        renderer.prepare(
                            device,
                            queue,
                            game_view_proj,
                            &vertices,
                            cfg,
                            render_state(emitter),
                            [views[0].as_deref(), views[1].as_deref()],
                            smp_refs,
                        );
                    }
                    continue;
                }
                let mut s = emitter_sim(emitter, &seeds[index]);
                s.set_particle_colour(draw.particle_colour);
                // The game updates an emitter once before each draw, the spawn frame included
                // (live capture: time 0 drawn after one Calculate): one step per placement.
                let first = placements.len().saturating_sub(601);
                for (index, set) in placements.iter().enumerate().skip(first) {
                    if kill_at.is_some_and(|kill| index >= kill) {
                        s.kill();
                    }
                    s.step_at(set);
                }
                for (emitter, s, matrices) in draw_parts(emitter, &s, &placement, &rt) {
                    let Some(mesh) = find_mesh(emitter) else {
                        continue;
                    };
                    if s.count() > 0 {
                        self.prepare_emitter(
                            queue,
                            &file,
                            emitter,
                            s,
                            mesh,
                            (&matrices.0, &matrices.1),
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
        if let Some(stripes) = &self.stripes {
            stripes.draw(pass);
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
        let mut jobs = Vec::new();
        if emitter.stripe.is_none() {
            let mesh = mesh_id(emitter) != u64::MAX;
            jobs.push(Job::Pipeline((
                program(emitter),
                render_state(emitter),
                mesh,
            )));
        }
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

/// An emitter and its child emitters as they draw, each with its matrix (with scale and
/// without) for a set at `placement` (`rt` without scale). A child draws on its own matrix,
/// its parent's times its own (Smash 0x8090), not its parent's: Sephiroth's Scintilla wall is
/// seven child rows of hexagons placed by their own translations, and on the parent's matrix
/// all seven drew on one line.
fn draw_parts<'a>(
    emitter: &'a Emitter,
    s: &'a sim::EmitterSim<'a>,
    placement: &Mat4,
    rt: &Mat4,
) -> Vec<(&'a Emitter, &'a sim::EmitterSim<'a>, (Mat4, Mat4))> {
    std::iter::once((emitter, s))
        .chain(emitter.children.iter().zip(s.children()))
        .map(|(e, s)| (e, s, (s.emitter_srt(placement), s.emitter_rt(rt))))
        .collect()
}

/// An emitter as the preview runs it: from this spawn's seeds, with its child emitters, and
/// with the field sections that move its particles (a convergence field pulls Sephiroth's
/// charge sparks in).
fn emitter_sim<'a>(emitter: &'a Emitter, seeds: &EmitterSeeds) -> sim::EmitterSim<'a> {
    let mut s = sim::EmitterSim::with_seed(&emitter.data, &emitter.fixed, seeds.own);
    s.set_fields(sim::Fields::of(&emitter.raw.attributes));
    for (child, seed) in emitter.children.iter().zip(&seeds.children) {
        s.add_child_with_seed(&child.data, &child.fixed, *seed)
            .set_fields(sim::Fields::of(&child.raw.attributes));
    }
    s
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
    let fixed = static_block::fixed_emitter_data(&raw, &|_| None, None);
    let stripe = StripeConfig::from_emitter(parsed, &fixed);
    Emitter {
        fixed,
        data: parsed.data.clone(),
        stripe,
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
        // 1 draws only front faces, 2 only back faces, as the game culls the other side (4,117
        // corpus emitters, nearly all meshes: an arc's inside and outside are often two
        // emitters, each drawn on its own side).
        display_side: rs.display_side,
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

// ── Stripe particle emitters (`EP01` / `EP02` / `EP03`) ─────────────────────────────────────────
//
// `nn::vfx` stripe emitters (such as Pikmin throw wind trails and Shy Guy's `CARROT_TRAIL`
// `tail_wind`) emit a single ribbon head and build triangle-strip history geometry on the CPU
// (`sysEmitterPluginAttr0..3`). `eff_wgpu` only drives instanced quad/mesh particles, so these
// emitters are rendered here using their own decoded textures, colours, blend state, and history
// parameters.

/// Parsed parameters for an `EP01`, `EP02`, or `EP03` stripe emitter.
#[derive(Debug, Clone)]
pub(crate) struct StripeConfig {
    pub history_count: usize,
    pub partitions: usize,
    pub head_alpha: f32,
    pub tail_alpha: f32,
    pub start_scale: f32,
    pub end_scale: f32,
    pub base_half_width: f32,
    pub color0: [f32; 4],
    pub color1: [f32; 4],
    pub alpha: f32,
    pub local: Mat4,
}

fn read_f32_le(bytes: &[u8], offset: usize) -> Option<f32> {
    let v = f32::from_le_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?);
    v.is_finite().then_some(v)
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

impl StripeConfig {
    pub(crate) fn from_emitter(parsed: &effect_library::Emitter, fixed: &[u8]) -> Option<Self> {
        let sub = parsed
            .subsections
            .iter()
            .find(|s| matches!(s.magic.as_str(), "EP01" | "EP02" | "EP03"))?;
        let d = sub.data.as_slice();
        let (history_count, partitions, head_alpha, tail_alpha, start_scale, end_scale) =
            match sub.magic.as_str() {
                "EP01" => (
                    read_u32_le(d, 0x10).unwrap_or(16).clamp(2, 128) as usize,
                    2usize,
                    read_f32_le(d, 0x18).unwrap_or(1.0).clamp(0.0, 1.0),
                    read_f32_le(d, 0x1c).unwrap_or(0.0).clamp(0.0, 1.0),
                    1.0,
                    1.0,
                ),
                "EP02" => (
                    read_f32_le(d, 0x14).unwrap_or(16.0).round().clamp(2.0, 128.0) as usize,
                    read_f32_le(d, 0x10).unwrap_or(2.0).round().clamp(1.0, 8.0) as usize,
                    read_f32_le(d, 0x1c).unwrap_or(1.0).clamp(0.0, 1.0),
                    read_f32_le(d, 0x20).unwrap_or(0.0).clamp(0.0, 1.0),
                    1.0,
                    1.0,
                ),
                _ => (
                    // EP03 (SuperStripe / Stripe2)
                    read_f32_le(d, 0x18).unwrap_or(16.0).round().clamp(2.0, 128.0) as usize,
                    read_u32_le(d, 0x28).unwrap_or(2).clamp(1, 8) as usize,
                    read_f32_le(d, 0x20).unwrap_or(1.0).clamp(0.0, 1.0),
                    read_f32_le(d, 0x24).unwrap_or(0.0).clamp(0.0, 1.0),
                    read_f32_le(d, 0x5c).filter(|v| *v > 0.0).unwrap_or(1.0),
                    read_f32_le(d, 0x60).filter(|v| *v >= 0.0).unwrap_or(1.0),
                ),
            };
        // Particle scale: base scale (0x130) * emitter particle scale (0x9c0).
        let base_x = read_f32_le(fixed, 0x130).filter(|v| *v > 0.001).unwrap_or(1.0);
        let base_y = read_f32_le(fixed, 0x134).filter(|v| *v > 0.001).unwrap_or(base_x);
        let em_x = read_f32_le(fixed, 0x9c0).filter(|v| *v > 0.001).unwrap_or(1.0);
        let em_y = read_f32_le(fixed, 0x9c4).filter(|v| *v > 0.001).unwrap_or(em_x);
        let width = (base_x * em_x).max(base_y * em_y).max(0.5);
        // Stripe ribbons in Smash are sized in particle units (often ~2–4 units wide when
        // base_scale is 1.0 on a projectile). Clamp to a visible minimum.
        let base_half_width = (width * 0.5).max(0.4);

        let em_c0 = [
            read_f32_le(fixed, 0x7ac).unwrap_or(1.0),
            read_f32_le(fixed, 0x7b0).unwrap_or(1.0),
            read_f32_le(fixed, 0x7b4).unwrap_or(1.0),
            read_f32_le(fixed, 0x7b8).unwrap_or(1.0),
        ];
        let em_c1 = [
            read_f32_le(fixed, 0x7bc).unwrap_or(1.0),
            read_f32_le(fixed, 0x7c0).unwrap_or(1.0),
            read_f32_le(fixed, 0x7c4).unwrap_or(1.0),
            read_f32_le(fixed, 0x7c8).unwrap_or(1.0),
        ];
        let p_c0 = [
            read_f32_le(fixed, 0x3c0).unwrap_or(1.0),
            read_f32_le(fixed, 0x3c4).unwrap_or(1.0),
            read_f32_le(fixed, 0x3c8).unwrap_or(1.0),
        ];
        let p_c1 = [
            read_f32_le(fixed, 0x4c0).unwrap_or(p_c0[0]),
            read_f32_le(fixed, 0x4c4).unwrap_or(p_c0[1]),
            read_f32_le(fixed, 0x4c8).unwrap_or(p_c0[2]),
        ];
        // Alpha0 may animate from 0 at key 0 up to 1 mid-life; pick the peak across active keys
        // so a fade-in curve does not leave the ribbon invisible.
        let mut p_alpha = read_f32_le(fixed, 0x440).unwrap_or(1.0);
        for k in 1..8 {
            if let Some(a) = read_f32_le(fixed, 0x440 + k * 16) {
                p_alpha = p_alpha.max(a);
            }
        }
        let alpha = (p_alpha * em_c0[3]).clamp(0.0, 1.0);
        let alpha = if alpha <= 0.001 { 1.0 } else { alpha };

        let color0 = [
            (p_c0[0] * em_c0[0]).max(0.0),
            (p_c0[1] * em_c0[1]).max(0.0),
            (p_c0[2] * em_c0[2]).max(0.0),
            1.0,
        ];
        let color1 = [
            (p_c1[0] * em_c1[0]).max(0.0),
            (p_c1[1] * em_c1[1]).max(0.0),
            (p_c1[2] * em_c1[2]).max(0.0),
            1.0,
        ];
        let local = {
            let s = sim::EmitterSim::new(&parsed.data, fixed);
            s.emitter_srt(&Mat4::IDENTITY)
        };
        Some(StripeConfig {
            history_count,
            partitions,
            head_alpha,
            tail_alpha,
            start_scale,
            end_scale,
            base_half_width,
            color0,
            color1,
            alpha,
            local,
        })
    }
}

/// One vertex of a stripe ribbon: position (3), UV (2), RGBA colour/alpha (4) = 9 floats.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StripeVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

fn catmull_rom(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f32) -> Vec3 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

/// Build triangle-list ribbon vertices for a stripe emitter along `placements` (oldest first,
/// newest last).
pub(crate) fn build_stripe_vertices(
    placements: &[Mat4],
    emitter_local: Mat4,
    start_frame: usize,
    kill_at: Option<usize>,
    cfg: &StripeConfig,
    eye: Vec3,
    tint: [f32; 4],
) -> Vec<StripeVertex> {
    if placements.len() <= start_frame {
        return Vec::new();
    }
    let last_active = kill_at
        .map(|k| k.min(placements.len()))
        .unwrap_or(placements.len());
    if last_active <= start_frame {
        return Vec::new();
    }
    // After a kill, let the tail catch up over `history_count` frames.
    let drained = placements.len().saturating_sub(last_active);
    if drained >= cfg.history_count {
        return Vec::new();
    }
    let remaining = cfg.history_count - drained;
    let slice_start = last_active.saturating_sub(remaining).max(start_frame);
    let mut raw_points: Vec<(Vec3, f32)> = Vec::with_capacity(last_active - slice_start);
    for set in &placements[slice_start..last_active] {
        let world = *set * emitter_local;
        let pos = world.w_axis.truncate();
        let (scale, _, _) = set.to_scale_rotation_translation();
        let s = ((scale.x.abs() + scale.y.abs() + scale.z.abs()) / 3.0).max(0.01);
        if raw_points
            .last()
            .is_none_or(|(prev, _)| prev.distance_squared(pos) > 1e-6)
        {
            raw_points.push((pos, s));
        }
    }
    if raw_points.len() < 2 {
        return Vec::new();
    }
    // Reverse so index 0 is the head (newest) and last is the tail (oldest).
    raw_points.reverse();

    // Subdivide with Catmull-Rom so curved arcs (like thrown projectiles) stay smooth.
    let partitions = cfg.partitions.max(1);
    let mut samples: Vec<(Vec3, f32)> =
        Vec::with_capacity((raw_points.len() - 1) * partitions + 1);
    let n = raw_points.len();
    for i in 0..n - 1 {
        let p0 = raw_points[i.saturating_sub(1)].0;
        let p1 = raw_points[i].0;
        let p2 = raw_points[i + 1].0;
        let p3 = raw_points[(i + 2).min(n - 1)].0;
        let s1 = raw_points[i].1;
        let s2 = raw_points[i + 1].1;
        for step in 0..partitions {
            let f = step as f32 / partitions as f32;
            let pos = if partitions == 1 {
                p1
            } else {
                catmull_rom(p0, p1, p2, p3, f)
            };
            samples.push((pos, s1 + (s2 - s1) * f));
        }
    }
    samples.push(*raw_points.last().unwrap());

    let m = samples.len();
    let denom = (m - 1).max(1) as f32;
    let kill_fade = if drained > 0 {
        1.0 - (drained as f32 / cfg.history_count.max(1) as f32)
    } else {
        1.0
    };
    let mut edges: Vec<([f32; 3], [f32; 3], f32, [f32; 4])> = Vec::with_capacity(m);
    for (i, &(pos, scale)) in samples.iter().enumerate() {
        let tangent = if i == 0 {
            samples[1].0 - pos
        } else if i + 1 == m {
            pos - samples[i - 1].0
        } else {
            samples[i + 1].0 - samples[i - 1].0
        };
        let dir = tangent.normalize_or_zero();
        if dir == Vec3::ZERO {
            continue;
        }
        let view_dir = (eye - pos).normalize_or_zero();
        let mut side = view_dir.cross(dir);
        if side.length_squared() < 1e-6 {
            side = Vec3::Y.cross(dir);
        }
        if side.length_squared() < 1e-6 {
            side = Vec3::X.cross(dir);
        }
        let side = side.normalize_or_zero();
        let t = i as f32 / denom;
        let scale_t = cfg.start_scale + (cfg.end_scale - cfg.start_scale) * t;
        let hw = cfg.base_half_width * scale_t * scale;
        let alpha_t = cfg.head_alpha + (cfg.tail_alpha - cfg.head_alpha) * t;
        let a = (alpha_t * cfg.alpha * tint[3] * kill_fade).clamp(0.0, 1.0);
        let left = (pos - side * hw).to_array();
        let right = (pos + side * hw).to_array();
        edges.push((left, right, t, [tint[0], tint[1], tint[2], a]));
    }
    if edges.len() < 2 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity((edges.len() - 1) * 6);
    for pair in edges.windows(2) {
        let (l0, r0, v0, c0) = pair[0];
        let (l1, r1, v1, c1) = pair[1];
        out.push(StripeVertex { pos: l0, uv: [0.0, v0], color: c0 });
        out.push(StripeVertex { pos: r0, uv: [1.0, v0], color: c0 });
        out.push(StripeVertex { pos: r1, uv: [1.0, v1], color: c1 });
        out.push(StripeVertex { pos: l0, uv: [0.0, v0], color: c0 });
        out.push(StripeVertex { pos: r1, uv: [1.0, v1], color: c1 });
        out.push(StripeVertex { pos: l1, uv: [0.0, v1], color: c1 });
    }
    out
}

const STRIPE_SHADER: &str = "
struct Frame { view_proj: mat4x4<f32> };
struct Params {
    color0: vec4<f32>,
    color1: vec4<f32>,
};
@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var tex0: texture_2d<f32>;
@group(1) @binding(1) var smp0: sampler;
@group(1) @binding(2) var tex1: texture_2d<f32>;
@group(1) @binding(3) var smp1: sampler;
@group(1) @binding(4) var<uniform> params: Params;
struct In {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};
struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};
@vertex fn vs(v: In) -> Out {
    var o: Out;
    o.pos = frame.view_proj * vec4<f32>(v.pos, 1.0);
    o.uv = v.uv;
    o.color = v.color;
    return o;
}
@fragment fn fs(o: Out) -> @location(0) vec4<f32> {
    let t0 = textureSample(tex0, smp0, o.uv);
    let t1 = textureSample(tex1, smp1, o.uv);
    let diff = abs(params.color0.rgb - params.color1.rgb);
    let two_tone = (diff.x + diff.y + diff.z) > 0.001;
    let base_rgb = select(
        t0.rgb * params.color0.rgb,
        mix(params.color1.rgb, params.color0.rgb, t0.rgb),
        two_tone,
    );
    let rgb = base_rgb * t1.rgb * o.color.rgb;
    let alpha = t0.a * t1.a * o.color.a;
    // The scene holds half intensity; the post pass doubles it (as for effects and trails).
    return vec4<f32>(rgb * 0.5, alpha);
}
";

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct StripePipelineKey {
    blend: render::Blend,
    depth_test: bool,
    depth_write: bool,
}

struct PreparedStripe {
    key: StripePipelineKey,
    vertices: wgpu::Buffer,
    count: u32,
    bind_group: wgpu::BindGroup,
}

struct StripeRenderer {
    target: wgpu::TextureFormat,
    depth: Option<wgpu::TextureFormat>,
    samples: u32,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    tex_layout: wgpu::BindGroupLayout,
    frame_buf: wgpu::Buffer,
    frame_bind: wgpu::BindGroup,
    white_view: wgpu::TextureView,
    default_sampler: wgpu::Sampler,
    pipelines: HashMap<StripePipelineKey, wgpu::RenderPipeline>,
    prepared: Vec<PreparedStripe>,
}

impl StripeRenderer {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: wgpu::TextureFormat,
        depth: Option<wgpu::TextureFormat>,
        samples: u32,
    ) -> Self {
        use wgpu::util::DeviceExt;
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vfx stripe frame"),
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
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let smp_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vfx stripe textures"),
            entries: &[
                tex_entry(0),
                smp_entry(1),
                tex_entry(2),
                smp_entry(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let frame_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vfx stripe frame"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vfx stripe frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame_buf.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vfx stripe"),
            source: wgpu::ShaderSource::Wgsl(STRIPE_SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vfx stripe"),
            bind_group_layouts: &[Some(&frame_layout), Some(&tex_layout)],
            immediate_size: 0,
        });
        let white_tex = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("vfx stripe white"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[255, 255, 255, 255],
        );
        let white_view = white_tex.create_view(&Default::default());
        let default_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            target,
            depth,
            samples,
            shader,
            pipeline_layout,
            tex_layout,
            frame_buf,
            frame_bind,
            white_view,
            default_sampler,
            pipelines: HashMap::new(),
            prepared: Vec::new(),
        }
    }

    fn clear(&mut self) {
        self.prepared.clear();
    }

    fn ensure_pipeline(&mut self, device: &wgpu::Device, key: StripePipelineKey) {
        if self.pipelines.contains_key(&key) {
            return;
        }
        let blend = stripe_blend_state(key.blend);
        let depth_stencil = self.depth.map(|format| wgpu::DepthStencilState {
            format,
            depth_write_enabled: Some(key.depth_write),
            depth_compare: Some(if key.depth_test {
                wgpu::CompareFunction::LessEqual
            } else {
                wgpu::CompareFunction::Always
            }),
            stencil: Default::default(),
            bias: Default::default(),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vfx stripe"),
            layout: Some(&self.pipeline_layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 36,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3,
                        1 => Float32x2,
                        2 => Float32x4
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil,
            multisample: wgpu::MultisampleState {
                count: self.samples,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: self.target,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        self.pipelines.insert(key, pipeline);
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view_proj: Mat4,
        vertices: &[StripeVertex],
        cfg: &StripeConfig,
        state: render::RenderState,
        textures: [Option<&render::GpuTexture>; 2],
        samplers: [Option<&wgpu::Sampler>; 2],
    ) {
        use wgpu::util::DeviceExt;
        let matrix: Vec<u8> = view_proj
            .to_cols_array()
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        queue.write_buffer(&self.frame_buf, 0, &matrix);

        let key = StripePipelineKey {
            blend: state.blend,
            depth_test: state.depth_test,
            depth_write: state.depth_write,
        };
        self.ensure_pipeline(device, key);

        let mut bytes = Vec::with_capacity(vertices.len() * 36);
        for v in vertices {
            for f in [
                v.pos[0], v.pos[1], v.pos[2], v.uv[0], v.uv[1], v.color[0], v.color[1], v.color[2],
                v.color[3],
            ] {
                bytes.extend_from_slice(&f.to_le_bytes());
            }
        }
        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vfx stripe vertices"),
            contents: &bytes,
            usage: wgpu::BufferUsages::VERTEX,
        });
        let params_bytes: Vec<u8> = cfg
            .color0
            .iter()
            .chain(cfg.color1.iter())
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vfx stripe params"),
            contents: &params_bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let t0 = textures[0].map_or(&self.white_view, |t| &t.flat);
        let s0 = samplers[0].unwrap_or(&self.default_sampler);
        let t1 = textures[1].map_or(&self.white_view, |t| &t.flat);
        let s1 = samplers[1].unwrap_or(&self.default_sampler);
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vfx stripe textures"),
            layout: &self.tex_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(t0),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(s0),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(t1),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(s1),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: params_buf.as_entire_binding(),
                },
            ],
        });
        self.prepared.push(PreparedStripe {
            key,
            vertices: vertex_buf,
            count: vertices.len() as u32,
            bind_group,
        });
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.prepared.is_empty() {
            return;
        }
        pass.set_bind_group(0, &self.frame_bind, &[]);
        for stripe in &self.prepared {
            let Some(pipeline) = self.pipelines.get(&stripe.key) else {
                continue;
            };
            pass.set_pipeline(pipeline);
            pass.set_bind_group(1, &stripe.bind_group, &[]);
            pass.set_vertex_buffer(0, stripe.vertices.slice(..));
            pass.draw(0..stripe.count, 0..1);
        }
    }
}

fn stripe_blend_state(blend: render::Blend) -> Option<wgpu::BlendState> {
    use wgpu::{BlendComponent, BlendFactor as F, BlendOperation as Op};
    let color = match blend {
        render::Blend::Off => return None,
        render::Blend::Normal => BlendComponent {
            src_factor: F::SrcAlpha,
            dst_factor: F::OneMinusSrcAlpha,
            operation: Op::Add,
        },
        render::Blend::Add => BlendComponent {
            src_factor: F::SrcAlpha,
            dst_factor: F::One,
            operation: Op::Add,
        },
        render::Blend::Sub => BlendComponent {
            src_factor: F::SrcAlpha,
            dst_factor: F::One,
            operation: Op::ReverseSubtract,
        },
        render::Blend::Screen => BlendComponent {
            src_factor: F::OneMinusDst,
            dst_factor: F::One,
            operation: Op::Add,
        },
        render::Blend::Multiply => BlendComponent {
            src_factor: F::Zero,
            dst_factor: F::Src,
            operation: Op::Add,
        },
    };
    Some(wgpu::BlendState {
        color,
        alpha: BlendComponent {
            src_factor: F::Zero,
            dst_factor: F::DstAlpha,
            operation: Op::Add,
        },
    })
}

#[cfg(test)]
mod stripe_tests {
    use super::*;

    #[test]
    fn stripe_wgsl_parses_and_validates() {
        let module = naga::front::wgsl::parse_str(STRIPE_SHADER).expect("valid WGSL");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator.validate(&module).expect("WGSL validates");
    }

    #[test]
    fn build_stripe_vertices_sweeps_moving_arc_and_drains_on_kill() {
        let cfg = StripeConfig {
            history_count: 10,
            partitions: 2,
            head_alpha: 1.0,
            tail_alpha: 0.0,
            start_scale: 1.0,
            end_scale: 0.5,
            base_half_width: 1.5,
            color0: [1.0; 4],
            color1: [1.0; 4],
            alpha: 1.0,
            local: Mat4::IDENTITY,
        };
        let placements: Vec<Mat4> = (0..15)
            .map(|i| {
                let t = i as f32;
                Mat4::from_translation(Vec3::new(0.0, 3.0 * t - 0.12 * t * t, 0.5 * t))
            })
            .collect();
        let verts = build_stripe_vertices(
            &placements,
            Mat4::IDENTITY,
            3,
            None,
            &cfg,
            Vec3::new(50.0, 10.0, 0.0),
            [1.0; 4],
        );
        // 10 history points subdivided by 2 -> 19 samples -> 18 quads -> 108 vertices.
        assert_eq!(verts.len(), 18 * 6);
        assert!(verts.first().unwrap().color[3] > verts.last().unwrap().color[3]);
        // Once killed long ago (>= history_count frames), the ribbon drains completely.
        let drained = build_stripe_vertices(
            &placements,
            Mat4::IDENTITY,
            3,
            Some(4),
            &cfg,
            Vec3::new(50.0, 10.0, 0.0),
            [1.0; 4],
        );
        assert!(drained.is_empty());
    }
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
        let seeds = set_seeds(emitters, 0);
        let at = emitters
            .iter()
            .position(|e| e.data.name.as_deref() == Some("sparks4"))
            .unwrap();
        let mut s = emitter_sim(sparks, &seeds[at]);
        for _ in 0..30 {
            s.step();
        }
        assert!(s.children().iter().all(|c| c.count() > 0));
        // The sparks' convergence field runs too: 0.11 of the way to the centre a frame on top
        // of their own speed, so the first ones (out at 6 to 30 on frame 18) are nearly in.
        // Without it the preview's sparks crept in at their own speed and drew short dashes.
        // A spawn has only a few sparks out at frame 25, so eight spawns are measured.
        let radius = |s: &sim::EmitterSim| {
            let positions = &s.streams()["sysLocalPosAttr"];
            positions
                .iter()
                .map(|p| glam::Vec3::new(p[0], p[1], p[2]).length())
                .sum::<f32>()
                / positions.len() as f32
        };
        let (mut with, mut without) = (0.0, 0.0);
        for spawn in 0..8 {
            let seed = &set_seeds(emitters, spawn)[at];
            let mut bare = sim::EmitterSim::with_seed(&sparks.data, &sparks.fixed, seed.own);
            let mut pulled = emitter_sim(sparks, seed);
            for _ in 0..25 {
                bare.step();
                pulled.step();
            }
            assert!(radius(&pulled) < radius(&bare) * 0.9, "spawn {spawn}");
            with += radius(&pulled);
            without += radius(&bare);
        }
        assert!(
            with < without * 0.8,
            "{with} with the field, {without} without"
        );
    }

    /// Sephiroth's Scintilla (edge/SpecialLw: edge_senkou_shield on top at 0, 13, 9, turned
    /// -125 about Y, scale 0.85). Its wall is seven child emitters of HexagonFlare01, one row
    /// of hexagons each, placed by its own translation (9.9 down to -13.8). The preview drew
    /// every child on its parent's matrix, so the seven rows fell on one line beside his chest.
    /// Each row must draw at its own height in the set's space.
    #[test]
    fn scintilla_draws_each_row_of_its_wall_at_its_own_height() {
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
            .position(|s| s.name == "P_EdgeSenkouShield")
            .unwrap();
        let emitters = &set_emitters(ptcl_file)[set];
        let at = emitters
            .iter()
            .position(|e| e.data.name.as_deref() == Some("HexagonFlare01"))
            .unwrap();
        let rotation = glam::Quat::from_rotation_y((-125f32).to_radians());
        let offset = glam::Vec3::new(0.0, 13.0, 9.0);
        let placement =
            Mat4::from_scale_rotation_translation(glam::Vec3::splat(0.85), rotation, offset);
        let rt = Mat4::from_rotation_translation(rotation, offset);
        let mut s = emitter_sim(&emitters[at], &set_seeds(emitters, 0)[at]);
        for _ in 0..8 {
            s.step_at(&placement);
        }
        let parts = draw_parts(&emitters[at], &s, &placement, &rt);
        // The parent itself has no transform of its own: it draws on the set's matrix.
        assert_eq!(parts[0].2, (placement, rt));
        let rows = [9.9, 5.9, 1.95, -1.95, -5.9, -9.9, -13.8];
        assert_eq!(parts.len(), 1 + rows.len());
        for ((_, child, (srt, _)), want) in parts[1..].iter().zip(rows) {
            let positions = &child.streams()["sysLocalPosAttr"];
            assert!(positions.len() >= 4, "{} hexagons", positions.len());
            for p in positions {
                let world = srt.transform_point3(glam::Vec3::new(p[0], p[1], p[2]));
                let y = placement.inverse().transform_point3(world).y;
                assert!(
                    (y - want).abs() < 1e-3,
                    "a hexagon at {y}, its row is at {want}"
                );
            }
        }
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

    /// Sephiroth's blue charge played on its own, round after round. The game seeds every spawn
    /// anew, so each charge throws its rays, sparks and lightning differently; the preview
    /// seeded every emitter the same way each time, so every loop was a copy of the first.
    /// Within one time round the frames must agree (each paint runs the round from its start).
    #[test]
    fn each_time_round_an_effect_on_its_own_plays_new_particles() {
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
        let period = effect_loop(emitters).length + 15;
        let base = 0x5eed;
        assert_eq!(
            looped_spawn(base, 3, period),
            looped_spawn(base, period - 1, period)
        );
        assert_eq!(looped_spawn(base, 0, period), base);
        let rounds: Vec<u32> = (0..4)
            .map(|r| looped_spawn(base, r * period + 5, period))
            .collect();
        for (i, a) in rounds.iter().enumerate() {
            assert!(rounds[i + 1..].iter().all(|b| b != a), "{rounds:?}");
        }
        // What each emitter draws 20 frames into a round.
        let play = |spawn: u32| -> Vec<Vec<[f32; 4]>> {
            let seeds = set_seeds(emitters, spawn);
            emitters
                .iter()
                .zip(&seeds)
                .map(|(e, seed)| {
                    let mut s = emitter_sim(e, seed);
                    for _ in 0..20 {
                        s.step();
                    }
                    let streams = s.streams();
                    let mut drawn = streams.get("sysRandomAttr").cloned().unwrap_or_default();
                    drawn.extend(streams.get("sysLocalPosAttr").cloned().unwrap_or_default());
                    drawn
                })
                .collect()
        };
        assert_eq!(play(rounds[0]), play(rounds[0]));
        let (first, second) = (play(rounds[0]), play(rounds[1]));
        let mut differ = 0;
        for (e, (a, b)) in emitters.iter().zip(first.iter().zip(&second)) {
            if !a.is_empty() {
                assert_ne!(a, b, "{:?} played the same particles", e.data.name);
                differ += 1;
            }
        }
        assert!(differ >= 5, "{differ}");
    }

    /// The emitters run in the game's world and the camera still sees the preview's picture:
    /// the fighter's forward (the preview's +Z, screen right) is the game's +X, the way world
    /// gravity and world-oriented velocities name screen right, and up stays up.
    #[test]
    fn effects_run_in_the_games_world_under_the_same_camera() {
        let forward = GAME_FROM_PREVIEW.transform_vector3(glam::Vec3::Z);
        assert!((forward - glam::Vec3::X).length() < 1e-6, "{forward}");
        let up = GAME_FROM_PREVIEW.transform_vector3(glam::Vec3::Y);
        assert!((up - glam::Vec3::Y).length() < 1e-6, "{up}");
        let view = glam::camera::rh::view::look_at_mat4(
            glam::Vec3::new(-60.0, 8.0, 3.0),
            glam::Vec3::new(0.0, 8.0, 0.0),
            glam::Vec3::Y,
        );
        let point = glam::Vec3::new(1.5, 12.0, -4.0);
        let seen = view.transform_point3(point);
        let in_game = game_view(view).transform_point3(GAME_FROM_PREVIEW.transform_point3(point));
        assert!((seen - in_game).length() < 1e-4, "{seen} against {in_game}");
    }

    /// The game draws only one side of an emitter whose display side says so (4,117 corpus
    /// emitters): Sephiroth's up special arcs (EDGE_OCTASLASH_ARC) draw arc1 on its front
    /// faces only. The preview drew every emitter on both sides.
    #[test]
    fn an_emitter_draws_only_the_side_it_names() {
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
            .position(|s| s.name == "P_EdgeOctaslashArc")
            .unwrap();
        let sides: Vec<(String, u8)> = set_emitters(ptcl_file)[set]
            .iter()
            .map(|e| {
                (
                    e.data.name.clone().unwrap_or_default(),
                    render_state(e).display_side,
                )
            })
            .collect();
        assert!(sides.contains(&("arc1".to_owned(), 1)), "{sides:?}");
        assert!(sides.contains(&("arc2".to_owned(), 0)), "{sides:?}");
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
