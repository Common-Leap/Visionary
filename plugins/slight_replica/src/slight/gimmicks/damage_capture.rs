use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

use parking_lot::Mutex;
use skyline::hooks::{getRegionAddress, A64InlineHook, InlineCtx, Region};

use super::contact_snapshot_model as contact;
use super::damage_capture_model::{self, RawSample};
use super::damage_identity_model::{Identity, IdentityTable};
use super::live_parameters::NativeReadCache;

const NOT_REQUESTED: u8 = 0;
const WAITING_FOR_ACTIVE_MATCH: u8 = 1;
const INSTALL_REJECTED: u8 = 2;
const INSTALL_ATTEMPTED: u8 = 3;
const INSTALL_DISABLED: u8 = 4;

const COLLISION_VENEER_OFFSET: usize = 0x2141600;
const COLLISION_VENEER: [u8; 12] = [
    0x00, 0x00, 0x40, 0xf9, 0x84, 0x00, 0x00, 0x12, 0x6a, 0xe4, 0x94, 0x17,
];
const COLLISION_BODY_OFFSET: usize = 0x67a7b0;
/// Exact historical `COLLISION_PATTERN`, verified against the cached 13.0.4 body bytes.
const COLLISION_BODY: [u8; 40] = [
    0xff, 0x03, 0x03, 0xd1, 0xe8, 0x2b, 0x00, 0xfd, 0xfc, 0x6f, 0x06, 0xa9, 0xfa, 0x67, 0x07, 0xa9,
    0xf8, 0x5f, 0x08, 0xa9, 0xf6, 0x57, 0x09, 0xa9, 0xf4, 0x4f, 0x0a, 0xa9, 0xfd, 0x7b, 0x0b, 0xa9,
    0xfd, 0xc3, 0x02, 0x91, 0xfb, 0x03, 0x00, 0xaa,
];

