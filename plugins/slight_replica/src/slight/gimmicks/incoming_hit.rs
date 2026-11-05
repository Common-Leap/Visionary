//! 13.0.4 DamageModule contact decision, before preparation/percent/reaction.
use super::{damage_capture, damage_feed, live_parameters::NativeReadCache};
use skyline::hooks::{A64InlineHook, InlineCtx};
static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(super) fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, std::sync::atomic::Ordering::Release);
}
const OFFSET: usize = 0x3fe4b4;
// Local main_1304.bin: preparation loop 3fe350 loads each 0x1c0-byte
// DamageInfo at 3fe4a8. Its +0x44 BOID is resolved at 3fe4dc, then its
// AttackModule (+0xa0) supplies the +0x52/+0x55 attack at 3fe540.
// Cancel by clearing ONLY this row's native admission scalars: +0xc/+0x10
// gate preparation (3fe4b4..cc); +4/+0x10/+0x19c gate percent/reaction
// (3febb0..3fec28); +0x190 gates the secondary percent path (3fedb8).
// The native loops still consume/retire the row. No percent is rolled back.
const PREPARE: [u8; 64] = [
    0x6e, 0x0e, 0x40, 0xf9, 0xd6, 0x01, 0x19, 0x8b, 0xc9, 0x52, 0x06, 0xd1, 0x20, 0x01, 0x40, 0xbd,
    0x08, 0x20, 0x20, 0x1e, 0xac, 0x00, 0x00, 0x54, 0xc8, 0x62, 0x06, 0xd1, 0x00, 0x01, 0x40, 0xbd,
    0x08, 0x20, 0x20, 0x1e, 0x8d, 0x28, 0x00, 0x54, 0xe9, 0x3b, 0x06, 0xa9, 0xc8, 0x82, 0x05, 0xd1,
    0x00, 0x01, 0x40, 0xb9, 0xe8, 0x27, 0x00, 0xf9, 0x20, 0xb8, 0xfe, 0x97, 0xa0, 0x00, 0x00, 0xb4,
];
const APPLY: [u8; 160] = [
    0x7c, 0x0e, 0x40, 0xf9, 0x15, 0x73, 0x1b, 0x9b, 0xf6, 0x03, 0x15, 0xaa, 0xc8, 0x4e, 0x40, 0xb8,
    0xe8, 0x2b, 0x00, 0xb9, 0x68, 0x02, 0x40, 0xf9, 0x08, 0x2d, 0x41, 0xf9, 0xe0, 0x03, 0x13, 0xaa,
    0x00, 0x01, 0x3f, 0xd6, 0xe0, 0x2b, 0x40, 0xbd, 0x08, 0x20, 0x20, 0x1e, 0x60, 0x01, 0x00, 0x54,
    0x40, 0x01, 0x00, 0x37, 0x68, 0x02, 0x40, 0xf9, 0x08, 0x3d, 0x40, 0xf9, 0xe2, 0xa3, 0x00, 0x91,
    0xe0, 0x03, 0x13, 0xaa, 0xe1, 0x03, 0x15, 0xaa, 0x00, 0x01, 0x3f, 0xd6, 0xe8, 0x2b, 0x40, 0xb9,
    0xc8, 0x02, 0x00, 0xb9, 0xe0, 0x2b, 0x40, 0xbd, 0x08, 0x20, 0x20, 0x1e, 0x0c, 0x01, 0x00, 0x54,
    0x08, 0x73, 0x1b, 0x9b, 0x00, 0x11, 0x40, 0xbd, 0x08, 0x20, 0x20, 0x1e, 0x8c, 0x00, 0x00, 0x54,
    0x08, 0x73, 0x1b, 0x9b, 0x08, 0x71, 0x46, 0x39, 0xa8, 0x0d, 0x00, 0x34, 0x68, 0x0e, 0x40, 0xf9,
    0x60, 0x3e, 0x40, 0xf9, 0x7f, 0x2e, 0x04, 0x39, 0x02, 0x23, 0x1b, 0x9b, 0x08, 0x00, 0x40, 0xf9,
    0x61, 0x06, 0x40, 0xf9, 0x08, 0x91, 0x40, 0xf9, 0xe3, 0x07, 0x40, 0xf9, 0x00, 0x01, 0x3f, 0xd6,
];
const SECONDARY: [u8; 20] = [
    0x08, 0x73, 0x1b, 0x9b, 0x00, 0x91, 0x41, 0xbd, 0x08, 0x20, 0x20, 0x1e, 0xe0, 0x2b, 0x00, 0xbd,
    0xe0, 0x05, 0x00, 0x54,
];

