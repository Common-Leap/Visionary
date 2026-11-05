use parking_lot::Mutex;
use skyline::hooks::{A64InlineHook, InlineCtx};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use super::live_parameters::NativeReadCache;
use super::{contact_snapshot_model as contact, damage_capture, damage_commit_model as model};

const FUNCTION_OFFSET: usize = 0x6c26c0;
const BEFORE_OFFSET: usize = 0x6c26f0;
const AFTER_OFFSET: usize = 0x6c2740;
const CONTACT_CALLER_OFFSET: usize = 0x3febc0;
const CONTACT_CALLER: [u8; 24] = [
    0x68, 0x02, 0x40, 0xf9, 0x08, 0x3d, 0x40, 0xf9, 0xe2, 0xa3, 0x00, 0x91, 0xe0, 0x03, 0x13, 0xaa,
    0xe1, 0x03, 0x15, 0xaa, 0x00, 0x01, 0x3f, 0xd6,
];
// Complete entry through the percent store, epilogue and adjacent branch target.
const WINDOW: [u8; 160] = [
    0xe9, 0x23, 0xba, 0x6d, 0xf9, 0x0b, 0x00, 0xf9, 0xf8, 0x5f, 0x02, 0xa9, 0xf6, 0x57, 0x03, 0xa9,
    0xf4, 0x4f, 0x04, 0xa9, 0xfd, 0x7b, 0x05, 0xa9, 0xfd, 0x43, 0x01, 0x91, 0x28, 0x20, 0x41, 0x39,
    0xf5, 0x03, 0x02, 0xaa, 0xf3, 0x03, 0x01, 0xaa, 0xf4, 0x03, 0x00, 0xaa, 0x88, 0x03, 0x00, 0x34,
    0x80, 0x3e, 0x40, 0xf9, 0x08, 0x00, 0x40, 0xf9, 0x60, 0x02, 0x40, 0xbd, 0xa1, 0x02, 0x40, 0xbd,
    0x81, 0x06, 0x40, 0xf9, 0x08, 0x28, 0x21, 0x1e, 0x08, 0xcd, 0x40, 0xf9, 0x00, 0x01, 0x3f, 0xd6,
    0xe0, 0x1f, 0x00, 0xbd, 0x80, 0x3e, 0x40, 0xf9, 0x08, 0x00, 0x40, 0xf9, 0x82, 0x06, 0x40, 0xf9,
    0x08, 0xd1, 0x40, 0xf9, 0xe1, 0x73, 0x00, 0x91, 0xe3, 0x03, 0x13, 0xaa, 0x00, 0x01, 0x3f, 0xd6,
    0xe0, 0x1f, 0x40, 0xbd, 0x00, 0x21, 0x20, 0x1e, 0x00, 0xcc, 0x28, 0x1e, 0x60, 0x02, 0x00, 0xbd,
    0xfd, 0x7b, 0x45, 0xa9, 0xf4, 0x4f, 0x44, 0xa9, 0xf9, 0x0b, 0x40, 0xf9, 0xf6, 0x57, 0x43, 0xa9,
    0xf8, 0x5f, 0x42, 0xa9, 0xe9, 0x23, 0xc6, 0x6c, 0xc0, 0x03, 0x5f, 0xd6, 0x68, 0x1a, 0x44, 0x39,
];

static ENABLED: AtomicBool = AtomicBool::new(false);
// 0: not requested, 1: installing, 2: patched, 3: rejected/faulted.
static INSTALL_STATE: AtomicU8 = AtomicU8::new(0);
static STATE: Mutex<State> = Mutex::new(State {
    pending: model::PendingStore::new(),
});
struct State {
    pending: model::PendingStore,
}

