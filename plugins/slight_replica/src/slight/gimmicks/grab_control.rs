//! Capture-link reads and commands delivered on the owning fighter's Main line.
use super::{
    actor_identity::Identity,
    damage_commands::Lease,
    grab_commands::{
        exit_owned_status, step_owned_hold, HoldSample, OwnedHold, Pending, Queue, ReleaseReason,
        Target,
    },
    native_inputs::{FighterValue, NativeInput, NativeObservation},
    phase_commands::Command,
};
use smash::{
    app::{
        lua_bind::{
            CatchModule, ControlModule, DamageModule, GrabModule, LinkModule, MotionModule,
            PostureModule, StatusModule, WorkModule,
        },
        sv_battle_object, BattleObjectModuleAccessor,
    },
    hash40,
    lib::lua_const::*,
};
use std::sync::atomic::{AtomicU64, Ordering};

static REVISION: AtomicU64 = AtomicU64::new(0);
static QUEUE: std::sync::LazyLock<parking_lot::Mutex<Queue<Identity>>> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new(Queue::default()));

fn check_owner(boid: u32, fighter: &str, identity: Identity) -> Result<(), String> {
    if Identity::from_live_fighter(boid, fighter) != Some(identity) {
        return Err("grab command fighter identity changed".into());
    }
    Ok(())
}

/// LINK_NO_CAPTURE is the grabber's child node (HDR get_grabbed_opponent_boma),
/// never a damage-contact receiver, partner, projectile owner or saved Work ID.
fn target(
    boid: u32,
    fighter: &str,
    identity: Identity,
) -> Result<Option<Target<Identity>>, String> {
    check_owner(boid, fighter, identity)?;
    let module = identity.module_accessor_addr as *mut BattleObjectModuleAccessor;
    let id = unsafe { LinkModule::get_node_object_id(module, *LINK_NO_CAPTURE) };
    check_owner(boid, fighter, identity)?;
    let Ok(id) = u32::try_from(id) else {
        return Ok(None);
    };
    if id == boid {
        return Ok(None);
    }
    unsafe {
        if sv_battle_object::is_null(id)
            || !sv_battle_object::is_active(id)
            || sv_battle_object::category(id) != *BATTLE_OBJECT_CATEGORY_FIGHTER
        {
            return Ok(None);
        }
        let Some(name) = super::registry::fighter_name(sv_battle_object::kind(id)) else {
            return Ok(None);
        };
        let Some(target_identity) = Identity::from_live_fighter(id, name) else {
            return Ok(None);
        };
        // Re-read the exact child link after the target's identity proof.
        check_owner(boid, fighter, identity)?;
        if LinkModule::get_node_object_id(module, *LINK_NO_CAPTURE) != u64::from(id) {
            return Err("capture link changed while resolving held opponent".into());
        }
        let target_module = target_identity.module_accessor_addr as *mut BattleObjectModuleAccessor;
        if LinkModule::get_parent_object_id(target_module, *LINK_NO_CAPTURE) != u64::from(boid) {
            return Ok(None);
        }
        Ok(Some(Target {
            boid: id,
            identity: target_identity,
        }))
    }
}

