//! Version-pinned native Limit helpers, available only through exact actor checks.
//!
//! Graph actions use this boundary through the versioned resource capability.
//! The callback copies native frame registers; resource calls run later on the game
//! thread after identity, phase, memory and complete-body digest checks.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

use sha2::{Digest, Sha256};

use super::actor_identity::Identity;
use super::cloud_resource_model::{self as model, Snapshot};
use super::live_parameters::NativeReadCache;

pub(crate) use model::Command;

const NOT_REQUESTED: u8 = 0;
const INSTALLED: u8 = 1;
const REJECTED: u8 = 2;
const FRAME_OFFSET: usize = 0x8dba00;
const ADJUST_OFFSET: usize = 0x8dc160;
const CLEAR_OFFSET: usize = 0x8db970;
const ROWS: usize = 16;

// Complete-body SHA-256 witnesses from the reviewed 13.0.4 Main image.
// Game instructions and arbitrary addresses are not part of saved projects.
const FRAME_DIGEST: [u8; 32] = [
    0xa6, 0xbb, 0xe4, 0x98, 0xd8, 0x03, 0x11, 0xeb, 0x47, 0x12, 0xf9, 0x0d, 0xec, 0x29, 0x0b, 0x89,
    0xa6, 0xed, 0xdc, 0x55, 0x3d, 0x68, 0xfb, 0xc3, 0x12, 0xb8, 0xfe, 0x8e, 0xaf, 0x68, 0x78, 0x02,
];
const ADJUST_DIGEST: [u8; 32] = [
    0x75, 0xff, 0x2e, 0x77, 0xd3, 0xec, 0x60, 0xc6, 0x8c, 0xcf, 0xb4, 0x8f, 0xc6, 0x1a, 0xe2, 0x84,
    0x13, 0xe6, 0x9c, 0x11, 0x82, 0xfe, 0x7b, 0x8d, 0x73, 0xf3, 0xde, 0xc9, 0xe5, 0x72, 0x39, 0xc2,
];
const CLEAR_DIGEST: [u8; 32] = [
    0xcf, 0xac, 0x8f, 0xf8, 0x48, 0x08, 0xfd, 0x4e, 0x59, 0xe6, 0xb0, 0xe8, 0xd6, 0x54, 0xb7, 0x98,
    0xf2, 0x0f, 0x9d, 0xbc, 0x42, 0xac, 0x06, 0x00, 0x55, 0x3d, 0x3f, 0x5e, 0x1d, 0x81, 0xe2, 0x81,
];
const CLEANUP_DIGEST: [u8; 32] = [
    0xe0, 0xbd, 0x4d, 0xd4, 0xd8, 0x18, 0xce, 0x2e, 0xc2, 0x0e, 0x25, 0x87, 0x3e, 0x97, 0x45, 0xf2,
    0x46, 0xef, 0x73, 0x69, 0x1c, 0x67, 0x4e, 0xd2, 0x00, 0xb6, 0x1a, 0xec, 0x81, 0xd9, 0x7c, 0x3d,
];
const RESET_DIGEST: [u8; 32] = [
    0x65, 0xc7, 0x80, 0xc9, 0xba, 0x73, 0xbd, 0x2d, 0x82, 0x70, 0xc5, 0x01, 0x7c, 0x37, 0x4f, 0xa2,
    0x7a, 0x4d, 0xf2, 0xfc, 0x6a, 0x4e, 0xfb, 0xbf, 0x7a, 0xe4, 0xe5, 0xb1, 0xb9, 0xce, 0x1e, 0x37,
];

struct FrameRow {
    sequence: AtomicU64,
    pointer: AtomicU64,
    frame: AtomicU64,
    epoch: AtomicU64,
}
impl FrameRow {
    const fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            pointer: AtomicU64::new(0),
            frame: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
        }
    }
}
static FRAMES: [FrameRow; ROWS] = [const { FrameRow::new() }; ROWS];
static REQUESTED: AtomicBool = AtomicBool::new(false);
static FORCED_REQUEST: AtomicBool = AtomicBool::new(false);
static INSTALL_STATE: AtomicU8 = AtomicU8::new(NOT_REQUESTED);
static FRAME: AtomicU64 = AtomicU64::new(0);
static EPOCH: AtomicU64 = AtomicU64::new(1);
static NEXT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Request the internal binding before its first game-thread operation.
/// Idle plugins install no resource observation hook.
pub(crate) fn request() {
    FORCED_REQUEST.store(true, Ordering::Release);
    REQUESTED.store(true, Ordering::Release);
}

unsafe extern "C" fn capture_frame(ctx: &skyline::hooks::InlineCtx) {
    // No native reads/calls, logging, allocation, lock or register mutation.
    if !REQUESTED.load(Ordering::Acquire) {
        return;
    }
    let sequence = NEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    if sequence == 0 {
        return;
    }
    let row = &FRAMES[sequence as usize % ROWS];
    row.sequence.store(0, Ordering::Release);
    row.pointer.store(ctx.registers[1].x(), Ordering::Relaxed);
    row.frame
        .store(FRAME.load(Ordering::Relaxed), Ordering::Relaxed);
    row.epoch
        .store(EPOCH.load(Ordering::Relaxed), Ordering::Relaxed);
    row.sequence.store(sequence, Ordering::Release);
}