pub(super) fn install_hook() {
    if INSTALL_STATE.load(Ordering::Acquire) != 0 {
        return;
    }
    if !super::damage_dispatch::has_consumers() {
        return;
    }
    let Some((_, _, text)) = damage_capture::capture_context() else {
        return;
    };
    if INSTALL_STATE
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    if !signatures_match(text) {
        INSTALL_STATE.store(3, Ordering::Release);
        crate::slight::diag::note(
            "DAMAGE_COMMIT rejected: exact 13.0.4 fighter percent-add/clamp signature mismatch",
        );
        return;
    }
    unsafe {
        A64InlineHook(
            (text + BEFORE_OFFSET) as *const skyline::libc::c_void,
            observe_before as *const skyline::libc::c_void,
        );
        A64InlineHook(
            (text + AFTER_OFFSET) as *const skyline::libc::c_void,
            observe_after as *const skyline::libc::c_void,
        );
    }
    let mut patched = NativeReadCache::default();
    let before = patched.read::<u32>(text + BEFORE_OFFSET);
    let after = patched.read::<u32>(text + AFTER_OFFSET);
    // The pinned inline pool is within direct-B reach. Refuse observation if
    // either void backend call leaves the original instruction in place.
    if !before.is_some_and(|word| word & 0xfc00_0000 == 0x1400_0000)
        || !after.is_some_and(|word| word & 0xfc00_0000 == 0x1400_0000)
    {
        INSTALL_STATE.store(3, Ordering::Release);
        crate::slight::diag::note(
            "DAMAGE_COMMIT rejected: expected direct-B inline patches are absent",
        );
        return;
    }
    ENABLED.store(true, Ordering::Release);
    INSTALL_STATE.store(2, Ordering::Release);
    if let Some((epoch, frame, _)) = damage_capture::capture_context() {
        super::damage_feed::enable(epoch, frame);
    }
    crate::slight::diag::note("DAMAGE_COMMIT inline patches present around verified fighter percent-add/clamp; typed controller damage delivery active");
}

pub(super) fn signatures_match(text: usize) -> bool {
    let mut memory = NativeReadCache::default();
    text != 0
        && text
            .checked_add(FUNCTION_OFFSET)
            .and_then(|at| memory.read::<[u8; 160]>(at))
            == Some(WINDOW)
        && text
            .checked_add(CONTACT_CALLER_OFFSET)
            .and_then(|at| memory.read::<[u8; 24]>(at))
            == Some(CONTACT_CALLER)
}

fn key(ctx: &InlineCtx, epoch: u64, frame: u64) -> model::CallKey {
    model::CallKey {
        epoch,
        frame,
        stack: ctx.sp.x(),
        module: ctx.registers[20].x(),
        data: ctx.registers[19].x(),
    }
}

