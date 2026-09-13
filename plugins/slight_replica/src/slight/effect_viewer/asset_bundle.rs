//! SD-backed model, animation, and swing-file serving for the hidden asset carrier.
//!
//! The TCP server only validates a complete immutable snapshot and registers the Arcropolis
//! callbacks. It never touches a game resource object. A game-thread pump hands the pending
//! snapshot to the carrier state machine, which retires the previous owner before switching the
//! callback view and recreating the owner. This is deliberately a lifecycle boundary rather than
//! a resident-buffer patcher: model, animation, and swing files have parsed/GPU/physics state
//! beyond their ARC bytes.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::LazyLock;

use parking_lot::Mutex;

use super::asset_path::{validate_game_path, validate_payload_path};

const MAX_FILES: usize = 4096;
const MAX_FILE_SIZE: u64 = 64 * 1024 * 1024;
const MAX_TOTAL_SIZE: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, serde::Deserialize)]
pub struct AssetBundleFile {
    /// Canonical ARC path served to the game.
    pub path: String,
    /// SD-relative payload path. Must mirror `path` below the dedicated, generation-specific
    /// live-assets directory.
    pub file: String,
    /// Exact byte count written by the desktop.
    pub size: u64,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct AssetBundle {
    /// Version 1 preserves fighter ACMD bindings for the native resource handoff.
    #[serde(default)]
    pub fighter_binding_version: u32,
    /// Nonzero, monotonically increasing desktop generation.
    pub generation: u64,
    /// Lowercase internal fighter name. Every game path must belong to this fighter.
    pub target: String,
    /// Full snapshot. An empty list clears the currently served asset bundle.
    pub files: Vec<AssetBundleFile>,
}

#[derive(Clone)]
struct ServedFile {
    sd_path: String,
    /// Exact path in the carrier's native load graph.
    carrier_path: String,
    /// Every staged file is part of the native constructor's resource graph.
    gated: bool,
    expected_size: usize,
    generation: u64,
}

#[derive(Clone, Default)]
struct Snapshot {
    target: String,
    generation: u64,
    files: HashMap<u64, ServedFile>,
}

/// Snapshot currently visible to Arcropolis callbacks. It remains the old snapshot while a new
/// one is waiting for the carrier to retire. The game-thread `begin_carrier_load` operation is the
/// only place that switches this view.
static SNAPSHOT: LazyLock<Mutex<Snapshot>> = LazyLock::new(|| Mutex::new(Snapshot::default()));
/// A fully validated snapshot waiting for a game-thread owner replacement. Keeping this separate
/// from `SNAPSHOT` means a malformed send or a carrier that cannot be retired never leaves the
/// existing owner reading half of a new bundle.
static PENDING: LazyLock<Mutex<Option<Snapshot>>> = LazyLock::new(|| Mutex::new(None));
/// The previous serving snapshot is retained until the new carrier has served every file and all
/// older callbacks have drained. It stays paired with a failed serving snapshot until a later
/// safe generation begins; it is never exposed as a mid-teardown rollback view.
static PREVIOUS: LazyLock<Mutex<Option<Snapshot>>> = LazyLock::new(|| Mutex::new(None));
/// Serializes generation validation, callback registration, and publication. Without this, two
/// network packets can both observe the same old generation and an older packet can publish
/// after the newer one.
static STAGE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
/// Largest callback capacity registered per ARC hash. Arcropolis size reservations only need to
/// grow; same-size and smaller generations use the existing callback registration.
static REGISTERED: LazyLock<Mutex<HashMap<u64, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// The callback registry is keyed by hash, so retain the path too and reject the (vanishingly
/// unlikely, but unsafe) case where two canonical paths collide.
static REGISTERED_PATHS: LazyLock<Mutex<HashMap<u64, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// Unique current-generation files genuinely requested through Arcropolis.
static SERVED: LazyLock<Mutex<HashSet<(u64, u64)>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
/// Disk callbacks that have cloned a snapshot entry but have not returned to Arcropolis yet.
/// `Ready` cannot release a previous SD generation while one of these callbacks still owns its
/// immutable path; the resource worker may still be copying from that path.
static IN_FLIGHT: LazyLock<Mutex<HashMap<u64, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static GENERATION: AtomicU64 = AtomicU64::new(0);
/// Generation of the snapshot currently being served by the callbacks.
static SERVING_GENERATION: AtomicU64 = AtomicU64::new(0);
static STAGED_COUNT: AtomicU64 = AtomicU64::new(0);
static STATUS_DIRTY: AtomicBool = AtomicBool::new(true);
static LAST_REPORT: AtomicU64 = AtomicU64::new(u64::MAX);

struct CallbackGuard {
    generation: u64,
}

impl CallbackGuard {
    fn enter(generation: u64) -> Self {
        *IN_FLIGHT.lock().entry(generation).or_default() += 1;
        Self { generation }
    }
}

impl Drop for CallbackGuard {
    fn drop(&mut self) {
        let mut in_flight = IN_FLIGHT.lock();
        if let Some(count) = in_flight.get_mut(&self.generation) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                in_flight.remove(&self.generation);
            }
        }
        STATUS_DIRTY.store(true, Ordering::Release);
    }
}

