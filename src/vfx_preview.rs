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
/// particle is gone, every part played with that kill. With no timed emitter at all it plays
/// for 240 frames and restarts.
fn effect_loop(emitters: &[Emitter]) -> Loop {
    const CAP: usize = 240;
    let runs_on = |e: &Emitter| !e.data.emission.is_one_time;
    let Some(timed_end) = emitters
        .iter()
        .filter(|e| !runs_on(e))
        .map(|e| e.data.emission.start as usize + e.data.emission.duration.max(1) as usize)
        .max()
    else {
        return Loop {
            length: CAP,
            kill: None,
        };
    };
    let kill = emitters.iter().any(runs_on).then(|| {
        let shown = emitters
            .iter()
            .filter(|e| runs_on(e))
            .map(|e| e.data.emission.start as usize + e.data.particle_data.life.max(1) as usize)
            .max()
            .unwrap_or(0);
        timed_end.max(shown).min(CAP)
    });
    // Played as the view plays it: the kill reaches every part, timed ones too. Measured
    // without it, a timed part can keep particles until one comes (Sephiroth's Fire1 bullet),
    // which left the view empty for most of the loop.
    let mut length = kill.unwrap_or(1);
    for emitter in emitters {
        let mut s = sim::EmitterSim::new(&emitter.data, &emitter.fixed);
        for step in 1..=CAP + kill.unwrap_or(0) {
            if kill.is_some_and(|kill| step > kill) {
                s.kill();
            }
            s.step();
            if s.count() > 0 {
                length = length.max(step + 1);
            }
        }
    }
    Loop {
        length: length.min(kill.unwrap_or(0) + CAP),
        kill,
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
    /// What refraction and floor-decal shaders read behind them: copies of the scene's colour
    /// and depth taken before the effects draw.
    scene: Option<(wgpu::TextureView, wgpu::TextureView)>,
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
    /// The last prepare's looping draw: its length and its period with the gap, in frames.
    looped: Option<(usize, usize)>,
    /// Whether that draw has a floor decal, which shows only on a floor close behind it.
    looped_on_floor: bool,
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
            scene: None,
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
            looped: None,
            looped_on_floor: false,
        }
    }

    /// The scene copy the next prepare's draws read (see `eff_wgpu::scene::SceneCopy`).
    pub fn set_scene(&mut self, scene: &eff_wgpu::scene::SceneCopy) {
        self.scene = Some((scene.colour_view().clone(), scene.depth_view().clone()));
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

    /// Whether the effect the last prepare played on a loop has a floor decal to show.
    pub fn looped_on_floor(&self) -> bool {
        self.looped_on_floor
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
        self.looped = None;
        self.looped_on_floor = false;
        self.collect_loaded();
        self.waiting = !self.reloading.is_empty();
        let rows = |m: Mat4| m.transpose().to_cols_array_2d();
        // The emitters run in the game's world, where world gravity and world-oriented
        // velocities point; the camera looks at it from the preview's side.
        let eye = GAME_FROM_PREVIEW
            .transform_point3(glam::Vec3::from(eye))
            .to_array();
        let camera = view_block::view_block(
            &rows(game_view(view)),
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
                    self.looped_on_floor = file.emitters[set].iter().any(has_decal);
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
            if let Some((colour, depth)) = &self.scene {
                file.renderer
                    .set_scene(Some(colour.clone()), Some(depth.clone()));
            }
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

/// Whether an emitter or a child of it draws as a floor decal (Kazuya's ground cracks).
fn has_decal(emitter: &Emitter) -> bool {
    emitter.data.particle_color.is_decal != 0 || emitter.children.iter().any(has_decal)
}

/// An emitter and its child emitters as they draw, each with its matrix (with scale and
/// without) for a set at `placement` (`rt` without scale). A child draws on its own matrix,
/// its parent's times its own (Smash 0x8090), not its parent's: Sephiroth's Scintilla wall is
/// seven child rows of hexagons placed by their own translations, and on the parent's matrix
/// all seven drew on one line. Children marked to draw first (res+0x7e1) draw before their
/// parent and the rest after it, as EmitterSet::Draw orders them (Smash 0x48e0).
fn draw_parts<'a>(
    emitter: &'a Emitter,
    s: &'a sim::EmitterSim<'a>,
    placement: &Mat4,
    rt: &Mat4,
) -> Vec<(&'a Emitter, &'a sim::EmitterSim<'a>, (Mat4, Mat4))> {
    let children = || emitter.children.iter().zip(s.children());
    let first = |e: &Emitter| e.fixed.get(0x7e1) == Some(&1);
    children()
        .filter(|(e, _)| first(e))
        .chain(std::iter::once((emitter, s)))
        .chain(children().filter(|(e, _)| !first(e)))
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

    /// Sephiroth's Fire1 bullet played on its own was empty for about 200 of its 256 frames,
    /// then for about 30 of 68: its parts were measured without the kill the view plays them
    /// with, so particles the kill ends counted as alive. It ends when the last particle is
    /// gone; a set of only timed parts is never killed.
    #[test]
    fn a_loop_ends_when_its_last_particle_is_gone() {
        let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(std::path::PathBuf::from)
        else {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
            return;
        };
        let bytes = std::fs::read(root.join("effect/fighter/edge/ef_edge.eff")).unwrap();
        let file = effect_library::NamcoEffectFile::load(&bytes).unwrap();
        let ptcl_file = file.ptcl_file.as_ref().unwrap();
        let sets = set_emitters(ptcl_file);
        let named = |name: &str| {
            let at = ptcl_file
                .emitter_list
                .emitter_sets
                .iter()
                .position(|s| s.name == name)
                .unwrap_or_else(|| panic!("{name}"));
            effect_loop(&sets[at])
        };
        let bullet = named("P_EdgeFire1Bullet");
        assert!(bullet.kill.is_some(), "{bullet:?}");
        // Every part is out of particles by step 36 with the kill applied.
        assert!(bullet.length <= 40, "{bullet:?}");
        // Only timed parts: the loop is as long as their particles last, as before.
        let timed = (0..sets.len())
            .find(|&at| {
                !sets[at].is_empty()
                    && sets[at].iter().all(|e| e.data.emission.is_one_time)
                    && effect_loop(&sets[at]).length > 1
            })
            .expect("a set with only timed parts");
        assert_eq!(effect_loop(&sets[timed]).kill, None);
    }

    /// The tester's case: Bayonetta's Witch Time clock hand (clock1) and the trail it leaves
    /// (trace1, a child whose own scale is 1). Scale ticked on trace1 makes the trail the
    /// hand's size (5.4 by 30.3 by 5.4); unticked it is its own size; the scale rate
    /// multiplies. Pre-draw puts the trail's draw before the hand's. Each edit goes the way
    /// the preview takes it: the editor's rebuild, then the preview's loader.
    #[test]
    fn a_ticked_inherit_row_changes_what_the_preview_draws() {
        let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(std::path::PathBuf::from)
        else {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
            return;
        };
        let bytes = std::fs::read(root.join("effect/fighter/bayonetta/ef_bayonetta.eff")).unwrap();
        let find = |file: &effect_library::NamcoEffectFile| {
            crate::effects::entry_set_handles(file)
                .into_iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("bayonetta_witchtime_clock"))
                .map(|(_, set)| set as usize)
                .unwrap()
        };
        let set = find(&effect_library::NamcoEffectFile::load(&bytes).unwrap());
        let run = |edits: &[(&str, crate::eff_attrs::AttrValue)]| {
            let edit = crate::mod_project::AuthoredEdit {
                set_name: effect_library::NamcoEffectFile::load(&bytes)
                    .unwrap()
                    .ptcl_file
                    .as_ref()
                    .unwrap()
                    .emitter_list
                    .emitter_sets[set]
                    .name
                    .clone(),
                entry_name: String::new(),
                set_idx: set,
                emitter_name: "trace1".into(),
                emitter_idx: 1,
                fields: crate::mod_project::EmitterFieldEdits {
                    attrs: edits.iter().map(|(id, v)| (id.to_string(), *v)).collect(),
                    ..Default::default()
                },
            };
            let eff = crate::mod_project::EffMod {
                source_rel: "effect/fighter/bayonetta/ef_bayonetta.eff".into(),
                authored: vec![edit],
                ..Default::default()
            };
            let rebuilt = crate::eff_export::rebuild_eff_bytes(&bytes, &eff, None).unwrap();
            let file = effect_library::NamcoEffectFile::load(&rebuilt).unwrap();
            let set = find(&file);
            let emitters = set_emitters(file.ptcl_file.as_ref().unwrap());
            let clock = &emitters[set][0];
            assert_eq!(clock.children[0].data.name.as_deref(), Some("trace1"));
            let seeds = set_seeds(&emitters[set], 0);
            let mut s = emitter_sim(clock, &seeds[0]);
            for _ in 0..8 {
                s.step();
            }
            let trail = s.children()[0].streams()["sysScaleAttr"][0];
            let order: Vec<String> = draw_parts(clock, &s, &Mat4::IDENTITY, &Mat4::IDENTITY)
                .iter()
                .map(|(e, ..)| e.data.name.clone().unwrap_or_default())
                .collect();
            (trail, order)
        };
        use crate::eff_attrs::AttrValue;
        let hand = [5.4, 30.28696, 5.4];
        let close = |scale: [f32; 4], by: f32| {
            (0..3).all(|axis| (scale[axis] - hand[axis] * by).abs() < 1e-3)
        };
        // The game's own file: Scale on, Pre-draw on.
        let (scale, order) = run(&[]);
        assert!(close(scale, 1.0), "{scale:?}");
        assert_eq!(order, ["trace1", "clock1"]);
        // Unticked: its own size.
        let (scale, _) = run(&[("child_inheritance.scale", AttrValue::Int(0))]);
        assert_eq!(scale[..3], [1.0; 3]);
        // Ticked again with half the rate, and Pre-draw off.
        let (scale, order) = run(&[
            ("child_inheritance.scale", AttrValue::Int(1)),
            ("child_inheritance.scale_rate", AttrValue::Float(0.5)),
            ("child_inheritance.pre_draw", AttrValue::Int(0)),
        ]);
        assert!(close(scale, 0.5), "{scale:?}");
        assert_eq!(order, ["clock1", "trace1"]);
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

    /// Played alone with no stage, an effect gets the plain floor only when it has a floor decal
    /// to show on it: Kazuya's Heaven's Impact does, Sephiroth's Scintilla wall does not.
    #[test]
    fn only_an_effect_with_a_floor_decal_asks_for_a_floor() {
        let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(std::path::PathBuf::from)
        else {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
            return;
        };
        let decal = |file: &str, entry: &str| {
            let bytes = std::fs::read(root.join("effect/fighter").join(file)).unwrap();
            let file = effect_library::NamcoEffectFile::load(&bytes).unwrap();
            let set = crate::effects::entry_set_handles(&file)[entry];
            set_emitters(file.ptcl_file.as_ref().unwrap())[set as usize]
                .iter()
                .any(has_decal)
        };
        assert!(decal("demon/ef_demon.eff", "demon_heavens_impact"));
        assert!(!decal("edge/ef_edge.eff", "edge_senkou_shield"));
    }

    /// Kazuya's ground crack (Heaven's Impact) draws only where the scene's depth lies close
    /// behind it. The preview handed it no depth, "nothing behind", and it drew nothing over
    /// any floor. Drawn as the viewport draws it (floor, scene copy, effects), it shows on the
    /// floor; with no scene copy handed over it still draws nothing.
    #[test]
    fn ground_cracks_draw_on_the_preview_floor() {
        let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT").map(std::path::PathBuf::from)
        else {
            eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
            return;
        };
        // wgpu's native requests are ready on their first polls.
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
        let Ok(adapter) = ready(instance.request_adapter(&Default::default())) else {
            eprintln!("no GPU; skipped");
            return;
        };
        let (device, queue) = ready(adapter.request_device(&Default::default())).unwrap();
        let (format, depth_format, samples) = (
            ssbh_wgpu::RGBA_COLOR_FORMAT,
            ssbh_wgpu::DEPTH_FORMAT,
            ssbh_wgpu::MSAA_SAMPLE_COUNT,
        );
        let size = 128u32;
        let texture = |format, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | usage,
                view_formats: &[],
            })
        };
        let eye = glam::Vec3::new(0.0, 40.0, 60.0);
        let view = glam::camera::rh::view::look_at_mat4(eye, glam::Vec3::ZERO, glam::Vec3::Y);
        let projection =
            glam::camera::rh::proj::directx::perspective(45f32.to_radians(), 1.0, 1.0, 1000.0);
        let draw = EffectDraw {
            file: root.join("effect/fighter/demon/ef_demon.eff"),
            name: "demon_heavens_impact".into(),
            placements: vec![Mat4::IDENTITY; 10],
            kill_at: None,
            colour: [1.0; 4],
            particle_colour: [1.0; 4],
            ink: None,
            // groundCrack2, the third part, alone: the set's flashes and rings draw anywhere.
            solo: Some(2),
            looping: None,
            rate: 1.0,
            spawn: 0,
        };
        let floor = crate::preview_floor::PreviewFloor::new(&device, format, depth_format, samples);
        floor.update(&queue, projection * view, [0.0, 0.0, 0.0, 1.0]);
        let mut copy = eff_wgpu::scene::SceneCopy::new(&device, format, samples);
        copy.resize(&device, size, size);
        // Lit pixels (any colour over the black floor) with and without the scene copy.
        let lit = |hand_over_scene: bool| {
            let mut vfx = VfxPreview::new(format, Some(depth_format), samples);
            if hand_over_scene {
                vfx.set_scene(&copy);
            }
            for _ in 0..400 {
                vfx.prepare(
                    &device,
                    &queue,
                    view,
                    projection,
                    eye.into(),
                    1.0,
                    1000.0,
                    std::slice::from_ref(&draw),
                );
                if !vfx.waiting() && !vfx.prepared.is_empty() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            assert!(!vfx.prepared.is_empty(), "the effect never prepared");
            let colour = texture(format, samples, wgpu::TextureUsages::empty());
            let resolve = texture(format, 1, wgpu::TextureUsages::COPY_SRC);
            let depth = texture(depth_format, samples, wgpu::TextureUsages::empty());
            let views = [&colour, &resolve, &depth].map(|t| t.create_view(&Default::default()));
            let scene = ssbh_wgpu::SceneTarget {
                color: &views[0],
                resolve: &views[1],
                depth: &views[2],
                sample_count: samples,
                format,
            };
            let mut encoder = device.create_command_encoder(&Default::default());
            // The models' pass leaves the depth cleared to "far" where it drew nothing.
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: scene.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            {
                let mut pass = crate::renderer::scene_pass(&mut encoder, &scene, "floor");
                floor.draw(&mut pass);
            }
            copy.copy(&device, &mut encoder, scene.resolve, scene.depth);
            vfx.paint(&mut crate::renderer::scene_pass(
                &mut encoder,
                &scene,
                "effects",
            ));
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: (size * size * 8) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                resolve.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(size * 8),
                        rows_per_image: None,
                    },
                },
                resolve.size(),
            );
            queue.submit([encoder.finish()]);
            buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let bytes = buffer.slice(..).get_mapped_range().unwrap().to_vec();
            // Red, green and blue as half floats: any above zero is lit.
            bytes
                .chunks_exact(8)
                .filter(|p| {
                    p[..6]
                        .chunks_exact(2)
                        .any(|h| h[1] & 0x7f != 0 || h[0] != 0)
                })
                .count()
        };
        let with_copy = lit(true);
        let without = lit(false);
        eprintln!("crack pixels: with the scene copy {with_copy}, without {without}");
        assert!(with_copy > 300, "the crack covers only {with_copy} pixels");
        assert_eq!(without, 0);
    }
}