fn text_bounds() -> Result<(usize, usize), String> {
    let start = unsafe { skyline::hooks::getRegionAddress(skyline::hooks::Region::Text) as usize };
    let end = unsafe { skyline::hooks::getRegionAddress(skyline::hooks::Region::Rodata) as usize };
    if start == 0 || end <= start {
        return Err("native Limit Main text bounds are unavailable".into());
    }
    Ok((start, end))
}

fn matches<const N: usize>(
    memory: &mut NativeReadCache,
    text: usize,
    end: usize,
    offset: usize,
    digest: &[u8; 32],
) -> bool {
    let Some(address) = text.checked_add(offset) else {
        return false;
    };
    if address.checked_add(N).is_none_or(|limit| limit > end) {
        return false;
    }
    memory
        .read::<[u8; N]>(address)
        .is_some_and(|body| Sha256::digest(body)[..] == digest[..])
}

pub(crate) fn on_frame() {
    let demand =
        FORCED_REQUEST.load(Ordering::Acquire) || super::live_behavior::cloud_resource_demand();
    if REQUESTED.swap(demand, Ordering::AcqRel) != demand {
        reset_match();
    }
    if !demand {
        return;
    }
    FRAME.fetch_add(1, Ordering::Relaxed);
    if INSTALL_STATE.load(Ordering::Acquire) != NOT_REQUESTED {
        return;
    }
    if !crate::slight::agents::all_records()
        .iter()
        .any(|actor| Identity::from_live_fighter(actor.boid, "cloud").is_some())
    {
        return;
    }
    let Ok((text, end)) = text_bounds() else {
        return;
    };
    let mut memory = NativeReadCache::default();
    if !matches::<0x234>(&mut memory, text, end, FRAME_OFFSET, &FRAME_DIGEST) {
        INSTALL_STATE.store(REJECTED, Ordering::Release);
        crate::slight::diag::note(
            "native Limit resource rejected: complete 13.0.4 frame signature mismatch",
        );
        return;
    }
    unsafe {
        skyline::hooks::A64InlineHook(
            (text + FRAME_OFFSET) as *const skyline::libc::c_void,
            capture_frame as *const skyline::libc::c_void,
        );
    }
    let patched = memory
        .read::<u32>(text + FRAME_OFFSET)
        .is_some_and(|word| word & 0xfc00_0000 == 0x1400_0000);
    INSTALL_STATE.store(
        if patched { INSTALLED } else { REJECTED },
        Ordering::Release,
    );
    if !patched {
        crate::slight::diag::note("native Limit resource rejected: frame observation patch absent");
    }
}

pub(crate) fn reset_match() {
    // Installed native code remains valid; old match pointers cannot be reused.
    EPOCH.fetch_add(1, Ordering::AcqRel);
    for row in &FRAMES {
        row.sequence.store(0, Ordering::Release);
    }
}

pub(crate) fn clear() {
    FORCED_REQUEST.store(false, Ordering::Release);
    REQUESTED.store(false, Ordering::Release);
    reset_match();
}

/// Retire only this object's native frame witnesses on stock/reset boundaries.
/// Other fighters keep their own independent observations.
pub(crate) fn retire(boid: u32) {
    if !REQUESTED.load(Ordering::Acquire) {
        return;
    }
    let mut memory = NativeReadCache::default();
    for row in &FRAMES {
        let sequence = row.sequence.load(Ordering::Acquire);
        let pointer = row.pointer.load(Ordering::Relaxed) as usize;
        if sequence != 0
            && pointer.checked_add(8).and_then(|p| memory.read::<u32>(p)) == Some(boid)
            && row.sequence.load(Ordering::Acquire) == sequence
        {
            let _ = row
                .sequence
                .compare_exchange(sequence, 0, Ordering::AcqRel, Ordering::Acquire);
        }
    }
}

fn current(boid: u32, expected: Identity) -> Result<(), String> {
    if Identity::from_live_fighter(boid, "cloud") != Some(expected) {
        return Err("native Limit actor identity changed".into());
    }
    Ok(())
}

fn fighter_pointer(
    memory: &mut NativeReadCache,
    boid: u32,
    expected: Identity,
) -> Result<usize, String> {
    let frame = FRAME.load(Ordering::Acquire);
    let epoch = EPOCH.load(Ordering::Acquire);
    for row in &FRAMES {
        let sequence = row.sequence.load(Ordering::Acquire);
        if sequence == 0 {
            continue;
        }
        let at = row.frame.load(Ordering::Relaxed);
        let observed_epoch = row.epoch.load(Ordering::Relaxed);
        let pointer = row.pointer.load(Ordering::Relaxed) as usize;
        if pointer == 0 || at == 0 || at > frame || frame - at > 1 || observed_epoch != epoch {
            continue;
        }
        let Some(header) = memory.read::<smash::app::BattleObject>(pointer) else {
            continue;
        };
        if row.sequence.load(Ordering::Acquire) != sequence {
            continue;
        }
        if header.battle_object_id == boid
            && header.kind == expected.fighter_kind as u32
            && header.entry_id == expected.entry_id as u64
            && header.module_accessor as u64 == expected.module_accessor_addr
        {
            current(boid, expected)?;
            return Ok(pointer);
        }
    }
    Err("native Limit operation is waiting for a fresh exact fighter frame".into())
}