fn callbacks_before_in_flight(generation: u64) -> usize {
    IN_FLIGHT
        .lock()
        .iter()
        .filter(|(callback_generation, _)| **callback_generation < generation)
        .map(|(_, count)| *count)
        .sum()
}

/// The asset carrier is a real Alucard item. Its normal constructor owns this fixed resource
/// graph, so an incoming fighter-slot path is translated to the corresponding Alucard resource
/// name before Arcropolis registration. This is what makes the callback run during the carrier's
/// blessed model/motion load instead of waiting for the already-live fighter to load again.
const ASSET_CARRIER_MODEL_ROOT: &str = "assist/alucard/model/body/c00/";
const ASSET_CARRIER_MOTION_ROOT: &str = "assist/alucard/motion/body/c00/";

/// Translate a user-facing fighter asset path to the exact file path requested by Alucard.
///
/// The desktop has already remapped descriptors and textures to native carrier names.
/// A late callback replaces bytes; it does not add a new native directory entry.
fn carrier_path(game_path: &str) -> Result<String, String> {
    let lower = game_path.to_ascii_lowercase();
    let Some((_, rest)) = lower.split_once("/model/body/") else {
        if let Some((_, rest)) = lower.split_once("/motion/body/") {
            let Some((_, file)) = rest.split_once('/') else {
                return Err("motion asset is missing its costume/file component".into());
            };
            let path = format!("{ASSET_CARRIER_MOTION_ROOT}{file}");
            return supported_carrier_motion(&path)
                .then_some(path)
                .ok_or_else(|| format!("Alucard carrier does not request motion file: {file}"));
        }
        return Err("asset must be under a fighter model/body or motion/body directory".into());
    };
    let Some((_, file)) = rest.split_once('/') else {
        return Err("model asset is missing its costume/file component".into());
    };
    let path = format!("{ASSET_CARRIER_MODEL_ROOT}{file}");
    supported_carrier_model(&path)
        .then_some(path)
        .ok_or_else(|| format!("Alucard carrier does not request model file: {file}"))
}

fn supported_carrier_model(path: &str) -> bool {
    let Some(file) = path.strip_prefix(ASSET_CARRIER_MODEL_ROOT) else {
        return false;
    };
    if file.ends_with(".nutexb") {
        return super::asset_pool::is_pool_file(file, false);
    }
    matches!(
        file,
        "model.xmb"
            | "model.nusktb"
            | "model.numdlb"
            | "model.numshb"
            | "model.nuanmb"
            | "model.numatb"
            | "model.nuhlpb"
            | "model.numshexb"
            | "model.nusrcmdlb"
            | "lod.xmb"
    )
}