static INSTALL_STATE: AtomicU8 = AtomicU8::new(NOT_REQUESTED);
// Low bits contain arm/budget; upper bits are the match epoch. Reset changes
// this atomic without acquiring the model lock, invalidating in-flight samples.
static CONTROL: AtomicU64 = AtomicU64::new(damage_capture_model::initial_control());
static CURRENT_FRAME: AtomicU64 = AtomicU64::new(0);
static TEXT_START: AtomicU64 = AtomicU64::new(0);
static TEXT_END: AtomicU64 = AtomicU64::new(0);
static PUBLISHED_REVISION: AtomicU64 = AtomicU64::new(u64::MAX);
static IDENTITIES: Mutex<IdentityTable> = Mutex::new(IdentityTable::new());
pub fn install() -> bool {
    if INSTALL_STATE
        .compare_exchange(
            NOT_REQUESTED,
            INSTALL_DISABLED,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_err()
    {
        return false;
    }
    if !super::damage_dispatch::has_consumers() {
        return false;
    }
    INSTALL_STATE.store(WAITING_FOR_ACTIVE_MATCH, Ordering::Release);
    true
}

/// Once-per-global-frame pump: flush owned rows first, then attempt a deferred
/// install or re-arm an already-installed observation hook for a fresh match.
pub fn on_frame() {
    if super::damage_dispatch::has_consumers() {
        let _ = INSTALL_STATE.compare_exchange(
            INSTALL_DISABLED,
            WAITING_FOR_ACTIVE_MATCH,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
    let state = INSTALL_STATE.load(Ordering::Acquire);
    if state != WAITING_FOR_ACTIVE_MATCH && state != INSTALL_ATTEMPTED {
        return;
    }
    let frame = CURRENT_FRAME
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1);
    if state == INSTALL_ATTEMPTED {
        super::damage_commit::install_hook();
    }
    super::damage_commit::on_frame(
        damage_capture_model::epoch(CONTROL.load(Ordering::Acquire)),
        frame,
    );
    if !crate::slight::frame_context::is_fight_active() {
        super::damage_feed::on_frame(
            damage_capture_model::epoch(CONTROL.load(Ordering::Acquire)),
            frame,
        );
        return;
    }
    // Publish only copied registry scalars outside the native callback. An
    // exact pump stamp prevents callbacks from consuming an older publication.
    if let Some(mut identities) = IDENTITIES.try_lock() {
        let control = CONTROL.load(Ordering::Acquire);
        let revision = super::damage_feed::revision();
        let records = crate::slight::agents::all_records();
        let _ = identities.replace(
            damage_capture_model::epoch(control),
            frame,
            records.iter().map(record_identity),
        );
        PUBLISHED_REVISION.store(
            if super::damage_feed::revision() == revision {
                revision
            } else {
                u64::MAX
            },
            Ordering::Release,
        );
    }
    super::damage_feed::on_frame(
        damage_capture_model::epoch(CONTROL.load(Ordering::Acquire)),
        frame,
    );

    super::shield_contacts::on_frame(
        damage_capture_model::epoch(CONTROL.load(Ordering::Acquire)),
        frame,
    );

    match state {
        WAITING_FOR_ACTIVE_MATCH if has_registered_fighter() => install_for_active_match(),
        INSTALL_ATTEMPTED if !damage_capture_model::is_armed(CONTROL.load(Ordering::Acquire)) => {
            if has_registered_fighter() {
                arm_for_current_epoch();
            }
        }
        _ => {}
    }
}

/// Advance/disarm the epoch without waiting for the callback's sample mutex.
/// The next frame pump or callback lazily synchronizes the fixed model buffer.
pub fn reset_match() {
    loop {
        let current = CONTROL.load(Ordering::Acquire);
        let next = damage_capture_model::reset_match(current);
        if CONTROL
            .compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return;
        }
    }
}

fn has_registered_fighter() -> bool {
    crate::slight::agents::all_records()
        .iter()
        .any(|record| record.category == 0 && record.kind >= 0 && record.entry_id >= 0)
}

fn install_for_active_match() {
    let text = unsafe { getRegionAddress(Region::Text) as usize };
    if !signatures_match()
        || (super::damage_dispatch::has_consumers()
            && !super::damage_commit::signatures_match(text))
    {
        if INSTALL_STATE
            .compare_exchange(
                WAITING_FOR_ACTIVE_MATCH,
                INSTALL_REJECTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            crate::slight::diag::note(
                "DAMAGE_PROBE rejected: exact 13.0.4 collision/shield-result or required percent-add/clamp signature mismatch",
            );
        }
        return;
    }
    if INSTALL_STATE
        .compare_exchange(
            WAITING_FOR_ACTIVE_MATCH,
            INSTALL_ATTEMPTED,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_err()
    {
        return;
    }
    unsafe {
        let text = getRegionAddress(Region::Text);
        // Publish immutable image bounds before the callback can run. LR is
        // copied from Skyline's saved register context and normalized using
        // only these bounds. No absolute caller address is persisted.
        TEXT_START.store(text as u64, Ordering::Relaxed);
        TEXT_END.store(getRegionAddress(Region::Rodata) as u64, Ordering::Relaxed);
        super::shield_contacts::install(text.cast());
        super::incoming_hit::install(text.cast());
        let target = text.add(COLLISION_BODY_OFFSET);
        A64InlineHook(
            target.cast(),
            observe_collision as *const skyline::libc::c_void,
        );
    }
    let patched = NativeReadCache::default().read::<u32>(text + COLLISION_BODY_OFFSET);
    if !patched.is_some_and(|word| word & 0xfc00_0000 == 0x1400_0000)
        || !super::shield_contacts::is_installed(text)
        || !super::incoming_hit::is_installed(text)
    {
        INSTALL_STATE.store(INSTALL_REJECTED, Ordering::Release);
        crate::slight::diag::note(
            "DAMAGE_PROBE rejected: expected direct-B inline patch is absent",
        );
        return;
    }
    arm_for_current_epoch();
    super::damage_commit::install_hook();
}

fn arm_for_current_epoch() {
    loop {
        let current = CONTROL.load(Ordering::Acquire);
        let next = damage_capture_model::arm(current);
        if next == current
            || CONTROL
                .compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            return;
        }
    }
}

fn signatures_match() -> bool {
    // Skyline exposes this FFI region lookup as unsafe in the pinned API.
    let text = unsafe { getRegionAddress(Region::Text) as usize };
    if text == 0 {
        return false;
    }
    let Some(veneer) = text.checked_add(COLLISION_VENEER_OFFSET) else {
        return false;
    };
    let Some(body) = text.checked_add(COLLISION_BODY_OFFSET) else {
        return false;
    };
    let mut memory = NativeReadCache::default();
    memory.read::<[u8; 12]>(veneer) == Some(COLLISION_VENEER)
        && memory.read::<[u8; 40]>(body) == Some(COLLISION_BODY)
        && super::shield_contacts::signatures_match(text)
        && super::incoming_hit::signatures_match(text)
}

/// The opt-in probe uses only the closed native object/percent getters below.
/// No formatting, logging, writes, event dispatch, allocation or blocking lock
/// runs here. Copied accessor numbers are never dereferenced during flushing.
unsafe extern "C" fn observe_collision(ctx: &InlineCtx) {
    let Some((epoch, frame, _)) = capture_context() else {
        return;
    };
    let feed_ticket = super::damage_feed::reserve_contact(epoch, frame);
    if feed_ticket.is_none() {
        return;
    }
    let sample = RawSample {
        epoch,
        frame,
        sample_index: u32::MAX,
        object_id_1: ctx.registers[1].w(),
        object_id_2: ctx.registers[2].w(),
        amount_bits: ctx.registers_f[0].s().to_bits(),
        arg3_bits: ctx.registers[3].w(),
        arg4_bits: ctx.registers[4].w(),
        caller_return_offset: damage_capture_model::caller_return_offset(
            ctx.registers[30].x(),
            TEXT_START.load(Ordering::Relaxed),
            TEXT_END.load(Ordering::Relaxed),
        ),
    };
    let mut native = [
        read_native_object(sample.object_id_1),
        read_native_object(sample.object_id_2),
        None,
    ];
    if let Some(actor) = native[0] {
        if actor.identity.category == 1 {
            if let Some(founder) = actor.identity.founder {
                native[2] =
                    read_native_object(founder).filter(|owner| owner.identity.category == 0);
                // Founder resolution must not splice together two actor lifetimes.
                if read_native_identity(sample.object_id_1) != Some(actor.identity) {
                    native[0] = None;
                    native[2] = None;
                }
            }
        }
    }
    if let Some(ticket) = feed_ticket {
        let captured = [0, 1].map(|index| {
            native[if index == 0 && native[0].is_some_and(|v| v.identity.category == 1) {
                2
            } else {
                index
            }]
            .and_then(|object| published_actor_with_status(epoch, frame, object.identity))
        });
        super::damage_feed::contact(
            ticket,
            super::damage_feed_model::Contact {
                raw: sample,
                objects: native,
                actors: captured.map(|actor| actor.map(|(actor, _)| actor)),
                statuses: captured.map(|actor| actor.map(|(_, status)| status)),
            },
        );
    }
}

unsafe fn read_native_identity(boid: u32) -> Option<contact::Identity> {
    use smash::app::sv_battle_object as objects;
    if !objects::is_active(boid) {
        return None;
    }
    let category = objects::category(boid);
    if !matches!(category, 0 | 1) {
        return None;
    }
    let accessor = objects::module_accessor(boid);
    if accessor.is_null() || (accessor as usize) % 8 != 0 || (*accessor).battle_object_id != boid {
        return None;
    }
    let founder = if category == 1 {
        let id = objects::get_founder_id(boid);
        // Require current direct ownership as well as the native founder.
        // Transfers, reflected credit and nested weapon ownership stay excluded.
        let owner = smash::app::lua_bind::WorkModule::get_int(
            accessor,
            *smash::lib::lua_const::WEAPON_INSTANCE_WORK_ID_INT_LINK_OWNER,
        );
        contact::fighter_owner(id, owner)
    } else {
        None
    };
    let identity = contact::Identity {
        object_id: boid,
        accessor: accessor as u64,
        category,
        kind: objects::kind(boid),
        entry: objects::entry_id(boid),
        founder,
    };
    (identity.valid() && objects::is_active(boid) && objects::module_accessor(boid) == accessor)
        .then_some(identity)
}

pub(super) unsafe fn read_native_object(boid: u32) -> Option<contact::Observation> {
    let before = read_native_identity(boid)?;
    let percent = (before.category == 0).then(|| {
        smash::app::lua_bind::DamageModule::damage(
            before.accessor as *mut smash::app::BattleObjectModuleAccessor,
            0,
        )
    });
    let after = read_native_identity(boid)?;
    contact::coherent(before, after, percent)
}

/// Match a native identity to the current stock publication without retaining
/// its pointer. Reads happen outside the publication lock and are bracketed by
/// the global fighter lifecycle fence. Reset/rebirth/entry statuses are ineligible.
pub(super) fn published_actor(
    epoch: u64,
    frame: u64,
    native: contact::Identity,
) -> Option<super::damage_feed_model::Actor> {
    published_actor_with_status(epoch, frame, native).map(|(actor, _)| actor)
}

/// Preserve the status already sampled by the immediate lifetime admission.
/// This is capture-time context, not an additional engine read or a status
/// expectation used by later lifetime checks.
pub(super) fn published_actor_with_status(
    epoch: u64,
    frame: u64,
    native: contact::Identity,
) -> Option<(super::damage_feed_model::Actor, i32)> {
    if native.category != 0 || !native.valid() {
        return None;
    }
    let revision = super::damage_feed::revision();
    let published = IDENTITIES
        .try_lock()?
        .lookup(epoch, frame, native.object_id)?
        .identity;
    if PUBLISHED_REVISION.load(Ordering::Acquire) != revision
        || published.module_accessor != native.accessor
        || published.kind != native.kind
        || published.category != 0
        || published.entry_id != native.entry
    {
        return None;
    }
    let generation = published
        .stock_generation
        .filter(|value| *value != u32::MAX)?;
    let status = unsafe {
        use smash::lib::lua_const::{
            FIGHTER_STATUS_KIND_DEAD, FIGHTER_STATUS_KIND_ENTRY, FIGHTER_STATUS_KIND_REBIRTH,
        };
        // Resolve again before dereferencing: the supplied address is only an
        // identity expectation, never a pointer borrowed from a queued record.
        let current = read_native_identity(native.object_id)?;
        if current != native {
            return None;
        }
        let status = smash::app::lua_bind::StatusModule::status_kind(
            current.accessor as *mut smash::app::BattleObjectModuleAccessor,
        );
        if status < 0
            || status == *FIGHTER_STATUS_KIND_DEAD
            || status == *FIGHTER_STATUS_KIND_REBIRTH
            || status == *FIGHTER_STATUS_KIND_ENTRY
            || read_native_identity(native.object_id) != Some(native)
        {
            return None;
        }
        status
    };
    (super::damage_feed::revision() == revision
        && PUBLISHED_REVISION.load(Ordering::Acquire) == revision)
        .then_some((
            super::damage_feed_model::Actor {
                identity: native,
                stock_generation: generation,
            },
            status,
        ))
}

pub(super) fn capture_context() -> Option<(u64, u64, usize)> {
    let control = CONTROL.load(Ordering::Acquire);
    damage_capture_model::is_armed(control).then(|| {
        (
            damage_capture_model::epoch(control),
            CURRENT_FRAME.load(Ordering::Relaxed),
            TEXT_START.load(Ordering::Relaxed) as usize,
        )
    })
}

pub(super) fn main_return_offset(address: u64) -> Option<u64> {
    damage_capture_model::caller_return_offset(
        address,
        TEXT_START.load(Ordering::Relaxed),
        TEXT_END.load(Ordering::Relaxed),
    )
}

fn record_identity(record: &crate::slight::agents::AgentRecord) -> Identity {
    Identity {
        object_id: record.boid,
        module_accessor: record.module_accessor_addr,
        category: record.category,
        kind: record.kind,
        entry_id: record.entry_id,
        owner_entry: record.founder_entry_id,
        stock_generation: (record.category == 0)
            .then(|| crate::slight::agents::stock_generation(record))
            .flatten(),
    }
}
