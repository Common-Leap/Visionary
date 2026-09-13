//! Transfer carrier-loaded native resources to the fighter's existing modules (13.0.4).
//!
//! The carrier is retired before binding. Native shared ownership pins the model and motion
//! resources across its destruction; no ARC resident-file pointers or module accessors are swapped.
use super::native_shared::SharedHandle;
use parking_lot::Mutex;
use smash::app::{lua_bind::*, BattleObjectModuleAccessor};
use smash::phx::Hash40;

#[repr(transparent)]
#[derive(Default)]
struct Shared(SharedHandle);
impl Shared {
    unsafe fn retain(address: usize) -> Self {
        Self((&*(address as *const SharedHandle)).retain())
    }
    fn valid(&self) -> bool {
        self.0.valid()
    }
}
impl Drop for Shared {
    fn drop(&mut self) {
        unsafe {
            std::mem::take(&mut self.0).release(release_native_weak);
        }
    }
}
#[skyline::from_offset(0x39c20c0)]
fn release_weak(control: usize);
unsafe extern "C" fn release_native_weak(control: usize) {
    release_weak(control);
}

// Helpers are parsed by MotionModule, not owned by the render model. Keep their
// ARC files alive across carrier destruction using the filesystem's locked ref API.
struct ResidentFile {
    index: u32,
}
#[skyline::from_offset(0x3540450)]
fn retain_file(filesystem: usize, index: u32);
#[skyline::from_offset(0x3540560)]
fn release_file(filesystem: usize, index: u32);
impl ResidentFile {
    unsafe fn pin(path: &str) -> Result<Self, String> {
        let state = super::resource_reload::resident_file_state(smash::hash40(path))
            .ok_or_else(|| format!("model support file is missing: {path}"))?;
        if state.data == 0 || !state.filepath_loaded || !state.is_used {
            return Err(format!("model support file is not resident: {path}"));
        }
        retain_file(
            *((text_base() + 0x5331f20) as *const usize),
            state.filepath_index,
        );
        Ok(Self {
            index: state.filepath_index,
        })
    }
}
impl Drop for ResidentFile {
    fn drop(&mut self) {
        unsafe {
            release_file(*((text_base() + 0x5331f20) as *const usize), self.index);
        }
    }
}

// The native motion cache also owns a strong reference. A plain shared-pointer release
// would leave its last cached entry resident forever, preventing the next asset reload.
struct MotionResource {
    handle: Shared,
    cached: bool,
    folder: Option<u32>,
    support: Vec<ResidentFile>,
}
#[skyline::from_offset(0x496210)]
fn unregister_motion(cache: usize, handle: *mut Shared) -> u32;
impl Drop for MotionResource {
    fn drop(&mut self) {
        if self.cached && self.handle.valid() {
            unsafe {
                let cache = *((text_base() + 0x52b7a30) as *const usize);
                unregister_motion(cache, &mut self.handle);
            }
        }
    }
}
unsafe fn retain_motion(module: usize, folder: Option<u32>) -> MotionResource {
    MotionResource {
        handle: Shared::retain(module + 0x148),
        cached: *((module + 0x256) as *const u8) != 0,
        folder,
        support: Vec::new(),
    }
}

fn model_folder(path: &str) -> Option<u32> {
    super::resource_reload::hash_to_index_for_path_hash(smash::hash40(path))
}
unsafe fn fighter_model_path(host: *mut BattleObjectModuleAccessor) -> Option<String> {
    let fighter = crate::slight::slight_consts::fighters::game_kind_name(
        smash::app::utility::get_kind(&mut *host),
    )?;
    let color = WorkModule::get_int(
        host,
        *smash::lib::lua_const::FIGHTER_INSTANCE_WORK_ID_INT_COLOR,
    );
    Some(format!("fighter/{fighter}/model/body/c{color:02}"))
}

// Native render queue helpers used by ModelModule::start_module/end_module. Shared
// ownership keeps a model alive but does not keep it registered after its owner retires.
#[skyline::from_offset(0x3559220)]
fn register_render_model(handle: *const Shared);
#[skyline::from_offset(0x35c22e0)]
fn create_render_model(handle: *mut Shared, path: *const u32, render_mode: u32);
#[skyline::from_offset(0x35d6f40)]
fn create_mesh_visibility_buffer(model: usize);
#[skyline::from_offset(0x3559340)]
fn unregister_render_model(handle: *const Shared);

struct Pending {
    host: usize,
    id: u32,
    model: Shared,
    motion: MotionResource,
}
struct Original {
    host: usize,
    id: u32,
    model: Shared,
    motion: MotionResource,
    owns_cache: u8,
    physics_options: (bool, bool),
    visibility_defaults: Vec<(u64, bool)>,
    _preview_motion: MotionResource,
}
static OBSERVE_FRAME: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(600);
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);
static ORIGINAL: Mutex<Option<Original>> = Mutex::new(None);
static RESTORE_TRACE: Mutex<Option<(usize, u32, u32)>> = Mutex::new(None);

pub unsafe fn trace_restored_frame(host: *mut BattleObjectModuleAccessor) {
    let mut pending = RESTORE_TRACE.lock();
    let Some((address, id, tick)) = *pending else {
        return;
    };
    if !smash::app::sv_battle_object::is_active(id) {
        *pending = None;
        return;
    }
    if !same_host(host, address, id) {
        return;
    }
    *pending = (tick < 59).then_some((address, id, tick + 1));
    drop(pending);
    if matches!(tick, 0 | 1 | 5 | 30 | 59) {
        trace_visibility(host, &format!("restored_tick_{tick}"), false);
    }
}