fn snapshot(boid: u32, expected: Identity) -> Result<Snapshot, String> {
    current(boid, expected)?;
    let p = expected.module_accessor_addr as *mut smash::app::BattleObjectModuleAccessor;
    use smash::app::lua_bind::{StatusModule, WorkModule};
    use smash::lib::lua_const::*;
    let before = unsafe { StatusModule::status_kind(p) };
    let value = unsafe {
        Snapshot {
            gauge: WorkModule::get_float(p, *FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE),
            remaining: WorkModule::get_int(
                p,
                *FIGHTER_CLOUD_INSTANCE_WORK_ID_INT_LIMIT_BREAK_CLEAR_FRAME,
            ),
            full: WorkModule::is_flag(p, *FIGHTER_CLOUD_INSTANCE_WORK_ID_FLAG_LIMIT_BREAK),
            set_custom: WorkModule::is_flag(
                p,
                *FIGHTER_CLOUD_INSTANCE_WORK_ID_FLAG_LIMIT_BREAK_SET_CUSTOM,
            ),
            special: WorkModule::is_flag(
                p,
                *FIGHTER_CLOUD_INSTANCE_WORK_ID_FLAG_LIMIT_BREAK_SPECIAL,
            ),
            // Native HUD's previous-gauge cell, confirmed by the reset/HUD trace.
            previous_gauge: WorkModule::get_float(
                p,
                *FIGHTER_CLOUD_INSTANCE_WORK_ID_FLOAT_LIMIT_GAUGE_NOTICE,
            ),
            status: before,
            changing: StatusModule::is_changing(p),
        }
    };
    if unsafe { StatusModule::status_kind(p) } != before {
        return Err("native Limit status changed during sampling".into());
    }
    current(boid, expected)?;
    value.validate().map_err(str::to_owned)?;
    Ok(value)
}

/// Validate a resolved graph effect before consuming its staged authored state.
/// This performs no resource, effect, status or HUD mutation.
pub(crate) fn preflight(boid: u32, expected: Identity, command: Command) -> Result<(), String> {
    checked_context(boid, expected, command).map(|_| ())
}

fn checked_context(
    boid: u32,
    expected: Identity,
    command: Command,
) -> Result<(Snapshot, usize, usize), String> {
    command.validate().map_err(str::to_owned)?;
    current(boid, expected)?;
    if INSTALL_STATE.load(Ordering::Acquire) != INSTALLED {
        return Err("native Limit resource binding is not ready".into());
    }
    let before = snapshot(boid, expected)?;
    model::preflight(command, before).map_err(str::to_owned)?;
    let (text, end) = text_bounds()?;
    let mut memory = NativeReadCache::default();
    let fighter = fighter_pointer(&mut memory, boid, expected)?;
    let signatures = match command {
        Command::Adjust(_) => {
            matches::<0x390>(&mut memory, text, end, ADJUST_OFFSET, &ADJUST_DIGEST)
        }
        Command::Clear => {
            matches::<0x70>(&mut memory, text, end, CLEAR_OFFSET, &CLEAR_DIGEST)
                && matches::<0xf4>(&mut memory, text, end, 0x8db3e0, &CLEANUP_DIGEST)
                && matches::<0x234>(&mut memory, text, end, 0x8db540, &RESET_DIGEST)
        }
    };
    if !signatures {
        return Err("native Limit operation signature mismatch".into());
    }
    current(boid, expected)?;
    Ok((before, text, fighter))
}

/// The caller supplies the identity of its current actor, never a native pointer.
/// A failed postcondition reports the failure; it never retries a native mutation.
pub(crate) fn execute(boid: u32, expected: Identity, command: Command) -> Result<Snapshot, String> {
    let (before, text, fighter) = checked_context(boid, expected, command)?;
    match command {
        Command::Adjust(delta) => {
            let apply: unsafe extern "C" fn(*mut smash::app::BattleObjectModuleAccessor, f32, u32) =
                unsafe { std::mem::transmute(text + ADJUST_OFFSET) };
            // These reviewed operations use a non-charge cause. Charge-cause
            // writes need their own native phase acceptance before exposure.
            unsafe {
                apply(
                    expected.module_accessor_addr as *mut smash::app::BattleObjectModuleAccessor,
                    delta,
                    0,
                );
            }
        }
        Command::Clear => {
            let clear: unsafe extern "C" fn(usize, usize) =
                unsafe { std::mem::transmute(text + CLEAR_OFFSET) };
            unsafe {
                clear(0, fighter);
            }
        }
    }
    let after = snapshot(boid, expected)?;
    model::verify(command, before, after).map_err(str::to_owned)?;
    Ok(after)
}