unsafe extern "C" fn observe_before(ctx: &InlineCtx) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let Some((epoch, frame, text)) = damage_capture::capture_context() else {
        return;
    };
    let call = key(ctx, epoch, frame);
    super::damage_feed::cancel_update(call);
    // The native stack slot may be reused later in this same frame. Clear a
    // missed end before *any* early return (including a zero/invalid pending
    // value), so this invocation cannot borrow the earlier call's reservation.
    // If that invalidation cannot acquire the lock, stop observation entirely.
    let Some(mut state) = STATE.try_lock() else {
        super::damage_feed::lose(frame);
        ENABLED.store(false, Ordering::Release);
        INSTALL_STATE.store(3, Ordering::Release);
        super::damage_feed::disable();
        return;
    };
    let _ = state.pending.end(call);
    drop(state);
    let mut memory = NativeReadCache::default();
    let Some(before) = memory.read::<f32>(call.data as usize) else {
        super::damage_feed::lose(frame);
        return;
    };
    let Some(pending) = memory.read::<f32>(ctx.registers[21].x() as usize) else {
        super::damage_feed::lose(frame);
        return;
    };
    if pending == 0.0 {
        return;
    }
    // This field's native object meaning remains a diagnostic question. It
    // cannot grant attacker credit or fall back to a matching player entry.
    let Some(context_id) = (call.data as usize)
        .checked_add(0x44)
        .and_then(|at| memory.read::<u32>(at))
    else {
        super::damage_feed::lose(frame);
        return;
    };
    let mut objects = [
        damage_capture::read_native_object(context_id),
        read_target(ctx, text),
        None,
    ];
    if let Some(actor) = objects[0].filter(|actor| actor.identity.category == 1) {
        if let Some(founder) = actor.identity.founder {
            objects[2] = damage_capture::read_native_object(founder)
                .filter(|row| row.identity.category == 0);
            if damage_capture::read_native_object(context_id) != Some(actor) {
                objects[0] = None;
                objects[2] = None;
            }
        }
    }
    let row = model::Sample {
        epoch,
        frame,
        index: u32::MAX,
        context_id,
        // The verified prologue saves its incoming LR at native SP + 0x58.
        // Alternate branches call helpers before reaching this observer, so
        // the current x30 would describe a helper return rather than our caller.
        caller_return_offset: (call.stack as usize)
            .checked_add(0x58)
            .and_then(|at| memory.read::<u64>(at))
            .and_then(damage_capture::main_return_offset),
        before_bits: before.to_bits(),
        pending_bits: pending.to_bits(),
        after_bits: 0,
        cap_bits: None,
        suppressed: false,
        objects,
    };
    super::damage_feed::begin_update(call, row);
    if let Some(mut state) = STATE.try_lock() {
        if damage_capture::capture_context() == Some((epoch, frame, text)) {
            state.pending.begin(call, row);
        }
    }
}

unsafe extern "C" fn observe_after(ctx: &InlineCtx) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let Some((epoch, frame, text)) = damage_capture::capture_context() else {
        return;
    };
    let Some(mut row) = STATE
        .try_lock()
        .and_then(|mut state| state.pending.end(key(ctx, epoch, frame)))
    else {
        return;
    };
    // Re-resolve source and owner after the native update. Stored addresses
    // remain expectations and are never dereferenced from the retained row.
    for index in [0, 2] {
        row.objects[index] = row.objects[index].and_then(|before| {
            damage_capture::read_native_object(before.identity.object_id)
                .filter(|after| after.identity == before.identity)
        });
    }
    let current = read_target(ctx, text);
    row.objects[1] = current
        .filter(|after| row.objects[1].is_some_and(|before| before.identity == after.identity));
    row.after_bits = ctx.registers_f[0].s().to_bits();
    // Final native clamp bound, after both virtual calls have completed.
    row.cap_bits = (ctx.sp.x() as usize)
        .checked_add(0x1c)
        .and_then(|at| NativeReadCache::default().read::<u32>(at));
    super::damage_feed::complete_update(key(ctx, epoch, frame), row);
}

unsafe fn read_target(ctx: &InlineCtx, text: usize) -> Option<contact::Observation> {
    let module = ctx.registers[20].x() as usize;
    let mut memory = NativeReadCache::default();
    let accessor: usize = memory.read(module.checked_add(8)?)?;
    let vtable: usize = memory.read(module)?;
    if text == 0
        || memory.read::<usize>(vtable.checked_add(0xc8)?)? != text.checked_add(0x3ff9f0)?
        || memory.read::<usize>(vtable.checked_add(0x320)?)? != text.checked_add(0x6c2be0)?
        || memory.read::<usize>(accessor.checked_add(0xa8)?)? != module
        || memory.read::<usize>(module.checked_add(0x18)?)? != ctx.registers[19].x() as usize
    {
        return None;
    }
    let offset = std::mem::offset_of!(smash::app::BattleObjectModuleAccessor, battle_object_id);
    let boid = memory.read::<u32>(accessor.checked_add(offset)?)?;
    let target = damage_capture::read_native_object(boid)?;
    (target.identity.category == 0 && target.identity.accessor == accessor as u64).then_some(target)
}

pub(super) fn on_frame(epoch: u64, frame: u64) {
    if let Some(mut state) = STATE.try_lock() {
        state.pending.expire(epoch, frame);
    }
}