pub(crate) fn sample(
    boid: u32,
    fighter: &str,
    identity: Identity,
    source: &NativeInput,
) -> Result<NativeObservation, String> {
    if !source.valid_for(fighter)
        || !matches!(
            source,
            NativeInput::Fighter {
                value: FighterValue::HoldingOpponent,
                ..
            } | NativeInput::Fighter {
                value: FighterValue::HeldOpponentDamage,
                ..
            } | NativeInput::Fighter {
                value: FighterValue::HeldOpponentWeight,
                ..
            } | NativeInput::Fighter {
                value: FighterValue::HeldOpponentEscapeTime,
                ..
            }
        )
    {
        return Err("unsupported or foreign held opponent source".into());
    }
    let held = target(boid, fighter, identity)?;
    if matches!(
        source,
        NativeInput::Fighter {
            value: FighterValue::HoldingOpponent,
            ..
        }
    ) {
        return Ok(NativeObservation::Flag(held.is_some()));
    }
    // Absent target has a defined neutral numeric observation. HoldingOpponent
    // distinguishes absence from an opponent at zero percent.
    let Some(held) = held else {
        return Ok(NativeObservation::Float(0.0));
    };
    let module = held.identity.module_accessor_addr as *mut BattleObjectModuleAccessor;
    let value = unsafe {
        match source {
            NativeInput::Fighter {
                value: FighterValue::HeldOpponentDamage,
                ..
            } => DamageModule::damage(module, 0),
            NativeInput::Fighter {
                value: FighterValue::HeldOpponentWeight,
                ..
            } => WorkModule::get_param_float(module, hash40("weight"), 0),
            NativeInput::Fighter {
                value: FighterValue::HeldOpponentEscapeTime,
                ..
            } => ControlModule::get_clatter_time(module, 0).max(0.0),
            _ => return Err("unsupported held opponent read".into()),
        }
    };
    if target(boid, fighter, identity)? != Some(held) {
        return Err("held opponent identity changed while sampling".into());
    }
    if !value.is_finite() || !(0.0..=1_000_000.0).contains(&value) {
        return Err("invalid held opponent numeric value".into());
    }
    Ok(NativeObservation::Float(value))
}

pub(crate) fn preflight(
    boid: u32,
    fighter: &str,
    identity: Identity,
    count: usize,
) -> Result<(), String> {
    check_owner(boid, fighter, identity)?;
    QUEUE
        .lock()
        .preflight(identity.entry_id, Target { boid, identity }, count)
}
fn lease(boid: u32, fighter: &str, identity: Identity) -> Result<Lease, String> {
    check_owner(boid, fighter, identity)?;
    let status = unsafe {
        StatusModule::status_kind(identity.module_accessor_addr as *mut BattleObjectModuleAccessor)
    };
    check_owner(boid, fighter, identity)?;
    Ok(Lease {
        status,
        revision: REVISION.load(Ordering::SeqCst),
    })
}
pub(crate) fn enqueue(
    boid: u32,
    fighter: &str,
    identity: Identity,
    command: Command,
) -> Result<(), String> {
    preflight(boid, fighter, identity, 1)?;
    let Some(target) = target(boid, fighter, identity)? else {
        crate::slight::diag::note(format!(
            "grab command skipped: actor={boid} no held opponent"
        ));
        return Ok(());
    };
    let pending = Pending {
        owner: Target { boid, identity },
        target,
        lease: lease(boid, fighter, identity)?,
        command,
    };
    QUEUE.lock().enqueue(identity.entry_id, pending)
}
pub(crate) fn configuration_changed() {
    let mut captures = CAPTURES.lock();
    REVISION.fetch_add(1, Ordering::SeqCst);
    captures.clear();
    HOLDS.lock().clear();
}
pub(crate) fn retire(boid: u32) {
    QUEUE.lock().take(boid);
    CAPTURES.lock().remove(&boid);
    HOLDS.lock().remove(&boid);
}