unsafe fn module(host: *mut BattleObjectModuleAccessor, offset: usize) -> usize {
    *((host as usize + offset) as *const usize)
}
unsafe fn method(module: usize, offset: usize) -> usize {
    *((*(module as *const usize) + offset) as *const usize)
}
fn text_base() -> usize {
    unsafe { skyline::hooks::getRegionAddress(skyline::hooks::Region::Text) as usize }
}
unsafe fn validate(host: *mut BattleObjectModuleAccessor) -> Result<(), String> {
    if host.is_null() {
        return Err("fighter resource owner is missing".into());
    }
    // Check the native implementation, not just non-null pointers, before invoking private ABI.
    for (field, slot, expected) in [
        (0x78, 0x430, 0x491e00),  // ModelModule: rebuild mesh visibility cache
        (0x88, 0x60, 0x498eb0),   // MotionModule: connect animation output to model
        (0x88, 0x70, 0x499480),   // MotionModule: shared motion-resource setter
        (0x88, 0x40, 0x6dee80),   // FighterMotionModule: end old motion
        (0x88, 0x48, 0x6defa0),   // FighterMotionModule: bind animation controllers
        (0x88, 0x290, 0x6dfb30),  // FighterMotionModule: destroy controllers and extra tracks
        (0x80, 0x38, 0x4a5630),   // PhysicsModule: bind motion-owned swing resources
        (0x150, 0x130, 0x4e2760), // VisibilityModule: rebuild model/group mappings
        (0x150, 0x138, 0x4e37a0), // VisibilityModule: live per-mesh masks
        (0x150, 0x140, 0x4e37b0), // VisibilityModule: assign one mesh mask
        (0x80, 0x40, 0x4a6110),   // PhysicsModule: detach old physics controllers
    ] {
        let m = module(host, field);
        if m == 0 || *(m as *const usize) == 0 || method(m, slot) != text_base() + expected {
            return Err(format!("fighter resource binding is unsupported by this native module ({field:#x}/{slot:#x})"));
        }
    }
    let animcmd = module(host, 0x188);
    if animcmd == 0 || *(animcmd as *const usize) == 0 {
        return Err("fighter animation script module is missing".into());
    }
    if !(*((module(host, 0x78) + 0x10) as *const Shared)).valid()
        || !(*((module(host, 0x88) + 0x148) as *const Shared)).valid()
    {
        return Err("fighter native resources are not initialized".into());
    }
    // Alternate/copy motion resources override the primary descriptor. Do not leave a mixed graph.
    if *((module(host, 0x88) + 0x158) as *const usize) != 0 {
        return Err("return the fighter to its normal form before reloading model assets".into());
    }
    Ok(())
}

/// Capture strong native handles while their constructor owner is still alive. No fighter writes.
pub unsafe fn capture(
    host: *mut BattleObjectModuleAccessor,
    carrier: *mut BattleObjectModuleAccessor,
) -> Result<(), String> {
    validate(host)?;
    if ORIGINAL.lock().is_some() || PENDING.lock().is_some() {
        return Err("previous fighter resource handoff has not finished retiring".into());
    }
    let source_model = Shared::retain(module(carrier, 0x78) + 0x10);
    if !source_model.valid() {
        return Err("carrier model resources are not ready".into());
    }
    let original_model = *((module(host, 0x78) + 0x10) as *const usize);
    if method(original_model, 0x240) != text_base() + 0x35d4800
        || method(original_model, 0x1b0) != text_base() + 0x35d44d0
        || method(original_model, 0x188) != text_base() + 0x35d4300
        || method(original_model, 0x180) != text_base() + 0x35d3fd0
    {
        return Err("fighter render-model layout is unsupported".into());
    }
    let model_path = super::resource_reload::hash_to_index_for_path_hash(smash::hash40(
        "assist/alucard/model/body/c00/model.numdlb",
    ))
    .ok_or("carrier model descriptor is missing from native search tables")?;
    // ModelModule retirement disables the instance's render lifetime state even when strong
    // references retain its storage. Build a separate instance from the already resident ARC
    // graph, with the fighter's render mode, before the carrier can retire its own instance.
    let render_mode = *((original_model + 0x9c) as *const u32);
    let mut model = Shared::default();
    create_render_model(&mut model, &model_path, render_mode);
    if !model.valid() || method(model.0.object, 0x180) != text_base() + 0x35d3fd0 {
        return Err("carrier render-model visibility layout is unsupported".into());
    }
    // The regular render initializer only allocates this buffer when the model has
    // model-level visibility animation data. Models without those tracks still need
    // it when fighter gameplay selects meshes through VisibilityModule: the native
    // mesh setter otherwise returns false without changing the render flags, while
    // ModelModule caches the requested state as though the write succeeded.
    let visibility_buffer = (model.0.object + 0x438) as *const usize;
    let missing_visibility_buffer = *visibility_buffer == 0;
    if missing_visibility_buffer {
        create_mesh_visibility_buffer(model.0.object);
    }
    if *visibility_buffer == 0 {
        return Err("carrier model mesh visibility buffer is not ready".into());
    }
    super::effect_reload::mark(&format!(
        "fighter_assets mesh visibility buffer={:#x} allocated={missing_visibility_buffer}",
        *visibility_buffer
    ));
    // Native +0x48 resolves model.nuhlpb (0xce82acb1c) and model.nuanmb
    // (0xc69ca3ce4) here. A motion directory silently omits helper-driven custom bones.
    let folder = model_folder("assist/alucard/model/body/c00")
        .ok_or("carrier model helper directory is missing")?;
    let mut motion = retain_motion(module(carrier, 0x88), Some(folder));
    for file in ["model.nuhlpb", "model.nuanmb"] {
        motion.support.push(ResidentFile::pin(&format!(
            "assist/alucard/model/body/c00/{file}"
        ))?);
    }
    if !model.valid() || !motion.handle.valid() {
        return Err("carrier native model/motion resources are not ready".into());
    }
    if !MotionModule::is_anim_resource(carrier, Hash40::new_raw(MotionModule::motion_kind(host))) {
        return Err("uploaded motion graph does not contain the fighter's current motion".into());
    }
    *PENDING.lock() = Some(Pending {
        host: host as usize,
        id: (*host).battle_object_id,
        model,
        motion,
    });
    Ok(())
}