pub(super) fn signatures_match(text: usize) -> bool {
    let mut m = NativeReadCache::default();
    m.read::<[u8; 64]>(text + 0x3fe4a8) == Some(PREPARE)
        && m.read::<[u8; 160]>(text + 0x3feb8c) == Some(APPLY)
        && m.read::<[u8; 20]>(text + 0x3fedb8) == Some(SECONDARY)
}
pub(super) fn is_installed(text: usize) -> bool {
    NativeReadCache::default()
        .read::<u32>(text + OFFSET)
        .is_some_and(|w| w & 0xfc00_0000 == 0x1400_0000)
}
pub(super) unsafe fn install(text: *mut u8) {
    A64InlineHook(
        text.add(OFFSET).cast(),
        decide as *const skyline::libc::c_void,
    );
}
unsafe extern "C" fn decide(ctx: &mut InlineCtx) {
    if !ENABLED.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    let Some((epoch, frame, text)) = damage_capture::capture_context() else {
        return;
    };
    let revision = damage_feed::revision();
    let mut memory = NativeReadCache::default();
    let module = ctx.registers[19].x() as usize;
    let Some(accessor) = memory.read::<u64>(module + 8) else {
        return;
    };
    let Some(vtable) = memory.read::<usize>(module) else {
        return;
    };
    // Same exact fighter DamageModule proof used by damage_commit::read_target.
    if memory.read::<usize>(vtable + 0xc8) != Some(text + 0x3ff9f0)
        || memory.read::<usize>(vtable + 0x320) != Some(text + 0x6c2be0)
        || memory.read::<usize>(accessor as usize + 0xa8) != Some(module)
    {
        return;
    }
    let Some(base) = memory.read::<usize>(module + 0x18) else {
        return;
    };
    let Some(count) = memory.read::<u32>(module + 0x10) else {
        return;
    };
    let index = ctx.registers[20].x() as usize;
    if index >= count as usize || index >= 32 {
        return;
    }
    let record = base + index * 0x1c0;
    if ctx.registers[22].x() as usize != record + 0x1a4 {
        return;
    }
    let Some(a) = memory.read::<f32>(record + 0xc) else {
        return;
    };
    let Some(b) = memory.read::<f32>(record + 0x10) else {
        return;
    };
    if !a.is_finite() || !b.is_finite() || (a <= 0.0 && b <= 0.0) {
        return;
    }
    let Some(source_id) = memory.read::<u32>(record + 0x44) else {
        return;
    };
    let Some(target_id) = memory.read::<u32>(accessor as usize + 8) else {
        return;
    };
    if source_id == target_id {
        return;
    }
    let Some(source) = damage_capture::read_native_object(source_id) else {
        return;
    };
    let Some(target) = damage_capture::read_native_object(target_id) else {
        return;
    };
    if target.identity.accessor != accessor || target.identity.category != 0 {
        return;
    }
    let Some((actor, _)) =
        damage_capture::published_actor_with_status(epoch, frame, target.identity)
    else {
        return;
    };
    let current = || {
        damage_capture::capture_context().is_some_and(|(e, f, _)| e == epoch && f == frame)
            && damage_feed::revision() == revision
            && damage_capture::read_native_object(source_id).map(|v| v.identity)
                == Some(source.identity)
            && damage_capture::read_native_object(target_id).map(|v| v.identity)
                == Some(target.identity)
            && damage_capture::published_actor_with_status(epoch, frame, target.identity)
                .is_some_and(|(v, _)| v == actor)
    };
    if !current() {
        return;
    }

    let Some(id) = memory.read::<u8>(record + 0x52) else {
        return;
    };
    let Some(part) = memory.read::<u8>(record + 0x55) else {
        return;
    };
    let power = smash::app::lua_bind::AttackModule::get_power(
        source.identity.accessor as *mut smash::app::BattleObjectModuleAccessor,
        i32::from(id),
        part != 0,
        1.0,
        false,
    );
    let Ok(input) = super::native_inputs::DamageInput::new(
        super::native_inputs::DamageDirection::Incoming,
        power,
    ) else {
        return;
    };
    let Ok(input) = input
        .with_projectile(source.identity.category == 1)
        .with_receiver_status(
            target.identity.kind,
            Some(smash::app::lua_bind::StatusModule::status_kind(
                target.identity.accessor as *mut _,
            )),
        )
    else {
        return;
    };
    if current() && super::live_behavior::dispatch_incoming(target_id, input, current) {
        crate::slight::diag::note(format!(
            "counter caught object={target_id} incoming_damage={power}"
        ));
        for offset in [4, 0xc, 0x10, 0x190] {
            std::ptr::write_unaligned((record + offset) as *mut f32, 0.0);
        }
        std::ptr::write_unaligned((record + 0x19c) as *mut u8, 0);
    }
}