unsafe fn module_of(identity: Identity) -> *mut BattleObjectModuleAccessor {
    identity.module_accessor_addr as *mut BattleObjectModuleAccessor
}
fn describe(target: Option<Target<Identity>>) -> String {
    target.map_or("none clatter=none".into(), |t| {
        format!("{} clatter={}", t.boid, unsafe {
            ControlModule::get_clatter_time(module_of(t.identity), 0)
        })
    })
}
/// Watch the released target until its capture status has had time to end.
fn watch_release(boid: u32, target: Option<Target<Identity>>) {
    if let Some(target) = target {
        RELEASED
            .lock()
            .insert(target.boid, (target.identity, boid, 2));
    }
}
/// Koopa's vanilla ejection (HDR koopa special_s_ejected): tell the target, then cut.
unsafe fn safety_cut(
    boid: u32,
    identity: Identity,
    target: Option<Target<Identity>>,
    reason: &str,
) {
    let module = module_of(identity);
    crate::slight::diag::note(format!(
        "capture release: owner={boid} target={} reason={reason} status={}",
        describe(target),
        StatusModule::status_kind(module)
    ));
    CatchModule::set_send_cut_event(module, true);
    CatchModule::catch_cut(module, false, false);
    watch_release(boid, target);
}
/// Statuses vanilla keeps a capture in. They end by themselves, except when a throw has no motion.
unsafe fn catch_family() -> [i32; 5] {
    [
        *FIGHTER_STATUS_KIND_CATCH_PULL,
        *FIGHTER_STATUS_KIND_CATCH_DASH_PULL,
        *FIGHTER_STATUS_KIND_CATCH_WAIT,
        *FIGHTER_STATUS_KIND_CATCH_ATTACK,
        *FIGHTER_STATUS_KIND_THROW,
    ]
}
unsafe fn capture_family() -> [i32; 4] {
    [
        *FIGHTER_STATUS_KIND_CAPTURE_PULLED,
        *FIGHTER_STATUS_KIND_CAPTURE_WAIT,
        *FIGHTER_STATUS_KIND_CAPTURE_DAMAGE,
        *FIGHTER_STATUS_KIND_THROWN,
    ]
}
/// Vanilla status_end_CatchWait and status_end_CatchPull turn LAST_STRANS into the motion
/// status_Throw plays. An owned status replaces them, and without this the throw plays no
/// motion: its ATK_HIT_ABS never runs and the status never ends.
unsafe fn select_throw_motion(module: *mut BattleObjectModuleAccessor) {
    let selected = WorkModule::get_int(module, *FIGHTER_STATUS_CATCH_WAIT_WORK_INT_LAST_STRANS);
    let motion = if selected == *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_THROW_B {
        "throw_b"
    } else if selected == *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_THROW_HI {
        "throw_hi"
    } else if selected == *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_THROW_LW {
        "throw_lw"
    } else {
        "throw_f"
    };
    WorkModule::set_int64(
        module,
        hash40(motion) as i64,
        *FIGHTER_STATUS_CATCH_WAIT_WORK_INT_MOTION_KIND,
    );
    crate::slight::diag::note(format!(
        "capture throw: motion selected={motion} last_strans={selected:#x}"
    ));
}
/// An owned status replaces the vanilla End and Exit, so this runs for every exit:
/// hit, stock loss, chart transition or recovery.
pub(crate) unsafe fn owned_status_exit(boid: u32, next: i32, next_owned: bool) {
    let Some((fighter, identity)) = owner(boid) else {
        return;
    };
    let module = module_of(identity);
    GrabModule::clear_all(module);
    GrabModule::set_rebound(module, false);
    if next == *FIGHTER_STATUS_KIND_THROW {
        select_throw_motion(module);
    }
    let held = target(boid, fighter, identity).ok().flatten();
    if held.is_none() && !CatchModule::is_catch(module) {
        return;
    }
    let previous = HOLDS.lock().get(&boid).copied();
    let (row, keep) = exit_owned_status(
        previous,
        held,
        DamageModule::damage(module, 0),
        next,
        next_owned,
        &catch_family(),
    );
    if let Some(row) = row {
        HOLDS.lock().insert(boid, row);
    }
    if keep {
        crate::slight::diag::note(format!(
            "capture kept: owner={boid} target={} next={next}",
            describe(held)
        ));
    } else {
        safety_cut(boid, identity, held, &format!("status_exit next={next}"));
    }
}
fn owner(boid: u32) -> Option<(&'static str, Identity)> {
    let record = crate::slight::agents::lookup(boid)?;
    let fighter = super::registry::fighter_name(record.kind)?;
    Some((fighter, Identity::from_live_fighter(boid, fighter)?))
}
/// The target's status, or None once that object is no longer this fighter.
unsafe fn live_status(target: Target<Identity>) -> Option<i32> {
    let name = super::registry::fighter_name(target.identity.fighter_kind)?;
    (Identity::from_live_fighter(target.boid, name) == Some(target.identity))
        .then(|| StatusModule::status_kind(module_of(target.identity)))
}
/// Last resort, for whatever status the owner is stuck in: cut the link, walk the owner
/// out of a catch status, and leave the target to finish_release on its own line.
unsafe fn rescue(boid: u32, identity: Identity, target: Target<Identity>) {
    let module = module_of(identity);
    let status = StatusModule::status_kind(module);
    let target = live_status(target).map(|_| target);
    safety_cut(boid, identity, target, ReleaseReason::Timeout.name());
    if catch_family().contains(&status) {
        let out = if StatusModule::situation_kind(module) == *SITUATION_KIND_AIR {
            *FIGHTER_STATUS_KIND_FALL
        } else {
            *FIGHTER_STATUS_KIND_WAIT
        };
        crate::slight::diag::note(format!(
            "capture rescue: owner={boid} status={status} forced_to={out}"
        ));
        StatusModule::change_status_force(module, out, true);
    }
}
/// Per-frame guard, from the first capture seen in a chart-owned status until both are free.
unsafe fn watch_owned_hold(boid: u32, identity: Identity, held: Option<Target<Identity>>) {
    let module = module_of(identity);
    let status = StatusModule::status_kind(module);
    let owned = super::live_behavior::owns_status(boid, status);
    let previous = HOLDS.lock().get(&boid).copied();
    if !owned && previous.is_none() {
        return;
    }
    let tracked = held.or(previous.map(|row| row.target));
    let target_status = tracked.and_then(|t| live_status(t));
    let alive = target_status.is_some_and(|s| {
        ![
            *FIGHTER_STATUS_KIND_DEAD,
            *FIGHTER_STATUS_KIND_STANDBY,
            *FIGHTER_STATUS_KIND_REBIRTH,
        ]
        .contains(&s)
    });
    let engaged = held.is_some()
        || catch_family().contains(&status)
        || target_status.is_some_and(|s| capture_family().contains(&s));
    let (next, reason) = step_owned_hold(
        previous,
        HoldSample {
            owned,
            held,
            owner_damage: DamageModule::damage(module, 0),
            target_alive: alive,
            engaged,
        },
    );
    match next {
        Some(row) => {
            HOLDS.lock().insert(boid, row);
            let tick = if row.owned {
                row.frames % 60 == 59
            } else {
                row.frames % 30 == 0
            };
            if tick && row.owned {
                crate::slight::diag::note(format!(
                    "capture carry: owner={boid} target={} frame={} owner_x={} target_x={}",
                    describe(Some(row.target)),
                    row.frames + 1,
                    PostureModule::pos_x(module),
                    PostureModule::pos_x(module_of(row.target.identity))
                ));
            } else if tick {
                crate::slight::diag::note(format!("capture watch: owner={boid} status={status} motion={:#x} motion_frame={} rate={} selected_motion={:#x} target_status={target_status:?} frames={}", MotionModule::motion_kind(module), MotionModule::frame(module), MotionModule::rate(module), WorkModule::get_int64(module, *FIGHTER_STATUS_CATCH_WAIT_WORK_INT_MOTION_KIND), row.frames));
            }
        }
        None => {
            HOLDS.lock().remove(&boid);
        }
    }
    match reason {
        Some(ReleaseReason::Timeout) if !owned => {
            if let Some(row) = next {
                rescue(boid, identity, row.target);
            }
        }
        Some(reason) if held.is_some() || CatchModule::is_catch(module) => {
            safety_cut(boid, identity, held, reason.name())
        }
        _ => {}
    }
}
/// HDR pickel trolley forces this status when it ends a capture itself. Only a target
/// this file released is forced, and only if nothing else has captured it since.
fn finish_release(boid: u32, identity: Identity) {
    let released = {
        let mut rows = RELEASED.lock();
        let Some(row) = rows.get_mut(&boid) else {
            return;
        };
        if row.2 > 0 && row.0 == identity {
            row.2 -= 1;
            return;
        }
        let row = *row;
        rows.remove(&boid);
        row
    };
    if released.0 != identity {
        return;
    }
    unsafe {
        let module = module_of(identity);
        let status = StatusModule::status_kind(module);
        if !capture_family().contains(&status) {
            return;
        }
        let parent = u32::try_from(LinkModule::get_parent_object_id(module, *LINK_NO_CAPTURE))
            .unwrap_or(u32::MAX);
        if parent == released.1
            || sv_battle_object::is_null(parent)
            || !sv_battle_object::is_active(parent)
        {
            crate::slight::diag::note(format!(
                "capture forced cut: target={boid} status={status} parent={parent:#x}"
            ));
            StatusModule::change_status_force(module, *FIGHTER_STATUS_KIND_CAPTURE_CUT, true);
        }
    }
}