unsafe fn same_host(host: *mut BattleObjectModuleAccessor, expected: usize, id: u32) -> bool {
    !host.is_null()
        && host as usize == expected
        && (*host).battle_object_id == id
        && smash::app::sv_battle_object::is_active(id)
        && smash::app::utility::get_category(&mut *host)
            == *smash::lib::lua_const::BATTLE_OBJECT_CATEGORY_FIGHTER
}

// Rebinding a visual graph must not restart the fighter's ACMD coroutines mid-move. The native
// change_motion routine calls this module through its virtual table, bypassing lua_bind hooks.
// Substitute only the two script-switch entries for the synchronous rebind, then restore the
// original table before returning to the game. All script execution/state remains untouched.
unsafe extern "C" fn keep_scripts(_: usize, _: Hash40, _: f32, _: bool, _: bool, _: f32, _: bool) {}
struct ScriptTableGuard {
    module: usize,
    original: usize,
}
impl Drop for ScriptTableGuard {
    fn drop(&mut self) {
        unsafe {
            *(self.module as *mut usize) = self.original;
        }
    }
}

// Mesh order changes in imported models. Carry visibility by native mesh-name hash,
// never by the old mesh index, and leave genuinely new custom meshes at their own defaults.
unsafe fn mesh_visibility(model: usize) -> Vec<(u64, bool)> {
    let names: unsafe extern "C" fn(usize) -> *const usize =
        std::mem::transmute(method(model, 0x1b0));
    let visible: unsafe extern "C" fn(usize, i32) -> bool =
        std::mem::transmute(method(model, 0x188));
    let range = names(model);
    let begin = *range;
    let end = *range.add(1);
    (0..(end - begin) / 8)
        .map(|index| {
            (
                *((begin + index * 8) as *const u64),
                visible(model, index as i32),
            )
        })
        .collect()
}

// The renderer has an independent override in bits 2..3 of each mesh entry's
// flags. Its ordinary visibility getter only reports bits 0..1. Preserve the
// override separately: otherwise an animation-visible, render-hidden alternate
// becomes visible when a fresh render instance replaces the fighter's model.
unsafe fn mesh_render_flags(model: usize) -> Vec<(u64, u8)> {
    if method(model, 0x1b8) != text_base() + 0x35d44e0 {
        return Vec::new();
    }
    let descriptor = *((model + 0x438) as *const usize);
    let names = *((model + 0x488) as *const usize);
    if descriptor == 0 || names == 0 {
        return Vec::new();
    }
    let count = *((descriptor + 0x18) as *const u32) as usize;
    let buffers = *((descriptor + 0x1c) as *const u32) as usize;
    let current = *((descriptor + 0x20) as *const i32);
    let begin = *(descriptor as *const usize);
    let end = *((descriptor + 8) as *const usize);
    let nodes = *((names + 0x28) as *const usize);
    if count > 4096
        || nodes > 4096
        || buffers > 8
        || current < 0
        || current as usize >= buffers
        || begin == 0
        || end < begin
        || (end - begin) / 8 < buffers
    {
        return Vec::new();
    }
    let data = *((begin + current as usize * 8) as *const usize);
    if data == 0 {
        return Vec::new();
    }
    let mut node = *((names + 0x20) as *const usize);
    let mut result = Vec::new();
    for _ in 0..nodes {
        if node == 0 {
            break;
        }
        let index = *((node + 0x18) as *const u32) as usize;
        if index < count {
            result.push((
                *((node + 0x10) as *const u64),
                *((data + index) as *const u8),
            ));
        }
        node = *(node as *const usize);
    }
    result
}

// Native hash-based setter updates every submesh while preserving animation bits.
#[skyline::from_offset(0x35e23b0)]
fn set_mesh_render_override(model: usize, name: u64, state: u8) -> bool;

unsafe fn restore_mesh_visibility(model: usize, state: &[(u64, bool)]) {
    let set: unsafe extern "C" fn(usize, u64, bool) -> bool =
        std::mem::transmute(method(model, 0x180));
    for &(name, visible) in state {
        // The native name lookup returns false for a mesh absent from this model.
        set(model, name, visible);
    }
}