/// Files served opportunistically: staged when the fighter ships them, but never required
/// for Ready. The LOD descriptor is the case in point — without the fighter's own mapping
/// the carrier builds the model under the wrong LOD assignment and high/low weapon meshes
/// draw together, yet a carrier graph that never reads it must not stall the whole preview.
fn is_opportunistic_carrier_file(carrier_path: &str) -> bool {
    carrier_path
        .rsplit_once('/')
        .map(|(_, file)| file == "lod.xmb")
        .unwrap_or(false)
}

fn supported_carrier_motion(path: &str) -> bool {
    let Some(file) = path.strip_prefix(ASSET_CARRIER_MOTION_ROOT) else {
        return false;
    };
    matches!(
        file,
        "swing.prc" | "swingblend.prc" | "ik.prc" | "motion_list.bin"
    ) || super::asset_pool::is_pool_file(file, true)
}

/// Lifecycle is reported separately from the old `staged/served` counters so the desktop can
/// distinguish a queued replacement from a live carrier. Values are kept as strings at the wire
/// boundary; this enum is only an internal stable tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetBundlePhase {
    Idle,
    Staged,
    Retiring,
    Loading,
    Ready,
    Failed,
}

impl AssetBundlePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Staged => "staged",
            Self::Retiring => "retiring",
            Self::Loading => "loading",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

static PHASE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn phase_from_u8(value: u8) -> AssetBundlePhase {
    match value {
        1 => AssetBundlePhase::Staged,
        2 => AssetBundlePhase::Retiring,
        3 => AssetBundlePhase::Loading,
        4 => AssetBundlePhase::Ready,
        5 => AssetBundlePhase::Failed,
        _ => AssetBundlePhase::Idle,
    }
}

fn set_phase(phase: AssetBundlePhase) {
    PHASE.store(
        match phase {
            AssetBundlePhase::Idle => 0,
            AssetBundlePhase::Staged => 1,
            AssetBundlePhase::Retiring => 2,
            AssetBundlePhase::Loading => 3,
            AssetBundlePhase::Ready => 4,
            AssetBundlePhase::Failed => 5,
        },
        Ordering::Release,
    );
    STATUS_DIRTY.store(true, Ordering::Release);
}

/// Read directly into Arcropolis's destination buffer. Avoiding an intermediate `Vec` is
/// important: complete fighter model bundles can be tens of megabytes, while Skyline's plugin
/// heap is small enough that one multi-megabyte diagnostic clone previously aborted the game.
extern "C" fn disk_cb(hash: u64, out: *mut u8, capacity: usize, out_size: &mut usize) -> bool {
    let entry = {
        let snapshot = SNAPSHOT.lock();
        if let Some(entry) = snapshot.files.get(&hash).cloned() {
            // Enter while the serving snapshot lock is held. This closes the race where a
            // game-thread finish could drop `PREVIOUS` after an old callback cloned its path
            // but before that callback was visible to the retirement gate.
            let guard = CallbackGuard::enter(entry.generation);
            (entry, guard, true)
        } else {
            return false;
        }
    };
    let (entry, _callback_guard, counts_toward_ready) = entry;
    if entry.expected_size > capacity || out.is_null() {
        crate::slight::diag::note(format!(
            "asset bundle capacity mismatch for {hash:#x}: {} > {capacity}",
            entry.expected_size
        ));
        return false;
    }
    let Ok(metadata) = std::fs::symlink_metadata(&entry.sd_path) else {
        crate::slight::diag::note(format!(
            "asset bundle payload disappeared: {}",
            entry.sd_path
        ));
        return false;
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        crate::slight::diag::note(format!(
            "asset bundle payload is not a regular file: {}",
            entry.sd_path
        ));
        return false;
    }
    let Ok(mut file) = std::fs::File::open(&entry.sd_path) else {
        crate::slight::diag::note(format!(
            "asset bundle payload disappeared: {}",
            entry.sd_path
        ));
        return false;
    };
    if metadata.len() as usize != entry.expected_size {
        crate::slight::diag::note(format!(
            "asset bundle payload changed size: {} ({} != {})",
            entry.sd_path,
            metadata.len(),
            entry.expected_size
        ));
        return false;
    }
    let destination = unsafe { std::slice::from_raw_parts_mut(out, entry.expected_size) };
    if file.read_exact(destination).is_err() {
        crate::slight::diag::note(format!("asset bundle read failed: {}", entry.sd_path));
        return false;
    }
    *out_size = entry.expected_size;

    // An old callback may finish after a newer snapshot was installed. It served valid bytes to
    // its own request, but must not advance the new generation's status.
    if counts_toward_ready && SERVING_GENERATION.load(Ordering::Acquire) == entry.generation {
        let inserted = SERVED.lock().insert((entry.generation, hash));
        if inserted {
            STATUS_DIRTY.store(true, Ordering::Release);
            // First-serve proof per file: opportunistic entries (lod.xmb) never appear in the
            // staged/served counters, so this line is the only record they were read at all.
            super::effect_reload::mark(&format!(
                "asset_bundle served {} gen={}",
                entry.carrier_path, entry.generation
            ));
        }
    }
    true
}