fn deliver(pending: Pending<Identity>) -> Result<(), String> {
    let boid = pending.owner.boid;
    let identity = pending.owner.identity;
    let fighter = super::registry::fighter_name(identity.fighter_kind).ok_or("unknown grabber")?;
    let check = || -> Result<(), String> {
        pending.check(
            pending.owner,
            target(boid, fighter, identity)?,
            lease(boid, fighter, identity)?,
        )
    };
    check()?;
    unsafe {
        match pending.command {
            Command::HeldDamage(amount) => {
                let module =
                    pending.target.identity.module_accessor_addr as *mut BattleObjectModuleAccessor;
                let current = DamageModule::damage(module, 0);
                let delta = super::damage_commands::damage_delta(current, amount)?;
                check()?;
                DamageModule::add_damage(module, delta, 0);
            }
            Command::ThrowHeldOpponent(direction) => {
                let module = identity.module_accessor_addr as *mut BattleObjectModuleAccessor;
                // Catch wait owns the native throw-selection slot. A chart-owned status
                // replaced the code that enables the terms, so they are not checked there.
                let status = StatusModule::status_kind(module);
                let owned = super::live_behavior::owns_status(boid, status);
                if (status != *FIGHTER_STATUS_KIND_CATCH_WAIT && !owned)
                    || StatusModule::situation_kind(module) != *SITUATION_KIND_GROUND
                {
                    return Err(
                        "throw requires grounded catch wait or a grounded chart-owned status"
                            .into(),
                    );
                }
                use super::native_inputs::ThrowDirection;
                let term = match direction {
                    ThrowDirection::Forward => *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_THROW_F,
                    ThrowDirection::Back => *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_THROW_B,
                    ThrowDirection::Up => *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_THROW_HI,
                    ThrowDirection::Down => *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_THROW_LW,
                };
                if !owned && !WorkModule::is_enable_transition_term(module, term) {
                    return Err("native throw transition is disabled".into());
                }
                check()?;
                crate::slight::diag::note(format!("capture throw: request owner={boid} target={} direction={direction:?} status={status} owned={owned} motion={:#x}", describe(Some(pending.target)), MotionModule::motion_kind(module)));
                WorkModule::set_int(
                    module,
                    term,
                    *FIGHTER_STATUS_CATCH_WAIT_WORK_INT_LAST_STRANS,
                );
                StatusModule::change_status_request_from_script(
                    module,
                    *FIGHTER_STATUS_KIND_THROW,
                    false,
                );
            }
            Command::ReleaseHeldOpponent => {
                check()?;
                retire(boid);
                crate::slight::diag::note(format!(
                    "capture release: owner={boid} target={} reason=action",
                    describe(Some(pending.target))
                ));
                CatchModule::catch_cut(
                    identity.module_accessor_addr as *mut BattleObjectModuleAccessor,
                    false,
                    false,
                );
                watch_release(boid, Some(pending.target));
            }
            _ => return Err("unsupported grab command".into()),
        }
    }
    Ok(())
}
pub(crate) fn flush(boid: u32) {
    // Drop the queue lock before calling native code, which can reenter.
    let commands = QUEUE.lock().take(boid);
    for pending in commands {
        if let Err(reason) = deliver(pending) {
            crate::slight::diag::note(format!(
                "grab command skipped: actor={boid} target={} {reason}",
                pending.target.boid
            ));
        }
    }
}