#[skyline::from_offset(0x4e3960)]
fn refresh_visibility_groups(module: usize, force: bool);

#[skyline::from_offset(0x3680960)]
fn bind_visibility_output(controller: usize, config: *const [usize; 4]) -> bool;

unsafe fn visibility_controller(motion: usize) -> usize {
    let channel = *((motion + 0x18) as *const usize);
    if channel == 0 {
        return 0;
    }
    let begin = *((channel + 0x88) as *const usize);
    let end = *((channel + 0x90) as *const usize);
    if begin == end {
        0
    } else {
        *(begin as *const usize)
    }
}

#[repr(C)]
struct VisibilityOutput {
    count: usize,
    values: *const u32,
}

/// Bounded, read-only evidence at each resource-lifecycle boundary. Keep this separate
/// from the pose log so Stop Preview does not erase the pre-load baseline.
pub unsafe fn trace_visibility(host: *mut BattleObjectModuleAccessor, phase: &str, reset: bool) {
    use std::io::Write;
    if validate(host).is_err() {
        return;
    }
    if reset {
        *RESTORE_TRACE.lock() = None;
    }
    let motion = module(host, 0x88);
    let model_module = module(host, 0x78);
    let model = *((model_module + 0x10) as *const usize);
    if model == 0
        || method(model, 0x188) != text_base() + 0x35d4300
        || method(model, 0x1b0) != text_base() + 0x35d44d0
        || method(motion, 0x4f0) != text_base() + 0x49f650
    {
        return;
    }
    let get: unsafe extern "C" fn(usize, u32) -> VisibilityOutput =
        std::mem::transmute(method(motion, 0x4f0));
    let output = get(motion, 0);
    let meshes = mesh_visibility(model);
    let visibility = module(host, 0x150);
    let masks = capture_visibility_masks(visibility, &meshes);
    let render_flags = mesh_render_flags(model);
    let cache = *((model_module + 0xc0) as *const *const u64);
    let cache_count = *((model_module + 0xc8) as *const usize);
    let mut report = format!(
        "build={} phase={phase} fighter={:#x} model={model:#x} motion={:#x} frame={} mesh_count={} output_count={} output_ptr={:p} cache_count={cache_count} mode={} flags={:?}\n",
        super::live_eff::BUILD_TAG, (*host).battle_object_id,
        MotionModule::motion_kind(host), MotionModule::frame(host), meshes.len(),
        output.count, output.values, *((visibility + 0x48) as *const u32),
        std::slice::from_raw_parts((motion + 0x24f) as *const u8, 4),
    );
    for (index, (name, rendered)) in meshes.iter().enumerate().take(4096) {
        let animated = if !output.values.is_null() && output.count <= 4096 && index < output.count {
            Some(*output.values.add(index))
        } else {
            None
        };
        let cached = if !cache.is_null() && cache_count <= 4096 && index < cache_count {
            Some((*cache.add(index / 64) >> (index % 64)) & 1)
        } else {
            None
        };
        report.push_str(&format!(
            "mesh={index} name={name:#012x} animation={animated:?} mask={:?} cache={cached:?} rendered={rendered} render_flags={:02x?}\n",
            masks.get(index).map(|(_, mask)| *mask),
            render_flags.iter().filter(|(hash, _)| hash == name).map(|(_, flags)| *flags).collect::<Vec<_>>(),
        ));
    }
    for name in [
        "haver", "havel", "sword1", "shield", "shieldb", "hilt", "scabbard", "trans",
    ] {
        let mut position = smash::phx::Vector3f {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        ModelModule::joint_global_position(host, Hash40::new(name), &mut position, false);
        report.push_str(&format!(
            "joint={name} world=({:.5},{:.5},{:.5})\n",
            position.x, position.y, position.z
        ));
    }
    let physics = module(host, 0x80);
    let count = *((physics + 0x10) as *const u32) as usize;
    let slots = *((physics + 0x18) as *const usize);
    if slots != 0 && count <= 256 {
        for slot in 0..count {
            let row = slots + slot * 0x40;
            report.push_str(&format!(
                "ik_slot={slot} names={:x?} weight={}\n",
                *((row) as *const [u64; 2]),
                *((row + 0x30) as *const f32)
            ));
        }
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(!reset)
        .truncate(reset)
        .open("sd:/effect_viewer_visibility_trace.txt")
    {
        let _ = file.write_all(report.as_bytes());
    }
}

unsafe fn visibility_masks(module: usize) -> *const usize {
    let get: unsafe extern "C" fn(usize) -> *const usize =
        std::mem::transmute(method(module, 0x138));
    get(module)
}

unsafe fn capture_visibility_masks(visibility: usize, meshes: &[(u64, bool)]) -> Vec<(u64, u8)> {
    let range = visibility_masks(visibility);
    let begin = *range;
    let count = *range.add(1) - begin;
    meshes
        .iter()
        .take(count)
        .enumerate()
        .map(|(index, &(name, _))| (name, *((begin + index) as *const u8)))
        .collect()
}

unsafe fn bind_visibility_groups(visibility: usize, model: *const Shared, masks: &[(u64, u8)]) {
    // Rebuild the native group -> variant -> mesh-index maps for this instance.
    // The fighter's group selections, status defaults, locks, and event subscriptions
    // stay in its existing VisibilityModule and continue to receive gameplay changes.
    let bind: unsafe extern "C" fn(usize, *const Shared) =
        std::mem::transmute(method(visibility, 0x130));
    bind(visibility, model);
    let names = mesh_visibility((*model).0.object);
    let state = super::fighter_visibility::final_masks_after_motion_restore(
        masks,
        names.iter().map(|&(name, _)| name),
    );
    let set: unsafe extern "C" fn(usize, i32, u8) = std::mem::transmute(method(visibility, 0x140));
    for (index, mask) in state.into_iter().enumerate() {
        set(visibility, index as i32, mask);
    }
    // Recalculate current group choices now; subsequent native updates use the new maps.
    refresh_visibility_groups(visibility, true);
    // Match the native selection setter's dirty flags so the normal frame pipeline
    // publishes the rebuilt masks even if the selected group name has not changed.
    *((visibility + 0x80) as *mut u8) = 1;
    *((visibility + 0x82) as *mut u8) = 1;
    super::effect_reload::mark("fighter_assets rebound live visibility groups");
}

unsafe fn bind(
    host: *mut BattleObjectModuleAccessor,
    model: Shared,
    motion: &MotionResource,
    retire_outgoing: bool,
    physics_options: (bool, bool),
    visibility_defaults: &[(u64, bool)],
) -> Shared {
    let model_module = module(host, 0x78);
    let motion_module = module(host, 0x88);
    let physics = module(host, 0x80);
    let visible = ModelModule::is_visible(host);
    let mesh_state = mesh_visibility(*((model_module + 0x10) as *const usize));
    let render_overrides = super::fighter_visibility::render_overrides(&mesh_render_flags(
        *((model_module + 0x10) as *const usize),
    ));
    let visibility = module(host, 0x150);
    let visibility_state = capture_visibility_masks(visibility, &mesh_state);
    let kind = MotionModule::motion_kind(host);
    let frame = MotionModule::frame(host);
    let rate = MotionModule::rate(host);
    let second = MotionModule::motion_kind_2nd(host);
    let second_frame = MotionModule::frame_2nd(host);
    let second_rate = MotionModule::rate_2nd(host);
    let weight = MotionModule::weight(host);
    let partial_count = *((motion_module + 0x24b) as *const u8) as i32;
    let partials: Vec<_> = (0..partial_count)
        .map(|slot| {
            (
                slot,
                MotionModule::motion_kind_partial(host, slot),
                MotionModule::frame_partial(host, slot),
                MotionModule::rate_partial(host, slot),
            )
        })
        .collect();
    // +0x18 / +0x24f is visibility; +0x20 / +0x251 is material.
    // Each can also exist solely for model-level tracks (+0x250 / +0x252).
    let has_visibility = *((motion_module + 0x24f) as *const u8) != 0
        || *((motion_module + 0x250) as *const u8) != 0;
    let has_material = *((motion_module + 0x251) as *const u8) != 0
        || *((motion_module + 0x252) as *const u8) != 0;
    let (physics_extra, physics_cloth) = physics_options;
    // These slots belong to the fighter, while their named constraints are recreated.
    // Preserve active IK weights and targets without retaining controller pointers.
    let slot_count = *((physics + 0x10) as *const u32) as usize;
    let slots = *((physics + 0x18) as *const usize);
    let ik_state: Vec<_> = if slots == 0 {
        Vec::new()
    } else {
        (0..slot_count)
            .map(|slot| {
                let record = slots + slot * 0x40;
                (
                    *((record) as *const [u64; 2]),
                    *((record + 0x10) as *const [u8; 0x20]),
                    *((record + 0x30) as *const f32),
                )
            })
            .collect()
    };

    let animcmd = module(host, 0x188);
    let original_table = *(animcmd as *const usize);
    // Preserve the C++ offset-to-top / RTTI prefix as well as every method slot.
    let mut script_table = [0usize; 2 + 0xe8 / 8];
    std::ptr::copy_nonoverlapping(
        (original_table - 0x10) as *const usize,
        script_table.as_mut_ptr(),
        script_table.len(),
    );
    script_table[2 + 0x68 / 8] = keep_scripts as *const () as usize;
    script_table[2 + 0x70 / 8] = keep_scripts as *const () as usize;
    *(animcmd as *mut usize) = script_table.as_ptr().add(2) as usize;
    let _scripts = ScriptTableGuard {
        module: animcmd,
        original: original_table,
    };

    let detach_physics: unsafe extern "C" fn(usize) = std::mem::transmute(method(physics, 0x40));
    super::effect_reload::mark("fighter_assets detach physics/motion");
    detach_physics(physics);
    // end_module disables the old constraints but leaves their hashes in the fighter's
    // persistent 0x40-byte slot records. start_module only writes slots present in the
    // incoming graph. Clear both constraint hashes after detaching the old controller,
    // as PhysicsModule::initialize does, so later cleanup cannot look up an old hash
    // in the replacement controller and call through its missing-constraint sentinel.
    let slot_count = *((physics + 0x10) as *const u32) as usize;
    let slots = *((physics + 0x18) as *const usize);
    if slots != 0 {
        for slot in 0..slot_count {
            let record = (slots + slot * 0x40) as *mut u64;
            record.write(0);
            record.add(1).write(0);
        }
    }
    super::effect_reload::mark("fighter_assets physics detached; constraint slots cleared");
    let end_motion: unsafe extern "C" fn(usize) = std::mem::transmute(method(motion_module, 0x40));
    end_motion(motion_module);
    let destroy_controllers: unsafe extern "C" fn(usize) =
        std::mem::transmute(method(motion_module, 0x290));
    destroy_controllers(motion_module);

    // Move strong ownership into the FIGHTER'S existing ModelModule. The saved original remains
    // alive for Stop/reload, and the carrier can no longer render or advance this instance.
    // Retained original render resources must not stay registered as a visible old mesh.
    // Visibility returns on the same fighter immediately after the binding changes.
    super::effect_reload::mark("fighter_assets bind model");
    ModelModule::set_visibility(host, false);
    // The original is only hidden while retained for Stop: unregistering it also invalidates
    // render lifetime state. Only the outgoing preview instance is retired permanently.
    if retire_outgoing {
        unregister_render_model((model_module + 0x10) as *const Shared);
    }
    let original = std::ptr::replace((model_module + 0x10) as *mut Shared, model);
    let rebuild_mesh_cache: unsafe extern "C" fn(usize) =
        std::mem::transmute(method(model_module, 0x430));
    rebuild_mesh_cache(model_module);
    bind_visibility_groups(
        visibility,
        (model_module + 0x10) as *const Shared,
        &visibility_state,
    );
    let set_motion_resource: unsafe extern "C" fn(usize, *const Shared) =
        std::mem::transmute(method(motion_module, 0x70));
    set_motion_resource(motion_module, &motion.handle);
    // The fighter now participates in the native cache lifecycle for this descriptor.
    *((motion_module + 0x256) as *mut u8) = u8::from(motion.cached);
    let bind_motion: unsafe extern "C" fn(
        usize,
        *const Shared,
        *const u32,
        bool,
        bool,
        bool,
        bool,
    ) = std::mem::transmute(method(motion_module, 0x48));
    super::effect_reload::mark("fighter_assets bind fighter animation controllers");
    bind_motion(
        motion_module,
        (model_module + 0x10) as *const Shared,
        motion
            .folder
            .as_ref()
            .map_or(std::ptr::null(), |folder| folder as *const u32),
        has_visibility,
        has_material,
        true,
        false,
    );
    // Use the ARC FilePath index directly; directory search indices are a separate
    // namespace and a failed lookup otherwise leaves a silently unbound helper.
    let helper = helper_controller(motion_module);
    if helper != 0 {
        if let Some(file) = motion.support.first() {
            let bind_helper: unsafe extern "C" fn(usize, *const u32) =
                std::mem::transmute(method(helper, 0xb0));
            bind_helper(helper, &file.index);
        }
    }
    let bind_physics: unsafe extern "C" fn(usize, bool, bool, f32) =
        std::mem::transmute(method(physics, 0x38));
    super::effect_reload::mark("fighter_assets bind swing physics");
    bind_physics(physics, physics_extra, physics_cloth, 1.0);
    // Re-enroll this instance after the carrier destructor removed it from the render queue.
    // Registration is idempotent for the saved original, which stayed registered while hidden.
    register_render_model((model_module + 0x10) as *const Shared);
    // Controller construction and render registration are distinct from connecting the
    // animation output to the model's skeleton. Rebind that output after retirement cleared
    // the old model connections, using MotionModule's native model-replacement entry point.
    let bind_animation_output: unsafe extern "C" fn(usize, *const Shared) =
        std::mem::transmute(method(motion_module, 0x60));
    bind_animation_output(motion_module, (model_module + 0x10) as *const Shared);
    // +0x60 only reconnects skeletal output. Bind visibility against the registered
    // replacement as well, before restoring the primary/secondary animation tracks.
    // This native config contains a weak model handle and an optional default-mask
    // array. The callee locks the handle; ownership stays in ModelModule.
    let visibility_output = visibility_controller(motion_module);
    if visibility_output != 0 && *(visibility_output as *const usize) == text_base() + 0x5238590 {
        let handle = &*((model_module + 0x10) as *const Shared);
        let names = mesh_visibility(handle.0.object);
        let defaults = super::fighter_visibility::remap_defaults(
            visibility_defaults,
            names.iter().map(|(name, _)| *name),
        );
        let was_bound = *((visibility_output + 0x88) as *const u8);
        let bound = bind_visibility_output(
            visibility_output,
            &[
                handle.0.object,
                handle.0.control,
                defaults.len(),
                defaults.as_ptr() as usize,
            ],
        );
        super::effect_reload::mark(&format!(
            "fighter_assets visibility output was_bound={was_bound} rebound={bound} controller={visibility_output:#x}"
        ));
    }
    // New render instances start with their own visibility flags. Reapply the fighter's
    // live selection (including hidden weapon/prop variants) before seeding ModelModule's
    // cache. Existing ACMD visibility commands can then keep updating the real fighter.
    restore_mesh_visibility(*((model_module + 0x10) as *const usize), &mesh_state);
    for &(name, state) in &render_overrides {
        set_mesh_render_override(*((model_module + 0x10) as *const usize), name, state);
    }
    super::effect_reload::mark(&format!(
        "fighter_assets transferred {} named render overrides",
        render_overrides.len()
    ));
    rebuild_mesh_cache(model_module);
    super::effect_reload::mark(&format!(
        "fighter_assets restored {} named mesh visibility states",
        mesh_state.len()
    ));
    ModelModule::set_visibility(host, visible);
    super::effect_reload::mark("fighter_assets restore current motion");
    if MotionModule::is_anim_resource(host, Hash40::new_raw(kind)) {
        MotionModule::change_motion(
            host,
            Hash40::new_raw(kind),
            frame,
            rate,
            false,
            0.0,
            false,
            false,
        );
    }
    if second != 0 && MotionModule::is_anim_resource(host, Hash40::new_raw(second)) {
        MotionModule::add_motion_2nd(
            host,
            Hash40::new_raw(second),
            second_frame,
            second_rate,
            false,
            weight,
        );
        MotionModule::set_weight(host, weight, true);
    }
    for (slot, partial_kind, partial_frame, partial_rate) in partials {
        if partial_kind != 0 && MotionModule::is_anim_resource(host, Hash40::new_raw(partial_kind))
        {
            MotionModule::add_motion_partial(
                host,
                slot,
                Hash40::new_raw(partial_kind),
                partial_frame,
                partial_rate,
                false,
                false,
                0.0,
                false,
                false,
                false,
            );
        }
    }
    // Restoring the motion re-evaluates visibility animation on the fresh instance and can
    // reset its masks to animation defaults. Publish the captured live selection last so only
    // the correct weapon variants show (Hero hand vs back sword/shield both visible otherwise).
    // Native gameplay updates keep flowing afterwards through the rebound group maps.
    bind_visibility_groups(
        visibility,
        (model_module + 0x10) as *const Shared,
        &visibility_state,
    );
    for &(name, state) in &render_overrides {
        set_mesh_render_override(*((model_module + 0x10) as *const usize), name, state);
    }
    rebuild_mesh_cache(model_module);
    PhysicsModule::reset_swing(host);
    for (slot, (names, targets, weight)) in ik_state.iter().enumerate() {
        let record = slots + slot * 0x40;
        // A different uploaded IK graph may omit or repurpose a slot. Apply state
        // only when the native binder recreated the same named constraints.
        if *names != [0, 0] && *((record) as *const [u64; 2]) == *names {
            *((record + 0x10) as *mut [u8; 0x20]) = *targets;
            PhysicsModule::set_ik(host, slot as i32, *weight);
        }
    }
    let controller = *((motion_module + 0x10) as *const usize);
    super::effect_reload::mark(&format!(
        "fighter_assets binding complete motion={:#x} frame={} controller={:#x} output_model={:#x} bound={}",
        MotionModule::motion_kind(host), MotionModule::frame(host), controller,
        if controller != 0 { *((controller + 0x88) as *const usize) } else { 0 },
        if controller != 0 { *((controller + 0x98) as *const u8) } else { 0 },
    ));
    original
}

/// Called only after the exact carrier has finished native destruction.
pub unsafe fn apply(host: *mut BattleObjectModuleAccessor) -> Result<(), String> {
    let pending = PENDING
        .lock()
        .take()
        .ok_or("carrier resource handoff was not captured")?;
    if !same_host(host, pending.host, pending.id) {
        return Err("fighter changed during resource handoff".into());
    }
    validate(host)?;
    let motion_module = module(host, 0x88);
    trace_visibility(host, "before_handoff", false);
    let original_path =
        fighter_model_path(host).ok_or("fighter model directory could not be resolved")?;
    let original_folder = model_folder(&original_path)
        .ok_or("fighter model helper directory could not be resolved")?;
    let mut original_motion = retain_motion(motion_module, Some(original_folder));
    for file in ["model.nuhlpb", "model.nuanmb"] {
        if file == "model.nuanmb"
            && super::resource_reload::resident_file_state(smash::hash40(&format!(
                "{original_path}/{file}"
            )))
            .is_none()
        {
            continue;
        }
        original_motion
            .support
            .push(ResidentFile::pin(&format!("{original_path}/{file}"))?);
    }
    let owns_cache = *((motion_module + 0x256) as *const u8);
    let physics = module(host, 0x80);
    let physics_options = (
        *((physics + 0xcd) as *const u8) != 0,
        *((physics + 0xce) as *const u8) != 0,
    );
    let visibility_defaults = mesh_visibility(*((module(host, 0x78) + 0x10) as *const usize));
    let original_model = bind(
        host,
        pending.model,
        &pending.motion,
        false,
        physics_options,
        &visibility_defaults,
    );
    trace_visibility(host, "after_handoff", false);
    OBSERVE_FRAME.store(0, std::sync::atomic::Ordering::Relaxed);
    *ORIGINAL.lock() = Some(Original {
        host: host as usize,
        id: pending.id,
        model: original_model,
        motion: original_motion,
        owns_cache,
        physics_options,
        visibility_defaults,
        _preview_motion: pending.motion,
    });
    let helper = helper_controller(motion_module);
    if helper == 0
        || *((helper + 0xc8) as *const u32) == 0xffffff
        || *((helper + 0x140) as *const usize) == 0
    {
        restore(Some(host))?;
        return Err(
            "fighter helper constraints could not be bound; original model restored".into(),
        );
    }
    let visibility_output = visibility_controller(motion_module);
    if visibility_output != 0 && *((visibility_output + 0x88) as *const u8) == 0 {
        restore(Some(host))?;
        return Err(
            "fighter visibility animation output could not be bound; original model restored"
                .into(),
        );
    }
    Ok(())
}

pub unsafe fn restore(host: Option<*mut BattleObjectModuleAccessor>) -> Result<(), String> {
    PENDING.lock().take();
    let mut original = ORIGINAL.lock();
    if let Some(saved) = original.as_ref() {
        if let Some(host) = host.filter(|&host| same_host(host, saved.host, saved.id)) {
            validate(host)?;
            trace_visibility(host, "before_restore", false);
            let saved = original.take().unwrap();
            let outgoing_motion = retain_motion(module(host, 0x88), None);
            let outgoing = bind(
                host,
                saved.model,
                &saved.motion,
                true,
                saved.physics_options,
                &saved.visibility_defaults,
            );
            *((module(host, 0x88) + 0x256) as *mut u8) = saved.owns_cache;
            drop(outgoing);
            drop(outgoing_motion);
            super::effect_reload::mark("fighter_assets original fighter restored");
            trace_visibility(host, "after_restore", false);
            *RESTORE_TRACE.lock() = Some((host as usize, (*host).battle_object_id, 0));
            return Ok(());
        }
    }
    // The preview owned the fighter's ModelModule when it was destroyed. Its retained
    // original stayed registered while hidden, so retire that registration explicitly now.
    if let Some(saved) = original.take() {
        unregister_render_model(&saved.model);
    }
    Ok(())
}

unsafe fn helper_controller(motion: usize) -> usize {
    let controller = *((motion + 0x10) as *const usize);
    if controller == 0 {
        return 0;
    }
    let helper = *((controller + 0x68) as *const usize);
    if helper != 0 && *(helper as *const usize) == text_base() + 0x5238fa8 {
        helper
    } else {
        0
    }
}

/// Read-only, bounded samples distinguish motion timing from skeleton output after a handoff.
pub unsafe fn observe(host: *mut BattleObjectModuleAccessor) {
    use std::io::Write;
    use std::sync::atomic::Ordering;
    let tick = OBSERVE_FRAME.load(Ordering::Relaxed);
    if tick >= 600 {
        return;
    }
    OBSERVE_FRAME.store(tick + 1, Ordering::Relaxed);
    if tick % 30 != 0 {
        return;
    }
    trace_visibility(host, &format!("preview_tick_{tick}"), false);
    let model = module(host, 0x78);
    let motion = module(host, 0x88);
    let render_model = *((model + 0x10) as *const usize);
    let visibility = module(host, 0x150);
    let meshes = mesh_visibility(render_model);
    let masks = capture_visibility_masks(visibility, &meshes);
    let controller = *((motion + 0x10) as *const usize);
    let helper = if controller != 0 {
        *((controller + 0x68) as *const usize)
    } else {
        0
    };
    let mut joints = String::new();
    let visibility_output = visibility_controller(motion);
    joints.push_str(&format!(
        " visibility_controller={visibility_output:#x} visibility_bound={} channel_flags={:?}",
        if visibility_output != 0 {
            *((visibility_output + 0x88) as *const u8)
        } else {
            0
        },
        std::slice::from_raw_parts((motion + 0x24f) as *const u8, 4),
    ));
    joints.push_str(&format!(
        " visibility_buffer={:#x} meshes={} hidden={} masks_hidden={} masks_visible={} visibility_mode={}",
        *((render_model + 0x438) as *const usize), meshes.len(),
        meshes.iter().filter(|(_, visible)| !visible).count(),
        masks.iter().filter(|(_, mask)| *mask == 0).count(),
        masks.iter().filter(|(_, mask)| *mask == 1).count(),
        *((visibility + 0x48) as *const u32),
    ));
    if helper != 0 && *(helper as *const usize) == text_base() + 0x5238fa8 {
        let stage = *((helper + 0x58) as *const usize);
        joints.push_str(&format!(
            " helper_file={:#x} helper_data={:#x} helper_enabled={}",
            *((helper + 0xc8) as *const u32),
            *((helper + 0x140) as *const usize),
            if stage != 0 {
                *((stage + 0x114) as *const u8)
            } else {
                0
            }
        ));
    }
    for name in [
        "top",
        "head",
        "haver",
        "havel",
        "h_exo_head",
        "h_exo_arm_1_l",
    ] {
        let mut position = smash::phx::Vector3f {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        ModelModule::joint_global_position(host, Hash40::new(name), &mut position, false);
        joints.push_str(&format!(
            " {name}=({:.3},{:.3},{:.3})",
            position.x, position.y, position.z
        ));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(tick != 0)
        .truncate(tick == 0)
        .open("sd:/effect_viewer_fighter_pose.txt")
    {
        let _ = writeln!(file, "build={} tick={tick} kind={:#x} frame={} rate={} model={:#x} controller={controller:#x} output={:#x} bound={}{}",
            super::live_eff::BUILD_TAG, MotionModule::motion_kind(host), MotionModule::frame(host),
            MotionModule::rate(host), *((model + 0x10) as *const usize),
            if controller != 0 { *((controller + 0x88) as *const usize) } else { 0 },
            if controller != 0 { *((controller + 0x98) as *const u8) } else { 0 }, joints);
    }
}