fn reject<T>(reason: impl Into<String>) -> Result<T, String> {
    let reason = reason.into();
    crate::slight::diag::note(format!(
        "asset bundle REJECTED, previous staging kept: {reason}"
    ));
    crate::rust_extender::debuggable_server::notify_asset_bundle_error(&reason);
    Err(reason)
}

/// Validate and atomically stage a full asset snapshot.
pub fn stage(bundle: AssetBundle) -> Result<(), String> {
    let _stage_guard = STAGE_LOCK.lock();
    if !bundle.files.is_empty() && bundle.fighter_binding_version != 1 {
        return reject("restart the updated Visionary application and reload assets; this bundle predates fighter resource binding");
    }
    // `GENERATION` is the newest accepted request, not necessarily the generation currently
    // being served. A newer edit may replace an older pending edit while the previous carrier is
    // still live; the active callback view is not changed until the game thread starts a swap.
    let current_generation = GENERATION.load(Ordering::Acquire);
    if bundle.generation == 0 {
        return reject("generation must be nonzero");
    }
    if bundle.generation <= current_generation {
        return reject(format!(
            "generation {} is not newer than current generation {current_generation}",
            bundle.generation
        ));
    }
    if bundle.files.len() > MAX_FILES {
        return reject(format!(
            "{} files exceeds the {MAX_FILES}-file bundle limit",
            bundle.files.len()
        ));
    }
    // A zero-file snapshot is the explicit clear command. It carries no fighter because no
    // owner is being staged. Nonempty snapshots validate the target again per path below.
    if bundle.files.is_empty() && !bundle.target.is_empty() {
        return reject("an empty asset snapshot must have an empty target");
    }

    let mut total_size = 0u64;
    let mut next = HashMap::with_capacity(bundle.files.len());
    for item in &bundle.files {
        validate_game_path(&bundle.target, &item.path)
            .map_err(|why| format!("{}: {why}", item.path))
            .or_else(reject)?;
        validate_payload_path(bundle.generation, &item.path, &item.file)
            .map_err(|why| format!("{}: {why}", item.file))
            .or_else(reject)?;
        if item.size == 0 || item.size > MAX_FILE_SIZE {
            return reject(format!(
                "{} has invalid size {} (limit {MAX_FILE_SIZE})",
                item.path, item.size
            ));
        }
        total_size = total_size
            .checked_add(item.size)
            .ok_or_else(|| "bundle byte count overflowed".to_string())
            .or_else(reject)?;
        if total_size > MAX_TOTAL_SIZE {
            return reject(format!(
                "bundle size {total_size} exceeds the {MAX_TOTAL_SIZE}-byte limit"
            ));
        }

        let carrier_path = carrier_path(&item.path)
            .map_err(|why| format!("{}: {why}", item.path))
            .or_else(reject)?;
        // Every other file belongs to the constructor's boot-installed load graph and
        // gates Ready; opportunistic files are served when read but never stall it.
        let gated = !is_opportunistic_carrier_file(&carrier_path);
        let hash = smash::hash40(&carrier_path);
        if next.contains_key(&hash) {
            return reject(format!("duplicate or colliding game path: {}", item.path));
        }
        if let Some(registered_path) = REGISTERED_PATHS.lock().get(&hash) {
            if registered_path != &carrier_path {
                return reject(format!(
                    "game path collides with an already registered path: {}",
                    carrier_path
                ));
            }
        }
        // The destination is one of the fixed Alucard carrier files above. It need not already be
        // resident: staging commonly happens before the hidden item is created, and the callback
        // is precisely what supplies the bytes during that first owner load.

        // The generation component is part of the immutable snapshot contract. Never reuse a
        // prior generation's path: Arcropolis/resource-worker reads can outlive this callback's
        // return and the desktop may replace the next snapshot immediately.
        let sd_path = format!("sd:/{}", item.file);
        let metadata = std::fs::symlink_metadata(&sd_path)
            .map_err(|error| format!("cannot stat {sd_path}: {error}"))
            .or_else(reject)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != item.size {
            return reject(format!(
                "payload size mismatch for {sd_path}: expected {}, found {}",
                item.size,
                metadata.len()
            ));
        }
        let expected_size = usize::try_from(item.size)
            .map_err(|_| format!("payload is too large for this runtime: {}", item.path))
            .or_else(reject)?;
        next.insert(
            hash,
            ServedFile {
                sd_path,
                carrier_path,
                gated,
                expected_size,
                generation: bundle.generation,
            },
        );
    }

    let staged = next.len();
    // Publish only to the pending slot. The old callback view remains intact until a game-thread
    // carrier teardown has completed. This is the critical distinction between staging and a
    // reload: a TCP packet cannot invalidate resources owned by the running game object. Callback
    // registration is also deferred to `begin_carrier_load`; registering a larger capacity here
    // would mutate Arcropolis/ARC metadata while the old owner could still be reading it.
    {
        let mut pending = PENDING.lock();
        *pending = Some(Snapshot {
            target: bundle.target.clone(),
            generation: bundle.generation,
            files: next,
        });
    }
    STAGED_COUNT.store(staged as u64, Ordering::Release);
    GENERATION.store(bundle.generation, Ordering::Release);
    set_phase(AssetBundlePhase::Staged);
    crate::slight::diag::note(format!(
        "asset bundle staged target={} generation={} files={staged}; carrier reload queued",
        bundle.target, bundle.generation
    ));
    Ok(())
}