// Key by object, since partner fighters can share an entry. Lifecycle resets retire rows.
type CaptureRow = (
    Identity,
    super::native_inputs::CaptureState,
    Option<Target<Identity>>,
);
type Rows<T> = std::sync::LazyLock<parking_lot::Mutex<std::collections::BTreeMap<u32, T>>>;
static CAPTURES: Rows<CaptureRow> = std::sync::LazyLock::new(Default::default);
static HOLDS: Rows<OwnedHold<Identity>> = std::sync::LazyLock::new(Default::default);
/// Target object -> (target, the owner that released it, frames left to wait).
static RELEASED: Rows<(Identity, u32, u8)> = std::sync::LazyLock::new(Default::default);

/// Sample on the owning Main boundary, before queued actions can cut the link.
pub(crate) fn observe(boid: u32) {
    if !super::is_installed() {
        return;
    }
    let Some(record) = crate::slight::agents::lookup(boid) else {
        return;
    };
    let Some(fighter) = super::registry::fighter_name(record.kind) else {
        return;
    };
    let Some(identity) = Identity::from_live_fighter(boid, fighter) else {
        return;
    };
    let Ok(0..=7) = usize::try_from(identity.entry_id) else {
        return;
    };
    finish_release(boid, identity);
    let revision = REVISION.load(Ordering::SeqCst);
    let sample = || -> Result<_, String> {
        let module = identity.module_accessor_addr as *mut BattleObjectModuleAccessor;
        let held = target(boid, fighter, identity)?;
        let parent = unsafe { LinkModule::get_parent_object_id(module, *LINK_NO_CAPTURE) };
        let captured = u32::try_from(parent)
            .ok()
            .filter(|id| *id != boid)
            .and_then(|id| {
                let record = crate::slight::agents::lookup(id)?;
                let name = super::registry::fighter_name(record.kind)?;
                let owner = Identity::from_live_fighter(id, name)?;
                let held = target(id, name, owner).ok().flatten()?;
                Some(held == Target { boid, identity })
            })
            .unwrap_or(false);
        let throwing = unsafe { StatusModule::status_kind(module) == *FIGHTER_STATUS_KIND_THROW };
        check_owner(boid, fighter, identity)?;
        Ok((
            super::native_inputs::CaptureState {
                holding: held.is_some(),
                captured,
                throwing,
            },
            held,
        ))
    };
    let Ok((next, held)) = sample() else {
        CAPTURES.lock().remove(&boid);
        return;
    };
    unsafe { watch_owned_hold(boid, identity, held) };
    let (mut edges, released) = {
        let mut rows = CAPTURES.lock();
        if REVISION.load(Ordering::SeqCst) != revision {
            return;
        }
        if rows.len() >= 128 && !rows.contains_key(&boid) {
            return;
        }
        let previous = rows.insert(boid, (identity, next, held));
        previous
            .filter(|(actor, _, _)| *actor == identity)
            .map(|(_, previous, target)| (previous.edges(next), target))
            .unwrap_or(([false; 3], None))
    };
    let status = unsafe { StatusModule::status_kind(module_of(identity)) };
    if edges[0] {
        crate::slight::diag::note(format!(
            "capture attach: owner={boid} target={} status={status}",
            describe(held)
        ));
    } else if released.is_some() && held.is_none() {
        crate::slight::diag::note(format!("capture ended: owner={boid} status={status}"));
    }
    // A dead/recycled opponent is not evidence of a throw release.
    if edges[1] {
        edges[1] = released.is_some_and(|held| {
            super::registry::fighter_name(held.identity.fighter_kind).is_some_and(|name| {
                Identity::from_live_fighter(held.boid, name) == Some(held.identity)
            })
        });
    }
    for (edge, hook) in edges.into_iter().zip([
        super::dispatcher::GimmickHook::Grab,
        super::dispatcher::GimmickHook::Throw,
        super::dispatcher::GimmickHook::Grabbed,
    ]) {
        if edge
            && check_owner(boid, fighter, identity).is_ok()
            && REVISION.load(Ordering::SeqCst) == revision
        {
            super::dispatcher::fire(hook, boid);
        }
    }
}
