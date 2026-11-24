//! Edits to an effect of the shared file: its name leads to the carrier's edited copy for whoever
//! plays it, the game's own code included.
//!
//! Every effect is made by one function of the effect manager, which finds the name in the
//! manager's map; the modules ask another for its flags. Both look up the name's hash from one
//! register, after which the request keeps the name it came with, so what stops, counts or pins an
//! effect by name still does.

use super::effect_reload::{kind_is_registered, NAME_MASK};
use crate::offsets;
use skyline::hooks::{getRegionAddress, A64InlineHook, InlineCtx, Region};
use std::sync::atomic::{AtomicBool, Ordering};

/// Shared effect → its edited copy.
static LEADS: parking_lot::RwLock<Vec<(u64, u64)>> = parking_lot::RwLock::new(Vec::new());
/// Skips the lookup when nothing shared is edited.
static ANY: AtomicBool = AtomicBool::new(false);
static INSTALLED: AtomicBool = AtomicBool::new(false);

pub fn set(leads: Vec<(u64, u64)>) {
    let any = !leads.is_empty();
    // Held only for one lookup. Spin rather than park, as `stage_rules` does.
    let mut current = loop {
        if let Some(guard) = LEADS.try_write() {
            break guard;
        }
        core::hint::spin_loop();
    };
    *current = leads;
    drop(current);
    ANY.store(any, Ordering::Release);
}

/// `and x8, <name>, #0xffffffffff` then `ands x13, x12, x11`, where the name's hash is in x8.
const MAKE: [u32; 2] = [0x92409f68, 0xea0b018d];
const FLAGS: [u32; 2] = [0x92409c28, 0xea0b018d];

unsafe fn lead(ctx: &mut InlineCtx, manager: u64) {
    if !ANY.load(Ordering::Acquire) {
        return;
    }
    let Some(leads) = LEADS.try_read() else {
        return;
    };
    let name = ctx.registers[8].x();
    let found = super::carrier_gate::looked_up(name & NAME_MASK, &leads, |copy| {
        kind_is_registered(manager as *mut u64, copy)
    });
    ctx.registers[8].set_x(found);
}

/// The manager's lock is held here, and its address in x20.
unsafe extern "C" fn made(ctx: &mut InlineCtx) {
    lead(ctx, ctx.registers[20].x());
}

unsafe extern "C" fn flags(ctx: &mut InlineCtx) {
    lead(ctx, ctx.registers[0].x());
}

pub fn install() {
    let text = unsafe { getRegionAddress(Region::Text) as usize };
    let sites: [(usize, [u32; 2], unsafe extern "C" fn(&mut InlineCtx)); 2] = [
        (offsets::MAKE_EFFECT_NAME.get(), MAKE, made),
        (offsets::EFFECT_FLAGS_NAME.get(), FLAGS, flags),
    ];
    let there = |(offset, words, _): &(usize, [u32; 2], _)| unsafe {
        *((text + offset - 4) as *const [u32; 2]) == *words
    };
    if text == 0 || !sites.iter().all(there) {
        crate::slight::diag::note("shared effects: this game's code is not the expected one");
        return;
    }
    for (offset, _, hook) in sites {
        unsafe { A64InlineHook((text + offset) as *const _, hook as *const _) };
    }
    INSTALLED.store(true, Ordering::Release);
}

/// Whether a shared effect's edit can reach the running game.
pub fn installed() -> bool {
    INSTALLED.load(Ordering::Acquire)
}