/// Return the newest validated snapshot waiting for a game-thread owner replacement.
pub fn pending_carrier_generation() -> Option<u64> {
    PENDING.lock().as_ref().map(|pending| pending.generation)
}

/// Return the target of the newest validated snapshot. This is used by the carrier host selector;
/// it intentionally reads only immutable staged metadata and performs no native game work.
pub fn pending_carrier_target() -> Option<String> {
    PENDING
        .lock()
        .as_ref()
        .map(|pending| pending.target.clone())
}

/// Whether the newest pending snapshot is the explicit clear command. A clear retires the
/// current owner and publishes an empty callback view; it must never create a fresh Alucard item.
pub fn pending_carrier_is_clear() -> bool {
    PENDING
        .lock()
        .as_ref()
        .is_some_and(|pending| pending.target.is_empty() && pending.files.is_empty())
}

/// Mark a replacement as being retired. This is called from the game thread when the carrier has
/// accepted the request, not from `stage` on the socket thread.
pub fn mark_carrier_retiring(generation: u64) {
    if pending_carrier_generation() == Some(generation) {
        set_phase(AssetBundlePhase::Retiring);
    }
}

/// Register callbacks for a snapshot immediately before its owner is created.
///
/// Registration is intentionally not part of [`stage`]. Arcropolis keeps capacity metadata beside
/// the callback, and growing that reservation while the outgoing Alucard owner is still live can
/// race its resource teardown. The caller invokes this from the game thread after the old owner
/// and both native resource directories have reached their release gate.
fn register_snapshot_files(snapshot: &Snapshot) -> Result<(), String> {
    if snapshot.files.is_empty() {
        return Ok(());
    }
    if !super::arcrop::init() {
        return Err("Arcropolis API is unavailable".into());
    }
    for (&hash, file) in &snapshot.files {
        if super::resource_reload::hash_to_index_for_path_hash(hash).is_none() {
            return Err(format!(
                "Live asset support is not loaded ({}). Start Visionary once, enable Visionary Live Assets in ARCropolis, then restart the game.",
                file.carrier_path,
            ));
        }
    }
    let to_register: Vec<(u64, usize, String)> = {
        let registered = REGISTERED.lock();
        let registered_paths = REGISTERED_PATHS.lock();
        snapshot
            .files
            .iter()
            .filter_map(|(&hash, file)| {
                if let Some(path) = registered_paths.get(&hash) {
                    if path != &file.carrier_path {
                        return Some(Err(format!(
                            "game path collides with an already registered path: {}",
                            file.carrier_path
                        )));
                    }
                }
                let capacity = registered.get(&hash).copied().unwrap_or(0);
                (file.expected_size > capacity)
                    .then(|| Ok((hash, file.expected_size, file.carrier_path.clone())))
            })
            .collect::<Result<_, _>>()?
    };
    for (hash, size, path) in to_register {
        // A partial registration is harmless: the snapshot is not made visible until all
        // registrations succeed, and a retry sees the successful entries in `REGISTERED`.
        if !super::arcrop::register_disk(hash, size, disk_cb) {
            return Err(format!("Arcropolis callback registration failed: {path}"));
        }
        REGISTERED.lock().insert(hash, size);
        REGISTERED_PATHS.lock().insert(hash, path);
    }
    Ok(())
}

