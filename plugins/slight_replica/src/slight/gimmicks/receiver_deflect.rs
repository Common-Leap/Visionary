//! Receiver events from the signature-checked per-contact AttackModule seam.
use super::{
    damage_capture, damage_feed,
    damage_feed_model::Stamp,
    live_parameters::NativeReadCache,
    native_inputs::{DamageDirection, DamageInput},
    receiver_deflect_model::{Contact, Queue},
};
use parking_lot::Mutex;
use skyline::hooks::InlineCtx;

static QUEUE: Mutex<Queue> = Mutex::new(Queue::new());

// main_1304.bin, 3df60c..3df944: get_power is saved at SP+0x1c;
// after native collision notification, 3df930 loads this row's collision
// kind (+0x2f), 3df938 builds its mask, and 3df93c merges the sticky result.
// The row's +0x24 is the actual contacted BOID, not the projectile founder.
// Reuse shield_contacts' exact signatures and hook; never poll sticky flags
// or infer a receiver from proximity / active reflector volumes.
pub(super) unsafe fn observe(ctx: &InlineCtx) {
    let kind = ctx.registers[8].w();
    let mut direction = if kind == *smash::lib::lua_const::COLLISION_KIND_MASK_REFLECTOR as u32 {
        DamageDirection::Reflect
    } else if kind == *smash::lib::lua_const::COLLISION_KIND_MASK_ABSORBER as u32 {
        DamageDirection::Absorb
    } else {
        return;
    };
    // An owned absorber is a reflector volume; claim the projectile before
    // any damage-feed gate so it vanishes even with no OnAbsorb rule.
    if direction == DamageDirection::Reflect {
        let mut memory = NativeReadCache::default();
        let weapon = memory
            .read::<u64>(ctx.registers[19].x() as usize + 8)
            .and_then(|accessor| Some((memory.read::<u32>(accessor as usize + 8)?, accessor)));
        let receiver = memory.read::<u32>(ctx.registers[22].x() as usize + 0x24);
        if let (Some((weapon, accessor)), Some(receiver)) = (weapon, receiver) {
            if super::deflect_volume::is_absorber(receiver) {
                super::deflect_volume::absorb(weapon, accessor);
                direction = DamageDirection::Absorb;
            }
        }
    }
    if !damage_feed::is_enabled() {
        return;
    }
    let Some((epoch, frame, _)) = damage_capture::capture_context() else {
        return;
    };
    let stamp = Stamp {
        epoch,
        frame,
        revision: damage_feed::revision(),
    };
    let mut memory = NativeReadCache::default();
    let Some(accessor) = memory.read::<u64>(ctx.registers[19].x() as usize + 8) else {
        return;
    };
    let Some(source) = memory
        .read::<u32>(accessor as usize + 8)
        .and_then(|id| damage_capture::read_native_object(id))
    else {
        return;
    };
    if source.identity.category != 1 || source.identity.accessor != accessor {
        return;
    }
    let Some(target) = memory
        .read::<u32>(ctx.registers[22].x() as usize + 0x24)
        .and_then(|id| damage_capture::read_native_object(id))
    else {
        return;
    };
    let Some((actor, status)) =
        damage_capture::published_actor_with_status(epoch, frame, target.identity)
    else {
        return;
    };
    let input = memory
        .read::<f32>(ctx.sp.x() as usize + 0x1c)
        .and_then(|power| DamageInput::new(direction, power).ok())
        .or_else(|| DamageInput::shield(direction).ok());
    let Some(input) = input.and_then(|input| {
        input
            .with_projectile(true)
            .with_receiver_status(actor.identity.kind, Some(status))
            .ok()
    }) else {
        return;
    };
    if damage_capture::read_native_object(source.identity.object_id).map(|s| s.identity)
        != Some(source.identity)
        || damage_feed::revision() != stamp.revision
    {
        return;
    }
    let Some(mut queue) = QUEUE.try_lock() else {
        damage_feed::lose(frame);
        return;
    };
    queue.push(Contact {
        actor,
        stamp,
        input,
    });
}

pub(super) fn on_frame(epoch: u64, frame: u64) {
    let stamp = Stamp {
        epoch,
        frame,
        revision: damage_feed::revision(),
    };
    let Some(rows) = QUEUE.try_lock().map(|mut q| q.drain(stamp)) else {
        damage_feed::lose(frame);
        return;
    };
    for row in rows.into_iter().flatten() {
        // Source weapons may already have been consumed. Only the copied
        // contact and receiver stock survive; no later weapon dereference.
        let current = || {
            damage_feed::is_enabled()
                && damage_feed::capture_is_complete(row.stamp.frame)
                && row.stamp.epoch == epoch
                && row.stamp.frame.checked_add(1) == Some(frame)
                && damage_feed::revision() == row.stamp.revision
                && damage_capture::capture_context()
                    .is_some_and(|(e, f, _)| e == epoch && f == frame)
                && damage_capture::published_actor_with_status(epoch, frame, row.actor.identity)
                    .is_some_and(|(a, _)| a == row.actor)
                && unsafe { damage_capture::read_native_object(row.actor.identity.object_id) }
                    .map(|o| o.identity)
                    == Some(row.actor.identity)
        };
        if current() {
            super::live_behavior::dispatch_incoming(
                row.actor.identity.object_id,
                row.input,
                current,
            );
        }
    }
}
