//! HDR's AttackModule collision-result seam (attack.rs::x03df93c).
//! w8 is the current contact mask, x19 the attacking AttackModule, and
//! [x22 + 0x24] the defender BOID. This runs before ORR w8,w8,w9
//! merges the current mask into the sticky result. SP+0x1c is the saved
//! get_power result from 0x3df628/0x3df640, not shield-HP depletion.
use super::{
    damage_capture, damage_feed, damage_feed_model::Stamp, live_parameters::NativeReadCache,
    shield_contacts_model::Queue,
};
use parking_lot::Mutex;
use skyline::hooks::{A64InlineHook, InlineCtx};

static QUEUE: Mutex<Queue> = Mutex::new(Queue::new());

/// Called once, only after damage_capture verifies the native 13.0.4 image.
pub(super) unsafe fn install(text: *mut u8) {
    A64InlineHook(
        text.add(0x3df93c).cast(),
        observe as *const skyline::libc::c_void,
    );
}

unsafe extern "C" fn observe(ctx: &InlineCtx) {
    super::receiver_deflect::observe(ctx);
    if ctx.registers[8].w() & *smash::lib::lua_const::COLLISION_KIND_MASK_SHIELD as u32 == 0 {
        return;
    }
    let Some((epoch, frame, _)) = damage_capture::capture_context() else {
        return;
    };
    if !damage_feed::is_enabled() {
        return;
    }
    let revision = damage_feed::revision();
    let mut memory = NativeReadCache::default();
    let Some(accessor) = memory.read::<u64>(ctx.registers[19].x() as usize + 8) else {
        return;
    };
    let Some(attacker) = memory.read::<u32>(accessor as usize + 8) else {
        return;
    };
    let Some(defender) = memory.read::<u32>(ctx.registers[22].x() as usize + 0x24) else {
        return;
    };
    let Some(attack) = damage_capture::read_native_object(attacker) else {
        return;
    };
    if attack.identity.accessor != accessor {
        return;
    }
    let weapon = (attack.identity.category == 1).then_some(attack.identity);
    let owner = if let Some(weapon) = weapon {
        let Some(founder) = weapon.founder else {
            return;
        };
        let Some(owner) = damage_capture::read_native_object(founder) else {
            return;
        };
        owner
    } else {
        attack
    };
    let Some(target) = damage_capture::read_native_object(defender) else {
        return;
    };
    let Some((source, source_status)) =
        damage_capture::published_actor_with_status(epoch, frame, owner.identity)
    else {
        return;
    };
    let Some((target, target_status)) =
        damage_capture::published_actor_with_status(epoch, frame, target.identity)
    else {
        return;
    };
    // Reprove the weapon's founder/current link owner after resolving its fighter.
    if damage_capture::read_native_object(attacker).map(|v| v.identity) != Some(attack.identity)
        || revision != damage_feed::revision()
    {
        return;
    }
    let stamp = Stamp {
        epoch,
        frame,
        revision,
    };
    if !QUEUE.try_lock().is_some_and(|mut queue| {
        queue.push(
            stamp,
            [source, target],
            [Some(source_status), Some(target_status)],
            weapon,
            memory.read::<u32>(ctx.sp.x() as usize + 0x1c),
        )
    }) {
        damage_feed::lose(frame);
    }
}

pub(super) fn on_frame(epoch: u64, frame: u64) {
    super::receiver_deflect::on_frame(epoch, frame);
    let delivery = Stamp {
        epoch,
        frame,
        revision: damage_feed::revision(),
    };
    let Some(rows) = QUEUE.try_lock().map(|mut queue| queue.drain(delivery)) else {
        damage_feed::lose(frame);
        return;
    };
    for row in rows.into_iter().flatten() {
        super::damage_dispatch::deliver(row, delivery, || {
            damage_feed::delivery_is_current(row, delivery)
        });
    }
}

/// Exact 13.0.4 instructions for the saved get_power scalar, its binding,
/// and the per-contact mask before it is ORed into the sticky result.
pub(super) fn signatures_match(text: usize) -> bool {
    let mut memory = NativeReadCache::default();
    let expected_3df60c: [u8; 64] = [
        0x68, 0x02, 0x40, 0xf9, 0xc1, 0xce, 0x40, 0x39, 0x00, 0x1d, 0xa8, 0x4e, 0xc2, 0xd6, 0x40,
        0x39, 0xe0, 0x03, 0x13, 0xaa, 0xe3, 0x03, 0x1f, 0x2a, 0x08, 0xad, 0x40, 0xf9, 0x00, 0x01,
        0x3f, 0xd6, 0xe8, 0x03, 0x1f, 0x32, 0xe8, 0x1b, 0x00, 0xb9, 0x48, 0x5c, 0x02, 0xd0, 0x08,
        0x21, 0x38, 0x91, 0xe8, 0x0b, 0x00, 0xf9, 0xe0, 0x1f, 0x00, 0xbd, 0xf6, 0x13, 0x00, 0xf9,
        0xff, 0x53, 0x00, 0x79,
    ];
    let expected_3df92c: [u8; 24] = [
        0xc9, 0x56, 0x00, 0x79, 0xc8, 0xbe, 0x40, 0x39, 0x69, 0xd6, 0x41, 0xb9, 0x28, 0x23, 0xc8,
        0x1a, 0x08, 0x01, 0x09, 0x2a, 0x68, 0xd6, 0x01, 0xb9,
    ];
    let expected_1fd0370: [u8; 24] = [
        0x00, 0x50, 0x40, 0xf9, 0x08, 0x00, 0x40, 0xf9, 0x04, 0xad, 0x40, 0xf9, 0x42, 0x00, 0x00,
        0x12, 0x63, 0x00, 0x00, 0x12, 0x80, 0x00, 0x1f, 0xd6,
    ];
    memory.read::<[u8; 64]>(text + 0x3df60c) == Some(expected_3df60c)
        && memory.read::<[u8; 24]>(text + 0x3df92c) == Some(expected_3df92c)
        && memory.read::<[u8; 24]>(text + 0x1fd0370) == Some(expected_1fd0370)
}

pub(super) fn is_installed(text: usize) -> bool {
    NativeReadCache::default()
        .read::<u32>(text + 0x3df93c)
        .is_some_and(|word| word & 0xfc00_0000 == 0x1400_0000)
}