/// Switch Arcropolis to the pending snapshot immediately before the new owner is created. The
/// caller must be on the game thread and must already have completed the old owner's teardown.
/// The previous snapshot remains retained in `PREVIOUS` until `finish_carrier_load` succeeds.
pub fn begin_carrier_load(generation: u64) -> Result<usize, String> {
    // Keep the snapshot clone, callback registration, and pending-slot take in one staging
    // critical section. Without this, `stage` could publish generation G+1 after this function
    // cloned G but before it took PENDING; the final take would then discard G+1 after registering
    // the older callback view.
    let _stage_guard = STAGE_LOCK.lock();
    let next = {
        let pending = PENDING.lock();
        let Some(next) = pending.as_ref() else {
            return Err("no pending asset bundle".into());
        };
        if next.generation != generation {
            return Err(format!(
                "pending asset generation changed: expected {generation}, found {}",
                next.generation
            ));
        }
        next.clone()
    };
    // Do this before publishing `SNAPSHOT`: if registration fails, the old callback view remains
    // valid and the pending snapshot can be retried without leaving a half-visible generation.
    register_snapshot_files(&next)?;
    let next = PENDING
        .lock()
        .take()
        .ok_or_else(|| "pending asset bundle disappeared during registration".to_string())?;
    if next.generation != generation {
        return Err(format!(
            "pending asset generation changed during registration: expected {generation}, found {}",
            next.generation
        ));
    }
    let old = {
        let mut serving = SNAPSHOT.lock();
        std::mem::replace(&mut *serving, next)
    };
    {
        let mut previous = PREVIOUS.lock();
        *previous = Some(old);
    }
    SERVED.lock().clear();
    SERVING_GENERATION.store(generation, Ordering::Release);
    STAGED_COUNT.store(
        SNAPSHOT
            .lock()
            .files
            .values()
            .filter(|file| file.gated)
            .count()
            .try_into()
            .unwrap_or(u64::MAX),
        Ordering::Release,
    );
    set_phase(AssetBundlePhase::Loading);
    Ok(STAGED_COUNT.load(Ordering::Acquire) as usize)
}

/// Number of current-generation callbacks genuinely completed by Arcropolis.
/// Counts carrier-gated files only; fighter-only serves never inflate the Ready counters.
pub fn served_count(generation: u64) -> usize {
    if SERVING_GENERATION.load(Ordering::Acquire) != generation {
        return 0;
    }
    let snapshot = SNAPSHOT.lock();
    SERVED
        .lock()
        .iter()
        .filter(|(served_generation, hash)| {
            *served_generation == generation
                && snapshot.files.get(hash).is_some_and(|file| file.gated)
        })
        .count()
}

/// Number of files in the snapshot currently being loaded. This remains available after the
/// pending slot is consumed, so the game-thread owner can report progress while callbacks are
/// running. Counts carrier-gated files only, matching `served_count`.
pub fn staged_count(generation: u64) -> usize {
    if SERVING_GENERATION.load(Ordering::Acquire) == generation {
        return SNAPSHOT
            .lock()
            .files
            .values()
            .filter(|file| file.gated)
            .count();
    }
    PENDING
        .lock()
        .as_ref()
        .filter(|pending| pending.generation == generation)
        .map_or(0, |pending| {
            pending.files.values().filter(|file| file.gated).count()
        })
}

/// True only when every gated file in the active carrier snapshot has been read through
/// Arcropolis. Fighter-only entries are excluded (see `gated`). Empty snapshots are
/// vacuously complete and are used to clear a carrier.
pub fn all_files_served(generation: u64) -> bool {
    if SERVING_GENERATION.load(Ordering::Acquire) != generation {
        return false;
    }
    let snapshot = SNAPSHOT.lock();
    let staged = snapshot.files.values().filter(|file| file.gated).count();
    SERVED
        .lock()
        .iter()
        .filter(|(served_generation, hash)| {
            *served_generation == generation
                && snapshot.files.get(hash).is_some_and(|file| file.gated)
        })
        .count()
        >= staged
}

/// Commit the replacement after the native owner is known to be live and all callbacks have
/// served. The old snapshot is released only here.
pub fn finish_carrier_load(generation: u64) -> bool {
    // `stage` and this commit both update PHASE. Serialize them so a late finish for G cannot
    // overwrite the Staged phase for a newer G+1 snapshot that arrived while G was loading.
    let _stage_guard = STAGE_LOCK.lock();
    if SERVING_GENERATION.load(Ordering::Acquire) != generation || !all_files_served(generation) {
        return false;
    }
    // Wait for callbacks from *every* older generation, not merely the immediate PREVIOUS
    // snapshot. A failed/rolled-back transition can leave an older callback alive while a later
    // snapshot is being prepared; Ready/Idle G is the cleanup watermark only once all callbacks
    // with generation < G have returned.
    if callbacks_before_in_flight(generation) != 0 {
        return false;
    }
    PREVIOUS.lock().take();
    let has_newer_pending = PENDING
        .lock()
        .as_ref()
        .is_some_and(|pending| pending.generation > generation);
    if has_newer_pending {
        set_phase(AssetBundlePhase::Staged);
    } else if SNAPSHOT.lock().files.is_empty() {
        set_phase(AssetBundlePhase::Idle);
    } else {
        set_phase(AssetBundlePhase::Ready);
    }
    true
}

/// Mark a native carrier failure without rolling back the callback view while the owner may still
/// be tearing down. `SNAPSHOT` and `PREVIOUS` remain paired until a later, safe generation begins;
/// swapping them here would let late callbacks mix two model/resource graphs. A failed pending
/// generation is dropped so a persistent slot/resource error is reported once and waits for the
/// desktop to submit a newer generation.
pub fn fail_carrier_load(generation: u64, reason: &str) {
    let _stage_guard = STAGE_LOCK.lock();
    let should_restore = SERVING_GENERATION.load(Ordering::Acquire) == generation;
    if !should_restore {
        // `begin_carrier_load` leaves PENDING intact when registration fails, and the state-0
        // guards call this function for occupied/native-conflict cases before beginning a load.
        // Drop only the failed generation; a newer desktop snapshot must survive untouched.
        let mut pending = PENDING.lock();
        if pending
            .as_ref()
            .is_some_and(|pending| pending.generation == generation)
        {
            pending.take();
        }
    }
    let has_newer_pending = PENDING
        .lock()
        .as_ref()
        .is_some_and(|pending| pending.generation > generation);
    set_phase(if has_newer_pending {
        AssetBundlePhase::Staged
    } else {
        AssetBundlePhase::Failed
    });
    crate::rust_extender::debuggable_server::notify_asset_bundle_error(reason);
}

/// Called once per frame on the game thread. It hands a pending snapshot to the native carrier
/// state machine; all actual native calls remain in `effect_reload::pump_asset_carrier`.
pub fn pump_game_thread() {
    let Some(generation) = pending_carrier_generation() else {
        return;
    };
    let phase = phase_from_u8(PHASE.load(Ordering::Acquire));
    if matches!(phase, AssetBundlePhase::Staged | AssetBundlePhase::Failed) {
        super::effect_reload::request_asset_carrier_reload(generation);
        if phase == AssetBundlePhase::Staged {
            set_phase(AssetBundlePhase::Retiring);
        }
    }
}

/// Current lifecycle phase for diagnostics and the desktop acknowledgement.
pub fn phase() -> AssetBundlePhase {
    phase_from_u8(PHASE.load(Ordering::Acquire))
}

/// Emit staged/served progress from the normal once-per-frame game-thread pump.
pub fn pump_status() {
    // Queue the handoff before reporting this frame's state. No native call happens here; the
    // actual owner transition is driven by the fighter callback on the game thread.
    pump_game_thread();
    let generation = GENERATION.load(Ordering::Acquire);
    let serving_generation = SERVING_GENERATION.load(Ordering::Acquire);
    let staged = PENDING
        .try_lock()
        .and_then(|pending| {
            pending
                .as_ref()
                .map(|p| p.files.values().filter(|file| file.gated).count() as u64)
        })
        .unwrap_or_else(|| STAGED_COUNT.load(Ordering::Acquire));
    let served = {
        let Some(served) = SERVED.try_lock() else {
            return;
        };
        served
            .iter()
            .filter(|(served_generation, _)| *served_generation == serving_generation)
            .count() as u64
    };
    let packed =
        generation.rotate_left(17) ^ (serving_generation.rotate_left(3)) ^ (staged << 32) ^ served;
    static HEARTBEAT: AtomicU64 = AtomicU64::new(0);
    let heartbeat = HEARTBEAT.fetch_add(1, Ordering::Relaxed) % 120 == 0;
    let dirty = STATUS_DIRTY.swap(false, Ordering::AcqRel);
    if !dirty && LAST_REPORT.swap(packed, Ordering::Relaxed) == packed && !heartbeat {
        return;
    }
    LAST_REPORT.store(packed, Ordering::Relaxed);
    let target = PENDING
        .try_lock()
        .and_then(|pending| pending.as_ref().map(|snapshot| snapshot.target.clone()))
        .or_else(|| SNAPSHOT.try_lock().map(|snapshot| snapshot.target.clone()))
        .unwrap_or_default();
    crate::rust_extender::debuggable_server::notify_asset_bundle_status(
        &target,
        generation,
        staged as usize,
        served as usize,
        serving_generation,
        phase().as_str(),
        phase() == AssetBundlePhase::Ready && generation == serving_generation,
    );
}

/// Force a fresh status after an editor connects; earlier reports had no socket consumer.
pub fn reset_status_latch() {
    LAST_REPORT.store(u64::MAX, Ordering::Relaxed);
    STATUS_DIRTY.store(true, Ordering::Release);
}
